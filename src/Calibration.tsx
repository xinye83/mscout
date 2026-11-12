import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Calibration, LearningInfo, Readings, Region } from "./types";

type Mode = "exp" | "level" | "map" | "name" | "job";

type Preview = {
  width: number;
  height: number;
  serial: number;
};

const MODE_LABEL: Record<Mode, string> = {
  exp: "EXP text",
  level: "Level digits",
  map: "Map label",
  name: "Name",
  job: "Job",
};

function normalize(a: { x: number; y: number }, b: { x: number; y: number }): Region {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  };
}

function clamp(value: number, min: number, max: number) {
  return Math.max(min, Math.min(max, value));
}

function withRegion(
  current: Calibration,
  key: Mode,
  region: Region | null,
): Calibration {
  if (key === "exp") return { ...current, exp: region };
  if (key === "level") return { ...current, level: region };
  if (key === "map") return { ...current, map: region };
  if (key === "name") return { ...current, name: region };
  return { ...current, job: region };
}

function Box({
  region,
  preview,
  className,
  label,
}: {
  region: Region;
  preview: Preview;
  className: string;
  label: string;
}) {
  return (
    <div
      className={`roi ${className}`}
      style={{
        left: `${(region.x / preview.width) * 100}%`,
        top: `${(region.y / preview.height) * 100}%`,
        width: `${(region.w / preview.width) * 100}%`,
        height: `${(region.h / preview.height) * 100}%`,
      }}
    >
      <span>{label}</span>
    </div>
  );
}

export default function Calibration() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);
  const startRef = useRef<{ x: number; y: number } | null>(null);

  const [preview, setPreview] = useState<Preview | null>(null);
  const [regions, setRegions] = useState<Calibration>({
    clientW: null,
    clientH: null,
    exp: null,
    level: null,
    map: null,
    name: null,
    job: null,
  });
  const [mode, setMode] = useState<Mode>("exp");
  const [drag, setDrag] = useState<Region | null>(null);
  const [learning, setLearning] = useState<LearningInfo | null>(null);
  const [typed, setTyped] = useState("");
  const [mapName, setMapName] = useState("");
  const [nameLabel, setNameLabel] = useState("");
  const [jobLabel, setJobLabel] = useState("");
  const [probe, setProbe] = useState<Readings | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const freeze = useCallback(async () => {
    setError(null);
    setMessage(null);
    try {
      const raw = await invoke<ArrayBuffer | number[]>("capture_preview");
      const bytes =
        raw instanceof ArrayBuffer ? new Uint8Array(raw) : new Uint8Array(raw);
      const header = new DataView(bytes.buffer, bytes.byteOffset, 12);
      const width = header.getUint32(0, true);
      const height = header.getUint32(4, true);
      const serial = Number(header.getBigUint64(8, true));

      const canvas = canvasRef.current;
      const context = canvas?.getContext("2d");
      if (!canvas || !context) return;

      const pixels = new Uint8ClampedArray(
        bytes.buffer,
        bytes.byteOffset + 12,
        width * height * 4,
      );
      canvas.width = width;
      canvas.height = height;
      context.putImageData(new ImageData(pixels, width, height), 0, 0);
      setPreview({ width, height, serial });
    } catch (cause) {
      setError(String(cause));
    }
  }, []);

  useEffect(() => {
    invoke<Calibration>("get_calibration")
      .then(setRegions)
      .catch(() => undefined);
    invoke<LearningInfo>("get_learning")
      .then(setLearning)
      .catch(() => undefined);
    void freeze();
  }, [freeze]);

  const saveRegions = useCallback(async () => {
    if (!preview) throw new Error("freeze a frame first");
    const saved = await invoke<Calibration>("set_calibration", {
      clientW: preview.width,
      clientH: preview.height,
      exp: regions.exp,
      level: regions.level,
      map: regions.map,
      name: regions.name,
      job: regions.job,
    });
    setRegions(saved);
    return saved;
  }, [preview, regions]);

  const point = (event: ReactPointerEvent) => {
    const element = previewRef.current;
    if (!element || !preview) return { x: 0, y: 0 };
    const bounds = element.getBoundingClientRect();
    return {
      x: clamp(
        Math.round(((event.clientX - bounds.left) / bounds.width) * preview.width),
        0,
        preview.width,
      ),
      y: clamp(
        Math.round(((event.clientY - bounds.top) / bounds.height) * preview.height),
        0,
        preview.height,
      ),
    };
  };

  const onPointerDown = (event: ReactPointerEvent) => {
    if (!preview) return;
    const start = point(event);
    startRef.current = start;
    setDrag({ x: start.x, y: start.y, w: 0, h: 0 });
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: ReactPointerEvent) => {
    if (!startRef.current) return;
    setDrag(normalize(startRef.current, point(event)));
  };

  const onPointerUp = () => {
    if (!startRef.current) return;
    startRef.current = null;
    if (drag && drag.w > 4 && drag.h > 4) {
      setRegions((current) => withRegion(current, mode, drag));
    }
    setDrag(null);
  };

  const learn = async () => {
    setError(null);
    setMessage(null);
    try {
      await saveRegions();
      const info = await invoke<LearningInfo>("learn_atlas", {
        serial: preview?.serial ?? 0,
        text: typed,
      });
      setLearning(info);
      setMessage(`Learned: ${info.learned || "—"}`);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const teach = async () => {
    setError(null);
    setMessage(null);
    try {
      await saveRegions();
      const info = await invoke<LearningInfo>("teach_map", {
        serial: preview?.serial ?? 0,
        name: mapName,
      });
      setLearning(info);
      setMessage(`Map taught: ${mapName.trim()}`);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const teachIdentity = async (kind: "name" | "job") => {
    setError(null);
    setMessage(null);
    const label = kind === "name" ? nameLabel : jobLabel;
    try {
      await saveRegions();
      const info = await invoke<LearningInfo>("teach_identity", {
        serial: preview?.serial ?? 0,
        kind,
        label,
      });
      setLearning(info);
      setMessage(`${kind === "name" ? "Name" : "Job"} taught: ${label.trim()}`);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const test = async () => {
    setError(null);
    setMessage(null);
    setProbe(null);
    try {
      await saveRegions();
      const result = await invoke<Readings>("parse_probe", {
        serial: preview?.serial ?? 0,
      });
      setProbe(result);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const coverage = learning ? learning.coverage : "";
  const missing = "0123456789"
    .split("")
    .filter((digit) => !coverage.includes(digit));

  return (
    <div className="page calibration">
      <header className="page-head">
        <div>
          <h1>Calibration</h1>
          <p className="muted">
            Freeze a frame, drag the boxes, then teach the EXP digits and the labels.
          </p>
        </div>
        <div className="presets">
          <button className="ghost" onClick={() => void freeze()}>
            Freeze frame
          </button>
        </div>
      </header>

      <div className="toolbar">
        {(Object.keys(MODE_LABEL) as Mode[]).map((value) => (
          <button
            key={value}
            className={`ghost ${mode === value ? "on" : ""}`}
            onClick={() => setMode(value)}
          >
            {MODE_LABEL[value]}
          </button>
        ))}
        <span className="muted small">
          {preview
            ? `${preview.width}×${preview.height} · drag on the image`
            : "no frame yet"}
        </span>
      </div>

      <div
        className="preview"
        ref={previewRef}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
      >
        <canvas ref={canvasRef} />
        {preview && regions.exp && (
          <Box region={regions.exp} preview={preview} className="roi-exp" label="EXP" />
        )}
        {preview && regions.level && (
          <Box
            region={regions.level}
            preview={preview}
            className="roi-level"
            label="LV"
          />
        )}
        {preview && regions.map && (
          <Box region={regions.map} preview={preview} className="roi-map" label="MAP" />
        )}
        {preview && regions.name && (
          <Box region={regions.name} preview={preview} className="roi-name" label="NAME" />
        )}
        {preview && regions.job && (
          <Box region={regions.job} preview={preview} className="roi-job" label="JOB" />
        )}
        {preview && drag && (
          <Box region={drag} preview={preview} className="roi-drag" label={mode} />
        )}
        {!preview && <div className="preview-empty">Freeze a frame to start</div>}
      </div>

      <div className="cal-grid">
        <section className="cal-card">
          <h2>Boxes</h2>
          <p className="muted small">
            EXP text includes the number and percentage. Level must be digits only.
            Map, name and job are optional fingerprints.
          </p>
          <div className="cal-row">
            {(Object.keys(MODE_LABEL) as Mode[]).map((key) => (
              <button
                key={key}
                className="ghost"
                onClick={() =>
                  setRegions((current) => withRegion(current, key, null))
                }
              >
                clear {MODE_LABEL[key].toLowerCase()}
              </button>
            ))}
          </div>
          <button
            className="ghost on"
            onClick={() => void saveRegions().catch((cause) => setError(String(cause)))}
          >
            Save boxes
          </button>
        </section>

        <section className="cal-card">
          <h2>EXP digits</h2>
          <p className="muted small">
            Type exactly what the EXP box shows, including punctuation.
          </p>
          <input
            className="cal-input"
            value={typed}
            placeholder="10,105 [91.53%]"
            onChange={(event) => setTyped(event.target.value)}
          />
          <button className="ghost on" onClick={() => void learn()} disabled={!preview}>
            Learn digits
          </button>
          <p className="muted small">
            Learned: {learning?.learned || "—"}
            {missing.length > 0 && ` · missing ${missing.join(" ")}`}
          </p>
        </section>

        <section className="cal-card">
          <h2>Labels</h2>
          <p className="muted small">Map, character name and job are taught once.</p>
          <input
            className="cal-input"
            value={mapName}
            placeholder="Map name (e.g. Kerning City)"
            onChange={(event) => setMapName(event.target.value)}
          />
          <button className="ghost on" onClick={() => void teach()} disabled={!preview}>
            Teach map
          </button>
          <p className="muted small">
            Known maps: {learning?.maps.length ? learning.maps.join(", ") : "—"}
          </p>
          <input
            className="cal-input"
            value={nameLabel}
            placeholder="Character name"
            onChange={(event) => setNameLabel(event.target.value)}
          />
          <button
            className="ghost on"
            onClick={() => void teachIdentity("name")}
            disabled={!preview}
          >
            Teach name
          </button>
          <input
            className="cal-input"
            value={jobLabel}
            placeholder="Job (e.g. Page)"
            onChange={(event) => setJobLabel(event.target.value)}
          />
          <button
            className="ghost on"
            onClick={() => void teachIdentity("job")}
            disabled={!preview}
          >
            Teach job
          </button>
          <p className="muted small">
            Name: {learning?.name ?? "—"} · Job: {learning?.job ?? "—"}
          </p>
        </section>

        <section className="cal-card">
          <h2>Test</h2>
          <p className="muted small">Reads the frozen frame with the saved boxes.</p>
          <button className="ghost on" onClick={() => void test()} disabled={!preview}>
            Test parse
          </button>
          {probe && (
            <p className="muted small">
              Lv {probe.lv} · {probe.exp.toLocaleString()} EXP · {probe.pct.toFixed(2)}%
              {" · "}
              {probe.map} · {probe.name ?? "—"} · {probe.job ?? "—"} · conf{" "}
              {probe.conf.toFixed(2)}
            </p>
          )}
        </section>
      </div>

      {message && <p className="ok small">{message}</p>}
      {error && <p className="error small">{error}</p>}
    </div>
  );
}
