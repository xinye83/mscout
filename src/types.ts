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

export type CaptureState = "reading" | "degraded" | "lost";

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
  maps: string[];
  name: string | null;
  job: string | null;
};

export type Profile = {
  name: string | null;
  job: string | null;
};

export type Settings = {
  rateWindowSec?: number;
  averageWindowSec?: number;
  etaWindowSec?: number;
  idleBandSec?: number;
  expRegion?: { x: number; y: number; w: number; h: number } | null;
  mapRegion?: { x: number; y: number; w: number; h: number } | null;
};
