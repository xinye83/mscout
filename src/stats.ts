// Active time, sessions and the estimates built on them (SPEC §7).
//
// Everything here works on slices: one per recorded row for the overlay, or
// one per character-minute from the history index for long ranges. Both carry
// the same fields, so the overlay and the History window compute sessions,
// per-map rates and bests the same way.

import { expRequired } from "./expTable.ts";
import { total } from "./rates.ts";
import type { Sample } from "./types";

/** A gap between rows longer than this is "not recording" (DATA.md). */
export const RECORDING_GAP = 5;
/**
 * Two gains at most this far apart are one stretch of play: the time between
 * them is active. A longer wait is a break, and the gain that ends it restarts
 * the clock instead of counting (as the beanguri overlay does).
 */
export const IDLE_PAUSE = 30;
/** No gain for this long ends a session. */
export const SESSION_BREAK = 600;

export type Slice = {
  /** End of the slice, Unix seconds. */
  t: number;
  /** Wall-clock seconds the slice covers while recording. */
  wall: number;
  /** Active seconds: time between gains no more than IDLE_PAUSE apart. */
  active: number;
  /** EXP gained in the slice. */
  gained: number;
  /** EXP gained during active time: the numerator of an active rate. */
  activeGained: number;
  /** EXP lost at recorded deaths. */
  lost: number;
  deaths: number;
  levelUps: number;
  /** Whether the slice ends with a gain. */
  gain: boolean;
  /** Time of the last gain at or before the slice's end, if any. */
  lastGain: number | null;
  lv: number;
  /** Level-local EXP at the slice's end, for the ETA. */
  exp: number;
  map: string;
  name?: string;
};

/**
 * One slice per pair of consecutive rows of one character's run. Rows must be
 * ascending and from one character ([`currentCharacterRun`]).
 */
export function slicesFromSamples(samples: Sample[]): Slice[] {
  const out: Slice[] = [];
  let lastGain: number | null = null;
  for (let i = 1; i < samples.length; i += 1) {
    const a = samples[i - 1];
    const b = samples[i];
    const dt = b.t - a.t;
    const recording = dt > 0 && dt <= RECORDING_GAP;
    const delta = total(b) - total(a);
    const death = b.event === "death";
    const gained = !death && delta > 0 ? delta : 0;
    const lost = death ? Math.max(0, -delta) : 0;
    let active = 0;
    let activeGained = 0;
    if (gained > 0) {
      // The whole wait since the previous gain is active when it is short;
      // `settle` spreads it back over the slices it spans.
      if (lastGain !== null && b.t - lastGain <= IDLE_PAUSE && recording) {
        active = b.t - lastGain;
        activeGained = gained;
      }
      lastGain = b.t;
    }
    if (!recording) lastGain = gained > 0 ? b.t : null;
    out.push({
      t: b.t,
      wall: recording ? dt : 0,
      active,
      gained,
      activeGained,
      lost,
      deaths: death ? 1 : 0,
      levelUps: b.lv > a.lv ? b.lv - a.lv : 0,
      gain: gained > 0,
      lastGain,
      lv: b.lv,
      exp: b.exp,
      map: b.map,
      name: b.name,
    });
  }
  return settle(out);
}

/**
 * Move the active time between two gains onto the slices it spans. A gain
 * slice first carries the whole wait since the previous gain; spreading it
 * keeps windowed rates from counting time outside their window.
 */
function settle(slices: Slice[]): Slice[] {
  for (let i = 0; i < slices.length; i += 1) {
    const slice = slices[i];
    if (!slice.gain || slice.active <= slice.wall) continue;
    let rest = slice.active - slice.wall;
    slice.active = slice.wall;
    for (let j = i - 1; j >= 0 && rest > 0; j -= 1) {
      const take = Math.min(rest, slices[j].wall - slices[j].active);
      slices[j].active += take;
      rest -= take;
    }
  }
  return slices;
}

/** Slices whose end falls in the last `windowSec` before the last slice. */
function trailing(slices: Slice[], windowSec: number): Slice[] {
  const last = slices.at(-1);
  if (!last) return [];
  const cutoff = last.t - windowSec;
  let start = slices.length;
  while (start > 0 && slices[start - 1].t > cutoff) start -= 1;
  return slices.slice(start);
}

/**
 * EXP per hour of active time over the last `windowSec` of wall-clock time.
 * `null` until the run covers the window and has at least a minute (or the
 * whole window, if shorter) of active time in it.
 */
export function activeRate(slices: Slice[], windowSec: number): number | null {
  const last = slices.at(-1);
  const first = slices[0];
  if (!last || !first || last.t - (first.t - first.wall) < windowSec) return null;
  const window = trailing(slices, windowSec);
  const active = window.reduce((sum, slice) => sum + slice.active, 0);
  if (active < Math.min(60, windowSec)) return null;
  const gained = window.reduce((sum, slice) => sum + slice.activeGained, 0);
  return (gained / active) * 3600;
}

/** Percent of the current level per hour at `ratePerHour`. */
export function percentRate(level: number, ratePerHour: number | null): number | null {
  const req = expRequired(level);
  if (ratePerHour === null || !req || req.value <= 0) return null;
  return (ratePerHour / req.value) * 100;
}

export type Session = {
  start: number;
  end: number;
  /** Wall-clock seconds while recording, breaks included. */
  wall: number;
  active: number;
  gained: number;
  lost: number;
  deaths: number;
  levelUps: number;
  levelFrom: number;
  levelTo: number;
  name?: string;
  maps: MapShare[];
  /** Best active EXP/h over any 5 and 60 minutes of the session. */
  best5m: number | null;
  best60m: number | null;
};

export type MapShare = { map: string; level: number; active: number; gained: number };

/**
 * Split slices into sessions: a new one starts after SESSION_BREAK without a
 * gain, or when the character changes. Slices before a session's first gain
 * and after its last belong to no session.
 */
export function sessions(slices: Slice[]): Session[] {
  const out: Session[] = [];
  let run: Slice[] = [];
  let lastGainAt: number | null = null;
  const close = () => {
    let end = run.length - 1;
    while (end >= 0 && !run[end].gain) end -= 1;
    if (end >= 0) out.push(summarise(run.slice(0, end + 1)));
    run = [];
  };
  for (const slice of slices) {
    const switched = run.length > 0 && slice.name && run[0].name && slice.name !== run[0].name;
    const broke = lastGainAt !== null && slice.t - lastGainAt > SESSION_BREAK && slice.gain;
    if (switched || broke) {
      close();
      lastGainAt = null;
    }
    if (run.length === 0 && !slice.gain) continue;
    run.push(slice);
    if (slice.gain) lastGainAt = slice.t;
  }
  close();
  return out;
}

function summarise(run: Slice[]): Session {
  const first = run[0];
  const last = run[run.length - 1];
  const maps = new Map<string, MapShare>();
  let wall = 0;
  let active = 0;
  let gained = 0;
  let lost = 0;
  let deaths = 0;
  let levelUps = 0;
  for (const slice of run) {
    wall += slice.wall;
    active += slice.active;
    gained += slice.gained;
    lost += slice.lost;
    deaths += slice.deaths;
    levelUps += slice.levelUps;
    const key = `${slice.map}\u0000${slice.lv}`;
    const share = maps.get(key) ?? { map: slice.map, level: slice.lv, active: 0, gained: 0 };
    share.active += slice.active;
    share.gained += slice.activeGained;
    maps.set(key, share);
  }
  return {
    start: first.t - first.wall,
    end: last.t,
    wall,
    active,
    gained,
    lost,
    deaths,
    levelUps,
    levelFrom: first.lv - first.levelUps,
    levelTo: last.lv,
    name: first.name,
    maps: [...maps.values()].filter((share) => share.active > 0),
    best5m: bestRate(run, 300),
    best60m: bestRate(run, 3600),
  };
}

/** The best active rate over any `windowSec` of wall-clock time in `run`. */
export function bestRate(run: Slice[], windowSec: number): number | null {
  let best: number | null = null;
  let start = 0;
  let active = 0;
  let gained = 0;
  for (let end = 0; end < run.length; end += 1) {
    active += run[end].active;
    gained += run[end].activeGained;
    while (run[end].t - run[start].t >= windowSec) {
      active -= run[start].active;
      gained -= run[start].activeGained;
      start += 1;
    }
    const covered = run[end].t - (run[start].t - run[start].wall);
    if (covered >= windowSec * 0.9 && active >= windowSec * 0.5) {
      const rate = (gained / active) * 3600;
      if (best === null || rate > best) best = rate;
    }
  }
  return best;
}

/**
 * The spread of the session's minute-by-minute active rate: the 20th and 80th
 * percentiles, once at least ten minutes had a gain. An ETA range comes from
 * these.
 */
export function rateSpread(run: Slice[]): { low: number; high: number } | null {
  const minutes = new Map<number, { active: number; gained: number }>();
  for (const slice of run) {
    const minute = Math.floor(slice.t / 60);
    const bucket = minutes.get(minute) ?? { active: 0, gained: 0 };
    bucket.active += slice.active;
    bucket.gained += slice.activeGained;
    minutes.set(minute, bucket);
  }
  const rates = [...minutes.values()]
    .filter((bucket) => bucket.active >= 20)
    .map((bucket) => (bucket.gained / bucket.active) * 3600)
    .sort((a, b) => a - b);
  if (rates.length < 10) return null;
  const at = (q: number) => rates[Math.min(rates.length - 1, Math.floor(q * rates.length))];
  return { low: at(0.2), high: at(0.8) };
}

/** Seconds to the next level from `lv`/`exp` at `ratePerHour`. */
export function secondsToLevel(lv: number, exp: number, ratePerHour: number | null): number | null {
  const req = expRequired(lv);
  if (ratePerHour === null || ratePerHour <= 0 || !req) return null;
  return (Math.max(0, req.value - exp) / ratePerHour) * 3600;
}

/**
 * Kills estimated from the typical gain: the median EXP of the gains in the
 * last `windowSec`, when there are enough of them and they agree (most within
 * a third of the median). A party, quests or mixed monsters make gains vary,
 * and then there is no estimate.
 */
export function typicalGain(slices: Slice[], windowSec = 300): number | null {
  const gains = trailing(slices, windowSec)
    .filter((slice) => slice.gain && slice.activeGained > 0)
    .map((slice) => slice.gained)
    .sort((a, b) => a - b);
  if (gains.length < 8) return null;
  const median = gains[Math.floor(gains.length / 2)];
  const close = gains.filter((gain) => Math.abs(gain - median) <= median / 3).length;
  return close >= gains.length * 0.6 ? median : null;
}

/** The rate behind each slice's 5-minute window, for the overlay's graph. */
export function rateSeries(slices: Slice[], windowSec = 300): { t: number; rate: number | null }[] {
  const out: { t: number; rate: number | null }[] = [];
  let start = 0;
  let active = 0;
  let gained = 0;
  for (let end = 0; end < slices.length; end += 1) {
    active += slices[end].active;
    gained += slices[end].activeGained;
    while (slices[end].t - slices[start].t >= windowSec) {
      active -= slices[start].active;
      gained -= slices[start].activeGained;
      start += 1;
    }
    out.push({ t: slices[end].t, rate: active >= 60 ? (gained / active) * 3600 : null });
  }
  return out;
}
