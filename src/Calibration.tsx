import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Calibration,
  GlyphGuess,
  LabelCrop,
  LearningInfo,
  ProfileSummary,
  Readings,
  Region,
  ReviewFrame,
  TargetStatus,
  UiModeOption,
} from "./types";

type Mode = "exp" | "map" | "identity";

function hexBytes(hex: string): Uint8Array {
  const out = new Uint8Array(Math.floor(hex.length / 2));
  for (let index = 0; index < out.length; index += 1) {
    out[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  return out;
}

// Paint a grey crop at 1:1; the browser magnifies it with pixelated CSS.
function paintReview(
  canvas: HTMLCanvasElement | null,
  frame: { w: number; h: number; px: string },
) {
  if (!canvas) return;
  const context = canvas.getContext("2d");
  if (!context) return;

  const bytes = hexBytes(frame.px);
  canvas.width = frame.w;
  canvas.height = frame.h;

  const image = context.createImageData(frame.w, frame.h);
  for (let index = 0; index < bytes.length; index += 1) {
    image.data[index * 4] = bytes[index];
    image.data[index * 4 + 1] = bytes[index];
    image.data[index * 4 + 2] = bytes[index];
    image.data[index * 4 + 3] = 255;
  }
  context.putImageData(image, 0, 0);
}

type Preview = {
  width: number;
  height: number;
  serial: number;
};

const MODE_LABEL: Record<Mode, string> = {
  exp: "EXP text",
  map: "Map label",
  identity: "Name / Job",
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
  if (key === "map") return { ...current, map: region };
  // Identity is one drawn box that becomes the name and job regions, so it is
  // applied as a pair by the caller, never through this single-region helper.
  return current;
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

const GLYPH_LABELS = "0123456789,.[]%".split("");

/** A magnified crop of one segmented shape, so it can be identified by eye. */
function GlyphCrop({
  source,
  serial,
  guess,
}: {
  source: HTMLCanvasElement | null;
  serial: number;
  guess: GlyphGuess;
}) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || !source) return;
    const scale = Math.max(2, Math.ceil(44 / Math.max(guess.w, guess.h)));
    canvas.width = guess.w * scale;
    canvas.height = guess.h * scale;
    const context = canvas.getContext("2d");
    if (!context) return;
    context.imageSmoothingEnabled = false;
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.drawImage(
      source,
      guess.x,
      guess.y,
      guess.w,
      guess.h,
      0,
      0,
      canvas.width,
      canvas.height,
    );
  }, [source, serial, guess]);

  return <canvas ref={ref} className="glyph-crop" />;
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
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [activeProfile, setActiveProfile] = useState<ProfileSummary | null>(null);
  const [uiModes, setUiModes] = useState<UiModeOption[]>([]);
  const [sizeChoice, setSizeChoice] = useState<string | null>(null);
  const [reviewFrames, setReviewFrames] = useState<ReviewFrame[]>([]);
  const [labelCrop, setLabelCrop] = useState<LabelCrop | null>(null);
  const [reviewId, setReviewId] = useState<string | null>(null);
  const [reviewText, setReviewText] = useState("");
  const [client, setClient] = useState<{ w: number; h: number } | null>(null);
  const [confirmKey, setConfirmKey] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [glyphs, setGlyphs] = useState<GlyphGuess[]>([]);
  const [selectedGlyph, setSelectedGlyph] = useState<number | null>(null);

  const refreshGlyphs = useCallback((serial: number) => {
    invoke<GlyphGuess[]>("exp_glyphs", { serial })
      .then(setGlyphs)
      .catch(() => setGlyphs([]));
  }, []);

  const refreshProfiles = useCallback(() => {
    invoke<ProfileSummary[]>("list_profiles")
      .then(setProfiles)
      .catch(() => undefined);
    invoke<ProfileSummary | null>("active_profile")
      .then(setActiveProfile)
      .catch(() => undefined);
    invoke<UiModeOption[]>("ui_modes")
      .then(setUiModes)
      .catch(() => undefined);
    invoke<ReviewFrame[]>("review_frames")
      .then(setReviewFrames)
      .catch(() => undefined);
  }, []);

  const labelReview = async (frame: ReviewFrame) => {
    setError(null);
    setMessage(null);
    try {
      const info = await invoke<LearningInfo>("label_review", {
        id: frame.id,
        text: reviewText,
      });
      setLearning(info);
      setMessage(`Learned ${reviewText.trim()} from the saved frame`);
      setReviewId(null);
      setReviewText("");
      refreshProfiles();
    } catch (cause) {
      setError(String(cause));
    }
  };

  const clearReview = async () => {
    setError(null);
    try {
      const removed = await invoke<number>("clear_review");
      setMessage(`Cleared ${removed} saved frames`);
      setReviewId(null);
      refreshProfiles();
    } catch (cause) {
      setError(String(cause));
    }
  };

  // Switching UI Size swaps the whole set in use: boxes, digits and labels all
  // have to be re-read for the newly selected mode.
  const chooseUiMode = useCallback(
    async (option: UiModeOption) => {
      if (option.active) return;
      setError(null);
      setMessage(null);
      try {
        await invoke("set_ui_mode", { mode: option.id });
        setMessage(`UI size set to ${option.label}`);
        refreshProfiles();
        invoke<Calibration>("get_calibration")
          .then(setRegions)
          .catch(() => undefined);
        invoke<LearningInfo>("get_learning")
          .then(setLearning)
          .catch(() => undefined);
        setPreview(null);
        setGlyphs([]);
        setSelectedGlyph(null);
        setProbe(null);
      } catch (cause) {
        setError(String(cause));
      }
    },
    [refreshProfiles],
  );

  const freeze = useCallback(async () => {
    setError(null);
    setMessage(null);
    try {
      const raw = await invoke<ArrayBuffer | number[]>("capture_preview");
      const bytes =
        raw instanceof ArrayBuffer ? new Uint8Array(raw) : new Uint8Array(raw);

      // Header written by capture_preview: width u32, height u32, serial u64.
      const HEADER = 16;
      if (bytes.byteLength < HEADER) {
        throw new Error(
          `preview payload ${Object.prototype.toString.call(raw)} len=${bytes.byteLength}`,
        );
      }
      const header = new DataView(bytes.buffer, bytes.byteOffset, HEADER);
      const width = header.getUint32(0, true);
      const height = header.getUint32(4, true);
      const serial = Number(header.getBigUint64(8, true));

      const canvas = canvasRef.current;
      const context = canvas?.getContext("2d");
      if (!canvas || !context) return;

      const pixels = new Uint8ClampedArray(
        bytes.buffer,
        bytes.byteOffset + HEADER,
        width * height * 4,
      );
      canvas.width = width;
      canvas.height = height;
      context.putImageData(new ImageData(pixels, width, height), 0, 0);
      setPreview({ width, height, serial });
      setSelectedGlyph(null);
      refreshGlyphs(serial);
      // The active profile can change under us (the engine picks it by client
      // size), so re-read what is learned as well as the shapes.
      invoke<LearningInfo>("get_learning").then(setLearning).catch(() => undefined);
      invoke<LabelCrop>("map_label_crop", { serial })
        .then(setLabelCrop)
        .catch(() => setLabelCrop(null));
    } catch (cause) {
      setError(String(cause));
    }
  }, [refreshGlyphs]);

  useEffect(() => {
    invoke<Calibration>("get_calibration")
      .then(setRegions)
      .catch(() => undefined);
    invoke<LearningInfo>("get_learning")
      .then(setLearning)
      .catch(() => undefined);
    void freeze();
  }, [freeze]);

  useEffect(() => {
    refreshProfiles();

    const set = (status: TargetStatus) =>
      setClient(status.client ? { w: status.client.w, h: status.client.h } : null);
    invoke<TargetStatus>("get_target_status").then(set).catch(() => undefined);

    const unlisten = listen<TargetStatus>("target-status", (event) => set(event.payload));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refreshProfiles]);

  const save = useCallback(
    async (toSave: Calibration) => {
      if (!preview) throw new Error("freeze a frame first");
      const saved = await invoke<Calibration>("set_calibration", {
        clientW: preview.width,
        clientH: preview.height,
        exp: toSave.exp,
        level: toSave.level,
        map: toSave.map,
        name: toSave.name,
        job: toSave.job,
      });
      setRegions(saved);
      refreshProfiles();
      return saved;
    },
    [preview, refreshProfiles],
  );

  const saveRegions = useCallback(() => save(regions), [save, regions]);

  // Before a teach, persist anything drawn in this window — but never remove a box
  // the profile already has. Taking the stored boxes and overlaying only what this
  // window knows means a window that opened before a profile or mode change can no
  // longer wipe the boxes it never saw.
  const teachSave = useCallback(async () => {
    if (!preview) return;
    const stored = await invoke<Calibration>("get_calibration");
    const saved = await invoke<Calibration>("set_calibration", {
      clientW: preview.width,
      clientH: preview.height,
      exp: regions.exp ?? stored.exp,
      level: regions.level ?? stored.level,
      map: regions.map ?? stored.map,
      name: regions.name ?? stored.name,
      job: regions.job ?? stored.job,
    });
    setRegions(saved);
  }, [preview, regions]);

  // Restore the shipped preset for this size: boxes and glyph set.
  const usePresetBoxes = async () => {
    if (!preview) return;
    setError(null);
    setMessage(null);
    try {
      await invoke("apply_preset", {
        clientW: preview.width,
        clientH: preview.height,
      });
      const [cal, info] = await Promise.all([
        invoke<Calibration>("get_calibration"),
        invoke<LearningInfo>("get_learning"),
      ]);
      setRegions(cal);
      setLearning(info);
      refreshProfiles();
      refreshGlyphs(preview.serial);
      setSelectedGlyph(null);
      setMessage("Reset to the preset boxes and glyphs");
    } catch (cause) {
      setError(String(cause));
    }
  };

  // Teach one segmented shape the character the player picks for it.
  const labelGlyph = async (guess: GlyphGuess, label: string) => {
    if (!preview) return;
    setError(null);
    setMessage(null);
    try {
      const info = await invoke<LearningInfo>("label_glyph", {
        clientW: preview.width,
        clientH: preview.height,
        ch: label,
        cell: guess.cell,
        w: guess.w,
        h: guess.h,
      });
      setLearning(info);
      setSelectedGlyph(null);
      refreshGlyphs(preview.serial);
      setMessage(`Labeled a ${guess.w}×${guess.h} shape as ${label}`);
    } catch (cause) {
      setError(String(cause));
    }
  };

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

  const onPointerUp = async () => {
    if (!startRef.current) return;
    startRef.current = null;
    const drawn = drag;
    setDrag(null);
    if (!drawn || drawn.w <= 4 || drawn.h <= 4) return;

    if (mode !== "identity") {
      setRegions((current) => withRegion(current, mode, drawn));
      return;
    }

    // One box around both lines: the game stacks job over name, so split the
    // drawn box into its two text lines. If that fails, split it in half so the
    // player still gets two boxes to nudge.
    let pair: Region[] | null = null;
    if (preview) {
      try {
        pair = await invoke<Region[]>("split_identity", {
          serial: preview.serial,
          x: drawn.x,
          y: drawn.y,
          w: drawn.w,
          h: drawn.h,
        });
      } catch {
        pair = null;
      }
    }
    if (!pair || pair.length !== 2) {
      const mid = drawn.y + Math.round(drawn.h / 2);
      pair = [
        { x: drawn.x, y: drawn.y, w: drawn.w, h: mid - drawn.y },
        { x: drawn.x, y: mid, w: drawn.w, h: drawn.y + drawn.h - mid },
      ];
    }
    const [job, name] = pair;
    setRegions((current) => ({ ...current, job, name }));
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
      refreshGlyphs(preview?.serial ?? 0);
      setMessage(`Learned: ${info.learned || "—"}`);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const teach = async () => {
    setError(null);
    setMessage(null);
    try {
      // Persist a box that was just drawn, without touching the others.
      await teachSave();
      const info = await invoke<LearningInfo>("teach_map", {
        serial: preview?.serial ?? 0,
        name: mapName,
      });
      setLearning(info);
      setMessage(`Map taught: ${mapName.trim()}`);
      invoke<Calibration>("get_calibration").then(setRegions).catch(() => undefined);
      refreshProfiles();
    } catch (cause) {
      setError(String(cause));
    }
  };

  const teachIdentity = async (kind: "name" | "job") => {
    setError(null);
    setMessage(null);
    const label = kind === "name" ? nameLabel : jobLabel;
    try {
      await teachSave();
      const info = await invoke<LearningInfo>("teach_identity", {
        serial: preview?.serial ?? 0,
        kind,
        label,
      });
      setLearning(info);
      setMessage(`${kind === "name" ? "Name" : "Job"} taught: ${label.trim()}`);
      invoke<Calibration>("get_calibration").then(setRegions).catch(() => undefined);
      refreshProfiles();
    } catch (cause) {
      setError(String(cause));
    }
  };

  const clearMaps = async () => {
    if (!preview) return;
    setError(null);
    setMessage(null);
    try {
      const info = await invoke<LearningInfo>("clear_maps", {
        clientW: preview.width,
        clientH: preview.height,
      });
      setLearning(info);
      setMessage("Cleared taught maps");
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

  const clearProfile = async (profile: ProfileSummary) => {
    setError(null);
    setMessage(null);
    setBusyKey(profile.key);
    try {
      await invoke("delete_profile", {
        clientW: profile.clientW,
        clientH: profile.clientH,
        uiMode: profile.uiMode,
      });
      setConfirmKey(null);
      setMessage(`Removed profile ${profile.key}`);
      refreshProfiles();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusyKey(null);
    }
  };

  const coverage = learning ? learning.coverage : "";
  const missing = "0123456789"
    .split("")
    .filter((digit) => !coverage.includes(digit));

  const liveMode = uiModes.find((option) => option.active)?.id ?? "default";
  const liveModeLabel =
    uiModes.find((option) => option.active)?.label ?? "Default Ratio";
  const liveSize = client ? `${client.w}x${client.h}` : null;
  const sizeOptions = Array.from(
    new Set([
      ...(liveSize ? [liveSize] : []),
      ...profiles.map((profile) => `${profile.clientW}x${profile.clientH}`),
    ]),
  ).sort();
  const chosenSize = sizeChoice ?? liveSize ?? sizeOptions[0] ?? null;
  const shownProfiles = profiles.filter(
    (profile) =>
      `${profile.clientW}x${profile.clientH}` === chosenSize &&
      profile.uiMode === liveMode,
  );
  const sameSet = (profile: ProfileSummary) =>
    client !== null &&
    profile.clientW === client.w &&
    profile.clientH === client.h &&
    profile.uiMode === liveMode;

  const unknownGlyphs = glyphs.filter((guess) => !guess.known);
  const chosenGlyph =
    selectedGlyph !== null ? unknownGlyphs[selectedGlyph] : undefined;

  // The drawn identity box is stored as its two lines; show it back as one box.
  const identityBox =
    regions.name && regions.job
      ? {
          x: Math.min(regions.name.x, regions.job.x),
          y: Math.min(regions.name.y, regions.job.y),
          w:
            Math.max(regions.name.x + regions.name.w, regions.job.x + regions.job.w) -
            Math.min(regions.name.x, regions.job.x),
          h:
            Math.max(regions.name.y + regions.name.h, regions.job.y + regions.job.h) -
            Math.min(regions.name.y, regions.job.y),
        }
      : regions.name ?? regions.job;

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

      {(error || message) && (
        <div className="cal-feedback">
          {error && <p className="error small">{error}</p>}
          {message && <p className="ok small">{message}</p>}
        </div>
      )}
      {!preview && (
        <p className="warn small">
          No frame frozen yet — press <strong>Freeze frame</strong>. Every teach button
          stays disabled until there is one.
        </p>
      )}

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
        {preview && regions.map && (
          <Box region={regions.map} preview={preview} className="roi-map" label="MAP" />
        )}
        {preview && identityBox && (
          <Box
            region={identityBox}
            preview={preview}
            className="roi-identity"
            label="NAME / JOB"
          />
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
            EXP text includes the number and percentage. The level is derived from
            that, so it needs no box of its own. Name / Job is one box around both
            lines (job over name) — the app splits them. Map is optional.
          </p>
          <div className="cal-row">
            {(Object.keys(MODE_LABEL) as Mode[]).map((key) => (
              <button
                key={key}
                className="ghost"
                onClick={() =>
                  setRegions((current) =>
                    key === "identity"
                      ? { ...current, name: null, job: null }
                      : withRegion(current, key, null),
                  )
                }
              >
                clear {MODE_LABEL[key].toLowerCase()}
              </button>
            ))}
          </div>
          <div className="cal-row">
            <button
              className="ghost on"
              onClick={() => void saveRegions().catch((cause) => setError(String(cause)))}
            >
              Save boxes
            </button>
            <button
              className="ghost"
              onClick={() => void usePresetBoxes()}
              disabled={!preview}
            >
              reset to preset
            </button>
          </div>
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
          {labelCrop && (
            <div className="label-preview">
              <canvas
                className="label-crop"
                ref={(element) => paintReview(element, labelCrop)}
              />
              <span className="muted small">
                {labelCrop.lines === 1 ? "One line" : `${labelCrop.lines} lines`} — type
                exactly this, one row per line
              </span>
            </div>
          )}
          <textarea
            className="cal-input cal-textarea"
            value={mapName}
            rows={2}
            placeholder={"Map name — one line per line on screen\ne.g. Victoria Road\nKerning City"}
            onChange={(event) => setMapName(event.target.value)}
          />
          <div className="cal-row">
            <button className="ghost on" onClick={() => void teach()} disabled={!preview}>
              Teach map
            </button>
            <button
              className="ghost"
              onClick={() => void clearMaps()}
              disabled={!preview}
            >
              clear maps
            </button>
          </div>
          <p className="muted small">
            Known maps: {learning?.maps.length ? learning.maps.join(", ") : "—"}
          </p>
          <p className="muted small">
            Map letters learned: {learning?.mapLetters ? learning.mapLetters : "—"}
            <br />
            Each map you teach also teaches its letters. Once the alphabet is covered,
            map names read as text and no longer need teaching.
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
            disabled={!preview || !regions.name}
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
            disabled={!preview || !regions.job}
          >
            Teach job
          </button>
          {(!regions.name || !regions.job) && (
            <p className="muted small">
              Teaching the name or job needs a box first. Draw one around both lines
              with the <strong>name / job</strong> box button above — the job sits
              above the name, and the box is split for you.
            </p>
          )}
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

      <section className="cal-card">
        <h2>Unknown shapes</h2>
        {unknownGlyphs.length === 0 ? (
          <p className="muted small">
            {preview
              ? "Everything in the EXP box is recognised — nothing to label."
              : "Freeze a frame to see the shapes in the EXP box."}
          </p>
        ) : (
          <>
            <p className="muted small">
              These shapes in the EXP box aren't recognised yet. Click one, then pick
              the character it shows. It is remembered for this size, so each shape
              only needs doing once.
            </p>
            <div className="glyph-list">
              {unknownGlyphs.map((guess, index) => (
                <button
                  key={`${guess.x}-${guess.y}-${guess.w}x${guess.h}-${index}`}
                  className={`glyph-item ${selectedGlyph === index ? "on" : ""}`}
                  onClick={() => setSelectedGlyph(index)}
                >
                  <GlyphCrop
                    source={canvasRef.current}
                    serial={preview?.serial ?? 0}
                    guess={guess}
                  />
                  <span>{guess.ch === "?" ? "?" : guess.ch}</span>
                </button>
              ))}
            </div>
            {chosenGlyph && (
              <div className="cal-row">
                {GLYPH_LABELS.map((label) => (
                  <button
                    key={label}
                    className="ghost"
                    onClick={() => void labelGlyph(chosenGlyph, label)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            )}
          </>
        )}
      </section>

      <section className="cal-card">
        <h2>Review frames</h2>
        <p className="muted small">
          Reads the overlay rejected are kept here — each one is the EXP box exactly as
          the reader saw it. Type what the box shows and the glyphs it could not read
          are learned, so the same frame reads next time.
        </p>

        {reviewFrames.length === 0 && (
          <span className="muted small">Nothing waiting — no rejected reads saved.</span>
        )}

        {reviewFrames.map((frame) => (
          <div key={frame.id} className="review-row">
            <canvas
              className="review-crop"
              ref={(element) => paintReview(element, frame)}
            />
            <div className="review-meta">
              <span className="muted small">
                {new Date(frame.t * 1000).toLocaleTimeString()} · {frame.reason}
              </span>
              {reviewId === frame.id ? (
                <div className="cal-row">
                  <input
                    className="cal-input"
                    value={reviewText}
                    placeholder="what the box shows, e.g. 45,004 [25.83%]"
                    onChange={(event) => setReviewText(event.target.value)}
                  />
                  <button
                    className="ghost on"
                    disabled={!reviewText.trim()}
                    onClick={() => void labelReview(frame)}
                  >
                    learn
                  </button>
                  <button className="ghost" onClick={() => setReviewId(null)}>
                    cancel
                  </button>
                </div>
              ) : (
                <button
                  className="ghost"
                  onClick={() => {
                    setReviewId(frame.id);
                    setReviewText("");
                  }}
                >
                  label…
                </button>
              )}
            </div>
          </div>
        ))}

        {reviewFrames.length > 0 && (
          <button className="ghost" onClick={() => void clearReview()}>
            clear frames
          </button>
        )}
      </section>

      <section className="cal-card">
        <h2>UI profiles</h2>
        <p className="muted small">
          Calibration, digits and labels are kept per game client size <em>and</em> per
          UI Size option. The engine activates the set matching the live window; a pair
          with no profile here records nothing until you calibrate it.
        </p>
        <p className="muted small">
          {client
            ? `Live game client: ${client.w}×${client.h} · ${liveModeLabel}`
            : "Live game client: not attached"}
          {preview ? ` · frame being calibrated: ${preview.width}×${preview.height}` : ""}
        </p>

        <div className="cal-row ui-mode-choices">
          <span className="muted small">Resolution</span>
          {sizeOptions.map((size) => (
            <button
              key={size}
              className={`ghost ${size === chosenSize ? "on" : ""}`}
              onClick={() => setSizeChoice(size)}
            >
              {size.replace("x", "×")}
              {size === liveSize ? " (live)" : ""}
            </button>
          ))}
        </div>

        <div className="cal-row ui-mode-choices">
          <span className="muted small">UI size</span>
          {uiModes.map((option) => (
            <button
              key={option.id}
              className={`ghost ${option.active ? "on" : ""}`}
              onClick={() => void chooseUiMode(option)}
            >
              {option.label}
            </button>
          ))}
        </div>

        <div className="profile-list">
          {shownProfiles.length === 0 && (
            <span className="muted small">
              No profile for{" "}
              {chosenSize ? chosenSize.replace("x", "×") : "this client size"} ·{" "}
              {liveModeLabel} yet — freeze a frame and save boxes to create one.
            </span>
          )}
          {shownProfiles.map((profile) => (
            <div
              key={profile.key}
              className={`profile-row ${activeProfile?.key === profile.key ? "on" : ""}`}
            >
              <div className="profile-meta">
                <strong>{profile.key}</strong>
                <span className="muted small">
                  {uiModes.find((option) => option.id === profile.uiMode)?.label ??
                    profile.uiMode}
                  {" · "}
                  {profile.calibrated ? "calibrated" : "boxes missing"}
                  {` · ${profile.glyphs} digits · ${profile.maps} maps`}
                  {profile.name ? ` · ${profile.name}` : ""}
                  {profile.job ? ` · ${profile.job}` : ""}
                </span>
              </div>
              <div className="profile-tags">
                {sameSet(profile) && <span className="tag current">live</span>}
                {activeProfile?.key === profile.key && (
                  <span className="tag active">active</span>
                )}
              </div>
              {confirmKey === profile.key ? (
                <span className="profile-actions">
                  <button
                    className="ghost danger"
                    disabled={busyKey === profile.key}
                    onClick={() => void clearProfile(profile)}
                  >
                    {busyKey === profile.key ? "removing…" : "delete"}
                  </button>
                  <button className="ghost" onClick={() => setConfirmKey(null)}>
                    cancel
                  </button>
                </span>
              ) : (
                <button className="ghost" onClick={() => setConfirmKey(profile.key)}>
                  clear
                </button>
              )}
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
