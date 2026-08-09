//! What the renderer draws from: the rows of the viewport, and the state that sits over them.
//!
//! A row is named by where it is on screen, from zero at the top. That is the whole of the
//! addressing: the grid the view draws is exactly the screen, and scrolling moves what the
//! emulator puts on the screen rather than moving the view over a longer document. Nothing here
//! has to agree with a scroll position, which is what makes it hard to get wrong.

mod build;
mod overlay;

use std::ops::RangeInclusive;

use compact_str::CompactString;
use smallvec::SmallVec;

pub use crate::snapshot::build::{Hyperlinks, RowBuilder};
pub use crate::snapshot::overlay::{
    CursorSnapshot, MatchKind, OverlaySnapshot, Preedit, SearchOverlay,
};

use crate::config::Rgba;

/// How large the grid is, and how much history sits above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GridDimensions {
    /// How many columns each row has.
    pub columns: u16,
    /// How many rows the screen has.
    pub screen_lines: u16,
    /// How many lines of history sit above the screen.
    pub history: usize,
    /// How many lines above the newest one the screen starts. Zero is the bottom.
    pub display_offset: usize,
}

/// What a run's cells look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct RunFlags(u16);

impl RunFlags {
    /// Drawn heavier.
    pub const BOLD: Self = Self(1 << 0);
    /// Drawn sloped.
    pub const ITALIC: Self = Self(1 << 1);
    /// One line below the text.
    pub const UNDERLINE: Self = Self(1 << 2);
    /// Two lines below the text.
    pub const DOUBLE_UNDERLINE: Self = Self(1 << 3);
    /// A wave below the text.
    pub const UNDERCURL: Self = Self(1 << 4);
    /// A dotted line below the text.
    pub const DOTTED_UNDERLINE: Self = Self(1 << 5);
    /// A dashed line below the text.
    pub const DASHED_UNDERLINE: Self = Self(1 << 6);
    /// A line through the text.
    pub const STRIKEOUT: Self = Self(1 << 7);
    /// Cells twice as wide as the others.
    pub const WIDE: Self = Self(1 << 8);

    /// Every underline this can carry.
    pub const ANY_UNDERLINE: Self = Self(
        Self::UNDERLINE.0
            | Self::DOUBLE_UNDERLINE.0
            | Self::UNDERCURL.0
            | Self::DOTTED_UNDERLINE.0
            | Self::DASHED_UNDERLINE.0,
    );

    /// Nothing set.
    pub const NONE: Self = Self(0);

    /// Whether every flag in `other` is set here.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any flag in `other` is set here.
    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// This set with `other` added.
    pub fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// This set with `other` removed.
    pub fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// This set with `other` added when `set`, and removed otherwise.
    pub fn set(self, other: Self, set: bool) -> Self {
        if set {
            self.with(other)
        } else {
            self.without(other)
        }
    }
}

/// Which link a run belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HyperlinkId(pub u64);

/// How a run of cells looks, resolved against the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RunStyle {
    /// The colour the text is drawn in.
    pub foreground: Rgba,
    /// The colour behind it.
    pub background: Rgba,
    /// The colour an underline is drawn in, when the program set one.
    pub underline_color: Option<Rgba>,
    /// What the run looks like.
    pub flags: RunFlags,
    /// The link the run belongs to.
    pub hyperlink: Option<HyperlinkId>,
}

impl Default for RunStyle {
    fn default() -> Self {
        Self {
            foreground: Rgba::default(),
            background: Rgba::default(),
            underline_color: None,
            flags: RunFlags::NONE,
            hyperlink: None,
        }
    }
}

/// A stretch of one row whose cells share a style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellRun {
    /// The column the run starts at.
    pub column: u16,
    /// How many columns it covers, counting a wide character as two.
    pub width: u16,
    /// The text, with each cell's zero-width characters attached to it.
    pub text: CompactString,
    /// How it looks.
    pub style: RunStyle,
}

/// One row of the screen, as the renderer draws it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RowSnapshot {
    /// Where the row is on screen, from zero at the top.
    pub row: u16,
    /// The row's cells, grouped by style.
    pub runs: SmallVec<[CellRun; 8]>,
    /// Whether the row continues into the next one, which copying needs.
    pub wrapped: bool,
}

impl RowSnapshot {
    /// An empty row at `row`.
    pub fn empty(row: u16) -> Self {
        Self {
            row,
            runs: SmallVec::new(),
            wrapped: false,
        }
    }

    /// Whether the row has nothing to draw.
    pub fn is_blank(&self) -> bool {
        self.runs.is_empty()
    }
}

/// What is drawn over one row: the parts the emulator does not report as damage.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RowOverlay {
    /// The columns the selection covers.
    pub selection: Option<RangeInclusive<u16>>,
    /// The search matches on this row, and which of them is the current one.
    pub matches: SmallVec<[(RangeInclusive<u16>, MatchKind); 4]>,
    /// Where the cursor is on this row, when it is on it.
    pub cursor: Option<CursorSnapshot>,
    /// The link the pointer is over.
    pub hovered_hyperlink: Option<HyperlinkId>,
}

impl RowOverlay {
    /// Whether nothing is drawn over the row.
    pub fn is_empty(&self) -> bool {
        self.selection.is_none()
            && self.matches.is_empty()
            && self.cursor.is_none()
            && self.hovered_hyperlink.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_add_and_remove_without_touching_the_rest() {
        let flags = RunFlags::NONE.with(RunFlags::BOLD).with(RunFlags::ITALIC);
        assert!(flags.contains(RunFlags::BOLD));
        assert!(flags.contains(RunFlags::ITALIC));
        let flags = flags.without(RunFlags::BOLD);
        assert!(!flags.contains(RunFlags::BOLD));
        assert!(flags.contains(RunFlags::ITALIC));
    }

    #[test]
    fn every_underline_style_counts_as_an_underline() {
        for style in [
            RunFlags::UNDERLINE,
            RunFlags::DOUBLE_UNDERLINE,
            RunFlags::UNDERCURL,
            RunFlags::DOTTED_UNDERLINE,
            RunFlags::DASHED_UNDERLINE,
        ] {
            assert!(
                RunFlags::NONE
                    .with(style)
                    .intersects(RunFlags::ANY_UNDERLINE)
            );
        }
        assert!(
            !RunFlags::NONE
                .with(RunFlags::BOLD)
                .intersects(RunFlags::ANY_UNDERLINE)
        );
    }
}
