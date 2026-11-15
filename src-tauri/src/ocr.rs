use crate::capture::Frame;
use crate::settings::Region;

pub const CELL_W: usize = 12;
pub const CELL_H: usize = 16;
pub const CELL_LEN: usize = CELL_W * CELL_H;

const MATCH_THRESHOLD: f64 = 30.0;
const MIN_GLYPH_PIXELS: usize = 1;
/// How far a sample may slide within its cell when matching (sub-pixel drift).
const SHIFT_SEARCH: i32 = 1;

pub type Cell = [u8; CELL_LEN];

#[derive(Clone, Debug)]
pub struct Segment {
    /// Native bitmap of the shape, row-major, `w*h` bytes of 0/255.
    pub mask: Vec<u8>,
    /// The same shape resampled to a fixed cell. Kept as the fallback for
    /// anti-aliased or scaled renderings whose native size does not match a
    /// learned glyph.
    pub cell: Cell,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// One segmented shape and what the atlas thinks it is.
#[derive(Clone, Debug)]
pub struct Guess {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub ch: char,
    pub distance: f64,
    pub known: bool,
    pub bitmap: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Glyph {
    pub ch: char,
    pub bitmap: Vec<u8>,
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

/// The fixed floor `bright_segments` uses: near-white UI text.
pub const TEXT_FLOOR_PUB: u8 = TEXT_FLOOR;

/// Reduce a shape to its own native ink and a CELL_W x CELL_H cell.
///
/// The native bitmap is the primary representation: the client draws these
/// glyphs as fixed 1-bit bitmaps, so an exact native match is possible. The
/// 12x16 cell is kept only as a size-independent fallback for anti-aliased or
/// scaled renderings (the map label).
///
/// The per-shape background fallback below still applies: the level readout
/// sits on pale tiles, so a crop-wide threshold calls the whole tile ink and
/// the digit would become a solid block. Measuring against the shape's own
/// floor recovers the strokes where the crop-wide threshold has failed.
fn normalize(
    gray: &Gray,
    mask: &[bool],
    stride: usize,
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
) -> (Cell, Vec<u8>) {
    let mut cell = [0u8; CELL_LEN];
    let mut native = vec![0u8; w * h];
    if w == 0 || h == 0 {
        return (cell, native);
    }
    let x_end = (x0 + w).min(gray.w);
    let y_end = (y0 + h).min(gray.h);
    if x0 >= x_end || y0 >= y_end {
        return (cell, native);
    }

    // How much of the shape the crop-wide threshold already calls ink. When nearly
    // all of it is, that threshold has failed rather than found something: it is a
    // character drawn on a pale tile, where the tile is "foreground" too and the
    // whole shape would collapse to a solid block that matches every other tile.
    //
    // Only that case falls back to a threshold against the shape's own background.
    // Applying the fallback everywhere was worse: a low-contrast shape has a tiny
    // span, most pixels clear it, and the cell saturates into a blob — so one
    // learned glyph matched the rest.
    let mut ink = 0usize;
    let mut total = 0usize;
    for y in y0..y_end {
        for x in x0..x_end {
            total += 1;
            if mask[y * stride + x] {
                ink += 1;
            }
        }
    }
    let saturated = total > 0 && ink * 100 >= total * 85;

    let mut floor = 0usize;
    let mut span = 1usize;
    if saturated {
        let mut histogram = [0u32; 256];
        for y in y0..y_end {
            for x in x0..x_end {
                histogram[gray.px[y * gray.w + x] as usize] += 1;
            }
        }
        for (value, count) in histogram.iter().enumerate() {
            if *count > histogram[floor] {
                floor = value;
            }
        }
        let peak = histogram
            .iter()
            .rposition(|count| *count > 0)
            .unwrap_or(floor);
        span = peak.saturating_sub(floor).max(1);
    }

    let on = |x: usize, y: usize| -> bool {
        if saturated {
            let lifted = (gray.px[y * gray.w + x] as usize).saturating_sub(floor);
            lifted * 2 >= span
        } else {
            mask[y * stride + x]
        }
    };

    for gy in 0..h {
        let y = y0 + gy;
        if y >= y_end {
            break;
        }
        for gx in 0..w {
            let x = x0 + gx;
            if x < x_end && on(x, y) {
                native[gy * w + gx] = 255;
            }
        }
    }

    for gy in 0..CELL_H {
        for gx in 0..CELL_W {
            let sx = (x0 + ((gx as f64 + 0.5) * w as f64 / CELL_W as f64) as usize)
                .min(x_end - 1);
            let sy = (y0 + ((gy as f64 + 0.5) * h as f64 / CELL_H as f64) as usize)
                .min(y_end - 1);
            cell[gy * CELL_W + gx] = if on(sx, sy) { 255 } else { 0 };
        }
    }
    (cell, native)
}

/// A native bitmap resampled to the fixed cell used by the soft fallback.
fn cell_of_native(bitmap: &[u8], w: usize, h: usize) -> Cell {
    let mut cell = [0u8; CELL_LEN];
    if w == 0 || h == 0 || bitmap.len() != w * h {
        return cell;
    }
    for gy in 0..CELL_H {
        for gx in 0..CELL_W {
            let sy = (((gy as f64 + 0.5) * h as f64 / CELL_H as f64) as usize).min(h - 1);
            let sx = (((gx as f64 + 0.5) * w as f64 / CELL_W as f64) as usize).min(w - 1);
            cell[gy * CELL_W + gx] = if bitmap[sy * w + sx] > 0 { 255 } else { 0 };
        }
    }
    cell
}

/// Recover a native bitmap from a legacy 12x16 cell. Best effort: the cell was
/// a resample of the original shape, so this cannot restore pixels the encoder
/// discarded. It exists so profiles written before the native format keep
/// reading until they are re-seeded or re-taught.
fn migrate_legacy_cell(cell: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    if w == 0 || h == 0 || cell.len() != CELL_LEN {
        return out;
    }
    for gy in 0..h {
        let sy = (((gy as f64 + 0.5) * CELL_H as f64 / h as f64) as usize).min(CELL_H - 1);
        for gx in 0..w {
            let sx = (((gx as f64 + 0.5) * CELL_W as f64 / w as f64) as usize).min(CELL_W - 1);
            out[gy * w + gx] = cell[sy * CELL_W + sx];
        }
    }
    out
}

/// How many foreground pixels sit in each column of `gray`. Splitting on the empty
/// columns is how `segments` finds glyphs, so this shows whether a rendering keeps
/// them apart at all. Diagnostic only.
#[cfg(test)]
pub fn column_ink(gray: &Gray) -> Vec<u32> {
    ink_of(&foreground(gray), gray.w, gray.h)
}

/// The same, with the fixed threshold the blob fallback uses. Diagnostic only.
#[cfg(test)]
pub fn column_ink_floor(gray: &Gray, floor: u8) -> Vec<u32> {
    ink_of(&foreground_with_floor(gray, floor), gray.w, gray.h)
}

/// The thresholded crop as text, one line per row, for eyeballing a rendering the
/// reader cannot segment. Diagnostic only.
#[cfg(test)]
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

#[cfg(test)]
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
    let found = split_shapes(gray, None, false);
    if found.len() > 1 {
        return found;
    }

    // A rendering whose background reaches the same brightness as the text makes
    // OTSU split the gradient instead of the text, and the whole line comes back as
    // a single shape — which is what the Ideal Ratio HUD does. These readouts are
    // near-white, so a high fixed threshold separates them where the adaptive one
    // cannot. Only used as a fallback, so renderings that already work are untouched.
    let bright = split_shapes(gray, Some(TEXT_FLOOR), false);
    if bright.len() > found.len() {
        bright
    } else {
        found
    }
}

/// Shapes of a single line of ordinary text.
///
/// The readout segmentation drops a shape as tall as the crop, because a generous
/// ROI can catch a panel rule. A text box drawn tightly around a name is exactly
/// the glyph height, so that rule would drop every capital and ascender (`E`,
/// `b`, `l`, `d`). Text reads keep them.
pub fn text_segments(gray: &Gray) -> Vec<Segment> {
    let found = split_shapes(gray, None, true);
    if found.len() > 1 {
        return found;
    }
    let bright = split_shapes(gray, Some(TEXT_FLOOR), true);
    if bright.len() > found.len() {
        bright
    } else {
        found
    }
}

fn split_shapes(gray: &Gray, floor: Option<u8>, keep_full_height: bool) -> Vec<Segment> {
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

        // A shape as tall as the crop is a rule or a panel edge that the box caught,
        // not a glyph: the box is sized to the text with margin above and below. Left
        // in, such a shape shifts every label during a teach. Only applied to a crop
        // tall enough to have margins at all, and never to a tight text line where a
        // capital or ascender legitimately spans the full height.
        if !keep_full_height && gray.h >= 4 && bottom - top >= gray.h {
            continue;
        }

        let (cell, native) =
            normalize(gray, &mask, gray.w, start, top, end - start, bottom - top);
        out.push(Segment {
            mask: native,
            cell,
            x: start as i32,
            y: top as i32,
            w: (end - start) as u32,
            h: (bottom - top) as u32,
        });
    }

    out
}

/// Shapes found with a fixed bright threshold, ignoring the adaptive one.
///
/// The level readout sits on pale tiles: Otsu calls the tile frame ink, so a
/// level digit segments as an 11x13 tile instead of the 7x7 glyph inside it.
/// The text itself is near-white, so a high fixed floor isolates it.
pub fn bright_segments(gray: &Gray, floor: u8) -> Vec<Segment> {
    split_shapes(gray, Some(floor), false)
}

/// The most common glyph height in a readout, ignoring punctuation-sized shapes.
/// This is the rendered font size (7 at Ideal, 11 at Default) and is what a
/// seeded atlas is scaled to.
pub fn dominant_glyph_height(gray: &Gray) -> Option<u32> {
    use std::collections::HashMap;

    let mut counts: HashMap<u32, usize> = HashMap::new();
    for segment in segments(gray) {
        if segment.w >= 3 && segment.h >= 4 {
            *counts.entry(segment.h).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(height, _)| height)
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
                let sy = y as i32 - dy;
                for x in 0..CELL_W {
                    let sx = x as i32 - dx;
                    let a_pixel = left[y * CELL_W + x] as i32;
                    // Out-of-bounds counts as background. Skipping those pixels
                    // instead let a narrow shape "match" by shifting its ink out
                    // of the box and comparing only the easy columns.
                    let b_pixel = if sx >= 0
                        && sy >= 0
                        && sx < CELL_W as i32
                        && sy < CELL_H as i32
                    {
                        right[sy as usize * CELL_W + sx as usize] as i32
                    } else {
                        0
                    };
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

/// Distance between two native bitmaps of the same size, in the same 0..255
/// scale as the cell distance (so `MATCH_THRESHOLD` keeps its meaning). The
/// sample may slide by `search` pixels, like the cell path.
pub(crate) fn native_distance(a: &[u8], b: &[u8], w: usize, h: usize, search: i32) -> f64 {
    if w == 0 || h == 0 || a.len() != w * h || b.len() != w * h {
        return f64::MAX;
    }
    let mut best = f64::MAX;
    for dy in -search..=search {
        for dx in -search..=search {
            let mut total: u64 = 0;
            for y in 0..h {
                let sy = y as i32 - dy;
                for x in 0..w {
                    let sx = x as i32 - dx;
                    let a_pixel = a[y * w + x] as i32;
                    // Out-of-bounds counts as background, so a shift cannot win
                    // by dropping the ink that moved out of the box.
                    let b_pixel = if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                        b[sy as usize * w + sx as usize] as i32
                    } else {
                        0
                    };
                    total += (a_pixel - b_pixel).unsigned_abs() as u64;
                }
            }
            let distance = total as f64 / (w * h) as f64;
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
            let w = stored.w as usize;
            let h = stored.h as usize;
            if w == 0 || h == 0 {
                continue;
            }
            let bitmap = match crate::settings::from_hex(&stored.bitmap) {
                Some(bytes) if bytes.len() == w * h => bytes,
                // Legacy 12x16 cells are migrated best-effort. The encoder is a
                // data format; this keeps old profiles readable until re-seeded.
                _ => match crate::settings::from_hex(&stored.cell) {
                    Some(bytes) if bytes.len() == CELL_LEN => migrate_legacy_cell(&bytes, w, h),
                    _ => continue,
                },
            };
            let cell = cell_of_native(&bitmap, w, h);
            glyphs.push(Glyph {
                ch,
                bitmap,
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
                bitmap: crate::settings::to_hex(&glyph.bitmap),
                cell: String::new(),
            })
            .collect();
        crate::settings::Atlas {
            format: 2,
            source: "taught".into(),
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
        self.learn_with(text, gray, false)
    }

    /// Learn a single line of ordinary text. Keeps full-height letters, which the
    /// readout path would discard in a tightly drawn name or label box.
    pub fn learn_text(&mut self, text: &str, gray: &Gray) -> Result<Vec<char>, String> {
        self.learn_with(text, gray, true)
    }

    fn learn_with(
        &mut self,
        text: &str,
        gray: &Gray,
        keep_full_height: bool,
    ) -> Result<Vec<char>, String> {
        let mut found = if keep_full_height {
            text_segments(gray)
        } else {
            segments(gray)
        };
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
                bitmap: segment.mask,
                cell: segment.cell,
                w: segment.w,
                h: segment.h,
            };
            // Keep several shapes per character: the same digit can render a
            // pixel or two differently depending on where it sits (scaling and
            // hinting), and collapsing them loses whichever variant came second.
            // Size is part of the identity: a glyph at a new size is a new
            // variant, and matching tries the native size bucket first.
            let known = self.glyphs.iter().any(|existing| {
                existing.ch == *ch
                    && existing.w == glyph.w
                    && existing.h == glyph.h
                    && existing.bitmap == glyph.bitmap
            });
            if !known {
                self.glyphs.push(glyph);
            }
        }

        Ok(chars)
    }

    fn distance_to(&self, segment: &Segment, glyph: &Glyph) -> f64 {
        // The client draws these glyphs from fixed bitmaps, so a same-size
        // comparison can be exact. That is the primary path and it is what makes
        // punctuation (a 2x2 comma, a 1x1 dot) separable again.
        if segment.w == glyph.w && segment.h == glyph.h {
            return native_distance(
                &segment.mask,
                &glyph.bitmap,
                segment.w as usize,
                segment.h as usize,
                SHIFT_SEARCH,
            );
        }

        // Different native size: the shape is a scaled/anti-aliased rendering
        // (the map label) or a size the atlas has not learned. Fall back to the
        // fixed cell, where size can be compared at all.
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
    fn readout_matches(&self, found: Vec<Segment>) -> Vec<(Segment, char, f64)> {
        const NUMBER: &str = "0123456789,[";
        const PERCENT: &str = "0123456789.%";
        const CLOSE: &str = "]";

        let mut out = Vec::new();
        let mut stage = 0usize;
        for segment in found {
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
            out.push((segment, ch, best));
        }
        out
    }

    pub fn recognize_readout(&self, gray: &Gray) -> Result<(String, f64), String> {
        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }

        let matches = self.readout_matches(segments(gray));
        if matches.is_empty() {
            return Err("nothing recognizable in the box — learn the digits".into());
        }

        let mut text = String::new();
        let mut confidence = 0.0;
        for (_, ch, best) in &matches {
            text.push(*ch);
            confidence += (1.0 - best / 255.0).max(0.0);
        }

        Ok((text, confidence / matches.len() as f64))
    }

    /// Read a single line of ordinary text (a name, job, street or map label).
    ///
    /// Unlike the readouts, these lines have spaces. `segments` splits on empty
    /// columns and drops them, so a gap at least `space_gap` pixels wide is read
    /// back as a space. Unmatched shapes are skipped: a label crop can catch a
    /// minimap marker or a panel edge next to the text.
    pub fn recognize_text(&self, gray: &Gray, space_gap: i32) -> Result<(String, f64), String> {
        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }

        let mut found = text_segments(gray);
        if found.is_empty() {
            found = split_shapes(gray, Some(TEXT_FLOOR), true);
        }
        if found.is_empty() {
            return Err("nothing readable in the box".into());
        }

        let mut text = String::new();
        let mut confidence = 0.0;
        let mut matched = 0usize;
        let mut last_end: Option<i32> = None;

        for segment in found {
            let (ch, best, _) = self.match_segment(&segment);
            if best > MATCH_THRESHOLD {
                continue;
            }
            if let Some(end) = last_end {
                if segment.x - end >= space_gap {
                    text.push(' ');
                }
            }
            text.push(ch);
            last_end = Some(segment.x + segment.w as i32);
            confidence += (1.0 - best / 255.0).max(0.0);
            matched += 1;
        }

        if matched == 0 {
            return Err("nothing recognizable in the box".into());
        }

        Ok((text, confidence / matched as f64))
    }

    /// Add the exact shapes of a live readout the seeded atlas already reads, so a
    /// rendering difference (at Ideal the `7` is one stroke off the asset) becomes
    /// an exact variant instead of a near miss. Only confident, non-exact matches
    /// are learned; a doubtful shape is left to the fallback rather than taught as
    /// a wrong character. Returns how many variants were added.
    pub fn learn_rendering(&mut self, gray: &Gray) -> usize {
        let confident = MATCH_THRESHOLD * 0.75;
        let mut added = 0;
        for (segment, ch, distance) in self.readout_matches(segments(gray)) {
            if distance == 0.0 || distance > confident {
                continue;
            }
            let before = self.glyphs.len();
            self.learn_glyph(ch, segment.mask, segment.w, segment.h);
            if self.glyphs.len() > before {
                added += 1;
            }
        }
        added
    }

    /// Add a shape the player identified by hand. Stored as the exact native
    /// bitmap it was labelled from — including one-pixel shapes like `.`.
    pub fn learn_glyph(&mut self, ch: char, bitmap: Vec<u8>, w: u32, h: u32) {
        if w == 0 || h == 0 || bitmap.len() != w as usize * h as usize {
            return;
        }
        let cell = cell_of_native(&bitmap, w as usize, h as usize);
        let known = self.glyphs.iter().any(|existing| {
            existing.ch == ch && existing.w == w && existing.h == h && existing.bitmap == bitmap
        });
        if !known {
            self.glyphs.push(Glyph {
                ch,
                bitmap,
                cell,
                w,
                h,
            });
        }
    }

    /// Per-segment classification. `known` means the best match is close enough
    /// to trust without asking the player.
    pub fn explain(&self, gray: &Gray) -> Vec<Guess> {
        self.explain_shapes(segments(gray))
    }

    /// Per-shape classification for a tightly drawn text line, keeping capitals
    /// and ascenders that span the crop height.
    pub fn explain_text(&self, gray: &Gray) -> Vec<Guess> {
        self.explain_shapes(text_segments(gray))
    }

    /// Per-shape classification for the level. Use the same bright-first
    /// segmentation as `recognize_level`; the adaptive path sees the pale tile and
    /// would flag digits the reader actually read as unrecognised.
    pub fn explain_level(&self, gray: &Gray) -> Vec<Guess> {
        let bright = bright_segments(gray, TEXT_FLOOR);
        if !bright.is_empty() {
            return self.explain_shapes(bright);
        }
        self.explain(gray)
    }

    fn explain_shapes(&self, found: Vec<Segment>) -> Vec<Guess> {
        found
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
                    bitmap: segment.mask,
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

        match self.read_shapes(found) {
            Ok((text, confidence)) if !text.is_empty() => Ok((text, confidence)),
            _ => {
                let bright = bright_segments(gray, TEXT_FLOOR);
                if bright.is_empty() {
                    return Err("nothing recognizable in the box — learn the digits".into());
                }
                self.read_shapes(bright)
            }
        }
    }

    /// Read a readout that sits on pale tiles (the level).
    ///
    /// Bright ink is tried first, not as a fallback. On a pale tile the adaptive
    /// threshold marks the whole tile as ink; the tile's interior can then match
    /// a learned glyph closely enough that the adaptive read looks successful
    /// while returning the wrong digits (a real `35` read as `55`). Only the
    /// near-white text is a trustworthy shape there.
    pub fn recognize_level(&self, gray: &Gray) -> Result<(String, f64), String> {
        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }

        let bright = bright_segments(gray, TEXT_FLOOR);
        if !bright.is_empty() {
            if let Ok((text, confidence)) = self.read_shapes(bright) {
                if !text.is_empty() {
                    return Ok((text, confidence));
                }
            }
        }

        self.recognize(gray)
    }

    fn read_shapes(&self, found: Vec<Segment>) -> Result<(String, f64), String> {
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
/// The minimap graphic is drawn with a near-black frame and dark map detail, while
/// the name text is white (its drop shadow measured no darker than 82 here). Anything
/// this dark in the name band is therefore the graphic, not the name.
const LABEL_MAX_DARK: u8 = 40;

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
///
/// The name is white; the minimap graphic beside it is the only thing in the band
/// below the title bar that also carries dark (near-black) ink — its frame and map
/// detail. So the name begins at the first white column that holds no dark pixel. A
/// fixed left gutter (a quarter of the panel) instead assumed the graphic's width and
/// clipped the name's first letters whenever the graphic was narrower than that.
pub fn detect_map_label(frame: &Frame, panel: Region) -> Option<Region> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let x_from = (panel.x.max(0) as usize).min(w);
    let x_end = ((panel.x + panel.w) as usize).min(w);
    // Skip the panel's title bar; the map name is drawn below it.
    let y_start = ((panel.y as f64 + panel.h as f64 * TITLE_BAR_FRACTION) as i32).max(0) as usize;
    let y_end = ((panel.y + panel.h) as usize).min(h);
    if x_end <= x_from || y_end <= y_start {
        return None;
    }

    let is_white = |index: usize| {
        frame.rgba[index] >= TEXT_MIN_RGB
            && frame.rgba[index + 1] >= TEXT_MIN_RGB
            && frame.rgba[index + 2] >= TEXT_MIN_RGB
    };
    let is_dark = |index: usize| {
        frame.rgba[index] < LABEL_MAX_DARK
            && frame.rgba[index + 1] < LABEL_MAX_DARK
            && frame.rgba[index + 2] < LABEL_MAX_DARK
    };

    let mut column_white = vec![false; x_end - x_from];
    let mut column_dark = vec![false; x_end - x_from];
    for x in x_from..x_end {
        for y in y_start..y_end {
            let index = (y * w + x) * 4;
            if is_white(index) {
                column_white[x - x_from] = true;
            }
            if is_dark(index) {
                column_dark[x - x_from] = true;
            }
        }
    }

    let start = (0..column_white.len())
        .find(|&i| column_white[i] && !column_dark[i])
        .map(|i| x_from + i)
        // Nothing matched — no separate graphic, or one without dark ink. Keep the old
        // proportional gutter so a panel that read before still reads.
        .unwrap_or_else(|| {
            ((panel.x as f64 + panel.w as f64 * GRAPHIC_GUTTER_FRACTION) as i32).max(0) as usize
        })
        .min(x_end);

    let mut min_x = usize::MAX;
    let mut max_x = 0usize;
    let mut min_y = usize::MAX;
    let mut max_y = 0usize;
    for y in y_start..y_end {
        for x in start..x_end {
            let index = (y * w + x) * 4;
            if is_white(index) {
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

/// The map-label crop on the fixed fingerprint grid.
///
/// Detection wins: it finds the whole label wherever the minimap is, and a longer
/// map name simply makes a wider crop. A stored box is only the fallback for a
/// frame where detection fails. Preferring a stored box clipped the next map's
/// name whenever it was longer than the one the box was drawn around, and two
/// maps sharing a prefix then looked identical.
pub fn map_label(frame: &Frame, drawn: Option<Region>) -> Option<Gray> {
    if let Some(gray) = read_map_label(frame) {
        return Some(gray);
    }
    drawn
        .filter(Region::is_usable)
        .and_then(|region| crop_gray(frame, region))
        .map(|gray| resample(&gray, MAP_LABEL_W, MAP_LABEL_H))
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
            .explain_text(line)
            .iter()
            .filter(|guess| !guess.known)
            .count();
        text.push(
            atlas
                .recognize_text(line, (line.h as i32 / 3).max(2))
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
                bitmap: vec![255],
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
    fn native_bitmaps_keep_punctuation_apart() {
        // A 2x2 comma and a 1x1 dot. The old 12x16 resample turned both into
        // similar blobs; native bitmaps must keep them distinct.
        let mut g = canvas(12, 12);
        g.px[2 * 12 + 1] = 230;
        g.px[3 * 12 + 2] = 230;
        g.px[6 * 12 + 7] = 230;

        let mut atlas = Atlas::default();
        atlas.learn(",.", &g).expect("learn");
        assert_eq!(atlas.glyphs.len(), 2);
        assert_eq!(atlas.glyphs[0].w, 2);
        assert_eq!(atlas.glyphs[0].h, 2);
        assert_eq!(atlas.glyphs[1].w, 1);
        assert_eq!(atlas.glyphs[1].h, 1);
        assert_eq!(
            atlas.recognize(&g).map(|(text, _)| text).as_deref(),
            Ok(",.")
        );
    }

    #[test]
    fn legacy_cells_migrate_to_native_bitmaps() {
        // A 2x2 glyph stored in the old 12x16 format. Loading it must recover a
        // native 2x2 bitmap rather than dropping the profile's learned digits.
        let stored = crate::settings::Atlas {
            format: 0,
            source: String::new(),
            cell_w: CELL_W as u32,
            cell_h: CELL_H as u32,
            glyphs: vec![crate::settings::GlyphCell {
                ch: "X".into(),
                w: 2,
                h: 2,
                bitmap: String::new(),
                cell: crate::settings::to_hex(&[255u8; CELL_LEN]),
            }],
        };

        let atlas = Atlas::from_settings(&stored);
        assert_eq!(atlas.glyphs.len(), 1);
        assert_eq!(atlas.glyphs[0].w, 2);
        assert_eq!(atlas.glyphs[0].h, 2);
        assert_eq!(atlas.glyphs[0].bitmap, vec![255, 255, 255, 255]);
    }

    #[test]
    fn refining_seed_learns_a_near_rendering() {
        // The seeded asset and the client's render can differ by a stroke (the
        // Ideal `7`). A confident near match must be stored as an exact variant.
        let base = [1u8, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 1, 1, 1, 1];
        let variant = [1u8, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1, 1, 1, 1];

        let render = |bits: &[u8]| {
            let mut gray = canvas(12, 12);
            for y in 0..5 {
                for x in 0..3 {
                    if bits[y * 3 + x] > 0 {
                        gray.px[(y + 2) * 12 + (x + 2)] = 255;
                    }
                }
            }
            gray
        };

        let mut atlas = Atlas::default();
        atlas.learn_glyph(
            '7',
            base.iter().map(|bit| if *bit > 0 { 255 } else { 0 }).collect(),
            3,
            5,
        );

        let gray = render(&variant);
        assert!(atlas.explain(&gray)[0].distance > 0.0);

        assert_eq!(atlas.learn_rendering(&gray), 1);
        assert_eq!(atlas.glyphs.len(), 2);
        assert_eq!(atlas.explain(&gray)[0].distance, 0.0);
    }

    #[test]
    fn atlas_round_trips_native_bitmaps() {
        let (a, _) = sample_pair();
        let mut atlas = Atlas::default();
        atlas.learn("01", &a).unwrap();

        let stored = atlas.to_settings();
        assert_eq!(stored.format, 2);
        let back = Atlas::from_settings(&stored);
        assert_eq!(back.glyphs.len(), atlas.glyphs.len());
        assert_eq!(back.glyphs[0].bitmap, atlas.glyphs[0].bitmap);
        assert_eq!(back.glyphs[0].w, atlas.glyphs[0].w);
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
