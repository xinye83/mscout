//! The game's own 1x picture behind a captured client frame.
//!
//! Up to 768 lines tall the client draws its UI at 1x, and every UI Size gives
//! the same pixels. Above that (measured on 1920x1080, 1920x1200 and 2560x1440
//! captures, `testdata/frames`):
//!
//! - **Default Ratio** draws a 768-line picture, [`picture_width`] wide, and
//!   stretches it bilinearly to the client. The stretch is linear and its
//!   geometry is known, so least squares undoes it: the recovered HUD and
//!   minimap match the 1366x768 capture to within a grey level.
//! - **Filter Applied** is the same stretch followed by a sharpen. Undoing a
//!   cross sharpen and then the stretch recovers the 1x picture closely (about
//!   98.5% of text pixels on the right side of the ink threshold), not exactly:
//!   the filter is not fully linear and its alignment drifts with the size.
//! - **Ideal Ratio** draws the UI at a whole scale with no smoothing: 1x at
//!   1080 and 1200 lines, 2x at 1440. One pixel per block is the 1x picture,
//!   exactly.
//!
//! Only the regions a reader asks for are recovered ([`to_native`]), so the
//! cost follows the HUD strip and the minimap, not the client size.

// Not wired into the readers yet; the corpus tests exercise it.
#![cfg_attr(not(test), allow(dead_code))]

use crate::capture::Frame;
use crate::settings::UiMode;

/// The tallest picture the client draws at 1x; taller clients are scaled.
pub const NATIVE_HEIGHT: u32 = 768;

/// How a client frame relates to the 1x picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scaling {
    /// The client is the 1x picture.
    Native,
    /// Every 1x pixel is a `k`x`k` block (Ideal Ratio on tall clients).
    Blocks(u32),
    /// A `width`x768 picture stretched bilinearly to the client; `sharpen` is
    /// the cross-sharpen weight Filter Applied adds after the stretch.
    Stretched { width: u32, sharpen: Option<f32> },
}

/// The width of the 768-line picture Default Ratio stretches to fill
/// `width`x`height`: the nearest even width with the client's shape (1366 at
/// 1920x1080 and 2560x1440, 1228 at 1920x1200), which keeps the centred HUD on
/// whole pixels.
pub fn picture_width(width: u32, height: u32) -> u32 {
    if height == 0 {
        return 0;
    }
    let half = (u64::from(width) * u64::from(NATIVE_HEIGHT / 2) + u64::from(height / 2))
        / u64::from(height);
    (2 * half) as u32
}

impl Scaling {
    /// The scaling a client of this size and UI Size draws with.
    pub fn for_client(width: u32, height: u32, mode: UiMode) -> Scaling {
        if height <= NATIVE_HEIGHT {
            return Scaling::Native;
        }
        match mode {
            // 1080 and 1200 lines draw at 1x and 1440 at 2x; `height / 720`
            // fits all three. Sizes in between are unmeasured.
            UiMode::Ideal => match height / 720 {
                0 | 1 => Scaling::Native,
                k => Scaling::Blocks(k),
            },
            UiMode::Default => Scaling::Stretched {
                width: picture_width(width, height),
                sharpen: None,
            },
            // Fitted weights: 0.13 at 1080 and 1200 lines, 0.16 at 1440.
            UiMode::Filtered => Scaling::Stretched {
                width: picture_width(width, height),
                sharpen: Some(if height >= 1400 { 0.16 } else { 0.13 }),
            },
        }
    }

    /// The 1x picture's size for a client of `width`x`height`.
    pub fn native_size(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Scaling::Native => (width, height),
            Scaling::Blocks(k) => (width / k, height / k),
            Scaling::Stretched { width: w, .. } => (w, NATIVE_HEIGHT),
        }
    }

    /// Client pixels per 1x pixel, horizontally and vertically.
    fn scale(self, width: u32, height: u32) -> (f64, f64) {
        let (nw, nh) = self.native_size(width, height);
        (
            f64::from(width) / f64::from(nw),
            f64::from(height) / f64::from(nh),
        )
    }
}

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The 1x picture behind `frame`, recovered only inside `regions` (1x
/// coordinates); everything else is black. Regions are clipped to the picture.
pub fn to_native(frame: &Frame, scaling: Scaling, regions: &[Rect]) -> Frame {
    let (nw, nh) = scaling.native_size(frame.width, frame.height);
    let mut out = Frame {
        width: nw,
        height: nh,
        rgba: vec![0; (nw as usize) * (nh as usize) * 4],
    };
    for pixel in out.rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    if let Scaling::Native = scaling {
        out.rgba.clone_from(&frame.rgba);
        return out;
    }
    for region in regions {
        let x1 = (region.x + region.w).min(nw);
        let y1 = (region.y + region.h).min(nh);
        if region.x >= x1 || region.y >= y1 {
            continue;
        }
        let region = Rect {
            x: region.x,
            y: region.y,
            w: x1 - region.x,
            h: y1 - region.y,
        };
        match scaling {
            Scaling::Native => unreachable!(),
            Scaling::Blocks(k) => copy_blocks(frame, &mut out, region, k),
            Scaling::Stretched { sharpen, .. } => {
                let (sx, sy) = scaling.scale(frame.width, frame.height);
                unstretch(frame, &mut out, region, sx, sy, sharpen);
            }
        }
    }
    out
}

fn copy_blocks(frame: &Frame, out: &mut Frame, region: Rect, k: u32) {
    for y in region.y..region.y + region.h {
        for x in region.x..region.x + region.w {
            let (cx, cy) = (x * k, y * k);
            if cx >= frame.width || cy >= frame.height {
                continue;
            }
            let src = ((cy * frame.width + cx) * 4) as usize;
            let dst = ((y * out.width + x) * 4) as usize;
            out.rgba[dst..dst + 3].copy_from_slice(&frame.rgba[src..src + 3]);
        }
    }
}

/// Extra 1x pixels solved on each side of a region and then dropped: the
/// solve is least constrained at its edges.
const MARGIN: u32 = 3;

/// Where client pixel `c` samples the 1x picture: the 1x index left of it and
/// the weight of the one right of it.
fn source(c: u32, scale: f64) -> (i64, f64) {
    let u = (f64::from(c) + 0.5) / scale - 0.5;
    let f = u.floor();
    (f as i64, u - f)
}

/// The client pixels whose samples fall inside 1x pixels `n0..n1`.
fn client_span(n0: i64, n1: i64, scale: f64, limit: u32) -> (u32, u32) {
    let first = (((n0 as f64) + 0.5) * scale - 0.5).ceil().max(0.0) as u32;
    let last = ((((n1 - 1) as f64) + 0.5) * scale - 0.5).floor().max(0.0) as u32;
    (first.min(limit), (last + 1).min(limit))
}

/// Least-squares recovery of 1x pixels `n0..n1` along one axis from client
/// samples `c0..c1` of one line. Every sample mixes two neighbouring 1x
/// pixels, so the normal equations are tridiagonal.
struct Axis {
    n0: i64,
    n: usize,
    samples: Vec<(usize, f64, f64)>,
    lower: Vec<f64>,
    diag: Vec<f64>,
    upper: Vec<f64>,
}

impl Axis {
    fn new(n0: i64, n1: i64, c0: u32, c1: u32, scale: f64) -> Axis {
        let n = (n1 - n0) as usize;
        let mut samples = Vec::new();
        let (mut lower, mut diag, mut upper) = (vec![0.0; n], vec![1e-6; n], vec![0.0; n]);
        for c in c0..c1 {
            let (f, fr) = source(c, scale);
            let i = f - n0;
            if i < 0 || i + 1 >= n as i64 {
                samples.push((usize::MAX, 0.0, 0.0));
                continue;
            }
            let i = i as usize;
            let (a, b) = (1.0 - fr, fr);
            diag[i] += a * a;
            diag[i + 1] += b * b;
            upper[i] += a * b;
            lower[i + 1] += a * b;
            samples.push((i, a, b));
        }
        Axis {
            n0,
            n,
            samples,
            lower,
            diag,
            upper,
        }
    }

    /// Solve for the 1x line behind `values` (one per client sample).
    fn solve(&self, values: &[f64]) -> Vec<f64> {
        let mut rhs = vec![0.0; self.n];
        for (&(i, a, b), &v) in self.samples.iter().zip(values) {
            if i == usize::MAX {
                continue;
            }
            rhs[i] += a * v;
            rhs[i + 1] += b * v;
        }
        // Thomas algorithm.
        let mut c = vec![0.0; self.n];
        let mut d = vec![0.0; self.n];
        for i in 0..self.n {
            let m = self.diag[i] - if i > 0 { self.lower[i] * c[i - 1] } else { 0.0 };
            c[i] = self.upper[i] / m;
            d[i] = (rhs[i] - if i > 0 { self.lower[i] * d[i - 1] } else { 0.0 }) / m;
        }
        for i in (0..self.n.saturating_sub(1)).rev() {
            d[i] -= c[i] * d[i + 1];
        }
        d
    }
}

fn unstretch(frame: &Frame, out: &mut Frame, region: Rect, sx: f64, sy: f64, sharpen: Option<f32>) {
    let (nx0, ny0) = (
        i64::from(region.x) - i64::from(MARGIN),
        i64::from(region.y) - i64::from(MARGIN),
    );
    let (nx1, ny1) = (
        i64::from(region.x + region.w + MARGIN),
        i64::from(region.y + region.h + MARGIN),
    );
    let (cx0, cx1) = client_span(nx0, nx1, sx, frame.width);
    let (cy0, cy1) = client_span(ny0, ny1, sy, frame.height);
    if cx0 >= cx1 || cy0 >= cy1 {
        return;
    }
    let (cw, ch) = ((cx1 - cx0) as usize, (cy1 - cy0) as usize);
    let xs = Axis::new(nx0, nx1, cx0, cx1, sx);
    let ys = Axis::new(ny0, ny1, cy0, cy1, sy);
    for channel in 0..3 {
        let mut plane = vec![0.0f64; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                let i = (((cy0 as usize + y) * frame.width as usize) + cx0 as usize + x) * 4;
                plane[y * cw + x] = f64::from(frame.rgba[i + channel]);
            }
        }
        if let Some(w) = sharpen {
            plane = unsharpen(&plane, cw, ch, f64::from(w));
        }
        // Rows first: each client row becomes a 1x-wide row.
        let mut rows = vec![0.0f64; ch * xs.n];
        for y in 0..ch {
            let line = xs.solve(&plane[y * cw..(y + 1) * cw]);
            rows[y * xs.n..(y + 1) * xs.n].copy_from_slice(&line);
        }
        // Then columns.
        let mut column = vec![0.0f64; ch];
        for x in 0..xs.n {
            for y in 0..ch {
                column[y] = rows[y * xs.n + x];
            }
            let line = ys.solve(&column);
            let nx = xs.n0 + x as i64;
            if nx < i64::from(region.x) || nx >= i64::from(region.x + region.w) {
                continue;
            }
            for (j, value) in line.iter().enumerate() {
                let ny = ys.n0 + j as i64;
                if ny < i64::from(region.y) || ny >= i64::from(region.y + region.h) {
                    continue;
                }
                let o = ((ny as u32 * out.width + nx as u32) * 4) as usize;
                out.rgba[o + channel] = value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Undo `(x - w * (up + down + left + right)) / (1 - 4w)`. The operator's
/// eigenvalues lie in `[1, (1 + 4w) / (1 - 4w)]`, so a damped Jacobi step of
/// about `2 / (1 + max)` shrinks every error component by at least half.
fn unsharpen(plane: &[f64], w: usize, h: usize, weight: f64) -> Vec<f64> {
    let max = (1.0 + 4.0 * weight) / (1.0 - 4.0 * weight);
    let step = 2.0 / (1.0 + max);
    let mut x = plane.to_vec();
    let mut next = vec![0.0; plane.len()];
    let at = |v: &[f64], i: isize, j: isize| -> f64 {
        let i = i.clamp(0, w as isize - 1) as usize;
        let j = j.clamp(0, h as isize - 1) as usize;
        v[j * w + i]
    };
    for _ in 0..12 {
        for j in 0..h as isize {
            for i in 0..w as isize {
                let cross =
                    at(&x, i - 1, j) + at(&x, i + 1, j) + at(&x, i, j - 1) + at(&x, i, j + 1);
                let sharpened = (at(&x, i, j) - weight * cross) / (1.0 - 4.0 * weight);
                let k = j as usize * w + i as usize;
                next[k] = x[k] + step * (plane[k] - sharpened);
            }
        }
        std::mem::swap(&mut x, &mut next);
    }
    x
}

/// Map a 1x rectangle to the client pixels it covers.
pub fn to_client(rect: Rect, scaling: Scaling, width: u32, height: u32) -> Rect {
    let (sx, sy) = scaling.scale(width, height);
    let x0 = (f64::from(rect.x) * sx).floor() as u32;
    let y0 = (f64::from(rect.y) * sy).floor() as u32;
    let x1 = ((f64::from(rect.x + rect.w) * sx).ceil() as u32).min(width);
    let y1 = ((f64::from(rect.y + rect.h) * sy).ceil() as u32).min(height);
    Rect {
        x: x0,
        y: y0,
        w: x1.saturating_sub(x0),
        h: y1.saturating_sub(y0),
    }
}

/// Map a client rectangle to the 1x pixels it covers.
pub fn from_client(rect: Rect, scaling: Scaling, width: u32, height: u32) -> Rect {
    let (sx, sy) = scaling.scale(width, height);
    let x0 = (f64::from(rect.x) / sx).floor() as u32;
    let y0 = (f64::from(rect.y) / sy).floor() as u32;
    let x1 = (f64::from(rect.x + rect.w) / sx).ceil() as u32;
    let y1 = (f64::from(rect.y + rect.h) / sy).ceil() as u32;
    Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus;

    #[test]
    fn picture_widths_match_the_measured_captures() {
        assert_eq!(picture_width(1920, 1080), 1366);
        assert_eq!(picture_width(2560, 1440), 1366);
        assert_eq!(picture_width(1920, 1200), 1228);
    }

    #[test]
    fn rectangles_map_between_the_client_and_the_1x_picture() {
        let scaling = Scaling::for_client(1920, 1080, UiMode::Default);
        let native = Rect {
            x: 283,
            y: 720,
            w: 800,
            h: 48,
        };
        let client = to_client(native, scaling, 1920, 1080);
        assert_eq!((client.x, client.y, client.y + client.h), (397, 1012, 1080));
        let back = from_client(client, scaling, 1920, 1080);
        assert!(
            back.x <= native.x && back.x + back.w >= native.x + native.w,
            "{back:?}"
        );
        let blocks = Scaling::Blocks(2);
        assert_eq!(
            to_client(native, blocks, 2560, 1440),
            Rect {
                x: 566,
                y: 1440,
                w: 1600,
                h: 0
            }
        );
    }

    #[test]
    fn ideal_ratio_scales_in_whole_steps() {
        assert_eq!(
            Scaling::for_client(1366, 768, UiMode::Ideal),
            Scaling::Native
        );
        assert_eq!(
            Scaling::for_client(1920, 1080, UiMode::Ideal),
            Scaling::Native
        );
        assert_eq!(
            Scaling::for_client(1920, 1200, UiMode::Ideal),
            Scaling::Native
        );
        assert_eq!(
            Scaling::for_client(2560, 1440, UiMode::Ideal),
            Scaling::Blocks(2)
        );
    }

    /// A known picture, stretched the way the client does, comes back.
    #[test]
    fn an_exact_stretch_is_undone() {
        let (nw, nh) = (60u32, 40u32);
        let (cw, ch) = (84u32, 56u32); // 1.4x both ways
        let native: Vec<f64> = (0..nw * nh)
            .map(|i| if (i * 7919) % 13 < 6 { 255.0 } else { 20.0 })
            .collect();
        let (sx, sy) = (f64::from(cw) / f64::from(nw), f64::from(ch) / f64::from(nh));
        let sample = |x: u32, y: u32| -> f64 {
            let (fx, ax) = source(x, sx);
            let (fy, ay) = source(y, sy);
            let p = |i: i64, j: i64| {
                native[(j.clamp(0, nh as i64 - 1) * nw as i64 + i.clamp(0, nw as i64 - 1)) as usize]
            };
            (p(fx, fy) * (1.0 - ax) + p(fx + 1, fy) * ax) * (1.0 - ay)
                + (p(fx, fy + 1) * (1.0 - ax) + p(fx + 1, fy + 1) * ax) * ay
        };
        let mut rgba = Vec::new();
        for y in 0..ch {
            for x in 0..cw {
                let v = sample(x, y).round() as u8;
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let frame = Frame {
            width: cw,
            height: ch,
            rgba,
        };
        let region = Rect {
            x: 5,
            y: 5,
            w: 50,
            h: 30,
        };
        let scaling = Scaling::Stretched {
            width: nw,
            sharpen: None,
        };
        // `native_size` reports 768 lines for a stretch; this test picture is
        // shorter, so solve through `unstretch` directly.
        let mut out = Frame {
            width: nw,
            height: nh,
            rgba: vec![0; (nw * nh * 4) as usize],
        };
        unstretch(&frame, &mut out, region, sx, sy, None);
        let _ = scaling;
        for y in region.y..region.y + region.h {
            for x in region.x..region.x + region.w {
                let got = f64::from(out.rgba[((y * nw + x) * 4) as usize]);
                let want = native[(y * nw + x) as usize];
                assert!((got - want).abs() <= 2.0, "({x},{y}) {got} vs {want}");
            }
        }
    }

    fn gray(frame: &Frame, x: u32, y: u32) -> f64 {
        let i = ((y * frame.width + x) * 4) as usize;
        (f64::from(frame.rgba[i]) + f64::from(frame.rgba[i + 1]) + f64::from(frame.rgba[i + 2]))
            / 3.0
    }

    /// Recover the 1x minimap of one scene at `pair` and compare it with the
    /// same scene's 1366x768 capture (drawn at 1x), aligned by the best shift.
    /// Returns (mean abs difference, pixels on the other side of 128).
    fn folded_minimap_vs_1x(pair: &str) -> (f64, usize) {
        let scene = "eurovein-lv10-ellinia-weapon-store";
        let id = format!("{scene}/minimap-folded");
        let reference = corpus::fixture_frame("1366x768", &id);
        let case = corpus::load_cases()
            .into_iter()
            .find(|case| case.key == format!("{pair}/{scene}/folded"))
            .expect("the scene");
        let frame = corpus::compose_frame(&case);
        let (w, h) = (frame.width, frame.height);
        let mode = case.mode();
        let scaling = Scaling::for_client(w, h, mode);
        let crop = case.minimap_crop;
        let client = Rect {
            x: crop.x as u32,
            y: crop.y as u32,
            w: crop.w as u32,
            h: crop.h as u32,
        };
        let mut region = from_client(client, scaling, w, h);
        // Stay clear of the pasted crop's edges, which are not part of the
        // stretch.
        region = Rect {
            x: region.x + 4,
            y: region.y + 4,
            w: region.w - 8,
            h: region.h - 8,
        };
        let native = to_native(&frame, scaling, &[region]);
        let ref_crop = corpus::load_cases()
            .into_iter()
            .find(|case| case.key == format!("1366x768/{scene}/folded"))
            .expect("the 1x scene")
            .minimap_crop;
        let mut best = (f64::MAX, usize::MAX);
        for dy in -8i64..=8 {
            for dx in -8i64..=8 {
                let (mut sum, mut flips, mut n) = (0.0, 0usize, 0usize);
                for y in region.y..region.y + region.h {
                    for x in region.x..region.x + region.w {
                        let rx = x as i64 + dx - ref_crop.x as i64;
                        let ry = y as i64 + dy - ref_crop.y as i64;
                        if rx < 0
                            || ry < 0
                            || rx >= reference.width as i64
                            || ry >= reference.height as i64
                        {
                            continue;
                        }
                        let a = gray(&native, x, y);
                        let b = gray(&reference, rx as u32, ry as u32);
                        sum += (a - b).abs();
                        flips += usize::from((a > 128.0) != (b > 128.0));
                        n += 1;
                    }
                }
                if n * 2 < (region.w * region.h) as usize {
                    continue;
                }
                let mean = sum / n as f64;
                if mean < best.0 {
                    best = (mean, flips);
                }
            }
        }
        best
    }

    #[test]
    fn default_and_ideal_recover_the_1x_picture_exactly() {
        for pair in [
            "1920x1080",
            "1920x1200",
            "2560x1440",
            "1920x1080@ideal",
            "1920x1200@ideal",
            "2560x1440@ideal",
        ] {
            let (mean, flips) = folded_minimap_vs_1x(pair);
            assert!(
                mean < 0.5 && flips == 0,
                "{pair}: mean {mean:.2}, {flips} flips"
            );
        }
    }

    #[test]
    fn filter_applied_recovers_the_1x_picture_closely() {
        for (pair, max_mean) in [
            ("1920x1080@filtered", 8.0),
            ("2560x1440@filtered", 8.0),
            // The 1200-line filter is aligned differently; recorded, not yet fitted.
            ("1920x1200@filtered", 14.0),
        ] {
            let (mean, flips) = folded_minimap_vs_1x(pair);
            println!("{pair}: mean {mean:.2}, {flips} flips");
            assert!(mean < max_mean, "{pair}: mean {mean:.2}, {flips} flips");
        }
    }
}
