//! The thread that talks to the program.
//!
//! Writing is kept off the interface thread. A program that has stopped reading would otherwise
//! block the window on the next keystroke.

use crate::transport::{TerminalSize, Transport};

/// What the writer thread does.
#[derive(Debug)]
pub(crate) enum WriteRequest {
    /// Send these bytes.
    Bytes(Vec<u8>),
    /// Tell the program the grid has a new size.
    Resize(TerminalSize),
    /// Stop.
    Shutdown,
}

/// Sends what the emulator and the person produce, one call at a time.
pub(crate) fn run(mut transport: Box<dyn Transport>, requests: flume::Receiver<WriteRequest>) {
    for request in requests.iter() {
        let result = match request {
            WriteRequest::Bytes(bytes) => transport.send(&bytes),
            WriteRequest::Resize(size) => transport.resize(size),
            WriteRequest::Shutdown => break,
        };
        if result.is_err() {
            // The program is gone. Its reader reports that, so there is nothing to say here.
            break;
        }
    }
    transport.shutdown();
}
