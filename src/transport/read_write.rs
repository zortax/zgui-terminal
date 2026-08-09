//! A transport over a blocking reader and a blocking writer.

use std::io::{Read, Write};

use crate::transport::{OutputSink, TerminalSize, Transport, TransportError};

/// What a resize is reported through, for a source that has somewhere to report it.
type ResizeHook = Box<dyn FnMut(TerminalSize) -> Result<(), TransportError> + Send>;

/// A transport reading `R` on a thread of its own and writing to `W`.
///
/// The simplest seam: anything that is a pair of byte streams works, and a size is reported only
/// when the caller supplies somewhere to report it.
///
/// ```no_run
/// use zgui_terminal::transport::ReadWrite;
///
/// # fn example(reader: std::fs::File, writer: std::fs::File) {
/// let transport = ReadWrite::new(reader, writer);
/// # let _ = transport;
/// # }
/// ```
pub struct ReadWrite<R, W> {
    reader: Option<R>,
    writer: W,
    resize: Option<ResizeHook>,
}

impl<R, W> ReadWrite<R, W>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    /// A transport reading `reader` and writing `writer`.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: Some(reader),
            writer,
            resize: None,
        }
    }

    /// Reports a new size through `resize`.
    ///
    /// A pseudo terminal needs this; a plain pipe has nowhere to put it.
    pub fn on_resize(
        mut self,
        resize: impl FnMut(TerminalSize) -> Result<(), TransportError> + Send + 'static,
    ) -> Self {
        self.resize = Some(Box::new(resize));
        self
    }
}

impl<R, W> Transport for ReadWrite<R, W>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    fn start(&mut self, size: TerminalSize, output: OutputSink) -> Result<(), TransportError> {
        let reader = self
            .reader
            .take()
            .ok_or_else(|| TransportError::new("the transport was already started"))?;
        self.resize(size)?;
        std::thread::Builder::new()
            .name("zgui-terminal-read".to_owned())
            .spawn(move || match output.drain(reader) {
                Ok(()) => output.exited(crate::transport::ExitStatus::default()),
                Err(error) => output.failed(error.into()),
            })
            .map_err(|error| TransportError::new(error.to_string()))?;
        Ok(())
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    fn resize(&mut self, size: TerminalSize) -> Result<(), TransportError> {
        match self.resize.as_mut() {
            Some(resize) => resize(size),
            None => Ok(()),
        }
    }
}
