//! Driving the emulator without a window.
//!
//! The component is the ordinary way in, and it needs a window and a frame loop. This is the
//! same emulator underneath, reached directly, so what a program writes and what the renderer
//! would draw can be tested on their own.

use std::time::Duration;

use crate::config::TerminalConfig;
use crate::core::TerminalCore;
use crate::core::command::TerminalCommand;
use crate::core::update::FrameUpdate;
use crate::snapshot::RowSnapshot;
use crate::transport::{TerminalSize, Transport, TransportError};

/// The emulator and its threads, without a window.
pub struct Core {
    inner: std::rc::Rc<TerminalCore>,
}

impl Core {
    /// Starts the emulator over `transport`.
    pub fn start(
        transport: impl Transport,
        config: TerminalConfig,
        size: TerminalSize,
    ) -> Result<Self, TransportError> {
        Ok(Self {
            inner: std::rc::Rc::new(TerminalCore::start(Box::new(transport), config, size)?),
        })
    }

    /// Sends `bytes` to the program.
    pub fn write(&self, bytes: Vec<u8>) {
        self.inner.send(TerminalCommand::Write(bytes));
    }

    /// Sends `text` to the program as though it were pasted.
    pub fn paste(&self, text: String) {
        self.inner.send(TerminalCommand::Paste(text));
    }

    /// Draws with `config` from now on, the way a view does once it read a style sheet.
    pub fn set_colors(&self, config: TerminalConfig) {
        self.inner
            .send(TerminalCommand::SetColors(Box::new(config)));
    }

    /// Draws with the colours of `scheme`, the way a session no view draws is told of a new
    /// colour scheme.
    pub fn set_scheme(&self, scheme: crate::config::SchemeColors) {
        self.inner
            .send(TerminalCommand::SetScheme(Box::new(scheme)));
    }

    /// Gives the grid a new size.
    pub fn resize(&self, size: TerminalSize) {
        self.inner.send(TerminalCommand::Resize(size));
    }

    /// Moves the view.
    pub fn scroll(&self, request: crate::scroll::ScrollRequest) {
        self.inner.send(TerminalCommand::Scroll(request));
    }

    /// Asks what is under `point`, the way a pointer moving over the grid does.
    ///
    /// Takes a raw point rather than a position, because the thing worth testing is what happens
    /// when one arrives that the grid no longer has a cell for.
    pub fn hyperlink_at(&self, point: alacritty_terminal::index::Point) {
        self.inner.send(TerminalCommand::HyperlinkAt(Some(point)));
    }

    /// Empties the screen, the history, or both.
    pub fn clear(&self, mode: crate::ClearMode) {
        self.inner.send(TerminalCommand::Clear(mode));
    }

    /// The next rows the emulator published, waiting up to `patience` for them.
    pub fn next_rows(&self, patience: Duration) -> Option<Vec<RowSnapshot>> {
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.inner.updates.recv_timeout(left) {
                Ok(FrameUpdate::Rows { rows, .. }) => return Some(rows),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    }

    /// Whether the program ended inside `patience`.
    pub fn wait_for_exit(&self, patience: Duration) -> bool {
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.inner.updates.recv_timeout(left) {
                Ok(FrameUpdate::Exited(_)) => return true,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
        false
    }

    /// What the emulator is doing now: which modes a program has turned on.
    pub fn mode(&self) -> alacritty_terminal::term::TermMode {
        *self.inner.term.lock().mode()
    }

    /// Every line the emulator holds, history and screen alike.
    pub fn contents(&self) -> crate::grid::Contents {
        crate::grid::read(&self.inner.term.lock())
    }

    /// Draws `cursor` in place of the program's, or the program's again when given nothing.
    pub fn set_cursor(&self, cursor: Option<crate::grid::AppCursor>) {
        self.inner.send(TerminalCommand::SetCursor(cursor));
    }

    /// Selects `span`, as a drag would have.
    pub fn select(&self, span: crate::grid::GridSpan) {
        self.inner.send(TerminalCommand::Select(span));
    }

    /// Puts `line` on screen, counted from the oldest one the history holds.
    pub fn reveal(&self, line: usize) {
        self.inner.send(TerminalCommand::Reveal(line));
    }

    /// Drops the selection.
    pub fn clear_selection(&self) {
        self.inner.send(TerminalCommand::SelectionClear);
    }

    /// Lights `spans` until told otherwise.
    pub fn flash(&self, spans: Vec<crate::grid::GridSpan>) {
        self.inner.send(TerminalCommand::Flash(spans));
    }

    /// Says whether the terminal has focus.
    pub fn set_focus(&self, focused: bool) {
        self.inner.send(TerminalCommand::SetFocus(focused));
    }

    /// The next overlay the emulator published, waiting up to `patience` for it.
    pub fn next_overlay(&self, patience: Duration) -> Option<crate::snapshot::OverlaySnapshot> {
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.inner.updates.recv_timeout(left) {
                Ok(FrameUpdate::Overlay(overlay)) => return Some(*overlay),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    }

    /// Where the view sits over the history.
    pub fn scroll_state(&self) -> crate::ScrollState {
        use alacritty_terminal::grid::Dimensions;

        let term = self.inner.term.lock();
        crate::ScrollState {
            display_offset: term.grid().display_offset(),
            history: term.history_size(),
            screen_lines: term.screen_lines(),
        }
    }
}
