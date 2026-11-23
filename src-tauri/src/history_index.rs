//! The history index: one-minute summaries of the sample history in
//! `data/index.sqlite3`, for the views that span more than a day (History's
//! 7-day, 30-day and all-time ranges, sessions, per-map rates, export).
//!
//! The day files (`samples.rs`) stay the source of truth. The index is derived:
//! the recorder adds each row as it appends it, and [`rebuild`] recomputes
//! everything from the day files after a repair, a schema change, or whenever
//! the index disagrees with them. Deleting the file loses nothing.
//!
//! A minute row sums the slices that end in it, with the same rules as the
//! overlay's `stats.ts`: a gap of more than 5 s between rows is not recording;
//! the time between two gains at most [`IDLE_PAUSE`] apart is active; EXP lost
//! at a recorded death is `lost`, not a negative gain; a character switch (a
//! different name, or a level drop) starts a new run.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

/// Rows further apart than this are not one recording.
pub const RECORDING_GAP: i64 = 5;
/// Gains at most this far apart are one stretch of play.
pub const IDLE_PAUSE: i64 = 30;
/// Bumped when the table or the rules change; a different stored version
/// triggers a rebuild.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS minutes (
    name TEXT NOT NULL,
    t INTEGER NOT NULL,
    wall INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 0,
    gained INTEGER NOT NULL DEFAULT 0,
    active_gained INTEGER NOT NULL DEFAULT 0,
    lost INTEGER NOT NULL DEFAULT 0,
    deaths INTEGER NOT NULL DEFAULT 0,
    level_ups INTEGER NOT NULL DEFAULT 0,
    last_gain INTEGER,
    last_t INTEGER NOT NULL,
    lv INTEGER NOT NULL,
    exp INTEGER NOT NULL,
    map TEXT NOT NULL,
    PRIMARY KEY (name, t)
);
CREATE INDEX IF NOT EXISTS minutes_by_time ON minutes (t);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// One row of the history, as much of it as the index needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub t: i64,
    pub lv: u32,
    pub exp: i64,
    pub map: String,
    pub name: Option<String>,
    pub death: bool,
}

impl Row {
    /// A history row, or `None` for a row every reader skips (flagged bad, no
    /// level, no time).
    pub fn from_value(value: &Value) -> Option<Row> {
        if value.get("bad").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        let lv = value.get("lv").and_then(Value::as_u64)? as u32;
        if lv == 0 {
            return None;
        }
        Some(Row {
            t: value.get("t").and_then(Value::as_i64)?,
            lv,
            exp: value.get("exp").and_then(Value::as_i64).unwrap_or(0),
            map: value
                .get("map")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            name: value
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string),
            death: value.get("event").and_then(Value::as_str) == Some("death"),
        })
    }

    fn total(&self) -> f64 {
        cumulative(self.lv) + self.exp as f64
    }
}

/// EXP to reach `level` from level 1.
fn cumulative(level: u32) -> f64 {
    (1..level)
        .map(|l| crate::exp_table::required(l).unwrap_or(0.0))
        .sum()
}

/// What one row adds to one minute.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delta {
    pub name: String,
    /// The minute, as Unix seconds of its start.
    pub minute: i64,
    pub wall: i64,
    pub active: i64,
    pub gained: i64,
    pub active_gained: i64,
    pub lost: i64,
    pub deaths: i64,
    pub level_ups: i64,
    pub last_gain: Option<i64>,
    /// The row that ends the slice: the minute's latest state.
    pub last_t: i64,
    pub lv: u32,
    pub exp: i64,
    pub map: String,
}

fn minute_of(t: i64) -> i64 {
    t.div_euclid(60) * 60
}

/// Turns consecutive rows into per-minute additions.
#[derive(Default)]
pub struct Aggregator {
    last: Option<Row>,
    last_gain: Option<i64>,
}

impl Aggregator {
    /// The additions row `b` makes. The first row of a run adds only its
    /// state; a gain's active wait can reach back into the previous minute.
    pub fn push(&mut self, b: Row) -> Vec<Delta> {
        let name = b.name.clone().unwrap_or_default();
        let base = Delta {
            name: name.clone(),
            minute: minute_of(b.t),
            last_t: b.t,
            lv: b.lv,
            exp: b.exp,
            map: b.map.clone(),
            ..Delta::default()
        };
        let Some(a) = self.last.replace(b.clone()) else {
            self.last_gain = None;
            return vec![base];
        };
        let switched =
            b.lv < a.lv || matches!((&a.name, &b.name), (Some(x), Some(y)) if x != y) || b.t <= a.t;
        if switched {
            self.last_gain = None;
            return vec![base];
        }
        let dt = b.t - a.t;
        let recording = dt <= RECORDING_GAP;
        let delta = (b.total() - a.total()).round() as i64;
        let gained = if !b.death && delta > 0 { delta } else { 0 };
        let mut own = Delta {
            wall: if recording { dt } else { 0 },
            gained,
            lost: if b.death { (-delta).max(0) } else { 0 },
            deaths: i64::from(b.death),
            level_ups: i64::from(b.lv.saturating_sub(a.lv)),
            last_gain: (gained > 0).then_some(b.t),
            ..base
        };
        let mut out = Vec::new();
        if gained > 0 {
            match self.last_gain {
                Some(previous) if recording && b.t - previous <= IDLE_PAUSE => {
                    own.active_gained = gained;
                    // The wait since the previous gain is active, second by
                    // second, in the minutes it falls in.
                    let mut spread: BTreeMap<i64, i64> = BTreeMap::new();
                    for second in previous..b.t {
                        *spread.entry(minute_of(second)).or_default() += 1;
                    }
                    for (minute, seconds) in spread {
                        if minute == own.minute {
                            own.active += seconds;
                        } else {
                            out.push(Delta {
                                name: name.clone(),
                                minute,
                                active: seconds,
                                last_t: b.t,
                                lv: b.lv,
                                exp: b.exp,
                                map: b.map.clone(),
                                ..Delta::default()
                            });
                        }
                    }
                }
                _ => {}
            }
            self.last_gain = Some(b.t);
        } else if !recording {
            self.last_gain = None;
        }
        out.push(own);
        out
    }
}

/// A minute as the History window reads it (the `Slice` of `stats.ts`).
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Minute {
    /// End of the minute's last slice.
    pub t: i64,
    pub minute: i64,
    pub name: String,
    pub wall: i64,
    pub active: i64,
    pub gained: i64,
    pub active_gained: i64,
    pub lost: i64,
    pub deaths: i64,
    pub level_ups: i64,
    pub gain: bool,
    pub last_gain: Option<i64>,
    pub lv: u32,
    pub exp: i64,
    pub map: String,
}

struct Index {
    connection: Connection,
    aggregator: Aggregator,
}

static INDEX: Mutex<Option<Index>> = Mutex::new(None);

fn path() -> Result<PathBuf, String> {
    Ok(crate::data_dir()?.join("index.sqlite3"))
}

fn open() -> Result<Connection, String> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let connection = Connection::open(&path).map_err(|e| e.to_string())?;
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .map_err(|e| e.to_string())?;
    connection
        .execute_batch(SCHEMA)
        .map_err(|e| e.to_string())?;
    Ok(connection)
}

fn meta(connection: &Connection, key: &str) -> Option<String> {
    connection
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .ok()
}

fn set_meta(connection: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    connection
        .execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map(|_| ())
}

fn apply(connection: &Connection, deltas: &[Delta]) -> rusqlite::Result<()> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO minutes (name, t, wall, active, gained, active_gained, lost, deaths,
                              level_ups, last_gain, last_t, lv, exp, map)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(name, t) DO UPDATE SET
            wall = wall + excluded.wall,
            active = active + excluded.active,
            gained = gained + excluded.gained,
            active_gained = active_gained + excluded.active_gained,
            lost = lost + excluded.lost,
            deaths = deaths + excluded.deaths,
            level_ups = level_ups + excluded.level_ups,
            last_gain = COALESCE(MAX(last_gain, excluded.last_gain), last_gain, excluded.last_gain),
            last_t = MAX(last_t, excluded.last_t),
            lv = CASE WHEN excluded.last_t >= last_t THEN excluded.lv ELSE lv END,
            exp = CASE WHEN excluded.last_t >= last_t THEN excluded.exp ELSE exp END,
            map = CASE WHEN excluded.last_t >= last_t THEN excluded.map ELSE map END",
    )?;
    for d in deltas {
        statement.execute(params![
            d.name,
            d.minute,
            d.wall,
            d.active,
            d.gained,
            d.active_gained,
            d.lost,
            d.deaths,
            d.level_ups,
            d.last_gain,
            d.last_t,
            d.lv,
            d.exp,
            d.map
        ])?;
    }
    Ok(())
}

/// Recompute the whole index from the day files.
pub fn rebuild() -> Result<usize, String> {
    let mut guard = INDEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = None;
    let mut connection = open()?;
    let rows = crate::samples::all_rows()?;
    let tx = connection.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM minutes", [])
        .map_err(|e| e.to_string())?;
    let mut aggregator = Aggregator::default();
    let mut last_t = 0;
    let mut count = 0usize;
    for row in rows.iter().filter_map(Row::from_value) {
        last_t = row.t;
        count += 1;
        let deltas = aggregator.push(row);
        apply(&tx, &deltas).map_err(|e| e.to_string())?;
    }
    set_meta(&tx, "schema", &SCHEMA_VERSION.to_string()).map_err(|e| e.to_string())?;
    set_meta(&tx, "last_t", &last_t.to_string()).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    *guard = Some(Index {
        connection,
        aggregator,
    });
    Ok(count)
}

/// Open the index, rebuilding it when it is missing, from another schema, or
/// behind the day files. Run once at startup, off the main thread.
pub fn ensure() -> Result<(), String> {
    let latest = crate::samples::last()
        .and_then(|value| value.get("t").and_then(Value::as_i64))
        .unwrap_or(0);
    let current = {
        let connection = open()?;
        meta(&connection, "schema") == Some(SCHEMA_VERSION.to_string())
            && meta(&connection, "last_t") == Some(latest.to_string())
    };
    if current {
        // The aggregator needs the last row (and the last gain) to continue
        // where the index stopped; the rebuild state is cheap to recover by
        // replaying today's rows.
        let mut aggregator = Aggregator::default();
        for row in crate::samples::recent_rows(3600)?
            .iter()
            .filter_map(Row::from_value)
        {
            aggregator.push(row);
        }
        let mut guard = INDEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(Index {
            connection: open()?,
            aggregator,
        });
        return Ok(());
    }
    let count = rebuild()?;
    crate::calibration::log(&format!("history index rebuilt from {count} rows"));
    Ok(())
}

/// Add one appended row. Never fails the append: a problem is logged and the
/// next startup's check rebuilds the index.
pub fn observe(value: &Value) {
    let Some(row) = Row::from_value(value) else {
        return;
    };
    let mut guard = INDEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(index) = guard.as_mut() else {
        return;
    };
    let t = row.t;
    let deltas = index.aggregator.push(row);
    let result = apply(&index.connection, &deltas)
        .and_then(|_| set_meta(&index.connection, "last_t", &t.to_string()));
    if let Err(error) = result {
        crate::calibration::log(&format!("history index update failed: {error}"));
    }
}

/// The minutes with `from <= minute <= to`, oldest first.
pub fn read(from: i64, to: i64) -> Result<Vec<Minute>, String> {
    let guard = INDEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let owned;
    let connection = match guard.as_ref() {
        Some(index) => &index.connection,
        None => {
            owned = open()?;
            &owned
        }
    };
    let mut statement = connection
        .prepare(
            "SELECT t, name, wall, active, gained, active_gained, lost, deaths, level_ups,
                    last_gain, last_t, lv, exp, map
             FROM minutes WHERE t >= ?1 AND t <= ?2 ORDER BY t, name",
        )
        .map_err(|e| e.to_string())?;
    let minutes = statement
        .query_map(params![minute_of(from), to], |row| {
            let last_gain: Option<i64> = row.get(9)?;
            let minute: i64 = row.get(0)?;
            Ok(Minute {
                minute,
                t: row.get(10)?,
                name: row.get(1)?,
                wall: row.get(2)?,
                active: row.get(3)?,
                gained: row.get(4)?,
                active_gained: row.get(5)?,
                lost: row.get(6)?,
                deaths: row.get(7)?,
                level_ups: row.get(8)?,
                gain: last_gain.is_some_and(|g| g >= minute),
                last_gain,
                lv: row.get(11)?,
                exp: row.get(12)?,
                map: row.get(13)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(t: i64, exp: i64) -> Row {
        Row {
            t,
            lv: 10,
            exp,
            map: "Field".into(),
            name: Some("A".into()),
            death: false,
        }
    }

    fn sum(deltas: &[Delta], field: impl Fn(&Delta) -> i64) -> i64 {
        deltas.iter().map(field).sum()
    }

    fn grind(from: i64, to: i64, every: i64, each: i64) -> Vec<Delta> {
        let mut aggregator = Aggregator::default();
        let mut out = Vec::new();
        let mut exp = 0;
        for t in from..=to {
            if t > from && (t - from) % every == 0 {
                exp += each;
            }
            out.extend(aggregator.push(row(t, exp)));
        }
        out
    }

    /// The same totals as `stats.test.ts` ("steady gains count as active time").
    #[test]
    fn steady_gains_are_active_like_the_overlay_counts_them() {
        let deltas = grind(0, 600, 5, 10);
        assert_eq!(sum(&deltas, |d| d.active), 595);
        assert_eq!(sum(&deltas, |d| d.gained), 1200);
        assert_eq!(sum(&deltas, |d| d.wall), 600);
    }

    #[test]
    fn a_wait_longer_than_the_pause_is_not_active() {
        let deltas = grind(0, 300, IDLE_PAUSE + 1, 100);
        assert_eq!(sum(&deltas, |d| d.active), 0);
        assert!(sum(&deltas, |d| d.gained) > 0);
    }

    #[test]
    fn an_active_wait_across_a_minute_boundary_is_split() {
        let mut aggregator = Aggregator::default();
        aggregator.push(row(50, 0));
        let mut deltas = Vec::new();
        for t in 51..=70 {
            deltas.extend(aggregator.push(row(t, if t >= 55 { 10 } else { 0 })));
        }
        for t in 71..=75 {
            deltas.extend(aggregator.push(row(t, if t == 75 { 20 } else { 10 })));
        }
        // The wait from 55 to 75 is active: 5 s in the first minute, 15 in the second.
        let first: i64 = deltas
            .iter()
            .filter(|d| d.minute == 0)
            .map(|d| d.active)
            .sum();
        let second: i64 = deltas
            .iter()
            .filter(|d| d.minute == 60)
            .map(|d| d.active)
            .sum();
        assert_eq!((first, second), (5, 15));
    }

    #[test]
    fn a_death_is_lost_exp_and_a_switch_starts_over() {
        let mut aggregator = Aggregator::default();
        aggregator.push(row(0, 1000));
        let mut dead = row(1, 900);
        dead.death = true;
        let deltas = aggregator.push(dead);
        assert_eq!(
            (deltas[0].lost, deltas[0].deaths, deltas[0].gained),
            (100, 1, 0)
        );
        let mut other = row(2, 5);
        other.name = Some("B".into());
        let deltas = aggregator.push(other);
        assert_eq!((deltas[0].gained, deltas[0].wall), (0, 0));
    }

    #[test]
    fn a_flagged_or_levelless_row_is_skipped() {
        let bad = serde_json::json!({"t": 1, "lv": 10, "exp": 5, "map": "M", "bad": true});
        let unread = serde_json::json!({"t": 1, "lv": 0, "exp": 5, "map": "M"});
        assert!(Row::from_value(&bad).is_none());
        assert!(Row::from_value(&unread).is_none());
    }

    #[test]
    fn the_index_rebuilds_from_the_day_files_and_reads_back() {
        crate::samples::with_history(|_| rebuilds_and_reads_back());
    }

    fn rebuilds_and_reads_back() {
        // Tests run against a per-process temp data folder.
        let day = chrono::Local::now().timestamp();
        let start = day - day.rem_euclid(3600) - 3600;
        let mut exp = 0;
        for t in start..start + 300 {
            if (t - start) % 5 == 0 && t > start {
                exp += 10;
            }
            crate::samples::append(&serde_json::json!({
                "t": t, "lv": 10, "exp": exp, "pct": 0.0, "map": "Field",
                "state": "gaining", "conf": 1.0, "name": "Indexed"
            }))
            .expect("append");
        }
        rebuild().expect("rebuild");
        let minutes: Vec<Minute> = read(start, start + 300)
            .expect("read")
            .into_iter()
            .filter(|minute| minute.name == "Indexed")
            .collect();
        assert_eq!(minutes.len(), 5);
        assert_eq!(minutes.iter().map(|m| m.gained).sum::<i64>(), exp);
        assert_eq!(minutes.iter().map(|m| m.active).sum::<i64>(), 295 - 5);
    }
}
