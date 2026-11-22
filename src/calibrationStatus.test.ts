import { test } from "node:test";
import assert from "node:assert/strict";
import { readoutStatus } from "./calibration/status.ts";
import type { StatusInput } from "./calibration/status.ts";
import type { Calibration, LearningInfo, Region } from "./types.ts";

const box = (x: number, y: number, w: number, h: number): Region => ({ x, y, w, h });

const noBoxes: Calibration = {
  clientW: 1920,
  clientH: 1080,
  exp: null,
  level: null,
  street: null,
  map: null,
  name: null,
  job: null,
};

const preset: Calibration = {
  ...noBoxes,
  exp: box(1023, 1050, 90, 11),
  level: box(598, 1050, 41, 26),
  name: box(645, 1064, 118, 9),
  job: box(645, 1050, 118, 11),
};

const seeded: LearningInfo = {
  learned: "0123456789,.[]%",
  coverage: "0123456789",
  levelLearned: "0123456789",
  levelCoverage: "0123456789",
  mapLetters: "",
  maps: [],
  name: null,
  job: null,
  source: "game",
  glyphSize: "5x7",
  levelSource: "game",
  levelSize: "7x7",
  identityLetters: "abc",
  identitySource: "font",
  identitySize: "5x9",
};

function input(overrides: Partial<StatusInput>): StatusInput {
  return {
    learning: null,
    mscw: null,
    regions: noBoxes,
    presetCal: null,
    manual: {},
    expanded: {},
    probe: null,
    probeProvenance: null,
    ...overrides,
  };
}

test("without an EXP box the window says it cannot read", () => {
  const status = readoutStatus(input({}));
  assert.equal(status.expUsable, false);
  assert.equal(status.statusText, "EXP box needed — MScout cannot read without it");
  assert.equal(status.statusFor.exp, "no box");
});

test("a fully seeded preset profile is ready and folds every readout", () => {
  const status = readoutStatus(
    input({
      learning: seeded,
      mscw: { path: "D:/MSCW", exists: true, atlasFound: true, mapNames: 1200, error: null },
      regions: preset,
      presetCal: preset,
    }),
  );
  assert.equal(status.statusTone, "ok");
  assert.equal(status.statusText, "Ready to read · digits, name/job and map/street auto");
  assert.equal(status.statusFor.exp, "auto · 10/10 digits");
  assert.equal(status.statusFor.identity, "auto · Arial");
  for (const key of ["exp", "level", "map", "identity"] as const) {
    assert.equal(status.done[key], true, key);
    assert.equal(status.showBody(key), false, key);
    assert.equal(status.showManual(key), false, key);
  }
  // Preset boxes are not the player's calibration, so none is drawn or clearable.
  assert.equal(status.showBox("exp"), false);
  assert.equal(status.canClear("exp"), false);
});

test("missing EXP digits are named in the status line", () => {
  const status = readoutStatus(
    input({
      learning: { ...seeded, coverage: "0123456" },
      regions: { ...preset, exp: box(1000, 1050, 90, 11) },
      presetCal: preset,
    }),
  );
  assert.equal(status.statusText, "Reading · EXP digits missing 7 8 9");
  assert.equal(status.showBox("exp"), true, "a box that differs from the preset is drawn");
  assert.equal(status.showManual("exp"), true, "an incomplete set opens manual teaching");
});
