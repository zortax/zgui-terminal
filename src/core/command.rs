//! What the view asks the emulator to do.

use alacritty_terminal::index::{Direction, Point, Side};
use alacritty_terminal::selection::SelectionType;

use crate::grid::{AppCursor, GridSpan};
use crate::search::{SearchDirection, SearchOptions};
use crate::transport::TerminalSize;

/// Where a scroll should land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollRequest {
    /// This many lines, up when positive.
    Lines(i32),
    /// This many screens, up when positive.
    Pages(i32),
    /// The oldest line the history holds.
    Top,
    /// The newest line.
    Bottom,
}

/// How much of the screen a clear removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearMode {
    /// What is on screen.
    Screen,
    /// The history above it.
    History,
    /// Both.
    All,
}

/// What the view asks the emulator to do.
#[derive(Debug)]
pub(crate) enum TerminalCommand {
    /// Put the screen this many lines above the newest one.
    ScrollTo(usize),
    /// The grid has a new size.
    Resize(TerminalSize),
    /// Move the view.
    Scroll(ScrollRequest),
    /// Start a selection.
    SelectionStart {
        /// Where it starts, in grid coordinates.
        point: Point,
        /// Which half of the cell the pointer is in.
        side: Side,
        /// What kind of selection it is.
        kind: SelectionType,
    },
    /// Move a selection's far end.
    SelectionUpdate {
        /// Where the end is now.
        point: Point,
        /// Which half of the cell the pointer is in.
        side: Side,
    },
    /// Select `span`, as a drag would have.
    Select(GridSpan),
    /// Light these spans until told otherwise. Nothing puts the light out.
    Flash(Vec<GridSpan>),
    /// Drop the selection.
    SelectionClear,
    /// Select everything the grid holds.
    SelectAll,
    /// Read the selection out, for the clipboard `kind`.
    CopySelection(zgui::prelude::ClipboardKind),
    /// Send `text` to the program, bracketed when the program asked for that.
    Paste(String),
    /// Look for `pattern`.
    Search(String, SearchOptions),
    /// Move to the next match.
    SearchStep(SearchDirection),
    /// Count the matches, one chunk at a time.
    SearchCount,
    /// Stop searching.
    SearchClear,
    /// Report the link at `point`, if there is one.
    HyperlinkAt(Option<Point>),
    /// Empty the screen, the history, or both.
    Clear(ClearMode),
    /// Return the emulator to its initial state.
    Reset,
    /// Draw with a new configuration.
    SetConfig(Box<crate::config::TerminalConfig>),
    /// Draw with new colours, leaving everything else as it is.
    SetColors(Box<crate::config::TerminalConfig>),
    /// Draw with the colours of a new colour scheme, leaving the other colours as they are.
    SetScheme(Box<crate::config::SchemeColors>),
    /// Whether the terminal has focus, which the cursor and focus reporting need.
    SetFocus(bool),
    /// Draw this cursor in place of the program's, or the program's again when there is none.
    SetCursor(Option<AppCursor>),
    /// Put this line on screen, counted from the oldest the history holds.
    Reveal(usize),
    /// Send `bytes` to the program.
    Write(Vec<u8>),
    /// Publish every row again, whatever was published before.
    ///
    /// What a view asks for when it starts drawing a terminal that was already running: it holds
    /// no rows at all, and what changes from here would leave the screen half empty.
    Redraw,
    /// Stop the pump.
    Shutdown,
}

/// How far a search looks, and which way.
pub(crate) fn direction_of(direction: SearchDirection) -> Direction {
    match direction {
        SearchDirection::Forward => Direction::Right,
        SearchDirection::Backward => Direction::Left,
    }
}
