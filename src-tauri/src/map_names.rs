//! The client's own map/street name table.
//!
//! `Data/String/String_000.wz` → `Map.img/<region>/<map id>` carries `mapName`
//! and `streetName`. Both minimap label lines are exactly these strings, so a
//! rendered-name match can identify a map without teaching or OCR segmentation.

use std::path::{Path, PathBuf};

use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;

#[derive(Clone, Debug)]
pub struct MapEntry {
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
    let node = root.read().ok()?.at_path_parsed(path).ok()?;
    let guard = node.read().ok()?;
    guard.try_as_string()?.get_string().ok()
}

/// Every `mapName`/`streetName` pair in the client's map string table.
/// The client's map table, cached per `String_000.wz` (path and modification
/// time). Parsing the archive takes long enough to matter on every call: the
/// calibration window asks for the table's size on open and after each freeze,
/// and the label reader asks for it on every OCR resolution.
pub fn load(mscw_dir: &Path) -> Result<Vec<MapEntry>, String> {
    type Cached = (PathBuf, Option<std::time::SystemTime>, Vec<MapEntry>);
    static CACHE: std::sync::Mutex<Option<Cached>> = std::sync::Mutex::new(None);

    let path = string_wz(mscw_dir)
        .ok_or_else(|| format!("no String_000.wz under {}", mscw_dir.display()))?;
    let modified = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_path, cached_time, entries)) = cache.as_ref() {
        if *cached_path == path && *cached_time == modified {
            return Ok(entries.clone());
        }
    }
    let entries = parse(&path)?;
    *cache = Some((path, modified, entries.clone()));
    Ok(entries)
}

/// Read every map's name and street from `String_000.wz`.
fn parse(path: &Path) -> Result<Vec<MapEntry>, String> {
    let root = crate::game_font::open_wz(path)?;

    let mut out = Vec::new();
    for region in children(&root, "Map.img") {
        for id in children(&root, &format!("Map.img/{region}")) {
            let base = format!("Map.img/{region}/{id}");
            let name = string_at(&root, &format!("{base}/mapName")).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let street = string_at(&root, &format!("{base}/streetName")).unwrap_or_default();
            out.push(MapEntry { name, street });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_font::open_wz;

    fn mscw() -> PathBuf {
        PathBuf::from(
            std::env::var("MSCOUT_MSCW_DIR")
                .ok()
                .or_else(crate::settings::mscw_dir)
                .expect("no MSCW dir; set MSCOUT_MSCW_DIR"),
        )
    }

    const JOB_NAMES: &[&str] = &[
        "Beginner",
        "Warrior",
        "Magician",
        "Bowman",
        "Thief",
        "Pirate",
        "Fighter",
        "Page",
        "Spearman",
        "Cleric",
        "Hunter",
        "Crossbowman",
        "Assassin",
        "Bandit",
        "Brawler",
        "Gunslinger",
        "Hero",
        "Paladin",
        "Dark Knight",
        "Arch Mage",
        "Bishop",
        "Bowmaster",
        "Marksman",
        "Night Lord",
        "Shadower",
        "Buccaneer",
        "Corsair",
    ];

    fn scan_job_strings(root: &WzNodeArc, path: &str, depth: usize) {
        if depth > 6 {
            return;
        }
        if let Some(value) = string_at(root, path) {
            let value = value.trim();
            if JOB_NAMES.contains(&value) {
                println!("  JOB {path} = {value:?}");
            }
        }
        for child in children(root, path) {
            let next = if path.is_empty() {
                child
            } else {
                format!("{path}/{child}")
            };
            scan_job_strings(root, &next, depth + 1);
        }
    }

    #[test]
    #[ignore = "diagnostic: searches the client map table for Deep Valley"]
    fn probe_deep_valley() {
        let entries = load(&mscw()).expect("map table");
        for entry in &entries {
            if entry.name.contains("Deep Valley") || entry.street.contains("Warning") {
                println!("{:?}", entry);
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: searches the game data for job names"]
    fn probe_string_jobs() {
        let base = mscw();

        for dir in [base.join("Data/String"), base.join("MSCW/Data/String")] {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_none_or(|extension| !extension.eq_ignore_ascii_case("wz"))
                {
                    continue;
                }
                let Ok(root) = open_wz(&path) else {
                    continue;
                };
                let roots = children(&root, "");
                println!("== {} root: {roots:?}", path.display());
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("String_000.wz"))
                {
                    for top in [
                        "StringTable.img",
                        "StringTable_WZ2.img",
                        "UI.img",
                        "Etc.img",
                        "NameChange.img",
                        "Consume.img",
                    ] {
                        if roots.iter().any(|root_name| root_name == top) {
                            println!("  scanning {top}");
                            scan_job_strings(&root, top, 0);
                        }
                    }
                }
            }
        }

        for rel in [
            "Data/Etc/Etc_000.wz",
            "Data/Etc/Etc.wz",
            "Data/UI/UI_000.wz",
            "Data/UI/UI.wz",
            "Data/Skill/Skill.wz",
            "Data/Character/Character.wz",
        ] {
            let path = base.join(rel);
            if !path.is_file() {
                continue;
            }
            let Ok(root) = open_wz(&path) else {
                continue;
            };
            let roots = children(&root, "");
            println!("== {} root: {roots:?}", path.display());
            for top in &roots {
                scan_job_strings(&root, top, 0);
            }
        }

        let etc000 = base.join("Data/Etc/Etc_000.wz");
        if let Ok(root) = open_wz(&etc000) {
            for top in [
                "MakeCharInfo.img",
                "FreeJobChange.img",
                "RecommendedSkill.img",
            ] {
                let entries = children(&root, top);
                if entries.is_empty() {
                    continue;
                }
                println!(
                    "== {top} entries ({}): {:?}",
                    entries.len(),
                    &entries[..entries.len().min(40)]
                );
                for id in entries.iter().take(40) {
                    let direct = string_at(&root, &format!("{top}/{id}"));
                    let named = string_at(&root, &format!("{top}/{id}/name"));
                    let sub = children(&root, &format!("{top}/{id}"));
                    println!("    {id}: direct={direct:?} name={named:?} sub={sub:?}");
                }
            }
        }

        let etc = base.join("Data/Etc/Etc.wz");
        if let Ok(root) = open_wz(&etc) {
            let languages = children(&root, "Language");
            println!("== Language entries ({}): {languages:?}", languages.len());
            for language in languages.iter().take(2) {
                let entries = children(&root, &format!("Language/{language}"));
                println!(
                    "    {language}: {} entries {:?}",
                    entries.len(),
                    &entries[..entries.len().min(40)]
                );
                for id in entries.iter().take(20) {
                    let direct = string_at(&root, &format!("Language/{language}/{id}"));
                    let named = string_at(&root, &format!("Language/{language}/{id}/name"));
                    println!("      {id}: direct={direct:?} name={named:?}");
                }
            }
        }

        for dir in [base.join("Data"), base.join("MSCW/Data")] {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_none_or(|extension| !extension.eq_ignore_ascii_case("wz"))
                {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if !(name.eq_ignore_ascii_case("Etc.wz") || name.eq_ignore_ascii_case("UI.wz")) {
                    continue;
                }
                let Ok(root) = open_wz(&path) else {
                    continue;
                };
                let roots = children(&root, "");
                println!("== {} root: {roots:?}", path.display());
                for top in &roots {
                    scan_job_strings(&root, top, 0);
                }
            }
        }
    }
}
