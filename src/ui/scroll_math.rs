// SPDX-License-Identifier: MPL-2.0
//! Vertical scrollbar geometry. History uses distance from the newest message:
//! zero is the bottom of the track, `max_scroll` is the oldest visible position.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    Thumb,
}

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub button: i32,
    pub height: i32,
    pub track_top: i32,
    pub track_bottom: i32,
    pub thumb_top: i32,
    pub thumb_bottom: i32,
    pub max_scroll: i32,
}

impl Geometry {
    pub fn new(
        height: i32,
        button: i32,
        min_thumb: i32,
        page: i32,
        max_scroll: i32,
        scroll: i32,
    ) -> Self {
        let height = height.max(0);
        let button = button.max(0).min(height / 2);
        let track_top = button;
        let track_bottom = height - button;
        let track_height = track_bottom - track_top;
        let max_scroll = max_scroll.max(0);
        let total = (page.max(1) as i64 + max_scroll as i64).max(1);
        let thumb_height = ((track_height as i64 * page.max(1) as i64 / total) as i32)
            .max(min_thumb.max(1))
            .min(track_height);
        let travel = track_height - thumb_height;
        let thumb_top = track_top
            + if max_scroll == 0 {
                0
            } else {
                ((max_scroll - scroll.clamp(0, max_scroll)) as i64 * travel as i64
                    / max_scroll as i64) as i32
            };
        Self {
            button,
            height,
            track_top,
            track_bottom,
            thumb_top,
            thumb_bottom: thumb_top + thumb_height,
            max_scroll,
        }
    }

    pub fn hit(self, y: i32) -> Option<Action> {
        if self.max_scroll == 0 || y < 0 || y >= self.height {
            None
        } else if y < self.button {
            Some(Action::Up)
        } else if y >= self.track_bottom {
            Some(Action::Down)
        } else if y < self.thumb_top {
            Some(Action::PageUp)
        } else if y >= self.thumb_bottom {
            Some(Action::PageDown)
        } else {
            Some(Action::Thumb)
        }
    }

    pub fn scroll_for_thumb(self, top: i32) -> i32 {
        let travel = self.track_bottom - self.track_top - (self.thumb_bottom - self.thumb_top);
        if self.max_scroll == 0 || travel <= 0 {
            return 0;
        }
        let offset = (top - self.track_top).clamp(0, travel);
        self.max_scroll - (offset as i64 * self.max_scroll as i64 / travel as i64) as i32
    }
}

/// Pin the newest conversation to the lower edge. Removing an older row then
/// leaves every surviving row at its previous position, including short logs.
pub fn content_origin(bottom: i32, content_height: i32, scroll: i32) -> i32 {
    bottom.saturating_sub(content_height).saturating_add(scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiring_oldest_rows_never_moves_the_surviving_conversations() {
        for bottom in [120, 500] {
            // Cover histories both shorter and taller than the visible area.
            for oldest_height in [30, 180] {
                let remaining_height = 90;
                let before = content_origin(bottom, oldest_height + remaining_height, 0);
                let survivor_before = before + oldest_height;
                let survivor_after = content_origin(bottom, remaining_height, 0);
                assert_eq!(survivor_before, survivor_after);
                assert_eq!(survivor_after + remaining_height, bottom);
            }
        }
    }

    #[test]
    fn dragging_reaches_oldest_and_newest_without_reversing_history() {
        let newest = Geometry::new(300, 22, 24, 200, 800, 0);
        assert_eq!(newest.thumb_bottom, newest.track_bottom);
        assert_eq!(newest.scroll_for_thumb(newest.track_top), 800);
        assert_eq!(newest.scroll_for_thumb(300), 0);
        let oldest = Geometry::new(300, 22, 24, 200, 800, 800);
        assert_eq!(oldest.thumb_top, oldest.track_top);
        assert_eq!(oldest.scroll_for_thumb(-50), 800);
        let middle = Geometry::new(300, 22, 24, 200, 800, 400);
        assert!((middle.scroll_for_thumb(middle.thumb_top) - 400).abs() <= 4);
    }

    #[test]
    fn controls_disable_without_overflow_and_keep_minimum_thumb_for_long_history() {
        let fitting = Geometry::new(240, 22, 24, 200, 0, 0);
        assert_eq!(fitting.thumb_top, fitting.track_top);
        assert_eq!(fitting.thumb_bottom, fitting.track_bottom);
        assert_eq!(fitting.hit(5), None);
        let long = Geometry::new(240, 22, 24, 200, 2_000_000, 0);
        assert_eq!(long.thumb_bottom - long.thumb_top, 24);
        assert_eq!(long.hit(5), Some(Action::Up));
        assert_eq!(long.hit(239), Some(Action::Down));
        assert_eq!(long.hit(long.track_top + 1), Some(Action::PageUp));
        assert_eq!(long.hit(long.thumb_bottom - 1), Some(Action::Thumb));
        let tiny = Geometry::new(10, 22, 24, 200, 100, 50);
        assert!(tiny.thumb_bottom >= tiny.thumb_top);
        assert_eq!(tiny.scroll_for_thumb(5), 0);
    }
}
