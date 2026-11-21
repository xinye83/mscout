import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import * as api from "./api";
import { listen } from "@tauri-apps/api/event";
import type {
  Calibration,
  FieldProvenance,
  GlyphGuess,
  LabelCrop,
  LearningInfo,
  MscwStatus,
  Probe,
  ProfileSummary,
  ReadProvenance,
  Readings,
  Region,
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

/// Plain-English names for the reader stages the backend reports. An id this build
/// does not know is shown as it arrived, so a newer backend still displays.
const SOURCE_LABEL: Record<string, string> = {
  atlas: "digit atlas",
  font: "Arial text",
  fingerprint: "taught fingerprint",
  ocr: "OCR text",
  table: "stored table",
  dtw: "DTW match",
  literal: "OCR text, no table match",
  glyphs: "minimap letter atlas",
  hidden: "minimap collapsed",
  none: "not read",
  unknown: "not recognised",
};

/// "digit atlas · game": the stage that read the field, then where its data came
/// from (`game`, `taught`, a font seed).
function sourceText(entry: FieldProvenance): string {
  const stage = SOURCE_LABEL[entry.source] ?? entry.source;
  return entry.engine ? `${stage} · ${entry.engine}` : stage;
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
  kind: "exp" | "level" | "name" | "job" | "street" | "map";
  serial: number;
  caption?: (crop: LabelCrop) => string;
}) {
  const [crop, setCrop] = useState<LabelCrop | null>(null);

  useEffect(() => {
    if (!serial) {
      setCrop(null);
      return;
    }
    api.boxCrop({ serial, kind })
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
    api.boxGlyphs({ serial, kind })
      .then(setGlyphs)
      .catch(() => setGlyphs([]));
  }, [kind, serial]);

  useEffect(() => {
    setSelected(null);
    refresh();
  }, [refresh, reload]);

  const labelShape = async (guess: GlyphGuess, character: string) => {
    try {
      const info = await api.labelGlyph({
        clientW,
        clientH,
        kind,
        ch: character,
        bitmap: guess.bitmap,
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
  // Shipped boxes for this size. A profile box equal to the preset is not a
  // calibration the player made, so the preview does not draw it as one.
  const [presetCal, setPresetCal] = useState<Calibration | null>(null);
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
  // Manual correction controls are hidden when the automatic source is ready.
  const [manual, setManual] = useState<Partial<Record<Readout, boolean>>>({});
  const [drawer, setDrawer] = useState<"profiles" | null>(null);
  const [drag, setDrag] = useState<Region | null>(null);
  const [learning, setLearning] = useState<LearningInfo | null>(null);
  const [mscw, setMscw] = useState<MscwStatus | null>(null);
  const [typed, setTyped] = useState("");
  const [typedLevel, setTypedLevel] = useState("");
  const [mapName, setMapName] = useState("");
  const [streetName, setStreetName] = useState("");
  const [nameLabel, setNameLabel] = useState("");
  const [jobLabel, setJobLabel] = useState("");
  const [probe, setProbe] = useState<Readings | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The backend's own rejection ("the EXP box is not set") and the provenance it
  // gathered before giving up, so a rejected frame still explains itself.
  const [probeRejected, setProbeRejected] = useState<string | null>(null);
  const [probeProvenance, setProbeProvenance] = useState<ReadProvenance | null>(
    null,
  );
  const [probeError, setProbeError] = useState<string | null>(null);
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [activeProfile, setActiveProfile] = useState<ProfileSummary | null>(null);
  const [uiModes, setUiModes] = useState<UiModeOption[]>([]);
  const [sizeChoice, setSizeChoice] = useState<string | null>(null);
  const [client, setClient] = useState<{ w: number; h: number } | null>(null);
  const [confirmKey, setConfirmKey] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  // Bumping this makes both shape lists re-read their glyphs after the atlas
  // changes underneath them (a preset reset, or learning digits by text).
  const [shapeReload, setShapeReload] = useState(0);

  const bumpShapes = useCallback(() => setShapeReload((n) => n + 1), []);

  // Apply one probe result to the readout panels. A rejected frame keeps its
  // provenance and reason, so the panels can show what was tried and why it stopped
  // even though there are no readings to display.
  const applyProbe = useCallback((result: Probe | null, commandError?: unknown) => {
    setProbe(result?.readings ?? null);
    setProbeProvenance(result?.provenance ?? null);
    setProbeRejected(result?.rejected ?? null);
    setProbeError(commandError ? String(commandError) : null);
  }, []);

  // Read the frozen frame for the per-panel "Read:" lines. Best effort: a panel
  // with no box or a rejected read simply shows "—" plus the backend's reason.
  const probeNow = useCallback(
    async (serial: number) => {
      if (!serial) return;
      try {
        applyProbe(await api.parseProbe({ serial }));
      } catch (cause) {
        applyProbe(null, cause);
      }
    },
    [applyProbe],
  );

  // The map is read by detection, so seed the preview from the detected line
  // regions rather than legacy saved boxes. Nothing is persisted here; a drawn box
  // is saved by the usual action.
  const seedMapBox = useCallback((serial: number) => {
    if (!serial) return;
    // The reader detects both lines; mirror those regions so the preview overlays
    // and the cards show the pixels the overlay actually reads. Saved boxes in an
    // older profile can hold the whole label and are only the fallback.
    api.mapLineRegions({ serial })
      .then((lines) => {
        if (lines.length > 0) {
          setRegions((current) => ({
            ...current,
            street: lines.length > 1 ? lines[0] : null,
            map: lines[lines.length - 1],
          }));
          return;
        }

        return api.mapRegion({ serial }).then(
          async (region) => {
            if (!region) return;
            let street: Region | null = null;
            let map: Region = region;
            try {
              const pair = await api.splitIdentity({
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
              if (current.street) return current;
              return { ...current, street, map: current.map ?? map };
            });
          },
        );
      })
      .catch(() => undefined);
  }, []);

  const refreshProfiles = useCallback(() => {
    api.listProfiles()
      .then(setProfiles)
      .catch(() => undefined);
    api.activeProfile()
      .then(setActiveProfile)
      .catch(() => undefined);
    api.uiModes()
      .then(setUiModes)
      .catch(() => undefined);
    api.getMscwStatus()
      .then(setMscw)
      .catch(() => undefined);
  }, []);

  // The one save path: whatever set of boxes the window is showing goes to the profile
  // for this frame's client size. A drawn or cleared box is persisted immediately, so
  // there is no separate "Save boxes" step to forget.
  const persist = useCallback(
    async (next: Calibration) => {
      if (!preview) throw new Error("freeze a frame first");
      const saved = await api.setCalibration({
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
      // Drawing the EXP box is the moment the seed can run. Do it here so the
      // first read works without a separate Load digits press. A complete taught
      // set is not overwritten; only an empty or incomplete one is seeded.
      if (
        preview &&
        next.exp &&
        mscw?.atlasFound &&
        learning !== null &&
        (learning.source === "" || learning.coverage.length < 10)
      ) {
        try {
          const info = await api.seedAtlas({
            serial: preview.serial,
          });
          setLearning(info);
          bumpShapes();
          setMessage(`${note} · digits loaded (${info.glyphSize})`);
        } catch {
          // The Load digits button reports the reason.
        }
      }
      if (preview) void probeNow(preview.serial);
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
        await api.setUiMode({ mode: option.id });
        const [cal, info] = await Promise.all([
          api.getCalibration(),
          api.getLearning(),
        ]);
        setRegions(cal);
        setLearning(info);
        setPreview(null);
        applyProbe(null);
        setMessage(`UI size set to ${option.label}`);
        refreshProfiles();
      } catch (cause) {
        setError(String(cause));
      }
    },
    [applyProbe, persist, preview, refreshProfiles, regions],
  );

  const saveDiagnostic = useCallback(async () => {
    if (!preview) return;
    setError(null);
    setMessage(null);
    try {
      const path = await api.saveDiagnostic({
        serial: preview.serial,
      });
      setMessage(`Diagnostic saved: ${path}`);
    } catch (cause) {
      setError(String(cause));
    }
  }, [preview, setError, setMessage]);

  const freeze = useCallback(async () => {
    setError(null);
    setMessage(null);
    applyProbe(null);
    try {
      const raw = await api.capturePreview();
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
      // The active profile can change under us (the engine picks it by client
      // size), so re-read what is learned as well as the shapes.
      const info = await api.getLearning().catch(() => null);
      if (info) setLearning(info);
      const status = await api.getMscwStatus().catch(() => null);
      if (status) setMscw(status);
      // Read the boxes back: the freeze callback was built before this render's
      // drags, so `regions` here can be a frame behind. The saved set cannot.
      const calibration = await api.getCalibration().catch(
        () => null,
      );
      if (calibration) setRegions(calibration);
      // Run after the saved set is installed: detection is the source of truth for
      // the two map lines, so it must not be overwritten by legacy saved boxes.
      seedMapBox(serial);
      const levelBox = usable(calibration?.level ?? null);
      // Seed when the set is absent or incomplete. A complete taught set is left
      // alone: reseeding replaces corrections the player made by hand.
      const needsSeed =
        status?.atlasFound &&
        (!info ||
          info.coverage.length < 10 ||
          info.source === "" ||
          (levelBox && info.levelSource !== "game" && info.levelCoverage.length < 10));
      if (needsSeed) {
        try {
          const seeded = await api.seedAtlas({ serial });
          setLearning(seeded);
          bumpShapes();
          setMessage(`Digits loaded from the game (${seeded.glyphSize})`);
        } catch {
          // The EXP box may not be saved yet; the Load digits button retries.
        }
      }
      await probeNow(serial);
    } catch (cause) {
      setError(String(cause));
    }
  }, [applyProbe, bumpShapes, probeNow, seedMapBox]);

  const seedNow = async () => {
    if (!preview) return;
    if (!usable(regions.exp)) {
      setError("draw the EXP box first, then load the digits");
      return;
    }
    setError(null);
    setMessage(null);
    try {
      await persist(regions);
      const info = await api.seedAtlas({
        serial: preview.serial,
      });
      setLearning(info);
      bumpShapes();
      setMessage(`Digits loaded from the game (${info.glyphSize})`);
      await probeNow(preview.serial);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const locateGame = async () => {
    setError(null);
    setMessage(null);
    try {
      const path = await api.pickMscwDir();
      if (!path) return;
      const status = await api.setMscwDir({ path });
      setMscw(status);
      if (status.atlasFound) {
        setMessage(`Game assets found: ${status.path}`);
        if (preview) await seedNow();
      } else {
        setError(status.error ?? "Game assets not found in that folder");
      }
    } catch (cause) {
      setError(String(cause));
    }
  };

  useEffect(() => {
    api.getCalibration()
      .then(setRegions)
      .catch(() => undefined);
    api.getLearning()
      .then(setLearning)
      .catch(() => undefined);
    void freeze();
  }, [freeze]);

  useEffect(() => {
    if (!preview) {
      setPresetCal(null);
      return;
    }
    api.presetCalibration({
      clientW: preview.width,
      clientH: preview.height,
    })
      .then(setPresetCal)
      .catch(() => setPresetCal(null));
  }, [preview]);

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
    api.getTargetStatus().then(set).catch(() => undefined);

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
    if (!preview || active === "map") return;
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

    if (active === "map") {
      // Map lines are detected, not drawn.
      return;
    }

    if (active !== "identity") {
      await applyRegions(
        withRegion(regions, active, drawn),
        `Saved the ${READOUT_TITLE[active]} box`,
      );
      return;
    }

    // One box around both stacked lines (job over name); split it into its two
    // lines. If that fails, split it in half so the player still gets two boxes
    // to nudge.
    let pair: Region[] | null = null;
    try {
      pair = await api.splitIdentity({
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

    const [job, name] = pair;
    await applyRegions({ ...regions, job, name }, "Saved the name / job box");
  };

  const clearReadout = async (readout: Readout) => {
    if (readout === "map") return;
    const next =
      readout === "identity"
        ? { ...regions, name: null, job: null }
        : withRegion(regions, readout, null);
    await applyRegions(next, `Cleared the ${READOUT_TITLE[readout]} box`);
  };

  // Every learn/teach/test works on the saved boxes, so persist first.
  const learn = async () => {
    setError(null);
    setMessage(null);
    try {
      await persist(regions);
      const info = await api.learnAtlas({
        serial: preview?.serial ?? 0,
        text: typed,
      });
      setLearning(info);
      bumpShapes();
      setMessage(`Learned: ${info.learned || "—"}`);
      void probeNow(preview?.serial ?? 0);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const learnLevel = async () => {
    setError(null);
    setMessage(null);
    try {
      await persist(regions);
      const info = await api.learnLevelAtlas({
        serial: preview?.serial ?? 0,
        text: typedLevel,
      });
      setLearning(info);
      bumpShapes();
      setMessage(`Learned from the level box: ${info.learned || "—"}`);
      void probeNow(preview?.serial ?? 0);
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
      const info = await api.teachMap({
        serial: preview?.serial ?? 0,
        name,
      });
      setLearning(info);
      setMessage(`Map taught: ${name}`);
      refreshProfiles();
      void probeNow(preview?.serial ?? 0);
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
      const info = await api.teachStreet({
        serial: preview?.serial ?? 0,
        street,
      });
      setLearning(info);
      setMessage(`Street taught: ${street}`);
      refreshProfiles();
      void probeNow(preview?.serial ?? 0);
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
      const info = await api.teachIdentity({
        serial: preview?.serial ?? 0,
        kind,
        label,
      });
      setLearning(info);
      setMessage(`${kind === "name" ? "Name" : "Job"} taught: ${label.trim()}`);
      refreshProfiles();
      void probeNow(preview?.serial ?? 0);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const test = async () => {
    setError(null);
    setMessage(null);
    applyProbe(null);
    try {
      await persist(regions);
      const result = await api.parseProbe({
        serial: preview?.serial ?? 0,
      });
      applyProbe(result);
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
      await api.applyPreset({
        clientW: preview.width,
        clientH: preview.height,
      });
      const [cal, info] = await Promise.all([
        api.getCalibration(),
        api.getLearning(),
      ]);
      setRegions(cal);
      setLearning(info);
      refreshProfiles();
      bumpShapes();
      seedMapBox(preview.serial);
      // The reset cleared both digit atlases; seed them now rather than making
      // the player wait for the next engine tick or press Load digits.
      if (mscw?.atlasFound && usable(cal.exp)) {
        try {
          const seeded = await api.seedAtlas({
            serial: preview.serial,
          });
          setLearning(seeded);
          bumpShapes();
          setMessage(`Reset to the preset boxes · digits reloaded (${seeded.glyphSize})`);
          void probeNow(preview.serial);
          return;
        } catch {
          // Fall through to the deferred message.
        }
      }
      setMessage("Reset to the preset boxes — digits will reload from the game");
      void probeNow(preview.serial);
    } catch (cause) {
      setError(String(cause));
    }
  };

  const clearProfile = async (profile: ProfileSummary) => {
    setError(null);
    setMessage(null);
    setBusyKey(profile.key);
    try {
      await api.deleteProfile({
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

  const expUsable = usable(regions.exp);
  const levelUsable = usable(regions.level);
  const streetUsable = usable(regions.street);
  const nameUsable = usable(regions.name);
  const jobUsable = usable(regions.job);
  const identityUsable = nameUsable || jobUsable;

  // A preset only supplies starting geometry; it is not something the player
  // calibrated. Draw a box only when the box differs from the shipped preset.
  const sameRegion = (a: Region | null, b: Region | null) =>
    !!a &&
    !!b &&
    a.x === b.x &&
    a.y === b.y &&
    a.w === b.w &&
    a.h === b.h;
  const isPresetBox = (key: "exp" | "level" | "name" | "job") =>
    sameRegion(regions[key], presetCal?.[key] ?? null);
  const showBox = (key: "exp" | "level" | "name" | "job") =>
    usable(regions[key]) && !isPresetBox(key);
  const showIdentityBox = showBox("name") || showBox("job");
  const canClear = (key: Readout) => {
    if (key === "map") return false;
    if (key === "identity") {
      return identityUsable && (!isPresetBox("name") || !isPresetBox("job"));
    }
    return showBox(key);
  };

  const mapNames = mscw?.mapNames ?? 0;
  const taughtMaps = learning?.maps.length ?? 0;
  const mapAuto = mapNames > 0;
  const identityAuto =
    learning?.identitySource === "font" && !!learning.identityLetters;
  const identityTaught = !!learning?.name && !!learning?.job;

  const sourceLabel =
    learning?.source === "game"
      ? "auto"
      : learning?.source === "taught"
        ? "taught"
        : learning?.learned
          ? "learned"
          : "";
  const levelSourceLabel =
    learning?.levelSource === "game"
      ? "auto"
      : learning?.levelSource === "taught"
        ? "taught"
        : learning?.levelLearned
          ? "learned"
          : "";
  const identitySourceLabel = identityAuto
    ? "auto · Arial"
    : learning?.identitySource === "taught"
      ? "taught"
      : learning?.identityLetters
        ? "learned"
        : "";
  const mapStatus = mapAuto
    ? `auto · ${mapNames} names${taughtMaps ? ` · ${taughtMaps} taught` : ""}`
    : taughtMaps
      ? `taught · ${taughtMaps} maps`
      : "no map table";
  const mapNote = mapAuto
    ? `Street and map are detected and matched from the client's map table (${mapNames} names). Use Correct manually only to fix a missing or misread map.`
    : taughtMaps
      ? "Using the taught map fingerprints; no client map table is loaded. Use Correct manually to add maps."
      : "No client map table found — locate the MSCW folder, or use Correct manually to teach street/map here.";
  const identityNote =
    !nameUsable || !jobUsable
      ? identityAuto || identityTaught
        ? "Name/job data is ready; draw one box around both lines (job over name) to read them."
        : "Draw one box around both lines (job over name), then freeze; or use Correct manually to teach them here."
      : identityAuto
        ? `Name and job are read automatically from Arial (${learning?.identitySize}). Use Correct manually only to fix a misread.`
        : identityTaught
          ? "Name and job are using the taught fingerprints. Use Correct manually only to fix a misread."
          : "No automatic name/job set yet — use Correct manually to teach them here.";

  const identityBoxes = (nameUsable ? 1 : 0) + (jobUsable ? 1 : 0);
  const digitStatus = (hasBox: boolean, covered: number, source: string) =>
    !hasBox
      ? "no box"
      : `${source ? `${source} · ` : ""}${covered}/10 digits`;
  const statusFor: Record<Readout, string> = {
    exp: digitStatus(expUsable, coverage.length, sourceLabel),
    level: digitStatus(levelUsable, levelCoverage.length, levelSourceLabel),
    map: mapStatus,
    identity:
      identityBoxes === 0
        ? "no box"
        : identityBoxes === 1
          ? "1 of 2 boxes"
          : identitySourceLabel || "no glyphs",
  };

  // The automatic source is the default; manual teaching is a correction.
  const autoReady: Record<Readout, boolean> = {
    exp: coverage.length === 10,
    level: levelCoverage.length === 10,
    map: mapAuto || taughtMaps > 0,
    identity: identityAuto || identityTaught,
  };
  const showManual = (key: Readout) => manual[key] ?? !autoReady[key];
  const toggleManual = (key: Readout) =>
    setManual((current) => ({ ...current, [key]: !showManual(key) }));

  // A readout folds once it is ready from either the automatic source or taught
  // data. The map no longer needs a drawn box: detection + the client table is
  // enough; identity folds once Arial is seeded and both boxes exist.
  const done: Record<Readout, boolean> = {
    exp: expUsable && missing.length === 0,
    level: levelUsable && levelCoverage.length === 10,
    map: mapAuto || taughtMaps > 0,
    identity: nameUsable && jobUsable && (identityAuto || identityTaught),
  };
  const showBody = (key: Readout) => expanded[key] ?? !done[key];

  const ready = expUsable && missing.length === 0;
  const seeded = learning?.source === "game";
  const allAuto =
    seeded &&
    ready &&
    levelUsable &&
    levelSourceLabel === "auto" &&
    identityAuto &&
    nameUsable &&
    jobUsable &&
    mapAuto;
  const expNote =
    coverage.length === 10
      ? sourceLabel === "auto"
        ? "EXP digits are loaded from the client atlas. Correct manually only if the value is wrong."
        : "EXP digits are learned for this box. Correct manually only if the value is wrong."
      : "No complete automatic set yet. Type exactly what the box shows, punctuation included.";
  const levelNote =
    levelCoverage.length === 10
      ? levelSourceLabel === "auto"
        ? "Level digits are loaded from the client atlas. Correct manually only if the value is wrong."
        : "Level digits are learned for this box. Correct manually only if the value is wrong."
      : "No complete automatic level set yet. Draw the level box, then type just the digits — no \"LV.\"";
  const statusText = !expUsable
    ? "EXP box needed — MScout cannot read without it"
    : !ready && !seeded && mscw?.atlasFound
      ? "Asset found — freeze a frame or press Load digits to seed the atlas"
      : !ready && !seeded
        ? "No digits yet — locate the MSCW folder or learn the digits"
        : missing.length > 0
          ? `Reading · EXP digits missing ${missing.join(" ")}`
          : ready
            ? allAuto
              ? "Ready to read · digits, name/job and map/street auto"
              : "Ready to read"
            : "Reading";
  const statusTone = ready ? "ok" : "warn";

  const readValue = (key: Readout): string | null => {
    switch (key) {
      case "exp":
        return probe?.expText || null;
      case "level":
        return probe && probe.lv > 0 ? `Lv ${probe.lv}` : null;
      case "map":
        if (probe?.mapHidden) return "minimap collapsed";
        return probe?.map && probe.map !== "unknown"
          ? probe.map.replace(/\n/g, " / ")
          : null;
      case "identity":
        return probe?.name || probe?.job
          ? `${probe?.name ?? "—"} / ${probe?.job ?? "—"}`
          : null;
      default:
        return null;
    }
  };

  /// Where this field's value came from. The probe carries the provenance beside
  /// the readings so a rejected frame has one too; absent from an older backend,
  /// which is why the readout falls back to the bare value.
  const provenanceFor = (key: Readout): FieldProvenance | null => {
    const provenance = probe?.provenance ?? probeProvenance;
    if (!provenance) return null;
    switch (key) {
      case "exp":
        return provenance.exp;
      case "level":
        return provenance.level;
      case "map":
        return provenance.map;
      case "identity":
        return provenance.identity;
      default:
        return null;
    }
  };

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
    regions.exp && showBox("exp")
      ? { key: "exp", region: regions.exp, text: "EXP" }
      : null,
    regions.level && showBox("level")
      ? { key: "level", region: regions.level, text: "LV" }
      : null,
    identityBox && showIdentityBox
      ? { key: "identity", region: identityBox, text: "NAME / JOB" }
      : null,
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
            className="ghost"
            disabled={!preview}
            title="Save a local JSON diagnostic with raw reads, glyph candidates, map crops and where each field came from"
            onClick={() => void saveDiagnostic()}
          >
            Save diagnostic
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

      <section className="cal-assets">
        <div className="cal-row">
          <span className="muted small">Game assets</span>
          {mscw?.atlasFound ? (
            <span className="ok small">
              digits available · map table {mscw.mapNames} names · {mscw.path}
            </span>
          ) : mscw?.exists ? (
            <span className="warn small">folder found, no HUD atlas</span>
          ) : (
            <span className="warn small">not set — locate your MSCW folder</span>
          )}
          <button
            className="ghost"
            style={{ marginLeft: "auto" }}
            title="Choose the MapleStory MSCW folder"
            onClick={() => void locateGame()}
          >
            Locate MSCW folder…
          </button>
          <button
            className="ghost"
            disabled={!preview || !mscw?.atlasFound || !expUsable}
            onClick={() => void seedNow()}
          >
            Load digits
          </button>
        </div>
        {mscw?.error && <p className="error small">{mscw.error}</p>}
      </section>

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
            {preview && showBox("exp") && regions.exp && (
              <Box
                region={regions.exp}
                preview={preview}
                className="roi-exp"
                label="EXP"
                labelTop={labelTops.exp}
              />
            )}
            {preview && showBox("level") && regions.level && (
              <Box
                region={regions.level}
                preview={preview}
                className="roi-level"
                label="LV"
                labelTop={labelTops.level}
              />
            )}
            {preview && showIdentityBox && identityBox && (
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
              ? active === "map"
                ? "Street and map are detected automatically — no box needed."
                : `Drag on the image to draw the ${READOUT_TITLE[active]} box.`
              : "Press Freeze frame to capture the game, then draw the boxes."}
          </p>
        </div>

        <div className="cal-readouts">
          {READOUTS.map((readout) => {
            const key = readout.key;
            const provenance = provenanceFor(key);
            return (
              <section
                key={key}
                className={`readout ${showBody(key) ? "open" : "folded"}`}
              >
                <header
                  className="readout-head"
                  onClick={() => {
                    setActive(key);
                    setExpanded((current) => {
                      const shown = current[key] ?? !done[key];
                      return { ...current, [key]: !shown };
                    });
                  }}
                >
                  <span className="readout-caret">{showBody(key) ? "▾" : "▸"}</span>
                  <span className="readout-name">{readout.title}</span>
                  <span className="readout-status">{statusFor[key]}</span>
                </header>

                {showBody(key) && (
                  <div className="readout-actions">
                    {key !== "map" && (
                      <button
                        className={`ghost ${active === key ? "on" : ""}`}
                        onClick={() => setActive(key)}
                      >
                        Draw box
                      </button>
                    )}
                    {key !== "map" && canClear(key) && (
                      <button
                        className="ghost"
                        onClick={() => void clearReadout(key)}
                      >
                        Clear box
                      </button>
                    )}
                    <button
                      className="ghost"
                      onClick={() => toggleManual(key)}
                    >
                      {showManual(key) ? "Hide corrections" : "Correct manually"}
                    </button>
                  </div>
                )}

                {showBody(key) && (
                  <div className="readout-body">
                    <p className="readout-read muted small">
                      Read: {readValue(key) ?? "—"}
                    </p>
                    {provenance && (
                      <p className="readout-source muted small">
                        Source: {sourceText(provenance)}
                      </p>
                    )}
                    {provenance?.reason && (
                      <p className="readout-reason warn small">
                        Why: {provenance.reason}
                      </p>
                    )}
                    {(probeRejected || probeError) && (
                      <p className="warn small">
                        Rejected: {probeRejected ?? probeError}
                      </p>
                    )}
                    {key === "exp" && (
                      <>
                        <p className="muted small">{expNote}</p>
                        <CropPreview
                          kind="exp"
                          serial={serial}
                          caption={() => "The EXP box, as the reader sees it"}
                        />
                        {showManual("exp") && (
                          <>
                            <p className="muted small">Manual correction:</p>
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
                          </>
                        )}
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
                        <p className="muted small">{levelNote}</p>
                        <CropPreview
                          kind="level"
                          serial={serial}
                          caption={() => "The level box, as the reader sees it"}
                        />
                        {showManual("level") && (
                          <>
                            <p className="muted small">Manual correction:</p>
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
                          </>
                        )}
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
                        <p className="muted small">{mapNote}</p>
                        <CropPreview
                          kind="street"
                          serial={serial}
                          caption={() => "The street line, as the reader sees it"}
                        />
                        <CropPreview
                          kind="map"
                          serial={serial}
                          caption={() => "The map name line, as the reader sees it"}
                        />
                        {showManual("map") && (
                          <>
                            <p className="muted small">Manual correction:</p>
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
                              Taught map fingerprints:{" "}
                              {learning?.maps.length
                                ? learning.maps
                                    .map((name) => name.replace(/\n/g, " · "))
                                    .join(", ")
                                : "—"}
                            </p>
                          </>
                        )}
                      </>
                    )}

                    {key === "identity" && (
                      <>
                        <p className="muted small">{identityNote}</p>
                        <CropPreview
                          kind="name"
                          serial={serial}
                          caption={() => "The name line, as the reader sees it"}
                        />
                        <CropPreview
                          kind="job"
                          serial={serial}
                          caption={() => "The job line, as the reader sees it"}
                        />
                        {showManual("identity") && (
                          <>
                            <p className="muted small">Manual correction:</p>
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
                              Taught override: {learning?.name ?? "—"} /{" "}
                              {learning?.job ?? "—"}
                            </p>
                          </>
                        )}
                      </>
                    )}
                  </div>
                )}
              </section>
            );
          })}
        </div>
      </div>

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
                freeze a frame to create one.
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
                    {profile.calibrated ? "can read" : "needs EXP box"}
                    {` · ${profile.glyphs} digits · ${profile.maps} maps`}
                    {profile.source
                      ? ` · ${profile.source} ${profile.glyphSize}`
                      : ""}
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
        <button className="ghost" onClick={() => void test()} disabled={!preview}>
          Test read
        </button>
        {probe && (
          <span className="muted small probe">
            read ok · conf {probe.conf.toFixed(2)}
            {probe.provenance
              ? ` · ${(probe.provenance.durationMs ?? 0).toFixed(1)} ms`
              : ""}
          </span>
        )}
      </footer>
    </div>
  );
}
