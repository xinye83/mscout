//! Seed text atlases from the Windows fonts the client draws with.
//!
//! The name, job, street and map label are Arial, not the Noto Sans KR TTF that
//! `Font_000.wz` carries (that is the general/Korean text font). The client's
//! name/job text was verified pixel-exact against GDI Arial with
//! `NONANTIALIASED_QUALITY` (1-bit); the map label is anti-aliased with a 1 px
//! shadow.
//!
//! We render each printable character into a memory DIB at a candidate pixel
//! height, trim it to ink, and store it as a native glyph. The candidate whose
//! glyphs best explain the live crop is the client's rendered size. This makes
//! the letter atlas available without the player teaching every character.
//!
//! Arial is proprietary and is never bundled: `CreateFontW` asks Windows for the
//! installed face at runtime.

use crate::game_font::{trim_bitmap, AtlasGlyph};
use crate::ocr::{self, Atlas, Gray};
use crate::settings;

/// Character names and job names are alphanumeric (plus the space in a job such
/// as `Dawn Warrior`). Leaving punctuation out keeps `l` from matching `!` at this
/// tiny size.
const IDENTITY: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 ";

/// Candidate origin offsets. GDI's hinting can shift by a pixel with the text
/// origin, so a glyph gets a small set of variants rather than one.
const OFFSETS: [(i32, i32); 3] = [(0, 0), (1, 0), (2, 0)];

// Ideal's 1-bit Arial matches near zero; Default Ratio's anti-aliased text is a
// looser match, so allow a larger mean distance. The client-tier read asserts the
// exact name/job, so a wrong size is caught there rather than guessed silently.
const IDENTITY_MAX_SCORE: f64 = 50.0;

/// Seed a 1-bit identity atlas (name/job) from the live crop.
pub fn seed_identity(gray: &Gray) -> Option<(settings::Atlas, u32)> {
    // Ideal's name/job are 1-bit Arial; Default Ratio at 1920 is anti-aliased.
    // Try both renders and a few ink thresholds, and keep whichever explains the
    // crop best.
    seed(
        gray,
        IDENTITY_MAX_SCORE,
        IDENTITY,
        &[(false, 1), (true, 96), (true, 128), (true, 160)],
    )
}

/// Fix the one ambiguity Arial cannot resolve at this size: a lowercase `l` and
/// an uppercase `I` are the same 1 px bar. Inside a word that has lowercase
/// letters, a bar is far more likely to be an `l` (`EurobIood` → `Euroblood`);
/// an all-caps word such as `II` keeps its `I`.
pub fn fix_identity_case(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            if !word.chars().any(|ch| ch.is_lowercase()) {
                return word.to_string();
            }
            let mut out = String::with_capacity(word.len());
            for (index, ch) in word.chars().enumerate() {
                if ch == 'I' && index > 0 {
                    out.push('l');
                } else {
                    out.push(ch);
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn seed(
    gray: &Gray,
    max_score: f64,
    charset: &str,
    modes: &[(bool, u8)],
) -> Option<(settings::Atlas, u32)> {
    let hint = ocr::dominant_glyph_height(gray).unwrap_or(9).max(4) as i32;
    // The ink height is smaller than the font's em size (Arial at 11 px gives a
    // ~8 px cap). Search from the ink height up, which covers both Ideal's 7 px
    // and Default's larger text.
    let low = (hint - 1).max(6);
    // Default Ratio's HUD text is larger than the ink-height hint suggests; the
    // best match can be well above it.
    let high = (hint + 14).min(36);

    let mut best: Option<(f64, Vec<AtlasGlyph>, i32)> = None;
    for &(anti_aliased, threshold) in modes {
        for px in low..=high {
            let glyphs = match render_mode("Arial", px, anti_aliased, threshold, charset) {
                Ok(glyphs) => glyphs,
                Err(_) => continue,
            };
            let Some(score) = score(&glyphs, gray) else {
                continue;
            };
            if best
                .as_ref()
                .map_or(true, |(best_score, _, _)| score < *best_score)
            {
                best = Some((score, glyphs, px));
            }
        }
    }

    let (score, glyphs, px) = best?;
    if score > max_score {
        return None;
    }

    let mut atlas = Atlas::default();
    for glyph in glyphs {
        atlas.learn_glyph(glyph.ch, glyph.bitmap, glyph.w, glyph.h);
    }
    // The Default rendering is anti-aliased; learn the exact live shapes that the
    // render already explains, so the first read is not a near miss.
    atlas.learn_text_rendering(gray);
    let mut stored = atlas.to_settings();
    stored.format = 2;
    stored.source = "font".into();
    Some((stored, px as u32))
}

/// Mean best-match distance of every segmented shape in `gray` against `glyphs`.
/// The correct font size reproduces the crop, so its mean is near zero.
fn score(glyphs: &[AtlasGlyph], gray: &Gray) -> Option<f64> {
    let mut atlas = Atlas::default();
    for glyph in glyphs {
        atlas.learn_glyph(glyph.ch, glyph.bitmap.clone(), glyph.w, glyph.h);
    }
    let guesses = atlas.explain(gray);
    if guesses.is_empty() {
        return None;
    }
    let total: f64 = guesses.iter().map(|guess| guess.distance).sum();
    Some(total / guesses.len() as f64)
}

#[cfg(windows)]
fn render_mode(
    font: &str,
    px: i32,
    anti_aliased: bool,
    threshold: u8,
    charset: &str,
) -> Result<Vec<AtlasGlyph>, String> {
    use windows_sys::Win32::Graphics::Gdi::{ANTIALIASED_QUALITY, NONANTIALIASED_QUALITY};

    let quality = if anti_aliased {
        ANTIALIASED_QUALITY
    } else {
        NONANTIALIASED_QUALITY
    };
    render_variant(font, px, false, quality, threshold, charset)
}

#[cfg(not(windows))]
fn render_mode(
    _font: &str,
    _px: i32,
    _anti_aliased: bool,
    _threshold: u8,
    _charset: &str,
) -> Result<Vec<AtlasGlyph>, String> {
    Err("font seeding is Windows-only".into())
}

#[cfg(windows)]
fn render_variant(
    font: &str,
    px: i32,
    bold: bool,
    quality: u8,
    threshold: u8,
    charset: &str,
) -> Result<Vec<AtlasGlyph>, String> {
    use std::ffi::c_void;

    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, SelectObject,
        SetBkColor, SetBkMode, SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, FF_DONTCARE, FW_BOLD,
        FW_NORMAL, OUT_DEFAULT_PRECIS, TRANSPARENT,
    };

    let width = (px * 4).max(32);
    let height = (px * 4).max(32);
    let face: Vec<u16> = font.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return Err("CreateCompatibleDC failed".into());
        }

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biHeight = -height;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return Err("CreateDIBSection failed".into());
        }
        let old_bitmap = SelectObject(dc, bitmap);

        let font_handle = CreateFontW(
            -px,
            0,
            0,
            0,
            if bold { FW_BOLD as i32 } else { FW_NORMAL as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            quality as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        );
        if font_handle.is_null() {
            SelectObject(dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(dc);
            return Err("CreateFontW(Arial) failed".into());
        }
        let old_font = SelectObject(dc, font_handle);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
        SetBkColor(dc, 0x0000_0000);

        let margin = px / 2 + 2;
        let mut out = Vec::new();
        for ch in charset.chars() {
            if ch == ' ' {
                continue;
            }
            let mut code = [ch as u16];
            for (dx, dy) in OFFSETS {
                std::ptr::write_bytes(bits as *mut u8, 0, (width * height * 4) as usize);
                TextOutW(dc, margin + dx, margin + dy, code.as_mut_ptr(), 1);

                let pixels =
                    std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize);
                let mut canvas = vec![0u8; (width * height) as usize];
                for index in 0..canvas.len() {
                    let luminance = (pixels[index * 4] as u32
                        + pixels[index * 4 + 1] as u32
                        + pixels[index * 4 + 2] as u32)
                        / 3;
                    if luminance >= threshold as u32 {
                        canvas[index] = 255;
                    }
                }

                if let Some(trimmed) = trim_bitmap(&canvas, width as u32, height as u32) {
                    out.push(AtlasGlyph {
                        ch,
                        w: trimmed.w,
                        h: trimmed.h,
                        bitmap: trimmed.bitmap,
                    });
                }
            }
        }

        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);
        DeleteObject(font_handle);
        DeleteObject(bitmap);
        DeleteDC(dc);

        out.sort_by_key(|glyph| (glyph.ch, glyph.w, glyph.h));
        out.dedup_by(|a, b| {
            a.ch == b.ch && a.w == b.w && a.h == b.h && a.bitmap == b.bitmap
        });
        Ok(out)
    }
}

#[cfg(not(windows))]
fn render_ascii(
    _font: &str,
    _px: i32,
    _anti_aliased: bool,
    _charset: &str,
) -> Result<Vec<AtlasGlyph>, String> {
    Err("font seeding is Windows-only".into())
}

/// Render a whole string to a trimmed binary mask. Diagnostic tool for
/// identifying a font from a live line without relying on segmentation.
#[cfg(windows)]
pub(crate) fn render_text_bitmap(
    font: &str,
    px: i32,
    bold: bool,
    quality: u8,
    threshold: u8,
    text: &str,
) -> Option<Gray> {
    use std::ffi::c_void;

    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, SelectObject,
        SetBkColor, SetBkMode, SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, FF_DONTCARE, FW_BOLD,
        FW_NORMAL, OUT_DEFAULT_PRECIS, TRANSPARENT,
    };

    let width = (px * text.chars().count() as i32 * 2).max(64);
    let height = (px * 4).max(32);
    let face: Vec<u16> = font.encode_utf16().chain(std::iter::once(0)).collect();
    let line: Vec<u16> = text.encode_utf16().collect();

    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biHeight = -height;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old_bitmap = SelectObject(dc, bitmap);
        let font_handle = CreateFontW(
            -px,
            0,
            0,
            0,
            if bold { FW_BOLD as i32 } else { FW_NORMAL as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            quality as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        );
        if font_handle.is_null() {
            SelectObject(dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(dc);
            return None;
        }
        let old_font = SelectObject(dc, font_handle);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
        SetBkColor(dc, 0x0000_0000);
        std::ptr::write_bytes(bits as *mut u8, 0, (width * height * 4) as usize);
        TextOutW(dc, px, px, line.as_ptr(), line.len() as i32);

        let pixels = std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize);
        let mut canvas = vec![0u8; (width * height) as usize];
        for index in 0..canvas.len() {
            let luminance = (pixels[index * 4] as u32
                + pixels[index * 4 + 1] as u32
                + pixels[index * 4 + 2] as u32)
                / 3;
            if luminance >= threshold as u32 {
                canvas[index] = 255;
            }
        }
        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);
        DeleteObject(font_handle);
        DeleteObject(bitmap);
        DeleteDC(dc);

        let trimmed = trim_bitmap(&canvas, width as u32, height as u32)?;
        Some(Gray {
            w: trimmed.w as usize,
            h: trimmed.h as usize,
            px: trimmed.bitmap,
        })
    }
}

#[cfg(not(windows))]
pub(crate) fn render_text_bitmap(
    _font: &str,
    _px: i32,
    _bold: bool,
    _quality: u8,
    _threshold: u8,
    _text: &str,
) -> Option<Gray> {
    None
}

/// Characters that can appear in a street or map label.
#[allow(dead_code)] // used by the segmentation-free OCR prototype/tests
const LABEL_CHARS: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

#[cfg(windows)]
#[allow(dead_code)] // used by the segmentation-free OCR prototype/tests
#[derive(Clone)]
pub struct LabelGlyph {
    ch: char,
    advance: i32,
    /// Cell in origin coordinates: width = advance, height = the font's ink
    /// height, with the glyph's ink at its left bearing and aligned to the
    /// tallest ink in the character set.
    cell: Gray,
}

#[cfg(windows)]
#[allow(dead_code)] // used by the segmentation-free OCR prototype/tests
pub struct LabelFont {
    glyphs: Vec<LabelGlyph>,
    cell_h: i32,
    max_left: i32,
    max_advance: i32,
}

#[cfg(windows)]
#[allow(dead_code)] // used by the segmentation-free OCR prototype/tests
fn label_font() -> Option<&'static LabelFont> {
    use std::sync::OnceLock;

    static FONT: OnceLock<Option<LabelFont>> = OnceLock::new();
    FONT.get_or_init(|| build_label_font("Arial", 12, true, 4, 180))
        .as_ref()
}

#[cfg(windows)]
#[allow(dead_code)] // used by the segmentation-free OCR prototype/tests
fn build_label_font(
    face: &str,
    px: i32,
    bold: bool,
    quality: u8,
    threshold: u8,
) -> Option<LabelFont> {
    use std::ffi::c_void;

    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GetCharWidth32W,
        SelectObject, SetBkColor, SetBkMode, SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, FF_DONTCARE,
        FW_BOLD, FW_NORMAL, OUT_DEFAULT_PRECIS, TRANSPARENT,
    };

    let canvas = 64i32;
    let margin = 16i32;
    let face_wide: Vec<u16> = face.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = canvas;
        bmi.bmiHeader.biHeight = -canvas;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old_bitmap = SelectObject(dc, bitmap);
        let font = CreateFontW(
            -px,
            0,
            0,
            0,
            if bold { FW_BOLD as i32 } else { FW_NORMAL as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            quality as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face_wide.as_ptr(),
        );
        if font.is_null() {
            SelectObject(dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(dc);
            return None;
        }
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
        SetBkColor(dc, 0x0000_0000);

        struct Raw {
            ch: char,
            advance: i32,
            left: i32,
            top: i32,
            w: i32,
            h: i32,
            bitmap: Vec<u8>,
        }

        let mut raw = Vec::new();
        for ch in LABEL_CHARS.chars() {
            let mut advance = 0i32;
            GetCharWidth32W(dc, ch as u32, ch as u32, &mut advance);
            advance = advance.max(1);

            std::ptr::write_bytes(bits as *mut u8, 0, (canvas * canvas * 4) as usize);
            if ch != ' ' {
                let mut code = [ch as u16];
                TextOutW(dc, margin, margin, code.as_mut_ptr(), 1);
            }

            let pixels =
                std::slice::from_raw_parts(bits as *const u8, (canvas * canvas * 4) as usize);
            let mut mask = vec![0u8; (canvas * canvas) as usize];
            for index in 0..mask.len() {
                let luminance = (pixels[index * 4] as u32
                    + pixels[index * 4 + 1] as u32
                    + pixels[index * 4 + 2] as u32)
                    / 3;
                if luminance >= threshold as u32 {
                    mask[index] = 255;
                }
            }
            let Some(trimmed) = trim_bitmap(&mask, canvas as u32, canvas as u32) else {
                raw.push(Raw {
                    ch,
                    advance,
                    left: 0,
                    top: 0,
                    w: 0,
                    h: 0,
                    bitmap: Vec::new(),
                });
                continue;
            };
            let left = trimmed.min_x as i32 - margin;
            let top = trimmed.min_y as i32 - margin;
            raw.push(Raw {
                ch,
                advance,
                left,
                top,
                w: trimmed.w as i32,
                h: trimmed.h as i32,
                bitmap: trimmed.bitmap,
            });
        }

        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);
        DeleteObject(font);
        DeleteObject(bitmap);
        DeleteDC(dc);

        let min_top = raw
            .iter()
            .filter(|glyph| glyph.w > 0)
            .map(|glyph| glyph.top)
            .min()?;
        let max_bottom = raw
            .iter()
            .filter(|glyph| glyph.w > 0)
            .map(|glyph| glyph.top + glyph.h)
            .max()?;
        let cell_h = (max_bottom - min_top).max(1);
        let max_left = raw.iter().map(|glyph| glyph.left).max().unwrap_or(0);
        let max_advance = raw.iter().map(|glyph| glyph.advance).max().unwrap_or(1);

        let mut glyphs = Vec::new();
        for glyph in raw {
            let advance = glyph.advance.max(glyph.left + glyph.w).max(1);
            let mut cell = vec![0u8; (advance * cell_h) as usize];
            for y in 0..glyph.h {
                for x in 0..glyph.w {
                    let cx = glyph.left + x;
                    let cy = glyph.top - min_top + y;
                    if cx >= 0
                        && cy >= 0
                        && cx < advance
                        && cy < cell_h
                        && glyph.bitmap[(y * glyph.w + x) as usize] > 0
                    {
                        cell[(cy * advance + cx) as usize] = 255;
                    }
                }
            }
            glyphs.push(LabelGlyph {
                ch: glyph.ch,
                advance,
                cell: Gray {
                    w: advance as usize,
                    h: cell_h as usize,
                    px: cell,
                },
            });
        }

        Some(LabelFont {
            glyphs,
            cell_h,
            max_left,
            max_advance,
        })
    }
}

fn binarize_gray(gray: &Gray, threshold: u8) -> Gray {
    Gray {
        w: gray.w,
        h: gray.h,
        px: gray
            .px
            .iter()
            .map(|pixel| if *pixel > threshold { 255 } else { 0 })
            .collect(),
    }
}

#[cfg(windows)]
#[allow(dead_code)] // used by the segmentation-free OCR prototype/tests
fn cell_cost(live: &Gray, x: i32, y: i32, glyph: &LabelGlyph, width: i32) -> f64 {
    let mut cost = 0u32;
    for yy in 0..glyph.cell.h as i32 {
        let ly = y + yy;
        for xx in 0..width {
            let lx = x + xx;
            let a = if lx >= 0 && ly >= 0 && lx < live.w as i32 && ly < live.h as i32 {
                live.px[ly as usize * live.w + lx as usize]
            } else {
                0
            };
            let b = if xx < glyph.cell.w as i32 {
                glyph.cell.px[yy as usize * glyph.cell.w + xx as usize]
            } else {
                0
            };
            if (a > 0) != (b > 0) {
                cost += 1;
            }
        }
    }
    cost as f64
}

/// Read one map/street label line with Arial Bold 12 and a dynamic-programming
/// search. Unlike column segmentation, the DP places whole glyph cells, so a
/// letter whose anti-aliased strokes leave empty columns is still compared as a
/// whole and cannot fragment into stems.
#[cfg(windows)]
#[allow(dead_code)] // used by the segmentation-free OCR prototype/tests
pub fn read_label_line(gray: &Gray) -> Option<String> {
    let font = label_font()?;
    let mut best: Option<(f64, String)> = None;

    for threshold in [160u8, 170, 180, 190, 200] {
        let live = binarize_gray(gray, threshold);
        let left_pad = font.max_left.max(0) as usize;
        let right_pad = font.max_advance.max(1) as usize;
        let w = live.w + left_pad + right_pad;
        let h = live.h + 4;
        let mut px = vec![0u8; w * h];
        for y in 0..live.h {
            for x in 0..live.w {
                if live.px[y * live.w + x] > 0 {
                    px[(y + 2) * w + x + left_pad] = 255;
                }
            }
        }
        let padded = Gray { w, h, px };

        for cy in 2..=2 {
            for scale in [1.0f64] {
            let inf = f64::INFINITY;
            let mut dp = vec![inf; w + 1];
            let mut prev: Vec<Option<(usize, Option<char>)>> = vec![None; w + 1];
            dp[..=left_pad].fill(0.0);

            for x in 0..w {
                if !dp[x].is_finite() {
                    continue;
                }
                let ink = (0..font.cell_h)
                    .filter(|yy| {
                        let ly = cy + yy;
                        ly >= 0 && ly < h as i32 && padded.px[ly as usize * w + x] > 0
                    })
                    .count();
                let skip = 1.0 + ink as f64 * 3.0;
                if dp[x] + skip < dp[x + 1] {
                    dp[x + 1] = dp[x] + skip;
                    prev[x + 1] = Some((x, None));
                }

                for glyph in &font.glyphs {
                    let cw = ((glyph.advance as f64) * scale).round() as i32;
                    if cw <= 0 {
                        continue;
                    }
                    let next = x + cw as usize;
                    if next > w {
                        continue;
                    }
                    for shift in -2..=2 {
                        let sx = x as i32 + shift;
                        if sx < 0 {
                            continue;
                        }
                        let cost = cell_cost(&padded, sx, cy, glyph, cw)
                            + 0.2
                            + (cw - glyph.advance).unsigned_abs() as f64 * 0.01
                            + shift.abs() as f64 * 0.05;
                        if dp[x] + cost < dp[next] {
                            dp[next] = dp[x] + cost;
                            prev[next] = Some((x, Some(glyph.ch)));
                        }
                    }
                }
            }

            let start = w.saturating_sub(right_pad + 2);
            let mut end = w;
            let mut score = f64::INFINITY;
            for (x, value) in dp.iter().enumerate().take(w + 1).skip(start) {
                if *value < score {
                    score = *value;
                    end = x;
                }
            }
            if !score.is_finite() {
                continue;
            }

            let mut chars = Vec::new();
            let mut x = end;
            while x > left_pad {
                let Some((previous, ch)) = prev[x] else {
                    break;
                };
                if let Some(ch) = ch {
                    chars.push(ch);
                }
                x = previous;
                if x == 0 {
                    break;
                }
            }
            chars.reverse();
            let text: String = chars.into_iter().collect();
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            if best
                .as_ref()
                .map_or(true, |(best_score, _)| score < *best_score)
            {
                best = Some((score, text));
            }
            }
        }
    }

    best.map(|(_, text)| text)
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn read_label_line(_gray: &Gray) -> Option<String> {
    None
}

fn column_bits(gray: &Gray, shift: i32) -> Vec<u32> {
    let height = gray.h.min(32) as u32;
    let mask = if height == 32 {
        u32::MAX
    } else {
        (1u32 << height) - 1
    };
    (0..gray.w)
        .map(|x| {
            let mut bits = 0u32;
            for y in 0..height as usize {
                if gray.px[y * gray.w + x] > 0 {
                    bits |= 1 << y;
                }
            }
            if shift >= 0 {
                (bits << shift) & mask
            } else {
                bits >> (-shift)
            }
        })
        .collect()
}

/// Dynamic-time-warp distance between the column ink patterns, allowing a small
/// local stretch. This compares the shape sequence without depending on glyph
/// segmentation or exact advances.
pub(crate) fn dtw_distance(live: &Gray, rendered: &Gray) -> f64 {
    let live_cols = column_bits(live, 0);
    let mut best = f64::MAX;
    for shift in -3..=3 {
        let rendered_cols = column_bits(rendered, shift);
        let a = &live_cols;
        let b = &rendered_cols;
        if a.is_empty() || b.is_empty() {
            continue;
        }
        let band = 8usize;
        let mut previous = vec![f64::INFINITY; b.len() + 1];
        previous[0] = 0.0;
        for i in 1..=a.len() {
            let mut current = vec![f64::INFINITY; b.len() + 1];
            let center = i * b.len() / a.len();
            let low = center.saturating_sub(band);
            let high = (center + band).min(b.len());
            for j in low.max(1)..=high {
                let cost = (a[i - 1] ^ b[j - 1]).count_ones() as f64;
                let step = previous[j - 1].min(previous[j]).min(current[j - 1]);
                current[j] = step + cost;
            }
            previous = current;
        }
        let score = previous[b.len()] / (a.len() + b.len()) as f64;
        if score < best {
            best = score;
        }
    }
    best
}

/// One rendered `(street, name)` folded-bar line.
pub type FoldedLine = ((String, String), Gray);

/// Rendered label candidates, cached for whole-line matching.
pub struct LabelIndex {
    pub map_names: Vec<(String, Gray)>,
    pub streets: Vec<(String, Gray)>,
    /// The folded bar's single line: `"{street} :{name}"`, rendered in the folded
    /// bar's font (Arial Regular 12, anti-aliased). Matching it returns both
    /// fields at once, so no colon detection or segmentation is needed.
    pub folded_lines: Vec<FoldedLine>,
}

static LABEL_INDEX: std::sync::Mutex<Option<(String, std::sync::Arc<LabelIndex>)>> =
    std::sync::Mutex::new(None);

/// Build or reuse the client's map/street name index for this install.
///
/// Names are rendered once with the identified label font (Arial Bold 12, GDI
/// anti-aliased) and cached. Matching a live line is then a width filter plus a
/// shape-sequence DTW, so no column segmentation is involved.
pub fn label_index(mscw_dir: &std::path::Path) -> Option<std::sync::Arc<LabelIndex>> {
    let key = mscw_dir.to_string_lossy().to_string();
    let mut guard = LABEL_INDEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached, index)) = guard.as_ref() {
        if cached == &key {
            return Some(index.clone());
        }
    }

    let entries = crate::map_names::load(mscw_dir).ok()?;
    let mut seen_names = std::collections::HashSet::new();
    let mut seen_streets = std::collections::HashSet::new();
    let mut seen_pairs = std::collections::HashSet::new();
    let mut map_names = Vec::new();
    let mut streets = Vec::new();
    let mut folded_lines = Vec::new();

    for entry in entries {
        if seen_names.insert(entry.name.clone()) {
            if let Some(gray) = render_text_bitmap("Arial", 12, true, 4, 180, &entry.name) {
                map_names.push((entry.name.clone(), gray));
            }
        }
        if !entry.street.is_empty() && seen_streets.insert(entry.street.clone()) {
            if let Some(gray) = render_text_bitmap("Arial", 12, true, 4, 180, &entry.street) {
                streets.push((entry.street.clone(), gray));
            }
        }
        if seen_pairs.insert((entry.street.clone(), entry.name.clone())) {
            let line = format!("{} :{}", entry.street, entry.name);
            if let Some(gray) = render_text_bitmap("Arial", 12, false, 4, 180, &line) {
                folded_lines.push(((entry.street, entry.name), gray));
            }
        }
    }

    let index = std::sync::Arc::new(LabelIndex {
        map_names,
        streets,
        folded_lines,
    });
    *guard = Some((key, index.clone()));
    Some(index)
}

/// Match a folded-bar line against `(street, name)` candidates and return both
/// fields from the winning entry.
pub fn match_folded_label(
    live: &Gray,
    candidates: &[FoldedLine],
    max_width_delta: i32,
    max_score: f64,
) -> Option<((String, String), f64)> {
    let binaries: Vec<Gray> = [160u8, 180, 200]
        .into_iter()
        .map(|threshold| binarize_gray(live, threshold))
        .collect();

    let mut scored: Vec<(f64, i32, &FoldedLine)> = Vec::new();
    for candidate in candidates {
        let rendered = &candidate.1;
        let mut best: Option<(f64, i32)> = None;
        for binary in &binaries {
            let delta = (binary.w as i32 - rendered.w as i32).abs();
            if delta > max_width_delta {
                continue;
            }
            let score = dtw_distance(binary, rendered);
            if best.map_or(true, |(current, _)| score < current) {
                best = Some((score, delta));
            }
        }
        if let Some((score, delta)) = best {
            scored.push((score, delta, candidate));
        }
    }
    scored.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    let (best_score, _best_delta, best_candidate) = *scored.first()?;
    if best_score > max_score {
        return None;
    }
    Some((best_candidate.0.clone(), best_score))
}

/// Match a live label line against rendered candidates. The width filter is what
/// keeps this cheap: a 142 px live line can only be a name that renders near
/// 142 px.
pub fn match_label(
    live: &Gray,
    candidates: &[(String, Gray)],
    max_width_delta: i32,
    max_score: f64,
) -> Option<(String, f64)> {
    let binaries: Vec<Gray> = [160u8, 180, 200]
        .into_iter()
        .map(|threshold| binarize_gray(live, threshold))
        .collect();

    // Score every width-compatible candidate once, taking its best threshold.
    let mut scored: Vec<(f64, i32, &str)> = Vec::new();
    for (text, rendered) in candidates {
        let mut best: Option<(f64, i32)> = None;
        for binary in &binaries {
            let delta = (binary.w as i32 - rendered.w as i32).abs();
            if delta > max_width_delta {
                continue;
            }
            let score = dtw_distance(binary, rendered);
            if best.map_or(true, |(current, _)| score < current) {
                best = Some((score, delta));
            }
        }
        if let Some((score, delta)) = best {
            scored.push((score, delta, text.as_str()));
        }
    }
    if scored.is_empty() {
        return None;
    }
    scored.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });

    let (best_score, _best_delta, best_text) = scored[0];
    if best_score > max_score {
        return None;
    }

    // A close runner-up means the line is ambiguous; refuse rather than display a
    // plausible-looking wrong map. Names that differ only by a trailing roman
    // numeral ("... Beach I/II/III") render 3 px apart, so the DTW scores overlap.
    // In that case the rendered width is the discriminator: the true name is the
    // one whose width matches the live ink most closely, and only if that winner
    // is unique.
    if let Some(second) = scored.get(1) {
        if best_score + 0.05 > second.0 {
            let rivals: Vec<&(f64, i32, &str)> = scored
                .iter()
                .take_while(|(score, _, _)| *score <= best_score + 0.05)
                .collect();
            if rivals.len() > 1 {
                let mut by_width = rivals.clone();
                by_width.sort_by_key(|(_, delta, _)| *delta);
                let (_, first_delta, first_text) = *by_width[0];
                let unique = by_width
                    .iter()
                    .skip(1)
                    .all(|(_, delta, _)| *delta > first_delta);
                if unique {
                    return Some((first_text.to_string(), best_score));
                }
                return None;
            }
        }
    }

    Some((best_text.to_string(), best_score))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_case_bar_is_a_lowercase_l_inside_a_word() {
        assert_eq!(fix_identity_case("EurobIood"), "Euroblood");
        assert_eq!(fix_identity_case("Page"), "Page");
        assert_eq!(fix_identity_case("II"), "II");
        assert_eq!(fix_identity_case("Ivan"), "Ivan");
        assert_eq!(fix_identity_case("BluebeIl"), "Bluebell");
        assert_eq!(fix_identity_case("Dawn Warrior"), "Dawn Warrior");
    }

}
