//! A page of the characters that have to fill their cell exactly.
//!
//! Run it with `cargo run --example pattern --release`.
//!
//! What it is for: box drawing, block elements and the powerline separators are built for the
//! cell the grid measured rather than taken from the font. Drawn large, any disagreement between
//! the two would show as a seam along every join. Change the size and the scale and the joins
//! stay closed.

use zgui::prelude::*;
use zgui_terminal::transport::Channels;
use zgui_terminal::{TerminalConfig, TerminalProps};

/// A peer that writes the page once and then keeps quiet.
fn peer() -> Channels {
    let (to_peer, _from_terminal) = flume::unbounded::<Vec<u8>>();
    let (to_terminal, from_peer) = flume::unbounded::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut page = String::new();
        page.push_str("┌───┬───┐  ╔═══╦═══╗  ┏━━━┳━━━┓  ╭───┬───╮\r\n");
        page.push_str("│   │   │  ║   ║   ║  ┃   ┃   ┃  │   │   │\r\n");
        page.push_str("├───┼───┤  ╠═══╬═══╣  ┣━━━╋━━━┫  ├───┼───┤\r\n");
        page.push_str("│   │   │  ║   ║   ║  ┃   ┃   ┃  │   │   │\r\n");
        page.push_str("└───┴───┘  ╚═══╩═══╝  ┗━━━┻━━━┛  ╰───┴───╯\r\n\r\n");
        page.push_str("████████  ▀▀▀▀▀▀▀▀  ▄▄▄▄▄▄▄▄  ▌▐▌▐▌▐▌▐  ░▒▓█\r\n");
        page.push_str("▁▂▃▄▅▆▇█  ▏▎▍▌▋▊▉█  ▖▗▘▝▚▞▙▟  ╱╲╳╱╲╳╱╲\r\n\r\n");
        page.push_str(
            "\u{1b}[7m \u{1b}[0m\u{E0B0}\u{1b}[7m branch \u{1b}[0m\u{E0B0}\u{1b}[7m main \u{1b}[0m\u{E0B0}\r\n",
        );
        page.push_str(
            "\u{E0B6}\u{1b}[7m rounded \u{1b}[0m\u{E0B4}  \u{E0B2}\u{1b}[7m left \u{1b}[0m\r\n",
        );
        let _ = to_terminal.send(page.into_bytes());
        // The peer stays alive, so the terminal is not told the program ended.
        std::thread::sleep(std::time::Duration::from_secs(3600));
    });
    Channels::new(to_peer, from_peer)
}

/// The page, drawn large enough to see every edge.
#[component]
fn Pattern() -> impl IntoView {
    let config = TerminalConfig {
        font_size: 28.0,
        padding: 12.0,
        ..TerminalConfig::default()
    };
    view! { Terminal(class = "pattern", transport = peer(), config = config) }
}

const SHEET: &str = css!(
    ":root { background-color: #1e1e2e; }
     .pattern { width: 100%; height: 100%; }"
);

fn main() -> Result<(), zgui::Error> {
    app()
        .with_application_id("dev.zgui.TerminalPattern")
        .with_title("zgui-terminal glyphs")
        .with_size(1100.0, 620.0)
        .with_stylesheet(SHEET)
        .run(|| view! { Pattern() })
}
