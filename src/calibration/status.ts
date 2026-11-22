// What the calibration window says about each readout: whether its box is
// usable, where its glyphs came from, whether it is done, and the notes and
// status line. Pure, so it can be tested without the window.

import type {
  Calibration,
  FieldProvenance,
  LearningInfo,
  MscwStatus,
  ReadProvenance,
  Readings,
  Region,
} from "../types";
import { usable } from "./model.ts";
import type { Readout } from "./model";

export type StatusInput = {
  learning: LearningInfo | null;
  mscw: MscwStatus | null;
  regions: Calibration;
  presetCal: Calibration | null;
  /** Readouts the player opened or closed by hand. */
  manual: Partial<Record<Readout, boolean>>;
  expanded: Partial<Record<Readout, boolean>>;
  probe: Readings | null;
  probeProvenance: ReadProvenance | null;
};

export function readoutStatus({
  learning,
  mscw,
  regions,
  presetCal,
  manual,
  expanded,
  probe,
  probeProvenance,
}: StatusInput) {
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

  return {
    expUsable,
    streetUsable,
    nameUsable,
    jobUsable,
    showBox,
    showIdentityBox,
    canClear,
    mapNote,
    identityNote,
    statusFor,
    showManual,
    done,
    showBody,
    expNote,
    levelNote,
    statusText,
    statusTone,
    readValue,
    provenanceFor,
  };
}
