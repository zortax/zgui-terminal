//! How the emulator reports what a program asked for.

use alacritty_terminal::event::{Event, EventListener};

use crate::core::pump::PumpMessage;

/// Passes the emulator's own events to the pump.
///
/// The emulator calls this while it holds its lock, so the whole of it is one send on a channel
/// that never fills. Anything that could block here would deadlock the pump against itself.
#[derive(Clone, Debug)]
pub struct EventProxy {
    sender: flume::Sender<PumpMessage>,
}

impl EventProxy {
    /// A proxy reporting into `sender`.
    pub fn new(sender: flume::Sender<PumpMessage>) -> Self {
        Self { sender }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.sender.send(PumpMessage::Term(event));
    }
}
