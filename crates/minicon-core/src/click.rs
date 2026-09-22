//! Click streaks: single, double and triple click.
//!
//! The terminal counts clicks by cell and the composer by byte offset, but the
//! rule is the same: a click on the same spot within the multi-click window
//! advances the streak, anything else starts a new one, and the streak cycles
//! 1 -> 2 -> 3 -> 1 so a fourth click returns to character selection instead of
//! sticking on whole-line. It lived twice, once per surface; it lives here once.

use std::time::{Duration, Instant};

/// How long after a click a second one still counts toward the streak.
///
/// Matches the common Windows default rather than reading `SPI_GETDBLCLKTIME`,
/// which would drag a platform dependency into host-neutral code.
pub const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// The click streak on one surface, keyed by whatever that surface calls a
/// position (a terminal cell, a byte offset).
#[derive(Clone, Copy, Debug)]
pub struct ClickCounter<K> {
    last: Option<(Instant, K, u8)>,
}

impl<K> Default for ClickCounter<K> {
    fn default() -> Self {
        Self { last: None }
    }
}

impl<K: Copy + PartialEq> ClickCounter<K> {
    /// Records a click at `at` and returns its place in the streak: 1, 2 or 3.
    pub fn register(&mut self, at: K, now: Instant) -> u8 {
        let count = match self.last {
            Some((when, position, count))
                if position == at && now.saturating_duration_since(when) <= MULTI_CLICK_WINDOW =>
            {
                count % 3 + 1
            }
            _ => 1,
        };
        self.last = Some((now, at, count));
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_on_one_spot_cycle_one_two_three_one() {
        let start = Instant::now();
        let mut clicks = ClickCounter::default();
        let streak: Vec<u8> = (0..5)
            .map(|i| clicks.register(7_usize, start + Duration::from_millis(100 * i)))
            .collect();
        assert_eq!(streak, [1, 2, 3, 1, 2]);
    }

    #[test]
    fn a_different_spot_or_a_late_click_starts_over() {
        let start = Instant::now();
        let mut clicks = ClickCounter::default();
        assert_eq!(clicks.register((1_u16, 1_u16), start), 1);
        assert_eq!(
            clicks.register((1, 2), start + Duration::from_millis(50)),
            1
        );
        let late =
            start + Duration::from_millis(50) + MULTI_CLICK_WINDOW + Duration::from_millis(1);
        assert_eq!(clicks.register((1, 2), late), 1);
        // Exactly at the window edge still counts.
        assert_eq!(clicks.register((1, 2), late + MULTI_CLICK_WINDOW), 2);
    }
}
