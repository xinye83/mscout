import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CaptureStatus, Profile, Sample, TargetStatus } from "./types";
import {
  currentCharacterRun,
  etaSeconds,
  formatDuration,
  formatRate,
  rateOver,
  total,
} from "./rates";
import { expRequired } from "./expTable";
import ExpChart from "./ExpChart";
import { dotState } from "./overlayState";

const MAX_SAMPLES = 5400;
/** Solid dash placeholders, sized to the element they stand in for. */
const PLACEHOLDER = {
  map: "———",
  street: "———",
  name: "———",
  job: "——",
  level: "—",
  pct: "—%",
  rate: "—/h",
  eta: "—",
  next: "—",
};

function joinWords(words: string[]): string {
  if (words.length <= 1) return words[0] ?? "";
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;
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

  // Only a reading captured during this run counts as live. On-disk history is
  // loaded for the chart and rates, but must never be shown as the current
  // value: with no game attached the overlay holds placeholders instead.
  const current =
    capture?.state === "lost"
      ? undefined
      : samples.filter((sample) => sample.t >= sessionStart).at(-1);

  // Rows whose level was never recognised are shown live but kept out of the
  // chart and the rates: with level 0 the cumulative total has no meaning, and the
  // curve would bend towards the origin. Only the character now on screen is
  // charted: a rate across a character switch compares two unrelated totals.
  const charted = useMemo(
    () => currentCharacterRun(samples.filter((sample) => sample.lv > 0)),
    [samples],
  );

  const derived = useMemo(() => {
    if (!current) return null;

    // A window is only shown once this session has lived through it, so a
    // restart does not display a rate or ETA computed from pre-restart history.
    const sessionSeconds = current.t - sessionStart;
    const windowRate = (seconds: number) =>
      sessionSeconds >= seconds ? rateOver(charted, seconds) : null;

    const rate1 = windowRate(60);
    const rate5 = windowRate(300);
    const rate30 = windowRate(1800);

    const gainAt = lastGainAt(charted);
    const idle = gainAt === null || current.t - gainAt > 120;

    return {
      rate1,
      rate5,
      rate30,
      // Both prominent numbers use the 5-minute average. No ETA while idle:
      // the average decays and the estimate would only climb.
      eta: idle ? null : etaSeconds(current, rate5),
    };
  }, [charted, current, sessionStart]);

  const chartData = useMemo(
    () => charted.map((sample) => ({ t: sample.t, total: total(sample) })),
    [charted],
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

  const mapHidden = current?.mapHidden === true;
  const mapUnknown = current?.map === "unknown" && !mapHidden;
  // Anything the overlay cannot show because it was never recognised. `profile` is
  // null until the first reading arrives, so it is not treated as missing. A level
  // of 0 is the backend's "not recognised" for the level box.
  const unrecognised: string[] = [];
  if (mapUnknown) unrecognised.push("map");
  if (profile && !profile.name) unrecognised.push("name");
  if (profile && !profile.job) unrecognised.push("job");
  if (current && current.lv <= 0) unrecognised.push("level");

  const dot = dotState(capture, unrecognised, mapHidden);
  const lastHeight = useRef(0);

  // The window is created at a fixed size, and content that wraps (a multi-line
  // warning) used to overlap the rows below it. Let the panel decide the height and
  // push it to the window instead.
  useEffect(() => {
    const panel = document.querySelector<HTMLElement>(".panel");
    if (!panel) return;
    lastHeight.current = 0;

    const push = () => {
      const height = Math.ceil(panel.getBoundingClientRect().height);
      if (Math.abs(height - lastHeight.current) < 1) return;
      lastHeight.current = height;
      void invoke("resize_overlay", { height }).catch(() => undefined);
    };

    push();
    const observer = new ResizeObserver(push);
    observer.observe(panel);
    return () => observer.disconnect();
  }, [expanded, current !== undefined]);
  const uncalibrated = capture?.state === "uncalibrated";
  const lost = capture?.state === "lost";
  const statusMapHidden = mapHidden && !lost;

  // One status line, not one per problem: two rows read as two separate complaints
  // and both ended with the same instruction, so they looked like echoes. The read
  // failure and the unrecognised fields are listed together and share one action.
  // A lost window is the exception: that reason is the whole message, and stale
  // field warnings from the last reading are noise.
  const readReason =
    capture && capture.state !== "reading"
      ? capture.reason ?? (uncalibrated ? "not calibrated for this size" : "not reading")
      : "";
  const recognisable =
    !lost && unrecognised.length > 0
      ? `${joinWords(unrecognised)} not recognised — teach ${
          unrecognised.length > 1 ? "them" : "it"
        } in Calibration`
      : "";

  // The overlay keeps its shape whatever the reading is doing. A value that could
  // not be read becomes a placeholder rather than dropping its row, so a failed
  // read can never reflow the window or hide the layout being read.
  const pct = current ? Math.max(0, Math.min(100, current.pct)) : 0;
  const lv = current?.lv ?? 0;
  const requirement = expRequired(lv);

  const rateText = (value: number | null | undefined) =>
    value === null || value === undefined ? PLACEHOLDER.rate : formatRate(value);
  const etaText = (value: number | null | undefined) =>
    value === null || value === undefined ? PLACEHOLDER.eta : formatDuration(value);

  // The label is stored as "street\nmap name". The map name (last line) is what
  // actually identifies the map, so it is the headline; the street is context.
  // A collapsed minimap draws no name at all: both lines become placeholders and
  // "minimap collapsed" joins the warnings at the bottom.
  const mapLines =
    mapHidden || !current
      ? []
      : current.map.split("\n");
  const mapLabel =
    mapHidden || !current ? PLACEHOLDER.map : mapLines[mapLines.length - 1];
  const mapSub =
    current && !mapHidden && mapLines.length > 1
      ? mapLines[0]
      : PLACEHOLDER.street;
  const mapSubRead = current && !mapHidden && mapLines.length > 1;

  return (
    <div className={`panel ${expanded ? "expanded" : ""} ${interactive ? "active" : ""}`}>
      <div className="row head" data-tauri-drag-region>
        <span className={`dot ${dot.tone}`} title={dot.title} />
        <span
          className={`map ${current && !mapHidden && !mapUnknown ? "" : "muted"}`}
          data-tauri-drag-region
        >
          {mapLabel}
        </span>
        {capture && capture.state !== "reading" && (
          <span className={`state ${capture.state}`}>{capture.state}</span>
        )}
      </div>

      {mapSub && (
        <div
          className={`row map-sub ${mapSubRead ? "" : "muted"}`}
          data-tauri-drag-region
        >
          <span>{mapSub}</span>
        </div>
      )}

      <div className="row identity">
        <span className={profile?.name ? undefined : "muted"}>
          {profile?.name ?? PLACEHOLDER.name}
        </span>
        <span className={profile?.job ? undefined : "muted"}>
          {profile?.job ?? PLACEHOLDER.job}
        </span>
      </div>

      <div className="row">
        <span className={lv > 0 ? "level" : "level muted"}>
          Lv {lv > 0 ? lv : PLACEHOLDER.level}
        </span>
        <span className={`pct ${current ? "" : "muted"}`}>
          {current ? `${pct.toFixed(2)}%` : PLACEHOLDER.pct}
        </span>
      </div>

      <div className="bar">
        <div className="fill" style={{ width: `${pct}%` }} />
      </div>

      <div className="row stats">
        <span className="rate">{rateText(derived?.rate5)}</span>
        <span className="eta">{etaText(derived?.eta)}</span>
      </div>

      {expanded && (
        <>
          <div className="sub">
            <span>1m {rateText(derived?.rate1)}</span>
            <span>5m {rateText(derived?.rate5)}</span>
            <span>30m {rateText(derived?.rate30)}</span>
          </div>
          <ExpChart samples={chartData} height={140} />
          <div className="foot">
            <span>
              next:{" "}
              {requirement
                ? `${requirement.value.toLocaleString()} exp${requirement.estimated ? " (est.)" : ""}`
                : PLACEHOLDER.next}
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

      {!expanded && (
        <div className="foot">
          <button className="ghost" onClick={() => setExpanded(true)}>
            more
          </button>
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
      )}

      {(readReason || recognisable || statusMapHidden) && (
        <div
          className={`row status-reason ${
            capture && capture.state !== "reading" ? capture.state : "unrecognised"
          }`}
        >
          <span>
            {lost ? (
              readReason
            ) : (
              <>
                {statusMapHidden && (
                  <>
                    minimap collapsed
                    {(readReason || recognisable) && " · "}
                  </>
                )}
                {readReason &&
                  (recognisable ? readReason.split(" — ")[0].trim() : readReason)}
                {readReason && recognisable && " · "}
                {recognisable && (
                  <>
                    <strong>{joinWords(unrecognised)}</strong> not recognised — teach{" "}
                    {unrecognised.length > 1 ? "them" : "it"} in Calibration
                  </>
                )}
              </>
            )}
          </span>
        </div>
      )}
    </div>
  );
}
