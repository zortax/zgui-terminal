//! Reading the terminal's colours off a style sheet.
//!
//! The colours a terminal draws with are a theme, and a theme in this framework is written in
//! CSS. Naming them as custom properties puts them where the rest of an application's colours
//! are: they inherit, they answer to a class or a dark-mode rule, and two terminals in one window
//! differ by a selector rather than by two configurations.
//!
//! Three things decide a colour, in this order. The configuration is the ground, and is what an
//! application with no style sheet gets. A custom property overrides it, one entry at a time, so
//! naming one colour leaves the rest alone. What a program sets while it runs overrides both,
//! because a program that sets a colour has said what it wants and a terminal that ignored it
//! would be lying to it.

use zgui::canvas::zgui_color::Color;
use zgui_css::ComputedStyle;

use crate::config::{ColorPalette, Rgba};

/// The names each colour is read from, without the `--` a declaration writes.
///
/// The sixteen named colours keep the names a person writing a theme already knows.
const ANSI_NAMES: [&str; 16] = [
    "terminal-black",
    "terminal-red",
    "terminal-green",
    "terminal-yellow",
    "terminal-blue",
    "terminal-magenta",
    "terminal-cyan",
    "terminal-white",
    "terminal-bright-black",
    "terminal-bright-red",
    "terminal-bright-green",
    "terminal-bright-yellow",
    "terminal-bright-blue",
    "terminal-bright-magenta",
    "terminal-bright-cyan",
    "terminal-bright-white",
];

/// What `color` a colour read off a style sheet becomes.
fn to_rgba(color: Color) -> Rgba {
    let [red, green, blue, alpha] = color.to_premultiplied_srgb();
    // The premultiplied form carries the alpha in the channels, so it comes back out of them.
    let restore = |channel: f32| {
        let value = if alpha > 0.0 {
            channel / alpha
        } else {
            channel
        };
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Rgba {
        r: restore(red),
        g: restore(green),
        b: restore(blue),
        a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    }
}

/// The colour the custom property `name` holds, when a rule set one.
fn read(style: &ComputedStyle, name: &str) -> Option<Rgba> {
    zgui_css::values::custom::color(style, name).map(to_rgba)
}

/// `base`, with every colour a style sheet named put over it.
///
/// The element's own `color` and `background-color` are the text and the background, because a
/// terminal is an element and those are what an element's colours mean. Everything else is a
/// custom property.
pub fn from_style(style: &ComputedStyle, base: &ColorPalette) -> ColorPalette {
    let mut palette = *base;

    // The two an element already has words for.
    let current = zgui_css::values::color::to_color(zgui_css::values::color::current(style));
    palette.foreground = to_rgba(current);
    if let Some(background) = read(style, "terminal-background") {
        palette.background = background;
    }
    if let Some(foreground) = read(style, "terminal-foreground") {
        palette.foreground = foreground;
    }

    for (index, name) in ANSI_NAMES.iter().enumerate() {
        if let Some(color) = read(style, name) {
            palette.ansi[index] = color;
        }
    }

    if let Some(color) = read(style, "terminal-cursor") {
        palette.cursor = color;
    }
    if let Some(color) = read(style, "terminal-cursor-text") {
        palette.cursor_text = color;
    }
    if let Some(color) = read(style, "terminal-selection") {
        palette.selection_background = color;
    }
    if let Some(color) = read(style, "terminal-selection-text") {
        palette.selection_foreground = Some(color);
    }
    if let Some(color) = read(style, "terminal-match") {
        palette.match_background = color;
    }
    if let Some(color) = read(style, "terminal-match-current") {
        palette.focused_match_background = color;
    }

    palette
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_colour_has_a_name_of_its_own() {
        let mut sorted = ANSI_NAMES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ANSI_NAMES.len());
    }

    #[test]
    fn the_bright_colours_are_the_second_eight() {
        for name in &ANSI_NAMES[8..] {
            assert!(name.starts_with("terminal-bright-"), "{name}");
        }
        for name in &ANSI_NAMES[..8] {
            assert!(!name.starts_with("terminal-bright-"), "{name}");
        }
    }

    #[test]
    fn an_opaque_colour_survives_the_round_trip() {
        let color = Color::srgb_u8(200, 100, 50, 255);
        assert_eq!(to_rgba(color), Rgba::rgb(200, 100, 50));
    }

    #[test]
    fn a_colour_that_is_partly_clear_keeps_its_channels_and_its_opacity() {
        // Premultiplication carries the alpha into the channels, so a naive read would darken
        // every colour a rule made partly clear.
        let color = Color::srgb_u8(200, 100, 50, 128);
        let read = to_rgba(color);
        assert!((read.r as i16 - 200).abs() <= 2, "red {}", read.r);
        assert!((read.g as i16 - 100).abs() <= 2, "green {}", read.g);
        assert!((read.b as i16 - 50).abs() <= 2, "blue {}", read.b);
        assert!((read.a as i16 - 128).abs() <= 2, "alpha {}", read.a);
    }
}
