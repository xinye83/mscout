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
}

enum TickError {
    Lost(String),
    Parse(String),
    Transient(String),
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
            Err(TickError::Parse(reason)) | Err(TickError::Transient(reason)) => {
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
        return Err(TickError::Lost(format!(
            "not calibrated for {}x{} — run calibration",
            profile.client_w, profile.client_h
        )));
    }
    if !profile.has_glyphs() {
        return Err(TickError::Lost(format!(
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
    let level_region = calibration
        .level
        .filter(Region::is_usable)
        .ok_or("the level box is not set")?;

    let exp_gray = ocr::crop_gray(frame, exp_region).ok_or("the EXP box is outside the window")?;
    let level_gray =
        ocr::crop_gray(frame, level_region).ok_or("the level box is outside the window")?;

    let (exp_text, exp_conf) = atlas.recognize(&exp_gray)?;
    let (level_text, level_conf) = atlas.recognize(&level_gray)?;

    let (exp, pct) = ocr::parse_exp_text(&exp_text)
        .ok_or_else(|| format!("could not parse EXP from '{exp_text}'"))?;
    let lv = ocr::parse_level(&level_text)
        .ok_or_else(|| format!("could not parse level from '{level_text}'"))?;
    let required = exp_table::required(lv)
        .ok_or_else(|| format!("level {lv} is outside the EXP table"))?;

    let expected_pct = exp as f64 / required * 100.0;
    let tolerance = if lv <= 49 || lv >= 90 {
        0.25
    } else {
        CROSS_CHECK_TOLERANCE
    };
    if (pct - expected_pct).abs() > tolerance {
        return Err(format!(
            "EXP cross-check failed: read {pct:.2}% but {exp} EXP is {expected_pct:.2}%"
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
    // The minimap moves and resizes, so its label is located on every frame.
    // The calibrated box is only a fallback for when detection fails.
    let crop = ocr::read_map_label(frame).or_else(|| {
        let region = calibration.map.filter(Region::is_usable)?;
        let gray = ocr::crop_gray(frame, region)?;
        Some(ocr::resample(&gray, ocr::MAP_LABEL_W, ocr::MAP_LABEL_H))
    });

    let Some(crop) = crop else {
        return "unknown".into();
    };

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
        Some((name, distance)) if distance <= MAP_DISTANCE_LIMIT => name.to_string(),
        _ => "unknown".into(),
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
    #[ignore = "diagnostic: reads the live game with saved settings"]
    fn live_settings_read() {
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");

        let calibration = settings::load_calibration();
        let atlas = Atlas::from_settings(&settings::load_atlas());
        let maps = settings::load_maps();
        let identities = settings::load_identities();

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

        match read_frame(&frame, &calibration, &atlas, &maps, &identities) {
            Ok(readings) => println!("{readings:?}"),
            Err(error) => println!("read failed: {error}"),
        }
    }
}
