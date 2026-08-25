//! Reading what a terminal holds, and drawing over it.
//!
//! The seam an application uses when it answers the keyboard itself: the lines as text, a cursor
//! of its own, a selection it decides, and a line it wants on screen.

use std::time::Duration;

use zgui_terminal::grid::{AppCursor, GridPoint, GridSpan, SpanKind};
use zgui_terminal::transport::{Channels, TerminalSize};
use zgui_terminal::{TerminalConfig, testing};

/// How long a test waits for the thread that owns the emulator.
const PATIENCE: Duration = Duration::from_secs(5);

/// A terminal of `columns` by `lines` holding `scrollback` lines of history.
fn terminal(
    columns: u16,
    lines: u16,
    scrollback: usize,
) -> (flume::Sender<Vec<u8>>, testing::Core) {
    let (to_peer, _from_terminal) = flume::unbounded::<Vec<u8>>();
    let (to_terminal, from_peer) = flume::unbounded::<Vec<u8>>();
    let config = TerminalConfig {
        coalesce_window: Duration::ZERO,
        scrollback,
        ..TerminalConfig::default()
    };
    let core = testing::Core::start(
        Channels::new(to_peer, from_peer),
        config,
        TerminalSize {
            columns,
            lines,
            cell_width: 9,
            cell_height: 19,
        },
    )
    .expect("a terminal");
    (to_terminal, core)
}

/// Waits until the emulator holds a line matching `wanted`.
fn wait_for(
    core: &testing::Core,
    wanted: impl Fn(&zgui_terminal::grid::Contents) -> bool,
) -> zgui_terminal::grid::Contents {
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        let contents = core.contents();
        if wanted(&contents) {
            return contents;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the terminal never held what the test waited for: {:?}",
            core.contents()
        );
        let _ = core.next_rows(Duration::from_millis(50));
    }
}

#[test]
fn the_lines_are_the_screen_and_the_history() {
    let (peer, core) = terminal(20, 3, 100);
    peer.send(b"one\r\ntwo\r\nthree\r\nfour\r\nfive".to_vec())
        .expect("the terminal");

    let contents = wait_for(&core, |held| held.lines.iter().any(|line| line == "five"));

    // Three on the screen and the rest above it, oldest first.
    assert_eq!(contents.screen, contents.lines.len() - 3);
    let text: Vec<&str> = contents.lines.iter().map(String::as_str).collect();
    assert_eq!(
        &text[text.len() - 5..],
        &["one", "two", "three", "four", "five"]
    );
}

#[test]
fn trailing_blanks_are_left_out_and_a_cell_is_a_character() {
    let (peer, core) = terminal(20, 3, 10);
    peer.send("a\u{6f22}b\r\n".as_bytes().to_vec())
        .expect("the terminal");

    let contents = wait_for(&core, |held| {
        held.lines.iter().any(|line| line == "a\u{6f22}b")
    });
    let line = contents
        .lines
        .iter()
        .find(|line| line.as_str() == "a\u{6f22}b")
        .expect("the line");
    // The cell the wide character covers contributes nothing, so counting the width of a prefix
    // answers which column it ends in.
    assert_eq!(line.chars().count(), 3);
}

#[test]
fn the_cursor_is_where_the_program_left_it() {
    let (peer, core) = terminal(20, 4, 10);
    peer.send(b"ab\r\ncd".to_vec()).expect("the terminal");

    let contents = wait_for(&core, |held| {
        held.cursor.column == 2 && held.cursor.line == 1
    });
    assert_eq!(contents.cursor, GridPoint::new(1, 2));
}

#[test]
fn an_application_draws_its_own_cursor_where_it_says() {
    let (peer, core) = terminal(20, 3, 100);
    peer.send(b"one\r\ntwo\r\nthree\r\nfour\r\nfive".to_vec())
        .expect("the terminal");
    wait_for(&core, |held| held.lines.iter().any(|line| line == "five"));

    // The oldest line in the history, which is above the screen. Revealing it is what puts it
    // where a cursor on it can be drawn.
    core.reveal(0);
    core.set_cursor(Some(AppCursor::block(GridPoint::new(0, 1))));

    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        let overlay = core
            .next_overlay(Duration::from_millis(200))
            .unwrap_or_default();
        if overlay.cursor.column == 1 && overlay.cursor.visible && !overlay.cursor.blinking {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no cursor arrived");
    }

    assert_eq!(
        core.scroll_state().display_offset,
        core.scroll_state().history
    );
}

#[test]
fn an_application_selects_the_cells_it_names() {
    let (peer, core) = terminal(20, 3, 10);
    peer.send(b"hello\r\nworld".to_vec()).expect("the terminal");
    wait_for(&core, |held| held.lines.iter().any(|line| line == "world"));

    let screen = core.contents().screen;
    core.select(GridSpan {
        from: GridPoint::new(screen, 1),
        to: GridPoint::new(screen, 3),
        kind: SpanKind::Cells,
    });

    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        let overlay = core
            .next_overlay(Duration::from_millis(200))
            .unwrap_or_default();
        if let Some(columns) = overlay.selection_on(0) {
            // Both ends included, which is what a cell a pointer is over comes to.
            assert_eq!((*columns.start(), *columns.end()), (1, 3));
            return;
        }
        assert!(std::time::Instant::now() < deadline, "no selection arrived");
    }
}

#[test]
fn revealing_a_line_already_on_screen_moves_nothing() {
    let (peer, core) = terminal(20, 3, 100);
    peer.send(b"one\r\ntwo\r\nthree\r\nfour\r\nfive".to_vec())
        .expect("the terminal");
    wait_for(&core, |held| held.lines.iter().any(|line| line == "five"));

    let before = core.scroll_state().display_offset;
    core.reveal(core.contents().screen);
    // The command is applied on the emulator's thread; giving it a frame to arrive.
    let _ = core.next_rows(Duration::from_millis(200));
    assert_eq!(core.scroll_state().display_offset, before);
}

#[test]
fn whether_the_terminal_has_focus_is_drawn_over_the_rows() {
    // What decides whether the cursor is filled or outlined. It travels with the rest of what is
    // drawn over the rows, so a terminal given the keyboard back fills its cursor in at once
    // rather than waiting for the next thing that moves.
    let (peer, core) = terminal(20, 3, 10);
    peer.send(b"hi".to_vec()).expect("the terminal");
    wait_for(&core, |held| held.lines.iter().any(|line| line == "hi"));

    for focused in [true, false, true] {
        core.set_focus(focused);
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            let overlay = core.next_overlay(Duration::from_millis(200));
            if overlay.is_some_and(|overlay| overlay.focused == focused) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no overlay said focused = {focused}"
            );
        }
    }
}

#[test]
fn a_light_is_its_own_layer_and_keeps_its_shape() {
    // What a yank leaves behind. It must survive the selection being dropped in the same breath,
    // and several spans must stay several rather than becoming everything between their corners.
    let (peer, core) = terminal(20, 4, 10);
    peer.send(b"alpha\r\nbeta\r\ngamma".to_vec())
        .expect("the terminal");
    wait_for(&core, |held| held.lines.iter().any(|line| line == "gamma"));

    let screen = core.contents().screen;
    core.flash(vec![
        GridSpan::cells(GridPoint::new(screen, 1), GridPoint::new(screen, 2)),
        GridSpan::cells(GridPoint::new(screen + 2, 0), GridPoint::new(screen + 2, 1)),
    ]);
    core.clear_selection();

    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        let overlay = core
            .next_overlay(Duration::from_millis(200))
            .unwrap_or_default();
        if let (Some(one), Some(two)) = (overlay.flash_on(0), overlay.flash_on(2)) {
            assert_eq!((*one.start(), *one.end()), (1, 2));
            assert_eq!((*two.start(), *two.end()), (0, 1));
            // The row between them is not lit, and dropping the selection took nothing away.
            assert_eq!(overlay.flash_on(1), None);
            assert!(overlay.selection.is_empty());
            return;
        }
        assert!(std::time::Instant::now() < deadline, "nothing was lit");
    }
}

#[test]
fn a_light_goes_out_when_the_application_says_so() {
    let (peer, core) = terminal(20, 3, 10);
    peer.send(b"hello".to_vec()).expect("the terminal");
    wait_for(&core, |held| held.lines.iter().any(|line| line == "hello"));

    let screen = core.contents().screen;
    core.flash(vec![GridSpan::cells(
        GridPoint::new(screen, 0),
        GridPoint::new(screen, 3),
    )]);
    let deadline = std::time::Instant::now() + PATIENCE;
    while core
        .next_overlay(Duration::from_millis(200))
        .is_none_or(|overlay| overlay.flash.is_empty())
    {
        assert!(std::time::Instant::now() < deadline, "nothing was lit");
    }

    core.flash(Vec::new());
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        if core
            .next_overlay(Duration::from_millis(200))
            .is_some_and(|overlay| overlay.flash.is_empty())
        {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "it stayed lit");
    }
}
