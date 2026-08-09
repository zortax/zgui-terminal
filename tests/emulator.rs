//! The emulator, driven end to end through a scripted transport.
//!
//! These tests use the seam an application uses: a transport goes in, bytes go through the
//! parser, and what comes out is what the renderer would draw.

use std::time::Duration;

use zgui_terminal::snapshot::RowSnapshot;
use zgui_terminal::transport::{Channels, TerminalSize};
use zgui_terminal::{TerminalConfig, testing};

/// How long a test waits for the thread that owns the emulator.
const PATIENCE: Duration = Duration::from_secs(5);

/// A terminal running against a peer the test drives.
struct Harness {
    /// What the peer writes to the terminal.
    to_terminal: flume::Sender<Vec<u8>>,
    /// What the terminal writes to the peer.
    from_terminal: flume::Receiver<Vec<u8>>,
    /// The emulator and its threads.
    core: testing::Core,
}

impl Harness {
    /// A terminal of `columns` by `lines`, with nothing on it.
    fn new(columns: u16, lines: u16) -> Self {
        let (to_peer, from_terminal) = flume::unbounded::<Vec<u8>>();
        let (to_terminal, from_peer) = flume::unbounded::<Vec<u8>>();
        let transport = Channels::new(to_peer, from_peer);
        let config = TerminalConfig {
            // Nothing is collected, so one write is one published frame and the tests are
            // about what the emulator does rather than about when it does it.
            coalesce_window: Duration::ZERO,
            ..TerminalConfig::default()
        };
        let size = TerminalSize {
            columns,
            lines,
            cell_width: 9,
            cell_height: 19,
        };
        let core = testing::Core::start(transport, config, size).expect("a terminal");
        Self {
            to_terminal,
            from_terminal,
            core,
        }
    }

    /// Writes `bytes` as though the program had.
    fn program_writes(&self, bytes: &[u8]) {
        self.to_terminal.send(bytes.to_vec()).expect("the terminal");
    }

    /// What the terminal wrote back to the program, waiting for it to arrive.
    fn terminal_wrote(&self) -> Vec<u8> {
        self.from_terminal
            .recv_timeout(PATIENCE)
            .expect("a reply from the terminal")
    }

    /// The rows the terminal published, waiting until `wanted` is among them.
    ///
    /// Publishing is a batch at a time, so a test asks for the state it is waiting for rather
    /// than for the next message.
    fn rows_until(&self, wanted: impl Fn(&[RowSnapshot]) -> bool) -> Vec<RowSnapshot> {
        let deadline = std::time::Instant::now() + PATIENCE;
        let mut seen: Vec<RowSnapshot> = Vec::new();
        while std::time::Instant::now() < deadline {
            let Some(rows) = self.core.next_rows(Duration::from_millis(200)) else {
                continue;
            };
            for row in rows {
                match seen.iter_mut().find(|held| held.row == row.row) {
                    Some(held) => *held = row,
                    None => seen.push(row),
                }
            }
            seen.sort_by_key(|row| row.row);
            if wanted(&seen) {
                return seen;
            }
        }
        panic!("the terminal never published what the test waited for: {seen:?}");
    }

    /// The text of the row holding `text`, waiting for it to appear.
    fn wait_for_text(&self, text: &str) -> Vec<RowSnapshot> {
        let wanted = text.to_owned();
        self.rows_until(move |rows| {
            rows.iter()
                .any(|row| row_text(row).contains(wanted.as_str()))
        })
    }
}

/// Everything one row draws, as plain text.
fn row_text(row: &RowSnapshot) -> String {
    let mut text = String::new();
    for run in &row.runs {
        while text.chars().count() < usize::from(run.column) {
            text.push(' ');
        }
        text.push_str(&run.text);
    }
    text
}

#[test]
fn what_a_program_writes_becomes_rows() {
    let harness = Harness::new(20, 5);
    harness.program_writes(b"hello");
    let rows = harness.wait_for_text("hello");
    assert!(rows.iter().any(|row| row_text(row) == "hello"));
}

#[test]
fn a_program_asking_what_this_terminal_is_gets_an_answer() {
    // The reply to a device attributes request. A terminal that drops these leaves every program
    // that asks waiting for an answer that never comes.
    let harness = Harness::new(20, 5);
    harness.program_writes(b"\x1b[c");
    let reply = harness.terminal_wrote();
    assert!(
        reply.starts_with(b"\x1b[?"),
        "the terminal answered {reply:?}"
    );
}

#[test]
fn a_program_asking_where_the_cursor_is_gets_an_answer() {
    let harness = Harness::new(20, 5);
    harness.program_writes(b"\x1b[3;5H\x1b[6n");
    let reply = harness.terminal_wrote();
    assert_eq!(reply, b"\x1b[3;5R");
}

#[test]
fn colours_reach_the_rows() {
    let harness = Harness::new(20, 5);
    harness.program_writes(b"\x1b[31mred\x1b[0m");
    let rows = harness.wait_for_text("red");
    let row = rows
        .iter()
        .find(|row| row_text(row).contains("red"))
        .expect("the row");
    let palette = TerminalConfig::default().colors;
    assert_eq!(row.runs[0].style.foreground, palette.ansi[1]);
}

#[test]
fn a_row_that_did_not_change_is_not_published_again() {
    let harness = Harness::new(20, 5);
    harness.program_writes(b"stable\r\n");
    harness.wait_for_text("stable");

    // Writing on the second line leaves the first alone, so it is not sent again.
    harness.program_writes(b"second");
    let published =
        harness.rows_until(|rows| rows.iter().any(|row| row_text(row).contains("second")));
    let first_line_sent = published
        .iter()
        .filter(|row| row_text(row).contains("stable"))
        .count();
    assert!(
        first_line_sent <= 1,
        "the unchanged row was published {first_line_sent} times"
    );
}

#[test]
fn the_alternate_screen_is_told_apart_from_the_ordinary_one() {
    let harness = Harness::new(20, 5);
    harness.program_writes(b"ordinary\r\n");
    harness.wait_for_text("ordinary");
    harness.program_writes(b"\x1b[?1049hfullscreen");
    let rows = harness.wait_for_text("fullscreen");
    // The screen a program puts in front has nothing else on it.
    assert!(!rows.iter().any(|row| row_text(row).contains("ordinary")));
}

#[test]
fn box_drawing_arrives_as_the_characters_it_was_written_as() {
    let harness = Harness::new(20, 5);
    harness.program_writes("┌──┐".as_bytes());
    let rows = harness.wait_for_text("┌──┐");
    let row = rows
        .iter()
        .find(|row| row_text(row).contains('┌'))
        .expect("the row");
    assert_eq!(row_text(row), "┌──┐");
}

#[test]
fn a_wide_character_takes_two_columns() {
    let harness = Harness::new(20, 5);
    harness.program_writes("世界".as_bytes());
    let rows = harness.wait_for_text("世");
    let row = rows
        .iter()
        .find(|row| row_text(row).contains('世'))
        .expect("the row");
    assert_eq!(row.runs[0].width, 4);
}

#[test]
fn what_the_person_types_reaches_the_program() {
    let harness = Harness::new(20, 5);
    harness.core.write(b"ls -la\r".to_vec());
    let sent = harness.terminal_wrote();
    assert_eq!(sent, b"ls -la\r");
}

#[test]
fn a_paste_is_bracketed_when_the_program_asked_for_that() {
    let harness = Harness::new(20, 5);
    harness.program_writes(b"\x1b[?2004h");
    // The mode has to be set before the paste, so the reply to a probe orders the two.
    harness.program_writes(b"\x1b[c");
    let _ = harness.terminal_wrote();

    harness.core.paste("text".to_owned());
    let sent = harness.terminal_wrote();
    assert_eq!(sent, b"\x1b[200~text\x1b[201~");
}

#[test]
fn the_program_learns_when_the_grid_changes_size() {
    let harness = Harness::new(20, 5);
    harness.program_writes(b"hello");
    harness.wait_for_text("hello");

    harness.core.resize(TerminalSize {
        columns: 40,
        lines: 10,
        cell_width: 9,
        cell_height: 19,
    });
    // The rows are rebuilt at the new width, which is what the view redraws from.
    let rows = harness.rows_until(|rows| !rows.is_empty());
    assert!(!rows.is_empty());
}

#[test]
fn a_program_that_ends_is_reported() {
    let harness = Harness::new(20, 5);
    drop(harness.to_terminal);
    let ended = harness.core.wait_for_exit(PATIENCE);
    assert!(ended, "the end of the program was never reported");
}

#[test]
fn a_program_that_puts_its_own_screen_in_front_gets_every_row_read_out() {
    // What vim, htop and their like do. Everything is replaced and no history comes with it, so
    // every row is new and the numbering the old rows had describes a grid that is gone.
    let harness = Harness::new(20, 4);
    for line in 0..30 {
        harness.program_writes(format!("line {line}\r\n").as_bytes());
    }
    harness.wait_for_text("line 29");

    harness.program_writes(b"\x1b[?1049h\x1b[Hfull screen");
    let rows = harness.wait_for_text("full screen");

    // The grid is the screen and nothing else, so what is drawn is four rows at most.
    let screen: Vec<&RowSnapshot> = rows.iter().filter(|row| !row.runs.is_empty()).collect();
    assert!(
        screen
            .iter()
            .any(|row| row_text(row).contains("full screen")),
        "the screen a program put in front never arrived"
    );
    assert!(
        !screen.iter().any(|row| row_text(row).contains("line 29")),
        "the ordinary screen was still drawn behind it"
    );
}

#[test]
fn leaving_the_screen_a_program_put_in_front_brings_the_history_back() {
    let harness = Harness::new(20, 4);
    harness.program_writes(b"before\r\n");
    harness.wait_for_text("before");
    harness.program_writes(b"\x1b[?1049h\x1b[Hin front");
    harness.wait_for_text("in front");

    harness.program_writes(b"\x1b[?1049l");
    let rows = harness.rows_until(|rows| rows.iter().any(|row| row_text(row).contains("before")));
    assert!(rows.iter().any(|row| row_text(row).contains("before")));
}
