//! What a key sends to a program that reads the ordinary sequences.

use alacritty_terminal::term::TermMode;
use zgui::prelude::Modifiers;
use zgui::vocab::{Key, KeyLocation, NamedKey};

use crate::input::keyboard::named::{KeyForm, form_of};
use crate::input::keyboard::{Bytes, KeyBindings, push_number};

/// The number the modifier keys are reported by.
///
/// One is nothing held; every modifier adds its own bit above that.
pub fn modifier_parameter(modifiers: Modifiers) -> u16 {
    let mut parameter = 1;
    if modifiers.shift() {
        parameter += 1;
    }
    if modifiers.alt() {
        parameter += 2;
    }
    if modifiers.control() {
        parameter += 4;
    }
    if modifiers.meta() {
        parameter += 8;
    }
    parameter
}

/// The bytes `key` sends.
///
/// Answers nothing when the key produces text instead, which arrives separately, or when it says
/// nothing at all.
pub fn encode(
    key: &Key,
    location: KeyLocation,
    modifiers: Modifiers,
    mode: TermMode,
    bindings: &KeyBindings,
) -> Option<Bytes> {
    match key {
        Key::Named(named) => encode_named(*named, location, modifiers, mode, bindings),
        Key::Character(text) => encode_text(text, modifiers, bindings),
        _ => None,
    }
}

/// The bytes a named key sends.
fn encode_named(
    key: NamedKey,
    location: KeyLocation,
    modifiers: Modifiers,
    mode: TermMode,
    bindings: &KeyBindings,
) -> Option<Bytes> {
    let mut bytes = Bytes::new();
    let parameter = modifier_parameter(modifiers);
    let modified = parameter > 1;

    // Tab with shift has its own sequence, which is how a program tells the two apart.
    if key == NamedKey::Tab && modifiers.shift() {
        bytes.extend_from_slice(b"\x1b[Z");
        return Some(bytes);
    }

    match form_of(key)? {
        KeyForm::Byte(byte) => {
            match key {
                // Control and backspace together delete a word in most shells, which is what the
                // other byte means.
                NamedKey::Backspace if modifiers.control() => bytes.push(0x08),
                NamedKey::Space if modifiers.control() => bytes.push(0x00),
                NamedKey::Enter if mode.contains(TermMode::LINE_FEED_NEW_LINE) => {
                    bytes.extend_from_slice(b"\r\n");
                }
                _ => {
                    if modifiers.alt() && bindings.alt_sends_escape {
                        bytes.push(0x1b);
                    }
                    bytes.push(byte);
                }
            }
            Some(bytes)
        }
        KeyForm::Letter(final_byte, application) => {
            if modified {
                // A modified key always uses the long form, so the number has somewhere to go.
                bytes.extend_from_slice(b"\x1b[1;");
                push_number(&mut bytes, u32::from(parameter));
                bytes.push(final_byte);
            } else if application
                && mode.contains(TermMode::APP_CURSOR)
                && location != KeyLocation::Numpad
            {
                bytes.extend_from_slice(b"\x1bO");
                bytes.push(final_byte);
            } else if !application {
                // The first four function keys keep their own introducer.
                bytes.extend_from_slice(b"\x1bO");
                bytes.push(final_byte);
            } else {
                bytes.extend_from_slice(b"\x1b[");
                bytes.push(final_byte);
            }
            Some(bytes)
        }
        KeyForm::Tilde(number) => {
            bytes.extend_from_slice(b"\x1b[");
            push_number(&mut bytes, u32::from(number));
            if modified {
                bytes.push(b';');
                push_number(&mut bytes, u32::from(parameter));
            }
            bytes.push(b'~');
            Some(bytes)
        }
    }
}

/// The bytes a key that produces text sends.
///
/// The key press carries the text itself, so this is where typing happens. What a person
/// composes through an input method arrives separately, on its own event.
fn encode_text(text: &str, modifiers: Modifiers, bindings: &KeyBindings) -> Option<Bytes> {
    // A key held with the platform's command modifier belongs to the application.
    if modifiers.meta() {
        return None;
    }
    let mut bytes = Bytes::new();

    if modifiers.control() {
        let character = text.chars().next()?;
        let control = control_byte(character)?;
        if modifiers.alt() && bindings.alt_sends_escape {
            bytes.push(0x1b);
        }
        bytes.push(control);
        return Some(bytes);
    }

    if modifiers.alt() && bindings.alt_sends_escape {
        bytes.push(0x1b);
        bytes.extend_from_slice(text.as_bytes());
        return Some(bytes);
    }

    if text.is_empty() {
        return None;
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

/// The byte control and `character` send together.
///
/// The letters fold onto one to twenty-six, and the handful of punctuation that has a control
/// byte follows the same table a terminal has always used.
pub fn control_byte(character: char) -> Option<u8> {
    let folded = character.to_ascii_uppercase();
    match folded {
        'A'..='Z' => Some(folded as u8 - b'@'),
        '@' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        ' ' => Some(0x00),
        // The digits two and six carry the same bytes as at and caret on most layouts.
        '2' => Some(0x00),
        '6' => Some(0x1e),
        '-' => Some(0x1f),
        '/' => Some(0x1f),
        _ => None,
    }
}
