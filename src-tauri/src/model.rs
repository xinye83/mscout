//! The calibration and learning data a profile stores: boxes, glyph atlases and
//! taught fingerprints. Plain serde types with no I/O; `profile` owns loading
//! and saving them, `settings` owns the app-wide settings file.

use serde::{Deserialize, Serialize};

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
    /// A read can be attempted once the EXP box is set. The level box is optional:
    /// without it the level is reported as unrecognised, never derived.
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
    /// Native glyph bitmap, row-major, `w*h` bytes of `0x00`/`0xff`.
    #[serde(default)]
    pub bitmap: String,
    /// Legacy 12x16 resampled cell. Read only, so profiles written before the
    /// native-bitmap format can still be migrated when they are loaded.
    #[serde(default, skip_serializing)]
    pub cell: String,
}

/// A glyph atlas as stored in a profile (hex bitmaps). `ocr::Atlas` is the
/// decoded form the readers match against.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAtlas {
    /// `2` = native bitmaps. Older profiles carry 12x16 cells and are migrated
    /// on read. The encoder is a data format: a change invalidates stored cells.
    #[serde(default)]
    pub format: u32,
    /// Where the set came from: `game` (seeded from the client atlas), `taught`
    /// (typed by the player), or empty (a preset / legacy set).
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub cell_w: u32,
    #[serde(default)]
    pub cell_h: u32,
    #[serde(default)]
    pub glyphs: Vec<GlyphCell>,
}

impl StoredAtlas {
    /// A hash of the glyphs, so a decoded copy can be reused until the stored
    /// set changes (decoding hex into bitmaps and cells every tick is waste).
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.format.hash(&mut hasher);
        for glyph in &self.glyphs {
            (&glyph.ch, glyph.w, glyph.h, &glyph.bitmap, &glyph.cell).hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The most common stored glyph size, e.g. `5x7`. It is the size the atlas is
    /// actually working at, so the UI can report it next to `game` / `taught`.
    pub fn common_glyph_size(&self) -> String {
        use std::collections::HashMap;

        let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
        for glyph in &self.glyphs {
            *counts.entry((glyph.w, glyph.h)).or_default() += 1;
        }
        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|((w, h), _)| format!("{w}x{h}"))
            .unwrap_or_default()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(ch: &str, bitmap: &str) -> GlyphCell {
        GlyphCell {
            ch: ch.into(),
            w: 1,
            h: 2,
            bitmap: bitmap.into(),
            cell: String::new(),
        }
    }

    #[test]
    fn the_fingerprint_follows_the_glyphs_only() {
        let mut atlas = StoredAtlas {
            glyphs: vec![glyph("1", "ffff")],
            ..StoredAtlas::default()
        };
        let before = atlas.fingerprint();
        atlas.source = "game".into();
        assert_eq!(
            atlas.fingerprint(),
            before,
            "the label is not part of the glyphs"
        );
        atlas.glyphs[0] = glyph("1", "ff00");
        assert_ne!(atlas.fingerprint(), before);
    }
}
