//! What each named key is called in an escape sequence.

use zgui::vocab::NamedKey;

/// The shape of the sequence a named key produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyForm {
    /// A sequence ending in a letter, as the arrow keys do: `CSI A`, or `SS3 A`.
    ///
    /// The flag says whether the application cursor mode changes the introducer.
    Letter(u8, bool),
    /// A sequence ending in a tilde, as the editing keys do: `CSI 2 ~`.
    Tilde(u16),
    /// A single byte, as return and tab do.
    Byte(u8),
}

/// How `key` is written, and the number the kitty protocol calls it by.
///
/// The number is what a disambiguated report uses. A key that has no number of its own is
/// reported by its own character.
pub fn form_of(key: NamedKey) -> Option<KeyForm> {
    use KeyForm::{Byte, Letter, Tilde};
    Some(match key {
        NamedKey::ArrowUp => Letter(b'A', true),
        NamedKey::ArrowDown => Letter(b'B', true),
        NamedKey::ArrowRight => Letter(b'C', true),
        NamedKey::ArrowLeft => Letter(b'D', true),
        NamedKey::Home => Letter(b'H', true),
        NamedKey::End => Letter(b'F', true),

        NamedKey::Insert => Tilde(2),
        NamedKey::Delete => Tilde(3),
        NamedKey::PageUp => Tilde(5),
        NamedKey::PageDown => Tilde(6),

        NamedKey::F1 => Letter(b'P', false),
        NamedKey::F2 => Letter(b'Q', false),
        NamedKey::F3 => Letter(b'R', false),
        NamedKey::F4 => Letter(b'S', false),
        NamedKey::F5 => Tilde(15),
        NamedKey::F6 => Tilde(17),
        NamedKey::F7 => Tilde(18),
        NamedKey::F8 => Tilde(19),
        NamedKey::F9 => Tilde(20),
        NamedKey::F10 => Tilde(21),
        NamedKey::F11 => Tilde(23),
        NamedKey::F12 => Tilde(24),
        NamedKey::F13 => Tilde(57),
        NamedKey::F14 => Tilde(58),
        NamedKey::F15 => Tilde(59),
        NamedKey::F16 => Tilde(60),
        NamedKey::F17 => Tilde(61),
        NamedKey::F18 => Tilde(62),
        NamedKey::F19 => Tilde(63),
        NamedKey::F20 => Tilde(64),

        NamedKey::Enter => Byte(b'\r'),
        NamedKey::Tab => Byte(b'\t'),
        NamedKey::Backspace => Byte(0x7f),
        NamedKey::Escape => Byte(0x1b),
        NamedKey::Space => Byte(b' '),
        _ => return None,
    })
}

/// The number the kitty protocol calls `key` by.
///
/// A key whose sequence already says which it is keeps that sequence, so only the keys with no
/// form of their own are listed here.
pub fn functional_number(key: NamedKey) -> Option<u32> {
    Some(match key {
        NamedKey::Escape => 27,
        NamedKey::Enter => 13,
        NamedKey::Tab => 9,
        NamedKey::Backspace => 127,
        NamedKey::CapsLock => 57358,
        NamedKey::ScrollLock => 57359,
        NamedKey::NumLock => 57360,
        NamedKey::PrintScreen => 57361,
        NamedKey::Pause => 57362,
        NamedKey::ContextMenu => 57363,
        NamedKey::Shift => 57441,
        NamedKey::Control => 57442,
        NamedKey::Alt => 57443,
        NamedKey::Super | NamedKey::Meta => 57444,
        NamedKey::Hyper => 57445,
        _ => return None,
    })
}

/// Whether `key` is a modifier rather than something that produces output.
pub fn is_modifier(key: NamedKey) -> bool {
    matches!(
        key,
        NamedKey::Shift
            | NamedKey::Control
            | NamedKey::Alt
            | NamedKey::Meta
            | NamedKey::Super
            | NamedKey::Hyper
            | NamedKey::AltGraph
            | NamedKey::CapsLock
            | NamedKey::NumLock
            | NamedKey::ScrollLock
            | NamedKey::Fn
            | NamedKey::FnLock
            | NamedKey::Symbol
            | NamedKey::SymbolLock
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrow_keys_answer_to_the_application_cursor_mode() {
        assert_eq!(
            form_of(NamedKey::ArrowUp),
            Some(KeyForm::Letter(b'A', true))
        );
        // The function keys use the same introducer whatever the mode.
        assert_eq!(form_of(NamedKey::F1), Some(KeyForm::Letter(b'P', false)));
    }

    #[test]
    fn the_editing_keys_are_numbered() {
        assert_eq!(form_of(NamedKey::Insert), Some(KeyForm::Tilde(2)));
        assert_eq!(form_of(NamedKey::PageDown), Some(KeyForm::Tilde(6)));
    }

    #[test]
    fn a_key_with_no_sequence_of_its_own_says_so() {
        assert!(form_of(NamedKey::Shift).is_none());
    }

    #[test]
    fn the_modifier_keys_are_known_as_modifiers() {
        assert!(is_modifier(NamedKey::Control));
        assert!(!is_modifier(NamedKey::Enter));
    }
}
