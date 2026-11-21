import table from "../src-tauri/exp-table.json" with { type: "json" };

/**
 * EXP needed to fill the bar at each level of Classic World, 1 to 99 (index
 * `level - 1`). The rows live in `src-tauri/exp-table.json`, the one copy the
 * engine, this UI and the mock generator all read; see there for the source.
 * Levels 50-89 are planning numbers, not yet confirmed in play, and are
 * labelled estimated.
 */
const REQUIRED: readonly number[] = table.required;
const CONFIRMED_THROUGH = table.confirmedThrough;
const ESTIMATED_THROUGH = table.estimatedThrough;

export const LEVEL_CAP = 100;

export type ExpRequirement = { value: number; estimated: boolean } | null;

export function expRequired(level: number): ExpRequirement {
  if (!Number.isInteger(level) || level < 1 || level >= LEVEL_CAP) return null;
  const value = REQUIRED[level - 1];
  if (value === undefined) return null;
  return { value, estimated: level > CONFIRMED_THROUGH && level <= ESTIMATED_THROUGH };
}

export function cumulativeTo(level: number): number {
  let total = 0;
  for (let l = 1; l < level; l += 1) {
    const req = expRequired(l);
    if (!req) break;
    total += req.value;
  }
  return total;
}

export function totalExp(level: number, expInLevel: number): number {
  return cumulativeTo(level) + expInLevel;
}
