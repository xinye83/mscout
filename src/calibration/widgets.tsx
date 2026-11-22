// The calibration window's small components: a drawn box, a box's crop, one
// shape, and the list of shapes the reader could not identify.

import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../api";
import type { GlyphGuess, LabelCrop, LearningInfo, Region } from "../types";
import {
  GLYPH_LABELS,
  GLYPH_NAME,
  paintReview,
} from "./model";
import type { Preview } from "./model";

export function Box({
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

/// The box's own pixels, magnified, so a readout can show what its box covers.
export function CropPreview({
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
export function GlyphCrop({
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
export function ShapeList({
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
