use std::path::PathBuf;

use crate::profile::Profile;
use crate::settings::{Region, UiMode};

/// Name/job boxes that earlier presets shipped, with the width that replaced
/// them. The old boxes were measured on one character and only fit "Page" and
/// "Euroblood": a longer job such as "Beginner", or a longer name, was clipped
/// and reported unrecognised. The new widths run to just short of the divider
/// that closes the level panel, measured on every `testdata/` HUD frame.
///
/// (client width, client height, old box, new width). The 1366×768 boxes are
/// shared by all three UI Sizes. 1920×1080 Default and Filtered keep their boxes
/// for now: their anti-aliased name/job reader is sensitive to how much empty
/// panel follows the text, and the corpus has one name and one job to check a
/// wider box against.
const NARROW_IDENTITY_BOXES: &[(u32, u32, Region, i32)] = &[
    (
        1366,
        768,
        Region {
            x: 366,
            y: 737,
            w: 34,
            h: 12,
        },
        120,
    ),
    (
        1366,
        768,
        Region {
            x: 366,
            y: 751,
            w: 60,
            h: 13,
        },
        120,
    ),
    (
        1920,
        1080,
        Region {
            x: 645,
            y: 1050,
            w: 27,
            h: 11,
        },
        118,
    ),
    (
        1920,
        1080,
        Region {
            x: 645,
            y: 1064,
            w: 55,
            h: 9,
        },
        118,
    ),
];

/// Widen a saved profile's name/job box that is still exactly an old shipped
/// one. A box the player drew is left alone, and so is a box with a taught
/// fingerprint, because the fingerprint is cropped to the box and would stop
/// matching. Returns whether anything changed.
pub fn widen_identity_boxes(profile: &mut Profile) -> bool {
    let (w, h) = (profile.client_w, profile.client_h);
    let widen = |region: &mut Option<Region>, taught: bool| -> bool {
        let Some(current) = region.as_mut() else {
            return false;
        };
        if taught {
            return false;
        }
        match NARROW_IDENTITY_BOXES
            .iter()
            .find(|(cw, ch, old, _)| (*cw, *ch) == (w, h) && old == current)
        {
            Some((_, _, _, new_w)) => {
                current.w = *new_w;
                true
            }
            None => false,
        }
    };
    let name = widen(
        &mut profile.calibration.name,
        profile.identities.name.is_some(),
    );
    let job = widen(
        &mut profile.calibration.job,
        profile.identities.job.is_some(),
    );
    name || job
}

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

    fn old_ideal_profile() -> Profile {
        let mut profile = Profile::new_for_mode(1920, 1080, UiMode::Ideal);
        profile.calibration.job = Some(Region {
            x: 645,
            y: 1050,
            w: 27,
            h: 11,
        });
        profile.calibration.name = Some(Region {
            x: 645,
            y: 1064,
            w: 55,
            h: 9,
        });
        profile
    }

    #[test]
    fn old_shipped_identity_boxes_are_widened() {
        let mut profile = old_ideal_profile();
        assert!(widen_identity_boxes(&mut profile));
        assert_eq!(profile.calibration.job.map(|r| r.w), Some(118));
        assert_eq!(profile.calibration.name.map(|r| r.w), Some(118));
        assert!(!widen_identity_boxes(&mut profile), "widening runs once");
    }

    #[test]
    fn drawn_or_taught_identity_boxes_are_left_alone() {
        let mut drawn = old_ideal_profile();
        drawn.calibration.job = Some(Region {
            x: 640,
            y: 1048,
            w: 60,
            h: 14,
        });
        drawn.identities.name = Some(crate::settings::Template {
            label: "Euroblood".into(),
            w: 55,
            h: 9,
            pixels: String::new(),
        });
        assert!(!widen_identity_boxes(&mut drawn));
        assert_eq!(drawn.calibration.job.map(|r| r.w), Some(60));
        assert_eq!(drawn.calibration.name.map(|r| r.w), Some(55));
    }

    #[test]
    fn shipped_presets_use_the_wide_identity_boxes() {
        for (w, h, mode) in [
            (1366, 768, UiMode::Default),
            (1366, 768, UiMode::Ideal),
            (1366, 768, UiMode::Filtered),
            (1920, 1080, UiMode::Ideal),
        ] {
            let mut profile = for_size(w, h, mode).expect("shipped preset");
            assert!(
                !widen_identity_boxes(&mut profile),
                "{} still ships a narrow name/job box",
                profile.key()
            );
        }
    }

    #[test]
    fn known_size_ships_with_boxes() {
        // Presets ship geometry only. Glyphs are seeded from the installed
        // client's bitmap atlas (or taught) at runtime, so no game-derived art
        // is committed to the repository.
        let profile = for_size(1920, 1080, UiMode::Ideal).expect("preset for 1920x1080");
        assert_eq!(profile.key(), "1920x1080@ideal");
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
        assert!(
            checked >= 3,
            "expected the shipped preset set, found {checked}"
        );
    }

    #[test]
    fn ideal_presets_cover_the_two_supported_resolutions() {
        for (w, h) in [(1366, 768), (1920, 1080)] {
            let profile =
                for_size(w, h, UiMode::Ideal).unwrap_or_else(|| panic!("Ideal preset for {w}x{h}"));
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
        // The 1920x1200 preset was removed; it falls back to calibration.
        assert!(for_size(1920, 1200, UiMode::Ideal).is_none());
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
            let profile =
                for_size(1366, 768, mode).unwrap_or_else(|| panic!("1366x768 preset for {mode:?}"));
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

        // 1920x1080 ships all three UI modes from 0.6.0. Ideal has its own
        // geometry; Default and Filtered share the measured set.
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

        // 1920x1200 has no shipped preset; every mode falls back to a local
        // calibration pass instead of borrowing geometry from another size.
        assert!(for_size(1920, 1200, UiMode::Ideal).is_none());
        assert!(for_size(1920, 1200, UiMode::Default).is_none());
        assert!(for_size(1920, 1200, UiMode::Filtered).is_none());

        // A size with no measured set must not borrow another size's.
        assert!(for_size(1280, 720, UiMode::Ideal).is_none());
        assert!(for_size(1280, 720, UiMode::Filtered).is_none());
    }
}
