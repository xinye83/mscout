import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CaptureStatus, Profile, Sample, TargetStatus } from "./types";
import { etaSeconds, formatDuration, formatRate, rateOver, total } from "./rates";
import { expRequired } from "./expTable";
import ExpChart from "./ExpChart";

const MAP_MAX = 30;
const MAX_SAMPLES = 5400;

function dotClass(status: CaptureStatus | null) {
  if (!status) return "off";
  if (status.state === "lost") return "lost";
  if (status.state === "degraded") return "degraded";
  return "reading";
}

function lastGainAt(samples: Sample[]): number | null {
  for (let i = samples.length - 1; i > 0; i -= 1) {
    if (total(samples[i]) > total(samples[i - 1])) {
      return samples[i].t;
    }
  }
  return null;
}

export default function App() {
  const [samples, setSamples] = useState<Sample[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [interactive, setInteractive] = useState(false);
  const [target, setTarget] = useState<TargetStatus | null>(null);
  const [capture, setCapture] = useState<CaptureStatus | null>(null);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [sessionStart] = useState(() => Math.floor(Date.now() / 1000));

  useEffect(() => {
    const now = Math.floor(Date.now() / 1000);
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    void invoke<Sample[]>("read_samples", {
      from: Math.floor(start.getTime() / 1000),
      to: now,
    })
      .then(setSamples)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    const unlisten = listen<Sample>("live-sample", (event) => {
      setSamples((current) => {
        const last = current[current.length - 1];
        if (last && last.t === event.payload.t) return current;
        const next = [...current, event.payload];
        return next.length > MAX_SAMPLES ? next.slice(next.length - MAX_SAMPLES) : next;
      });
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<boolean>("interactive", (event) => setInteractive(event.payload));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<CaptureStatus>("capture-status", (event) =>
      setCapture(event.payload),
    );
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    void invoke<Profile>("get_profile").then(setProfile).catch(() => undefined);
    const unlisten = listen<Profile>("profile", (event) => setProfile(event.payload));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    void invoke<TargetStatus>("get_target_status")
      .then(setTarget)
      .catch(() => undefined);
    const unlisten = listen<TargetStatus>("target-status", (event) =>
      setTarget(event.payload),
    );
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    void invoke("set_mode", { expanded }).catch(() => undefined);
  }, [expanded]);

  const current = samples[samples.length - 1];
  const derived = useMemo(() => {
    if (!current) return null;

    // A window is only shown once this session has lived through it, so a
    // restart does not display a rate or ETA computed from pre-restart history.
    const sessionSeconds = current.t - sessionStart;
    const windowRate = (seconds: number) =>
      sessionSeconds >= seconds ? rateOver(samples, seconds) : null;

    const rate1 = windowRate(60);
    const rate5 = windowRate(300);
    const rate30 = windowRate(1800);

    const gainAt = lastGainAt(samples);
    const idle = gainAt === null || current.t - gainAt > 120;

    return {
      rate1,
      rate5,
      rate30,
      // Both prominent numbers use the 5-minute average. No ETA while idle:
      // the average decays and the estimate would only climb.
      eta: idle ? null : etaSeconds(current, rate5),
    };
  }, [samples, current, sessionStart]);

  const chartData = useMemo(
    () => samples.map((sample) => ({ t: sample.t, total: total(sample) })),
    [samples],
  );

  const targetLabel = !target
    ? "…"
    : !target.configured
      ? "none"
      : target.found
        ? target.minimized
          ? "minimized"
          : target.process || target.title || "attached"
        : "not found";

  const dot = dotClass(capture);

  if (!current || !derived) {
    return (
      <div className={`panel ${expanded ? "expanded" : ""} ${interactive ? "active" : ""}`}>
        <div className="row head" data-tauri-drag-region>
          <span className={`dot ${dot}`} />
          <span className="map" data-tauri-drag-region>
            {capture?.state === "lost"
              ? capture.reason ?? "not reading"
              : "waiting for readings"}
          </span>
          {interactive && <span className="chip">interactive</span>}
        </div>
        {interactive && (
          <div className="foot">
            <button
              className="ghost"
              onClick={() => void invoke("open_target_window")}
            >
              window
            </button>
            <button className="ghost" onClick={() => void invoke("open_calibration")}>
              calibrate
            </button>
          </div>
        )}
      </div>
    );
  }

  const mapLines = current.map.split("\n");
  const mapLabel =
    mapLines[0].length > MAP_MAX ? `${mapLines[0].slice(0, MAP_MAX - 1)}…` : mapLines[0];
  const mapSub = mapLines[1] ?? null;
  const pct = Math.max(0, Math.min(100, current.pct));
  const requirement = expRequired(current.lv);

  return (
    <div className={`panel ${expanded ? "expanded" : ""} ${interactive ? "active" : ""}`}>
      <div className="row head" data-tauri-drag-region>
        <span className={`dot ${dot}`} />
        <span className="map" data-tauri-drag-region>
          {mapLabel}
        </span>
        {interactive && <span className="chip">interactive</span>}
        {capture && capture.state !== "reading" && (
          <span className={`state ${capture.state}`}>{capture.state}</span>
        )}
      </div>

      {mapSub && (
        <div className="row map-sub" data-tauri-drag-region>
          <span>{mapSub}</span>
        </div>
      )}

      {profile && (profile.name || profile.job) && (
        <div className="row identity">
          <span>{profile.name ?? "—"}</span>
          <span>{profile.job ?? "—"}</span>
        </div>
      )}

      <div className="row">
        <span className="level">Lv {current.lv}</span>
        <span className="pct">{pct.toFixed(2)}%</span>
      </div>

      <div className="bar">
        <div className="fill" style={{ width: `${pct}%` }} />
      </div>

      <div className="row stats">
        <span className="rate">{formatRate(derived.rate5)}</span>
        <span className="eta">{formatDuration(derived.eta)}</span>
      </div>

      {expanded && (
        <>
          <div className="sub">
            <span>1m {formatRate(derived.rate1)}</span>
            <span>5m {formatRate(derived.rate5)}</span>
            <span>30m {formatRate(derived.rate30)}</span>
          </div>
          <ExpChart samples={chartData} height={140} />
          <div className="foot">
            <span>
              next:{" "}
              {requirement
                ? `${requirement.value.toLocaleString()} exp${requirement.estimated ? " (est.)" : ""}`
                : "—"}
            </span>
            <button className="ghost" onClick={() => setExpanded(false)}>
              less
            </button>
          </div>
          <div className="foot">
            <span>window: {targetLabel}</span>
            <span className="foot-actions">
              <button
                className="ghost"
                onClick={() => void invoke("open_target_window")}
              >
                pick
              </button>
              <button
                className="ghost"
                onClick={() => void invoke("open_calibration")}
              >
                calibrate
              </button>
            </span>
          </div>
        </>
      )}

      {!expanded && interactive && (
        <div className="foot">
          <button className="ghost" onClick={() => setExpanded(true)}>
            more
          </button>
        </div>
      )}
    </div>
  );
}
