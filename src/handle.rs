//! What an application drives the terminal through.

use std::rc::Rc;

use zgui::prelude::{ClipboardKind, GetUntracked, LocalStorage, RwSignal, Set, Signal};

use crate::config::TerminalConfig;
use crate::core::TerminalCore;
use crate::core::command::{ClearMode, TerminalCommand};
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
    pub(crate) core: Rc<TerminalCore>,
    pub(crate) title: RwSignal<Option<String>, LocalStorage>,
    pub(crate) dimensions: RwSignal<GridDimensions, LocalStorage>,
    pub(crate) scroll: RwSignal<ScrollState, LocalStorage>,
    pub(crate) search: RwSignal<SearchState, LocalStorage>,
    pub(crate) metrics: RwSignal<CellMetrics, LocalStorage>,
    pub(crate) size: RwSignal<TerminalSize, LocalStorage>,
    pub(crate) config: RwSignal<TerminalConfig, LocalStorage>,
}

impl TerminalHandle {
    /// Sends `bytes` to the program.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        self.core.send(TerminalCommand::Write(bytes.into()));
    }

    /// Sends `text` to the program as though it were pasted.
    pub fn paste(&self, text: impl Into<String>) {
        self.core.send(TerminalCommand::Paste(text.into()));
    }

    /// Puts the selected text on the clipboard `kind`.
    pub fn copy(&self, kind: ClipboardKind) {
        self.core.send(TerminalCommand::CopySelection(kind));
    }

    /// Selects everything the grid holds.
    pub fn select_all(&self) {
        self.core.send(TerminalCommand::SelectAll);
    }

    /// Drops the selection.
    pub fn clear_selection(&self) {
        self.core.send(TerminalCommand::SelectionClear);
    }

    /// Moves the view.
    pub fn scroll(&self, request: ScrollRequest) {
        self.core.send(TerminalCommand::Scroll(request));
    }

    /// Empties the screen, the history, or both.
    pub fn clear(&self, mode: ClearMode) {
        self.core.send(TerminalCommand::Clear(mode));
    }

    /// Returns the emulator to the state it started in.
    pub fn reset(&self) {
        self.core.send(TerminalCommand::Reset);
    }

    /// Draws with a new configuration from now on.
    ///
    /// The cell is measured again, so a change of size, family or line height reflows the grid
    /// and the program is told the new number of columns and lines.
    pub fn set_config(&self, config: TerminalConfig) {
        self.core
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
        self.core.send(TerminalCommand::Shutdown);
    }

    /// What the emulator is doing now: which modes a program has turned on.
    pub fn mode(&self) -> alacritty_terminal::term::TermMode {
        *self.core.term.lock().mode()
    }

    /// Starts looking for `pattern`.
    pub fn search(&self, pattern: impl Into<String>, options: SearchOptions) {
        self.core
            .send(TerminalCommand::Search(pattern.into(), options));
    }

    /// Moves to the next match, wrapping at the ends of the history.
    pub fn search_step(&self, direction: SearchDirection) {
        self.core.send(TerminalCommand::SearchStep(direction));
    }

    /// Stops searching.
    pub fn search_clear(&self) {
        self.core.send(TerminalCommand::SearchClear);
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
}
