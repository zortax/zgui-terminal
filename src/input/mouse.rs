//! Telling a program where the pointer is.

use alacritty_terminal::term::TermMode;
use smallvec::SmallVec;
use zgui::prelude::{Modifiers, PointerButton};

/// The bytes one report takes.
pub type Report = SmallVec<[u8; 16]>;

/// What the pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseReport {
    /// A button went down.
    Press(PointerButton),
    /// A button came up.
    Release(PointerButton),
    /// The pointer moved, with `button` held.
    Motion(Option<PointerButton>),
    /// The wheel turned up.
    WheelUp,
    /// The wheel turned down.
    WheelDown,
}

/// The number a button has in a report.
fn button_number(button: PointerButton) -> Option<u8> {
    match button {
        PointerButton::Primary => Some(0),
        PointerButton::Middle => Some(1),
        PointerButton::Secondary => Some(2),
        _ => None,
    }
}

/// The bits the modifier keys add to a report.
pub fn modifier_bits(modifiers: Modifiers) -> u8 {
    let mut bits = 0;
    if modifiers.shift() {
        bits |= 4;
    }
    if modifiers.alt() {
        bits |= 8;
    }
    if modifiers.control() {
        bits |= 16;
    }
    bits
}

/// The bytes reporting `report` at column `column` and line `line`, both counted from zero.
///
/// Answers nothing when the program did not ask for this kind of report.
pub fn encode(
    report: MouseReport,
    column: usize,
    line: usize,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<Report> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }

    let (mut code, released) = match report {
        MouseReport::Press(button) => (button_number(button)?, false),
        MouseReport::Release(button) => (button_number(button)?, true),
        MouseReport::Motion(button) => {
            let motion_wanted = mode.contains(TermMode::MOUSE_MOTION)
                || (mode.contains(TermMode::MOUSE_DRAG) && button.is_some());
            if !motion_wanted {
                return None;
            }
            // A move with nothing held is reported as button three, which is the number for
            // "none" in this protocol.
            let held = button.and_then(button_number).unwrap_or(3);
            (held + 32, false)
        }
        MouseReport::WheelUp => (64, false),
        MouseReport::WheelDown => (65, false),
    };
    code += modifier_bits(modifiers);

    let mut bytes = Report::new();
    if mode.contains(TermMode::SGR_MOUSE) {
        // The modern form: no limit on how far the pointer can be, and a release says which
        // button it was.
        bytes.extend_from_slice(b"\x1b[<");
        push_number(&mut bytes, u32::from(code));
        bytes.push(b';');
        push_number(&mut bytes, column as u32 + 1);
        bytes.push(b';');
        push_number(&mut bytes, line as u32 + 1);
        bytes.push(if released { b'm' } else { b'M' });
        return Some(bytes);
    }

    // The original form: one byte each, so nothing past column 223 can be said.
    if column >= 223 || line >= 223 {
        return None;
    }
    // A release says only that something came up.
    let code = if released {
        3 + modifier_bits(modifiers)
    } else {
        code
    };
    bytes.extend_from_slice(b"\x1b[M");
    bytes.push(32 + code);
    bytes.push(32 + column as u8 + 1);
    bytes.push(32 + line as u8 + 1);
    Some(bytes)
}

/// Writes `value` as decimal digits.
fn push_number(bytes: &mut Report, value: u32) {
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

/// The buttons a program was told went down and has not yet been told came up.
///
/// A move reports the lowest of them, as xterm does, so a program sees a drag with the button
/// that started it. The pointer events of a move carry no button of their own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldButtons(u8);

impl HeldButtons {
    /// Records that the program was told `button` went down. Answers whether it is one the
    /// protocol can name.
    pub fn press(&mut self, button: PointerButton) -> bool {
        let Some(number) = button_number(button) else {
            return false;
        };
        self.0 |= 1 << number;
        true
    }

    /// Records that `button` came up. Answers whether the program had been told it was down.
    pub fn release(&mut self, button: PointerButton) -> bool {
        let Some(number) = button_number(button) else {
            return false;
        };
        let held = self.0 & (1 << number) != 0;
        self.0 &= !(1 << number);
        held
    }

    /// The button a move reports, when any is held.
    pub fn reported(self) -> Option<PointerButton> {
        [
            PointerButton::Primary,
            PointerButton::Middle,
            PointerButton::Secondary,
        ]
        .into_iter()
        .find(|button| button_number(*button).is_some_and(|number| self.0 & (1 << number) != 0))
    }

    /// Whether no button is held.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Which cell a point inside the grid falls in.
///
/// Both answers are clamped to the grid, so a pointer dragged past an edge selects to that edge
/// rather than stopping.
pub fn cell_at(
    x: f32,
    y: f32,
    cell_width: f32,
    cell_height: f32,
    columns: usize,
    lines: usize,
) -> (usize, usize, bool) {
    if cell_width <= 0.0 || cell_height <= 0.0 {
        return (0, 0, false);
    }
    let column_f = x / cell_width;
    let column = (column_f.floor().max(0.0) as usize).min(columns.saturating_sub(1));
    let line = ((y / cell_height).floor().max(0.0) as usize).min(lines.saturating_sub(1));
    // Which half of the cell the pointer is in, which is what decides where a selection ends.
    // The clamped column is what this asks about, so a point outside the grid reports the half
    // of the cell it was pulled into rather than the half of the cell it came from.
    let into_cell = column_f - column as f32;
    let right_half = into_cell >= 0.5;
    (column, line, right_half)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr() -> TermMode {
        TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE
    }

    #[test]
    fn nothing_is_reported_when_the_program_did_not_ask() {
        let report = encode(
            MouseReport::Press(PointerButton::Primary),
            0,
            0,
            Modifiers::default(),
            TermMode::default(),
        );
        assert!(report.is_none());
    }

    #[test]
    fn a_press_and_a_release_differ_only_in_the_last_byte() {
        let press = encode(
            MouseReport::Press(PointerButton::Primary),
            4,
            9,
            Modifiers::default(),
            sgr(),
        )
        .expect("a report");
        let release = encode(
            MouseReport::Release(PointerButton::Primary),
            4,
            9,
            Modifiers::default(),
            sgr(),
        )
        .expect("a report");
        assert_eq!(press.as_slice(), b"\x1b[<0;5;10M");
        assert_eq!(release.as_slice(), b"\x1b[<0;5;10m");
    }

    #[test]
    fn the_wheel_is_reported_as_its_own_buttons() {
        let up = encode(MouseReport::WheelUp, 0, 0, Modifiers::default(), sgr()).expect("a report");
        let down =
            encode(MouseReport::WheelDown, 0, 0, Modifiers::default(), sgr()).expect("a report");
        assert_eq!(up.as_slice(), b"\x1b[<64;1;1M");
        assert_eq!(down.as_slice(), b"\x1b[<65;1;1M");
    }

    #[test]
    fn the_modifier_keys_are_added_to_the_button() {
        let modifiers = Modifiers::CONTROL;
        let report = encode(
            MouseReport::Press(PointerButton::Primary),
            0,
            0,
            modifiers,
            sgr(),
        )
        .expect("a report");
        assert_eq!(report.as_slice(), b"\x1b[<16;1;1M");
    }

    #[test]
    fn a_move_is_reported_only_when_the_program_asked_for_moves() {
        let dragging = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE | TermMode::MOUSE_DRAG;
        assert!(
            encode(
                MouseReport::Motion(None),
                0,
                0,
                Modifiers::default(),
                dragging
            )
            .is_none()
        );
        assert!(
            encode(
                MouseReport::Motion(Some(PointerButton::Primary)),
                0,
                0,
                Modifiers::default(),
                dragging
            )
            .is_some()
        );
    }

    #[test]
    fn a_move_with_nothing_held_is_button_three_plus_thirty_two() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE | TermMode::MOUSE_MOTION;
        let report =
            encode(MouseReport::Motion(None), 0, 0, Modifiers::default(), mode).expect("a report");
        assert_eq!(report.as_slice(), b"\x1b[<35;1;1M");
    }

    #[test]
    fn the_original_form_says_nothing_past_its_limit() {
        let legacy = TermMode::MOUSE_REPORT_CLICK;
        assert!(
            encode(
                MouseReport::Press(PointerButton::Primary),
                5,
                5,
                Modifiers::default(),
                legacy
            )
            .is_some()
        );
        assert!(
            encode(
                MouseReport::Press(PointerButton::Primary),
                300,
                5,
                Modifiers::default(),
                legacy
            )
            .is_none()
        );
    }

    #[test]
    fn the_original_form_says_only_that_a_button_came_up() {
        let legacy = TermMode::MOUSE_REPORT_CLICK;
        let report = encode(
            MouseReport::Release(PointerButton::Secondary),
            0,
            0,
            Modifiers::default(),
            legacy,
        )
        .expect("a report");
        assert_eq!(report.as_slice(), &[0x1b, b'[', b'M', 32 + 3, 33, 33]);
    }

    #[test]
    fn a_move_while_a_button_is_held_reports_that_button() {
        let mut held = HeldButtons::default();
        assert_eq!(held.reported(), None);
        assert!(held.press(PointerButton::Primary));
        let mode = TermMode::MOUSE_REPORT_CLICK
            | TermMode::SGR_MOUSE
            | TermMode::MOUSE_DRAG
            | TermMode::MOUSE_MOTION;
        let report = encode(
            MouseReport::Motion(held.reported()),
            3,
            4,
            Modifiers::default(),
            mode,
        )
        .expect("a report");
        assert_eq!(report.as_slice(), b"\x1b[<32;4;5M");
    }

    #[test]
    fn the_lowest_held_button_is_the_one_a_move_reports() {
        let mut held = HeldButtons::default();
        held.press(PointerButton::Secondary);
        held.press(PointerButton::Middle);
        assert_eq!(held.reported(), Some(PointerButton::Middle));
        assert!(held.release(PointerButton::Middle));
        assert_eq!(held.reported(), Some(PointerButton::Secondary));
        assert!(held.release(PointerButton::Secondary));
        assert!(held.is_empty());
    }

    #[test]
    fn a_button_the_program_never_heard_of_is_not_held() {
        let mut held = HeldButtons::default();
        assert!(!held.press(PointerButton::Back));
        assert!(!held.release(PointerButton::Primary));
        assert!(held.is_empty());
    }

    #[test]
    fn a_point_falls_in_the_cell_it_is_over() {
        assert_eq!(cell_at(0.0, 0.0, 10.0, 20.0, 80, 24), (0, 0, false));
        assert_eq!(cell_at(25.0, 45.0, 10.0, 20.0, 80, 24), (2, 2, true));
    }

    #[test]
    fn a_point_past_the_grid_lands_on_its_edge() {
        assert_eq!(cell_at(10_000.0, 10_000.0, 10.0, 20.0, 80, 24).0, 79);
        assert_eq!(cell_at(10_000.0, 10_000.0, 10.0, 20.0, 80, 24).1, 23);
        assert_eq!(cell_at(-5.0, -5.0, 10.0, 20.0, 80, 24), (0, 0, false));
    }
}
