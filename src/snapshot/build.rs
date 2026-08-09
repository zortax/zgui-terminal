//! Turning a grid row into styled runs.

use alacritty_terminal::index::Column;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use compact_str::CompactString;
use smallvec::SmallVec;

use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::{ColorPalette, DIM_AMOUNT, Rgba};
use crate::snapshot::{CellRun, HyperlinkId, RowSnapshot, RunFlags, RunStyle};

/// The identifier every link keeps, for as long as the terminal lives.
///
/// Shared, because a row can be built on either thread: the one that owns the emulator, and the
/// one that draws. Two tables would give one link two identifiers, and the row drawn from the
/// second would not light up with the row drawn from the first.
#[derive(Debug, Default)]
pub struct Hyperlinks {
    known: rustc_hash::FxHashMap<String, HyperlinkId>,
    next: u64,
}

impl Hyperlinks {
    /// The identifier `uri` has, giving it one if it is new.
    fn id(&mut self, uri: &str) -> HyperlinkId {
        if let Some(id) = self.known.get(uri) {
            return *id;
        }
        let id = HyperlinkId(self.next);
        self.next += 1;
        self.known.insert(uri.to_owned(), id);
        id
    }
}

/// Builds rows of runs from grid cells.
///
/// The builder holds the palette and a table of the links it has seen, so a link keeps one
/// identifier for as long as the terminal lives.
pub struct RowBuilder {
    palette: ColorPalette,
    /// The colours a program set while it was running.
    ///
    /// These win over everything: a program that set a colour has said what it wants, and a
    /// terminal that drew something else would be answering a question it was not asked.
    overrides: Colors,
    hyperlinks: Arc<Mutex<Hyperlinks>>,
}

impl RowBuilder {
    /// A builder drawing with `palette`.
    pub fn new(palette: ColorPalette) -> Self {
        Self::sharing(palette, Arc::new(Mutex::new(Hyperlinks::default())))
    }

    /// A builder drawing with `palette` and naming links the same as every other builder sharing
    /// `hyperlinks`.
    pub fn sharing(palette: ColorPalette, hyperlinks: Arc<Mutex<Hyperlinks>>) -> Self {
        Self {
            palette,
            overrides: Colors::default(),
            hyperlinks,
        }
    }

    /// Draws with the colours a program has set, from now on.
    pub fn set_overrides(&mut self, overrides: &Colors) {
        self.overrides = *overrides;
    }

    /// The table of links this builder names them from.
    pub fn hyperlinks(&self) -> Arc<Mutex<Hyperlinks>> {
        Arc::clone(&self.hyperlinks)
    }

    /// Draws with `palette` from now on.
    pub fn set_palette(&mut self, palette: ColorPalette) {
        self.palette = palette;
    }

    /// The palette in force.
    pub fn palette(&self) -> &ColorPalette {
        &self.palette
    }

    /// The identifier `uri` has, giving it one if it is new.
    pub fn hyperlink_id(&mut self, uri: &str) -> HyperlinkId {
        self.hyperlinks.lock().id(uri)
    }

    /// The colour a program set for `name`, when it set one.
    fn overridden(&self, name: NamedColor) -> Option<Rgba> {
        self.overrides[name].map(|rgb| Rgba::rgb(rgb.r, rgb.g, rgb.b))
    }

    /// The colour `color` stands for, as a foreground.
    fn resolve(&self, color: Color, dim: bool, bold: bool) -> Rgba {
        let resolved = match color {
            Color::Named(name) => {
                // A bold cell drawn in a normal palette colour is brightened, which is what
                // terminals have always done with the low eight.
                let name = if bold {
                    match name {
                        NamedColor::Black => NamedColor::BrightBlack,
                        NamedColor::Red => NamedColor::BrightRed,
                        NamedColor::Green => NamedColor::BrightGreen,
                        NamedColor::Yellow => NamedColor::BrightYellow,
                        NamedColor::Blue => NamedColor::BrightBlue,
                        NamedColor::Magenta => NamedColor::BrightMagenta,
                        NamedColor::Cyan => NamedColor::BrightCyan,
                        NamedColor::White => NamedColor::BrightWhite,
                        other => other,
                    }
                } else {
                    name
                };
                self.overridden(name)
                    .unwrap_or_else(|| self.palette.named(name))
            }
            Color::Spec(rgb) => Rgba::rgb(rgb.r, rgb.g, rgb.b),
            Color::Indexed(index) => self.overrides[index as usize]
                .map(|rgb| Rgba::rgb(rgb.r, rgb.g, rgb.b))
                .unwrap_or_else(|| self.palette.indexed(index)),
        };
        if dim {
            resolved.mix(self.palette.background, DIM_AMOUNT)
        } else {
            resolved
        }
    }

    /// The colour `color` stands for, as a background.
    fn resolve_background(&self, color: Color) -> Rgba {
        match color {
            Color::Named(name) => self
                .overridden(name)
                .unwrap_or_else(|| self.palette.named(name)),
            Color::Spec(rgb) => Rgba::rgb(rgb.r, rgb.g, rgb.b),
            Color::Indexed(index) => self.overrides[index as usize]
                .map(|rgb| Rgba::rgb(rgb.r, rgb.g, rgb.b))
                .unwrap_or_else(|| self.palette.indexed(index)),
        }
    }

    /// How one cell looks.
    fn style_of(&mut self, cell: &Cell) -> RunStyle {
        let flags = cell.flags;
        let bold = flags.contains(Flags::BOLD);
        let dim = flags.contains(Flags::DIM);

        let mut foreground = self.resolve(cell.fg, dim, bold);
        let mut background = self.resolve_background(cell.bg);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut foreground, &mut background);
        }
        if flags.contains(Flags::HIDDEN) {
            foreground = background;
        }

        let run_flags = RunFlags::NONE
            .set(RunFlags::BOLD, bold)
            .set(RunFlags::ITALIC, flags.contains(Flags::ITALIC))
            .set(RunFlags::UNDERLINE, flags.contains(Flags::UNDERLINE))
            .set(
                RunFlags::DOUBLE_UNDERLINE,
                flags.contains(Flags::DOUBLE_UNDERLINE),
            )
            .set(RunFlags::UNDERCURL, flags.contains(Flags::UNDERCURL))
            .set(
                RunFlags::DOTTED_UNDERLINE,
                flags.contains(Flags::DOTTED_UNDERLINE),
            )
            .set(
                RunFlags::DASHED_UNDERLINE,
                flags.contains(Flags::DASHED_UNDERLINE),
            )
            .set(RunFlags::STRIKEOUT, flags.contains(Flags::STRIKEOUT))
            .set(RunFlags::WIDE, flags.contains(Flags::WIDE_CHAR));

        let underline_color = cell
            .underline_color()
            .map(|color| self.resolve(color, dim, bold));
        let hyperlink = cell.hyperlink().map(|link| self.hyperlink_id(link.uri()));

        RunStyle {
            foreground,
            background,
            underline_color,
            flags: run_flags,
            hyperlink,
        }
    }

    /// Builds the screen row `row` from `cells`, which are the row's columns in order.
    ///
    /// A cell whose text and colours are the default contributes nothing: the background behind
    /// the grid already draws it, and a run that draws nothing costs a frame nothing.
    pub fn build<'a>(
        &mut self,
        row: u16,
        cells: impl Iterator<Item = (usize, &'a Cell)>,
        wrapped: bool,
    ) -> RowSnapshot {
        let mut runs: SmallVec<[CellRun; 8]> = SmallVec::new();
        let default_background = self.palette.background;

        for (column, cell) in cells {
            let column = column as u16;
            // The second half of a wide character has no glyph of its own; the first half draws
            // across both cells.
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }

            let style = self.style_of(cell);
            let character = if cell.c == '\0' { ' ' } else { cell.c };
            let zero_width = cell.zerowidth().unwrap_or(&[]);
            let blank = character == ' '
                && zero_width.is_empty()
                && style.background == default_background
                && !style.flags.intersects(RunFlags::ANY_UNDERLINE)
                && !style.flags.contains(RunFlags::STRIKEOUT);
            if blank {
                continue;
            }

            let width = if style.flags.contains(RunFlags::WIDE) {
                2
            } else {
                1
            };
            let extends = runs.last().is_some_and(|run: &CellRun| {
                run.style == style && run.column + run.width == column
            });
            if extends {
                let run = runs.last_mut().expect("a run to extend");
                run.text.push(character);
                for mark in zero_width {
                    run.text.push(*mark);
                }
                run.width += width;
            } else {
                let mut text = CompactString::default();
                text.push(character);
                for mark in zero_width {
                    text.push(*mark);
                }
                runs.push(CellRun {
                    column,
                    width,
                    text,
                    style,
                });
            }
        }

        RowSnapshot { row, runs, wrapped }
    }

    /// Builds the screen row `row` from the grid row `cells`.
    pub fn build_row(
        &mut self,
        row: u16,
        cells: &alacritty_terminal::grid::Row<Cell>,
        columns: usize,
    ) -> RowSnapshot {
        // A wrapped line is marked on the cell it ran out of room in, which is its last one.
        let wrapped = columns
            .checked_sub(1)
            .is_some_and(|last| cells[Column(last)].flags.contains(Flags::WRAPLINE));
        let columns = (0..columns).map(|column| (column, &cells[Column(column)]));
        self.build(row, columns, wrapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Rgb;

    fn cell(character: char) -> Cell {
        Cell {
            c: character,
            ..Cell::default()
        }
    }

    fn build(cells: Vec<Cell>) -> RowSnapshot {
        let mut builder = RowBuilder::new(ColorPalette::default());
        let refs: Vec<(usize, &Cell)> = cells.iter().enumerate().collect();
        builder.build(0, refs.into_iter(), false)
    }

    #[test]
    fn plain_text_becomes_one_run() {
        let row = build("hello".chars().map(cell).collect());
        assert_eq!(row.runs.len(), 1);
        assert_eq!(row.runs[0].text, "hello");
        assert_eq!(row.runs[0].column, 0);
        assert_eq!(row.runs[0].width, 5);
    }

    #[test]
    fn default_blanks_are_left_out_and_break_the_run() {
        let mut cells: Vec<Cell> = "ab".chars().map(cell).collect();
        cells.push(cell(' '));
        cells.extend("cd".chars().map(cell));
        let row = build(cells);
        assert_eq!(row.runs.len(), 2);
        assert_eq!(row.runs[0].text, "ab");
        assert_eq!(row.runs[1].text, "cd");
        assert_eq!(row.runs[1].column, 3);
    }

    #[test]
    fn a_style_change_starts_a_new_run() {
        let mut cells: Vec<Cell> = "ab".chars().map(cell).collect();
        let mut bold = cell('c');
        bold.flags.insert(Flags::BOLD);
        cells.push(bold);
        let row = build(cells);
        assert_eq!(row.runs.len(), 2);
        assert!(row.runs[1].style.flags.contains(RunFlags::BOLD));
    }

    #[test]
    fn inverse_swaps_the_two_colours() {
        let mut inverse = cell('x');
        inverse.fg = Color::Spec(Rgb {
            r: 10,
            g: 20,
            b: 30,
        });
        inverse.bg = Color::Spec(Rgb {
            r: 40,
            g: 50,
            b: 60,
        });
        inverse.flags.insert(Flags::INVERSE);
        let row = build(vec![inverse]);
        assert_eq!(row.runs[0].style.foreground, Rgba::rgb(40, 50, 60));
        assert_eq!(row.runs[0].style.background, Rgba::rgb(10, 20, 30));
    }

    #[test]
    fn a_hidden_cell_draws_its_text_in_its_background() {
        let mut hidden = cell('x');
        hidden.bg = Color::Spec(Rgb { r: 7, g: 8, b: 9 });
        hidden.flags.insert(Flags::HIDDEN);
        let row = build(vec![hidden]);
        assert_eq!(row.runs[0].style.foreground, row.runs[0].style.background);
    }

    #[test]
    fn a_wide_character_takes_two_columns_and_its_spacer_takes_none() {
        let mut wide = cell('世');
        wide.flags.insert(Flags::WIDE_CHAR);
        let mut spacer = Cell::default();
        spacer.flags.insert(Flags::WIDE_CHAR_SPACER);
        let row = build(vec![wide, spacer, cell('a')]);
        assert_eq!(row.runs.len(), 2);
        assert_eq!(row.runs[0].width, 2);
        assert_eq!(row.runs[0].text, "世");
        assert_eq!(row.runs[1].column, 2);
    }

    #[test]
    fn zero_width_marks_ride_their_own_cell() {
        let mut base = cell('e');
        base.push_zerowidth('\u{0301}');
        let row = build(vec![base]);
        assert_eq!(row.runs[0].text, "e\u{0301}");
        assert_eq!(row.runs[0].width, 1);
    }

    #[test]
    fn a_coloured_blank_is_kept_because_its_background_shows() {
        let mut painted = cell(' ');
        painted.bg = Color::Spec(Rgb { r: 1, g: 2, b: 3 });
        let row = build(vec![painted]);
        assert_eq!(row.runs.len(), 1);
        assert_eq!(row.runs[0].text, " ");
    }

    #[test]
    fn an_underlined_blank_is_kept_because_its_line_shows() {
        let mut underlined = cell(' ');
        underlined.flags.insert(Flags::UNDERLINE);
        let row = build(vec![underlined]);
        assert_eq!(row.runs.len(), 1);
    }

    #[test]
    fn a_bold_palette_colour_is_brightened() {
        let palette = ColorPalette::default();
        let mut builder = RowBuilder::new(palette);
        let mut bold = cell('x');
        bold.fg = Color::Named(NamedColor::Red);
        bold.flags.insert(Flags::BOLD);
        let cells = [(0usize, &bold)];
        let row = builder.build(0, cells.into_iter(), false);
        assert_eq!(row.runs[0].style.foreground, palette.ansi[9]);
    }

    #[test]
    fn two_builders_sharing_a_table_name_a_link_the_same() {
        // What lets a row be built on either thread: the thread that owns the emulator and the
        // thread that draws agree about which link is which, so hovering one lights up the other.
        use alacritty_terminal::term::cell::Hyperlink as CellLink;
        let mut pump = RowBuilder::new(ColorPalette::default());
        let mut view = RowBuilder::sharing(ColorPalette::default(), pump.hyperlinks());

        let mut cell = cell('a');
        cell.set_hyperlink(Some(CellLink::new(
            None::<String>,
            "https://example.com".to_owned(),
        )));
        let cells = [(0usize, &cell)];
        let from_pump = pump.build(0, cells.into_iter(), false);
        let cells = [(0usize, &cell)];
        let from_view = view.build(1, cells.into_iter(), false);

        assert_eq!(
            from_pump.runs[0].style.hyperlink,
            from_view.runs[0].style.hyperlink
        );
        assert!(from_pump.runs[0].style.hyperlink.is_some());
    }

    #[test]
    fn one_link_keeps_one_identifier() {
        use alacritty_terminal::term::cell::Hyperlink as CellLink;
        let mut builder = RowBuilder::new(ColorPalette::default());
        let mut first = cell('a');
        first.set_hyperlink(Some(CellLink::new(
            None::<String>,
            "https://example.com".to_owned(),
        )));
        let mut second = cell('b');
        second.set_hyperlink(Some(CellLink::new(
            None::<String>,
            "https://example.com".to_owned(),
        )));
        let cells = [(0usize, &first)];
        let one = builder.build(0, cells.into_iter(), false);
        let cells = [(0usize, &second)];
        let two = builder.build(1, cells.into_iter(), false);
        assert_eq!(one.runs[0].style.hyperlink, two.runs[0].style.hyperlink);
        assert!(one.runs[0].style.hyperlink.is_some());
    }
}
