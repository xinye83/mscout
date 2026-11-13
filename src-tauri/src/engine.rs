use std::cmp::Ordering;
use std::collections::VecDeque;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::capture::{self, Capturer};
use crate::capture::Frame;
use crate::ocr::{self, Atlas};
use crate::settings::{self, Calibration, Identities, Region, Template};
use crate::profile;
use crate::window_target::{self, TargetStatus};
use crate::exp_table;

const OUTCOME_WINDOW: usize = 10;
const DEGRADED_FAILURES: usize = 3;
const CROSS_CHECK_TOLERANCE: f64 = 1.0;
const MAP_DISTANCE_LIMIT: f64 = 24.0;
/// How close a read label has to be to a taught one before the taught name wins.
/// Letters are read one glyph at a time, so a near miss is expected now and then.
const MAP_SIMILARITY: f64 = 0.75;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub t: i64,
    pub lv: u32,
    pub exp: i64,
    pub pct: f64,
    pub map: String,
    pub state: String,
    pub conf: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Readings {
    pub lv: u32,
    pub exp: i64,
    pub pct: f64,
    pub expected_pct: f64,
    pub map: String,
    pub conf: f64,
    pub exp_text: String,
    pub level_text: String,
    pub name: Option<String>,
    pub job: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: Option<String>,
    pub job: Option<String>,
}

/// A successful tick: the reading plus whether it may be recorded yet.
struct TickOutcome {
    sample: Sample,
    profile: Profile,
    verified: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub state: String,
    pub reason: Option<String>,
}

impl CaptureStatus {
    fn reading() -> Self {
        Self {
            state: "reading".into(),
            reason: None,
        }
    }

    fn degraded(reason: Option<String>) -> Self {
        Self {
            state: "degraded".into(),
            reason,
        }
    }

    fn lost(reason: impl Into<String>) -> Self {
        Self {
            state: "lost".into(),
            reason: Some(reason.into()),
        }
    }

    /// A size with no usable profile: the player can fix it by calibrating, so
    /// it is reported apart from a lost capture (window gone, capture failed).
    fn uncalibrated(reason: impl Into<String>) -> Self {
        Self {
            state: "uncalibrated".into(),
            reason: Some(reason.into()),
        }
    }
}

enum TickError {
    Lost(String),
    Parse(String),
    Transient(String),
    /// The client size has no usable profile yet. The overlay reports this and
    /// offers a Calibrate button; the window is not opened unasked.
    Uncalibrated(String),
}

struct EngineState {
    capturer: Option<(i64, Capturer)>,
    last_frame: Option<Frame>,
    outcomes: VecDeque<bool>,
    last_sample: Option<Sample>,
    published: Option<CaptureStatus>,
    published_profile: Option<Profile>,
    anchored: Option<(i32, i32)>,
    good_reads: usize,
}

impl EngineState {
    fn new() -> Self {
        Self {
            capturer: None,
            last_frame: None,
            outcomes: VecDeque::new(),
            last_sample: None,
            published: None,
            published_profile: None,
            anchored: None,
            good_reads: 0,
        }
    }

    fn keep_outcome(&mut self, ok: bool) {
        self.outcomes.push_back(ok);
        while self.outcomes.len() > OUTCOME_WINDOW {
            self.outcomes.pop_front();
        }
    }

    fn status(&self, reason: Option<String>) -> CaptureStatus {
        let failures = self.outcomes.iter().filter(|ok| !**ok).count();
        if failures >= DEGRADED_FAILURES {
            CaptureStatus::degraded(reason)
        } else {
            CaptureStatus::reading()
        }
    }

    fn reset_target(&mut self) {
        self.capturer = None;
        self.last_frame = None;
        self.outcomes.clear();
        self.last_sample = None;
        self.good_reads = 0;
    }
}

fn publish_capture<R: Runtime>(
    app: &AppHandle<R>,
    engine: &mut EngineState,
    status: CaptureStatus,
) {
    // Emit every tick: the overlay may mount or reload after the last change,
    // and a status event is cheap at 1 Hz.
    engine.published = Some(status.clone());
    let _ = app.emit("capture-status", status);
}

fn anchor_overlay<R: Runtime>(
    app: &AppHandle<R>,
    engine: &mut EngineState,
    screen: window_target::Rect,
) {
    // Overlay top-left sits at the game client's top-right corner.
    let anchor = (screen.x + screen.w, screen.y);
    if engine.anchored == Some(anchor) {
        return;
    }

    if let Some(window) = app.get_webview_window("main") {
        if window
            .set_position(tauri::PhysicalPosition::new(anchor.0, anchor.1))
            .is_ok()
        {
            engine.anchored = Some(anchor);
        }
    }
}

pub fn start<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || run(app));
}

fn run<R: Runtime>(app: AppHandle<R>) {
    let mut engine = EngineState::new();

    // Carry the dip guard across a restart. The guard compares each reading with
    // the last one recorded, so without this the first reading after a restart has
    // nothing to compare against — which is how a two-EXP blip got into the history.
    if let Some(record) = crate::last_sample_record() {
        engine.last_sample = Some(Sample {
            t: record.get("t").and_then(serde_json::Value::as_i64).unwrap_or(0),
            lv: record
                .get("lv")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32,
            exp: record
                .get("exp")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            pct: record
                .get("pct")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0),
            map: String::new(),
            state: "idle".into(),
            conf: 1.0,
        });
    }

    loop {
        std::thread::sleep(Duration::from_secs(1));

        let target_state = app.state::<window_target::TargetState>();
        let selector = target_state.selector();
        let target_status = window_target::status_for(&selector);
        window_target::publish_status(&app, &target_state, target_status.clone());

        if let Some(screen) = target_status.screen {
            anchor_overlay(&app, &mut engine, screen);
        }

        match tick(&mut engine, &target_status) {
            Ok(outcome) => {
                if engine.published_profile.as_ref() != Some(&outcome.profile) {
                    engine.published_profile = Some(outcome.profile.clone());
                    let _ = app.emit("profile", &outcome.profile);
                }

                if outcome.verified {
                    let record = serde_json::to_value(&outcome.sample).unwrap_or_default();
                    if crate::append_sample_record(&record).is_ok() {
                        let _ = app.emit("live-sample", &outcome.sample);
                        engine.last_sample = Some(outcome.sample);
                    }
                }
                engine.keep_outcome(true);
                let status = engine.status(None);
                publish_capture(&app, &mut engine, status);
            }
            Err(TickError::Lost(reason)) => {
                engine.reset_target();
                publish_capture(&app, &mut engine, CaptureStatus::lost(reason));
            }
            Err(TickError::Uncalibrated(reason)) => {
                // The window is fine — only the calibration is missing — so keep
                // the capturer attached. Resetting here would drop it, and next
                // tick would re-attach with no frame yet and flip to a transient
                // "reading" state, making the panel flicker once a second.
                // The overlay reports this and offers a Calibrate button; the
                // window stays closed until the player asks for it.
                publish_capture(&app, &mut engine, CaptureStatus::uncalibrated(reason));
            }
            Err(TickError::Parse(reason)) => {
                // A rejected read is the frame worth keeping: it is exactly the one
                // the reader could not handle, so labelling it fills the gap.
                if let Some(frame) = engine.last_frame.clone() {
                    crate::calibration::save_review(&frame, &reason);
                }
                engine.keep_outcome(false);
                let status = engine.status(Some(reason));
                publish_capture(&app, &mut engine, status);
            }
            Err(TickError::Transient(reason)) => {
                engine.keep_outcome(false);
                let status = engine.status(Some(reason));
                publish_capture(&app, &mut engine, status);
            }
        }
    }
}

fn tick(
    engine: &mut EngineState,
    status: &TargetStatus,
) -> Result<TickOutcome, TickError> {
    if !status.configured {
        return Err(TickError::Lost("no target window selected".into()));
    }
    if !status.found {
        return Err(TickError::Lost("game window not found".into()));
    }
    if status.minimized {
        return Err(TickError::Lost("game window is minimized".into()));
    }

    let hwnd = status
        .hwnd
        .ok_or_else(|| TickError::Lost("game window not found".into()))?;

    if engine.capturer.as_ref().map(|(handle, _)| *handle) != Some(hwnd) {
        let capturer = Capturer::attach(hwnd)
            .map_err(|error| TickError::Lost(format!("capture failed: {error}")))?;
        engine.capturer = Some((hwnd, capturer));
        engine.last_frame = None;
    }

    let frame = match engine
        .capturer
        .as_mut()
        .expect("capturer just attached")
        .1
        .try_frame()
    {
        Ok(Some(frame)) => {
            let cropped =
                capture::crop_to_client(frame, hwnd).map_err(TickError::Lost)?;
            engine.last_frame = Some(cropped.clone());
            cropped
        }
        Ok(None) => match &engine.last_frame {
            Some(frame) => frame.clone(),
            None => return Err(TickError::Transient("waiting for the first frame".into())),
        },
        Err(error) => return Err(TickError::Lost(format!("capture failed: {error}"))),
    };

    // The profile is chosen by the live client size, so a resolution change
    // switches calibration sets instead of invalidating everything.
    let profile = profile::activate(frame.width, frame.height)
        .map_err(|error| TickError::Lost(format!("profile failed: {error}")))?;

    if !profile.is_calibrated() {
        return Err(TickError::Uncalibrated(format!(
            "not calibrated for {}x{}",
            profile.client_w, profile.client_h
        )));
    }
    if !profile.has_glyphs() {
        return Err(TickError::Uncalibrated(format!(
            "no digits learned for {}x{}",
            profile.client_w, profile.client_h
        )));
    }

    if let (Some(width), Some(height)) = (
        profile.calibration.client_w,
        profile.calibration.client_h,
    ) {
        if width != frame.width || height != frame.height {
            return Err(TickError::Lost(format!(
                "window is {}x{} but calibrated at {width}x{height} — recalibrate",
                frame.width, frame.height
            )));
        }
    }

    // Switch the active profile, so calibration and teach commands target the
    // size that is actually on screen.
    if profile::active_key().as_deref() != Some(profile.key().as_str()) {
        profile::activate(frame.width, frame.height)
            .map_err(|error| TickError::Lost(format!("profile failed: {error}")))?;
    }

    let calibration = profile.calibration.clone();
    let atlas = Atlas::from_settings(&profile.atlas);
    let readings = read_frame(&frame, &calibration, &atlas, &profile.maps, &profile.identities)
        .map_err(TickError::Parse)?;

    // A character does not lose levels, so a derivation that lands below the last
    // accepted one is a misread however self-consistent it looks. This is what let
    // a truncated number — 21,476 read as 2,106 — record itself as level 19 and
    // put a dip in the history.
    if let Some(previous) = &engine.last_sample {
        if readings.lv < previous.lv {
            return Err(TickError::Parse(format!(
                "derived level fell from {} to {}",
                previous.lv, readings.lv
            )));
        }
    }

    let state = match &engine.last_sample {
        Some(previous) if previous.lv == readings.lv && readings.exp < previous.exp => {
            return Err(TickError::Parse(
                "EXP dropped without a level change".into(),
            ));
        }
        Some(previous) if previous.lv == readings.lv && readings.exp == previous.exp => "idle",
        _ => "gaining",
    };

    // Recording starts only after consecutive good reads, so a mismatched
    // profile can never write plausible-looking rows.
    engine.good_reads = if matches!(
        state,
        "gaining" | "idle"
    ) {
        engine.good_reads.saturating_add(1)
    } else {
        0
    };
    let verified = engine.good_reads >= profile::VERIFY_READS
        || profile::VERIFY_READS == 0;

    Ok(TickOutcome {
        sample: Sample {
            t: chrono::Utc::now().timestamp(),
            lv: readings.lv,
            exp: readings.exp,
            pct: readings.pct,
            map: readings.map,
            state: state.into(),
            conf: readings.conf,
        },
        profile: Profile {
            name: readings.name.clone(),
            job: readings.job.clone(),
        },
        verified,
    })
}

pub(crate) fn read_frame(
    frame: &Frame,
    calibration: &Calibration,
    atlas: &Atlas,
    maps: &[Template],
    identities: &Identities,
) -> Result<Readings, String> {
    let exp_region = calibration
        .exp
        .filter(Region::is_usable)
        .ok_or("the EXP box is not set")?;

    let exp_gray = ocr::crop_gray(frame, exp_region).ok_or("the EXP box is outside the window")?;
    let (exp_text, exp_conf) = atlas.recognize_readout(&exp_gray)?;
    let (exp, pct) = ocr::parse_exp_text(&exp_text)
        .ok_or_else(|| format!("could not parse EXP from '{exp_text}'"))?;

    // The level comes from its own box when that box is set and readable, and is
    // otherwise derived from EXP and the percentage. Deriving is what lets a
    // profile ship with only the EXP box: the level digits are a different glyph
    // size, so they can never be pre-learned for every level.
    let (lv, level_text, level_conf) =
        level_reading(frame, calibration, atlas, exp, pct, exp_conf)?;

    let required = exp_table::required(lv)
        .ok_or_else(|| format!("level {lv} is outside the EXP table"))?;

    // The percentage is displayed to two decimals, so a genuine read agrees with
    // itself to within about 0.005. The recorded dips agreed to 0.13 and 0.23,
    // which the old margin of 0.25 allowed through.
    let expected_pct = exp as f64 / required * 100.0;
    let tolerance = if lv <= 49 || lv >= 90 {
        0.02
    } else {
        CROSS_CHECK_TOLERANCE
    };
    if (pct - expected_pct).abs() > tolerance {
        return Err(format!(
            "EXP read inconsistent ({pct:.2}% vs {expected_pct:.2}%) — learn more digits"
        ));
    }

    let map = match_map(frame, calibration, maps);

    let name = match (calibration.name.filter(Region::is_usable), identities.name.as_ref()) {
        (Some(region), Some(template)) => match_template(frame, region, template),
        _ => None,
    };
    let job = match (calibration.job.filter(Region::is_usable), identities.job.as_ref()) {
        (Some(region), Some(template)) => match_template(frame, region, template),
        _ => None,
    };

    Ok(Readings {
        lv,
        exp,
        pct,
        expected_pct,
        map,
        conf: exp_conf.min(level_conf),
        exp_text,
        level_text,
        name,
        job,
    })
}

/// The level for this frame: from the level box when it is set and readable,
/// otherwise derived from EXP and the percentage.
fn level_reading(
    frame: &Frame,
    calibration: &Calibration,
    atlas: &Atlas,
    exp: i64,
    pct: f64,
    exp_conf: f64,
) -> Result<(u32, String, f64), String> {
    if let Some(region) = calibration.level.filter(Region::is_usable) {
        if let Some(gray) = ocr::crop_gray(frame, region) {
            if let Ok((text, conf)) = atlas.recognize(&gray) {
                if let Some(lv) = ocr::parse_level(&text) {
                    if exp_table::required(lv).is_some() {
                        return Ok((lv, text, conf));
                    }
                }
            }
        }
    }

    match exp_table::level_for_exp_pct(exp, pct) {
        Some(lv) => Ok((lv, format!("derived from {pct:.2}%"), exp_conf)),
        None => Err(
            "level not readable and not derivable at this percentage — set the level box".into(),
        ),
    }
}

fn match_template(frame: &Frame, region: Region, template: &Template) -> Option<String> {
    let crop = ocr::crop_gray(frame, region)?;
    let bytes = settings::from_hex(&template.pixels)?;
    if template.w as usize != crop.w
        || template.h as usize != crop.h
        || bytes.len() != crop.px.len()
    {
        return None;
    }

    let candidate = ocr::Gray {
        w: crop.w,
        h: crop.h,
        px: bytes,
    };
    let distance = ocr::map_distance(&crop, &candidate)?;
    if distance <= MAP_DISTANCE_LIMIT {
        Some(template.label.clone())
    } else {
        None
    }
}

fn template_gray(template: &Template) -> Option<ocr::Gray> {
    let bytes = settings::from_hex(&template.pixels)?;
    let w = template.w as usize;
    let h = template.h as usize;
    if w == 0 || h == 0 || bytes.len() != w * h {
        return None;
    }
    let gray = ocr::Gray {
        w,
        h,
        px: bytes,
    };
    if w == ocr::MAP_LABEL_W && h == ocr::MAP_LABEL_H {
        Some(gray)
    } else {
        Some(ocr::resample(&gray, ocr::MAP_LABEL_W, ocr::MAP_LABEL_H))
    }
}

fn match_map(frame: &Frame, calibration: &Calibration, templates: &[Template]) -> String {
    // A taught fingerprint is an exact picture of that map, so it wins. The letter
    // atlas comes from a very small label (4-6 px glyphs in the Ideal Ratio HUD) and
    // can be learned misaligned — every shape then matches at distance 0 with the
    // wrong character, which no consistency check can catch.
    if let Some(name) = match_map_fingerprint(frame, calibration, templates) {
        return name;
    }

    // Nothing taught matched: read the label as text, which is how a map the app has
    // never seen gets named. A near miss snaps to a name that was taught explicitly.
    let map_atlas = Atlas::from_settings(&settings::load_map_atlas());
    if let Some(lines) = ocr::read_map_text(frame, &map_atlas) {
        let text = lines.join("\n");
        if !text.trim().is_empty() {
            let best = templates
                .iter()
                .map(|template| {
                    (
                        template.label.clone(),
                        ocr::label_similarity(&text, &template.label),
                    )
                })
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));

            return match best {
                Some((label, similarity)) if similarity >= MAP_SIMILARITY => label,
                _ => text,
            };
        }
    }

    "unknown".into()
}

fn match_map_fingerprint(
    frame: &Frame,
    calibration: &Calibration,
    templates: &[Template],
) -> Option<String> {
    // The minimap moves and resizes, so its label is located on every frame.
    // The calibrated box is only a fallback for when detection fails.
    let crop = ocr::read_map_label(frame).or_else(|| {
        let region = calibration.map.filter(Region::is_usable)?;
        let gray = ocr::crop_gray(frame, region)?;
        Some(ocr::resample(&gray, ocr::MAP_LABEL_W, ocr::MAP_LABEL_H))
    })?;

    let mut best: Option<(&str, f64)> = None;
    for template in templates {
        let Some(candidate) = template_gray(template) else {
            continue;
        };
        if let Some(distance) = ocr::map_distance(&crop, &candidate) {
            if best.map_or(true, |(_, closest)| distance < closest) {
                best = Some((&template.label, distance));
            }
        }
    }

    match best {
        Some((name, distance)) if distance <= MAP_DISTANCE_LIMIT => Some(name.to_string()),
        _ => None,
    }
}

#[tauri::command]
pub fn get_profile() -> Profile {
    let identities = settings::load_identities();
    Profile {
        name: identities.name.map(|template| template.label),
        job: identities.job.map(|template| template.label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_bmp_frame(path: &std::path::Path) -> Frame {
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

    #[test]
    #[ignore = "diagnostic: learns data/debug-frame.bmp, text from MSCOUT_LEARN_TEXT"]
    fn learn_from_bmp() {
        let text = std::env::var("MSCOUT_LEARN_TEXT").expect("MSCOUT_LEARN_TEXT");
        let frame = load_bmp_frame(&crate::data_dir().unwrap().join("debug-frame.bmp"));
        let calibration = settings::load_calibration();
        let mut atlas = Atlas::from_settings(&settings::load_atlas());
        let before = atlas.learned();

        atlas
            .learn(
                &text,
                &ocr::crop_gray(&frame, calibration.exp.unwrap()).unwrap(),
            )
            .expect("learn");

        settings::save_atlas(&atlas.to_settings()).unwrap();
        println!(
            "before={before} after={} variants={}",
            atlas.learned(),
            atlas.glyphs.len()
        );
    }

    #[test]
    #[ignore = "diagnostic: teaches MSCOUT_MAP_NAME from the detected minimap label"]
    fn teach_map_from_bmp() {
        let name = std::env::var("MSCOUT_MAP_NAME").expect("MSCOUT_MAP_NAME");
        let frame = load_bmp_frame(&crate::data_dir().unwrap().join("debug-frame.bmp"));

        let gray = ocr::read_map_label(&frame).expect("detected minimap label");
        let template = settings::Template {
            label: name.clone(),
            w: gray.w as u32,
            h: gray.h as u32,
            pixels: settings::to_hex(&gray.px),
        };

        let mut maps = settings::load_maps();
        maps.retain(|existing| existing.label != name);
        maps.push(template);
        settings::save_maps(&maps).unwrap();

        let calibration = settings::load_calibration();
        let atlas = Atlas::from_settings(&settings::load_atlas());
        let identities = settings::load_identities();
        let readings =
            read_frame(&frame, &calibration, &atlas, &maps, &identities).expect("read");
        println!("map={:?} (taught {:?})", readings.map, name);
    }

    #[test]
    #[ignore = "diagnostic: prints detected minimap panel and label boxes"]
    fn detect_minimap() {
        let frame = load_bmp_frame(&crate::data_dir().unwrap().join("debug-frame.bmp"));
        let panel = ocr::detect_minimap_panel(&frame);
        let label = panel.and_then(|panel| ocr::detect_map_label(&frame, panel));
        println!("panel={panel:?}");
        println!("label={label:?}");
        println!("normalised={:?}", ocr::read_map_label(&frame).map(|g| (g.w, g.h)));
    }

    #[test]
    #[ignore = "build tool: learns MSCOUT_LEARN_TEXT from a frame into data/atlas-build.json"]
    fn build_atlas() {
        let text = std::env::var("MSCOUT_LEARN_TEXT").expect("MSCOUT_LEARN_TEXT");
        let dir = crate::data_dir().expect("data dir");
        let frame_path = std::env::var("MSCOUT_FRAME")
            .map(|name| dir.join(name))
            .unwrap_or_else(|_| dir.join("debug-frame.bmp"));
        let frame = load_bmp_frame(&frame_path);

        // Use the boxes of the profile this atlas is for (default 1920x1200). The
        // UI Size modes share box positions, so the default set supplies them even
        // when no set exists yet for the mode being built. MSCOUT_CROP overrides it
        // for a mode whose HUD sits somewhere else.
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
            Err(_) => {
                let key =
                    std::env::var("MSCOUT_PROFILE_KEY").unwrap_or_else(|_| "1920x1200".into());
                let (w, h) = key.split_once('x').expect("WxH");
                let profile = crate::profile::load_for_mode(
                    w.parse().expect("width"),
                    h.parse().expect("height"),
                    settings::UiMode::Default,
                )
                .expect("profile on disk");
                profile.calibration.exp.expect("EXP box")
            }
        };
        let gray = ocr::crop_gray(&frame, region).expect("crop");

        let path = dir.join("atlas-build.json");
        let mut atlas = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<settings::Atlas>(&raw).ok())
            .map(|stored| Atlas::from_settings(&stored))
            .unwrap_or(Atlas { glyphs: Vec::new() });

        if let Err(error) = atlas.learn(&text, &gray) {
            println!("LEARN FAILED: {error}");
            return;
        }
        std::fs::write(&path, serde_json::to_string_pretty(&atlas.to_settings()).unwrap())
            .expect("write atlas");

        // Self-check: reading the same box back must reproduce the typed text.
        println!("round-trip = {:?}", atlas.recognize(&gray));
        for guess in atlas.explain(&gray) {
            println!(
                "  seg x={} y={} {}x{} -> '{}' d={:.1} known={}",
                guess.x, guess.y, guess.w, guess.h, guess.ch, guess.distance, guess.known
            );
        }

        let mut chars: Vec<char> = atlas.glyphs.iter().map(|glyph| glyph.ch).collect();
        chars.sort_unstable();
        chars.dedup();
        let coverage: String = chars.iter().collect();
        let missing: String = "0123456789,.[]%"
            .chars()
            .filter(|ch| !chars.contains(ch))
            .collect();
        println!("coverage={coverage:?} missing={missing:?}");
    }

    #[test]
    #[ignore = "diagnostic: groups the status-bar text into clusters, to locate the EXP box"]
    fn probes_status_bar() {
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");

        let region = Region {
            x: 1023,
            y: 1164,
            w: 102,
            h: 17,
        };
        let gray = ocr::crop_gray(&frame, region).expect("crop");

        // Column ink profile: `segments` splits on columns with nothing in them, so
        // this shows whether the glyphs separate at all in this rendering.
        let ink = ocr::column_ink(&gray);
        println!("otsu ink: {ink:?}");
        println!("otsu blank columns: {}", ink.iter().filter(|count| **count == 0).count());
        let bright = ocr::column_ink_floor(&gray, 170);
        println!("floored ink: {bright:?}");
        println!(
            "floored blank columns: {}",
            bright.iter().filter(|count| **count == 0).count()
        );
        println!("segments: {} shapes", ocr::segments(&gray).len());
        for (row, line) in ocr::shape_art(&gray).iter().enumerate() {
            println!("{row:2} {line}");
        }
    }

    #[test]
    #[ignore = "diagnostic: writes one review frame from the live game"]
    fn dumps_a_review_frame() {
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");

        crate::calibration::save_review(&frame, "diagnostic");
        let dir = crate::data_dir().expect("data dir").join("review");
        let count = std::fs::read_dir(&dir).map(|entries| entries.count()).unwrap_or(0);
        println!("review frames now: {count}");
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            println!("  {}", entry.file_name().to_string_lossy());
        }
    }

    #[test]
    #[ignore = "diagnostic: learns the live minimap label into a label atlas and reads it back"]
    fn map_text_round_trip() {
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");
        let profile = crate::profile::load(1920, 1200).expect("profile");

        let lines = ocr::map_label_lines(&frame).expect("label lines");
        for (index, line) in lines.iter().enumerate() {
            println!(
                "  line {index}: {}x{} with {} shapes",
                line.w,
                line.h,
                ocr::segments(line).len()
            );
        }

        let typed = std::env::var("MSCOUT_MAP_TEXT")
            .unwrap_or_else(|_| "Hidden Street\nFree Market <15>".into());
        let mut atlas = Atlas::from_settings(&profile.map_atlas);
        for (line, wanted) in lines.iter().zip(typed.split('\n')) {
            match atlas.learn(wanted.trim(), line) {
                Ok(chars) => println!("  learned {:?}", chars.iter().collect::<String>()),
                Err(error) => println!("  learn failed: {error}"),
            }
        }

        println!("letters = {:?}", atlas.learned());
        println!("read back = {:?}", ocr::read_map_text(&frame, &atlas));

        if std::env::var_os("MSCOUT_MAP_SAVE").is_some() {
            let mut stored = crate::profile::load(1920, 1200).expect("profile on disk");
            stored.map_atlas = atlas.to_settings();
            let mut maps = stored.maps.clone();
            maps.retain(|template| template.label != typed);
            maps.push(settings::Template {
                label: typed.clone(),
                w: 0,
                h: 0,
                pixels: String::new(),
            });
            stored.maps = maps;
            crate::profile::save(&stored).expect("save profile");
            println!("saved label letters and {:?} into {}", typed, stored.key());
        }
    }

    #[test]
    #[ignore = "diagnostic: reads the live game with saved settings"]
    fn live_settings_read() {
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");

        let key = std::env::var("MSCOUT_PROFILE_KEY").unwrap_or_else(|_| "1920x1200".into());
        let (width, height) = key.split_once('x').expect("WxH");
        let ui_mode = std::env::var("MSCOUT_UI_MODE")
            .ok()
            .and_then(|value| settings::UiMode::parse(&value))
            .unwrap_or_default();
        let profile = crate::profile::load_for_mode(
            width.parse().expect("width"),
            height.parse().expect("height"),
            ui_mode,
        )
        .expect("profile on disk");
        println!("profile {} ({:?})", profile.key(), ui_mode);
        let calibration = profile.calibration;
        let atlas = Atlas::from_settings(&profile.atlas);
        let maps = profile.maps;
        let identities = profile.identities;

        println!("calibration: {calibration:?}");
        for (kind, region, template) in [
            ("name", calibration.name, identities.name.as_ref()),
            ("job", calibration.job, identities.job.as_ref()),
            ("map", calibration.map, maps.first()),
        ] {
            let (Some(region), Some(template)) = (region, template) else {
                continue;
            };
            let Some(crop) = ocr::crop_gray(&frame, region) else {
                continue;
            };
            let Some(bytes) = settings::from_hex(&template.pixels) else {
                continue;
            };
            let candidate = ocr::Gray {
                w: crop.w,
                h: crop.h,
                px: bytes,
            };
            println!(
                "{kind}: crop {}x{}, template {}x{}, distance {:?}",
                crop.w,
                crop.h,
                template.w,
                template.h,
                ocr::map_distance(&crop, &candidate)
            );
        }

        if let Some(region) = calibration.exp.filter(Region::is_usable) {
            if let Some(gray) = ocr::crop_gray(&frame, region) {
                println!("exp box {region:?} -> crop {}x{}", gray.w, gray.h);
                for guess in atlas.explain(&gray) {
                    println!(
                        "  seg x={} y={} {}x{} -> '{}' d={:.1} known={}",
                        guess.x, guess.y, guess.w, guess.h, guess.ch, guess.distance, guess.known
                    );
                }
                println!("  text = {:?}", atlas.recognize_readout(&gray));
                println!("  plain = {:?}", atlas.recognize(&gray));
            }
        }

        let map_atlas = Atlas::from_settings(&profile.map_atlas);
        match ocr::map_label_lines(&frame) {
            Some(lines) => {
                for (index, line) in lines.iter().enumerate() {
                    let text = map_atlas
                        .recognize(line)
                        .map(|(text, _)| text)
                        .unwrap_or_default();
                    println!(
                        "label line {index}: {text:?} ({}x{}, {} shapes)",
                        line.w,
                        line.h,
                        ocr::segments(line).len()
                    );
                    for guess in map_atlas.explain(line) {
                        println!(
                            "   {}x{} -> '{}' d={:.1} known={}",
                            guess.w, guess.h, guess.ch, guess.distance, guess.known
                        );
                    }
                }
                println!("map text = {:?}", ocr::read_map_text(&frame, &map_atlas));
            }
            None => println!("label lines not found"),
        }

        match ocr::read_map_label(&frame) {
            Some(label) => {
                println!("minimap label detected {}x{}", label.w, label.h);
                for template in &maps {
                    let Some(candidate) = template_gray(template) else {
                        println!("  map {:?} -> template will not decode", template.label);
                        continue;
                    };
                    println!(
                        "  map {:?} -> {:?}",
                        template.label,
                        ocr::map_distance(&label, &candidate)
                    );
                }
            }
            None => println!("minimap label not detected"),
        }

        match read_frame(&frame, &calibration, &atlas, &maps, &identities) {
            Ok(readings) => println!("{readings:?}"),
            Err(error) => println!("read failed: {error}"),
        }
    }
}
