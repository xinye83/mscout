use std::path::PathBuf;

use crate::profile::Profile;
use crate::settings::UiMode;

/// Where presets are looked up, in order. A `data/presets/` file wins so a
/// preset can be corrected or added without rebuilding; otherwise the copy that
/// ships with the app is used.
fn dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(data) = crate::data_dir() {
        dirs.push(data.join("presets"));
    }

    if cfg!(debug_assertions) {
        dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("presets"));
    } else if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("presets"));
        }
    }

    dirs
}

/// The preset for a client size and UI Size, if one ships (or was dropped in). A
/// preset carries the boxes *and* the glyph set, so a known pair reads without
/// calibrating — only the map/name/job fingerprints stay per-player. A pair with
/// no preset falls back to calibration.
pub fn for_size(client_w: u32, client_h: u32, ui_mode: UiMode) -> Option<Profile> {
    let name = format!("{client_w}x{client_h}{}.json", ui_mode.suffix());

    for dir in dirs() {
        let Ok(text) = std::fs::read_to_string(dir.join(&name)) else {
            continue;
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let Ok(mut profile) = serde_json::from_str::<Profile>(text) else {
            continue;
        };
        if profile.client_w == client_w && profile.client_h == client_h {
            // The filename carries the mode, so a preset written before UI Size
            // was tracked still lands on its own mode.
            profile.ui_mode = ui_mode;
            return Some(profile);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_size_ships_with_boxes_and_glyphs() {
        let profile = for_size(1920, 1200, UiMode::Default).expect("preset for 1920x1200");
        assert_eq!(profile.key(), "1920x1200");
        assert!(profile.is_calibrated(), "preset must carry usable boxes");
        assert!(profile.has_glyphs(), "preset must carry a glyph set");
    }

    #[test]
    fn unknown_size_has_none() {
        assert!(for_size(1280, 720, UiMode::Default).is_none());
    }

    #[test]
    fn other_ui_modes_are_keyed_apart() {
        // Ideal Ratio ships its own set — different box, its own glyphs — rather
        // than borrowing the default mode's under another name.
        let default = for_size(1920, 1200, UiMode::Default).expect("default preset");
        let ideal = for_size(1920, 1200, UiMode::Ideal).expect("ideal preset");
        assert_eq!(ideal.key(), "1920x1200@ideal");
        assert_ne!(ideal.calibration.exp, default.calibration.exp);

        // A mode with no measured set must not borrow another mode's.
        assert!(for_size(1280, 720, UiMode::Ideal).is_none());
        assert!(for_size(1280, 720, UiMode::Filtered).is_none());
    }
}
