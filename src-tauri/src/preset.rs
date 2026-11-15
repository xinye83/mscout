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
/// preset carries the boxes only: the digit atlases are seeded from the installed
/// client (or taught), and the map/name/job fingerprints stay per-player. A pair
/// with no preset falls back to drawing its own boxes.
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
    fn known_size_ships_with_boxes() {
        // Presets ship geometry only. Glyphs are seeded from the installed
        // client's bitmap atlas (or taught) at runtime, so no game-derived art
        // is committed to the repository.
        let profile = for_size(1920, 1200, UiMode::Ideal).expect("preset for 1920x1200");
        assert_eq!(profile.key(), "1920x1200@ideal");
        assert!(profile.is_calibrated(), "preset must carry usable boxes");
    }

    #[test]
    fn shipped_presets_contain_no_game_art() {
        // D24: presets ship geometry only. No Nexon glyph atlases, map
        // fingerprints or identity templates may enter the repository.
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("presets");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("presets dir") {
            let path = entry.expect("preset entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("preset text");
            let profile: Profile = serde_json::from_str(&text).expect("preset json");
            assert!(
                profile.atlas.glyphs.is_empty(),
                "{} ships EXP glyph art",
                path.display()
            );
            assert!(
                profile.level_atlas.glyphs.is_empty(),
                "{} ships level glyph art",
                path.display()
            );
            assert!(
                profile.map_atlas.glyphs.is_empty(),
                "{} ships map glyph art",
                path.display()
            );
            assert!(
                profile.maps.is_empty() && profile.streets.is_empty(),
                "{} ships taught map fingerprints",
                path.display()
            );
            assert!(
                profile.identities.name.is_none() && profile.identities.job.is_none(),
                "{} ships identity fingerprints",
                path.display()
            );
            checked += 1;
        }
        assert!(checked >= 3, "expected the shipped preset set, found {checked}");
    }

    #[test]
    fn ideal_presets_cover_the_three_supported_resolutions() {
        for (w, h) in [(1366, 768), (1920, 1080), (1920, 1200)] {
            let profile = for_size(w, h, UiMode::Ideal)
                .unwrap_or_else(|| panic!("Ideal preset for {w}x{h}"));
            assert!(profile.is_calibrated(), "{w}x{h} has no usable boxes");
            assert!(
                profile.calibration.level.is_some(),
                "{w}x{h} Ideal preset has no level box"
            );
            assert!(
                profile.calibration.name.is_some() && profile.calibration.job.is_some(),
                "{w}x{h} Ideal preset has no name/job boxes"
            );
        }
    }

    #[test]
    fn unknown_size_has_none() {
        assert!(for_size(1280, 720, UiMode::Default).is_none());
    }

    #[test]
    fn verified_modes_ship_presets_and_the_rest_fall_back() {
        // 1366x768 is verified in all three UI modes. The modes share the same
        // HUD geometry, and the saved-frame corpus reads every mode and minimap
        // state with the shipped boxes.
        for mode in UiMode::ALL {
            let profile = for_size(1366, 768, mode)
                .unwrap_or_else(|| panic!("1366x768 preset for {mode:?}"));
            assert!(
                profile.is_calibrated(),
                "1366x768 {mode:?} has no usable boxes"
            );
            assert!(profile.calibration.level.is_some());
            assert!(
                profile.calibration.name.is_some() && profile.calibration.job.is_some(),
                "1366x768 {mode:?} has no name/job boxes"
            );
        }

        // 1920x1080 ships all three UI modes. Default and Filtered share their
        // geometry; the saved-frame corpus reads EXP, level and name.
        for mode in UiMode::ALL {
            let profile = for_size(1920, 1080, mode)
                .unwrap_or_else(|| panic!("1920x1080 preset for {mode:?}"));
            assert!(
                profile.is_calibrated(),
                "1920x1080 {mode:?} has no usable boxes"
            );
            assert!(profile.calibration.level.is_some());
            assert!(
                profile.calibration.name.is_some() && profile.calibration.job.is_some(),
                "1920x1080 {mode:?} has no name/job boxes"
            );
        }

        // 1920x1200 is verified at Ideal only; Default/Filtered must fall back to
        // a local calibration pass instead of borrowing Ideal geometry.
        assert!(for_size(1920, 1200, UiMode::Ideal).is_some());
        assert!(for_size(1920, 1200, UiMode::Default).is_none());
        assert!(for_size(1920, 1200, UiMode::Filtered).is_none());

        // A size with no measured set must not borrow another size's.
        assert!(for_size(1280, 720, UiMode::Ideal).is_none());
        assert!(for_size(1280, 720, UiMode::Filtered).is_none());
    }
}
