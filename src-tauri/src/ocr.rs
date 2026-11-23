use crate::capture::Frame;
use crate::settings::Region;

pub const CELL_W: usize = 12;
pub const CELL_H: usize = 16;
pub const CELL_LEN: usize = CELL_W * CELL_H;

const MATCH_THRESHOLD: f64 = 30.0;
/// Ordinary text (name/job) is anti-aliased at Default Ratio and further
/// softened by the Filter Applied rendering, so its letters match further out
/// than a bitmap readout. The map label and readouts use their own thresholds.
const TEXT_MATCH_THRESHOLD: f64 = 65.0;
/// A readout glyph in the Filtered rendering can sit far from the strict
/// threshold (the live `3` measured 31, the 4x11 `1` measured 58). The engine
/// cross-checks the EXP against the level and percentage, so the closest
/// readout glyph is safe to accept here. A shape past this tolerance is still
/// reported as unknown.
const READOUT_TOLERANCE: f64 = TEXT_MATCH_THRESHOLD;
/// How much closer the winning character has to be than the closest *different*
/// character before a readout glyph is trusted.
///
/// Measured against the live Filtered corpus, because a *correct* digit can have a
/// rival this close: the `36,818` readout's `3` measures 31.6 with its `8` at 33.4
/// (1.8 apart) and its `9` measures 24.3 with a `0` at 26.5 (2.2). A margin wider
/// than that starts rejecting readouts the reader gets exactly today, so the
/// margin only guards the genuine tie. When two characters explain a shape equally
/// well the shape is not a reading, and the readout is rejected instead of guessed.
///
/// This does not repair every Filtered near miss: the `26,487` fixture still draws
/// an `8` that matches its `6` at 9.9 with its own `8` at 23.7, far outside any
/// margin that leaves the reads above intact. Its `13.58%` `3` does tie (30.8 with
/// an `8` at 31.6), which is what the runner-up rule can and does refuse.
const AMBIGUITY_MARGIN: f64 = 1.0;
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

fn crop_bounds(frame: &Frame, region: Region) -> Option<(usize, usize, usize, usize)> {
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

    Some((x0, y0, x1 - x0, y1 - y0))
}

pub fn crop_gray(frame: &Frame, region: Region) -> Option<Gray> {
    let (x0, y0, w, h) = crop_bounds(frame, region)?;
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

/// Crop `region` as a white-text ink image: saturated pixels (a coloured HUD
/// tile, panel or world pixel) become background, and only low-saturation
/// bright pixels survive. This is the level-tile case: the digits are white,
/// the orange tile and its pale edge are not.
///
/// The extraction is the saturation-aware white key used by mpl-ocr; it is
/// reimplemented here for MScout's `Gray` pipeline. See
/// `THIRD_PARTY_NOTICES.md`.
pub fn crop_white_key(frame: &Frame, region: Region) -> Option<Gray> {
    let (x0, y0, w, h) = crop_bounds(frame, region)?;
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let index = ((y0 + y) * frame.width as usize + (x0 + x)) * 4;
            let r = frame.rgba[index] as i32;
            let g = frame.rgba[index + 1] as i32;
            let b = frame.rgba[index + 2] as i32;
            let lo = r.min(g).min(b);
            let hi = r.max(g).max(b);
            let ink = if hi - lo >= 60 {
                0
            } else {
                ((lo - 150) * 5 / 2).clamp(0, 255) as u8
            };
            px.push(ink);
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
        let variance =
            weight_background * weight_foreground * (mean_background - mean_foreground).powi(2);

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

/// Grey levels between a shape's most common value and its brightest that still
/// make it solid ink rather than ink on a pale tile. A picture recovered from
/// the client's stretch is within a level or two of the drawn one, and one
/// recovered from Filter Applied's sharpened stretch within about 20 (a name's
/// 239 ink comes back as 220 to 255); a real pale tile, such as the orange
/// level tile under its white digit, sits far further below its ink.
const SOLID_SPREAD: usize = 32;

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
    let mut saturated = total > 0 && ink * 100 >= total * 85;

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
        // The shape's most common value is already its brightest: there is no
        // pale tile under it, the shape is solid ink. A thin stroke such as the
        // `i` in a name (a 1 px bar with a one-pixel gap under its dot) is 8 of 9
        // pixels ink; the own-floor threshold would call none of it ink and
        // leave an empty shape, which then matches any 1 px glyph exactly. Some
        // spread still counts as solid ([`SOLID_SPREAD`]).
        if peak <= floor + SOLID_SPREAD {
            saturated = false;
        }
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
            let sx = (x0 + ((gx as f64 + 0.5) * w as f64 / CELL_W as f64) as usize).min(x_end - 1);
            let sy = (y0 + ((gy as f64 + 0.5) * h as f64 / CELL_H as f64) as usize).min(y_end - 1);
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

/// One half of a wide shape, cut at `start` for `width` columns.
fn split_part(segment: &Segment, start: u32, width: u32) -> Segment {
    let mut mask = vec![0u8; (width * segment.h) as usize];
    for y in 0..segment.h {
        for x in 0..width {
            mask[(y * width + x) as usize] = segment.mask[(y * segment.w + start + x) as usize];
        }
    }
    let cell = cell_of_native(&mask, width as usize, segment.h as usize);
    Segment {
        mask,
        cell,
        x: segment.x + start as i32,
        y: segment.y,
        w: width,
        h: segment.h,
    }
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
        .map(|y| (0..gray.w).filter(|x| mask[y * gray.w + x]).count() as u32)
        .collect()
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

        let (cell, native) = normalize(gray, &mask, gray.w, start, top, end - start, bottom - top);
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

    // The map label's crop includes the panel fill and sometimes the icon frame;
    // Otsu then calls the fill ink and merges the street and map lines. The label
    // text is near-white, so use a white floor to isolate the lines.
    let mask = foreground_with_floor(gray, 200);
    let mut occupied = vec![false; gray.h];
    // The label crop can include the panel's white border; a border column would
    // mark every row occupied and merge the street and map lines. Ignore the
    // outer two columns, which are frame rather than text.
    let x_start = 2.min(gray.w);
    let x_end = gray.w.saturating_sub(2).max(x_start);
    // A single bright icon/border column should not make a row count as text; a
    // real text row has many white pixels across the line.
    let row_floor = (gray.w / 12).max(2);
    for y in 0..gray.h {
        let mut count = 0usize;
        for x in x_start..x_end {
            if mask[y * gray.w + x] {
                count += 1;
            }
        }
        occupied[y] = count >= row_floor;
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
                    let b_pixel = if sx >= 0 && sy >= 0 && sx < CELL_W as i32 && sy < CELL_H as i32
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

/// What the atlas thinks one shape is, and what else it very nearly is.
#[derive(Clone, Copy, Debug)]
struct Ranked {
    ch: char,
    distance: f64,
    /// The closest *different* allowed character, and how far away it is.
    /// `'?'` / `f64::MAX` when no other character is allowed here.
    rival: char,
    rival_distance: f64,
}

impl Ranked {
    /// True when a different character is close enough that the winner is a guess.
    fn ambiguous(self) -> bool {
        self.rival_distance - self.distance < AMBIGUITY_MARGIN
    }

    /// Why the shape was not read, for the rejection the caller reports.
    fn refusal(self) -> String {
        format!(
            "'{}' ({:.1}) and '{}' ({:.1}) are {:.1} apart — too close to tell apart",
            self.ch,
            self.distance,
            self.rival,
            self.rival_distance,
            self.rival_distance - self.distance
        )
    }
}

/// How the two halves of a split shape are matched.
///
/// The readouts refuse a half whose runner-up explains it as well as the winner:
/// a guessed digit changes the number the player sees. Ordinary text keeps the
/// plain nearest glyph, because a name or job letter is anti-aliased at a size the
/// Filtered digits' margin was never calibrated on, and the identity reader
/// cross-checks the finished string against the profile.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SplitPolicy {
    Readout,
    Text,
}

impl Atlas {
    pub fn from_settings(atlas: &crate::settings::StoredAtlas) -> Self {
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

    pub fn to_settings(&self) -> crate::settings::StoredAtlas {
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
        crate::settings::StoredAtlas {
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
    /// closest of them is too far to trust, or when a different allowed character
    /// is within [`AMBIGUITY_MARGIN`] of it.
    fn match_segment_within(&self, segment: &Segment, allowed: &str) -> Option<(char, f64)> {
        self.match_ranked(segment, allowed, MATCH_THRESHOLD)
            .filter(|ranked| !ranked.ambiguous())
            .map(|ranked| (ranked.ch, ranked.distance))
    }

    /// Nearest glyph whose character is one of `allowed`, or `None` when even the
    /// closest of them is too far to trust. No runner-up veto: this is the
    /// ordinary-text matcher, which never had one.
    fn match_segment_nearest(&self, segment: &Segment, allowed: &str) -> Option<(char, f64)> {
        self.match_ranked(segment, allowed, MATCH_THRESHOLD)
            .map(|ranked| (ranked.ch, ranked.distance))
    }

    /// The two nearest allowed characters for a shape, or `None` when the closest
    /// allowed glyph is further than `tolerance`.
    ///
    /// The runner-up is the closest *different* character: another shape of the
    /// same character is a rendering variant, not a competing reading.
    fn match_ranked(&self, segment: &Segment, allowed: &str, tolerance: f64) -> Option<Ranked> {
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

        let (ch, distance) = best?;
        if distance > tolerance {
            return None;
        }

        let mut rival = ('?', f64::MAX);
        for glyph in &self.glyphs {
            if glyph.ch == ch || !allowed.contains(glyph.ch) {
                continue;
            }
            let distance = self.distance_to(segment, glyph);
            if distance < rival.1 {
                rival = (glyph.ch, distance);
            }
        }

        let ranked = Ranked {
            ch,
            distance,
            rival: rival.0,
            rival_distance: rival.1,
        };
        Some(if ranked.ambiguous() {
            self.settle_in_place(segment, ranked)
        } else {
            ranked
        })
    }

    /// Settle a near tie by how the two characters match **in place**.
    ///
    /// The native comparison searches a one-pixel shift, which lets a wrong
    /// glyph close the gap: on the Filtered HUD a live `3` scored 29.9 against
    /// the `3` variants and 29.7 against an `8` shifted one pixel, although
    /// unshifted it is 11 pixels from a `3` and 15 from an `8`. When both
    /// characters have glyphs of the shape's exact size and one is clearly
    /// closer without a shift, it wins; otherwise the tie stands and the read
    /// is refused as before. Only a refused shape can change here.
    fn settle_in_place(&self, segment: &Segment, ranked: Ranked) -> Ranked {
        let in_place = |ch: char| -> Option<f64> {
            self.glyphs
                .iter()
                .filter(|glyph| glyph.ch == ch && glyph.w == segment.w && glyph.h == segment.h)
                .map(|glyph| {
                    native_distance(
                        &segment.mask,
                        &glyph.bitmap,
                        segment.w as usize,
                        segment.h as usize,
                        0,
                    )
                })
                .min_by(f64::total_cmp)
        };
        let (Some(own), Some(rival)) = (in_place(ranked.ch), in_place(ranked.rival)) else {
            return ranked;
        };
        // Two whole pixels of the shape, in distance units.
        let margin = 2.0 * 255.0 / (segment.w * segment.h).max(1) as f64;
        let settled = |ch: char, rival: char, distance: f64| Ranked {
            ch,
            distance,
            rival,
            rival_distance: distance + AMBIGUITY_MARGIN,
        };
        if rival + margin <= own {
            settled(ranked.rival, ranked.ch, ranked.rival_distance)
        } else if own + margin <= rival {
            settled(ranked.ch, ranked.rival, ranked.distance)
        } else {
            ranked
        }
    }

    /// Read an EXP-style readout — a number, then a bracketed percentage — using
    /// the shape of the readout to restrict what each position may contain. The
    /// closing bracket is only a candidate once the percentage has started, which
    /// is what stops a `1` (the same narrow bar) being read as one.
    ///
    /// The order of attempts is deliberate: the strict glyph, then the
    /// punctuation the grammar implies, then a touching pair split in two, then
    /// the tolerant near miss.
    ///
    /// A shape none of those explains is a hole. A hole beside the readout is junk
    /// the box happened to catch (the `EXP` label, a panel rule) and is ignored;
    /// a hole *inside* the readout would silently drop a digit, so it rejects the
    /// read instead of returning a shorter, plausible number.
    fn readout_matches(&self, found: Vec<Segment>) -> Result<Vec<(Segment, char, f64)>, String> {
        const NUMBER: &str = "0123456789,[";
        const PERCENT: &str = "0123456789.%";
        const CLOSE: &str = "]";

        let mut out: Vec<(Segment, char, f64)> = Vec::new();
        // Unread shapes and why, kept until the end: whether a hole is inside the
        // readout is only known once the last shape has been placed.
        let mut unread: Vec<(i32, String)> = Vec::new();
        let mut stage = 0usize;

        for segment in found {
            let allowed = match stage {
                0 => NUMBER,
                1 => PERCENT,
                _ => CLOSE,
            };
            let mut refusal: Option<String> = None;
            // The distance of the single-glyph candidate the runner-up vetoed, if
            // the strict step found one. Used to judge a split below.
            let mut single_best: Option<f64> = None;

            if let Some(ranked) = self.match_ranked(&segment, allowed, MATCH_THRESHOLD) {
                if !ranked.ambiguous() {
                    stage = advance_stage(stage, ranked.ch);
                    out.push((segment, ranked.ch, ranked.distance));
                    continue;
                }
                refusal = Some(ranked.refusal());
                single_best = Some(ranked.distance);
            }

            // A tiny shape in a punctuation slot cannot be a digit at this font
            // size. The Filtered rendering changes the comma/period enough that
            // the atlas can be far away; infer the punctuation from the readout
            // grammar and let the digits decide the value.
            if segment.w <= 4 && segment.h <= 5 {
                let punct = match stage {
                    0 => Some(','),
                    1 => Some('.'),
                    _ => None,
                };
                if let Some(ch) = punct {
                    let (_, best, _) = self.match_segment(&segment);
                    out.push((segment, ch, best));
                    continue;
                }
            }

            // A wide shape can be two touching glyphs (the 1920 Default `2`+`7`);
            // try every cut and keep the pair that explains the pixels best. A split
            // is only worth taking when it explains the shape better than the single
            // glyph that was vetoed above: cutting a tied digit in half would be
            // another way of guessing.
            if let Some(parts) = self.split_segment(&segment, allowed, SplitPolicy::Readout) {
                let total: f64 = parts.iter().map(|(_, _, best)| best).sum();
                if single_best.map_or(true, |best| total < best) {
                    for (sub, ch, best) in parts {
                        stage = advance_stage(stage, ch);
                        out.push((sub, ch, best));
                    }
                    continue;
                }
            }

            // A single glyph that is just past the strict threshold (the live
            // Filtered `3` measures 31). Try this after splitting so a touching
            // pair is not mistaken for one wide digit.
            if let Some(ranked) = self.match_ranked(&segment, allowed, READOUT_TOLERANCE) {
                if !ranked.ambiguous() {
                    stage = advance_stage(stage, ranked.ch);
                    out.push((segment, ranked.ch, ranked.distance));
                    continue;
                }
                refusal.get_or_insert_with(|| ranked.refusal());
            }

            unread.push((
                segment.x,
                refusal.unwrap_or_else(|| "no glyph in the atlas matches it".into()),
            ));
        }

        // A hole *between* two read glyphs is a dropped digit. Outside that span it
        // is the junk the box always could catch — the `EXP` label to its left, a
        // panel rule to its right — which is why an edge shape may still be ignored.
        let span = out
            .first()
            .zip(out.last())
            .map(|(first, last)| (first.0.x, last.0.x));
        if let Some((left, right)) = span {
            if let Some((x, refusal)) = unread.iter().find(|(x, _)| *x > left && *x < right) {
                return Err(format!(
                    "the EXP readout has a shape at x={x} that cannot be read: {refusal}"
                ));
            }
        }

        Ok(out)
    }

    /// Cut a wide shape into two touching glyphs and match both halves, under
    /// `policy`.
    fn split_segment(
        &self,
        segment: &Segment,
        allowed: &str,
        policy: SplitPolicy,
    ) -> Option<Vec<(Segment, char, f64)>> {
        if segment.w < 9 {
            return None;
        }
        let accept = |sub: &Segment, allowed: &str| match policy {
            SplitPolicy::Readout => self.match_segment_within(sub, allowed),
            SplitPolicy::Text => self.match_segment_nearest(sub, allowed),
        };
        type SplitMatch = (Segment, char, f64);
        let mut best: Option<(f64, Vec<SplitMatch>)> = None;
        for cut in 3..=segment.w.saturating_sub(3) {
            let left = split_part(segment, 0, cut);
            let right = split_part(segment, cut, segment.w - cut);
            let Some((left_ch, left_distance)) = accept(&left, allowed) else {
                continue;
            };
            let right_allowed = match left_ch {
                '[' => "0123456789.%",
                '%' => "]",
                _ => allowed,
            };
            let Some((right_ch, right_distance)) = accept(&right, right_allowed) else {
                continue;
            };
            let total = left_distance + right_distance;
            if best
                .as_ref()
                .map_or(true, |(best_total, _)| total < *best_total)
            {
                best = Some((
                    total,
                    vec![
                        (left, left_ch, left_distance),
                        (right, right_ch, right_distance),
                    ],
                ));
            }
        }
        best.map(|(_, parts)| parts)
    }

    pub fn recognize_readout(&self, gray: &Gray) -> Result<(String, f64), String> {
        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }

        let matches = self.readout_matches(segments(gray))?;
        if matches.is_empty() {
            return Err("nothing recognizable in the box — learn the digits".into());
        }

        let mut text = String::new();
        let mut confidence = 0.0;
        for (_, ch, best) in &matches {
            text.push(*ch);
            confidence += (1.0 - best / 255.0).max(0.0);
        }

        // The brackets are part of the readout, not decoration: a crop with no `[`
        // or `]` is a different (or clipped) box, and inventing one would invent a
        // digit position that was never drawn.
        validate_readout(&text)?;

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
        let mut all_chars: Option<String> = None;

        for segment in found {
            let (ch, best, _) = self.match_segment(&segment);
            if best > TEXT_MATCH_THRESHOLD {
                // Anti-aliased text at Default sizes lets adjacent letters touch.
                // Split the shape and match both halves against the whole set.
                let allowed = all_chars.get_or_insert_with(|| self.learned());
                let Some(parts) = self.split_segment(&segment, allowed.as_str(), SplitPolicy::Text)
                else {
                    continue;
                };
                for (sub, ch, best) in parts {
                    if best > TEXT_MATCH_THRESHOLD {
                        continue;
                    }
                    if let Some(end) = last_end {
                        if sub.x - end >= space_gap {
                            text.push(' ');
                        }
                    }
                    text.push(ch);
                    last_end = Some(sub.x + sub.w as i32);
                    confidence += (1.0 - best / 255.0).max(0.0);
                    matched += 1;
                }
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

    /// Learn the live rendering of a text line (name/job). Unlike the readout
    /// learning, keep full-height letters, because a tight crop makes a capital or
    /// ascender exactly the box height.
    pub fn learn_text_rendering(&mut self, gray: &Gray) -> usize {
        let mut added = 0;
        for _ in 0..2 {
            let mut pass = 0;
            for segment in text_segments(gray) {
                let (ch, best, other) = self.match_segment(&segment);
                if best == 0.0 || best > 65.0 || other < best + 4.0 {
                    continue;
                }
                let before = self.glyphs.len();
                self.learn_glyph(ch, segment.mask, segment.w, segment.h);
                if self.glyphs.len() > before {
                    pass += 1;
                }
            }
            added += pass;
            if pass == 0 {
                break;
            }
        }
        added
    }

    /// Add the exact shapes of a live readout so a rendering difference becomes a
    /// variant instead of a near miss. A shape is learned when the best character
    /// is clear of the runner-up; the loop re-matches after each pass so a learned
    /// variant can expose the next shape. This is what lets the 1920 Default `2`
    /// and `%` (one stroke off the asset) read at distance 0 after seeding.
    pub fn learn_rendering(&mut self, gray: &Gray) -> usize {
        let mut added = 0;
        for _ in 0..3 {
            let mut pass = 0;
            for segment in segments(gray) {
                let (ch, best, other) = self.match_segment(&segment);
                if best == 0.0 || best > 45.0 || other < best + 4.0 {
                    continue;
                }
                let before = self.glyphs.len();
                self.learn_glyph(ch, segment.mask, segment.w, segment.h);
                if self.glyphs.len() > before {
                    pass += 1;
                }
            }
            added += pass;
            if pass == 0 {
                break;
            }
        }
        added
    }

    /// Learn the level rendering from bright ink. The level sits on pale tiles, so
    /// its reader segments on near-white text; the same shapes must be learned for
    /// the live glyph to match exactly (the 1920 Default `3` is far from the
    /// scaled EXP design).
    pub fn learn_bright(&mut self, gray: &Gray, floor: u8) -> usize {
        let mut added = 0;
        for _ in 0..2 {
            let mut pass = 0;
            for segment in bright_segments(gray, floor) {
                let (ch, best, other) = self.match_segment(&segment);
                if best == 0.0 || best > 65.0 || other < best + 4.0 {
                    continue;
                }
                let before = self.glyphs.len();
                self.learn_glyph(ch, segment.mask, segment.w, segment.h);
                if self.glyphs.len() > before {
                    pass += 1;
                }
            }
            added += pass;
            if pass == 0 {
                break;
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

    #[cfg(test)]
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
            if let Ok((text, confidence)) = self.read_digit_shapes(bright) {
                if !text.is_empty() {
                    return Ok((text, confidence));
                }
            }
        }

        self.recognize_digits(gray)
    }

    /// Read the level from a white-ink keyed crop. The key has already removed
    /// the coloured tile, so a lower fixed floor is safe and keeps anti-aliased
    /// strokes that [`TEXT_FLOOR`] would drop.
    pub fn recognize_level_keyed(&self, keyed: &Gray) -> Result<(String, f64), String> {
        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }

        let bright = bright_segments(keyed, 127);
        if !bright.is_empty() {
            if let Ok((text, confidence)) = self.read_digit_shapes(bright) {
                if !text.is_empty() {
                    return Ok((text, confidence));
                }
            }
        }

        self.recognize_digits(keyed)
    }

    fn recognize_digits(&self, gray: &Gray) -> Result<(String, f64), String> {
        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }
        let found = segments(gray);
        if found.is_empty() {
            return Err("nothing readable in the box".into());
        }
        self.read_digit_shapes(found)
    }

    /// Read a digits-only readout (the level) with the same ordered fallback as
    /// the EXP readout: strict glyph, split a touching pair, then accept a clear
    /// near miss. The level box has no punctuation and nothing else drawn inside
    /// it, so a shape none of those explains is a failed read: dropping it would
    /// turn `35` into a level of `5`, a plausible wrong number rather than a
    /// rejection.
    fn read_digit_shapes(&self, found: Vec<Segment>) -> Result<(String, f64), String> {
        const DIGITS: &str = "0123456789";
        let mut text = String::with_capacity(found.len());
        let mut confidence = 0.0;
        let mut matched = 0usize;

        for segment in found {
            let mut hits: Vec<(char, f64)> = Vec::new();
            let mut refusal: Option<String> = None;
            let mut single_best: Option<f64> = None;

            if let Some(ranked) = self.match_ranked(&segment, DIGITS, MATCH_THRESHOLD) {
                if !ranked.ambiguous() {
                    hits.push((ranked.ch, ranked.distance));
                } else {
                    refusal = Some(ranked.refusal());
                    single_best = Some(ranked.distance);
                }
            }

            if hits.is_empty() {
                if let Some(parts) = self.split_segment(&segment, DIGITS, SplitPolicy::Readout) {
                    let total: f64 = parts.iter().map(|(_, _, best)| best).sum();
                    if single_best.map_or(true, |best| total < best) {
                        for (_, ch, best) in parts {
                            hits.push((ch, best));
                        }
                    }
                }
            }

            if hits.is_empty() {
                if let Some(ranked) = self.match_ranked(&segment, DIGITS, READOUT_TOLERANCE) {
                    if !ranked.ambiguous() {
                        hits.push((ranked.ch, ranked.distance));
                    } else {
                        refusal.get_or_insert_with(|| ranked.refusal());
                    }
                }
            }

            if hits.is_empty() {
                let reason = refusal.unwrap_or_else(|| "no glyph in the atlas matches it".into());
                return Err(format!(
                    "the level readout has a shape at x={} that cannot be read: {reason}",
                    segment.x
                ));
            }

            for (ch, best) in hits {
                text.push(ch);
                confidence += (1.0 - best / 255.0).max(0.0);
                matched += 1;
            }
        }

        if matched == 0 {
            return Err("nothing recognizable in the box — learn the digits".into());
        }
        Ok((text, confidence / matched as f64))
    }

    #[cfg(test)]
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

/// The readout stage the grammar is in after reading `ch`: the number, the
/// percentage between the brackets, or the closing bracket.
fn advance_stage(stage: usize, ch: char) -> usize {
    match ch {
        '[' => 1,
        '%' => 2,
        _ => stage,
    }
}

/// Check that a read EXP readout is structurally complete: a number
/// (`digits` and thousands `,`), then `[`, then `digits[.digits]`, then `%`, then
/// `]` as the last character. Nothing else, in that order, exactly once.
///
/// `readout_matches` already restricts each position by stage, but it judges one
/// shape at a time: a dropped bracket, or a `]` that came out before the `%`,
/// still left a string that looked like a value. This is the readout's own
/// grammar applied to the finished text, so a readout that is missing a bracket
/// is rejected rather than repaired — synthesising one would invent a digit
/// position the client never drew.
fn validate_readout(text: &str) -> Result<(), String> {
    let open = text
        .find('[')
        .ok_or_else(|| format!("'{text}' has no '[' — not a complete EXP readout"))?;
    let percent_at = text[open..]
        .find('%')
        .map(|index| open + index)
        .ok_or_else(|| format!("'{text}' has no '%' — not a complete EXP readout"))?;
    let close = text
        .rfind(']')
        .ok_or_else(|| format!("'{text}' has no ']' — not a complete EXP readout"))?;

    if close < percent_at {
        return Err(format!("'{text}' closes the bracket before its '%'"));
    }
    if close + 1 != text.len() {
        return Err(format!("'{text}' has characters after its ']'"));
    }
    // Nothing may sit between the '%' and the ']': the percentage slice below
    // stops at the '%', so `91.53%x`, `91.53%%` and `91.53%1` would otherwise
    // all validate as a readout whose percentage was never drawn.
    if percent_at + 1 != close {
        return Err(format!(
            "'{text}' has characters between its '%' and its ']'"
        ));
    }
    if text[open + 1..close]
        .chars()
        .any(|ch| ch == '[' || ch == ']')
    {
        return Err(format!("'{text}' has more than one bracket pair"));
    }
    if text[..open]
        .chars()
        .any(|ch| ch == '[' || ch == ']' || ch == '%')
    {
        return Err(format!("'{text}' has a bracket or '%' before its '['"));
    }

    let number = &text[..open];
    if number.is_empty() {
        return Err(format!("'{text}' has no EXP number before its '['"));
    }
    if !number.chars().all(|ch| ch.is_ascii_digit() || ch == ',') {
        return Err(format!(
            "'{text}' has a character that is not a digit or ',' in its EXP number"
        ));
    }
    if number.starts_with(',') || number.ends_with(',') || number.contains(",,") {
        return Err(format!("'{text}' has a misplaced ',' in its EXP number"));
    }

    let percent = &text[open + 1..percent_at];
    if percent.is_empty() {
        return Err(format!("'{text}' has no percentage between '[' and '%'"));
    }
    if !percent.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
        return Err(format!(
            "'{text}' has a character that is not a digit or '.' in its percentage"
        ));
    }
    if percent.matches('.').count() > 1 || percent.starts_with('.') || percent.ends_with('.') {
        return Err(format!("'{text}' has a misplaced '.' in its percentage"));
    }

    Ok(())
}

pub fn parse_exp_text(text: &str) -> Option<(i64, f64)> {
    // A readout without both brackets is not one: the reader rejects those, and
    // nothing downstream should quietly accept the same string.
    let open = text.find('[')?;
    if !text[open..].contains(']') {
        return None;
    }

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
        let sy =
            (((y as f64 + 0.5) * gray.h as f64 / h as f64) as usize).min(gray.h.saturating_sub(1));
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

/// The three states of the minimap widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MinimapState {
    /// Title bar plus the icon/name band.
    Expanded,
    /// Title bar only; no name is drawn.
    TitleBarOnly,
    /// One-line bar with the whole `Street :Map` label inside it.
    Folded,
}

#[derive(Clone, Copy, Debug)]
pub struct Minimap {
    pub state: MinimapState,
    /// The chrome band: the title bar in states 1/2, the whole bar when folded.
    pub bar: Region,
    /// The `WORLD` badge (the orange chip).
    pub badge: Region,
}

const FOLDED_FILL: (i32, i32, i32) = (221, 221, 221);

fn near_rgb(frame: &Frame, x: usize, y: usize, rgb: (i32, i32, i32), tol: i32) -> bool {
    let w = frame.width as usize;
    let index = (y * w + x) * 4;
    (frame.rgba[index] as i32 - rgb.0).abs() <= tol
        && (frame.rgba[index + 1] as i32 - rgb.1).abs() <= tol
        && (frame.rgba[index + 2] as i32 - rgb.2).abs() <= tol
}

/// Every connected component whose size passes `accept`, largest first.
fn component_regions(
    frame: &Frame,
    matches: impl Fn(usize, usize) -> bool,
    accept: impl Fn(i32, i32) -> bool,
) -> Vec<Region> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let mut mask = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            mask[y * w + x] = matches(x, y);
        }
    }

    let mut visited = vec![false; w * h];
    let mut out: Vec<Region> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !mask[start] || visited[start] {
            continue;
        }
        stack.clear();
        stack.push(start);
        visited[start] = true;
        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        while let Some(index) = stack.pop() {
            let x = index % w;
            let y = index / w;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < w && ny < h {
                    let next = ny * w + nx;
                    if mask[next] && !visited[next] {
                        visited[next] = true;
                        stack.push(next);
                    }
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
        if accept(region.w, region.h) {
            out.push(region);
        }
    }
    out.sort_by_key(|region| -(region.w * region.h));
    out
}

/// Fraction of a bbox that the predicate matches. Used to reject a hollow frame
/// (a white title bar's gray border) from being taken for a solid folded bar or
/// name band.
fn component_solidity(
    frame: &Frame,
    region: Region,
    matches: impl Fn(usize, usize) -> bool,
) -> f64 {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let x0 = region.x.max(0) as usize;
    let y0 = region.y.max(0) as usize;
    let x1 = (region.x + region.w).max(0).min(w as i32) as usize;
    let y1 = (region.y + region.h).max(0).min(h as i32) as usize;
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    let mut count = 0usize;
    let mut area = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            area += 1;
            if matches(x, y) {
                count += 1;
            }
        }
    }
    count as f64 * 100.0 / area as f64
}

/// The badge is dithered between its border and fill shades, so it is matched as
/// an orange range rather than one exact colour.
fn is_badge_pixel(frame: &Frame, x: usize, y: usize) -> bool {
    let index = (y * frame.width as usize + x) * 4;
    let r = frame.rgba[index] as i32;
    let g = frame.rgba[index + 1] as i32;
    let b = frame.rgba[index + 2] as i32;
    r >= 190 && (130..=235).contains(&g) && b <= 130
}

/// Broader badge match for the Filter Applied rendering. The filter blurs the
/// chip and shifts its shades, so the exact range breaks into fragments; this
/// keeps the orange family and lets the component shape and the enclosing body
/// reject scenery.
fn is_badge_pixel_tolerant(frame: &Frame, x: usize, y: usize) -> bool {
    let index = (y * frame.width as usize + x) * 4;
    let r = frame.rgba[index] as i32;
    let g = frame.rgba[index + 1] as i32;
    let b = frame.rgba[index + 2] as i32;
    r >= 190 && (110..=240).contains(&g) && b <= 140 && r - b >= 60 && g - b >= 40
}

/// The minimap name band's fill (`#99BBCC`) after the filter: the same blue-gray
/// family, with the lighter/darker shades the filter produces. Sky and clouds are
/// bluer and lighter, so the `b > r` / `g > r` split keeps them out; the caller
/// also requires a solid component near the badge.
fn is_panel_pixel_tolerant(frame: &Frame, x: usize, y: usize) -> bool {
    let index = (y * frame.width as usize + x) * 4;
    let r = frame.rgba[index] as i32;
    let g = frame.rgba[index + 1] as i32;
    let b = frame.rgba[index + 2] as i32;
    (r - PANEL_RGB.0).abs() <= 28
        && (g - PANEL_RGB.1).abs() <= 28
        && (b - PANEL_RGB.2).abs() <= 24
        && b > r + 20
        && g > r + 8
}

/// The folded bar's light neutral fill (`#DDDDDD`) after the filter. White is
/// excluded, so a white title bar cannot become a folded bar; the caller also
/// requires a solid component.
fn is_folded_pixel_tolerant(frame: &Frame, x: usize, y: usize) -> bool {
    let index = (y * frame.width as usize + x) * 4;
    let r = frame.rgba[index] as i32;
    let g = frame.rgba[index + 1] as i32;
    let b = frame.rgba[index + 2] as i32;
    let high = r.max(g).max(b);
    let low = r.min(g).min(b);
    (200..=245).contains(&high) && high - low <= 12
}

/// Count the active (bright blue) zoom buttons in the title bar.
///
/// State 1 has one active button (`-` blue, `+` grey); state 2 has two. This
/// corroborates the panel-height test, so a clipped expanded panel (shorter
/// component) cannot be mistaken for the title-bar-only state.
fn active_zoom_buttons(frame: &Frame, bar: Region, badge: Region) -> usize {
    let w = frame.width as usize;
    let x0 = (badge.x - 72).max(bar.x).max(0) as usize;
    let x1 = badge.x.max(0) as usize;
    let y0 = badge.y.max(0) as usize;
    let y1 = (badge.y + badge.h).max(0) as usize;
    if x1 <= x0 || y1 <= y0 || y1 > frame.height as usize || x1 > w {
        return 0;
    }

    let mut column_blue = vec![false; x1 - x0];
    for y in y0..y1 {
        for x in x0..x1 {
            let index = (y * w + x) * 4;
            let r = frame.rgba[index] as i32;
            let g = frame.rgba[index + 1] as i32;
            let b = frame.rgba[index + 2] as i32;
            if b > 190 && b - r > 70 && g > 140 {
                column_blue[x - x0] = true;
            }
        }
    }

    let mut runs = 0;
    let mut in_run = false;
    let mut gap = 0;
    for blue in column_blue {
        if blue {
            if !in_run {
                runs += 1;
                in_run = true;
            }
            gap = 0;
        } else {
            gap += 1;
            // A one-pixel gap is dithering inside one button; a wider gap
            // separates the two.
            if gap >= 2 {
                in_run = false;
            }
        }
    }
    runs
}

/// Luminance-based ink box. The Filter Applied `MINI MAP` text is blue-gray with a
/// high blue channel, so the all-channel `< 60` cutoff used for the map label
/// would miss it; the bar and sky stay much brighter than the text.
fn ink_below_luma(frame: &Frame, region: Region, cutoff: u8) -> Option<(Region, usize)> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let x0 = region.x.max(0) as usize;
    let y0 = region.y.max(0) as usize;
    let x1 = (region.x + region.w).max(0).min(w as i32) as usize;
    let y1 = (region.y + region.h).max(0).min(h as i32) as usize;
    let mut min_x = usize::MAX;
    let mut min_y = usize::MAX;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut count = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let index = (y * w + x) * 4;
            let r = frame.rgba[index] as u32;
            let g = frame.rgba[index + 1] as u32;
            let b = frame.rgba[index + 2] as u32;
            if ((77 * r + 150 * g + 29 * b) >> 8) < cutoff as u32 {
                count += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if count == 0 {
        return None;
    }
    Some((
        Region {
            x: min_x as i32,
            y: min_y as i32,
            w: (max_x - min_x + 1) as i32,
            h: (max_y - min_y + 1) as i32,
        },
        count,
    ))
}

/// The title-bar-only state when the title bar is white rather than `#99BBCC`
/// (the 1920 Default rendering). The `MINI MAP` text plus the dark map body below
/// it identify the widget: a folded bar has no map body, and an expanded name band
/// is already caught by the panel match.
fn titlebar_fallback(frame: &Frame, badge: Region) -> Option<Minimap> {
    let x1 = (badge.x - 72).max(0);
    // Start inside the widget's left frame (about 150 px left of the badge) and
    // just right of it: the frame is dark in the filtered rendering, and the
    // composed frame's black background also counts as ink.
    let x0 = (badge.x - 140).max(0);
    let text_region = Region {
        x: x0,
        // Keep the box inside the title bar's frame: the filtered rendering makes
        // the bar's border dark enough to fill a taller box.
        y: (badge.y + 2).max(0),
        w: (x1 - x0).max(0),
        h: (badge.h - 4).max(4),
    };
    let (ink, _) = ink_below_luma(frame, text_region, 130)?;
    if !(20..=80).contains(&ink.w) || !(4..=14).contains(&ink.h) {
        return None;
    }
    let body = Region {
        x: (badge.x - 150).max(0),
        y: (badge.y + badge.h + 10).max(0),
        w: 190,
        h: 80,
    };
    let (_, dark) = ink_below_luma(frame, body, 100)?;
    if dark < 300 {
        return None;
    }
    let bar = Region {
        x: (badge.x - 170).max(0),
        y: (badge.y - 10).max(0),
        w: 210,
        h: 22,
    };
    Some(Minimap {
        state: MinimapState::TitleBarOnly,
        bar,
        badge,
    })
}

/// Locate the minimap and which of its three zoom states is showing.
///
/// The `WORLD` badge (`#DD9911` border / `#FFCC55` fill, 34×10) is present in
/// every state, so it anchors detection. The fill immediately left of it says
/// which state: `#99BBCC` is the normal panel (expanded or title-bar-only), while
/// a light neutral fill is the folded bar.
pub fn detect_minimap(frame: &Frame) -> Option<Minimap> {
    let badge_shape = |w: i32, h: i32| (24..=48).contains(&w) && (8..=16).contains(&h);
    let mut badges = component_regions(frame, |x, y| is_badge_pixel(frame, x, y), badge_shape);
    // Always add the tolerant matches: the exact pass can find scenery blobs while
    // missing a filtered badge whose component no longer fits the exact shape.
    for badge in component_regions(
        frame,
        |x, y| is_badge_pixel_tolerant(frame, x, y),
        |w, h| (20..=72).contains(&w) && (8..=24).contains(&h),
    ) {
        let duplicate = badges.iter().any(|existing| {
            existing.x == badge.x
                && existing.y == badge.y
                && existing.w == badge.w
                && existing.h == badge.h
        });
        if !duplicate {
            badges.push(badge);
        }
    }
    if badges.is_empty() {
        return None;
    }

    let bar_shape = |w: i32, h: i32| (100..=900).contains(&w) && (12..=30).contains(&h);
    let panel_shape = |w: i32, h: i32| (80..=900).contains(&w) && (12..=300).contains(&h);

    // The same orange range occurs in scenery, so try each badge-shaped blob and
    // keep the first that actually sits inside a minimap body. A weak
    // titlebar-only fallback is deferred until every badge has had a chance to
    // match a real folded or expanded body; otherwise a scenery blob near the
    // bottom-right can hide the actual folded bar at the top-left.
    let mut titlebar_fallback_match = None;
    for badge in &badges {
        let cx = badge.x + badge.w / 2;
        let cy = badge.y + badge.h / 2;
        let inside = |region: &Region| {
            cx >= region.x && cx < region.x + region.w && cy >= region.y && cy < region.y + region.h
        };

        for bar in component_regions(
            frame,
            |x, y| near_rgb(frame, x, y, FOLDED_FILL, 6),
            bar_shape,
        ) {
            if inside(&bar) {
                return Some(folded_minimap(bar, *badge));
            }
        }
        // Filter Applied: the exact fill breaks into fragments; a solid light-gray
        // bar containing the badge is the same state.
        for bar in component_regions(
            frame,
            |x, y| is_folded_pixel_tolerant(frame, x, y),
            bar_shape,
        ) {
            if inside(&bar)
                && component_solidity(frame, bar, |x, y| is_folded_pixel_tolerant(frame, x, y))
                    >= 30.0
            {
                return Some(folded_minimap(bar, *badge));
            }
        }

        // Expanded and title-bar-only share the same title bar; only the #99BBCC
        // component differs. Iterate every component so a larger scenery blob does
        // not hide the real panel.
        for panel in component_regions(frame, |x, y| is_panel_pixel(frame, x, y), panel_shape) {
            if !inside(&panel) {
                continue;
            }
            let bar = bar_of(&panel);
            let title = detect_title_text(frame, bar, *badge);
            let icon = detect_icon(frame, panel, bar);
            let active_buttons = active_zoom_buttons(frame, bar, *badge);
            let state = if icon.is_some() || panel.h >= 30 {
                MinimapState::Expanded
            } else if active_buttons >= 2 && title.is_some() {
                MinimapState::TitleBarOnly
            } else {
                MinimapState::Expanded
            };
            return Some(Minimap {
                state,
                bar,
                badge: *badge,
            });
        }

        // Filter Applied: the name band's fill is shifted and can join the title
        // bar, so the badge may be inside the component rather than below it. A
        // solid blue-gray band at least as tall as a name band is the expanded
        // state; the shorter (hollow) title bar is rejected by the solidity and
        // height checks.
        for panel in component_regions(
            frame,
            |x, y| is_panel_pixel_tolerant(frame, x, y),
            panel_shape,
        ) {
            let near = inside(&panel)
                || (cx >= panel.x
                    && cx < panel.x + panel.w
                    && panel.y >= badge.y - 2
                    && panel.y <= badge.y + badge.h + 30);
            if !near || panel.h < 25 {
                continue;
            }
            if component_solidity(frame, panel, |x, y| is_panel_pixel_tolerant(frame, x, y)) < 30.0
            {
                continue;
            }
            let bar = bar_of(&panel);
            return Some(Minimap {
                state: MinimapState::Expanded,
                bar,
                badge: *badge,
            });
        }

        // The name band can sit below the title bar, outside the badge's
        // component match. Reuse the panel detector the label reader uses: a
        // solid #99BBCC band near the badge is the expanded state, not the
        // title-bar-only fallback.
        if let Some(panel) = detect_minimap_panel(frame) {
            let near = inside(&panel)
                || (cx >= panel.x
                    && cx < panel.x + panel.w
                    && panel.y >= badge.y - 2
                    && panel.y <= badge.y + badge.h + 40);
            let lines = map_label_lines(frame).map(|lines| lines.len()).unwrap_or(0);
            if lines >= 2 || (near && panel.h >= 25) {
                let bar = bar_of(&panel);
                return Some(Minimap {
                    state: MinimapState::Expanded,
                    bar,
                    badge: *badge,
                });
            }
        }

        // Default title-bar-only: the title bar is white, not #99BBCC, and there is
        // no name band. The MINI MAP text plus the dark map body identify it. Keep
        // it as a fallback until the remaining badges have been checked.
        if titlebar_fallback_match.is_none() {
            titlebar_fallback_match = titlebar_fallback(frame, *badge);
        }
    }

    titlebar_fallback_match
}

fn folded_minimap(bar: Region, badge: Region) -> Minimap {
    Minimap {
        state: MinimapState::Folded,
        bar,
        badge,
    }
}

fn bar_of(panel: &Region) -> Region {
    Region {
        x: panel.x,
        y: panel.y,
        w: panel.w,
        h: panel.h.min(20),
    }
}

/// Dark-pixel bounding box inside `region`, with the pixel count.
fn dark_ink(frame: &Frame, region: Region) -> Option<(Region, usize)> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let x0 = region.x.max(0) as usize;
    let y0 = region.y.max(0) as usize;
    let x1 = (region.x + region.w).max(0).min(w as i32) as usize;
    let y1 = (region.y + region.h).max(0).min(h as i32) as usize;
    let mut min_x = usize::MAX;
    let mut min_y = usize::MAX;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut count = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let index = (y * w + x) * 4;
            if frame.rgba[index] < 60 && frame.rgba[index + 1] < 60 && frame.rgba[index + 2] < 60 {
                count += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if count == 0 {
        return None;
    }
    Some((
        Region {
            x: min_x as i32,
            y: min_y as i32,
            w: (max_x - min_x + 1) as i32,
            h: (max_y - min_y + 1) as i32,
        },
        count,
    ))
}

/// The `MINI MAP` title text: a short dark run at the title bar's left, before the
/// zoom buttons. The folded bar's map line is much wider and is therefore not
/// mistaken for it.
fn detect_title_text(frame: &Frame, bar: Region, badge: Region) -> Option<Region> {
    let width = (badge.x - 72).max(bar.x + 2) - bar.x;
    if width < 20 {
        return None;
    }
    let region = Region {
        x: bar.x + 2,
        y: bar.y + 1,
        w: width,
        h: bar.h.saturating_sub(2),
    };
    let (ink, _) = dark_ink(frame, region)?;
    if (20..=80).contains(&ink.w) && (4..=14).contains(&ink.h) {
        Some(ink)
    } else {
        None
    }
}

/// The map icon in the name band: a dark-framed square under the title bar, left
/// of the name text. Only the expanded state draws one.
fn detect_icon(frame: &Frame, panel: Region, bar: Region) -> Option<Region> {
    let top = bar.y + bar.h;
    let height = (panel.y + panel.h) - top;
    if height < 16 {
        return None;
    }
    let region = Region {
        x: panel.x + 2,
        y: top,
        w: 64,
        h: height,
    };
    let (ink, count) = dark_ink(frame, region)?;
    if (20..=48).contains(&ink.w) && (20..=48).contains(&ink.h) && count >= 150 {
        Some(ink)
    } else {
        None
    }
}

/// The folded bar's label crop: dark text on a light fill, cropped to the ink.
///
/// The bar is `Street :Map` on one line. The anchored bounds stop short of the
/// `-`/`+`/`WORLD` controls, then the crop is inverted so the existing matcher
/// (which expects ink-on-dark) sees the text as ink.
pub fn folded_label(frame: &Frame, minimap: &Minimap) -> Option<Gray> {
    if minimap.state != MinimapState::Folded {
        return None;
    }
    let bar = minimap.bar;
    // Controls sit immediately left of the badge: two 12 px buttons and gaps.
    // Anchor the text's right edge on the badge rather than the bar width, so a
    // longer name (wider bar) does not shift the bound.
    let text_right = minimap.badge.x - 34;
    let x0 = (bar.x + 2).max(0);
    let x1 = text_right.min(bar.x + bar.w);
    let y0 = (bar.y + 1).max(0);
    let y1 = bar.y + bar.h - 1;
    if x1 - x0 < 20 || y1 - y0 < 6 {
        return None;
    }
    let region = Region {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    };
    let gray = crop_gray(frame, region)?;
    // Invert: the folded bar draws black text on a light fill.
    let inverted = Gray {
        w: gray.w,
        h: gray.h,
        px: gray.px.iter().map(|pixel| 255 - *pixel).collect(),
    };
    let (min_x, min_y, max_x, max_y) = ink_bounds(&inverted, 128)?;
    let trimmed = Region {
        x: region.x + min_x,
        y: region.y + min_y,
        w: max_x - min_x + 1,
        h: max_y - min_y + 1,
    };
    let gray = crop_gray(frame, trimmed)?;
    Some(Gray {
        w: gray.w,
        h: gray.h,
        px: gray.px.iter().map(|pixel| 255 - *pixel).collect(),
    })
}

fn ink_bounds(gray: &Gray, threshold: u8) -> Option<(i32, i32, i32, i32)> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = -1i32;
    let mut max_y = -1i32;
    for y in 0..gray.h {
        for x in 0..gray.w {
            if gray.px[y * gray.w + x] > threshold {
                min_x = min_x.min(x as i32);
                min_y = min_y.min(y as i32);
                max_x = max_x.max(x as i32);
                max_y = max_y.max(y as i32);
            }
        }
    }
    if max_x < 0 {
        None
    } else {
        Some((min_x, min_y, max_x, max_y))
    }
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
    map_label_line_regions(frame).map(|lines| {
        lines
            .into_iter()
            .map(|(_, gray)| gray)
            .collect::<Vec<Gray>>()
    })
}

/// The minimap label's text lines with the region each one occupies. The reader
/// uses the line crops; the calibration window uses the regions to show and
/// store the exact boxes the detector selected.
pub fn map_label_line_regions(frame: &Frame) -> Option<Vec<(Region, Gray)>> {
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
            out.push((region, line));
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
            format!("{}{}", &text[..open], &text[close + 1..])
                .trim()
                .to_string()
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

    fn near_miss_digit() -> (Atlas, Segment) {
        let mut three = vec![0u8; 7 * 11];
        for y in 0..11 {
            three[y * 7 + 3] = 255;
        }
        let mut mask = three.clone();
        for y in 2..9 {
            for x in 1..6 {
                if (x + y) % 2 == 0 {
                    mask[y * 7 + x] = 255;
                }
            }
        }
        let glyph = |ch, bitmap| Glyph {
            ch,
            bitmap,
            cell: [0; CELL_LEN],
            w: 7,
            h: 11,
        };
        let atlas = Atlas {
            glyphs: vec![glyph('3', three), glyph('8', vec![255; 7 * 11])],
        };
        let segment = Segment {
            mask,
            cell: [0; CELL_LEN],
            x: 0,
            y: 0,
            w: 7,
            h: 11,
        };
        (atlas, segment)
    }

    #[test]
    fn readout_accepts_a_near_miss_digit() {
        let (atlas, segment) = near_miss_digit();
        assert!(atlas.match_segment_within(&segment, "38").is_none());
        let ranked = atlas
            .match_ranked(&segment, "38", READOUT_TOLERANCE)
            .expect("near miss");
        assert_eq!(ranked.ch, '3');
        assert!(
            ranked.distance > MATCH_THRESHOLD && ranked.distance <= READOUT_TOLERANCE,
            "{ranked:?}"
        );
        // Past the strict threshold, but nothing else is close: this is a read.
        assert!(!ranked.ambiguous(), "{ranked:?}");
    }

    /// A live Filtered `3` (`101,013[46.64%]`, 2026-11-21) against the seeded
    /// 7x11 `3` and `8` variants. With the one-pixel shift search the two were
    /// 0.2 apart and the read was refused; in place the `3` is clearly closer.
    #[test]
    fn a_near_tie_is_settled_by_the_in_place_match() {
        let bitmap = |rows: &[&str]| -> Vec<u8> {
            rows.iter()
                .flat_map(|row| row.bytes().map(|b| if b == b'#' { 255 } else { 0 }))
                .collect()
        };
        let variants: [(char, [&str; 11]); 6] = [
            (
                '3',
                [
                    ".#####.", ".#####.", "#.....#", "......#", "......#", "...###.", "......#",
                    "......#", "#.....#", ".#####.", ".#####.",
                ],
            ),
            (
                '3',
                [
                    ".#####.", "#######", "##...##", ".....##", ".....##", "...###.", ".....##",
                    ".....##", "##...##", "#######", ".#####.",
                ],
            ),
            (
                '3',
                [
                    ".#####.", "######.", "##...##", ".....##", "...####", "...###.", "...####",
                    ".....##", "##...##", "######.", ".#####.",
                ],
            ),
            (
                '8',
                [
                    ".#####.", ".#####.", "#.....#", "#.....#", "#.....#", ".#####.", "#.....#",
                    "#.....#", "#.....#", ".#####.", ".#####.",
                ],
            ),
            (
                '8',
                [
                    ".#####.", "#######", "##...##", "##...##", "##...##", ".#####.", "##...##",
                    "##...##", "##...##", "#######", ".#####.",
                ],
            ),
            (
                '8',
                [
                    ".#####.", ".#####.", "##...##", "##...##", "#######", ".#####.", "#######",
                    "##...##", "##...##", ".#####.", ".#####.",
                ],
            ),
        ];
        let atlas = Atlas {
            glyphs: variants
                .iter()
                .map(|(ch, rows)| Glyph {
                    ch: *ch,
                    bitmap: bitmap(rows),
                    cell: [0; CELL_LEN],
                    w: 7,
                    h: 11,
                })
                .collect(),
        };
        let segment = Segment {
            mask: bitmap(&[
                "..###..", ".#####.", "##...##", "#....##", ".....##", "..####.", ".....##",
                "#....##", "##...##", ".#####.", "..###..",
            ]),
            cell: [0; CELL_LEN],
            x: 0,
            y: 0,
            w: 7,
            h: 11,
        };
        let ranked = atlas
            .match_ranked(&segment, "38", READOUT_TOLERANCE)
            .expect("a read");
        assert_eq!(ranked.ch, '3', "{ranked:?}");
        assert!(!ranked.ambiguous(), "{ranked:?}");
    }

    #[test]
    fn level_reads_a_near_miss_digit() {
        let (atlas, segment) = near_miss_digit();
        let (text, confidence) = atlas.read_digit_shapes(vec![segment]).expect("level read");
        assert_eq!(text, "3");
        assert!(confidence > 0.7);
    }

    /// A deterministic 7x11 ink shape, the size the Filtered EXP digits are drawn
    /// at. High-frequency on purpose: a one-pixel shift changes many pixels, so the
    /// aligned comparison is always the best one.
    fn readout_base() -> Vec<u8> {
        (0..7 * 11)
            .map(|index| {
                if (index % 7 + index / 7) % 3 != 0 {
                    255
                } else {
                    0
                }
            })
            .collect()
    }

    /// The bracket family: the complement of [`readout_base`] at the same size, so a
    /// synthetic bracket and a synthetic digit are as far apart as the real ones
    /// are. Without that, a shape the current stage does not allow can quietly match
    /// the neighbouring character inside the tolerance and the test proves nothing.
    fn readout_bracket_base() -> Vec<u8> {
        (0..7 * 11)
            .map(|index| {
                if (index % 7 + index / 7) % 3 == 0 {
                    255
                } else {
                    0
                }
            })
            .collect()
    }

    /// `base` with `count` more ink pixels cleared, starting after `seed`. One
    /// cleared pixel of a `w*h` shape is `255/(w*h)` of the native distance, so a
    /// test can put a glyph a chosen distance from a shape — and two glyphs built
    /// with the same arguments are exactly the same distance from it.
    fn with_holes(base: &[u8], count: usize, seed: usize) -> Vec<u8> {
        let mut out = base.to_vec();
        let mut left = count;
        let mut index = seed % base.len();
        while left > 0 {
            index = (index + 1) % base.len();
            if out[index] == 255 {
                out[index] = 0;
                left -= 1;
            }
        }
        out
    }

    /// An atlas of 7x11 characters, one shape each: exact for its own position and,
    /// for the digits, far enough from a bare [`readout_base`] shape that only the
    /// glyphs a test adds itself can be candidates for one.
    fn readout_font(chars: &str) -> (Atlas, Vec<(char, Vec<u8>)>) {
        let mut atlas = Atlas::default();
        let mut shapes = Vec::new();
        for (index, ch) in chars.chars().enumerate() {
            let bracket = ch == '[' || ch == ']';
            let (base, holes) = if bracket {
                (readout_bracket_base(), 2 + (index % 2) * 2)
            } else {
                (readout_base(), 10 + index * 2)
            };
            // Few holes, spread by the seed: enough to be a distinct shape, few
            // enough that no column of the stamped bitmap loses its ink and the
            // segmenter cuts it in two.
            let bitmap = with_holes(&base, holes, index * 5);
            atlas.learn_glyph(ch, bitmap.clone(), 7, 11);
            shapes.push((ch, bitmap));
        }
        (atlas, shapes)
    }

    fn font_shape(shapes: &[(char, Vec<u8>)], ch: char) -> Vec<u8> {
        shapes
            .iter()
            .find(|(font_ch, _)| *font_ch == ch)
            .unwrap_or_else(|| panic!("no shape for {ch}"))
            .1
            .clone()
    }

    /// The shapes drawn as one readout line: left to right with a blank column
    /// between them, like the client's glyph run.
    fn draw_readout(shapes: &[Vec<u8>], w: usize, h: usize) -> Gray {
        const GAP: usize = 2;
        let width = (w + GAP) * shapes.len() + GAP;
        let mut gray = canvas(width, h + 4);
        for (index, shape) in shapes.iter().enumerate() {
            let x0 = GAP + index * (w + GAP);
            for y in 0..h {
                for x in 0..w {
                    if shape[y * w + x] > 0 {
                        gray.px[(y + 2) * width + x0 + x] = 230;
                    }
                }
            }
        }
        gray
    }

    #[test]
    fn readout_rejects_a_digit_two_characters_explain_equally() {
        // The Filtered `8` of the 26,487 HUD is drawn in a shape the seeded atlas
        // does not hold: its `6` and `9` are both near misses of it, a point or two
        // apart. Taking the closer of the two is what turned the readout into the
        // perfectly plausible `26,467`.
        let (mut atlas, font) = readout_font("1[%]");
        let base = readout_base();
        let tied = with_holes(&base, 2, 3);
        atlas.learn_glyph('6', tied.clone(), 7, 11);
        atlas.learn_glyph('9', tied, 7, 11);
        atlas.learn_glyph('8', with_holes(&base, 12, 4), 7, 11);

        let drawn = vec![
            font_shape(&font, '1'),
            font_shape(&font, '['),
            base,
            font_shape(&font, '%'),
            font_shape(&font, ']'),
        ];
        let gray = draw_readout(&drawn, 7, 11);

        // The tie is real, not an artefact of the threshold: both characters are the
        // same distance from the shape.
        let mut found = segments(&gray);
        let digit = found.remove(2);
        let ranked = atlas
            .match_ranked(&digit, "0123456789", READOUT_TOLERANCE)
            .expect("candidates");
        assert!(ranked.ambiguous(), "{ranked:?}");
        assert_eq!(ranked.distance, ranked.rival_distance, "{ranked:?}");

        let error = atlas
            .recognize_readout(&gray)
            .expect_err("a tie is not a read");
        assert!(error.contains("too close to tell apart"), "{error}");
    }

    #[test]
    fn readout_rejects_a_near_miss_its_rival_also_explains() {
        // The other half of the Filtered problem: the live `3` is drawn past the
        // strict threshold (it measures 31) and its `8` is barely further away. Two
        // characters that explain a near miss equally well are not a read either —
        // the tolerant step has to refuse it.
        let (mut atlas, font) = readout_font("[]%");
        let base = readout_base();
        // The EXP number is exact for its own position and too far from a
        // Filtered-shaped digit to compete inside the percentage, so the only
        // candidates there are the tied pair.
        let number = with_holes(&base, 26, 11);
        atlas.learn_glyph('7', number.clone(), 7, 11);
        let near = with_holes(&base, 12, 6);
        atlas.learn_glyph('3', near.clone(), 7, 11);
        atlas.learn_glyph('8', near, 7, 11);

        let drawn = vec![
            number,
            font_shape(&font, '['),
            base,
            font_shape(&font, '%'),
            font_shape(&font, ']'),
        ];
        let gray = draw_readout(&drawn, 7, 11);

        let mut found = segments(&gray);
        let digit = found.remove(2);
        let ranked = atlas
            .match_ranked(&digit, "0123456789", READOUT_TOLERANCE)
            .expect("candidates");
        assert!(
            ranked.distance > MATCH_THRESHOLD,
            "the strict step must not accept it: {ranked:?}"
        );
        assert!(ranked.ambiguous(), "{ranked:?}");

        let error = atlas
            .recognize_readout(&gray)
            .expect_err("a tie is not a read");
        assert!(error.contains("too close to tell apart"), "{error}");
    }

    #[test]
    fn ambiguity_margin_sits_where_the_corpus_measured_it() {
        // The harness cannot express a gap under 3.3 through the atlas (one cleared
        // pixel of a 7x11 shape is 255/77), which is the regime the rule operates
        // in, so the boundary is pinned on the numbers the margin was chosen from.
        let ranked = |distance: f64, rival_distance: f64| Ranked {
            ch: '3',
            distance,
            rival: '8',
            rival_distance,
        };

        // Reads the corpus keeps: the 36,818 `3` is 31.6 with its `8` at 33.4
        // (1.8 apart) and its `9` is 24.3 with a `0` at 26.5 (2.2 apart).
        assert!(!ranked(31.6, 33.4).ambiguous());
        assert!(!ranked(24.3, 26.5).ambiguous());
        // The 26,487 `13.58%` `3` is 30.8 with its `8` at 31.6: 0.8 apart, the tie
        // the rule refuses.
        assert!(ranked(30.8, 31.6).ambiguous());
        // The boundary is the margin itself: exactly `AMBIGUITY_MARGIN` apart is
        // still a read, anything under it is a tie.
        assert_eq!(AMBIGUITY_MARGIN, 1.0);
        assert!(!ranked(30.0, 31.0).ambiguous());
        assert!(ranked(30.0, 30.999).ambiguous());
    }

    /// Two 7x11 digits drawn as one wide shape with no blank column at the seam, so
    /// the segmenter keeps them together and only a split can read them.
    fn touching_pair() -> Vec<u8> {
        let mut wide = with_holes(&readout_base(), 10, 0);
        wide.extend_from_slice(&with_holes(&readout_base(), 12, 5));
        wide
    }

    /// [`draw_readout`] for a line whose shapes are not all the same size.
    fn draw_shapes(shapes: &[(&[u8], usize, usize)]) -> Gray {
        const GAP: usize = 2;
        let width = shapes.iter().map(|(_, w, _)| w + GAP).sum::<usize>() + GAP;
        let height = shapes.iter().map(|(_, _, h)| *h).max().unwrap_or(0) + 4;
        let mut gray = canvas(width, height);
        let mut x0 = GAP;
        for (shape, w, h) in shapes {
            for y in 0..*h {
                for x in 0..*w {
                    if shape[y * w + x] > 0 {
                        gray.px[(y + 2) * width + x0 + x] = 230;
                    }
                }
            }
            x0 += w + GAP;
        }
        gray
    }

    /// The one wide shape of [`touching_pair`], as the segmenter sees it.
    fn wide_pair() -> Segment {
        let mut found = segments(&draw_shapes(&[(&touching_pair(), 14, 11)]));
        assert_eq!(found.len(), 1, "the pair must not segment apart: {found:?}");
        found.remove(0)
    }

    #[test]
    fn a_touching_pair_splits_when_it_explains_the_shape_better() {
        // The strict step is vetoed — two characters explain the wide shape the same
        // way, a few points from it — and the split into the two digits explains it
        // exactly, so the split is taken: a vetoed single glyph must not cost the
        // digits the shape is made of.
        let (mut atlas, font) = readout_font("3[]%");
        let wide = wide_pair();
        let (w, h) = (wide.w, wide.h);
        let (left, right) = (split_part(&wide, 0, 7), split_part(&wide, 7, w - 7));
        atlas.learn_glyph('1', left.mask.clone(), 7, h);
        atlas.learn_glyph('2', right.mask.clone(), w - 7, h);
        let tied = with_holes(&wide.mask, 2, 3);
        atlas.learn_glyph('6', tied.clone(), w, h);
        atlas.learn_glyph('9', tied, w, h);

        // The tie is real: both characters are the same distance from the shape, and
        // close enough for the strict step to find them at all.
        let ranked = atlas
            .match_ranked(&wide, "0123456789,[", MATCH_THRESHOLD)
            .expect("the tie is inside the strict threshold");
        assert!(ranked.ambiguous(), "{ranked:?}");
        assert_eq!(ranked.distance, ranked.rival_distance, "{ranked:?}");

        let parts = atlas
            .split_segment(&wide, "0123456789,[", SplitPolicy::Readout)
            .expect("the touching pair splits");
        assert_eq!(parts.iter().map(|(_, ch, _)| *ch).collect::<String>(), "12");
        let total: f64 = parts.iter().map(|(_, _, best)| best).sum();
        assert!(
            total < ranked.distance,
            "the split explains the shape better: {total} vs {}",
            ranked.distance
        );

        let gray = draw_shapes(&[
            (&wide.mask, w as usize, h as usize),
            (&font_shape(&font, '['), 7, 11),
            (&font_shape(&font, '3'), 7, 11),
            (&font_shape(&font, '%'), 7, 11),
            (&font_shape(&font, ']'), 7, 11),
        ]);
        let (text, _) = atlas.recognize_readout(&gray).expect("split read");
        assert_eq!(text, "12[3%]");
    }

    #[test]
    fn a_touching_pair_is_refused_when_the_vetoed_glyph_explains_it_as_well() {
        // The other side of the same rule. Here two characters explain the wide shape
        // exactly, so the split explains it no better than the single reading:
        // cutting the shape in half would be a second guess, and the level is rejected
        // instead of read as two digits.
        let mut atlas = Atlas::default();
        let wide = wide_pair();
        let (w, h) = (wide.w, wide.h);
        let (left, right) = (split_part(&wide, 0, 7), split_part(&wide, 7, w - 7));
        atlas.learn_glyph('1', left.mask.clone(), 7, h);
        atlas.learn_glyph('2', right.mask.clone(), w - 7, h);
        atlas.learn_glyph('6', wide.mask.clone(), w, h);
        atlas.learn_glyph('9', wide.mask.clone(), w, h);

        assert!(
            atlas
                .split_segment(&wide, "0123456789", SplitPolicy::Readout)
                .is_some(),
            "the halves themselves still read"
        );

        let error = atlas
            .read_digit_shapes(vec![wide])
            .expect_err("a tie is not a level");
        assert!(error.contains("too close to tell apart"), "{error}");
    }

    #[test]
    fn readout_rejects_a_missing_closing_bracket() {
        // A clipped box: the percentage and its `[` are drawn, the `]` is not.
        // Synthesising the bracket (or returning the truncated text) would report a
        // readout the client never drew.
        let (atlas, font) = readout_font("12[%]");
        let drawn = vec![
            font_shape(&font, '1'),
            font_shape(&font, '2'),
            font_shape(&font, '['),
            font_shape(&font, '%'),
        ];
        let gray = draw_readout(&drawn, 7, 11);

        let error = atlas
            .recognize_readout(&gray)
            .expect_err("a readout without its ']' is not a read");
        assert!(error.contains("']'"), "{error}");
    }

    #[test]
    fn readout_rejects_a_bracket_where_a_digit_belongs() {
        // "12[3]%" would drop the `]` out of order if the stage were a hint rather
        // than a grammar: the bracket has to be refused at the percentage stage, and
        // the readout with it, instead of silently reading `12[3%`.
        let (atlas, font) = readout_font("123[%]");
        let drawn = vec![
            font_shape(&font, '1'),
            font_shape(&font, '2'),
            font_shape(&font, '['),
            font_shape(&font, '3'),
            font_shape(&font, ']'),
            font_shape(&font, '%'),
        ];
        let gray = draw_readout(&drawn, 7, 11);

        let error = atlas
            .recognize_readout(&gray)
            .expect_err("a bracket inside the percentage is not a read");
        assert!(error.contains("cannot be read"), "{error}");
    }

    /// A 7x11 shape nothing in the readout font can match: horizontal bars, so no
    /// column is blank (the segmenter would cut it in two) and every glyph is far
    /// more than [`READOUT_TOLERANCE`] away.
    fn readout_junk() -> Vec<u8> {
        (0..7 * 11)
            .map(|index| if (index / 7) % 2 == 0 { 255 } else { 0 })
            .collect()
    }

    #[test]
    fn readout_ignores_junk_beside_the_readout_but_not_inside_it() {
        // A box drawn by hand also catches the `EXP` label on its left. That junk is
        // dropped, exactly as before, while a shape the reader cannot explain
        // *between* two read glyphs is a lost digit and rejects the read.
        let (atlas, font) = readout_font("123[]%");
        let junk = readout_junk();
        let mut drawn = vec![junk.clone()];
        for ch in "12[3%]".chars() {
            drawn.push(font_shape(&font, ch));
        }
        let gray = draw_readout(&drawn, 7, 11);
        assert_eq!(
            atlas.recognize_readout(&gray).map(|(text, _)| text),
            Ok("12[3%]".to_string())
        );

        let mut holed = Vec::new();
        for (index, ch) in "12[3%]".chars().enumerate() {
            if index == 4 {
                holed.push(junk.clone());
            }
            holed.push(font_shape(&font, ch));
        }
        let gray = draw_readout(&holed, 7, 11);
        let error = atlas
            .recognize_readout(&gray)
            .expect_err("a hole inside the readout is a dropped digit");
        assert!(error.contains("cannot be read"), "{error}");
    }

    #[test]
    fn level_read_rejects_a_dropped_digit() {
        // `35` with the `3` unreadable must not come back as a level of `5`: that
        // parses, fits the profile and puts a wrong level on the overlay.
        let (atlas, font) = readout_font("35");
        let gray = draw_readout(&[readout_junk(), font_shape(&font, '5')], 7, 11);
        let found = segments(&gray);
        let error = atlas
            .read_digit_shapes(found)
            .expect_err("a level with a dropped digit is not a read");
        assert!(error.contains("cannot be read"), "{error}");
    }

    #[test]
    fn readout_structure_is_the_exp_grammar() {
        for text in ["10,105[91.53%]", "0[0.00%]", "1,234,567[100%]", "1[2%]"] {
            assert_eq!(validate_readout(text), Ok(()), "{text}");
        }
        for text in [
            "10,105[91.53%",   // no closing bracket
            "10,105 91.53%]",  // no opening bracket
            "10,105[91.53]%",  // bracket closed before the percent
            "10,105[91.53%]x", // trailing characters
            "10,105[91.53%x]", // characters between the '%' and the ']'
            "10,105[91.53%%]", // a second '%' after the percentage
            "10,105[91.53%1]", // a digit between the '%' and the ']'
            "10,105[91[.53%]", // a second opening bracket
            "[91.53%]",        // no EXP number
            "10,105[]",        // no percentage
            "10,105[91..53%]", // two periods
            "10,105[91.53%]]", // a second closing bracket
            "10,105,91.53%]",  // a comma in the percentage
            ",105[91.53%]",    // a leading comma
        ] {
            assert!(validate_readout(text).is_err(), "{text} should be rejected");
        }
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
        let stored = crate::settings::StoredAtlas {
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
            base.iter()
                .map(|bit| if *bit > 0 { 255 } else { 0 })
                .collect(),
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
        assert_eq!(
            label_similarity("Free Market <15>", "Free Market <16>"),
            1.0
        );

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
        // No brackets, no readout: the reader rejects those, and parsing must not
        // accept a string the reader would never produce.
        assert_eq!(parse_exp_text("10,105 91.53%"), None);
        assert_eq!(parse_exp_text("10,105[91.53%"), None);
        assert_eq!(parse_exp_text("no numbers"), None);
    }

    #[test]
    fn parses_the_level() {
        assert_eq!(parse_level("71"), Some(71));
        assert_eq!(parse_level("Lv 99"), Some(99));
        assert_eq!(parse_level("--"), None);
    }

    #[test]
    fn white_key_drops_coloured_tile_and_keeps_digits() {
        // A dark status bar with a saturated orange level tile and two white
        // digits. The luma bright floor sees the tile as ink too; the white key
        // must not, so the two digits segment on their own.
        let mut frame = Frame {
            width: 20,
            height: 12,
            rgba: vec![0; 20 * 12 * 4],
        };
        let put = |frame: &mut Frame, x: usize, y: usize, [b, g, r]: [u8; 3]| {
            let index = (y * 20 + x) * 4;
            frame.rgba[index..index + 4].copy_from_slice(&[b, g, r, 255]);
        };
        for y in 0..12 {
            for x in 0..20 {
                put(&mut frame, x, y, [32, 32, 32]);
            }
        }
        for y in 1..11 {
            for x in 1..19 {
                put(&mut frame, x, y, [30, 140, 240]);
            }
        }
        for x0 in [3usize, 11] {
            for y in 3..8 {
                for x in x0..x0 + 3 {
                    put(&mut frame, x, y, [255, 255, 255]);
                }
            }
        }

        let region = Region {
            x: 0,
            y: 0,
            w: 20,
            h: 12,
        };
        let keyed = crop_white_key(&frame, region).expect("crop");
        assert_eq!(keyed.px[0], 0, "dark panel is background");
        assert_eq!(keyed.px[2 * 20 + 2], 0, "orange tile is background");
        assert_eq!(keyed.px[4 * 20 + 4], 255, "white digit is ink");
        assert_eq!(
            bright_segments(&keyed, 127).len(),
            2,
            "the two digits survive; the tile does not"
        );
    }
    #[test]
    fn folded_bar_wins_over_a_scenery_titlebar_fallback() {
        // A real folded bar at the top-left and a slightly larger scenery badge
        // at the bottom-right. The scenery badge passes the weak titlebar
        // fallback (the frame background is dark), so the scan must not return it
        // before checking the real folded body.
        let mut frame = Frame {
            width: 600,
            height: 600,
            rgba: vec![0; 600 * 600 * 4],
        };
        for pixel in frame.rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let paint =
            |frame: &mut Frame, x0: usize, y0: usize, x1: usize, y1: usize, rgb: [u8; 3]| {
                for y in y0..y1 {
                    for x in x0..x1 {
                        let index = (y * 600 + x) * 4;
                        frame.rgba[index..index + 4]
                            .copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                    }
                }
            };

        // Folded body and its WORLD badge.
        paint(&mut frame, 20, 20, 220, 40, [221, 221, 221]);
        paint(&mut frame, 170, 25, 204, 35, [230, 170, 40]);
        // Larger orange blob in the scenery; the dark frame around it satisfies
        // the titlebar fallback's ink/body checks.
        paint(&mut frame, 400, 400, 436, 412, [230, 170, 40]);

        let minimap = detect_minimap(&frame).expect("folded minimap");
        assert_eq!(minimap.state, MinimapState::Folded);
        assert_eq!(
            (minimap.badge.x, minimap.badge.y),
            (170, 25),
            "the real folded badge must win"
        );
    }
}
