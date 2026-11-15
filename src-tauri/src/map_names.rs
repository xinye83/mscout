//! The client's own map/street name table.
//!
//! `Data/String/String_000.wz` → `Map.img/<region>/<map id>` carries `mapName`
//! and `streetName`. Both minimap label lines are exactly these strings, so a
//! rendered-name match can identify a map without teaching or OCR segmentation.

use std::path::{Path, PathBuf};

use wz_reader::WzNodeCast;
use wz_reader::WzNodeArc;

#[derive(Clone, Debug)]
pub struct MapEntry {
    #[allow(dead_code)] // kept for future map-id lookups
    pub id: String,
    pub name: String,
    pub street: String,
}

fn string_wz(mscw_dir: &Path) -> Option<PathBuf> {
    [
        mscw_dir.join("Data/String/String_000.wz"),
        mscw_dir.join("MSCW/Data/String/String_000.wz"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn children(root: &WzNodeArc, path: &str) -> Vec<String> {
    let node = if path.is_empty() {
        root.clone()
    } else {
        let Ok(node) = root.read().expect("wz root").at_path_parsed(path) else {
            return Vec::new();
        };
        node
    };
    {
        let mut guard = node.write().expect("wz node");
        let _ = guard.parse(&node);
    }
    let mut names: Vec<String> = node
        .read()
        .expect("wz node")
        .children
        .keys()
        .map(|name| name.as_str().to_string())
        .collect();
    names.sort();
    names
}

fn string_at(root: &WzNodeArc, path: &str) -> Option<String> {
    let node = root
        .read()
        .ok()?
        .at_path_parsed(path)
        .ok()?;
    let guard = node.read().ok()?;
    guard.try_as_string()?.get_string().ok()
}

/// Every `mapName`/`streetName` pair in the client's map string table.
pub fn load(mscw_dir: &Path) -> Result<Vec<MapEntry>, String> {
    let path = string_wz(mscw_dir)
        .ok_or_else(|| format!("no String_000.wz under {}", mscw_dir.display()))?;
    let root = crate::game_font::open_wz(&path)?;

    let mut out = Vec::new();
    for region in children(&root, "Map.img") {
        for id in children(&root, &format!("Map.img/{region}")) {
            let base = format!("Map.img/{region}/{id}");
            let name = string_at(&root, &format!("{base}/mapName")).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let street = string_at(&root, &format!("{base}/streetName")).unwrap_or_default();
            out.push(MapEntry {
                id: id.replace(".img", ""),
                name,
                street,
            });
        }
    }
    Ok(out)
}
