use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, State};

use crate::capture::{self, Frame};
use crate::engine::{self, Readings};
use crate::ocr::{self, Atlas};
use crate::profile;
use crate::settings::{self, Calibration, Region};
use crate::window_target;

static PREVIEW: Mutex<Option<CachedPreview>> = Mutex::new(None);
static SERIAL: AtomicU64 = AtomicU64::new(1);

struct CachedPreview {
    serial: u64,
    frame: Frame,
}

fn lock_preview() -> std::sync::MutexGuard<'static, Option<CachedPreview>> {
    PREVIEW.lock().unwrap_or_else(PoisonError::into_inner)
}

fn cached_frame(serial: u64) -> Result<Frame, String> {
    let guard = lock_preview();
    let cached = guard.as_ref().ok_or("freeze a frame first")?;
    if cached.serial != serial {
        return Err("the preview is stale — freeze a frame again".into());
    }
    Ok(cached.frame.clone())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningInfo {
    pub learned: String,
    pub coverage: String,
    /// Characters of the minimap label font learned so far. Once these cover the
    /// letters a map name uses, the label reads as text with no fingerprint.
    pub map_letters: String,
    pub maps: Vec<String>,
    pub name: Option<String>,
    pub job: Option<String>,
}

fn learning_info(atlas: &Atlas) -> LearningInfo {
    let coverage = "0123456789"
        .chars()
        .filter(|ch| atlas.glyphs.iter().any(|glyph| glyph.ch == *ch))
        .collect();
    let identities = settings::load_identities();
    let map_atlas = Atlas::from_settings(&settings::load_map_atlas());

    LearningInfo {
        learned: atlas.learned(),
        coverage,
        map_letters: map_atlas.learned(),
        maps: settings::load_maps()
            .into_iter()
            .map(|template| template.label)
            .collect(),
        name: identities.name.map(|template| template.label),
        job: identities.job.map(|template| template.label),
    }
}

#[tauri::command]
pub fn capture_preview(
    state: State<'_, window_target::TargetState>,
) -> Result<tauri::ipc::Response, String> {
    let selector = state
        .selector()
        .ok_or("attach a game window first")?;
    let window = window_target::resolve_selector(&selector).ok_or("game window not found")?;
    if window.minimized {
        return Err("game window is minimized".into());
    }

    let frame = capture::client_frame(window.hwnd)?;
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    log(&format!(
        "capture_preview serial={serial} frame {}x{}",
        frame.width, frame.height
    ));
    *lock_preview() = Some(CachedPreview {
        serial,
        frame: frame.clone(),
    });

    // Header: width u32, height u32, serial u64 — 16 bytes, then raw RGBA.
    // The frontend parses this exact layout; keep the two in step.
    let mut bytes = Vec::with_capacity(16 + frame.rgba.len());
    bytes.extend_from_slice(&frame.width.to_le_bytes());
    bytes.extend_from_slice(&frame.height.to_le_bytes());
    bytes.extend_from_slice(&serial.to_le_bytes());
    bytes.extend_from_slice(&frame.rgba);
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub fn get_calibration(
    state: State<'_, window_target::TargetState>,
) -> Calibration {
    // Make sure a profile is active before reading it. Right after startup the engine
    // may not have ticked yet, and `active()` then has nothing — which handed the
    // window an all-null set it could save straight back over the real one.
    if crate::profile::active().is_none() {
        if let Some(selector) = state.selector() {
            if let Some(window) = window_target::resolve_selector(&selector) {
                let _ = profile::activate(window.client.w as u32, window.client.h as u32);
            }
        }
    }
    settings::load_calibration()
}

/// The calibration set for a specific client size, without switching to it.
#[tauri::command]
pub fn calibration_for(client_w: u32, client_h: u32) -> Option<Calibration> {
    profile::load(client_w, client_h).map(|profile| profile.calibration)
}

#[tauri::command]
pub fn set_calibration(
    client_w: u32,
    client_h: u32,
    exp: Option<Region>,
    level: Option<Region>,
    map: Option<Region>,
    name: Option<Region>,
    job: Option<Region>,
) -> Result<Calibration, String> {
    // Calibration belongs to the profile for the size being calibrated.
    profile::activate(client_w, client_h)?;

    // A save with nothing in it is never what the player meant: it is a window whose
    // box state came back empty, and letting it through wiped a profile's boxes more
    // than once. Clearing one box is fine; clearing all of them is not a save.
    if exp.is_none()
        && level.is_none()
        && map.is_none()
        && name.is_none()
        && job.is_none()
    {
        log("set_calibration: refused an empty calibration");
        return Err("refusing to save an empty calibration — freeze a frame and draw a box".into());
    }

    let calibration = Calibration {
        client_w: Some(client_w),
        client_h: Some(client_h),
        exp,
        level,
        map,
        name,
        job,
    };
    settings::save_calibration(&calibration)?;
    Ok(calibration)
}

#[tauri::command]
pub fn get_learning() -> LearningInfo {
    learning_info(&Atlas::from_settings(&settings::load_atlas()))
}

/// Make the profile for this preview frame active, so every teach/probe command
/// writes to the calibration set belonging to that resolution.
fn use_frame_profile(frame: &capture::Frame) -> Result<crate::profile::Profile, String> {
    profile::activate(frame.width, frame.height)
}

#[tauri::command]
pub fn learn_atlas(serial: u64, text: String) -> Result<LearningInfo, String> {
    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();
    let region = calibration
        .exp
        .filter(Region::is_usable)
        .ok_or("save the EXP box first")?;
    let gray = ocr::crop_gray(&frame, region).ok_or("the EXP box is outside the window")?;

    let mut atlas = Atlas::from_settings(&settings::load_atlas());
    atlas.learn(&text, &gray)?;
    settings::save_atlas(&atlas.to_settings())?;

    Ok(learning_info(&atlas))
}

#[tauri::command]
pub fn teach_map(serial: u64, name: String) -> Result<LearningInfo, String> {
    let name = name.trim().to_string();
    log(&format!("teach_map serial={serial} name={name:?}"));
    if name.is_empty() {
        log("teach_map: empty name");
        return Err("type the map name first".into());
    }

    let frame = cached_frame(serial)?;
    log(&format!(
        "teach_map: frame {}x{}",
        frame.width, frame.height
    ));
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();

    // Prefer the detected minimap label: fingerprints are stored on a fixed
    // grid, so they stay valid when the minimap is moved or resized.
    let gray = match ocr::read_map_label(&frame) {
        Some(gray) => gray,
        None => {
            let region = calibration
                .map
                .filter(Region::is_usable)
                .ok_or("could not find the minimap — drag the map box first")?;
            let crop = ocr::crop_gray(&frame, region).ok_or("the map box is outside the window")?;
            ocr::resample(&crop, ocr::MAP_LABEL_W, ocr::MAP_LABEL_H)
        }
    };

    let mut maps = settings::load_maps();
    maps.retain(|template| template.label != name);
    maps.push(settings::Template {
        label: name.clone(),
        w: gray.w as u32,
        h: gray.h as u32,
        pixels: settings::to_hex(&gray.px),
    });
    settings::save_maps(&maps)?;

    // Feed the typed name into the label letter atlas, one line at a time, so the
    // map eventually reads as text. The two have to line up: a mismatch would
    // label the wrong glyphs, and the fingerprint above still covers this map, so
    // it is skipped rather than guessed.
    let wanted: Vec<&str> = name
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if let Some(lines) = ocr::map_label_lines(&frame) {
        if lines.len() == wanted.len() {
            let mut map_atlas = Atlas::from_settings(&settings::load_map_atlas());
            for (line, text) in lines.iter().zip(&wanted) {
                let _ = map_atlas.learn(text, line);
            }
            settings::save_map_atlas(&map_atlas.to_settings())?;
            log(&format!(
                "teach_map: learned letters {:?}",
                Atlas::from_settings(&settings::load_map_atlas()).learned()
            ));
        } else {
            log(&format!(
                "teach_map: {} label lines detected but {} typed — letters skipped",
                lines.len(),
                wanted.len()
            ));
        }
    } else {
        log("teach_map: minimap label not found — letters skipped");
    }

    Ok(learning_info(&Atlas::from_settings(&settings::load_atlas())))
}

#[tauri::command]
pub fn teach_identity(serial: u64, kind: String, label: String) -> Result<LearningInfo, String> {
    let label = label.trim().to_string();
    log(&format!("teach_identity serial={serial} kind={kind} label={label:?}"));
    if label.is_empty() {
        return Err("type the name first".into());
    }

    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();
    let region = match kind.as_str() {
        "name" => calibration.name,
        "job" => calibration.job,
        _ => return Err("unknown identity".into()),
    }
    .filter(Region::is_usable);
    let Some(region) = region else {
        log(&format!("teach_identity: no usable {kind} box"));
        return Err(format!("draw the {kind} box first, then teach"));
    };
    log(&format!("teach_identity: {kind} box {region:?}"));

    let gray = ocr::crop_gray(&frame, region).ok_or("the box is outside the window")?;
    let template = settings::Template {
        label,
        w: gray.w as u32,
        h: gray.h as u32,
        pixels: settings::to_hex(&gray.px),
    };

    let mut identities = settings::load_identities();
    match kind.as_str() {
        "name" => identities.name = Some(template),
        "job" => identities.job = Some(template),
        _ => return Err("unknown identity".into()),
    }
    settings::save_identities(&identities)?;

    Ok(learning_info(&Atlas::from_settings(&settings::load_atlas())))
}

#[tauri::command]
pub fn parse_probe(serial: u64) -> Result<Readings, String> {
    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();
    if !calibration.is_ready() {
        return Err("save the EXP box first".into());
    }

    let atlas = Atlas::from_settings(&settings::load_atlas());
    let maps = settings::load_maps();
    let identities = settings::load_identities();
    engine::read_frame(&frame, &calibration, &atlas, &maps, &identities)
}

/// Split a single name/job box into the two stacked lines it encloses, top
/// line first (the game draws job above name). The player draws one generous box
/// around both and each line becomes its own fingerprint region.
#[tauri::command]
pub fn split_identity(
    serial: u64,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<Vec<Region>, String> {
    log(&format!("split_identity serial={serial} box {x},{y} {w}x{h}"));
    let frame = cached_frame(serial)?;
    let region = Region { x, y, w, h };
    let gray = ocr::crop_gray(&frame, region).ok_or("the box is outside the frame")?;

    let mut lines = ocr::text_lines(&gray);
    log(&format!("split_identity: text_lines found {:?}", lines));

    if lines.len() != 2 {
        // The Ideal Ratio HUD draws the name and job in a 7 px font, which can leave
        // only one blank row between them — and the line grouping treats a gap under
        // two rows as being inside the same line. Split at the emptiest row inside the
        // box instead, so one drawn box still yields two regions.
        if let Some((split_row, _)) = {
            let ink = ocr::row_ink(&gray);
            let height = ink.len();
            if height >= 8 {
                let low = height / 4;
                let high = height - height / 4;
                (low..high).map(|row| (row, ink[row])).min_by_key(|(_, count)| *count)
            } else {
                None
            }
        } {
            let ink = ocr::row_ink(&gray);
            let bounds = |from: usize, to: usize| {
                let rows: Vec<usize> = (from..to).filter(|row| ink[*row] > 0).collect();
                rows.first().zip(rows.last()).map(|(first, last)| (*first, last + 1))
            };

            if let (Some((top_start, top_end)), Some((bottom_start, bottom_end))) =
                (bounds(0, split_row), bounds(split_row, ink.len()))
            {
                lines = vec![
                    (0, top_start as i32, gray.w as i32, (top_end - top_start) as i32),
                    (
                        0,
                        bottom_start as i32,
                        gray.w as i32,
                        (bottom_end - bottom_start) as i32,
                    ),
                ];
                log(&format!("split_identity: fell back to a row split {:?}", lines));
            }
        }
    }

    if lines.len() != 2 {
        log(&format!(
            "split_identity: still {} lines — refusing",
            lines.len()
        ));
        return Err(format!(
            "expected two lines (job over name) in the box, found {}",
            lines.len()
        ));
    }

    Ok(lines
        .into_iter()
        .map(|(lx, ly, lw, lh)| Region {
            x: x + lx,
            y: y + ly,
            w: lw,
            h: lh,
        })
        .collect())
}

/// One shape segmented out of the EXP box.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpGlyph {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub ch: String,
    pub distance: f64,
    pub known: bool,
    /// The normalised shape, so it can be labelled without re-cropping.
    pub cell: String,
}

/// The shapes in the EXP box, with what the atlas thinks each one is. Unknown
/// shapes are what the player labels.
#[tauri::command]
pub fn exp_glyphs(serial: u64) -> Result<Vec<ExpGlyph>, String> {
    let frame = cached_frame(serial)?;
    let calibration = settings::load_calibration();
    let region = calibration
        .exp
        .filter(Region::is_usable)
        .ok_or("save the EXP box first")?;
    let gray = ocr::crop_gray(&frame, region).ok_or("the EXP box is outside the window")?;
    let atlas = Atlas::from_settings(&settings::load_atlas());

    Ok(atlas
        .explain(&gray)
        .into_iter()
        .map(|guess| ExpGlyph {
            x: region.x + guess.x,
            y: region.y + guess.y,
            w: guess.w,
            h: guess.h,
            ch: guess.ch.to_string(),
            distance: guess.distance,
            known: guess.known,
            cell: settings::to_hex(&guess.cell),
        })
        .collect())
}

/// Label one shape as a character. The cell comes straight from `exp_glyphs`, so
/// the exact pixels the player looked at are what gets stored — no re-cropping,
/// which would reject one-pixel shapes like `.`.
#[tauri::command]
pub fn label_glyph(
    client_w: u32,
    client_h: u32,
    ch: String,
    cell: String,
    w: u32,
    h: u32,
) -> Result<LearningInfo, String> {
    let trimmed = ch.trim();
    let character = trimmed
        .chars()
        .next()
        .filter(|_| trimmed.chars().count() == 1)
        .ok_or("pick a single character")?;

    let bytes = settings::from_hex(&cell).ok_or("the shape data is invalid")?;
    if bytes.len() != ocr::CELL_LEN {
        return Err("the shape data is invalid".into());
    }
    let mut shape = [0u8; ocr::CELL_LEN];
    shape.copy_from_slice(&bytes);

    profile::activate(client_w, client_h)?;
    let mut atlas = Atlas::from_settings(&settings::load_atlas());
    atlas.learn_glyph(character, shape, w, h);
    settings::save_atlas(&atlas.to_settings())?;

    Ok(learning_info(&atlas))
}

/// Forget every taught map fingerprint for this size, so stale or bad ones can be
/// re-taught (e.g. after the label crop changed).
#[tauri::command]
pub fn clear_maps(client_w: u32, client_h: u32) -> Result<LearningInfo, String> {
    profile::activate(client_w, client_h)?;
    crate::profile::update(|profile| profile.maps.clear())?;
    Ok(learning_info(&Atlas::from_settings(&settings::load_atlas())))
}

/// Restore a size to its shipped preset: the boxes and glyph set are replaced,
/// while the per-player teaches (map, name, job) are kept.
#[tauri::command]
pub fn apply_preset(client_w: u32, client_h: u32) -> Result<(), String> {
    let ui_mode = crate::settings::ui_mode();
    let preset = crate::preset::for_size(client_w, client_h, ui_mode).ok_or_else(|| {
        format!(
            "no preset for {client_w}x{client_h}{}",
            ui_mode.suffix()
        )
    })?;
    crate::profile::activate(client_w, client_h)?;
    crate::profile::update(|profile| {
        // Only take the boxes the preset actually defines. A preset carries no
        // map/name/job boxes (they are per-player), so resetting must not wipe
        // boxes the player drew and taught.
        let source = &preset.calibration;
        if source.exp.is_some() {
            profile.calibration.exp = source.exp;
        }
        if source.level.is_some() {
            profile.calibration.level = source.level;
        }
        if source.map.is_some() {
            profile.calibration.map = source.map;
        }
        // The name/job boxes are layout, so presets ship them — but a player who drew
        // their own has fingerprints cropped to that box, and replacing it would stop
        // them matching. Only fill these in where the player has none.
        if source.name.is_some() && profile.calibration.name.is_none() {
            profile.calibration.name = source.name;
        }
        if source.job.is_some() && profile.calibration.job.is_none() {
            profile.calibration.job = source.job;
        }
        profile.calibration.client_w = source.client_w.or(profile.calibration.client_w);
        profile.calibration.client_h = source.client_h.or(profile.calibration.client_h);
        profile.atlas = preset.atlas.clone();
    })?;
    Ok(())
}

#[tauri::command]
pub fn open_calibration<R: Runtime>(app: AppHandle<R>) {
    open_calibration_window(&app);
}

/// Open (or reveal) the calibration window. Only ever reached from a deliberate
/// action — the tray item or the overlay's Calibrate button — so it takes focus.
///
/// The window must be built from the event loop, never synchronously inside the
/// IPC message handler: a webview created there comes up blank. Calling
/// `run_on_main_thread` directly is not enough because from the main thread it
/// runs the closure inline, still inside the handler — so hop through a thread
/// first, which guarantees the build is queued to the loop.
pub fn open_calibration_window<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let inner = handle.clone();
        let _ = handle.run_on_main_thread(move || open_calibration_window_on_main(&inner));
    });
}

pub(crate) fn open_calibration_window_on_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("calibration") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let result = tauri::WebviewWindowBuilder::new(
        app,
        "calibration",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("MScout — Calibration")
    .inner_size(980.0, 780.0)
    .min_inner_size(720.0, 560.0)
    .build();

    // A window that builds without a working webview shows up blank, so record
    // any failure rather than swallowing it.
    if let Err(error) = result {
        if let Ok(dir) = crate::data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(
                dir.join("window-error.log"),
                format!("calibration: {error}"),
            );
        }
    }
}

/// How many rejected frames to keep. Each is a few kilobytes.
const REVIEW_LIMIT: usize = 50;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRecord {
    pub t: i64,
    pub reason: String,
    pub w: u32,
    pub h: u32,
    pub px: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFrame {
    pub id: String,
    pub t: i64,
    pub reason: String,
    pub w: u32,
    pub h: u32,
    pub px: String,
}

fn review_dir() -> Result<std::path::PathBuf, String> {
    Ok(crate::data_dir()?.join("review"))
}

/// Append a line to `data/debug.log`. The calibration window has no console, so a
/// click that appears to do nothing has to leave a trace somewhere.
pub(crate) fn log(message: &str) {
    let Ok(dir) = crate::data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("debug.log"))
    else {
        return;
    };
    use std::io::Write;
    let _ = writeln!(file, "{} {message}", chrono::Local::now().format("%H:%M:%S"));
}

fn review_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    files
}

fn prune_reviews(dir: &std::path::Path) {
    let mut files = review_files(dir);
    while files.len() > REVIEW_LIMIT {
        let oldest = files.remove(0);
        let _ = std::fs::remove_file(oldest);
    }
}

/// A minimap label at full resolution, so the window can show what it is looking at.
/// Typing the map name blind is how a fingerprint ended up stored under a name that
/// belonged to a different map.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelCrop {
    pub w: u32,
    pub h: u32,
    pub px: String,
    pub lines: usize,
}

#[tauri::command]
pub fn map_label_crop(serial: u64) -> Result<LabelCrop, String> {
    let frame = cached_frame(serial)?;
    let panel = ocr::detect_minimap_panel(&frame).ok_or("the minimap panel was not found")?;
    let label = ocr::detect_map_label(&frame, panel).ok_or("the map label was not found")?;
    let gray = ocr::crop_gray(&frame, label).ok_or("the map label is outside the window")?;
    let lines = ocr::text_lines(&gray).len();

    Ok(LabelCrop {
        w: gray.w as u32,
        h: gray.h as u32,
        px: settings::to_hex(&gray.px),
        lines,
    })
}

/// Keep the EXP box of a frame the reader rejected. It is the training data that
/// matters: a frame the reader could not handle, which a person can label once and
/// turn into a glyph it will recognise from then on.
pub(crate) fn save_review(frame: &capture::Frame, reason: &str) {
    // Keyed to the frame's own size rather than the active profile, so it captures
    // the box that was actually being read.
    let Some(region) = profile::load(frame.width, frame.height)
        .and_then(|profile| profile.calibration.exp)
        .filter(Region::is_usable)
    else {
        return;
    };
    let Some(gray) = ocr::crop_gray(frame, region) else {
        return;
    };
    let Ok(dir) = review_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }

    let px = settings::to_hex(&gray.px);

    // A frame that fails stays on screen and fails again every second; keep it
    // once rather than fifty times.
    if let Some(newest) = review_files(&dir).pop() {
        if let Ok(text) = std::fs::read_to_string(newest) {
            if let Ok(record) = serde_json::from_str::<ReviewRecord>(&text) {
                if record.px == px {
                    return;
                }
            }
        }
    }

    let record = ReviewRecord {
        t: chrono::Utc::now().timestamp(),
        reason: reason.to_string(),
        w: gray.w as u32,
        h: gray.h as u32,
        px,
    };
    let Ok(text) = serde_json::to_string(&record) else {
        return;
    };
    let _ = std::fs::write(dir.join(format!("{}.json", record.t)), text);
    prune_reviews(&dir);
}

#[tauri::command]
pub fn review_frames() -> Vec<ReviewFrame> {
    let Ok(dir) = review_dir() else {
        return Vec::new();
    };

    let mut frames: Vec<ReviewFrame> = review_files(&dir)
        .into_iter()
        .filter_map(|path| {
            let id = path.file_stem()?.to_str()?.to_string();
            let text = std::fs::read_to_string(&path).ok()?;
            let record: ReviewRecord = serde_json::from_str(&text).ok()?;
            Some(ReviewFrame {
                id,
                t: record.t,
                reason: record.reason,
                w: record.w,
                h: record.h,
                px: record.px,
            })
        })
        .collect();

    frames.sort_by(|a, b| b.t.cmp(&a.t));
    frames
}

/// Label a rejected frame: learn its glyphs into the atlas for the size in use,
/// then drop the frame. The typed text is the ground truth the frame was missing.
#[tauri::command]
pub fn label_review(id: String, text: String) -> Result<LearningInfo, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("type what the box shows".into());
    }

    let path = review_dir()?.join(format!("{id}.json"));
    let raw = std::fs::read_to_string(&path).map_err(|_| "that frame is gone".to_string())?;
    let record: ReviewRecord = serde_json::from_str(&raw).map_err(|e| e.to_string())?;

    let px = settings::from_hex(&record.px).ok_or("the stored frame is unreadable")?;
    if px.len() != record.w as usize * record.h as usize {
        return Err("the stored frame is the wrong size".into());
    }

    let mut atlas = Atlas::from_settings(&settings::load_atlas());
    atlas.learn(
        &text,
        &ocr::Gray {
            w: record.w as usize,
            h: record.h as usize,
            px,
        },
    )?;
    settings::save_atlas(&atlas.to_settings())?;
    let _ = std::fs::remove_file(&path);

    Ok(learning_info(&atlas))
}

#[tauri::command]
pub fn clear_review() -> Result<usize, String> {
    let dir = review_dir()?;
    let mut removed = 0usize;
    for path in review_files(&dir) {
        if std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}
