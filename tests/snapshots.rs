//! Whole screens from a peer that encodes them for a size of its own.
//!
//! A peer that attaches to a remote terminal sends its screen as escape sequences for an empty
//! terminal of the remote size. The grid can have another size by then. What is asserted here is
//! that the screen lands as a grid of the remote size would show it after a resize: the prompt on
//! its row, nothing stale below it, and the grid's size told back to the peer.

use std::time::{Duration, Instant};

use zgui_terminal::TerminalConfig;
use zgui_terminal::testing::Core;
use zgui_terminal::transport::{Channels, Output, TerminalSize};

/// How long a test waits for the thread that owns the emulator.
const PATIENCE: Duration = Duration::from_secs(5);

/// A grid size of `columns` by `lines`.
fn size(columns: u16, lines: u16) -> TerminalSize {
    TerminalSize {
        columns,
        lines,
        cell_width: 8,
        cell_height: 16,
    }
}

/// A shell at 30 by 4: history above, output, and a marked prompt with the cursor after it.
const SHELL: &[u8] = b"\x1bc\
    one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n\
    \x1b[4;1H\x1b]133;A\x07$ \x1b[4;3H";

/// An emulator over a framed transport, the peer's ends, and the sizes it reports.
struct Peer {
    core: Core,
    output: flume::Sender<Output>,
    sizes: flume::Receiver<TerminalSize>,
}

impl Peer {
    fn start(grid: TerminalSize) -> Self {
        let (input, _from_terminal) = flume::unbounded();
        let (output, from_peer) = flume::unbounded();
        let (resize, sizes) = flume::unbounded();
        let transport = Channels::framed(input, from_peer).with_resize(resize);
        let core = Core::start(transport, TerminalConfig::default(), grid).expect("it starts");
        // The view lays the grid out once, so the grid holds a measured size.
        core.resize(grid);
        Self {
            core,
            output,
            sizes,
        }
    }

    /// The screen rows, once `done` holds or the patience runs out.
    fn screen_when(&self, done: impl Fn(&[String]) -> bool) -> Vec<String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let contents = self.core.contents();
            let screen = contents.lines[contents.screen..].to_vec();
            if done(&screen) || Instant::now() >= deadline {
                return screen;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Every size reported until `patience` passes with none.
    fn reported(&self, patience: Duration) -> Vec<TerminalSize> {
        let mut sizes = Vec::new();
        while let Ok(size) = self.sizes.recv_timeout(patience) {
            sizes.push(size);
        }
        sizes
    }
}

#[test]
fn a_snapshot_for_a_smaller_terminal_keeps_the_prompt_on_its_row() {
    let peer = Peer::start(size(30, 8));
    let stale: Vec<u8> = (0..8)
        .flat_map(|n| format!("stale {n}\r\n").into_bytes())
        .collect();
    peer.output.send(Output::Bytes(stale)).unwrap();
    peer.screen_when(|rows| rows.iter().any(|row| row == "stale 7"));
    peer.reported(Duration::from_millis(100));

    peer.output
        .send(Output::Snapshot {
            size: size(30, 4),
            bytes: SHELL.to_vec(),
        })
        .unwrap();
    let rows = peer.screen_when(|rows| rows.iter().any(|row| row == "$"));
    let prompt = rows.iter().position(|row| row == "$").expect("a prompt");
    assert_eq!(rows[prompt - 1], "five", "{rows:?}");
    assert!(rows[prompt + 1..].iter().all(String::is_empty), "{rows:?}");
    assert!(!rows.iter().any(|row| row.starts_with("stale")), "{rows:?}");
    let contents = peer.core.contents();
    assert!(
        !contents.lines.iter().any(|line| line.starts_with("stale")),
        "the history is the snapshot's own"
    );
    assert_eq!(peer.reported(Duration::from_millis(200)), vec![size(30, 8)]);

    // Live output after the snapshot lands where the cursor is.
    peer.output.send(Output::Bytes(b"ls".to_vec())).unwrap();
    let rows = peer.screen_when(|rows| rows.iter().any(|row| row == "$ ls"));
    assert_eq!(rows[prompt], "$ ls", "{rows:?}");
}

#[test]
fn a_resize_after_a_snapshot_reaches_the_peer() {
    let peer = Peer::start(size(30, 4));
    peer.output
        .send(Output::Snapshot {
            size: size(30, 4),
            bytes: SHELL.to_vec(),
        })
        .unwrap();
    peer.screen_when(|rows| rows.last().is_some_and(|row| row == "$"));
    peer.reported(Duration::from_millis(100));

    peer.core.resize(size(40, 10));
    let reported = peer.reported(Duration::from_millis(500));
    assert_eq!(reported.last(), Some(&size(40, 10)), "{reported:?}");
}

#[test]
fn plain_bytes_still_draw_on_a_framed_transport() {
    let peer = Peer::start(size(20, 3));
    peer.output.send(Output::from(b"hello".to_vec())).unwrap();
    let rows = peer.screen_when(|rows| rows.first().is_some_and(|row| row == "hello"));
    assert_eq!(rows[0], "hello");
}
