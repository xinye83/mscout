export type Sample = {
  t: number;
  lv: number;
  exp: number;
  pct: number;
  map: string;
  state: "gaining" | "idle";
  conf: number;
  /** True when the minimap is collapsed and no map name is on screen. */
  mapHidden?: boolean;
};

export type Rect = { x: number; y: number; w: number; h: number };

export type Region = Rect;

export type Calibration = {
  clientW: number | null;
  clientH: number | null;
  exp: Region | null;
  level: Region | null;
  street: Region | null;
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
  /** True when the minimap is collapsed; there is no name to read or calibrate. */
  mapHidden: boolean;
};

export type LearningInfo = {
  learned: string;
  coverage: string;
  levelLearned: string;
  levelCoverage: string;
  mapLetters: string;
  maps: string[];
  name: string | null;
  job: string | null;
  /** "game", "taught", or empty for a preset / legacy set. */
  source: string;
  /** Most common stored glyph size, e.g. "5x7". */
  glyphSize: string;
  /** Where the level set came from: "game", "taught", or empty. */
  levelSource: string;
  /** Most common level glyph size, e.g. "7x7". */
  levelSize: string;
  /** Characters in the name/job letter atlas. */
  identityLetters: string;
  /** "font" when seeded from Arial, "taught", or empty. */
  identitySource: string;
  /** Most common identity glyph size, e.g. "5x7" (seeded from Arial 12 px). */
  identitySize: string;
};

/** The player's MapleStory MSCW folder and whether its glyph atlas is usable. */
export type MscwStatus = {
  path: string | null;
  exists: boolean;
  atlasFound: boolean;
  /** Map/street names found in the client's String.wz. */
  mapNames: number;
  error: string | null;
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
  /** Native glyph bitmap, one hex byte per pixel (00/ff). */
  bitmap: string;
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
  source: string;
  glyphSize: string;
};

/** One option of the game's Graphics → UI Size setting. */
export type UiModeOption = {
  id: string;
  label: string;
  active: boolean;
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
