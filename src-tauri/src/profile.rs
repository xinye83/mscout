use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::settings::{self, Atlas, Calibration, Identities, Template};

/// Good consecutive reads required before a profile is trusted to record.
pub const VERIFY_READS: usize = 3;

/// One calibration set, scoped to a client size.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub client_w: u32,
    pub client_h: u32,
    #[serde(default)]
    pub calibration: Calibration,
    #[serde(default)]
    pub atlas: Atlas,
    #[serde(default)]
    pub maps: Vec<Template>,
    #[serde(default)]
    pub identities: Identities,
}

impl Profile {
    pub fn new(client_w: u32, client_h: u32) -> Self {
        Self {
            client_w,
            client_h,
            ..Default::default()
        }
    }

    pub fn key(&self) -> String {
        format!("{}x{}", self.client_w, self.client_h)
    }

    pub fn is_calibrated(&self) -> bool {
        self.calibration.is_ready()
    }

    pub fn has_glyphs(&self) -> bool {
        !self.atlas.glyphs.is_empty()
    }

    pub fn summary(&self) -> ProfileSummary {
        ProfileSummary {
            key: self.key(),
            client_w: self.client_w,
            client_h: self.client_h,
            calibrated: self.is_calibrated(),
            glyphs: self.atlas.glyphs.len(),
            maps: self.maps.len(),
            name: self.identities.name.as_ref().map(|t| t.label.clone()),
            job: self.identities.job.as_ref().map(|t| t.label.clone()),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub key: String,
    pub client_w: u32,
    pub client_h: u32,
    pub calibrated: bool,
    pub glyphs: usize,
    pub maps: usize,
    pub name: Option<String>,
    pub job: Option<String>,
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

pub fn path_for(client_w: u32, client_h: u32) -> Result<PathBuf, String> {
    Ok(dir()?.join(format!("{client_w}x{client_h}.json")))
}

fn parse_profile(text: &str) -> Option<Profile> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let profile: Profile = serde_json::from_str(text).ok()?;
    if profile.client_w == 0 || profile.client_h == 0 {
        return None;
    }
    Some(profile)
}

pub fn load(client_w: u32, client_h: u32) -> Option<Profile> {
    let path = path_for(client_w, client_h).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let profile = parse_profile(&text)?;
    if profile.client_w != client_w || profile.client_h != client_h {
        return None;
    }
    Some(profile)
}

pub fn save(profile: &Profile) -> Result<(), String> {
    let _guard = lock(&DISK_LOCK);
    let dir = dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(profile).map_err(|e| e.to_string())?;
    std::fs::write(path_for(profile.client_w, profile.client_h)?, text).map_err(|e| e.to_string())
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

/// Make the profile for this client size active, creating an empty one when the
/// size has never been seen. Returns the profile that is now active.
pub fn activate(client_w: u32, client_h: u32) -> Result<Profile, String> {
    let key = format!("{client_w}x{client_h}");
    let mut active = lock(&ACTIVE);
    if let Some(current) = active.as_ref() {
        if current.key == key {
            return Ok(current.profile.clone());
        }
    }

    let profile = load(client_w, client_h).unwrap_or_else(|| Profile::new(client_w, client_h));
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

/// Adopt an externally loaded profile (e.g. after a calibration edit).
#[allow(dead_code)]
pub fn activate_with(profile: Profile) -> Result<Profile, String> {
    save(&profile)?;
    let key = profile.key();
    *lock(&ACTIVE) = Some(Active {
        key,
        profile: profile.clone(),
    });
    Ok(profile)
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

    let existing = load(client_w, client_h);
    let profile = existing.unwrap_or_else(|| {
        let mut profile = Profile::new(client_w, client_h);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_the_client_size() {
        assert_eq!(Profile::new(1366, 768).key(), "1366x768");
        assert_eq!(Profile::new(1920, 1080).key(), "1920x1080");
    }

    #[test]
    fn summary_reports_readiness() {
        let mut profile = Profile::new(1366, 768);
        let summary = profile.summary();
        assert!(!summary.calibrated);
        assert_eq!(summary.glyphs, 0);
        assert_eq!(summary.key, "1366x768");

        profile.calibration.exp = Some(settings::Region { x: 1, y: 2, w: 3, h: 4 });
        profile.calibration.level = Some(settings::Region { x: 5, y: 6, w: 7, h: 8 });
        assert!(profile.summary().calibrated);
    }

    #[test]
    fn profile_round_trips_through_json() {
        let mut profile = Profile::new(1920, 1080);
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
        let text = format!("\u{feff}{}", serde_json::to_string(&Profile::new(1, 1)).unwrap());
        assert!(parse_profile(&text).is_some());
    }

    #[test]
    fn empty_profile_is_rejected() {
        assert!(parse_profile("{}").is_none());
        assert!(parse_profile("not json").is_none());
    }
}
