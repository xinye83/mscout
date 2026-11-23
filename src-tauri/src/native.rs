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

use std::sync::Mutex;

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
    /// the cross-sharpen weight Filter Applied adds after the stretch, and
    /// `shift` how far (in 1x pixels) its horizontal sampling sits from the
    /// Default stretch's.
    Stretched {
        width: u32,
        sharpen: Option<f32>,
        shift: f64,
    },
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
                shift: 0.0,
            },
            // The geometry a Filter Applied frame is fitted from
            // ([`fit_filtered`]); this is its starting point.
            UiMode::Filtered => Scaling::Stretched {
                width: picture_width(width, height),
                sharpen: Some(FILTER_SHARPEN),
                shift: 0.0,
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
            Scaling::Stretched { sharpen, shift, .. } => {
                let (sx, sy) = scaling.scale(frame.width, frame.height);
                unstretch(frame, &mut out, region, (sx, shift), sy, sharpen);
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
fn source(c: u32, scale: f64, shift: f64) -> (i64, f64) {
    let u = (f64::from(c) + 0.5) / scale - 0.5 + shift;
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
/// pixels, so the normal equations are tridiagonal. At the picture's edge
/// (`0` and `limit`) the stretch repeats the edge pixel, as the client does.
struct Axis {
    n0: i64,
    n: usize,
    /// Per client sample: the two 1x pixels it mixes (relative) and their
    /// weights, or `None` when it reaches outside `n0..n1`.
    samples: Vec<Option<(usize, f64, usize, f64)>>,
    lower: Vec<f64>,
    diag: Vec<f64>,
    upper: Vec<f64>,
}

impl Axis {
    fn new(n0: i64, n1: i64, c0: u32, c1: u32, (scale, shift): (f64, f64), limit: i64) -> Axis {
        let n = (n1 - n0) as usize;
        let mut samples = Vec::new();
        let (mut lower, mut diag, mut upper) = (vec![0.0; n], vec![1e-6; n], vec![0.0; n]);
        for c in c0..c1 {
            let (f, fr) = source(c, scale, shift);
            let left = f.clamp(0, limit - 1);
            let right = (f + 1).clamp(0, limit - 1);
            if left < n0 || right >= n1 {
                samples.push(None);
                continue;
            }
            let (i, j) = ((left - n0) as usize, (right - n0) as usize);
            let (a, b) = (1.0 - fr, fr);
            diag[i] += a * a;
            diag[j] += b * b;
            if i == j {
                diag[i] += 2.0 * a * b;
            } else {
                upper[i] += a * b;
                lower[j] += a * b;
            }
            samples.push(Some((i, a, j, b)));
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
        for (sample, &v) in self.samples.iter().zip(values) {
            let Some((i, a, j, b)) = *sample else {
                continue;
            };
            rhs[i] += a * v;
            rhs[j] += b * v;
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

fn unstretch(
    frame: &Frame,
    out: &mut Frame,
    region: Rect,
    (sx, shift): (f64, f64),
    sy: f64,
    sharpen: Option<f32>,
) {
    let (limit_x, limit_y) = (i64::from(out.width), i64::from(out.height));
    let (nx0, ny0) = (
        (i64::from(region.x) - i64::from(MARGIN)).max(0),
        (i64::from(region.y) - i64::from(MARGIN)).max(0),
    );
    let (nx1, ny1) = (
        i64::from(region.x + region.w + MARGIN).min(limit_x),
        i64::from(region.y + region.h + MARGIN).min(limit_y),
    );
    let (mut cx0, mut cx1) = client_span(nx0, nx1, sx, frame.width);
    let (mut cy0, mut cy1) = client_span(ny0, ny1, sy, frame.height);
    // Client pixels past the last 1x pixel's centre sample only the edge pixel.
    if nx0 == 0 {
        cx0 = 0;
    }
    if nx1 == limit_x {
        cx1 = frame.width;
    }
    if ny0 == 0 {
        cy0 = 0;
    }
    if ny1 == limit_y {
        cy1 = frame.height;
    }
    if cx0 >= cx1 || cy0 >= cy1 {
        return;
    }
    let (cw, ch) = ((cx1 - cx0) as usize, (cy1 - cy0) as usize);
    let xs = Axis::new(nx0, nx1, cx0, cx1, (sx, shift), limit_x);
    let ys = Axis::new(ny0, ny1, cy0, cy1, (sy, 0.0), limit_y);
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

/// How far, on average, a client frame's HUD band is from `scaling` applied to
/// the frame's own least-squares 1x picture under that scaling. When the
/// scaling is the one the client drew with, the frame comes back almost
/// exactly; a wrong width, shift or UI Size leaves a clear residual.
fn model_residual(frame: &Frame, scaling: Scaling, band: Band) -> Option<f64> {
    let Scaling::Stretched {
        width,
        sharpen,
        shift,
    } = scaling
    else {
        return None;
    };
    let band = band.rect(width)?;
    let native = to_native(frame, scaling, &[band]);
    let (sx, sy) = scaling.scale(frame.width, frame.height);
    let inner = Rect {
        x: band.x + 2,
        y: band.y + 2,
        w: band.w - 4,
        h: band.h - 4,
    };
    let client = to_client(inner, scaling, frame.width, frame.height);
    let (x0, y0) = (client.x + 2, client.y + 2);
    let (w, h) = (
        client.w.saturating_sub(4) as usize,
        client.h.saturating_sub(4) as usize,
    );
    let px = |x: i64, y: i64, c: usize| -> f64 {
        f64::from(native.rgba[((y as u32 * native.width + x as u32) * 4) as usize + c])
    };
    let inside = |v: i64, lo: u32, len: u32| v >= i64::from(lo) && v + 1 < i64::from(lo + len);
    let (mut sum, mut n) = (0.0, 0usize);
    for c in 0..3 {
        // The stretch of the recovered picture over the patch; `None` where it
        // samples outside the solved band.
        let mut patch: Vec<Option<f64>> = vec![None; w * h];
        for y in 0..h {
            for x in 0..w {
                let (fx, ax) = source(x0 + x as u32, sx, shift);
                let (fy, ay) = source(y0 + y as u32, sy, 0.0);
                if !inside(fx, inner.x, inner.w) || !inside(fy, inner.y, inner.h) {
                    continue;
                }
                patch[y * w + x] = Some(
                    (px(fx, fy, c) * (1.0 - ax) + px(fx + 1, fy, c) * ax) * (1.0 - ay)
                        + (px(fx, fy + 1, c) * (1.0 - ax) + px(fx + 1, fy + 1, c) * ax) * ay,
                );
            }
        }
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                let Some(centre) = patch[y * w + x] else {
                    continue;
                };
                let value = match sharpen {
                    None => centre,
                    Some(weight) => {
                        let weight = f64::from(weight);
                        let around = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)];
                        let Some(cross) = around
                            .iter()
                            .map(|&(i, j)| patch[j * w + i])
                            .sum::<Option<f64>>()
                        else {
                            continue;
                        };
                        (centre - weight * cross) / (1.0 - 4.0 * weight)
                    }
                };
                let i = (((y0 + y as u32) * frame.width + x0 + x as u32) * 4) as usize + c;
                sum += (value - f64::from(frame.rgba[i])).abs();
                n += 1;
            }
        }
    }
    (n > 0).then(|| sum / n as f64)
}

/// The part of the HUD bar a residual is measured over, in 1x pixels from the
/// bar's centre.
#[derive(Clone, Copy)]
enum Band {
    /// Level, name, gauges and the EXP readout: what the UI Size thresholds
    /// were measured on.
    Detect,
    /// The level tile and the name and job lines: thin 1-bit strokes, which
    /// a half-pixel error in the fitted shift smears across two pixels. The
    /// gauges' gradients barely notice that error.
    Text,
}

impl Band {
    fn rect(self, width: u32) -> Option<Rect> {
        Some(match self {
            Band::Detect => Rect {
                x: (width / 2).checked_sub(300)?,
                y: NATIVE_HEIGHT - 40,
                w: 500,
                h: 32,
            },
            Band::Text => Rect {
                x: (width / 2).checked_sub(380)?,
                y: NATIVE_HEIGHT - 40,
                w: 250,
                h: 38,
            },
        })
    }
}

/// [`model_residual`] for the Default stretch: about 0.1 for a frame the
/// client stretched, 1.3 to 4 for Filter Applied's sharpened stretch, 12 or
/// more for Ideal Ratio's crisp 1x pixels, which are not a stretch of anything
/// (measured on every `testdata/frames` scene above 768 lines).
fn stretch_residual(frame: &Frame) -> Option<f64> {
    model_residual(
        frame,
        Scaling::Stretched {
            width: picture_width(frame.width, frame.height),
            sharpen: None,
            shift: 0.0,
        },
        Band::Detect,
    )
}

/// The cross-sharpen weight used to undo Filter Applied. The filter is not
/// fully linear; across 0.03 to 0.1 the recovered text differs by a pixel or
/// two, while the geometry decides whether it is readable at all.
const FILTER_SHARPEN: f32 = 0.1;

/// The geometry a Filter Applied frame was drawn with. The client does not
/// stretch its sharpened picture quite like the Default one: at 1920x1200 the
/// picture is 1230 wide, not 1228 (the level, name and EXP readout each fit
/// 1230 on their own). Try the widths and shifts near the Default stretch's
/// and keep the one whose recovered picture explains the HUD text best; with
/// it the text comes back within a few pixels of the 1x picture.
pub fn fit_filtered(frame: &Frame) -> Scaling {
    let base = picture_width(frame.width, frame.height);
    let mut best = (
        f64::MAX,
        Scaling::Stretched {
            width: base,
            sharpen: Some(FILTER_SHARPEN),
            shift: 0.0,
        },
    );
    for width in base - 2..=base + 2 {
        for step in -4..=4 {
            let shift = f64::from(step) * 0.125;
            let scaling = Scaling::Stretched {
                width,
                sharpen: Some(FILTER_SHARPEN),
                shift,
            };
            if let Some(residual) = model_residual(frame, scaling, Band::Text) {
                if residual < best.0 {
                    best = (residual, scaling);
                }
            }
        }
    }
    best.1
}

/// The fitted Filter Applied geometry per client size: it belongs to the
/// client's scaler, not to a frame, so one fit serves until the size changes.
static FITTED: Mutex<Option<(u32, u32, Scaling)>> = Mutex::new(None);

fn filtered_scaling(frame: &Frame) -> Scaling {
    let mut fitted = FITTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((w, h, scaling)) = *fitted {
        if (w, h) == (frame.width, frame.height) {
            return scaling;
        }
    }
    let scaling = fit_filtered(frame);
    *fitted = Some((frame.width, frame.height, scaling));
    scaling
}

/// Default stretches below this residual; Filter Applied below
/// [`FILTERED_BELOW`]; Ideal Ratio above. The measured classes sit at <= 0.15,
/// 1.3 to 3.9 and >= 11.9.
const DEFAULT_BELOW: f64 = 0.6;
const FILTERED_BELOW: f64 = 7.0;

/// The UI Size a client frame was drawn with. `None` up to 768 lines, where
/// every UI Size draws the same pixels and the choice does not matter.
pub fn detect_mode(frame: &Frame) -> Option<UiMode> {
    if frame.height <= NATIVE_HEIGHT {
        return None;
    }
    let residual = stretch_residual(frame)?;
    Some(if residual < DEFAULT_BELOW {
        UiMode::Default
    } else if residual < FILTERED_BELOW {
        UiMode::Filtered
    } else {
        UiMode::Ideal
    })
}

/// A captured frame as the readers see it.
pub struct Prepared {
    /// The 1x picture: the HUD band and the minimap corner recovered, the rest
    /// black. The client frame itself when the client draws at 1x.
    pub frame: Frame,
    /// The UI Size the frame was drawn with: detected above 768 lines, else
    /// the setting. The app takes it through [`mode_for`]; the regression
    /// reads it directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub mode: UiMode,
}

/// The last prepared picture's size and UI Size, so the profile chosen for a
/// prepared frame follows the detected UI Size rather than the setting.
static LAST: Mutex<Option<(u32, u32, UiMode)>> = Mutex::new(None);

/// The UI Size the last prepared frame was read with, whatever its size.
pub fn last_mode() -> Option<UiMode> {
    let last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    last.map(|(_, _, mode)| mode)
}

/// The UI Size detected for the last prepared picture of this size.
pub fn mode_for(width: u32, height: u32) -> Option<UiMode> {
    let last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    last.and_then(|(w, h, mode)| (w == width && h == height).then_some(mode))
}

/// The 1x picture behind a captured client frame, the one every reader and
/// profile works in: one HUD geometry and one glyph size for every resolution
/// and UI Size.
pub fn prepare(frame: Frame, setting: UiMode) -> Prepared {
    prepare_with(frame, setting, false)
}

/// [`prepare`], recovering the whole picture rather than the HUD band and the
/// minimap corner: what the calibration window shows.
pub fn prepare_whole(frame: Frame, setting: UiMode) -> Prepared {
    prepare_with(frame, setting, true)
}

fn prepare_with(frame: Frame, setting: UiMode, whole: bool) -> Prepared {
    let mode = detect_mode(&frame).unwrap_or(setting);
    let scaling = match Scaling::for_client(frame.width, frame.height, mode) {
        Scaling::Stretched {
            sharpen: Some(_), ..
        } => filtered_scaling(&frame),
        scaling => scaling,
    };
    let frame = match scaling {
        Scaling::Native => frame,
        Scaling::Blocks(_) => {
            let (w, h) = scaling.native_size(frame.width, frame.height);
            to_native(&frame, scaling, &[Rect { x: 0, y: 0, w, h }])
        }
        Scaling::Stretched { .. } if whole => {
            let (w, h) = scaling.native_size(frame.width, frame.height);
            to_native(&frame, scaling, &[Rect { x: 0, y: 0, w, h }])
        }
        Scaling::Stretched { .. } => {
            let (w, h) = scaling.native_size(frame.width, frame.height);
            let hud = Rect {
                x: 0,
                y: h.saturating_sub(HUD_ROWS),
                w,
                h: HUD_ROWS.min(h),
            };
            let minimap = Rect {
                x: 0,
                y: 0,
                w: MINIMAP_CORNER.0.min(w),
                h: MINIMAP_CORNER.1.min(h),
            };
            let mut out = to_native(&frame, scaling, &[hud]);
            let corner = to_native(&frame, corner_scaling(scaling), &[minimap]);
            copy_rect(&corner, &mut out, minimap);
            out
        }
    };
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = Some((frame.width, frame.height, mode));
    Prepared { frame, mode }
}

/// The scaling the minimap corner is recovered with: a fitted Filter Applied
/// shift explains the HUD band, not the bright minimap bar, which reads better
/// sampled like the Default stretch (no shift) at the fitted width.
fn corner_scaling(scaling: Scaling) -> Scaling {
    match scaling {
        Scaling::Stretched { width, sharpen, .. } => Scaling::Stretched {
            width,
            sharpen,
            shift: 0.0,
        },
        other => other,
    }
}

fn copy_rect(from: &Frame, to: &mut Frame, rect: Rect) {
    for y in rect.y..(rect.y + rect.h).min(to.height) {
        let start = ((y * to.width + rect.x) * 4) as usize;
        let end = ((y * to.width + (rect.x + rect.w).min(to.width)) * 4) as usize;
        to.rgba[start..end].copy_from_slice(&from.rgba[start..end]);
    }
}

/// The 1x rows at the bottom that hold the HUD bar, with room above it.
const HUD_ROWS: u32 = 64;
/// The 1x corner the minimap is drawn in, at its largest.
const MINIMAP_CORNER: (u32, u32) = (460, 360);

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
#[cfg(test)]
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
            let (fx, ax) = source(x, sx, 0.0);
            let (fy, ay) = source(y, sy, 0.0);
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
            shift: 0.0,
        };
        // `native_size` reports 768 lines for a stretch; this test picture is
        // shorter, so solve through `unstretch` directly.
        let mut out = Frame {
            width: nw,
            height: nh,
            rgba: vec![0; (nw * nh * 4) as usize],
        };
        unstretch(&frame, &mut out, region, (sx, 0.0), sy, None);
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
        let scaling = match mode {
            UiMode::Filtered => corner_scaling(fit_filtered(&frame)),
            _ => Scaling::for_client(w, h, mode),
        };
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
    fn the_ui_size_is_detected_from_the_frame() {
        let mut checked = 0;
        for case in corpus::load_cases() {
            if case.client_h <= NATIVE_HEIGHT {
                continue;
            }
            let frame = corpus::compose_frame(&case);
            assert_eq!(detect_mode(&frame), Some(case.mode()), "{}", case.key);
            checked += 1;
        }
        assert!(checked >= 20, "only {checked} cases above 768 lines");
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
    fn the_filter_applied_geometry_is_fitted_per_size() {
        for (pair, width) in [
            ("1920x1080@filtered", 1366),
            ("1920x1200@filtered", 1230),
            ("2560x1440@filtered", 1366),
        ] {
            let case = corpus::load_cases()
                .into_iter()
                .find(|case| case.pair_key == pair)
                .expect("a scene");
            let fitted = fit_filtered(&corpus::compose_frame(&case));
            let Scaling::Stretched { width: got, .. } = fitted else {
                panic!("{pair}: {fitted:?}");
            };
            assert_eq!(got, width, "{pair}: {fitted:?}");
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
