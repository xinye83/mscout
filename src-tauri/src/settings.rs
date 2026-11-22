use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use crate::model::{Calibration, GlyphCell, Identities, Region, StoredAtlas, Template};

/// `settings.json` as last read or written, keyed by its path, behind the one
/// lock every read and write takes. The engine asks for the UI Size every tick;
/// reading it from disk each time raced the in-place rewrite done by overlay
/// drags, and a torn read fell back to the Default UI Size and activated the
/// wrong profile. The file is read once and then only ever replaced by
/// [`update`], so edits made to it by hand while the app runs are not picked up.
static SETTINGS: Mutex<Option<(std::path::PathBuf, Value)>> = Mutex::new(None);

type Cache = std::sync::MutexGuard<'static, Option<(std::path::PathBuf, Value)>>;

fn lock() -> Cache {
    SETTINGS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn path() -> Result<std::path::PathBuf, String> {
    Ok(crate::data_dir()?.join("settings.json"))
}

pub fn read_raw() -> Value {
    let Ok(path) = path() else {
        return Value::Object(Map::new());
    };
    let mut cache = lock();
    if let Some((cached_path, value)) = cache.as_ref() {
        if *cached_path == path {
            return value.clone();
        }
    }
    let value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| parse_object(&raw))
        .unwrap_or_else(|| Value::Object(Map::new()));
    *cache = Some((path, value.clone()));
    value
}

/// Parse settings text as a JSON object. A UTF-8 BOM (e.g. from an editor or
/// PowerShell's Set-Content) would otherwise make serde_json reject the file.
fn parse_object(raw: &str) -> Option<Value> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    serde_json::from_str::<Value>(raw)
        .ok()
        .filter(Value::is_object)
}

pub fn update<F>(apply: F) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>),
{
    let mut cache = lock();
    let dir = crate::data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("settings.json");

    // Never write `{}` plus one key over a file that merely failed to parse: that
    // would erase the target window, MSCW folder and UI Size. An unparseable file
    // is moved aside first; a file that cannot be read at all is left alone.
    let mut raw = match std::fs::read_to_string(&file) {
        Ok(text) => match parse_object(&text) {
            Some(value) => value,
            None => {
                let backup = crate::fsutil::move_aside(&file).map_err(|error| {
                    format!("cannot move aside unreadable {}: {error}", file.display())
                })?;
                crate::calibration::log(&format!(
                    "settings.json could not be parsed; kept it as {}",
                    backup.display()
                ));
                Value::Object(Map::new())
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", file.display())),
    };

    {
        let object = raw
            .as_object_mut()
            .ok_or("settings.json must contain an object")?;
        apply(object);
    }

    let text = serde_json::to_string_pretty(&raw).map_err(|e| e.to_string())?;
    crate::fsutil::write_atomic(&file, text.as_bytes()).map_err(|e| e.to_string())?;
    *cache = Some((file, raw));
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowSelector {
    pub title: String,
    pub process: String,
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
    let _timed = crate::calibration::Timed::new("ui_modes");
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
pub fn set_ui_mode(mode: UiMode) -> Result<(), String> {
    let _timed = crate::calibration::Timed::new("set_ui_mode");
    save_ui_mode(mode)
}

/// The player's MapleStory MSCW folder, used to seed HUD glyphs from the
/// client's own bitmap atlas. Never bundled: read from the install on demand.
pub fn mscw_dir() -> Option<String> {
    read_raw()
        .get("mscwDir")
        .and_then(Value::as_str)
        .map(|path| path.to_string())
}

pub fn save_mscw_dir(path: Option<&str>) -> Result<(), String> {
    let path = path.map(|value| value.to_string());
    update(move |object| match path {
        Some(path) => {
            object.insert("mscwDir".into(), Value::String(path));
        }
        None => {
            object.remove("mscwDir");
        }
    })
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

/// Where the player last dragged the overlay, in physical pixels.
///
/// Stored apart from the computed anchor: the anchor follows the game window, and
/// a dragged panel must not be yanked back to it on the next tick or restart.
/// `None` means "never dragged", which is what lets the anchor place the panel.
pub fn load_overlay_position() -> Option<(i32, i32)> {
    parse_overlay_position(&read_raw())
}

/// Read a position out of a settings object.
///
/// Split from the file read so a malformed or half-written value is testable: a
/// position that does not parse must read as "never dragged", never as `(0, 0)`,
/// which would move the panel to the top-left corner.
fn parse_overlay_position(value: &Value) -> Option<(i32, i32)> {
    let position = value.get("overlayPosition")?;
    Some((
        position.get("x")?.as_i64()? as i32,
        position.get("y")?.as_i64()? as i32,
    ))
}

pub fn save_overlay_position(x: i32, y: i32) -> Result<(), String> {
    update(move |object| {
        let mut position = Map::new();
        position.insert("x".into(), Value::from(x));
        position.insert("y".into(), Value::from(y));
        object.insert("overlayPosition".into(), Value::Object(position));
    })
}

pub fn clear_overlay_position() -> Result<(), String> {
    update(|object| {
        object.remove("overlayPosition");
    })
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
    fn settings_parse_accepts_a_bom_and_rejects_anything_but_an_object() {
        assert!(parse_object("\u{feff}{\"uiMode\":\"ideal\"}").is_some());
        assert!(parse_object("{}").is_some());
        assert!(parse_object("{ torn").is_none());
        assert!(parse_object("[1, 2]").is_none());
        assert!(parse_object("").is_none());
    }

    #[test]
    fn hex_round_trip() {
        let bytes = [0u8, 1, 15, 16, 127, 255];
        assert_eq!(from_hex(&to_hex(&bytes)).unwrap(), bytes);
        assert_eq!(from_hex("abc"), None);
    }

    #[test]
    fn a_saved_overlay_position_round_trips() {
        let value: Value = serde_json::json!({
            "overlayPosition": { "x": 1690, "y": 285 }
        });
        assert_eq!(parse_overlay_position(&value), Some((1690, 285)));
    }

    #[test]
    fn a_negative_position_is_kept() {
        // Dragging onto a monitor left of the primary one gives negative coordinates,
        // which must not be clamped away.
        let value: Value = serde_json::json!({
            "overlayPosition": { "x": -1920, "y": -40 }
        });
        assert_eq!(parse_overlay_position(&value), Some((-1920, -40)));
    }

    #[test]
    fn a_missing_or_broken_position_means_never_dragged() {
        // Never `(0, 0)`: that would silently move the panel to the top-left corner.
        for value in [
            serde_json::json!({}),
            serde_json::json!({ "overlayPosition": null }),
            serde_json::json!({ "overlayPosition": {} }),
            serde_json::json!({ "overlayPosition": { "x": 10 } }),
            serde_json::json!({ "overlayPosition": { "x": "10", "y": 20 } }),
            serde_json::json!({ "overlayPosition": [10, 20] }),
            serde_json::json!("not an object"),
        ] {
            assert_eq!(
                parse_overlay_position(&value),
                None,
                "must not invent a position from {value}"
            );
        }
    }

    #[test]
    fn the_position_is_independent_of_the_other_settings() {
        // A drag must survive alongside the target and UI Size, and vice versa.
        let value: Value = serde_json::json!({
            "mscwDir": "D:\\MapleStory\\MSCW",
            "uiMode": "ideal",
            "targetWindow": { "title": "MapleStory", "process": "MapleStory.exe" },
            "overlayPosition": { "x": 1690, "y": 285 }
        });
        assert_eq!(parse_overlay_position(&value), Some((1690, 285)));
    }
}
