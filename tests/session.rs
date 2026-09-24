//! A terminal that outlives the view drawing it.
//!
//! The promise a workspace with several sessions depends on: taking a terminal off screen — into
//! a hidden tab, out of a window somebody closed — must not be a program that was killed. What is
//! asserted here is that the emulator keeps running with nobody watching, and that what it holds
//! comes back whole when something watches again.

use std::time::Duration;

use zgui_terminal::transport::{Channels, TerminalSize};
use zgui_terminal::{TerminalConfig, TerminalSession};

/// How long a test waits for the thread that owns the emulator.
const PATIENCE: Duration = Duration::from_secs(5);

/// A terminal of eighty by twenty-four, with a peer the test writes as.
fn session() -> (TerminalSession, flume::Sender<Vec<u8>>) {
    let (to_peer, _from_terminal) = flume::unbounded::<Vec<u8>>();
    let (to_terminal, from_peer) = flume::unbounded::<Vec<u8>>();
    let session = TerminalSession::start(
        Box::new(Channels::new(to_peer, from_peer)),
        TerminalConfig::default(),
        TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 8,
            cell_height: 16,
        },
    )
    .expect("the terminal starts");
    (session, to_terminal)
}

/// Everything the terminal has published so far, as text, up to `patience`.
fn drawn(rows: &flume::Receiver<zgui_terminal::snapshot::RowSnapshot>) -> String {
    let mut text = String::new();
    while let Ok(row) = rows.recv_timeout(Duration::from_millis(200)) {
        let line: String = row.runs.iter().map(|run| run.text.as_str()).collect();
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text
}

/// What a watcher sees, flattened into rows.
fn watch(session: &TerminalSession) -> flume::Receiver<zgui_terminal::snapshot::RowSnapshot> {
    let (rows, seen) = flume::unbounded();
    let frames = session.watch_for_test();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + PATIENCE;
        while std::time::Instant::now() < deadline {
            let Ok(update) = frames.recv_timeout(Duration::from_millis(200)) else {
                continue;
            };
            for row in update {
                if rows.send(row).is_err() {
                    return;
                }
            }
        }
    });
    seen
}

#[test]
fn nothing_is_drawing_a_session_that_has_just_started() {
    let (session, _peer) = session();
    assert!(!session.is_drawn());
}

#[test]
fn what_the_program_wrote_comes_back_to_a_watcher_that_arrives_late() {
    // The whole point. The program writes while nobody is looking; a view attaches afterwards and
    // must see everything, because the emulator's grid is the truth and attaching asks for it.
    let (session, peer) = session();
    peer.send(b"MARKER-ALPHA\r\n".to_vec()).expect("it writes");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!session.is_drawn(), "nobody was looking while it wrote");

    let seen = watch(&session);
    assert!(session.is_drawn());
    assert!(
        drawn(&seen).contains("MARKER-ALPHA"),
        "a watcher that arrived late still sees it",
    );
}

#[test]
fn a_watcher_going_away_does_not_stop_the_program() {
    let (session, peer) = session();
    peer.send(b"BEFORE\r\n".to_vec()).expect("it writes");
    let seen = watch(&session);
    assert!(drawn(&seen).contains("BEFORE"));

    // The view unmounts. What a hidden tab, an evicted session and a closed window all do.
    session.unwatch();
    assert!(!session.is_drawn());
    peer.send(b"AFTER\r\n".to_vec()).expect("it still writes");
    std::thread::sleep(Duration::from_millis(300));

    // And a second view draws the same program, holding both.
    let again = watch(&session);
    let text = drawn(&again);
    assert!(text.contains("BEFORE"), "what it held is still there");
    assert!(text.contains("AFTER"), "and what it did meanwhile");
}

#[test]
fn a_title_is_kept_for_a_terminal_nobody_is_drawing() {
    // An application labels a tab for a terminal it is not showing.
    let (session, peer) = session();
    peer.send(b"\x1b]0;a title\x07".to_vec())
        .expect("it writes");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(session.title().as_deref(), Some("a title"));
}

#[test]
fn the_end_of_a_program_is_kept_for_a_view_that_attaches_later() {
    // A tab that was off screen when its shell ended still shows how it ended.
    let (to_peer, _from_terminal) = flume::unbounded::<Vec<u8>>();
    let (to_terminal, from_peer) = flume::unbounded::<Vec<u8>>();
    let (exit, exited) = flume::unbounded();
    let session = TerminalSession::start(
        Box::new(Channels::new(to_peer, from_peer).with_exit(exited)),
        TerminalConfig::default(),
        TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 8,
            cell_height: 16,
        },
    )
    .expect("the terminal starts");
    assert_eq!(session.exit_status(), None);

    exit.send(zgui_terminal::transport::ExitStatus::code(3))
        .expect("it ends");
    drop(to_terminal);
    let deadline = std::time::Instant::now() + PATIENCE;
    while session.exit_status().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        session.exit_status(),
        Some(zgui_terminal::transport::ExitStatus::code(3))
    );
}
