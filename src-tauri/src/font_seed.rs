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

/// Characters that can appear in a street or map label. Diagnostic only until
/// the label font is identified; identity seeding uses `IDENTITY`.
#[cfg(test)]
const PRINTABLE: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,'-:/!?()&[]%+*=<>\"#@_";

/// Character names and job names are alphanumeric (plus the space in a job such
/// as `Dawn Warrior`). Leaving punctuation out keeps `l` from matching `!` at this
/// tiny size.
const IDENTITY: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 ";

/// Candidate origin offsets. GDI's hinting can shift by a pixel with the text
/// origin, so a glyph gets a small set of variants rather than one.
const OFFSETS: [(i32, i32); 3] = [(0, 0), (1, 0), (2, 0)];

const IDENTITY_MAX_SCORE: f64 = 18.0;
#[cfg(test)]
const LABEL_MAX_SCORE: f64 = 30.0;

/// Seed a 1-bit identity atlas (name/job) from the live crop.
pub fn seed_identity(gray: &Gray) -> Option<(settings::Atlas, u32)> {
    seed(gray, false, IDENTITY_MAX_SCORE, IDENTITY)
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

/// Seed an anti-aliased label atlas (street/map name) from the live crop.
/// Diagnostic only: the minimap label does not match Arial well enough to wire
/// into the reader yet.
#[cfg(test)]
pub fn seed_label(gray: &Gray) -> Option<(settings::Atlas, u32)> {
    seed(gray, true, LABEL_MAX_SCORE, PRINTABLE)
}

fn seed(
    gray: &Gray,
    anti_aliased: bool,
    max_score: f64,
    charset: &str,
) -> Option<(settings::Atlas, u32)> {
    let hint = ocr::dominant_glyph_height(gray).unwrap_or(9).max(4) as i32;
    // The ink height is smaller than the font's em size (Arial at 11 px gives a
    // ~8 px cap). Search from the ink height up, which covers both Ideal's 7 px
    // and Default's larger text.
    let low = (hint - 1).max(6);
    let high = (hint + 8).min(28);

    let mut best: Option<(f64, Vec<AtlasGlyph>, i32)> = None;
    for px in low..=high {
        let glyphs = match render_ascii("Arial", px, anti_aliased, charset) {
            Ok(glyphs) => glyphs,
            Err(_) => return None,
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

    let (score, glyphs, px) = best?;
    if score > max_score {
        return None;
    }

    let mut atlas = Atlas::default();
    for glyph in glyphs {
        atlas.learn_glyph(glyph.ch, glyph.bitmap, glyph.w, glyph.h);
    }
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
fn render_ascii(
    font: &str,
    px: i32,
    anti_aliased: bool,
    charset: &str,
) -> Result<Vec<AtlasGlyph>, String> {
    use windows_sys::Win32::Graphics::Gdi::{ANTIALIASED_QUALITY, NONANTIALIASED_QUALITY};

    let quality = if anti_aliased {
        ANTIALIASED_QUALITY
    } else {
        NONANTIALIASED_QUALITY
    };
    render_variant(
        font,
        px,
        false,
        quality,
        if anti_aliased { 128 } else { 1 },
        charset,
    )
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

/// A rendered Arial Bold 12 name, cached for whole-line matching.
pub struct LabelIndex {
    pub map_names: Vec<(String, Gray)>,
    pub streets: Vec<(String, Gray)>,
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
    let mut map_names = Vec::new();
    let mut streets = Vec::new();

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
    }

    let index = std::sync::Arc::new(LabelIndex { map_names, streets });
    *guard = Some((key, index.clone()));
    Some(index)
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
    let mut best: Option<(String, f64)> = None;
    let mut second = f64::MAX;
    for threshold in [160u8, 180, 200] {
        let binary = binarize_gray(live, threshold);
        for (text, rendered) in candidates {
            if (binary.w as i32 - rendered.w as i32).abs() > max_width_delta {
                continue;
            }
            let score = dtw_distance(&binary, rendered);
            match &best {
                None => best = Some((text.clone(), score)),
                Some((best_text, best_score)) => {
                    if score < *best_score {
                        if text != best_text {
                            second = second.min(*best_score);
                        }
                        best = Some((text.clone(), score));
                    } else if text != best_text && score < second {
                        second = score;
                    }
                }
            }
        }
        if best.as_ref().is_some_and(|(_, score)| *score < 0.35) {
            break;
        }
    }

    let (text, score) = best?;
    // A close runner-up means the line is ambiguous (two similar names); refuse
    // rather than display a plausible-looking wrong map.
    if score <= max_score && score + 0.05 <= second {
        Some((text, score))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Frame;
    use crate::settings::{Region, UiMode};

    fn load_bmp(path: &std::path::Path) -> Frame {
        let bytes = std::fs::read(path).expect("bmp");
        let offset = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
        let width = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
        let height = i32::from_le_bytes(bytes[22..26].try_into().unwrap()) as u32;
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 32);

        let row = width as usize * 4;
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height as usize {
            let source_y = height as usize - 1 - y;
            let start = offset + source_y * row;
            for x in 0..width as usize {
                let pixel = start + x * 4;
                let out = (y * width as usize + x) * 4;
                rgba[out] = bytes[pixel + 2];
                rgba[out + 1] = bytes[pixel + 1];
                rgba[out + 2] = bytes[pixel];
                rgba[out + 3] = 255;
            }
        }
        Frame {
            width,
            height,
            rgba,
        }
    }

    fn read(gray: &Gray, stored: &settings::Atlas) -> Option<String> {
        let atlas = Atlas::from_settings(stored);
        let gap = (gray.h as i32 / 3).max(2);
        atlas
            .recognize_text(gray, gap)
            .ok()
            .map(|(text, _)| text)
    }

    fn frame_gray(frame: &Frame) -> Gray {
        let mut px = Vec::with_capacity(frame.width as usize * frame.height as usize);
        for index in (0..frame.rgba.len()).step_by(4) {
            let r = frame.rgba[index] as u32;
            let g = frame.rgba[index + 1] as u32;
            let b = frame.rgba[index + 2] as u32;
            px.push(((77 * r + 150 * g + 29 * b) >> 8) as u8);
        }
        Gray {
            w: frame.width as usize,
            h: frame.height as usize,
            px,
        }
    }

    fn binarize(gray: &Gray, threshold: u8) -> Gray {
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

    /// Mean absolute difference of two trimmed binary masks, over the union with
    /// a small shift search. Lower is better; a matching font is near 0.
    fn mask_distance(live: &Gray, rendered: &Gray, search: i32) -> f64 {
        let mut best = f64::MAX;
        for dy in -search..=search {
            for dx in -search..=search {
                let mut total = 0u64;
                let mut count = 0usize;
                let w = rendered.w as i32 + search * 2;
                let h = rendered.h as i32 + search * 2;
                for y in 0..h {
                    for x in 0..w {
                        let b = if x >= 0 && y >= 0 && x < rendered.w as i32 && y < rendered.h as i32 {
                            rendered.px[(y * rendered.w as i32 + x) as usize]
                        } else {
                            0
                        };
                        let ax = x - dx;
                        let ay = y - dy;
                        let a = if ax >= 0 && ay >= 0 && ax < live.w as i32 && ay < live.h as i32 {
                            live.px[(ay * live.w as i32 + ax) as usize]
                        } else {
                            0
                        };
                        total += (a as i32 - b as i32).unsigned_abs() as u64;
                        count += 1;
                    }
                }
                let score = total as f64 / count as f64;
                if score < best {
                    best = score;
                }
            }
        }
        best
    }

    fn art(gray: &Gray) -> String {
        (0..gray.h)
            .map(|y| {
                (0..gray.w)
                    .map(|x| {
                        if gray.px[y * gray.w + x] > 0 {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn game_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(
            std::env::var("MSCOUT_MSCW_DIR")
                .unwrap_or_else(|_| r"D:\SteamLibrary\steamapps\common\MapleStory\MSCW".into()),
        )
    }

    /// Pull the Noto Sans KR Medium TTF out of `Font_000.wz` (`FONT_DATA` has a
    /// 7-byte wrapper; the sfnt table directory gives its real length).
    fn extract_noto() -> Option<std::path::PathBuf> {
        use wz_reader::WzNodeCast;

        let dir = game_dir();
        let wz = [
            dir.join("Data/UI/Font/Font_000.wz"),
            dir.join("MSCW/Data/UI/Font/Font_000.wz"),
        ]
        .into_iter()
        .find(|path| path.is_file())?;
        let root = crate::game_font::open_wz(&wz).ok()?;
        let node = root
            .read()
            .ok()?
            .at_path_parsed("NotoSansKRMedium.img/FONT_DATA")
            .ok()?;
        let guard = node.read().ok()?;
        let data = guard.try_as_raw_data()?.get_buffer();
        // The payload normally starts with a valid sfnt header (`00 01 00 00`).
        // Older notes mentioned a 7-byte wrapper; accept either layout.
        let offset = if data.starts_with(&[0, 1, 0, 0])
            || data.starts_with(b"OTTO")
            || data.starts_with(b"true")
        {
            0
        } else {
            7
        };
        if data.len() < offset + 12 {
            return None;
        }
        let ttf = &data[offset..];
        let tables = u16::from_be_bytes([ttf[4], ttf[5]]) as usize;
        if tables == 0 || ttf.len() < 12 + tables * 16 {
            return None;
        }
        let mut end = 12 + tables * 16;
        for index in 0..tables {
            let record = &ttf[12 + index * 16..];
            let offset = u32::from_be_bytes(record[8..12].try_into().ok()?) as usize;
            let length = u32::from_be_bytes(record[12..16].try_into().ok()?) as usize;
            end = end.max(offset + length);
        }
        if end > ttf.len() {
            return None;
        }
        let out = crate::data_dir().ok()?.join("fonts").join("NotoSansKR-Medium.ttf");
        std::fs::create_dir_all(out.parent()?).ok()?;
        std::fs::write(&out, &ttf[..end]).ok()?;
        println!("extracted {} bytes to {}", end, out.display());
        Some(out)
    }

    #[test]
    #[ignore = "diagnostic: tests the extracted Noto Sans KR face against the saved labels"]
    fn probe_noto_label() {
        use windows_sys::Win32::Graphics::Gdi::{AddFontResourceExW, RemoveFontResourceExW, FR_PRIVATE};

        let Some(ttf) = extract_noto() else {
            println!("could not extract Noto TTF");
            return;
        };
        let wide: Vec<u16> = ttf
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let added = unsafe { AddFontResourceExW(wide.as_ptr(), FR_PRIVATE, std::ptr::null()) };
        println!("AddFontResourceExW -> {added}");

        let data = crate::data_dir().expect("data dir");
        let samples = [
            ("20261114-122032", "Kerning City Civic Center"),
            ("20261114-122137", "Sunset Sky"),
        ];
        for (folder, text) in samples {
            let path = data.join("map-samples").join(folder).join("line-1.bmp");
            let frame = load_bmp(&path);
            let live = binarize(&frame_gray(&frame), 180);
            let mut results = Vec::new();
            for face in ["Noto Sans KR Medium", "Noto Sans KR"] {
                for bold in [false, true] {
                    for quality in [3u8, 4] {
                        for threshold in [128u8, 160, 180, 200] {
                            for px in 9..=14 {
                                let Some(rendered) = render_text_bitmap(
                                    face, px, bold, quality, threshold, text,
                                ) else {
                                    continue;
                                };
                                results.push((
                                    dtw_distance(&live, &rendered),
                                    face,
                                    bold,
                                    quality,
                                    threshold,
                                    px,
                                    rendered.w,
                                    rendered.h,
                                ));
                            }
                        }
                    }
                }
            }
            results.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            println!("\n== {folder} Noto (\"{text}\") ==");
            for (score, face, bold, quality, threshold, px, w, h) in results.iter().take(12) {
                println!(
                    "{score:6.1}  {face:<20} bold={bold:<5} q={quality} t={threshold} px={px} -> {w}x{h}"
                );
            }
        }

        unsafe {
            RemoveFontResourceExW(wide.as_ptr(), FR_PRIVATE, std::ptr::null());
        }
    }

    #[test]
    #[ignore = "diagnostic: matches the saved labels against the client's name table"]
    fn match_saved_names() {
        let data = crate::data_dir().expect("data dir");
        let dir = game_dir();
        let index = label_index(&dir).expect("label index");
        println!(
            "index: {} map names, {} streets",
            index.map_names.len(),
            index.streets.len()
        );

        for (folder, file, candidates, expected) in [
            (
                "20261114-122032",
                "line-1.bmp",
                &index.map_names,
                "Kerning City Civic Center",
            ),
            (
                "20261114-122137",
                "line-1.bmp",
                &index.map_names,
                "Sunset Sky",
            ),
            (
                "20261114-122032",
                "line-0.bmp",
                &index.streets,
                "Victoria Road",
            ),
        ] {
            let path = data.join("map-samples").join(folder).join(file);
            let frame = load_bmp(&path);
            let gray = frame_gray(&frame);
            let best = match_label(&gray, candidates, 8, 10.0);
            println!(
                "{folder}/{file}: expected {expected:?} -> {best:?}"
            );
            let binary = binarize_gray(&gray, 180);
            let mut scores: Vec<(f64, String)> = candidates
                .iter()
                .filter(|(_, rendered)| {
                    (binary.w as i32 - rendered.w as i32).abs() <= 8
                })
                .map(|(text, rendered)| (dtw_distance(&binary, rendered), text.clone()))
                .collect();
            scores.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (score, name) in scores.iter().take(5) {
                println!("    {score:.3}  {name}");
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: compares live t/f/i/r with their template cells"]
    fn probe_t_f() {
        let data = crate::data_dir().expect("data dir");
        let path = data
            .join("map-samples")
            .join("20261114-122032")
            .join("line-1.bmp");
        let frame = load_bmp(&path);
        let gray = frame_gray(&frame);
        let live = binarize(&gray, 180);
        println!("live {}x{}", live.w, live.h);
        for x in (40..80).step_by(1) {
            if x % 10 == 0 {
                print!("\n{x:3} ");
            }
        }
        println!();
        for y in 0..live.h {
            let row: String = (40..80)
                .map(|x| if live.px[y * live.w + x] > 0 { '#' } else { '.' })
                .collect();
            println!("{y:2} {row}");
        }

        let font = label_font().expect("font");
        for ch in ['t', 'f', 'i', 'r', 'l'] {
            let Some(glyph) = font.glyphs.iter().find(|glyph| glyph.ch == ch) else {
                continue;
            };
            println!(
                "\n'{}' advance={} cell {}x{}\n{}",
                ch,
                glyph.advance,
                glyph.cell.w,
                glyph.cell.h,
                art(&glyph.cell)
            );
            let mut best = (f64::MAX, 0, 0);
            for x in 40..80 {
                for cy in 0..=4 {
                    let padded = {
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
                        (Gray { w, h, px }, left_pad)
                    };
                    let (padded, left_pad) = padded;
                    let cost = cell_cost(
                        &padded,
                        x + left_pad as i32,
                        cy,
                        glyph,
                        glyph.advance,
                    );
                    if cost < best.0 {
                        best = (cost, x, cy);
                    }
                }
            }
            println!("  min cost {:.0} at x={} cy={}", best.0, best.1, best.2);
        }
    }

    #[test]
    #[ignore = "diagnostic: reads the saved map/street lines with the DP OCR"]
    fn read_saved_labels() {
        let data = crate::data_dir().expect("data dir");
        for (folder, file, expected) in [
            (
                "20261114-122032",
                "line-1.bmp",
                "Kerning City Civic Center",
            ),
            ("20261114-122137", "line-1.bmp", "Sunset Sky"),
            ("20261114-122032", "line-0.bmp", "Victoria Road"),
        ] {
            let path = data.join("map-samples").join(folder).join(file);
            if !path.is_file() {
                println!("{folder}/{file}: missing");
                continue;
            }
            let frame = load_bmp(&path);
            let gray = frame_gray(&frame);
            let read = read_label_line(&gray);
            println!(
                "{folder}/{file}: {}x{} expected {expected:?} -> {read:?}",
                gray.w, gray.h
            );
        }
    }

    #[test]
    #[ignore = "diagnostic: prints the live label next to candidate font renders"]
    fn probe_sample_art() {
        let data = crate::data_dir().expect("data dir");
        let folder = "20261114-122032";
        let text = "Kerning City Civic Center";
        let path = data.join("map-samples").join(folder).join("line-1.bmp");
        let frame = load_bmp(&path);
        let live = binarize(&frame_gray(&frame), 180);
        println!("live {}x{}\n{}", live.w, live.h, art(&live));

        for (font, px, bold, quality) in [
            ("Arial", 12, true, 4u8),
            ("Microsoft Sans Serif", 11, true, 4),
            ("MS Sans Serif", 11, true, 3),
            ("Trebuchet MS", 13, true, 4),
            ("Tahoma", 12, true, 4),
            ("Segoe UI", 13, true, 4),
        ] {
            if let Some(rendered) = render_text_bitmap(font, px, bold, quality, 180, text) {
                println!(
                    "\n== {font} {px}px bold={bold} q={quality} {}x{} ==\n{}",
                    rendered.w,
                    rendered.h,
                    art(&rendered)
                );
            }
        }
    }

    /// Rescale the candidate to the live mask's size before comparing. The
    /// client may use fractional advances, so the same face can accumulate a few
    /// pixels of width difference by the end of a line.
    #[allow(dead_code)]
    fn scaled_distance(live: &Gray, rendered: &Gray, search: i32) -> f64 {
        let scaled = crate::ocr::resample(rendered, live.w, live.h);
        mask_distance(live, &scaled, search)
    }

    #[test]
    #[ignore = "diagnostic: identifies the map-label font from the saved samples"]
    fn probe_sample_fonts() {
        let samples = [
            ("20261114-122032", "Kerning City Civic Center"),
            ("20261114-122137", "Sunset Sky"),
        ];
        let fonts = [
            "Arial",
            "Arial Narrow",
            "Arial Black",
            "Tahoma",
            "Verdana",
            "Trebuchet MS",
            "Segoe UI",
            "Microsoft Sans Serif",
            "MS Sans Serif",
            "Calibri",
            "Franklin Gothic Medium",
            "Lucida Sans Unicode",
        ];
        let data = crate::data_dir().expect("data dir");

        for (folder, text) in samples {
            let path = data.join("map-samples").join(folder).join("line-1.bmp");
            if !path.is_file() {
                println!("{folder}: no line-1.bmp");
                continue;
            }
            let frame = load_bmp(&path);
            let live_raw = frame_gray(&frame);
            let live = binarize(&live_raw, 180);
            let mut results = Vec::new();

            for font in fonts {
                for bold in [false, true] {
                    for quality in [3u8, 4] {
                        for threshold in [128u8, 160, 180, 200] {
                            for px in 9..=14 {
                                let Some(rendered) = render_text_bitmap(
                                    font, px, bold, quality, threshold, text,
                                ) else {
                                    continue;
                                };
                                let score = dtw_distance(&live, &rendered);
                                results.push((
                                    score,
                                    font,
                                    bold,
                                    quality,
                                    threshold,
                                    px,
                                    rendered.w,
                                    rendered.h,
                                ));
                            }
                        }
                    }
                }
            }

            results.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            println!(
                "\n== {folder} (\"{text}\") live {}x{} ==",
                live.w, live.h
            );
            for (score, font, bold, quality, threshold, px, w, h) in
                results.iter().take(15)
            {
                println!(
                    "{score:6.1}  {font:<22} bold={bold:<5} q={quality} t={threshold} px={px} -> {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn identity_case_bar_is_a_lowercase_l_inside_a_word() {
        assert_eq!(fix_identity_case("EurobIood"), "Euroblood");
        assert_eq!(fix_identity_case("Page"), "Page");
        assert_eq!(fix_identity_case("II"), "II");
        assert_eq!(fix_identity_case("Ivan"), "Ivan");
        assert_eq!(fix_identity_case("BluebeIl"), "Bluebell");
        assert_eq!(fix_identity_case("Dawn Warrior"), "Dawn Warrior");
    }

    #[test]
    #[ignore = "diagnostic: map label segmentation at fixed thresholds"]
    fn probe_label_thresholds() {
        let data = crate::data_dir().expect("data dir");
        let frame = load_bmp(&data.join("debug-frame.bmp"));
        let Some(line) = crate::ocr::map_label_lines(&frame).and_then(|lines| lines.last().cloned())
        else {
            println!("no label lines");
            return;
        };
        let Ok(glyphs) = render_variant("Arial", 12, true, 3, 128, PRINTABLE) else {
            return;
        };

        for threshold in [60u8, 80, 100, 120, 140, 160, 180, 200, 220] {
            let mut binary = line.clone();
            for pixel in &mut binary.px {
                *pixel = if *pixel > threshold { 255 } else { 0 };
            }
            let shapes = crate::ocr::text_segments(&binary);
            let sizes: Vec<(u32, u32)> = shapes.iter().map(|s| (s.w, s.h)).collect();
            println!(
                "threshold {threshold}: {} shapes {:?} score {:?}",
                shapes.len(),
                sizes,
                score(&glyphs, &binary)
            );

            if threshold == 180 {
                let mut atlas = Atlas::default();
                for glyph in glyphs.clone() {
                    atlas.learn_glyph(glyph.ch, glyph.bitmap, glyph.w, glyph.h);
                }
                println!(
                    "  read at 180: {:?}",
                    atlas.recognize_text(&binary, (line.h as i32 / 3).max(2))
                );
                for guess in atlas.explain_text(&binary) {
                    println!(
                        "   shape {}x{} at {} -> '{}' d={:.1}",
                        guess.w, guess.h, guess.x, guess.ch, guess.distance
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: scores label fonts (bold, quality, threshold, size)"]
    fn probe_label_styles() {
        let data = crate::data_dir().expect("data dir");
        let frame = load_bmp(&data.join("debug-frame.bmp"));
        let Some(line) = crate::ocr::map_label_lines(&frame).and_then(|lines| lines.last().cloned())
        else {
            println!("no label lines");
            return;
        };

        let mut results = Vec::new();
        for bold in [false, true] {
            for (quality, quality_name) in [(3u8, "1bit"), (4u8, "aa")] {
                for threshold in [64u8, 96, 128, 160, 192] {
                    for px in 8..=13 {
                        let Ok(glyphs) =
                            render_variant("Arial", px, bold, quality, threshold, PRINTABLE)
                        else {
                            continue;
                        };
                        if let Some(score) = score(&glyphs, &line) {
                            results.push((score, bold, quality_name, threshold, px));
                        }
                    }
                }
            }
        }
        results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (score, bold, quality, threshold, px) in results.iter().take(20) {
            println!(
                "score {score:.1} bold={bold} quality={quality} threshold={threshold} px={px}"
            );
        }

        for (bold, quality, threshold, px) in
            [(true, 4, 192, 12), (true, 3, 128, 12), (false, 4, 128, 10)]
        {
            let Ok(glyphs) = render_variant("Arial", px, bold, quality, threshold, PRINTABLE)
            else {
                continue;
            };
            let mut atlas = Atlas::default();
            for glyph in glyphs {
                atlas.learn_glyph(glyph.ch, glyph.bitmap, glyph.w, glyph.h);
            }
            println!(
                "variant bold={bold} q={quality} t={threshold} px={px}: read={:?}",
                atlas.recognize_text(&line, (line.h as i32 / 3).max(2))
            );
            for guess in atlas.explain_text(&line) {
                println!(
                    "   shape {}x{} at {} -> '{}' d={:.1}",
                    guess.w, guess.h, guess.x, guess.ch, guess.distance
                );
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: seeds Arial from the shipped Ideal name/job boxes"]
    fn reads_ideal_identity() {
        let data = crate::data_dir().expect("data dir");
        for (name, boxes) in [
            (
                "live-frame-1366.bmp",
                [
                    (
                        "job",
                        Region {
                            x: 366,
                            y: 737,
                            w: 34,
                            h: 12,
                        },
                    ),
                    (
                        "name",
                        Region {
                            x: 366,
                            y: 751,
                            w: 60,
                            h: 13,
                        },
                    ),
                ],
            ),
            (
                "live-frame.bmp",
                [
                    (
                        "job",
                        Region {
                            x: 645,
                            y: 1166,
                            w: 96,
                            h: 15,
                        },
                    ),
                    (
                        "name",
                        Region {
                            x: 645,
                            y: 1181,
                            w: 96,
                            h: 15,
                        },
                    ),
                ],
            ),
            (
                "live-frame-1080.bmp",
                [
                    (
                        "job",
                        Region {
                            x: 645,
                            y: 1050,
                            w: 27,
                            h: 11,
                        },
                    ),
                    (
                        "name",
                        Region {
                            x: 645,
                            y: 1064,
                            w: 55,
                            h: 9,
                        },
                    ),
                ],
            ),
        ] {
            let frame = load_bmp(&data.join(name));
            for (kind, region) in boxes {
                let Some(gray) = crate::ocr::crop_gray(&frame, region) else {
                    println!("{name} {kind}: crop outside frame");
                    continue;
                };
                match seed_identity(&gray) {
                    Some((stored, px)) => println!(
                        "{name} {kind}: box {region:?} font px={px} read={:?}",
                        read(&gray, &stored)
                    ),
                    None => println!("{name} {kind}: no Arial size matched ({region:?})"),
                }
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: seeds Arial from the measured 1920x1080 Default name/job boxes"]
    fn reads_1080_default_identity() {
        let data = crate::data_dir().expect("data dir");
        let frame = load_bmp(&data.join("live-frame-1080-default.bmp"));
        for (kind, region) in [
            (
                "job",
                Region {
                    x: 514,
                    y: 1035,
                    w: 45,
                    h: 20,
                },
            ),
            (
                "name",
                Region {
                    x: 514,
                    y: 1055,
                    w: 82,
                    h: 18,
                },
            ),
        ] {
            let Some(gray) = crate::ocr::crop_gray(&frame, region) else {
                println!("{kind}: crop outside frame");
                continue;
            };
            match seed_identity(&gray) {
                Some((stored, px)) => {
                    println!(
                        "{kind}: box {region:?} crop {}x{} font px={px} read={:?}",
                        gray.w,
                        gray.h,
                        read(&gray, &stored)
                    );
                }
                None => println!("{kind}: no Arial size matched"),
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: renders Arial and reads the live name/job/map label"]
    fn reads_live_text() {
        let data = crate::data_dir().expect("data dir");
        let path = data.join("debug-frame.bmp");
        if !path.is_file() {
            println!("no {} — run capture::tests::dumps_a_frame_for_inspection", path.display());
            return;
        }
        let frame = load_bmp(&path);
        let profile = crate::profile::load_for_mode(1920, 1080, UiMode::Ideal)
            .expect("1920x1080@ideal profile");

        for (kind, region) in [
            ("job", profile.calibration.job),
            ("name", profile.calibration.name),
        ] {
            let Some(region) = region.filter(Region::is_usable) else {
                println!("{kind}: no box");
                continue;
            };
            let Some(gray) = crate::ocr::crop_gray(&frame, region) else {
                println!("{kind}: crop outside frame");
                continue;
            };
            match seed_identity(&gray) {
                Some((stored, px)) => {
                    let atlas = Atlas::from_settings(&stored);
                    println!(
                        "{kind}: box {region:?} crop {}x{} font px={px} glyphs={} read={:?}",
                        gray.w,
                        gray.h,
                        stored.glyphs.len(),
                        read(&gray, &stored)
                    );
                    for (row, line) in crate::ocr::shape_art(&gray).iter().enumerate() {
                        println!("{row:2} {line}");
                    }
                    for guess in atlas.explain(&gray) {
                        println!(
                            "   shape {}x{} at {} -> '{}' d={:.1} known={}",
                            guess.w,
                            guess.h,
                            guess.x,
                            guess.ch,
                            guess.distance,
                            guess.known
                        );
                    }
                }
                None => println!("{kind}: no Arial size matched"),
            }
        }

        let Some(lines) = crate::ocr::map_label_lines(&frame) else {
            println!("map: no label lines");
            return;
        };
        for (index, line) in lines.iter().enumerate() {
            println!("label line {index}: {}x{}", line.w, line.h);
            for (row, text) in crate::ocr::shape_art(line).iter().enumerate() {
                println!("{row:2} {text}");
            }
            let hint = crate::ocr::dominant_glyph_height(line).unwrap_or(9) as i32;
            for px in (hint - 1).max(6)..=(hint + 8).min(28) {
                if let Ok(glyphs) = render_ascii("Arial", px, true, PRINTABLE) {
                    println!("  px {px} score {:?}", score(&glyphs, line));
                }
            }
            match seed_label(line) {
                Some((stored, px)) => {
                    let atlas = Atlas::from_settings(&stored);
                    println!(
                        "label line {index}: font px={px} glyphs={} read={:?}",
                        stored.glyphs.len(),
                        read(line, &stored)
                    );
                    for guess in atlas.explain_text(line) {
                        println!(
                            "   shape {}x{} at {} -> '{}' d={:.1} known={}",
                            guess.w,
                            guess.h,
                            guess.x,
                            guess.ch,
                            guess.distance,
                            guess.known
                        );
                    }
                }
                None => println!("label line {index}: no Arial size matched"),
            }
        }
    }
}
