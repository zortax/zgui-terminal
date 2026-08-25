//! Turning a row into the pieces that are drawn.
//!
//! What the emulator publishes is grouped by what a program said. What the renderer needs is
//! grouped by what has to be drawn at once: a stretch of text in one face and one colour, a
//! rectangle of background, a character built from paths. This is where one becomes the other.

use smallvec::SmallVec;

use crate::config::{ColorPalette, Rgba};
use crate::render::boxchars;
use crate::snapshot::{CursorSnapshot, MatchKind, RowOverlay, RowSnapshot, RunFlags, RunStyle};

/// A stretch of background of one colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackgroundRun {
    /// The column it starts at.
    pub column: u16,
    /// How many columns it covers.
    pub width: u16,
    /// What colour it is.
    pub color: Rgba,
}

/// A character that is drawn from paths rather than from the font.
#[derive(Clone, Debug)]
pub struct BuiltinRun {
    /// The column it sits in.
    pub column: u16,
    /// How it is drawn.
    pub builtin: boxchars::Builtin,
    /// What colour it is drawn in.
    pub color: Rgba,
}

/// A stretch of text in one face and one colour.
#[derive(Clone, Debug)]
pub struct TextRun {
    /// The column it starts at.
    pub column: u16,
    /// The text.
    pub text: compact_str::CompactString,
    /// What colour it is drawn in.
    pub color: Rgba,
    /// Whether it is drawn heavier.
    pub bold: bool,
    /// Whether it is drawn sloped.
    pub italic: bool,
}

/// A line drawn under or through a stretch of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecorationRun {
    /// The column it starts at.
    pub column: u16,
    /// How many columns it covers.
    pub width: u16,
    /// Which line it is.
    pub kind: DecorationKind,
    /// What colour it is drawn in.
    pub color: Rgba,
}

/// Which line a decoration is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    /// One line below the text.
    Underline,
    /// Two lines below it.
    DoubleUnderline,
    /// A wave below it.
    Undercurl,
    /// A dotted line below it.
    DottedUnderline,
    /// A dashed line below it.
    DashedUnderline,
    /// A line through it.
    Strikeout,
}

/// Everything one row draws.
#[derive(Clone, Debug, Default)]
pub struct RowPieces {
    /// The rectangles of background, in the order they are drawn.
    pub backgrounds: SmallVec<[BackgroundRun; 8]>,
    /// The characters built from paths.
    pub builtins: SmallVec<[BuiltinRun; 4]>,
    /// The stretches of text.
    pub texts: SmallVec<[TextRun; 8]>,
    /// The lines under and through the text.
    pub decorations: SmallVec<[DecorationRun; 4]>,
    /// The cursor, when it is on this row.
    pub cursor: Option<CursorSnapshot>,
    /// What colour the cursor is drawn in.
    pub cursor_color: Rgba,
}

/// What the pieces of a row are worked out against.
#[derive(Clone, Copy, Debug)]
pub struct RowContext<'a> {
    /// The colours in force.
    pub palette: &'a ColorPalette,
    /// How wide the grid is.
    pub columns: u16,
    /// The size of a cell, for the built-in characters.
    pub cell: (u32, u32),
    /// Whether built-in characters are drawn from paths.
    pub builtin_glyphs: bool,
    /// Whether the terminal has focus.
    pub focused: bool,
}

/// The pieces `row` draws, with `overlay` over it.
pub fn pieces(row: &RowSnapshot, overlay: &RowOverlay, context: RowContext<'_>) -> RowPieces {
    let mut pieces = RowPieces {
        cursor_color: context.palette.cursor,
        ..RowPieces::default()
    };
    let palette = context.palette;

    // What each cell looks like once the selection, the search and the cursor have had their say.
    // Working cell by cell and grouping afterwards is what keeps the rules in one place.
    let mut cells: Vec<Option<CellLook>> = vec![None; usize::from(context.columns)];
    for run in &row.runs {
        let mut column = run.column;
        for grapheme in graphemes(&run.text) {
            if usize::from(column) >= cells.len() {
                break;
            }
            let width = if run.style.flags.contains(RunFlags::WIDE) {
                2
            } else {
                1
            };
            cells[usize::from(column)] = Some(CellLook {
                text: grapheme,
                style: run.style,
                width,
            });
            column += width;
        }
    }

    // A cell with nothing in it is still a cell. The selection and the search highlight the
    // place, so a highlighted empty cell becomes a space and takes the background with the rest.
    for (index, cell) in cells.iter_mut().enumerate() {
        let column = index as u16;
        if cell.is_some() {
            continue;
        }
        let highlighted = overlay
            .selection
            .as_ref()
            .is_some_and(|columns| columns.contains(&column))
            || overlay
                .matches
                .iter()
                .any(|(columns, _)| columns.contains(&column));
        if highlighted {
            *cell = Some(CellLook {
                text: " ".into(),
                style: RunStyle {
                    foreground: palette.foreground,
                    background: palette.background,
                    ..RunStyle::default()
                },
                width: 1,
            });
        }
    }

    for (index, cell) in cells.iter_mut().enumerate() {
        let column = index as u16;
        let Some(look) = cell else { continue };

        // The selection, the light an application put on, and the search decide the background;
        // the cursor decides both. A light is drawn as a selection is, because what it means is
        // the same: these cells are the ones a command was about.
        if let Some(flash) = &overlay.flash
            && flash.contains(&column)
        {
            look.style.background = palette.selection_background;
            if let Some(foreground) = palette.selection_foreground {
                look.style.foreground = foreground;
            }
        }
        if let Some(selection) = &overlay.selection
            && selection.contains(&column)
        {
            look.style.background = palette.selection_background;
            if let Some(foreground) = palette.selection_foreground {
                look.style.foreground = foreground;
            }
        }
        for (columns, kind) in &overlay.matches {
            if columns.contains(&column) {
                look.style.background = match kind {
                    MatchKind::Focused => palette.focused_match_background,
                    MatchKind::Other => palette.match_background,
                };
            }
        }
        // A hovered link is underlined, which is how a person knows it can be followed.
        if overlay.hovered_hyperlink.is_some() && look.style.hyperlink == overlay.hovered_hyperlink
        {
            look.style.flags = look.style.flags.with(RunFlags::UNDERLINE);
        }
    }

    // The cursor, which swaps the colours of the cell it is on so the character stays readable.
    if let Some(cursor) = overlay.cursor {
        pieces.cursor = Some(cursor);
        let solid = matches!(
            cursor.shape,
            alacritty_terminal::vte::ansi::CursorShape::Block
        ) && context.focused;
        if solid && let Some(Some(look)) = cells.get_mut(usize::from(cursor.column)) {
            look.style.foreground = palette.cursor_text;
            look.style.background = palette.cursor;
        }
    }

    // Backgrounds, merged across cells that share a colour.
    let mut background: Option<BackgroundRun> = None;
    for (index, cell) in cells.iter().enumerate() {
        let column = index as u16;
        let color = cell
            .as_ref()
            .map(|look| look.style.background)
            .unwrap_or(palette.background);
        // The colour behind the whole grid is already there; drawing it again costs a quad and
        // shows nothing.
        if color == palette.background {
            pieces.backgrounds.extend(background.take());
            continue;
        }
        match background.as_mut() {
            Some(run) if run.color == color && run.column + run.width == column => {
                run.width += 1;
            }
            _ => {
                pieces.backgrounds.extend(background.take());
                background = Some(BackgroundRun {
                    column,
                    width: 1,
                    color,
                });
            }
        }
    }
    pieces.backgrounds.extend(background.take());

    // The text, the built-in characters and the lines under them.
    let mut text: Option<TextRun> = None;
    let mut decoration: Option<DecorationRun> = None;
    let (cell_width, cell_height) = context.cell;

    for (index, cell) in cells.iter().enumerate() {
        let column = index as u16;
        let Some(look) = cell else {
            pieces.texts.extend(text.take());
            pieces.decorations.extend(decoration.take());
            continue;
        };

        // A character that has to fill its cell exactly is built rather than shaped.
        let builtin = (context.builtin_glyphs && look.text.chars().count() == 1)
            .then(|| look.text.chars().next())
            .flatten()
            .filter(|character| boxchars::covers(*character))
            .and_then(|character| boxchars::glyph(character, cell_width, cell_height));

        if let Some(builtin) = builtin {
            pieces.texts.extend(text.take());
            pieces.builtins.push(BuiltinRun {
                column,
                builtin,
                color: look.style.foreground,
            });
        } else if look.style.foreground == look.style.background {
            // A hidden cell draws nothing at all.
            pieces.texts.extend(text.take());
        } else {
            let bold = look.style.flags.contains(RunFlags::BOLD);
            let italic = look.style.flags.contains(RunFlags::ITALIC);
            let extends = text.as_ref().is_some_and(|run: &TextRun| {
                run.color == look.style.foreground && run.bold == bold && run.italic == italic
            });
            match text.as_mut() {
                Some(run) if extends => run.text.push_str(&look.text),
                _ => {
                    pieces.texts.extend(text.take());
                    text = Some(TextRun {
                        column,
                        text: look.text.clone(),
                        color: look.style.foreground,
                        bold,
                        italic,
                    });
                }
            }
        }

        // The lines under and through the text follow the cells rather than the runs, because a
        // line runs on across a change of colour.
        let kind = decoration_kind(look.style.flags);
        match kind {
            Some(kind) => {
                let color = look.style.underline_color.unwrap_or(look.style.foreground);
                let extends = decoration.as_ref().is_some_and(|run: &DecorationRun| {
                    run.kind == kind && run.color == color && run.column + run.width == column
                });
                match decoration.as_mut() {
                    Some(run) if extends => run.width += look.width,
                    _ => {
                        pieces.decorations.extend(decoration.take());
                        decoration = Some(DecorationRun {
                            column,
                            width: look.width,
                            kind,
                            color,
                        });
                    }
                }
            }
            None => pieces.decorations.extend(decoration.take()),
        }

        // A line through the text is drawn as well as a line under it.
        if look.style.flags.contains(RunFlags::STRIKEOUT) {
            let color = look.style.foreground;
            let last = pieces.decorations.last_mut();
            match last {
                Some(run)
                    if run.kind == DecorationKind::Strikeout
                        && run.color == color
                        && run.column + run.width == column =>
                {
                    run.width += look.width;
                }
                _ => pieces.decorations.push(DecorationRun {
                    column,
                    width: look.width,
                    kind: DecorationKind::Strikeout,
                    color,
                }),
            }
        }
    }
    pieces.texts.extend(text.take());
    pieces.decorations.extend(decoration.take());

    pieces
}

/// What one cell looks like once everything has had its say.
#[derive(Clone, Debug)]
struct CellLook {
    text: compact_str::CompactString,
    style: RunStyle,
    width: u16,
}

/// Which line goes under a cell with these flags.
fn decoration_kind(flags: RunFlags) -> Option<DecorationKind> {
    // One line at a time, in the order a program that sets several would expect.
    if flags.contains(RunFlags::UNDERCURL) {
        Some(DecorationKind::Undercurl)
    } else if flags.contains(RunFlags::DOUBLE_UNDERLINE) {
        Some(DecorationKind::DoubleUnderline)
    } else if flags.contains(RunFlags::DOTTED_UNDERLINE) {
        Some(DecorationKind::DottedUnderline)
    } else if flags.contains(RunFlags::DASHED_UNDERLINE) {
        Some(DecorationKind::DashedUnderline)
    } else if flags.contains(RunFlags::UNDERLINE) {
        Some(DecorationKind::Underline)
    } else {
        None
    }
}

/// The text of one cell at a time.
///
/// A cell holds one character and whatever accents were put on it, which belong to it and are
/// drawn with it.
fn graphemes(text: &str) -> impl Iterator<Item = compact_str::CompactString> + '_ {
    let mut characters = text.chars().peekable();
    std::iter::from_fn(move || {
        let base = characters.next()?;
        let mut cell = compact_str::CompactString::default();
        cell.push(base);
        while let Some(next) = characters.peek() {
            // A mark with no width of its own belongs to the character before it.
            if is_zero_width(*next) {
                cell.push(*next);
                characters.next();
            } else {
                break;
            }
        }
        Some(cell)
    })
}

/// Whether `character` takes no room of its own.
fn is_zero_width(character: char) -> bool {
    matches!(character as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x200D)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::CellRun;
    use smallvec::smallvec;

    fn palette() -> ColorPalette {
        ColorPalette::default()
    }

    fn context(palette: &ColorPalette) -> RowContext<'_> {
        RowContext {
            palette,
            columns: 20,
            cell: (9, 19),
            builtin_glyphs: true,
            focused: true,
        }
    }

    fn row(runs: Vec<CellRun>) -> RowSnapshot {
        RowSnapshot {
            row: 0,
            runs: runs.into(),
            wrapped: false,
        }
    }

    fn plain(column: u16, text: &str, palette: &ColorPalette) -> CellRun {
        CellRun {
            column,
            width: text.chars().count() as u16,
            text: text.into(),
            style: RunStyle {
                foreground: palette.foreground,
                background: palette.background,
                ..RunStyle::default()
            },
        }
    }

    #[test]
    fn plain_text_becomes_one_run_and_no_background() {
        let palette = palette();
        let row = row(vec![plain(0, "hello", &palette)]);
        let pieces = pieces(&row, &RowOverlay::default(), context(&palette));
        assert_eq!(pieces.texts.len(), 1);
        assert_eq!(pieces.texts[0].text, "hello");
        // The colour behind the grid is already there, so nothing is drawn for it.
        assert!(pieces.backgrounds.is_empty());
    }

    #[test]
    fn a_coloured_background_is_drawn_and_merged() {
        let palette = palette();
        let mut run = plain(0, "abc", &palette);
        run.style.background = Rgba::rgb(1, 2, 3);
        let pieces = pieces(&row(vec![run]), &RowOverlay::default(), context(&palette));
        assert_eq!(pieces.backgrounds.len(), 1);
        assert_eq!(pieces.backgrounds[0].column, 0);
        assert_eq!(pieces.backgrounds[0].width, 3);
    }

    #[test]
    fn a_gap_between_two_runs_breaks_the_background() {
        let palette = palette();
        let mut first = plain(0, "ab", &palette);
        first.style.background = Rgba::rgb(1, 2, 3);
        let mut second = plain(5, "cd", &palette);
        second.style.background = Rgba::rgb(1, 2, 3);
        let pieces = pieces(
            &row(vec![first, second]),
            &RowOverlay::default(),
            context(&palette),
        );
        assert_eq!(pieces.backgrounds.len(), 2);
        assert_eq!(pieces.backgrounds[1].column, 5);
    }

    #[test]
    fn box_drawing_is_built_rather_than_shaped() {
        let palette = palette();
        let row = row(vec![plain(0, "─┼─", &palette)]);
        let pieces = pieces(&row, &RowOverlay::default(), context(&palette));
        assert_eq!(pieces.builtins.len(), 3);
        assert!(pieces.texts.is_empty());
    }

    #[test]
    fn box_drawing_comes_from_the_font_when_that_is_asked_for() {
        let palette = palette();
        let mut context = context(&palette);
        context.builtin_glyphs = false;
        let row = row(vec![plain(0, "─┼─", &palette)]);
        let pieces = pieces(&row, &RowOverlay::default(), context);
        assert!(pieces.builtins.is_empty());
        assert_eq!(pieces.texts.len(), 1);
    }

    #[test]
    fn a_built_in_character_breaks_the_text_around_it() {
        let palette = palette();
        let row = row(vec![plain(0, "ab─cd", &palette)]);
        let pieces = pieces(&row, &RowOverlay::default(), context(&palette));
        assert_eq!(pieces.texts.len(), 2);
        assert_eq!(pieces.texts[0].text, "ab");
        assert_eq!(pieces.texts[1].text, "cd");
        assert_eq!(pieces.texts[1].column, 3);
    }

    #[test]
    fn a_selection_colours_the_cells_it_covers() {
        let palette = palette();
        let row = row(vec![plain(0, "hello", &palette)]);
        let overlay = RowOverlay {
            selection: Some(1..=3),
            ..RowOverlay::default()
        };
        let pieces = pieces(&row, &overlay, context(&palette));
        assert_eq!(pieces.backgrounds.len(), 1);
        assert_eq!(pieces.backgrounds[0].column, 1);
        assert_eq!(pieces.backgrounds[0].width, 3);
        assert_eq!(pieces.backgrounds[0].color, palette.selection_background);
    }

    #[test]
    fn a_selection_highlights_the_empty_cells_it_covers() {
        // Selecting past the end of a line still shows where the selection is: the empty cells
        // take the selection background the same as the ones with text in them.
        let palette = palette();
        let row = row(vec![plain(0, "ab", &palette)]);
        let overlay = RowOverlay {
            selection: Some(0..=7),
            ..RowOverlay::default()
        };
        let pieces = pieces(&row, &overlay, context(&palette));
        assert_eq!(pieces.backgrounds.len(), 1);
        assert_eq!(pieces.backgrounds[0].column, 0);
        assert_eq!(pieces.backgrounds[0].width, 8);
        assert_eq!(pieces.backgrounds[0].color, palette.selection_background);
    }

    #[test]
    fn the_match_the_search_is_on_is_coloured_differently() {
        let palette = palette();
        let row = row(vec![plain(0, "hello world", &palette)]);
        let overlay = RowOverlay {
            matches: smallvec![(0..=1, MatchKind::Other), (6..=7, MatchKind::Focused)],
            ..RowOverlay::default()
        };
        let pieces = pieces(&row, &overlay, context(&palette));
        let colors: Vec<Rgba> = pieces.backgrounds.iter().map(|run| run.color).collect();
        assert!(colors.contains(&palette.match_background));
        assert!(colors.contains(&palette.focused_match_background));
    }

    #[test]
    fn the_cursor_swaps_the_colours_of_the_cell_it_is_on() {
        let palette = palette();
        let row = row(vec![plain(0, "hello", &palette)]);
        let overlay = RowOverlay {
            cursor: Some(CursorSnapshot {
                column: 2,
                ..CursorSnapshot::default()
            }),
            ..RowOverlay::default()
        };
        let pieces = pieces(&row, &overlay, context(&palette));
        // The cell under the cursor is drawn in the cursor's colours, so it stays readable.
        let cursor_cell = pieces
            .texts
            .iter()
            .find(|run| run.column == 2)
            .expect("the cell under the cursor");
        assert_eq!(cursor_cell.color, palette.cursor_text);
        assert!(
            pieces
                .backgrounds
                .iter()
                .any(|run| run.color == palette.cursor)
        );
    }

    #[test]
    fn an_unfocused_terminal_leaves_the_cell_under_the_cursor_alone() {
        let palette = palette();
        let mut context = context(&palette);
        context.focused = false;
        let row = row(vec![plain(0, "hello", &palette)]);
        let overlay = RowOverlay {
            cursor: Some(CursorSnapshot {
                column: 2,
                ..CursorSnapshot::default()
            }),
            ..RowOverlay::default()
        };
        let pieces = pieces(&row, &overlay, context);
        assert!(
            pieces
                .texts
                .iter()
                .all(|run| run.color == palette.foreground)
        );
    }

    #[test]
    fn an_underline_runs_on_across_a_change_of_colour() {
        let palette = palette();
        let mut first = plain(0, "ab", &palette);
        first.style.flags = RunFlags::NONE.with(RunFlags::UNDERLINE);
        let mut second = plain(2, "cd", &palette);
        second.style.flags = RunFlags::NONE.with(RunFlags::UNDERLINE);
        second.style.foreground = Rgba::rgb(9, 9, 9);
        let pieces = pieces(
            &row(vec![first, second]),
            &RowOverlay::default(),
            context(&palette),
        );
        // The colour differs, so the line is drawn in two pieces rather than one.
        assert_eq!(pieces.decorations.len(), 2);
        assert!(
            pieces
                .decorations
                .iter()
                .all(|run| run.kind == DecorationKind::Underline)
        );
    }

    #[test]
    fn each_underline_style_is_told_apart() {
        for (flag, kind) in [
            (RunFlags::UNDERLINE, DecorationKind::Underline),
            (RunFlags::DOUBLE_UNDERLINE, DecorationKind::DoubleUnderline),
            (RunFlags::UNDERCURL, DecorationKind::Undercurl),
            (RunFlags::DOTTED_UNDERLINE, DecorationKind::DottedUnderline),
            (RunFlags::DASHED_UNDERLINE, DecorationKind::DashedUnderline),
        ] {
            assert_eq!(decoration_kind(RunFlags::NONE.with(flag)), Some(kind));
        }
        assert_eq!(decoration_kind(RunFlags::NONE), None);
    }

    #[test]
    fn a_line_through_the_text_is_drawn_as_well_as_one_under_it() {
        let palette = palette();
        let mut run = plain(0, "ab", &palette);
        run.style.flags = RunFlags::NONE
            .with(RunFlags::UNDERLINE)
            .with(RunFlags::STRIKEOUT);
        let pieces = pieces(&row(vec![run]), &RowOverlay::default(), context(&palette));
        let kinds: Vec<DecorationKind> = pieces.decorations.iter().map(|run| run.kind).collect();
        assert!(kinds.contains(&DecorationKind::Underline));
        assert!(kinds.contains(&DecorationKind::Strikeout));
    }

    #[test]
    fn a_hidden_cell_draws_nothing() {
        let palette = palette();
        let mut run = plain(0, "secret", &palette);
        run.style.foreground = run.style.background;
        let pieces = pieces(&row(vec![run]), &RowOverlay::default(), context(&palette));
        assert!(pieces.texts.is_empty());
    }

    #[test]
    fn accents_stay_with_the_character_they_belong_to() {
        let palette = palette();
        let row = row(vec![plain(0, "e\u{0301}x", &palette)]);
        let pieces = pieces(&row, &RowOverlay::default(), context(&palette));
        assert_eq!(pieces.texts.len(), 1);
        assert_eq!(pieces.texts[0].text, "e\u{0301}x");
    }

    #[test]
    fn a_hovered_link_is_underlined() {
        let palette = palette();
        let link = crate::snapshot::HyperlinkId(7);
        let mut run = plain(0, "click", &palette);
        run.style.hyperlink = Some(link);
        let overlay = RowOverlay {
            hovered_hyperlink: Some(link),
            ..RowOverlay::default()
        };
        let pieces = pieces(&row(vec![run]), &overlay, context(&palette));
        assert_eq!(pieces.decorations.len(), 1);
        assert_eq!(pieces.decorations[0].kind, DecorationKind::Underline);
    }

    #[test]
    fn a_row_wider_than_the_grid_is_cut_off() {
        let palette = palette();
        let mut context = context(&palette);
        context.columns = 3;
        let row = row(vec![plain(0, "abcdefgh", &palette)]);
        let pieces = pieces(&row, &RowOverlay::default(), context);
        assert_eq!(pieces.texts[0].text.chars().count(), 3);
    }
}
