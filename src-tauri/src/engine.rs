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

/// Seed the name/job letter atlas from whichever of the two boxes is on screen.
/// The job box usually has capitals and a descender, which is the best sample for
/// choosing the Arial pixel height.
fn seed_identity_from_frame(
    frame: &Frame,
    calibration: &Calibration,
) -> Option<(settings::Atlas, u32)> {
    for region in [calibration.job, calibration.name] {
        let Some(region) = region.filter(Region::is_usable) else {
            continue;
        };
        let Some(gray) = ocr::crop_gray(frame, region) else {
            continue;
        };
        if let Some(seeded) = crate::font_seed::seed_identity(&gray) {
            return Some(seeded);
        }
    }
    None
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
    let mut profile = profile::activate(frame.width, frame.height)
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
        profile::activate(frame.width, frame.height)
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

    // The caller passes the atlas from the profile it just loaded/seeded, so the
    // read never depends on a separate active-profile cache being in sync.
    let name = read_identity(
        frame,
        calibration.name,
        identity_atlas,
        identities.name.as_ref(),
    );
    let job = read_identity(
        frame,
        calibration.job,
        identity_atlas,
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
    let index = crate::font_seed::label_index(std::path::Path::new(&dir))?;
    let lines = ocr::map_label_lines(frame)?;
    if lines.len() < 2 {
        return None;
    }
    let (street, score) =
        crate::font_seed::match_label(lines.first()?, &index.streets, 8, 0.55)?;
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
            let Some(index) = crate::font_seed::label_index(std::path::Path::new(&dir)) else {
                return ("unknown".into(), None, false);
            };
            match crate::font_seed::match_folded_label(&label, &index.folded_lines, 8, 0.55) {
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
    let index = crate::font_seed::label_index(std::path::Path::new(&dir))?;
    let line = ocr::map_label_lines(frame)?.last()?.clone();
    let (name, score) =
        crate::font_seed::match_label(&line, &index.map_names, 8, 0.55)?;
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

        let gray = ocr::map_label(&frame, settings::load_calibration().map_box())
            .expect("detected minimap label");
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
        let streets = settings::load_streets();
        let readings = read_frame(
            &frame,
            &calibration,
            &atlas,
            &Atlas::from_settings(&settings::load_level_atlas()),
            &Atlas::from_settings(&settings::load_identity_atlas()),
            &maps,
            &streets,
            &identities,
        )
        .expect("read");
        println!("map={:?} (taught {:?})", readings.map, name);
    }

    #[test]
    #[ignore = "diagnostic: a clipping stored map box must not override detection"]
    fn map_label_ignores_a_clipping_box() {
        let frame = load_bmp_frame(&crate::data_dir().unwrap().join("debug-frame.bmp"));
        let detected = ocr::read_map_label(&frame).expect("detected label");
        // The box the 1920x1080 Ideal profile shipped with, measured around a
        // shorter name (70 px). It must not clip a 142 px name.
        let boxed = ocr::map_label(
            &frame,
            Some(Region { x: 72, y: 85, w: 70, h: 11 }),
        )
        .expect("boxed label");
        println!(
            "detected {}x{}, boxed {}x{}, distance {:?}",
            detected.w,
            detected.h,
            boxed.w,
            boxed.h,
            ocr::map_distance(&boxed, &detected)
        );
        assert_eq!(
            ocr::map_distance(&boxed, &detected),
            Some(0.0),
            "a stored box must not clip the detected label"
        );
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
            Err(_) => Region {
                x: 1023,
                y: 1164,
                w: 102,
                h: 17,
            },
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
    #[ignore = "tool: records the live map label into the profile on disk"]
    fn record_live_map() {
        use crate::settings::Template;

        let name = std::env::var("MSCOUT_MAP_NAME").expect("MSCOUT_MAP_NAME");
        let key = std::env::var("MSCOUT_PROFILE_KEY").unwrap_or_else(|_| "1366x768".into());
        let (width, height) = key.split_once('x').expect("WxH");

        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");

        let gray = ocr::map_label(&frame, settings::load_calibration().map_box())
            .expect("minimap label");
        let mut profile = crate::profile::load(
            width.parse().expect("width"),
            height.parse().expect("height"),
        )
        .expect("profile on disk");

        profile.maps.retain(|template| template.label != name);
        profile.maps.push(Template {
            label: name.clone(),
            w: gray.w as u32,
            h: gray.h as u32,
            pixels: settings::to_hex(&gray.px),
        });
        crate::profile::save(&profile).expect("save profile");

        println!(
            "recorded {:?} into {} — {} maps now",
            name,
            profile.key(),
            profile.maps.len()
        );
    }

    #[test]
    #[ignore = "tool: re-records the live map and identity fingerprints for the mode in use"]
    fn record_live_labels() {
        use crate::settings::Template;

        let key = std::env::var("MSCOUT_PROFILE_KEY").unwrap_or_else(|_| "1366x768".into());
        let (width, height) = key.split_once('x').expect("WxH");
        let (client_w, client_h) = (
            width.parse::<u32>().expect("width"),
            height.parse::<u32>().expect("height"),
        );

        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");

        // Labels come from the default-mode set: same account, same character. Only the
        // pixels differ between modes, which is the whole point of re-recording.
        let source = crate::profile::load_for_mode(
            client_w,
            client_h,
            settings::UiMode::Default,
        )
        .expect("default profile");
        let mut target = crate::profile::load_for_mode(client_w, client_h, settings::ui_mode())
            .expect("profile for this mode");

        if let Some(gray) = ocr::map_label(&frame, settings::load_calibration().map_box()) {
            let label = std::env::var("MSCOUT_MAP_NAME")
                .ok()
                .or_else(|| source.maps.first().map(|template| template.label.clone()))
                .unwrap_or_else(|| "unknown".into());
            target.maps.retain(|template| template.label != label);
            target.maps.push(Template {
                label,
                w: gray.w as u32,
                h: gray.h as u32,
                pixels: settings::to_hex(&gray.px),
            });
        }

        let name_label = std::env::var("MSCOUT_NAME")
            .ok()
            .or_else(|| source.identities.name.map(|template| template.label));
        let job_label = std::env::var("MSCOUT_JOB")
            .ok()
            .or_else(|| source.identities.job.map(|template| template.label));
        for (kind, region, label) in [
            ("name", target.calibration.name, name_label),
            ("job", target.calibration.job, job_label),
        ] {
            let (Some(region), Some(label)) = (region, label) else {
                continue;
            };
            let Some(gray) = ocr::crop_gray(&frame, region) else {
                continue;
            };
            let template = Template {
                label,
                w: gray.w as u32,
                h: gray.h as u32,
                pixels: settings::to_hex(&gray.px),
            };
            if kind == "name" {
                target.identities.name = Some(template);
            } else {
                target.identities.job = Some(template);
            }
        }

        crate::profile::save(&target).expect("save profile");
        println!(
            "{}: maps={} name={:?} job={:?}",
            target.key(),
            target.maps.len(),
            target.identities.name.as_ref().map(|t| t.label.clone()),
            target.identities.job.as_ref().map(|t| t.label.clone())
        );
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
    #[ignore = "proof: reads saved full frames, including the client map table"]
    fn reads_saved_map_frames() {
        let data = crate::data_dir().expect("data dir");
        for (folder, map_name, street) in [
            (
                "20261114-122032",
                "Kerning City Civic Center",
                "Victoria Road",
            ),
            ("20261114-122137", "Sunset Sky", "Victoria Road"),
        ] {
            let path = data.join("map-samples").join(folder).join("frame.bmp");
            if !path.is_file() {
                println!("{folder}: no frame.bmp");
                continue;
            }
            let frame = load_bmp_frame(&path);
            let profile = profile::activate(frame.width, frame.height).expect("profile");
            let mscw = settings::mscw_dir().map(std::path::PathBuf::from);

            // Simulate a clean install: the preset supplies boxes, everything
            // glyph-like is seeded from the saved frame + the installed client.
            let exp_stored = if profile.atlas.glyphs.is_empty() {
                let region = profile.calibration.exp.expect("EXP box");
                let gray = ocr::crop_gray(&frame, region).expect("EXP crop");
                let dir = mscw.as_ref().expect("mscwDir");
                crate::game_font::seed_exp_atlas(&gray, dir)
                    .expect("seed EXP")
                    .0
            } else {
                profile.atlas.clone()
            };
            let level_stored = if profile.level_atlas.glyphs.is_empty() {
                let mut stored = settings::Atlas::default();
                if let Some(region) = profile.calibration.level {
                    if let Some(gray) = ocr::crop_gray(&frame, region) {
                        if let Ok((seeded, _)) = crate::game_font::seed_level_atlas(
                            &gray,
                            mscw.as_ref().expect("mscwDir"),
                        ) {
                            stored = seeded;
                        }
                    }
                }
                stored
            } else {
                profile.level_atlas.clone()
            };
            if profile.identity_atlas.glyphs.is_empty() {
                for region in [profile.calibration.job, profile.calibration.name] {
                    let Some(region) = region.filter(Region::is_usable) else {
                        continue;
                    };
                    let Some(gray) = ocr::crop_gray(&frame, region) else {
                        continue;
                    };
                    if let Some((stored, _)) = crate::font_seed::seed_identity(&gray) {
                        settings::save_identity_atlas(&stored).expect("save identity");
                        break;
                    }
                }
            }

            let readings = read_frame(
                &frame,
                &profile.calibration,
                &Atlas::from_settings(&exp_stored),
                &Atlas::from_settings(&level_stored),
                &Atlas::from_settings(&profile.identity_atlas),
                &profile.maps,
                &profile.streets,
                &profile.identities,
            )
            .expect("read saved frame");
            println!("{folder}: {readings:?}");
            assert_eq!(
                readings.map,
                format!("{street}\n{map_name}"),
                "the client map table should name this frame"
            );
            assert_eq!(readings.lv, 35, "level should be read/seeded");
            assert!(
                readings.exp > 70_000,
                "EXP should be read/seeded: {}",
                readings.exp
            );
            assert_eq!(readings.name.as_deref(), Some("Euroblood"));
            assert_eq!(readings.job.as_deref(), Some("Page"));
        }
    }

    #[test]
    #[ignore = "proof: reads the map name in all three minimap states"]
    fn reads_minimap_states() {
        let data = crate::data_dir().expect("data dir");
        for (folder, expected) in [
            ("20261114-194246", Some("Victoria Road\nKerning City")),
            ("20261114-194533", None),
            ("20261114-194948", Some("Victoria Road\nKerning City")),
            (
                "20261114-193427-folded",
                Some("Hidden Street\n1st Accompaniment <Exit>"),
            ),
        ] {
            let path = data.join("map-samples").join(folder).join("frame.bmp");
            if !path.is_file() {
                println!("{folder}: no frame.bmp");
                continue;
            }
            let frame = load_bmp_frame(&path);
            let profile = profile::activate(frame.width, frame.height).expect("profile");
            let readings = read_frame(
                &frame,
                &profile.calibration,
                &Atlas::from_settings(&profile.atlas),
                &Atlas::from_settings(&profile.level_atlas),
                &Atlas::from_settings(&profile.identity_atlas),
                &profile.maps,
                &profile.streets,
                &profile.identities,
            )
            .expect("read saved frame");
            println!(
                "{folder}: state={:?} map={:?}",
                ocr::detect_minimap(&frame).map(|minimap| minimap.state),
                readings.map
            );
            match expected {
                Some(expected) => {
                    assert_eq!(readings.map, expected, "{folder}: wrong map");
                    assert!(!readings.map_hidden, "{folder}: should not be hidden");
                }
                None => {
                    assert_eq!(
                        readings.map, "unknown",
                        "{folder}: a frame with no name must report unknown"
                    );
                    assert!(
                        readings.map_hidden,
                        "{folder}: title-bar-only must be flagged as hidden"
                    );
                }
            }
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

    #[test]
    #[ignore = "diagnostic: explains the live level box against the saved level atlas"]
    fn explains_live_level_box() {
        let width: u32 = std::env::var("MSCOUT_W")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1920);
        let height: u32 = std::env::var("MSCOUT_H")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1080);
        let ui_mode = std::env::var("MSCOUT_UI_MODE")
            .ok()
            .and_then(|value| settings::UiMode::parse(&value))
            .unwrap_or(settings::UiMode::Ideal);
        let profile = profile::load_for_mode(width, height, ui_mode).expect("profile");
        let region = profile.calibration.level.expect("level box");

        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = capture::client_frame(window.hwnd).expect("frame");
        let gray = ocr::crop_gray(&frame, region).expect("crop");
        println!("level box {region:?} crop {}x{}", gray.w, gray.h);
        println!(
            "adaptive shapes: {:?}",
            ocr::segments(&gray)
                .iter()
                .map(|shape| (shape.w, shape.h, shape.x, shape.y))
                .collect::<Vec<_>>()
        );
        println!(
            "bright shapes: {:?}",
            ocr::bright_segments(&gray, ocr::TEXT_FLOOR_PUB)
                .iter()
                .map(|shape| (shape.w, shape.h, shape.x, shape.y))
                .collect::<Vec<_>>()
        );
        for (row, line) in ocr::shape_art(&gray).iter().enumerate() {
            println!("{row:2} {line}");
        }

        let atlas = Atlas::from_settings(&profile.level_atlas);
        for guess in atlas.explain(&gray) {
            println!(
                "  shape {}x{} at {},{} -> '{}' d={:.1} known={}",
                guess.w, guess.h, guess.x, guess.y, guess.ch, guess.distance, guess.known
            );
        }
        println!("recognize = {:?}", atlas.recognize(&gray));
        println!("recognize_level = {:?}", atlas.recognize_level(&gray));
    }

    #[test]
    #[ignore = "diagnostic: explains every shape in the level box against the level atlas"]
    fn explains_level_box() {
        let profile = profile::load_for_mode(1920, 1200, settings::ui_mode()).expect("profile");
        let region = profile.calibration.level.expect("level box");
        let selector = crate::settings::WindowSelector {
            title: String::new(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");
        let gray = ocr::crop_gray(&frame, region).expect("crop");
        let atlas = Atlas::from_settings(&profile.level_atlas);
        println!("level box {region:?} crop {}x{}", gray.w, gray.h);
        let shapes = ocr::segments(&gray);
        println!("{} shapes", shapes.len());
        for guess in atlas.explain(&gray) {
            println!(
                "  shape {}x{} at {},{} -> best '{}' d={:.1} known={}",
                guess.w, guess.h, guess.x, guess.y, guess.ch, guess.distance, guess.known
            );
        }
        for glyph in &profile.level_atlas.glyphs {
            println!("  atlas '{}' {}x{}", glyph.ch, glyph.w, glyph.h);
        }
        println!("recognize = {:?}", atlas.recognize(&gray));
    }

    #[test]
    #[ignore = "diagnostic: reports the glyph count of each atlas in the saved profile"]
    fn dumps_atlas_counts() {
        let width: u32 = std::env::var("MSCOUT_W")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1920);
        let height: u32 = std::env::var("MSCOUT_H")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1200);
        let profile = profile::load_for_mode(width, height, settings::ui_mode()).expect("profile");
        println!(
            "profile {}x{} mode={:?}",
            profile.client_w, profile.client_h, profile.ui_mode
        );
        println!("  atlas       (EXP): {} glyphs", profile.atlas.glyphs.len());
        println!("  level_atlas      : {} glyphs", profile.level_atlas.glyphs.len());
        println!("  map_atlas        : {} glyphs", profile.map_atlas.glyphs.len());
        for glyph in &profile.level_atlas.glyphs {
            println!("    level '{}' {}x{}", glyph.ch, glyph.w, glyph.h);
        }
        println!(
            "  active level_atlas via settings: {} glyphs",
            settings::load_level_atlas().glyphs.len()
        );
    }
}
