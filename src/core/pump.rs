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
use alacritty_terminal::vte::ansi::{Handler, Processor};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;

use crate::config::TerminalConfig;
use crate::core::command::{ClearMode, ScrollRequest, TerminalCommand, direction_of};
use crate::core::listener::EventProxy;
use crate::core::prefilter::Prefilter;
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
    /// The size the program was last told about.
    size: TerminalSize,
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
            size,
            search: None,
            search_state: SearchState::default(),
            focused: false,
            mouse_shape: MouseShape::default(),
            dirty: true,
            rebuild: true,
            hovered_link: None,
            app_cursor: None,
            app_flash: Vec::new(),
        };
        (pump, term)
    }

    /// Reads and publishes until the program ends or the view goes away.
    ///
    /// The first message of a batch is applied at once, which is what keeps a keystroke's echo
    /// quick. Everything that arrives inside the window after it joins the same frame, which is
    /// what keeps a flood from asking for a frame per read.
    pub fn run(mut self) {
        loop {
            let Ok(first) = self.inbox.recv() else {
                return;
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
            self.publish();
        }
    }

    /// Applies one message. Answers whether the pump should keep going.
    fn apply(&mut self, message: PumpMessage) -> bool {
        match message {
            PumpMessage::Bytes(bytes) => {
                let term = &self.term;
                let processor = &mut self.processor;
                let mut locked = term.lock();
                self.prefilter.filter(&bytes, &mut |chunk| {
                    processor.advance(&mut *locked, chunk);
                });
                drop(locked);
                self.dirty = true;
                true
            }
            PumpMessage::Term(event) => self.on_term_event(event),
            PumpMessage::Command(command) => self.on_command(command),
            PumpMessage::Exited(status) => {
                let _ = self.updates.send(FrameUpdate::Exited(status));
                false
            }
            PumpMessage::Failed(error) => {
                let _ = self.updates.send(FrameUpdate::Failed(error));
                false
            }
        }
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
                let color = self.term.lock().colors()[index].unwrap_or_else(|| {
                    let fallback = self.config.colors.indexed(index as u8);
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
            Event::Exit => {
                let _ = self
                    .updates
                    .send(FrameUpdate::Exited(ExitStatus::default()));
                return false;
            }
            Event::ChildExit(status) => {
                let _ = self.updates.send(FrameUpdate::Exited(how_it_ended(status)));
                return false;
            }
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
            TerminalCommand::Resize(size) => self.resize(size),
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
                self.restart_rows();
            }
            TerminalCommand::SetColors(config) => {
                self.builder.set_palette(config.colors);
                self.config.colors = config.colors;
                // Every row was built with the old colours in it.
                self.restart_rows();
            }
            TerminalCommand::SetConfig(config) => {
                let config = *config;
                self.builder.set_palette(config.colors);
                self.term.lock().set_options(config.term_config());
                self.config = config;
                self.restart_rows();
            }
        }
        true
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

    /// Gives the grid a new size, and tells the program about it.
    fn resize(&mut self, size: TerminalSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        let dimensions = TermSize::new(usize::from(size.columns), usize::from(size.lines));
        self.term.lock().resize(dimensions);
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
        if overlay != self.overlay {
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
