use std::cmp::Ordering;
use std::collections::VecDeque;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::capture::Frame;
use crate::capture::{self, Capturer};
use crate::exp_table;
use crate::ocr::{self, Atlas};
use crate::profile;
use crate::settings::{self, Calibration, Identities, Region, Template};
use crate::window_target::{self, TargetStatus};

const OUTCOME_WINDOW: usize = 10;
const DEGRADED_FAILURES: usize = 3;
const CROSS_CHECK_TOLERANCE: f64 = 1.0;

/// The percentage is redundant with EXP and level, so a small disagreement is a
/// bad percentage read, not a bad EXP/level. Repair it up to one point; a larger
/// gap means one of the two independent reads is wrong and the frame is rejected.
const PCT_REPAIR_TOLERANCE: f64 = 1.0;

enum LevelCheck {
    Agrees,
    RepairPct(f64),
    Disagrees,
}

/// Replace the percentage part of a readout string, keeping the EXP digits and
/// the bracket style. Calibration shows this string, so it must not continue to
/// show a percentage the engine has repaired.
fn replace_pct_text(text: &str, pct: f64) -> String {
    let Some(start) = text.find('[') else {
        return text.to_string();
    };
    let Some(end) = text[start + 1..].find('%').map(|index| start + 1 + index) else {
        return text.to_string();
    };
    let mut out = format!("{}[{pct:.2}%", &text[..start]);
    if text[end + 1..].contains(']') {
        out.push(']');
    }
    out
}

/// Compare the EXP and percentage with the level's EXP requirement. With no
/// level there is nothing to cross-check, so the EXP is kept.
fn check_exp_level(exp: i64, pct: f64, lv: u32) -> LevelCheck {
    let Some(required) = exp_table::required(lv) else {
        return LevelCheck::Agrees;
    };
    let expected = exp as f64 / required * 100.0;
    let tolerance = if lv <= 49 || lv >= 90 {
        0.02
    } else {
        CROSS_CHECK_TOLERANCE
    };
    let difference = (pct - expected).abs();
    if difference <= tolerance {
        LevelCheck::Agrees
    } else if difference <= PCT_REPAIR_TOLERANCE {
        LevelCheck::RepairPct(expected)
    } else {
        LevelCheck::Disagrees
    }
}
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
            t: record
                .get("t")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
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
                        .unwrap_or_else(|error| error.into_inner()) = Some(outcome.profile.clone());
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
                }
                // Display each successful read immediately; only the history write
                // waits for consecutive good reads. That keeps a map change from
                // waiting three ticks before the overlay shows the new name.
                let _ = app.emit("live-sample", &outcome.sample);
                if outcome.verified {
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
    // The Filter Applied tile needs the white-ink key; Ideal reads from luma.
    if let Some(region) = profile.calibration.level.filter(Region::is_usable) {
        match crate::game_font::seed_level_best(frame, region, dir) {
            Some((level_stored, (w, h), text)) => {
                let _ = settings::save_level_atlas(&level_stored);
                crate::calibration::log(&format!("auto-seeded level {w}x{h} from '{text}'"));
            }
            None => crate::calibration::log("seed from game: level not seeded"),
        }
    }

    crate::calibration::log(&format!(
        "auto-seeded {}x{} digits at h={height} from {}",
        profile.client_w,
        profile.client_h,
        dir.display()
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

fn tick(engine: &mut EngineState, status: &TargetStatus) -> Result<TickOutcome, TickError> {
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
            let cropped = capture::crop_to_client(frame, hwnd).map_err(TickError::Lost)?;
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
                crate::calibration::log(&format!("auto-seeded name/job text at {px}px from Arial"));
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
                return Err(TickError::Transient(
                    "seeded the digits from the game".into(),
                ));
            }
        }
        return Err(TickError::Uncalibrated(format!(
            "no digits learned for {}x{}",
            profile.client_w, profile.client_h
        )));
    }

    if let (Some(width), Some(height)) =
        (profile.calibration.client_w, profile.calibration.client_h)
    {
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
            if readings.lv > 0 && readings.lv == previous.lv && readings.exp < previous.exp =>
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
    engine.good_reads = if matches!(state, "gaining" | "idle") {
        engine.good_reads.saturating_add(1)
    } else {
        0
    };
    let verified = engine.good_reads >= profile::VERIFY_READS || profile::VERIFY_READS == 0;

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
    // A mismatch means one of the two boxes is wrong. The percentage is
    // redundant, so a small gap is repaired from the EXP and level (the Filtered
    // `23.33` can read as `23.83`); a large gap invalidates both the EXP and the
    // level while map/name/job are kept.
    if lv > 0 && exp_table::required(lv).is_none() {
        return Err(format!("level {lv} is outside the EXP table"));
    }
    let (lv, level_text, exp, pct, exp_text, verified) = match check_exp_level(exp, pct, lv) {
        LevelCheck::Agrees => (lv, level_text, exp, pct, exp_text, true),
        LevelCheck::RepairPct(expected) => {
            log_once(&format!(
                "read_frame: percentage {pct:.2}% does not match level {lv} / {} EXP — using {expected:.2}%",
                exp
            ));
            let repaired_text = replace_pct_text(&exp_text, expected);
            (lv, level_text, exp, expected, repaired_text, true)
        }
        LevelCheck::Disagrees => {
            log_once(&format!(
                "read_frame: level {lv} does not fit {pct:.2}% for {} EXP — reporting EXP and level as unrecognised",
                exp
            ));
            (0, String::new(), 0, 0.0, String::new(), false)
        }
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
    // The profile's identity atlas is already seeded from both lines (and the
    // merged set holds each line's size bucket). Re-seeding both lines on every
    // tick costs seconds of GDI rendering; only do it when the profile has no
    // identity atlas at all.
    let (name_atlas, job_atlas) = if identity_atlas.glyphs.is_empty() {
        (
            seed_line_identity(frame, calibration.name),
            seed_line_identity(frame, calibration.job),
        )
    } else {
        (None, None)
    };
    let name = read_identity(
        frame,
        calibration.name,
        name_atlas.as_ref().unwrap_or(identity_atlas),
        identities.name.as_ref(),
        IdentityKind::Name,
    );
    let job = read_identity(
        frame,
        calibration.job,
        job_atlas.as_ref().unwrap_or(identity_atlas),
        identities.job.as_ref(),
        IdentityKind::Job,
    );

    Ok(Readings {
        lv,
        exp,
        pct,
        expected_pct,
        map,
        conf: if !verified {
            0.0
        } else if lv == 0 {
            exp_conf
        } else {
            exp_conf.min(level_conf)
        },
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
    static LAST: std::sync::Mutex<Option<HashMap<String, String>>> = std::sync::Mutex::new(None);
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
    if atlas.glyphs.is_empty() {
        log_once("level_reading: no level glyphs learned yet");
        return unknown;
    }

    // Read both extractions: plain luma for Ideal, the white key for the
    // Filter Applied tile whose coloured edge merges with a digit. The longer
    // valid read wins, then confidence, so a frame that already reads is not
    // changed by the weaker extraction.
    let normal = ocr::crop_gray(frame, region);
    let keyed = ocr::crop_white_key(frame, region);
    type LevelRead<'a> = (&'a str, Result<(String, f64), String>);
    let mut readings: Vec<LevelRead> = Vec::new();
    if let Some(gray) = &normal {
        readings.push(("luma", atlas.recognize_level(gray)));
    }
    if let Some(keyed) = &keyed {
        readings.push(("white key", atlas.recognize_level_keyed(keyed)));
    }

    let mut best: Option<(u32, String, f64)> = None;
    let mut errors: Vec<String> = Vec::new();
    for (label, result) in readings {
        match result {
            Ok((text, confidence)) => match ocr::parse_level(&text) {
                Some(lv) if exp_table::required(lv).is_some() => {
                    let better = best.as_ref().map_or(true, |(_, current, current_conf)| {
                        text.len() > current.len()
                            || (text.len() == current.len() && confidence > *current_conf)
                    });
                    if better {
                        best = Some((lv, text, confidence));
                    }
                }
                Some(lv) => errors.push(format!("{label}: '{text}' -> {lv} is outside the table")),
                None => errors.push(format!("{label}: read '{text}' — not a level")),
            },
            Err(error) => errors.push(format!("{label}: {error}")),
        }
    }

    match best {
        Some(best) => Ok(best),
        None => {
            if errors.is_empty() {
                errors.push("the box is outside the window".into());
            }
            let (w, h) = normal.as_ref().map_or((0, 0), |gray| (gray.w, gray.h));
            log_once(&format!(
                "level_reading: box {region:?} crop {w}x{h} failed: {}",
                errors.join("; ")
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IdentityKind {
    Name,
    Job,
}

fn finish_identity(text: &str, kind: IdentityKind) -> String {
    let text = text.trim();
    match kind {
        IdentityKind::Name => crate::font_seed::fix_identity_case(text),
        IdentityKind::Job => title_case_job(text),
    }
}

fn title_case_job(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Combine the glyph read with OCR when the glyph atlas is partial. The OCR can
/// drop a leading capital (`age` for `Page`); the glyph reader matched that
/// shape exactly, so its confident prefix is put back.
fn choose_identity_text(
    glyph_text: &str,
    guesses: &[ocr::Guess],
    known_ratio: f64,
    confidence: f64,
    ocr: &[String],
) -> Option<String> {
    let shape_count = guesses.len();
    let glyph_letters: String = glyph_text
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    let mut longest: Option<&String> = None;

    for candidate in ocr {
        let letters: String = candidate
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect();
        if letters.len() < 2 {
            continue;
        }
        if letters.len() == shape_count {
            // OCR explains the same number of shapes; use it when the glyph
            // atlas is weak, otherwise keep the exact glyph read.
            if known_ratio < 0.75 {
                return Some(candidate.clone());
            }
            continue;
        }
        if shape_count > letters.len() {
            let gap = shape_count - letters.len();
            if gap <= 2 {
                let prefix: Option<String> = guesses
                    .iter()
                    .take(gap)
                    .map(|guess| {
                        (guess.distance <= 65.0 && guess.ch.is_ascii_alphanumeric())
                            .then_some(guess.ch)
                    })
                    .collect();
                if let Some(prefix) = prefix {
                    let fused = format!("{prefix}{candidate}");
                    if fused
                        .chars()
                        .filter(|ch| ch.is_ascii_alphanumeric())
                        .count()
                        == shape_count
                    {
                        return Some(fused);
                    }
                }
            }
        }
        if letters.len() > glyph_letters.len() && known_ratio < 0.5 {
            return Some(candidate.clone());
        }
        if longest.map_or(true, |current| candidate.len() > current.len()) {
            longest = Some(candidate);
        }
    }

    if known_ratio < 0.5 && confidence < 0.7 {
        longest.cloned()
    } else {
        None
    }
}

fn read_identity(
    frame: &Frame,
    region: Option<Region>,
    atlas: &Atlas,
    template: Option<&Template>,
    kind: IdentityKind,
) -> Option<String> {
    let region = region.filter(Region::is_usable)?;

    let mut glyph_text = String::new();
    let mut guesses = Vec::new();
    let mut confidence = 0.0;
    if !atlas.glyphs.is_empty() {
        if let Some(gray) = ocr::crop_gray(frame, region) {
            let gap = (gray.h as i32 / 3).max(2);
            if let Ok((text, conf)) = atlas.recognize_text(&gray, gap) {
                glyph_text = text.trim().to_string();
                confidence = conf;
            }
            guesses = atlas.explain_text(&gray);
        }
    }

    let total = guesses.len();
    let known = guesses.iter().filter(|guess| guess.known).count();
    let known_ratio = if total == 0 {
        0.0
    } else {
        known as f64 / total as f64
    };
    let mean = if total == 0 {
        f64::MAX
    } else {
        guesses.iter().map(|guess| guess.distance).sum::<f64>() / total as f64
    };

    // Ideal and the 1366 renderings reproduce every shape exactly. Keep the
    // exact glyph read and do not pay for OCR.
    if total > 0 && known == total && mean <= 30.0 {
        let text = finish_identity(&glyph_text, kind);
        if !text.is_empty() {
            return Some(text);
        }
    }

    let ocr = crate::winocr::identity_candidates(frame, region, kind == IdentityKind::Job);
    if let Some(text) = choose_identity_text(&glyph_text, &guesses, known_ratio, confidence, &ocr) {
        let text = finish_identity(&text, kind);
        if !text.is_empty() {
            return Some(text);
        }
    }

    if !glyph_text.is_empty() && confidence >= 0.55 && known_ratio >= 0.5 {
        let text = finish_identity(&glyph_text, kind);
        if !text.is_empty() {
            return Some(text);
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

fn match_street(frame: &Frame, region: Option<Region>, streets: &[Template]) -> Option<String> {
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
        let Some(index) = crate::font_seed::label_index(std::path::Path::new(&dir), px) else {
            continue;
        };
        let max_score = if px >= 14 { 0.95 } else { 0.55 };
        if let Some((street, score)) =
            crate::font_seed::match_label(line, &index.streets, 14, max_score)
        {
            if best.as_ref().map_or(true, |(_, current)| score < *current) {
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

fn match_street_fingerprint(frame: &Frame, region: Region, streets: &[Template]) -> Option<String> {
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
    let gray = ocr::Gray { w, h, px: bytes };
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

    // OCR is the better text source for the Roman-numeral siblings and for maps
    // the client table does not contain. An exact table match from OCR outranks
    // the whole-line DTW, which can read a single trailing `I` as `III`.
    let label_ocr = match_label_ocr(frame);
    if let Some((_, Some(name), true)) = label_ocr.as_ref() {
        log_once(&format!("map text from OCR (exact): {name:?}"));
        return name.clone();
    }

    // Nothing taught matched: the client's own map table has the exact name. The
    // label is Arial Bold 12, so a width filter plus a shape-sequence DTW against
    // the rendered names identifies it without segmenting the line.
    if let Some(name) = match_map_name(frame) {
        return name;
    }

    // A map that is not in the client table still has its name on screen. OCR is
    // the better text source than the tiny map glyph atlas, which can be learned
    // misaligned and confidently return the wrong letters. `match_label_ocr`
    // snaps known names to the table and otherwise returns the literal line.
    if let Some((_, Some(name), _)) = label_ocr {
        log_once(&format!("map text from OCR: {name:?}"));
        return name;
    }

    // No OCR (or no label): fall back to the learned glyph text. A near miss
    // snaps to a name that was taught explicitly.
    let map_atlas = Atlas::from_settings(&settings::load_map_atlas());
    if let Some(lines) = ocr::read_map_text(frame, &map_atlas) {
        // The map name is the bottom line; the street is matched separately.
        let text = lines.last().cloned().unwrap_or_default();
        if !text.trim().is_empty() {
            log_once(&format!("map text fallback: {text:?}"));
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

/// Last expanded-label result, keyed by the label's pixels and the client name
/// table. The label only changes on a map transition, so a stationary frame
/// skips the table/DTW/OCR work entirely.
static MAP_CACHE: std::sync::Mutex<Option<(u64, String, Option<String>)>> =
    std::sync::Mutex::new(None);

fn map_label_hash(frame: &Frame) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let lines = ocr::map_label_lines(frame)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for line in &lines {
        line.w.hash(&mut hasher);
        line.h.hash(&mut hasher);
        line.px.hash(&mut hasher);
    }
    settings::mscw_dir().hash(&mut hasher);
    Some(hasher.finish())
}

fn folded_label_hash(label: &ocr::Gray) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    label.w.hash(&mut hasher);
    label.h.hash(&mut hasher);
    label.px.hash(&mut hasher);
    settings::mscw_dir().hash(&mut hasher);
    hasher.finish()
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
            let cache_key = folded_label_hash(&label);
            if let Ok(guard) = MAP_CACHE.lock() {
                if let Some((cached_key, cached_map, cached_street)) = guard.as_ref() {
                    if *cached_key == cache_key {
                        return (cached_map.clone(), cached_street.clone(), false);
                    }
                }
            }
            if let Some((street, name)) = match_folded_ocr(&label) {
                log_once(&format!("folded minimap OCR: {street:?} / {name:?}"));
                if let Ok(mut guard) = MAP_CACHE.lock() {
                    *guard = Some((cache_key, name.clone(), Some(street.clone())));
                }
                return (name, Some(street), false);
            }
            let Some(dir) = settings::mscw_dir() else {
                return ("unknown".into(), None, false);
            };
            let mut best: Option<((String, String), f64)> = None;
            for px in LABEL_SIZES {
                let Some(index) = crate::font_seed::label_index(std::path::Path::new(&dir), px)
                else {
                    continue;
                };
                let max_score = if px >= 14 { 0.95 } else { 0.55 };
                if let Some((pair, score)) =
                    crate::font_seed::match_folded_label(&label, &index.folded_lines, 14, max_score)
                {
                    if best.as_ref().map_or(true, |(_, current)| score < *current) {
                        best = Some((pair, score));
                    }
                }
            }
            match best {
                Some(((street, name), score)) => {
                    log_once(&format!(
                        "folded minimap: {street:?} / {name:?} (score {score:.3})"
                    ));
                    if let Ok(mut guard) = MAP_CACHE.lock() {
                        *guard = Some((cache_key, name.clone(), Some(street.clone())));
                    }
                    (name, Some(street), false)
                }
                None => {
                    log_once("folded minimap: label did not match the client table");
                    ("unknown".into(), None, false)
                }
            }
        }
        _ => {
            // The label is static while the player stands still. Hashing its
            // pixels is far cheaper than the table/DTW/OCR match, so a repeated
            // frame reuses the last result. Taught fingerprints can change at
            // runtime, so those profiles keep matching every frame.
            let cacheable = maps.is_empty() && streets.is_empty();
            let cache_key = cacheable.then(|| map_label_hash(frame)).flatten();
            if let Some(key) = cache_key {
                if let Ok(guard) = MAP_CACHE.lock() {
                    if let Some((cached_key, cached_map, cached_street)) = guard.as_ref() {
                        if *cached_key == key {
                            return (cached_map.clone(), cached_street.clone(), false);
                        }
                    }
                }
            }

            let mut map_name = match_map(frame, calibration, maps);
            let mut street = match_street(frame, calibration.street, streets);
            if map_name == "unknown" || street.is_none() {
                if let Some((ocr_street, ocr_map, _)) = match_label_ocr(frame) {
                    map_name = ocr_map.unwrap_or(map_name);
                    street = ocr_street.or(street);
                }
            }
            if let Some(key) = cache_key {
                if map_name != "unknown" || street.is_some() {
                    if let Ok(mut guard) = MAP_CACHE.lock() {
                        *guard = Some((key, map_name.clone(), street.clone()));
                    }
                }
            }
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
            if best.as_ref().map_or(true, |(_, current)| score < *current) {
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

fn normalize_label(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == ' ' {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_label_line(text: &str) -> String {
    text.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != ' ')
        .trim()
        .to_string()
}

/// OCR fallback for the expanded minimap label. The text reader supplies the
/// street and the base map name; the client table narrows the candidates and
/// the trailing `<N>` is chosen from the suffix pixels, because Windows OCR
/// drops the bracketed token.
fn match_label_ocr(frame: &Frame) -> Option<(Option<String>, Option<String>, bool)> {
    let dir = settings::mscw_dir()?;
    let dir = std::path::Path::new(&dir);
    let entries = crate::map_names::load(dir).ok()?;
    let regions = ocr::map_label_line_regions(frame)?;
    if regions.is_empty() {
        return None;
    }

    let x0 = regions.iter().map(|(region, _)| region.x).min()? - 4;
    let y0 = regions.iter().map(|(region, _)| region.y).min()? - 4;
    let x1 = regions
        .iter()
        .map(|(region, _)| region.x + region.w)
        .max()?
        + 4;
    let y1 = regions
        .iter()
        .map(|(region, _)| region.y + region.h)
        .max()?
        + 4;
    let union = Region {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    };
    let keyed = ocr::crop_white_key(frame, union)?;
    let lines: Vec<String> = crate::winocr::recognize(&keyed)
        .ok()?
        .into_iter()
        .map(|line| clean_label_line(&line))
        .filter(|line| line.chars().any(|ch| ch.is_alphabetic()))
        .collect();
    if lines.is_empty() {
        return None;
    }
    let map_text = lines.last()?.as_str();
    let street_text = if lines.len() >= 2 {
        lines.first().map(String::as_str)
    } else {
        None
    };
    let live = regions.last().map(|(_, gray)| gray);
    Some(resolve_label_text(
        dir,
        &entries,
        street_text,
        map_text,
        live,
    ))
}

/// Resolve one OCR label against the client table. Known names snap to the table
/// (exact, suffix, then fuzzy); a name the table does not contain is returned as
/// the literal on-screen text. That last case is why a map added after the
/// client's string table can still be named.
fn resolve_label_text(
    dir: &std::path::Path,
    entries: &[crate::map_names::MapEntry],
    street_text: Option<&str>,
    map_text: &str,
    live: Option<&ocr::Gray>,
) -> (Option<String>, Option<String>, bool) {
    let mut street: Option<(String, f64)> = None;
    if let Some(street_text) = street_text {
        for entry in entries {
            if entry.street.is_empty() {
                continue;
            }
            let score = ocr::label_similarity(street_text, &entry.street);
            if street.as_ref().map_or(true, |(_, best)| score > *best) {
                street = Some((entry.street.clone(), score));
            }
        }
    }
    let street = street.and_then(|(name, score)| (score >= 0.75).then_some(name));

    let ocr_map = normalize_label(map_text);
    let literal = clean_label_line(map_text);
    if ocr_map.is_empty() || literal.is_empty() {
        return (street, None, false);
    }

    let mut candidates: Vec<&crate::map_names::MapEntry> = entries
        .iter()
        .filter(|entry| {
            let name = normalize_label(&entry.name);
            name == ocr_map
                || name.starts_with(&ocr_map)
                || ocr::label_similarity(&entry.name, map_text) >= 0.75
        })
        .collect();
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    candidates.dedup_by(|a, b| a.name == b.name);

    if candidates.is_empty() {
        log_once(&format!("map label OCR literal: {literal:?}"));
        return (street, Some(literal), false);
    }

    // OCR can read the full name exactly (the roman-numeral maps are the obvious
    // case); that beats any fuzzy/DTW alternative.
    let exact: Vec<&crate::map_names::MapEntry> = candidates
        .iter()
        .copied()
        .filter(|entry| normalize_label(&entry.name) == ocr_map)
        .collect();
    if exact.len() == 1 {
        return (street, Some(exact[0].name.clone()), true);
    }
    if candidates.len() == 1 {
        return (street, Some(candidates[0].name.clone()), false);
    }

    // `<N>` instances are told apart by their suffix.
    let suffix_candidates: Vec<&crate::map_names::MapEntry> = candidates
        .iter()
        .copied()
        .filter(|entry| {
            entry.name.contains('<') && normalize_label(&entry.name).starts_with(&ocr_map)
        })
        .collect();
    if !suffix_candidates.is_empty() {
        if let Some(live) = live {
            let mut scored: Vec<(f64, String)> = Vec::new();
            for px in LABEL_SIZES {
                let Some(index) = crate::font_seed::label_index(dir, px) else {
                    continue;
                };
                for entry in &suffix_candidates {
                    let Some((_, rendered)) =
                        index.map_names.iter().find(|(name, _)| name == &entry.name)
                    else {
                        continue;
                    };
                    if let Some(score) = crate::font_seed::suffix_distance(live, rendered) {
                        scored.push((score, entry.name.clone()));
                    }
                }
            }
            scored.sort_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((best_score, best_name)) = scored.first() {
                let runner = scored
                    .iter()
                    .find(|(_, name)| name != best_name)
                    .map(|(score, _)| *score)
                    .unwrap_or(f64::MAX);
                if *best_score <= 2.5 && runner - *best_score >= 0.03 {
                    log_once(&format!(
                        "map label OCR: {best_name:?} (suffix score {best_score:.2})"
                    ));
                    return (street, Some(best_name.clone()), false);
                }
            }
        }
    }

    // OCR often reads one character wrong (`Deep Vallev I`). Accept a clear
    // fuzzy winner; roman-numeral siblings differ by whole characters, so the
    // margin keeps them safe.
    let mut by_similarity: Vec<(f64, &crate::map_names::MapEntry)> = candidates
        .iter()
        .map(|entry| (ocr::label_similarity(&entry.name, map_text), *entry))
        .collect();
    by_similarity.sort_by(|a, b| b.0.total_cmp(&a.0));
    if let Some((best, entry)) = by_similarity.first() {
        let runner = by_similarity.get(1).map(|(score, _)| *score).unwrap_or(0.0);
        if *best >= 0.80 && best - runner >= 0.05 {
            log_once(&format!(
                "map label OCR fuzzy: {:?} (score {best:.2})",
                entry.name
            ));
            return (street, Some(entry.name.clone()), false);
        }
    }

    log_once(&format!("map label OCR literal: {literal:?}"));
    (street, Some(literal), false)
}

/// OCR fallback for the folded `Street :Map` bar. The line is matched to the
/// client table; a shared base name is resolved by the trailing `<N>` suffix,
/// which OCR may or may not have read.
fn match_folded_ocr(label: &ocr::Gray) -> Option<(String, String)> {
    let dir = settings::mscw_dir()?;
    let dir = std::path::Path::new(&dir);
    let lines = crate::winocr::recognize(label).ok()?;
    let text = lines.into_iter().next()?;
    let (street_text, map_text) = text.split_once(':')?;
    let street_text = clean_label_line(street_text);
    let map_text = clean_label_line(map_text);
    if street_text.is_empty() || map_text.is_empty() {
        return None;
    }
    let ocr_map = normalize_label(&map_text);
    if ocr_map.is_empty() {
        return None;
    }

    let mut candidates: Vec<((String, String), ocr::Gray)> = Vec::new();
    for px in LABEL_SIZES {
        let Some(index) = crate::font_seed::label_index(dir, px) else {
            continue;
        };
        for ((street, name), rendered) in &index.folded_lines {
            if ocr::label_similarity(&street_text, street) < 0.75 {
                continue;
            }
            let normalized = normalize_label(name);
            if (normalized == ocr_map
                || normalized.starts_with(&ocr_map)
                || ocr::label_similarity(name, &map_text) >= 0.75)
                && !candidates
                    .iter()
                    .any(|((s, n), _)| s == street && n == name)
            {
                candidates.push(((street.clone(), name.clone()), rendered.clone()));
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let exact: Vec<&((String, String), ocr::Gray)> = candidates
        .iter()
        .filter(|((_, name), _)| normalize_label(name) == ocr_map)
        .collect();
    if exact.len() == 1 {
        return Some(exact[0].0.clone());
    }
    if candidates.len() == 1 {
        return Some(candidates[0].0.clone());
    }

    let suffix_names: Vec<(String, String)> = candidates
        .iter()
        .filter(|((_, name), _)| name.contains('<') && normalize_label(name).starts_with(&ocr_map))
        .map(|((street, name), _)| (street.clone(), name.clone()))
        .collect();
    if !suffix_names.is_empty() {
        let mut variants: Vec<crate::font_seed::FoldedLine> = Vec::new();
        for px in LABEL_SIZES {
            let Some(index) = crate::font_seed::label_index(dir, px) else {
                continue;
            };
            for ((street, name), rendered) in &index.folded_lines {
                if suffix_names.iter().any(|(s, n)| s == street && n == name) {
                    variants.push(((street.clone(), name.clone()), rendered.clone()));
                }
            }
        }
        let mut best_by_name: std::collections::BTreeMap<(String, String), f64> =
            std::collections::BTreeMap::new();
        for (pair, rendered) in &variants {
            if let Some(score) = crate::font_seed::suffix_distance(label, rendered) {
                let entry = best_by_name.entry(pair.clone()).or_insert(f64::MAX);
                if score < *entry {
                    *entry = score;
                }
            }
        }
        let mut scored: Vec<(f64, (String, String))> = best_by_name
            .into_iter()
            .map(|(pair, score)| (score, pair))
            .collect();
        scored.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((best, pair)) = scored.first() {
            let runner = scored
                .iter()
                .find(|(_, other)| other != pair)
                .map(|(score, _)| *score)
                .unwrap_or(f64::MAX);
            if *best <= 2.5 && runner - *best >= 0.03 {
                return Some(pair.clone());
            }
        }
    }

    let mut by_similarity: Vec<(f64, (String, String))> = candidates
        .iter()
        .map(|((street, name), _)| {
            (
                ocr::label_similarity(&map_text, name),
                (street.clone(), name.clone()),
            )
        })
        .collect();
    by_similarity.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, pair) = by_similarity.first()?;
    let runner = by_similarity.get(1).map(|(score, _)| *score).unwrap_or(0.0);
    if *best >= 0.85 && best - runner >= 0.05 {
        return Some(pair.clone());
    }
    None
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
    fn unknown_map_label_falls_back_to_the_ocr_text() {
        let entries = vec![
            crate::map_names::MapEntry {
                name: "Perion".into(),
                street: "Victoria Road".into(),
            },
            crate::map_names::MapEntry {
                name: "Ellinia".into(),
                street: "Victoria Road".into(),
            },
            crate::map_names::MapEntry {
                name: "An Unrelated Place".into(),
                street: "Warning Street".into(),
            },
        ];
        let (street, map, exact) = resolve_label_text(
            std::path::Path::new("."),
            &entries,
            Some("Warning Street"),
            "Where the Balrogs Live",
            None,
        );
        assert_eq!(street.as_deref(), Some("Warning Street"));
        assert_eq!(map.as_deref(), Some("Where the Balrogs Live"));
        assert!(!exact);
    }

    #[test]
    #[ignore = "requires MSCW: live Tree That Grew I minimap label"]
    fn reads_live_tree_that_grew_one() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/1920x1080@filtered/minimap-tree-1.png");
        let image = image::open(&path).expect("minimap fixture").to_rgba8();
        let frame = Frame {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        };

        let (street, map, exact) = match_label_ocr(&frame).expect("map label");
        assert_eq!(street.as_deref(), Some("Victoria Road"));
        assert_eq!(map.as_deref(), Some("The Tree That Grew I"));
        assert!(exact);

        // The whole-line DTW used to pick the `III` sibling; an exact OCR match
        // must win before it runs.
        assert_eq!(
            match_map(&frame, &Calibration::default(), &[]),
            "The Tree That Grew I"
        );
    }

    #[test]
    #[ignore = "requires MSCW: Balrog map literal against the live client table"]
    fn balrog_map_is_literal_against_the_live_table() {
        let Some(dir) = settings::mscw_dir() else {
            return;
        };
        let entries = crate::map_names::load(std::path::Path::new(&dir)).expect("map table");
        let (street, map, exact) = resolve_label_text(
            std::path::Path::new(&dir),
            &entries,
            Some("Warning Street"),
            "Where the Balrogs Live",
            None,
        );
        assert_eq!(street.as_deref(), Some("Warning Street"));
        assert_eq!(map.as_deref(), Some("Where the Balrogs Live"));
        assert!(!exact);
    }

    #[test]
    fn known_map_label_snaps_to_the_client_table() {
        let entries = vec![crate::map_names::MapEntry {
            name: "Deep Valley I".into(),
            street: "Warning Street".into(),
        }];
        let (street, map, exact) = resolve_label_text(
            std::path::Path::new("."),
            &entries,
            Some("Warning Street"),
            "Deep Vallev I",
            None,
        );
        assert_eq!(street.as_deref(), Some("Warning Street"));
        assert_eq!(map.as_deref(), Some("Deep Valley I"));
        assert!(!exact);
    }

    #[test]
    fn one_line_map_label_has_no_street() {
        let (street, map, exact) = resolve_label_text(
            std::path::Path::new("."),
            &[],
            None,
            "Where the Balrogs Live",
            None,
        );
        assert!(street.is_none());
        assert_eq!(map.as_deref(), Some("Where the Balrogs Live"));
        assert!(!exact);
    }

    #[test]
    fn repaired_pct_is_reflected_in_the_display_text() {
        assert_eq!(replace_pct_text("45,458[23.83%]", 23.333), "45,458[23.33%]");
        assert_eq!(replace_pct_text("45,458[23.83%", 23.333), "45,458[23.33%");
        assert_eq!(replace_pct_text("45,458", 23.333), "45,458");
    }

    #[test]
    fn exp_level_disagreement_is_detected_and_small_pct_errors_are_repaired() {
        assert!(matches!(
            check_exp_level(36_262, 18.61, 36),
            LevelCheck::Agrees
        ));
        assert!(matches!(
            check_exp_level(6_818, 18.90, 36),
            LevelCheck::Disagrees
        ));
        assert!(matches!(
            check_exp_level(6_818, 18.90, 0),
            LevelCheck::Agrees
        ));
        match check_exp_level(45_458, 23.83, 36) {
            LevelCheck::RepairPct(pct) => {
                assert!((pct - 23.333).abs() < 0.01, "{pct}");
            }
            _ => panic!("expected a repaired percentage"),
        }
    }

    #[test]
    #[ignore = "requires MSCW: Deep Valley minimap label"]
    fn reads_deep_valley_label() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/1920x1080@filtered/minimap-deep-valley.png");
        let image = image::open(&path).expect("minimap fixture").to_rgba8();
        let frame = Frame {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        };
        let (street, map, _) = match_label_ocr(&frame).expect("map label");
        assert_eq!(street.as_deref(), Some("Warning Street"));
        assert_eq!(map.as_deref(), Some("Deep Valley I"));
    }

    #[test]
    #[ignore = "diagnostic: times live read_frame on the current game frame"]
    fn live_read_timing() {
        let selector = settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = window_target::resolve_selector(&selector).expect("game window");
        let frame = crate::capture::client_frame(window.hwnd).expect("frame");
        let profile = crate::profile::activate(frame.width, frame.height).expect("profile");
        println!("profile {}", profile.key());
        let calibration = profile.calibration.clone();
        let atlas = Atlas::from_settings(&profile.atlas);
        let level_atlas = Atlas::from_settings(&profile.level_atlas);
        let identity_atlas = std::env::var("MSCOUT_FRESH_IDENTITY")
            .ok()
            .and_then(|value| {
                (value == "1").then(|| {
                    let (stored, _) =
                        seed_identity_from_frame(&frame, &calibration).expect("identity seed");
                    Atlas::from_settings(&stored)
                })
            })
            .unwrap_or_else(|| Atlas::from_settings(&profile.identity_atlas));
        for pass in 0..4 {
            let start = std::time::Instant::now();
            let readings = read_frame(
                &frame,
                &calibration,
                &atlas,
                &level_atlas,
                &identity_atlas,
                &profile.maps,
                &profile.streets,
                &profile.identities,
            );
            println!(
                "pass {pass}: {:.1} ms -> {:?}",
                start.elapsed().as_secs_f64() * 1000.0,
                readings.map(|r| (r.lv, r.exp, r.pct, r.name, r.job, r.map))
            );
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
