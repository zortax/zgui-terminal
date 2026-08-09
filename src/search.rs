//! Looking through the history.
//!
//! Everything a search does runs on the pump: building the expression and reading a long history
//! are exactly the work that must not happen where a frame is drawn.

use alacritty_terminal::index::Point;

/// Which way a search reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SearchDirection {
    /// Toward the newest line.
    #[default]
    Forward,
    /// Toward the oldest line.
    Backward,
}

/// Where a search starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SearchOrigin {
    /// What is on screen.
    #[default]
    Viewport,
    /// The match the view is on.
    Current,
    /// One place in the grid.
    Point(Point),
}

/// How a search reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SearchOptions {
    /// Which way it reads.
    pub direction: SearchDirection,
    /// Where it starts.
    pub origin: SearchOrigin,
}

/// How far a search has got.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum SearchStatus {
    /// Nothing is being looked for.
    #[default]
    Idle,
    /// The matches are still being counted.
    Counting,
    /// Everything has been read.
    Ready,
    /// The pattern is not an expression, for the reason given.
    Invalid(String),
}

/// What the view knows about the search.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SearchState {
    /// What is being looked for.
    pub pattern: String,
    /// How far the search has got.
    pub status: SearchStatus,
    /// How many matches have been found so far.
    pub total: usize,
    /// Which of them the view is on.
    pub current: Option<usize>,
    /// Whether the last step started again from the far end.
    pub wrapped: bool,
}

impl SearchState {
    /// Whether a search is running.
    pub fn is_active(&self) -> bool {
        !matches!(self.status, SearchStatus::Idle)
    }

    /// Whether the count stopped at its limit.
    pub fn is_capped(&self) -> bool {
        self.total >= 10_000
    }

    /// What a search bar shows: which match this is, out of how many.
    ///
    /// ```
    /// use zgui_terminal::{SearchState, SearchStatus};
    ///
    /// let state = SearchState {
    ///     status: SearchStatus::Ready,
    ///     total: 12,
    ///     current: Some(2),
    ///     ..SearchState::default()
    /// };
    /// assert_eq!(state.summary(), "3 of 12");
    /// ```
    pub fn summary(&self) -> String {
        match &self.status {
            SearchStatus::Idle => String::new(),
            SearchStatus::Invalid(reason) => reason.clone(),
            SearchStatus::Counting | SearchStatus::Ready => {
                let total = if self.is_capped() {
                    format!("{}+", self.total)
                } else {
                    self.total.to_string()
                };
                match self.current {
                    Some(current) if self.total > 0 => format!("{} of {}", current + 1, total),
                    _ if self.total == 0 && matches!(self.status, SearchStatus::Ready) => {
                        "no matches".to_owned()
                    }
                    _ => format!("{total} matches"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_search_says_nothing() {
        assert_eq!(SearchState::default().summary(), "");
    }

    #[test]
    fn a_bad_pattern_reports_why() {
        let state = SearchState {
            status: SearchStatus::Invalid("unclosed group".to_owned()),
            ..SearchState::default()
        };
        assert_eq!(state.summary(), "unclosed group");
    }

    #[test]
    fn a_search_with_nothing_found_says_so() {
        let state = SearchState {
            status: SearchStatus::Ready,
            ..SearchState::default()
        };
        assert_eq!(state.summary(), "no matches");
    }

    #[test]
    fn a_capped_count_says_there_are_more() {
        let state = SearchState {
            status: SearchStatus::Ready,
            total: 10_000,
            current: Some(0),
            ..SearchState::default()
        };
        assert_eq!(state.summary(), "1 of 10000+");
    }
}
