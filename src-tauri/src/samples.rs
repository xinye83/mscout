//! Sample history: the `data/samples/YYYY-MM-DD.jsonl` day files.
//!
//! One owner and one lock. The engine appends a row every second while the
//! history window reads and the repair rewrites; without a shared lock a repair
//! could read a day, miss the row appended meanwhile, and write the day back
//! without it. Every function here takes [`LOCK`] for its whole file access.
//!
//! Day files are named by **local** date (the day the player sees), while rows
//! carry UTC `t`. A range read therefore opens the files whose date is within a
//! day of the range, and filters rows by `t`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::NaiveDate;
use serde_json::Value;

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn samples_dir() -> Result<PathBuf, String> {
    Ok(crate::data_dir()?.join("samples"))
}

fn local_day(t: i64) -> Option<NaiveDate> {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|time| time.with_timezone(&chrono::Local).date_naive())
}

/// Every day file in the folder, oldest first, with its date.
fn day_files(dir: &Path) -> Result<Vec<(NaiveDate, PathBuf)>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut days: Vec<(NaiveDate, PathBuf)> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            let stem = name
                .strip_suffix(".jsonl")
                .or_else(|| name.strip_suffix(".jsonl.gz"))?;
            let day = NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()?;
            Some((day, path))
        })
        .collect();
    days.sort();
    // A day both compressed and not (an interrupted compression) is read once,
    // from the plain file, which is the one appends went to.
    days.dedup_by(|later, earlier| {
        later.0 == earlier.0 && {
            if is_compressed(&earlier.1) {
                std::mem::swap(later, earlier);
            }
            true
        }
    });
    Ok(days)
}

/// The rows of one day file that parse as JSON objects, in file order, and how
/// many lines did not. A line that is not UTF-8 (the NUL padding an interrupted
/// write leaves) or not JSON is skipped and counted, never fatal: one bad line
/// used to fail the whole read and blank the history window.
fn read_day(path: &Path) -> std::io::Result<(Vec<Value>, usize)> {
    let bytes = read_bytes(path)?;
    let mut rows = Vec::new();
    let mut bad = 0usize;
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(text) = std::str::from_utf8(line) else {
            bad += 1;
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(text) {
            Ok(value) if value.is_object() => rows.push(value),
            _ => bad += 1,
        }
    }
    Ok((rows, bad))
}

fn is_compressed(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("gz")
}

/// A day file's bytes, decompressed when it is a `.jsonl.gz`.
fn read_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    if !is_compressed(path) {
        return Ok(bytes);
    }
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut out)?;
    Ok(out)
}

/// Write a day file back in the form it was in.
fn write_day(path: &Path, text: &[u8]) -> std::io::Result<()> {
    if !is_compressed(path) {
        return crate::fsutil::write_atomic(path, text);
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(text)?;
    crate::fsutil::write_atomic(path, &encoder.finish()?)
}

/// Day files are compressed once they are this many days old. Nothing is
/// dropped: the rows stay, about 28 times smaller (measured on real history),
/// and every reader opens either form.
pub const COMPRESS_AFTER_DAYS: i64 = 30;

/// Compress the day files older than [`COMPRESS_AFTER_DAYS`]. The compressed
/// copy is written and checked before the plain file is removed, so an
/// interruption leaves the plain file (which readers then prefer).
pub fn compress_old() -> Result<usize, String> {
    let today = chrono::Local::now().date_naive();
    let _guard = lock();
    let mut done = 0usize;
    for (day, path) in day_files(&samples_dir()?)? {
        if is_compressed(&path) || (today - day).num_days() < COMPRESS_AFTER_DAYS {
            continue;
        }
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let gz = path.with_extension("jsonl.gz");
        write_day(&gz, &bytes).map_err(|e| e.to_string())?;
        if read_bytes(&gz).map_err(|e| e.to_string())? != bytes {
            let _ = fs::remove_file(&gz);
            return Err(format!("compressing {} did not round-trip", path.display()));
        }
        fs::remove_file(&path).map_err(|e| e.to_string())?;
        done += 1;
    }
    Ok(done)
}

/// Every good row of every day file, oldest first: what the history index is
/// rebuilt from.
pub fn all_rows() -> Result<Vec<Value>, String> {
    let _guard = lock();
    let mut rows = Vec::new();
    for (_, path) in day_files(&samples_dir()?)? {
        let (day_rows, _) = read_day(&path).map_err(|e| e.to_string())?;
        rows.extend(day_rows.into_iter().filter(|row| !is_bad(row)));
    }
    rows.sort_by_key(|row| time_of(row).unwrap_or(0));
    Ok(rows)
}

/// The good rows of the last `seconds` before the latest row.
pub fn recent_rows(seconds: i64) -> Result<Vec<Value>, String> {
    let _guard = lock();
    let days = day_files(&samples_dir()?)?;
    let mut rows = Vec::new();
    for (_, path) in days.iter().rev().take(2) {
        let (day_rows, _) = read_day(path).map_err(|e| e.to_string())?;
        rows.extend(day_rows.into_iter().filter(|row| !is_bad(row)));
    }
    rows.sort_by_key(|row| time_of(row).unwrap_or(0));
    let latest = rows.last().and_then(time_of).unwrap_or(0);
    rows.retain(|row| time_of(row).is_some_and(|t| t >= latest - seconds));
    Ok(rows)
}

/// Rows the reader knows were wrong (`"bad": true`, set by the repair) are
/// skipped by every reader, so the chart and every rate see clean data.
fn is_bad(row: &Value) -> bool {
    row.get("bad").and_then(Value::as_bool).unwrap_or(false)
}

fn time_of(row: &Value) -> Option<i64> {
    row.get("t").and_then(Value::as_i64)
}

/// Append one row to its day file.
pub fn append(record: &Value) -> Result<(), String> {
    let t = time_of(record).ok_or("sample is missing t")?;
    let day = local_day(t).ok_or("invalid timestamp")?;

    let _guard = lock();
    let dir = samples_dir()?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = dir.join(format!("{}.jsonl", day.format("%Y-%m-%d")));
    let mut line = serde_json::to_string(record).map_err(|e| e.to_string())?;
    line.push('\n');
    append_line(&path, &line).map_err(|e| e.to_string())?;
    // The index takes its own lock; the history lock is released first, since a
    // rebuild holds the index lock while it reads the history.
    drop(_guard);
    crate::history_index::observe(record);
    Ok(())
}

/// Append one complete line. The line goes out in a single `write_all`, so a
/// crash leaves at most a missing row rather than half of one. If the file does
/// not end in a newline - the tail of an earlier interrupted write - a newline is
/// written first, so the new row is not glued onto the broken one and both lost.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let needs_break = ends_without_newline(path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if needs_break {
        file.write_all(b"\n")?;
    }
    file.write_all(line.as_bytes())
}

fn ends_without_newline(path: &Path) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

/// Recorded rows with `from <= t <= to`, oldest first, bad rows skipped.
///
/// With `bucket` (seconds), consecutive rows in the same bucket collapse to the
/// bucket's last row, so a 30-day range does not send millions of 1 Hz rows to
/// the window. A row with an event, or the first row of a different character,
/// is always kept: a death or a switch must stay visible in the history.
pub fn read(from: i64, to: i64, bucket: Option<i64>) -> Result<Vec<Value>, String> {
    let (Some(first_day), Some(last_day)) = (local_day(from), local_day(to)) else {
        return Err("invalid range".into());
    };
    // Files are named by local date; allow a day either side so a time-zone
    // change between recording and reading cannot drop the edges.
    let first_day = first_day.pred_opt().unwrap_or(first_day);
    let last_day = last_day.succ_opt().unwrap_or(last_day);

    let _guard = lock();
    let mut rows = Vec::new();
    let mut skipped = 0usize;
    for (day, path) in day_files(&samples_dir()?)? {
        if day < first_day || day > last_day {
            continue;
        }
        let (day_rows, bad) = read_day(&path).map_err(|e| e.to_string())?;
        skipped += bad;
        rows.extend(
            day_rows
                .into_iter()
                .filter(|row| !is_bad(row) && time_of(row).is_some_and(|t| t >= from && t <= to)),
        );
    }
    drop(_guard);

    if skipped > 0 {
        crate::calibration::log(&format!("history read skipped {skipped} unreadable lines"));
    }
    rows.sort_by_key(|row| time_of(row).unwrap_or(0));
    Ok(match bucket.filter(|size| *size > 1) {
        Some(size) => thin(rows, size),
        None => rows,
    })
}

/// Keep the last row of each `size`-second bucket, plus every row with an event
/// and every row that starts a different character.
fn thin(rows: Vec<Value>, size: i64) -> Vec<Value> {
    let name = |row: &Value| row.get("name").and_then(Value::as_str).map(str::to_string);
    let has_event = |row: &Value| row.get("event").is_some();
    let mut out: Vec<Value> = Vec::new();
    for row in rows {
        if let Some(last) = out.last_mut() {
            let same_bucket = time_of(last).map(|t| t.div_euclid(size))
                == time_of(&row).map(|t| t.div_euclid(size));
            let same_character =
                name(last).is_none() || name(&row).is_none() || name(last) == name(&row);
            if same_bucket && same_character && !has_event(last) && !has_event(&row) {
                *last = row;
                continue;
            }
        }
        out.push(row);
    }
    out
}

/// The newest recorded row. The engine compares each reading against the last one
/// it recorded, so without seeding from this the first reading after a restart is
/// unchecked — which is how a two-EXP blip got into the history.
pub fn last() -> Option<Value> {
    let _guard = lock();
    let days = day_files(&samples_dir().ok()?).ok()?;
    let (_, path) = days.last()?;
    let (rows, _) = read_day(path).ok()?;
    rows.into_iter().rfind(|row| !is_bad(row))
}

/// Indices of rows that fall below the running high-water mark and later climb back
/// over it. A genuine loss of EXP — dying costs EXP — never recovers, so it is left
/// alone; a misread always does, because the next good read overtakes it.
///
/// The third field marks a recorded death (`"event":"death"`). It starts a new
/// high-water mark: without that, the next level-up would "climb back" over the
/// pre-death mark and flag every row in between.
pub(crate) fn find_misreads(rows: &[(u32, i64, bool)]) -> Vec<usize> {
    let mut flagged = Vec::new();
    let mut parked: Vec<usize> = Vec::new();
    let mut baseline: Option<(u32, i64)> = None;

    for (index, (level, exp, death)) in rows.iter().enumerate() {
        if *death {
            parked.clear();
            baseline = Some((*level, *exp));
            continue;
        }
        match baseline {
            Some((base_level, base_exp))
                if *level < base_level || (*level == base_level && *exp < base_exp) =>
            {
                parked.push(index);
            }
            _ => {
                if let Some((base_level, base_exp)) = baseline {
                    if !parked.is_empty() {
                        if *level > base_level || (*level == base_level && *exp >= base_exp) {
                            flagged.append(&mut parked);
                        } else {
                            parked.clear();
                        }
                    }
                }
                baseline = Some((*level, *exp));
            }
        }
    }

    flagged
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairSummary {
    pub checked: usize,
    pub flagged: usize,
    pub files: usize,
}

/// Flag the recorded rows that break the invariants the reader now enforces, so
/// the chart and the rate maths stop being dragged down by misreads.
///
/// Rows are flagged rather than deleted: the file stays as the evidence of how
/// often the reader failed, and a mistaken flag can be removed by hand. A row is
/// only flagged when it sits below the running high-water mark *and* the recording
/// later climbs back over it — a genuine loss of EXP never recovers, so it stays.
///
/// The whole repair holds the history lock, so no row is appended between the
/// read and the rewrite, and each changed day is replaced atomically: an
/// interrupted repair leaves the old file, never a truncated one. Lines that do
/// not parse are written back unchanged.
pub fn repair() -> Result<RepairSummary, String> {
    let _guard = lock();
    let files: Vec<PathBuf> = day_files(&samples_dir()?)?
        .into_iter()
        .map(|(_, path)| path)
        .collect();

    let mut lines_per_file: Vec<Vec<String>> = Vec::new();
    let mut rows: Vec<(usize, usize, Value)> = Vec::new();
    for (file_index, path) in files.iter().enumerate() {
        let bytes = read_bytes(path).map_err(|e| e.to_string())?;
        let mut lines: Vec<String> = Vec::new();
        for line in String::from_utf8_lossy(&bytes).lines() {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                if time_of(&value).is_some() {
                    rows.push((file_index, lines.len(), value));
                }
            }
            lines.push(line.to_string());
        }
        lines_per_file.push(lines);
    }

    // Every row of every day, in time order, so a dip across midnight is caught too.
    rows.sort_by_key(|(_, _, value)| time_of(value).unwrap_or(0));
    let checked = rows.len();

    let series: Vec<(u32, i64, bool)> = rows
        .iter()
        .map(|(_, _, row)| {
            (
                row.get("lv").and_then(Value::as_u64).unwrap_or(0) as u32,
                row.get("exp").and_then(Value::as_i64).unwrap_or(0),
                row.get("event").and_then(Value::as_str) == Some("death"),
            )
        })
        .collect();

    let flagged: Vec<(usize, usize)> = find_misreads(&series)
        .into_iter()
        .filter_map(|index| {
            rows.get(index)
                .map(|(file_index, line_index, _)| (*file_index, *line_index))
        })
        .collect();

    let mut changed_files = 0usize;
    for (file_index, lines) in lines_per_file.iter_mut().enumerate() {
        let mut changed = false;
        for (flagged_file, line_index) in &flagged {
            if *flagged_file != file_index {
                continue;
            }
            let Ok(mut value) = serde_json::from_str::<Value>(&lines[*line_index]) else {
                continue;
            };
            let Some(object) = value.as_object_mut() else {
                continue;
            };
            if object.get("bad").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            object.insert("bad".into(), Value::Bool(true));
            lines[*line_index] = serde_json::to_string(&value).map_err(|e| e.to_string())?;
            changed = true;
        }

        if changed {
            let text = lines.join("\n");
            write_day(&files[file_index], format!("{text}\n").as_bytes())
                .map_err(|e| e.to_string())?;
            changed_files += 1;
        }
    }

    // Report what the files now carry, so pressing the button twice is stable.
    let total = lines_per_file
        .iter()
        .flat_map(|lines| lines.iter())
        .filter(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .is_some_and(|value| is_bad(&value))
        })
        .count();

    drop(_guard);
    if changed_files > 0 {
        // Flagged rows leave the index's sums; recompute them.
        if let Err(error) = crate::history_index::rebuild() {
            crate::calibration::log(&format!(
                "history index rebuild after repair failed: {error}"
            ));
        }
    }

    Ok(RepairSummary {
        checked,
        flagged: total,
        files: changed_files,
    })
}

/// A fresh samples folder under its own data dir. Tests that touch the global
/// history (here and in `history_index`) run one at a time through this, since
/// they share a dir.
#[cfg(test)]
pub(crate) fn with_history(test: impl FnOnce(&Path)) {
    static SERIAL: Mutex<()> = Mutex::new(());
    let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
    let dir = samples_dir().expect("samples dir");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("samples dir");
    test(&dir);
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use super::with_history;

    #[test]
    fn an_old_day_is_compressed_and_still_read() {
        with_history(|dir| {
            let old = chrono::Local::now() - chrono::Duration::days(COMPRESS_AFTER_DAYS + 2);
            let t = old.timestamp();
            for offset in 0..3 {
                append(&json!({"t": t + offset, "lv": 5, "exp": offset, "map": "M"}))
                    .expect("append");
            }
            assert_eq!(compress_old().expect("compress"), 1);
            let names: Vec<String> = fs::read_dir(dir)
                .expect("dir")
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect();
            assert!(
                names.iter().all(|name| name.ends_with(".jsonl.gz")),
                "{names:?}"
            );
            assert_eq!(read(t, t + 2, None).expect("read").len(), 3);
            assert_eq!(all_rows().expect("all").len(), 3);
        });
    }

    fn row(t: i64, lv: u32, exp: i64) -> Value {
        json!({ "t": t, "lv": lv, "exp": exp, "pct": 0.0, "map": "", "state": "gaining", "conf": 1.0 })
    }

    #[test]
    fn appends_whole_lines_and_repairs_a_torn_tail() {
        let dir = std::env::temp_dir().join(format!("mscout-append-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("2026-11-19.jsonl");

        append_line(&path, "{\"t\":1}\n").expect("first row");
        // A crash mid-write leaves a partial row (or NUL padding) with no newline.
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(b"{\"t\":2,\"ex\0\0"))
            .expect("simulate a torn tail");
        append_line(&path, "{\"t\":3}\n").expect("row after the tear");

        let text = fs::read_to_string(&path).expect("read back");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text:?}");
        assert_eq!(lines[0], "{\"t\":1}");
        assert_eq!(lines[2], "{\"t\":3}", "the new row stands on its own line");
        assert!(serde_json::from_str::<Value>(lines[2]).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_that_is_not_text_is_skipped_not_fatal() {
        with_history(|dir| {
            let t = 1_795_000_000;
            let day = local_day(t).expect("day").format("%Y-%m-%d").to_string();
            let mut bytes = serde_json::to_vec(&row(t, 30, 10)).expect("row");
            bytes.extend_from_slice(b"\n\xff\xfe\0\0\n");
            bytes.extend_from_slice(&serde_json::to_vec(&row(t + 1, 30, 11)).expect("row"));
            bytes.push(b'\n');
            fs::write(dir.join(format!("{day}.jsonl")), bytes).expect("write day");

            let rows = read(t - 10, t + 10, None).expect("read");
            assert_eq!(rows.len(), 2);
        });
    }

    #[test]
    fn a_range_read_opens_only_the_days_around_it() {
        with_history(|dir| {
            let t = 1_795_000_000;
            append(&row(t, 30, 10)).expect("append");
            // A file far outside the range that would fail if it were opened.
            fs::write(
                dir.join("2001-01-01.jsonl"),
                b"{\"t\":1795000001,\"lv\":1}\n",
            )
            .expect("old day");
            let rows = read(t - 10, t + 10, None).expect("read");
            assert_eq!(
                rows.len(),
                1,
                "a row in a far-away file is not read: {rows:?}"
            );
        });
    }

    #[test]
    fn thinning_keeps_one_row_per_bucket_and_every_event() {
        let mut death = row(130, 30, 5);
        death["event"] = json!("death");
        let mut alt = row(170, 1, 0);
        alt["name"] = json!("Alt");
        let mut main = row(160, 30, 6);
        main["name"] = json!("Main");
        let rows = vec![
            row(0, 30, 1),
            row(30, 30, 2),
            row(59, 30, 3),
            row(61, 30, 4),
            death,
            main,
            alt,
        ];
        let kept: Vec<i64> = thin(rows, 60).iter().filter_map(time_of).collect();
        assert_eq!(kept, vec![59, 61, 130, 160, 170]);
    }

    #[test]
    fn repair_flags_rows_in_place_and_keeps_the_rest() {
        with_history(|dir| {
            let t = 1_795_000_000;
            for (offset, exp) in [(0, 21_478), (1, 21_476), (2, 21_500)] {
                append(&row(t + offset, 35, exp)).expect("append");
            }
            let summary = repair().expect("repair");
            assert_eq!((summary.checked, summary.flagged, summary.files), (3, 1, 1));
            let rows = read(t - 10, t + 10, None).expect("read");
            assert_eq!(
                rows.iter().filter_map(time_of).collect::<Vec<_>>(),
                vec![t, t + 2]
            );
            // No temp file is left behind by the atomic rewrite.
            let leftovers = fs::read_dir(dir)
                .expect("dir")
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl"))
                .count();
            assert_eq!(leftovers, 0);
        });
    }

    #[test]
    fn flags_the_dips_the_reader_used_to_record() {
        // The shape of the recorded history: a two-EXP blip, then a truncated read
        // whose derived level falls away and later comes back.
        let rows = no_events(&[
            (35, 21_478),
            (35, 21_476),
            (35, 21_476),
            (35, 21_500),
            (19, 2_106),
            (19, 2_106),
            (35, 21_704),
            (35, 21_710),
        ]);
        assert_eq!(find_misreads(&rows), vec![1, 2, 4, 5]);
    }

    /// History rows with no recorded events.
    fn no_events(rows: &[(u32, i64)]) -> Vec<(u32, i64, bool)> {
        rows.iter().map(|&(lv, exp)| (lv, exp, false)).collect()
    }

    #[test]
    fn a_recorded_death_is_not_flagged_when_the_level_is_regained() {
        let rows = vec![
            (35, 100_000, false),
            (35, 82_579, true), // the death row
            (35, 90_000, false),
            (35, 170_000, false),
            (36, 1_000, false), // level-up: climbs past the pre-death mark
        ];
        assert_eq!(find_misreads(&rows), Vec::<usize>::new());
    }

    #[test]
    fn keeps_a_loss_that_never_recovers() {
        // Dying costs EXP and the recording stays lower, so nothing is flagged.
        let rows = no_events(&[(35, 50_000), (35, 49_000), (35, 49_500), (35, 49_800)]);
        assert_eq!(find_misreads(&rows), Vec::<usize>::new());
    }

    #[test]
    fn keeps_an_ordinary_climb() {
        let rows = no_events(&[(35, 1_000), (35, 2_000), (36, 10), (36, 900)]);
        assert_eq!(find_misreads(&rows), Vec::<usize>::new());
    }

    #[test]
    #[ignore = "tool: flags misread rows; set MSCOUT_DATA_DIR to the data folder to repair"]
    fn repair_history_now() {
        match repair() {
            Ok(summary) => println!(
                "checked {} rows, {} now flagged, {} files rewritten",
                summary.checked, summary.flagged, summary.files
            ),
            Err(error) => println!("repair failed: {error}"),
        }
    }
}
