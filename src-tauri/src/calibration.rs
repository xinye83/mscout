use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::capture::{self, Frame};
use crate::engine::{self, FieldProvenance, ReadProvenance, Readings};
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
    /// The level readout is drawn at its own size and keeps its own glyph set, so its
    /// coverage is reported separately from the EXP one.
    pub level_learned: String,
    pub level_coverage: String,
    /// Characters of the minimap label font learned so far. Once these cover the
    /// letters a map name uses, the label reads as text with no fingerprint.
    pub map_letters: String,
    pub maps: Vec<String>,
    pub name: Option<String>,
    pub job: Option<String>,
    /// `game` when seeded from the client atlas, `taught` when the player typed
    /// the digits, empty for a preset or a legacy set.
    pub source: String,
    /// The most common stored glyph size, e.g. `5x7`.
    pub glyph_size: String,
    /// Where the level set came from. It is seeded from the same client atlas as
    /// EXP but at its own rendered size, so it is reported separately.
    pub level_source: String,
    /// Most common stored level glyph size, e.g. `7x7`.
    pub level_size: String,
    /// Characters in the name/job letter atlas.
    pub identity_letters: String,
    /// `font` when the name/job atlas was seeded from Arial, `taught` when the
    /// player corrected it, empty when nothing is seeded.
    pub identity_source: String,
    /// Most common identity glyph size, e.g. `5x7` (seeded from Arial 12 px).
    pub identity_size: String,
}

fn digit_coverage(atlas: &Atlas) -> String {
    "0123456789"
        .chars()
        .filter(|ch| atlas.glyphs.iter().any(|glyph| glyph.ch == *ch))
        .collect()
}

/// The learning view for a stored atlas. Everything shown comes from what is on
/// disk, so the window cannot report a different source than the reader uses.
fn learning_info(stored: &settings::Atlas) -> LearningInfo {
    let atlas = Atlas::from_settings(stored);
    let level_stored = settings::load_level_atlas();
    let level_atlas = Atlas::from_settings(&level_stored);
    let level_size = level_stored.common_glyph_size();
    let map_atlas = Atlas::from_settings(&settings::load_map_atlas());
    let identity_stored = settings::load_identity_atlas();
    let identity_atlas = Atlas::from_settings(&identity_stored);
    let identities = settings::load_identities();

    LearningInfo {
        learned: atlas.learned(),
        coverage: digit_coverage(&atlas),
        level_learned: level_atlas.learned(),
        level_coverage: digit_coverage(&level_atlas),
        map_letters: map_atlas.learned(),
        maps: settings::load_maps()
            .into_iter()
            .map(|template| template.label)
            .collect(),
        name: identities.name.map(|template| template.label),
        job: identities.job.map(|template| template.label),
        source: stored.source.clone(),
        glyph_size: stored.common_glyph_size(),
        level_source: level_stored.source,
        level_size,
        identity_letters: identity_atlas.learned(),
        identity_source: identity_stored.source.clone(),
        identity_size: identity_stored.common_glyph_size(),
    }
}

// The frame-reading commands below are async: a plain command runs on the main
// thread, so a capture, a WZ atlas parse or a full read there froze every window,
// the overlay included, for as long as it took.
#[tauri::command(async)]
pub fn capture_preview(
    state: State<'_, window_target::TargetState>,
) -> Result<tauri::ipc::Response, String> {
    let selector = state.selector().ok_or("attach a game window first")?;
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
pub fn get_calibration(state: State<'_, window_target::TargetState>) -> Calibration {
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
#[allow(clippy::too_many_arguments)] // the IPC payload is one flat object per box
pub fn set_calibration(
    client_w: u32,
    client_h: u32,
    exp: Option<Region>,
    level: Option<Region>,
    street: Option<Region>,
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
        && street.is_none()
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
        street,
        map,
        name,
        job,
    };
    settings::save_calibration(&calibration)?;
    Ok(calibration)
}

#[tauri::command]
pub fn get_learning() -> LearningInfo {
    learning_info(&settings::load_atlas())
}

/// Make the profile for this preview frame active, so every teach/probe command
/// writes to the calibration set belonging to that resolution.
fn use_frame_profile(frame: &capture::Frame) -> Result<crate::profile::Profile, String> {
    profile::activate(frame.width, frame.height)
}

#[tauri::command]
pub fn learn_atlas(serial: u64, text: String) -> Result<LearningInfo, String> {
    learn_region(serial, text, "exp")
}

/// The level readout is drawn at its own size, so its glyph variants usually have
/// to be learned separately from the EXP text's even though it is the same alphabet.
#[tauri::command]
pub fn learn_level_atlas(serial: u64, text: String) -> Result<LearningInfo, String> {
    learn_region(serial, text, "level")
}

fn learn_region(serial: u64, text: String, target: &str) -> Result<LearningInfo, String> {
    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();
    let level = target == "level";
    let (region, what) = if level {
        (calibration.level, "level")
    } else {
        (calibration.exp, "EXP")
    };
    let region = region
        .filter(Region::is_usable)
        .ok_or_else(|| format!("save the {what} box first"))?;
    let gray = ocr::crop_gray(&frame, region)
        .ok_or_else(|| format!("the {what} box is outside the window"))?;

    log(&format!(
        "learn_region target={target} box={:?} crop {}x{} text={text:?}",
        region, gray.w, gray.h
    ));

    // Each readout owns its atlas — see `Profile::level_atlas`. Learning level
    // glyphs into the EXP atlas used to change how EXP was read, and vice versa.
    let mut atlas = if level {
        Atlas::from_settings(&settings::load_level_atlas())
    } else {
        Atlas::from_settings(&settings::load_atlas())
    };
    if let Err(error) = atlas.learn(&text, &gray) {
        // Without this the failed teach left no trace at all, which is why a box
        // that could not be learned looked like the button doing nothing.
        log(&format!("learn_region failed: {error}"));
        return Err(error);
    }
    let stored = atlas.to_settings();
    if level {
        settings::save_level_atlas(&stored)?;
    } else {
        settings::save_atlas(&stored)?;
    }
    log(&format!(
        "learn_region: atlas now {} glyphs",
        atlas.glyphs.len()
    ));

    Ok(learning_info(&stored))
}

/// Seed the EXP atlas from the installed client's bitmap atlas. The rendered
/// size comes from the live EXP box, so the same client atlas serves every
/// resolution that draws at that size.
#[tauri::command(async)]
pub fn seed_atlas(serial: u64) -> Result<LearningInfo, String> {
    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();
    let region = calibration
        .exp
        .filter(Region::is_usable)
        .ok_or("save the EXP box first")?;
    let gray = ocr::crop_gray(&frame, region).ok_or("the EXP box is outside the window")?;
    let dir = settings::mscw_dir().ok_or("choose your MapleStory MSCW folder first")?;
    let (stored, height) = crate::game_font::seed_exp_atlas(&gray, std::path::Path::new(&dir))?;
    settings::save_atlas(&stored)?;

    // The level box is optional, but when it is drawn and reads digits, seed it
    // too — it is the same font drawn wider, so it needs no teaching.
    let mut level_note = String::new();
    if let Some(level_region) = calibration.level.filter(Region::is_usable) {
        match crate::game_font::seed_level_best(&frame, level_region, std::path::Path::new(&dir)) {
            Some((level_stored, (w, h), _text)) => {
                settings::save_level_atlas(&level_stored)?;
                level_note = format!(", level {w}x{h}");
            }
            None => log("seed_atlas: level not seeded"),
        }
    }

    // Name/job are Arial and can be seeded from the same frozen frame. A taught
    // set is left alone; a font set is refreshed, since the size is measured.
    let identity_stored = settings::load_identity_atlas();
    let mut identity_note = String::new();
    if identity_stored.glyphs.is_empty() || identity_stored.source == "font" {
        for region in [calibration.job, calibration.name] {
            let Some(region) = region.filter(Region::is_usable) else {
                continue;
            };
            let Some(identity_gray) = ocr::crop_gray(&frame, region) else {
                continue;
            };
            if let Some((identity, px)) = crate::font_seed::seed_identity(&identity_gray) {
                settings::save_identity_atlas(&identity)?;
                identity_note = format!(", name/job {px}px");
                break;
            }
        }
    }

    log(&format!(
        "seed_atlas: {} glyphs at h={height}{level_note}{identity_note} from {dir}",
        stored.glyphs.len()
    ));
    Ok(learning_info(&stored))
}

#[tauri::command(async)]
pub fn teach_map(serial: u64, name: String) -> Result<LearningInfo, String> {
    let name = name.trim().to_string();
    log(&format!("teach_map serial={serial} name={name:?}"));
    if name.is_empty() {
        return Err("type the map name first".into());
    }

    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();

    // The map is identified by the whole two-line label, so fingerprint that. The
    // label stored is just the map name; the street is taught and matched on its own.
    let gray = ocr::map_label(&frame, calibration.map_box())
        .ok_or("could not find the minimap — draw the map box first")?;

    let mut maps = settings::load_maps();
    maps.retain(|template| template.label != name);
    maps.push(settings::Template {
        label: name.clone(),
        w: gray.w as u32,
        h: gray.h as u32,
        pixels: settings::to_hex(&gray.px),
    });
    settings::save_maps(&maps)?;

    // Learn the map-name line (the bottom one) into the label letter atlas.
    if let Some(line) = ocr::map_label_lines(&frame).and_then(|lines| lines.last().cloned()) {
        let mut map_atlas = Atlas::from_settings(&settings::load_map_atlas());
        let _ = map_atlas.learn_text(&name, &line);
        settings::save_map_atlas(&map_atlas.to_settings())?;
        log(&format!(
            "teach_map: learned letters {:?}",
            Atlas::from_settings(&settings::load_map_atlas()).learned()
        ));
    }

    Ok(learning_info(&settings::load_atlas()))
}

/// Taught separately from the map: a street is shared by several maps, so it is
/// fingerprinted and matched on its own line.
#[tauri::command(async)]
pub fn teach_street(serial: u64, street: String) -> Result<LearningInfo, String> {
    let street = street.trim().to_string();
    log(&format!("teach_street serial={serial} street={street:?}"));
    if street.is_empty() {
        return Err("type the street name first".into());
    }

    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();

    let region = calibration
        .street
        .filter(Region::is_usable)
        .ok_or("draw the street box first, then teach")?;
    let gray = ocr::crop_gray(&frame, region).ok_or("the street box is outside the window")?;

    let mut streets = settings::load_streets();
    streets.retain(|template| template.label != street);
    streets.push(settings::Template {
        label: street.clone(),
        w: gray.w as u32,
        h: gray.h as u32,
        pixels: settings::to_hex(&gray.px),
    });
    settings::save_streets(&streets)?;

    // Learn the street line (the top one) into the label letter atlas.
    if let Some(line) = ocr::map_label_lines(&frame).and_then(|lines| lines.first().cloned()) {
        let mut map_atlas = Atlas::from_settings(&settings::load_map_atlas());
        let _ = map_atlas.learn_text(&street, &line);
        settings::save_map_atlas(&map_atlas.to_settings())?;
    }

    Ok(learning_info(&settings::load_atlas()))
}

#[tauri::command(async)]
pub fn teach_identity(serial: u64, kind: String, label: String) -> Result<LearningInfo, String> {
    let label = label.trim().to_string();
    log(&format!(
        "teach_identity serial={serial} kind={kind} label={label:?}"
    ));
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

    Ok(learning_info(&settings::load_atlas()))
}

/// The calibration window's probe.
///
/// The provenance and the rejection reason travel beside the readings, so a frame
/// that was rejected still shows which stage was tried and what stopped it. A
/// rejected frame has no readings, which is why they are optional.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub readings: Option<Readings>,
    pub provenance: ReadProvenance,
    pub rejected: Option<String>,
}

#[tauri::command(async)]
pub fn parse_probe(serial: u64) -> Result<Probe, String> {
    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();
    if !calibration.is_ready() {
        return Err("save the EXP box first".into());
    }

    let exp_atlas = settings::load_atlas();
    let level_atlas = settings::load_level_atlas();
    let identity_atlas = settings::load_identity_atlas();
    let (readings, mut provenance, rejected) = match engine::read_frame_detailed(
        &frame,
        &calibration,
        &Atlas::from_settings(&exp_atlas),
        &Atlas::from_settings(&level_atlas),
        &Atlas::from_settings(&identity_atlas),
        &settings::load_maps(),
        &settings::load_streets(),
        &settings::load_identities(),
    ) {
        Ok(readings) => {
            let provenance = readings.provenance.clone();
            (Some(readings), provenance, None)
        }
        Err(failed) => {
            let engine::RejectedRead { reason, provenance } = *failed;
            (None, provenance, Some(reason))
        }
    };
    // The reader sees the glyph sets only; the window shows where each set came
    // from, so stamp the stored origin beside the read. A rejected frame is
    // stamped too: that is the frame a support case asks about.
    provenance.stamp_engines(
        &exp_atlas.source,
        &level_atlas.source,
        &identity_atlas.source,
    );
    Ok(Probe {
        readings,
        provenance,
        rejected,
    })
}

/// Write a local, support-friendly snapshot of the current calibration read:
/// raw readout text, glyph candidates, map label crops, the frame's dimensions,
/// and where each field of the read came from. This is the "why did it not read?"
/// artefact; it contains no full screenshot and is written only under
/// `data/diagnostics/`.
#[tauri::command(async)]
pub fn save_diagnostic(serial: u64) -> Result<String, String> {
    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();
    let exp_atlas = settings::load_atlas();
    let level_atlas = settings::load_level_atlas();
    let identity_atlas = settings::load_identity_atlas();
    let started = std::time::Instant::now();
    // Where the stored atlases came from, stamped on every arm below: the frame
    // that failed to read is exactly the one where it matters whether the digits
    // were `game`, `taught` or a stale preset.
    let stamp = |provenance: &mut ReadProvenance| {
        provenance.stamp_engines(
            &exp_atlas.source,
            &level_atlas.source,
            &identity_atlas.source,
        );
    };
    let (readings, provenance, rejected) = if calibration.is_ready() {
        match engine::read_frame_detailed(
            &frame,
            &calibration,
            &Atlas::from_settings(&exp_atlas),
            &Atlas::from_settings(&level_atlas),
            &Atlas::from_settings(&identity_atlas),
            &settings::load_maps(),
            &settings::load_streets(),
            &settings::load_identities(),
        ) {
            Ok(mut readings) => {
                stamp(&mut readings.provenance);
                let provenance = serde_json::json!(readings.provenance);
                (serde_json::json!(readings), provenance, None)
            }
            // A rejected frame is the one most worth having on disk, so keep the
            // reason and whatever provenance the read had gathered.
            Err(failed) => {
                let engine::RejectedRead {
                    reason,
                    mut provenance,
                } = *failed;
                stamp(&mut provenance);
                (
                    serde_json::json!({ "error": reason }),
                    serde_json::json!(provenance),
                    Some(reason),
                )
            }
        }
    } else {
        let mut provenance = ReadProvenance {
            exp: FieldProvenance::rejected("the EXP box is not set, so nothing was read"),
            ..ReadProvenance::default()
        };
        stamp(&mut provenance);
        (
            serde_json::json!({ "error": "save the EXP box first" }),
            serde_json::json!(provenance),
            Some("save the EXP box first".to_string()),
        )
    };
    let lines = ocr::map_label_line_regions(&frame)
        .unwrap_or_default()
        .into_iter()
        .map(|(region, gray)| {
            serde_json::json!({
                "region": region,
                "w": gray.w,
                "h": gray.h,
                "px": settings::to_hex(&gray.px),
            })
        })
        .collect::<Vec<_>>();
    // The name/job crops exactly as the reader saw them, so a misread letter (an
    // `i` read as `l` or `j`) can be reproduced from the file alone.
    let identity_crops = [("job", calibration.job), ("name", calibration.name)]
        .into_iter()
        .filter_map(|(kind, region)| {
            let region = region.filter(Region::is_usable)?;
            let gray = ocr::crop_gray(&frame, region)?;
            Some(serde_json::json!({
                "kind": kind,
                "region": region,
                "w": gray.w,
                "h": gray.h,
                "px": settings::to_hex(&gray.px),
            }))
        })
        .collect::<Vec<_>>();
    // The HUD strip around every read box, as raw RGBA. It is lossless, so a
    // misread frame can become a `testdata/` fixture without a second capture.
    let hud_strip = hud_strip_region(&frame, &calibration).and_then(|region| {
        let rgba = crop_rgba(&frame, region)?;
        Some(serde_json::json!({
            "region": region,
            "rgba": settings::to_hex(&rgba),
        }))
    });
    let payload = serde_json::json!({
        "version": 2,
        "capturedAt": chrono::Local::now().to_rfc3339(),
        "readMs": started.elapsed().as_secs_f64() * 1000.0,
        // The stage list, not an atlas origin: `provenance.*.engine` below answers
        // "where did this stage's data come from", so the two are kept apart by name.
        "stages": "hud-atlas+map-table+dtw",
        "frame": { "w": frame.width, "h": frame.height },
        "profile": crate::profile::active_key(),
        "uiMode": settings::ui_mode().id(),
        "calibration": calibration,
        "readings": readings,
        // Where each field came from, and why an empty one is empty. Repeated
        // outside `readings` so a rejected frame (which has no readings) still
        // reports it.
        "provenance": provenance,
        "rejected": rejected,
        "expGlyphs": box_glyphs(serial, "exp".into()).unwrap_or_default(),
        "levelGlyphs": box_glyphs(serial, "level".into()).unwrap_or_default(),
        "mapLines": lines,
        "identityCrops": identity_crops,
        "hudStrip": hud_strip,
        "atlas": {
            "expGlyphs": exp_atlas.glyphs.len(),
            "expSize": exp_atlas.common_glyph_size(),
            "levelGlyphs": level_atlas.glyphs.len(),
            "levelSize": level_atlas.common_glyph_size(),
            "identityGlyphs": identity_atlas.glyphs.len(),
            "identitySize": identity_atlas.common_glyph_size(),
        },
    });
    let dir = crate::data_dir()?.join("diagnostics");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join(format!(
        "read-{}.json",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(path.display().to_string())
}

/// Margin kept around the read boxes in a diagnostic's HUD strip, so a fixture
/// cut from it is not flush to the glyphs.
const HUD_STRIP_MARGIN: i32 = 8;

/// The union of the level, job, name and EXP boxes plus a margin, clamped to the
/// frame. `None` when no box is set.
fn hud_strip_region(frame: &Frame, calibration: &settings::Calibration) -> Option<Region> {
    let boxes = [
        calibration.level,
        calibration.job,
        calibration.name,
        calibration.exp,
    ];
    let mut bounds: Option<(i32, i32, i32, i32)> = None;
    for region in boxes.into_iter().flatten().filter(Region::is_usable) {
        let (x0, y0, x1, y1) = (region.x, region.y, region.x + region.w, region.y + region.h);
        bounds = Some(match bounds {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        });
    }
    let (x0, y0, x1, y1) = bounds?;
    let x0 = (x0 - HUD_STRIP_MARGIN).max(0);
    let y0 = (y0 - HUD_STRIP_MARGIN).max(0);
    let x1 = (x1 + HUD_STRIP_MARGIN).min(frame.width as i32);
    let y1 = (y1 + HUD_STRIP_MARGIN).min(frame.height as i32);
    (x1 > x0 && y1 > y0).then_some(Region {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// The frame's RGBA bytes inside `region`, row by row. `None` when the region
/// leaves the frame.
fn crop_rgba(frame: &Frame, region: Region) -> Option<Vec<u8>> {
    if region.x < 0
        || region.y < 0
        || region.w <= 0
        || region.h <= 0
        || (region.x + region.w) as u32 > frame.width
        || (region.y + region.h) as u32 > frame.height
    {
        return None;
    }
    let stride = frame.width as usize * 4;
    let mut out = Vec::with_capacity(region.w as usize * region.h as usize * 4);
    for y in region.y as usize..(region.y + region.h) as usize {
        let start = y * stride + region.x as usize * 4;
        out.extend_from_slice(&frame.rgba[start..start + region.w as usize * 4]);
    }
    Some(out)
}

/// Split a single name/job box into the two stacked lines it encloses, top
/// line first (the game draws job above name). The player draws one generous box
/// around both and each line becomes its own fingerprint region.
#[tauri::command]
pub fn split_identity(serial: u64, x: i32, y: i32, w: i32, h: i32) -> Result<Vec<Region>, String> {
    log(&format!(
        "split_identity serial={serial} box {x},{y} {w}x{h}"
    ));
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
                (low..high)
                    .map(|row| (row, ink[row]))
                    .min_by_key(|(_, count)| *count)
            } else {
                None
            }
        } {
            let ink = ocr::row_ink(&gray);
            let bounds = |from: usize, to: usize| {
                let rows: Vec<usize> = (from..to).filter(|row| ink[*row] > 0).collect();
                rows.first()
                    .zip(rows.last())
                    .map(|(first, last)| (*first, last + 1))
            };

            if let (Some((top_start, top_end)), Some((bottom_start, bottom_end))) =
                (bounds(0, split_row), bounds(split_row, ink.len()))
            {
                lines = vec![
                    (
                        0,
                        top_start as i32,
                        gray.w as i32,
                        (top_end - top_start) as i32,
                    ),
                    (
                        0,
                        bottom_start as i32,
                        gray.w as i32,
                        (bottom_end - bottom_start) as i32,
                    ),
                ];
                log(&format!(
                    "split_identity: fell back to a row split {:?}",
                    lines
                ));
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
    /// The normalised native shape, so it can be labelled without re-cropping.
    pub bitmap: String,
}

/// Every shape in a box with its best guess, so one that cannot be segmented can be
/// labelled by eye. `kind` selects both the box and the atlas — each readout owns
/// its own glyph set, so a shape from the level box must be judged by the level
/// glyphs, not the EXP ones.
#[tauri::command]
pub fn box_glyphs(serial: u64, kind: String) -> Result<Vec<ExpGlyph>, String> {
    let frame = cached_frame(serial)?;
    let calibration = settings::load_calibration();
    let level = kind == "level";
    let (region, atlas) = if level {
        (
            calibration.level,
            Atlas::from_settings(&settings::load_level_atlas()),
        )
    } else {
        (
            calibration.exp,
            Atlas::from_settings(&settings::load_atlas()),
        )
    };
    let region = region
        .filter(Region::is_usable)
        .ok_or_else(|| format!("save the {kind} box first"))?;
    let gray = ocr::crop_gray(&frame, region)
        .ok_or_else(|| format!("the {kind} box is outside the window"))?;

    let guesses = if level {
        atlas.explain_level(&gray)
    } else {
        atlas.explain(&gray)
    };
    Ok(guesses
        .into_iter()
        .map(|guess| ExpGlyph {
            x: region.x + guess.x,
            y: region.y + guess.y,
            w: guess.w,
            h: guess.h,
            ch: guess.ch.to_string(),
            distance: guess.distance,
            known: guess.known,
            bitmap: settings::to_hex(&guess.bitmap),
        })
        .collect())
}

#[tauri::command]
pub fn exp_glyphs(serial: u64) -> Result<Vec<ExpGlyph>, String> {
    box_glyphs(serial, "exp".into())
}

/// Label one shape as a character. The native bitmap comes straight from
/// `box_glyphs`, so the exact pixels the player looked at are what gets stored —
/// no re-cropping, which would reject one-pixel shapes like `.`.
#[tauri::command]
pub fn label_glyph(
    client_w: u32,
    client_h: u32,
    kind: String,
    ch: String,
    bitmap: String,
    w: u32,
    h: u32,
) -> Result<LearningInfo, String> {
    let trimmed = ch.trim();
    let character = trimmed
        .chars()
        .next()
        .filter(|_| trimmed.chars().count() == 1)
        .ok_or("pick a single character")?;

    let bytes = settings::from_hex(&bitmap).ok_or("the shape data is invalid")?;
    if w == 0 || h == 0 || bytes.len() != w as usize * h as usize {
        return Err("the shape data is invalid".into());
    }

    profile::activate(client_w, client_h)?;
    // Each readout owns its glyph set, so the shape is stored where the reader that
    // produced it will look for it.
    let level = kind == "level";
    let stored = if level {
        settings::load_level_atlas()
    } else {
        settings::load_atlas()
    };
    let source = stored.source.clone();
    let mut atlas = Atlas::from_settings(&stored);
    atlas.learn_glyph(character, bytes, w, h);
    let mut updated = atlas.to_settings();
    // A correction on a game-seeded set stays game-sourced, so the UI keeps
    // showing "auto" and does not fall back to the type-the-readout flow.
    updated.source = if source.is_empty() {
        "taught".into()
    } else {
        source
    };
    if level {
        settings::save_level_atlas(&updated)?;
    } else {
        settings::save_atlas(&updated)?;
    }
    Ok(learning_info(&updated))
}

/// Forget every taught map fingerprint for this size, so stale or bad ones can be
/// re-taught (e.g. after the label crop changed).
#[tauri::command]
pub fn clear_maps(client_w: u32, client_h: u32) -> Result<LearningInfo, String> {
    profile::activate(client_w, client_h)?;
    crate::profile::update(|profile| profile.maps.clear())?;
    Ok(learning_info(&settings::load_atlas()))
}

/// Restore a size to its shipped preset: the boxes are replaced and both digit
/// atlases are cleared, so the next frame reseeds them from the installed client
/// (or they can be taught again). The per-player teaches (map, name, job) are kept.
#[tauri::command]
pub fn apply_preset(client_w: u32, client_h: u32) -> Result<(), String> {
    let ui_mode = crate::settings::ui_mode();
    let preset = crate::preset::for_size(client_w, client_h, ui_mode)
        .ok_or_else(|| format!("no preset for {client_w}x{client_h}{}", ui_mode.suffix()))?;
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
        // Presets ship no glyph art, so this clears the digits. Both readouts are
        // reset together: a preset that reseeded EXP but kept a stale level set
        // would leave the two half-calibrated.
        profile.atlas = preset.atlas.clone();
        profile.level_atlas = preset.level_atlas.clone();
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
    let _ = writeln!(
        file,
        "{} {message}",
        chrono::Local::now().format("%H:%M:%S")
    );
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

/// The crop of any box, so every card in the calibration window can show what its
/// box actually covers. `map` and `street` use the line detector, matching the
/// overlay; a saved box is only the fallback when detection fails.
#[tauri::command]
pub fn box_crop(serial: u64, kind: String) -> Result<LabelCrop, String> {
    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    let calibration = settings::load_calibration();

    let (gray, lines) = match kind.as_str() {
        "street" | "map" => {
            let map_line = kind == "map";
            // The line detector is what the overlay reads. Prefer its regions over
            // legacy saved boxes: an older profile can hold the whole two-line label
            // as `map`, which shows the wrong pixels for both cards.
            if let Some(lines) = ocr::map_label_line_regions(&frame) {
                let picked = if map_line {
                    lines.last()
                } else if lines.len() > 1 {
                    lines.first()
                } else {
                    return Err("the street line was not found".into());
                };
                if let Some((_, gray)) = picked {
                    return Ok(LabelCrop {
                        w: gray.w as u32,
                        h: gray.h as u32,
                        px: settings::to_hex(&gray.px),
                        lines: if map_line { 1 } else { 0 },
                    });
                }
            }

            // Detection failed (for example the minimap is folded): fall back to a
            // saved split box.
            let split = calibration.street.filter(Region::is_usable).is_some();
            let saved = if map_line {
                if split {
                    calibration.map.filter(Region::is_usable)
                } else {
                    None
                }
            } else {
                calibration.street.filter(Region::is_usable)
            };
            if let Some(region) = saved {
                let gray = ocr::crop_gray(&frame, region)
                    .ok_or_else(|| format!("the {kind} box is outside the window"))?;
                let lines = if map_line {
                    ocr::text_lines(&gray).len()
                } else {
                    0
                };
                (gray, lines)
            } else {
                // Nothing saved and nothing detected.
                return Err(format!("the {kind} line was not found"));
            }
        }
        other => {
            let region = match other {
                "exp" => calibration.exp,
                "level" => calibration.level,
                "name" => calibration.name,
                "job" => calibration.job,
                _ => return Err(format!("unknown box '{other}'")),
            }
            .filter(Region::is_usable)
            .ok_or_else(|| format!("save the {other} box first"))?;
            let gray = ocr::crop_gray(&frame, region)
                .ok_or_else(|| format!("the {other} box is outside the window"))?;
            (gray, 0)
        }
    };

    Ok(LabelCrop {
        w: gray.w as u32,
        h: gray.h as u32,
        px: settings::to_hex(&gray.px),
        lines,
    })
}

/// The shipped preset's boxes for this client size, if one exists. The calibration
/// window uses it to tell the player's own boxes apart from geometry that merely
/// came from the preset.
#[tauri::command]
pub fn preset_calibration(client_w: u32, client_h: u32) -> Option<Calibration> {
    let ui_mode = settings::ui_mode();
    crate::preset::for_size(client_w, client_h, ui_mode).map(|profile| profile.calibration)
}

/// The exact line regions the map reader detects, so the calibration window can
/// show and store the boxes the overlay is actually using.
#[tauri::command]
pub fn map_line_regions(serial: u64) -> Result<Vec<Region>, String> {
    let frame = cached_frame(serial)?;
    use_frame_profile(&frame)?;
    Ok(ocr::map_label_line_regions(&frame)
        .map(|lines| lines.into_iter().map(|(region, _)| region).collect())
        .unwrap_or_default())
}

/// The map box the detector would use for this frame, so the calibration window can
/// show one where the profile has none. Nothing is stored: the player can adjust the
/// box and save it, and a saved box then wins over detection.
#[tauri::command]
pub fn map_region(serial: u64) -> Result<Option<Region>, String> {
    let frame = cached_frame(serial)?;
    if let Some(lines) = ocr::map_label_line_regions(&frame) {
        let left = lines.iter().map(|(region, _)| region.x).min();
        let top = lines.iter().map(|(region, _)| region.y).min();
        let right = lines.iter().map(|(region, _)| region.x + region.w).max();
        let bottom = lines.iter().map(|(region, _)| region.y + region.h).max();
        if let (Some(left), Some(top), Some(right), Some(bottom)) = (left, top, right, bottom) {
            return Ok(Some(Region {
                x: left,
                y: top,
                w: right - left,
                h: bottom - top,
            }));
        }
    }

    let Some(panel) = ocr::detect_minimap_panel(&frame) else {
        return Ok(None);
    };
    Ok(ocr::detect_map_label(&frame, panel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hud_strip_wraps_every_box_and_stays_in_the_frame() {
        let frame = Frame {
            width: 20,
            height: 10,
            rgba: (0..20 * 10 * 4).map(|i| i as u8).collect(),
        };
        let calibration = Calibration {
            level: Some(Region {
                x: 2,
                y: 4,
                w: 3,
                h: 3,
            }),
            exp: Some(Region {
                x: 10,
                y: 5,
                w: 6,
                h: 2,
            }),
            ..Calibration::default()
        };
        let region = hud_strip_region(&frame, &calibration).expect("region");
        assert_eq!(
            region,
            Region {
                x: 0,
                y: 0,
                w: 20,
                h: 10
            }
        );
        let inner = Region {
            x: 1,
            y: 2,
            w: 2,
            h: 1,
        };
        let start = (2 * 20 + 1) * 4;
        assert_eq!(
            crop_rgba(&frame, inner).expect("crop"),
            frame.rgba[start..start + 8].to_vec()
        );
        assert!(crop_rgba(
            &frame,
            Region {
                x: 19,
                y: 0,
                w: 2,
                h: 1
            }
        )
        .is_none());
        assert!(hud_strip_region(&frame, &Calibration::default()).is_none());
    }
}
