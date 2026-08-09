//! Turning a key press into the bytes a program expects.
//!
//! Encoding is a plain function of the key, the modifiers and the emulator's mode, so the whole
//! of it can be tested without a window.

mod kitty;
mod legacy;
mod named;

use alacritty_terminal::term::TermMode;
use smallvec::SmallVec;
use zgui::prelude::Modifiers;
use zgui::vocab::KeyEvent;

pub use crate::input::keyboard::legacy::control_byte;

/// The bytes one key press sends.
pub type Bytes = SmallVec<[u8; 16]>;

/// Whether a key went down or came up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// The key went down.
    Down,
    /// The key came up.
    Up,
}

/// How keys are read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBindings {
    /// Whether alt puts an escape before the key.
    pub alt_sends_escape: bool,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            alt_sends_escape: true,
        }
    }
}

/// The bytes `event` sends to a program in `mode`.
///
/// A key that produces text carries the text itself, so a printable key is encoded here. What a
/// person composes through an input method arrives on its own event and is written separately.
///
/// ```
/// use alacritty_terminal::term::TermMode;
/// use zgui::prelude::Modifiers;
/// use zgui::vocab::{KeyEvent, NamedKey, PhysicalKey};
/// use zgui_terminal::input::{KeyBindings, KeyState, encode_key};
///
/// let event = KeyEvent::named(NamedKey::ArrowUp, PhysicalKey::Unidentified(0));
/// let bytes = encode_key(
///     &event,
///     Modifiers::NONE,
///     KeyState::Down,
///     TermMode::default(),
///     &KeyBindings::default(),
/// );
/// assert_eq!(bytes.as_deref(), Some(&b"\x1b[A"[..]));
/// ```
pub fn encode_key(
    event: &KeyEvent,
    modifiers: Modifiers,
    state: KeyState,
    mode: TermMode,
    bindings: &KeyBindings,
) -> Option<Bytes> {
    if mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        return kitty::encode(
            &event.key,
            &event.key_without_modifiers,
            event.location,
            modifiers,
            state,
            event.repeat,
            mode,
        );
    }
    if state == KeyState::Up {
        return None;
    }
    // The control byte follows from the key the layout produces with nothing held, so that a
    // shortcut works the same on every layout.
    let key = if modifiers.control() {
        &event.key_without_modifiers
    } else {
        &event.key
    };
    legacy::encode(key, event.location, modifiers, mode, bindings)
}

/// Writes `value` as decimal digits.
pub(crate) fn push_number(bytes: &mut Bytes, value: u32) {
    let mut buffer = [0u8; 10];
    let mut length = 0;
    let mut value = value;
    loop {
        buffer[length] = b'0' + (value % 10) as u8;
        value /= 10;
        length += 1;
        if value == 0 {
            break;
        }
    }
    for index in (0..length).rev() {
        bytes.push(buffer[index]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zgui::vocab::{Key, KeyLocation, NamedKey, PhysicalKey};

    fn named(key: NamedKey) -> KeyEvent {
        KeyEvent::named(key, PhysicalKey::Unidentified(0))
    }

    fn character(text: &str) -> KeyEvent {
        KeyEvent {
            key: Key::Character(text.into()),
            key_without_modifiers: Key::Character(text.into()),
            physical: PhysicalKey::Unidentified(0),
            location: KeyLocation::Standard,
            repeat: false,
        }
    }

    fn encode(event: &KeyEvent, modifiers: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
        encode_key(
            event,
            modifiers,
            KeyState::Down,
            mode,
            &KeyBindings::default(),
        )
        .map(|bytes| bytes.to_vec())
    }

    #[test]
    fn the_arrow_keys_answer_to_the_application_cursor_mode() {
        let up = named(NamedKey::ArrowUp);
        assert_eq!(
            encode(&up, Modifiers::NONE, TermMode::default()).as_deref(),
            Some(&b"\x1b[A"[..])
        );
        assert_eq!(
            encode(&up, Modifiers::NONE, TermMode::APP_CURSOR).as_deref(),
            Some(&b"\x1bOA"[..])
        );
    }

    #[test]
    fn a_modified_arrow_key_carries_the_modifier_number() {
        let right = named(NamedKey::ArrowRight);
        assert_eq!(
            encode(&right, Modifiers::CONTROL, TermMode::default()).as_deref(),
            Some(&b"\x1b[1;5C"[..])
        );
        assert_eq!(
            encode(&right, Modifiers::SHIFT, TermMode::default()).as_deref(),
            Some(&b"\x1b[1;2C"[..])
        );
        assert_eq!(
            encode(
                &right,
                Modifiers::CONTROL | Modifiers::SHIFT,
                TermMode::default()
            )
            .as_deref(),
            Some(&b"\x1b[1;6C"[..])
        );
    }

    #[test]
    fn the_editing_keys_use_their_numbers() {
        assert_eq!(
            encode(
                &named(NamedKey::Delete),
                Modifiers::NONE,
                TermMode::default()
            )
            .as_deref(),
            Some(&b"\x1b[3~"[..])
        );
        assert_eq!(
            encode(
                &named(NamedKey::PageUp),
                Modifiers::CONTROL,
                TermMode::default()
            )
            .as_deref(),
            Some(&b"\x1b[5;5~"[..])
        );
    }

    #[test]
    fn the_first_four_function_keys_keep_their_own_introducer() {
        assert_eq!(
            encode(&named(NamedKey::F1), Modifiers::NONE, TermMode::default()).as_deref(),
            Some(&b"\x1bOP"[..])
        );
        assert_eq!(
            encode(&named(NamedKey::F5), Modifiers::NONE, TermMode::default()).as_deref(),
            Some(&b"\x1b[15~"[..])
        );
        assert_eq!(
            encode(&named(NamedKey::F12), Modifiers::NONE, TermMode::default()).as_deref(),
            Some(&b"\x1b[24~"[..])
        );
    }

    #[test]
    fn shift_and_tab_have_their_own_sequence() {
        assert_eq!(
            encode(&named(NamedKey::Tab), Modifiers::NONE, TermMode::default()).as_deref(),
            Some(&b"\t"[..])
        );
        assert_eq!(
            encode(&named(NamedKey::Tab), Modifiers::SHIFT, TermMode::default()).as_deref(),
            Some(&b"\x1b[Z"[..])
        );
    }

    #[test]
    fn control_and_a_letter_fold_onto_its_control_byte() {
        assert_eq!(
            encode(&character("c"), Modifiers::CONTROL, TermMode::default()).as_deref(),
            Some(&[0x03][..])
        );
        assert_eq!(
            encode(&character("a"), Modifiers::CONTROL, TermMode::default()).as_deref(),
            Some(&[0x01][..])
        );
    }

    #[test]
    fn control_folds_through_the_key_the_layout_gives_with_nothing_held() {
        // Shift and control together still send the letter's control byte.
        let event = KeyEvent {
            key: Key::Character("C".into()),
            key_without_modifiers: Key::Character("c".into()),
            physical: PhysicalKey::Unidentified(0),
            location: KeyLocation::Standard,
            repeat: false,
        };
        assert_eq!(
            encode(
                &event,
                Modifiers::CONTROL | Modifiers::SHIFT,
                TermMode::default()
            )
            .as_deref(),
            Some(&[0x03][..])
        );
    }

    #[test]
    fn control_and_punctuation_follow_the_old_table() {
        for (character_text, byte) in [
            ("[", 0x1bu8),
            ("\\", 0x1c),
            ("]", 0x1d),
            ("_", 0x1f),
            ("?", 0x7f),
        ] {
            assert_eq!(
                encode(
                    &character(character_text),
                    Modifiers::CONTROL,
                    TermMode::default()
                )
                .as_deref(),
                Some(&[byte][..]),
                "for {character_text}"
            );
        }
    }

    #[test]
    fn alt_puts_an_escape_before_the_key() {
        assert_eq!(
            encode(&character("a"), Modifiers::ALT, TermMode::default()).as_deref(),
            Some(&b"\x1ba"[..])
        );
    }

    #[test]
    fn a_plain_printable_key_sends_its_text() {
        assert_eq!(
            encode(&character("a"), Modifiers::NONE, TermMode::default()).as_deref(),
            Some(&b"a"[..])
        );
    }

    #[test]
    fn a_key_held_with_the_command_modifier_belongs_to_the_application() {
        assert_eq!(
            encode(&character("a"), Modifiers::META, TermMode::default()),
            None
        );
    }

    #[test]
    fn control_and_space_send_a_zero_byte() {
        assert_eq!(
            encode(
                &named(NamedKey::Space),
                Modifiers::CONTROL,
                TermMode::default()
            )
            .as_deref(),
            Some(&[0x00][..])
        );
    }

    #[test]
    fn control_and_backspace_delete_a_word() {
        assert_eq!(
            encode(
                &named(NamedKey::Backspace),
                Modifiers::NONE,
                TermMode::default()
            )
            .as_deref(),
            Some(&[0x7f][..])
        );
        assert_eq!(
            encode(
                &named(NamedKey::Backspace),
                Modifiers::CONTROL,
                TermMode::default()
            )
            .as_deref(),
            Some(&[0x08][..])
        );
    }

    #[test]
    fn a_release_says_nothing_to_a_program_reading_the_old_sequences() {
        let bytes = encode_key(
            &named(NamedKey::ArrowUp),
            Modifiers::NONE,
            KeyState::Up,
            TermMode::default(),
            &KeyBindings::default(),
        );
        assert!(bytes.is_none());
    }

    // The disambiguated protocol.

    fn kitty_mode() -> TermMode {
        TermMode::DISAMBIGUATE_ESC_CODES
    }

    #[test]
    fn the_protocol_reports_control_and_a_letter_by_number() {
        let bytes = encode(&character("c"), Modifiers::CONTROL, kitty_mode());
        assert_eq!(bytes.as_deref(), Some(&b"\x1b[99;5u"[..]));
    }

    #[test]
    fn the_protocol_sends_the_text_of_a_plain_key() {
        assert_eq!(
            encode(&character("a"), Modifiers::NONE, kitty_mode()).as_deref(),
            Some(&b"a"[..])
        );
    }

    #[test]
    fn the_protocol_reports_every_key_when_asked() {
        let mode = kitty_mode() | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            encode(&character("a"), Modifiers::NONE, mode).as_deref(),
            Some(&b"\x1b[97u"[..])
        );
    }

    #[test]
    fn the_protocol_keeps_the_arrow_sequences() {
        assert_eq!(
            encode(&named(NamedKey::ArrowUp), Modifiers::NONE, kitty_mode()).as_deref(),
            Some(&b"\x1b[1;1A"[..])
        );
    }

    #[test]
    fn a_release_is_reported_when_the_program_asked_for_the_kinds() {
        let mode = kitty_mode() | TermMode::REPORT_EVENT_TYPES;
        let bytes = encode_key(
            &character("a"),
            Modifiers::CONTROL,
            KeyState::Up,
            mode,
            &KeyBindings::default(),
        );
        assert_eq!(
            bytes.map(|bytes| bytes.to_vec()).as_deref(),
            Some(&b"\x1b[97;5:3u"[..])
        );
    }

    #[test]
    fn a_repeat_is_told_apart_from_a_press() {
        let mode = kitty_mode() | TermMode::REPORT_EVENT_TYPES;
        let mut event = character("a");
        event.repeat = true;
        let bytes = encode_key(
            &event,
            Modifiers::CONTROL,
            KeyState::Down,
            mode,
            &KeyBindings::default(),
        );
        assert_eq!(
            bytes.map(|bytes| bytes.to_vec()).as_deref(),
            Some(&b"\x1b[97;5:2u"[..])
        );
    }

    #[test]
    fn a_release_says_nothing_unless_the_program_asked_for_the_kinds() {
        let bytes = encode_key(
            &character("a"),
            Modifiers::CONTROL,
            KeyState::Up,
            kitty_mode(),
            &KeyBindings::default(),
        );
        assert!(bytes.is_none());
    }

    #[test]
    fn a_modifier_on_its_own_is_reported_only_when_every_key_is() {
        assert_eq!(
            encode(&named(NamedKey::Shift), Modifiers::SHIFT, kitty_mode()),
            None
        );
        let mode = kitty_mode() | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            encode(&named(NamedKey::Shift), Modifiers::SHIFT, mode).as_deref(),
            Some(&b"\x1b[57441;2u"[..])
        );
    }

    #[test]
    fn the_shifted_key_is_reported_beside_the_base_one() {
        let mode = kitty_mode() | TermMode::REPORT_ALTERNATE_KEYS;
        let event = KeyEvent {
            key: Key::Character("A".into()),
            key_without_modifiers: Key::Character("a".into()),
            physical: PhysicalKey::Unidentified(0),
            location: KeyLocation::Standard,
            repeat: false,
        };
        assert_eq!(
            encode(&event, Modifiers::CONTROL | Modifiers::SHIFT, mode).as_deref(),
            Some(&b"\x1b[97:65;6u"[..])
        );
    }

    #[test]
    fn the_text_a_key_produces_is_reported_when_asked() {
        let mode =
            kitty_mode() | TermMode::REPORT_ASSOCIATED_TEXT | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            encode(&character("a"), Modifiers::NONE, mode).as_deref(),
            Some(&b"\x1b[97;1;97u"[..])
        );
    }

    #[test]
    fn return_and_tab_are_reported_by_number_when_modified() {
        assert_eq!(
            encode(&named(NamedKey::Enter), Modifiers::NONE, kitty_mode()).as_deref(),
            Some(&b"\r"[..])
        );
        assert_eq!(
            encode(&named(NamedKey::Enter), Modifiers::CONTROL, kitty_mode()).as_deref(),
            Some(&b"\x1b[13;5u"[..])
        );
    }
}
