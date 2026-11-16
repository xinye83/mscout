use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::settings::{self, Atlas, Calibration, Identities, Template, UiMode};

/// Good consecutive reads required before a profile is trusted to record.
pub const VERIFY_READS: usize = 3;

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

/// The set for the client size under the UI Size currently selected.
pub fn load(client_w: u32, client_h: u32) -> Option<Profile> {
    load_for_mode(client_w, client_h, settings::ui_mode())
}

pub fn load_for_mode(client_w: u32, client_h: u32, ui_mode: UiMode) -> Option<Profile> {
    let path = path_for(client_w, client_h, ui_mode).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let mut profile = parse_profile(&text)?;
    if profile.client_w != client_w || profile.client_h != client_h {
        return None;
    }
    // The filename is authoritative: sets written before UI Size was tracked
    // carry no `uiMode` field but still belong to their file's mode.
    profile.ui_mode = ui_mode;
    Some(profile)
}

pub fn save(profile: &Profile) -> Result<(), String> {
    let _guard = lock(&DISK_LOCK);
    let dir = dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(profile).map_err(|e| e.to_string())?;
    std::fs::write(
        path_for(profile.client_w, profile.client_h, profile.ui_mode)?,
        text,
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

    let profile = match load_for_mode(client_w, client_h, ui_mode) {
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
    let existing = load_for_mode(client_w, client_h, UiMode::Default);
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
    // the regression frame walker can never mistake them for image pairs. They
    // are covered parse-only: nothing here writes to `data/profiles/` or calls
    // `migrate_legacy`, so a real player's store is never read or touched.
    // -----------------------------------------------------------------------

    /// The release-shaped profiles kept with the crate.
    fn upgrade_fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade")
    }

    fn fixture_text(name: &str) -> String {
        std::fs::read_to_string(upgrade_fixtures().join(name))
            .unwrap_or_else(|error| panic!("cannot read upgrade fixture {name}: {error}"))
    }

    /// The profile file name a fixture stands in for: the release prefix only
    /// records which release wrote it, so everything after the first `-` is the
    /// name the file has in `data/profiles/`.
    fn profile_file_name(name: &str) -> &str {
        name.split_once('-').map(|(_, rest)| rest).unwrap_or(name)
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

    /// Parse a fixture the way `load_for_mode` does once the file has been read:
    /// `parse_profile` decides whether the file is usable, then the *filename*
    /// decides the UI mode whatever the JSON says. The disk half of
    /// `load_for_mode` is deliberately not exercised — it would need a data root.
    fn load_fixture(name: &str) -> Profile {
        let mut profile = parse_profile(&fixture_text(name))
            .unwrap_or_else(|| panic!("upgrade fixture {name} does not parse"));
        profile.ui_mode = mode_from_file_name(name);
        profile
    }

    fn glyphs(atlas: &Atlas) -> Vec<(String, u32, u32, String)> {
        atlas
            .glyphs
            .iter()
            .map(|glyph| (glyph.ch.clone(), glyph.w, glyph.h, glyph.bitmap.clone()))
            .collect()
    }

    /// The upgrade fixtures must stay out of the frame corpus, and the set must
    /// cover every release the profile format changed in.
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
        assert_eq!(
            names,
            [
                "0.1.0-1366x768.json",
                "0.5.0-1920x1080@filtered.json",
                "0.6.0-1366x768.json",
            ]
        );
    }

    /// The pre-UI-Size profile: no `uiMode`, no per-readout atlases, no street
    /// box, legacy 12x16 `cell` glyphs, and a key a later release dropped. It has
    /// to parse with the new fields defaulted and its data unchanged.
    #[test]
    fn pre_ui_size_profile_keeps_its_data_and_defaults() {
        let name = "0.1.0-1366x768.json";
        let text = fixture_text(name);
        assert!(
            !text.contains("\"uiMode\""),
            "the pre-UI-Size fixture must not carry a uiMode key"
        );
        assert!(
            text.contains("\"reviewQueue\""),
            "the fixture should keep a key a later release dropped"
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

    /// The 0.5.0 file is one an editor or PowerShell `Set-Content` left with a
    /// UTF-8 BOM; it must still parse, and its own `uiMode` is stale.
    #[test]
    fn bom_prefixed_release_profile_parses() {
        let name = "0.5.0-1920x1080@filtered.json";
        let text = fixture_text(name);
        assert!(
            text.starts_with('\u{feff}'),
            "the 0.5.0 fixture must carry a BOM"
        );

        let mut profile = parse_profile(&text).expect("a BOM-prefixed profile parses");
        assert_eq!(profile.client_w, 1920);
        assert_eq!(profile.client_h, 1080);
        // Stored mode says Default — a copy taken from the Default pair — while
        // the file name says Filtered. The raw parse reports the stored value;
        // the load path then overrides it from the file name.
        assert_eq!(profile.ui_mode, UiMode::Default);
        profile.ui_mode = mode_from_file_name(name);
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
    /// exactly the name each fixture stands in for.
    #[test]
    fn file_name_is_authoritative_for_ui_mode() {
        for (name, expected) in [
            ("0.1.0-1366x768.json", UiMode::Default),
            ("0.5.0-1920x1080@filtered.json", UiMode::Filtered),
            ("0.6.0-1366x768.json", UiMode::Default),
        ] {
            assert_eq!(mode_from_file_name(name), expected, "{name}");

            // What the load path keeps: whatever the file stored, the mode the
            // file name encodes is the one reported.
            let profile = load_fixture(name);
            assert_eq!(profile.ui_mode, expected, "{name}");
            assert_eq!(
                profile.key(),
                profile_file_name(name).trim_end_matches(".json"),
                "{name}"
            );

            // `path_for` writes exactly the name each fixture stands in for, so
            // the suffix and the stored mode can never disagree on disk.
            let saved_name = path_for(profile.client_w, profile.client_h, expected)
                .expect("a profile path")
                .file_name()
                .map(|file| file.to_string_lossy().into_owned())
                .expect("a profile file name");
            assert_eq!(saved_name, profile_file_name(name), "{name}");
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

    /// A save cycle after the upgrade must not rewrite the stored data. The one
    /// exception is the legacy `cell`, which is read-only and never written back.
    #[test]
    fn upgrading_a_profile_does_not_rewrite_its_data() {
        for name in [
            "0.1.0-1366x768.json",
            "0.5.0-1920x1080@filtered.json",
            "0.6.0-1366x768.json",
        ] {
            let profile = load_fixture(name);
            let saved = serde_json::to_string(&profile).expect("a loaded profile serializes");
            let reloaded = parse_profile(&saved).unwrap_or_else(|| panic!("{name} reparses"));

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
                assert_eq!(before[key], after[key], "{name}: {key} changed on a save");
            }
            assert_eq!(glyphs(&reloaded.atlas), glyphs(&profile.atlas), "{name}");
            assert_eq!(
                glyphs(&reloaded.map_atlas),
                glyphs(&profile.map_atlas),
                "{name}"
            );
            assert_eq!(
                glyphs(&reloaded.level_atlas),
                glyphs(&profile.level_atlas),
                "{name}"
            );
            assert_eq!(
                glyphs(&reloaded.identity_atlas),
                glyphs(&profile.identity_atlas),
                "{name}"
            );
            assert_eq!(reloaded.key(), profile.key(), "{name}");
            assert_eq!(
                reloaded.summary().glyphs,
                profile.summary().glyphs,
                "{name}"
            );
            assert!(
                !saved.contains("\"cell\""),
                "{name}: the read-only legacy cell must not be written back"
            );
        }
    }
}
