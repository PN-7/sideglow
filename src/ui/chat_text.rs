// SPDX-License-Identifier: MPL-2.0
//! Measured chat text with native glyph-path strokes and supersampled coverage.
//! Wrapping, paths and glyph fill use the same font and UTF-16 advances. A
//! Surface-owned scratch bitmap/DC and geometric pen are reused across draws.

/// Mix disjoint foreground and outer-stroke coverage before compositing once.
/// Repeatedly alpha-blending translated text masks makes small strokes darker
/// and wider; here the total coverage is measured only once by area sampling.
pub(super) fn blend_glyph_pixel(
    destination_rgb: u32,
    destination_mask: u32,
    foreground: u8,
    total: u8,
    text_rgb: u32,
    outline_rgb: u32,
) -> (u32, u32) {
    let foreground = u32::from(foreground);
    let total = u32::from(total).max(foreground);
    let outer = total - foreground;
    let inverse = 255 - total;
    let old_alpha = (destination_mask & 255)
        .max((destination_mask >> 8) & 255)
        .max((destination_mask >> 16) & 255);
    let alpha = total + (old_alpha * inverse + 127) / 255;
    let mut rgb = 0;
    for shift in [0, 8, 16] {
        let source = (((text_rgb >> shift) & 255) * foreground
            + ((outline_rgb >> shift) & 255) * outer
            + 127)
            / 255;
        let channel = source + (((destination_rgb >> shift) & 255) * inverse + 127) / 255;
        rgb |= channel.min(alpha) << shift;
    }
    (rgb, alpha * 0x00010101)
}

pub(super) fn coverage_4x(foreground_sum: u32, total_sum: u32) -> (u8, u8) {
    let foreground = ((foreground_sum.min(16 * 255) + 8) / 16) as u8;
    let total = ((total_sum.min(16 * 255) + 8) / 16) as u8;
    (foreground, total.max(foreground))
}

fn visible_lines(
    base: i32,
    font_height: i32,
    stride: i32,
    padding: i32,
    clip_top: i32,
    clip_bottom: i32,
    count: usize,
) -> std::ops::Range<usize> {
    if clip_bottom <= clip_top || count == 0 {
        return 0..0;
    }
    let stride = i64::from(stride.max(1));
    let first =
        ((i64::from(clip_top) - i64::from(base) - i64::from(font_height) - i64::from(padding))
            .div_euclid(stride)
            + 1)
        .clamp(0, count as i64) as usize;
    let end = ((i64::from(clip_bottom) - i64::from(base) + i64::from(padding) - 1)
        .div_euclid(stride)
        + 1)
    .clamp(first as i64, count as i64) as usize;
    first..end
}

#[derive(Clone, Copy, Debug)]
struct Cluster {
    end: usize,
    first: char,
    last: char,
}

fn extend(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036f | 0x0483..=0x0489 | 0x0591..=0x05bd | 0x05bf |
        0x05c1..=0x05c2 | 0x05c4..=0x05c5 | 0x05c7 | 0x0610..=0x061a |
        0x064b..=0x065f | 0x0670 | 0x06d6..=0x06ed | 0x0711 | 0x0730..=0x074a |
        0x07a6..=0x07b0 | 0x07eb..=0x07f3 | 0x0816..=0x082d | 0x0859..=0x085b |
        0x08d3..=0x0903 | 0x093a..=0x094f | 0x0951..=0x0957 | 0x0962..=0x0963 |
        0x0981..=0x0983 | 0x09bc | 0x09be..=0x09cd | 0x09d7 | 0x09e2..=0x09e3 |
        0x0a01..=0x0a03 | 0x0a3c | 0x0a3e..=0x0a4d | 0x0a70..=0x0a71 |
        0x0a81..=0x0a83 | 0x0abc | 0x0abe..=0x0acd | 0x0ae2..=0x0ae3 |
        0x0b01..=0x0b03 | 0x0b3c | 0x0b3e..=0x0b57 | 0x0b62..=0x0b63 |
        0x0b82 | 0x0bbe..=0x0bcd | 0x0bd7 | 0x0c00..=0x0c04 | 0x0c3e..=0x0c56 |
        0x0c62..=0x0c63 | 0x0c81..=0x0c83 | 0x0cbc | 0x0cbe..=0x0cd6 |
        0x0ce2..=0x0ce3 | 0x0d00..=0x0d03 | 0x0d3b..=0x0d4d | 0x0d57 |
        0x0d62..=0x0d63 | 0x0d81..=0x0d83 | 0x0dca | 0x0dcf..=0x0ddf |
        0x0df2..=0x0df3 | 0x0e31 | 0x0e33..=0x0e3a | 0x0e47..=0x0e4e |
        0x0eb1 | 0x0eb3..=0x0ebc | 0x0ec8..=0x0ece | 0x0f18..=0x0f19 |
        0x0f35 | 0x0f37 | 0x0f39 | 0x0f71..=0x0f84 | 0x0f86..=0x0f87 |
        0x0f8d..=0x0fbc | 0x102b..=0x103e | 0x1056..=0x1059 | 0x105e..=0x1060 |
        0x1712..=0x1715 | 0x1732..=0x1734 | 0x17b4..=0x17d3 | 0x17dd |
        0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff |
        0x302a..=0x302f | 0x3099..=0x309a | 0xfe00..=0xfe0f | 0xfe20..=0xfe2f |
        0x1f3fb..=0x1f3ff | 0xe0020..=0xe007f | 0xe0100..=0xe01ef)
}

fn hangul_kind(c: char) -> u8 {
    match c as u32 {
        0x1100..=0x115f | 0xa960..=0xa97c => 1,
        0x1160..=0x11a7 | 0xd7b0..=0xd7c6 => 2,
        0x11a8..=0x11ff | 0xd7cb..=0xd7fb => 3,
        n @ 0xac00..=0xd7a3 if (n - 0xac00) % 28 == 0 => 4,
        0xac00..=0xd7a3 => 5,
        _ => 0,
    }
}

fn joins_hangul(a: char, b: char) -> bool {
    matches!(
        (hangul_kind(a), hangul_kind(b)),
        (1, 1 | 2 | 4 | 5) | (2 | 4, 2 | 3) | (3 | 5, 3)
    )
}

fn regional(c: char) -> bool {
    matches!(c as u32, 0x1f1e6..=0x1f1ff)
}

/// A deliberately small grapheme-like boundary routine for layout, not a
/// Unicode normalization implementation. Native ExtTextOut retains Uniscribe
/// shaping/fallback for the complete line; we never draw one UTF-16 unit at a time.
fn clusters(text: &str) -> Vec<Cluster> {
    let mut result: Vec<Cluster> = Vec::new();
    let mut units = 0;
    let mut previous = '\0';
    let mut regional_count = 0;
    for c in text.chars() {
        units += c.len_utf16();
        let join = !result.is_empty()
            && (extend(c)
                || c == '\u{200d}'
                || previous == '\u{200d}'
                || joins_hangul(previous, c)
                || (regional(c) && regional(previous) && regional_count % 2 == 1));
        if join {
            let last = result.last_mut().unwrap();
            last.end = units;
            last.last = c;
        } else {
            result.push(Cluster {
                end: units,
                first: c,
                last: c,
            });
        }
        regional_count = if regional(c) { regional_count + 1 } else { 0 };
        previous = c;
    }
    result
}

fn whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{200b}'
}
fn cjk(c: char) -> bool {
    matches!(c as u32, 0x2e80..=0xa4cf | 0xac00..=0xd7ff | 0xf900..=0xfaff | 0x1f000..=0x1ffff | 0x20000..=0x323af)
}
fn break_between(a: &Cluster, b: &Cluster) -> bool {
    if whitespace(a.first) || matches!(a.last, '-' | '\u{2010}' | '\u{2013}' | '/') {
        return true;
    }
    (cjk(a.first) || cjk(b.first))
        && !"([{〈《「『【〔（［｛".contains(a.last)
        && !")]},.!?:;〉》」』】〕）、。，．！？：；］｝".contains(b.first)
}

/// Return how many clusters to display and consume. Whitespace at a soft
/// wrap is consumed, but explicit paragraph indentation is otherwise retained.
fn line_end(items: &[Cluster], widths: &[i32], width: i32, more: bool) -> (usize, usize) {
    if items.is_empty() {
        return (0, 0);
    }
    let fit = widths
        .iter()
        .take_while(|&&w| w <= width)
        .count()
        .max(1)
        .min(items.len());
    if fit == items.len() && !more {
        return (fit, fit);
    }
    let mut end = (1..=fit)
        .rev()
        .find(|&i| i < items.len() && break_between(&items[i - 1], &items[i]))
        .unwrap_or(fit);
    while end > 0 && whitespace(items[end - 1].first) {
        end -= 1;
    }
    if end == 0 {
        end = fit;
    }
    let mut consumed = end;
    while consumed < items.len() && whitespace(items[consumed].first) {
        consumed += 1;
    }
    (end, consumed)
}

/// GetTextExtentExPoint gives cumulative UTF-16 extents. ExtTextOut wants
/// individual advances. Add tracking only between whole clusters and never
/// to the final cluster; signed tracking cannot produce backward progress.
fn advances(cumulative: &[i32], items: &[Cluster], spacing: i32) -> (Vec<i32>, Vec<i32>) {
    let mut dx = Vec::with_capacity(cumulative.len());
    let mut previous = 0;
    for &n in cumulative {
        dx.push(n - previous);
        previous = n;
    }
    let mut widths = Vec::with_capacity(items.len());
    let mut extra = 0;
    let mut start = 0;
    for (i, item) in items.iter().enumerate() {
        let raw = cumulative[item.end - 1];
        widths.push(raw.saturating_add(extra));
        if i + 1 < items.len() {
            let before = if start == 0 { 0 } else { cumulative[start - 1] };
            let step = spacing.max(1 - (raw - before));
            dx[item.end - 1] = dx[item.end - 1].saturating_add(step);
            extra = extra.saturating_add(step);
        }
        start = item.end;
    }
    (dx, widths)
}

#[cfg(windows)]
mod native {
    use super::{advances, clusters, line_end, visible_lines, Cluster};
    use crate::ui::surface::Surface;
    use std::{
        mem::zeroed,
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{COLORREF, RECT, SIZE},
        Graphics::Gdi::*,
    };

    const MAX_RUN: usize = 8192; // ExtTextOutW's documented per-call limit.
    const MAX_OUTLINE: i32 = 8;

    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Outline {
        pub color: COLORREF,
        pub radius_px: i32,
    }
    #[derive(Clone, Copy, Debug, Default)]
    pub(crate) struct TextStyle {
        pub letter_spacing_px: i32,
        pub line_gap_px: i32,
        pub outline: Option<Outline>,
    }
    struct Line {
        text: Vec<u16>,
        dx: Vec<i32>,
    }
    pub(crate) struct Layout {
        lines: Vec<Line>,
        style: TextStyle,
        font: HGDIOBJ,
        font_height: i32,
        width: i32,
        height: i32,
        padding: i32,
    }
    impl Layout {
        pub(crate) fn height(&self) -> i32 {
            self.height
        }
        pub(crate) fn line_height(&self) -> i32 {
            self.font_height.saturating_add(self.padding * 2)
        }
    }

    fn normalized(value: &str) -> String {
        value
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ")
    }

    unsafe fn prepare(dc: HDC, width: i32, mut style: TextStyle) -> Layout {
        style.letter_spacing_px = style.letter_spacing_px.clamp(-32, 128);
        style.line_gap_px = style.line_gap_px.clamp(0, 256);
        style.outline = style.outline.and_then(|mut outline| {
            outline.radius_px = outline.radius_px.clamp(0, MAX_OUTLINE);
            (outline.radius_px > 0).then_some(outline)
        });
        let mut metric: TEXTMETRICW = zeroed();
        let font_height = if GetTextMetricsW(dc, &mut metric) != 0 {
            metric.tmHeight.max(1)
        } else {
            16
        };
        let padding = style.outline.map_or(0, |o| o.radius_px);
        Layout {
            lines: Vec::new(),
            style,
            font: GetCurrentObject(dc, OBJ_FONT as u32),
            font_height,
            width: width.max(1),
            height: 0,
            padding,
        }
    }

    unsafe fn measure_run(
        dc: HDC,
        text: &[u16],
        items: &[Cluster],
        spacing: i32,
        fallback: i32,
    ) -> (Line, Vec<i32>) {
        let mut cumulative = vec![0; text.len()];
        let mut size: SIZE = zeroed();
        if !text.is_empty()
            && GetTextExtentExPointW(
                dc,
                text.as_ptr(),
                text.len() as i32,
                i32::MAX,
                std::ptr::null_mut(),
                cumulative.as_mut_ptr(),
                &mut size,
            ) == 0
        {
            // A bounded fallback keeps the same advances in measure and draw.
            let mut begin = 0;
            for (i, item) in items.iter().enumerate() {
                cumulative[begin..item.end].fill((i as i32 + 1) * fallback.max(1));
                begin = item.end;
            }
        }
        let (dx, widths) = advances(&cumulative, items, spacing);
        (
            Line {
                text: text.to_vec(),
                dx,
            },
            widths,
        )
    }

    fn relative(items: &[Cluster], offset: usize) -> Vec<Cluster> {
        items
            .iter()
            .map(|item| Cluster {
                end: item.end - offset,
                ..*item
            })
            .collect()
    }

    fn finish(mut out: Layout) -> Layout {
        let n = out.lines.len().max(1) as i32;
        out.height = out.line_height().saturating_add(
            (n - 1).saturating_mul(out.font_height.saturating_add(out.style.line_gap_px)),
        );
        out
    }

    pub(crate) unsafe fn layout(dc: HDC, value: &str, width: i32, style: TextStyle) -> Layout {
        let saved = SaveDC(dc);
        if saved != 0 {
            SetTextCharacterExtra(dc, 0);
            SetTextJustification(dc, 0, 0);
        }
        let mut out = prepare(dc, width, style);
        let available = (out.width - out.padding * 2).max(1);
        for paragraph in normalized(value).split('\n') {
            let units: Vec<u16> = paragraph.encode_utf16().collect();
            let all = clusters(paragraph);
            if all.is_empty() {
                out.lines.push(Line {
                    text: Vec::new(),
                    dx: Vec::new(),
                });
                continue;
            }
            let mut first = 0;
            while first < all.len() {
                let start = if first == 0 { 0 } else { all[first - 1].end };
                // A small adaptive lookahead avoids measuring the entire
                // remaining long paragraph once per visible line.
                let mut budget = 256;
                let (mut end, items, line, widths) = loop {
                    let end = all[first..]
                        .iter()
                        .take_while(|c| c.end - start <= budget)
                        .count()
                        + first;
                    let end = end.max(first + 1).min(all.len());
                    if all[end - 1].end - start > MAX_RUN {
                        let ellipsis = [0x2026];
                        let c = clusters("…");
                        let (line, widths) = measure_run(dc, &ellipsis, &c, 0, out.font_height / 2);
                        break (end, c, line, widths);
                    }
                    let items = relative(&all[first..end], start);
                    let (line, widths) = measure_run(
                        dc,
                        &units[start..all[end - 1].end],
                        &items,
                        out.style.letter_spacing_px,
                        out.font_height / 2,
                    );
                    if end == all.len()
                        || widths.last().copied().unwrap_or(0) > available
                        || budget >= MAX_RUN
                    {
                        break (end, items, line, widths);
                    }
                    budget = (budget * 2).min(MAX_RUN);
                };
                // Pathological >8192-unit clusters are represented by a single
                // ellipsis instead of splitting a surrogate/combining sequence.
                if all[end - 1].end - start > MAX_RUN {
                    out.lines.push(line);
                    first = end;
                    continue;
                }
                let (display, consume) = line_end(&items, &widths, available, end < all.len());
                end = first + display;
                let display_end = all[end - 1].end;
                if display == items.len() {
                    out.lines.push(line);
                } else {
                    let items = relative(&all[first..end], start);
                    let (line, _) = measure_run(
                        dc,
                        &units[start..display_end],
                        &items,
                        out.style.letter_spacing_px,
                        out.font_height / 2,
                    );
                    out.lines.push(line);
                }
                first += consume.max(1);
            }
        }
        if saved != 0 {
            RestoreDC(dc, saved);
        }
        finish(out)
    }

    pub(crate) unsafe fn single_line(dc: HDC, value: &str, width: i32, style: TextStyle) -> Layout {
        let saved = SaveDC(dc);
        if saved != 0 {
            SetTextCharacterExtra(dc, 0);
            SetTextJustification(dc, 0, 0);
        }
        let mut out = prepare(dc, width, style);
        let available = (out.width - out.padding * 2).max(1);
        let clean = normalized(value).replace('\n', " ");
        let units: Vec<u16> = clean.encode_utf16().collect();
        let all = clusters(&clean);
        let count = all.iter().take_while(|c| c.end < MAX_RUN).count();
        let end = if count == 0 { 0 } else { all[count - 1].end };
        let (whole, widths) = measure_run(
            dc,
            &units[..end],
            &all[..count],
            out.style.letter_spacing_px,
            out.font_height / 2,
        );
        if count == all.len() && widths.last().copied().unwrap_or(0) <= available {
            out.lines.push(whole);
        } else {
            let (ellipsis, ew) = measure_run(dc, &[0x2026], &clusters("…"), 0, out.font_height / 2);
            let ellipsis_width = ew[0];
            let mut keep = widths
                .iter()
                .take_while(|&&w| {
                    w.saturating_add(out.style.letter_spacing_px.max(0))
                        .saturating_add(ellipsis_width)
                        <= available
                })
                .count();
            loop {
                let at = if keep == 0 { 0 } else { all[keep - 1].end };
                let mut shortened = String::from_utf16_lossy(&units[..at]);
                shortened.push('…');
                let candidate: Vec<u16> = shortened.encode_utf16().collect();
                let (line, widths) = measure_run(
                    dc,
                    &candidate,
                    &clusters(&shortened),
                    out.style.letter_spacing_px,
                    out.font_height / 2,
                );
                if keep == 0 || widths.last().copied().unwrap_or(0) <= available {
                    out.lines.push(line);
                    break;
                }
                keep -= 1;
            }
            drop(ellipsis);
        }
        if saved != 0 {
            RestoreDC(dc, saved);
        }
        finish(out)
    }

    unsafe fn pass(dc: HDC, line: &Line, x: i32, y: i32) -> bool {
        if line.text.is_empty() {
            true
        } else {
            ExtTextOutW(
                dc,
                x,
                y,
                0,
                null(),
                line.text.as_ptr(),
                line.text.len() as u32,
                line.dx.as_ptr(),
            ) != 0
        }
    }

    unsafe fn draw_plain(dc: HDC, mask: HDC, layout: &Layout, left: i32, top: i32, clip: &RECT) {
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let saved_mask = SaveDC(mask);
        if saved_mask == 0 {
            RestoreDC(dc, saved);
            return;
        }
        for target in [dc, mask] {
            if !layout.font.is_null() {
                SelectObject(target, layout.font);
            }
            SetBkMode(target, TRANSPARENT as i32);
            SetTextAlign(target, TA_LEFT | TA_TOP | TA_NOUPDATECP);
            SetTextCharacterExtra(target, 0);
            SetTextJustification(target, 0, 0);
            IntersectClipRect(target, clip.left, clip.top, clip.right, clip.bottom);
            IntersectClipRect(
                target,
                left,
                top,
                left.saturating_add(layout.width),
                top.saturating_add(layout.height),
            );
        }
        SetTextColor(mask, 0x00ffffff);
        let x = left.saturating_add(layout.padding);
        let stride = layout.font_height.saturating_add(layout.style.line_gap_px);
        let base = top.saturating_add(layout.padding);
        let lines = visible_lines(
            base,
            layout.font_height,
            stride,
            layout.padding,
            clip.top,
            clip.bottom,
            layout.lines.len(),
        );
        for i in lines {
            let line = &layout.lines[i];
            let y = top
                .saturating_add(layout.padding)
                .saturating_add((i as i32).saturating_mul(stride));
            pass(dc, line, x, y);
            pass(mask, line, x, y);
        }
        RestoreDC(mask, saved_mask);
        RestoreDC(dc, saved);
    }

    fn rgb_from_colorref(value: COLORREF) -> u32 {
        ((value & 255) << 16) | (value & 0x00ff00) | ((value >> 16) & 255)
    }

    unsafe fn stroke_lines(
        dc: HDC,
        layout: &Layout,
        left: i32,
        base: i32,
        stride: i32,
        tile: &RECT,
        lines: std::ops::Range<usize>,
    ) -> bool {
        if BeginPath(dc) == 0 {
            return false;
        }
        for i in lines {
            let y = base.saturating_add((i as i32).saturating_mul(stride));
            if !pass(dc, &layout.lines[i], left - tile.left, y - tile.top) {
                AbortPath(dc);
                return false;
            }
        }
        if EndPath(dc) == 0 || GetPath(dc, null_mut(), null_mut(), 0) <= 0 {
            AbortPath(dc);
            return false;
        }
        StrokePath(dc) != 0
    }

    pub(crate) unsafe fn draw(
        surface: &mut Surface,
        layout: &Layout,
        left: i32,
        top: i32,
        clip: &RECT,
    ) {
        let dc = surface.dc();
        let mask = surface.mask_dc();
        let Some(outline) = layout.style.outline else {
            draw_plain(dc, mask, layout, left, top, clip);
            return;
        };
        let bounds = RECT {
            left: left.max(clip.left).max(0),
            top: top.max(clip.top).max(0),
            right: left
                .saturating_add(layout.width)
                .min(clip.right)
                .min(surface.width),
            bottom: top
                .saturating_add(layout.height)
                .min(clip.bottom)
                .min(surface.height),
        };
        if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
            return;
        }
        let text_rgb = rgb_from_colorref(GetTextColor(dc));
        let outline_rgb = rgb_from_colorref(outline.color);
        let Some(pen) = surface.text_outline_pen(outline.radius_px) else {
            draw_plain(dc, mask, layout, left, top, clip);
            return;
        };
        const SCALE: i32 = 4;
        const STRIP_HEIGHT: i32 = 64;
        let base = top.saturating_add(layout.padding);
        let stride = layout.font_height.saturating_add(layout.style.line_gap_px);
        let mut tile_top = bounds.top;
        while tile_top < bounds.bottom {
            let tile = RECT {
                left: bounds.left,
                top: tile_top,
                right: bounds.right,
                bottom: tile_top.saturating_add(STRIP_HEIGHT).min(bounds.bottom),
            };
            let width = (tile.right - tile.left) * SCALE;
            let height = (tile.bottom - tile.top) * SCALE;
            let Some(raster) = surface.text_raster_dc(width, height) else {
                // Keep text readable if a DIB/DC allocation fails. The remaining
                // region uses one normal glyph pass, never repeated offset text.
                let remaining = RECT {
                    top: tile_top,
                    ..bounds
                };
                draw_plain(dc, mask, layout, left, top, &remaining);
                return;
            };
            let saved = SaveDC(raster);
            if saved == 0 {
                let remaining = RECT {
                    top: tile_top,
                    ..bounds
                };
                draw_plain(dc, mask, layout, left, top, &remaining);
                return;
            }
            IntersectClipRect(raster, 0, 0, width, height);
            let transform = XFORM {
                eM11: SCALE as f32,
                eM12: 0.0,
                eM21: 0.0,
                eM22: SCALE as f32,
                eDx: 0.0,
                eDy: 0.0,
            };
            if SetGraphicsMode(raster, GM_ADVANCED) == 0
                || SetWorldTransform(raster, &transform) == 0
            {
                RestoreDC(raster, saved);
                let remaining = RECT {
                    top: tile_top,
                    ..bounds
                };
                draw_plain(dc, mask, layout, left, top, &remaining);
                return;
            }
            if !layout.font.is_null() {
                SelectObject(raster, layout.font);
            }
            SetBkMode(raster, TRANSPARENT as i32);
            SetTextAlign(raster, TA_LEFT | TA_TOP | TA_NOUPDATECP);
            SetTextCharacterExtra(raster, 0);
            SetTextJustification(raster, 0, 0);
            SetPolyFillMode(raster, WINDING);
            SelectObject(raster, pen);
            let lines = visible_lines(
                base,
                layout.font_height,
                stride,
                layout.padding,
                tile.top,
                tile.bottom,
                layout.lines.len(),
            );
            let x = left.saturating_add(layout.padding);
            // A geometric path stroke has round joins and exact requested width.
            // TRANSPARENT background makes the path describe glyphs, not boxes.
            // Green carries total stroke/fill coverage; white fill also adds red.
            let _ = stroke_lines(raster, layout, x, base, stride, &tile, lines.clone());
            AbortPath(raster);
            SetTextColor(raster, 0x00ffffff);
            // Fill through the original text API so bitmap/fallback/emoji glyphs
            // still appear even if they have no usable TrueType outline path.
            // Every glyph is drawn once at its original position; no offsets.
            for i in lines {
                let y = base.saturating_add((i as i32).saturating_mul(stride));
                pass(raster, &layout.lines[i], x - tile.left, y - tile.top);
            }
            RestoreDC(raster, saved);
            surface.blend_text_raster(&tile, text_rgb, outline_rgb);
            tile_top = tile.bottom;
        }
    }
}

#[cfg(windows)]
pub(super) use native::{draw, layout, single_line, Outline, TextStyle};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_samples_keep_fractional_edges_without_accumulating_offset_masks() {
        assert_eq!(coverage_4x(8 * 255, 12 * 255), (128, 191));
        assert_eq!(coverage_4x(255, 255), (16, 16));
        assert_eq!(coverage_4x(0, 0), (0, 0));
        assert_eq!(coverage_4x(16 * 255, 16 * 255), (255, 255));
        let (rgb, alpha) = blend_glyph_pixel(0, 0, 128, 191, 0xffffff, 0);
        assert_eq!(rgb, 0x808080);
        assert_eq!(alpha, 0xbfbfbf);
    }

    #[test]
    fn real_fill_keeps_its_color_and_black_outline_keeps_alpha() {
        assert_eq!(
            blend_glyph_pixel(0, 0, 255, 255, 0xabcdef, 0),
            (0xabcdef, 0xffffff)
        );
        assert_eq!(blend_glyph_pixel(0, 0, 0, 128, 0xffffff, 0), (0, 0x808080));
        assert_eq!(
            blend_glyph_pixel(0xffffff, 0xffffff, 0, 128, 0, 0),
            (0x7f7f7f, 0xffffff)
        );
        assert_eq!(
            blend_glyph_pixel(0x102030, 0x808080, 0, 0, 0xffffff, 0),
            (0x102030, 0x808080)
        );
    }

    #[test]
    fn colored_stroke_and_fill_are_disjoint_premultiplied_coverages() {
        let (rgb, mask) = blend_glyph_pixel(0, 0, 64, 192, 0xff0000, 0x0000ff);
        assert_eq!((rgb, mask), (0x400080, 0xc0c0c0));
        for foreground in [0, 1, 63, 128, 254, 255] {
            for total in [foreground, 255] {
                let (rgb, mask) =
                    blend_glyph_pixel(0x204080, 0x808080, foreground, total, 0xffffff, 0x80ff80);
                let a = mask & 255;
                assert!((rgb & 255) <= a && ((rgb >> 8) & 255) <= a && ((rgb >> 16) & 255) <= a);
            }
        }
    }

    #[test]
    fn strips_visit_only_intersecting_lines_and_include_outline_edges() {
        assert_eq!(visible_lines(10, 12, 16, 1, 0, 9, 100), 0..0);
        assert_eq!(visible_lines(10, 12, 16, 1, 9, 10, 100), 0..1);
        assert_eq!(visible_lines(10, 12, 16, 1, 23, 25, 100), 1..1);
        assert_eq!(visible_lines(10, 12, 16, 1, 25, 26, 100), 1..2);
        assert_eq!(
            visible_lines(-100_000, 12, 16, 1, 0, 64, 20_000),
            6250..6255
        );
        assert_eq!(visible_lines(0, 12, 16, 1, 5, 5, 1), 0..0);
    }
    #[test]
    fn clusters_keep_surrogates_marks_joiners_flags_and_jamo_together() {
        for (s, n) in [
            ("A😀B", 3),
            ("e\u{301}x", 2),
            ("👨‍👩‍👧‍👦!", 2),
            ("🇰🇷🇯🇵", 2),
            ("한글", 2),
            ("👍🏽!", 2),
            ("か\u{3099}き", 2),
        ] {
            let c = clusters(s);
            assert_eq!(c.len(), n, "{s}");
            let u: Vec<u16> = s.encode_utf16().collect();
            let mut start = 0;
            for item in c {
                assert!(String::from_utf16(&u[start..item.end]).is_ok());
                start = item.end;
            }
        }
    }
    #[test]
    fn wrapping_prefers_word_boundaries_and_consumes_break_spaces() {
        let c = clusters("hello world");
        let w: Vec<i32> = (1..=c.len() as i32).collect();
        assert_eq!(line_end(&c, &w, 8, false), (5, 6));
        assert_eq!(line_end(&c, &w, 20, false), (11, 11));
        let c = clusters("abcdefgh");
        assert_eq!(line_end(&c, &[1, 2, 3, 4, 5, 6, 7, 8], 3, false), (3, 3));
    }
    #[test]
    fn cjk_wrap_keeps_closing_punctuation_with_preceding_cluster() {
        let c = clusters("한글。다음");
        assert_eq!(line_end(&c, &[1, 2, 3, 4, 5], 2, false), (1, 1));
        assert_eq!(line_end(&c, &[1, 2, 3, 4, 5], 3, false), (3, 3));
    }
    #[test]
    fn spacing_is_between_clusters_and_width_matches_advances() {
        let c = clusters("A😀e\u{301}");
        let (dx, widths) = advances(&[6, 16, 16, 22, 22], &c, 2);
        assert_eq!(dx, vec![8, 10, 2, 6, 0]);
        assert_eq!(widths, vec![6, 18, 26]);
        assert_eq!(dx.iter().sum::<i32>(), *widths.last().unwrap());
        let (dx, w) = advances(&[1, 2, 3], &clusters("iii"), -5);
        assert_eq!(dx, vec![1, 1, 1]);
        assert_eq!(w, vec![1, 2, 3]);
    }
}
