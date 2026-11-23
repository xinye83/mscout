/**
 * One typed function per Rust command.
 *
 * Every IPC call goes through here, so a command's name and payload are written
 * once, next to its return type, instead of as string literals scattered over
 * the components. Argument objects are the exact IPC payloads (camelCase, as
 * Tauri maps them to the Rust parameters).
 */
import { invoke } from "@tauri-apps/api/core";
import type {
  Calibration,
  GlyphGuess,
  HistoryMinute,
  LabelCrop,
  LearningInfo,
  MscwStatus,
  Probe,
  Profile,
  ProfileSummary,
  Region,
  RepairSummary,
  Sample,
  TargetStatus,
  UiModeOption,
  WindowInfo,
} from "./types";

type Serial = { serial: number };
type ClientSize = { clientW: number; clientH: number };

/** A box the calibration window can preview (Rust `BoxKind`). */
export type BoxKind = "exp" | "level" | "name" | "job" | "street" | "map";
/** A digit readout with its own glyph atlas (Rust `DigitBox`). */
export type DigitBox = "exp" | "level";
/** A line taught as a fingerprint (Rust `IdentityBox`). */
export type IdentityBox = "name" | "job";
/** The game's UI Size option id (Rust `UiMode`). */
export type UiModeId = "default" | "filtered" | "ideal";

// History and overlay
export const readSamples = (args: { from: number; to: number; bucket?: number }) =>
  invoke<Sample[]>("read_samples", args);
export const repairHistory = () => invoke<RepairSummary>("repair_history");
export const readMinutes = (args: { from: number; to: number }) =>
  invoke<HistoryMinute[]>("read_minutes", args);
export const saveExport = (args: { name: string; text: string }) =>
  invoke<string>("save_export", args);
export const getProfile = () => invoke<Profile>("get_profile");
export const setMode = (args: { expanded: boolean }) => invoke<void>("set_mode", args);
export const resizeOverlay = (args: { height: number }) => invoke<void>("resize_overlay", args);

// Windows
export const openCalibration = () => invoke<void>("open_calibration");
export const openTargetWindow = () => invoke<void>("open_target_window");

// Game window target
export const listWindows = () => invoke<WindowInfo[]>("list_windows");
export const getTargetStatus = () => invoke<TargetStatus>("get_target_status");
export const setTarget = (args: { title: string; process: string }) =>
  invoke<TargetStatus>("set_target", args);
export const clearTarget = () => invoke<TargetStatus>("clear_target");

// Profiles and UI Size
export const listProfiles = () => invoke<ProfileSummary[]>("list_profiles");
export const activeProfile = () => invoke<ProfileSummary | null>("active_profile");
export const deleteProfile = (args: ClientSize & { uiMode?: string }) =>
  invoke<void>("delete_profile", args);
export const uiModes = () => invoke<UiModeOption[]>("ui_modes");
export const setUiMode = (args: { mode: UiModeId }) => invoke<void>("set_ui_mode", args);

// Game assets
export const getMscwStatus = () => invoke<MscwStatus>("get_mscw_status");
export const pickMscwDir = () => invoke<string | null>("pick_mscw_dir");
export const setMscwDir = (args: { path: string }) => invoke<MscwStatus>("set_mscw_dir", args);

// Calibration
export const capturePreview = () => invoke<ArrayBuffer | number[]>("capture_preview");
export const getCalibration = () => invoke<Calibration>("get_calibration");
export const setCalibration = (
  args: ClientSize & Pick<Calibration, "exp" | "level" | "street" | "map" | "name" | "job">,
) => invoke<Calibration>("set_calibration", args);
export const presetCalibration = (args: ClientSize) =>
  invoke<Calibration | null>("preset_calibration", args);
export const applyPreset = (args: ClientSize) => invoke<void>("apply_preset", args);
export const getLearning = () => invoke<LearningInfo>("get_learning");
export const learnAtlas = (args: Serial & { text: string }) =>
  invoke<LearningInfo>("learn_atlas", args);
export const learnLevelAtlas = (args: Serial & { text: string }) =>
  invoke<LearningInfo>("learn_level_atlas", args);
export const seedAtlas = (args: Serial) => invoke<LearningInfo>("seed_atlas", args);
export const teachMap = (args: Serial & { name: string }) =>
  invoke<LearningInfo>("teach_map", args);
export const teachStreet = (args: Serial & { street: string }) =>
  invoke<LearningInfo>("teach_street", args);
export const teachIdentity = (args: Serial & { kind: IdentityBox; label: string }) =>
  invoke<LearningInfo>("teach_identity", args);
export const parseProbe = (args: Serial) => invoke<Probe>("parse_probe", args);
export const saveDiagnostic = (args: Serial) => invoke<string>("save_diagnostic", args);
export const splitIdentity = (args: Serial & Region) => invoke<Region[]>("split_identity", args);
export const boxGlyphs = (args: Serial & { kind: DigitBox }) =>
  invoke<GlyphGuess[]>("box_glyphs", args);
export const labelGlyph = (
  args: ClientSize & { kind: DigitBox; ch: string; bitmap: string; w: number; h: number },
) => invoke<LearningInfo>("label_glyph", args);
export const boxCrop = (args: Serial & { kind: BoxKind }) => invoke<LabelCrop>("box_crop", args);
export const mapLineRegions = (args: Serial) => invoke<Region[]>("map_line_regions", args);
export const mapRegion = (args: Serial) => invoke<Region | null>("map_region", args);
