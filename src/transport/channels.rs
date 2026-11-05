//! A transport over channels, for a source that is already asynchronous.
//!
//! The output channel carries plain bytes, or [`Output`] items for a peer that also sends whole
//! screens.

use crate::transport::{ExitStatus, OutputSink, TerminalSize, Transport, TransportError};

/// A transport whose peer speaks in channels.
///
/// This is the seam for a source that has a runtime of its own: a kubernetes exec attach, a
/// websocket, an SSH channel. The peer holds the other end of `input`, writes what it reads into
/// `output`, and neither side names the other's executor.
///
/// ```
/// use zgui_terminal::transport::Channels;
///
/// let (input, from_terminal) = flume::unbounded();
/// let (to_terminal, output) = flume::unbounded();
/// let transport = Channels::new(input, output);
/// // The peer now reads `from_terminal` and writes `to_terminal`.
/// # let _ = (transport, from_terminal, to_terminal);
/// ```
pub struct Channels {
    input: flume::Sender<Vec<u8>>,
    output: Option<Source>,
    resize: Option<flume::Sender<TerminalSize>>,
    exit: Option<flume::Receiver<ExitStatus>>,
}

/// One item of a peer's output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// Bytes the emulator parses on top of what it holds.
    Bytes(Vec<u8>),
    /// A whole screen: `bytes` encoded for an empty terminal of `size`.
    ///
    /// See [`OutputSink::snapshot`] for how the emulator takes it.
    Snapshot {
        /// The size the bytes were encoded for.
        size: TerminalSize,
        /// The bytes that draw the screen.
        bytes: Vec<u8>,
    },
}

impl From<Vec<u8>> for Output {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

/// Where the transport reads the peer's output.
enum Source {
    /// Plain bytes.
    Bytes(flume::Receiver<Vec<u8>>),
    /// Bytes and whole screens.
    Output(flume::Receiver<Output>),
}

impl Source {
    /// Hands everything the peer writes to `sink`, until the peer closes or the emulator goes.
    fn drain(self, sink: &OutputSink) {
        match self {
            Self::Bytes(source) => {
                for bytes in source.iter() {
                    if sink.is_closed() {
                        return;
                    }
                    sink.bytes(bytes);
                }
            }
            Self::Output(source) => {
                for output in source.iter() {
                    if sink.is_closed() {
                        return;
                    }
                    sink.output(output);
                }
            }
        }
    }
}

impl Channels {
    /// A transport writing to `input` and reading from `output`.
    pub fn new(input: flume::Sender<Vec<u8>>, output: flume::Receiver<Vec<u8>>) -> Self {
        Self::with_source(input, Source::Bytes(output))
    }

    /// A transport writing to `input` and reading bytes and whole screens from `output`.
    ///
    /// For a peer that sends a snapshot of the screen when it attaches or catches up: the
    /// snapshot carries the size it was encoded for, and the emulator parses it at that size.
    pub fn framed(input: flume::Sender<Vec<u8>>, output: flume::Receiver<Output>) -> Self {
        Self::with_source(input, Source::Output(output))
    }

    fn with_source(input: flume::Sender<Vec<u8>>, output: Source) -> Self {
        Self {
            input,
            output: Some(output),
            resize: None,
            exit: None,
        }
    }

    /// Reports a new size through `resize`.
    pub fn with_resize(mut self, resize: flume::Sender<TerminalSize>) -> Self {
        self.resize = Some(resize);
        self
    }

    /// Takes the program's exit status from `exit`.
    ///
    /// Without one, the emulator learns the program ended when `output` closes, and reports no
    /// status.
    pub fn with_exit(mut self, exit: flume::Receiver<ExitStatus>) -> Self {
        self.exit = Some(exit);
        self
    }
}

impl Transport for Channels {
    fn start(&mut self, size: TerminalSize, output: OutputSink) -> Result<(), TransportError> {
        let source = self
            .output
            .take()
            .ok_or_else(|| TransportError::new("the transport was already started"))?;
        let exit = self.exit.take();
        self.resize(size)?;
        std::thread::Builder::new()
            .name("zgui-terminal-channel".to_owned())
            .spawn(move || {
                source.drain(&output);
                if output.is_closed() {
                    return;
                }
                let status = exit
                    .and_then(|exit| exit.try_recv().ok())
                    .unwrap_or_default();
                output.exited(status);
            })
            .map_err(|error| TransportError::new(error.to_string()))?;
        Ok(())
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.input
            .send(bytes.to_vec())
            .map_err(|_| TransportError::new("the program is gone"))
    }

    fn resize(&mut self, size: TerminalSize) -> Result<(), TransportError> {
        if let Some(resize) = self.resize.as_ref() {
            let _ = resize.send(size);
        }
        Ok(())
    }
}
