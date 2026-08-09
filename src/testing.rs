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

    /// Gives the grid a new size.
    pub fn resize(&self, size: TerminalSize) {
        self.inner.send(TerminalCommand::Resize(size));
    }

    /// Moves the view.
    pub fn scroll(&self, request: crate::scroll::ScrollRequest) {
        self.inner.send(TerminalCommand::Scroll(request));
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
}
