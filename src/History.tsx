import { useEffect, useMemo, useState } from "react";
import * as api from "./api";
import type { HistoryMinute } from "./types";
import { formatDuration, formatRate } from "./rates";
import { totalExp } from "./expTable";
import { sessions, type Session, type Slice } from "./stats";
import ExpChart, { type ChartPoint } from "./ExpChart";
import { downsample } from "./chartData";

const RANGES = [
  { label: "1 day", days: 1 },
  { label: "7 days", days: 7 },
  { label: "30 days", days: 30 },
  { label: "all", days: 0 },
];

/** Minutes further apart than this are drawn as a break in the line. */
const GAP_SEC = 180;

/** A map counts in the per-map table once this much active time was spent there. */
const MAP_MIN_ACTIVE = 300;

function toSlice(minute: HistoryMinute): Slice {
  return {
    t: minute.t,
    wall: minute.wall,
    active: minute.active,
    gained: minute.gained,
    activeGained: minute.activeGained,
    lost: minute.lost,
    deaths: minute.deaths,
    levelUps: minute.levelUps,
    gain: minute.gain,
    lastGain: minute.lastGain,
    lv: minute.lv,
    exp: minute.exp,
    map: minute.map,
    name: minute.name || undefined,
  };
}

function rate(gained: number, seconds: number): number | null {
  return seconds > 0 ? (gained / seconds) * 3600 : null;
}

function when(t: number): string {
  return new Date(t * 1000).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function mapName(map: string): string {
  const lines = map.split("\n");
  return lines[lines.length - 1];
}

function band(level: number): string {
  const low = Math.floor(level / 10) * 10;
  return `${Math.max(1, low)}–${low + 9}`;
}

type MapRow = {
  map: string;
  band: string;
  active: number;
  gained: number;
  sessions: number;
  best: number | null;
};

function mapTable(found: Session[]): MapRow[] {
  const rows = new Map<string, MapRow>();
  for (const session of found) {
    const seen = new Set<string>();
    for (const share of session.maps) {
      if (share.map === "unknown") continue;
      const key = `${mapName(share.map)}\u0000${band(share.level)}`;
      const row = rows.get(key) ?? {
        map: mapName(share.map),
        band: band(share.level),
        active: 0,
        gained: 0,
        sessions: 0,
        best: null,
      };
      row.active += share.active;
      row.gained += share.gained;
      if (!seen.has(key)) row.sessions += 1;
      seen.add(key);
      const shareRate = share.active >= MAP_MIN_ACTIVE ? rate(share.gained, share.active) : null;
      if (shareRate !== null && (row.best === null || shareRate > row.best)) row.best = shareRate;
      rows.set(key, row);
    }
  }
  return [...rows.values()]
    .filter((row) => row.active >= MAP_MIN_ACTIVE)
    .sort((a, b) => (rate(b.gained, b.active) ?? 0) - (rate(a.gained, a.active) ?? 0));
}

function csv(header: string[], rows: (string | number | null)[][]): string {
  const field = (value: string | number | null) => {
    const text = value === null ? "" : String(value);
    return /[",\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
  };
  return [header, ...rows].map((row) => row.map(field).join(",")).join("\r\n") + "\r\n";
}

export default function History() {
  const [days, setDays] = useState(7);
  const [minutes, setMinutes] = useState<HistoryMinute[] | null>(null);
  const [character, setCharacter] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    const now = Math.floor(Date.now() / 1000);
    setMinutes(null);
    api
      .readMinutes({ from: days > 0 ? now - days * 86_400 : 0, to: now })
      .then(setMinutes)
      .catch((cause) => {
        setNote(String(cause));
        setMinutes([]);
      });
  }, [days, reload]);

  const names = useMemo(() => {
    const order: string[] = [];
    for (const minute of minutes ?? []) {
      const name = minute.name || "(no name)";
      const at = order.indexOf(name);
      if (at >= 0) order.splice(at, 1);
      order.push(name);
    }
    return order.reverse();
  }, [minutes]);
  const shown = character && names.includes(character) ? character : (names[0] ?? null);

  const slices = useMemo(
    () =>
      (minutes ?? [])
        .filter((minute) => (minute.name || "(no name)") === shown)
        .map(toSlice),
    [minutes, shown],
  );
  const found = useMemo(() => sessions(slices), [slices]);
  const maps = useMemo(() => mapTable(found), [found]);

  const points = useMemo(() => {
    const full: ChartPoint[] = [];
    let prev: number | null = null;
    for (const slice of slices) {
      if (prev !== null && slice.t - prev > GAP_SEC) full.push({ t: prev + 1, total: null });
      full.push({ t: slice.t, total: totalExp(slice.lv, slice.exp) });
      prev = slice.t;
    }
    return downsample(full, 4000);
  }, [slices]);

  const summary = useMemo(() => {
    if (slices.length === 0) return null;
    const sum = (pick: (slice: Slice) => number) => slices.reduce((s, slice) => s + pick(slice), 0);
    const active = sum((s) => s.active);
    const wall = sum((s) => s.wall);
    return {
      gained: sum((s) => s.gained),
      active,
      wall,
      activeRate: rate(sum((s) => s.activeGained), active),
      withBreaks: rate(sum((s) => s.gained), wall),
      deaths: sum((s) => s.deaths),
      lost: sum((s) => s.lost),
      levelUps: sum((s) => s.levelUps),
    };
  }, [slices]);

  const repair = async () => {
    setNote(null);
    try {
      const result = await api.repairHistory();
      setNote(
        result.flagged === 0
          ? `Checked ${result.checked.toLocaleString()} rows — none needed flagging.`
          : `Flagged ${result.flagged.toLocaleString()} misread rows of ${result.checked.toLocaleString()} checked. They are skipped from now on, not deleted.`,
      );
      setReload((value) => value + 1);
    } catch (cause) {
      setNote(String(cause));
    }
  };

  const exportSessions = async () => {
    const text = csv(
      ["start", "end", "character", "wall_s", "active_s", "exp_gained", "exp_per_active_hour", "exp_per_hour_with_breaks", "best_5m", "best_60m", "level_from", "level_to", "deaths", "exp_lost", "main_map"],
      found.map((s) => [
        new Date(s.start * 1000).toISOString(),
        new Date(s.end * 1000).toISOString(),
        s.name ?? "",
        s.wall,
        s.active,
        s.gained,
        rate(s.gained, s.active)?.toFixed(0) ?? null,
        rate(s.gained, s.wall)?.toFixed(0) ?? null,
        s.best5m?.toFixed(0) ?? null,
        s.best60m?.toFixed(0) ?? null,
        s.levelFrom,
        s.levelTo,
        s.deaths,
        s.lost,
        mainMap(s),
      ]),
    );
    await save(`sessions-${stamp()}`, text);
  };

  const exportMinutes = async () => {
    const text = csv(
      ["minute", "character", "level", "exp", "map", "wall_s", "active_s", "exp_gained", "exp_lost", "deaths", "level_ups"],
      (minutes ?? []).map((m) => [
        new Date(m.minute * 1000).toISOString(),
        m.name,
        m.lv,
        m.exp,
        mapName(m.map),
        m.wall,
        m.active,
        m.gained,
        m.lost,
        m.deaths,
        m.levelUps,
      ]),
    );
    await save(`minutes-${stamp()}`, text);
  };

  const save = async (name: string, text: string) => {
    try {
      const path = await api.saveExport({ name, text });
      setNote(`Saved ${path}`);
    } catch (cause) {
      setNote(String(cause));
    }
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>History</h1>
          <p className="muted">
            Rates use active time (gains no more than 30 s apart); "with breaks" uses the whole
            recorded time.
          </p>
        </div>
        <div className="presets">
          <button className="ghost" onClick={() => void repair()}>
            repair history
          </button>
          {RANGES.map((range) => (
            <button
              key={range.days}
              className={`ghost ${range.days === days ? "on" : ""}`}
              onClick={() => setDays(range.days)}
            >
              {range.label}
            </button>
          ))}
        </div>
      </header>

      {note && <p className="muted small">{note}</p>}

      {minutes === null && <div className="empty">loading…</div>}

      {minutes !== null && slices.length === 0 && (
        <div className="empty">No recorded play in this range.</div>
      )}

      {names.length > 1 && (
        <div className="presets characters">
          {names.map((name) => (
            <button
              key={name}
              className={`ghost ${name === shown ? "on" : ""}`}
              onClick={() => setCharacter(name)}
            >
              {name}
            </button>
          ))}
        </div>
      )}

      {summary && (
        <>
          <div className="summary">
            <div>
              <span className="k">{summary.gained.toLocaleString()}</span>
              <span className="l">exp gained</span>
            </div>
            <div>
              <span className="k">{formatRate(summary.activeRate)}</span>
              <span className="l">active rate</span>
            </div>
            <div>
              <span className="k">{formatRate(summary.withBreaks)}</span>
              <span className="l">with breaks</span>
            </div>
            <div>
              <span className="k">{formatDuration(summary.active)}</span>
              <span className="l">active of {formatDuration(summary.wall)}</span>
            </div>
            <div>
              <span className="k">{found.length}</span>
              <span className="l">sessions</span>
            </div>
            <div>
              <span className="k">
                +{summary.levelUps} lv · {summary.deaths}
              </span>
              <span className="l">levels · deaths</span>
            </div>
          </div>
          <ExpChart samples={points} height={280} />
          <p className="muted small">
            Flat steps are idle time. Breaks in the line are periods with no recording.
          </p>

          <h2>Sessions</h2>
          <table className="history-table">
            <thead>
              <tr>
                <th>start</th>
                <th>length</th>
                <th>active</th>
                <th>exp</th>
                <th>rate</th>
                <th>best 5m</th>
                <th>levels</th>
                <th>deaths</th>
                <th>main map</th>
              </tr>
            </thead>
            <tbody>
              {[...found].reverse().map((s) => (
                <tr key={s.start}>
                  <td>{when(s.start)}</td>
                  <td>{formatDuration(s.end - s.start)}</td>
                  <td>{formatDuration(s.active)}</td>
                  <td>{s.gained.toLocaleString()}</td>
                  <td>{formatRate(rate(s.gained, s.active))}</td>
                  <td>{formatRate(s.best5m)}</td>
                  <td>
                    {s.levelFrom === s.levelTo ? s.levelTo : `${s.levelFrom}→${s.levelTo}`}
                  </td>
                  <td>{s.deaths || ""}</td>
                  <td>{mainMap(s)}</td>
                </tr>
              ))}
            </tbody>
          </table>

          <h2>Maps</h2>
          {maps.length === 0 ? (
            <p className="muted small">
              No map with five minutes of active play in this range.
            </p>
          ) : (
            <table className="history-table">
              <thead>
                <tr>
                  <th>map</th>
                  <th>levels</th>
                  <th>active</th>
                  <th>rate</th>
                  <th>best session</th>
                  <th>sessions</th>
                </tr>
              </thead>
              <tbody>
                {maps.map((row) => (
                  <tr key={`${row.map}-${row.band}`}>
                    <td>{row.map}</td>
                    <td>{row.band}</td>
                    <td>{formatDuration(row.active)}</td>
                    <td>{formatRate(rate(row.gained, row.active))}</td>
                    <td>{formatRate(row.best)}</td>
                    <td>{row.sessions}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          <div className="presets exports">
            <button className="ghost" onClick={() => void exportSessions()}>
              export sessions (CSV)
            </button>
            <button className="ghost" onClick={() => void exportMinutes()}>
              export minutes (CSV)
            </button>
          </div>
        </>
      )}
    </div>
  );
}

function mainMap(session: Session): string {
  const best = [...session.maps].sort((a, b) => b.active - a.active)[0];
  return best ? mapName(best.map) : "";
}

function stamp(): string {
  return new Date().toISOString().slice(0, 16).replace(/[:T]/g, "-");
}
