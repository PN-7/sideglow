// SPDX-License-Identifier: MPL-2.0
//! Pure pixel and drag geometry used by the native overlay.
//! Can be tested independently on a non-Windows build host with `rustc --test`.

pub const LEFT: u8 = 1;
pub const RIGHT: u8 = 2;
pub const TOP: u8 = 4;
pub const BOTTOM: u8 = 8;

/// The colored GDI pass was drawn over black and already contains coverage-
/// weighted RGB. Copy coverage from the white mask, without multiplying twice.
pub fn apply_coverage(color: &mut [u32], mask: &[u32]) {
    assert_eq!(color.len(), mask.len());
    for (pixel, mask) in color.iter_mut().zip(mask) {
        let alpha = (mask & 255).max((mask >> 8) & 255).max((mask >> 16) & 255);
        let blue = (*pixel & 255).min(alpha);
        let green = ((*pixel >> 8) & 255).min(alpha);
        let red = ((*pixel >> 16) & 255).min(alpha);
        *pixel = (alpha << 24) | (red << 16) | (green << 8) | blue;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy)]
pub struct Drag {
    pub rect: Rect,
    pub pointer_x: i32,
    pub pointer_y: i32,
    /// Zero moves the whole rectangle; nonzero bits resize selected edges.
    pub edges: u8,
}

impl Drag {
    pub fn update(self, x: i32, y: i32, min_width: i32, min_height: i32) -> Rect {
        let dx = x.saturating_sub(self.pointer_x);
        let dy = y.saturating_sub(self.pointer_y);
        let mut r = self.rect;
        if self.edges == 0 {
            r.left = r.left.saturating_add(dx);
            r.right = r.right.saturating_add(dx);
            r.top = r.top.saturating_add(dy);
            r.bottom = r.bottom.saturating_add(dy);
            return r;
        }
        let min_width = min_width.clamp(1, 3840);
        let min_height = min_height.clamp(1, 2160);
        if self.edges & LEFT != 0 {
            r.left = r.left.saturating_add(dx).clamp(
                r.right.saturating_sub(3840),
                r.right.saturating_sub(min_width),
            );
        }
        if self.edges & RIGHT != 0 {
            r.right = r.right.saturating_add(dx).clamp(
                r.left.saturating_add(min_width),
                r.left.saturating_add(3840),
            );
        }
        if self.edges & TOP != 0 {
            r.top = r.top.saturating_add(dy).clamp(
                r.bottom.saturating_sub(2160),
                r.bottom.saturating_sub(min_height),
            );
        }
        if self.edges & BOTTOM != 0 {
            r.bottom = r
                .bottom
                .saturating_add(dy)
                .clamp(r.top.saturating_add(min_height), r.top.saturating_add(2160));
        }
        r
    }
}

pub fn hit_edges(x: i32, y: i32, width: i32, height: i32, edge: i32) -> u8 {
    let mut result = 0;
    if x < edge {
        result |= LEFT;
    } else if x >= width - edge {
        result |= RIGHT;
    }
    if y < edge {
        result |= TOP;
    } else if y >= height - edge {
        result |= BOTTOM;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_keeps_black_glyphs_and_does_not_premultiply_twice() {
        let mut colors = [0x00800000, 0, 0x00ffffff, 0x00102030];
        apply_coverage(&mut colors, &[0x00808080, 0x00808080, 0, 0x00ffffff]);
        assert_eq!(colors, [0x80800000, 0x80000000, 0, 0xff102030]);
    }

    #[test]
    fn left_top_resize_keeps_opposite_corner_and_clamps_minimum() {
        let drag = Drag {
            rect: Rect {
                left: 100,
                top: 100,
                right: 600,
                bottom: 500,
            },
            pointer_x: 100,
            pointer_y: 100,
            edges: LEFT | TOP,
        };
        assert_eq!(
            drag.update(550, 450, 340, 200),
            Rect {
                left: 260,
                top: 300,
                right: 600,
                bottom: 500
            }
        );
    }

    #[test]
    fn resize_limits_match_saved_settings_and_move_preserves_size() {
        let drag = Drag {
            rect: Rect {
                left: -500,
                top: 20,
                right: 0,
                bottom: 420,
            },
            pointer_x: 0,
            pointer_y: 420,
            edges: RIGHT | BOTTOM,
        };
        assert_eq!(
            drag.update(10000, 10000, 340, 200),
            Rect {
                left: -500,
                top: 20,
                right: 3340,
                bottom: 2180
            }
        );
        let moved = Drag { edges: 0, ..drag }.update(-200, 220, 340, 200);
        assert_eq!(
            moved,
            Rect {
                left: -700,
                top: -180,
                right: -200,
                bottom: 220
            }
        );
        assert_eq!(hit_edges(1, 199, 500, 200, 7), LEFT | BOTTOM);
        assert_eq!(hit_edges(250, 100, 500, 200, 7), 0);
    }
}
