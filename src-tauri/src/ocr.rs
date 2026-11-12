use crate::capture::Frame;
use crate::settings::Region;

pub const CELL_W: usize = 12;
pub const CELL_H: usize = 16;
pub const CELL_LEN: usize = CELL_W * CELL_H;

const MATCH_THRESHOLD: f64 = 30.0;
const MATCH_MARGIN: f64 = 12.0;
const MIN_GLYPH_PIXELS: usize = 1;

pub type Cell = [u8; CELL_LEN];

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub cell: Cell,
    pub w: u32,
    pub h: u32,
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

pub fn segments(gray: &Gray) -> Vec<Segment> {
    let mask = foreground(gray);
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
            w: (end - start) as u32,
            h: (bottom - top) as u32,
        });
    }

    out
}

fn cell_distance(a: &Cell, b: &Cell) -> f64 {
    let total: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum();
    total as f64 / CELL_LEN as f64
}

const SIZE_PENALTY: f64 = 18.0;

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
        let found = segments(gray);
        let chars: Vec<char> = text.chars().filter(|ch| !ch.is_whitespace()).collect();

        if chars.is_empty() {
            return Err("type the text shown in the box".into());
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
            if let Some(existing) = self
                .glyphs
                .iter_mut()
                .find(|known| known.ch == *ch && known.w == glyph.w && known.h == glyph.h)
            {
                *existing = glyph;
            } else {
                self.glyphs.push(glyph);
            }
        }

        Ok(chars)
    }

    pub fn recognize(&self, gray: &Gray) -> Result<(String, f64), String> {
        if self.glyphs.is_empty() {
            return Err("no glyphs learned yet".into());
        }

        let found = segments(gray);
        if found.is_empty() {
            return Err("nothing readable in the box".into());
        }

        let count = found.len();
        let mut text = String::with_capacity(count);
        let mut confidence = 0.0;

        for segment in found {
            let mut best = ('?', f64::MAX);
            let mut second = f64::MAX;

            for glyph in &self.glyphs {
                let size_penalty = SIZE_PENALTY
                    * ((segment.w as f64 - glyph.w as f64).abs()
                        + (segment.h as f64 - glyph.h as f64).abs());
                let distance = cell_distance(&segment.cell, &glyph.cell) + size_penalty;

                if distance < best.1 {
                    second = best.1;
                    best = (glyph.ch, distance);
                } else if distance < second {
                    second = distance;
                }
            }

            if best.1 > MATCH_THRESHOLD {
                return Err("unrecognized glyph — learn the digits again".into());
            }
            if best.1 > 6.0 && second < f64::MAX && second - best.1 < MATCH_MARGIN {
                return Err("ambiguous glyph — learn more digits".into());
            }

            text.push(best.0);
            confidence += (1.0 - best.1 / 255.0).max(0.0);
        }

        Ok((text, confidence / count as f64))
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
const TITLE_BAR_H: i32 = 20;
const ICON_GUTTER: i32 = 40;
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
    let x_start = (panel.x + ICON_GUTTER).max(0) as usize;
    let x_end = ((panel.x + panel.w) as usize).min(w);
    // Skip the panel's title bar; the map name is drawn below it.
    let y_start = (panel.y + TITLE_BAR_H).max(0) as usize;
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
