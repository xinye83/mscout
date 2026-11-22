// Types, constants and pure helpers of the calibration window.

import type { Calibration, FieldProvenance, Region } from "../types";

/// The four things MScout reads. EXP is the only one it cannot work without —
/// everything the engine needs is an EXP box plus EXP glyphs (`engine.rs` gates on
/// `is_calibrated()` then `has_glyphs()`); the rest are refinements.
export type Readout = "exp" | "level" | "map" | "identity";

export const READOUTS: { key: Readout; title: string }[] = [
  { key: "exp", title: "EXP" },
  { key: "level", title: "Level" },
  { key: "map", title: "Street / Map" },
  { key: "identity", title: "Name / Job" },
];

export const READOUT_TITLE: Record<Readout, string> = {
  exp: "EXP",
  level: "Level",
  map: "Street / Map",
  identity: "Name / Job",
};

/// The label on the box while it is being drawn.
export const DRAW_LABEL: Record<Readout, string> = {
  exp: "EXP",
  level: "LV",
  map: "MAP",
  identity: "NAME / JOB",
};

/// A box the reader can actually use (matches `Region::is_usable` on the backend).
export function usable(region: Region | null): region is Region {
  return region !== null && region.w > 1 && region.h > 1;
}

/// Plain-English names for the reader stages the backend reports. An id this build
/// does not know is shown as it arrived, so a newer backend still displays.
export const SOURCE_LABEL: Record<string, string> = {
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
export function sourceText(entry: FieldProvenance): string {
  const stage = SOURCE_LABEL[entry.source] ?? entry.source;
  return entry.engine ? `${stage} · ${entry.engine}` : stage;
}

export function hexBytes(hex: string): Uint8Array {
  const out = new Uint8Array(Math.floor(hex.length / 2));
  for (let index = 0; index < out.length; index += 1) {
    out[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  return out;
}

// Paint a grey crop at 1:1; the browser magnifies it with pixelated CSS.
export function paintReview(
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

export type Preview = {
  width: number;
  height: number;
  serial: number;
};

export function normalize(a: { x: number; y: number }, b: { x: number; y: number }): Region {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  };
}

export function clamp(value: number, min: number, max: number) {
  return Math.max(min, Math.min(max, value));
}

export function withRegion(
  current: Calibration,
  key: "exp" | "level",
  region: Region | null,
): Calibration {
  if (key === "exp") return { ...current, exp: region };
  return { ...current, level: region };
}

export const GLYPH_LABELS = "0123456789,.[]%".split("");

/// Punctuation that is hard to tell apart at button size, so the picker can name it.
/// A comma and a full stop differ by two pixels; a wrong choice here is a glyph
/// learned under the wrong character, which then misreads every frame.
export const GLYPH_NAME: Record<string, string> = {
  ",": "comma",
  ".": "dot",
  "[": "open",
  "]": "close",
  "%": "percent",
};

/// A drawn box's label is 12px/600 text with 4px of horizontal padding (see
/// `.roi span`), and its height is the fixed CSS line-height. Those two numbers are
/// enough to keep labels clear of the boxes and of each other.
export const LABEL_HEIGHT = 16;

export type LabelBox = { key: string; region: Region; text: string };

let labelMeasurer: CanvasRenderingContext2D | null | undefined;

export function labelWidth(text: string): number {
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
export function labelOffsets(boxes: LabelBox[], scale: number): Record<string, number> {
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
