use crate::capture::Frame;
use crate::settings::Region;

pub const CELL_W: usize = 12;
pub const CELL_H: usize = 16;
pub const CELL_LEN: usize = CELL_W * CELL_H;

const MATCH_THRESHOLD: f64 = 30.0;
const MIN_GLYPH_PIXELS: usize = 1;
/// Two shapes of the same character closer than this are the same variant.
const VARIANT_DISTANCE: f64 = 8.0;
/// How far a sample may slide within its cell when matching (sub-pixel drift).
const SHIFT_SEARCH: i32 = 1;

pub type Cell = [u8; CELL_LEN];

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub cell: Cell,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// One segmented shape and what the atlas thinks it is.
#[derive(Clone, Copy, Debug)]
pub struct Guess {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub ch: char,
    pub distance: f64,
    pub known: bool,
    pub cell: Cell,
}

#[derive(Clone, Debug)]
pub struct Glyph {
    pub ch: char,
    pub cell: Cell,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Debug)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

pub fn crop_gray(frame: &Frame, region: Region) -> Option<Gray> {
    if region.w <= 1 || region.h <= 1 {
        return None;
    }

    let x0 = region.x.max(0) as usize;
    let y0 = region.y.max(0) as usize;
    let x1 = (region.x.saturating_add(region.w))
        .min(frame.width as i32)
        .max(0) as usize;
    let y1 = (region.y.saturating_add(region.h))
        .min(frame.height as i32)
        .max(0) as usize;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    let w = x1 - x0;
    let h = y1 - y0;
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let index = ((y0 + y) * frame.width as usize + (x0 + x)) * 4;
            let r = frame.rgba[index] as u32;
            let g = frame.rgba[index + 1] as u32;
            let b = frame.rgba[index + 2] as u32;
            px.push(((77 * r + 150 * g + 29 * b) >> 8) as u8);
        }
    }

    Some(Gray { w, h, px })
}

fn otsu_threshold(gray: &Gray) -> u8 {
    let mut histogram = [0u32; 256];
    for &pixel in &gray.px {
        histogram[pixel as usize] += 1;
    }

    let total = gray.px.len() as f64;
    let sum: f64 = histogram
        .iter()
        .enumerate()
        .map(|(value, count)| value as f64 * *count as f64)
        .sum();

    let mut sum_background = 0.0;
    let mut weight_background = 0.0;
    let mut best_threshold = 0u8;
    let mut best_variance = -1.0;

    for (value, count) in histogram.iter().enumerate() {
        weight_background += *count as f64;
        if weight_background == 0.0 {
            continue;
        }

        let weight_foreground = total - weight_background;
        if weight_foreground == 0.0 {
            break;
        }

        sum_background += value as f64 * *count as f64;
        let mean_background = sum_background / weight_background;
        let mean_foreground = (sum - sum_background) / weight_foreground;
        let variance = weight_background
            * weight_foreground
            * (mean_background - mean_foreground).powi(2);

        if variance > best_variance {
            best_variance = variance;
            best_threshold = value as u8;
        }
    }

    best_threshold
}

fn foreground(gray: &Gray) -> Vec<bool> {
    let threshold = otsu_threshold(gray);
    gray.px.iter().map(|&pixel| pixel > threshold).collect()
}

fn foreground_with_floor(gray: &Gray, floor: u8) -> Vec<bool> {
    let threshold = otsu_threshold(gray).max(floor);
    gray.px.iter().map(|&pixel| pixel > threshold).collect()
}

/// Threshold used when the adaptive one collapses a readout into one blob. These
/// renderings draw near-white text, so anything this bright is text.
const TEXT_FLOOR: u8 = 170;

fn normalize(
    mask: &[bool],
    stride: usize,
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
) -> Cell {
    let mut cell = [0u8; CELL_LEN];
    for gy in 0..CELL_H {
        for gx in 0..CELL_W {
            let sx = x0 + ((gx as f64 + 0.5) * w as f64 / CELL_W as f64) as usize;
            let sy = y0 + ((gy as f64 + 0.5) * h as f64 / CELL_H as f64) as usize;
            let sx = sx.min(x0 + w - 1);
            let sy = sy.min(y0 + h - 1);
            if mask[sy * stride + sx] {
                cell[gy * CELL_W + gx] = 255;
            }
        }
    }
    cell
}

/// How many foreground pixels sit in each column of `gray`. Splitting on the empty
/// columns is how `segments` finds glyphs, so this shows whether a rendering keeps
/// them apart at all.
pub fn column_ink(gray: &Gray) -> Vec<u32> {
    ink_of(&foreground(gray), gray.w, gray.h)
}

/// The same, with the fixed threshold the blob fallback uses.
pub fn column_ink_floor(gray: &Gray, floor: u8) -> Vec<u32> {
    ink_of(&foreground_with_floor(gray, floor), gray.w, gray.h)
}

/// The thresholded crop as text, one line per row, for eyeballing a rendering the
/// reader cannot segment.
pub fn shape_art(gray: &Gray) -> Vec<String> {
    let mask = foreground(gray);
    (0..gray.h)
        .map(|y| {
            (0..gray.w)
                .map(|x| if mask[y * gray.w + x] { '#' } else { '.' })
                .collect()
        })
        .collect()
}

/// The same, per row. Used to find where two stacked lines meet when the gap
/// between them is too thin for `text_lines` to see.
pub fn row_ink(gray: &Gray) -> Vec<u32> {
    let mask = foreground(gray);
    (0..gray.h)
        .map(|y| {
            (0..gray.w)
                .filter(|x| mask[y * gray.w + x])
                .count() as u32
        })
        .collect()
}

fn ink_of(mask: &[bool], w: usize, h: usize) -> Vec<u32> {
    let mut columns = vec![0u32; w];
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                columns[x] += 1;
            }
        }
    }
    columns
}

pub fn segments(gray: &Gray) -> Vec<Segment> {
    let found = split_shapes(gray, None);
    if found.len() > 1 {
        return found;
    }

    // A rendering whose background reaches the same brightness as the text makes
    // OTSU split the gradient instead of the text, and the whole line comes back as
    // a single shape — which is what the Ideal Ratio HUD does. These readouts are
    // near-white, so a high fixed threshold separates them where the adaptive one
    // cannot. Only used as a fallback, so renderings that already work are untouched.
    let bright = split_shapes(gray, Some(TEXT_FLOOR));
    if bright.len() > found.len() {
        bright
    } else {
        found
    }
}

fn split_shapes(gray: &Gray, floor: Option<u8>) -> Vec<Segment> {
    let mask = match floor {
        Some(floor) => foreground_with_floor(gray, floor),
        None => foreground(gray),
    };
    let mut columns = vec![0u32; gray.w];
    for y in 0..gray.h {
        for x in 0..gray.w {
            if mask[y * gray.w + x] {
                columns[x] += 1;
            }
        }
    }

    let mut out = Vec::new();
    let mut x = 0;
    while x < gray.w {
        if columns[x] == 0 {
            x += 1;
            continue;
        }

        let start = x;
        while x < gray.w && columns[x] > 0 {
            x += 1;
        }
        let end = x;

        let mut top = usize::MAX;
        let mut bottom = 0usize;
        let mut pixels = 0usize;
        for y in 0..gray.h {
            for column in start..end {
                if mask[y * gray.w + column] {
                    pixels += 1;
                    top = top.min(y);
                    bottom = bottom.max(y + 1);
                }
            }
        }

        if pixels < MIN_GLYPH_PIXELS || bottom <= top {
            continue;
        }

        out.push(Segment {
            cell: normalize(&mask, gray.w, start, top, end - start, bottom - top),
            x: start as i32,
            y: top as i32,
            w: (end - start) as u32,
            h: (bottom - top) as u32,
        });
    }

    out
}

/// Bounding boxes of the horizontal text lines in `gray`, top to bottom, as
/// `(x, y, w, h)` relative to the crop. A run of blank rows splits the lines; a
/// run shorter than `LINE_GAP` is treated as inside the same line, so a gap
/// between glyphs (or a descender row) does not fragment it.
pub fn text_lines(gray: &Gray) -> Vec<(i32, i32, i32, i32)> {
    const LINE_GAP: usize = 2;
    // Shorter than any glyph line: a panel border or separator caught by a
    // generous box edge, not text.
    const MIN_LINE_H: usize = 4;

    let mask = foreground(gray);
    let mut occupied = vec![false; gray.h];
    for y in 0..gray.h {
        for x in 0..gray.w {
            if mask[y * gray.w + x] {
                occupied[y] = true;
                break;
            }
        }
    }

    let mut lines = Vec::new();
    let mut y = 0;
    while y < gray.h {
        if !occupied[y] {
            y += 1;
            continue;
        }

        let start = y;
        let mut last = y;
        let mut blank = 0;
        while y < gray.h {
            if occupied[y] {
                last = y;
                blank = 0;
            } else {
                blank += 1;
                if blank > LINE_GAP {
                    break;
                }
            }
            y += 1;
        }

        let mut min_x = usize::MAX;
        let mut max_x = 0;
        for row in start..=last {
            for x in 0..gray.w {
                if mask[row * gray.w + x] {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        let height = last - start + 1;
        if min_x != usize::MAX && height >= MIN_LINE_H {
            lines.push((
                min_x as i32,
                start as i32,
                (max_x - min_x + 1) as i32,
                height as i32,
            ));
        }
    }

    lines
}

/// Blur a cell before comparing it. The game rasterises the same character a
/// pixel wider or narrower from one frame to the next, and against a sharp cell
/// that shows up as a large distance — which is what made a glyph need its own
/// learned variant per rendering. Softening both sides keeps such differences
/// proportionate while leaving genuinely different characters distinct.
fn soften(cell: &Cell) -> Cell {
    let mut out = [0u8; CELL_LEN];
    for y in 0..CELL_H {
        for x in 0..CELL_W {
            // Centre-weighted: a full box blur also merges narrow shapes — a `1`
            // starts reading as the `]` bracket, which is the same kind of bar.
            let mut total = cell[y * CELL_W + x] as u32 * 4;
            let mut count = 4u32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let sx = x as i32 + dx;
                    let sy = y as i32 + dy;
                    if sx < 0 || sy < 0 || sx >= CELL_W as i32 || sy >= CELL_H as i32 {
                        continue;
                    }
                    total += cell[sy as usize * CELL_W + sx as usize] as u32;
                    count += 1;
                }
            }
            out[y * CELL_W + x] = (total / count) as u8;
        }
    }
    out
}

fn cell_distance(a: &Cell, b: &Cell) -> f64 {
    let left = soften(a);
    let right = soften(b);
    let total: u64 = left
        .iter()
        .zip(&right)
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum();
    total as f64 / CELL_LEN as f64
}

/// Cell distance allowing the sample to slide by `search` cells in each
/// direction. The game rasterises the readout at sub-pixel positions, so the same
/// character comes back a fraction of a cell off from one frame to the next.
fn cell_distance_shifted(a: &Cell, b: &Cell, search: i32) -> f64 {
    let left = soften(a);
    let right = soften(b);
    let mut best = f64::MAX;

    for dy in -search..=search {
        for dx in -search..=search {
            let mut total: u64 = 0;
            for y in 0..CELL_H {
                for x in 0..CELL_W {
                    let sx = x as i32 - dx;
                    let sy = y as i32 - dy;
                    if sx < 0 || sy < 0 || sx >= CELL_W as i32 || sy >= CELL_H as i32 {
                        continue;
                    }
                    let a_pixel = left[y * CELL_W + x] as i32;
                    let b_pixel = right[sy as usize * CELL_W + sx as usize] as i32;
                    total += (a_pixel - b_pixel).unsigned_abs() as u64;
                }
            }
            let distance = total as f64 / CELL_LEN as f64;
            if distance < best {
                best = distance;
            }
        }
    }

    best
}

/// Cost per pixel of size difference when matching. It exists to keep shapes
/// that differ a lot in size apart — a period from a comma, a `1` from an `8` —
/// but a glyph's own outline flickers by a pixel from frame to frame (anti-
/// aliasing, and the Filter Applied rendering), so a one-pixel difference has to
/// stay cheap. At the old value a single pixel added 18 and pushed a correct
/// match past the threshold, which dropped the digit and failed the read.
const SIZE_PENALTY: f64 = 4.0;

#[derive(Clone, Debug, Default)]
pub struct Atlas {
    pub glyphs: Vec<Glyph>,
}

impl Atlas {
    pub fn from_settings(atlas: &crate::settings::Atlas) -> Self {
        let mut glyphs = Vec::new();
        for stored in &atlas.glyphs {
            let Some(ch) = stored.ch.chars().next() else {
                continue;
            };
            let Some(bytes) = crate::settings::from_hex(&stored.cell) else {
                continue;
            };
            if bytes.len() != CELL_LEN {
                continue;
            }
            let mut cell = [0u8; CELL_LEN];
            cell.copy_from_slice(&bytes);
            glyphs.push(Glyph {
                ch,
                cell,
                w: stored.w,
                h: stored.h,
            });
        }
        Self { glyphs }
    }

    pub fn to_settings(&self) -> crate::settings::Atlas {
        let glyphs = self
            .glyphs
            .iter()
            .map(|glyph| crate::settings::GlyphCell {
                ch: glyph.ch.to_string(),
                w: glyph.w,
                h: glyph.h,
                cell: crate::settings::to_hex(&glyph.cell),
            })
            .collect();
        crate::settings::Atlas {
            cell_w: CELL_W as u32,
            cell_h: CELL_H as u32,
            glyphs,
        }
    }

    pub fn learned(&self) -> String {
        let mut chars: Vec<char> = self.glyphs.iter().map(|glyph| glyph.ch).collect();
        chars.sort_unstable();
        chars.dedup();
        chars.into_iter().collect()
    }

    pub fn learn(&mut self, text: &str, gray: &Gray) -> Result<Vec<char>, String> {
        let mut found = segments(gray);
        let chars: Vec<char> = text.chars().filter(|ch| !ch.is_whitespace()).collect();

        if chars.is_empty() {
            return Err("type the text shown in the box".into());
        }

        // A box drawn around the whole readout also catches the "EXP" label. If
        // there are extra shapes, drop them from the left — the label sits there —
        // so typing just the number and percentage still lines up.
        if found.len() > chars.len() {
            let extra = found.len() - chars.len();
            found.drain(0..extra);
        }

        if found.len() != chars.len() {
            return Err(format!(
                "found {} shapes but {} characters — drag a tighter box or type exactly what is shown",
                found.len(),
                chars.len()
            ));
        }

        for (ch, segment) in chars.iter().zip(found) {
            let glyph = Glyph {
                ch: *ch,
                cell: segment.cell,
                w: segment.w,
                h: segment.h,
            };
            // Keep several shapes per character: the same digit can render a
            // pixel or two differently depending on where it sits (scaling and
            // hinting), and collapsing them loses whichever variant came second.
            // Size is part of the identity because matching penalises it — if a
            // shape is the same character at a different size it is a new variant,
            // not something already known.
            let known = self.glyphs.iter().any(|existing| {
                existing.ch == *ch
                    && existing.w == glyph.w
                    && existing.h == glyph.h
                    && cell_distance(&existing.cell, &glyph.cell) <= VARIANT_DISTANCE
            });
            if !known {
                self.glyphs.push(glyph);
            }
        }

        Ok(chars)
    }

    fn distance_to(&self, segment: &Segment, glyph: &Glyph) -> f64 {
        let size_penalty = SIZE_PENALTY
            * ((segment.w as f64 - glyph.w as f64).abs()
                + (segment.h as f64 - glyph.h as f64).abs());
        cell_distance_shifted(&segment.cell, &glyph.cell, SHIFT_SEARCH) + size_penalty
    }

    /// Nearest glyph for a segment: (character, best distance, runner-up of a
    /// *different* character). Another shape of the same character is a variant,
    /// not ambiguity.
    fn match_segment(&self, segment: &Segment) -> (char, f64, f64) {
        let distance_to = |glyph: &Glyph| self.distance_to(segment, glyph);

        let mut best = ('?', f64::MAX);
        for glyph in &self.glyphs {
            let distance = distance_to(glyph);
            if distance < best.1 {
                best = (glyph.ch, distance);
            }
        }

        let mut other = f64::MAX;
        for glyph in &self.glyphs {
            if glyph.ch == best.0 {
                continue;
            }
            let distance = distance_to(glyph);
            if distance < other {
                other = distance;
            }
        }

        (best.0, best.1, other)
    }

    /// Nearest glyph whose character is one of `allowed`, or `None` when even the
    /// closest of them is too far to trust.
    fn match_segment_within(&self, segment: &Segment, allowed: &str) -> Option<(char, f64)> {
        let mut best: Option<(char, f64)> = None;
        for glyph in &self.glyphs {
            if !allowed.contains(glyph.ch) {
                continue;
            }
            let distance = self.distance_to(segment, glyph);
            if best.map_or(true, |(_, closest)| distance < closest) {
                best = Some((glyph.ch, distance));
            }
        }

        best.filter(|(_, distance)| *distance <= MATCH_THRESHOLD)
    }

    /// Read an EXP-style readout — a number, then a bracketed percentage — using
    /// the shape of the readout to restrict what each position may contain. The
    /// closing bracket is only a candidate once the percentage has started, which
    /// is what stops a `1` (the same narrow bar) being read as one.
    pub fn recognize_readout(&self, gray: &Gray) -> Result<(String, f64), String> {
        const NUMBER: &str = "0123456789,[";
        const PERCENT: &str = "0123456789.%";
        const CLOSE: &str = "]";

        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }

        let mut text = String::new();
        let mut confidence = 0.0;
        let mut matched = 0usize;
        let mut stage = 0usize;

        for segment in segments(gray) {
            let allowed = match stage {
                0 => NUMBER,
                1 => PERCENT,
                _ => CLOSE,
            };

            let Some((ch, best)) = self.match_segment_within(&segment, allowed) else {
                continue;
            };
            if ch == '[' {
                stage = 1;
            } else if ch == '%' {
                stage = 2;
            }

            text.push(ch);
            confidence += (1.0 - best / 255.0).max(0.0);
            matched += 1;
        }

        if matched == 0 {
            return Err("nothing recognizable in the box — learn the digits".into());
        }

        Ok((text, confidence / matched as f64))
    }

    /// Add a shape the player identified by hand. Stored as-is, so it matches the
    /// exact pixels it was labelled from — including one-pixel shapes like `.`.
    pub fn learn_glyph(&mut self, ch: char, cell: Cell, w: u32, h: u32) {
        let known = self.glyphs.iter().any(|existing| {
            existing.ch == ch
                && existing.w == w
                && existing.h == h
                && cell_distance(&existing.cell, &cell) <= VARIANT_DISTANCE
        });
        if !known {
            self.glyphs.push(Glyph { ch, cell, w, h });
        }
    }

    /// Per-segment classification. `known` means the best match is close enough
    /// to trust without asking the player.
    pub fn explain(&self, gray: &Gray) -> Vec<Guess> {
        segments(gray)
            .into_iter()
            .map(|segment| {
                let (ch, best, _) = self.match_segment(&segment);
                Guess {
                    x: segment.x,
                    y: segment.y,
                    w: segment.w,
                    h: segment.h,
                    ch,
                    distance: best,
                    known: best <= MATCH_THRESHOLD,
                    cell: segment.cell,
                }
            })
            .collect()
    }

    pub fn recognize(&self, gray: &Gray) -> Result<(String, f64), String> {
        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }

        let found = segments(gray);
        if found.is_empty() {
            return Err("nothing readable in the box".into());
        }

        let mut text = String::with_capacity(found.len());
        let mut confidence = 0.0;
        let mut matched = 0usize;

        for segment in found {
            let (ch, best, _) = self.match_segment(&segment);

            // Two cases are deliberately not errors here:
            // - a shape that matches nothing is skipped (a box around the readout
            //   also catches the "EXP" label, which is not in the glyph set);
            // - a close runner-up is kept as the best match.
            // Either way the EXP cross-check and the verify-reads gate reject a
            // read where a digit was dropped or misread.
            if best > MATCH_THRESHOLD {
                continue;
            }

            text.push(ch);
            confidence += (1.0 - best / 255.0).max(0.0);
            matched += 1;
        }

        if matched == 0 {
            return Err("nothing recognizable in the box — learn the digits".into());
        }

        Ok((text, confidence / matched as f64))
    }
}

pub fn parse_exp_text(text: &str) -> Option<(i64, f64)> {
    let percent_at = text.find('%')?;
    let before = &text[..percent_at];
    let start = before
        .rfind(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .map(|index| index + 1)
        .unwrap_or(0);
    let percent: f64 = before[start..].trim().parse().ok()?;
    if !(0.0..=100.0).contains(&percent) {
        return None;
    }

    let mut digits = String::new();
    let mut started = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            started = true;
        } else if started && ch == ',' {
            continue;
        } else if started {
            break;
        }
    }

    Some((digits.parse().ok()?, percent))
}

pub fn parse_level(text: &str) -> Option<u32> {
    let digits: String = text.chars().filter(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Fingerprints are compared on a fixed grid so a one-pixel difference in the
/// detected label box cannot change the dimensions and veto the match.
pub const MAP_LABEL_W: usize = 128;
pub const MAP_LABEL_H: usize = 24;

const PANEL_RGB: (i32, i32, i32) = (153, 187, 204);
const PANEL_TOLERANCE: i32 = 6;
const PANEL_MIN_W: i32 = 80;
const PANEL_MAX_W: i32 = 900;
const PANEL_MIN_H: i32 = 20;
const PANEL_MAX_H: i32 = 300;
const PANEL_MIN_SOLIDITY: f64 = 30.0;
/// The title bar is the panel's top band and the minimap graphic sits on its
/// left; both scale with the panel, so the name crop is proportional rather than
/// a fixed number of pixels (which started inside the title bar and the graphic).
const TITLE_BAR_FRACTION: f64 = 0.34;
const GRAPHIC_GUTTER_FRACTION: f64 = 0.25;
const TEXT_MIN_RGB: u8 = 225;

pub fn resample(gray: &Gray, w: usize, h: usize) -> Gray {
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let sy = (((y as f64 + 0.5) * gray.h as f64 / h as f64) as usize)
            .min(gray.h.saturating_sub(1));
        for x in 0..w {
            let sx = (((x as f64 + 0.5) * gray.w as f64 / w as f64) as usize)
                .min(gray.w.saturating_sub(1));
            px.push(gray.px[sy * gray.w + sx]);
        }
    }
    Gray { w, h, px }
}

fn is_panel_pixel(frame: &Frame, x: usize, y: usize) -> bool {
    let index = (y * frame.width as usize + x) * 4;
    let r = frame.rgba[index] as i32;
    let g = frame.rgba[index + 1] as i32;
    let b = frame.rgba[index + 2] as i32;
    (r - PANEL_RGB.0).abs() <= PANEL_TOLERANCE
        && (g - PANEL_RGB.1).abs() <= PANEL_TOLERANCE
        && (b - PANEL_RGB.2).abs() <= PANEL_TOLERANCE
}

/// Locate the minimap's info panel by its solid fill colour. The panel moves and
/// resizes with the minimap, so its position is discovered rather than stored.
/// Connected components are used because the same colour also occurs in scenery;
/// only a solid rectangle of minimap-panel proportions is accepted.
pub fn detect_minimap_panel(frame: &Frame) -> Option<Region> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    if w == 0 || h == 0 {
        return None;
    }

    let mut mask = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            mask[y * w + x] = is_panel_pixel(frame, x, y);
        }
    }

    let mut visited = vec![false; w * h];
    let mut best: Option<(u32, Region)> = None;
    let mut stack: Vec<usize> = Vec::new();

    for start in 0..w * h {
        if !mask[start] || visited[start] {
            continue;
        }

        stack.clear();
        stack.push(start);
        visited[start] = true;

        let mut min_x = usize::MAX;
        let mut max_x = 0usize;
        let mut min_y = usize::MAX;
        let mut max_y = 0usize;
        let mut pixels = 0usize;

        while let Some(index) = stack.pop() {
            let x = index % w;
            let y = index / w;
            pixels += 1;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);

            let neighbours = [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ];
            for (nx, ny) in neighbours {
                if nx >= w || ny >= h {
                    continue;
                }
                let next = ny * w + nx;
                if mask[next] && !visited[next] {
                    visited[next] = true;
                    stack.push(next);
                }
            }
        }

        if min_x == usize::MAX {
            continue;
        }

        let region = Region {
            x: min_x as i32,
            y: min_y as i32,
            w: (max_x - min_x + 1) as i32,
            h: (max_y - min_y + 1) as i32,
        };

        // Only a minimap-shaped solid block qualifies.
        if region.w < PANEL_MIN_W
            || region.w > PANEL_MAX_W
            || region.h < PANEL_MIN_H
            || region.h > PANEL_MAX_H
        {
            continue;
        }
        let area = region.w as usize * region.h as usize;
        let solidity = pixels as f64 * 100.0 / area as f64;
        if solidity < PANEL_MIN_SOLIDITY {
            continue;
        }

        if best.map_or(true, |(best_area, _)| area as u32 > best_area) {
            best = Some((area as u32, region));
        }
    }

    best.map(|(_, region)| region)
}

/// Bounding box of the white map-name text inside the panel, excluding the icon.
pub fn detect_map_label(frame: &Frame, panel: Region) -> Option<Region> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let x_start = ((panel.x as f64 + panel.w as f64 * GRAPHIC_GUTTER_FRACTION) as i32).max(0)
        as usize;
    let x_end = ((panel.x + panel.w) as usize).min(w);
    // Skip the panel's title bar; the map name is drawn below it.
    let y_start = ((panel.y as f64 + panel.h as f64 * TITLE_BAR_FRACTION) as i32).max(0) as usize;
    let y_end = ((panel.y + panel.h) as usize).min(h);
    if x_end <= x_start || y_end <= y_start {
        return None;
    }

    let mut min_x = usize::MAX;
    let mut max_x = 0usize;
    let mut min_y = usize::MAX;
    let mut max_y = 0usize;

    for y in y_start..y_end {
        for x in x_start..x_end {
            let index = (y * w + x) * 4;
            if frame.rgba[index] >= TEXT_MIN_RGB
                && frame.rgba[index + 1] >= TEXT_MIN_RGB
                && frame.rgba[index + 2] >= TEXT_MIN_RGB
            {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }

    if min_x == usize::MAX || max_x <= min_x || max_y <= min_y {
        return None;
    }

    Some(Region {
        x: min_x as i32 - 1,
        y: min_y as i32 - 1,
        w: (max_x - min_x + 3) as i32,
        h: (max_y - min_y + 3) as i32,
    })
}

/// The current map-label crop, normalised to the fixed fingerprint grid.
pub fn read_map_label(frame: &Frame) -> Option<Gray> {
    let panel = detect_minimap_panel(frame)?;
    let label = detect_map_label(frame, panel)?;
    let gray = crop_gray(frame, label)?;
    if gray.w < 8 || gray.h < 6 {
        return None;
    }
    Some(resample(&gray, MAP_LABEL_W, MAP_LABEL_H))
}

/// The minimap label's text lines, cropped from the frame, top to bottom. The
/// label is the street line followed by the map name.
pub fn map_label_lines(frame: &Frame) -> Option<Vec<Gray>> {
    let panel = detect_minimap_panel(frame)?;
    let label = detect_map_label(frame, panel)?;
    let gray = crop_gray(frame, label)?;
    if gray.w < 8 || gray.h < 6 {
        return None;
    }

    let mut out = Vec::new();
    let bands = text_lines(&gray);
    // The label's lines are all the same font, so the first band's height is the
    // height of every later line. Without that cap a band runs on into the
    // minimap graphic below the name and the crop swallows it.
    let line_h = bands.first().map(|(_, _, _, h)| *h).unwrap_or(0);
    for (x, y, w, h) in bands {
        let region = Region {
            x: label.x + x,
            y: label.y + y,
            w,
            h: if line_h > 0 { h.min(line_h) } else { h },
        };
        if let Some(line) = crop_gray(frame, region) {
            out.push(line);
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// The minimap label read as text, one string per line. `None` when the label
/// cannot be found, no label glyphs have been learned yet, or too many shapes
/// went unrecognised — a partly-read label would name the wrong map, so it is
/// better to report nothing and fall back to a taught fingerprint.
pub fn read_map_text(frame: &Frame, atlas: &Atlas) -> Option<Vec<String>> {
    if atlas.glyphs.is_empty() {
        return None;
    }

    let lines = map_label_lines(frame)?;

    let mut unrecognised = 0usize;
    let mut text = Vec::with_capacity(lines.len());
    for line in &lines {
        // One stray shape per line is tolerated: the crop can catch a minimap
        // marker or an edge of the panel next to the text.
        unrecognised += atlas
            .explain(line)
            .iter()
            .filter(|guess| !guess.known)
            .count();
        text.push(
            atlas
                .recognize(line)
                .map(|(text, _)| text)
                .unwrap_or_default(),
        );
    }

    if unrecognised > lines.len() || text.iter().all(|line| line.is_empty()) {
        return None;
    }

    Some(text)
}

/// Drop a trailing channel marker: `Free Market <15>` and `Free Market <16>` are
/// the same map on different channels.
fn without_channel(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    match (lower.find('<'), lower.rfind('>')) {
        (Some(open), Some(close)) if close > open => {
            format!("{}{}", &text[..open], &text[close + 1..]).trim().to_string()
        }
        _ => text.trim().to_string(),
    }
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(left != right);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// How close two labels are, 0..=1, ignoring case and the channel marker. Used to
/// snap a mostly-correct read onto a name that was taught explicitly.
pub fn label_similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = without_channel(a)
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    let b: Vec<char> = without_channel(b)
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let longest = a.len().max(b.len()) as f64;
    1.0 - edit_distance(&a, &b) as f64 / longest
}

pub fn map_distance(crop: &Gray, template: &Gray) -> Option<f64> {
    if crop.w != template.w || crop.h != template.h || crop.px.len() != template.px.len() {
        return None;
    }

    let total: u64 = crop
        .px
        .iter()
        .zip(&template.px)
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum();
    Some(total as f64 / crop.px.len() as f64)
}

// Best mean distance of the template anywhere within `search` pixels of the
// crop's origin. Unused by the engine (map labels are located by detection),
// but kept for diagnostics on fingerprints taught against a fixed box.
#[allow(dead_code)]
pub fn map_distance_shifted(crop: &Gray, template: &Gray, search: usize) -> Option<f64> {
    if crop.w != template.w || crop.h != template.h || crop.px.len() != template.px.len() {
        return None;
    }
    if search == 0 {
        return map_distance(crop, template);
    }

    let mut best = f64::MAX;
    for dy in 0..=search * 2 {
        for dx in 0..=search * 2 {
            let ox = dx as isize - search as isize;
            let oy = dy as isize - search as isize;

            let mut total: u64 = 0;
            let mut count: usize = 0;
            for y in 0..crop.h {
                let ty = y as isize + oy;
                if ty < 0 || ty >= crop.h as isize {
                    continue;
                }
                for x in 0..crop.w {
                    let tx = x as isize + ox;
                    if tx < 0 || tx >= crop.w as isize {
                        continue;
                    }
                    let a = crop.px[y * crop.w + x] as i32;
                    let b = template.px[ty as usize * crop.w + tx as usize] as i32;
                    total += (a - b).unsigned_abs() as u64;
                    count += 1;
                }
            }

            if count == 0 {
                continue;
            }
            // Require a mostly-overlapping alignment so a shift cannot win by
            // comparing only a handful of easy pixels.
            let coverage = count as f64 / crop.px.len() as f64;
            if coverage < 0.8 {
                continue;
            }

            let distance = (total as f64 / count as f64) / coverage;
            if distance < best {
                best = distance;
            }
        }
    }

    if best == f64::MAX {
        None
    } else {
        Some(best)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: usize, h: usize) -> Gray {
        Gray {
            w,
            h,
            px: vec![20; w * h],
        }
    }

    fn draw_rect(gray: &mut Gray, x0: usize, y0: usize, x1: usize, y1: usize, value: u8) {
        for y in y0..y1 {
            for x in x0..x1 {
                gray.px[y * gray.w + x] = value;
            }
        }
    }

    fn sample_pair() -> (Gray, Gray) {
        let mut a = canvas(30, 20);
        // "0": hollow rectangle
        draw_rect(&mut a, 2, 3, 8, 17, 230);
        draw_rect(&mut a, 4, 5, 6, 15, 20);
        // "1": solid bar
        draw_rect(&mut a, 14, 3, 18, 17, 230);

        let mut b = canvas(30, 20);
        draw_rect(&mut b, 2, 3, 8, 17, 230);
        draw_rect(&mut b, 4, 5, 6, 15, 20);
        draw_rect(&mut b, 14, 3, 18, 17, 230);

        (a, b)
    }

    #[test]
    fn learns_and_reads_two_glyphs() {
        let (a, b) = sample_pair();
        let mut atlas = Atlas::default();
        let learned = atlas.learn("01", &a).unwrap();
        assert_eq!(learned, vec!['0', '1']);

        let (text, confidence) = atlas.recognize(&b).unwrap();
        assert_eq!(text, "01");
        assert!(confidence > 0.9);
    }

    #[test]
    fn rejects_unknown_glyph() {
        let (payload, _) = sample_pair();
        let mut atlas = Atlas::default();
        atlas.learn("01", &payload).unwrap();

        let mut unknown = canvas(30, 20);
        for step in 0..10 {
            draw_rect(&mut unknown, 2 + step, 3 + step, 5 + step, 6 + step, 230);
        }

        assert!(atlas.recognize(&unknown).is_err());
    }

    #[test]
    fn rejects_mismatched_character_count() {
        let (payload, _) = sample_pair();
        let mut atlas = Atlas::default();
        assert!(atlas.learn("012", &payload).is_err());
    }

    #[test]
    fn same_character_at_a_new_size_is_a_new_variant() {
        // A dot already known at 1x1, then the same character drawn at 2x2: it
        // must be stored, otherwise matching keeps rejecting it on size.
        let mut atlas = Atlas {
            glyphs: vec![Glyph {
                ch: '.',
                cell: [255; CELL_LEN],
                w: 1,
                h: 1,
            }],
        };
        let dot = Gray {
            w: 2,
            h: 2,
            px: vec![255, 255, 255, 255],
        };

        atlas.learn(".", &dot).expect("learn");
        assert_eq!(atlas.glyphs.len(), 2, "a new size is a new variant");
        assert_eq!(
            atlas.recognize(&dot).map(|(text, _)| text).as_deref(),
            Ok(".")
        );
    }

    #[test]
    fn labels_snap_across_channels() {
        assert_eq!(label_similarity("Free Market <15>", "Free Market <16>"), 1.0);

        let near = label_similarity(
            "Hidden Street\nFree Markei <15>",
            "Hidden Street\nFree Market <15>",
        );
        assert!(near > 0.95, "one wrong letter should still snap: {near}");

        assert!(label_similarity("Kerning City", "Victoria Road\nKerning City") < 0.6);
        assert_eq!(label_similarity("", "x"), 0.0);
    }

    #[test]
    fn parses_the_exp_readout() {
        assert_eq!(parse_exp_text("10,105 [91.53%]"), Some((10_105, 91.53)));
        assert_eq!(parse_exp_text("0 [0.00%]"), Some((0, 0.0)));
        assert_eq!(parse_exp_text("1,234,567[100%]"), Some((1_234_567, 100.0)));
        assert_eq!(parse_exp_text("no numbers"), None);
    }

    #[test]
    fn parses_the_level() {
        assert_eq!(parse_level("71"), Some(71));
        assert_eq!(parse_level("Lv 99"), Some(99));
        assert_eq!(parse_level("--"), None);
    }

    #[test]
    #[ignore = "diagnostic: prints segments from the live game frame"]
    fn dumps_live_segments() {
        let selector = crate::settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = crate::window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("capture");

        for (name, region) in [
            ("exp", Region { x: 747, y: 737, w: 83, h: 10 }),
            ("level", Region { x: 330, y: 746, w: 21, h: 9 }),
        ] {
            let gray = crop_gray(&frame, region).expect("crop");
            let cells = segments(&gray);
            println!("== {name}: {} segments ==", cells.len());
            for (index, segment) in cells.iter().enumerate() {
                println!("-- {index}");
                for y in 0..CELL_H {
                    let row: String = (0..CELL_W)
                        .map(|x| {
                            if segment.cell[y * CELL_W + x] > 0 {
                                '#'
                            } else {
                                '.'
                            }
                        })
                        .collect();
                    println!("{row}");
                }
            }
        }
    }
}
