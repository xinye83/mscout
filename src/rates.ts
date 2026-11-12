import type { Sample } from "./types";
import { expRequired, totalExp } from "./expTable";

export function total(sample: Sample): number {
  return totalExp(sample.lv, sample.exp);
}

export function rateOver(samples: Sample[], windowSec: number): number | null {
  if (samples.length < 2) return null;

  const last = samples[samples.length - 1];
  const cutoff = last.t - windowSec;

  // Require a full window of history: the baseline must be at or before the
  // cutoff. Without this, a session that just started computes a "30-minute"
  // rate from a few seconds of data and the ETA drifts wildly while idle.
  let first: Sample | null = null;
  for (let i = samples.length - 2; i >= 0; i -= 1) {
    if (samples[i].t <= cutoff) {
      first = samples[i];
      break;
    }
  }
  if (!first) return null;

  const elapsed = last.t - first.t;
  if (elapsed <= 0) return null;

  return ((total(last) - total(first)) / elapsed) * 3600;
}

export function etaSeconds(sample: Sample, ratePerHour: number | null): number | null {
  if (ratePerHour === null || ratePerHour <= 0) return null;

  const req = expRequired(sample.lv);
  if (!req) return null;

  const remaining = Math.max(0, req.value - sample.exp);
  return (remaining / ratePerHour) * 3600;
}

export function formatRate(ratePerHour: number | null): string {
  if (ratePerHour === null) return "—";
  const abs = Math.abs(ratePerHour);
  if (abs >= 1_000_000) return `${(ratePerHour / 1_000_000).toFixed(2)}M/h`;
  if (abs >= 1_000) return `${(ratePerHour / 1_000).toFixed(1)}k/h`;
  const rounded = Math.round(ratePerHour);
  if (rounded === 0) return "—";
  return `${rounded}/h`;
}

export function formatDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return "—";
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  return `${m}m`;
}
