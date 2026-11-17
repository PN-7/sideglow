// SPDX-License-Identifier: MPL-2.0
//! Compact management-only channel tabs. No native windows or background work.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Select(usize),
    Add,
}

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub action: Action,
}

pub struct Layout {
    pub cells: Vec<Cell>,
    pub bottom: i32,
}

impl Layout {
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        self.cells
            .iter()
            .find(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            .map(|r| r.action)
    }
}

pub fn layout(width: i32, dpi: u32, count: usize) -> Layout {
    let s = |v: i32| (i64::from(v) * i64::from(dpi.max(1)) / 96) as i32;
    let count = count.clamp(1, crate::config::MAX_CHANNEL_TABS);
    let items = count + 1;
    let margin = s(12);
    let gap = s(6).max(1);
    let available = (width - margin * 2).max(items as i32);
    let columns = ((available + gap) / (s(108).max(1) + gap))
        .max(1)
        .min(items as i32);
    let cell_width = ((available - (columns - 1) * gap) / columns).min(s(148).max(1));
    let height = s(24).max(1);
    let row_gap = s(4).max(1);
    let top = s(37);
    let cells = (0..items)
        .map(|i| {
            let left = margin + (i as i32 % columns) * (cell_width + gap);
            let y = top + (i as i32 / columns) * (height + row_gap);
            Cell {
                left,
                top: y,
                right: left
                    + if i == count {
                        cell_width.min(s(32).max(1))
                    } else {
                        cell_width
                    },
                bottom: y + height,
                action: if i == count {
                    Action::Add
                } else {
                    Action::Select(i)
                },
            }
        })
        .collect::<Vec<_>>();
    let bottom = cells.last().map_or(top, |c| c.bottom) + s(4);
    Layout { cells, bottom }
}

/// Keep a usable message viewport and both footer lines beneath wrapped tabs.
pub fn minimum_height(width: i32, dpi: u32, count: usize) -> i32 {
    let s = |v: i32| (i64::from(v) * i64::from(dpi.max(1)) / 96) as i32;
    s(200).max(layout(width, dpi, count).bottom + s(120))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_tabs_remain_reachable_at_small_widths_and_high_dpi() {
        for dpi in [96, 120, 144, 192] {
            let width = 340 * dpi as i32 / 96;
            for count in 1..=crate::config::MAX_CHANNEL_TABS {
                let l = layout(width, dpi, count);
                assert_eq!(l.cells.len(), count + 1);
                assert!(
                    minimum_height(width, dpi, count) - l.bottom - 56 * dpi as i32 / 96
                        >= 64 * dpi as i32 / 96
                );
                for (i, c) in l.cells.iter().enumerate() {
                    assert!(c.left >= 0 && c.right <= width && c.right > c.left);
                    assert!(c.top >= 34 * dpi as i32 / 96 && c.bottom < l.bottom);
                    assert_eq!(l.hit(c.left, c.top), Some(c.action));
                    for other in &l.cells[i + 1..] {
                        assert!(
                            c.right <= other.left
                                || other.right <= c.left
                                || c.bottom <= other.top
                                || other.bottom <= c.top
                        );
                    }
                }
            }
        }
    }
}
