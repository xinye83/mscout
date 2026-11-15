import type { CaptureStatus } from "./types";

/**
 * The overlay's status dot. One place decides the colour and the reason, in
 * priority order, so a new state (e.g. a collapsed minimap) cannot silently
 * fall through to green.
 */
export type DotTone =
  | "off"
  | "lost"
  | "uncalibrated"
  | "degraded"
  | "hidden"
  | "nomap"
  | "reading";

export type DotState = {
  tone: DotTone;
  /** Tooltip text: why the dot has this colour. */
  title: string;
};

export function dotState(
  capture: CaptureStatus | null,
  unrecognised: string[],
  mapHidden: boolean,
): DotState {
  if (!capture) {
    return { tone: "off", title: "not attached" };
  }
  if (capture.state === "lost") {
    return { tone: "lost", title: capture.reason ?? "not reading" };
  }
  if (capture.state === "uncalibrated") {
    return {
      tone: "uncalibrated",
      title: capture.reason ?? "not calibrated for this size",
    };
  }
  if (capture.state === "degraded") {
    return { tone: "degraded", title: capture.reason ?? "degraded" };
  }
  if (mapHidden) {
    return { tone: "hidden", title: "minimap collapsed — no map name on screen" };
  }
  if (unrecognised.length > 0) {
    return {
      tone: "nomap",
      title: `${unrecognised.join(", ")} not recognised`,
    };
  }
  return { tone: "reading", title: "reading" };
}
