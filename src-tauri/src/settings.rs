use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    SETTINGS_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn path() -> Result<std::path::PathBuf, String> {
    Ok(crate::data_dir()?.join("settings.json"))
}

pub fn read_raw() -> Value {
    let Ok(path) = path() else {
        return Value::Object(Map::new());
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Value::Object(Map::new());
    };
    // A UTF-8 BOM (e.g. from an editor or PowerShell's Set-Content) makes
    // serde_json reject the file, which would silently clear every setting.
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);

    match serde_json::from_str::<Value>(raw) {
        Ok(value) if value.is_object() => value,
        _ => Value::Object(Map::new()),
    }
}

fn update<F>(apply: F) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>),
{
    let _guard = lock();
    let dir = crate::data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let mut raw = read_raw();
    if !raw.is_object() {
        raw = Value::Object(Map::new());
    }

    {
        let object = raw
            .as_object_mut()
            .ok_or("settings.json must contain an object")?;
        apply(object);
    }

    let text = serde_json::to_string_pretty(&raw).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("settings.json"), text).map_err(|e| e.to_string())
}

pub fn update_settings<F>(apply: F) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>),
{
    update(apply)
}

#[allow(dead_code)]
fn section<T: for<'de> Deserialize<'de> + Default>(key: &str) -> T {
    read_raw()
        .get(key)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowSelector {
    pub title: String,
    pub process: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Region {
    pub fn is_usable(&self) -> bool {
        self.w > 1 && self.h > 1
    }
}

/// The game's Graphics → UI Size option. It rescales the whole HUD *and* the
/// font, so every readout is keyed by it as well as by the client size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiMode {
    #[default]
    Default,
    Filtered,
    Ideal,
}

impl UiMode {
    pub const ALL: [UiMode; 3] = [UiMode::Default, UiMode::Filtered, UiMode::Ideal];

    /// Stable id: used in settings, profile summaries and IPC calls.
    pub fn id(self) -> &'static str {
        match self {
            UiMode::Default => "default",
            UiMode::Filtered => "filtered",
            UiMode::Ideal => "ideal",
        }
    }

    /// Filename suffix. The default mode keeps the bare `WxH` name, so sets
    /// captured before UI Size was tracked keep working.
    pub fn suffix(self) -> &'static str {
        match self {
            UiMode::Default => "",
            UiMode::Filtered => "@filtered",
            UiMode::Ideal => "@ideal",
        }
    }

    /// The option as the game's Graphics menu names it.
    pub fn label(self) -> &'static str {
        match self {
            UiMode::Default => "Default Ratio",
            UiMode::Filtered => "Default Ratio (Filter Applied)",
            UiMode::Ideal => "Ideal Ratio",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == value)
    }
}

/// The UI Size the player has chosen in the game. It is picked in the app
/// rather than detected: the two non-default options move and rescale every
/// readout, so guessing wrong reads nothing.
pub fn ui_mode() -> UiMode {
    read_raw()
        .get("uiMode")
        .and_then(|value| value.as_str())
        .and_then(UiMode::parse)
        .unwrap_or_default()
}

pub fn save_ui_mode(mode: UiMode) -> Result<(), String> {
    update(move |object| {
        object.insert("uiMode".into(), Value::String(mode.id().into()));
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiModeOption {
    pub id: String,
    pub label: String,
    pub active: bool,
}

/// The three options the game offers, with the current one flagged, so the
/// picker and the app cannot drift apart.
#[tauri::command]
pub fn ui_modes() -> Vec<UiModeOption> {
    let current = ui_mode();
    UiMode::ALL
        .into_iter()
        .map(|mode| UiModeOption {
            id: mode.id().to_string(),
            label: mode.label().to_string(),
            active: mode == current,
        })
        .collect()
}

#[tauri::command]
pub fn set_ui_mode(mode: String) -> Result<(), String> {
    let mode = UiMode::parse(&mode).ok_or_else(|| format!("unknown UI size mode {mode}"))?;
    save_ui_mode(mode)
}


#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct Calibration {
    pub client_w: Option<u32>,
    pub client_h: Option<u32>,
    pub exp: Option<Region>,
    pub level: Option<Region>,
    /// The minimap label's street line (the top line, e.g. "Victoria Road").
    pub street: Option<Region>,
    /// The minimap label's map-name line (the bottom line, e.g. "Kerning City").
    pub map: Option<Region>,
    pub name: Option<Region>,
    pub job: Option<Region>,
}

impl Calibration {
    /// A read can be attempted once the EXP box is set. The level box is
    /// optional: without it the level is derived from EXP and the percentage.
    pub fn is_ready(&self) -> bool {
        self.exp.map(|r| r.is_usable()).unwrap_or(false)
    }

    /// The box around both map-label lines (street over map name). The reader
    /// fingerprints the whole label, so it needs the union of the two split boxes.
    pub fn map_box(&self) -> Option<Region> {
        match (self.street, self.map) {
            (Some(a), Some(b)) => {
                let left = a.x.min(b.x);
                let top = a.y.min(b.y);
                let right = (a.x + a.w).max(b.x + b.w);
                let bottom = (a.y + a.h).max(b.y + b.h);
                Some(Region {
                    x: left,
                    y: top,
                    w: right - left,
                    h: bottom - top,
                })
            }
            (Some(region), None) | (None, Some(region)) => Some(region),
            (None, None) => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlyphCell {
    pub ch: String,
    pub w: u32,
    pub h: u32,
    pub cell: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Atlas {
    pub cell_w: u32,
    pub cell_h: u32,
    pub glyphs: Vec<GlyphCell>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub label: String,
    pub w: u32,
    pub h: u32,
    pub pixels: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identities {
    pub name: Option<Template>,
    pub job: Option<Template>,
}

pub fn load_target() -> Option<WindowSelector> {
    let value = read_raw();
    serde_json::from_value(value.get("targetWindow")?.clone()).ok()
}

pub fn save_target(selector: Option<&WindowSelector>) -> Result<(), String> {
    let selector = selector.cloned();
    update(move |object| match selector {
        Some(selector) => {
            if let Ok(value) = serde_json::to_value(&selector) {
                object.insert("targetWindow".into(), value);
            }
        }
        None => {
            object.remove("targetWindow");
        }
    })
}

pub fn load_calibration() -> Calibration {
    crate::profile::active()
        .map(|profile| profile.calibration)
        .unwrap_or_default()
}

pub fn save_calibration(calibration: &Calibration) -> Result<(), String> {
    let calibration = calibration.clone();
    crate::profile::update(move |profile| {
        profile.calibration = calibration;
    })
    .map(|_| ())
}

pub fn load_atlas() -> Atlas {
    let mut atlas = crate::profile::active()
        .map(|profile| profile.atlas)
        .unwrap_or_default();
    if atlas.cell_w == 0 || atlas.cell_h == 0 {
        atlas.cell_w = crate::ocr::CELL_W as u32;
        atlas.cell_h = crate::ocr::CELL_H as u32;
    }
    atlas
}

pub fn save_atlas(atlas: &Atlas) -> Result<(), String> {
    let atlas = atlas.clone();
    crate::profile::update(move |profile| {
        profile.atlas = atlas;
    })
    .map(|_| ())
}

pub fn load_map_atlas() -> Atlas {
    let mut atlas = crate::profile::active()
        .map(|profile| profile.map_atlas)
        .unwrap_or_default();
    if atlas.cell_w == 0 || atlas.cell_h == 0 {
        atlas.cell_w = crate::ocr::CELL_W as u32;
        atlas.cell_h = crate::ocr::CELL_H as u32;
    }
    atlas
}

pub fn save_map_atlas(atlas: &Atlas) -> Result<(), String> {
    let atlas = atlas.clone();
    crate::profile::update(move |profile| {
        profile.map_atlas = atlas;
    })
    .map(|_| ())
}

pub fn load_level_atlas() -> Atlas {
    let mut atlas = crate::profile::active()
        .map(|profile| profile.level_atlas)
        .unwrap_or_default();
    if atlas.cell_w == 0 || atlas.cell_h == 0 {
        atlas.cell_w = crate::ocr::CELL_W as u32;
        atlas.cell_h = crate::ocr::CELL_H as u32;
    }
    atlas
}

pub fn save_level_atlas(atlas: &Atlas) -> Result<(), String> {
    let atlas = atlas.clone();
    crate::profile::update(move |profile| {
        profile.level_atlas = atlas;
    })
    .map(|_| ())
}

pub fn load_maps() -> Vec<Template> {
    crate::profile::active()
        .map(|profile| profile.maps)
        .unwrap_or_default()
}

pub fn save_maps(maps: &[Template]) -> Result<(), String> {
    let maps = maps.to_vec();
    crate::profile::update(move |profile| {
        profile.maps = maps;
    })
    .map(|_| ())
}

pub fn load_streets() -> Vec<Template> {
    crate::profile::active()
        .map(|profile| profile.streets)
        .unwrap_or_default()
}

pub fn save_streets(streets: &[Template]) -> Result<(), String> {
    let streets = streets.to_vec();
    crate::profile::update(move |profile| {
        profile.streets = streets;
    })
    .map(|_| ())
}

pub fn load_identities() -> Identities {
    crate::profile::active()
        .map(|profile| profile.identities)
        .unwrap_or_default()
}

pub fn save_identities(identities: &Identities) -> Result<(), String> {
    let identities = identities.clone();
    crate::profile::update(move |profile| {
        profile.identities = identities;
    })
    .map(|_| ())
}

pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return None;
    }

    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let bytes = [0u8, 1, 15, 16, 127, 255];
        assert_eq!(from_hex(&to_hex(&bytes)).unwrap(), bytes);
        assert_eq!(from_hex("abc"), None);
    }
}
