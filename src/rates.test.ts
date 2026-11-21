import { test } from "node:test";
import assert from "node:assert/strict";
import { currentCharacterRun, rateOver } from "./rates.ts";
import type { Sample } from "./types.ts";

function row(t: number, lv: number, exp: number, name?: string): Sample {
  return { t, lv, exp, pct: 0, map: "", state: "gaining", conf: 1, name };
}

test("a switch to a lower-level character starts a new run", () => {
  const samples = [
    row(0, 37, 101_000, "Euroblood"),
    row(60, 37, 101_013, "Euroblood"),
    row(120, 1, 0, "Euroveln"),
    row(180, 1, 2, "Euroveln"),
  ];
  assert.deepEqual(
    currentCharacterRun(samples).map((sample) => sample.t),
    [120, 180],
  );
});

test("a different name starts a new run even at a higher level", () => {
  const samples = [row(0, 5, 10, "Alt"), row(60, 30, 500, "Main"), row(120, 30, 600, "Main")];
  assert.deepEqual(
    currentCharacterRun(samples).map((sample) => sample.t),
    [60, 120],
  );
});

test("older rows without a name stay with the run until the level drops", () => {
  const samples = [row(0, 40, 1), row(60, 35, 5), row(120, 35, 9), row(180, 36, 1, "Main")];
  assert.deepEqual(
    currentCharacterRun(samples).map((sample) => sample.t),
    [60, 120, 180],
  );
});

test("the rate across a character switch is not negative", () => {
  const samples = [
    row(0, 37, 101_000, "Euroblood"),
    row(30, 37, 101_013, "Euroblood"),
    row(40, 1, 0, "Euroveln"),
    row(100, 1, 10, "Euroveln"),
  ];
  // An 80 s window reaches back to t = 0, before the switch.
  assert.ok((rateOver(samples, 80) ?? 0) < 0, "the unfiltered rate is the bug");
  assert.equal(rateOver(currentCharacterRun(samples), 80), null, "no full window yet");
  assert.equal(rateOver(currentCharacterRun(samples), 60), 600);
});

test("EXP lost to a recorded death is not counted against the rate", () => {
  const death: Sample = { ...row(120, 30, 900, "Main"), event: "death" };
  const samples = [row(0, 30, 1000, "Main"), row(60, 30, 1000, "Main"), death, row(180, 30, 1000, "Main")];
  // 100 lost at the death, 100 re-earned: 100 EXP earned over 180 s.
  assert.equal(rateOver(samples, 180), 2000);
});
