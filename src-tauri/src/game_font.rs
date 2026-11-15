//! Reading the client's HUD glyph atlases.
//!
//! The EXP/level digits are not a font: the client blits baked bitmaps from
//! `Data/UI/_Canvas/_Canvas_000.wz`. This module locates that archive in the
//! player's MSCW install and extracts the digit sets at their native size.
//!
//! The archive uses the CLASSIC WZ key (a zero IV). `wz_reader` handles the
//! PKG1 directory, encrypted strings and canvas decompression.

use std::path::{Path, PathBuf};

use wz_reader::util::version::WzMapleVersion;
use wz_reader::{WzNode, WzNodeArc};

/// One extracted glyph, trimmed to its own ink and stored row-major.
#[derive(Clone, Debug)]
pub struct AtlasGlyph {
    pub ch: char,
    pub w: u32,
    pub h: u32,
    /// `w*h` bytes, `0x00`/`0xff`.
    pub bitmap: Vec<u8>,
}

/// Locate `_Canvas_000.wz` under either the `MSCW` folder itself or its parent.
pub fn canvas_wz(mscw_dir: &Path) -> Option<PathBuf> {
    [
        mscw_dir.join("Data/UI/_Canvas/_Canvas_000.wz"),
        mscw_dir.join("MSCW/Data/UI/_Canvas/_Canvas_000.wz"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

pub fn open_wz(path: &Path) -> Result<WzNodeArc, String> {
    let node: WzNodeArc = WzNode::from_wz_file_full(
        path,
        Some(WzMapleVersion::CLASSIC),
        None,
        None,
        None,
    )
    .map_err(|error| format!("open {}: {error}", path.display()))?
    .into();
    // The root's children are populated by parsing the root itself; `at_path`
    // only walks children that already exist.
    {
        let mut guard = node
            .write()
            .map_err(|_| "the WZ root is poisoned".to_string())?;
        guard
            .parse(&node)
            .map_err(|error| format!("parse {}: {error}", path.display()))?;
    }
    Ok(node)
}

/// Read one canvas by path, threshold it to ink, and trim it to its bounding box.
pub fn glyph_at(root: &WzNodeArc, path: &str, ch: char) -> Result<AtlasGlyph, String> {
    let (w, h, bitmap) = canvas_at(root, path)?;
    let Some(trimmed) = trim_bitmap(&bitmap, w, h) else {
        return Err(format!("{path}: no glyph pixels"));
    };
    Ok(AtlasGlyph {
        ch,
        w: trimmed.w,
        h: trimmed.h,
        bitmap: trimmed.bitmap,
    })
}

/// One trimmed bitmap.
pub struct Trimmed {
    /// Bounding box of the ink inside the source bitmap.
    #[allow(dead_code)] // read by the font-cell prototype
    pub min_x: u32,
    #[allow(dead_code)]
    pub min_y: u32,
    pub w: u32,
    pub h: u32,
    pub bitmap: Vec<u8>,
}

/// Read a canvas as ink, without trimming.
pub fn canvas_at(root: &WzNodeArc, path: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let node = root
        .read()
        .map_err(|_| "the WZ root is poisoned".to_string())?
        .at_path_parsed(path)
        .map_err(|error| format!("{path}: {error}"))?;
    let image = wz_reader::property::get_image(&node).map_err(|error| format!("{path}: {error}"))?;
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    if width == 0 || height == 0 {
        return Err(format!("{path}: empty image"));
    }

    let mut bitmap = vec![0u8; (width * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let pixel = rgba.get_pixel(x, y);
            let luminance = (pixel[0] as u32 + pixel[1] as u32 + pixel[2] as u32) / 3;
            // The glyphs are bright on a transparent/near-black background. The
            // atlas PNG's alpha channel is not a usable mask (JPEG-ish edges), so
            // threshold luminance.
            if luminance > 128 {
                bitmap[(y * width + x) as usize] = 255;
            }
        }
    }
    Ok((width, height, bitmap))
}

/// Trim a bitmap to the bounding box of its ink.
pub fn trim_bitmap(bitmap: &[u8], width: u32, height: u32) -> Option<Trimmed> {
    let (mut min_x, mut min_y) = (width, height);
    let (mut max_x, mut max_y) = (0u32, 0u32);
    for y in 0..height {
        for x in 0..width {
            if bitmap[(y * width + x) as usize] > 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x < min_x || max_y < min_y {
        return None;
    }
    let w = max_x - min_x + 1;
    let h = max_y - min_y + 1;
    let mut out = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            out[(y * w + x) as usize] = bitmap[((y + min_y) * width + (x + min_x)) as usize];
        }
    }
    Some(Trimmed {
        min_x,
        min_y,
        w,
        h,
        bitmap: out,
    })
}

/// The EXP readout's glyph set, scaled to the height the client is drawing at.
///
/// `target_h` is the rendered digit height (7 for ideal, 11 for default). Widths
/// are scaled proportionally, so a narrow `1` and a wide `%` keep their shape.
/// `EXPBar/number` has no `1` or `]` and no punctuation; `1` is filled from
/// `Basic.img/ShowLevel/0/1` and the rest are derived shapes.
pub fn exp_glyphs(mscw_dir: &Path, target_h: u32) -> Result<Vec<AtlasGlyph>, String> {
    if !(4..=64).contains(&target_h) {
        return Err(format!("unexpected glyph height {target_h}"));
    }
    let wz_path = canvas_wz(mscw_dir)
        .ok_or_else(|| format!("no _Canvas_000.wz under {}", mscw_dir.display()))?;
    let root = open_wz(&wz_path)?;

    let mut out = Vec::new();
    for (path, ch) in [
        ("StatusBar3.img/mainBar/EXPBar/number/0", '0'),
        ("StatusBar3.img/mainBar/EXPBar/number/2", '2'),
        ("StatusBar3.img/mainBar/EXPBar/number/3", '3'),
        ("StatusBar3.img/mainBar/EXPBar/number/4", '4'),
        ("StatusBar3.img/mainBar/EXPBar/number/5", '5'),
        ("StatusBar3.img/mainBar/EXPBar/number/6", '6'),
        ("StatusBar3.img/mainBar/EXPBar/number/7", '7'),
        ("StatusBar3.img/mainBar/EXPBar/number/8", '8'),
        ("StatusBar3.img/mainBar/EXPBar/number/9", '9'),
        ("StatusBar3.img/mainBar/EXPBar/number/[", '['),
        ("StatusBar3.img/mainBar/EXPBar/number/%", '%'),
        ("Basic.img/ShowLevel/0/1", '1'),
    ] {
        let glyph = glyph_at(&root, path, ch)?;
        out.extend(scale_variants(&glyph, target_h));
    }

    // `]` is the horizontal mirror of `[` in this font.
    for open in out.iter().filter(|glyph| glyph.ch == '[').cloned().collect::<Vec<_>>() {
        out.push(mirror(&open, ']'));
    }
    out.push(comma(target_h));
    out.push(period(target_h));
    Ok(out)
}

/// Seed a glyph at its natural scaled width and one pixel either side. The
/// client's own scaling is not perfectly uniform: at 1366 Ideal the `7` comes
/// back one pixel narrower than the other digits, and at Default the digits
/// alternate 8/9 px wide. The variants let native matching hit the real shape
/// instead of relying on the size fallback.
fn scale_variants(glyph: &AtlasGlyph, height: u32) -> Vec<AtlasGlyph> {
    if glyph.h == 0 || height == 0 {
        return vec![glyph.clone()];
    }
    let natural = ((glyph.w as f64 * height as f64 / glyph.h as f64).round() as u32).max(1);
    let mut widths = vec![natural];
    if natural > 1 {
        widths.push(natural - 1);
    }
    widths.push(natural + 1);
    widths.sort_unstable();
    widths.dedup();

    let mut out = Vec::new();
    for width in widths {
        // Nearest keeps the ideal-size bitmaps exact and is the fallback on
        // both sides of the scale.
        out.push(scale_with(
            glyph,
            width,
            height,
            image::imageops::FilterType::Nearest,
            32,
        ));
        if height > glyph.h || width > glyph.w {
            // The client thickens strokes when it enlarges. Triangle at 96 was
            // the closest match for most Default digits; Lanczos covers the
            // narrow `1`.
            out.push(scale_with(
                glyph,
                width,
                height,
                image::imageops::FilterType::Triangle,
                96,
            ));
            out.push(scale_with(
                glyph,
                width,
                height,
                image::imageops::FilterType::Lanczos3,
                128,
            ));
            out.push(scale_with(
                glyph,
                width,
                height,
                image::imageops::FilterType::Lanczos3,
                96,
            ));
        } else if height < glyph.h || width < glyph.w {
            // A one-pixel reduction (the 1366 `7`); a soft filter matches the
            // client's own downscale better than nearest.
            out.push(scale_with(
                glyph,
                width,
                height,
                image::imageops::FilterType::Gaussian,
                64,
            ));
        }
    }

    out.sort_by_key(|glyph| (glyph.w, glyph.h));
    out.dedup_by(|a, b| a.w == b.w && a.h == b.h && a.bitmap == b.bitmap);
    out
}

fn scale_with(
    glyph: &AtlasGlyph,
    width: u32,
    height: u32,
    filter: image::imageops::FilterType,
    threshold: u8,
) -> AtlasGlyph {
    if width == glyph.w && height == glyph.h {
        return glyph.clone();
    }
    let Some(source) = image::GrayImage::from_raw(glyph.w, glyph.h, glyph.bitmap.clone()) else {
        return glyph.clone();
    };
    let scaled = image::imageops::resize(&source, width.max(1), height.max(1), filter);
    AtlasGlyph {
        ch: glyph.ch,
        w: width.max(1),
        h: height.max(1),
        bitmap: scaled
            .pixels()
            .map(|pixel| if pixel[0] >= threshold { 255 } else { 0 })
            .collect(),
    }
}

fn mirror(glyph: &AtlasGlyph, ch: char) -> AtlasGlyph {
    let mut bitmap = vec![0u8; (glyph.w * glyph.h) as usize];
    for y in 0..glyph.h {
        for x in 0..glyph.w {
            bitmap[(y * glyph.w + (glyph.w - 1 - x)) as usize] =
                glyph.bitmap[(y * glyph.w + x) as usize];
        }
    }
    AtlasGlyph {
        ch,
        w: glyph.w,
        h: glyph.h,
        bitmap,
    }
}

/// A comma at the readout's scale: a short anti-diagonal near the baseline.
fn comma(height: u32) -> AtlasGlyph {
    let w = ((height as f64 * 0.34).round() as u32).max(2);
    let h = ((height as f64 * 0.28).round() as u32).max(2);
    let mut bitmap = vec![0u8; (w * h) as usize];
    if w <= 2 {
        // Ideal: the live comma is the 2x2 `.#` / `#.`.
        if h >= 2 {
            bitmap[1] = 255;
            bitmap[w as usize] = 255;
        } else {
            bitmap[0] = 255;
        }
    } else {
        // Larger scales thicken the diagonal to two pixels.
        for y in 0..h {
            let x = (w - 1).saturating_sub((y * w) / h.max(1));
            bitmap[(y * w + x) as usize] = 255;
            if x > 0 {
                bitmap[(y * w + x - 1) as usize] = 255;
            }
        }
    }
    AtlasGlyph {
        ch: ',',
        w,
        h,
        bitmap,
    }
}

/// A full stop at the readout's scale: a small block (2x2 at Default).
fn period(height: u32) -> AtlasGlyph {
    let size = ((height as f64 * 0.16).round() as u32).max(1);
    let mut bitmap = vec![255u8; (size * size) as usize];
    if size == 2 {
        // The live Default stop is `.#` / `##`, not a solid 2x2.
        bitmap[0] = 0;
    }
    AtlasGlyph {
        ch: '.',
        w: size,
        h: size,
        bitmap,
    }
}

/// Build the stored EXP atlas for a live crop: detect the rendered digit
/// height, extract and scale the client's glyphs, and return them.
pub fn seed_exp_atlas(
    gray: &crate::ocr::Gray,
    mscw_dir: &Path,
) -> Result<(crate::settings::Atlas, u32), String> {
    let height = crate::ocr::dominant_glyph_height(gray)
        .ok_or("could not segment the EXP readout")?;
    let glyphs = exp_glyphs(mscw_dir, height)?;
    let mut atlas = crate::ocr::Atlas::default();
    for glyph in glyphs {
        atlas.learn_glyph(glyph.ch, glyph.bitmap, glyph.w, glyph.h);
    }
    // The seeded set is the design; the client's own rendering can differ in a
    // stroke (the Ideal `7`). Learn those exact shapes too, so the first read is
    // not a near miss. Doubtful shapes are left alone by `learn_rendering`.
    atlas.learn_rendering(gray);
    let mut stored = atlas.to_settings();
    stored.format = 2;
    stored.source = "game".into();
    Ok((stored, height))
}

/// The level readout uses the same digit font as the EXP readout, drawn wider.
///
/// A live level digit at 1920 Ideal is a 7x7 bitmap whose shape is the EXP 5x7
/// glyph stretched horizontally and softened (bilinear at threshold 64). It is
/// not `Basic.img/LevelNo`, which is a different tile-framed design, and it is
/// not a separate font. Verified pixel-exact for `3` and `5`.
pub fn level_glyphs(mscw_dir: &Path, target_w: u32, target_h: u32) -> Result<Vec<AtlasGlyph>, String> {
    if target_w < 3 || target_h < 4 || target_w > 64 || target_h > 64 {
        return Err(format!("unexpected level glyph size {target_w}x{target_h}"));
    }
    let wz_path = canvas_wz(mscw_dir)
        .ok_or_else(|| format!("no _Canvas_000.wz under {}", mscw_dir.display()))?;
    let root = open_wz(&wz_path)?;

    let mut out = Vec::new();
    for (path, ch) in [
        ("StatusBar3.img/mainBar/EXPBar/number/0", '0'),
        ("StatusBar3.img/mainBar/EXPBar/number/2", '2'),
        ("StatusBar3.img/mainBar/EXPBar/number/3", '3'),
        ("StatusBar3.img/mainBar/EXPBar/number/4", '4'),
        ("StatusBar3.img/mainBar/EXPBar/number/5", '5'),
        ("StatusBar3.img/mainBar/EXPBar/number/6", '6'),
        ("StatusBar3.img/mainBar/EXPBar/number/7", '7'),
        ("StatusBar3.img/mainBar/EXPBar/number/8", '8'),
        ("StatusBar3.img/mainBar/EXPBar/number/9", '9'),
        ("Basic.img/ShowLevel/0/1", '1'),
    ] {
        let glyph = glyph_at(&root, path, ch)?;
        // The live level digit is the EXP glyph widened to the level's size
        // (7×7 at Ideal), so seed that exactly.
        out.push(scale_with(
            &glyph,
            target_w,
            target_h,
            image::imageops::FilterType::Triangle,
            64,
        ));
        // A level containing a narrow `1` (or a different UI scale) may render at
        // the glyph's natural width instead. Seed those neighbours too.
        out.extend(scale_variants(&glyph, target_h));
    }
    out.sort_by_key(|glyph| (glyph.w, glyph.h, glyph.ch));
    out.dedup_by(|a, b| a.w == b.w && a.h == b.h && a.ch == b.ch && a.bitmap == b.bitmap);
    Ok(out)
}

/// The most common digit-sized shape in a segmented level box.
///
/// The tallest bright shape is not necessarily a digit: a pale tile frame can
/// survive the bright threshold at 11×13 while the digits are 7×7. Counting sizes
/// and taking the mode keeps the frame from becoming the size every glyph is
/// scaled to.
fn dominant_size(segments: &[crate::ocr::Segment]) -> Option<(u32, u32)> {
    use std::collections::HashMap;

    let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
    for segment in segments {
        if (3..=24).contains(&segment.w) && (4..=24).contains(&segment.h) {
            *counts.entry((segment.w, segment.h)).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|((w, h), _)| (w, h))
}

/// Build the stored level atlas from the live level box, when it reads digits.
pub fn seed_level_atlas(
    gray: &crate::ocr::Gray,
    mscw_dir: &Path,
) -> Result<(crate::settings::Atlas, (u32, u32)), String> {
    // Otsu sees the pale tile frame, so segment on bright ink: the digits are
    // near-white, the tile is not.
    let mut segments = crate::ocr::bright_segments(gray, crate::ocr::TEXT_FLOOR_PUB);
    if segments.is_empty() {
        segments = crate::ocr::segments(gray);
    }
    let (target_w, target_h) = dominant_size(&segments)
        .or_else(|| {
            segments
                .iter()
                .map(|segment| (segment.w, segment.h))
                .max_by_key(|(_, h)| *h)
        })
        .ok_or("no level shape")?;
    let glyphs = level_glyphs(mscw_dir, target_w, target_h)?;
    let mut atlas = crate::ocr::Atlas::default();
    for glyph in glyphs {
        atlas.learn_glyph(glyph.ch, glyph.bitmap, glyph.w, glyph.h);
    }
    let mut stored = atlas.to_settings();
    stored.format = 2;
    stored.source = "game".into();
    Ok((stored, (target_w, target_h)))
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MscwStatus {
    pub path: Option<String>,
    pub exists: bool,
    pub atlas_found: bool,
    /// Map/street names found in `String_000.wz` (the label source).
    pub map_names: usize,
    pub error: Option<String>,
}

pub fn status() -> MscwStatus {
    let Some(dir) = crate::settings::mscw_dir() else {
        return MscwStatus {
            path: None,
            exists: false,
            atlas_found: false,
            map_names: 0,
            error: None,
        };
    };
    let path = PathBuf::from(&dir);
    let exists = path.is_dir();
    let atlas = if exists { canvas_wz(&path) } else { None };
    let map_names = if exists {
        crate::map_names::load(&path).map(|entries| entries.len()).unwrap_or(0)
    } else {
        0
    };
    MscwStatus {
        path: Some(dir),
        exists,
        atlas_found: atlas.is_some(),
        map_names,
        error: if exists && atlas.is_none() {
            Some("Data/UI/_Canvas/_Canvas_000.wz was not found".into())
        } else {
            None
        },
    }
}

#[tauri::command]
pub fn get_mscw_status() -> MscwStatus {
    status()
}

#[tauri::command]
pub fn set_mscw_dir(path: String) -> Result<MscwStatus, String> {
    let dir = PathBuf::from(path.trim());
    if dir.as_os_str().is_empty() {
        return Err("choose your MapleStory MSCW folder".into());
    }
    if !dir.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    crate::settings::save_mscw_dir(Some(&dir.to_string_lossy()))?;
    Ok(status())
}

#[tauri::command]
pub fn pick_mscw_dir() -> Result<Option<String>, String> {
    pick_folder()
}

#[cfg(windows)]
fn pick_folder() -> Result<Option<String>, String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS,
        SIGDN_FILESYSPATH,
    };

    unsafe {
        // The IPC command runs on a worker thread; give it an STA for the
        // common dialog. A thread that is already initialised returns S_FALSE.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(
            &FileOpenDialog,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(|error| error.to_string())?;
        let options = dialog
            .cast::<IFileDialog>()
            .map_err(|error| error.to_string())?
            .GetOptions()
            .map_err(|error| error.to_string())?;
        dialog
            .cast::<IFileDialog>()
            .map_err(|error| error.to_string())?
            .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
            .map_err(|error| error.to_string())?;
        if dialog.Show(None).is_err() {
            CoUninitialize();
            return Ok(None);
        }
        let item = dialog.GetResult().map_err(|error| error.to_string())?;
        let name = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|error| error.to_string())?;
        let path = String::from_utf16_lossy(name.as_wide());
        CoTaskMemFree(Some(name.0 as *const _));
        CoUninitialize();
        Ok(Some(path))
    }
}

#[cfg(not(windows))]
fn pick_folder() -> Result<Option<String>, String> {
    Err("the folder picker is Windows-only".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Frame;
    use crate::settings::Region;

    fn game_dir() -> PathBuf {
        PathBuf::from(
            std::env::var("MSCOUT_MSCW_DIR")
                .unwrap_or_else(|_| r"D:\SteamLibrary\steamapps\common\MapleStory\MSCW".into()),
        )
    }

    fn load_bmp(path: &Path) -> Frame {
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

    fn active_level_region() -> Option<Region> {
        crate::profile::active()
            .and_then(|profile| profile.calibration.level)
            .filter(Region::is_usable)
    }

    #[test]
    #[ignore = "diagnostic: seeds EXP and level from the live game at 1920x1080 ideal"]
    fn seed_live_level() {
        use crate::settings;

        let dir = game_dir();
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = crate::window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("capture");
        let data = crate::data_dir().expect("data dir");
        let frame_path = data.join("live-frame-1080.bmp");
        let frame = if frame_path.is_file() {
            load_bmp(&frame_path)
        } else {
            frame
        };

        let level_region = Region { x: 598, y: 1050, w: 41, h: 26 };
        let gray = crate::ocr::crop_gray(&frame, level_region).expect("crop");
        println!("level crop {}x{}", gray.w, gray.h);
        let segments = crate::ocr::segments(&gray);
        println!(
            "shapes: {:?}",
            segments
                .iter()
                .map(|s| (s.w, s.h, s.x, s.y))
                .collect::<Vec<_>>()
        );
        let (stored, size) = seed_level_atlas(&gray, &dir).expect("seed level");
        let atlas = crate::ocr::Atlas::from_settings(&stored);
        println!(
            "level seeded {}x{} source={} glyphs={}",
            size.0,
            size.1,
            stored.source,
            atlas.glyphs.len()
        );
        // A level box also needs the reader to segment the digit the same way.
        let read = atlas.recognize(&gray);
        println!("recognize (adaptive) = {read:?}");
        let bright = crate::ocr::bright_segments(&gray, 170);
        println!(
            "bright shapes: {:?}",
            bright
                .iter()
                .map(|s| (s.w, s.h, s.x, s.y))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "diagnostic: seeds the live profile's EXP atlas from the running game"]
    fn seed_live_profile() {
        use crate::settings;

        let dir = game_dir();
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = crate::window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("capture");
        println!(
            "frame {}x{} mode={:?}",
            frame.width,
            frame.height,
            settings::ui_mode()
        );
        // Same path the app takes: activate the profile for the live size/mode,
        // which loads the shipped preset when no profile exists yet.
        let profile = crate::profile::activate(frame.width, frame.height)
            .expect("activate profile for the live client size");
        println!("using profile {} ({:?})", profile.key(), profile.ui_mode);
        let region = match std::env::var("MSCOUT_CROP") {
            Ok(spec) => {
                let parts: Vec<i32> = spec
                    .split(',')
                    .map(|part| part.trim().parse().expect("number"))
                    .collect();
                settings::Region {
                    x: parts[0],
                    y: parts[1],
                    w: parts[2],
                    h: parts[3],
                }
            }
            Err(_) => profile
                .calibration
                .exp
                .filter(settings::Region::is_usable)
                .expect("EXP box"),
        };
        let gray = crate::ocr::crop_gray(&frame, region).expect("crop");
        if std::env::var_os("MSCOUT_SHOW_CROP").is_some() {
            for (row, line) in crate::ocr::shape_art(&gray).iter().enumerate() {
                println!("{row:2} {line}");
            }
        }
        let (stored, height) = seed_exp_atlas(&gray, &dir).expect("seed");
        let atlas = crate::ocr::Atlas::from_settings(&stored);
        println!(
            "profile={} crop={}x{} height={height} source={} glyphs={} read={:?}",
            profile.key(),
            gray.w,
            gray.h,
            stored.source,
            atlas.glyphs.len(),
            atlas.recognize_readout(&gray)
        );
        if std::env::var_os("MSCOUT_SAVE_SEED").is_some() {
            crate::profile::update(|active| active.atlas = stored).expect("save profile");
            for region in [profile.calibration.job, profile.calibration.name] {
                let Some(region) = region.filter(Region::is_usable) else {
                    continue;
                };
                let Some(identity_gray) = crate::ocr::crop_gray(&frame, region) else {
                    continue;
                };
                if let Some((identity, px)) = crate::font_seed::seed_identity(&identity_gray) {
                    crate::profile::update(|active| active.identity_atlas = identity)
                        .expect("save identity atlas");
                    println!("saved identity atlas from {px}px Arial");
                    break;
                }
            }
            if let Some(region) = active_level_region() {
                if let Some(level_gray) = crate::ocr::crop_gray(&frame, region) {
                    if let Ok((level_stored, _)) = seed_level_atlas(&level_gray, &dir) {
                        crate::profile::update(|active| active.level_atlas = level_stored)
                            .expect("save level atlas");
                        println!("saved level atlas");
                    }
                }
            }
            println!("saved into {}", profile.key());
        }
    }

    #[test]
    #[ignore = "diagnostic: reads the live client's HUD digit atlas"]
    fn probe_game_atlas() {
        let dir = game_dir();
        let Some(wz_path) = canvas_wz(&dir) else {
            println!("no canvas wz under {}", dir.display());
            return;
        };
        println!("wz: {}", wz_path.display());
        let root = open_wz(&wz_path).expect("open canvas wz");

        for (path, ch) in [
            ("StatusBar3.img/mainBar/EXPBar/number/0", '0'),
            ("StatusBar3.img/mainBar/EXPBar/number/7", '7'),
            ("StatusBar3.img/mainBar/EXPBar/number/[", '['),
            ("StatusBar3.img/mainBar/EXPBar/number/%", '%'),
            ("StatusBar3.img/mainBar/status/lvNumber/3", '3'),
            ("Basic.img/ItemNo/1", '1'),
        ] {
            match glyph_at(&root, path, ch) {
                Ok(glyph) => {
                    println!("{path} -> '{}' {}x{}", glyph.ch, glyph.w, glyph.h);
                    for y in 0..glyph.h {
                        let row: String = (0..glyph.w)
                            .map(|x| {
                                if glyph.bitmap[(y * glyph.w + x) as usize] > 0 {
                                    '#'
                                } else {
                                    '.'
                                }
                            })
                            .collect();
                        println!("  {row}");
                    }
                }
                Err(error) => println!("{path} FAILED: {error}"),
            }
        }
    }
}
