use std::collections::VecDeque;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::capture::Frame;
use crate::capture::{self, Capturer};
use crate::exp_table;
use crate::guard::{self, Guard};
use crate::map_match::*;
use crate::ocr::{self, Atlas};
use crate::profile;
use crate::settings::{self, Calibration, Identities, Region, StoredAtlas, Template};
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
    // Confirmed rows only differ from the bar by rounding; the estimated rows
    // get a wider band until they are confirmed in play.
    let tolerance = if exp_table::ESTIMATED.contains(&lv) {
        CROSS_CHECK_TOLERANCE
    } else {
        0.02
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
/// How often a profile with no glyphs retries seeding from the installed client.
/// Parsing the 35 MB canvas archive is not free, and a readout that is not on
/// screen yet would otherwise make the engine rebuild it every second.
const SEED_RETRY: Duration = Duration::from_secs(30);
/// Font-seeding retry cadence. Rendering Arial for every candidate size is
/// heavier than the digit seed, so a failed attempt backs off the same way.
const TEXT_SEED_RETRY: Duration = Duration::from_secs(30);

/// How long the last captured frame may stand in for a new one. The game redraws
/// continuously while it runs (animated maps, the character's idle motion), so a
/// frame this old means a hung client or a long black screen: re-reading it would
/// keep recording `idle` rows of a picture that is no longer on screen.
const STALE_FRAME: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub t: i64,
    pub lv: u32,
    pub exp: i64,
    pub pct: f64,
    pub map: String,
    pub state: guard::State,
    pub conf: f64,
    /// True when the minimap is collapsed to its title bar, so no map name is on
    /// screen. Absent in older rows, so it defaults to false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub map_hidden: bool,
    /// The character name when it was read, so a restart can tell whether the
    /// last row belongs to the character now on screen. Absent in older rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `"death"` on the first row after a confirmed death: EXP fell and stayed
    /// down. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
}

/// A sample as the overlay receives it. `recorded` says whether the row also
/// went into the history: the overlay shows every read at once, but computes
/// rates and the chart only from recorded rows, so its rates match the history's
/// (an unverified read can be a misread the guard would not record).
#[derive(Clone, Serialize)]
struct LiveSample<'a> {
    #[serde(flatten)]
    sample: &'a Sample,
    recorded: bool,
}

/// The reader stage that produced one field of a reading.
///
/// The ids are part of the diagnostic format (`atlas`, `font`, `fingerprint`,
/// `ocr`, `table`, `dtw`, `literal`, `glyphs`, `hidden`, `unknown`, `none`), so a
/// support file written by this build reads in an older one and vice versa.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadSource {
    /// The digit glyph atlas: the EXP readout and the level box.
    Atlas,
    /// The Arial letter atlas seeded from the font: the name and job lines.
    Font,
    /// A taught pixel fingerprint: a name, a job, a street, or a taught map label.
    Fingerprint,
    /// Windows OCR read the text on screen.
    Ocr,
    /// A stored table answered: the client's map-name table, or a taught map
    /// fingerprint held as a table of stored label pixels.
    Table,
    /// A distance match against rendered names — the whole-line or folded DTW.
    Dtw,
    /// On-screen OCR text kept as it was read because no table entry matched it.
    Literal,
    /// The learned minimap letter atlas read the label: its own text, or a taught
    /// map name it was close enough to. Reported apart from `ocr` and `dtw` because
    /// the label is 4-6 px tall and can be learned misaligned, so a name from this
    /// stage is worth less than one Windows OCR confirmed.
    Glyphs,
    /// Nothing to read: the minimap is collapsed to its title bar.
    Hidden,
    /// No provenance was recorded for this field: a reading written by an older
    /// build, or a stage that could not say which reader answered.
    #[default]
    Unknown,
    /// No value was produced for this field; `reason` says which check stopped it.
    None,
}

/// Where one field's value came from, and why it is empty when it is.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldProvenance {
    /// The reader stage that produced the value.
    #[serde(default)]
    pub source: ReadSource,
    /// Where that stage's data came from: `game` or `taught` for a digit atlas,
    /// `font` or `taught` for the identity atlas. Empty for a preset, a legacy set,
    /// or a stage with no stored data. It never changes a read decision; the engine
    /// stamps it from the atlases it loaded.
    #[serde(default)]
    pub engine: String,
    /// Why the field is empty. A rejection here — the level did not cross-check
    /// against the EXP table, the map line matched nothing — is what a saved
    /// diagnostic is read for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl FieldProvenance {
    /// A field that was read by `source`.
    fn read(source: ReadSource) -> Self {
        Self {
            source,
            engine: String::new(),
            reason: None,
        }
    }

    /// A field with no value, plus the reader's own reason.
    fn unread(source: ReadSource, reason: impl Into<String>) -> Self {
        Self {
            source,
            engine: String::new(),
            reason: Some(reason.into()),
        }
    }

    /// A field a check rejected, so nothing was read from it.
    pub(crate) fn rejected(reason: impl Into<String>) -> Self {
        Self::unread(ReadSource::None, reason)
    }
}

/// Where a reading came from: one entry per field, plus how long the read took.
///
/// It is serialised with `Readings` and repeated in the calibration diagnostic, so
/// a rejected or a surprising frame explains itself without the game.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadProvenance {
    #[serde(default)]
    pub exp: FieldProvenance,
    #[serde(default)]
    pub level: FieldProvenance,
    #[serde(default)]
    pub map: FieldProvenance,
    #[serde(default)]
    pub identity: FieldProvenance,
    /// Wall-clock duration of the read, in milliseconds.
    #[serde(default)]
    pub duration_ms: f64,
}

impl ReadProvenance {
    /// Record where the stored atlases came from. `read_frame` sees only the glyph
    /// sets, not the `game` / `taught` marker stored beside them, so the caller that
    /// loaded them stamps it here.
    pub fn stamp_engines(&mut self, exp: &str, level: &str, identity: &str) {
        self.exp.engine = exp.to_string();
        self.level.engine = level.to_string();
        self.identity.engine = identity.to_string();
    }
}

/// A read that produced no sample. The provenance gathered before the failure is
/// kept: a rejected frame is exactly the one a diagnostic has to explain.
#[derive(Clone, Debug)]
pub struct RejectedRead {
    pub reason: String,
    pub provenance: ReadProvenance,
}

impl RejectedRead {
    fn new(
        reason: impl Into<String>,
        mut provenance: ReadProvenance,
        started: std::time::Instant,
    ) -> Self {
        provenance.duration_ms = started.elapsed().as_secs_f64() * 1000.0;
        Self {
            reason: reason.into(),
            provenance,
        }
    }
}

/// A rejection of the EXP read, carrying the reason on the EXP provenance entry.
fn reject_exp(
    provenance: ReadProvenance,
    reason: &str,
    started: std::time::Instant,
) -> RejectedRead {
    let mut provenance = provenance;
    provenance.exp = FieldProvenance::rejected(reason);
    RejectedRead::new(reason, provenance, started)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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
    /// Where each field came from and why an empty one is empty. Defaulted so a
    /// reading written by an older build still parses.
    #[serde(default)]
    pub provenance: ReadProvenance,
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
    /// Write the sample to history (the guard has seen enough good reads).
    verified: bool,
    /// The frame was shown but did not count as a good read (see `tick`).
    warning: Option<String>,
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
    /// The frame parsed, but the guard kept it out of history. The guard has
    /// already updated its own state (a drop it is confirming must survive), so
    /// this is counted as a failed read without resetting the guard.
    Guarded(String),
    Transient(String),
    /// The client size has no usable profile yet. The overlay reports this and
    /// offers a Calibrate button; the window is not opened unasked.
    Uncalibrated(String),
}

struct EngineState {
    capturer: Option<(i64, Capturer)>,
    /// The last frame the capture delivered, and when. Windows Graphics Capture
    /// only delivers a frame when the window changes, so a quiet screen reuses
    /// it; [`STALE_FRAME`] bounds how long.
    last_frame: Option<(Frame, std::time::Instant)>,
    outcomes: VecDeque<bool>,
    /// Decides which readings become history rows; holds the last recorded one.
    guard: Guard,
    published: Option<CaptureStatus>,
    published_profile: Option<Profile>,
    /// The last anchor this engine placed the overlay at, so a game that has not
    /// moved does not cost a `SetWindowPos` every tick.
    anchored: Option<(i32, i32)>,
    /// Where the player dragged the overlay, in physical pixels. While this is set
    /// the anchor leaves the panel alone: a dragged position outranks the computed
    /// one until the player gives up the override.
    restored_position: Option<(i32, i32)>,
    last_seed_attempt: Option<std::time::Instant>,
    identity_seed_attempt: Option<std::time::Instant>,
    /// The EXP, level and name/job atlases decoded from the profile, each with
    /// the fingerprint of the stored set it came from.
    decoded: [Option<(u64, Atlas)>; 3],
}

impl EngineState {
    fn new() -> Self {
        Self {
            capturer: None,
            last_frame: None,
            outcomes: VecDeque::new(),
            guard: Guard::new(),
            published: None,
            published_profile: None,
            decoded: [None, None, None],
            anchored: None,
            restored_position: settings::load_overlay_position(),
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
        self.guard.reset();
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

/// Movement smaller than this is not a drag: the window manager can round a
/// position by a pixel during a resize or a DPI change.
const DRAG_TOLERANCE: i32 = 2;

/// Record a position the player dragged to, and stop anchoring from overriding it.
///
/// This is the one place a drag is noticed. Comparing the window's real position
/// with the one this engine last set catches a drag however it happened — a
/// drag-region drag, a move from the window manager, or a restored session — so
/// nothing depends on the frontend reporting the gesture.
fn adopt_dragged_position<R: Runtime>(app: &AppHandle<R>, engine: &mut EngineState) {
    // A reset makes the panel follow the game window again. Dropping the remembered
    // anchor too means the next `anchor_overlay` places it afresh instead of
    // assuming it is already where it should be.
    if crate::take_position_reset() {
        engine.restored_position = None;
        engine.anchored = None;
        return;
    }

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(actual) = window.outer_position() else {
        return;
    };

    // With no baseline yet — nothing anchored, nothing saved — the window is where
    // Windows put it. Adopt that as the baseline so a later drag is measurable, but
    // do not save it: the player has not moved anything yet, and saving here would
    // pin the panel before the anchor ever got a chance to place it.
    let Some(expected) = engine.anchored else {
        engine.anchored = Some((actual.x, actual.y));
        return;
    };

    if (actual.x - expected.0).abs() <= DRAG_TOLERANCE
        && (actual.y - expected.1).abs() <= DRAG_TOLERANCE
    {
        return;
    }

    let position = (actual.x, actual.y);
    engine.restored_position = Some(position);
    engine.anchored = Some(position);
    // Losing the saved position on a write failure would be worse than a drag that
    // does not survive the next restart, so the failure is logged rather than
    // swallowed.
    if let Err(error) = settings::save_overlay_position(position.0, position.1) {
        crate::calibration::log(&format!("could not save the overlay position: {error}"));
    }
}

fn anchor_overlay<R: Runtime>(
    app: &AppHandle<R>,
    engine: &mut EngineState,
    screen: window_target::Rect,
) {
    // A dragged position wins. The player moved the panel deliberately, so the
    // computed anchor must not pull it back on this tick or the next restart —
    // that was the "position is not remembered" bug.
    if let Some(position) = engine.restored_position {
        if engine.anchored != Some(position) {
            if let Some(window) = app.get_webview_window("main") {
                if window
                    .set_position(tauri::PhysicalPosition::new(position.0, position.1))
                    .is_ok()
                {
                    engine.anchored = Some(position);
                }
            }
        }
        return;
    }

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let overlay = window
        .outer_size()
        .map(|size| (size.width as i32, size.height as i32))
        .unwrap_or((0, 0));
    // The monitor the game is on, found from the client's centre.
    let monitor = window
        .monitor_from_point(
            f64::from(screen.x + screen.w / 2),
            f64::from(screen.y + screen.h / 2),
        )
        .ok()
        .flatten()
        .map(|monitor| window_target::Rect {
            x: monitor.position().x,
            y: monitor.position().y,
            w: monitor.size().width as i32,
            h: monitor.size().height as i32,
        });
    let anchor = overlay_anchor(screen, overlay, monitor);
    if engine.anchored == Some(anchor) {
        return;
    }

    {
        if window
            .set_position(tauri::PhysicalPosition::new(anchor.0, anchor.1))
            .is_ok()
        {
            engine.anchored = Some(anchor);
        }
    }
}

/// Where the overlay's top-left goes: just outside the game client's top-right
/// corner, or inside that corner when the outside spot would put the overlay
/// past the edge of the game's monitor (a game filling its monitor would
/// otherwise push the overlay onto the next display).
fn overlay_anchor(
    client: window_target::Rect,
    overlay: (i32, i32),
    monitor: Option<window_target::Rect>,
) -> (i32, i32) {
    let outside = (client.x + client.w, client.y);
    let Some(monitor) = monitor else {
        return outside;
    };
    if outside.0 + overlay.0 <= monitor.x + monitor.w {
        return outside;
    }
    let x = (client.x + client.w - overlay.0).max(monitor.x);
    (x, client.y.max(monitor.y))
}

pub fn start<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || supervise(app));
}

/// The longest wait before restarting the engine after a panic.
const RESTART_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Run the engine loop, and restart it if it panics.
///
/// A panic (a poisoned lock reaching an `expect`, a bad index in a reader) used to
/// end the thread silently: reading stopped while the overlay kept showing the
/// last status. Now the panic is logged, the overlay says the capture is lost, and the loop starts again from a fresh state, waiting longer
/// after each consecutive failure so a panic on every tick cannot spin.
fn supervise<R: Runtime>(app: AppHandle<R>) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = std::time::Instant::now();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(app.clone())));
        let Err(payload) = outcome else {
            return;
        };
        let reason = format!("reader crashed and restarted: {}", panic_message(&*payload));
        crate::calibration::log(&reason);
        let _ = app.emit("capture-status", CaptureStatus::lost(reason));
        // A run that lasted a while was a one-off; start the backoff again.
        if started.elapsed() > RESTART_BACKOFF_MAX {
            backoff = Duration::from_secs(1);
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(RESTART_BACKOFF_MAX);
    }
}

/// The decoded form of `stored`, reusing the slot's copy while the stored glyphs
/// are unchanged.
fn decoded<'a>(slot: &'a mut Option<(u64, Atlas)>, stored: &StoredAtlas) -> &'a Atlas {
    let fingerprint = stored.fingerprint();
    if slot.as_ref().map(|(seen, _)| *seen) != Some(fingerprint) {
        *slot = Some((fingerprint, Atlas::from_settings(stored)));
    }
    &slot.as_ref().expect("just filled").1
}

/// The fields of the last recorded row the guard needs after a restart.
#[derive(serde::Deserialize)]
struct LastRow {
    lv: u32,
    exp: i64,
    #[serde(default)]
    name: Option<String>,
}

/// The guard's starting point from the last recorded row, when it has a level.
fn restart_seed(record: serde_json::Value) -> Option<guard::Point> {
    let row: LastRow = serde_json::from_value(record).ok()?;
    (row.lv > 0).then_some(guard::Point {
        lv: row.lv,
        exp: row.exp,
        name: row.name,
    })
}

/// The text of a panic payload (`panic!` with a literal or a formatted message).
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn run<R: Runtime>(app: AppHandle<R>) {
    let mut engine = EngineState::new();

    // Carry the dip guard across a restart. The guard compares each reading with
    // the last one recorded, so without this the first reading after a restart has
    // nothing to compare against — which is how a two-EXP blip got into the history.
    if let Some(point) = crate::samples::last().and_then(restart_seed) {
        engine.guard.seed(point);
    }

    loop {
        std::thread::sleep(Duration::from_secs(1));

        let target_state = app.state::<window_target::TargetState>();
        let selector = target_state.selector();
        let target_status = window_target::status_for(&selector);
        window_target::publish_status(&app, &target_state, target_status.clone());

        // Notice a drag before anchoring, so the panel the player just moved is not
        // snapped back on this tick. This runs whether or not a target is attached:
        // the player can drag the panel while the game is closed, and that position
        // should still be remembered.
        adopt_dragged_position(&app, &mut engine);

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

                // A history row that cannot be written is reported at once: the
                // overlay must not say "reading" while nothing is being recorded.
                let mut history_error = None;
                let mut recorded = false;
                if outcome.verified {
                    // An unrecognised level still updates the overlay, but it is kept
                    // out of the history: a row with level 0 has no place on the EXP
                    // curve and would bend it towards the origin.
                    if outcome.sample.lv > 0 {
                        let written = serde_json::to_value(&outcome.sample)
                            .map_err(|error| error.to_string())
                            .and_then(|record| crate::samples::append(&record));
                        match written {
                            Ok(()) => recorded = true,
                            Err(error) => {
                                let reason = format!("history not saved: {error}");
                                log_once(&reason);
                                history_error = Some(reason);
                            }
                        }
                    }
                }
                // Display each successful read immediately; only the history write
                // waits for consecutive good reads. That keeps a map change from
                // waiting three ticks before the overlay shows the new name.
                let _ = app.emit(
                    "live-sample",
                    LiveSample {
                        sample: &outcome.sample,
                        recorded,
                    },
                );
                engine.keep_outcome(outcome.warning.is_none());
                let status = match history_error {
                    Some(reason) => CaptureStatus::degraded(Some(reason)),
                    None => engine.status(outcome.warning),
                };
                publish_capture(&app, &mut engine, status);
            }
            Err(TickError::Lost(reason)) => {
                engine.reset_target();
                // The overlay's red banner is the signal. A desktop toast on every
                // drop to `lost` (closing or minimising the game included) was more
                // noise than help, so none is posted.
                publish_capture(&app, &mut engine, CaptureStatus::lost(reason));
            }
            Err(TickError::Uncalibrated(reason)) => {
                engine.guard.miss();
                // The window is fine — only the calibration is missing — so keep
                // the capturer attached. Resetting here would drop it, and next
                // tick would re-attach with no frame yet and flip to a transient
                // "reading" state, making the panel flicker once a second.
                // The overlay reports this and offers a Calibrate button; the
                // window stays closed until the player asks for it.
                publish_capture(&app, &mut engine, CaptureStatus::uncalibrated(reason));
            }
            Err(TickError::Parse(reason)) => {
                engine.guard.miss();
                engine.keep_outcome(false);
                let status = engine.status(Some(reason));
                publish_capture(&app, &mut engine, status);
            }
            Err(TickError::Guarded(reason)) => {
                engine.keep_outcome(false);
                let status = engine.status(Some(reason));
                publish_capture(&app, &mut engine, status);
            }
            Err(TickError::Transient(reason)) => {
                engine.guard.miss();
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
    let key = profile.key();
    let saved = profile::update_for(&key, |active| active.atlas = stored.clone());
    if let Err(error) = saved {
        crate::calibration::log(&format!("seed from game could not save: {error}"));
        return false;
    }

    // The level box is optional and may not be on screen; seed it when it reads.
    // The Filter Applied tile needs the white-ink key; Ideal reads from luma.
    if let Some(region) = profile.calibration.level.filter(Region::is_usable) {
        match crate::game_font::seed_level_best(frame, region, dir) {
            Some((level_stored, (w, h), text)) => {
                if let Err(error) =
                    profile::update_for(&key, |active| active.level_atlas = level_stored)
                {
                    crate::calibration::log(&format!("seed level could not save: {error}"));
                }
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
) -> Option<(settings::StoredAtlas, u32)> {
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
            engine.last_frame = Some((cropped.clone(), std::time::Instant::now()));
            cropped
        }
        Ok(None) => match &engine.last_frame {
            Some((frame, at)) if at.elapsed() <= STALE_FRAME => frame.clone(),
            Some(_) => {
                return Err(TickError::Transient(format!(
                    "no new frame from the game for {} s",
                    STALE_FRAME.as_secs()
                )))
            }
            None => return Err(TickError::Transient("waiting for the first frame".into())),
        },
        Err(error) => return Err(TickError::Lost(format!("capture failed: {error}"))),
    };

    // Every reader works on the game's 1x picture, whatever the resolution and
    // UI Size: one HUD geometry and one glyph size.
    let frame = crate::native::prepare(frame, settings::ui_mode()).frame;

    // The profile is chosen by the 1x picture's size and UI Size, so a
    // resolution change switches calibration sets instead of invalidating
    // everything.
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
                let saved = profile::update_for(&profile.key(), |active| {
                    active.identity_atlas = stored.clone();
                });
                if let Err(error) = saved {
                    crate::calibration::log(&format!("identity seed could not save: {error}"));
                }
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

    let calibration = profile.calibration.clone();
    let [exp_slot, level_slot, identity_slot] = &mut engine.decoded;
    let atlas = decoded(exp_slot, &profile.atlas);
    let level_atlas = decoded(level_slot, &profile.level_atlas);
    let identity_atlas = decoded(identity_slot, &profile.identity_atlas);
    let mut readings = read_frame_detailed(
        &frame,
        &calibration,
        atlas,
        level_atlas,
        identity_atlas,
        &profile.maps,
        &profile.streets,
        &profile.identities,
    )
    .map_err(|rejected| TickError::Parse(rejected.reason))?;
    readings.provenance.stamp_engines(
        &profile.atlas.source,
        &profile.level_atlas.source,
        &profile.identity_atlas.source,
    );

    // Whether this reading may become history is the guard's call: levels only
    // go up, EXP only goes up within a level except at a confirmed death, and rows
    // are written only after consecutive good reads (see `guard`). A truncated
    // 21,476 read as level 19, and a two-EXP blip, are what it was written against.
    // A level that does not cross-check against the EXP table comes back with EXP
    // and level blanked (the only time a parsed reading carries an EXP rejection).
    // Map, name and job are still worth showing, but the frame must count as a
    // failed read so the overlay goes degraded instead of saying "reading" while
    // nothing is recorded.
    let warning = readings.provenance.exp.reason.clone();

    let point = guard::Point {
        lv: readings.lv,
        exp: readings.exp,
        name: readings.name.clone(),
    };
    let (state, verified, death) =
        match engine.guard.judge(&point, exp_table::required(readings.lv)) {
            guard::Verdict::Accept {
                state,
                verified,
                death,
            } => (state, verified, death),
            guard::Verdict::Hold(reason) => {
                log_once(&format!("guard: {reason}"));
                (guard::State::Idle, false, false)
            }
            guard::Verdict::Reject(reason) => return Err(TickError::Guarded(reason)),
        };

    Ok(TickOutcome {
        sample: Sample {
            t: chrono::Utc::now().timestamp(),
            lv: readings.lv,
            exp: readings.exp,
            pct: readings.pct,
            map: readings.map,
            state,
            conf: readings.conf,
            map_hidden: readings.map_hidden,
            name: readings.name.clone(),
            event: death.then(|| "death".to_string()),
        },
        profile: Profile {
            name: readings.name.clone(),
            job: readings.job.clone(),
        },
        verified,
        warning,
    })
}

/// Read one frame, keeping only the rejection reason as a plain string.
///
/// Every runtime caller now wants the provenance beside the readings — the
/// calibration probe and the saved diagnostic included — so the plain wrapper is
/// used only by the regression harness and the capture tests.
#[cfg(test)]
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
    read_frame_detailed(
        frame,
        calibration,
        atlas,
        level_atlas,
        identity_atlas,
        maps,
        streets,
        identities,
    )
    .map_err(|rejected| rejected.reason)
}

/// Read one frame, keeping the reason and the partial provenance when it is
/// rejected. Diagnostics need the second half: a frame that produced no sample is
/// the one worth explaining.
#[allow(clippy::too_many_arguments)]
pub(crate) fn read_frame_detailed(
    frame: &Frame,
    calibration: &Calibration,
    atlas: &Atlas,
    level_atlas: &Atlas,
    identity_atlas: &Atlas,
    maps: &[Template],
    streets: &[Template],
    identities: &Identities,
) -> Result<Readings, Box<RejectedRead>> {
    let started = std::time::Instant::now();
    let mut provenance = ReadProvenance::default();

    let Some(exp_region) = calibration.exp.filter(Region::is_usable) else {
        return Err(Box::new(reject_exp(
            provenance,
            "the EXP box is not set",
            started,
        )));
    };

    let Some(exp_gray) = ocr::crop_gray(frame, exp_region) else {
        return Err(Box::new(reject_exp(
            provenance,
            "the EXP box is outside the window",
            started,
        )));
    };
    let (exp_text, exp_conf) = match atlas.recognize_readout(&exp_gray) {
        Ok(read) => read,
        Err(reason) => return Err(Box::new(reject_exp(provenance, &reason, started))),
    };
    let Some((exp, pct)) = ocr::parse_exp_text(&exp_text) else {
        let reason = format!("could not parse EXP from '{exp_text}'");
        return Err(Box::new(reject_exp(provenance, &reason, started)));
    };
    provenance.exp = FieldProvenance::read(ReadSource::Atlas);

    // The level is read from its own box and never derived. A derived level is only
    // as right as the EXP table and, when it is wrong, puts a plausible-looking but
    // false number on the overlay — which is what "dont derive the level digits"
    // was about. An unreadable level comes back as 0 and is reported as unrecognised.
    // The level has its own atlas: the digits are drawn at a different size from the
    // EXP readout, and sharing one atlas let the two readers compete for characters.
    let level = level_reading(frame, calibration, level_atlas);
    provenance.level = match &level.reason {
        Some(reason) => FieldProvenance::rejected(reason),
        None => FieldProvenance::read(ReadSource::Atlas),
    };
    let (lv, level_text, level_conf) = (level.lv, level.text, level.confidence);

    // The percentage check exists to catch a bad EXP read, but EXP and its
    // percentage come from one read of one box, so they cannot disagree with each
    // other — only with the level, which is a different box and a different atlas.
    // A mismatch means one of the two boxes is wrong. The percentage is
    // redundant, so a small gap is repaired from the EXP and level (the Filtered
    // `23.33` can read as `23.83`); a large gap invalidates both the EXP and the
    // level while map/name/job are kept.
    if lv > 0 && exp_table::required(lv).is_none() {
        let reason = format!("level {lv} is outside the EXP table");
        provenance.level = FieldProvenance::rejected(&reason);
        return Err(Box::new(RejectedRead::new(reason, provenance, started)));
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
            let reason = format!("level {lv} does not fit {pct:.2}% for {exp} EXP");
            provenance.exp = FieldProvenance::rejected(&reason);
            provenance.level = FieldProvenance::rejected(&reason);
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
    let map_read = match_map_and_street(frame, calibration, maps, streets, minimap);
    provenance.map = map_provenance(&map_read);
    let map = match &map_read.street {
        Some(street) if map_read.name != "unknown" => format!("{street}\n{}", map_read.name),
        _ => map_read.name.clone(),
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
    // The atlas the read actually had: the profile's merged set, or the per-line
    // seed, which is used only when the profile has none. Reporting on the profile
    // atlas alone said "nothing to read with" for a frame that did seed letters and
    // simply failed to read them.
    let seeded_glyphs = name_atlas
        .iter()
        .chain(job_atlas.iter())
        .any(|atlas| !atlas.glyphs.is_empty());
    let has_glyphs = !identity_atlas.glyphs.is_empty() || seeded_glyphs;
    let has_fingerprint = identities.name.is_some() || identities.job.is_some();
    provenance.identity =
        identity_provenance(&name, &job, calibration, has_glyphs, has_fingerprint);

    provenance.duration_ms = started.elapsed().as_secs_f64() * 1000.0;
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
        name: name.text,
        job: job.text,
        map_hidden: map_read.hidden,
        provenance,
    })
}

/// The level for this frame, read from its own box. Deliberately never derived from
/// EXP and the percentage: the derivation leans on the EXP table, and when it is
/// wrong it reports a level that looks reasonable and is not.
///
/// An unreadable level yields 0 with a reason, which every caller treats as "not
/// recognised" — the same way an unrecognised name is reported. Failing the whole
/// frame instead would hide a perfectly good EXP reading behind a missing box.
/// Write a diagnostic to data/debug.log, but only when it differs from the last
/// one. The level is read every tick, so logging unconditionally would bury the
/// log under thousands of identical lines.
pub(crate) fn log_once(message: &str) {
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

/// One level read: the digits, their confidence, and why they are missing when
/// they are.
struct LevelRead {
    lv: u32,
    text: String,
    confidence: f64,
    /// Why the level is 0, when it is: no box, no glyphs, or a read that did not
    /// parse. `None` when a level was read.
    reason: Option<String>,
}

fn level_reading(frame: &Frame, calibration: &Calibration, atlas: &Atlas) -> LevelRead {
    let unknown = |reason: &str| LevelRead {
        lv: 0,
        text: String::new(),
        confidence: 0.0,
        reason: Some(reason.to_string()),
    };

    let Some(region) = calibration.level.filter(Region::is_usable) else {
        log_once("level_reading: no level box set");
        return unknown("no level box is set");
    };
    if atlas.glyphs.is_empty() {
        log_once("level_reading: no level glyphs learned yet");
        return unknown("no level glyphs learned yet");
    }

    // Read both extractions: plain luma for Ideal, the white key for the
    // Filter Applied tile whose coloured edge merges with a digit. The longer
    // valid read wins, then confidence, so a frame that already reads is not
    // changed by the weaker extraction.
    let normal = ocr::crop_gray(frame, region);
    let keyed = ocr::crop_white_key(frame, region);
    type LevelAttempt<'a> = (&'a str, Result<(String, f64), String>);
    let mut readings: Vec<LevelAttempt> = Vec::new();
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
        Some((lv, text, confidence)) => LevelRead {
            lv,
            text,
            confidence,
            reason: None,
        },
        None => {
            if errors.is_empty() {
                errors.push("the box is outside the window".into());
            }
            let (w, h) = normal.as_ref().map_or((0, 0), |gray| (gray.w, gray.h));
            let reason = errors.join("; ");
            log_once(&format!(
                "level_reading: box {region:?} crop {w}x{h} failed: {reason}"
            ));
            unknown(&reason)
        }
    }
}

/// One identity line: the text the reader settled on, and the stage that produced
/// it. The two lines are reported as one provenance entry, because the calibration
/// window shows them as one readout.
struct IdentityRead {
    text: Option<String>,
    source: ReadSource,
}

/// The map's provenance entry: the stage that named the map, or why nothing did.
///
/// The reason comes from the stage that gave up, so an `unknown` map says whether
/// the label could not be cropped, the client table was not installed, or the
/// label simply matched nothing — the same as EXP and level already report.
fn map_provenance(read: &MapRead) -> FieldProvenance {
    if let Some(reason) = &read.reason {
        return FieldProvenance::unread(read.source, reason.clone());
    }
    FieldProvenance::read(read.source)
}

/// Which stage answered for the name/job readout, and why neither line read when
/// they did not.
fn identity_provenance(
    name: &IdentityRead,
    job: &IdentityRead,
    calibration: &Calibration,
    has_glyphs: bool,
    has_fingerprint: bool,
) -> FieldProvenance {
    let source = match (name.source, job.source) {
        (ReadSource::Font, _) | (_, ReadSource::Font) => ReadSource::Font,
        (ReadSource::Fingerprint, _) | (_, ReadSource::Fingerprint) => ReadSource::Fingerprint,
        _ => ReadSource::None,
    };
    if source != ReadSource::None {
        return FieldProvenance::read(source);
    }
    // Each box is tested on its own: `Option::or` only falls through when `name` is
    // unset, so a name box that is drawn but too small would hide a usable job box
    // and blame the player for a box that is set.
    let has_box = calibration
        .name
        .filter(Region::is_usable)
        .or(calibration.job.filter(Region::is_usable))
        .is_some();
    let reason = if !has_box {
        "no name/job box is set"
    } else if !has_glyphs && !has_fingerprint {
        "no name/job letters or fingerprint to read with"
    } else {
        // Letters were available — the profile's atlas or the per-line seed — and a
        // taught fingerprint too when one exists, so the lines simply did not read.
        "the name and job lines did not read"
    };
    FieldProvenance::rejected(reason)
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

/// Finish a *read* identity line, or return `None` when it cannot be shown.
///
/// A name is open-ended, so it is displayed as transcribed. A job is a closed
/// vocabulary: text that resolves to no known job is not a job, and saying so is
/// the point of the vocabulary — otherwise a confident-looking misread would be
/// displayed and never flagged. Only the readers' own text goes through here; a
/// taught fingerprint is exact and is returned without this gate.
fn finish_identity(text: &str, kind: IdentityKind) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match kind {
        IdentityKind::Name => {
            let text = crate::font_seed::fix_identity_case(text);
            (!text.is_empty()).then_some(text)
        }
        // Close to a real job snaps to it (repairing the documented `Page` → `age`
        // drop); anything else is reported as unrecognised rather than displayed.
        IdentityKind::Job => crate::jobs::resolve(text),
    }
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
) -> IdentityRead {
    let read = |text: String, source: ReadSource| IdentityRead {
        text: Some(text),
        source,
    };
    let unread = IdentityRead {
        text: None,
        source: ReadSource::None,
    };

    let Some(region) = region.filter(Region::is_usable) else {
        return unread;
    };

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
        if let Some(text) = finish_identity(&glyph_text, kind) {
            return read(text, ReadSource::Font);
        }
    }

    let ocr = crate::winocr::identity_candidates(frame, region, kind == IdentityKind::Job);
    if let Some(text) = choose_identity_text(&glyph_text, &guesses, known_ratio, confidence, &ocr) {
        if let Some(text) = finish_identity(&text, kind) {
            return read(text, ReadSource::Font);
        }
    }

    if !glyph_text.is_empty() && confidence >= 0.55 && known_ratio >= 0.5 {
        if let Some(text) = finish_identity(&glyph_text, kind) {
            return read(text, ReadSource::Font);
        }
    }

    // A taught fingerprint is the player's own labelling, so it is returned as
    // taught: the job vocabulary exists to police the readers' guesses, not to
    // overrule text the player entered deliberately.
    match template.and_then(|template| match_template(frame, region, template)) {
        Some(text) if !text.trim().is_empty() => read(text, ReadSource::Fingerprint),
        _ => unread,
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

    let identities = profile::read(|p| &p.identities);
    Profile {
        name: identities.name.map(|template| template.label),
        job: identities.job.map(|template| template.label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> window_target::Rect {
        window_target::Rect { x, y, w, h }
    }

    #[test]
    fn the_overlay_sits_outside_the_game_when_the_monitor_has_room() {
        let monitor = Some(rect(0, 0, 2560, 1440));
        let client = rect(171, 111, 1920, 1080);
        assert_eq!(overlay_anchor(client, (300, 400), monitor), (2091, 111));
    }

    #[test]
    fn the_overlay_moves_inside_a_game_that_fills_its_monitor() {
        let monitor = Some(rect(0, 0, 2560, 1440));
        let client = rect(0, 0, 2560, 1440);
        assert_eq!(overlay_anchor(client, (300, 400), monitor), (2260, 0));
        // On a second monitor to the left of the primary one.
        let left = Some(rect(-1920, 0, 1920, 1080));
        assert_eq!(
            overlay_anchor(rect(-1920, 0, 1920, 1080), (300, 400), left),
            (-300, 0)
        );
    }

    #[test]
    fn without_a_monitor_the_overlay_stays_outside() {
        assert_eq!(
            overlay_anchor(rect(0, 0, 800, 600), (300, 400), None),
            (800, 0)
        );
    }

    fn blank_frame() -> Frame {
        Frame {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        }
    }

    fn empty_atlas() -> Atlas {
        Atlas { glyphs: Vec::new() }
    }

    #[test]
    fn read_sources_serialize_to_their_documented_ids() {
        // The ids are the diagnostic format. A change here is a format change, so
        // the set is pinned deliberately.
        for (source, id) in [
            (ReadSource::Atlas, "atlas"),
            (ReadSource::Font, "font"),
            (ReadSource::Fingerprint, "fingerprint"),
            (ReadSource::Ocr, "ocr"),
            (ReadSource::Table, "table"),
            (ReadSource::Dtw, "dtw"),
            (ReadSource::Literal, "literal"),
            (ReadSource::Glyphs, "glyphs"),
            (ReadSource::Hidden, "hidden"),
            (ReadSource::Unknown, "unknown"),
            (ReadSource::None, "none"),
        ] {
            assert_eq!(
                serde_json::to_value(source).expect("serialize a source"),
                serde_json::Value::String(id.into()),
                "{source:?} must serialise as {id:?}"
            );
            // A support file from this build has to read back in the same build.
            assert_eq!(
                serde_json::from_value::<ReadSource>(serde_json::Value::String(id.into()))
                    .expect("deserialize a source"),
                source
            );
        }
    }

    #[test]
    fn readings_provenance_defaults_when_a_reading_has_none() {
        // A row written before provenance existed still parses, and reports "not
        // known" rather than inventing a source.
        let legacy = serde_json::json!({
            "lv": 36,
            "exp": 6818,
            "pct": 18.90,
            "expectedPct": 18.90,
            "map": "Kerning City",
            "conf": 0.98,
            "expText": "6,818[18.90%]",
            "levelText": "36",
            "name": "Hero",
            "job": "Page",
            "mapHidden": false
        });
        let readings: Readings = serde_json::from_value(legacy).expect("a legacy reading");
        assert_eq!(readings.provenance, ReadProvenance::default());
        assert_eq!(readings.provenance.exp.source, ReadSource::Unknown);
        assert_eq!(readings.provenance.map.source, ReadSource::Unknown);
        assert_eq!(readings.provenance.map.reason, None);
        assert_eq!(readings.provenance.duration_ms, 0.0);

        // And the current shape round-trips through the same wire format.
        let value = serde_json::to_value(&readings).expect("serialize a reading");
        assert_eq!(value["provenance"]["exp"]["source"], "unknown");
        assert_eq!(value["provenance"]["exp"]["engine"], "");
        assert_eq!(value["provenance"]["durationMs"], 0.0);
        // An empty reason is left out instead of serialising as null.
        assert!(value["provenance"]["exp"].get("reason").is_none());
    }

    #[test]
    fn provenance_engines_are_stamped_from_the_stored_atlases() {
        let mut provenance = ReadProvenance::default();
        provenance.stamp_engines("game", "taught", "font");
        assert_eq!(provenance.exp.engine, "game");
        assert_eq!(provenance.level.engine, "taught");
        assert_eq!(provenance.identity.engine, "font");
        // The map has no stored atlas of its own; the client table is read live.
        assert_eq!(provenance.map.engine, "");
    }

    #[test]
    fn map_provenance_carries_the_source_and_the_reason() {
        // A collapsed minimap is not a miss: there is nothing on screen to read.
        let hidden = map_provenance(&MapRead {
            name: "unknown".into(),
            street: None,
            hidden: true,
            source: ReadSource::Hidden,
            reason: Some("the minimap is collapsed — there is no map name on screen".into()),
        });
        assert_eq!(hidden.source, ReadSource::Hidden);
        assert_eq!(
            hidden.reason.as_deref(),
            Some("the minimap is collapsed — there is no map name on screen")
        );

        // An unresolved map names the stage that gave up, instead of one sentence
        // that covers a missing crop, a missing client table and a non-match alike.
        let folded = map_provenance(&MapRead::unknown(
            "the folded map label did not match the client table",
        ));
        assert_eq!(folded.source, ReadSource::Unknown);
        assert_eq!(
            folded.reason.as_deref(),
            Some("the folded map label did not match the client table")
        );

        // A named map is not a rejection, so it carries no reason.
        for source in [
            ReadSource::Ocr,
            ReadSource::Table,
            ReadSource::Dtw,
            ReadSource::Literal,
            ReadSource::Glyphs,
        ] {
            let entry = map_provenance(&MapRead {
                name: "Perion".into(),
                street: None,
                hidden: false,
                source,
                reason: None,
            });
            assert_eq!(entry.source, source);
            assert_eq!(entry.reason, None);
        }
    }

    #[test]
    fn identity_provenance_blames_the_right_missing_piece() {
        let unread = || IdentityRead {
            text: None,
            source: ReadSource::None,
        };
        let tiny = Region {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        };
        let real = Region {
            x: 0,
            y: 0,
            w: 40,
            h: 12,
        };

        // A name box that is drawn but too small must not hide a usable job box.
        let calibration = Calibration {
            name: Some(tiny),
            job: Some(real),
            ..Calibration::default()
        };
        let letters_but_no_read =
            identity_provenance(&unread(), &unread(), &calibration, true, false);
        assert_eq!(letters_but_no_read.source, ReadSource::None);
        assert_eq!(
            letters_but_no_read.reason.as_deref(),
            Some("the name and job lines did not read"),
            "a set job box must not be reported as missing"
        );

        // The per-line seed counted as letters, so "nothing to read with" is wrong.
        let nothing_at_all = identity_provenance(&unread(), &unread(), &calibration, false, false);
        assert_eq!(
            nothing_at_all.reason.as_deref(),
            Some("no name/job letters or fingerprint to read with")
        );

        // A taught fingerprint alone is still something to read with.
        let fingerprint_only = identity_provenance(&unread(), &unread(), &calibration, false, true);
        assert_eq!(
            fingerprint_only.reason.as_deref(),
            Some("the name and job lines did not read")
        );

        // Neither box is set at all.
        let no_box = identity_provenance(&unread(), &unread(), &Calibration::default(), true, true);
        assert_eq!(no_box.reason.as_deref(), Some("no name/job box is set"));
    }

    #[test]
    fn the_job_gate_accepts_a_real_job_and_a_close_miss() {
        // A real job passes through, canonicalised.
        assert_eq!(
            finish_identity("Page", IdentityKind::Job).as_deref(),
            Some("Page")
        );
        // The documented Default rendering drops the leading `P`.
        assert_eq!(
            finish_identity("age", IdentityKind::Job).as_deref(),
            Some("Page")
        );
        // Punctuation and spacing are the renderer's, not the vocabulary's.
        assert_eq!(
            finish_identity("Wizard (Ice, Lightning)", IdentityKind::Job).as_deref(),
            Some("Wizard (Ice,Lightning)")
        );
    }

    #[test]
    fn the_job_gate_rejects_text_that_is_not_a_job() {
        // This is the whole point of the vocabulary: a confident-looking misread
        // must be reported as unrecognised, not displayed as a class.
        assert_eq!(finish_identity("Zqx", IdentityKind::Job), None);
        assert_eq!(finish_identity("Euroblood", IdentityKind::Job), None);
        assert_eq!(finish_identity("", IdentityKind::Job), None);
        assert_eq!(finish_identity("   ", IdentityKind::Job), None);
        // A game this build does not have must not resolve to one it does.
        assert_eq!(finish_identity("Pirate", IdentityKind::Job), None);
        assert_eq!(finish_identity("Corsair", IdentityKind::Job), None);
    }

    #[test]
    fn a_two_edit_near_miss_still_snaps_by_design() {
        // `Pqe` is two edits from `Page`, which is inside the tolerance that exists
        // to repair exactly this kind of damaged rendering. Snapping it to `Page` is
        // the correction; the tolerance is deliberately not tightened to zero.
        assert_eq!(
            finish_identity("Pqe", IdentityKind::Job).as_deref(),
            Some("Page")
        );
        // Beyond the tolerance it stops being a repair and becomes a guess.
        assert_eq!(finish_identity("Pqxv", IdentityKind::Job), None);
    }

    #[test]
    fn the_name_gate_still_accepts_an_open_ended_name() {
        // Names are not a closed set, so any non-empty text is a valid name.
        assert_eq!(
            finish_identity("Euroblood", IdentityKind::Name).as_deref(),
            Some("Euroblood")
        );
        assert_eq!(
            finish_identity("xXSlayerXx", IdentityKind::Name).as_deref(),
            Some("xXSlayerXx")
        );
        assert_eq!(finish_identity("", IdentityKind::Name), None);
    }

    #[test]
    fn a_rejected_job_is_reported_as_unrecognised_not_displayed() {
        // End to end through the reader: the gate returns None, so the published
        // profile carries no job, which is what the overlay flags as unrecognised.
        let calibration = Calibration {
            job: Some(Region {
                x: 0,
                y: 0,
                w: 40,
                h: 12,
            }),
            ..Calibration::default()
        };
        let read = read_identity(
            &blank_frame(),
            calibration.job,
            &empty_atlas(),
            None,
            IdentityKind::Job,
        );
        assert_eq!(read.text, None, "a blank crop must not become a job");
        assert_eq!(read.source, ReadSource::None);
    }

    #[test]
    fn a_collapsed_minimap_reports_the_hidden_source() {
        let minimap = ocr::Minimap {
            state: ocr::MinimapState::TitleBarOnly,
            bar: Region {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
            badge: Region {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
        };
        let read = match_map_and_street(
            &blank_frame(),
            &Calibration::default(),
            &[],
            &[],
            Some(minimap),
        );
        assert_eq!(read.name, "unknown");
        assert!(read.hidden);
        assert_eq!(read.source, ReadSource::Hidden);
    }

    #[test]
    fn an_unreadable_level_reports_its_reason() {
        // No level box: the level is unrecognised, and the reason says why instead
        // of leaving a bare 0 in the readout.
        let read = level_reading(&blank_frame(), &Calibration::default(), &empty_atlas());
        assert_eq!(read.lv, 0);
        assert_eq!(read.reason.as_deref(), Some("no level box is set"));

        // A level box with no glyphs learned is a different reason.
        let calibration = Calibration {
            level: Some(Region {
                x: 0,
                y: 0,
                w: 20,
                h: 20,
            }),
            ..Calibration::default()
        };
        let read = level_reading(&blank_frame(), &calibration, &empty_atlas());
        assert_eq!(read.lv, 0);
        assert_eq!(
            read.reason.as_deref(),
            Some("no level glyphs learned yet"),
            "a box with no atlas must not blame the box"
        );
    }

    #[test]
    fn a_rejected_frame_still_explains_which_field_failed() {
        let rejected = read_frame_detailed(
            &blank_frame(),
            &Calibration::default(),
            &empty_atlas(),
            &empty_atlas(),
            &empty_atlas(),
            &[],
            &[],
            &Identities::default(),
        )
        .expect_err("an empty calibration cannot read");

        assert_eq!(rejected.reason, "the EXP box is not set");
        assert_eq!(rejected.provenance.exp.source, ReadSource::None);
        assert_eq!(
            rejected.provenance.exp.reason.as_deref(),
            Some("the EXP box is not set")
        );
        // The fields the read never reached stay unknown rather than claimed.
        assert_eq!(rejected.provenance.level.source, ReadSource::Unknown);
        assert_eq!(rejected.provenance.map.source, ReadSource::Unknown);
        assert_eq!(rejected.provenance.identity.source, ReadSource::Unknown);
        assert!(rejected.provenance.duration_ms >= 0.0);

        // The plain reader keeps its old contract: the reason as a string.
        assert_eq!(
            read_frame(
                &blank_frame(),
                &Calibration::default(),
                &empty_atlas(),
                &empty_atlas(),
                &empty_atlas(),
                &[],
                &[],
                &Identities::default(),
            ),
            Err("the EXP box is not set".to_string())
        );
    }

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
        let read = resolve_label_text(
            std::path::Path::new("."),
            &entries,
            Some("Warning Street"),
            "Where the Balrogs Live",
            None,
        );
        assert_eq!(read.street.as_deref(), Some("Warning Street"));
        assert_eq!(read.map.as_deref(), Some("Where the Balrogs Live"));
        assert!(!read.exact);
        // Kept as the on-screen text: no table entry matched it.
        assert_eq!(read.map_source, ReadSource::Literal);
    }

    #[test]
    #[ignore = "requires MSCW: live Tree That Grew I minimap label"]
    fn reads_live_tree_that_grew_one() {
        let frame = crate::corpus::fixture_frame("1920x1080@filtered", "map-tree-1");

        let read = match_label_ocr(&frame).expect("map label");
        assert_eq!(read.street.as_deref(), Some("Victoria Road"));
        assert_eq!(read.map.as_deref(), Some("The Tree That Grew I"));
        assert!(read.exact);
        assert_eq!(read.map_source, ReadSource::Ocr);

        // The whole-line DTW used to pick the `III` sibling; an exact OCR match
        // must win before it runs.
        assert_eq!(
            match_map(&frame, &Calibration::default(), &[]),
            ("The Tree That Grew I".into(), ReadSource::Ocr)
        );
    }

    #[test]
    #[ignore = "requires MSCW: Balrog map literal against the live client table"]
    fn balrog_map_is_literal_against_the_live_table() {
        let Some(dir) = settings::mscw_dir() else {
            return;
        };
        let entries = crate::map_names::load(std::path::Path::new(&dir)).expect("map table");
        let read = resolve_label_text(
            std::path::Path::new(&dir),
            &entries,
            Some("Warning Street"),
            "Where the Balrogs Live",
            None,
        );
        assert_eq!(read.street.as_deref(), Some("Warning Street"));
        assert_eq!(read.map.as_deref(), Some("Where the Balrogs Live"));
        assert!(!read.exact);
        assert_eq!(read.map_source, ReadSource::Literal);
    }

    #[test]
    fn known_map_label_snaps_to_the_client_table() {
        let entries = vec![crate::map_names::MapEntry {
            name: "Deep Valley I".into(),
            street: "Warning Street".into(),
        }];
        let read = resolve_label_text(
            std::path::Path::new("."),
            &entries,
            Some("Warning Street"),
            "Deep Vallev I",
            None,
        );
        assert_eq!(read.street.as_deref(), Some("Warning Street"));
        assert_eq!(read.map.as_deref(), Some("Deep Valley I"));
        assert!(!read.exact);
        assert_eq!(read.map_source, ReadSource::Ocr);
    }

    #[test]
    fn one_line_map_label_has_no_street() {
        let read = resolve_label_text(
            std::path::Path::new("."),
            &[],
            None,
            "Where the Balrogs Live",
            None,
        );
        assert!(read.street.is_none());
        assert_eq!(read.map.as_deref(), Some("Where the Balrogs Live"));
        assert!(!read.exact);
        assert_eq!(read.map_source, ReadSource::Literal);
    }

    #[test]
    fn repaired_pct_is_reflected_in_the_display_text() {
        assert_eq!(replace_pct_text("45,458[23.83%]", 23.333), "45,458[23.33%]");
        assert_eq!(replace_pct_text("45,458[23.83%", 23.333), "45,458[23.33%");
        assert_eq!(replace_pct_text("45,458", 23.333), "45,458");
    }

    #[test]
    fn roman_suffix_is_reranked_by_rendered_width() {
        fn gray(width: usize) -> crate::ocr::Gray {
            crate::ocr::Gray {
                w: width,
                h: 10,
                px: vec![0; width * 10],
            }
        }
        let index = crate::font_seed::LabelIndex {
            map_names: vec![
                ("The Tree That Grew I".into(), gray(165)),
                ("The Tree That Grew II".into(), gray(169)),
                ("The Tree That Grew III".into(), gray(173)),
            ],
            streets: Vec::new(),
            folded_lines: Vec::new(),
        };
        assert_eq!(
            resolve_roman_sibling("The Tree That Grew III", &gray(166), &index),
            "The Tree That Grew I"
        );
        assert_eq!(
            resolve_roman_sibling("The Tree That Grew I", &gray(173), &index),
            "The Tree That Grew III"
        );
        assert_eq!(
            resolve_roman_sibling("The Tree That Grew III", &gray(170), &index),
            "The Tree That Grew II"
        );
    }

    #[test]
    fn roman_trailing_bars_are_counted_as_runs() {
        fn line(bars: &[usize]) -> crate::ocr::Gray {
            let width = 24;
            let height = 10;
            let mut px = vec![0u8; width * height];
            for x in 0..10 {
                for y in 0..height {
                    px[y * width + x] = 255;
                }
            }
            for &x in bars {
                for y in 0..height {
                    px[y * width + x] = 255;
                }
            }
            crate::ocr::Gray {
                w: width,
                h: height,
                px,
            }
        }
        assert_eq!(trailing_bar_count(&line(&[13])), Some(1));
        assert_eq!(trailing_bar_count(&line(&[13, 15])), Some(2));
        assert_eq!(trailing_bar_count(&line(&[13, 15, 17])), Some(3));
    }

    #[test]
    #[ignore = "requires MSCW: Tree That Grew Roman-numeral DTW fallback"]
    fn reads_tree_roman_from_table_dtw() {
        let frame = crate::corpus::fixture_frame("1920x1080@filtered", "map-tree-1");
        assert_eq!(
            match_map_name(&frame).as_deref(),
            Some("The Tree That Grew I")
        );
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
        let frame = crate::corpus::fixture_frame("1920x1080@filtered", "map-deep-valley");
        let read = match_label_ocr(&frame).expect("map label");
        assert_eq!(read.street.as_deref(), Some("Warning Street"));
        assert_eq!(read.map.as_deref(), Some("Deep Valley I"));
        assert_eq!(read.map_source, ReadSource::Ocr);
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

        let streets = profile::read(|p| &p.streets);
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
    fn a_panic_payload_becomes_its_message() {
        let literal = std::panic::catch_unwind(|| panic!("poisoned")).unwrap_err();
        assert_eq!(panic_message(&*literal), "poisoned");
        let formatted =
            std::panic::catch_unwind(|| panic!("index {} out of range", 7)).unwrap_err();
        assert_eq!(panic_message(&*formatted), "index 7 out of range");
    }

    #[test]
    fn a_restart_seeds_from_the_last_row_with_a_level() {
        let seed = restart_seed(
            serde_json::json!({"t": 1, "lv": 35, "exp": 21_478, "name": "Euroblood", "pct": 1.0}),
        )
        .expect("seed");
        assert_eq!(
            (seed.lv, seed.exp, seed.name.as_deref()),
            (35, 21_478, Some("Euroblood"))
        );
        assert!(restart_seed(serde_json::json!({"t": 1, "lv": 0, "exp": 0})).is_none());
        assert!(
            restart_seed(serde_json::json!({"t": 1, "lv": 35})).is_none(),
            "no exp, no seed"
        );
    }
}
