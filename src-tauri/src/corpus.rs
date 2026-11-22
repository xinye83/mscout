//! The labelled test-data corpus in `src-tauri/testdata/`: loading, lookup and
//! the hermetic checks that keep it complete.
//!
//! Layout (see `testdata/README.md`):
//!
//! ```text
//! testdata/
//!   frames/<WxH@mode>/pair.json              client size, UI Size, settings-panel evidence
//!   frames/<WxH@mode>/settings-panel.png
//!   frames/<WxH@mode>/<scene>/scene.json      one capture moment: its truth and its crops
//!   frames/<WxH@mode>/<scene>/*.png
//!   crops/<WxH@mode>/<kind>-<stem>.png        a focused crop, read on its own
//!   crops/<WxH@mode>/<kind>-<stem>.json       ... and what it must read as
//!   profiles/upgrade/*.json                   release-shaped profiles (not images)
//! ```
//!
//! A scene owns its truth, so a folder can hold any number of characters,
//! levels and maps at one client size. Frame cases are composed from a scene's
//! HUD crop plus one of its minimap crops; focused crops are read directly by
//! targeted tests through [`fixture_image`] / [`fixture_frame`].
//!
//! Nothing here skips a fixture. A missing file, a crop outside the client, an
//! unindexed PNG, or truth nobody has checked (`"verified": false`) panics with
//! the fixture's name — a corpus that silently shrinks is a gate that silently
//! stops gating.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::capture::Frame;
use crate::ocr::{self, MinimapState};
use crate::settings::{Region, UiMode};

/// The manifest format every `pair.json`, `scene.json` and crop sidecar carries.
const VERSION: u32 = 3;

/// `src-tauri/testdata`. `CARGO_MANIFEST_DIR` is `src-tauri`, and the directory
/// ships with the source, so this resolves wherever the crate is built from.
pub(crate) fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

/// The frame corpus: one directory per `WxH@mode`.
pub(crate) fn frames_dir() -> PathBuf {
    fixtures_dir().join("frames")
}

/// Focused crops, one directory per `WxH@mode`.
pub(crate) fn crops_dir() -> PathBuf {
    fixtures_dir().join("crops")
}

/// A crop rectangle in client coordinates (scene images) or image coordinates
/// (a crop's `readCrop`).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: i32,
    pub(crate) h: i32,
}

impl Rect {
    /// True when the rect lies inside a `client_w` x `client_h` area, i.e. every
    /// pixel it names exists.
    pub(crate) fn fits(self, client_w: u32, client_h: u32) -> bool {
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsDisplay {
    /// Resolution control as the client draws it, `"1920 x 1080"`.
    resolution: String,
    /// UI Size control as the client draws it, `"Ideal Ratio"`.
    ui_size: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PanelEntry {
    file: String,
    crop: Rect,
}

/// `pair.json`: what holds for every scene at this client size and UI Size.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairManifest {
    version: u32,
    client_w: u32,
    client_h: u32,
    ui_mode: String,
    settings_display: SettingsDisplay,
    /// The open Graphics panel, evidence that the pair is what its name says.
    /// Never composed into a frame and never read.
    settings_panel: PanelEntry,
}

/// What a scene's HUD and minimap must read as. Required fields have no
/// defaults: a manifest that has lost one must fail to parse.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Truth {
    pub(crate) name: String,
    pub(crate) job: String,
    pub(crate) level: u32,
    pub(crate) exp_text: String,
    pub(crate) exp: i64,
    pub(crate) pct: f64,
    pub(crate) street: String,
    pub(crate) map: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SceneImage {
    id: String,
    /// `hud` or `minimap`.
    kind: String,
    #[serde(default)]
    state: Option<String>,
    /// The frozen hold-out. Exactly one minimap image in the corpus carries it;
    /// see `testdata/README.md`.
    #[serde(default)]
    holdout: bool,
    file: String,
    crop: Rect,
}

/// `scene.json`: one capture moment.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SceneManifest {
    version: u32,
    /// The day the crops entered the corpus.
    added: String,
    /// The client build the crops came from, when known.
    #[serde(default)]
    client: Option<String>,
    /// Set by a person after checking every truth value against the pixels.
    /// An imported fixture starts `false` and fails the gate until then.
    verified: bool,
    truth: Truth,
    images: Vec<SceneImage>,
}

/// What a focused crop must read as. Which fields are required depends on the
/// crop's kind; see [`validate_crop`].
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CropTruth {
    exp_text: Option<String>,
    exp: Option<i64>,
    pct: Option<f64>,
    level: Option<u32>,
    name: Option<String>,
    job: Option<String>,
    street: Option<String>,
    map: Option<String>,
}

/// `<stem>.json` beside a focused crop.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CropManifest {
    version: u32,
    /// `exp`, `level`, `name`, `job` or `map` — also the file-name prefix.
    kind: String,
    added: String,
    #[serde(default)]
    client: Option<String>,
    verified: bool,
    /// The rect the reader consumes, for a crop wider than its readout (a whole
    /// HUD strip whose EXP field is read). Defaults to the whole image.
    #[serde(default)]
    read_crop: Option<Rect>,
    truth: CropTruth,
}

/// The crop kinds, which are also the file-name prefixes.
const CROP_KINDS: [&str; 5] = ["exp", "level", "name", "job", "map"];

/// One image resolved by id, for targeted tests.
#[derive(Clone, Debug)]
pub(crate) struct FixtureImage {
    /// The rect a reader consumes, in the image's own coordinates: a crop's
    /// `readCrop`, otherwise the whole image.
    read: Rect,
}

impl FixtureImage {
    pub(crate) fn read_region(&self) -> Region {
        self.read.region()
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> T {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not a complete {what}: {error}", path.display()))
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()))
                .path()
        })
        .collect();
    entries.sort();
    entries
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn stem(file: &str) -> &str {
    file.rsplit_once('.').map_or(file, |(stem, _)| stem)
}

/// Decode a lossless PNG fixture into straight RGBA, the form the readers take.
pub(crate) fn load_png_rgba(path: &Path) -> (u32, u32, Vec<u8>) {
    let image = image::open(path)
        .unwrap_or_else(|error| panic!("cannot decode {}: {error}", path.display()))
        .to_rgba8();
    (image.width(), image.height(), image.into_raw())
}

/// Resolve an image by id inside a pair. A scene image is found by its id
/// (unique within the pair, or written `<scene>/<id>`); a focused crop by its
/// file stem under `crops/<pair>/`.
pub(crate) fn fixture_image(pair: &str, id: &str) -> (PathBuf, FixtureImage) {
    let crop_png = crops_dir().join(pair).join(format!("{id}.png"));
    if crop_png.is_file() {
        let manifest: CropManifest = read_json(&crop_png.with_extension("json"), "crop sidecar");
        let (w, h, _) = load_png_rgba(&crop_png);
        let whole = Rect {
            x: 0,
            y: 0,
            w: w as i32,
            h: h as i32,
        };
        let image = FixtureImage {
            read: manifest.read_crop.unwrap_or(whole),
        };
        return (crop_png, image);
    }

    let (scene_filter, image_id) = match id.split_once('/') {
        Some((scene, image)) => (Some(scene), image),
        None => (None, id),
    };
    let pair_dir = frames_dir().join(pair);
    let mut found = Vec::new();
    for scene_dir in sorted_entries(&pair_dir)
        .into_iter()
        .filter(|path| path.is_dir())
    {
        let scene = file_name(&scene_dir);
        if scene_filter.is_some_and(|wanted| wanted != scene) {
            continue;
        }
        let manifest: SceneManifest = read_json(&scene_dir.join("scene.json"), "scene manifest");
        for image in manifest
            .images
            .into_iter()
            .filter(|image| image.id == image_id)
        {
            found.push((scene_dir.join(&image.file), image));
        }
    }
    assert!(
        found.len() == 1,
        "{pair}: image id {id:?} matched {} images (crops are looked up by file stem, \
         scene images by id; write <scene>/<id> when an id repeats)",
        found.len()
    );
    let (path, image) = found.remove(0);
    assert!(
        path.is_file(),
        "{pair}: {id:?} points at missing {}",
        path.display()
    );
    let (w, h, _) = load_png_rgba(&path);
    let _ = image;
    let fixture = FixtureImage {
        read: Rect {
            x: 0,
            y: 0,
            w: w as i32,
            h: h as i32,
        },
    };
    (path, fixture)
}

/// Decode an image found by [`fixture_image`] as a frame of its own size.
pub(crate) fn fixture_frame(pair: &str, id: &str) -> Frame {
    let (path, _) = fixture_image(pair, id);
    let (width, height, rgba) = load_png_rgba(&path);
    Frame {
        width,
        height,
        rgba,
    }
}

/// One frame case: a scene's HUD plus one of its minimap states, composed into
/// a client-sized frame.
pub(crate) struct Case {
    /// Display key: `pair/scene/state`.
    pub(crate) key: String,
    /// The pair directory's name, which is the preset/profile key.
    pub(crate) pair_key: String,
    pub(crate) scene: String,
    /// The scene directory holding the crops.
    dir: PathBuf,
    pub(crate) client_w: u32,
    pub(crate) client_h: u32,
    ui_mode: String,
    pub(crate) minimap_state: String,
    minimap_file: String,
    pub(crate) minimap_crop: Rect,
    hud_file: String,
    hud_crop: Rect,
    pub(crate) settings_crop: Rect,
    pub(crate) holdout: bool,
    pub(crate) truth: Truth,
}

impl Case {
    pub(crate) fn mode(&self) -> UiMode {
        UiMode::parse(&self.ui_mode)
            .unwrap_or_else(|| panic!("{}: uiMode {:?} is not a UI Size", self.key, self.ui_mode))
    }

    pub(crate) fn client_size(&self) -> (u32, u32) {
        (self.client_w, self.client_h)
    }

    /// The in-file message prefix: every panic names its case.
    pub(crate) fn label(&self, what: &str) -> String {
        format!("{}: {what}", self.key)
    }
}

/// Every frame case, in a stable order. `MSCOUT_CASE=<prefix>` narrows the run
/// to the cases whose key is or starts with `<prefix>/` (a pair, or a pair and
/// scene), and `MSCOUT_STATE=<state>` to one minimap state.
pub(crate) fn load_cases() -> Vec<Case> {
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

/// Load and validate every frame case under `<root>/frames`.
///
/// Validation happens here, not in a separate test, so no test can read a
/// case the hermetic checks would reject.
pub(crate) fn load_cases_in(root: &Path) -> Vec<Case> {
    let frames = root.join("frames");
    assert!(
        frames.is_dir(),
        "the fixture corpus is missing: {} does not exist",
        frames.display()
    );

    let mut cases = Vec::new();
    for pair_dir in sorted_entries(&frames) {
        let pair_key = file_name(&pair_dir);
        assert!(
            pair_dir.is_dir(),
            "frames/{pair_key}: only WxH@mode directories belong in frames/"
        );
        let pair_path = pair_dir.join("pair.json");
        assert!(
            pair_path.is_file(),
            "{pair_key}: no pair.json — every pair directory needs one"
        );
        let pair: PairManifest = read_json(&pair_path, "pair manifest");
        validate_pair(&pair_key, &pair_dir, &pair);

        for entry in sorted_entries(&pair_dir) {
            let name = file_name(&entry);
            if entry.is_dir() {
                cases.extend(load_scene(&pair_key, &pair, &entry));
            } else {
                assert!(
                    name == "pair.json" || name == pair.settings_panel.file,
                    "{pair_key}: {name} is not indexed — a pair directory holds pair.json, \
                     the settings panel and scene directories"
                );
            }
        }
    }

    assert!(
        !cases.is_empty(),
        "{} has no fixtures — the regression gate would pass without checking anything",
        frames.display()
    );
    cases
}

/// The pair manifest against its directory name and its settings panel.
fn validate_pair(pair_key: &str, dir: &Path, pair: &PairManifest) {
    assert_eq!(
        pair.version, VERSION,
        "{pair_key}: pair.json version must be {VERSION}"
    );
    let mode = UiMode::parse(&pair.ui_mode)
        .unwrap_or_else(|| panic!("{pair_key}: uiMode {:?} is not a UI Size", pair.ui_mode));

    // The directory name is the preset/profile key, so a misnamed pair would
    // silently test the wrong geometry.
    let expected_key = format!("{}x{}{}", pair.client_w, pair.client_h, mode.suffix());
    assert_eq!(
        pair_key, expected_key,
        "pair directory {pair_key:?} must be named like its preset ({expected_key})"
    );

    // The client's own Graphics panel has to agree, or the uiMode (and so the
    // preset a read uses) is wrong.
    assert_eq!(
        pair.settings_display.resolution,
        format!("{} x {}", pair.client_w, pair.client_h),
        "{pair_key}: settingsDisplay.resolution disagrees with clientW/clientH"
    );
    assert_eq!(
        pair.settings_display.ui_size,
        mode.label(),
        "{pair_key}: settingsDisplay.uiSize should name uiMode {:?}",
        pair.ui_mode
    );

    let panel = &pair.settings_panel;
    let path = dir.join(&panel.file);
    assert!(
        path.is_file(),
        "{pair_key}: settings panel missing at {}",
        path.display()
    );
    let (w, h, rgba) = load_png_rgba(&path);
    assert_eq!(
        (w as i32, h as i32),
        (panel.crop.w, panel.crop.h),
        "{pair_key}: {} is {w}x{h} but its crop says {}x{}",
        panel.file,
        panel.crop.w,
        panel.crop.h
    );
    assert!(
        panel.crop.fits(pair.client_w, pair.client_h),
        "{pair_key}: the settings panel crop is outside the client"
    );
    let colours: BTreeSet<[u8; 3]> = rgba
        .chunks_exact(4)
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    assert!(
        colours.len() > 1,
        "{pair_key}: {} is one flat colour — it cannot be evidence",
        panel.file
    );
}

/// One scene's cases, after checking its manifest and every file in it.
fn load_scene(pair_key: &str, pair: &PairManifest, dir: &Path) -> Vec<Case> {
    let scene = file_name(dir);
    let key = format!("{pair_key}/{scene}");
    let manifest_path = dir.join("scene.json");
    assert!(manifest_path.is_file(), "{key}: no scene.json");
    let manifest: SceneManifest = read_json(&manifest_path, "scene manifest");
    assert_eq!(
        manifest.version, VERSION,
        "{key}: scene.json version must be {VERSION}"
    );
    assert!(
        manifest.verified,
        "{key}: truth is not verified — check every value against the pixels, then set \
         \"verified\": true"
    );
    assert!(!manifest.added.is_empty(), "{key}: added is empty");
    let _ = &manifest.client;
    validate_truth_readout(
        &key,
        &manifest.truth.exp_text,
        manifest.truth.exp,
        manifest.truth.pct,
    );

    let mut ids = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut hud: Option<&SceneImage> = None;
    let mut minimaps = Vec::new();
    for image in &manifest.images {
        assert!(
            ids.insert(image.id.clone()),
            "{key}: duplicate image id {:?}",
            image.id
        );
        assert!(
            files.insert(image.file.clone()),
            "{key}: duplicate file {:?}",
            image.file
        );
        assert_eq!(
            stem(&image.file),
            image.id,
            "{key}: image id {:?} must be its file stem",
            image.id
        );
        let path = dir.join(&image.file);
        assert!(path.is_file(), "{key}: {} is missing", image.file);
        let (w, h, _) = load_png_rgba(&path);
        assert_eq!(
            (w as i32, h as i32),
            (image.crop.w, image.crop.h),
            "{key}: {} is {w}x{h} but its crop says {}x{}",
            image.file,
            image.crop.w,
            image.crop.h
        );
        assert!(
            image.crop.fits(pair.client_w, pair.client_h),
            "{key}: {} crop {:?} is outside the {}x{} client",
            image.file,
            image.crop,
            pair.client_w,
            pair.client_h
        );
        match image.kind.as_str() {
            "hud" => {
                assert!(
                    !image.holdout,
                    "{key}: {:?} is a hud — only a minimap state can be the frozen hold-out",
                    image.id
                );
                assert!(hud.is_none(), "{key}: more than one hud image");
                hud = Some(image);
            }
            "minimap" => {
                let state = image
                    .state
                    .as_deref()
                    .unwrap_or_else(|| panic!("{key}: minimap {:?} has no state", image.id));
                expected_state(state);
                minimaps.push(image);
            }
            other => panic!("{key}: image {:?} has unknown kind {other:?}", image.id),
        }
    }
    let hud = hud.unwrap_or_else(|| panic!("{key}: scene.json needs a hud image"));
    assert!(
        !minimaps.is_empty(),
        "{key}: scene.json needs at least one minimap image"
    );

    for entry in sorted_entries(dir) {
        let name = file_name(&entry);
        assert!(
            name == "scene.json" || files.contains(&name),
            "{key}: {name} is not indexed in scene.json"
        );
    }

    minimaps
        .into_iter()
        .map(|minimap| {
            let state = minimap.state.clone().unwrap_or_default();
            Case {
                key: format!("{key}/{state}"),
                pair_key: pair_key.to_string(),
                scene: scene.clone(),
                dir: dir.to_path_buf(),
                client_w: pair.client_w,
                client_h: pair.client_h,
                ui_mode: pair.ui_mode.clone(),
                minimap_state: state,
                minimap_file: minimap.file.clone(),
                minimap_crop: minimap.crop,
                hud_file: hud.file.clone(),
                hud_crop: hud.crop,
                settings_crop: pair.settings_panel.crop,
                holdout: minimap.holdout,
                truth: manifest.truth.clone(),
            }
        })
        .collect()
}

/// An EXP readout's text has to say the same as its numbers.
fn validate_truth_readout(key: &str, text: &str, exp: i64, pct: f64) {
    let parsed = ocr::parse_exp_text(text)
        .unwrap_or_else(|| panic!("{key}: expText {text:?} is not a readout"));
    assert_eq!(parsed, (exp, pct), "{key}: expText disagrees with exp/pct");
}

/// Every focused crop: a PNG and its sidecar, both present, the kind named by
/// the file prefix, the truth that kind needs, and verified.
fn validate_crops(root: &Path) -> usize {
    let crops = root.join("crops");
    if !crops.is_dir() {
        return 0;
    }
    let mut count = 0;
    for pair_dir in sorted_entries(&crops) {
        let pair_key = file_name(&pair_dir);
        assert!(
            pair_dir.is_dir(),
            "crops/{pair_key}: only WxH@mode directories belong here"
        );
        assert!(
            root.join("frames").join(&pair_key).is_dir(),
            "crops/{pair_key}: no frames/{pair_key} — crops belong to a known pair"
        );
        for entry in sorted_entries(&pair_dir) {
            let name = file_name(&entry);
            match name.rsplit_once('.') {
                Some((stem, "png")) => {
                    validate_crop(&format!("crops/{pair_key}/{stem}"), &entry);
                    count += 1;
                }
                Some((stem, "json")) => assert!(
                    pair_dir.join(format!("{stem}.png")).is_file(),
                    "crops/{pair_key}/{name}: sidecar without {stem}.png"
                ),
                _ => {
                    panic!("crops/{pair_key}/{name}: only PNGs and their JSON sidecars belong here")
                }
            }
        }
    }
    count
}

fn validate_crop(key: &str, png: &Path) {
    let sidecar = png.with_extension("json");
    assert!(sidecar.is_file(), "{key}: no {key}.json sidecar");
    let manifest: CropManifest = read_json(&sidecar, "crop sidecar");
    assert_eq!(
        manifest.version, VERSION,
        "{key}: sidecar version must be {VERSION}"
    );
    assert!(
        manifest.verified,
        "{key}: truth is not verified — check it against the pixels, then set \"verified\": true"
    );
    assert!(!manifest.added.is_empty(), "{key}: added is empty");
    let _ = &manifest.client;
    assert!(
        CROP_KINDS.contains(&manifest.kind.as_str()),
        "{key}: kind {:?} is not one of {CROP_KINDS:?}",
        manifest.kind
    );
    let stem = file_name(png);
    assert!(
        stem.starts_with(&format!("{}-", manifest.kind)),
        "{key}: a {} crop's file name starts with \"{}-\"",
        manifest.kind,
        manifest.kind
    );

    let (w, h, _) = load_png_rgba(png);
    if let Some(read) = manifest.read_crop {
        assert!(
            read.fits(w, h),
            "{key}: readCrop {read:?} is outside the {w}x{h} image"
        );
    }

    let truth = &manifest.truth;
    match manifest.kind.as_str() {
        "exp" => {
            let (Some(text), Some(exp), Some(pct)) = (&truth.exp_text, truth.exp, truth.pct) else {
                panic!("{key}: an exp crop needs expText, exp and pct");
            };
            validate_truth_readout(key, text, exp, pct);
        }
        "level" => assert!(truth.level.is_some(), "{key}: a level crop needs level"),
        "name" => assert!(truth.name.is_some(), "{key}: a name crop needs name"),
        "job" => assert!(truth.job.is_some(), "{key}: a job crop needs job"),
        "map" => assert!(
            truth.street.is_some() && truth.map.is_some(),
            "{key}: a map crop needs street and map"
        ),
        _ => unreachable!("kind checked above"),
    }
}

/// Build the frame a capture would have produced: `clientW` x `clientH`, black,
/// with the scene's minimap and HUD crops pasted where they were cut from.
///
/// The settings panel is deliberately not pasted: it is evidence, never input.
pub(crate) fn compose_frame(case: &Case) -> Frame {
    let (width, height) = case.client_size();
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    let mut frame = Frame {
        width,
        height,
        rgba,
    };
    paste(&mut frame, case, &case.minimap_file, case.minimap_crop);
    paste(&mut frame, case, &case.hud_file, case.hud_crop);
    frame
}

fn paste(frame: &mut Frame, case: &Case, file: &str, rect: Rect) {
    let (w, h, rgba) = load_png_rgba(&case.dir.join(file));
    for y in 0..h {
        let src = (y * w) as usize * 4;
        let dst = (((rect.y as u32 + y) * frame.width + rect.x as u32) * 4) as usize;
        let len = (w * 4) as usize;
        frame.rgba[dst..dst + len].copy_from_slice(&rgba[src..src + len]);
    }
}

/// The reader's expectation for a minimap `state`.
pub(crate) fn expected_state(state: &str) -> Option<MinimapState> {
    match state {
        "expanded" => Some(MinimapState::Expanded),
        "titlebar" => Some(MinimapState::TitleBarOnly),
        "folded" => Some(MinimapState::Folded),
        // No widget on screen at all: nothing to detect.
        "hidden" => None,
        other => panic!("unknown minimap state {other:?} — expanded, titlebar, folded or hidden"),
    }
}

// ---------------------------------------------------------------------------
// Import: a saved diagnostic becomes focused crops.
// ---------------------------------------------------------------------------

/// The pieces of a diagnostic (`data/diagnostics/read-*.json`) the importer uses.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostic {
    profile: String,
    calibration: crate::settings::Calibration,
    readings: serde_json::Value,
    hud_strip: Option<HudStrip>,
}

#[derive(Deserialize)]
struct HudStrip {
    region: Region,
    rgba: String,
}

/// The crops an import writes: for each box, its kind, the file stem and the
/// truth MScout read there.
fn import_plan(diagnostic: &Diagnostic) -> Vec<(&'static str, Region, String, CropTruth)> {
    let readings = &diagnostic.readings;
    let text = |field: &str| readings.get(field).and_then(serde_json::Value::as_str);
    let slug = |value: &str| {
        value
            .to_lowercase()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>()
    };
    let calibration = &diagnostic.calibration;
    let mut plan = Vec::new();
    if let (Some(region), Some(name)) = (calibration.name, text("name")) {
        let truth = CropTruth {
            name: Some(name.to_string()),
            ..CropTruth::default()
        };
        plan.push(("name", region, format!("name-{}", slug(name)), truth));
    }
    if let (Some(region), Some(job)) = (calibration.job, text("job")) {
        let truth = CropTruth {
            job: Some(job.to_string()),
            ..CropTruth::default()
        };
        plan.push(("job", region, format!("job-{}", slug(job)), truth));
    }
    let exp = readings.get("exp").and_then(serde_json::Value::as_i64);
    let pct = readings.get("pct").and_then(serde_json::Value::as_f64);
    if let (Some(region), Some(exp_text), Some(exp), Some(pct)) =
        (calibration.exp, text("expText"), exp, pct)
    {
        let truth = CropTruth {
            exp_text: Some(exp_text.to_string()),
            exp: Some(exp),
            pct: Some(pct),
            ..CropTruth::default()
        };
        plan.push(("exp", region, format!("exp-{exp}"), truth));
    }
    let level = readings
        .get("lv")
        .and_then(serde_json::Value::as_u64)
        .filter(|lv| *lv > 0);
    if let (Some(region), Some(level)) = (calibration.level, level) {
        let truth = CropTruth {
            level: Some(level as u32),
            ..CropTruth::default()
        };
        plan.push(("level", region, format!("level-{level}"), truth));
    }
    plan
}

/// Cut `region` (frame coordinates) out of the HUD strip, losslessly.
fn cut_from_strip(strip: &HudStrip, region: Region) -> Option<image::RgbaImage> {
    let rgba = crate::settings::from_hex(&strip.rgba)?;
    let origin = strip.region;
    let (x0, y0) = (region.x - origin.x, region.y - origin.y);
    if x0 < 0 || y0 < 0 || x0 + region.w > origin.w || y0 + region.h > origin.h {
        return None;
    }
    let mut out = image::RgbaImage::new(region.w as u32, region.h as u32);
    for y in 0..region.h {
        for x in 0..region.w {
            let index = (((y0 + y) * origin.w + x0 + x) * 4) as usize;
            out.put_pixel(
                x as u32,
                y as u32,
                image::Rgba([
                    rgba[index],
                    rgba[index + 1],
                    rgba[index + 2],
                    rgba[index + 3],
                ]),
            );
        }
    }
    Some(out)
}

/// Turn a saved diagnostic into focused crops under `crops/<pair>/`.
///
/// `MSCOUT_DIAGNOSTIC=<path to read-*.json>`. Each box with a reading becomes
/// `<kind>-<value>.png` plus a sidecar holding what MScout **read**, marked
/// `"verified": false`. The gate refuses unverified truth, so the import is not
/// done until a person has checked every value against the pixels, corrected
/// the misread one (that is usually why the diagnostic was saved), renamed the
/// file if the value in its name was wrong, and set `"verified": true`. An
/// existing file is never overwritten.
#[test]
#[ignore = "tool: set MSCOUT_DIAGNOSTIC to a diagnostic to import its crops"]
fn import_diagnostic() {
    let path = std::env::var("MSCOUT_DIAGNOSTIC").expect("set MSCOUT_DIAGNOSTIC");
    let diagnostic: Diagnostic = read_json(Path::new(&path), "diagnostic");
    let strip = diagnostic
        .hud_strip
        .as_ref()
        .expect("the diagnostic has no hudStrip — save it again with a current build");
    let dir = crops_dir().join(&diagnostic.profile);
    assert!(
        frames_dir().join(&diagnostic.profile).is_dir(),
        "no frames/{} pair: add the pair before its crops",
        diagnostic.profile
    );
    std::fs::create_dir_all(&dir).expect("create the crops directory");
    let added = chrono::Local::now().format("%Y-%m-%d").to_string();
    for (kind, region, stem, truth) in import_plan(&diagnostic) {
        let png = dir.join(format!("{stem}.png"));
        if png.exists() {
            println!("skipped {}: already exists", png.display());
            continue;
        }
        let Some(image) = cut_from_strip(strip, region) else {
            println!("skipped {stem}: {region:?} is outside the HUD strip");
            continue;
        };
        image.save(&png).expect("write the crop");
        let sidecar = serde_json::json!({
            "version": VERSION,
            "kind": kind,
            "added": added,
            "verified": false,
            "truth": serde_json::to_value(TruthOut::from(&truth)).expect("truth"),
        });
        std::fs::write(
            png.with_extension("json"),
            serde_json::to_string_pretty(&sidecar).expect("sidecar") + "\n",
        )
        .expect("write the sidecar");
        println!(
            "wrote {} (verify it, then set \"verified\": true)",
            png.display()
        );
    }
}

/// [`CropTruth`] as written to a sidecar: only the fields that are set.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TruthOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    exp_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<String>,
}

impl From<&CropTruth> for TruthOut {
    fn from(truth: &CropTruth) -> Self {
        Self {
            exp_text: truth.exp_text.clone(),
            exp: truth.exp,
            pct: truth.pct,
            level: truth.level,
            name: truth.name.clone(),
            job: truth.job.clone(),
        }
    }
}

/// The import plan and the lossless cut, on a synthetic diagnostic.
#[test]
fn an_import_cuts_each_read_box_from_the_hud_strip() {
    let strip_region = Region {
        x: 100,
        y: 50,
        w: 4,
        h: 3,
    };
    let rgba: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
    let diagnostic: Diagnostic = serde_json::from_value(serde_json::json!({
        "profile": "1920x1080@ideal",
        "calibration": {
            "name": { "x": 101, "y": 51, "w": 2, "h": 1 },
            "level": { "x": 100, "y": 50, "w": 1, "h": 1 }
        },
        "readings": { "name": "Euroveln", "lv": 2, "job": null },
        "hudStrip": { "region": strip_region, "rgba": crate::settings::to_hex(&rgba) }
    }))
    .expect("diagnostic");
    let plan = import_plan(&diagnostic);
    let stems: Vec<&str> = plan.iter().map(|(_, _, stem, _)| stem.as_str()).collect();
    assert_eq!(stems, ["name-euroveln", "level-2"]);

    let strip = diagnostic.hud_strip.as_ref().expect("strip");
    let name = cut_from_strip(strip, plan[0].1).expect("inside the strip");
    // Row 1, columns 1-2 of a 4-wide strip: pixels 5 and 6.
    assert_eq!(name.get_pixel(0, 0).0, [20, 21, 22, 23]);
    assert_eq!(name.get_pixel(1, 0).0, [24, 25, 26, 27]);
    assert!(cut_from_strip(
        strip,
        Region {
            x: 99,
            y: 50,
            w: 2,
            h: 1
        }
    )
    .is_none());
}

// ---------------------------------------------------------------------------
// Hermetic tier: needs the repository, nothing else.
// ---------------------------------------------------------------------------

/// The corpus itself: present, complete, self-consistent and in bounds. Loading
/// validates every pair and scene; the crops are checked here too.
#[test]
fn fixture_corpus_is_complete() {
    let cases = load_cases_in(&fixtures_dir());
    for case in &cases {
        println!(
            "{}: {}x{} {}",
            case.key,
            case.client_w,
            case.client_h,
            case.mode().id()
        );
    }
    let crops = validate_crops(&fixtures_dir());
    println!("{} frame cases, {crops} focused crops", cases.len());
}

/// Cases the minimap detector is known to misname, as `<pair>/<scene>/<state>`.
/// Detection runs on the 1x picture (`native::prepare`), so none are known
/// today; a listed case that starts passing fails the test until it is removed.
const MINIMAP_STATE_KNOWN_GAPS: &[&str] = &[];

/// The minimap state detector must name the state each frame was captured in.
#[test]
fn minimap_state_matches_ground_truth() {
    let mut wrong = Vec::new();
    let mut fixed = Vec::new();
    for case in load_cases() {
        // The app detects the minimap on the 1x picture.
        let frame = crate::native::prepare(compose_frame(&case), case.mode()).frame;
        let detected = ocr::detect_minimap(&frame).map(|minimap| minimap.state);
        let id = case.key.clone();
        let known = MINIMAP_STATE_KNOWN_GAPS.contains(&id.as_str());
        match (detected == expected_state(&case.minimap_state), known) {
            (false, false) => wrong.push(format!("{id}: detected {detected:?}")),
            (true, true) => fixed.push(id),
            _ => {}
        }
    }
    assert!(wrong.is_empty(), "detect_minimap disagreed: {wrong:#?}");
    assert!(
        fixed.is_empty(),
        "now detected, remove from MINIMAP_STATE_KNOWN_GAPS: {fixed:#?}"
    );
}

/// The settings panel is evidence, not input: its rect stays black in the
/// composed frame.
#[test]
fn settings_panel_is_evidence_only() {
    for case in load_cases() {
        let frame = compose_frame(&case);
        let rect = case.settings_crop;
        for y in rect.y as u32..(rect.y + rect.h) as u32 {
            for x in rect.x as u32..(rect.x + rect.w) as u32 {
                let index = ((y * frame.width + x) * 4) as usize;
                let pixel = &frame.rgba[index..index + 3];
                assert_eq!(
                    pixel,
                    [0, 0, 0],
                    "{}",
                    case.label(&format!("the settings panel was composed in at {x},{y}"))
                );
            }
        }
    }
}

/// Exactly one minimap state in the whole corpus is the frozen hold-out. The
/// manifest flag is the one record of which; a second would make "never tuned
/// against" meaningless and none would silently free it for tuning.
///
/// Checks the **unfiltered** corpus, so `MSCOUT_CASE`/`MSCOUT_STATE` cannot
/// narrow the run into a pass.
#[test]
fn exactly_one_frozen_holdout() {
    let frozen: Vec<Case> = load_cases_in(&fixtures_dir())
        .into_iter()
        .filter(|case| case.holdout)
        .collect();
    assert_eq!(
        frozen.len(),
        1,
        "exactly one minimap state must be the frozen hold-out; found {}: {}",
        frozen.len(),
        frozen
            .iter()
            .map(|case| case.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let case = &frozen[0];
    assert!(
        matches!(
            case.minimap_state.as_str(),
            "expanded" | "folded" | "titlebar"
        ),
        "{}: the hold-out must be a read minimap state",
        case.key
    );
    println!("frozen hold-out: {}", case.key);
}

/// A crop rect that leaves the client is rejected: `crop_gray` silently clamps,
/// which would make a bad manifest read as a shifted image instead of an error.
#[test]
fn crop_bounds_check_rejects_rects_outside_the_client() {
    let (w, h) = (1920u32, 1080u32);
    let rect = |x, y, rw, rh| Rect { x, y, w: rw, h: rh };
    assert!(rect(19, 19, 226, 183).fits(w, h));
    assert!(rect(1910, 1070, 10, 10).fits(w, h));
    for bad in [
        rect(-1, 0, 10, 10),
        rect(0, -1, 10, 10),
        rect(1911, 0, 10, 10),
        rect(0, 1071, 10, 10),
        rect(0, 0, 0, 10),
        rect(0, 0, 10, 0),
    ] {
        assert!(!bad.fits(w, h), "{bad:?} must not fit inside {w}x{h}");
    }
}

/// A throwaway corpus root under the system temp directory, removed on drop so
/// the `should_panic` guards clean up while unwinding.
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
        std::fs::create_dir_all(dir.join("frames")).expect("create the temporary corpus root");
        Self(dir)
    }

    /// Copy one real pair into the temporary corpus, so a test can break one
    /// thing about an otherwise valid fixture.
    fn with_pair(tag: &str, pair: &str) -> (Self, PathBuf) {
        let root = Self::new(tag);
        let from = frames_dir().join(pair);
        let to = root.0.join("frames").join(pair);
        copy_tree(&from, &to);
        (root, to)
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create directory");
    for entry in sorted_entries(from) {
        let target = to.join(file_name(&entry));
        if entry.is_dir() {
            copy_tree(&entry, &target);
        } else {
            std::fs::copy(&entry, &target).expect("copy fixture file");
        }
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The only scene directory of a copied pair.
fn only_scene(pair_dir: &Path) -> PathBuf {
    sorted_entries(pair_dir)
        .into_iter()
        .find(|path| path.is_dir())
        .expect("a scene directory")
}

/// Rewrite one scene.json through a JSON edit.
fn edit_scene(scene: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = scene.join("scene.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    edit(&mut value);
    std::fs::write(&path, value.to_string()).expect("write");
}

/// A corpus with no cases is a gate that cannot fail.
#[test]
#[should_panic(expected = "has no fixtures")]
fn an_empty_corpus_panics() {
    let root = TempRoot::new("empty");
    load_cases_in(&root.0);
}

/// A directory of images with no manifest is a fixture that quietly stopped
/// being tested.
#[test]
#[should_panic(expected = "no pair.json")]
fn a_pair_directory_without_a_manifest_panics() {
    let root = TempRoot::new("no-manifest");
    let pair = root.0.join("frames").join("1920x1080@ideal");
    std::fs::create_dir_all(&pair).expect("create the pair directory");
    std::fs::write(pair.join("hud-main.png"), b"not really a png").expect("write a stray image");
    load_cases_in(&root.0);
}

/// A second scene at the same size is a second set of cases with its own truth.
#[test]
fn a_pair_can_hold_more_than_one_scene() {
    let (root, pair) = TempRoot::with_pair("two-scenes", "1920x1080@ideal");
    let before = scene_names(&load_cases_in(&root.0));
    let scene = only_scene(&pair);
    copy_tree(&scene, &pair.join("second-scene"));
    edit_scene(&pair.join("second-scene"), |value| {
        value["images"]
            .as_array_mut()
            .expect("images")
            .iter_mut()
            .for_each(|image| image["holdout"] = serde_json::Value::Bool(false));
    });
    let after = scene_names(&load_cases_in(&root.0));
    assert_eq!(after.len(), before.len() + 1, "{after:?}");
    assert!(after.contains("second-scene"), "{after:?}");
}

fn scene_names(cases: &[Case]) -> BTreeSet<String> {
    cases.iter().map(|case| case.scene.clone()).collect()
}

/// Truth nobody has checked (an imported fixture) fails the gate.
#[test]
#[should_panic(expected = "not verified")]
fn unverified_truth_panics() {
    let (root, pair) = TempRoot::with_pair("unverified", "1920x1080@ideal");
    edit_scene(&only_scene(&pair), |value| {
        value["verified"] = serde_json::Value::Bool(false);
    });
    load_cases_in(&root.0);
}

/// The reservation names a minimap state; a flag on a HUD would reserve nothing.
#[test]
#[should_panic(expected = "only a minimap state can be the frozen hold-out")]
fn a_holdout_flag_on_a_hud_panics() {
    let (root, pair) = TempRoot::with_pair("holdout-hud", "1920x1080@ideal");
    edit_scene(&only_scene(&pair), |value| {
        for image in value["images"].as_array_mut().expect("images") {
            if image["kind"] == "hud" {
                image["holdout"] = serde_json::Value::Bool(true);
            }
        }
    });
    load_cases_in(&root.0);
}

/// A PNG nothing indexes is a fixture nothing reads.
#[test]
#[should_panic(expected = "is not indexed in scene.json")]
fn an_unindexed_png_panics() {
    let (root, pair) = TempRoot::with_pair("unindexed", "1920x1080@ideal");
    let scene = only_scene(&pair);
    std::fs::copy(scene.join("hud-main.png"), scene.join("stray.png")).expect("copy");
    load_cases_in(&root.0);
}
