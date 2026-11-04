//! Keeping the line at the top of a scrolled-back view in place across a resize.
//!
//! A view at the bottom follows the bottom, which the emulator does by itself. A view scrolled
//! into the history shows a line a person is reading. The emulator keeps the distance from the
//! bottom in rows, and a reflow changes how many rows each line takes, so the view lands on
//! other text. The anchor counts in lines instead: which line is at the top, counted from the
//! bottom, and how far into it the top row is.

use alacritty_terminal::grid::{Dimensions, Grid, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};

/// Where a scrolled-back view is, in terms a reflow keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Anchor {
    /// How many lines start below the line at the top of the view.
    lines_below: usize,
    /// How many cells of that line come before the top row.
    cells_before: usize,
}

impl Anchor {
    /// The anchor of `grid`'s view, when it is scrolled back.
    pub fn of(grid: &Grid<Cell>) -> Option<Self> {
        let offset = grid.display_offset();
        if offset == 0 {
            return None;
        }
        let top = Line(-(offset as i32));
        let start = line_start(grid, top);
        let lines_below = starts_between(grid, start, grid.bottommost_line());
        let cells_before = (top.0 - start.0) as usize * grid.columns();
        Some(Self {
            lines_below,
            cells_before,
        })
    }

    /// Scrolls `grid` so the anchored row is at the top of the view again.
    pub fn restore(self, grid: &mut Grid<Cell>) {
        let Some(start) = nth_start_from_bottom(grid, self.lines_below) else {
            return;
        };
        let end = line_end(grid, start);
        let rows = self.cells_before / grid.columns().max(1);
        let top = Line((start.0 + rows as i32).min(end.0));
        let wanted = (-top.0).clamp(0, grid.history_size() as i32);
        let current = grid.display_offset() as i32;
        if wanted != current {
            grid.scroll_display(Scroll::Delta(wanted - current));
        }
    }
}

/// Whether the row above `line` wraps into it.
fn continues(grid: &Grid<Cell>, line: Line) -> bool {
    line > grid.topmost_line()
        && grid[line - 1usize][Column(grid.columns() - 1)]
            .flags
            .contains(Flags::WRAPLINE)
}

/// The first row of the line `line` is part of.
fn line_start(grid: &Grid<Cell>, mut line: Line) -> Line {
    while continues(grid, line) {
        line -= 1;
    }
    line
}

/// The last row of the line that starts at `start`.
fn line_end(grid: &Grid<Cell>, mut line: Line) -> Line {
    while line < grid.bottommost_line() && continues(grid, line + 1) {
        line += 1;
    }
    line
}

/// How many lines start after `start`, up to and including `last`.
fn starts_between(grid: &Grid<Cell>, start: Line, last: Line) -> usize {
    let mut count = 0;
    let mut line = start + 1;
    while line <= last {
        if !continues(grid, line) {
            count += 1;
        }
        line += 1;
    }
    count
}

/// The first row of the line with `below` lines starting under it.
fn nth_start_from_bottom(grid: &Grid<Cell>, below: usize) -> Option<Line> {
    let mut seen = 0;
    let mut line = grid.bottommost_line();
    loop {
        if !continues(grid, line) {
            if seen == below {
                return Some(line);
            }
            seen += 1;
        }
        if line <= grid.topmost_line() {
            return None;
        }
        line -= 1;
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::Term;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::core::pump::TermSize;

    fn term(columns: usize, lines: usize) -> Term<VoidListener> {
        let mut term = Term::new(
            Config::default(),
            &TermSize::new(columns, lines),
            VoidListener,
        );
        let mut parser: Processor = Processor::new();
        for index in 0..60 {
            let line = format!("line {index:02} {}\r\n", "x".repeat(index % 37));
            parser.advance(&mut term, line.as_bytes());
        }
        term
    }

    /// The text of the row at the top of the view.
    fn top(term: &Term<VoidListener>) -> String {
        let grid = term.grid();
        let row = &grid[Line(-(grid.display_offset() as i32))];
        let text: String = (0..grid.columns())
            .map(|column| row[Column(column)].c)
            .collect();
        text.trim_end().to_owned()
    }

    fn resized(columns: usize, lines: usize) -> (String, String) {
        let mut term = term(40, 6);
        term.scroll_display(Scroll::Delta(12));
        let before = top(&term);
        let anchor = Anchor::of(term.grid());
        term.resize(TermSize::new(columns, lines));
        if let Some(anchor) = anchor {
            anchor.restore(term.grid_mut());
        }
        (before, top(&term))
    }

    #[test]
    fn a_line_that_wraps_after_a_resize_stays_at_the_top() {
        // Every line wraps once at the narrower width, so the rows below the top double.
        let mut term = Term::new(Config::default(), &TermSize::new(40, 6), VoidListener);
        let mut parser: Processor = Processor::new();
        for index in 0..60 {
            let line = format!("line {index:02} {}\r\n", "y".repeat(22));
            parser.advance(&mut term, line.as_bytes());
        }
        term.scroll_display(Scroll::Delta(12));
        let before = top(&term);
        let anchor = Anchor::of(term.grid()).expect("a scrolled view");
        term.resize(TermSize::new(20, 6));
        anchor.restore(term.grid_mut());
        assert_eq!(top(&term), before[..20]);
    }

    #[test]
    fn a_narrower_grid_keeps_the_line_at_the_top() {
        let (before, after) = resized(20, 6);
        assert!(before.starts_with("line "), "{before}");
        assert_eq!(after, before[..before.len().min(20)]);
    }

    #[test]
    fn a_wider_grid_keeps_the_line_at_the_top() {
        let (before, after) = resized(70, 6);
        assert!(after.starts_with(&before), "{before} / {after}");
    }

    #[test]
    fn a_taller_and_narrower_grid_keeps_the_line_at_the_top() {
        let (before, after) = resized(25, 12);
        assert_eq!(after, before[..before.len().min(25)]);
    }

    #[test]
    fn a_view_at_the_bottom_has_no_anchor() {
        let term = term(40, 6);
        assert_eq!(Anchor::of(term.grid()), None);
    }
}
