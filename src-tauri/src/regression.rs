//! Fixture-driven regression gate for the capture readers.
//!
//! Every immediate subdirectory of `src-tauri/testdata/` is one `WxH@mode`
//! corpus. Its `meta.json` indexes every PNG with a crop and ground truth; frame
//! cases are composed from the shared HUD plus one minimap image, while focused
//! EXP/map-label images are validated but read directly by targeted tests. The
//! suite replays the real code — `ocr::detect_minimap` and, on the
//! client-dependent tier, `engine::read_frame` over atlases seeded from the
//! frame itself — and asserts the manifest for every case.
//!
//! Two tiers:
//!
//! * **Hermetic** (default, `cargo test --lib`): corpus integrity, minimap state
//!   detection and the "settings panel is evidence only" invariant. They need
//!   nothing but the repository.
//! * **Client-dependent** (`--ignored`): the full read. Seeding glyphs and labels
//!   needs the installed client (`MSCW`) and the Windows Arial face. This tier
//!   also carries `accuracy_report`, which scores every case **exact /
//!   rejected / wrong** against its manifest so the corpus reports its accuracy
//!   instead of only asserting it.
//!
//! Exactly one minimap state is flagged `holdout` in its manifest: the frozen
//! hold-out. It was frozen retrospectively (see `testdata/README.md`), so it is
//! *not* historically independent of the tuning that came before it — but from
//! now on it is read and reported, never tuned against.
//!
//! Nothing here skips a fixture. A case that is missing a file, whose crop lies
//! outside the client, or whose manifest disagrees with its pixels panics with
//! the case name in the message — a corpus that silently shrinks is a gate that
//! silently stops gating.

use std::collections::{BTreeMap, BTreeSet};
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

/// A crop rectangle in the coordinate space named by its image entry.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
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

    pub(crate) fn region(self) -> Region {
        Region {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsDisplay {
    /// Resolution control as the client draws it, `"1920 x 1080"`.
    resolution: String,
    /// UI Size control as the client draws it, `"Ideal Ratio"`.
    ui_size: String,
}

/// One PNG in a pair's manifest. `client` images are pasted into a composed
/// client frame; `standalone` images are complete fixtures in their own right.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FixtureImage {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) file: String,
    pub(crate) source: String,
    pub(crate) crop: Rect,
    #[serde(default)]
    pub(crate) state: Option<String>,
    /// For a standalone readout crop (for example a full HUD whose EXP field is
    /// read), the rect the reader consumes. Defaults to the whole image.
    #[serde(default)]
    pub(crate) read_crop: Option<Rect>,
    #[serde(default)]
    pub(crate) exp_text: Option<String>,
    #[serde(default)]
    pub(crate) exp: Option<i64>,
    #[serde(default)]
    pub(crate) pct: Option<f64>,
    #[serde(default)]
    pub(crate) street: Option<String>,
    #[serde(default)]
    pub(crate) map: Option<String>,
    /// Frozen hold-out. Exactly one minimap image in the whole corpus carries
    /// this flag; `exactly_one_frozen_holdout` enforces that. A hold-out case is
    /// read and reported like any other, but it is not input to tuning: it was
    /// frozen on 2026-11-16 and must stay untouched.
    #[serde(default)]
    pub(crate) holdout: bool,
}

impl FixtureImage {
    pub(crate) fn read_region(&self) -> Region {
        self.read_crop.unwrap_or(self.crop).region()
    }
}

/// `meta.json`: the pair's ground truth and the index of every PNG. Required
/// fields are left without defaults on purpose — a manifest that has lost a
/// field must fail to parse, not read as a default.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Meta {
    pub(crate) version: u32,
    pub(crate) client_w: u32,
    pub(crate) client_h: u32,
    pub(crate) ui_mode: String,
    name: String,
    job: String,
    level: u32,
    exp_text: String,
    exp: i64,
    pct: f64,
    street: String,
    map: String,
    settings_display: SettingsDisplay,
    pub(crate) images: Vec<FixtureImage>,
}

/// One fixture case: the manifest plus where its images live.
struct Case {
    /// Display key: `pair/state`.
    key: String,
    /// The pair directory's name (`1920x1080`, `1366x768`), checked against the
    /// preset key.
    pair_key: String,
    /// The directory holding this case's manifest and crops.
    dir: PathBuf,
    /// The minimap variant this case exercises.
    minimap_state: String,
    minimap_file: String,
    minimap_crop: Rect,
    hud_file: String,
    hud_crop: Rect,
    settings_crop: Rect,
    /// True when this state is the frozen hold-out (see `FixtureImage::holdout`).
    holdout: bool,
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

    fn client_size(&self) -> (u32, u32) {
        (self.meta.client_w, self.meta.client_h)
    }

    /// The in-file message prefix: every panic names its case.
    fn label(&self, what: &str) -> String {
        format!("{}: {what}", self.key)
    }
}

/// Read a pair's manifest without going through the frame walker. Targeted
/// tests use this to resolve an image by id, so renaming a PNG is a manifest
/// edit rather than a source edit.
pub(crate) fn fixture_manifest(pair: &str) -> Meta {
    let path = fixtures_dir().join(pair).join("meta.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let meta: Meta = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not a manifest: {error}", path.display()));
    assert_eq!(
        meta.version,
        2,
        "{}: expected manifest version 2",
        path.display()
    );
    meta
}

/// Resolve one image id inside a pair and return its path and manifest entry.
pub(crate) fn fixture_image(pair: &str, id: &str) -> (PathBuf, FixtureImage) {
    let meta = fixture_manifest(pair);
    let image = meta
        .images
        .iter()
        .find(|image| image.id == id)
        .unwrap_or_else(|| {
            panic!(
                "{pair}: no image {id:?} in meta.json; ids are {}",
                meta.images
                    .iter()
                    .map(|image| image.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .clone();
    let path = fixtures_dir().join(pair).join(&image.file);
    assert!(
        path.is_file(),
        "{pair}: image {id:?} points at missing {}",
        path.display()
    );
    (path, image)
}

/// Decode a manifest image as a standalone frame. This is for focused fixtures
/// such as EXP/map-label crops; frame cases are composed by `compose_frame`.
pub(crate) fn fixture_frame(pair: &str, id: &str) -> Frame {
    let (path, _) = fixture_image(pair, id);
    let (width, height, rgba) = load_png_rgba(&path);
    Frame {
        width,
        height,
        rgba,
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
        assert!(
            !cases.is_empty(),
            "MSCOUT_STATE={state:?} matched no fixture"
        );
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
            panic!(
                "{pair_key}: {} is not a complete manifest: {error}",
                meta_path.display()
            )
        });

        assert_eq!(
            meta.version, 2,
            "{pair_key}: manifest version must be 2 (image-indexed corpus)"
        );
        let mut hud: Option<FixtureImage> = None;
        let mut settings: Option<FixtureImage> = None;
        let mut minimaps: Vec<FixtureImage> = Vec::new();
        for image in &meta.images {
            match image.kind.as_str() {
                "hud" => {
                    assert!(hud.is_none(), "{pair_key}: more than one hud image");
                    hud = Some(image.clone());
                }
                "settings" => {
                    assert!(
                        settings.is_none(),
                        "{pair_key}: more than one settings image"
                    );
                    settings = Some(image.clone());
                }
                "minimap" => minimaps.push(image.clone()),
                "exp" | "map-label" => {}
                other => panic!(
                    "{pair_key}: image {:?} has unknown kind {other:?}",
                    image.id
                ),
            }
        }
        let hud = hud.unwrap_or_else(|| panic!("{pair_key}: meta.json needs a hud image"));
        let settings =
            settings.unwrap_or_else(|| panic!("{pair_key}: meta.json needs a settings image"));
        assert!(
            !minimaps.is_empty(),
            "{pair_key}: meta.json needs at least one minimap image"
        );

        for variant in minimaps {
            let state = variant.state.clone().unwrap_or_else(|| {
                panic!("{pair_key}: minimap image {:?} needs a state", variant.id)
            });
            cases.push(Case {
                key: format!("{pair_key}/{state}"),
                pair_key: pair_key.clone(),
                dir: dir.clone(),
                minimap_state: state,
                minimap_file: variant.file,
                minimap_crop: variant.crop,
                hud_file: hud.file.clone(),
                hud_crop: hud.crop,
                settings_crop: settings.crop,
                holdout: variant.holdout,
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
    } else if file.starts_with("hud") {
        "hud"
    } else {
        "settings"
    }
}

/// Build the frame a capture would have produced: `clientW` x `clientH`, black,
/// with the minimap and HUD crops pasted at the rects they were cut from.
///
/// The settings panel is deliberately not pasted: it is evidence for the
/// manifest, never input to a reader.
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

    for (name, rect) in [("minimap", case.minimap_crop), ("hud", case.hud_crop)] {
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
    paste(&mut frame, case, &case.hud_file, case.hud_crop);
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
        other => panic!(
            "unknown minimapState {other:?} — the manifest schema has three states plus \"hidden\""
        ),
    }
}

// ---------------------------------------------------------------------------
// Hermetic tier: needs the repository, nothing else.
// ---------------------------------------------------------------------------

/// The corpus itself: present, complete, self-consistent and in bounds. This is
/// the test that fails when a fixture is half-added, a manifest drifts, or a PNG
/// is left out of the index.
#[test]
fn fixture_corpus_is_complete() {
    let cases = load_cases();
    assert!(!cases.is_empty(), "expected at least one fixture case");

    let mut checked = BTreeSet::new();
    for case in &cases {
        if checked.insert(case.pair_key.clone()) {
            validate_manifest(case);
        }
        println!(
            "{}: {}x{} {}",
            case.key,
            case.meta.client_w,
            case.meta.client_h,
            case.mode().id()
        );
    }
}

/// Validate one pair's manifest, every indexed PNG, and every PNG on disk.
fn validate_manifest(case: &Case) {
    let meta = &case.meta;
    let (client_w, client_h) = case.client_size();
    let mode = case.mode();
    let manifest = case.manifest_path();
    assert!(
        manifest.is_file(),
        "{}",
        case.label(&format!("manifest is missing at {}", manifest.display()))
    );

    // The pair directory name is the preset/profile key, so a misnamed fixture
    // would silently test the wrong geometry.
    let expected_key = format!("{client_w}x{client_h}{}", mode.suffix());
    assert_eq!(
        case.pair_key,
        expected_key,
        "fixture directory {:?} must be named like its preset ({expected_key})",
        case.dir.display()
    );

    let mut ids = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut kinds = std::collections::BTreeMap::new();
    for image in &meta.images {
        assert!(!image.id.is_empty(), "{}: image id is empty", case.key);
        assert!(!image.file.is_empty(), "{}: image file is empty", case.key);
        assert!(
            ids.insert(image.id.clone()),
            "{}: duplicate image id {:?}",
            case.key,
            image.id
        );
        assert!(
            files.insert(image.file.clone()),
            "{}: duplicate image file {:?}",
            case.key,
            image.file
        );
        *kinds.entry(image.kind.clone()).or_insert(0usize) += 1;

        let path = case.path(&image.file);
        assert!(
            path.is_file(),
            "{}",
            case.label(&format!("{} is missing at {}", image.file, path.display()))
        );
        let (w, h, rgba) = load_png_rgba(&path);
        assert_eq!(
            (w, h),
            (image.crop.w as u32, image.crop.h as u32),
            "{}",
            case.label(&format!(
                "{} (id {:?}) is {w}x{h} but its crop says {}x{}",
                image.file, image.id, image.crop.w, image.crop.h
            ))
        );

        match image.source.as_str() {
            "client" => assert!(
                image.crop.fits(client_w, client_h),
                "{}",
                case.label(&format!(
                    "{} crop {},{} {}x{} is outside the {client_w}x{client_h} client",
                    image.file, image.crop.x, image.crop.y, image.crop.w, image.crop.h
                ))
            ),
            "standalone" => assert!(
                image.crop.x == 0
                    && image.crop.y == 0
                    && image.crop.w == w as i32
                    && image.crop.h == h as i32,
                "{}",
                case.label(&format!(
                    "{} is standalone so its crop must be the whole image (0,0 {w}x{h})",
                    image.file
                ))
            ),
            other => panic!(
                "{}: image {:?} has unknown source {other:?}",
                case.key, image.id
            ),
        }

        match image.kind.as_str() {
            "hud" | "settings" => assert!(
                image.source == "client",
                "{}",
                case.label(&format!("{} must be a client crop", image.file))
            ),
            "minimap" => {
                assert!(
                    image.source == "client",
                    "{}",
                    case.label(&format!("{} must be a client crop", image.file))
                );
                let state = image
                    .state
                    .as_deref()
                    .unwrap_or_else(|| panic!("{}: minimap {:?} has no state", case.key, image.id));
                expected_state(state);
            }
            "exp" => {
                assert!(
                    image.source == "standalone",
                    "{}",
                    case.label(&format!("{} is a standalone readout", image.file))
                );
                let text = image.exp_text.as_deref().unwrap_or_else(|| {
                    panic!("{}: exp image {:?} has no expText", case.key, image.id)
                });
                let exp = image
                    .exp
                    .unwrap_or_else(|| panic!("{}: exp image {:?} has no exp", case.key, image.id));
                let pct = image
                    .pct
                    .unwrap_or_else(|| panic!("{}: exp image {:?} has no pct", case.key, image.id));
                let parsed = ocr::parse_exp_text(text)
                    .unwrap_or_else(|| panic!("{}: expText {text:?} is not a readout", case.key));
                assert_eq!(parsed, (exp, pct), "{}: expText disagrees", case.key);
                let read = image.read_crop.unwrap_or(image.crop);
                assert!(
                    read.fits(w, h),
                    "{}",
                    case.label(&format!(
                        "{} readCrop {},{} {}x{} is outside the {w}x{h} image",
                        image.file, read.x, read.y, read.w, read.h
                    ))
                );
            }
            "map-label" => {
                assert!(
                    image.source == "standalone",
                    "{}",
                    case.label(&format!("{} is a standalone map label", image.file))
                );
                assert!(
                    image.street.is_some() && image.map.is_some(),
                    "{}: map-label {:?} needs street and map",
                    case.key,
                    image.id
                );
            }
            other => panic!(
                "{}: image {:?} has unknown kind {other:?}",
                case.key, image.id
            ),
        }

        if image.kind == "settings" {
            let colours: BTreeSet<[u8; 3]> = rgba
                .chunks_exact(4)
                .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect();
            assert!(
                colours.len() > 1,
                "{}",
                case.label(&format!(
                    "{} is a single flat colour ({:?}) — it cannot be evidence",
                    image.file,
                    colours.iter().next()
                ))
            );
        }
    }

    assert_eq!(
        kinds.get("hud").copied().unwrap_or(0),
        1,
        "{}: manifest needs exactly one hud image",
        case.key
    );
    assert_eq!(
        kinds.get("settings").copied().unwrap_or(0),
        1,
        "{}: manifest needs exactly one settings image",
        case.key
    );
    assert!(
        kinds.get("minimap").copied().unwrap_or(0) >= 1,
        "{}: manifest needs at least one minimap image",
        case.key
    );

    // No PNG may sit in the directory without an index entry: that is the
    // focused-fixture leak this schema exists to prevent.
    let mut on_disk = BTreeSet::new();
    for entry in std::fs::read_dir(&case.dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", case.dir.display()))
    {
        let path = entry
            .unwrap_or_else(|error| panic!("cannot list {}: {error}", case.dir.display()))
            .path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("png") {
            on_disk.insert(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }
    if let Some(file) = on_disk.difference(&files).next() {
        panic!(
            "{}: {file} is not indexed in meta.json — every PNG needs an image entry",
            case.key
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
        let rect = case.settings_crop;
        assert!(
            rect.fits(frame.width, frame.height),
            "{}",
            case.label("the settings crop is outside the client")
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

/// Exactly one state in the corpus is the frozen hold-out, and it stays exactly
/// one: a second hold-out would make "never tuned against" meaningless, and none
/// would silently free the reserved case for tuning again.
///
/// Deliberately checks the **unfiltered** corpus, so `MSCOUT_CASE`/`MSCOUT_STATE`
/// cannot narrow the run into a pass.
#[test]
fn exactly_one_frozen_holdout() {
    let frozen: Vec<Case> = load_cases_in(&fixtures_dir())
        .into_iter()
        .filter(|case| case.holdout)
        .collect();
    assert_eq!(
        frozen.len(),
        1,
        "exactly one fixture state must be the frozen hold-out; found {}: {}",
        frozen.len(),
        frozen
            .iter()
            .map(|case| case.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let case = &frozen[0];
    // A hold-out has to be a real client state that a reader consumes, not a
    // focused crop or a state with nothing on screen.
    assert!(
        case.minimap_state == "expanded"
            || case.minimap_state == "folded"
            || case.minimap_state == "titlebar",
        "{}: the frozen hold-out must be a read minimap state, not {:?}",
        case.key,
        case.minimap_state
    );
    println!(
        "frozen hold-out: {} ({}x{} {})",
        case.key,
        case.meta.client_w,
        case.meta.client_h,
        case.mode().id()
    );
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

    assert!(Rect {
        x: 19,
        y: 19,
        w: 226,
        h: 183
    }
    .fits(w, h));
    assert!(Rect {
        x: wi - 10,
        y: hi - 10,
        w: 10,
        h: 10
    }
    .fits(w, h));

    for bad in [
        Rect {
            x: -1,
            y: 0,
            w: 10,
            h: 10,
        },
        Rect {
            x: 0,
            y: -1,
            w: 10,
            h: 10,
        },
        Rect {
            x: wi - 9,
            y: 0,
            w: 10,
            h: 10,
        },
        Rect {
            x: 0,
            y: hi - 9,
            w: 10,
            h: 10,
        },
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 10,
        },
        Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 0,
        },
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
    std::fs::write(case.join("hud-main.png"), b"not really a png").expect("write a stray image");
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
    let meta = &case.meta;
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
    let meta = &case.meta;
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
