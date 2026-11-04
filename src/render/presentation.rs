//! Deciding whether a cell is drawn as text or as an emoji.
//!
//! Many symbols have two presentations: a monochrome glyph in the colour of the text, and a
//! colour emoji. Unicode gives each character a default, and two variation selectors override it:
//! U+FE0E asks for text and U+FE0F asks for an emoji. A terminal follows the same rule, so a
//! media symbol such as U+23F8 draws as text in the cell colour, and the same symbol followed by
//! U+FE0F draws as an emoji.

use unicode_properties::{EmojiStatus, UnicodeEmoji};

/// The selector that asks for the text presentation.
const TEXT_SELECTOR: char = '\u{FE0E}';

/// The selector that asks for the emoji presentation.
const EMOJI_SELECTOR: char = '\u{FE0F}';

/// Whether the cell holding `text` is drawn as an emoji.
///
/// `text` is one cell: a base character and the marks that belong to it.
pub fn is_emoji(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(base) = characters.next() else {
        return false;
    };
    for mark in characters {
        match mark {
            TEXT_SELECTOR => return false,
            EMOJI_SELECTOR => return true,
            _ => {}
        }
    }
    has_emoji_default(base)
}

/// Whether `character` is drawn as an emoji when nothing selects a presentation.
fn has_emoji_default(character: char) -> bool {
    matches!(
        character.emoji_status(),
        EmojiStatus::EmojiPresentation
            | EmojiStatus::EmojiPresentationAndModifierBase
            | EmojiStatus::EmojiPresentationAndEmojiComponent
            | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_media_symbols_of_a_mode_line_are_text() {
        for symbol in [
            '⏴', '⏵', '⏶', '⏷', '⏸', '⏹', '⏺', '⏻', '▶', '◀', '⚙', '✔', '❯',
        ] {
            assert!(!is_emoji(&symbol.to_string()), "{symbol:?}");
        }
    }

    #[test]
    fn a_character_whose_default_is_an_emoji_is_an_emoji() {
        for symbol in ['😀', '⏩', '⌛', '✅', '🚀', '⭐'] {
            assert!(is_emoji(&symbol.to_string()), "{symbol:?}");
        }
    }

    #[test]
    fn the_emoji_selector_makes_a_text_symbol_an_emoji() {
        assert!(is_emoji("⏸\u{FE0F}"));
        assert!(is_emoji("❤\u{FE0F}"));
    }

    #[test]
    fn the_text_selector_makes_an_emoji_text() {
        assert!(!is_emoji("⌛\u{FE0E}"));
    }

    #[test]
    fn letters_and_box_drawing_are_text() {
        for character in ['a', '0', '#', '─', '│', '\u{e0b0}', '世'] {
            assert!(!is_emoji(&character.to_string()), "{character:?}");
        }
    }
}
