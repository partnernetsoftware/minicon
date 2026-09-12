//! Scrollbar geometry and pointer mapping.
//!
//! A scrollbar is the one piece of terminal chrome whose *arithmetic* is easy to
//! get subtly wrong and hard to notice: a thumb that is one pixel off, a drag
//! that drifts by one row per screen, a clamped value that jumps at the ends.
//! None of that is platform-specific, so it lives here where it can be tested
//! directly rather than through a window.
//!
//! The two directions have to agree. `terminal_scrollbar_geometry` turns a
//! scroll offset into a thumb position, and `scrollback_for_thumb_top` turns a
//! thumb position back into a scroll offset. They are inverse *by intent*, and a
//! test checks that round trip, because a drag that does not return to where it
//! started is the bug a user notices immediately and a developer almost never
//! reproduces.

/// The shortest a thumb may be drawn, regardless of how large the scrollback is.
///
/// Proportional sizing alone makes a thumb unreachable with a large scrollback,
/// which is precisely when a user needs to grab it.
const MIN_THUMB_HEIGHT: i32 = 24;

/// An axis-aligned integer rectangle.
///
/// Distinct from any pixel-buffer rectangle: this one is about coordinates in a
/// window, which is why it is `i32` (a scrollbar can be laid out at a negative
/// origin during a resize) rather than unsigned.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScrollbarRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScrollbarRect {
    pub const fn width(self) -> i32 {
        self.right - self.left
    }

    pub const fn height(self) -> i32 {
        self.bottom - self.top
    }

    /// Half-open on both axes: the right and bottom edges are outside. A
    /// degenerate or inverted rectangle contains nothing rather than everything.
    pub const fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScrollbarGeometry {
    pub track: ScrollbarRect,
    pub thumb: ScrollbarRect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollbarHit {
    Thumb,
    TrackAbove,
    TrackBelow,
}

/// A thumb drag that remembers where inside the thumb it was grabbed.
///
/// Without the grab offset the thumb jumps so its top edge meets the pointer,
/// which feels like the scrollbar slipped out from under the cursor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScrollbarThumbDrag {
    grab: i32,
}

impl ScrollbarThumbDrag {
    pub const fn begin(pointer_y: i32, thumb_top: i32) -> Self {
        Self {
            grab: pointer_y - thumb_top,
        }
    }

    pub const fn thumb_top(self, pointer_y: i32) -> i32 {
        pointer_y - self.grab
    }
}

/// Lays out a vertical scrollbar in `terminal`.
///
/// `visible` is the number of rows on screen, `offset` is how far scrolled back
/// and `maximum` the largest offset. The thumb sits at the bottom when the view
/// is at the live end of the buffer, and at the top when scrolled fully back, so
/// `maximum == 0` means "nothing to scroll" and fills the track.
pub fn terminal_scrollbar_geometry(
    terminal: ScrollbarRect,
    width: i32,
    visible: usize,
    offset: usize,
    maximum: usize,
) -> ScrollbarGeometry {
    let track = ScrollbarRect {
        left: (terminal.right - width.max(0)).max(terminal.left),
        ..terminal
    };
    let height = track.height().max(0);
    // `visible + maximum` can exceed a `usize` on a hostile terminal size, and a
    // zero total would divide by zero, so both are guarded here rather than at
    // each use.
    let total = visible.saturating_add(maximum).max(1);
    let proportional = (i64::from(height) * visible.max(1) as i64 / total as i64) as i32;
    let thumb_height = if maximum == 0 {
        height
    } else {
        proportional.max(MIN_THUMB_HEIGHT).min(height)
    };
    let travel = (height - thumb_height).max(0);
    let from_bottom = if maximum == 0 {
        0
    } else {
        // Scaled through `i64`: `offset * travel` overflows `i32` for a tall
        // track and a large scrollback, which is a real configuration and not a
        // hostile one.
        (offset.min(maximum) as i64 * i64::from(travel) / maximum as i64) as i32
    };
    let top = track.bottom - thumb_height - from_bottom;
    ScrollbarGeometry {
        track,
        thumb: ScrollbarRect {
            // Inset two pixels so the thumb does not sit flush against the
            // track's own edges. The `.min`/`.max` keep the rectangle ordered
            // when the track is narrower than the inset, where a naive
            // subtraction produces a right edge left of the left edge.
            left: (track.left + 2).min(track.right),
            top,
            right: (track.right - 2).max((track.left + 2).min(track.right)),
            bottom: top + thumb_height,
        },
    }
}

/// Maps a thumb top position back to a scrollback offset.
///
/// Rounding to nearest rather than truncating keeps the drag reversible: with
/// truncation a thumb moved down and back up lands one row short, and repeated
/// drags drift.
pub fn scrollback_for_thumb_top(geometry: ScrollbarGeometry, top: i32, maximum: usize) -> usize {
    let travel = geometry.track.height() - geometry.thumb.height();
    if maximum == 0 || travel <= 0 {
        return 0;
    }
    let top = top.clamp(
        geometry.track.top,
        geometry.track.bottom - geometry.thumb.height(),
    );
    let from_bottom = geometry.track.bottom - geometry.thumb.height() - top;
    ((i64::from(from_bottom) * maximum as i64 + i64::from(travel) / 2) / i64::from(travel)) as usize
}

/// Which part of the scrollbar a pointer is over, if any.
///
/// The thumb is tested before the track halves so a click that lands on the
/// thumb starts a drag rather than a page jump.
pub fn scrollbar_hit_test(
    geometry: &ScrollbarGeometry,
    x: i32,
    y: i32,
) -> Option<ScrollbarHit> {
    if !geometry.track.contains(x, y) {
        None
    } else if geometry.thumb.contains(x, y) {
        Some(ScrollbarHit::Thumb)
    } else if y < geometry.thumb.top {
        Some(ScrollbarHit::TrackAbove)
    } else {
        Some(ScrollbarHit::TrackBelow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> ScrollbarRect {
        ScrollbarRect {
            left: 200,
            top: 0,
            right: 1000,
            bottom: 600,
        }
    }

    #[test]
    fn thumb_positions_span_the_track_from_bottom_to_top() {
        let bottom = terminal_scrollbar_geometry(rect(), 12, 30, 0, 90);
        let middle = terminal_scrollbar_geometry(rect(), 12, 30, 45, 90);
        let top = terminal_scrollbar_geometry(rect(), 12, 30, 90, 90);
        assert_eq!(bottom.thumb.bottom, bottom.track.bottom);
        assert_eq!(top.thumb.top, top.track.top);
        assert!(middle.thumb.top < bottom.thumb.top);
        assert!(middle.thumb.top > top.thumb.top);
        // The thumb's vertical extent never changes while scrolling.
        assert_eq!(bottom.thumb.height(), middle.thumb.height());
        assert_eq!(middle.thumb.height(), top.thumb.height());
    }

    #[test]
    fn thumb_position_round_trips_through_the_scroll_offset() {
        for maximum in [1_usize, 7, 90, 1000] {
            let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 0, maximum);
            for offset in [0, maximum / 3, maximum / 2, maximum] {
                let drawn = terminal_scrollbar_geometry(rect(), 12, 30, offset, maximum);
                let back = scrollback_for_thumb_top(geometry, drawn.thumb.top, maximum);
                // Rounding may move a value by at most a row's worth of travel;
                // anything larger means the two directions disagree.
                let tolerance = (maximum / 30).max(1);
                assert!(
                    back.abs_diff(offset) <= tolerance,
                    "offset {offset} of {maximum} came back as {back}"
                );
            }
        }
    }

    #[test]
    fn a_full_track_thumb_never_scrolls() {
        let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 0, 0);
        assert_eq!(geometry.thumb.top, geometry.track.top);
        assert_eq!(geometry.thumb.height(), geometry.track.height());
        assert_eq!(scrollback_for_thumb_top(geometry, 0, 0), 0);
    }

    /// A track shorter than the initial carve-out would otherwise produce a
    /// rectangle whose right edge is left of its left edge, and a negative
    /// height would make every downstream comparison nonsense.
    #[test]
    fn a_track_narrower_than_the_inset_stays_ordered() {
        for width in 0..8 {
            for track_width in 0..8 {
                let narrow = ScrollbarRect {
                    left: 0,
                    top: 0,
                    right: track_width,
                    bottom: 40,
                };
                let geometry = terminal_scrollbar_geometry(narrow, width, 10, 0, 10);
                assert!(
                    geometry.thumb.right >= geometry.thumb.left,
                    "width {width} track {track_width}"
                );
                assert!(geometry.thumb.bottom >= geometry.thumb.top);
            }
        }
    }

    /// The track is passed through as given, so an inverted rectangle stays
    /// inverted: `height()` is negative rather than clamped to zero. Callers
    /// therefore cannot treat a non-negative `height()` as an invariant they
    /// get for free, and the drawing path has to guard its own arithmetic.
    /// Recorded here because it is surprising and easy to assume otherwise.
    #[test]
    fn an_inverted_track_is_passed_through_with_a_negative_height() {
        let inverted = ScrollbarRect {
            left: 100,
            top: 60,
            right: 40,
            bottom: 0,
        };
        let geometry = terminal_scrollbar_geometry(inverted, 12, 10, 0, 10);
        assert_eq!(geometry.track, inverted);
        assert_eq!(geometry.track.height(), -60);
        // The clamped locals still keep the thumb degenerate rather than
        // producing a rectangle that wraps.
        assert_eq!(geometry.thumb.height(), 0);
        assert_eq!(scrollback_for_thumb_top(geometry, 0, 10), 0);
    }

    /// A zero-height track has no area for a thumb, and neither direction may
    /// divide by its zero travel.
    #[test]
    fn an_empty_track_yields_no_usable_thumb() {
        let empty = ScrollbarRect {
            left: 0,
            top: 100,
            right: 20,
            bottom: 100,
        };
        let geometry = terminal_scrollbar_geometry(empty, 12, 10, 0, 10);
        assert_eq!(geometry.track.height(), 0);
        assert_eq!(geometry.thumb.height(), 0);
        assert_eq!(scrollback_for_thumb_top(geometry, 0, 10), 0);
    }

    #[test]
    fn an_oversized_offset_is_clamped_to_the_maximum() {
        let at_max = terminal_scrollbar_geometry(rect(), 12, 30, 90, 90);
        let over = terminal_scrollbar_geometry(rect(), 12, 30, usize::MAX, 90);
        assert_eq!(at_max.thumb.top, over.thumb.top);
    }

    /// The thumb is tested before the track halves so a click on the thumb
    /// starts a drag. With the view at the live end the thumb covers the bottom
    /// of the track, so there is deliberately no `TrackBelow` to hit — a click
    /// below the thumb can only exist once the view is scrolled back.
    #[test]
    fn hit_testing_prefers_the_thumb_and_names_the_track_half() {
        // Scrolled back halfway, so the thumb has track on both sides.
        let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 45, 90);
        let x = geometry.thumb.left;
        let y = geometry.thumb.top + geometry.thumb.height() / 2;
        assert_eq!(scrollbar_hit_test(&geometry, x, y), Some(ScrollbarHit::Thumb));
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.thumb.top - 1),
            Some(ScrollbarHit::TrackAbove)
        );
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.thumb.bottom),
            Some(ScrollbarHit::TrackBelow)
        );
        // A point outside the track is not a hit at all, so a click in the
        // terminal body is not swallowed by the scrollbar.
        assert_eq!(
            scrollbar_hit_test(&geometry, geometry.track.left - 1, y),
            None
        );
        assert_eq!(scrollbar_hit_test(&geometry, x, geometry.track.bottom), None);
    }

    /// At the live end the thumb rests on the track's bottom edge, so the whole
    /// track above it is `TrackAbove` and nothing is below.
    #[test]
    fn a_live_end_thumb_has_no_track_below_it() {
        let geometry = terminal_scrollbar_geometry(rect(), 12, 30, 0, 90);
        let x = geometry.thumb.left;
        assert_eq!(geometry.thumb.bottom, geometry.track.bottom);
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.track.top),
            Some(ScrollbarHit::TrackAbove)
        );
        assert_eq!(
            scrollbar_hit_test(&geometry, x, geometry.thumb.bottom),
            None,
            "the bottom edge of the track is outside it"
        );
    }

    #[test]
    fn a_drag_keeps_the_grab_offset_so_the_thumb_does_not_jump() {
        let drag = ScrollbarThumbDrag::begin(500, 480);
        assert_eq!(drag.thumb_top(500), 480);
        assert_eq!(drag.thumb_top(520), 500);
        assert_eq!(drag.thumb_top(480), 460);
    }
}
