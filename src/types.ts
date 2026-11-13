export type Sample = {
  t: number;
  lv: number;
  exp: number;
  pct: number;
  map: string;
  state: "gaining" | "idle";
  conf: number;
};

export type Rect = { x: number; y: number; w: number; h: number };

export type Region = Rect;

export type Calibration = {
  clientW: number | null;
  clientH: number | null;
  exp: Region | null;
  level: Region | null;
  map: Region | null;
  name: Region | null;
  job: Region | null;
};

export type WindowSelector = {
  title: string;
  process: string;
};

export type WindowInfo = {
  hwnd: number;
  title: string;
  process: string;
  pid: number;
  minimized: boolean;
  client: Rect;
  screen: Rect;
};

export type TargetStatus = {
  selector: WindowSelector | null;
  configured: boolean;
  found: boolean;
  title: string | null;
  process: string | null;
  pid: number | null;
  hwnd: number | null;
  minimized: boolean;
  client: Rect | null;
  screen: Rect | null;
};

export type CaptureState = "reading" | "degraded" | "lost" | "uncalibrated";

export type CaptureStatus = {
  state: CaptureState;
  reason: string | null;
};

export type Readings = {
  lv: number;
  exp: number;
  pct: number;
  expectedPct: number;
  map: string;
  conf: number;
  expText: string;
  levelText: string;
  name: string | null;
  job: string | null;
};

export type LearningInfo = {
  learned: string;
  coverage: string;
  mapLetters: string;
  maps: string[];
  name: string | null;
  job: string | null;
};

export type Profile = {
  name: string | null;
  job: string | null;
};

export type GlyphGuess = {
  x: number;
  y: number;
  w: number;
  h: number;
  ch: string;
  distance: number;
  known: boolean;
  cell: string;
};

export type ProfileSummary = {
  key: string;
  clientW: number;
  clientH: number;
  uiMode: string;
  calibrated: boolean;
  glyphs: number;
  maps: number;
  name: string | null;
  job: string | null;
};

/** One option of the game's Graphics → UI Size setting. */
export type UiModeOption = {
  id: string;
  label: string;
  active: boolean;
};

/** An EXP box the reader rejected, kept so it can be labelled by hand. */
export type ReviewFrame = {
  id: string;
  t: number;
  reason: string;
  w: number;
  h: number;
  /** One hex byte per pixel, plain luminance. */
  px: string;
};

export type RepairSummary = {
  checked: number;
  flagged: number;
  files: number;
};

/** The minimap label as the app detects it, at full resolution. */
export type LabelCrop = {
  w: number;
  h: number;
  /** One hex byte per pixel, plain luminance. */
  px: string;
  lines: number;
};

export type Settings = {
  rateWindowSec?: number;
  averageWindowSec?: number;
  etaWindowSec?: number;
  idleBandSec?: number;
  expRegion?: { x: number; y: number; w: number; h: number } | null;
  mapRegion?: { x: number; y: number; w: number; h: number } | null;
};
