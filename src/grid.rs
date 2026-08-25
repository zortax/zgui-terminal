//! Reading what the terminal holds, and drawing over it.
//!
//! Everything here names a line by counting from the oldest one the history holds, so a place in
//! the history and a place on the screen are written the same way. That is the coordinate an
//! application works in when it walks the whole terminal rather than what is on screen.
//!
//! # What this is for
//!
//! An application that answers the keyboard itself — a copy mode, a search of its own, a modal
//! layer — needs three things the emulator does not otherwise give out: the text, a cursor of its
//! own, and a selection it decides. Each is a message to the thread that owns the emulator, so
//! nothing here waits for a program.

use alacritty_terminal::vte::ansi::CursorShape;

/// A place in the grid.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct GridPoint {
    /// Which line, counted from the oldest one the history holds.
    pub line: usize,
    /// Which column, counted in cells.
    pub column: u16,
}

impl GridPoint {
    /// The point at `line` and `column`.
    #[must_use]
    pub const fn new(line: usize, column: u16) -> Self {
        Self { line, column }
    }
}

/// Everything the terminal holds, history and screen alike.
///
/// One line per grid line, wrapped ones included: a line that ran out of room is two lines here,
/// the way it is two lines on the screen.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Contents {
    /// Every line, oldest first, with trailing blanks removed.
    ///
    /// Each cell contributes exactly one character, so counting the width of a prefix answers
    /// which column that prefix ends in. A wide character is one character of width two, and the
    /// cell it covers contributes nothing.
    pub lines: Vec<String>,
    /// Which of them the screen starts at, which is how many lines the history holds.
    pub screen: usize,
    /// Where the program's cursor is.
    pub cursor: GridPoint,
    /// How many columns the grid has.
    pub columns: u16,
}

/// A cursor an application draws in place of the program's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AppCursor {
    /// Where it is.
    pub at: GridPoint,
    /// What it looks like.
    pub shape: CursorShape,
}

impl AppCursor {
    /// A block at `at`, which is what a normal mode draws.
    #[must_use]
    pub const fn block(at: GridPoint) -> Self {
        Self {
            at,
            shape: CursorShape::Block,
        }
    }
}

/// What a region of the grid covers.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SpanKind {
    /// The cells between the two ends, following the lines.
    #[default]
    Cells,
    /// Every line the ends touch, whole.
    Lines,
    /// The rectangle between them.
    Block,
}

/// A region of the grid an application selected.
///
/// Both ends are included, the way a cell a pointer is over is included in what it selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GridSpan {
    /// Where it starts.
    pub from: GridPoint,
    /// Where it ends.
    pub to: GridPoint,
    /// What it covers.
    pub kind: SpanKind,
}

impl GridSpan {
    /// The cells from `from` to `to`.
    #[must_use]
    pub const fn cells(from: GridPoint, to: GridPoint) -> Self {
        Self {
            from,
            to,
            kind: SpanKind::Cells,
        }
    }
}

/// Which part of a pointer gesture an event is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PointerPhase {
    /// A button went down.
    Down,
    /// The pointer moved with a button held.
    Move,
    /// The button came up.
    Up,
}

/// Whether what a person types reaches the program.
///
/// [`Live`](Self::Live) is what a terminal does. [`Held`](Self::Held) is for an application that
/// is reading the keyboard itself and must not have the program read it too: a key, its release
/// and what an input method composes are all kept back. Scrolling, the scroll bar and selecting
/// with the pointer carry on either way, because a mode that reads the terminal still wants them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Input {
    /// Keys, composed text and mouse reports reach the program.
    #[default]
    Live,
    /// None of them do.
    Held,
}

impl Input {
    /// Whether the program is given what a person types.
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }
}

/// What is asked about a pointer before the terminal acts on it.
///
/// The cell it landed on and which part of the gesture it is. Answer `true` to keep it from the
/// terminal, which is what an application that owns the selection does.
pub type PointFilter = Box<dyn Fn(GridPoint, PointerPhase) -> bool>;

/// Reads every line `term` holds, history and screen alike.
///
/// The whole grid is walked, so the cost is the size of the history. A caller that reads often
/// should keep what it built and read again only when the lines have moved.
pub(crate) fn read<T: alacritty_terminal::event::EventListener>(
    term: &alacritty_terminal::Term<T>,
) -> Contents {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::Column;
    use alacritty_terminal::term::cell::Flags;

    let columns = term.columns();
    let history = term.history_size();
    let mut lines = Vec::with_capacity(history + term.screen_lines());

    for line in term.topmost_line().0..=term.bottommost_line().0 {
        let cells = &term.grid()[alacritty_terminal::index::Line(line)];
        let mut text = String::with_capacity(columns);
        for column in 0..columns {
            let cell = &cells[Column(column)];
            // The second half of a wide character has no character of its own; the first half
            // covers both cells, so counting its width answers for both.
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            text.push(if cell.c == '\0' { ' ' } else { cell.c });
            for mark in cell.zerowidth().unwrap_or(&[]) {
                text.push(*mark);
            }
        }
        while text.ends_with(' ') {
            text.pop();
        }
        lines.push(text);
    }

    let cursor = term.grid().cursor.point;
    Contents {
        lines,
        screen: history,
        cursor: GridPoint {
            line: history.saturating_add_signed(cursor.line.0 as isize),
            column: cursor.column.0 as u16,
        },
        columns: columns.min(usize::from(u16::MAX)) as u16,
    }
}

/// Which alacritty line `line` is, counting the history the way the emulator does.
///
/// The emulator numbers the top of the screen zero and the history below it, so a line counted
/// from the oldest one is that many lines past the top of the history.
pub(crate) fn to_line(line: usize, history: usize) -> alacritty_terminal::index::Line {
    alacritty_terminal::index::Line(line as i32 - history as i32)
}
