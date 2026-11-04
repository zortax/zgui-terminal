//! Keeping the top of the screen in place when a reflow makes its lines take more rows.
//!
//! A narrower grid wraps long lines into more rows. The emulator keeps the cursor on its row and
//! pushes the rows above it into the history, even with blank rows below the cursor. The screen
//! keeps its top line instead: the rows the reflow pushed out come back from the history, as many
//! as there are blank rows below the cursor, so the content grows downward.

use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::Line;
use alacritty_terminal::term::cell::Cell;

use crate::core::anchor::{continues, line_end, line_start, starts_between};

/// The row at the top of a screen at the bottom of its history, in terms a reflow keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Top {
    /// How many lines start after the line at the top, up to the line of the cursor.
    starts_to_cursor: usize,
    /// How many cells of that line come before the top row.
    cells_before: usize,
}

impl Top {
    /// The top of `grid`'s screen, when the view shows the bottom.
    pub fn of(grid: &Grid<Cell>) -> Option<Self> {
        if grid.display_offset() != 0 {
            return None;
        }
        let start = line_start(grid, Line(0));
        let cursor = line_start(grid, grid.cursor.point.line);
        Some(Self {
            starts_to_cursor: starts_between(grid, start, cursor),
            cells_before: (-start.0).max(0) as usize * grid.columns(),
        })
    }

    /// Brings back the rows above the old top that a reflow pushed into the history, as far as
    /// the blank rows below the cursor make room.
    pub fn keep(self, grid: &mut Grid<Cell>) {
        if grid.display_offset() != 0 {
            return;
        }
        let cursor = line_start(grid, grid.cursor.point.line);
        let Some(start) = nth_start_above(grid, cursor, self.starts_to_cursor) else {
            return;
        };
        let rows = self.cells_before / grid.columns().max(1);
        let top = (start.0 + rows as i32).min(line_end(grid, start).0);
        let pushed = usize::try_from(-top).unwrap_or(0);
        let back = pushed
            .min(blank_below_cursor(grid))
            .min(grid.history_size());
        if back == 0 {
            return;
        }
        // Growing the screen takes rows from the history above it; shrinking it again drops
        // the blank rows at the bottom.
        let (lines, columns) = (grid.screen_lines(), grid.columns());
        grid.resize(false, lines + back, columns);
        grid.resize(false, lines, columns);
    }
}

/// The start of the line `count` lines above the line that starts at `from`.
fn nth_start_above(grid: &Grid<Cell>, from: Line, count: usize) -> Option<Line> {
    let mut seen = 0;
    let mut line = from;
    while seen < count {
        if line <= grid.topmost_line() {
            return None;
        }
        line -= 1;
        if !continues(grid, line) {
            seen += 1;
        }
    }
    Some(line)
}

/// How many blank rows the screen has below the cursor, counted up from the bottom.
fn blank_below_cursor(grid: &Grid<Cell>) -> usize {
    let cursor = grid.cursor.point.line.0;
    let mut line = grid.bottommost_line();
    let mut blank = 0;
    while line.0 > cursor && grid[line].is_clear() {
        blank += 1;
        line -= 1;
    }
    blank
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::Term;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::Column;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::core::pump::TermSize;

    fn term(columns: usize, lines: usize, text: &str) -> Term<VoidListener> {
        let mut term = Term::new(
            Config::default(),
            &TermSize::new(columns, lines),
            VoidListener,
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    fn screen(term: &Term<VoidListener>) -> Vec<String> {
        let grid = term.grid();
        (0..grid.screen_lines())
            .map(|line| {
                let row = &grid[Line(line as i32)];
                let text: String = (0..grid.columns())
                    .map(|column| row[Column(column)].c)
                    .collect();
                text.trim_end().to_owned()
            })
            .collect()
    }

    fn resize(term: &mut Term<VoidListener>, columns: usize, lines: usize) {
        let top = Top::of(term.grid());
        term.resize(TermSize::new(columns, lines));
        if let Some(top) = top {
            top.keep(term.grid_mut());
        }
    }

    const LONG: &str = "a line as wide as the grid is wide";

    #[test]
    fn a_narrower_grid_grows_downward_into_the_blank_rows() {
        let mut term = term(40, 10, &format!("first\r\n{LONG}\r\n{LONG}\r\n$ "));
        resize(&mut term, 20, 10);
        let screen = screen(&term);
        assert_eq!(screen[0], "first", "{screen:?}");
        assert_eq!(screen[1], &LONG[..20]);
        assert_eq!(screen[5], "$");
        assert_eq!(term.grid().history_size(), 0);
        assert_eq!(
            term.grid().cursor.point,
            alacritty_terminal::index::Point::new(Line(5), Column(2))
        );
    }

    #[test]
    fn a_full_screen_still_pushes_its_top_into_the_history() {
        let mut term = term(40, 4, &format!("first\r\n{LONG}\r\n{LONG}\r\n$ "));
        resize(&mut term, 20, 4);
        let screen = screen(&term);
        assert_eq!(screen[3], "$", "{screen:?}");
        assert_eq!(term.grid().history_size(), 2);
    }

    #[test]
    fn a_screen_with_few_blank_rows_takes_back_as_many() {
        let mut term = term(40, 5, &format!("first\r\n{LONG}\r\n{LONG}\r\n$ "));
        resize(&mut term, 20, 5);
        let screen = screen(&term);
        assert_eq!(screen[4], "$", "{screen:?}");
        assert_eq!(term.grid().history_size(), 1);
        assert_eq!(screen[0], &LONG[..20]);
    }

    #[test]
    fn a_wider_grid_changes_nothing_more() {
        let mut term = term(20, 10, &format!("first\r\n{LONG}\r\n$ "));
        resize(&mut term, 40, 10);
        let screen = screen(&term);
        assert_eq!(screen[0], "first", "{screen:?}");
        assert_eq!(screen[1], LONG);
        assert_eq!(screen[2], "$");
    }
}
