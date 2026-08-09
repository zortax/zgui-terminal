//! A terminal with no pseudo terminal behind it.
//!
//! Run it with `cargo run --example echo_stream --release`.
//!
//! What it is worth reading for: the transport seam holds nothing about processes or files. The
//! peer here is a thread that reads what was typed and writes something back, and the same
//! twenty lines would be a kubernetes exec attach, an SSH channel or a test script. The
//! component is exactly the one the shell example mounts.

use zgui::prelude::*;
use zgui_terminal::transport::Channels;
use zgui_terminal::{TerminalConfig, TerminalProps};

/// Starts the peer, and answers with the transport that reaches it.
fn start_peer() -> Channels {
    // What the terminal writes, the peer reads; what the peer writes, the terminal reads.
    let (to_peer, from_terminal) = flume::unbounded::<Vec<u8>>();
    let (to_terminal, from_peer) = flume::unbounded::<Vec<u8>>();

    std::thread::spawn(move || {
        let banner = "\x1b[1;35mzgui-terminal\x1b[0m over a pair of channels.\r\n\
             Type, and it comes back in capitals.\r\n\r\n\
             \x1b[36m┌──────────────┐\x1b[0m\r\n\
             \x1b[36m│\x1b[0m box drawing  \x1b[36m│\x1b[0m\r\n\
             \x1b[36m└──────────────┘\x1b[0m\r\n\r\n\
             \x1b[7m \x1b[0m\u{E0B0}\x1b[7m powerline \x1b[0m\u{E0B0}\r\n\r\n> ";
        let _ = to_terminal.send(banner.as_bytes().to_vec());

        for bytes in from_terminal.iter() {
            let mut answer = Vec::new();
            for byte in bytes {
                match byte {
                    // A return starts a new line and a new prompt.
                    b'\r' => answer.extend_from_slice(b"\r\n> "),
                    // A backspace steps back, clears, and steps back again.
                    0x7f => answer.extend_from_slice(b"\x08 \x08"),
                    other => answer.push(other.to_ascii_uppercase()),
                }
            }
            if to_terminal.send(answer).is_err() {
                return;
            }
        }
    });

    Channels::new(to_peer, from_peer)
}

/// A window holding one terminal.
#[component]
fn Echo() -> impl IntoView {
    let transport = start_peer();
    let config = TerminalConfig {
        font_size: 15.0,
        padding: 10.0,
        ..TerminalConfig::default()
    };

    view! {
        Terminal(class = "echo", transport = transport, config = config)
    }
}

const SHEET: &str = css!(
    ":root {
        background-color: #1e1e2e;
        font-family: sans-serif;
    }

    .echo {
        width: 100%;
        height: 100%;
        background-color: #1e1e2e;
    }"
);

fn main() -> Result<(), zgui::Error> {
    app()
        .with_application_id("dev.zgui.TerminalEcho")
        .with_title("zgui-terminal over channels")
        .with_size(760.0, 460.0)
        .with_stylesheet(SHEET)
        .run(|| view! { Echo() })
}
