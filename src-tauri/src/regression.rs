//! Fixture-driven regression gate for the capture readers.
//!
//! Every immediate subdirectory of `src-tauri/testdata/` that carries a
//! `meta.json` is one saved client frame: lossless PNG crops of the regions a
//! reader consumes, plus the ground truth that reader must reproduce. The suite
//! replays the real code — `ocr::detect_minimap` and, on the client-dependent
//! tier, `engine::read_frame` over atlases seeded from the frame itself — and
//! asserts the manifest for every case.
//!
//! Two tiers:
//!
//! * **Hermetic** (default, `cargo test --lib`): corpus integrity, minimap state
//!   detection and the "settings panel is evidence only" invariant. They need
//!   nothing but the repository.
//! * **Client-dependent** (`--ignored`): the full read. Seeding glyphs and labels
//!   needs the installed client (`MSCW`) and the Windows Arial face.
//!
//! Nothing here skips a fixture. A case that is missing a file, whose crop lies
//! outside the client, or whose manifest disagrees with its pixels panics with
//! the case name in the message — a corpus that silently shrinks is a gate that
//! silently stops gating.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::capture::Frame;
use crate::ocr::{self, Atlas, MinimapState};
use crate::settings::{Region, UiMode};

/// The tracked fixture corpus. `CARGO_MANIFEST_DIR` is `src-tauri`, and the
/// directory ships with the source, so this resolves wherever the crate is built
/// from.
pub(crate) fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

/// A crop rectangle in client coordinates, as written in `meta.json`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    /// True when the rect lies inside a `client_w` x `client_h` client, i.e. every
    /// pixel it names exists in the composed frame.
    fn fits(self, client_w: u32, client_h: u32) -> bool {
        self.w > 0
            && self.h > 0
            && self.x >= 0
            && self.y >= 0
            && self.x as i64 + self.w as i64 <= client_w as i64
            && self.y as i64 + self.h as i64 <= client_h as i64
    }
}

/// The shared crops. A single-case manifest also carries `minimap`; a
/// multi-state manifest lists each minimap under `minimaps` instead.
#[derive(Clone, Copy, Debug, Deserialize)]
struct Crops {
    #[serde(default)]
    minimap: Option<Rect>,
    hud: Rect,
    settings: Rect,
}

/// One minimap-state variant inside a pair's single `meta.json`.
#[derive(Clone, Debug, Deserialize)]
struct MinimapCase {
    state: String,
    file: String,
    crop: Rect,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsDisplay {
    /// Resolution control as the client draws it, `"1920 x 1080"`.
    resolution: String,
    /// UI Size control as the client draws it, `"Ideal Ratio"`.
    ui_size: String,
}

/// `meta.json`: the ground truth for one captured frame. Required fields are left
/// without defaults on purpose — a manifest that has lost a field must fail to
/// parse, not read as a default.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    client_w: u32,
    client_h: u32,
    ui_mode: String,
    #[serde(default)]
    minimap_state: Option<String>,
    #[serde(default)]
    minimaps: Vec<MinimapCase>,
    name: String,
    job: String,
    level: u32,
    exp_text: String,
    exp: i64,
    pct: f64,
    street: String,
    map: String,
    crops: Crops,
    settings_display: SettingsDisplay,
}

/// One fixture case: the manifest plus where its images live.
struct Case {
    /// Display key: the pair name for a single case, `pair/state` for a multi-state
    /// pair.
    key: String,
    /// The pair directory's name (`1920x1080`, `1366x768`), checked against the
    /// preset key.
    pair_key: String,
    /// The directory holding this case's manifest and crops.
    dir: PathBuf,
    /// The pair directory, where a shared `settings.png` lives (may equal `dir`).
    pair_dir: PathBuf,
    /// The minimap variant this case exercises.
    minimap_state: String,
    minimap_file: String,
    minimap_crop: Rect,
    meta: Meta,
}

impl Case {
    fn mode(&self) -> UiMode {
        UiMode::parse(&self.meta.ui_mode).unwrap_or_else(|| {
            panic!(
                "{}: uiMode {:?} is not one of {}",
                self.key,
                self.meta.ui_mode,
                UiMode::ALL
                    .iter()
                    .map(|mode| mode.id())
                    .collect::<Vec<_>>()
                    .join(" / ")
            )
        })
    }

    fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join("meta.json")
    }

    /// The evidence-only panel. A pair with one case keeps it in the case
    /// directory; a pair with several minimap-state cases keeps one copy at the
    /// pair root, because the panel is pair-level evidence, not frame state.
    fn settings_path(&self) -> PathBuf {
        let local = self.dir.join("settings.png");
        if local.is_file() {
            local
        } else {
            self.pair_dir.join("settings.png")
        }
    }

    fn client_size(&self) -> (u32, u32) {
        (self.meta.client_w, self.meta.client_h)
    }

    /// The in-file message prefix: every panic names its case.
    fn label(&self, what: &str) -> String {
        format!("{}: {what}", self.key)
    }
}

/// Every fixture directory, in a stable order. `MSCOUT_CASE=<pair>` narrows the
/// run to one fixture (or all of a pair's state cases), and `MSCOUT_STATE=<state>`
/// narrows it to one minimap state.
fn load_cases() -> Vec<Case> {
    let mut cases = load_cases_in(&fixtures_dir());
    if let Ok(only) = std::env::var("MSCOUT_CASE") {
        cases.retain(|case| case.key == only || case.key.starts_with(&format!("{only}/")));
        assert!(!cases.is_empty(), "MSCOUT_CASE={only:?} matched no fixture");
    }
    if let Ok(state) = std::env::var("MSCOUT_STATE") {
        cases.retain(|case| case.minimap_state == state);
        assert!(!cases.is_empty(), "MSCOUT_STATE={state:?} matched no fixture");
    }
    cases
}

/// Load the corpus under `root`.
///
/// A subdirectory with no `meta.json` is never skipped, even when it looks like a
/// draft: a directory of PNGs that nothing reads is exactly the silent skip this
/// suite exists to prevent.
fn load_cases_in(root: &Path) -> Vec<Case> {
    assert!(
        root.is_dir(),
        "the fixture corpus is missing: {} does not exist",
        root.display()
    );

    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", root.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("cannot list {}: {error}", root.display()))
                .path()
        })
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();

    let mut cases = Vec::new();
    for dir in dirs {
        let pair_key = dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.display().to_string());

        let meta_path = dir.join("meta.json");
        assert!(
            meta_path.is_file(),
            "{pair_key}: no meta.json — every pair directory needs one manifest"
        );
        let text = std::fs::read_to_string(&meta_path).unwrap_or_else(|error| {
            panic!("{pair_key}: cannot read {}: {error}", meta_path.display())
        });
        let meta: Meta = serde_json::from_str(&text).unwrap_or_else(|error| {
            panic!("{pair_key}: {} is not a complete manifest: {error}", meta_path.display())
        });

        if meta.minimaps.is_empty() {
            let state = meta.minimap_state.clone().unwrap_or_else(|| {
                panic!("{pair_key}: meta.json needs minimapState or a minimaps list")
            });
            let crop = meta.crops.minimap.unwrap_or_else(|| {
                panic!("{pair_key}: a single-case meta.json needs crops.minimap")
            });
            cases.push(Case {
                key: pair_key.clone(),
                pair_key: pair_key.clone(),
                dir: dir.clone(),
                pair_dir: dir.clone(),
                minimap_state: state,
                minimap_file: "minimap.png".into(),
                minimap_crop: crop,
                meta,
            });
            continue;
        }

        assert!(
            meta.minimap_state.is_none(),
            "{pair_key}: use either minimapState or minimaps, not both"
        );
        assert!(
            meta.crops.minimap.is_none(),
            "{pair_key}: use either crops.minimap or minimaps, not both"
        );
        for variant in &meta.minimaps {
            cases.push(Case {
                key: format!("{pair_key}/{}", variant.state),
                pair_key: pair_key.clone(),
                dir: dir.clone(),
                pair_dir: dir.clone(),
                minimap_state: variant.state.clone(),
                minimap_file: variant.file.clone(),
                minimap_crop: variant.crop,
                meta: meta.clone(),
            });
        }
    }

    assert!(
        !cases.is_empty(),
        "{} has no fixtures — the regression gate would pass without checking anything",
        root.display()
    );
    cases
}

/// Decode a lossless PNG fixture into straight RGBA. The reader consumes
/// `Frame::rgba`, so the fixture is loaded in exactly that form.
fn load_png_rgba(path: &Path) -> (u32, u32, Vec<u8>) {
    let image = image::open(path)
        .unwrap_or_else(|error| panic!("cannot decode {}: {error}", path.display()))
        .to_rgba8();
    (image.width(), image.height(), image.into_raw())
}

/// Paste a PNG into the frame at `rect`, after checking both against the case.
fn paste(frame: &mut Frame, case: &Case, file: &str, rect: Rect) {
    let path = case.path(file);
    assert!(
        path.is_file(),
        "{}",
        case.label(&format!("{file} is missing at {}", path.display()))
    );

    let (w, h, rgba) = load_png_rgba(&path);
    assert_eq!(
        (w, h),
        (rect.w as u32, rect.h as u32),
        "{}",
        case.label(&format!(
            "{file} is {w}x{h} but crops.{} says {}x{}",
            crop_name(file),
            rect.w,
            rect.h
        ))
    );

    for y in 0..h {
        let src = (y * w) as usize * 4;
        let dst = (((rect.y as u32 + y) * frame.width + rect.x as u32) * 4) as usize;
        let len = (w * 4) as usize;
        frame.rgba[dst..dst + len].copy_from_slice(&rgba[src..src + len]);
    }
}

fn crop_name(file: &str) -> &'static str {
    if file.starts_with("minimap") {
        "minimap"
    } else if file == "hud.png" {
        "hud"
    } else {
        "settings"
    }
}

/// Build the frame a capture would have produced: `clientW` x `clientH`, black,
/// with the minimap and HUD crops pasted at the rects they were cut from.
///
/// `settings.png` is deliberately not pasted: it is evidence for the manifest,
/// never input to a reader.
fn compose_frame(case: &Case) -> Frame {
    let (width, height) = case.client_size();
    assert!(
        width > 0 && height > 0,
        "{}",
        case.label("clientW/clientH must be non-zero")
    );

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    let mut frame = Frame {
        width,
        height,
        rgba,
    };

    for (name, rect) in [
        ("minimap", case.minimap_crop),
        ("hud", case.meta.crops.hud),
    ] {
        assert!(
            rect.fits(width, height),
            "{}",
            case.label(&format!(
                "crops.{name} is {},{} {}x{} — outside the {width}x{height} client",
                rect.x, rect.y, rect.w, rect.h
            ))
        );
    }

    paste(&mut frame, case, &case.minimap_file, case.minimap_crop);
    paste(&mut frame, case, "hud.png", case.meta.crops.hud);
    frame
}

/// The reader's expectation for a `minimapState` string.
fn expected_state(state: &str) -> Option<MinimapState> {
    match state {
        "expanded" => Some(MinimapState::Expanded),
        "titlebar" => Some(MinimapState::TitleBarOnly),
        "folded" => Some(MinimapState::Folded),
        // No widget on screen at all: nothing to detect.
        "hidden" => None,
        other => panic!("unknown minimapState {other:?} — the manifest schema has three states plus \"hidden\""),
    }
}

// ---------------------------------------------------------------------------
// Hermetic tier: needs the repository, nothing else.
// ---------------------------------------------------------------------------

/// The corpus itself: present, complete, self-consistent and in bounds. This is
/// the test that fails when a fixture is half-added or a manifest drifts.
#[test]
fn fixture_corpus_is_complete() {
    let cases = load_cases();
    assert!(!cases.is_empty(), "expected at least one fixture case");

    for case in &cases {
        let meta = &case.meta;
        let (client_w, client_h) = case.client_size();
        let mode = case.mode();

        // The pair directory name is the preset/profile key, so a misnamed
        // fixture would silently test the wrong geometry. A nested case adds only
        // a state subdirectory below it.
        let expected_key = format!("{client_w}x{client_h}{}", mode.suffix());
        assert_eq!(
            case.pair_key, expected_key,
            "fixture directory {:?} must be named like its preset ({expected_key})",
            case.dir.display()
        );

        // Every file the suite consumes, including the evidence-only panel.
        for file in [case.minimap_file.as_str(), "hud.png"] {
            let path = case.path(file);
            assert!(
                path.is_file(),
                "{}",
                case.label(&format!("{file} is missing at {}", path.display()))
            );
        }
        let manifest = case.manifest_path();
        assert!(
            manifest.is_file(),
            "{}",
            case.label(&format!("manifest is missing at {}", manifest.display()))
        );
        let settings_path = case.settings_path();
        assert!(
            settings_path.is_file(),
            "{}",
            case.label(&format!("settings.png is missing at {}", settings_path.display()))
        );

        // Crop rects, and the PNGs that must match them pixel for pixel.
        for (name, file, rect) in [
            ("minimap", case.minimap_file.as_str(), case.minimap_crop),
            ("hud", "hud.png", meta.crops.hud),
            ("settings", "settings.png", meta.crops.settings),
        ] {
            assert!(
                rect.fits(client_w, client_h),
                "{}",
                case.label(&format!(
                    "crops.{name} is {},{} {}x{} — outside the {client_w}x{client_h} client",
                    rect.x, rect.y, rect.w, rect.h
                ))
            );

            let path = if name == "settings" {
                case.settings_path()
            } else {
                case.path(file)
            };
            let (w, h, _) = load_png_rgba(&path);
            assert_eq!(
                (w, h),
                (rect.w as u32, rect.h as u32),
                "{}",
                case.label(&format!(
                    "{file} is {w}x{h} but crops.{name} says {}x{}",
                    rect.w, rect.h
                ))
            );
        }

        // The frame's own settings panel has to agree with the manifest, or the
        // manifest's uiMode (and therefore the preset the read uses) is wrong.
        assert_eq!(
            meta.settings_display.resolution,
            format!("{client_w} x {client_h}"),
            "{}",
            case.label("settingsDisplay.resolution disagrees with clientW/clientH")
        );
        assert_eq!(
            meta.settings_display.ui_size,
            mode.label(),
            "{}",
            case.label(&format!(
                "settingsDisplay.uiSize should name uiMode {:?}",
                meta.ui_mode
            ))
        );

        // A blank settings.png would "agree" with anything, so require it to
        // actually carry an image.
        let (_, _, settings) = load_png_rgba(&case.settings_path());
        let colours: BTreeSet<[u8; 3]> = settings
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        assert!(
            colours.len() > 1,
            "{}",
            case.label(&format!(
                "settings.png is a single flat colour ({:?}) — it cannot be evidence",
                colours.iter().next()
            ))
        );

        println!("{}: {client_w}x{client_h} {}", case.key, mode.id());
    }
}

/// The minimap state detector must name the state each frame was captured in.
/// This is the cheap half of `read_frame`'s map handling, and it is what decides
/// whether a map name is even on screen.
#[test]
fn minimap_state_matches_ground_truth() {
    for case in load_cases() {
        let frame = compose_frame(&case);
        let detected = ocr::detect_minimap(&frame);
        let expected = expected_state(&case.minimap_state);
        assert_eq!(
            detected.map(|minimap| minimap.state),
            expected,
            "{}",
            case.label(&format!(
                "detect_minimap disagreed with the case minimap state {:?}",
                case.minimap_state
            ))
        );
    }
}

/// The settings panel is evidence, not input: nothing may reach a reader. The
/// composed frame leaves its rect at the background value, which proves the crop
/// is not composed in — and a reader that found anything there would be reading
/// pixels no fixture provides.
#[test]
fn settings_panel_is_evidence_only() {
    for case in load_cases() {
        let frame = compose_frame(&case);
        let rect = case.meta.crops.settings;
        assert!(
            rect.fits(frame.width, frame.height),
            "{}",
            case.label("crops.settings is outside the client")
        );

        for y in rect.y as u32..(rect.y as u32 + rect.h as u32) {
            for x in rect.x as u32..(rect.x as u32 + rect.w as u32) {
                let index = ((y * frame.width + x) * 4) as usize;
                let pixel = &frame.rgba[index..index + 4];
                assert_eq!(
                    (pixel[0], pixel[1], pixel[2]),
                    (0, 0, 0),
                    "{}",
                    case.label(&format!(
                        "the settings panel was composed into the frame at {x},{y} — it is evidence only"
                    ))
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The gate's own guards: bounds checking and the refusal to skip.
// ---------------------------------------------------------------------------

/// A crop rect that leaves the client must be rejected before it is composed or
/// cropped: `crop_gray` silently clamps, which would make a bad manifest read as
/// a shifted image instead of an error.
#[test]
fn crop_bounds_check_rejects_rects_outside_the_client() {
    let (w, h) = (1920u32, 1080u32);
    let (wi, hi) = (w as i32, h as i32);

    assert!(Rect { x: 19, y: 19, w: 226, h: 183 }.fits(w, h));
    assert!(Rect { x: wi - 10, y: hi - 10, w: 10, h: 10 }.fits(w, h));

    for bad in [
        Rect { x: -1, y: 0, w: 10, h: 10 },
        Rect { x: 0, y: -1, w: 10, h: 10 },
        Rect { x: wi - 9, y: 0, w: 10, h: 10 },
        Rect { x: 0, y: hi - 9, w: 10, h: 10 },
        Rect { x: 0, y: 0, w: 0, h: 10 },
        Rect { x: 0, y: 0, w: 10, h: 0 },
    ] {
        assert!(!bad.fits(w, h), "{bad:?} must not fit inside {w}x{h}");
    }
}

/// A corpus with no cases is a gate that cannot fail, so it has to fail loudly.
#[test]
#[should_panic(expected = "has no fixtures")]
fn an_empty_corpus_panics() {
    let root = TempRoot::new("empty");
    load_cases_in(&root.0);
}

/// A directory of images with no manifest is a fixture that quietly stopped
/// being tested. It must panic, not be skipped.
#[test]
#[should_panic(expected = "no meta.json")]
fn a_fixture_directory_without_a_manifest_panics() {
    let root = TempRoot::new("no-manifest");
    let case = root.0.join("1920x1080@ideal");
    std::fs::create_dir_all(&case).expect("create the fixture directory");
    std::fs::write(case.join("hud.png"), b"not really a png").expect("write a stray image");
    load_cases_in(&root.0);
}

/// A throwaway corpus root under the system temp directory. Removed on drop, so
/// the two `should_panic` guards clean up while unwinding.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mscout-fixtures-{tag}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the temporary corpus root");
        Self(dir)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Client-dependent tier: the whole reader, from empty atlases.
// ---------------------------------------------------------------------------
/// The installed client, looking only at the environment and the configured
/// setting — never writing either.
fn mscw_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("MSCOUT_MSCW_DIR")
            .ok()
            .or_else(|| crate::settings::mscw_dir())
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
fn read_case(case: &Case, mscw: &Path) -> Option<crate::engine::Readings> {
    let (client_w, client_h) = case.client_size();
    let mode = case.mode();
    let key = format!("{client_w}x{client_h}{}", mode.suffix());

    let Some(preset) = crate::preset::for_size(client_w, client_h, mode) else {
        println!(
            "{}: no shipped preset for {key} — unsupported pair, skipping",
            case.key
        );
        return None;
    };

    let frame = compose_frame(case);

    let exp_region = preset
        .calibration
        .exp
        .filter(Region::is_usable)
        .unwrap_or_else(|| panic!("{}: preset {key} has no EXP box", case.key));
    let exp_gray = ocr::crop_gray(&frame, exp_region)
        .unwrap_or_else(|| panic!("{}: EXP box {exp_region:?} is outside the frame", case.key));
    let (exp, exp_height) = crate::game_font::seed_exp_atlas(&exp_gray, mscw)
        .unwrap_or_else(|error| panic!("{}: seeding the EXP atlas failed: {error}", case.key));

    let level_region = preset
        .calibration
        .level
        .filter(Region::is_usable)
        .unwrap_or_else(|| panic!("{}: preset {key} has no level box", case.key));
    let level_gray = ocr::crop_gray(&frame, level_region)
        .unwrap_or_else(|| panic!("{}: level box {level_region:?} is outside the frame", case.key));
    let (level, level_size) = crate::game_font::seed_level_atlas(&level_gray, mscw)
        .unwrap_or_else(|error| panic!("{}: seeding the level atlas failed: {error}", case.key));

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
        .unwrap_or_else(|| {
            panic!(
                "{}: could not seed the name/job atlas from any identity box",
                case.key
            )
        })
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
    .unwrap_or_else(|error| panic!("{}: read_frame failed: {error}", case.key));

    println!("{}: {readings:?}", case.key);
    Some(readings)
}

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
        let meta = &case.meta;
        let Some(readings) = read_case(&case, &mscw) else {
            continue;
        };

        println!("--- {} readings ---", case.key);
        println!("  exp_text   = {:?}", readings.exp_text);
        println!("  exp        = {}", readings.exp);
        println!("  pct        = {:.2} (expected {:.2})", readings.pct, meta.pct);
        println!(
            "  level      = {} (level_text {:?})",
            readings.lv, readings.level_text
        );
        println!("  name       = {:?}", readings.name);
        println!("  job        = {:?}", readings.job);
        println!("  map        = {:?}", readings.map);
        println!("  map_hidden = {}", readings.map_hidden);
        println!("  conf       = {:.3}", readings.conf);

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
        let meta = &case.meta;
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
                assert!(!readings.map_hidden, "{}: folded map reported hidden", case.key);
                assert_eq!(
                    readings.map,
                    format!("{}\n{}", meta.street, meta.map),
                    "{}: folded street :map",
                    case.key
                );
            }
            "expanded" => {
                assert!(!readings.map_hidden, "{}: expanded map reported hidden", case.key);
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
        let level_gray = ocr::crop_gray(&frame, level_region).expect("level crop");
        let (level_stored, level_size) =
            crate::game_font::seed_level_atlas(&level_gray, &mscw).expect("seed level");

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
