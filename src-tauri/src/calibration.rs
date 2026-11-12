use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
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

    LearningInfo {
        learned: atlas.learned(),
        coverage,
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
    *lock_preview() = Some(CachedPreview {
        serial,
        frame: frame.clone(),
    });

    let mut bytes = Vec::with_capacity(12 + frame.rgba.len());
    bytes.extend_from_slice(&frame.width.to_le_bytes());
    bytes.extend_from_slice(&frame.height.to_le_bytes());
    bytes.extend_from_slice(&serial.to_le_bytes());
    bytes.extend_from_slice(&frame.rgba);
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub fn get_calibration() -> Calibration {
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
    if name.is_empty() {
        return Err("type the map name first".into());
    }

    let frame = cached_frame(serial)?;
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
        label: name,
        w: gray.w as u32,
        h: gray.h as u32,
        pixels: settings::to_hex(&gray.px),
    });
    settings::save_maps(&maps)?;

    Ok(learning_info(&Atlas::from_settings(&settings::load_atlas())))
}

#[tauri::command]
pub fn teach_identity(serial: u64, kind: String, label: String) -> Result<LearningInfo, String> {
    let label = label.trim().to_string();
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
    .filter(Region::is_usable)
    .ok_or("save that box first")?;

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
        return Err("save the EXP and level boxes first".into());
    }

    let atlas = Atlas::from_settings(&settings::load_atlas());
    let maps = settings::load_maps();
    let identities = settings::load_identities();
    engine::read_frame(&frame, &calibration, &atlas, &maps, &identities)
}

#[tauri::command]
pub fn open_calibration<R: Runtime>(app: AppHandle<R>) {
    open_calibration_window(&app);
}

pub fn open_calibration_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("calibration") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "calibration",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("MScout — Calibration")
    .inner_size(980.0, 780.0)
    .min_inner_size(720.0, 560.0)
    .build();
}
