//! The keys a terminal keeps for the clipboard.
//!
//! Control and shift together produce nothing a program reads, so every terminal spares them for
//! copy and paste. The plain control chords stay with the program.

use zgui::prelude::Modifiers;
use zgui::vocab::{Key, KeyEvent};

/// What a clipboard chord asks for.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ClipboardChord {
    /// Put the selection on the clipboard.
    Copy,
    /// Send the clipboard to the program.
    Paste,
}

/// The clipboard chord `event` is under `modifiers`, when it is one.
pub fn clipboard_chord(event: &KeyEvent, modifiers: Modifiers) -> Option<ClipboardChord> {
    if !(modifiers.control() && modifiers.shift()) || modifiers.alt() || modifiers.meta() {
        return None;
    }
    let Key::Character(text) = &event.key_without_modifiers else {
        return None;
    };
    if text.eq_ignore_ascii_case("c") {
        Some(ClipboardChord::Copy)
    } else if text.eq_ignore_ascii_case("v") {
        Some(ClipboardChord::Paste)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use zgui::prelude::Modifiers;
    use zgui::vocab::KeyEvent;

    use super::{ClipboardChord, clipboard_chord};

    fn letter(text: &str) -> KeyEvent {
        KeyEvent::character(text.to_owned())
    }

    #[test]
    fn control_shift_c_and_v_are_the_clipboard() {
        let both = Modifiers::CONTROL | Modifiers::SHIFT;
        assert_eq!(
            clipboard_chord(&letter("c"), both),
            Some(ClipboardChord::Copy)
        );
        assert_eq!(
            clipboard_chord(&letter("C"), both),
            Some(ClipboardChord::Copy)
        );
        assert_eq!(
            clipboard_chord(&letter("v"), both),
            Some(ClipboardChord::Paste)
        );
    }

    #[test]
    fn plain_control_stays_with_the_program() {
        assert_eq!(clipboard_chord(&letter("c"), Modifiers::CONTROL), None);
        assert_eq!(clipboard_chord(&letter("v"), Modifiers::CONTROL), None);
        assert_eq!(
            clipboard_chord(&letter("x"), Modifiers::CONTROL | Modifiers::SHIFT),
            None
        );
    }
}
