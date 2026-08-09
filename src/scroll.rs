//! Where the view sits over the history.
//!
//! The view scrolls, and the emulator is told where it landed. Both have to agree, because
//! selection, mouse reporting and search all speak in grid coordinates while the view speaks in
//! pixels.

pub use crate::core::command::ScrollRequest;

/// Where the view sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ScrollState {
    /// How many lines above the newest one the view starts. Zero is the bottom.
    pub display_offset: usize,
    /// How many lines of history the grid holds.
    pub history: usize,
    /// How many lines the screen has.
    pub screen_lines: usize,
}

impl ScrollState {
    /// Whether the view is showing the newest line.
    pub fn is_at_bottom(&self) -> bool {
        self.display_offset == 0
    }

    /// How many lines the grid holds, history and screen together.
    pub fn total_lines(&self) -> usize {
        self.history + self.screen_lines
    }

    /// How far the view can be scrolled back.
    pub fn max_offset(&self) -> usize {
        self.history
    }

    /// Which line of the whole grid the view starts at, counting from the oldest.
    pub fn first_visible(&self) -> usize {
        self.history.saturating_sub(self.display_offset)
    }
}

/// Whether the view still follows the newest line, after it moved.
///
/// The terminal moves the view only because the content grew, and only ever toward the newest
/// line. A person moves it for their own reasons, in either direction. Telling the two apart is
/// what lets a terminal stay at the bottom while a program writes, and stay where it is put when
/// somebody drags the bar.
///
/// Going by the offset alone would not do: a view told to go further than the content reaches is
/// put where the content ends instead, and that correction looks exactly like a person scrolling.
pub fn follows_after(
    following: bool,
    offset: f32,
    previous: f32,
    bottom: f32,
    content_grew: bool,
) -> bool {
    let at_bottom = offset >= bottom - 1.0;
    let moved = (offset - previous).abs() > 0.5;
    // Toward the older lines is a person's doing whatever else happened.
    let moved_back = offset < previous - 0.5;

    if (moved && !content_grew) || moved_back {
        return at_bottom;
    }
    following || at_bottom
}

/// How a wheel event is answered.
///
/// A program that reads the mouse gets the notches, unless the shift key says the person meant
/// the view rather than the program. Otherwise a full-screen program moves through its own
/// history with the arrow keys, and everything else scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelAction {
    /// Move the view by this many lines, up when positive.
    Scroll(i32),
    /// Report the notches to the program.
    Report(i32),
    /// Send this many arrow keys to the program.
    Arrows(i32),
}

/// What a wheel event should do.
pub fn wheel_action(
    lines: i32,
    shift: bool,
    mode: alacritty_terminal::term::TermMode,
) -> WheelAction {
    use alacritty_terminal::term::TermMode;

    if lines == 0 {
        return WheelAction::Scroll(0);
    }
    // Shift is the way past a program that has taken the mouse.
    if !shift && mode.intersects(TermMode::MOUSE_MODE) {
        return WheelAction::Report(lines);
    }
    if !shift && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        return WheelAction::Arrows(lines);
    }
    WheelAction::Scroll(lines)
}

/// How many lines a wheel movement of `pixels` covers, keeping what is left over.
///
/// A trackpad reports fractions of a line. Dropping them would make a slow drag move nothing at
/// all, so what is left over is kept for the next event.
pub fn lines_from_pixels(pixels: f32, cell_height: f32, remainder: &mut f32) -> i32 {
    if cell_height <= 0.0 {
        return 0;
    }
    let total = *remainder + pixels / cell_height;
    let lines = total.trunc();
    *remainder = total - lines;
    lines as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::TermMode;

    #[test]
    fn a_program_reading_the_mouse_gets_the_notches() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(wheel_action(3, false, mode), WheelAction::Report(3));
    }

    #[test]
    fn shift_reaches_past_a_program_that_took_the_mouse() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(wheel_action(3, true, mode), WheelAction::Scroll(3));
    }

    #[test]
    fn a_full_screen_program_is_sent_arrow_keys() {
        let mode = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        assert_eq!(wheel_action(2, false, mode), WheelAction::Arrows(2));
    }

    #[test]
    fn ordinary_output_scrolls_the_view() {
        assert_eq!(
            wheel_action(2, false, TermMode::default()),
            WheelAction::Scroll(2)
        );
    }

    #[test]
    fn a_trackpad_keeps_what_is_left_over() {
        let mut remainder = 0.0;
        assert_eq!(lines_from_pixels(8.0, 20.0, &mut remainder), 0);
        assert_eq!(lines_from_pixels(8.0, 20.0, &mut remainder), 0);
        // Three small movements add up to more than one line.
        assert_eq!(lines_from_pixels(8.0, 20.0, &mut remainder), 1);
    }

    #[test]
    fn output_alone_keeps_the_view_following() {
        // The content grew and the terminal moved the view along with it. Nobody scrolled.
        assert!(follows_after(true, 500.0, 400.0, 500.0, true));
    }

    #[test]
    fn a_view_put_short_of_the_end_keeps_following() {
        // What a view told to go further than the content reaches looks like. Reading this as a
        // person scrolling is what stopped a terminal following its own output.
        assert!(follows_after(true, 460.0, 400.0, 500.0, true));
    }

    #[test]
    fn dragging_the_bar_upward_stops_the_view_following() {
        // The case the wheel does not cover: the content is the same size, so the movement is a
        // person's, whichever way it went.
        assert!(!follows_after(true, 200.0, 500.0, 500.0, false));
    }

    #[test]
    fn dragging_upward_stops_it_following_even_while_a_program_writes() {
        // The terminal never moves the view toward the older lines, so this is a person.
        assert!(!follows_after(true, 200.0, 500.0, 600.0, true));
    }

    #[test]
    fn coming_back_to_the_bottom_starts_it_following_again() {
        assert!(follows_after(false, 500.0, 200.0, 500.0, false));
    }

    #[test]
    fn a_view_left_where_it_was_stays_as_it_was() {
        assert!(!follows_after(false, 200.0, 200.0, 500.0, false));
        assert!(follows_after(true, 500.0, 500.0, 500.0, false));
    }

    #[test]
    fn the_bottom_of_the_history_is_offset_zero() {
        let state = ScrollState {
            display_offset: 0,
            history: 100,
            screen_lines: 24,
        };
        assert!(state.is_at_bottom());
        assert_eq!(state.total_lines(), 124);
        assert_eq!(state.first_visible(), 100);
    }
}
