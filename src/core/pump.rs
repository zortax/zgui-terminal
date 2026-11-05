//! The thread that owns the emulator.
//!
//! Everything that changes the emulator happens here: bytes are parsed, commands are applied,
//! and the rows that changed are published. Keeping it off the interface thread is what lets a
//! program write as fast as it likes without the window stopping.

use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::Term;
use alacritty_terminal::event::Event;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Handler, Processor, Timeout};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;

use crate::config::{ColorPalette, TerminalConfig};
use crate::core::command::{ClearMode, ScrollRequest, TerminalCommand, direction_of};
use crate::core::listener::EventProxy;
use crate::core::prefilter::{Control, Piece, Prefilter};
use crate::core::prompt::PromptTracker;
use crate::core::update::{ClipboardReply, FrameUpdate};
use crate::core::writer::WriteRequest;
use crate::event::{Hyperlink, MouseShape};
use crate::grid::{AppCursor, GridSpan, SpanKind};
use crate::scroll::ScrollState;
use crate::search::{SearchDirection, SearchOptions, SearchOrigin, SearchState, SearchStatus};
use crate::snapshot::{
    CursorSnapshot, GridDimensions, OverlaySnapshot, RowBuilder, RowSnapshot, SearchOverlay,
};
use crate::transport::{ExitStatus, TerminalSize, TransportError, TransportMessage};

/// The rows a region covers, and the columns it covers on each.
type Rows = SmallVec<[(u16, std::ops::RangeInclusive<u16>); 8]>;

/// Which columns of each screen row `range` covers.
///
/// A block keeps its columns on every row. Anything else runs from where it starts to the end of
/// that row, whole rows after it, and up to where it ends on the last.
fn rows_of(
    range: &alacritty_terminal::selection::SelectionRange,
    term: &Term<EventProxy>,
    display_offset: usize,
) -> Rows {
    let mut rows = SmallVec::new();
    for row in 0..term.screen_lines() {
        let line = Line(row as i32 - display_offset as i32);
        if line < range.start.line || line > range.end.line {
            continue;
        }
        let start = if line == range.start.line || range.is_block {
            range.start.column.0
        } else {
            0
        };
        let end = if line == range.end.line || range.is_block {
            range.end.column.0
        } else {
            term.columns().saturating_sub(1)
        };
        if start > end {
            continue;
        }
        rows.push((row as u16, start as u16..=end as u16));
    }
    rows
}

/// The hash of a row that has never been published.
const NOTHING_SENT: u64 = u64::MAX;

/// How many lines one step of the match count reads.
const COUNT_CHUNK: usize = 2_000;

/// How many matches the count stops at.
const COUNT_LIMIT: usize = 10_000;

/// What the pump reads.
#[derive(Debug)]
pub(crate) enum PumpMessage {
    /// Output from the program.
    Bytes(Vec<u8>),
    /// A whole screen, encoded for an empty terminal of `size`.
    Snapshot {
        /// The size the bytes were encoded for.
        size: TerminalSize,
        /// The bytes that draw the screen.
        bytes: Vec<u8>,
    },
    /// Something the emulator reported.
    Term(Event),
    /// Something the view asked for.
    Command(TerminalCommand),
    /// The program ended.
    Exited(ExitStatus),
    /// The transport failed.
    Failed(TransportError),
}

impl From<TransportMessage> for PumpMessage {
    fn from(message: TransportMessage) -> Self {
        match message {
            TransportMessage::Bytes(bytes) => Self::Bytes(bytes),
            TransportMessage::Snapshot { size, bytes } => Self::Snapshot { size, bytes },
            TransportMessage::Exited(status) => Self::Exited(status),
            TransportMessage::Failed(error) => Self::Failed(error),
        }
    }
}

/// How large the emulator's grid is, as the emulator asks it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl TermSize {
    /// A grid of `columns` by `screen_lines`.
    pub fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns: columns.max(2),
            screen_lines: screen_lines.max(1),
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// What the pump keeps about a search in progress.
struct Search {
    /// What is being looked for.
    regex: RegexSearch,
    /// Where the match the view is on starts.
    focus: Option<Point>,
    /// How far the count has read.
    counted_to: Option<Point>,
    /// How many matches the count has found.
    total: usize,
    /// Whether the count has read everything.
    complete: bool,
}

/// The pump and the emulator it owns.
pub(crate) type Built = (Pump, Arc<FairMutex<Term<EventProxy>>>);

/// The thread that owns the emulator.
pub(crate) struct Pump {
    term: Arc<FairMutex<Term<EventProxy>>>,
    processor: Processor,
    prefilter: Prefilter,
    /// Where the prompt the shell edits starts, which a resize keeps out of the reflow.
    prompt: PromptTracker,
    builder: RowBuilder,
    config: TerminalConfig,
    /// Where replies to the program go.
    writer: flume::Sender<WriteRequest>,
    /// Where published frames go.
    updates: flume::Sender<FrameUpdate>,
    /// Where commands and bytes arrive.
    inbox: flume::Receiver<PumpMessage>,
    /// The other end of the inbox, for work the pump gives itself.
    inbox_sender: flume::Sender<PumpMessage>,
    /// A hash of every row published, by where it is on screen, so an unchanged row is published
    /// once.
    sent: Vec<u64>,
    /// How far back the screen was when the rows were last published.
    published_offset: usize,
    /// How wide it was.
    published_columns: usize,
    /// What was drawn over the rows last time.
    overlay: OverlaySnapshot,
    /// Whether the view drawing now has heard of `overlay`. A view that attaches has not.
    overlay_sent: bool,
    /// The size the program was last told about.
    size: TerminalSize,
    /// A size the view asked for that has not rested long enough to apply, and when it applies.
    pending_size: Option<(TerminalSize, Instant)>,
    /// Whether the grid has taken a size the view asked for.
    sized: bool,
    /// The search in progress.
    search: Option<Search>,
    /// What the view knows about the search.
    search_state: SearchState,
    /// Whether the terminal has focus.
    focused: bool,
    /// What the pointer last looked like.
    mouse_shape: MouseShape,
    /// Whether this batch changed anything the view draws.
    dirty: bool,
    /// Whether this batch has to rebuild every row.
    rebuild: bool,
    /// The link the pointer is over.
    hovered_link: Option<crate::snapshot::HyperlinkId>,
    /// The cursor an application draws in place of the program's, when it draws one.
    app_cursor: Option<AppCursor>,
    /// What an application has lit, which nothing but the application puts out.
    app_flash: Vec<GridSpan>,
    /// Whether the program has ended. The grid stays readable after the end.
    ended: bool,
    /// Whether the program asked to hear when the colour scheme changes (private mode 2031).
    scheme_reports: bool,
}

impl Pump {
    /// Builds the pump and the emulator it owns.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: TerminalConfig,
        size: TerminalSize,
        proxy: EventProxy,
        writer: flume::Sender<WriteRequest>,
        updates: flume::Sender<FrameUpdate>,
        inbox: flume::Receiver<PumpMessage>,
        inbox_sender: flume::Sender<PumpMessage>,
    ) -> Built {
        let dimensions = TermSize::new(usize::from(size.columns), usize::from(size.lines));
        let term = Term::new(config.term_config(), &dimensions, proxy);
        let term = Arc::new(FairMutex::new(term));
        let builder = RowBuilder::new(config.colors);
        let pump = Self {
            term: Arc::clone(&term),
            processor: Processor::new(),
            prefilter: Prefilter::new(),
            prompt: PromptTracker::default(),
            builder,
            config,
            writer,
            updates,
            inbox,
            inbox_sender,
            sent: vec![NOTHING_SENT; dimensions.screen_lines],
            published_offset: 0,
            published_columns: dimensions.columns,
            overlay: OverlaySnapshot::default(),
            overlay_sent: false,
            size,
            pending_size: None,
            sized: false,
            search: None,
            search_state: SearchState::default(),
            focused: false,
            mouse_shape: MouseShape::default(),
            dirty: true,
            rebuild: true,
            hovered_link: None,
            app_cursor: None,
            app_flash: Vec::new(),
            ended: false,
            scheme_reports: false,
        };
        (pump, term)
    }

    /// Reads and publishes until the session goes away.
    ///
    /// The end of the program stops the reading. The pump carries on after it, so a view can
    /// still draw, scroll, select and copy what the program left.
    ///
    /// The first message of a batch is applied at once, which is what keeps a keystroke's echo
    /// quick. Everything that arrives inside the window after it joins the same frame, which is
    /// what keeps a flood from asking for a frame per read.
    ///
    /// A size waiting to rest bounds the wait, and applies when nothing newer came before it. A
    /// synchronized update the program did not end bounds it too, and applies when it expires.
    pub fn run(mut self) {
        loop {
            let size_due = self.pending_size.map(|(_, due)| due);
            let wake = match (size_due, self.sync_deadline()) {
                (Some(size), Some(sync)) => Some(size.min(sync)),
                (size, sync) => size.or(sync),
            };
            let first = match wake {
                Some(due) => match self.inbox.recv_deadline(due) {
                    Ok(message) => message,
                    Err(flume::RecvTimeoutError::Timeout) => {
                        self.end_expired_sync();
                        if size_due.is_some_and(|due| due <= Instant::now()) {
                            self.apply_pending_size();
                        }
                        self.publish();
                        continue;
                    }
                    Err(flume::RecvTimeoutError::Disconnected) => return,
                },
                None => match self.inbox.recv() {
                    Ok(message) => message,
                    Err(_) => return,
                },
            };
            if !self.apply(first) {
                return;
            }

            let deadline = Instant::now() + self.config.coalesce_window;
            let mut applied = 1;
            while applied < self.config.coalesce_limit {
                match self.inbox.recv_deadline(deadline) {
                    Ok(message) => {
                        if !self.apply(message) {
                            self.publish();
                            return;
                        }
                        applied += 1;
                    }
                    Err(_) => break,
                }
            }
            if self
                .pending_size
                .is_some_and(|(_, due)| due <= Instant::now())
            {
                self.apply_pending_size();
            }
            self.end_expired_sync();
            self.publish();
        }
    }

    /// When the synchronized update the program started expires.
    fn sync_deadline(&self) -> Option<Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    /// Applies a synchronized update whose time ran out, as though the program had ended it.
    fn end_expired_sync(&mut self) {
        if self
            .sync_deadline()
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            self.processor.stop_sync(&mut *self.term.lock());
            self.dirty = true;
        }
    }

    /// Applies one message. Answers whether the pump should keep going.
    fn apply(&mut self, message: PumpMessage) -> bool {
        match message {
            PumpMessage::Bytes(bytes) => {
                self.end_expired_sync();
                self.parse(&bytes);
                true
            }
            PumpMessage::Snapshot { size, bytes } => {
                self.take_snapshot(size, &bytes);
                true
            }
            PumpMessage::Term(event) => self.on_term_event(event),
            PumpMessage::Command(command) => self.on_command(command),
            PumpMessage::Exited(status) => {
                self.end(status);
                true
            }
            PumpMessage::Failed(error) => {
                let _ = self.updates.send(FrameUpdate::Failed(error));
                false
            }
        }
    }

    /// Parses output from the program.
    fn parse(&mut self, bytes: &[u8]) {
        let term = Arc::clone(&self.term);
        self.parse_into(&mut term.lock(), bytes);
    }

    /// Parses output from the program into `term`, which the caller holds locked.
    fn parse_into(&mut self, term: &mut Term<EventProxy>, bytes: &[u8]) {
        let processor = &mut self.processor;
        let prompt = &mut self.prompt;
        let mut scheme_reports = self.scheme_reports;
        let mut scheme_asked = false;
        self.prefilter.filter(bytes, &mut |piece| match piece {
            Piece::Bytes(chunk) => processor.advance(&mut *term, chunk),
            Piece::Mark(mark) => prompt.mark(mark, term),
            Piece::Control(Control::SchemeReports(on)) => scheme_reports = on,
            Piece::Control(Control::SchemeQuery) => scheme_asked = true,
            Piece::Control(Control::FullReset) => {
                // A reset ends every mode, a synchronized update too. The update held
                // the reset back, so it applies now, with everything before it.
                if processor.sync_timeout().pending_timeout() {
                    processor.stop_sync(&mut *term);
                }
                prompt.forget();
                scheme_reports = false;
            }
        });
        self.scheme_reports = scheme_reports;
        if scheme_asked {
            self.report_scheme();
        }
        self.dirty = true;
    }

    /// Replaces everything the emulator holds with a whole screen.
    ///
    /// The emulator starts over from an empty state: no history, no modes, no half read
    /// sequence and no synchronized update. It takes `size`, which is what the bytes place their
    /// rows and cursor for, parses them, and then returns to the size of the grid. That last step
    /// reflows the screen as a resize does, the prompt kept out of it. The emulator stays locked
    /// through all of it, and the batch publishes only the result.
    ///
    /// Once the grid took a size from the view, the transport hears that size again when `size`
    /// differs from it, so the program behind it takes the grid's size and draws for it.
    fn take_snapshot(&mut self, size: TerminalSize, bytes: &[u8]) {
        self.processor = Processor::new();
        self.prefilter = Prefilter::new();
        self.prompt.forget();
        self.scheme_reports = false;
        let grid = TermSize::new(usize::from(self.size.columns), usize::from(self.size.lines));
        let encoded = TermSize::new(usize::from(size.columns), usize::from(size.lines));
        let differs = grid.columns != encoded.columns || grid.screen_lines != encoded.screen_lines;
        let term = Arc::clone(&self.term);
        let mut locked = term.lock();
        locked.reset_state();
        locked.resize(encoded);
        self.parse_into(&mut locked, bytes);
        if differs {
            self.prompt.resize(&mut *locked, grid);
        }
        drop(locked);
        if differs && self.sized {
            let _ = self.writer.send(WriteRequest::Resize(self.size));
        }
        self.restart_search();
        self.restart_rows();
    }

    /// Counts the matches of the search in progress again, after the grid changed under it.
    fn restart_search(&mut self) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.focus = None;
        search.counted_to = None;
        search.total = 0;
        search.complete = false;
        self.search_state.current = None;
        self.search_state.wrapped = false;
        self.count_search();
    }

    /// Answers one of the emulator's own events.
    fn on_term_event(&mut self, event: Event) -> bool {
        match event {
            // The emulator answering a question the program asked. Dropping these leaves a
            // program that asked waiting for an answer that never comes.
            Event::PtyWrite(text) => {
                self.write(text.into_bytes());
            }
            Event::ColorRequest(index, format) => {
                // A colour the program set answers for itself. Every other one answers with the
                // colour the grid is drawn in, which is the styled palette once a view read it.
                let color = self.term.lock().colors()[index].unwrap_or_else(|| {
                    let fallback = self.config.colors.at(index);
                    alacritty_terminal::vte::ansi::Rgb {
                        r: fallback.r,
                        g: fallback.g,
                        b: fallback.b,
                    }
                });
                self.write(format(color).into_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                self.write(format(self.size.into()).into_bytes());
            }
            Event::ClipboardStore(kind, text) => {
                let _ = self
                    .updates
                    .send(FrameUpdate::ClipboardStore(clipboard_kind(kind), text));
            }
            Event::ClipboardLoad(kind, format) => {
                let reply = ClipboardReply::new(Some(format), self.writer.clone());
                let _ = self.updates.send(FrameUpdate::ClipboardLoad {
                    kind: clipboard_kind(kind),
                    reply,
                });
            }
            Event::Title(title) => {
                let _ = self.updates.send(FrameUpdate::Title(Some(title)));
            }
            Event::ResetTitle => {
                let _ = self.updates.send(FrameUpdate::Title(None));
            }
            Event::Bell => {
                let _ = self.updates.send(FrameUpdate::Bell);
            }
            Event::CursorBlinkingChange => {
                let blinking = self.term.lock().cursor_style().blinking;
                let _ = self.updates.send(FrameUpdate::CursorBlink(blinking));
            }
            Event::MouseCursorDirty => {
                self.update_mouse_shape();
            }
            Event::Wakeup => self.dirty = true,
            Event::Exit => self.end(ExitStatus::default()),
            Event::ChildExit(status) => self.end(how_it_ended(status)),
        }
        true
    }

    /// Applies one of the view's commands. Answers whether the pump should keep going.
    fn on_command(&mut self, command: TerminalCommand) -> bool {
        match command {
            TerminalCommand::Shutdown => return false,
            TerminalCommand::Redraw => self.restart_rows(),
            TerminalCommand::Write(bytes) => {
                self.write(bytes);
            }
            TerminalCommand::Resize(size) => self.request_resize(size),
            TerminalCommand::Scroll(request) => self.scroll(request),
            TerminalCommand::ScrollTo(offset) => {
                let mut term = self.term.lock();
                let current = term.grid().display_offset() as i32;
                term.scroll_display(Scroll::Delta(offset as i32 - current));
                drop(term);
                self.dirty = true;
            }
            TerminalCommand::SetFocus(focused) => {
                self.focused = focused;
                let mut term = self.term.lock();
                term.is_focused = focused;
                if term.mode().contains(TermMode::FOCUS_IN_OUT) {
                    let report = if focused {
                        b"\x1b[I".as_ref()
                    } else {
                        b"\x1b[O".as_ref()
                    };
                    self.write(report.to_vec());
                }
                drop(term);
                self.dirty = true;
            }
            TerminalCommand::SetCursor(cursor) => {
                self.app_cursor = cursor;
                self.dirty = true;
            }
            TerminalCommand::Reveal(line) => {
                self.reveal(line);
            }
            TerminalCommand::SelectionStart { point, side, kind } => {
                self.term.lock().selection = Some(Selection::new(kind, point, side));
                self.dirty = true;
            }
            TerminalCommand::SelectionUpdate { point, side } => {
                let mut term = self.term.lock();
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
                drop(term);
                self.dirty = true;
            }
            TerminalCommand::Select(span) => {
                self.select(span);
            }
            TerminalCommand::Flash(spans) => {
                self.app_flash = spans;
                self.dirty = true;
            }
            TerminalCommand::SelectionClear => {
                self.term.lock().selection = None;
                self.dirty = true;
            }
            TerminalCommand::SelectAll => {
                let mut term = self.term.lock();
                let start = Point::new(term.topmost_line(), Column(0));
                let end = Point::new(term.bottommost_line(), term.last_column());
                let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
                selection.update(end, Side::Right);
                term.selection = Some(selection);
                drop(term);
                self.dirty = true;
            }
            TerminalCommand::CopySelection(kind) => {
                let text = self.term.lock().selection_to_string();
                let _ = self.updates.send(FrameUpdate::Selection(kind, text));
            }
            TerminalCommand::Paste(text) => self.paste(&text),
            TerminalCommand::Search(pattern, options) => self.start_search(pattern, options),
            TerminalCommand::SearchStep(direction) => self.step_search(direction),
            TerminalCommand::SearchCount => self.count_search(),
            TerminalCommand::SearchClear => {
                self.search = None;
                self.search_state = SearchState::default();
                let _ = self
                    .updates
                    .send(FrameUpdate::Search(self.search_state.clone()));
                self.dirty = true;
            }
            TerminalCommand::HyperlinkAt(point) => {
                let link = point.and_then(|point| {
                    let term = self.term.lock();
                    // The point was worked out from the view's own layout, which is a frame behind
                    // the grid. After a shrink it names a cell that is no longer there, and
                    // indexing one panics *this* thread — which is the thread that reads the
                    // program, so the terminal stops dead and never comes back.
                    if point.line < term.topmost_line()
                        || point.line > term.bottommost_line()
                        || point.column.0 >= term.columns()
                    {
                        return None;
                    }
                    term.grid()[point].hyperlink().map(|link| Hyperlink {
                        uri: link.uri().to_owned(),
                        id: Some(link.id().to_owned()).filter(|id| !id.is_empty()),
                    })
                });
                let hovered = link
                    .as_ref()
                    .map(|link| self.builder.hyperlink_id(&link.uri));
                if hovered != self.hovered_link {
                    self.hovered_link = hovered;
                    // The rows under the pointer draw the link underlined, so the change has to
                    // reach the view.
                    self.dirty = true;
                }
                let _ = self.updates.send(FrameUpdate::Hyperlink(link));
            }
            TerminalCommand::Clear(mode) => {
                let mut term = self.term.lock();
                match mode {
                    ClearMode::Screen => {
                        term.grid_mut().clear_viewport();
                    }
                    ClearMode::History => term.grid_mut().clear_history(),
                    ClearMode::All => {
                        term.grid_mut().clear_history();
                        term.grid_mut().clear_viewport();
                    }
                }
                drop(term);
                self.restart_rows();
            }
            TerminalCommand::Reset => {
                let dimensions =
                    TermSize::new(usize::from(self.size.columns), usize::from(self.size.lines));
                let mut term = self.term.lock();
                term.reset_state();
                drop(term);
                let _ = dimensions;
                self.scheme_reports = false;
                self.restart_rows();
            }
            TerminalCommand::SetColors(config) => {
                let before = self.config.colors;
                self.builder.set_palette(config.colors);
                self.config.colors = config.colors;
                self.scheme_changed(before);
                // Every row was built with the old colours in it.
                self.restart_rows();
            }
            TerminalCommand::SetScheme(scheme) => {
                let before = self.config.colors;
                self.config.colors = before.with_scheme(*scheme);
                self.builder.set_palette(self.config.colors);
                self.scheme_changed(before);
                self.restart_rows();
            }
            TerminalCommand::SetConfig(config) => {
                let config = *config;
                let before = self.config.colors;
                self.builder.set_palette(config.colors);
                self.term.lock().set_options(config.term_config());
                self.config = config;
                self.scheme_changed(before);
                self.restart_rows();
            }
        }
        true
    }

    /// Reports the end of the program once. A later report of the same end is dropped.
    fn end(&mut self, status: ExitStatus) {
        if !self.ended {
            self.ended = true;
            let _ = self.updates.send(FrameUpdate::Exited(status));
        }
    }

    /// Tells a program that asked for it that the colours changed from `before`.
    ///
    /// The new colours are in place first, so a query that follows the report reads them.
    fn scheme_changed(&self, before: ColorPalette) {
        let now = self.config.colors;
        if self.scheme_reports
            && (now.background != before.background || now.foreground != before.foreground)
        {
            self.report_scheme();
        }
    }

    /// Tells the program whether the colour scheme is dark or light.
    fn report_scheme(&self) {
        let report = if self.config.colors.is_dark() {
            b"\x1b[?997;1n".as_ref()
        } else {
            b"\x1b[?997;2n".as_ref()
        };
        self.write(report.to_vec());
    }

    /// Sends `bytes` to the program.
    fn write(&self, bytes: Vec<u8>) {
        let _ = self.writer.send(WriteRequest::Bytes(bytes));
    }

    /// Sends `text` to the program, bracketed when the program asked for that.
    fn paste(&mut self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let bytes = crate::input::paste::encode(text, bracketed);
        self.write(bytes);
        if self.config.scroll_to_bottom_on_input {
            self.scroll(ScrollRequest::Bottom);
        }
    }

    /// Takes a size the view asked for once it rests.
    ///
    /// The first size applies at once, because nothing was laid out at the size before it.
    fn request_resize(&mut self, size: TerminalSize) {
        if !self.sized || self.config.resize_settle.is_zero() {
            self.sized = true;
            self.pending_size = None;
            self.resize(size);
            return;
        }
        self.pending_size = Some((size, Instant::now() + self.config.resize_settle));
    }

    /// Applies the size that rested.
    fn apply_pending_size(&mut self) {
        if let Some((size, _)) = self.pending_size.take() {
            self.resize(size);
        }
    }

    /// Gives the grid a new size, and tells the program about it.
    ///
    /// A prompt the shell edits stays out of the reflow, so the shell draws it again over itself.
    fn resize(&mut self, size: TerminalSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        let dimensions = TermSize::new(usize::from(size.columns), usize::from(size.lines));
        self.prompt.resize(&mut *self.term.lock(), dimensions);
        let _ = self.writer.send(WriteRequest::Resize(size));
        // The rows are laid out again from scratch: a resize reflows the history, so which line
        // is which no longer follows from what was published before.
        self.restart_rows();
    }

    /// Moves the view.
    fn scroll(&mut self, request: ScrollRequest) {
        let mut term = self.term.lock();
        let screen_lines = term.screen_lines() as i32;
        let scroll = match request {
            ScrollRequest::Lines(lines) => Scroll::Delta(lines),
            ScrollRequest::Pages(pages) => Scroll::Delta(pages * screen_lines),
            ScrollRequest::Top => Scroll::Top,
            ScrollRequest::Bottom => Scroll::Bottom,
        };
        term.scroll_display(scroll);
        drop(term);
        self.dirty = true;
    }

    /// Puts `line` on screen, counted from the oldest one the history holds.
    ///
    /// A line already on screen moves nothing: what this promises is that the line can be seen,
    /// and not where on the screen it is.
    fn reveal(&mut self, line: usize) {
        let mut term = self.term.lock();
        let history = term.history_size();
        let screen_lines = term.screen_lines();
        let offset = term.grid().display_offset();
        // The screen shows `history - offset` up to that plus its own height.
        let top = history.saturating_sub(offset);
        let wanted = if line < top {
            history.saturating_sub(line)
        } else if line >= top + screen_lines {
            (history + screen_lines).saturating_sub(line + 1)
        } else {
            offset
        };
        let wanted = wanted.min(history);
        if wanted != offset {
            term.scroll_display(Scroll::Delta(wanted as i32 - offset as i32));
            self.dirty = true;
        }
        drop(term);
    }

    /// Selects `span`, as a drag would have.
    fn select(&mut self, span: GridSpan) {
        let mut term = self.term.lock();
        let selection = self.selection_of(span, &term);
        term.selection = Some(selection);
        drop(term);
        self.dirty = true;
    }

    /// The selection `span` describes.
    ///
    /// What an application means by a region of the grid, said in the emulator's own terms, so
    /// that a selection and a light are worked out the same way.
    fn selection_of(&self, span: GridSpan, term: &Term<EventProxy>) -> Selection {
        let history = term.history_size();
        let last = term.last_column();
        let kind = match span.kind {
            SpanKind::Cells => SelectionType::Simple,
            SpanKind::Lines => SelectionType::Lines,
            SpanKind::Block => SelectionType::Block,
        };
        let place = |point: crate::grid::GridPoint| {
            Point::new(
                crate::grid::to_line(point.line, history),
                Column(usize::from(point.column).min(last.0)),
            )
        };
        // Both ends are included, so the far end is taken by its right side: a selection of one
        // cell is one cell, and not none.
        let mut selection = Selection::new(kind, place(span.from), Side::Left);
        selection.update(place(span.to), Side::Right);
        selection
    }

    /// Publishes every row next time, whatever was published before.
    fn restart_rows(&mut self) {
        self.sent.clear();
        self.rebuild = true;
        self.dirty = true;
        self.overlay_sent = false;
    }

    /// What the pointer should look like now.
    fn update_mouse_shape(&mut self) {
        let mode = *self.term.lock().mode();
        let shape = if mode.intersects(TermMode::MOUSE_MODE) {
            MouseShape::Default
        } else {
            MouseShape::Text
        };
        if shape != self.mouse_shape {
            self.mouse_shape = shape;
            let _ = self.updates.send(FrameUpdate::MouseShape(shape));
        }
    }

    /// Reads what changed and publishes it.
    fn publish(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;

        let mut term = self.term.lock();
        let columns = term.columns();
        let screen_lines = term.screen_lines();
        let display_offset = term.grid().display_offset();
        let history = term.history_size();

        // Everything the view draws is named by where it is on screen, so anything that changes
        // what sits where means every row is different. Scrolling is exactly that, and so is a
        // resize and a program putting its own screen in front.
        let moved = display_offset != self.published_offset
            || screen_lines != self.sent.len()
            || columns != self.published_columns;
        if moved || self.rebuild {
            self.sent.clear();
            self.sent.resize(screen_lines, NOTHING_SENT);
        }
        let rebuild = std::mem::take(&mut self.rebuild) || moved;
        self.published_offset = display_offset;
        self.published_columns = columns;

        // Which rows to read. Damage is already reported against the screen, which is the same
        // thing the view draws, so a row of it is a row of what is shown.
        // What a program set for itself, which outranks the configuration and the style sheet.
        self.builder.set_overrides(term.colors());

        let damage = term.damage();
        let mut wanted: FxHashSet<usize> = FxHashSet::default();
        let full = match damage {
            TermDamage::Full => true,
            TermDamage::Partial(lines) => {
                for line in lines {
                    if line.line < screen_lines {
                        wanted.insert(line.line);
                    }
                }
                false
            }
        };
        term.reset_damage();
        if full || rebuild {
            wanted = (0..screen_lines).collect();
        }

        let mut rows = Vec::with_capacity(wanted.len());
        let mut ordered: Vec<usize> = wanted.into_iter().collect();
        ordered.sort_unstable();
        for row in ordered {
            // The emulator counts from the top of its screen, and the screen sits this far above
            // the newest line.
            let index = row as i32 - display_offset as i32;
            let cells = &term.grid()[Line(index)];
            let snapshot = self.builder.build_row(row as u16, cells, columns);
            let hash = row_hash(&snapshot);
            if self.sent[row] != hash {
                self.sent[row] = hash;
                rows.push(snapshot);
            }
        }

        let dimensions = GridDimensions {
            columns: columns as u16,
            screen_lines: screen_lines as u16,
            history,
            display_offset,
        };
        let scroll = ScrollState {
            display_offset,
            history,
            screen_lines,
        };
        let overlay = self.read_overlay(&term, display_offset);
        drop(term);

        if !rows.is_empty() || rebuild {
            let _ = self.updates.send(FrameUpdate::Rows {
                rows,
                dimensions,
                scroll,
                rebuild,
            });
        }
        if !self.overlay_sent || overlay != self.overlay {
            self.overlay_sent = true;
            self.overlay = overlay.clone();
            let _ = self.updates.send(FrameUpdate::Overlay(Box::new(overlay)));
        }
    }

    /// What is drawn over the rows: the state the emulator does not report as damage.
    fn read_overlay(&self, term: &Term<EventProxy>, display_offset: usize) -> OverlaySnapshot {
        let screen_lines = term.screen_lines();
        let style = term.cursor_style();
        // Where the cursor is, and what it looks like. An application drawing its own says both,
        // and its cursor never blinks: it moves when a key is pressed and at no other time.
        let (point, shape, blinking, shown) = match self.app_cursor {
            Some(cursor) => (
                Point::new(
                    crate::grid::to_line(cursor.at.line, term.history_size()),
                    Column(usize::from(cursor.at.column)),
                ),
                cursor.shape,
                false,
                true,
            ),
            None => (
                term.grid().cursor.point,
                style.shape,
                style.blinking || self.config.cursor.blinking,
                term.mode().contains(TermMode::SHOW_CURSOR),
            ),
        };
        // The cursor is on the screen the emulator writes to, which is this far down the screen
        // the view draws once the history above it is taken into account.
        let cursor_row = point.line.0 + display_offset as i32;
        let cursor = CursorSnapshot {
            row: cursor_row.clamp(0, screen_lines as i32 - 1) as u16,
            column: point.column.0 as u16,
            shape,
            blinking,
            visible: shown && cursor_row >= 0 && cursor_row < screen_lines as i32,
        };

        let selection = term
            .selection
            .as_ref()
            .and_then(|it| it.to_range(term))
            .map(|range| rows_of(&range, term, display_offset))
            .unwrap_or_default();

        let mut flash: Rows = SmallVec::new();
        for span in &self.app_flash {
            let lit = self.selection_of(*span, term);
            if let Some(range) = lit.to_range(term) {
                flash.extend(rows_of(&range, term, display_offset));
            }
        }

        OverlaySnapshot {
            cursor,
            focused: self.focused,
            selection,
            flash,
            search: self.search_overlay(term, display_offset),
            hovered_hyperlink: self.hovered_link,
            preedit: self.overlay.preedit.clone(),
        }
    }

    /// The matches that fall inside what is on screen.
    ///
    /// Only the screen is looked at, so the work is bounded by what is shown rather than by the
    /// history, however much of it there is.
    fn search_overlay(&self, term: &Term<EventProxy>, display_offset: usize) -> SearchOverlay {
        let Some(search) = self.search.as_ref() else {
            return SearchOverlay::default();
        };
        let screen_lines = term.screen_lines() as i32;
        let top = Point::new(Line(-(display_offset as i32)), Column(0));
        let bottom_line = Line(screen_lines - 1 - display_offset as i32);
        let mut regex = search.regex.clone();

        let mut matches = SmallVec::new();
        let mut focused = None;
        let mut origin = top;
        let limit = Some(term.screen_lines());
        while let Some(found) =
            term.search_next(&mut regex, origin, Direction::Right, Side::Left, limit)
        {
            let (start, end) = (*found.start(), *found.end());
            if start.line > bottom_line {
                break;
            }
            for line in start.line.0..=end.line.0 {
                let row = line + display_offset as i32;
                if row < 0 || row >= screen_lines {
                    continue;
                }
                let first = if line == start.line.0 {
                    start.column.0
                } else {
                    0
                };
                let last = if line == end.line.0 {
                    end.column.0
                } else {
                    term.columns().saturating_sub(1)
                };
                if Some(start) == search.focus {
                    focused = Some(matches.len());
                }
                matches.push((row as u16, first as u16..=last as u16));
            }
            if matches.len() > 512 {
                break;
            }
            // The next search starts after this match, or an empty match would repeat forever.
            origin = match end.add(term, Boundary::Grid, 1) {
                next if next == end => break,
                next => next,
            };
        }

        SearchOverlay { matches, focused }
    }

    /// Starts looking for `pattern`.
    fn start_search(&mut self, pattern: String, options: SearchOptions) {
        if pattern.is_empty() {
            self.search = None;
            self.search_state = SearchState::default();
            let _ = self
                .updates
                .send(FrameUpdate::Search(self.search_state.clone()));
            self.dirty = true;
            return;
        }
        match RegexSearch::new(&pattern) {
            Ok(regex) => {
                self.search = Some(Search {
                    regex,
                    focus: None,
                    counted_to: None,
                    total: 0,
                    complete: false,
                });
                self.search_state = SearchState {
                    pattern,
                    status: SearchStatus::Counting,
                    ..SearchState::default()
                };
                self.step_search_from(options.direction, options.origin);
                self.count_search();
            }
            Err(error) => {
                self.search = None;
                self.search_state = SearchState {
                    pattern,
                    status: SearchStatus::Invalid(error.to_string()),
                    ..SearchState::default()
                };
                let _ = self
                    .updates
                    .send(FrameUpdate::Search(self.search_state.clone()));
            }
        }
        self.dirty = true;
    }

    /// Moves to the next match in `direction`.
    fn step_search(&mut self, direction: SearchDirection) {
        self.step_search_from(direction, SearchOrigin::Current);
        self.dirty = true;
    }

    /// Moves to the next match in `direction`, starting from `origin`.
    fn step_search_from(&mut self, direction: SearchDirection, origin: SearchOrigin) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset();
        let screen_lines = term.screen_lines() as i32;
        let way = direction_of(direction);

        let from = match origin {
            SearchOrigin::Current => search.focus.map(|point| {
                // One cell past the current match, so the same one is not found again.
                match way {
                    Direction::Right => point.add(&*term, Boundary::Grid, 1),
                    Direction::Left => point.sub(&*term, Boundary::Grid, 1),
                }
            }),
            SearchOrigin::Point(point) => Some(point),
            SearchOrigin::Viewport => None,
        }
        .unwrap_or_else(|| match way {
            Direction::Right => Point::new(Line(-(display_offset as i32)), Column(0)),
            Direction::Left => Point::new(
                Line(screen_lines - 1 - display_offset as i32),
                term.last_column(),
            ),
        });

        let found = term.search_next(&mut search.regex, from, way, Side::Left, None);
        let (found, wrapped) = match found {
            Some(found) => (Some(found), false),
            None => {
                // Nothing ahead: start again from the far end.
                let restart = match way {
                    Direction::Right => Point::new(term.topmost_line(), Column(0)),
                    Direction::Left => Point::new(term.bottommost_line(), term.last_column()),
                };
                let found = term.search_next(&mut search.regex, restart, way, Side::Left, None);
                let wrapped = found.is_some();
                (found, wrapped)
            }
        };

        match found {
            Some(found) => {
                let start = *found.start();
                search.focus = Some(start);
                term.scroll_to_point(start);
                self.search_state.wrapped = wrapped;
                self.search_state.status = if search.complete {
                    SearchStatus::Ready
                } else {
                    SearchStatus::Counting
                };
            }
            None => {
                search.focus = None;
                self.search_state.status = SearchStatus::Ready;
                self.search_state.total = 0;
                self.search_state.current = None;
            }
        }
        drop(term);
        self.publish_search();
    }

    /// Counts the matches, one bounded chunk at a time.
    ///
    /// A long history is read across several steps, each queued behind whatever else the view
    /// asks for, so counting never holds up the output of a program.
    fn count_search(&mut self) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if search.complete {
            return;
        }
        let term = self.term.lock();
        let mut origin = search
            .counted_to
            .unwrap_or_else(|| Point::new(term.topmost_line(), Column(0)));
        let mut regex = search.regex.clone();
        let start_line = origin.line;

        loop {
            let Some(found) =
                term.search_next(&mut regex, origin, Direction::Right, Side::Left, None)
            else {
                search.complete = true;
                break;
            };
            search.total += 1;
            let end = *found.end();
            let next = end.add(&*term, Boundary::Grid, 1);
            if next == end {
                search.complete = true;
                break;
            }
            origin = next;
            let read = (origin.line - start_line).0.unsigned_abs() as usize;
            if search.total >= COUNT_LIMIT {
                search.complete = true;
                break;
            }
            if read >= COUNT_CHUNK {
                break;
            }
        }
        search.counted_to = Some(origin);
        let complete = search.complete;
        let total = search.total;
        drop(term);

        self.search_state.total = total;
        self.search_state.status = if complete {
            SearchStatus::Ready
        } else {
            SearchStatus::Counting
        };
        self.publish_search();
        if !complete {
            // The rest is read behind whatever else is waiting, so a long history never holds up
            // the output of a program.
            let _ = self
                .inbox_sender
                .send(PumpMessage::Command(TerminalCommand::SearchCount));
        }
    }

    /// Tells the view what the search knows.
    fn publish_search(&mut self) {
        let _ = self
            .updates
            .send(FrameUpdate::Search(self.search_state.clone()));
    }
}

/// How a program ended, as this crate reports it.
///
/// The signal is asked for where there are signals, which is what tells a program that was killed
/// from one that chose to stop.
fn how_it_ended(status: std::process::ExitStatus) -> ExitStatus {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal = None;
    ExitStatus {
        code: status.code(),
        signal,
    }
}

/// A hash of everything a row draws.
fn row_hash(row: &RowSnapshot) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    row.wrapped.hash(&mut hasher);
    for run in &row.runs {
        run.column.hash(&mut hasher);
        run.width.hash(&mut hasher);
        run.text.hash(&mut hasher);
        run.style.hash(&mut hasher);
    }
    hasher.finish()
}

/// Which clipboard the emulator means.
fn clipboard_kind(kind: alacritty_terminal::term::ClipboardType) -> zgui::prelude::ClipboardKind {
    match kind {
        alacritty_terminal::term::ClipboardType::Clipboard => {
            zgui::prelude::ClipboardKind::Standard
        }
        alacritty_terminal::term::ClipboardType::Selection => zgui::prelude::ClipboardKind::Primary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pump over a 20 by 4 grid, driven by hand.
    fn pump() -> Pump {
        sized(20, 4).0
    }

    /// A grid size of `columns` by `lines`.
    fn size(columns: u16, lines: u16) -> TerminalSize {
        TerminalSize {
            columns,
            lines,
            cell_width: 9,
            cell_height: 19,
        }
    }

    /// A pump over a `columns` by `lines` grid that took its size, and what it writes to the
    /// program.
    fn sized(columns: u16, lines: u16) -> (Pump, flume::Receiver<WriteRequest>) {
        let (pump, written, _) = watched(columns, lines);
        (pump, written)
    }

    /// A pump over a `columns` by `lines` grid that took its size, what it writes to the
    /// program, and what it publishes.
    fn watched(
        columns: u16,
        lines: u16,
    ) -> (
        Pump,
        flume::Receiver<WriteRequest>,
        flume::Receiver<FrameUpdate>,
    ) {
        let (inbox_sender, inbox) = flume::unbounded();
        let (writer, written) = flume::unbounded();
        let (updates, published) = flume::unbounded();
        let proxy = EventProxy::new(inbox_sender.clone());
        let (mut pump, _) = Pump::new(
            TerminalConfig::default(),
            size(columns, lines),
            proxy,
            writer,
            updates,
            inbox,
            inbox_sender,
        );
        pump.sized = true;
        (pump, written, published)
    }

    /// Where the cursor is on the screen, as row and column.
    fn cursor(pump: &Pump) -> (i32, usize) {
        let point = pump.term.lock().grid().cursor.point;
        (point.line.0, point.column.0)
    }

    /// The text of every line the history holds, oldest first.
    fn history(pump: &Pump) -> Vec<String> {
        let contents = crate::grid::read(&pump.term.lock());
        contents.lines[..contents.screen].to_vec()
    }

    /// A snapshot of a shell at 20 by 3: two lines in the history, two of output on the
    /// screen, and a marked prompt with the cursor after it on the last row.
    const SHELL: &[u8] = b"\x1b[?2026l\x1bc\
        one\r\ntwo\r\nthree\r\nfour\r\n\
        \x1b[3;1H\x1b]133;A\x07$ \x1b[3;3H";

    /// The sizes the pump told the program about.
    fn resizes(written: &flume::Receiver<WriteRequest>) -> Vec<TerminalSize> {
        written
            .try_iter()
            .filter_map(|request| match request {
                WriteRequest::Resize(size) => Some(size),
                _ => None,
            })
            .collect()
    }

    /// The text of every screen line, joined by newlines.
    fn screen(pump: &Pump) -> String {
        let term = pump.term.lock();
        (0..term.screen_lines())
            .map(|line| {
                let row = &term.grid()[Line(line as i32)];
                (0..term.columns())
                    .map(|column| row[Column(column)].c)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn feed(pump: &mut Pump, bytes: &[u8]) {
        assert!(pump.apply(PumpMessage::Bytes(bytes.to_vec())));
    }

    #[test]
    fn a_reset_ends_a_synchronized_update_the_program_left_open() {
        let mut pump = pump();
        feed(&mut pump, b"old\x1b[?2026h\r\x1b[2Kthinking");
        assert_eq!(screen(&pump).lines().next(), Some("old"));
        feed(&mut pump, b"\x1bc\x1b[3Jfresh");
        assert_eq!(screen(&pump).lines().next(), Some("fresh"));
        assert!(pump.sync_deadline().is_none());
    }

    #[test]
    fn a_synchronized_update_applies_once_it_expires() {
        let mut pump = pump();
        feed(&mut pump, b"old\x1b[?2026h\r\x1b[2Kthinking");
        assert_eq!(screen(&pump).lines().next(), Some("old"));
        let deadline = pump.sync_deadline().expect("an update in progress");
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        pump.end_expired_sync();
        assert_eq!(screen(&pump).lines().next(), Some("thinking"));
        // Output after it is drawn at once.
        feed(&mut pump, b"\r\nnext");
        assert_eq!(screen(&pump).lines().nth(1), Some("next"));
    }

    #[test]
    fn an_ended_synchronized_update_applies_at_once() {
        let mut pump = pump();
        feed(&mut pump, b"\x1b[?2026hframe\x1b[?2026l");
        assert_eq!(screen(&pump).lines().next(), Some("frame"));
    }

    #[test]
    fn a_reset_forgets_where_the_prompt_starts() {
        let mut pump = pump();
        feed(&mut pump, b"\x1b]133;A\x07$ ");
        assert!(pump.prompt.start_on_screen(&*pump.term.lock()).is_some());
        feed(&mut pump, b"\x1bc");
        assert!(pump.prompt.start_on_screen(&*pump.term.lock()).is_none());
    }

    #[test]
    fn a_snapshot_lands_as_it_would_on_a_grid_of_its_own_size_then_resized() {
        // What a grid of the snapshot's size shows after a resize to the grid's size.
        let (mut reference, _) = sized(20, 3);
        feed(&mut reference, SHELL);
        reference.resize(size(20, 6));

        let (mut pump, written) = sized(20, 6);
        feed(
            &mut pump,
            b"stale 1\r\nstale 2\r\nstale 3\r\nstale 4\r\nstale 5\r\nstale 6",
        );
        assert!(pump.apply(PumpMessage::Snapshot {
            size: size(20, 3),
            bytes: SHELL.to_vec(),
        }));

        assert_eq!(screen(&pump), screen(&reference));
        assert_eq!(history(&pump), history(&reference));
        assert_eq!(cursor(&pump), cursor(&reference));
        let (row, column) = cursor(&pump);
        let rows: Vec<String> = screen(&pump).lines().map(str::to_owned).collect();
        assert_eq!(rows[row as usize], "$", "{rows:?}");
        assert_eq!(column, 2);
        assert!(
            rows[row as usize + 1..].iter().all(String::is_empty),
            "nothing stale below the prompt: {rows:?}"
        );
        assert!(!screen(&pump).contains("stale"));
        assert_eq!(
            pump.prompt.start_on_screen(&*pump.term.lock()),
            Some(row as usize)
        );
        // The program hears the grid's size, so it draws for the grid.
        assert_eq!(resizes(&written), vec![size(20, 6)]);
    }

    #[test]
    fn a_snapshot_parsed_at_the_grid_size_alone_puts_the_prompt_too_high() {
        // The bytes place the cursor by absolute rows: on a taller grid the history comes back
        // onto the screen, and the cursor lands on the output above the prompt.
        let (mut pump, _) = sized(20, 6);
        feed(&mut pump, SHELL);
        let (row, _) = cursor(&pump);
        let rows: Vec<String> = screen(&pump).lines().map(str::to_owned).collect();
        assert_ne!(rows[row as usize], "$", "{rows:?}");
    }

    #[test]
    fn a_narrower_snapshot_reflows_into_a_wider_grid() {
        let bytes = b"\x1bc0123456789abcdefghij\r\nnext\x1b[3;5H";
        let (mut reference, _) = sized(10, 4);
        feed(&mut reference, bytes);
        reference.resize(size(30, 4));

        let (mut pump, _) = sized(30, 4);
        assert!(pump.apply(PumpMessage::Snapshot {
            size: size(10, 4),
            bytes: bytes.to_vec(),
        }));
        assert_eq!(screen(&pump), screen(&reference));
        assert_eq!(cursor(&pump), cursor(&reference));
        assert_eq!(screen(&pump).lines().next(), Some("0123456789abcdefghij"));
    }

    #[test]
    fn a_snapshot_empties_the_history_and_the_modes() {
        let (mut pump, _) = sized(20, 4);
        let old: Vec<u8> = (0..30)
            .flat_map(|n| format!("old {n}\r\n").into_bytes())
            .chain(b"\x1b[?2004h\x1b[?1000h".iter().copied())
            .collect();
        feed(&mut pump, &old);
        assert!(!history(&pump).is_empty());
        assert!(pump.term.lock().mode().contains(TermMode::BRACKETED_PASTE));

        assert!(pump.apply(PumpMessage::Snapshot {
            size: size(20, 4),
            bytes: b"fresh".to_vec(),
        }));
        assert!(history(&pump).is_empty());
        assert_eq!(screen(&pump), "fresh\n\n\n");
        let mode = *pump.term.lock().mode();
        assert!(!mode.contains(TermMode::BRACKETED_PASTE));
        assert!(!mode.intersects(TermMode::MOUSE_MODE));
    }

    #[test]
    fn a_snapshot_of_the_same_size_tells_the_program_nothing() {
        let (mut pump, written) = sized(20, 4);
        assert!(pump.apply(PumpMessage::Snapshot {
            size: size(20, 4),
            bytes: b"\x1bcsame".to_vec(),
        }));
        assert_eq!(screen(&pump).lines().next(), Some("same"));
        assert!(resizes(&written).is_empty());
    }

    #[test]
    fn a_snapshot_of_the_alternate_screen_lands_on_it() {
        let bytes = b"\x1bcshell\x1b[?1049h\x1b[H\x1b[2Jtop\x1b[3;1Hbottom row";
        let (mut reference, _) = sized(12, 3);
        feed(&mut reference, bytes);
        reference.resize(size(24, 5));

        let (mut pump, _) = sized(24, 5);
        assert!(pump.apply(PumpMessage::Snapshot {
            size: size(12, 3),
            bytes: bytes.to_vec(),
        }));
        assert!(pump.term.lock().mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(screen(&pump), screen(&reference));
        assert_eq!(cursor(&pump), cursor(&reference));
        assert_eq!(screen(&pump).lines().nth(2), Some("bottom row"));
        // Leaving the alternate screen shows the shell the snapshot drew under it.
        feed(&mut pump, b"\x1b[?1049l");
        assert_eq!(screen(&pump).lines().next(), Some("shell"));
    }

    #[test]
    fn a_snapshot_ends_a_synchronized_update_and_a_cut_sequence() {
        let (mut pump, _) = sized(20, 4);
        // The stream stops inside a frame, and inside a sequence of it.
        feed(&mut pump, b"old\x1b[?2026h\r\x1b[2Kthinking\x1b[3");
        assert!(pump.sync_deadline().is_some());

        assert!(pump.apply(PumpMessage::Snapshot {
            size: size(10, 2),
            bytes: b"\x1bcfresh".to_vec(),
        }));
        assert!(pump.sync_deadline().is_none());
        assert_eq!(screen(&pump).lines().next(), Some("fresh"));
        // Output after the snapshot is drawn at once, and parsed from a clean state.
        feed(&mut pump, b"!");
        assert_eq!(screen(&pump).lines().next(), Some("fresh!"));
    }

    #[test]
    fn a_snapshot_publishes_one_frame_at_the_grid_size() {
        let (mut pump, _, published) = watched(20, 6);
        pump.publish();
        published.try_iter().for_each(drop);

        assert!(pump.apply(PumpMessage::Snapshot {
            size: size(20, 3),
            bytes: SHELL.to_vec(),
        }));
        pump.publish();
        let frames: Vec<GridDimensions> = published
            .try_iter()
            .filter_map(|update| match update {
                FrameUpdate::Rows {
                    dimensions,
                    rebuild,
                    ..
                } => {
                    assert!(rebuild, "every row is published again");
                    Some(dimensions)
                }
                _ => None,
            })
            .collect();
        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!((frames[0].columns, frames[0].screen_lines), (20, 6));
    }

    #[test]
    fn a_snapshot_before_the_view_measured_tells_the_program_nothing() {
        let (mut pump, written) = sized(20, 6);
        pump.sized = false;
        assert!(pump.apply(PumpMessage::Snapshot {
            size: size(20, 3),
            bytes: SHELL.to_vec(),
        }));
        assert_eq!(pump.term.lock().screen_lines(), 6);
        assert!(resizes(&written).is_empty());
    }
}
