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
const MAP_DISTANCE_LIMIT: f64 = 12.0;
/// How close a read label has to be to a taught one before the taught name wins.
/// Letters are read one glyph at a time, so a near miss is expected now and then.
const MAP_SIMILARITY: f64 = 0.75;
/// How often a profile with no glyphs retries seeding from the installed client.
/// Parsing the 35 MB canvas archive is not free, and a readout that is not on
/// screen yet would otherwise make the engine rebuild it every second.
const SEED_RETRY: Duration = Duration::from_secs(30);
/// Font-seeding retry cadence. Rendering Arial for every candidate size is
/// heavier than the digit seed, so a failed attempt backs off the same way.
const TEXT_SEED_RETRY: Duration = Duration::from_secs(30);

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
    /// True when the minimap is collapsed to its title bar, so no map name is on
    /// screen. Absent in older rows, so it defaults to false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub map_hidden: bool,
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
    /// True when the minimap is collapsed to its title bar. There is no map name
    /// to read and nothing to calibrate; the overlay shows a distinct state.
    pub map_hidden: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: Option<String>,
    pub job: Option<String>,
}

/// Last profile published by the reader, so a frontend reload can recover it
/// without waiting for the next change event.
static LAST_PROFILE: std::sync::Mutex<Option<Profile>> = std::sync::Mutex::new(None);

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
    last_seed_attempt: Option<std::time::Instant>,
    identity_seed_attempt: Option<std::time::Instant>,
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
            last_seed_attempt: None,
            identity_seed_attempt: None,
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
        self.last_seed_attempt = None;
        self.identity_seed_attempt = None;
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
            map_hidden: false,
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
                    *LAST_PROFILE
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) =
                        Some(outcome.profile.clone());
                    let _ = app.emit("profile", &outcome.profile);
                }

                if outcome.verified {
                    // An unrecognised level still updates the overlay, but it is kept
                    // out of the history: a row with level 0 has no place on the EXP
                    // curve and would bend it towards the origin.
                    if outcome.sample.lv > 0 {
                        let record = serde_json::to_value(&outcome.sample).unwrap_or_default();
                        let _ = crate::append_sample_record(&record);
                    }
                    let _ = app.emit("live-sample", &outcome.sample);
                    engine.last_sample = Some(outcome.sample);
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

/// Seed the active profile's digit atlases from the installed client, using this
/// frame to measure the rendered glyph height. Returns whether anything was
/// written. Best-effort by design: an unset folder, a missing archive or a
/// readout that is not on screen all leave the profile untouched.
fn seed_from_game(frame: &Frame, profile: &profile::Profile) -> bool {
    let Some(dir) = settings::mscw_dir() else {
        return false;
    };
    let dir = std::path::Path::new(&dir);

    let Some(exp) = profile
        .calibration
        .exp
        .filter(Region::is_usable)
        .and_then(|region| ocr::crop_gray(frame, region))
    else {
        return false;
    };
    let (stored, height) = match crate::game_font::seed_exp_atlas(&exp, dir) {
        Ok(seeded) => seeded,
        Err(error) => {
            // Reachable every SEED_RETRY while the readout is hidden or the
            // archive is unreadable; the log says why instead of failing silently.
            crate::calibration::log(&format!("seed from game failed: {error}"));
            return false;
        }
    };
    if let Err(error) = settings::save_atlas(&stored) {
        crate::calibration::log(&format!("seed from game could not save: {error}"));
        return false;
    }

    // The level box is optional and may not be on screen; seed it when it reads.
    if let Some(level) = profile
        .calibration
        .level
        .filter(Region::is_usable)
        .and_then(|region| ocr::crop_gray(frame, region))
    {
        if let Ok((level_stored, _)) = crate::game_font::seed_level_atlas(&level, dir) {
            let _ = settings::save_level_atlas(&level_stored);
        }
    }

    crate::calibration::log(&format!(
        "auto-seeded {}x{} digits at h={height} from {}",
        profile.client_w, profile.client_h, dir.display()
    ));
    true
}

/// Seed the name/job letter atlas from both identity boxes. Each crop exposes a
/// different subset of Arial glyphs, and the Default rendering is anti-aliased,
/// so merging the per-crop learning gives one atlas that reads both lines.
fn seed_identity_from_frame(
    frame: &Frame,
    calibration: &Calibration,
) -> Option<(settings::Atlas, u32)> {
    let mut merged: Option<crate::ocr::Atlas> = None;
    let mut px = 0;
    for region in [calibration.job, calibration.name] {
        let Some(region) = region.filter(Region::is_usable) else {
            continue;
        };
        let Some(gray) = ocr::crop_gray(frame, region) else {
            continue;
        };
        if let Some((stored, seeded_px)) = crate::font_seed::seed_identity(&gray) {
            let atlas = crate::ocr::Atlas::from_settings(&stored);
            match &mut merged {
                Some(combined) => combined.glyphs.extend(atlas.glyphs),
                None => {
                    px = seeded_px;
                    merged = Some(atlas);
                }
            }
        }
    }
    merged.map(|atlas| (atlas.to_settings(), px))
}

/// Identify the 1920x1080 UI layout from the orange level-tile cluster. The
/// Default and Filter Applied modes share the Default geometry; Ideal moves the
/// status bar readouts to a different position. 1366x768 has one layout for all
/// three modes, and every other size keeps the manual setting.
pub(crate) fn detect_ui_mode(frame: &Frame) -> Option<settings::UiMode> {
    match (frame.width, frame.height) {
        (1366, 768) => Some(settings::ui_mode()),
        (1920, 1080) => {
            let default_tiles = orange_count(
                frame,
                Region {
                    x: 461,
                    y: 1044,
                    w: 36,
                    h: 23,
                },
            );
            let ideal_tiles = orange_count(
                frame,
                Region {
                    x: 598,
                    y: 1050,
                    w: 41,
                    h: 26,
                },
            );
            if default_tiles >= 20 && default_tiles > ideal_tiles {
                Some(settings::UiMode::Default)
            } else if ideal_tiles >= 20 && ideal_tiles > default_tiles {
                Some(settings::UiMode::Ideal)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn orange_count(frame: &Frame, region: Region) -> usize {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let x0 = region.x.max(0) as usize;
    let y0 = region.y.max(0) as usize;
    let x1 = (region.x + region.w).max(0).min(w as i32) as usize;
    let y1 = (region.y + region.h).max(0).min(h as i32) as usize;
    let mut count = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let index = (y * w + x) * 4;
            let r = frame.rgba[index] as i32;
            let g = frame.rgba[index + 1] as i32;
            let b = frame.rgba[index + 2] as i32;
            if r >= 190 && (130..=235).contains(&g) && b <= 130 {
                count += 1;
            }
        }
    }
    count
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
    // switches calibration sets instead of invalidating everything. The UI
    // layout is detected from the frame; the manual setting is the fallback.
    let detected = detect_ui_mode(&frame).unwrap_or_else(settings::ui_mode);
    let mut profile = profile::activate_mode(frame.width, frame.height, detected)
        .map_err(|error| TickError::Lost(format!("profile failed: {error}")))?;

    if !profile.is_calibrated() {
        return Err(TickError::Uncalibrated(format!(
            "not calibrated for {}x{}",
            profile.client_w, profile.client_h
        )));
    }

    // Name/job are Arial: seed a text atlas from the live crop once, then read
    // them without asking the player to teach the letters. Fingerprints remain
    // the exact fallback.
    let has_identity_box = profile
        .calibration
        .name
        .or(profile.calibration.job)
        .filter(Region::is_usable)
        .is_some();
    if has_identity_box && profile.identity_atlas.glyphs.is_empty() {
        let due = engine
            .identity_seed_attempt
            .map_or(true, |attempt| attempt.elapsed() >= TEXT_SEED_RETRY);
        if due {
            engine.identity_seed_attempt = Some(std::time::Instant::now());
            if let Some((stored, px)) = seed_identity_from_frame(&frame, &profile.calibration) {
                profile.identity_atlas = stored.clone();
                let _ = settings::save_identity_atlas(&stored);
                crate::calibration::log(&format!(
                    "auto-seeded name/job text at {px}px from Arial"
                ));
            }
        }
    }

    if !profile.has_glyphs() {
        // The digits are a bitmap atlas the installed client already ships. Seed
        // from it before reporting failure, so a known size reads with no
        // calibration step. The next tick picks the seeded profile up; one second
        // later is soon enough for a 1 Hz reader. Retried on a slow cadence so a
        // readout that is not on screen cannot trigger a WZ parse every tick.
        let due = engine
            .last_seed_attempt
            .map_or(true, |attempt| attempt.elapsed() >= SEED_RETRY);
        if due {
            engine.last_seed_attempt = Some(std::time::Instant::now());
            if seed_from_game(&frame, &profile) {
                return Err(TickError::Transient("seeded the digits from the game".into()));
            }
        }
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
        profile::activate_mode(frame.width, frame.height, detected)
            .map_err(|error| TickError::Lost(format!("profile failed: {error}")))?;
    }

    let calibration = profile.calibration.clone();
    let atlas = Atlas::from_settings(&profile.atlas);
    let level_atlas = Atlas::from_settings(&profile.level_atlas);
    let identity_atlas = Atlas::from_settings(&profile.identity_atlas);
    let readings = read_frame(
        &frame,
        &calibration,
        &atlas,
        &level_atlas,
        &identity_atlas,
        &profile.maps,
        &profile.streets,
        &profile.identities,
    )
    .map_err(TickError::Parse)?;

    // A character does not lose levels, so a level below the last accepted one is a
    // misread however self-consistent it looks. This is what let a truncated number
    // — 21,476 read as 2,106 — record itself as level 19 and put a dip in the
    // history. An unrecognised level is 0 and takes no part in the comparison.
    if let Some(previous) = &engine.last_sample {
        if readings.lv > 0 && previous.lv > 0 && readings.lv < previous.lv {
            return Err(TickError::Parse(format!(
                "level fell from {} to {}",
                previous.lv, readings.lv
            )));
        }
    }

    let state = match &engine.last_sample {
        Some(previous)
            if readings.lv > 0
                && readings.lv == previous.lv
                && readings.exp < previous.exp =>
        {
            return Err(TickError::Parse(
                "EXP dropped without a level change".into(),
            ));
        }
        Some(previous)
            if readings.lv > 0 && readings.lv == previous.lv && readings.exp == previous.exp =>
        {
            "idle"
        }
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
            map_hidden: readings.map_hidden,
        },
        profile: Profile {
            name: readings.name.clone(),
            job: readings.job.clone(),
        },
        verified,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn read_frame(
    frame: &Frame,
    calibration: &Calibration,
    atlas: &Atlas,
    level_atlas: &Atlas,
    identity_atlas: &Atlas,
    maps: &[Template],
    streets: &[Template],
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

    // The level is read from its own box and never derived. A derived level is only
    // as right as the EXP table and, when it is wrong, puts a plausible-looking but
    // false number on the overlay — which is what "dont derive the level digits"
    // was about. An unreadable level comes back as 0 and is reported as unrecognised.
    // The level has its own atlas: the digits are drawn at a different size from the
    // EXP readout, and sharing one atlas let the two readers compete for characters.
    let (lv, level_text, level_conf) = level_reading(frame, calibration, level_atlas)?;

    // The percentage check exists to catch a bad EXP read, but EXP and its
    // percentage come from one read of one box, so they cannot disagree with each
    // other — only with the level, which is a different box and a different atlas.
    // A mismatch is therefore evidence against the level. Rejecting the whole frame
    // for it threw away a perfectly good EXP, map, name and job and left the overlay
    // on placeholders, which is why every successful teach looked like nothing had
    // happened. Blame the weaker reader and keep the rest.
    let required = exp_table::required(lv);
    if lv > 0 && required.is_none() {
        return Err(format!("level {lv} is outside the EXP table"));
    }
    let tolerance = if lv <= 49 || lv >= 90 {
        0.02
    } else {
        CROSS_CHECK_TOLERANCE
    };
    let level_agrees = required
        .map(|required| (pct - exp as f64 / required * 100.0).abs() <= tolerance)
        .unwrap_or(true);
    let (lv, level_text) = if level_agrees {
        (lv, level_text)
    } else {
        log_once(&format!(
            "read_frame: level {lv} does not fit {pct:.2}% for {} EXP — reporting the level as unrecognised",
            exp
        ));
        (0, String::new())
    };
    let expected_pct = match exp_table::required(lv) {
        Some(required) => exp as f64 / required * 100.0,
        None => pct,
    };

    // The minimap has three zoom states. Detection runs once and both the map and
    // street readers consume it, instead of each re-running panel detection.
    let minimap = ocr::detect_minimap(frame);
    let (map_name, street, map_hidden) =
        match_map_and_street(frame, calibration, maps, streets, minimap);
    let map = match street {
        Some(street) if map_name != "unknown" => format!("{street}\n{map_name}"),
        _ => map_name,
    };

    // Job and name can draw at different Arial sizes (the Filter Applied
    // rendering seeds the job at 14 px and the name at 19 px). Seed each line
    // from its own crop and read it with that atlas; only fall back to the
    // profile's shared atlas when a line cannot be seeded.
    let name_atlas = seed_line_identity(frame, calibration.name);
    let job_atlas = seed_line_identity(frame, calibration.job);
    let name = read_identity(
        frame,
        calibration.name,
        name_atlas.as_ref().unwrap_or(identity_atlas),
        identities.name.as_ref(),
    );
    let job = read_identity(
        frame,
        calibration.job,
        job_atlas.as_ref().unwrap_or(identity_atlas),
        identities.job.as_ref(),
    );

    Ok(Readings {
        lv,
        exp,
        pct,
        expected_pct,
        map,
        conf: if lv == 0 { exp_conf } else { exp_conf.min(level_conf) },
        exp_text,
        level_text,
        name,
        job,
        map_hidden,
    })
}

/// The level for this frame, read from its own box. Deliberately never derived from
/// EXP and the percentage: the derivation leans on the EXP table, and when it is
/// wrong it reports a level that looks reasonable and is not.
///
/// An unreadable level yields 0, which every caller treats as "not recognised" —
/// the same way an unrecognised name is reported. Failing the whole frame instead
/// would hide a perfectly good EXP reading behind a missing box.
/// Write a diagnostic to data/debug.log, but only when it differs from the last
/// one. The level is read every tick, so logging unconditionally would bury the
/// log under thousands of identical lines.
fn log_once(message: &str) {
    use std::collections::HashMap;

    // One last message per category (the text before the first colon), so the
    // map and street matches can log independently instead of alternating and
    // defeating a single shared slot.
    static LAST: std::sync::Mutex<Option<HashMap<String, String>>> =
        std::sync::Mutex::new(None);
    let key = message.split(':').next().unwrap_or(message).to_string();
    let mut guard = LAST.lock().unwrap_or_else(|error| error.into_inner());
    let last = guard.get_or_insert_with(HashMap::new);
    if last.get(&key).map(String::as_str) == Some(message) {
        return;
    }
    last.insert(key, message.to_string());
    crate::calibration::log(message);
}

fn level_reading(
    frame: &Frame,
    calibration: &Calibration,
    atlas: &Atlas,
) -> Result<(u32, String, f64), String> {
    let unknown = Ok((0u32, String::new(), 0.0));

    let Some(region) = calibration.level.filter(Region::is_usable) else {
        log_once("level_reading: no level box set");
        return unknown;
    };
    let Some(gray) = ocr::crop_gray(frame, region) else {
        log_once(&format!("level_reading: box {region:?} is outside the window"));
        return unknown;
    };
    if atlas.glyphs.is_empty() {
        log_once("level_reading: no level glyphs learned yet");
        return unknown;
    }
    match atlas.recognize_level(&gray) {
        Ok((text, conf)) => match ocr::parse_level(&text) {
            Some(lv) if exp_table::required(lv).is_some() => Ok((lv, text, conf)),
            Some(lv) => {
                log_once(&format!("level_reading: '{text}' -> {lv} is outside the table"));
                unknown
            }
            None => {
                log_once(&format!(
                    "level_reading: box {region:?} crop {}x{} read '{text}' — not a level",
                    gray.w, gray.h
                ));
                unknown
            }
        },
        Err(error) => {
            log_once(&format!(
                "level_reading: box {region:?} crop {}x{} failed: {error}",
                gray.w, gray.h
            ));
            unknown
        }
    }
}

/// Read one identity line as text, falling back to its taught fingerprint.
///
/// The text path is what removes the one-time teach. It is only trusted when the
/// line is non-empty and reasonably confident; a poor read is left to the exact
/// fingerprint rather than displayed as a wrong name.
fn seed_line_identity(frame: &Frame, region: Option<Region>) -> Option<Atlas> {
    let region = region.filter(Region::is_usable)?;
    let gray = ocr::crop_gray(frame, region)?;
    let (stored, _) = crate::font_seed::seed_identity(&gray)?;
    Some(Atlas::from_settings(&stored))
}

fn read_identity(
    frame: &Frame,
    region: Option<Region>,
    atlas: &Atlas,
    template: Option<&Template>,
) -> Option<String> {
    let region = region.filter(Region::is_usable)?;

    if !atlas.glyphs.is_empty() {
        if let Some(gray) = ocr::crop_gray(frame, region) {
            let gap = (gray.h as i32 / 3).max(2);
            if let Ok((text, confidence)) = atlas.recognize_text(&gray, gap) {
                let text = crate::font_seed::fix_identity_case(text.trim());
                if !text.is_empty() && confidence >= 0.55 {
                    return Some(text);
                }
            }
        }
    }

    template.and_then(|template| match_template(frame, region, template))
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

/// Label font sizes tried by the whole-line matcher. Ideal is 12 px; Default
/// Ratio draws the same label larger, so the matcher picks the size whose
/// rendered width and shape sequence explain the live line best.
const LABEL_SIZES: [u32; 7] = [12, 14, 16, 18, 20, 22, 24];

fn match_street(
    frame: &Frame,
    region: Option<Region>,
    streets: &[Template],
) -> Option<String> {
    if let Some(region) = region.filter(Region::is_usable) {
        if let Some(street) = match_street_fingerprint(frame, region, streets) {
            return Some(street);
        }
    }

    // Nothing taught matched (or no box was drawn): the client's own map table
    // has the street line, and detection finds it without a stored box.
    let dir = settings::mscw_dir()?;
    let lines = ocr::map_label_lines(frame)?;
    if lines.len() < 2 {
        return None;
    }
    let line = lines.first()?;
    let mut best: Option<(String, f64)> = None;
    for px in LABEL_SIZES {
        let Some(index) =
            crate::font_seed::label_index(std::path::Path::new(&dir), px)
        else {
            continue;
        };
        let max_score = if px >= 14 { 0.95 } else { 0.55 };
        if let Some((street, score)) =
            crate::font_seed::match_label(line, &index.streets, 14, max_score)
        {
            if best
                .as_ref()
                .map_or(true, |(_, current)| score < *current)
            {
                best = Some((street, score));
            }
        }
    }
    let (street, score) = best?;
    log_once(&format!(
        "street from the client's map table: {street:?} (score {score:.2})"
    ));
    Some(street)
}

fn match_street_fingerprint(
    frame: &Frame,
    region: Region,
    streets: &[Template],
) -> Option<String> {
    let crop = ocr::crop_gray(frame, region)?;
    let mut best: Option<(&str, f64)> = None;
    for template in streets {
        let Some(bytes) = settings::from_hex(&template.pixels) else {
            continue;
        };
        if template.w as usize != crop.w
            || template.h as usize != crop.h
            || bytes.len() != crop.px.len()
        {
            continue;
        }
        let candidate = ocr::Gray {
            w: crop.w,
            h: crop.h,
            px: bytes,
        };
        if let Some(distance) = ocr::map_distance(&crop, &candidate) {
            if best.map_or(true, |(_, closest)| distance < closest) {
                best = Some((&template.label, distance));
            }
        }
    }
    match best {
        Some((street, distance)) if distance <= MAP_DISTANCE_LIMIT => Some(street.to_string()),
        _ => None,
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

    // Nothing taught matched: the client's own map table has the exact name. The
    // label is Arial Bold 12, so a width filter plus a shape-sequence DTW against
    // the rendered names identifies it without segmenting the line.
    if let Some(name) = match_map_name(frame) {
        return name;
    }

    // Last resort: read the label as text, which is how a map not in the table gets
    // named. A near miss snaps to a name that was taught explicitly.
    let map_atlas = Atlas::from_settings(&settings::load_map_atlas());
    if let Some(lines) = ocr::read_map_text(frame, &map_atlas) {
        // The map name is the bottom line; the street is matched separately.
        let text = lines.last().cloned().unwrap_or_default();
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

/// Resolve the map and street for the minimap state on this frame.
///
/// Expanded reads the two-line label; folded reads the single `Street :Map` bar;
/// title-bar-only has no text at all and returns nothing. A frame with no
/// detectable minimap keeps the old fingerprint/name-table path.
fn match_map_and_street(
    frame: &Frame,
    calibration: &Calibration,
    maps: &[Template],
    streets: &[Template],
    minimap: Option<ocr::Minimap>,
) -> (String, Option<String>, bool) {
    match minimap.as_ref().map(|minimap| minimap.state) {
        Some(ocr::MinimapState::TitleBarOnly) => ("unknown".into(), None, true),
        Some(ocr::MinimapState::Folded) => {
            let Some(minimap) = minimap.as_ref() else {
                return ("unknown".into(), None, false);
            };
            let Some(label) = ocr::folded_label(frame, minimap) else {
                log_once("folded minimap: no label crop");
                return ("unknown".into(), None, false);
            };
            let Some(dir) = settings::mscw_dir() else {
                return ("unknown".into(), None, false);
            };
            let mut best: Option<((String, String), f64)> = None;
            for px in LABEL_SIZES {
                let Some(index) =
                    crate::font_seed::label_index(std::path::Path::new(&dir), px)
                else {
                    continue;
                };
                let max_score = if px >= 14 { 0.95 } else { 0.55 };
                if let Some((pair, score)) =
                    crate::font_seed::match_folded_label(&label, &index.folded_lines, 14, max_score)
                {
                    if best
                        .as_ref()
                        .map_or(true, |(_, current)| score < *current)
                    {
                        best = Some((pair, score));
                    }
                }
            }
            match best {
                Some(((street, name), score)) => {
                    log_once(&format!(
                        "folded minimap: {street:?} / {name:?} (score {score:.3})"
                    ));
                    (name, Some(street), false)
                }
                None => {
                    log_once("folded minimap: label did not match the client table");
                    ("unknown".into(), None, false)
                }
            }
        }
        _ => {
            let map_name = match_map(frame, calibration, maps);
            let street = match_street(frame, calibration.street, streets);
            (map_name, street, false)
        }
    }
}

fn match_map_name(frame: &Frame) -> Option<String> {
    let dir = settings::mscw_dir()?;
    let line = ocr::map_label_lines(frame)?.last()?.clone();
    let mut best: Option<(String, f64)> = None;
    for px in LABEL_SIZES {
        let Some(index) = crate::font_seed::label_index(std::path::Path::new(&dir), px) else {
            continue;
        };
        let max_score = if px >= 14 { 0.95 } else { 0.55 };
        if let Some((name, score)) =
            crate::font_seed::match_label(&line, &index.map_names, 14, max_score)
        {
            if best
                .as_ref()
                .map_or(true, |(_, current)| score < *current)
            {
                best = Some((name, score));
            }
        }
    }
    let (name, score) = best?;
    log_once(&format!(
        "map name from the client's map table: {name:?} (score {score:.2})"
    ));
    Some(name)
}

fn match_map_fingerprint(
    frame: &Frame,
    calibration: &Calibration,
    templates: &[Template],
) -> Option<String> {
    // Detection finds the whole label and follows a longer map name; `map_label`
    // only falls back to the stored box when detection fails. A stored box drawn
    // around a shorter name must not clip the next map's name, or two maps that
    // share a prefix would look identical.
    let crop = ocr::map_label(frame, calibration.map_box())?;

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
    // The frontend calls this on every mount. Return the last reading, not just
    // the taught identities: a webview reload would otherwise come back empty
    // and the engine would not re-emit a profile that has not changed.
    if let Some(profile) = LAST_PROFILE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
    {
        return profile;
    }

    let identities = settings::load_identities();
    Profile {
        name: identities.name.map(|template| template.label),
        job: identities.job.map(|template| template.label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "diagnostic: reads the live game with saved settings"]
    fn live_settings_read() {
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");

        let key = std::env::var("MSCOUT_PROFILE_KEY").unwrap_or_else(|_| "1920x1080".into());
        let (width, height) = key.split_once('x').expect("WxH");
        let ui_mode = std::env::var("MSCOUT_UI_MODE")
            .ok()
            .and_then(|value| settings::UiMode::parse(&value))
            .unwrap_or_default();
        // Activate it so `read_frame` sees the same profile the engine would.
        let profile = crate::profile::activate(
            width.parse().expect("width"),
            height.parse().expect("height"),
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
            ("map", calibration.map_box(), maps.first()),
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

        let streets = settings::load_streets();
        match read_frame(
            &frame,
            &calibration,
            &atlas,
            &Atlas::from_settings(&profile.level_atlas),
            &Atlas::from_settings(&profile.identity_atlas),
            &maps,
            &streets,
            &identities,
        ) {
            Ok(readings) => println!("{readings:?}"),
            Err(error) => println!("read failed: {error}"),
        }
    }
}

