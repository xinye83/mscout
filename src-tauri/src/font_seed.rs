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

/// A candidate that leaves a shape unexplained is worse than one whose mean
/// distance is slightly higher. Without this, Default Ratio could pick an 18 px
/// render that matches two job letters exactly and drops the other two.
const UNKNOWN_SHAPE_PENALTY: f64 = 50.0;

/// Seed a 1-bit identity atlas (name/job) from the live crop.
pub fn seed_identity(gray: &Gray) -> Option<(settings::StoredAtlas, u32)> {
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
) -> Option<(settings::StoredAtlas, u32)> {
    let hint = ocr::dominant_glyph_height(gray).unwrap_or(9).max(4) as i32;
    // The ink height is smaller than the font's em size (Arial at 11 px gives a
    // ~8 px cap). Search from the ink height up, which covers both Ideal's 7 px
    // and Default's larger text.
    let low = (hint - 1).max(6);
    // Default Ratio's HUD text is larger than the ink-height hint suggests; the
    // best match can be well above it.
    let high = (hint + 14).min(36);

    let mut best: Option<(f64, f64, f64, Vec<AtlasGlyph>, i32)> = None;
    for &(anti_aliased, threshold) in modes {
        for px in low..=high {
            let glyphs = match render_mode("Arial", px, anti_aliased, threshold, charset) {
                Ok(glyphs) => glyphs,
                Err(_) => continue,
            };
            let Some((rank, mean, unknown)) = score(&glyphs, gray) else {
                continue;
            };
            // A render that explains every shape may sit a little above the
            // normal text threshold (Default Ratio's anti-aliasing); a render
            // that drops shapes must still meet it.
            let acceptable = mean <= max_score || (unknown == 0.0 && mean <= max_score * 1.5);
            if acceptable
                && best
                    .as_ref()
                    .map_or(true, |(best_rank, _, _, _, _)| rank < *best_rank)
            {
                best = Some((rank, mean, unknown, glyphs, px));
            }
        }
    }

    let (_, _mean, _unknown, glyphs, px) = best?;

    let mut atlas = Atlas::default();
    for glyph in glyphs {
        atlas.learn_glyph(glyph.ch, glyph.bitmap, glyph.w, glyph.h);
    }
    // Learn the live rendering only when the GDI atlas explains every shape. A
    // partial fit is a wrong size or a filtered rendering; learning it would bake
    // a wrong letter (the Filtered `Euroh ood`) into the atlas as an exact variant.
    let guesses = atlas.explain_text(gray);
    if !guesses.is_empty() && guesses.iter().all(|guess| guess.known) {
        atlas.learn_text_rendering(gray);
    }
    let mut stored = atlas.to_settings();
    stored.format = 2;
    stored.source = "font".into();
    Some((stored, px as u32))
}

/// `(rank, mean)` for a candidate render. The mean is the best-match distance
/// of every segmented shape; the correct font size reproduces the crop, so its
/// mean is near zero. The rank penalizes unexplained shapes so a size that
/// matches two letters exactly and drops two cannot win on mean alone.
fn score(glyphs: &[AtlasGlyph], gray: &Gray) -> Option<(f64, f64, f64)> {
    let mut atlas = Atlas::default();
    for glyph in glyphs {
        atlas.learn_glyph(glyph.ch, glyph.bitmap.clone(), glyph.w, glyph.h);
    }
    let guesses = atlas.explain(gray);
    if guesses.is_empty() {
        return None;
    }
    let total: f64 = guesses.iter().map(|guess| guess.distance).sum();
    let mean = total / guesses.len() as f64;
    let unknown = guesses.iter().filter(|guess| !guess.known).count() as f64;
    Some((mean + unknown * UNKNOWN_SHAPE_PENALTY, mean, unknown))
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
        let bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
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
            if bold {
                FW_BOLD as i32
            } else {
                FW_NORMAL as i32
            },
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
        out.dedup_by(|a, b| a.ch == b.ch && a.w == b.w && a.h == b.h && a.bitmap == b.bitmap);
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
        let bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
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
            if bold {
                FW_BOLD as i32
            } else {
                FW_NORMAL as i32
            },
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

static LABEL_INDEX: std::sync::Mutex<Vec<(String, std::sync::Arc<LabelIndex>)>> =
    std::sync::Mutex::new(Vec::new());

/// Build or reuse the client's map/street name index at `px`.
///
/// Names are rendered once with the identified label font (Arial Bold, GDI
/// anti-aliased) and cached per size. Matching a live line is then a width filter
/// plus a shape-sequence DTW, so no column segmentation is involved. Default
/// Ratio draws the label larger than Ideal, so the caller tries several sizes.
pub fn label_index(mscw_dir: &std::path::Path, px: u32) -> Option<std::sync::Arc<LabelIndex>> {
    let key = format!("{}#{px}", mscw_dir.to_string_lossy());
    let mut guard = LABEL_INDEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, index)) = guard.iter().find(|(cached, _)| cached == &key) {
        return Some(index.clone());
    }

    let entries = crate::map_names::load(mscw_dir).ok()?;
    let mut seen_names = std::collections::HashSet::new();
    let mut seen_streets = std::collections::HashSet::new();
    let mut seen_pairs = std::collections::HashSet::new();
    let mut map_names = Vec::new();
    let mut streets = Vec::new();
    let mut folded_lines = Vec::new();
    let px = px as i32;

    for entry in entries {
        if seen_names.insert(entry.name.clone()) {
            if let Some(gray) = render_text_bitmap("Arial", px, true, 4, 180, &entry.name) {
                map_names.push((entry.name.clone(), gray));
            }
        }
        if !entry.street.is_empty() && seen_streets.insert(entry.street.clone()) {
            if let Some(gray) = render_text_bitmap("Arial", px, true, 4, 180, &entry.street) {
                streets.push((entry.street.clone(), gray));
            }
        }
        if seen_pairs.insert((entry.street.clone(), entry.name.clone())) {
            let line = format!("{} :{}", entry.street, entry.name);
            if let Some(gray) = render_text_bitmap("Arial", px, false, 4, 180, &line) {
                folded_lines.push(((entry.street, entry.name), gray));
            }
        }
    }

    let index = std::sync::Arc::new(LabelIndex {
        map_names,
        streets,
        folded_lines,
    });
    guard.push((key, index.clone()));
    if guard.len() > 16 {
        guard.remove(0);
    }
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

/// The last word-like run of a label line: the text after its final wide gap.
/// `Free Market <8>` splits to `<8>`, which is what distinguishes map instances
/// that share a base name.
fn binarized_suffix(gray: &Gray) -> Option<Gray> {
    let binary = binarize_gray(gray, 180);
    let ink = |x: usize| (0..binary.h).any(|y| binary.px[y * binary.w + x] > 0);
    let mut runs = Vec::new();
    let mut x = 0;
    while x < binary.w {
        if !ink(x) {
            let start = x;
            while x < binary.w && !ink(x) {
                x += 1;
            }
            runs.push((start, x - start));
        } else {
            x += 1;
        }
    }
    // The folded bar draws zoom controls and the WORLD badge to the right of
    // the text, separated by a wide blank gap. Ignore that tail so the suffix
    // is the map text's last token, not a control icon.
    let mut limit = binary.w;
    while let Some((start, len)) = runs.last().copied() {
        if len >= 10 && start > binary.w / 2 {
            limit = start;
            runs.pop();
        } else {
            break;
        }
    }
    let pick = |min_len: usize| {
        runs.iter()
            .rev()
            .find(|(start, len)| {
                if *len < min_len || *start <= binary.w / 2 || *start + *len > limit {
                    return false;
                }
                let suffix_width = limit - (*start + *len);
                (8..=64).contains(&suffix_width)
            })
            .copied()
    };
    // A word space is wider than the gaps between glyphs; prefer it so the
    // suffix starts at `<N>`, not inside it (where `<8>` would look like `<18>`).
    let (start, len) = pick(4).or_else(|| pick(2))?;
    let x0 = start + len;
    if x0 + 4 >= limit {
        return None;
    }
    let mut y0 = binary.h;
    let mut y1 = 0;
    for yy in 0..binary.h {
        for xx in x0..limit {
            if binary.px[yy * binary.w + xx] > 0 {
                y0 = y0.min(yy);
                y1 = y1.max(yy + 1);
            }
        }
    }
    if y0 >= y1 {
        return None;
    }
    let mut suffix = Gray {
        w: limit - x0,
        h: y1 - y0,
        px: Vec::new(),
    };
    for yy in y0..y1 {
        suffix
            .px
            .extend_from_slice(&binary.px[yy * binary.w + x0..yy * binary.w + limit]);
    }
    Some(suffix)
}

/// DTW distance between the trailing token of a live label and a rendered
/// candidate. Comparing only the suffix keeps the shared `Free Market` prefix
/// from diluting the one character that identifies the instance.
pub(crate) fn suffix_distance(live: &Gray, rendered: &Gray) -> Option<f64> {
    let live = binarized_suffix(live)?;
    let rendered = binarized_suffix(rendered)?;
    if live.w == 0 || rendered.w == 0 {
        return None;
    }
    let ratio = rendered.w as f64 / live.w as f64;
    if !(0.4..=2.2).contains(&ratio) {
        return None;
    }
    let live = crate::ocr::resample(&live, 60, 16);
    let rendered = crate::ocr::resample(&rendered, 60, 16);
    Some(dtw_distance(&live, &rendered))
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
    #[ignore = "diagnostic: folded-bar suffix art"]
    fn probe_folded_suffix_art() {
        let dir = std::env::var("MSCOUT_MSCW_DIR")
            .ok()
            .or_else(crate::settings::mscw_dir)
            .expect("MSCW dir");
        let dir = std::path::Path::new(&dir);
        let art = |gray: &Gray| -> String {
            let binary = binarize_gray(gray, 180);
            (0..binary.h)
                .map(|y| {
                    (0..binary.w)
                        .map(|x| {
                            if binary.px[y * binary.w + x] > 0 {
                                '#'
                            } else {
                                '.'
                            }
                        })
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        for pair in ["1920x1080", "1920x1080@filtered"] {
            let path = crate::regression::fixture_image(pair, "minimap-folded").0;
            let image = image::open(&path).expect("folded fixture").to_rgba8();
            let frame = crate::capture::Frame {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
            };
            let Some(minimap) = crate::ocr::detect_minimap(&frame) else {
                continue;
            };
            let Some(label) = crate::ocr::folded_label(&frame, &minimap) else {
                continue;
            };
            println!("== {pair} label {}x{}", label.w, label.h);
            let binary = binarize_gray(&label, 180);
            let ink = |x: usize| (0..binary.h).any(|y| binary.px[y * binary.w + x] > 0);
            let mut runs = Vec::new();
            let mut x = 0;
            while x < binary.w {
                if !ink(x) {
                    let start = x;
                    while x < binary.w && !ink(x) {
                        x += 1;
                    }
                    runs.push((start, x - start));
                } else {
                    x += 1;
                }
            }
            println!("gaps: {:?}", runs);
            println!("label art:\n{}", art(&label));
            if let Some(suffix) = binarized_suffix(&label) {
                println!(
                    "live suffix {}x{} art:\n{}",
                    suffix.w,
                    suffix.h,
                    art(&suffix)
                );
            } else {
                println!("live suffix: none");
            }
            for px in [18u32, 20, 22, 24] {
                let Some(index) = label_index(dir, px) else {
                    continue;
                };
                let Some((_, rendered)) = index
                    .folded_lines
                    .iter()
                    .find(|((s, n), _)| s == "Hidden Street" && n == "Free Market <8>")
                else {
                    continue;
                };
                println!("  px {px} rendered {}x{}", rendered.w, rendered.h);
                if let Some(suffix) = binarized_suffix(rendered) {
                    println!(
                        "    rendered suffix {}x{} art:\n{}",
                        suffix.w,
                        suffix.h,
                        art(&suffix)
                    );
                } else {
                    println!("    rendered suffix: none");
                }
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: Free Market suffix matching at 1920 Default/Filtered"]
    fn probe_free_market_suffix() {
        let dir = std::env::var("MSCOUT_MSCW_DIR")
            .ok()
            .or_else(crate::settings::mscw_dir)
            .expect("MSCW dir");
        let dir = std::path::Path::new(&dir);
        for pair in ["1920x1080", "1920x1080@filtered"] {
            let path = crate::regression::fixture_image(pair, "minimap-expanded").0;
            let image = image::open(&path).expect("minimap fixture").to_rgba8();
            let frame = crate::capture::Frame {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
            };
            let Some(lines) = crate::ocr::map_label_lines(&frame) else {
                continue;
            };
            let Some(live) = lines.last() else { continue };
            println!("== {pair} live {}x{}", live.w, live.h);
            let live_suffix = binarized_suffix(live);
            for px in [12u32, 14, 16, 18, 20, 22, 24] {
                let Some(index) = label_index(dir, px) else {
                    continue;
                };
                let candidates: Vec<(String, Gray)> = index
                    .map_names
                    .iter()
                    .filter(|(name, _)| name.starts_with("Free Market"))
                    .cloned()
                    .collect();
                let current = match_label(live, &candidates, 14, 10.0);
                let mut normalized = Vec::new();
                for (name, rendered) in &candidates {
                    let live_n = crate::ocr::resample(live, 240, 16);
                    let rendered_n = crate::ocr::resample(rendered, 240, 16);
                    let score = dtw_distance(&live_n, &rendered_n);
                    normalized.push((score, name.clone(), rendered.w, rendered.h));
                }
                normalized.sort_by(|a, b| a.0.total_cmp(&b.0));
                println!(
                    "  px {px}: current={current:?} normalized={:?}",
                    normalized.iter().take(3).collect::<Vec<_>>()
                );
                if let (Some(street_live), Some((_, street_rendered))) = (
                    lines.first(),
                    index
                        .streets
                        .iter()
                        .find(|(name, _)| name == "Hidden Street"),
                ) {
                    let scale_x = street_live.w as f64 / street_rendered.w as f64;
                    let mut by_width: Vec<(f64, String, usize)> = candidates
                        .iter()
                        .map(|(name, rendered)| {
                            (
                                (rendered.w as f64 * scale_x - live.w as f64).abs(),
                                name.clone(),
                                rendered.w,
                            )
                        })
                        .collect();
                    by_width.sort_by(|a, b| a.0.total_cmp(&b.0));
                    println!(
                        "    scale_x={scale_x:.3} by_width={:?}",
                        by_width.iter().take(3).collect::<Vec<_>>()
                    );
                }
                if let Some(live_suffix) = &live_suffix {
                    let mut by_suffix: Vec<(f64, String, usize, usize)> = candidates
                        .iter()
                        .filter_map(|(name, rendered)| {
                            let rendered_suffix = binarized_suffix(rendered)?;
                            let a = crate::ocr::resample(live_suffix, 60, 16);
                            let b = crate::ocr::resample(&rendered_suffix, 60, 16);
                            Some((
                                dtw_distance(&a, &b),
                                name.clone(),
                                rendered_suffix.w,
                                rendered_suffix.h,
                            ))
                        })
                        .collect();
                    by_suffix.sort_by(|a, b| a.0.total_cmp(&b.0));
                    println!(
                        "    by_suffix={:?}",
                        by_suffix.iter().take(3).collect::<Vec<_>>()
                    );
                }
            }

            let art = |gray: &Gray| -> String {
                let binary = binarize_gray(gray, 180);
                (0..binary.h)
                    .map(|y| {
                        (0..binary.w)
                            .map(|x| {
                                if binary.px[y * binary.w + x] > 0 {
                                    '#'
                                } else {
                                    '.'
                                }
                            })
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            println!("  live art:\n{}", art(live));
            let binary = binarize_gray(live, 180);
            let ink = |x: usize| (0..binary.h).any(|y| binary.px[y * binary.w + x] > 0);
            let mut runs = Vec::new();
            let mut x = 0;
            while x < binary.w {
                if !ink(x) {
                    let start = x;
                    while x < binary.w && !ink(x) {
                        x += 1;
                    }
                    runs.push((start, x - start));
                } else {
                    x += 1;
                }
            }
            if let Some((start, len)) = runs
                .iter()
                .rev()
                .find(|(start, len)| *len >= 2 && *start > binary.w / 2)
                .copied()
            {
                let x0 = start + len;
                if x0 + 4 < binary.w {
                    let mut y0 = binary.h;
                    let mut y1 = 0;
                    for yy in 0..binary.h {
                        for xx in x0..binary.w {
                            if binary.px[yy * binary.w + xx] > 0 {
                                y0 = y0.min(yy);
                                y1 = y1.max(yy + 1);
                            }
                        }
                    }
                    if y0 < y1 {
                        let mut suffix = Gray {
                            w: binary.w - x0,
                            h: y1 - y0,
                            px: Vec::new(),
                        };
                        for yy in y0..y1 {
                            suffix.px.extend_from_slice(
                                &binary.px[yy * binary.w + x0..yy * binary.w + binary.w],
                            );
                        }
                        println!(
                            "  suffix at {x0} {}x{} art:\n{}",
                            suffix.w,
                            suffix.h,
                            art(&suffix)
                        );
                        let segments = crate::ocr::text_segments(&suffix);
                        println!(
                            "  suffix segments {:?}",
                            segments.iter().map(|s| (s.x, s.w, s.h)).collect::<Vec<_>>()
                        );
                        for (index, segment) in segments.iter().enumerate() {
                            if segment.w < 2 {
                                continue;
                            }
                            let mut crop = Gray {
                                w: segment.w as usize,
                                h: segment.h as usize,
                                px: Vec::new(),
                            };
                            for yy in segment.y as usize..(segment.y + segment.h as i32) as usize {
                                for xx in
                                    segment.x as usize..(segment.x + segment.w as i32) as usize
                                {
                                    crop.px.push(binary.px[yy * binary.w + xx]);
                                }
                            }
                            let up = crate::ocr::resample(&crop, crop.w * 4, crop.h * 4);
                            println!(
                                "    seg {index} {}x{} ocr={:?}",
                                crop.w,
                                crop.h,
                                crate::winocr::recognize(&up)
                            );
                        }
                    }
                }
            }
            if let Some(index) = label_index(dir, 16) {
                for name in [
                    "Free Market <8>",
                    "Free Market <10>",
                    "Free Market Entrance",
                ] {
                    if let Some((_, rendered)) =
                        index.map_names.iter().find(|(entry, _)| entry == name)
                    {
                        println!(
                            "  {name} {}x{} art:\n{}",
                            rendered.w,
                            rendered.h,
                            art(rendered)
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: identity seed candidates for the 1920 Default/Filtered HUDs"]
    fn probe_identity_candidates() {
        let cases = [
            ("default", "job", 112, 13, 70, 22),
            ("default", "name", 112, 33, 110, 20),
            ("filtered", "job", 112, 13, 70, 22),
            ("filtered", "name", 112, 33, 110, 20),
        ];
        for (pair, label, x, y, w, h) in cases {
            let pair_key = if pair == "default" {
                "1920x1080"
            } else {
                "1920x1080@filtered"
            };
            let path = crate::regression::fixture_image(pair_key, "hud-main").0;
            let hud = image::open(&path).expect("hud fixture").to_rgba8();
            let frame = crate::capture::Frame {
                width: hud.width(),
                height: hud.height(),
                rgba: hud.into_raw(),
            };
            let gray = crate::ocr::crop_gray(&frame, crate::settings::Region { x, y, w, h })
                .expect("crop");
            let hint = ocr::dominant_glyph_height(&gray).unwrap_or(9).max(4) as i32;
            let low = (hint - 1).max(6);
            let high = (hint + 14).min(36);
            let gap = (gray.h as i32 / 3).max(2);

            let mut rows = Vec::new();
            for (anti_aliased, threshold) in [(false, 1u8), (true, 96), (true, 128), (true, 160)] {
                for px in low..=high {
                    let Ok(glyphs) = render_mode("Arial", px, anti_aliased, threshold, IDENTITY)
                    else {
                        continue;
                    };
                    let Some((rank, mean, unknown)) = score(&glyphs, &gray) else {
                        continue;
                    };
                    let mut atlas = Atlas::default();
                    for glyph in &glyphs {
                        atlas.learn_glyph(glyph.ch, glyph.bitmap.clone(), glyph.w, glyph.h);
                    }
                    let guesses = atlas.explain_text(&gray);
                    let known = guesses.iter().filter(|guess| guess.known).count();
                    let total = guesses.len();
                    let read = atlas
                        .recognize_text(&gray, gap)
                        .map(|(text, _)| text)
                        .unwrap_or_default();
                    rows.push((
                        rank,
                        mean,
                        unknown,
                        px,
                        anti_aliased,
                        threshold,
                        read,
                        known,
                        total,
                    ));
                }
            }
            rows.sort_by(|a, b| a.0.total_cmp(&b.0));
            println!(
                "== {pair} {label} shapes {:?}",
                crate::ocr::text_segments(&gray).len()
            );
            for (rank, mean, unknown, px, aa, threshold, read, known, total) in rows.iter().take(12)
            {
                println!(
                    "  rank={rank:.1} mean={mean:.1} unknown={unknown} px={px} aa={aa} thr={threshold} read={read:?} known={known}/{total}"
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

    /// The live Ideal name crop for `Eurovein` (1920x1080, saved by a diagnostic).
    /// The `i` is a 1 px bar with a one-pixel gap under its dot; it used to come
    /// out as an empty shape that matched the `I` bar, so the name read
    /// `Euroveln` after the case fix.
    #[test]
    fn a_dotted_i_in_a_name_is_not_read_as_a_bar() {
        let rows = [
            "######................................#...........",
            "#.................................................",
            "#.......#...#..#.#..###..#...#..###...#..#.##.....",
            "#.......#...#..##..#...#.#...#.#...#..#..##..#....",
            "######..#...#..#...#...#..#.#..#...#..#..#...#....",
            "#.......#...#..#...#...#..#.#..#####..#..#...#....",
            "#.......#...#..#...#...#..#.#..#......#..#...#....",
            "#.......#...#..#...#...#...#...#...#..#..#...#....",
            "######...####..#....###....#....###...#..#...#....",
        ];
        let w = rows[0].len();
        let px = rows
            .iter()
            .flat_map(|row| row.bytes().map(|b| if b == b'#' { 239 } else { 60 }))
            .collect();
        let gray = Gray {
            w,
            h: rows.len(),
            px,
        };
        let (stored, _) = seed_identity(&gray).expect("seed");
        let atlas = Atlas::from_settings(&stored);
        let (text, _) = atlas
            .recognize_text(&gray, (gray.h as i32 / 3).max(2))
            .expect("read");
        assert_eq!(fix_identity_case(text.trim()), "Eurovein");
    }
}
