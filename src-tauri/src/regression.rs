//! The client-dependent regression tier: every frame case in the corpus read
//! the way a fresh install reads it, from atlases seeded from the frame itself,
//! plus the accuracy report and the live probes.
//!
//! The corpus, its loader and the hermetic checks are in `corpus.rs`. These
//! tests are `#[ignore]`d: seeding glyphs and labels needs the installed client
//! (`MSCW`) and the Windows Arial face. `accuracy_report` scores every case
//! **exact / rejected / wrong** against its scene's truth, and prints the frozen
//! hold-out's verdict without asserting it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::corpus::{compose_frame, fixtures_dir, load_cases, load_cases_in, Case};
use crate::ocr::{self, Atlas};
use crate::settings::Region;

// ---------------------------------------------------------------------------
// Client-dependent tier: the whole reader, from empty atlases.
// ---------------------------------------------------------------------------
/// The installed client, looking only at the environment and the configured
/// setting — never writing either.
fn mscw_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("MSCOUT_MSCW_DIR")
            .ok()
            .or_else(crate::settings::mscw_dir)
            .unwrap_or_else(|| {
                panic!(
                    "no MSCW install: set MSCOUT_MSCW_DIR or pick the folder in the app \
                     (data/settings.json \"mscwDir\")"
                )
            }),
    )
}

/// Read one fixture the way a fresh install does: seed every atlas from the
/// frame itself (never from a saved profile) and run the production reader.
///
/// `Ok(None)` means the pair has no shipped preset, so there is nothing to read.
/// `Err` is the reader (or its seeding) refusing to produce a reading — the
/// *rejected* class in `accuracy_report`.
fn attempt_read(case: &Case, mscw: &Path) -> Result<Option<crate::engine::Readings>, String> {
    let (client_w, client_h) = case.client_size();
    let mode = case.mode();
    let key = format!("{client_w}x{client_h}{}", mode.suffix());

    let Some(preset) = crate::preset::for_size(client_w, client_h, mode) else {
        println!(
            "{}: no shipped preset for {key} — unsupported pair, skipping",
            case.key
        );
        return Ok(None);
    };

    let frame = compose_frame(case);

    let exp_region = preset
        .calibration
        .exp
        .filter(Region::is_usable)
        .ok_or_else(|| format!("preset {key} has no EXP box"))?;
    let exp_gray = ocr::crop_gray(&frame, exp_region)
        .ok_or_else(|| format!("EXP box {exp_region:?} is outside the frame"))?;
    let (exp, exp_height) = crate::game_font::seed_exp_atlas(&exp_gray, mscw)
        .map_err(|error| format!("seeding the EXP atlas failed: {error}"))?;

    let level_region = preset
        .calibration
        .level
        .filter(Region::is_usable)
        .ok_or_else(|| format!("preset {key} has no level box"))?;
    let (level, level_size, level_text) =
        crate::game_font::seed_level_best(&frame, level_region, mscw)
            .ok_or_else(|| "seeding the level atlas failed".to_string())?;
    println!(
        "{}: level seeded {:?} from {level_text:?}",
        case.key, level_size
    );

    // Name and job use the same Arial face, but the Default rendering is
    // anti-aliased and each crop exposes different glyphs; seed both and merge so
    // one atlas reads both lines.
    let mut identity: Option<crate::ocr::Atlas> = None;
    for region in [preset.calibration.job, preset.calibration.name] {
        let Some(region) = region.filter(Region::is_usable) else {
            continue;
        };
        let Some(gray) = ocr::crop_gray(&frame, region) else {
            continue;
        };
        if let Some((stored, px)) = crate::font_seed::seed_identity(&gray) {
            println!("{}: identity seeded from {region:?} at {px}px", case.key);
            let atlas = crate::ocr::Atlas::from_settings(&stored);
            match &mut identity {
                Some(combined) => combined.glyphs.extend(atlas.glyphs),
                None => identity = Some(atlas),
            }
        }
    }
    let identity = identity
        .ok_or_else(|| "could not seed the name/job atlas from any identity box".to_string())?
        .to_settings();

    println!(
        "{}: preset {key}, EXP atlas {} glyphs ({}px), level atlas {} glyphs ({}x{}), identity {} glyphs",
        case.key,
        exp.glyphs.len(),
        exp_height,
        level.glyphs.len(),
        level_size.0,
        level_size.1,
        identity.glyphs.len()
    );

    let exp_atlas = Atlas::from_settings(&exp);
    let level_atlas = Atlas::from_settings(&level);
    let identity_atlas = Atlas::from_settings(&identity);

    let readings = crate::engine::read_frame(
        &frame,
        &preset.calibration,
        &exp_atlas,
        &level_atlas,
        &identity_atlas,
        &preset.maps,
        &preset.streets,
        &preset.identities,
    )
    .map_err(|error| format!("read_frame failed: {error}"))?;

    println!("{}: {readings:?}", case.key);
    Ok(Some(readings))
}

/// `attempt_read`, but a rejection panics: this is what the two gate tests use,
/// so a fixture the reader cannot read fails the gate rather than being scored.
fn read_case(case: &Case, mscw: &Path) -> Option<crate::engine::Readings> {
    attempt_read(case, mscw).unwrap_or_else(|error| panic!("{}: {error}", case.key))
}

/// Cases `reads_every_fixture_from_empty_atlases` is known to misread, by case
/// key, with what goes wrong. Each one is removed when the reader is fixed; a
/// listed case that reads its name and job right fails the test until it is.
const READ_KNOWN_GAPS: &[(&str, &str)] = &[(
    "1920x1080/eurovein-lv10-ellinia-weapon-store/folded",
    "Default Ratio stretches the name: `Eurovein` reads as `Eurovejn` and the job \
     `Magician` is not read. Reading the 1x picture (`native`) is the fix.",
)];

/// The full regression: every fixture, read from empty atlases, asserted against
/// its manifest.
///
/// Needs the installed client (digit glyphs and map/street labels) and the
/// Windows Arial face (name/job), which is why it is `#[ignore]`d: the hermetic
/// tier must pass on any machine, this one only where MScout can actually read.
#[test]
#[ignore = "requires the MSCW install and Windows Arial"]
fn reads_every_fixture_from_empty_atlases() {
    let mscw = mscw_dir();
    assert!(
        mscw.is_dir(),
        "MSCW install not found at {} — set MSCOUT_MSCW_DIR",
        mscw.display()
    );

    for case in load_cases() {
        let meta = &case.truth;
        let Some(readings) = read_case(&case, &mscw) else {
            continue;
        };

        println!("--- {} readings ---", case.key);
        println!("  exp_text   = {:?}", readings.exp_text);
        println!("  exp        = {}", readings.exp);
        println!(
            "  pct        = {:.2} (expected {:.2})",
            readings.pct, meta.pct
        );
        println!(
            "  level      = {} (level_text {:?})",
            readings.lv, readings.level_text
        );
        println!("  name       = {:?}", readings.name);
        println!("  job        = {:?}", readings.job);
        println!("  map        = {:?}", readings.map);
        println!("  map_hidden = {}", readings.map_hidden);
        println!("  conf       = {:.3}", readings.conf);

        if let Some((_, why)) = READ_KNOWN_GAPS.iter().find(|(key, _)| *key == case.key) {
            let identity_right = readings.name.as_deref() == Some(meta.name.as_str())
                && readings.job.as_deref() == Some(meta.job.as_str());
            assert!(
                !identity_right,
                "{}: reads its name and job now; remove it from READ_KNOWN_GAPS",
                case.key
            );
            println!("  known gap  : {why}");
            continue;
        }

        assert_eq!(
            readings.exp_text, meta.exp_text,
            "{}: EXP readout",
            case.key
        );
        assert_eq!(readings.exp, meta.exp, "{}: EXP value", case.key);
        assert!(
            (readings.pct - meta.pct).abs() < 0.005,
            "{}: percentage {} vs {}",
            case.key,
            readings.pct,
            meta.pct
        );
        assert_eq!(readings.lv, meta.level, "{}: level", case.key);
        assert_eq!(
            readings.name.as_deref(),
            Some(meta.name.as_str()),
            "{}: name",
            case.key
        );
        assert_eq!(
            readings.job.as_deref(),
            Some(meta.job.as_str()),
            "{}: job",
            case.key
        );

        // An expanded or folded minimap draws the label as two lines, street over
        // map name; that is the shape `readings.map` carries.
        match case.minimap_state.as_str() {
            "expanded" | "folded" => {
                assert_eq!(
                    readings.map,
                    format!("{}\n{}", meta.street, meta.map),
                    "{}: map/street",
                    case.key
                );
                assert!(!readings.map_hidden, "{}: map reported hidden", case.key);
            }
            other => {
                // The state-aware test covers these in detail; here the manifest
                // only has to not claim a label that is not drawn.
                println!("  ({other} minimap: checked by reads_follow_the_manifest_minimap_state)");
            }
        }
    }
}

/// The same read, asserted per minimap state: a title-bar-only minimap has no
/// label at all, a folded one still carries `street :map`, and a hidden one has
/// no widget to read.
///
/// Driven entirely by `meta.minimapState`, so the corpus can gain folded and
/// title-bar frames without touching this test.
#[test]
#[ignore = "requires the MSCW install and Windows Arial"]
fn reads_follow_the_manifest_minimap_state() {
    let mscw = mscw_dir();
    assert!(
        mscw.is_dir(),
        "MSCW install not found at {} — set MSCOUT_MSCW_DIR",
        mscw.display()
    );

    for case in load_cases() {
        let meta = &case.truth;
        let Some(readings) = read_case(&case, &mscw) else {
            continue;
        };

        match case.minimap_state.as_str() {
            "titlebar" => {
                assert!(
                    readings.map_hidden,
                    "{}: a title-bar-only minimap must report the map as hidden",
                    case.key
                );
                assert_eq!(readings.map, "unknown", "{}: hidden map name", case.key);
            }
            "folded" => {
                assert!(
                    !readings.map_hidden,
                    "{}: folded map reported hidden",
                    case.key
                );
                assert_eq!(
                    readings.map,
                    format!("{}\n{}", meta.street, meta.map),
                    "{}: folded street :map",
                    case.key
                );
            }
            "expanded" => {
                assert!(
                    !readings.map_hidden,
                    "{}: expanded map reported hidden",
                    case.key
                );
                assert_eq!(
                    readings.map,
                    format!("{}\n{}", meta.street, meta.map),
                    "{}: expanded label",
                    case.key
                );
            }
            "hidden" => {
                assert!(
                    !readings.map_hidden,
                    "{}: with no minimap there is no title bar to call hidden",
                    case.key
                );
                assert_eq!(readings.map, "unknown", "{}: map name", case.key);
            }
            other => panic!("{}: unknown minimapState {other:?}", case.key),
        }
    }
}

// ---------------------------------------------------------------------------
// Accuracy reporting: the same reads, scored instead of asserted.
// ---------------------------------------------------------------------------

/// The map/map_hidden expectation for a case's minimap state: the union of what
/// `reads_every_fixture_from_empty_atlases` and
/// `reads_follow_the_manifest_minimap_state` assert.
fn expected_map(case: &Case) -> (String, bool) {
    let meta = &case.truth;
    match case.minimap_state.as_str() {
        "expanded" | "folded" => (format!("{}\n{}", meta.street, meta.map), false),
        "titlebar" => ("unknown".to_string(), true),
        "hidden" => ("unknown".to_string(), false),
        other => panic!("{}: unknown minimapState {other:?}", case.key),
    }
}

/// Every field of `readings` that disagrees with the manifest, as
/// `Field actual != expected`. Empty means the case read exactly.
fn field_mismatches(case: &Case, readings: &crate::engine::Readings) -> Vec<String> {
    let meta = &case.truth;
    let mut fields = Vec::new();
    if readings.exp_text != meta.exp_text {
        fields.push(format!(
            "exp_text {:?} != {:?}",
            readings.exp_text, meta.exp_text
        ));
    }
    if readings.exp != meta.exp {
        fields.push(format!("exp {} != {}", readings.exp, meta.exp));
    }
    if (readings.pct - meta.pct).abs() >= 0.005 {
        fields.push(format!("pct {:.2} != {:.2}", readings.pct, meta.pct));
    }
    if readings.lv != meta.level {
        fields.push(format!("level {} != {}", readings.lv, meta.level));
    }
    if readings.name.as_deref() != Some(meta.name.as_str()) {
        fields.push(format!("name {:?} != {:?}", readings.name, meta.name));
    }
    if readings.job.as_deref() != Some(meta.job.as_str()) {
        fields.push(format!("job {:?} != {:?}", readings.job, meta.job));
    }
    let (expected_map, expected_hidden) = expected_map(case);
    if readings.map != expected_map {
        fields.push(format!("map {:?} != {:?}", readings.map, expected_map));
    }
    if readings.map_hidden != expected_hidden {
        fields.push(format!(
            "map_hidden {} != {expected_hidden}",
            readings.map_hidden
        ));
    }
    fields
}

/// What one case did when it was read and scored. The three counts the report
/// publishes are `Exact`, `Rejected` and `Wrong`; the other two classes exist so
/// nothing can be dropped from the report.
enum Verdict {
    /// No shipped preset for the pair: there is nothing to read. Never a pass.
    NoPreset,
    /// The reader (or its seeding) refused to produce a reading.
    Rejected(String),
    /// A reading was produced but at least one field disagrees with the manifest.
    Wrong(Vec<String>),
    /// Every field agrees with the manifest.
    Exact,
}

impl Verdict {
    fn label(&self) -> &'static str {
        match self {
            Self::NoPreset => "no-preset",
            Self::Rejected(_) => "rejected",
            Self::Wrong(_) => "wrong",
            Self::Exact => "exact",
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::NoPreset | Self::Exact => String::new(),
            Self::Rejected(reason) => reason.clone(),
            Self::Wrong(fields) => fields.join("; "),
        }
    }

    /// `label`, then `detail` when there is one: what the hold-out line prints.
    fn summary(&self) -> String {
        match self.detail() {
            detail if detail.is_empty() => self.label().to_string(),
            detail => format!("{} {detail}", self.label()),
        }
    }
}

/// Score one case: `attempt_read` plus the manifest comparison, without
/// asserting anything about the result.
fn verdict(case: &Case, mscw: &Path) -> Verdict {
    match attempt_read(case, mscw) {
        Ok(None) => Verdict::NoPreset,
        Err(reason) => Verdict::Rejected(reason),
        Ok(Some(readings)) => {
            let fields = field_mismatches(case, &readings);
            if fields.is_empty() {
                Verdict::Exact
            } else {
                Verdict::Wrong(fields)
            }
        }
    }
}

/// The descriptive active-filter note, so a published report cannot be mistaken
/// for a whole-corpus run when `MSCOUT_CASE`/`MSCOUT_STATE` narrowed it.
fn filters_note() -> String {
    let mut active = Vec::new();
    if let Ok(case) = std::env::var("MSCOUT_CASE") {
        active.push(format!("MSCOUT_CASE={case}"));
    }
    if let Ok(state) = std::env::var("MSCOUT_STATE") {
        active.push(format!("MSCOUT_STATE={state}"));
    }
    if active.is_empty() {
        "none (whole corpus)".to_string()
    } else {
        active.join(", ")
    }
}

/// Publish the corpus's accuracy instead of only asserting it: for every frame
/// case, print whether the read was **exact**, **rejected** or **wrong**, with
/// the wrong fields spelled out, then the per-pair counts and the frozen
/// hold-out line.
///
/// `docs/ACCURACY.md` records these numbers with the date and the command. The
/// report deliberately does **not** assert the counts: a known wrong stays a
/// printed wrong, so it cannot be turned into a silent pass and a real
/// improvement cannot be mistaken for one either.
#[test]
#[ignore = "requires the MSCW install and Windows Arial"]
fn accuracy_report() {
    let mscw = mscw_dir();
    assert!(
        mscw.is_dir(),
        "MSCW install not found at {} — set MSCOUT_MSCW_DIR",
        mscw.display()
    );

    let cases = load_cases();
    assert!(!cases.is_empty(), "no fixture cases to report");

    println!();
    println!("=== fixture accuracy report ===");
    println!("fixtures : {}", fixtures_dir().display());
    println!("filters  : {}", filters_note());
    println!("cases    : {}", cases.len());
    println!();

    let mut counts: BTreeMap<String, [usize; 4]> = BTreeMap::new();
    let mut scored: Vec<(String, Verdict)> = Vec::new();
    for case in &cases {
        let verdict = verdict(case, &mscw);
        let entry = counts.entry(case.pair_key.clone()).or_insert([0; 4]);
        match &verdict {
            Verdict::Exact => entry[0] += 1,
            Verdict::Rejected(_) => entry[1] += 1,
            Verdict::Wrong(_) => entry[2] += 1,
            Verdict::NoPreset => entry[3] += 1,
        }
        println!(
            "  {:<24} {:<10} {}",
            case.key,
            verdict.label(),
            verdict.detail()
        );
        scored.push((case.key.clone(), verdict));
    }

    println!();
    println!(
        "{:<24} {:>5} {:>9} {:>6} {:>10} {:>6}",
        "pair", "exact", "rejected", "wrong", "no-preset", "cases"
    );
    let mut total = [0usize; 4];
    for (pair, row) in &counts {
        for (sum, value) in total.iter_mut().zip(row) {
            *sum += *value;
        }
        println!(
            "{:<24} {:>5} {:>9} {:>6} {:>10} {:>6}",
            pair,
            row[0],
            row[1],
            row[2],
            row[3],
            row.iter().sum::<usize>()
        );
    }
    println!(
        "{:<24} {:>5} {:>9} {:>6} {:>10} {:>6}",
        "total",
        total[0],
        total[1],
        total[2],
        total[3],
        total.iter().sum::<usize>()
    );

    // The hold-out line always names the frozen state, even when the filters
    // excluded it from the counted run: the reservation is a corpus fact, not a
    // property of this invocation.
    let frozen: Vec<Case> = load_cases_in(&fixtures_dir())
        .into_iter()
        .filter(|case| case.holdout)
        .collect();
    assert_eq!(
        frozen.len(),
        1,
        "exactly one frozen hold-out expected; found {}",
        frozen.len()
    );
    let frozen_case = &frozen[0];
    let frozen_verdict = match scored.iter().find(|(key, _)| *key == frozen_case.key) {
        Some((_, verdict)) => verdict.summary(),
        // Not a second case and not a duplicate read: the filters excluded the
        // hold-out from the counted run, so this is its only read here. The line
        // exists because the reservation is a corpus fact, not a property of this
        // invocation.
        None => format!(
            "{} (not in this filtered run)",
            verdict(frozen_case, &mscw).summary()
        ),
    };
    println!();
    println!("hold-out : {} — {frozen_verdict}", frozen_case.key);

    println!();
    println!(
        "note: counts are printed, not asserted — a known wrong stays visible as \
         a wrong and is never promoted to a pass."
    );
}

/// Live confirmation: capture the running client and read it with the
/// preset for the app's current UI Size. The game must be at that size.
#[test]
#[ignore = "live: reads the running 1920x1080 client"]
fn reads_live_1920() {
    let selector = crate::settings::WindowSelector {
        title: "MapleStory".into(),
        process: "MapleStory.exe".into(),
    };
    let window = crate::window_target::resolve_selector(&selector)
        .expect("the MapleStory window is running");
    let frame = crate::capture::client_frame(window.hwnd).expect("capture the client");
    let mode = crate::settings::ui_mode();
    println!(
        "live frame {}x{}; app uiMode={mode:?}",
        frame.width, frame.height
    );
    assert_eq!(
        (frame.width, frame.height),
        (1920, 1080),
        "the game client is not 1920x1080"
    );

    let Some(preset) = crate::preset::for_size(1920, 1080, mode) else {
        println!("no 1920x1080 preset for the live UI mode {mode:?}; skipping");
        return;
    };
    let mscw = mscw_dir();

    let exp_region = preset
        .calibration
        .exp
        .filter(Region::is_usable)
        .expect("EXP box");
    let exp_gray = ocr::crop_gray(&frame, exp_region).expect("EXP crop");
    let (exp_stored, exp_height) =
        crate::game_font::seed_exp_atlas(&exp_gray, &mscw).expect("seed EXP");

    let level_region = preset
        .calibration
        .level
        .filter(Region::is_usable)
        .expect("level box");
    let (level_stored, level_size, _) =
        crate::game_font::seed_level_best(&frame, level_region, &mscw).expect("seed level");

    let mut identity: Option<Atlas> = None;
    for region in [preset.calibration.job, preset.calibration.name] {
        let Some(region) = region.filter(Region::is_usable) else {
            continue;
        };
        let Some(gray) = ocr::crop_gray(&frame, region) else {
            continue;
        };
        if let Some((stored, px)) = crate::font_seed::seed_identity(&gray) {
            println!("identity seeded from {region:?} at {px}px");
            let atlas = Atlas::from_settings(&stored);
            match &mut identity {
                Some(combined) => combined.glyphs.extend(atlas.glyphs),
                None => identity = Some(atlas),
            }
        }
    }
    let identity = identity.expect("identity seed").to_settings();

    let exp_atlas = Atlas::from_settings(&exp_stored);
    let level_atlas = Atlas::from_settings(&level_stored);
    let identity_atlas = Atlas::from_settings(&identity);
    let readings = crate::engine::read_frame(
        &frame,
        &preset.calibration,
        &exp_atlas,
        &level_atlas,
        &identity_atlas,
        &preset.maps,
        &preset.streets,
        &preset.identities,
    )
    .expect("live read");
    println!("live EXP height {exp_height}px, level size {level_size:?}");
    println!("live readings {readings:?}");
}

#[test]
#[ignore = "live: prints the minimap detection on the running client"]
fn probe_live_minimap() {
    let selector = crate::settings::WindowSelector {
        title: "MapleStory".into(),
        process: "MapleStory.exe".into(),
    };
    let window = crate::window_target::resolve_selector(&selector).expect("game window");
    let frame = crate::capture::client_frame(window.hwnd).expect("capture");
    println!("frame {}x{}", frame.width, frame.height);
    println!("minimap: {:?}", ocr::detect_minimap(&frame));
    if let Some(lines) = ocr::map_label_lines(&frame) {
        println!(
            "labels: {:?}",
            lines
                .iter()
                .map(|line| (line.w, line.h))
                .collect::<Vec<_>>()
        );
    }
}
