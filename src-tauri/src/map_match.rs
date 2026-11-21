//! The minimap label reader: street and map name from the expanded, folded and
//! title-bar minimap, matched against the client's own map table, Windows OCR
//! and taught fingerprints. Moved out of `engine.rs`, which keeps the tick and
//! the HUD readers.

use std::cmp::Ordering;

use crate::capture::Frame;
use crate::engine::{log_once, ReadSource};
use crate::ocr::{self, Atlas};
use crate::profile;
use crate::settings::{self, Calibration, Region, Template};

/// How far a fingerprint may be from a taught map or street label and still
/// match it.
pub(crate) const MAP_DISTANCE_LIMIT: f64 = 12.0;
/// How close a read label has to be to a taught one before the taught name wins.
/// Letters are read one glyph at a time, so a near miss is expected now and then.
pub(crate) const MAP_SIMILARITY: f64 = 0.75;

/// Label font sizes tried by the whole-line matcher. Ideal is 12 px; Default
/// Ratio draws the same label larger, so the matcher picks the size whose
/// rendered width and shape sequence explain the live line best.
pub(crate) const LABEL_SIZES: [u32; 7] = [12, 14, 16, 18, 20, 22, 24];

pub(crate) fn match_street(
    frame: &Frame,
    region: Option<Region>,
    streets: &[Template],
) -> Option<String> {
    if let Some(region) = region.filter(Region::is_usable) {
        if let Some(street) = match_street_fingerprint(frame, region, streets) {
            return Some(street);
        }
    }

    // Nothing taught matched (or no box was drawn): the client's own map table
    // has the street line, and detection finds it without a stored box.
    let dir = settings::mscw_dir()?;
    let lines = ocr::map_label_lines(frame)?;
    if lines.len() < 2 {
        return None;
    }
    let line = lines.first()?;
    let mut best: Option<(String, f64)> = None;
    for px in LABEL_SIZES {
        let Some(index) = crate::font_seed::label_index(std::path::Path::new(&dir), px) else {
            continue;
        };
        let max_score = if px >= 14 { 0.95 } else { 0.55 };
        if let Some((street, score)) =
            crate::font_seed::match_label(line, &index.streets, 14, max_score)
        {
            if best.as_ref().map_or(true, |(_, current)| score < *current) {
                best = Some((street, score));
            }
        }
    }
    let (street, score) = best?;
    log_once(&format!(
        "street from the client's map table: {street:?} (score {score:.2})"
    ));
    Some(street)
}

pub(crate) fn match_street_fingerprint(
    frame: &Frame,
    region: Region,
    streets: &[Template],
) -> Option<String> {
    let crop = ocr::crop_gray(frame, region)?;
    let mut best: Option<(&str, f64)> = None;
    for template in streets {
        let Some(bytes) = settings::from_hex(&template.pixels) else {
            continue;
        };
        if template.w as usize != crop.w
            || template.h as usize != crop.h
            || bytes.len() != crop.px.len()
        {
            continue;
        }
        let candidate = ocr::Gray {
            w: crop.w,
            h: crop.h,
            px: bytes,
        };
        if let Some(distance) = ocr::map_distance(&crop, &candidate) {
            if best.map_or(true, |(_, closest)| distance < closest) {
                best = Some((&template.label, distance));
            }
        }
    }
    match best {
        Some((street, distance)) if distance <= MAP_DISTANCE_LIMIT => Some(street.to_string()),
        _ => None,
    }
}

pub(crate) fn template_gray(template: &Template) -> Option<ocr::Gray> {
    let bytes = settings::from_hex(&template.pixels)?;
    let w = template.w as usize;
    let h = template.h as usize;
    if w == 0 || h == 0 || bytes.len() != w * h {
        return None;
    }
    let gray = ocr::Gray { w, h, px: bytes };
    if w == ocr::MAP_LABEL_W && h == ocr::MAP_LABEL_H {
        Some(gray)
    } else {
        Some(ocr::resample(&gray, ocr::MAP_LABEL_W, ocr::MAP_LABEL_H))
    }
}

pub(crate) fn match_map(
    frame: &Frame,
    calibration: &Calibration,
    templates: &[Template],
) -> (String, ReadSource) {
    // A taught fingerprint is an exact picture of that map, so it wins. The letter
    // atlas comes from a very small label (4-6 px glyphs in the Ideal Ratio HUD) and
    // can be learned misaligned — every shape then matches at distance 0 with the
    // wrong character, which no consistency check can catch.
    if let Some(name) = match_map_fingerprint(frame, calibration, templates) {
        return (name, ReadSource::Table);
    }

    // OCR is the better text source for the Roman-numeral siblings and for maps
    // the client table does not contain. An exact table match from OCR outranks
    // the whole-line DTW, which can read a single trailing `I` as `III`.
    let label_ocr = match_label_ocr(frame);
    if let Some(label) = label_ocr.as_ref() {
        if label.exact {
            if let Some(name) = label.map.as_ref() {
                log_once(&format!("map text from OCR (exact): {name:?}"));
                return (name.clone(), ReadSource::Ocr);
            }
        }
    }

    // Nothing taught matched: the client's own map table has the exact name. The
    // label is Arial Bold 12, so a width filter plus a shape-sequence DTW against
    // the rendered names identifies it without segmenting the line.
    if let Some(name) = match_map_name(frame) {
        return (name, ReadSource::Dtw);
    }

    // A map that is not in the client table still has its name on screen. OCR is
    // the better text source than the tiny map glyph atlas, which can be learned
    // misaligned and confidently return the wrong letters. `match_label_ocr`
    // snaps known names to the table and otherwise returns the literal line.
    if let Some(label) = label_ocr {
        if let Some(name) = label.map {
            log_once(&format!("map text from OCR: {name:?}"));
            return (name, label.map_source);
        }
    }

    // No OCR (or no label): fall back to the learned glyph text. A near miss
    // snaps to a name that was taught explicitly. Both outcomes come from the
    // learned letter atlas, so they are reported as `glyphs` — a support reader
    // must not mistake them for OCR text or for the rendered-name DTW above.
    let map_atlas = Atlas::from_settings(&profile::read_atlas(|p| &p.map_atlas));
    if let Some(lines) = ocr::read_map_text(frame, &map_atlas) {
        // The map name is the bottom line; the street is matched separately.
        let text = lines.last().cloned().unwrap_or_default();
        if !text.trim().is_empty() {
            log_once(&format!("map text fallback: {text:?}"));
            let best = templates
                .iter()
                .map(|template| {
                    (
                        template.label.clone(),
                        ocr::label_similarity(&text, &template.label),
                    )
                })
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));

            let name = match best {
                Some((label, similarity)) if similarity >= MAP_SIMILARITY => label,
                _ => text,
            };
            return (name, ReadSource::Glyphs);
        }
    }

    ("unknown".into(), ReadSource::Unknown)
}

/// Last expanded-label result, keyed by the label's pixels and the client name
/// table. The label only changes on a map transition, so a stationary frame
/// skips the table/DTW/OCR work entirely.
pub(crate) static MAP_CACHE: std::sync::Mutex<Option<(u64, MapRead)>> = std::sync::Mutex::new(None);

pub(crate) fn map_label_hash(frame: &Frame) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let lines = ocr::map_label_lines(frame)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for line in &lines {
        line.w.hash(&mut hasher);
        line.h.hash(&mut hasher);
        line.px.hash(&mut hasher);
    }
    settings::mscw_dir().hash(&mut hasher);
    Some(hasher.finish())
}

pub(crate) fn folded_label_hash(label: &ocr::Gray) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    label.w.hash(&mut hasher);
    label.h.hash(&mut hasher);
    label.px.hash(&mut hasher);
    settings::mscw_dir().hash(&mut hasher);
    hasher.finish()
}

/// The minimap state's answer: the map name, the street line when the label has
/// one, whether the minimap has no text at all, which stage named the map, and why
/// no map was named when none was.
#[derive(Clone, Debug)]
pub(crate) struct MapRead {
    pub(crate) name: String,
    pub(crate) street: Option<String>,
    pub(crate) hidden: bool,
    pub(crate) source: ReadSource,
    /// The stage's own reason for an unnamed map, carried into the provenance the
    /// same way `LevelRead::reason` is.
    pub(crate) reason: Option<String>,
}

impl MapRead {
    pub(crate) fn unknown(reason: impl Into<String>) -> Self {
        Self {
            name: "unknown".into(),
            street: None,
            hidden: false,
            source: ReadSource::Unknown,
            reason: Some(reason.into()),
        }
    }
}

/// Resolve the map and street for the minimap state on this frame.
///
/// Expanded reads the two-line label; folded reads the single `Street :Map` bar;
/// title-bar-only has no text at all and returns nothing. A frame with no
/// detectable minimap keeps the old fingerprint/name-table path.
pub(crate) fn match_map_and_street(
    frame: &Frame,
    calibration: &Calibration,
    maps: &[Template],
    streets: &[Template],
    minimap: Option<ocr::Minimap>,
) -> MapRead {
    match minimap.as_ref().map(|minimap| minimap.state) {
        Some(ocr::MinimapState::TitleBarOnly) => MapRead {
            name: "unknown".into(),
            street: None,
            hidden: true,
            source: ReadSource::Hidden,
            reason: Some("the minimap is collapsed — there is no map name on screen".into()),
        },
        Some(ocr::MinimapState::Folded) => {
            let Some(minimap) = minimap.as_ref() else {
                return MapRead::unknown("no minimap was detected");
            };
            let Some(label) = ocr::folded_label(frame, minimap) else {
                log_once("folded minimap: no label crop");
                return MapRead::unknown("the folded minimap label could not be cropped");
            };
            let cache_key = folded_label_hash(&label);
            if let Ok(guard) = MAP_CACHE.lock() {
                if let Some((cached_key, cached)) = guard.as_ref() {
                    if *cached_key == cache_key {
                        return cached.clone();
                    }
                }
            }
            if let Some((street, name)) = match_folded_ocr(&label) {
                log_once(&format!("folded minimap OCR: {street:?} / {name:?}"));
                let read = MapRead {
                    name,
                    street: Some(street),
                    hidden: false,
                    source: ReadSource::Ocr,
                    reason: None,
                };
                if let Ok(mut guard) = MAP_CACHE.lock() {
                    *guard = Some((cache_key, read.clone()));
                }
                return read;
            }
            let Some(dir) = settings::mscw_dir() else {
                return MapRead::unknown(
                    "no MSCW client directory, so the map table cannot be read",
                );
            };
            let mut best: Option<((String, String), f64)> = None;
            for px in LABEL_SIZES {
                let Some(index) = crate::font_seed::label_index(std::path::Path::new(&dir), px)
                else {
                    continue;
                };
                let max_score = if px >= 14 { 0.95 } else { 0.55 };
                if let Some((pair, score)) =
                    crate::font_seed::match_folded_label(&label, &index.folded_lines, 14, max_score)
                {
                    if best.as_ref().map_or(true, |(_, current)| score < *current) {
                        best = Some((pair, score));
                    }
                }
            }
            match best {
                Some(((street, name), score)) => {
                    log_once(&format!(
                        "folded minimap: {street:?} / {name:?} (score {score:.3})"
                    ));
                    let read = MapRead {
                        name,
                        street: Some(street),
                        hidden: false,
                        source: ReadSource::Dtw,
                        reason: None,
                    };
                    if let Ok(mut guard) = MAP_CACHE.lock() {
                        *guard = Some((cache_key, read.clone()));
                    }
                    read
                }
                None => {
                    log_once("folded minimap: label did not match the client table");
                    MapRead::unknown("the folded map label did not match the client table")
                }
            }
        }
        _ => {
            // The label is static while the player stands still. Hashing its
            // pixels is far cheaper than the table/DTW/OCR match, so a repeated
            // frame reuses the last result. Taught fingerprints can change at
            // runtime, so those profiles keep matching every frame.
            let cacheable = maps.is_empty() && streets.is_empty();
            let cache_key = cacheable.then(|| map_label_hash(frame)).flatten();
            if let Some(key) = cache_key {
                if let Ok(guard) = MAP_CACHE.lock() {
                    if let Some((cached_key, cached)) = guard.as_ref() {
                        if *cached_key == key {
                            return cached.clone();
                        }
                    }
                }
            }

            let (mut map_name, mut source) = match_map(frame, calibration, maps);
            let mut street = match_street(frame, calibration.street, streets);
            if map_name == "unknown" || street.is_none() {
                if let Some(label) = match_label_ocr(frame) {
                    if let Some(name) = label.map {
                        map_name = name;
                        source = label.map_source;
                    }
                    street = label.street.or(street);
                }
            }
            let read = MapRead {
                reason: if map_name == "unknown" {
                    Some("no minimap label text and no matching map name".into())
                } else {
                    None
                },
                name: map_name,
                street,
                hidden: false,
                source,
            };
            if let Some(key) = cache_key {
                if read.name != "unknown" || read.street.is_some() {
                    if let Ok(mut guard) = MAP_CACHE.lock() {
                        *guard = Some((key, read.clone()));
                    }
                }
            }
            read
        }
    }
}

/// Split a map name's trailing Roman-numeral instance (`The Tree That Grew I`).
/// Only a small, known set is treated as a suffix; ordinary words are untouched.
pub(crate) fn roman_suffix(name: &str) -> Option<(&str, &str)> {
    let (base, suffix) = name.rsplit_once(' ')?;
    matches!(
        suffix,
        "I" | "II" | "III" | "IV" | "V" | "VI" | "VII" | "VIII" | "IX" | "X"
    )
    .then_some((base, suffix))
}

/// Count the vertical bars in the last word of a label line. `I`, `II` and `III`
/// produce one, two and three runs; this is independent of the line's width.
pub(crate) fn trailing_bar_count(gray: &ocr::Gray) -> Option<usize> {
    if gray.w == 0 || gray.h == 0 {
        return None;
    }
    let ink: Vec<bool> = (0..gray.w)
        .map(|x| (0..gray.h).any(|y| gray.px[y * gray.w + x] > 180))
        .collect();
    let mut end = gray.w;
    while end > 0 && !ink[end - 1] {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let mut start = None;
    let mut gap = 0usize;
    for x in (0..end).rev() {
        if ink[x] {
            if gap >= 2 {
                start = Some(x + 1);
                break;
            }
            gap = 0;
        } else {
            gap += 1;
        }
    }
    let start = start?;
    let mut bars = 0usize;
    let mut inside = false;
    for &column in &ink[start..end] {
        if column && !inside {
            bars += 1;
            inside = true;
        } else if !column {
            inside = false;
        }
    }
    (bars > 0).then_some(bars)
}

/// The whole-line DTW can prefer a longer Roman-numeral sibling because the
/// labels are otherwise nearly identical. Re-rank only within that sibling set
/// by the trailing numeral's bar count, then by rendered width. This is the
/// fallback path; exact OCR still wins before it runs.
pub(crate) fn resolve_roman_sibling(
    name: &str,
    live: &ocr::Gray,
    index: &crate::font_seed::LabelIndex,
) -> String {
    let Some((base, _)) = roman_suffix(name) else {
        return name.to_string();
    };
    let siblings: Vec<(&str, &ocr::Gray)> = index
        .map_names
        .iter()
        .filter_map(|(candidate, rendered)| {
            let (candidate_base, _) = roman_suffix(candidate)?;
            (candidate_base == base).then_some((candidate.as_str(), rendered))
        })
        .collect();
    if siblings.len() < 2 {
        return name.to_string();
    }
    if let Some(bars) = trailing_bar_count(live) {
        let bar_matches: Vec<&str> = siblings
            .iter()
            .filter_map(|(candidate, _)| {
                let (_, suffix) = roman_suffix(candidate)?;
                (suffix.chars().all(|ch| ch == 'I') && suffix.len() == bars).then_some(*candidate)
            })
            .collect();
        if let [only] = bar_matches.as_slice() {
            return (*only).to_string();
        }
    }
    let mut scored: Vec<(f64, &str)> = siblings
        .iter()
        .map(|(candidate, rendered)| ((rendered.w as f64 - live.w as f64).abs(), *candidate))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some((best_delta, best_name)) = scored.first().copied() else {
        return name.to_string();
    };
    let runner = scored.get(1).map(|(delta, _)| *delta).unwrap_or(f64::MAX);
    if runner - best_delta >= 1.5 {
        best_name.to_string()
    } else {
        name.to_string()
    }
}

pub(crate) fn match_map_name(frame: &Frame) -> Option<String> {
    let dir = settings::mscw_dir()?;
    let line = ocr::map_label_lines(frame)?.last()?.clone();
    let mut best: Option<(String, f64, u32)> = None;
    for px in LABEL_SIZES {
        let Some(index) = crate::font_seed::label_index(std::path::Path::new(&dir), px) else {
            continue;
        };
        let max_score = if px >= 14 { 0.95 } else { 0.55 };
        if let Some((name, score)) =
            crate::font_seed::match_label(&line, &index.map_names, 14, max_score)
        {
            if best
                .as_ref()
                .map_or(true, |(_, current, _)| score < *current)
            {
                best = Some((name, score, px));
            }
        }
    }
    let (name, score, px) = best?;
    let index = crate::font_seed::label_index(std::path::Path::new(&dir), px)?;
    let name = resolve_roman_sibling(&name, &line, &index);
    log_once(&format!(
        "map name from the client's map table: {name:?} (score {score:.2})"
    ));
    Some(name)
}

pub(crate) fn normalize_label(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == ' ' {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn clean_label_line(text: &str) -> String {
    text.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != ' ')
        .trim()
        .to_string()
}

/// One OCR read of the minimap label: the street and map name the resolver
/// settled on, and where the map name came from.
pub(crate) struct LabelRead {
    pub(crate) street: Option<String>,
    pub(crate) map: Option<String>,
    pub(crate) map_source: ReadSource,
    /// True when OCR read the full table name exactly, which no later stage may
    /// override.
    pub(crate) exact: bool,
}

/// OCR fallback for the expanded minimap label. The text reader supplies the
/// street and the base map name; the client table narrows the candidates and
/// the trailing `<N>` is chosen from the suffix pixels, because Windows OCR
/// drops the bracketed token.
pub(crate) fn match_label_ocr(frame: &Frame) -> Option<LabelRead> {
    let dir = settings::mscw_dir()?;
    let dir = std::path::Path::new(&dir);
    let entries = crate::map_names::load(dir).ok()?;
    let regions = ocr::map_label_line_regions(frame)?;
    if regions.is_empty() {
        return None;
    }

    let x0 = regions.iter().map(|(region, _)| region.x).min()? - 4;
    let y0 = regions.iter().map(|(region, _)| region.y).min()? - 4;
    let x1 = regions
        .iter()
        .map(|(region, _)| region.x + region.w)
        .max()?
        + 4;
    let y1 = regions
        .iter()
        .map(|(region, _)| region.y + region.h)
        .max()?
        + 4;
    let union = Region {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    };
    let keyed = ocr::crop_white_key(frame, union)?;
    let lines: Vec<String> = crate::winocr::recognize(&keyed)
        .ok()?
        .into_iter()
        .map(|line| clean_label_line(&line))
        .filter(|line| line.chars().any(|ch| ch.is_alphabetic()))
        .collect();
    if lines.is_empty() {
        return None;
    }
    let map_text = lines.last()?.as_str();
    let street_text = if lines.len() >= 2 {
        lines.first().map(String::as_str)
    } else {
        None
    };
    let live = regions.last().map(|(_, gray)| gray);
    Some(resolve_label_text(
        dir,
        &entries,
        street_text,
        map_text,
        live,
    ))
}

/// Resolve one OCR label against the client table. Known names snap to the table
/// (exact, suffix, then fuzzy); a name the table does not contain is returned as
/// the literal on-screen text. That last case is why a map added after the
/// client's string table can still be named — and it is reported as the `literal`
/// source, apart from a table match.
pub(crate) fn resolve_label_text(
    dir: &std::path::Path,
    entries: &[crate::map_names::MapEntry],
    street_text: Option<&str>,
    map_text: &str,
    live: Option<&ocr::Gray>,
) -> LabelRead {
    let mut street: Option<(String, f64)> = None;
    if let Some(street_text) = street_text {
        for entry in entries {
            if entry.street.is_empty() {
                continue;
            }
            let score = ocr::label_similarity(street_text, &entry.street);
            if street.as_ref().map_or(true, |(_, best)| score > *best) {
                street = Some((entry.street.clone(), score));
            }
        }
    }
    let street = street.and_then(|(name, score)| (score >= 0.75).then_some(name));

    let ocr_map = normalize_label(map_text);
    let literal = clean_label_line(map_text);
    if ocr_map.is_empty() || literal.is_empty() {
        return LabelRead {
            street,
            map: None,
            map_source: ReadSource::Unknown,
            exact: false,
        };
    }

    let mut candidates: Vec<&crate::map_names::MapEntry> = entries
        .iter()
        .filter(|entry| {
            let name = normalize_label(&entry.name);
            name == ocr_map
                || name.starts_with(&ocr_map)
                || ocr::label_similarity(&entry.name, map_text) >= 0.75
        })
        .collect();
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    candidates.dedup_by(|a, b| a.name == b.name);

    if candidates.is_empty() {
        log_once(&format!("map label OCR literal: {literal:?}"));
        return LabelRead {
            street,
            map: Some(literal),
            map_source: ReadSource::Literal,
            exact: false,
        };
    }

    // OCR can read the full name exactly (the roman-numeral maps are the obvious
    // case); that beats any fuzzy/DTW alternative.
    let exact: Vec<&crate::map_names::MapEntry> = candidates
        .iter()
        .copied()
        .filter(|entry| normalize_label(&entry.name) == ocr_map)
        .collect();
    if exact.len() == 1 {
        return LabelRead {
            street,
            map: Some(exact[0].name.clone()),
            map_source: ReadSource::Ocr,
            exact: true,
        };
    }
    if candidates.len() == 1 {
        return LabelRead {
            street,
            map: Some(candidates[0].name.clone()),
            map_source: ReadSource::Ocr,
            exact: false,
        };
    }

    // `<N>` instances are told apart by their suffix.
    let suffix_candidates: Vec<&crate::map_names::MapEntry> = candidates
        .iter()
        .copied()
        .filter(|entry| {
            entry.name.contains('<') && normalize_label(&entry.name).starts_with(&ocr_map)
        })
        .collect();
    if !suffix_candidates.is_empty() {
        if let Some(live) = live {
            let mut scored: Vec<(f64, String)> = Vec::new();
            for px in LABEL_SIZES {
                let Some(index) = crate::font_seed::label_index(dir, px) else {
                    continue;
                };
                for entry in &suffix_candidates {
                    let Some((_, rendered)) =
                        index.map_names.iter().find(|(name, _)| name == &entry.name)
                    else {
                        continue;
                    };
                    if let Some(score) = crate::font_seed::suffix_distance(live, rendered) {
                        scored.push((score, entry.name.clone()));
                    }
                }
            }
            scored.sort_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((best_score, best_name)) = scored.first() {
                let runner = scored
                    .iter()
                    .find(|(_, name)| name != best_name)
                    .map(|(score, _)| *score)
                    .unwrap_or(f64::MAX);
                if *best_score <= 2.5 && runner - *best_score >= 0.03 {
                    log_once(&format!(
                        "map label OCR: {best_name:?} (suffix score {best_score:.2})"
                    ));
                    return LabelRead {
                        street,
                        map: Some(best_name.clone()),
                        map_source: ReadSource::Ocr,
                        exact: false,
                    };
                }
            }
        }
    }

    // OCR often reads one character wrong (`Deep Vallev I`). Accept a clear
    // fuzzy winner; roman-numeral siblings differ by whole characters, so the
    // margin keeps them safe.
    let mut by_similarity: Vec<(f64, &crate::map_names::MapEntry)> = candidates
        .iter()
        .map(|entry| (ocr::label_similarity(&entry.name, map_text), *entry))
        .collect();
    by_similarity.sort_by(|a, b| b.0.total_cmp(&a.0));
    if let Some((best, entry)) = by_similarity.first() {
        let runner = by_similarity.get(1).map(|(score, _)| *score).unwrap_or(0.0);
        if *best >= 0.80 && best - runner >= 0.05 {
            log_once(&format!(
                "map label OCR fuzzy: {:?} (score {best:.2})",
                entry.name
            ));
            return LabelRead {
                street,
                map: Some(entry.name.clone()),
                map_source: ReadSource::Ocr,
                exact: false,
            };
        }
    }

    log_once(&format!("map label OCR literal: {literal:?}"));
    LabelRead {
        street,
        map: Some(literal),
        map_source: ReadSource::Literal,
        exact: false,
    }
}

/// OCR fallback for the folded `Street :Map` bar. The line is matched to the
/// client table; a shared base name is resolved by the trailing `<N>` suffix,
/// which OCR may or may not have read.
pub(crate) fn match_folded_ocr(label: &ocr::Gray) -> Option<(String, String)> {
    let dir = settings::mscw_dir()?;
    let dir = std::path::Path::new(&dir);
    let lines = crate::winocr::recognize(label).ok()?;
    let text = lines.into_iter().next()?;
    let (street_text, map_text) = text.split_once(':')?;
    let street_text = clean_label_line(street_text);
    let map_text = clean_label_line(map_text);
    if street_text.is_empty() || map_text.is_empty() {
        return None;
    }
    let ocr_map = normalize_label(&map_text);
    if ocr_map.is_empty() {
        return None;
    }

    let mut candidates: Vec<((String, String), ocr::Gray)> = Vec::new();
    for px in LABEL_SIZES {
        let Some(index) = crate::font_seed::label_index(dir, px) else {
            continue;
        };
        for ((street, name), rendered) in &index.folded_lines {
            if ocr::label_similarity(&street_text, street) < 0.75 {
                continue;
            }
            let normalized = normalize_label(name);
            if (normalized == ocr_map
                || normalized.starts_with(&ocr_map)
                || ocr::label_similarity(name, &map_text) >= 0.75)
                && !candidates
                    .iter()
                    .any(|((s, n), _)| s == street && n == name)
            {
                candidates.push(((street.clone(), name.clone()), rendered.clone()));
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let exact: Vec<&((String, String), ocr::Gray)> = candidates
        .iter()
        .filter(|((_, name), _)| normalize_label(name) == ocr_map)
        .collect();
    if exact.len() == 1 {
        return Some(exact[0].0.clone());
    }
    if candidates.len() == 1 {
        return Some(candidates[0].0.clone());
    }

    let suffix_names: Vec<(String, String)> = candidates
        .iter()
        .filter(|((_, name), _)| name.contains('<') && normalize_label(name).starts_with(&ocr_map))
        .map(|((street, name), _)| (street.clone(), name.clone()))
        .collect();
    if !suffix_names.is_empty() {
        let mut variants: Vec<crate::font_seed::FoldedLine> = Vec::new();
        for px in LABEL_SIZES {
            let Some(index) = crate::font_seed::label_index(dir, px) else {
                continue;
            };
            for ((street, name), rendered) in &index.folded_lines {
                if suffix_names.iter().any(|(s, n)| s == street && n == name) {
                    variants.push(((street.clone(), name.clone()), rendered.clone()));
                }
            }
        }
        let mut best_by_name: std::collections::BTreeMap<(String, String), f64> =
            std::collections::BTreeMap::new();
        for (pair, rendered) in &variants {
            if let Some(score) = crate::font_seed::suffix_distance(label, rendered) {
                let entry = best_by_name.entry(pair.clone()).or_insert(f64::MAX);
                if score < *entry {
                    *entry = score;
                }
            }
        }
        let mut scored: Vec<(f64, (String, String))> = best_by_name
            .into_iter()
            .map(|(pair, score)| (score, pair))
            .collect();
        scored.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((best, pair)) = scored.first() {
            let runner = scored
                .iter()
                .find(|(_, other)| other != pair)
                .map(|(score, _)| *score)
                .unwrap_or(f64::MAX);
            if *best <= 2.5 && runner - *best >= 0.03 {
                return Some(pair.clone());
            }
        }
    }

    let mut by_similarity: Vec<(f64, (String, String))> = candidates
        .iter()
        .map(|((street, name), _)| {
            (
                ocr::label_similarity(&map_text, name),
                (street.clone(), name.clone()),
            )
        })
        .collect();
    by_similarity.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, pair) = by_similarity.first()?;
    let runner = by_similarity.get(1).map(|(score, _)| *score).unwrap_or(0.0);
    if *best >= 0.85 && best - runner >= 0.05 {
        return Some(pair.clone());
    }
    None
}

pub(crate) fn match_map_fingerprint(
    frame: &Frame,
    calibration: &Calibration,
    templates: &[Template],
) -> Option<String> {
    // Detection finds the whole label and follows a longer map name; `map_label`
    // only falls back to the stored box when detection fails. A stored box drawn
    // around a shorter name must not clip the next map's name, or two maps that
    // share a prefix would look identical.
    let crop = ocr::map_label(frame, calibration.map_box())?;

    let mut best: Option<(&str, f64)> = None;
    for template in templates {
        let Some(candidate) = template_gray(template) else {
            continue;
        };
        if let Some(distance) = ocr::map_distance(&crop, &candidate) {
            if best.map_or(true, |(_, closest)| distance < closest) {
                best = Some((&template.label, distance));
            }
        }
    }

    match best {
        Some((name, distance)) if distance <= MAP_DISTANCE_LIMIT => Some(name.to_string()),
        _ => None,
    }
}
