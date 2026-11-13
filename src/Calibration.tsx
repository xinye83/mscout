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

/// The four things MScout reads. EXP is the only one it cannot work without —
/// everything the engine needs is an EXP box plus EXP glyphs (`engine.rs` gates on
/// `is_calibrated()` then `has_glyphs()`); the rest are refinements.
type Readout = "exp" | "level" | "map" | "identity";

const READOUTS: { key: Readout; title: string }[] = [
  { key: "exp", title: "EXP" },
  { key: "level", title: "Level" },
  { key: "map", title: "Street / Map" },
  { key: "identity", title: "Name / Job" },
];

const READOUT_TITLE: Record<Readout, string> = {
  exp: "EXP",
  level: "Level",
  map: "Street / Map",
  identity: "Name / Job",
};

/// The label on the box while it is being drawn.
const DRAW_LABEL: Record<Readout, string> = {
  exp: "EXP",
  level: "LV",
  map: "MAP",
  identity: "NAME / JOB",
};

/// A box the reader can actually use (matches `Region::is_usable` on the backend).
function usable(region: Region | null): region is Region {
  return region !== null && region.w > 1 && region.h > 1;
}

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
  key: "exp" | "level",
  region: Region | null,
): Calibration {
  if (key === "exp") return { ...current, exp: region };
  return { ...current, level: region };
}

function Box({
  region,
  preview,
  className,
  label,
  labelTop,
}: {
  region: Region;
  preview: Preview;
  className: string;
  label: string;
  labelTop?: number;
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
      <span style={labelTop === undefined ? undefined : { top: `${labelTop}px` }}>
        {label}
      </span>
    </div>
  );
}

const GLYPH_LABELS = "0123456789,.[]%".split("");

/// Punctuation that is hard to tell apart at button size, so the picker can name it.
/// A comma and a full stop differ by two pixels; a wrong choice here is a glyph
/// learned under the wrong character, which then misreads every frame.
const GLYPH_NAME: Record<string, string> = {
  ",": "comma",
  ".": "dot",
  "[": "open",
  "]": "close",
  "%": "percent",
};

/// A drawn box's label is 12px/600 text with 4px of horizontal padding (see
/// `.roi span`), and its height is the fixed CSS line-height. Those two numbers are
/// enough to keep labels clear of the boxes and of each other.
const LABEL_HEIGHT = 16;

type LabelBox = { key: string; region: Region; text: string };

let labelMeasurer: CanvasRenderingContext2D | null | undefined;

function labelWidth(text: string): number {
  if (labelMeasurer === undefined) {
    labelMeasurer = document.createElement("canvas").getContext("2d");
  }
  if (!labelMeasurer) {
    return text.length * 7 + 8;
  }
  labelMeasurer.font = "600 12px 'Segoe UI', system-ui, sans-serif";
  return labelMeasurer.measureText(text).width + 8;
}

/// Each label's offset from its box's top edge. A label starts in the row above its
/// box; one that would land on a box or on another label is pushed up a row until it
/// is clear, so no label covers a box and no two labels overlap.
function labelOffsets(boxes: LabelBox[], scale: number): Record<string, number> {
  const placed = boxes.map((box) => ({
    x: box.region.x * scale,
    y: box.region.y * scale,
    w: box.region.w * scale,
    h: box.region.h * scale,
  }));
  const offsets: Record<string, number> = {};
  for (const box of [...boxes].sort((a, b) => a.region.x - b.region.x)) {
    const x = box.region.x * scale;
    const boxTop = box.region.y * scale;
    const w = labelWidth(box.text);
    let top = boxTop - LABEL_HEIGHT - 1;
    for (let guard = 0; guard < 8; guard += 1) {
      const hit = placed.some(
        (p) =>
          x < p.x + p.w - 1 &&
          x + w - 1 > p.x &&
          top < p.y + p.h - 1 &&
          top + LABEL_HEIGHT - 1 > p.y,
      );
      if (!hit) break;
      top -= LABEL_HEIGHT;
    }
    placed.push({ x, y: top, w, h: LABEL_HEIGHT });
    offsets[box.key] = top - boxTop;
  }
  return offsets;
}

/// The box's own pixels, magnified, so a readout can show what its box covers.
function CropPreview({
  kind,
  serial,
  caption,
}: {
  kind: string;
  serial: number;
  caption?: (crop: LabelCrop) => string;
}) {
  const [crop, setCrop] = useState<LabelCrop | null>(null);

  useEffect(() => {
    if (!serial) {
      setCrop(null);
      return;
    }
    invoke<LabelCrop>("box_crop", { serial, kind })
      .then(setCrop)
      .catch(() => setCrop(null));
  }, [kind, serial]);

  if (!crop) return null;
  return (
    <div className="label-preview">
      <canvas className="label-crop" ref={(element) => paintReview(element, crop)} />
      {caption && <span className="muted small">{caption(crop)}</span>}
    </div>
  );
}

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

/// The shapes in one readout's box that the reader could not identify, plus the
/// character picker. Each readout owns its glyph set, so the list reads from and
/// labels back into the set selected by `kind` — a shape from the level box must
/// be judged by the level glyphs, not the EXP ones.
function ShapeList({
  kind,
  serial,
  source,
  clientW,
  clientH,
  reload,
  onLabeled,
  onError,
}: {
  kind: "exp" | "level";
  serial: number;
  source: HTMLCanvasElement | null;
  clientW: number;
  clientH: number;
  reload: number;
  onLabeled: (info: LearningInfo, note: string) => void;
  onError: (cause: unknown) => void;
}) {
  const [glyphs, setGlyphs] = useState<GlyphGuess[]>([]);
  const [selected, setSelected] = useState<number | null>(null);

  const refresh = useCallback(() => {
    if (!serial) {
      setGlyphs([]);
      return;
    }
    invoke<GlyphGuess[]>("box_glyphs", { serial, kind })
      .then(setGlyphs)
      .catch(() => setGlyphs([]));
  }, [kind, serial]);

  useEffect(() => {
    setSelected(null);
    refresh();
  }, [refresh, reload]);

  const labelShape = async (guess: GlyphGuess, character: string) => {
    try {
      const info = await invoke<LearningInfo>("label_glyph", {
        clientW,
        clientH,
        kind,
        ch: character,
        cell: guess.cell,
        w: guess.w,
        h: guess.h,
      });
      setSelected(null);
      refresh();
      onLabeled(info, `Labeled a ${guess.w}×${guess.h} shape as ${character}`);
    } catch (cause) {
      onError(cause);
    }
  };

  const where = kind === "level" ? "level box" : "EXP box";
  const unknowns = glyphs.filter((guess) => !guess.known);
  const chosen = selected !== null ? unknowns[selected] : undefined;

  if (unknowns.length === 0) {
    return null;
  }

  return (
    <div className="readout-body-shapes">
      <p className="muted small">
        These shapes in the {where} aren't recognised yet. Click one, then pick the
        character it shows.
      </p>
      <div className="glyph-list">
        {unknowns.map((guess, index) => (
          <button
            key={`${guess.x}-${guess.y}-${guess.w}x${guess.h}-${index}`}
            className={`glyph-item ${selected === index ? "on" : ""}`}
            onClick={() => setSelected(index)}
          >
            <GlyphCrop source={source} serial={serial} guess={guess} />
            <span>{guess.ch === "?" ? "?" : guess.ch}</span>
          </button>
        ))}
      </div>
      {chosen && (
        <div className="cal-row">
          {GLYPH_LABELS.map((character) => (
            <button
              key={character}
              className="ghost"
              title={GLYPH_NAME[character] ?? character}
              style={{ display: "grid", justifyItems: "center", lineHeight: 1.1 }}
              onClick={() => void labelShape(chosen, character)}
            >
              <span style={{ fontSize: "1.5em", fontFamily: "monospace" }}>
                {character}
              </span>
              {/* `,` and `.` differ by a couple of pixels, so name them. */}
              {GLYPH_NAME[character] && (
                <span style={{ fontSize: "0.7em", opacity: 0.7 }}>
                  {GLYPH_NAME[character]}
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export default function Calibration() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);
  const startRef = useRef<{ x: number; y: number } | null>(null);

  const [preview, setPreview] = useState<Preview | null>(null);
  // Rendered size of the preview element, so ROI labels can be placed clear of the
  // boxes and of each other.
  const [previewBox, setPreviewBox] = useState<{ w: number; h: number } | null>(null);
  const [regions, setRegions] = useState<Calibration>({
    clientW: null,
    clientH: null,
    exp: null,
    level: null,
    street: null,
    map: null,
    name: null,
    job: null,
  });
  // Which readout a new drag draws.
  const [active, setActive] = useState<Readout>("exp");
  // A readout that is fully set up folds away; this re-opens one on demand.
  const [expanded, setExpanded] = useState<Partial<Record<Readout, boolean>>>({});
  const [drawer, setDrawer] = useState<"profiles" | "fixes" | null>(null);
  const [drag, setDrag] = useState<Region | null>(null);
  const [learning, setLearning] = useState<LearningInfo | null>(null);
  const [typed, setTyped] = useState("");
  const [typedLevel, setTypedLevel] = useState("");
  const [mapName, setMapName] = useState("");
  const [streetName, setStreetName] = useState("");
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
  const [reviewId, setReviewId] = useState<string | null>(null);
  const [reviewText, setReviewText] = useState("");
  const [client, setClient] = useState<{ w: number; h: number } | null>(null);
  const [confirmKey, setConfirmKey] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  // Bumping this makes both shape lists re-read their glyphs after the atlas
  // changes underneath them (a preset reset, or learning digits by text).
  const [shapeReload, setShapeReload] = useState(0);

  const bumpShapes = useCallback(() => setShapeReload((n) => n + 1), []);

  // A profile with no saved map box would show nothing for the map, so seed the
  // detector's label and split it into its two lines. Nothing is saved until some
  // other action saves; a box already held (drawn or saved) is left alone.
  const seedMapBox = useCallback((serial: number) => {
    if (!serial) return;
    invoke<Region | null>("map_region", { serial })
      .then(async (region) => {
        if (!region) return;
        let street: Region | null = null;
        let map: Region = region;
        try {
          const pair = await invoke<Region[]>("split_identity", {
            serial,
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
          });
          if (pair.length === 2) {
            street = pair[0];
            map = pair[1];
          }
        } catch {
          // Keep the unsplit label as the map-name line.
        }
        setRegions((current) => {
          // Fill the street line even when a map box already exists (an older profile
          // may hold an unsplit full-label box); only take the map line if missing.
          if (current.street) return current;
          return { ...current, street, map: current.map ?? map };
        });
      })
      .catch(() => undefined);
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

  // The one save path: whatever set of boxes the window is showing goes to the profile
  // for this frame's client size. A drawn or cleared box is persisted immediately, so
  // there is no separate "Save boxes" step to forget.
  const persist = useCallback(
    async (next: Calibration) => {
      if (!preview) throw new Error("freeze a frame first");
      const saved = await invoke<Calibration>("set_calibration", {
        clientW: preview.width,
        clientH: preview.height,
        exp: next.exp,
        level: next.level,
        street: next.street,
        map: next.map,
        name: next.name,
        job: next.job,
      });
      setRegions(saved);
      refreshProfiles();
      return saved;
    },
    [preview, refreshProfiles],
  );

  // Apply a box change and save it, rolling back if the backend refuses (e.g. clearing
  // the very last box, which is never what was meant).
  const applyRegions = async (next: Calibration, note: string) => {
    const before = regions;
    setRegions(next);
    try {
      await persist(next);
      setError(null);
      setMessage(note);
    } catch (cause) {
      setRegions(before);
      setMessage(null);
      setError(String(cause));
    }
  };

  // Switching UI Size swaps the whole set in use, so a just-drawn box is saved first.
  const chooseUiMode = useCallback(
    async (option: UiModeOption) => {
      if (option.active) return;
      setError(null);
      setMessage(null);
      try {
        if (preview) await persist(regions);
        await invoke("set_ui_mode", { mode: option.id });
        const [cal, info] = await Promise.all([
          invoke<Calibration>("get_calibration"),
          invoke<LearningInfo>("get_learning"),
        ]);
        setRegions(cal);
        setLearning(info);
        setPreview(null);
        setProbe(null);
        setMessage(`UI size set to ${option.label}`);
        refreshProfiles();
      } catch (cause) {
        setError(String(cause));
      }
    },
    [persist, preview, refreshProfiles, regions],
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
      bumpShapes();
      seedMapBox(serial);
      // The active profile can change under us (the engine picks it by client
      // size), so re-read what is learned as well as the shapes.
      invoke<LearningInfo>("get_learning").then(setLearning).catch(() => undefined);
    } catch (cause) {
      setError(String(cause));
    }
  }, [bumpShapes, seedMapBox]);

  useEffect(() => {
    invoke<Calibration>("get_calibration")
      .then(setRegions)
      .catch(() => undefined);
    invoke<LearningInfo>("get_learning")
      .then(setLearning)
      .catch(() => undefined);
    void freeze();
  }, [freeze]);

  // Track the preview element's rendered size so label placement can use real pixels.
  useEffect(() => {
    const element = previewRef.current;
    if (!element) return;
    const measure = () =>
      setPreviewBox({ w: element.clientWidth, h: element.clientHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

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
    if (!startRef.current || !preview) return;
    startRef.current = null;
    const drawn = drag;
    setDrag(null);
    if (!drawn || drawn.w <= 4 || drawn.h <= 4) return;

    if (active !== "identity" && active !== "map") {
      await applyRegions(
        withRegion(regions, active, drawn),
        `Saved the ${READOUT_TITLE[active]} box`,
      );
      return;
    }

    // One box around both stacked lines (job over name, or street over map name);
    // split it into its two lines. If that fails, split it in half so the player
    // still gets two boxes to nudge.
    let pair: Region[] | null = null;
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
    if (!pair || pair.length !== 2) {
      const mid = drawn.y + Math.round(drawn.h / 2);
      pair = [
        { x: drawn.x, y: drawn.y, w: drawn.w, h: mid - drawn.y },
        { x: drawn.x, y: mid, w: drawn.w, h: drawn.y + drawn.h - mid },
      ];
    }

    if (active === "map") {
      const [street, map] = pair;
      await applyRegions({ ...regions, street, map }, "Saved the map box");
    } else {
      const [job, name] = pair;
      await applyRegions({ ...regions, job, name }, "Saved the name / job box");
    }
  };

  const clearReadout = async (readout: Readout) => {
    const next =
      readout === "identity"
        ? { ...regions, name: null, job: null }
        : readout === "map"
          ? { ...regions, street: null, map: null }
          : withRegion(regions, readout, null);
    await applyRegions(next, `Cleared the ${READOUT_TITLE[readout]} box`);
  };

  // Every learn/teach/test works on the saved boxes, so persist first.
  const learn = async () => {
    setError(null);
    setMessage(null);
    try {
      await persist(regions);
      const info = await invoke<LearningInfo>("learn_atlas", {
        serial: preview?.serial ?? 0,
        text: typed,
      });
      setLearning(info);
      bumpShapes();
      setMessage(`Learned: ${info.learned || "—"}`);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const learnLevel = async () => {
    setError(null);
    setMessage(null);
    try {
      await persist(regions);
      const info = await invoke<LearningInfo>("learn_level_atlas", {
        serial: preview?.serial ?? 0,
        text: typedLevel,
      });
      setLearning(info);
      bumpShapes();
      setMessage(`Learned from the level box: ${info.learned || "—"}`);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const teach = async () => {
    setError(null);
    setMessage(null);
    const name = mapName.trim();
    try {
      await persist(regions);
      const info = await invoke<LearningInfo>("teach_map", {
        serial: preview?.serial ?? 0,
        name,
      });
      setLearning(info);
      setMessage(`Map taught: ${name}`);
      refreshProfiles();
    } catch (cause) {
      setError(String(cause));
    }
  };

  const teachStreet = async () => {
    setError(null);
    setMessage(null);
    const street = streetName.trim();
    try {
      await persist(regions);
      const info = await invoke<LearningInfo>("teach_street", {
        serial: preview?.serial ?? 0,
        street,
      });
      setLearning(info);
      setMessage(`Street taught: ${street}`);
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
      await persist(regions);
      const info = await invoke<LearningInfo>("teach_identity", {
        serial: preview?.serial ?? 0,
        kind,
        label,
      });
      setLearning(info);
      setMessage(`${kind === "name" ? "Name" : "Job"} taught: ${label.trim()}`);
      refreshProfiles();
    } catch (cause) {
      setError(String(cause));
    }
  };

  const test = async () => {
    setError(null);
    setMessage(null);
    setProbe(null);
    try {
      await persist(regions);
      const result = await invoke<Readings>("parse_probe", {
        serial: preview?.serial ?? 0,
      });
      setProbe(result);
    } catch (cause) {
      setError(String(cause));
    }
  };

  // Restore the shipped preset for this size: boxes and glyph set.
  const resetPreset = async () => {
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
      bumpShapes();
      seedMapBox(preview.serial);
      setMessage("Reset to the preset boxes and glyphs");
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

  const coverage = learning?.coverage ?? "";
  const levelCoverage = learning?.levelCoverage ?? "";
  const missing = "0123456789"
    .split("")
    .filter((digit) => !coverage.includes(digit));

  // The map box is stored as its two lines (street over map name); the reader
  // fingerprints the whole label, so the preview shows one box around both.
  const mapBox =
    regions.street && regions.map
      ? {
          x: Math.min(regions.street.x, regions.map.x),
          y: Math.min(regions.street.y, regions.map.y),
          w:
            Math.max(
              regions.street.x + regions.street.w,
              regions.map.x + regions.map.w,
            ) - Math.min(regions.street.x, regions.map.x),
          h:
            Math.max(
              regions.street.y + regions.street.h,
              regions.map.y + regions.map.h,
            ) - Math.min(regions.street.y, regions.map.y),
        }
      : regions.street ?? regions.map;

  const expUsable = usable(regions.exp);
  const levelUsable = usable(regions.level);
  const mapUsable = usable(mapBox);
  const streetUsable = usable(regions.street);
  const nameUsable = usable(regions.name);
  const jobUsable = usable(regions.job);
  const identityUsable = nameUsable || jobUsable;

  const boxFor: Record<Readout, boolean> = {
    exp: expUsable,
    level: levelUsable,
    map: mapUsable,
    identity: identityUsable,
  };

  const mark = (ok: boolean) => (ok ? "box ✓" : "box ✗");
  const statusFor: Record<Readout, string> = {
    exp: `${mark(expUsable)} · ${coverage.length}/10 digits`,
    level: `${mark(levelUsable)} · ${levelCoverage.length}/10 digits`,
    map: `${mark(mapUsable)}${
      learning?.maps.length ? ` · ${learning.maps.length} taught` : ""
    }`,
    identity: `name ${nameUsable ? "✓" : "✗"} · job ${jobUsable ? "✓" : "✗"}`,
  };

  // A readout folds once it is set up: the EXP/level boxes hold every digit, a map is
  // taught, or both name and job are taught. Clicking the header re-opens a folded one.
  const done: Record<Readout, boolean> = {
    exp: expUsable && missing.length === 0,
    level: levelUsable && levelCoverage.length === 10,
    map: mapUsable && (learning?.maps.length ?? 0) > 0,
    identity: nameUsable && jobUsable && !!learning?.name && !!learning?.job,
  };
  const showBody = (key: Readout) => expanded[key] ?? !done[key];

  const ready = expUsable && missing.length === 0;
  const statusText = !expUsable
    ? "EXP box needed — MScout cannot read without it"
    : missing.length > 0
      ? `Reading · EXP digits missing ${missing.join(" ")}`
      : ready
        ? "Ready to read"
        : "Reading";
  const statusTone = ready ? "ok" : "warn";

  const liveMode = uiModes.find((option) => option.active)?.id ?? "default";
  const liveModeLabel = uiModes.find((option) => option.active)?.label ?? "—";
  const liveSize = client ? `${client.w}x${client.h}` : null;
  const sizeOptions = Array.from(
    new Set([
      ...(liveSize ? [liveSize] : []),
      ...profiles.map((profile) => `${profile.clientW}x${profile.clientH}`),
    ]),
  ).sort();
  const chosenSize = sizeChoice ?? liveSize ?? sizeOptions[0] ?? null;
  const shownProfiles = profiles.filter(
    (profile) => `${profile.clientW}x${profile.clientH}` === chosenSize,
  );
  const sameSet = (profile: ProfileSummary) =>
    client !== null &&
    profile.clientW === client.w &&
    profile.clientH === client.h &&
    profile.uiMode === liveMode;

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

  // A label starts above its box and is pushed up a row when it would otherwise cover
  // a box or another label. Needs the preview's pixel size to compare against.
  const labelBoxes: LabelBox[] = [
    regions.exp && { key: "exp", region: regions.exp, text: "EXP" },
    regions.level && { key: "level", region: regions.level, text: "LV" },
    mapBox && { key: "map", region: mapBox, text: "MAP" },
    identityBox && { key: "identity", region: identityBox, text: "NAME / JOB" },
  ].filter((box): box is LabelBox => box !== null);
  const labelTops: Record<string, number> =
    preview && previewBox && preview.width > 0
      ? labelOffsets(labelBoxes, previewBox.w / preview.width)
      : {};

  const serial = preview?.serial ?? 0;

  return (
    <div className="page calibration">
      <header className="page-head">
        <div>
          <h1>Calibration</h1>
          <p className="muted small">
            {client ? `${client.w}×${client.h}` : "no game window"} · {liveModeLabel}
            {" · "}
            <span className={statusTone}>{statusText}</span>
          </p>
        </div>
        <div className="presets">
          <button className="ghost" onClick={() => void freeze()}>
            Freeze frame
          </button>
          <button
            className={`ghost ${drawer === "profiles" ? "on" : ""}`}
            onClick={() => setDrawer(drawer === "profiles" ? null : "profiles")}
          >
            Profiles…
          </button>
        </div>
      </header>

      {(error || message) && (
        <div className="cal-feedback">
          {error && <p className="error small">{error}</p>}
          {message && <p className="ok small">{message}</p>}
        </div>
      )}

      <div className="cal-main">
        <div className="cal-preview-col">
          <div
            className="preview"
            ref={previewRef}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
          >
            <canvas ref={canvasRef} />
            {preview && regions.exp && (
              <Box
                region={regions.exp}
                preview={preview}
                className="roi-exp"
                label="EXP"
                labelTop={labelTops.exp}
              />
            )}
            {preview && regions.level && (
              <Box
                region={regions.level}
                preview={preview}
                className="roi-level"
                label="LV"
                labelTop={labelTops.level}
              />
            )}
            {preview && mapBox && (
              <Box
                region={mapBox}
                preview={preview}
                className="roi-map"
                label="MAP"
                labelTop={labelTops.map}
              />
            )}
            {preview && identityBox && (
              <Box
                region={identityBox}
                preview={preview}
                className="roi-identity"
                label="NAME / JOB"
                labelTop={labelTops.identity}
              />
            )}
            {preview && drag && (
              <Box
                region={drag}
                preview={preview}
                className="roi-drag"
                label={DRAW_LABEL[active]}
              />
            )}
            {!preview && <div className="preview-empty">Freeze a frame to start</div>}
          </div>
          <p className="muted small">
            {preview
              ? `Drag on the image to draw the ${READOUT_TITLE[active]} box.`
              : "Press Freeze frame to capture the game, then draw the boxes."}
          </p>
        </div>

        <div className="cal-readouts">
          {READOUTS.map((readout) => {
            const key = readout.key;
            return (
              <section key={key} className={`readout ${active === key ? "on" : ""}`}>
                <header
                  className="readout-head"
                  onClick={() =>
                    setExpanded((current) => {
                      const shown = current[key] ?? !done[key];
                      return { ...current, [key]: !shown };
                    })
                  }
                >
                  <span className="readout-caret">{showBody(key) ? "▾" : "▸"}</span>
                  <span className="readout-name">{readout.title}</span>
                  <span className="readout-status">{statusFor[key]}</span>
                </header>

                {showBody(key) && (
                  <div className="readout-actions">
                    <button
                      className={`ghost ${active === key ? "on" : ""}`}
                      onClick={() => setActive(key)}
                    >
                      Draw box
                    </button>
                    <button
                      className="ghost"
                      disabled={!boxFor[key]}
                      onClick={() => void clearReadout(key)}
                    >
                      Clear box
                    </button>
                  </div>
                )}

                {showBody(key) && (
                  <div className="readout-body">
                    {key === "exp" && (
                      <>
                        <CropPreview
                          kind="exp"
                          serial={serial}
                          caption={() => "The EXP box, as the reader sees it"}
                        />
                        <p className="muted small">
                          Type exactly what the box shows, punctuation included.
                        </p>
                        <input
                          className="cal-input"
                          value={typed}
                          placeholder="10,105 [91.53%]"
                          onChange={(event) => setTyped(event.target.value)}
                        />
                        <button
                          className="ghost on"
                          disabled={!preview}
                          onClick={() => void learn()}
                        >
                          Learn digits
                        </button>
                        <ShapeList
                          kind="exp"
                          serial={serial}
                          source={canvasRef.current}
                          clientW={preview?.width ?? 0}
                          clientH={preview?.height ?? 0}
                          reload={shapeReload}
                          onLabeled={(info, note) => {
                            setError(null);
                            setLearning(info);
                            setMessage(note);
                          }}
                          onError={(cause) => {
                            setMessage(null);
                            setError(String(cause));
                          }}
                        />
                      </>
                    )}

                    {key === "level" && (
                      <>
                        <CropPreview
                          kind="level"
                          serial={serial}
                          caption={() => "The level box, as the reader sees it"}
                        />
                        <p className="muted small">
                          Type just the digits — no "LV." — once the level box is drawn.
                        </p>
                        <input
                          className="cal-input"
                          value={typedLevel}
                          placeholder="35"
                          onChange={(event) => setTypedLevel(event.target.value)}
                        />
                        <button
                          className="ghost on"
                          disabled={!preview}
                          onClick={() => void learnLevel()}
                        >
                          Learn level digits
                        </button>
                        <ShapeList
                          kind="level"
                          serial={serial}
                          source={canvasRef.current}
                          clientW={preview?.width ?? 0}
                          clientH={preview?.height ?? 0}
                          reload={shapeReload}
                          onLabeled={(info, note) => {
                            setError(null);
                            setLearning(info);
                            setMessage(note);
                          }}
                          onError={(cause) => {
                            setMessage(null);
                            setError(String(cause));
                          }}
                        />
                      </>
                    )}

                    {key === "map" && (
                      <>
                        <CropPreview
                          kind="street"
                          serial={serial}
                          caption={() => "The street line, as the reader sees it"}
                        />
                        <input
                          className="cal-input"
                          value={streetName}
                          placeholder="Street name"
                          onChange={(event) => setStreetName(event.target.value)}
                        />
                        <button
                          className="ghost on"
                          disabled={!preview || !streetUsable}
                          onClick={() => void teachStreet()}
                        >
                          Teach street
                        </button>
                        <CropPreview
                          kind="map"
                          serial={serial}
                          caption={() => "The map name line, as the reader sees it"}
                        />
                        <input
                          className="cal-input"
                          value={mapName}
                          placeholder="Map name"
                          onChange={(event) => setMapName(event.target.value)}
                        />
                        <button
                          className="ghost on"
                          disabled={!preview}
                          onClick={() => void teach()}
                        >
                          Teach map
                        </button>
                        <p className="muted small">
                          Known maps:{" "}
                          {learning?.maps.length
                            ? learning.maps
                                .map((name) => name.replace(/\n/g, " · "))
                                .join(", ")
                            : "—"}
                        </p>
                        <p className="muted small">
                          Map letters: {learning?.mapLetters || "—"} — each map you teach
                          also teaches its letters.
                        </p>
                      </>
                    )}

                    {key === "identity" && (
                      <>
                        <p className="muted small">
                          Draw one box around both lines (job over name) — the app
                          splits them.
                        </p>
                        <CropPreview
                          kind="name"
                          serial={serial}
                          caption={() => "The name line, as the reader sees it"}
                        />
                        <input
                          className="cal-input"
                          value={nameLabel}
                          placeholder="Character name"
                          onChange={(event) => setNameLabel(event.target.value)}
                        />
                        <button
                          className="ghost on"
                          disabled={!preview || !nameUsable}
                          onClick={() => void teachIdentity("name")}
                        >
                          Teach name
                        </button>
                        <CropPreview
                          kind="job"
                          serial={serial}
                          caption={() => "The job line, as the reader sees it"}
                        />
                        <input
                          className="cal-input"
                          value={jobLabel}
                          placeholder="Job (e.g. Page)"
                          onChange={(event) => setJobLabel(event.target.value)}
                        />
                        <button
                          className="ghost on"
                          disabled={!preview || !jobUsable}
                          onClick={() => void teachIdentity("job")}
                        >
                          Teach job
                        </button>
                        <p className="muted small">
                          Name: {learning?.name ?? "—"} · Job: {learning?.job ?? "—"}
                        </p>
                      </>
                    )}
                  </div>
                )}
              </section>
            );
          })}
        </div>
      </div>

      {drawer === "fixes" && (
        <section className="cal-drawer">
          <div className="drawer-head">
            <strong>Fixes waiting</strong>
            <span className="muted small">
              Reads the overlay rejected — label one and its digits are learned.
            </span>
            {reviewFrames.length > 0 && (
              <button className="ghost" onClick={() => void clearReview()}>
                Clear all
              </button>
            )}
          </div>
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
        </section>
      )}

      {drawer === "profiles" && (
        <section className="cal-drawer">
          <div className="drawer-head">
            <strong>Profiles</strong>
            <span className="muted small">
              Boxes, digits and labels are kept per game client size and UI size. The
              engine activates the pair matching the live window.
            </span>
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

          <div className="cal-row ui-mode-choices">
            <span className="muted small">Show size</span>
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

          <div className="profile-list">
            {shownProfiles.length === 0 && (
              <span className="muted small">
                No profile for{" "}
                {chosenSize ? chosenSize.replace("x", "×") : "this client size"} yet —
                freeze a frame and draw a box to create one.
              </span>
            )}
            {shownProfiles.map((profile) => (
              <div
                key={profile.key}
                className={`profile-row ${
                  activeProfile?.key === profile.key ? "on" : ""
                }`}
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
                  <button
                    className="ghost"
                    onClick={() => setConfirmKey(profile.key)}
                  >
                    clear
                  </button>
                )}
              </div>
            ))}
          </div>

          <div className="cal-row">
            <button
              className="ghost"
              onClick={() => void resetPreset()}
              disabled={!preview}
            >
              Reset to preset
            </button>
          </div>
        </section>
      )}

      <footer className="cal-foot">
        <button
          className={`ghost ${drawer === "fixes" ? "on" : ""}`}
          onClick={() => setDrawer(drawer === "fixes" ? null : "fixes")}
        >
          Fixes waiting: {reviewFrames.length}
        </button>
        <button className="ghost" onClick={() => void test()} disabled={!preview}>
          Test read
        </button>
        {probe && (
          <span className="muted small probe">
            Lv {probe.lv} · {probe.exp.toLocaleString()} EXP · {probe.pct.toFixed(2)}% ·{" "}
            {probe.map} · {probe.name ?? "—"} · {probe.job ?? "—"} · conf{" "}
            {probe.conf.toFixed(2)}
          </span>
        )}
      </footer>
    </div>
  );
}
