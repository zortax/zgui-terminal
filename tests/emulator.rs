//! The emulator, driven end to end through a scripted transport.
//!
//! These tests use the seam an application uses: a transport goes in, bytes go through the
//! parser, and what comes out is what the renderer would draw.

use std::time::Duration;

use zgui_terminal::snapshot::RowSnapshot;
use zgui_terminal::transport::{Channels, TerminalSize};
use zgui_terminal::{ClearMode, TerminalConfig, testing};

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

/// A palette that differs from the default in every colour a query can read.
fn styled() -> TerminalConfig {
    let mut config = TerminalConfig::default();
    config.colors.foreground = zgui_terminal::config::Rgba::rgb(0xe8, 0xe8, 0xd3);
    config.colors.background = zgui_terminal::config::Rgba::rgb(0x12, 0x12, 0x12);
    config.colors.cursor = zgui_terminal::config::Rgba::rgb(0xff, 0xa5, 0x60);
    config.colors.ansi[4] = zgui_terminal::config::Rgba::rgb(0x84, 0x97, 0xbf);
    config
}

#[test]
fn a_program_asking_for_the_colours_hears_the_styled_ones() {
    let harness = Harness::new(20, 5);
    harness.core.set_colors(styled());
    harness.program_writes(b"\x1b]10;?\x07");
    assert_eq!(harness.terminal_wrote(), b"\x1b]10;rgb:e8e8/e8e8/d3d3\x07");
    harness.program_writes(b"\x1b]11;?\x1b\\");
    assert_eq!(
        harness.terminal_wrote(),
        b"\x1b]11;rgb:1212/1212/1212\x1b\\"
    );
    harness.program_writes(b"\x1b]12;?\x07");
    assert_eq!(harness.terminal_wrote(), b"\x1b]12;rgb:ffff/a5a5/6060\x07");
    harness.program_writes(b"\x1b]4;4;?\x07");
    assert_eq!(harness.terminal_wrote(), b"\x1b]4;4;rgb:8484/9797/bfbf\x07");
}

#[test]
fn a_colour_the_program_set_answers_for_itself() {
    let harness = Harness::new(20, 5);
    harness.core.set_colors(styled());
    harness.program_writes(b"\x1b]11;#203040\x07\x1b]11;?\x07");
    assert_eq!(harness.terminal_wrote(), b"\x1b]11;rgb:2020/3030/4040\x07");
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
fn clearing_the_screen_takes_what_was_on_it_away() {
    // The mode is named here, from outside the crate, because that is half of what a clear is: a
    // mode an application cannot write is a clear it cannot ask for.
    let harness = Harness::new(20, 5);
    harness.program_writes(b"hello");
    harness.wait_for_text("hello");

    harness.core.clear(ClearMode::Screen);
    // Every row is read out again after a clear, so the row that held the text comes back empty.
    let rows = harness.rows_until(|rows| rows.iter().any(|row| row.row == 0));
    assert!(
        !rows.iter().any(|row| row_text(row).contains("hello")),
        "the cleared text was still drawn"
    );
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

// ---- A pointer over a grid that has just shrunk ---------------------------------------------

/// The point a pointer names is worked out from the view's own layout, which is a frame behind the
/// grid. Between a shrink and the next layout it names a cell the grid no longer has — and the
/// thread that answers the question is the thread that reads the program, so a panic there is a
/// terminal that stops dead and never comes back. It was reachable by dragging a split narrower
/// with the pointer inside it.
#[test]
fn a_point_off_the_end_of_a_shrunken_grid_does_not_stop_the_terminal() {
    use alacritty_terminal::index::{Column, Line, Point};

    let harness = Harness::new(80, 24);
    harness.program_writes(b"before\r\n");
    harness.wait_for_text("before");

    harness.core.resize(TerminalSize {
        columns: 20,
        lines: 6,
        cell_width: 9,
        cell_height: 19,
    });

    // Every way a stale point can be out of range: past the last column, past the last line, and
    // above the top of the history — which arrives as a line that has wrapped around to a huge
    // number once it is used as an index.
    for point in [
        Point::new(Line(0), Column(79)),
        Point::new(Line(23), Column(0)),
        Point::new(Line(23), Column(79)),
        Point::new(Line(-4000), Column(0)),
        Point::new(Line(i32::MAX), Column(usize::MAX)),
    ] {
        harness.core.hyperlink_at(point);
    }

    // The proof is that the terminal is still reading: a program writing after all that is still
    // drawn. A pump that panicked would never publish this.
    harness.program_writes(b"after\r\n");
    harness.wait_for_text("after");
}

#[test]
fn sizes_in_quick_succession_reach_the_program_once_they_rest() {
    let (to_peer, _from_terminal) = flume::unbounded::<Vec<u8>>();
    let (_to_terminal, from_peer) = flume::unbounded::<Vec<u8>>();
    let (resize_tx, resized) = flume::unbounded::<TerminalSize>();
    let transport = Channels::new(to_peer, from_peer).with_resize(resize_tx);
    let config = TerminalConfig {
        coalesce_window: Duration::ZERO,
        resize_settle: Duration::from_millis(80),
        ..TerminalConfig::default()
    };
    let size = |columns| TerminalSize {
        columns,
        lines: 10,
        cell_width: 9,
        cell_height: 19,
    };
    let core = testing::Core::start(transport, config, size(20)).expect("a terminal");

    // The transport hears the starting size.
    assert_eq!(resized.recv_timeout(PATIENCE).unwrap().columns, 20);
    // The first size is the one the view measured, and applies at once.
    core.resize(size(30));
    assert_eq!(
        resized
            .recv_timeout(Duration::from_millis(40))
            .unwrap()
            .columns,
        30
    );

    for columns in [40, 50, 60, 70] {
        core.resize(size(columns));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(resized.recv_timeout(PATIENCE).unwrap().columns, 70);
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        resized.is_empty(),
        "the sizes before the last never reach the program"
    );
}

/// A palette with dark text on a white background.
fn light() -> TerminalConfig {
    let mut config = TerminalConfig::default();
    config.colors.foreground = zgui_terminal::config::Rgba::rgb(0x20, 0x20, 0x20);
    config.colors.background = zgui_terminal::config::Rgba::rgb(0xff, 0xff, 0xff);
    config
}

impl Harness {
    /// Writes `bytes` as the program, and waits until the terminal has read them.
    fn program_writes_and_waits(&self, bytes: &[u8]) {
        let mut bytes = bytes.to_vec();
        bytes.extend_from_slice(b"\x1b[5n");
        self.program_writes(&bytes);
        assert_eq!(self.terminal_wrote(), b"\x1b[0n");
    }

    /// Asserts that the terminal wrote nothing to the program since the last reply.
    fn terminal_wrote_nothing(&self) {
        self.program_writes(b"\x1b[5n");
        assert_eq!(self.terminal_wrote(), b"\x1b[0n");
    }
}

#[test]
fn a_program_that_asked_hears_when_the_colour_scheme_changes() {
    let harness = Harness::new(20, 5);
    harness.program_writes_and_waits(b"\x1b[?2031h");
    harness.core.set_colors(light());
    assert_eq!(harness.terminal_wrote(), b"\x1b[?997;2n");
    // The query that follows the report reads the new colour.
    harness.program_writes(b"\x1b]11;?\x07");
    assert_eq!(harness.terminal_wrote(), b"\x1b]11;rgb:ffff/ffff/ffff\x07");
    harness.core.set_colors(styled());
    assert_eq!(harness.terminal_wrote(), b"\x1b[?997;1n");
}

#[test]
fn the_colour_scheme_is_reported_only_while_the_program_asks() {
    for before in [
        b"".as_ref(),
        b"\x1b[?2031h\x1b[?2031l".as_ref(),
        b"\x1b[?2031h\x1bc".as_ref(),
    ] {
        let harness = Harness::new(20, 5);
        harness.program_writes_and_waits(before);
        harness.core.set_colors(light());
        harness.terminal_wrote_nothing();
    }
}

#[test]
fn colours_that_keep_the_scheme_are_not_reported() {
    let harness = Harness::new(20, 5);
    harness.core.set_colors(light());
    harness.program_writes_and_waits(b"\x1b[?2031h");
    let mut cursor = light();
    cursor.colors.cursor = zgui_terminal::config::Rgba::rgb(0xff, 0, 0);
    harness.core.set_colors(cursor);
    harness.terminal_wrote_nothing();
}

#[test]
fn a_program_asking_for_the_colour_scheme_gets_an_answer() {
    let harness = Harness::new(20, 5);
    harness.program_writes(b"\x1b[?996n");
    assert_eq!(harness.terminal_wrote(), b"\x1b[?997;1n");
    harness.core.set_colors(light());
    harness.program_writes(b"\x1b[?996n");
    assert_eq!(harness.terminal_wrote(), b"\x1b[?997;2n");
}
