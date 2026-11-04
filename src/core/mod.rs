//! What owns the emulator, and the threads that keep it running.

pub(crate) mod anchor;
pub(crate) mod command;
pub(crate) mod growth;
pub(crate) mod listener;
pub(crate) mod prefilter;
pub(crate) mod prompt;
pub(crate) mod pump;
pub(crate) mod update;
pub(crate) mod writer;

use std::sync::Arc;

use alacritty_terminal::Term;
use alacritty_terminal::sync::FairMutex;

use crate::config::TerminalConfig;
use crate::core::command::TerminalCommand;
use crate::core::listener::EventProxy;
use crate::core::pump::{Pump, PumpMessage};
use crate::core::update::FrameUpdate;
use crate::core::writer::WriteRequest;
use crate::transport::{TerminalSize, Transport, TransportError};

/// The emulator, its threads, and the ways in and out.
pub(crate) struct TerminalCore {
    /// Where commands and bytes go.
    pub commands: flume::Sender<PumpMessage>,
    /// Where published frames arrive.
    pub updates: flume::Receiver<FrameUpdate>,
    /// Where writes to the program go.
    pub writes: flume::Sender<WriteRequest>,
    /// The emulator, for the few questions the interface thread asks directly.
    pub term: Arc<FairMutex<Term<EventProxy>>>,
}

impl TerminalCore {
    /// Builds the emulator, starts its threads, and starts `transport`.
    pub fn start(
        mut transport: Box<dyn Transport>,
        config: TerminalConfig,
        size: TerminalSize,
    ) -> Result<Self, TransportError> {
        let (pump_tx, pump_rx) = flume::unbounded::<PumpMessage>();
        let (update_tx, update_rx) = flume::unbounded::<FrameUpdate>();
        let (write_tx, write_rx) = flume::unbounded::<WriteRequest>();

        // The transport reports into the same queue the emulator's own events use, so what a
        // program wrote and what the emulator said about it stay in the order they happened.
        let (transport_tx, transport_rx) = flume::unbounded();
        transport.start(size, crate::transport::OutputSink::new(transport_tx))?;
        let to_pump = pump_tx.clone();
        std::thread::Builder::new()
            .name("zgui-terminal-input".to_owned())
            .spawn(move || {
                for message in transport_rx.iter() {
                    if to_pump.send(PumpMessage::from(message)).is_err() {
                        return;
                    }
                }
            })
            .map_err(|error| TransportError::new(error.to_string()))?;

        let proxy = EventProxy::new(pump_tx.clone());
        let (pump, term) = Pump::new(
            config,
            size,
            proxy,
            write_tx.clone(),
            update_tx,
            pump_rx,
            pump_tx.clone(),
        );

        std::thread::Builder::new()
            .name("zgui-terminal-pump".to_owned())
            .spawn(move || pump.run())
            .map_err(|error| TransportError::new(error.to_string()))?;
        std::thread::Builder::new()
            .name("zgui-terminal-writer".to_owned())
            .spawn(move || writer::run(transport, write_rx))
            .map_err(|error| TransportError::new(error.to_string()))?;

        Ok(Self {
            commands: pump_tx,
            updates: update_rx,
            writes: write_tx,
            term,
        })
    }

    /// Asks the emulator to do something.
    pub fn send(&self, command: TerminalCommand) {
        let _ = self.commands.send(PumpMessage::Command(command));
    }
}

impl Drop for TerminalCore {
    fn drop(&mut self) {
        self.send(TerminalCommand::Shutdown);
        let _ = self.writes.send(WriteRequest::Shutdown);
    }
}
