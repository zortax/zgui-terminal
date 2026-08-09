//! What the emulator reports to the application.

use crate::transport::{ExitStatus, TerminalSize, TransportError};

/// A link a program marked in its output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hyperlink {
    /// Where the link points.
    pub uri: String,
    /// The identifier the program gave it, when it gave one.
    pub id: Option<String>,
}

/// What the pointer should look like over the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MouseShape {
    /// Text is selectable here.
    #[default]
    Text,
    /// The program is reading the mouse itself.
    Default,
    /// A link is under the pointer.
    Pointer,
}

/// What the emulator reports.
///
/// The named callbacks on the component cover the common cases. This is the whole set, for an
/// application that wants one place to answer them.
#[derive(Debug)]
#[non_exhaustive]
pub enum TerminalEvent {
    /// The program set a title, or asked for the default one.
    Title(Option<String>),
    /// The program rang the bell.
    Bell,
    /// The program ended.
    Exited(ExitStatus),
    /// The transport failed.
    Failed(TransportError),
    /// The grid changed size.
    Resized(TerminalSize),
    /// The program copied text.
    ClipboardStore(zgui::prelude::ClipboardKind, String),
    /// The program asked to read the clipboard.
    ClipboardReadRequested(zgui::prelude::ClipboardKind),
    /// A link was activated.
    LinkActivated(Hyperlink),
    /// The program asked the cursor to blink, or to stop.
    CursorBlink(bool),
    /// What the pointer should look like changed.
    MouseShape(MouseShape),
}
