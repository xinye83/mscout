use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::settings::{self, Atlas, Calibration, Identities, Template, UiMode};

/// One calibration set, scoped to a client size *and* a UI Size option.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub client_w: u32,
    pub client_h: u32,
    #[serde(default)]
    pub ui_mode: UiMode,
    #[serde(default)]
    pub calibration: Calibration,
    #[serde(default)]
    pub atlas: Atlas,
    /// Glyphs of the minimap label font. Separate from `atlas` because the label
    /// is a different size and rendering from the EXP readout.
    #[serde(default)]
    pub map_atlas: Atlas,
    /// Glyphs of the level readout. Separate from `atlas` for the same reason as
    /// `map_atlas`: the level is drawn at its own size, and sharing one atlas made
    /// the two readers compete for the same characters, so learning a level glyph
    /// could change how the EXP was read and vice versa.
    #[serde(default)]
    pub level_atlas: Atlas,
    /// Name/job text atlas, seeded from the Windows Arial face. The taught
    /// `identities` fingerprints remain the exact fallback.
    #[serde(default)]
    pub identity_atlas: Atlas,
    #[serde(default)]
    pub maps: Vec<Template>,
    /// Taught street lines (the top line of the minimap label). Separate from `maps`
    /// because a street is shared by several maps and is matched on its own.
    #[serde(default)]
    pub streets: Vec<Template>,
    #[serde(default)]
    pub identities: Identities,
}

impl Profile {
    pub fn new_for_mode(client_w: u32, client_h: u32, ui_mode: UiMode) -> Self {
        Self {
            client_w,
            client_h,
            ui_mode,
            ..Default::default()
        }
    }

    /// `1920x1080`, or `1920x1080@ideal` for a non-default UI Size.
    pub fn key(&self) -> String {
        format!(
            "{}x{}{}",
            self.client_w,
            self.client_h,
            self.ui_mode.suffix()
        )
    }

    pub fn is_calibrated(&self) -> bool {
        self.calibration.is_ready()
    }

    pub fn has_glyphs(&self) -> bool {
        !self.atlas.glyphs.is_empty()
    }

    pub fn summary(&self) -> ProfileSummary {
        let glyph_size = self.atlas.common_glyph_size();

        ProfileSummary {
            key: self.key(),
            client_w: self.client_w,
            client_h: self.client_h,
            ui_mode: self.ui_mode.id().to_string(),
            calibrated: self.is_calibrated(),
            glyphs: self.atlas.glyphs.len(),
            maps: self.maps.len(),
            name: self.identities.name.as_ref().map(|t| t.label.clone()),
            job: self.identities.job.as_ref().map(|t| t.label.clone()),
            source: self.atlas.source.clone(),
            glyph_size,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub key: String,
    pub client_w: u32,
    pub client_h: u32,
    pub ui_mode: String,
    pub calibrated: bool,
    pub glyphs: usize,
    pub maps: usize,
    pub name: Option<String>,
    pub job: Option<String>,
    pub source: String,
    pub glyph_size: String,
}

struct Active {
    key: String,
    profile: Profile,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
static DISK_LOCK: Mutex<()> = Mutex::new(());

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn dir() -> Result<PathBuf, String> {
    Ok(crate::data_dir()?.join("profiles"))
}

pub fn path_for(client_w: u32, client_h: u32, ui_mode: UiMode) -> Result<PathBuf, String> {
    Ok(dir()?.join(format!("{client_w}x{client_h}{}.json", ui_mode.suffix())))
}

fn parse_profile(text: &str) -> Option<Profile> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let profile: Profile = serde_json::from_str(text).ok()?;
    if profile.client_w == 0 || profile.client_h == 0 {
        return None;
    }
    Some(profile)
}

/// The read half of [`load_for_mode`]: parse a profile file and keep it only if
/// it is for `client_w`×`client_h`, with the *file name*'s UI mode forced on it.
///
/// Split out from the disk half so the upgrade fixtures can exercise the code
/// that actually runs on load without needing a data root.
fn parse_for_mode(text: &str, client_w: u32, client_h: u32, ui_mode: UiMode) -> Option<Profile> {
    let mut profile = parse_profile(text)?;
    if profile.client_w != client_w || profile.client_h != client_h {
        return None;
    }
    // The filename is authoritative: sets written before UI Size was tracked
    // carry no `uiMode` field but still belong to their file's mode.
    profile.ui_mode = ui_mode;
    Some(profile)
}

/// The set for the client size under the UI Size currently selected.
pub fn load(client_w: u32, client_h: u32) -> Option<Profile> {
    load_for_mode(client_w, client_h, settings::ui_mode())
}

pub fn load_for_mode(client_w: u32, client_h: u32, ui_mode: UiMode) -> Option<Profile> {
    let path = path_for(client_w, client_h, ui_mode).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    parse_for_mode(&text, client_w, client_h, ui_mode)
}

/// Load the saved set for a pair, for callers that are about to write one.
///
/// `Ok(None)` means there is no file. A file that exists but does not parse as a
/// profile for this pair (a torn write, a newer schema, a hand edit) is renamed
/// aside to `<name>.json.bad-<UTC time>` before `Ok(None)` is returned, so the
/// caller's fresh profile never overwrites the only copy of the player's taught
/// maps, names and atlases. A file that cannot be read at all (locked, denied)
/// is an error: nothing is replaced.
fn load_or_quarantine(
    client_w: u32,
    client_h: u32,
    ui_mode: UiMode,
) -> Result<Option<Profile>, String> {
    let path = path_for(client_w, client_h, ui_mode)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if let Some(profile) = parse_for_mode(&text, client_w, client_h, ui_mode) {
        return Ok(Some(profile));
    }
    let backup = quarantine(&path)?;
    crate::calibration::log(&format!(
        "profile {} could not be read; kept it as {} and started a new one",
        path.display(),
        backup.display()
    ));
    Ok(None)
}

/// Rename an unreadable profile out of the way. The new name no longer ends in
/// `.json`, so [`list`] ignores it, but the player's data is still on disk.
fn quarantine(path: &std::path::Path) -> Result<PathBuf, String> {
    crate::fsutil::move_aside(path)
        .map_err(|error| format!("cannot move aside {}: {error}", path.display()))
}

pub fn save(profile: &Profile) -> Result<(), String> {
    let _guard = lock(&DISK_LOCK);
    let dir = dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(profile).map_err(|e| e.to_string())?;
    crate::fsutil::write_atomic(
        &path_for(profile.client_w, profile.client_h, profile.ui_mode)?,
        text.as_bytes(),
    )
    .map_err(|e| e.to_string())
}

pub fn list() -> Vec<ProfileSummary> {
    let Ok(dir) = dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut out: Vec<ProfileSummary> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path()).ok()?;
            parse_profile(&text).map(|profile| profile.summary())
        })
        .collect();

    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// The profile currently in use, if any.
pub fn active() -> Option<Profile> {
    lock(&ACTIVE).as_ref().map(|active| active.profile.clone())
}

pub fn active_key() -> Option<String> {
    lock(&ACTIVE).as_ref().map(|active| active.key.clone())
}

/// Make the profile for this client size and UI Size active, creating an empty
/// one when the pair has never been seen. Returns the profile now active.
pub fn activate(client_w: u32, client_h: u32) -> Result<Profile, String> {
    let ui_mode = settings::ui_mode();
    let key = format!("{client_w}x{client_h}{}", ui_mode.suffix());
    let mut active = lock(&ACTIVE);
    if let Some(current) = active.as_ref() {
        if current.key == key {
            return Ok(current.profile.clone());
        }
    }

    let profile = match load_or_quarantine(client_w, client_h, ui_mode)? {
        Some(profile) => profile,
        // A known size starts from its shipped preset and is saved straight
        // away, so it reads without any drawing and shows up as a normal,
        // editable profile. Deleting it just restores this starting point.
        None => match crate::preset::for_size(client_w, client_h, ui_mode) {
            Some(profile) => {
                let _ = save(&profile);
                profile
            }
            None => Profile::new_for_mode(client_w, client_h, ui_mode),
        },
    };
    *active = Some(Active {
        key,
        profile: profile.clone(),
    });
    Ok(profile)
}

/// Load-modify-save the active profile.
pub fn update<F>(apply: F) -> Result<Profile, String>
where
    F: FnOnce(&mut Profile),
{
    let mut active = lock(&ACTIVE);
    let current = active
        .as_mut()
        .ok_or("no active profile — attach the game window first")?;
    apply(&mut current.profile);
    save(&current.profile)?;
    Ok(current.profile.clone())
}

/// Delete the saved set for a client size and UI Size. If that set is the active
/// one, drop the active pointer so the engine re-derives it (as an empty
/// profile) on the next tick. Missing files count as already removed.
pub fn remove(client_w: u32, client_h: u32, ui_mode: UiMode) -> Result<(), String> {
    let key = format!("{client_w}x{client_h}{}", ui_mode.suffix());

    {
        let mut active = lock(&ACTIVE);
        if active.as_ref().map(|current| current.key.as_str()) == Some(key.as_str()) {
            *active = None;
        }
    }

    match std::fs::remove_file(path_for(client_w, client_h, ui_mode)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Move the pre-profile `settings.json` sections into a size-scoped profile.
/// Returns the key of the profile that was created, if any.
pub fn migrate_legacy() -> Option<String> {
    let raw = settings::read_raw();

    let calibration: Calibration = raw
        .get("calibration")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let (Some(client_w), Some(client_h)) = (calibration.client_w, calibration.client_h) else {
        return None;
    };

    let atlas: Atlas = raw
        .get("atlas")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let maps: Vec<Template> = raw
        .get("maps")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let identities: Identities = raw
        .get("identities")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();

    let legacy_empty = calibration.exp.is_none()
        && calibration.level.is_none()
        && atlas.glyphs.is_empty()
        && maps.is_empty()
        && identities.name.is_none()
        && identities.job.is_none();
    if legacy_empty {
        return None;
    }

    // The legacy sections predate UI Size, so they belong to the default mode.
    let existing = load_or_quarantine(client_w, client_h, UiMode::Default).ok()?;
    let profile = existing.unwrap_or_else(|| {
        let mut profile = Profile::new_for_mode(client_w, client_h, UiMode::Default);
        profile.calibration = calibration;
        profile.atlas = atlas;
        profile.maps = maps;
        profile.identities = identities;
        profile
    });

    // Only claim the legacy sections once the profile is safely on disk.
    if save(&profile).is_err() {
        return None;
    }

    let _ = settings::update_settings(|object| {
        for key in ["calibration", "atlas", "maps", "identities"] {
            object.remove(key);
        }
    });

    Some(profile.key())
}

#[tauri::command]
pub fn list_profiles() -> Vec<ProfileSummary> {
    list()
}

#[tauri::command]
pub fn active_profile() -> Option<ProfileSummary> {
    active().map(|profile| profile.summary())
}

#[tauri::command]
pub fn delete_profile(client_w: u32, client_h: u32, ui_mode: Option<String>) -> Result<(), String> {
    let ui_mode = match ui_mode.as_deref() {
        Some(value) => {
            UiMode::parse(value).ok_or_else(|| format!("unknown UI size mode {value}"))?
        }
        None => UiMode::default(),
    };
    remove(client_w, client_h, ui_mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_the_client_size() {
        assert_eq!(
            Profile::new_for_mode(1366, 768, UiMode::Default).key(),
            "1366x768"
        );
        assert_eq!(
            Profile::new_for_mode(1920, 1080, UiMode::Ideal).key(),
            "1920x1080@ideal"
        );
    }

    #[test]
    fn summary_reports_readiness() {
        let mut profile = Profile::new_for_mode(1366, 768, UiMode::Default);
        let summary = profile.summary();
        assert!(!summary.calibrated);
        assert_eq!(summary.glyphs, 0);
        assert_eq!(summary.key, "1366x768");

        profile.calibration.exp = Some(settings::Region {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        });
        profile.calibration.level = Some(settings::Region {
            x: 5,
            y: 6,
            w: 7,
            h: 8,
        });
        assert!(profile.summary().calibrated);
    }

    #[test]
    fn profile_round_trips_through_json() {
        let mut profile = Profile::new_for_mode(1920, 1080, UiMode::Default);
        profile.maps.push(Template {
            label: "Victoria Road\nKerning City".into(),
            w: 2,
            h: 2,
            pixels: "10203040".into(),
        });

        let text = serde_json::to_string(&profile).unwrap();
        let parsed = parse_profile(&text).expect("parses");
        assert_eq!(parsed.key(), "1920x1080");
        assert_eq!(parsed.maps.len(), 1);
        assert!(parsed.maps[0].label.contains('\n'));
    }

    #[test]
    fn bom_prefixed_profile_still_parses() {
        let text = format!(
            "\u{feff}{}",
            serde_json::to_string(&Profile::new_for_mode(1, 1, UiMode::Default)).unwrap()
        );
        assert!(parse_profile(&text).is_some());
    }

    #[test]
    fn empty_profile_is_rejected() {
        assert!(parse_profile("{}").is_none());
        assert!(parse_profile("not json").is_none());
    }

    // -----------------------------------------------------------------------
    // Per-release upgrade fixtures.
    //
    // The fixtures live in `tests/fixtures/upgrade/`, outside `testdata/`, so
    // the regression frame walker can never mistake them for image pairs. Each
    // one is reconstructed from the `Profile` struct a tagged release wrote, not
    // captured from a player, so the hex payloads are synthetic patterns. They
    // are covered without touching a store: nothing here writes to
    // `data/profiles/` or calls `migrate_legacy`.
    // -----------------------------------------------------------------------

    /// The release-shaped profiles kept with the crate, and the UI mode each
    /// file name encodes.
    const UPGRADE_FIXTURES: [(&str, UiMode); 4] = [
        ("0.1.0-1366x768.json", UiMode::Default),
        ("0.2.0-1920x1200@ideal.json", UiMode::Ideal),
        ("0.5.0-1920x1080@filtered.json", UiMode::Filtered),
        ("0.6.0-1366x768.json", UiMode::Default),
    ];

    /// The directory the release-shaped profiles are kept in.
    fn upgrade_fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade")
    }

    fn fixture_text(name: &str) -> String {
        std::fs::read_to_string(upgrade_fixtures().join(name))
            .unwrap_or_else(|error| panic!("cannot read upgrade fixture {name}: {error}"))
    }

    /// The profile file name a fixture stands in for: the release prefix only
    /// records which release wrote it, so everything after the first `-` is the
    /// name the file has in `data/profiles/`. The prefix has to be a release
    /// number, so a label that itself contains a dash cannot quietly shift the
    /// name the fixture claims to stand in for.
    fn profile_file_name(fixture: &str) -> &str {
        let (release, name) = fixture
            .split_once('-')
            .unwrap_or_else(|| panic!("{fixture} has no release prefix"));
        assert!(
            release.split('.').count() == 3
                && release
                    .split('.')
                    .all(|part| !part.is_empty() && part.parse::<u32>().is_ok()),
            "{fixture}: the prefix before the first '-' must be MAJOR.MINOR.PATCH"
        );
        name
    }

    /// The UI mode a profile file name encodes: the bare `<W>x<H>` name is the
    /// default mode, `@filtered` and `@ideal` name the others.
    fn mode_from_file_name(name: &str) -> UiMode {
        UiMode::ALL
            .into_iter()
            .find(|mode| {
                !mode.suffix().is_empty() && name.ends_with(&format!("{}.json", mode.suffix()))
            })
            .unwrap_or(UiMode::Default)
    }

    /// The client size a profile file name encodes, e.g. `1366x768@ideal.json`.
    fn client_size_from_file_name(name: &str) -> (u32, u32) {
        let key = name.trim_end_matches(".json");
        let key = key
            .strip_suffix(mode_from_file_name(name).suffix())
            .unwrap_or(key);
        let (width, height) = key
            .split_once('x')
            .unwrap_or_else(|| panic!("{name} does not encode a client size"));
        (
            width
                .parse()
                .unwrap_or_else(|_| panic!("{name}: bad client width")),
            height
                .parse()
                .unwrap_or_else(|_| panic!("{name}: bad client height")),
        )
    }

    /// Load a fixture through the code that runs on load: `parse_for_mode`, with
    /// the client size and the UI mode read out of the file name exactly as
    /// `load_for_mode` reads them. The disk half of `load_for_mode` is
    /// deliberately not exercised — it would need a data root — but the size
    /// check and the filename-authoritative override are the production lines,
    /// not a copy of them.
    fn load_fixture(fixture: &str) -> Profile {
        let name = profile_file_name(fixture);
        let (client_w, client_h) = client_size_from_file_name(name);
        parse_for_mode(
            &fixture_text(fixture),
            client_w,
            client_h,
            mode_from_file_name(name),
        )
        .unwrap_or_else(|| panic!("upgrade fixture {fixture} does not load"))
    }

    fn glyphs(atlas: &Atlas) -> Vec<(String, u32, u32, String)> {
        atlas
            .glyphs
            .iter()
            .map(|glyph| (glyph.ch.clone(), glyph.w, glyph.h, glyph.bitmap.clone()))
            .collect()
    }

    /// The atlas fields a load-then-save cycle must not rewrite, besides the
    /// glyphs themselves.
    fn atlas_meta(atlas: &Atlas) -> (u32, String, u32, u32) {
        (
            atlas.format,
            atlas.source.clone(),
            atlas.cell_w,
            atlas.cell_h,
        )
    }

    /// Every atlas of a profile, in a stable order, with a label for messages.
    fn atlases(profile: &Profile) -> [(&'static str, &Atlas); 4] {
        [
            ("atlas", &profile.atlas),
            ("mapAtlas", &profile.map_atlas),
            ("levelAtlas", &profile.level_atlas),
            ("identityAtlas", &profile.identity_atlas),
        ]
    }

    /// The upgrade fixtures must stay out of the frame corpus, and the directory
    /// must hold exactly the set the rest of these tests iterate.
    #[test]
    fn upgrade_fixtures_stay_out_of_the_frame_corpus() {
        let dir = upgrade_fixtures();
        assert!(
            dir.is_dir(),
            "upgrade fixture directory is missing: {dir:?}"
        );
        // The frame walker only reads `testdata/`. Keeping the upgrade fixtures
        // outside it is what stops them being read as (broken) image pairs.
        assert!(
            !dir.starts_with(crate::regression::fixtures_dir()),
            "{dir:?} must not live under the frame corpus"
        );

        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("list the upgrade fixtures")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.sort();
        let mut expected: Vec<String> = UPGRADE_FIXTURES
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        expected.sort();
        assert_eq!(names, expected);
    }

    /// The pre-UI-Size profile: no `uiMode`, no per-readout atlases, no street
    /// box, legacy 12x16 `cell` glyphs, and one unknown key. It has to parse with
    /// the new fields defaulted and its data unchanged.
    #[test]
    fn pre_ui_size_profile_keeps_its_data_and_defaults() {
        let name = "0.1.0-1366x768.json";
        let text = fixture_text(name);
        assert!(
            !text.contains("\"uiMode\""),
            "the pre-UI-Size fixture must not carry a uiMode key"
        );
        // No release ever wrote a profile key of this name; it is here only to
        // prove unknown keys are ignored instead of failing the file.
        assert!(
            text.contains("\"obsoleteKey\""),
            "the fixture should keep an unknown key"
        );

        let profile = load_fixture(name);
        assert_eq!(profile.client_w, 1366);
        assert_eq!(profile.client_h, 768);
        // An absent `uiMode` deserializes to the default mode, and the bare file
        // name says the same thing.
        assert_eq!(profile.ui_mode, UiMode::Default);
        assert_eq!(profile.key(), "1366x768");

        // Calibration boxes are carried through untouched.
        assert_eq!(
            profile.calibration.exp,
            Some(settings::Region {
                x: 772,
                y: 733,
                w: 96,
                h: 16,
            })
        );
        assert_eq!(
            profile.calibration.level,
            Some(settings::Region {
                x: 330,
                y: 745,
                w: 21,
                h: 10,
            })
        );
        assert_eq!(
            profile.calibration.map,
            Some(settings::Region {
                x: 1051,
                y: 96,
                w: 118,
                h: 30,
            })
        );
        assert_eq!(
            profile.calibration.name,
            Some(settings::Region {
                x: 366,
                y: 751,
                w: 60,
                h: 13,
            })
        );
        assert_eq!(
            profile.calibration.job,
            Some(settings::Region {
                x: 366,
                y: 737,
                w: 34,
                h: 12,
            })
        );
        // The split street box did not exist in this shape.
        assert_eq!(profile.calibration.street, None);
        assert!(profile.is_calibrated());

        // Legacy 12x16 `cell` glyphs still parse; the native fields default.
        assert_eq!(profile.atlas.format, 0);
        assert_eq!(profile.atlas.source, "");
        assert_eq!(profile.atlas.cell_w, 12);
        assert_eq!(profile.atlas.cell_h, 16);
        assert_eq!(profile.atlas.glyphs.len(), 3);
        assert_eq!(profile.atlas.glyphs[0].ch, "0");
        assert_eq!(profile.atlas.glyphs[0].w, 6);
        assert_eq!(profile.atlas.glyphs[0].h, 9);
        assert_eq!(profile.atlas.glyphs[0].bitmap, "");
        assert_eq!(
            profile.atlas.glyphs[0].cell.len(),
            crate::ocr::CELL_W * crate::ocr::CELL_H * 2
        );
        assert!(profile.has_glyphs());

        // The per-readout atlases and the taught streets were added later, so
        // they default to empty instead of failing the file.
        for atlas in [
            &profile.map_atlas,
            &profile.level_atlas,
            &profile.identity_atlas,
        ] {
            assert!(atlas.glyphs.is_empty());
            assert_eq!(atlas.format, 0);
            assert_eq!(atlas.source, "");
        }
        assert!(profile.streets.is_empty());

        // Taught maps and identities survive in place.
        assert_eq!(profile.maps.len(), 1);
        assert_eq!(profile.maps[0].label, "Victoria Road\nKerning City");
        assert_eq!(profile.maps[0].pixels.len(), 8 * 4 * 2);
        assert_eq!(
            profile
                .identities
                .name
                .as_ref()
                .map(|item| item.label.as_str()),
            Some("Furoh")
        );
        assert_eq!(
            profile
                .identities
                .job
                .as_ref()
                .map(|item| item.label.as_str()),
            Some("Page")
        );

        let summary = profile.summary();
        assert_eq!(summary.key, "1366x768");
        assert!(summary.calibrated);
        assert_eq!(summary.glyphs, 3);
        assert_eq!(summary.maps, 1);
        assert_eq!(summary.glyph_size, "6x9");
        assert_eq!(summary.name.as_deref(), Some("Furoh"));
        assert_eq!(summary.job.as_deref(), Some("Page"));
    }

    /// The 0.2.0 file is the realistic upgrade path: the release that first wrote
    /// `uiMode` still stored atlas glyphs as 12x16 `cell`s, so this shape carries
    /// both risk areas at once. `levelAtlas`, `identityAtlas` and the split street
    /// box came later and have to default instead of failing the file.
    #[test]
    fn ui_mode_release_carries_legacy_cells_and_defaults() {
        let name = "0.2.0-1920x1200@ideal.json";
        let value: serde_json::Value =
            serde_json::from_str(&fixture_text(name)).expect("the 0.2.0 fixture is JSON");
        for key in ["levelAtlas", "identityAtlas", "streets"] {
            assert!(value.get(key).is_none(), "0.2.0 never wrote {key}");
        }

        let profile = load_fixture(name);
        assert_eq!((profile.client_w, profile.client_h), (1920, 1200));
        assert_eq!(profile.ui_mode, UiMode::Ideal);
        assert_eq!(profile.key(), "1920x1200@ideal");
        assert_eq!(profile.calibration.street, None);
        assert!(profile.is_calibrated());

        // Both atlases are the legacy shape: cells only, no native bitmap.
        assert_eq!(profile.atlas.glyphs.len(), 3);
        assert_eq!(profile.map_atlas.glyphs.len(), 1);
        for atlas in [&profile.atlas, &profile.map_atlas] {
            assert_eq!(atlas.format, 0);
            assert_eq!(atlas.source, "");
            assert_eq!((atlas.cell_w, atlas.cell_h), (12, 16));
            for glyph in &atlas.glyphs {
                assert!(glyph.bitmap.is_empty(), "{}: no native bitmap", glyph.ch);
                assert_eq!(
                    glyph.cell.len(),
                    crate::ocr::CELL_W * crate::ocr::CELL_H * 2,
                    "{}: cell size",
                    glyph.ch
                );
            }
        }

        // Fields added after 0.2.0 default instead of failing the file.
        assert!(profile.level_atlas.glyphs.is_empty());
        assert!(profile.identity_atlas.glyphs.is_empty());
        assert!(profile.streets.is_empty());
        assert_eq!(profile.maps.len(), 1);
        assert_eq!(profile.maps[0].label, "Victoria Road\nKerning City");
        assert_eq!(
            profile
                .identities
                .name
                .as_ref()
                .map(|item| item.label.as_str()),
            Some("Furoh")
        );
    }

    /// The 0.5.0 file is one an editor or PowerShell `Set-Content` left with a
    /// UTF-8 BOM; it must still load. Its own `uiMode` is stale — the pair was
    /// hand-copied from the Default set, so the stored mode and the file name
    /// disagree. (A 0.5.0 save always wrote the two in agreement, so only a
    /// hand-edited file can look like this; the guard is still worth pinning.)
    #[test]
    fn bom_prefixed_release_profile_parses() {
        let name = "0.5.0-1920x1080@filtered.json";
        let text = fixture_text(name);
        assert!(
            text.starts_with('\u{feff}'),
            "the 0.5.0 fixture must carry a BOM"
        );

        // The raw parse reports the stored value; the load path overrides it from
        // the file name.
        let stored = parse_profile(&text).expect("a BOM-prefixed profile parses");
        assert_eq!((stored.client_w, stored.client_h), (1920, 1080));
        assert_eq!(stored.ui_mode, UiMode::Default);

        let profile = load_fixture(name);
        assert_eq!(profile.ui_mode, UiMode::Filtered);
        assert_eq!(profile.key(), "1920x1080@filtered");

        // Both minimap label lines and the taught street survive.
        assert_eq!(
            profile.calibration.street,
            Some(settings::Region {
                x: 1264,
                y: 986,
                w: 150,
                h: 12,
            })
        );
        assert_eq!(
            profile.calibration.map,
            Some(settings::Region {
                x: 1264,
                y: 998,
                w: 150,
                h: 13,
            })
        );
        assert_eq!(profile.maps[0].label, "Hidden Street\nFree Market <8>");
        assert_eq!(profile.streets[0].label, "Hidden Street");
        assert_eq!(profile.identity_atlas.source, "font");
        assert_eq!(profile.level_atlas.glyphs.len(), 1);
        assert_eq!(profile.map_atlas.source, "taught");
        // Native bitmaps, one byte per glyph pixel.
        for glyph in &profile.atlas.glyphs {
            assert_eq!(glyph.bitmap.len(), (glyph.w * glyph.h * 2) as usize);
            assert_eq!(glyph.cell, "");
        }
    }

    /// The file name decides the UI mode, not the `uiMode` field: the bare name
    /// is Default, `@filtered` and `@ideal` are the others, and `path_for` writes
    /// exactly the name each fixture stands in for. Every assertion goes through
    /// `parse_for_mode`, the code `load_for_mode` runs after reading the file, so
    /// dropping or reordering the override there fails this test.
    #[test]
    fn file_name_is_authoritative_for_ui_mode() {
        for (fixture, expected) in UPGRADE_FIXTURES {
            let name = profile_file_name(fixture);
            let (client_w, client_h) = client_size_from_file_name(name);
            let text = fixture_text(fixture);
            assert_eq!(mode_from_file_name(name), expected, "{fixture}");

            // What the load path keeps: whatever the file stored, the mode the
            // file name encodes is the one reported.
            let profile = parse_for_mode(&text, client_w, client_h, expected)
                .unwrap_or_else(|| panic!("{fixture} does not load"));
            assert_eq!(profile.ui_mode, expected, "{fixture}");
            assert_eq!(
                (profile.client_w, profile.client_h),
                (client_w, client_h),
                "{fixture}"
            );
            assert_eq!(profile.key(), name.trim_end_matches(".json"), "{fixture}");

            // The read path refuses a file that is not for the size its name
            // claims, whatever the file itself stores.
            assert!(
                parse_for_mode(&text, client_w + 1, client_h, expected).is_none(),
                "{fixture}: a client-width mismatch must not load"
            );
            assert!(
                parse_for_mode(&text, client_w, client_h + 1, expected).is_none(),
                "{fixture}: a client-height mismatch must not load"
            );

            // `path_for` writes exactly the name each fixture stands in for, so
            // the suffix and the stored mode can never disagree on disk.
            let saved_name = path_for(client_w, client_h, expected)
                .expect("a profile path")
                .file_name()
                .map(|file| file.to_string_lossy().into_owned())
                .expect("a profile file name");
            assert_eq!(saved_name, name, "{fixture}");
        }
    }

    /// The current shape: every field 0.6.0 writes is present and parsed.
    #[test]
    fn current_release_profile_is_the_full_shape() {
        let name = "0.6.0-1366x768.json";
        let value: serde_json::Value =
            serde_json::from_str(&fixture_text(name)).expect("the current fixture is JSON");
        for key in [
            "clientW",
            "clientH",
            "uiMode",
            "calibration",
            "atlas",
            "mapAtlas",
            "levelAtlas",
            "identityAtlas",
            "maps",
            "streets",
            "identities",
        ] {
            assert!(value.get(key).is_some(), "current fixture is missing {key}");
        }

        let profile = load_fixture(name);
        assert_eq!(profile.ui_mode, UiMode::Default);
        assert_eq!(profile.key(), "1366x768");
        assert_eq!(profile.calibration.street.map(|region| region.h), Some(12));
        assert_eq!(profile.atlas.format, 2);
        assert_eq!(profile.atlas.source, "game");
        assert_eq!(glyphs(&profile.atlas).len(), 2);
        assert_eq!(profile.map_atlas.source, "taught");
        assert_eq!(profile.level_atlas.source, "game");
        assert_eq!(profile.identity_atlas.source, "font");
        assert_eq!(profile.maps.len(), 1);
        assert_eq!(profile.streets.len(), 1);
        assert_eq!(profile.streets[0].label, "Victoria Road");
        for atlas in [
            &profile.atlas,
            &profile.map_atlas,
            &profile.level_atlas,
            &profile.identity_atlas,
        ] {
            assert_eq!(atlas.format, 2);
            for glyph in &atlas.glyphs {
                assert_eq!(glyph.bitmap.len(), (glyph.w * glyph.h * 2) as usize);
            }
        }
    }

    /// "Parse and migrate": the legacy `cell` glyphs are only usable because
    /// `ocr::Atlas::from_settings` decodes them into native bitmaps, and the save
    /// path then writes `format: 2` and drops the read-only cells. Both release
    /// shapes that carry cells go through it, and no glyph may decode to an empty
    /// shape (a cell whose length no longer matches `CELL_LEN` degrades to zeros).
    #[test]
    fn legacy_cell_glyphs_migrate_to_native_bitmaps() {
        for fixture in ["0.1.0-1366x768.json", "0.2.0-1920x1200@ideal.json"] {
            let profile = load_fixture(fixture);
            for (label, atlas) in atlases(&profile) {
                if atlas.glyphs.is_empty() {
                    continue;
                }
                let migrated = crate::ocr::Atlas::from_settings(atlas);
                assert_eq!(
                    migrated.glyphs.len(),
                    atlas.glyphs.len(),
                    "{fixture}/{label}: every legacy cell decoded"
                );
                for glyph in &migrated.glyphs {
                    assert_eq!(
                        glyph.bitmap.len(),
                        glyph.w as usize * glyph.h as usize,
                        "{fixture}/{label}: {} bitmap size",
                        glyph.ch
                    );
                    assert!(
                        glyph.bitmap.iter().any(|value| *value != 0),
                        "{fixture}/{label}: {} decoded to an empty shape",
                        glyph.ch
                    );
                }

                // What the next save writes: native bitmaps, no cells.
                let back = migrated.to_settings();
                assert_eq!(back.format, 2, "{fixture}/{label}");
                assert_eq!(back.glyphs.len(), atlas.glyphs.len(), "{fixture}/{label}");
                for glyph in &back.glyphs {
                    assert!(
                        glyph.cell.is_empty(),
                        "{fixture}/{label}: {} kept a cell",
                        glyph.ch
                    );
                    assert_eq!(
                        glyph.bitmap.len(),
                        glyph.w as usize * glyph.h as usize * 2,
                        "{fixture}/{label}: {} bitmap hex",
                        glyph.ch
                    );
                }
            }
        }
    }

    /// A save cycle after the upgrade must not rewrite the stored data — neither
    /// the glyphs nor the atlas metadata (`format`, `source`, `cellW`, `cellH`)
    /// that tells the reader what those glyphs are. The one exception is the
    /// legacy `cell`, which is read-only and never written back.
    #[test]
    fn upgrading_a_profile_does_not_rewrite_its_data() {
        for (fixture, _) in UPGRADE_FIXTURES {
            let profile = load_fixture(fixture);
            let saved = serde_json::to_string(&profile).expect("a loaded profile serializes");
            let reloaded = parse_profile(&saved).unwrap_or_else(|| panic!("{fixture} reparses"));

            let before = serde_json::to_value(&profile).expect("profile to value");
            let after = serde_json::to_value(&reloaded).expect("profile to value");
            for key in [
                "clientW",
                "clientH",
                "uiMode",
                "calibration",
                "maps",
                "streets",
                "identities",
            ] {
                assert_eq!(
                    before[key], after[key],
                    "{fixture}: {key} changed on a save"
                );
            }
            for ((label, before), (_, after)) in
                atlases(&profile).into_iter().zip(atlases(&reloaded))
            {
                assert_eq!(
                    atlas_meta(after),
                    atlas_meta(before),
                    "{fixture}/{label}: atlas metadata changed on a save"
                );
                assert_eq!(
                    glyphs(after),
                    glyphs(before),
                    "{fixture}/{label}: glyphs changed on a save"
                );
            }
            assert_eq!(reloaded.key(), profile.key(), "{fixture}");
            assert_eq!(
                reloaded.summary().glyphs,
                profile.summary().glyphs,
                "{fixture}"
            );
            assert!(
                !saved.contains("\"cell\""),
                "{fixture}: the read-only legacy cell must not be written back"
            );
        }
    }
}
