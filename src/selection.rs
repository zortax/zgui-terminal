//! Choosing text with the pointer.

use std::time::{Duration, Instant};

use alacritty_terminal::selection::SelectionType;
use zgui::prelude::Modifiers;

/// How long two clicks may be apart and still count as one gesture.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);

/// How far apart two clicks may be and still count as one gesture, in cells.
const MULTI_CLICK_DISTANCE: usize = 1;

/// Counts the clicks of one gesture.
///
/// The framework reports a click and a double click; a terminal also selects a whole line on the
/// third, so the count is kept here.
#[derive(Debug, Default)]
pub struct ClickCounter {
    last: Option<(Instant, usize, usize)>,
    count: usize,
}

impl ClickCounter {
    /// A counter that has seen nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts a click at `column` and `line`, at the time `now`.
    ///
    /// Answers how many clicks the gesture is up to, from one.
    pub fn click_at(&mut self, column: usize, line: usize, now: Instant) -> usize {
        let continues = self.last.is_some_and(|(at, last_column, last_line)| {
            now.duration_since(at) <= MULTI_CLICK_INTERVAL
                && column.abs_diff(last_column) <= MULTI_CLICK_DISTANCE
                && line.abs_diff(last_line) <= MULTI_CLICK_DISTANCE
        });
        self.count = if continues { self.count % 3 + 1 } else { 1 };
        self.last = Some((now, column, line));
        self.count
    }

    /// Forgets the gesture, so the next click starts a new one.
    pub fn reset(&mut self) {
        self.last = None;
        self.count = 0;
    }
}

/// What a click of this many presses selects.
///
/// One click selects what the pointer covers, two select a word, and three select a line. Holding
/// the block modifier selects a rectangle instead, which is what a person wants when they are
/// taking one column out of a table.
pub fn selection_type(clicks: usize, modifiers: Modifiers) -> SelectionType {
    if modifiers.control() {
        return SelectionType::Block;
    }
    match clicks {
        2 => SelectionType::Semantic,
        3 => SelectionType::Lines,
        _ => SelectionType::Simple,
    }
}

/// Whether a pointer press should select rather than reach the program.
///
/// A program that reads the mouse gets the press, unless shift says the person meant to select.
pub fn selects_rather_than_reports(
    modifiers: Modifiers,
    mode: alacritty_terminal::term::TermMode,
) -> bool {
    modifiers.shift() || !mode.intersects(alacritty_terminal::term::TermMode::MOUSE_MODE)
}

/// Whether a press of `button` starts a selection.
///
/// The primary button selects. The secondary button leaves the selection as it is, so a context
/// menu opened over it can still copy it.
pub fn button_selects(button: Option<zgui::vocab::PointerButton>) -> bool {
    matches!(button, None | Some(zgui::vocab::PointerButton::Primary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::TermMode;

    #[test]
    fn clicks_in_the_same_place_add_up() {
        let mut counter = ClickCounter::new();
        let now = Instant::now();
        assert_eq!(counter.click_at(4, 2, now), 1);
        assert_eq!(counter.click_at(4, 2, now + Duration::from_millis(100)), 2);
        assert_eq!(counter.click_at(4, 2, now + Duration::from_millis(200)), 3);
        // A fourth starts the gesture again.
        assert_eq!(counter.click_at(4, 2, now + Duration::from_millis(300)), 1);
    }

    #[test]
    fn a_slow_second_click_starts_again() {
        let mut counter = ClickCounter::new();
        let now = Instant::now();
        assert_eq!(counter.click_at(4, 2, now), 1);
        assert_eq!(counter.click_at(4, 2, now + Duration::from_millis(900)), 1);
    }

    #[test]
    fn a_click_somewhere_else_starts_again() {
        let mut counter = ClickCounter::new();
        let now = Instant::now();
        assert_eq!(counter.click_at(4, 2, now), 1);
        assert_eq!(counter.click_at(40, 2, now + Duration::from_millis(50)), 1);
    }

    #[test]
    fn each_count_selects_more() {
        assert_eq!(selection_type(1, Modifiers::NONE), SelectionType::Simple);
        assert_eq!(selection_type(2, Modifiers::NONE), SelectionType::Semantic);
        assert_eq!(selection_type(3, Modifiers::NONE), SelectionType::Lines);
    }

    #[test]
    fn the_block_modifier_selects_a_rectangle() {
        assert_eq!(selection_type(1, Modifiers::CONTROL), SelectionType::Block);
        assert_eq!(selection_type(3, Modifiers::CONTROL), SelectionType::Block);
    }

    #[test]
    fn shift_selects_past_a_program_that_took_the_mouse() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert!(!selects_rather_than_reports(Modifiers::NONE, mode));
        assert!(selects_rather_than_reports(Modifiers::SHIFT, mode));
        assert!(selects_rather_than_reports(
            Modifiers::NONE,
            TermMode::default()
        ));
    }

    #[test]
    fn only_the_primary_button_selects() {
        use zgui::vocab::PointerButton;
        assert!(button_selects(None));
        assert!(button_selects(Some(PointerButton::Primary)));
        assert!(!button_selects(Some(PointerButton::Secondary)));
    }
}
