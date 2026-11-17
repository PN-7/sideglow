// SPDX-License-Identifier: MPL-2.0
//! Cached, per-pixel alpha surface. There is no color key and no ClearType fringe.
use super::chat_text::{blend_glyph_pixel, coverage_4x};
use super::surface_math::apply_coverage;
use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};

struct Dib {
    dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    default_font: HGDIOBJ,
    bits: *mut u32,
    len: usize,
    width: i32,
    height: i32,
}

impl Dib {
    unsafe fn new(width: i32, height: i32) -> Result<Self, String> {
        let len = (width as usize)
            .checked_mul(height as usize)
            .filter(|&len| len <= 32 * 1024 * 1024)
            .ok_or(crate::i18n::text(crate::i18n::Key::M6C4042B5628B))?;
        let dc = CreateCompatibleDC(null_mut());
        if dc.is_null() {
            return Err(crate::i18n::text(crate::i18n::Key::M1AEB54EFF3BA));
        }
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap as _);
            }
            DeleteDC(dc);
            return Err(crate::i18n::text(crate::i18n::Key::M97BCD4B6C88A));
        }
        let old_bitmap = SelectObject(dc, bitmap as _);
        let default_font = GetCurrentObject(dc, OBJ_FONT as u32);
        SetBkMode(dc, TRANSPARENT as i32);
        Ok(Self {
            dc,
            bitmap,
            old_bitmap,
            default_font,
            bits: bits.cast(),
            len,
            width,
            height,
        })
    }
    unsafe fn pixels(&mut self) -> &mut [u32] {
        std::slice::from_raw_parts_mut(self.bits, self.len)
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.default_font);
            SelectObject(self.dc, self.old_bitmap);
            DeleteObject(self.bitmap as _);
            DeleteDC(self.dc);
        }
    }
}

pub(super) struct Surface {
    color: Dib,
    mask: Dib,
    text_raster: Option<Dib>,
    text_pen: HPEN,
    text_pen_radius: i32,
    pub width: i32,
    pub height: i32,
}

impl Surface {
    pub unsafe fn new(width: i32, height: i32) -> Result<Self, String> {
        if width <= 0 || height <= 0 {
            return Err(crate::i18n::text(crate::i18n::Key::M64931A4B750D));
        }
        Ok(Self {
            color: Dib::new(width, height)?,
            mask: Dib::new(width, height)?,
            text_raster: None,
            text_pen: null_mut(),
            text_pen_radius: 0,
            width,
            height,
        })
    }
    pub fn dc(&self) -> HDC {
        self.color.dc
    }
    pub fn mask_dc(&self) -> HDC {
        self.mask.dc
    }

    /// Scratch is bounded by one visible strip, never by a long paragraph's
    /// height. Grow only when needed and keep it for subsequent paint events.
    pub(super) unsafe fn text_raster_dc(&mut self, width: i32, height: i32) -> Option<HDC> {
        if width <= 0 || height <= 0 || width > self.width.saturating_mul(4) || height > 256 {
            return None;
        }
        let replace = self
            .text_raster
            .as_ref()
            .is_none_or(|r| width > r.width || height > r.height);
        if replace {
            let (width, height) = self.text_raster.as_ref().map_or((width, height), |r| {
                (width.max(r.width), height.max(r.height))
            });
            self.text_raster = Some(Dib::new(width, height).ok()?);
        }
        let raster = self.text_raster.as_mut()?;
        GdiFlush();
        let pitch = raster.width as usize;
        let pixels = raster.pixels();
        for y in 0..height as usize {
            pixels[y * pitch..y * pitch + width as usize].fill(0);
        }
        Some(raster.dc)
    }

    pub(super) unsafe fn text_outline_pen(&mut self, radius: i32) -> Option<HPEN> {
        let radius = radius.clamp(1, 8);
        if self.text_pen.is_null() || self.text_pen_radius != radius {
            let brush = LOGBRUSH {
                lbStyle: BS_SOLID,
                lbColor: 0x0000ff00,
                lbHatch: 0,
            };
            let pen = ExtCreatePen(
                (PS_GEOMETRIC | PS_SOLID | PS_JOIN_ROUND | PS_ENDCAP_ROUND) as u32,
                (radius * 2) as u32,
                &brush,
                0,
                null(),
            );
            if pen.is_null() {
                return None;
            }
            if !self.text_pen.is_null() {
                DeleteObject(self.text_pen);
            }
            self.text_pen = pen;
            self.text_pen_radius = radius;
        }
        Some(self.text_pen)
    }

    pub(super) unsafe fn blend_text_raster(
        &mut self,
        rect: &RECT,
        text_rgb: u32,
        outline_rgb: u32,
    ) {
        const SCALE: usize = 4;
        let Some(raster) = self.text_raster.as_ref() else {
            return;
        };
        if rect.left < 0
            || rect.top < 0
            || rect.right > self.width
            || rect.bottom > self.height
            || rect.right <= rect.left
            || rect.bottom <= rect.top
        {
            return;
        }
        let width = (rect.right - rect.left) as usize;
        let height = (rect.bottom - rect.top) as usize;
        if width * SCALE > raster.width as usize || height * SCALE > raster.height as usize {
            return;
        }
        // Flush the path/fill pass and earlier UI GDI writes before CPU access.
        GdiFlush();
        let samples = std::slice::from_raw_parts(raster.bits, raster.len);
        let pitch = raster.width as usize;
        let destination_pitch = self.width as usize;
        let colors = self.color.pixels();
        let masks = self.mask.pixels();
        for y in 0..height {
            for x in 0..width {
                let mut foreground = 0;
                let mut total = 0;
                for sy in 0..SCALE {
                    let start = (y * SCALE + sy) * pitch + x * SCALE;
                    for pixel in &samples[start..start + SCALE] {
                        // Green marks stroke+fill; red marks only glyph fill.
                        total += (pixel >> 8) & 255;
                        foreground += (pixel >> 16) & 255;
                    }
                }
                let (foreground, total) = coverage_4x(foreground, total);
                if total == 0 {
                    continue;
                }
                let index = (y + rect.top as usize) * destination_pitch + x + rect.left as usize;
                let (rgb, coverage) = blend_glyph_pixel(
                    colors[index],
                    masks[index],
                    foreground,
                    total,
                    text_rgb,
                    outline_rgb,
                );
                colors[index] = rgb;
                masks[index] = coverage;
            }
        }
    }
    pub unsafe fn clear(&mut self, opaque_rgb: Option<u32>) {
        // Previous presentation flushes GDI; explicitly flush again before CPU writes.
        GdiFlush();
        self.color.pixels().fill(opaque_rgb.unwrap_or(0) & 0xFFFFFF);
        self.mask
            .pixels()
            .fill(if opaque_rgb.is_some() { 0xFFFFFF } else { 0 });
    }
    pub unsafe fn present(&mut self, hwnd: HWND, opacity: u8) -> Result<(), String> {
        // GDI's alpha bytes are undefined. A matching white grayscale mask supplies
        // coverage, including for black user-selected text which has zero RGB.
        GdiFlush();
        apply_coverage(self.color.pixels(), self.mask.pixels());
        // Do not leave app-owned HFONTs selected in cached DCs across settings changes.
        SelectObject(self.color.dc, self.color.default_font);
        SelectObject(self.mask.dc, self.mask.default_font);
        let size = SIZE {
            cx: self.width,
            cy: self.height,
        };
        let origin = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: opacity,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if UpdateLayeredWindow(
            hwnd,
            null_mut(),
            null(),
            &size,
            self.color.dc,
            &origin,
            0,
            &blend,
            ULW_ALPHA,
        ) == 0
        {
            return Err({
                let __ui_arg0 = &(std::io::Error::last_os_error());
                crate::i18n::format(
                    crate::i18n::Key::MA4228DFD283D,
                    &[("arg0", format!("{}", __ui_arg0))],
                )
            });
        }
        Ok(())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            // Drawing restores the scratch DC before returning, so the cached
            // pen and application fonts are never left selected here.
            if !self.text_pen.is_null() {
                DeleteObject(self.text_pen);
            }
        }
    }
}
