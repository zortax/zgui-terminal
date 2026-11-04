//! Keeping the prompt a shell edits out of the reflow of a resize.
//!
//! A shell draws its prompt again when the grid changes size. It moves the cursor up to where it
//! believes the prompt starts and draws from there. A reflow changes how many rows the prompt
//! takes, so the shell starts too low and leaves part of the old prompt above the new one, or
//! starts too high and draws over the output above it.
//!
//! The semantic prompt marks of shell integration tell the emulator where the prompt starts. On a
//! resize the rows from there to the bottom stay out of the reflow, the cursor stays the same
//! number of rows below the prompt start, and the old rows come back unreflowed. The shell then
//! draws the prompt over them at the new width.
//!
//! A view scrolled into the history keeps the line at its top across the resize.

use alacritty_terminal::Term;
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};

use crate::core::anchor::Anchor;
use crate::core::prefilter::PromptMark;

/// Where the prompt the shell edits now starts.
#[derive(Debug, Default)]
pub(crate) struct PromptTracker {
    /// The line the prompt starts on, counted from the oldest line of the history. Absent while
    /// a command runs or when no shell marks its prompt.
    start: Option<usize>,
}

impl PromptTracker {
    /// Follows `mark`, which the program wrote with the cursor where `term` has it.
    pub fn mark<T>(&mut self, mark: PromptMark, term: &Term<T>) {
        match mark {
            PromptMark::PromptStart { redraws } => {
                self.start = (redraws && !term.mode().contains(TermMode::ALT_SCREEN))
                    .then(|| term.history_size() + cursor_line(term));
            }
            PromptMark::OutputStart | PromptMark::CommandEnd => self.start = None,
            PromptMark::InputStart => {}
        }
    }

    /// The screen line the prompt starts on, when the cursor is in a prompt on the screen.
    ///
    /// Lines that scroll off the top while the history is full leave the count unchanged, so
    /// the answer is never below the cursor.
    fn start_on_screen<T>(&self, term: &Term<T>) -> Option<usize> {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        let line = self.start?.checked_sub(term.history_size())?;
        Some(line.min(cursor_line(term)))
    }

    /// Gives `term` the size `size`, with the prompt kept out of the reflow.
    pub fn resize<T, S: Dimensions>(&mut self, term: &mut Term<T>, size: S) {
        let Some(start) = self.start_on_screen(term) else {
            let anchor = Anchor::of(term.grid());
            term.resize(size);
            if let Some(anchor) = anchor {
                anchor.restore(term.grid_mut());
            }
            return;
        };

        let grid = term.grid_mut();
        let cursor = grid.cursor.point;
        let above = cursor.line.0.max(0) as usize - start;
        let old_columns = grid.columns();
        let old_lines = grid.screen_lines();

        // The rows of the prompt leave the grid blank, so the reflow moves them as single rows
        // and the cursor stays `above` rows below the start.
        let template = Cell::default();
        let rows: Vec<Row<Cell>> = (start..old_lines)
            .map(|line| {
                let row = &mut grid[Line(line as i32)];
                let kept = row.clone();
                row.reset(&template);
                kept
            })
            .collect();
        // The line before the prompt no longer wraps into it.
        if start > 0
            && let Some(cell) = grid[Line(start as i32 - 1)].last_mut()
        {
            cell.flags.remove(Flags::WRAPLINE);
        }
        grid.cursor.point.column = Column(0);
        grid.cursor.input_needs_wrap = false;

        // Read with the prompt rows blank, which is how the grid reflows them.
        let anchor = Anchor::of(term.grid());
        term.resize(size);

        let grid = term.grid_mut();
        let columns = grid.columns();
        let lines = grid.screen_lines();
        let top = (grid.cursor.point.line.0.max(0) as usize).saturating_sub(above);
        let kept_columns = columns.min(old_columns);
        for (line, row) in (top..lines).zip(&rows) {
            let target = &mut grid[Line(line as i32)];
            for column in 0..kept_columns {
                let mut cell = row[Column(column)].clone();
                cell.flags.remove(Flags::WRAPLINE);
                target[Column(column)] = cell;
            }
            // Half a wide character is no character.
            let last = &mut target[Column(kept_columns - 1)];
            if kept_columns < old_columns && last.flags.contains(Flags::WIDE_CHAR) {
                *last = Cell::default();
            }
        }
        grid.cursor.point.column = Column(cursor.column.0.min(columns - 1));
        if let Some(anchor) = anchor {
            anchor.restore(grid);
        }
        self.start = Some(term.history_size() + top);
    }
}

/// The screen line the cursor is on.
fn cursor_line<T>(term: &Term<T>) -> usize {
    term.grid().cursor.point.line.0.max(0) as usize
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::core::prefilter::{Piece, Prefilter};
    use crate::core::pump::TermSize;

    /// A terminal, the parser that feeds it, and the prompt it follows.
    struct Shell {
        term: Term<VoidListener>,
        parser: Processor,
        filter: Prefilter,
        prompt: PromptTracker,
    }

    impl Shell {
        fn new(columns: usize, lines: usize) -> Self {
            Self {
                term: Term::new(
                    Config::default(),
                    &TermSize::new(columns, lines),
                    VoidListener,
                ),
                parser: Processor::new(),
                filter: Prefilter::new(),
                prompt: PromptTracker::default(),
            }
        }

        fn write(&mut self, bytes: &[u8]) {
            let Self {
                term,
                parser,
                filter,
                prompt,
            } = self;
            filter.filter(bytes, &mut |piece| match piece {
                Piece::Bytes(bytes) => parser.advance(term, bytes),
                Piece::Mark(mark) => prompt.mark(mark, term),
            });
        }

        fn resize(&mut self, columns: usize, lines: usize) {
            self.prompt
                .resize(&mut self.term, TermSize::new(columns, lines));
        }

        /// What zsh does on a resize: up to the start of the prompt, clear below, draw it again.
        fn redraw(&mut self, rows_above_cursor: usize, prompt: &str) {
            let mut bytes = Vec::new();
            if rows_above_cursor > 0 {
                bytes.extend_from_slice(format!("\x1b[{rows_above_cursor}A").as_bytes());
            }
            bytes.extend_from_slice(b"\r\x1b[J\x1b]133;A\x07");
            bytes.extend_from_slice(prompt.as_bytes());
            self.write(&bytes);
        }

        fn screen(&self) -> Vec<String> {
            let grid = self.term.grid();
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

        fn cursor(&self) -> (usize, usize) {
            let point = self.term.grid().cursor.point;
            (point.line.0 as usize, point.column.0)
        }
    }

    /// A prompt as wide as the grid, so a narrower grid wraps it.
    const PROMPT: &str = "user@host ~/a/long/path $ typed";

    #[test]
    fn a_narrower_grid_keeps_the_rows_above_the_cursor_the_shell_counts() {
        let mut shell = Shell::new(40, 6);
        shell.write(b"output\r\n\x1b]133;A\x07");
        shell.write(PROMPT.as_bytes());
        assert_eq!(shell.cursor(), (1, PROMPT.len()));

        shell.resize(20, 6);
        // The cursor is as many rows below the prompt start as before, which is the count the
        // shell moves up by.
        assert_eq!(shell.cursor().0, 1);
        shell.redraw(0, PROMPT);
        let screen = shell.screen();
        assert_eq!(screen[0], "output");
        assert_eq!(screen[1], &PROMPT[..20]);
        assert_eq!(screen[2], &PROMPT[20..]);
        assert!(screen[3..].iter().all(String::is_empty), "{screen:?}");
    }

    #[test]
    fn without_marks_the_reflow_leaves_the_old_prompt_behind() {
        // The behaviour the marks exist to avoid, so the test above is known to be about it.
        let mut shell = Shell::new(40, 6);
        shell.write(b"output\r\n");
        shell.write(PROMPT.as_bytes());
        shell.resize(20, 6);
        shell.redraw(0, PROMPT);
        let screen = shell.screen();
        assert_eq!(
            screen
                .iter()
                .filter(|line| line.starts_with("user"))
                .count(),
            2
        );
    }

    #[test]
    fn a_wider_grid_keeps_the_output_above_the_prompt() {
        let mut shell = Shell::new(20, 6);
        shell.write(b"output\r\n\x1b]133;A\x07");
        shell.write(PROMPT.as_bytes());
        // The prompt wrapped once, so the shell counts one row above the cursor.
        assert_eq!(shell.cursor().0, 2);

        shell.resize(40, 6);
        shell.redraw(1, PROMPT);
        let screen = shell.screen();
        assert_eq!(screen[0], "output");
        assert_eq!(screen[1], PROMPT);
        assert!(screen[2..].iter().all(String::is_empty), "{screen:?}");
    }

    #[test]
    fn the_old_prompt_stays_until_the_shell_draws_the_new_one() {
        let mut shell = Shell::new(40, 6);
        shell.write(b"output\r\n\x1b]133;A\x07");
        shell.write(PROMPT.as_bytes());
        shell.resize(20, 6);
        let screen = shell.screen();
        assert_eq!(screen[1], &PROMPT[..20]);
    }

    #[test]
    fn output_after_the_prompt_reflows_as_usual() {
        let mut shell = Shell::new(40, 6);
        shell.write(b"\x1b]133;A\x07$ cmd\r\n\x1b]133;C\x07");
        shell.write(PROMPT.as_bytes());
        shell.resize(20, 6);
        let screen = shell.screen();
        let first = screen.iter().position(|line| line == &PROMPT[..20]);
        let first = first.unwrap_or_else(|| panic!("{screen:?}"));
        assert_eq!(screen[first + 1], &PROMPT[20..]);
    }

    #[test]
    fn the_prompt_start_follows_the_resize() {
        let mut shell = Shell::new(40, 6);
        shell.write(b"output\r\n\x1b]133;A\x07");
        shell.write(PROMPT.as_bytes());
        shell.resize(20, 6);
        shell.resize(40, 6);
        assert_eq!(shell.cursor().0, 1);
        shell.redraw(0, PROMPT);
        let screen = shell.screen();
        assert_eq!(screen[0], "output");
        assert_eq!(screen[1], PROMPT);
    }

    #[test]
    fn a_prompt_at_the_bottom_of_a_full_screen_stays_whole() {
        let mut shell = Shell::new(40, 4);
        shell.write(b"1\r\n2\r\n3\r\n\x1b]133;A\x07");
        shell.write(PROMPT.as_bytes());
        shell.resize(20, 4);
        assert_eq!(shell.cursor().0, 3);
        shell.redraw(0, PROMPT);
        let screen = shell.screen();
        assert_eq!(screen[2], &PROMPT[..20]);
        assert_eq!(screen[3], &PROMPT[20..]);
        assert_eq!(
            screen
                .iter()
                .filter(|line| line.starts_with("user"))
                .count(),
            1
        );
    }
}
