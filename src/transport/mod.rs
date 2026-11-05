//! What carries bytes between the emulator and the program behind it.
//!
//! The seam is one trait, [`Transport`], and one non-blocking sink, [`OutputSink`]. A transport
//! is active: it starts its own thread or task and pushes what it reads into the sink. One
//! contract therefore fits a blocking reader and an asynchronous stream, and no runtime type
//! appears in it.

mod channels;
#[cfg(feature = "pty")]
mod pty;
mod read_write;

use std::cell::Cell;
use std::fmt;

pub use crate::transport::channels::{Channels, Output};
#[cfg(feature = "pty")]
pub use crate::transport::pty::Pty;
pub use crate::transport::read_write::ReadWrite;

/// How large the grid is, in cells and in pixels.
///
/// The pixel sizes are real. A pseudo terminal passes them through `TIOCSWINSZ`, and a program
/// that draws pictures asks for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TerminalSize {
    /// How many columns the grid has.
    pub columns: u16,
    /// How many lines the screen has.
    pub lines: u16,
    /// How wide one cell is, in device pixels.
    pub cell_width: u16,
    /// How tall one cell is, in device pixels.
    pub cell_height: u16,
}

impl From<TerminalSize> for alacritty_terminal::event::WindowSize {
    fn from(size: TerminalSize) -> Self {
        Self {
            num_lines: size.lines,
            num_cols: size.columns,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        }
    }
}

/// How a program ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ExitStatus {
    /// The status the program returned, when it returned one.
    pub code: Option<i32>,
    /// The signal that ended it, when a signal did.
    pub signal: Option<i32>,
}

impl ExitStatus {
    /// A normal end with `code`.
    pub fn code(code: i32) -> Self {
        Self {
            code: Some(code),
            signal: None,
        }
    }

    /// Whether the program reported success.
    pub fn is_success(&self) -> bool {
        self.code == Some(0) && self.signal.is_none()
    }
}

/// What went wrong in a transport.
#[derive(Debug)]
pub struct TransportError {
    /// What happened, for a person to read.
    message: String,
}

impl TransportError {
    /// An error described by `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TransportError {}

impl From<std::io::Error> for TransportError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// What a transport reports, on its way to the emulator.
#[derive(Debug)]
pub(crate) enum TransportMessage {
    /// One chunk of output.
    Bytes(Vec<u8>),
    /// A whole screen, encoded for an empty terminal of `size`.
    Snapshot {
        /// The size the bytes were encoded for.
        size: TerminalSize,
        /// The bytes that draw the screen.
        bytes: Vec<u8>,
    },
    /// The program ended.
    Exited(ExitStatus),
    /// The transport failed.
    Failed(TransportError),
}

/// Where a transport puts what it reads.
///
/// Every method returns at once, so a transport may call these from inside an asynchronous task
/// that has no runtime of its own. The sink is cloneable, and closing happens when the emulator
/// goes away.
#[derive(Clone, Debug)]
pub struct OutputSink {
    sender: flume::Sender<TransportMessage>,
}

impl OutputSink {
    /// The sink feeding `sender`.
    pub(crate) fn new(sender: flume::Sender<TransportMessage>) -> Self {
        Self { sender }
    }

    /// Hands over one chunk of output.
    pub fn bytes(&self, bytes: Vec<u8>) {
        let _ = self.sender.send(TransportMessage::Bytes(bytes));
    }

    /// Hands over a whole screen: `bytes` encoded for an empty terminal of `size`.
    ///
    /// The emulator empties itself, history and modes included, takes `size`, parses `bytes`
    /// and then returns to the size of its grid, which reflows the screen as a resize does. No
    /// frame shows the steps between. When `size` differs from the grid's, the transport hears
    /// the grid's size again, so the peer takes it.
    pub fn snapshot(&self, size: TerminalSize, bytes: Vec<u8>) {
        let _ = self.sender.send(TransportMessage::Snapshot { size, bytes });
    }

    /// Hands over one item of `output`.
    pub fn output(&self, output: Output) {
        match output {
            Output::Bytes(bytes) => self.bytes(bytes),
            Output::Snapshot { size, bytes } => self.snapshot(size, bytes),
        }
    }

    /// Reports that the program ended.
    pub fn exited(&self, status: ExitStatus) {
        let _ = self.sender.send(TransportMessage::Exited(status));
    }

    /// Reports that the transport failed.
    pub fn failed(&self, error: TransportError) {
        let _ = self.sender.send(TransportMessage::Failed(error));
    }

    /// Whether the emulator is gone, which is the signal to stop reading.
    pub fn is_closed(&self) -> bool {
        self.sender.is_disconnected()
    }

    /// Reads `reader` until the end, handing every chunk over.
    ///
    /// For a blocking source. Call it on a thread of the transport's own.
    pub fn drain(&self, mut reader: impl std::io::Read) -> std::io::Result<()> {
        let mut buffer = vec![0u8; 65536];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(read) => {
                    if self.is_closed() {
                        return Ok(());
                    }
                    self.bytes(buffer[..read].to_vec());
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

/// What carries bytes between the emulator and the program behind it.
///
/// The emulator calls [`start`](Transport::start) once, then [`send`](Transport::send) and
/// [`resize`](Transport::resize) from one thread of its own, one call at a time. An
/// implementation therefore needs no lock and may block.
pub trait Transport: Send + 'static {
    /// Starts carrying bytes, pushing everything read into `output`.
    fn start(&mut self, size: TerminalSize, output: OutputSink) -> Result<(), TransportError>;

    /// Sends `bytes` toward the program.
    fn send(&mut self, bytes: &[u8]) -> Result<(), TransportError>;

    /// Reports a new size toward the program.
    fn resize(&mut self, size: TerminalSize) -> Result<(), TransportError>;

    /// Asks the program to end.
    ///
    /// Called when the emulator is dropped. The default does nothing, which suits a transport
    /// whose reader ends when its peer closes.
    fn shutdown(&mut self) {}
}

/// A transport on its way into the component, taken once at mount.
///
/// A component property is built at the call site and consumed when the component is created,
/// which is what this cell holds.
pub struct TransportSource {
    transport: Cell<Option<Box<dyn Transport>>>,
}

impl TransportSource {
    /// The source carrying `transport`.
    pub fn new(transport: impl Transport) -> Self {
        Self {
            transport: Cell::new(Some(Box::new(transport))),
        }
    }

    /// Takes the transport out, leaving nothing behind.
    pub(crate) fn take(&self) -> Option<Box<dyn Transport>> {
        self.transport.take()
    }
}

/// A source carrying nothing, which is what a terminal drawing a session it was handed has.
impl Default for TransportSource {
    fn default() -> Self {
        Self {
            transport: Cell::new(None),
        }
    }
}

impl fmt::Debug for TransportSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TransportSource")
    }
}

impl<T: Transport> From<T> for TransportSource {
    fn from(transport: T) -> Self {
        Self::new(transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_reports_every_chunk_and_stops_at_the_end() {
        let (sender, receiver) = flume::unbounded();
        let sink = OutputSink::new(sender);
        sink.drain(&b"hello world"[..]).expect("read");
        drop(sink);

        let chunks: Vec<Vec<u8>> = receiver
            .into_iter()
            .map(|message| match message {
                TransportMessage::Bytes(bytes) => bytes,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(chunks.concat(), b"hello world");
    }

    #[test]
    fn a_closed_sink_is_visible_to_the_transport() {
        let (sender, receiver) = flume::unbounded::<TransportMessage>();
        let sink = OutputSink::new(sender);
        assert!(!sink.is_closed());
        drop(receiver);
        assert!(sink.is_closed());
    }

    #[test]
    fn terminal_size_carries_its_pixels_into_the_window_size() {
        let size = TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 9,
            cell_height: 19,
        };
        let window: alacritty_terminal::event::WindowSize = size.into();
        assert_eq!(window.num_cols, 80);
        assert_eq!(window.num_lines, 24);
        assert_eq!(window.cell_width, 9);
        assert_eq!(window.cell_height, 19);
    }
}
