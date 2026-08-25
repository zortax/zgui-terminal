//! What an application drives the terminal through.

pub use crate::core::command::ClearMode;

use zgui::prelude::{
    ClipboardKind, GetUntracked, LocalStorage, Modifiers, RwSignal, Set, Signal, WithUntracked,
};
use zgui::vocab::KeyEvent;

use crate::config::TerminalConfig;
use crate::core::command::TerminalCommand;
use crate::grid::{AppCursor, Contents, GridSpan};
use crate::input::keyboard::{KeyBindings, KeyState, encode_key};
use crate::render::CellMetrics;
use crate::scroll::{ScrollRequest, ScrollState};
use crate::search::{SearchDirection, SearchOptions, SearchState};
use crate::snapshot::GridDimensions;
use crate::transport::TerminalSize;

/// What the terminal is driven and read through.
///
/// Cloneable, and belongs to the interface thread. Everything that changes the emulator is a
/// message to the thread that owns it, so nothing here waits for a program.
#[derive(Clone)]
pub struct TerminalHandle {
    pub(crate) session: crate::TerminalSession,
    pub(crate) title: RwSignal<Option<String>, LocalStorage>,
    pub(crate) dimensions: RwSignal<GridDimensions, LocalStorage>,
    pub(crate) scroll: RwSignal<ScrollState, LocalStorage>,
    pub(crate) search: RwSignal<SearchState, LocalStorage>,
    pub(crate) metrics: RwSignal<CellMetrics, LocalStorage>,
    pub(crate) size: RwSignal<TerminalSize, LocalStorage>,
    pub(crate) config: RwSignal<TerminalConfig, LocalStorage>,
    pub(crate) revision: RwSignal<u64, LocalStorage>,
}

impl TerminalHandle {
    /// Sends `bytes` to the program.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        self.session.send(TerminalCommand::Write(bytes.into()));
    }

    /// Sends `text` to the program as though it were pasted.
    pub fn paste(&self, text: impl Into<String>) {
        self.session.send(TerminalCommand::Paste(text.into()));
    }

    /// Puts the selected text on the clipboard `kind`.
    pub fn copy(&self, kind: ClipboardKind) {
        self.session.send(TerminalCommand::CopySelection(kind));
    }

    /// Sends `event` to the program, encoded as this terminal encodes what is typed.
    ///
    /// For an application that held a key back to see what followed it and then decided the
    /// program should have had it after all. A key that encodes to nothing is dropped, the way a
    /// bare modifier is.
    pub fn send_key(&self, event: &KeyEvent, modifiers: Modifiers) {
        let (alt_sends_escape, to_bottom) = self
            .config
            .with_untracked(|config| (config.alt_sends_escape, config.scroll_to_bottom_on_input));
        let bindings = KeyBindings { alt_sends_escape };
        let Some(bytes) = encode_key(event, modifiers, KeyState::Down, self.mode(), &bindings)
        else {
            return;
        };
        self.session.send(TerminalCommand::Write(bytes.to_vec()));
        if to_bottom {
            self.session
                .send(TerminalCommand::Scroll(ScrollRequest::Bottom));
        }
    }

    /// Reads every line the terminal holds, history and screen alike.
    ///
    /// Answered at once rather than on the next frame, because a caller reading this is deciding
    /// what a key means. The cost is the size of the history, so a caller that reads on every key
    /// should keep what it built and read again only when the lines have moved.
    pub fn contents(&self) -> Contents {
        crate::grid::read(&self.session.core().term.lock())
    }

    /// Draws `cursor` in place of the program's, or the program's again when given nothing.
    ///
    /// An application's own cursor does not blink and is drawn wherever it is put, including in
    /// the history. What the program asked for is remembered and comes back untouched.
    pub fn set_cursor(&self, cursor: Option<AppCursor>) {
        self.session.send(TerminalCommand::SetCursor(cursor));
    }

    /// Selects `span`, as a drag would have.
    pub fn select(&self, span: GridSpan) {
        self.session.send(TerminalCommand::Select(span));
    }

    /// Lights `spans` until told otherwise, in the colour a selection is drawn in.
    ///
    /// A layer of its own, so lighting what a command took and selecting what a person is
    /// choosing never take the same slot from one another. Nothing puts the light out on its own:
    /// an empty list is what does.
    pub fn flash(&self, spans: Vec<GridSpan>) {
        self.session.send(TerminalCommand::Flash(spans));
    }

    /// Puts `line` on screen, counted from the oldest one the history holds.
    pub fn reveal(&self, line: usize) {
        self.session.send(TerminalCommand::Reveal(line));
    }

    /// Selects everything the grid holds.
    pub fn select_all(&self) {
        self.session.send(TerminalCommand::SelectAll);
    }

    /// Drops the selection.
    pub fn clear_selection(&self) {
        self.session.send(TerminalCommand::SelectionClear);
    }

    /// Moves the view.
    pub fn scroll(&self, request: ScrollRequest) {
        self.session.send(TerminalCommand::Scroll(request));
    }

    /// Empties the screen, the history, or both.
    pub fn clear(&self, mode: ClearMode) {
        self.session.send(TerminalCommand::Clear(mode));
    }

    /// Returns the emulator to the state it started in.
    pub fn reset(&self) {
        self.session.send(TerminalCommand::Reset);
    }

    /// Draws with a new configuration from now on.
    ///
    /// The cell is measured again, so a change of size, family or line height reflows the grid
    /// and the program is told the new number of columns and lines.
    pub fn set_config(&self, config: TerminalConfig) {
        self.session
            .send(TerminalCommand::SetConfig(Box::new(config.clone())));
        self.config.set(config);
    }

    /// Changes the configuration in place.
    ///
    /// ```no_run
    /// # use zgui_terminal::TerminalHandle;
    /// # fn example(terminal: &TerminalHandle) {
    /// terminal.update_config(|config| config.font_size += 1.0);
    /// # }
    /// ```
    pub fn update_config(&self, change: impl FnOnce(&mut TerminalConfig)) {
        let mut config = self.config.get_untracked();
        change(&mut config);
        self.set_config(config);
    }

    /// The configuration in force.
    pub fn config(&self) -> Signal<TerminalConfig, LocalStorage> {
        self.config.into()
    }

    /// Asks the program to end.
    pub fn shutdown(&self) {
        self.session.send(TerminalCommand::Shutdown);
    }

    /// What the emulator is doing now: which modes a program has turned on.
    pub fn mode(&self) -> alacritty_terminal::term::TermMode {
        *self.session.core().term.lock().mode()
    }

    /// Starts looking for `pattern`.
    pub fn search(&self, pattern: impl Into<String>, options: SearchOptions) {
        self.session
            .send(TerminalCommand::Search(pattern.into(), options));
    }

    /// Moves to the next match, wrapping at the ends of the history.
    pub fn search_step(&self, direction: SearchDirection) {
        self.session.send(TerminalCommand::SearchStep(direction));
    }

    /// Stops searching.
    pub fn search_clear(&self) {
        self.session.send(TerminalCommand::SearchClear);
    }

    /// The title the program asked for.
    pub fn title(&self) -> Signal<Option<String>, LocalStorage> {
        self.title.into()
    }

    /// How large the grid is.
    pub fn dimensions(&self) -> Signal<GridDimensions, LocalStorage> {
        self.dimensions.into()
    }

    /// Where the view sits over the history, for an application drawing its own scroll bar.
    pub fn scroll_state(&self) -> Signal<ScrollState, LocalStorage> {
        self.scroll.into()
    }

    /// What the search knows, for an application drawing its own search bar.
    pub fn search_state(&self) -> Signal<SearchState, LocalStorage> {
        self.search.into()
    }

    /// How large a cell is, which an application needs to size a window to whole cells.
    pub fn cell_metrics(&self) -> Signal<CellMetrics, LocalStorage> {
        self.metrics.into()
    }

    /// How large the grid is in cells and in pixels.
    pub fn size(&self) -> Signal<TerminalSize, LocalStorage> {
        self.size.into()
    }

    /// A number that changes whenever what [`Self::contents`] would answer might have. Tracked.
    ///
    /// What an application reading the lines watches, so it keeps what it built until the grid
    /// moves. It counts frames that drew something, so it can say a line changed when only the
    /// cursor did; it never says nothing changed when something has.
    pub fn revision(&self) -> Signal<u64, LocalStorage> {
        self.revision.into()
    }
}
