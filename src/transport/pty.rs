//! A transport running a program behind a local pseudo terminal.

use std::io::Write;
use std::sync::Arc;

use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::transport::{ExitStatus, OutputSink, TerminalSize, Transport, TransportError};

/// A transport over a pseudo terminal on this machine.
///
/// The child is kept and waited for, so its status is reported and no process is left behind.
///
/// ```no_run
/// use portable_pty::CommandBuilder;
/// use zgui_terminal::transport::Pty;
///
/// let mut command = CommandBuilder::new("/bin/sh");
/// command.env("TERM", "xterm-256color");
/// let transport = Pty::spawn(command).expect("a shell");
/// ```
pub struct Pty {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Box<dyn Write + Send>,
    child: Option<Box<dyn Child + Send + Sync>>,
    reader: Option<Box<dyn std::io::Read + Send>>,
}

impl Pty {
    /// Opens a pseudo terminal and runs `command` behind it.
    pub fn spawn(command: CommandBuilder) -> Result<Self, TransportError> {
        // A real size arrives with `start`; this one only has to be valid.
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TransportError::new(error.to_string()))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TransportError::new(error.to_string()))?;
        // The slave goes away here: while this side holds it open, the reader never sees the end
        // of the program's output.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TransportError::new(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| TransportError::new(error.to_string()))?;

        Ok(Self {
            master: Arc::new(Mutex::new(pair.master)),
            writer,
            child: Some(child),
            reader: Some(reader),
        })
    }
}

impl Transport for Pty {
    fn start(&mut self, size: TerminalSize, output: OutputSink) -> Result<(), TransportError> {
        let reader = self
            .reader
            .take()
            .ok_or_else(|| TransportError::new("the transport was already started"))?;
        let mut child = self
            .child
            .take()
            .ok_or_else(|| TransportError::new("the transport was already started"))?;
        self.resize(size)?;

        let reap = output.clone();
        std::thread::Builder::new()
            .name("zgui-terminal-pty-read".to_owned())
            .spawn(move || {
                if let Err(error) = output.drain(reader) {
                    output.failed(error.into());
                }
            })
            .map_err(|error| TransportError::new(error.to_string()))?;

        // The child is waited for on its own thread: this is what reports the status and what
        // keeps a finished program from staying around as a zombie.
        std::thread::Builder::new()
            .name("zgui-terminal-pty-wait".to_owned())
            .spawn(move || {
                let status = match child.wait() {
                    Ok(status) => ExitStatus {
                        code: i32::try_from(status.exit_code()).ok(),
                        signal: None,
                    },
                    Err(_) => ExitStatus::default(),
                };
                reap.exited(status);
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
        self.master
            .lock()
            .resize(PtySize {
                rows: size.lines.max(1),
                cols: size.columns.max(1),
                // A very large grid would run past what the field holds, and a size that wrapped
                // would tell the program something wildly wrong.
                pixel_width: size.cell_width.saturating_mul(size.columns),
                pixel_height: size.cell_height.saturating_mul(size.lines),
            })
            .map_err(|error| TransportError::new(error.to_string()))
    }
}
