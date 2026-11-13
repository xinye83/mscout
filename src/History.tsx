import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RepairSummary, Sample } from "./types";
import { formatRate, total } from "./rates";
import ExpChart, { type ChartPoint } from "./ExpChart";

const RANGES = [
  { label: "1 day", days: 1 },
  { label: "7 days", days: 7 },
  { label: "30 days", days: 30 },
];

const GAP_SEC = 120;

function downsample(points: ChartPoint[], max: number): ChartPoint[] {
  if (points.length <= max) return points;
  const bucket = Math.ceil(points.length / max);
  const out: ChartPoint[] = [];
  for (let i = 0; i < points.length; i += bucket) {
    out.push(points[Math.min(i + bucket - 1, points.length - 1)]);
  }
  return out;
}

export default function History() {
  const [days, setDays] = useState(7);
  const [samples, setSamples] = useState<Sample[] | null>(null);
  const [reload, setReload] = useState(0);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    const now = Math.floor(Date.now() / 1000);
    setSamples(null);
    invoke<Sample[]>("read_samples", { from: now - days * 86_400, to: now })
      .then(setSamples)
      .catch(() => setSamples([]));
  }, [days, reload]);

  const repair = async () => {
    setNote(null);
    try {
      const summary = await invoke<RepairSummary>("repair_history");
      setNote(
        summary.flagged === 0
          ? `Checked ${summary.checked.toLocaleString()} rows — none needed flagging.`
          : `Flagged ${summary.flagged.toLocaleString()} misread rows of ${summary.checked.toLocaleString()} checked. They are skipped from now on, not deleted.`,
      );
      setReload((value) => value + 1);
    } catch (cause) {
      setNote(String(cause));
    }
  };

  const points = useMemo(() => {
    if (!samples) return [];
    const full: ChartPoint[] = [];
    let prev: number | null = null;

    for (const sample of samples) {
      if (prev !== null && sample.t - prev > GAP_SEC) {
        full.push({ t: prev + 1, total: null });
      }
      full.push({ t: sample.t, total: total(sample) });
      prev = sample.t;
    }

    return downsample(full, 4000);
  }, [samples]);

  const summary = useMemo(() => {
    if (!samples || samples.length < 2) return null;

    let gained = 0;
    let recordedSec = 0;
    for (let i = 1; i < samples.length; i += 1) {
      const dt = samples[i].t - samples[i - 1].t;
      if (dt <= GAP_SEC) recordedSec += dt;
      gained += total(samples[i]) - total(samples[i - 1]);
    }

    return {
      count: samples.length,
      gained,
      hours: recordedSec / 3600,
      perHour: recordedSec > 0 ? gained / (recordedSec / 3600) : 0,
      maps: new Set(samples.map((s) => s.map)).size,
    };
  }, [samples]);

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>History</h1>
          <p className="muted">Recorded EXP samples, wall-clock time.</p>
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

      {samples === null && <div className="empty">loading…</div>}

      {samples !== null && samples.length < 2 && (
        <div className="empty">
          No recorded samples in this range.
          <br />
          Capture and storage land next.
        </div>
      )}

      {summary && (
        <>
          <div className="summary">
            <div>
              <span className="k">{summary.count.toLocaleString()}</span>
              <span className="l">samples</span>
            </div>
            <div>
              <span className="k">{summary.gained.toLocaleString()}</span>
              <span className="l">exp gained</span>
            </div>
            <div>
              <span className="k">{formatRate(summary.perHour)}</span>
              <span className="l">avg rate</span>
            </div>
            <div>
              <span className="k">{summary.hours.toFixed(1)}h</span>
              <span className="l">recorded</span>
            </div>
          </div>
          <ExpChart samples={points} height={330} />
          <p className="muted small">
            Flat steps are idle time. Breaks in the line are periods with no recording.
          </p>
        </>
      )}
    </div>
  );
}
