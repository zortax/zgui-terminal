//! What the emulator publishes to the view.

use crate::event::{Hyperlink, MouseShape};
use crate::scroll::ScrollState;
use crate::search::SearchState;
use crate::snapshot::{GridDimensions, OverlaySnapshot, RowSnapshot};
use crate::transport::{ExitStatus, TransportError};

/// How an answer is formatted before it goes back to the program.
type Formatter = std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The answer to a clipboard read, which the program is waiting for.
///
/// A read is answered once. Dropping this answers with nothing, so a program that asked never
/// waits forever.
pub struct ClipboardReply {
    /// How the answer is formatted before it is sent, when the request came from the program.
    format: Option<Formatter>,
    /// Where the answer goes.
    writer: flume::Sender<crate::core::writer::WriteRequest>,
    /// Whether the answer has already been given.
    answered: bool,
}

impl ClipboardReply {
    /// A reply that formats with `format` and writes to `writer`.
    pub(crate) fn new(
        format: Option<Formatter>,
        writer: flume::Sender<crate::core::writer::WriteRequest>,
    ) -> Self {
        Self {
            format,
            writer,
            answered: false,
        }
    }

    /// Answers with `text`.
    pub fn answer(mut self, text: &str) {
        self.send(text);
    }

    /// Answers with nothing.
    pub fn refuse(mut self) {
        self.send("");
    }

    fn send(&mut self, text: &str) {
        if self.answered {
            return;
        }
        self.answered = true;
        if let Some(format) = self.format.as_ref() {
            let _ = self.writer.send(crate::core::writer::WriteRequest::Bytes(
                format(text).into_bytes(),
            ));
        }
    }
}

impl Drop for ClipboardReply {
    fn drop(&mut self) {
        // A program that asked and was never answered would wait for as long as it is running.
        self.send("");
    }
}

impl std::fmt::Debug for ClipboardReply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClipboardReply")
            .field("answered", &self.answered)
            .finish()
    }
}

/// What the emulator publishes after one batch of work.
#[derive(Debug)]
pub(crate) enum FrameUpdate {
    /// The rows that changed, with the view they were read at.
    Rows {
        /// Every row whose content is different from the one before it.
        rows: Vec<RowSnapshot>,
        /// How large the grid is now.
        dimensions: GridDimensions,
        /// Where the view sits.
        scroll: ScrollState,
        /// Whether every row the view holds has to be rebuilt.
        rebuild: bool,
    },
    /// What is drawn over the rows.
    Overlay(Box<OverlaySnapshot>),
    /// The program set a title, or asked for the default.
    Title(Option<String>),
    /// The program rang the bell.
    Bell,
    /// The program copied text.
    ClipboardStore(zgui::prelude::ClipboardKind, String),
    /// The program asked to read the clipboard.
    ClipboardLoad {
        /// Which clipboard it asked for.
        kind: zgui::prelude::ClipboardKind,
        /// Where the answer goes.
        reply: ClipboardReply,
    },
    /// The answer to a copy, ready for the clipboard `kind`.
    Selection(zgui::prelude::ClipboardKind, Option<String>),
    /// The link at a point the view asked about.
    Hyperlink(Option<Hyperlink>),
    /// What the search knows now.
    Search(SearchState),
    /// What the pointer should look like.
    MouseShape(MouseShape),
    /// The program asked the cursor to blink, or to stop.
    CursorBlink(bool),
    /// The program ended.
    Exited(ExitStatus),
    /// The transport failed.
    Failed(TransportError),
}
