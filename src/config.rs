//! What the emulator is set up with.

use std::time::Duration;

use alacritty_terminal::vte::ansi::{CursorShape, NamedColor};
pub use zgui::app::{FontFeature, FontVariation};

/// A colour, as the emulator resolves one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rgba {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Opacity, where 255 is opaque.
    pub a: u8,
}

impl Rgba {
    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A colour from `0xRRGGBB`.
    pub const fn hex(value: u32) -> Self {
        Self::rgb(
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
        )
    }

    /// This colour mixed toward `other` by `amount`, from zero to one.
    pub fn mix(self, other: Self, amount: f32) -> Self {
        let mix = |from: u8, to: u8| {
            (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round() as u8
        };
        Self {
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
            a: self.a,
        }
    }

    /// The same colour at `alpha`, from zero to one.
    pub fn with_alpha(self, alpha: f32) -> Self {
        Self {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }
}

/// The colours the emulator draws with.
///
/// The sixteen named colours are the palette a program addresses by name. Everything above them
/// is the standard 256-colour cube, which is computed rather than stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorPalette {
    /// The background behind everything.
    pub background: Rgba,
    /// The colour text is drawn in.
    pub foreground: Rgba,
    /// The colour the cursor is drawn in.
    pub cursor: Rgba,
    /// The colour text under a block cursor is drawn in.
    pub cursor_text: Rgba,
    /// The background of selected text.
    pub selection_background: Rgba,
    /// The foreground of selected text, when it is forced.
    pub selection_foreground: Option<Rgba>,
    /// The background of a search match.
    pub match_background: Rgba,
    /// The background of the match the search is on.
    pub focused_match_background: Rgba,
    /// The eight normal colours, then the eight bright ones.
    pub ansi: [Rgba; 16],
}

/// The colours a colour scheme names: the text, the background, the cursor and the sixteen named
/// colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemeColors {
    /// The colour text is drawn in.
    pub foreground: Rgba,
    /// The background behind everything.
    pub background: Rgba,
    /// The colour the cursor is drawn in.
    pub cursor: Rgba,
    /// The eight normal colours, then the eight bright ones.
    pub ansi: [Rgba; 16],
}

impl ColorPalette {
    /// This palette with the colours of `scheme` in place of its own.
    #[must_use]
    pub fn with_scheme(self, scheme: SchemeColors) -> Self {
        Self {
            foreground: scheme.foreground,
            background: scheme.background,
            cursor: scheme.cursor,
            ansi: scheme.ansi,
            ..self
        }
    }

    /// Whether the text is lighter than the background, by the Rec. 601 luma.
    pub fn is_dark(&self) -> bool {
        let luma = |color: Rgba| {
            299 * u32::from(color.r) + 587 * u32::from(color.g) + 114 * u32::from(color.b)
        };
        luma(self.background) < luma(self.foreground)
    }
}

impl Default for ColorPalette {
    fn default() -> Self {
        Self {
            background: Rgba::hex(0x1e1e2e),
            foreground: Rgba::hex(0xcdd6f4),
            cursor: Rgba::hex(0xf5e0dc),
            cursor_text: Rgba::hex(0x1e1e2e),
            selection_background: Rgba::hex(0x414458),
            selection_foreground: None,
            match_background: Rgba::hex(0x5b5f77),
            focused_match_background: Rgba::hex(0xf9e2af),
            ansi: [
                Rgba::hex(0x45475a), // black
                Rgba::hex(0xf38ba8), // red
                Rgba::hex(0xa6e3a1), // green
                Rgba::hex(0xf9e2af), // yellow
                Rgba::hex(0x89b4fa), // blue
                Rgba::hex(0xf5c2e7), // magenta
                Rgba::hex(0x94e2d5), // cyan
                Rgba::hex(0xbac2de), // white
                Rgba::hex(0x585b70), // bright black
                Rgba::hex(0xf38ba8), // bright red
                Rgba::hex(0xa6e3a1), // bright green
                Rgba::hex(0xf9e2af), // bright yellow
                Rgba::hex(0x89b4fa), // bright blue
                Rgba::hex(0xf5c2e7), // bright magenta
                Rgba::hex(0x94e2d5), // bright cyan
                Rgba::hex(0xa6adc8), // bright white
            ],
        }
    }
}

impl ColorPalette {
    /// The colour at `index` of the emulator's whole colour table.
    ///
    /// The first 256 entries are the indexed palette. The entries after them are the named
    /// colours in the emulator's own order: the foreground, the background, the cursor, the eight
    /// dim colours, the bright foreground and the dim foreground. A query for any of them reads
    /// the same colour the grid is drawn in.
    pub fn at(&self, index: usize) -> Rgba {
        match u8::try_from(index) {
            Ok(index) => self.indexed(index),
            Err(_) => named_at(index).map_or(self.foreground, |name| self.named(name)),
        }
    }

    /// The colour at `index` of the 256-colour palette.
    pub fn indexed(&self, index: u8) -> Rgba {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                // The 6×6×6 cube, at the levels the standard names.
                const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
                let index = usize::from(index) - 16;
                Rgba::rgb(
                    LEVELS[index / 36],
                    LEVELS[(index / 6) % 6],
                    LEVELS[index % 6],
                )
            }
            232..=255 => {
                let level = 8 + (u32::from(index) - 232) * 10;
                let level = level.min(255) as u8;
                Rgba::rgb(level, level, level)
            }
        }
    }

    /// The colour a named one stands for.
    pub fn named(&self, name: NamedColor) -> Rgba {
        match name {
            NamedColor::Black => self.ansi[0],
            NamedColor::Red => self.ansi[1],
            NamedColor::Green => self.ansi[2],
            NamedColor::Yellow => self.ansi[3],
            NamedColor::Blue => self.ansi[4],
            NamedColor::Magenta => self.ansi[5],
            NamedColor::Cyan => self.ansi[6],
            NamedColor::White => self.ansi[7],
            NamedColor::BrightBlack => self.ansi[8],
            NamedColor::BrightRed => self.ansi[9],
            NamedColor::BrightGreen => self.ansi[10],
            NamedColor::BrightYellow => self.ansi[11],
            NamedColor::BrightBlue => self.ansi[12],
            NamedColor::BrightMagenta => self.ansi[13],
            NamedColor::BrightCyan => self.ansi[14],
            NamedColor::BrightWhite => self.ansi[15],
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimBlack => self.ansi[0].mix(self.background, DIM_AMOUNT),
            NamedColor::DimRed => self.ansi[1].mix(self.background, DIM_AMOUNT),
            NamedColor::DimGreen => self.ansi[2].mix(self.background, DIM_AMOUNT),
            NamedColor::DimYellow => self.ansi[3].mix(self.background, DIM_AMOUNT),
            NamedColor::DimBlue => self.ansi[4].mix(self.background, DIM_AMOUNT),
            NamedColor::DimMagenta => self.ansi[5].mix(self.background, DIM_AMOUNT),
            NamedColor::DimCyan => self.ansi[6].mix(self.background, DIM_AMOUNT),
            NamedColor::DimWhite => self.ansi[7].mix(self.background, DIM_AMOUNT),
            NamedColor::DimForeground => self.foreground.mix(self.background, DIM_AMOUNT),
        }
    }
}

/// The named colour at `index` of the emulator's colour table, for an index past the palette.
fn named_at(index: usize) -> Option<NamedColor> {
    const NAMED: [NamedColor; 13] = [
        NamedColor::Foreground,
        NamedColor::Background,
        NamedColor::Cursor,
        NamedColor::DimBlack,
        NamedColor::DimRed,
        NamedColor::DimGreen,
        NamedColor::DimYellow,
        NamedColor::DimBlue,
        NamedColor::DimMagenta,
        NamedColor::DimCyan,
        NamedColor::DimWhite,
        NamedColor::BrightForeground,
        NamedColor::DimForeground,
    ];
    NAMED.iter().copied().find(|name| *name as usize == index)
}

/// How far a dim colour is moved toward the background.
pub(crate) const DIM_AMOUNT: f32 = 0.4;

/// How the cursor looks and behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorConfig {
    /// The shape a reset returns the cursor to.
    pub shape: CursorShape,
    /// Whether the cursor blinks when the program has not said.
    pub blinking: bool,
    /// How long one blink phase lasts.
    pub blink_interval: Duration,
    /// Whether an unfocused terminal draws the cursor as an outline.
    pub hollow_when_unfocused: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            shape: CursorShape::Block,
            blinking: false,
            blink_interval: Duration::from_millis(600),
            hollow_when_unfocused: true,
        }
    }
}

/// What the emulator is set up with.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalConfig {
    /// The font families to draw cells with, tried in order.
    pub font_family: Vec<String>,
    /// The size to draw at, in CSS pixels.
    pub font_size: f32,
    /// The weight of ordinary text, as a CSS weight from 1 to 1000.
    pub font_weight: u16,
    /// The weight of bold text, as a CSS weight from 1 to 1000.
    pub bold_weight: u16,
    /// Whether the face's ligatures and contextual alternates are shaped.
    ///
    /// Off by default. A ligature takes the cells of the characters it joins only in a face drawn
    /// for that, such as a programming face with ligatures.
    pub ligatures: bool,
    /// OpenType features on top of the ligature switch, in order: stylistic sets, a slashed zero
    /// and the like. A feature named here outranks the switch for the same tag.
    pub font_features: Vec<FontFeature>,
    /// Coordinates on the axes of a variable face, in order. An axis named here outranks the one
    /// the weight implies.
    pub font_variations: Vec<FontVariation>,
    /// How tall a line is, as a multiple of the face's own line height.
    pub line_height: f32,
    /// Extra space between cells, in CSS pixels.
    pub letter_spacing: f32,
    /// The padding around the grid, in CSS pixels.
    pub padding: f32,
    /// The colours to draw with.
    pub colors: ColorPalette,
    /// How the cursor looks.
    pub cursor: CursorConfig,
    /// How many lines of history to keep.
    pub scrollback: usize,
    /// How many lines one notch of the wheel moves.
    pub scroll_multiplier: u8,
    /// Whether selecting text puts it on the primary selection.
    ///
    /// On by default, because that is how a terminal behaves where a primary selection exists,
    /// and putting text there does nothing anywhere else.
    pub copy_on_select: bool,
    /// Whether the view returns to the bottom when a key is pressed.
    pub scroll_to_bottom_on_input: bool,
    /// Whether the view returns to the bottom when the program writes.
    pub scroll_to_bottom_on_output: bool,
    /// Whether box drawing and powerline glyphs are drawn from paths rather than from the font.
    pub builtin_glyphs: bool,
    /// Whether the kitty keyboard protocol is offered.
    pub kitty_keyboard: bool,
    /// The characters that end a word, for selection by word.
    pub semantic_escape_chars: String,
    /// Whether a program may write to the clipboard through OSC 52.
    pub allow_clipboard_write: bool,
    /// Whether a program may read the clipboard through OSC 52.
    ///
    /// Off by default: a program that reads the clipboard can send it anywhere.
    pub allow_clipboard_read: bool,
    /// Whether pressing Alt sends an escape before the key.
    pub alt_sends_escape: bool,
    /// How long the pump collects work before it publishes a frame.
    pub coalesce_window: Duration,
    /// How many messages one publish may collect.
    pub coalesce_limit: usize,
    /// How long a new size rests before the grid and the program take it.
    ///
    /// A drag passes through many sizes. Each size the program hears of makes it draw again, and
    /// each one the grid takes reflows the history. Only the size the drag rests at is applied.
    /// The first size a terminal is given applies at once.
    pub resize_settle: Duration,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            font_family: vec!["monospace".to_owned()],
            font_size: 14.0,
            font_weight: 400,
            bold_weight: 700,
            ligatures: false,
            font_features: Vec::new(),
            font_variations: Vec::new(),
            line_height: 1.2,
            letter_spacing: 0.0,
            padding: 6.0,
            colors: ColorPalette::default(),
            cursor: CursorConfig::default(),
            scrollback: 10_000,
            scroll_multiplier: 3,
            copy_on_select: true,
            scroll_to_bottom_on_input: true,
            scroll_to_bottom_on_output: false,
            builtin_glyphs: true,
            kitty_keyboard: true,
            semantic_escape_chars: ",│`|:\"' ()[]{}<>\t".to_owned(),
            allow_clipboard_write: true,
            allow_clipboard_read: false,
            alt_sends_escape: true,
            coalesce_window: Duration::from_millis(4),
            coalesce_limit: 256,
            resize_settle: Duration::from_millis(60),
        }
    }
}

impl TerminalConfig {
    /// What the emulator core is built from.
    pub(crate) fn term_config(&self) -> alacritty_terminal::term::Config {
        alacritty_terminal::term::Config {
            scrolling_history: self.scrollback,
            default_cursor_style: alacritty_terminal::vte::ansi::CursorStyle {
                shape: self.cursor.shape,
                blinking: self.cursor.blinking,
            },
            vi_mode_cursor_style: None,
            semantic_escape_chars: self.semantic_escape_chars.clone(),
            kitty_keyboard: self.kitty_keyboard,
            osc52: match (self.allow_clipboard_write, self.allow_clipboard_read) {
                (true, true) => alacritty_terminal::term::Osc52::CopyPaste,
                (true, false) => alacritty_terminal::term::Osc52::OnlyCopy,
                (false, true) => alacritty_terminal::term::Osc52::OnlyPaste,
                (false, false) => alacritty_terminal::term::Osc52::Disabled,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_colour_cube_lands_on_the_standard_levels() {
        let palette = ColorPalette::default();
        assert_eq!(palette.indexed(16), Rgba::rgb(0, 0, 0));
        assert_eq!(palette.indexed(231), Rgba::rgb(255, 255, 255));
        assert_eq!(palette.indexed(196), Rgba::rgb(255, 0, 0));
    }

    #[test]
    fn the_grey_ramp_runs_from_eight_to_the_top() {
        let palette = ColorPalette::default();
        assert_eq!(palette.indexed(232), Rgba::rgb(8, 8, 8));
        assert_eq!(palette.indexed(255), Rgba::rgb(238, 238, 238));
    }

    #[test]
    fn the_low_sixteen_are_the_named_palette() {
        let palette = ColorPalette::default();
        for index in 0..16u8 {
            assert_eq!(palette.indexed(index), palette.ansi[usize::from(index)]);
        }
    }

    #[test]
    fn clipboard_permission_chooses_the_osc_mode() {
        let mut config = TerminalConfig::default();
        assert_eq!(
            config.term_config().osc52,
            alacritty_terminal::term::Osc52::OnlyCopy
        );
        config.allow_clipboard_read = true;
        assert_eq!(
            config.term_config().osc52,
            alacritty_terminal::term::Osc52::CopyPaste
        );
        config.allow_clipboard_write = false;
        assert_eq!(
            config.term_config().osc52,
            alacritty_terminal::term::Osc52::OnlyPaste
        );
    }
}
