//! The state that sits over the rows.
//!
//! Alacritty reports damage for what its parser wrote. The cursor, the selection, the search and
//! the composition are the application's own, so they are compared between frames here instead.
//!
//! Everything is named by where it is on screen, the same way a row is.

use std::ops::RangeInclusive;

use alacritty_terminal::vte::ansi::CursorShape;
use smallvec::SmallVec;

use crate::snapshot::HyperlinkId;

/// Where the cursor is and what it looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorSnapshot {
    /// Which row of the screen the cursor is on.
    pub row: u16,
    /// Which column it is in.
    pub column: u16,
    /// What it looks like.
    pub shape: CursorShape,
    /// Whether the program asked it to blink.
    pub blinking: bool,
    /// Whether it is drawn at all.
    pub visible: bool,
}

impl Default for CursorSnapshot {
    fn default() -> Self {
        Self {
            row: 0,
            column: 0,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
        }
    }
}

/// Which kind of search match a range is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchKind {
    /// One of the matches on screen.
    Other,
    /// The match the search is on.
    Focused,
}

/// What a person is composing, before it is committed.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Preedit {
    /// The provisional text.
    pub text: String,
    /// Which part of it the input method has selected.
    pub cursor: Option<(usize, usize)>,
}

/// The matches the search found on the screen.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SearchOverlay {
    /// Every match, by row, as a range of columns.
    pub matches: SmallVec<[(u16, RangeInclusive<u16>); 16]>,
    /// Which of them the search is on.
    pub focused: Option<usize>,
}

impl SearchOverlay {
    /// Whether the search has nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// The matches on `row`, with the kind each one is.
    pub fn on_row(&self, row: u16) -> SmallVec<[(RangeInclusive<u16>, MatchKind); 4]> {
        self.matches
            .iter()
            .enumerate()
            .filter(|(_, (at, _))| *at == row)
            .map(|(index, (_, columns))| {
                let kind = if self.focused == Some(index) {
                    MatchKind::Focused
                } else {
                    MatchKind::Other
                };
                (columns.clone(), kind)
            })
            .collect()
    }
}

/// The columns `rows` covers on `row`.
fn columns_on(
    rows: &SmallVec<[(u16, RangeInclusive<u16>); 8]>,
    row: u16,
) -> Option<RangeInclusive<u16>> {
    rows.iter()
        .find(|(at, _)| *at == row)
        .map(|(_, columns)| columns.clone())
}

/// Everything drawn over the rows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct OverlaySnapshot {
    /// Where the cursor is.
    pub cursor: CursorSnapshot,
    /// Whether the terminal has focus, which decides whether the cursor is filled or outlined.
    ///
    /// Drawn over the rows and the same answer for all of them, so it travels with the rest of
    /// what is drawn over them rather than beside it. A row is repainted when this arrives, which
    /// is what makes a terminal that is given the keyboard back fill its cursor in at once.
    pub focused: bool,
    /// The rows the selection covers, and the columns on each.
    pub selection: SmallVec<[(u16, RangeInclusive<u16>); 8]>,
    /// The rows the application lit, and the columns on each.
    ///
    /// A layer of its own rather than a second selection, so that lighting what a command took
    /// and selecting what a person is choosing never take the same slot from one another.
    pub flash: SmallVec<[(u16, RangeInclusive<u16>); 8]>,
    /// What the search found.
    pub search: SearchOverlay,
    /// The link the pointer is over.
    pub hovered_hyperlink: Option<HyperlinkId>,
    /// What an input method is composing.
    pub preedit: Option<Preedit>,
}

impl OverlaySnapshot {
    /// The columns the selection covers on `row`.
    pub fn selection_on(&self, row: u16) -> Option<RangeInclusive<u16>> {
        columns_on(&self.selection, row)
    }

    /// The columns the application lit on `row`.
    pub fn flash_on(&self, row: u16) -> Option<RangeInclusive<u16>> {
        columns_on(&self.flash, row)
    }

    /// What is drawn over `row`.
    pub fn for_row(&self, row: u16) -> crate::snapshot::RowOverlay {
        crate::snapshot::RowOverlay {
            focused: self.focused,
            selection: self.selection_on(row),
            flash: self.flash_on(row),
            matches: self.search.on_row(row),
            cursor: (self.cursor.row == row && self.cursor.visible).then_some(self.cursor),
            hovered_hyperlink: self.hovered_hyperlink,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focused_match_is_the_only_one_marked() {
        let search = SearchOverlay {
            matches: SmallVec::from_vec(vec![(3, 0..=4), (3, 10..=14), (7, 2..=6)]),
            focused: Some(1),
        };
        let on_three = search.on_row(3);
        assert_eq!(on_three.len(), 2);
        assert_eq!(on_three[0].1, MatchKind::Other);
        assert_eq!(on_three[1].1, MatchKind::Focused);
        assert_eq!(search.on_row(7)[0].1, MatchKind::Other);
    }

    #[test]
    fn a_hidden_cursor_draws_on_no_row() {
        let overlay = OverlaySnapshot {
            cursor: CursorSnapshot {
                row: 2,
                visible: false,
                ..CursorSnapshot::default()
            },
            ..OverlaySnapshot::default()
        };
        assert!(overlay.for_row(2).cursor.is_none());
    }

    #[test]
    fn the_cursor_is_drawn_on_the_row_it_is_on() {
        let overlay = OverlaySnapshot {
            cursor: CursorSnapshot {
                row: 2,
                column: 5,
                ..CursorSnapshot::default()
            },
            ..OverlaySnapshot::default()
        };
        assert!(overlay.for_row(2).cursor.is_some());
        assert!(overlay.for_row(1).cursor.is_none());
    }
}
