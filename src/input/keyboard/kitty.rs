//! What a key sends to a program that asked for the disambiguated reports.
//!
//! The protocol lets a program tell apart keys the ordinary sequences cannot: control and a
//! letter against the letter's control byte, a press against a release, one modifier against
//! another. A program turns it on a level at a time, and each level is a mode bit.

use alacritty_terminal::term::TermMode;
use zgui::prelude::Modifiers;
use zgui::vocab::{Key, KeyLocation};

use crate::input::keyboard::legacy::modifier_parameter;
use crate::input::keyboard::named::{KeyForm, form_of, functional_number, is_modifier};
use crate::input::keyboard::{Bytes, KeyState, push_number};

/// Which kind of event a report describes.
fn event_number(state: KeyState, repeat: bool) -> u8 {
    match (state, repeat) {
        (KeyState::Down, false) => 1,
        (KeyState::Down, true) => 2,
        (KeyState::Up, _) => 3,
    }
}

/// The bytes `key` sends.
#[allow(clippy::too_many_arguments)]
pub fn encode(
    key: &Key,
    base: &Key,
    location: KeyLocation,
    modifiers: Modifiers,
    state: KeyState,
    repeat: bool,
    mode: TermMode,
) -> Option<Bytes> {
    let report_events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let report_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let report_alternates = mode.contains(TermMode::REPORT_ALTERNATE_KEYS);
    let report_text = mode.contains(TermMode::REPORT_ASSOCIATED_TEXT);

    // A release says nothing unless the program asked to hear about them.
    if state == KeyState::Up && !report_events {
        return None;
    }

    // A modifier on its own is only reported when every key is.
    if let Key::Named(named) = key
        && is_modifier(*named)
        && !report_all
    {
        return None;
    }

    let parameter = modifier_parameter(modifiers);
    let event = event_number(state, repeat);
    let needs_event = report_events && event != 1;

    // A key that has a sequence of its own keeps it, with the number added.
    if let Key::Named(named) = key {
        if let Some(form) = form_of(*named) {
            match form {
                KeyForm::Letter(final_byte, _) => {
                    let mut bytes = Bytes::new();
                    bytes.extend_from_slice(b"\x1b[1;");
                    push_number(&mut bytes, u32::from(parameter));
                    if needs_event {
                        bytes.push(b':');
                        push_number(&mut bytes, u32::from(event));
                    }
                    bytes.push(final_byte);
                    return Some(bytes);
                }
                KeyForm::Tilde(number) => {
                    let mut bytes = Bytes::new();
                    bytes.extend_from_slice(b"\x1b[");
                    push_number(&mut bytes, u32::from(number));
                    bytes.push(b';');
                    push_number(&mut bytes, u32::from(parameter));
                    if needs_event {
                        bytes.push(b':');
                        push_number(&mut bytes, u32::from(event));
                    }
                    bytes.push(b'~');
                    return Some(bytes);
                }
                KeyForm::Byte(byte) => {
                    // Return, tab, backspace and escape are reported by their number, so that
                    // control and the key are told apart from the key alone.
                    let plain = parameter == 1 && !needs_event;
                    if plain && !report_all {
                        let mut bytes = Bytes::new();
                        bytes.push(byte);
                        return Some(bytes);
                    }
                    let number = functional_number(*named).unwrap_or(u32::from(byte));
                    return Some(csi_u(
                        number,
                        None,
                        parameter,
                        needs_event.then_some(event),
                        None,
                    ));
                }
            }
        }
        // Everything else with a number of its own.
        let number = functional_number(*named)?;
        return Some(csi_u(
            number,
            None,
            parameter,
            needs_event.then_some(event),
            None,
        ));
    }

    let Key::Character(text) = key else {
        return None;
    };
    let character = text.chars().next()?;
    let base_character = match base {
        Key::Character(base) => base.chars().next(),
        _ => None,
    };
    // The number is the key as the layout produces it with nothing held, which is what lets a
    // program match a shortcut whatever the shift state.
    let number = u32::from(base_character.unwrap_or(character));

    // Shift alone produces text and nothing more, until the program asks for everything.
    let only_shift = !modifiers.control() && !modifiers.alt() && !modifiers.meta();
    if only_shift && !report_all {
        if state == KeyState::Up {
            return None;
        }
        let mut bytes = Bytes::new();
        bytes.extend_from_slice(text.as_bytes());
        return Some(bytes);
    }

    let alternates = report_alternates
        .then(|| (character != base_character.unwrap_or(character)).then_some(u32::from(character)))
        .flatten();
    let associated = (report_text && state == KeyState::Down)
        .then(|| text.chars().map(u32::from).collect::<Vec<_>>())
        .filter(|text| !text.is_empty());

    let _ = location;
    Some(csi_u(
        number,
        alternates,
        parameter,
        needs_event.then_some(event),
        associated,
    ))
}

/// One report: `CSI number ; modifiers u`, with what the program asked for added.
fn csi_u(
    number: u32,
    shifted: Option<u32>,
    parameter: u16,
    event: Option<u8>,
    text: Option<Vec<u32>>,
) -> Bytes {
    let mut bytes = Bytes::new();
    bytes.extend_from_slice(b"\x1b[");
    push_number(&mut bytes, number);
    if let Some(shifted) = shifted {
        bytes.push(b':');
        push_number(&mut bytes, shifted);
    }
    // The modifiers, and after them the kind of event, are the second parameter.
    if parameter > 1 || event.is_some() || text.is_some() {
        bytes.push(b';');
        push_number(&mut bytes, u32::from(parameter));
        if let Some(event) = event {
            bytes.push(b':');
            push_number(&mut bytes, u32::from(event));
        }
    }
    if let Some(text) = text {
        bytes.push(b';');
        for (index, codepoint) in text.iter().enumerate() {
            if index > 0 {
                bytes.push(b':');
            }
            push_number(&mut bytes, *codepoint);
        }
    }
    bytes.push(b'u');
    bytes
}
