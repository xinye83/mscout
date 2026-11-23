import { test } from "node:test";
import assert from "node:assert/strict";
import {
  activeRate,
  bestRate,
  IDLE_PAUSE,
  rateSpread,
  sessions,
  slicesFromSamples,
  typicalGain,
} from "./stats.ts";
import type { Sample } from "./types.ts";

function row(t: number, exp: number, extra: Partial<Sample> = {}): Sample {
  return { t, lv: 10, exp, pct: 0, map: "Field", state: "gaining", conf: 1, ...extra };
}

/** One row a second from `from` to `to`, gaining `each` every `every` seconds. */
function grind(from: number, to: number, start: number, every: number, each: number, extra: Partial<Sample> = {}): Sample[] {
  const rows: Sample[] = [];
  let exp = start;
  for (let t = from; t <= to; t += 1) {
    if (t > from && (t - from) % every === 0) exp += each;
    rows.push(row(t, exp, extra));
  }
  return rows;
}

test("steady gains count as active time and give their rate", () => {
  const slices = slicesFromSamples(grind(0, 600, 0, 5, 10));
  const active = slices.reduce((sum, slice) => sum + slice.active, 0);
  // The first gain has no gain before it to measure from.
  assert.equal(active, 595);
  // 10 EXP every 5 s is 7,200 an hour.
  assert.equal(Math.round(activeRate(slices, 300)!), 7200);
});

test("a break longer than the pause is not active time", () => {
  const rows = [...grind(0, 300, 0, 5, 10), ...grind(301, 900, 600, 1000, 0), ...grind(901, 1200, 600, 5, 10)];
  const slices = slicesFromSamples(rows);
  const active = slices.reduce((sum, slice) => sum + slice.active, 0);
  // 300 s before the break and 295 s after it (the gain that ends the break
  // restarts the clock); the 600 s break itself does not count.
  assert.ok(active >= 585 && active <= 600, `active ${active}`);
  const gained = slices.reduce((sum, slice) => sum + slice.gained, 0);
  assert.equal(gained, rows.at(-1)!.exp);
});

test("waits up to the pause count, longer ones do not", () => {
  const short = slicesFromSamples(grind(0, 300, 0, IDLE_PAUSE, 100));
  const long = slicesFromSamples(grind(0, 300, 0, IDLE_PAUSE + 1, 100));
  assert.ok(short.reduce((sum, s) => sum + s.active, 0) >= 270);
  assert.equal(long.reduce((sum, s) => sum + s.active, 0), 0);
});

test("a rate needs the window covered", () => {
  const slices = slicesFromSamples(grind(0, 200, 0, 5, 10));
  assert.equal(activeRate(slices, 300), null);
});

test("a recording gap breaks the stretch", () => {
  const rows = [...grind(0, 100, 0, 5, 10), ...grind(120, 200, 200, 5, 10)];
  const slices = slicesFromSamples(rows);
  assert.equal(slices.find((slice) => slice.t === 120)!.wall, 0);
  assert.equal(slices.find((slice) => slice.t === 120)!.active, 0);
});

test("a death is lost EXP, not a negative gain", () => {
  const rows = [row(0, 1000), row(1, 1010), row(2, 900, { event: "death" }), row(3, 905)];
  const slices = slicesFromSamples(rows);
  assert.equal(slices[1].lost, 110);
  assert.equal(slices[1].deaths, 1);
  assert.equal(slices.reduce((sum, s) => sum + s.gained, 0), 15);
});

test("ten minutes without a gain start a new session", () => {
  const rows = [...grind(0, 600, 0, 5, 10), ...grind(601, 1300, 1200, 10_000, 0), ...grind(1301, 1900, 1200, 5, 10, { map: "Other" })];
  const found = sessions(slicesFromSamples(rows));
  assert.equal(found.length, 2);
  assert.equal(found[0].gained, 1200);
  assert.deepEqual(found[1].maps.map((share) => share.map), ["Other"]);
});

test("a different character starts a new session", () => {
  const rows = [...grind(0, 120, 0, 5, 10, { name: "A" }), ...grind(121, 240, 0, 5, 10, { name: "B" })];
  const found = sessions(slicesFromSamples(rows));
  assert.deepEqual(found.map((session) => session.name), ["A", "B"]);
});

test("the best 5 minutes are found", () => {
  const rows = [...grind(0, 600, 0, 5, 10), ...grind(601, 1200, 1200, 5, 30)];
  const best = bestRate(slicesFromSamples(rows), 300)!;
  assert.ok(Math.abs(best - 21_600) < 500, `best ${best}`);
});

test("the spread needs ten minutes and orders low below high", () => {
  assert.equal(rateSpread(slicesFromSamples(grind(0, 300, 0, 5, 10))), null);
  const rows = [...grind(0, 600, 0, 5, 10), ...grind(601, 1200, 1200, 5, 20)];
  const spread = rateSpread(slicesFromSamples(rows))!;
  assert.ok(spread.low < spread.high);
});

test("a steady gain size is the typical gain; a mixed one is not", () => {
  assert.equal(typicalGain(slicesFromSamples(grind(0, 300, 0, 5, 42))), 42);
  const mixed: Sample[] = [];
  let exp = 0;
  for (let t = 0; t <= 300; t += 1) {
    if (t > 0 && t % 5 === 0) exp += t % 10 === 0 ? 10 : 200;
    mixed.push(row(t, exp));
  }
  assert.equal(typicalGain(slicesFromSamples(mixed)), null);
});
