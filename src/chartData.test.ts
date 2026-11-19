import { test } from "node:test";
import assert from "node:assert/strict";
import { downsample } from "./chartData.ts";
import type { ChartPoint } from "./ExpChart.ts";

/** A run of real samples, then a break, then another run. */
function series(runs: number[][], start = 1000): ChartPoint[] {
  const points: ChartPoint[] = [];
  let t = start;
  let value = 0;
  for (const run of runs) {
    for (const delta of run) {
      value += delta;
      points.push({ t, total: value });
      t += 1;
    }
    // a gap marker, exactly as History emits it: null at the break time
    points.push({ t, total: null });
    t += 1;
  }
  return points;
}

test("a small series is returned untouched", () => {
  const points = series([[1, 1, 1], [1, 1]]);
  assert.deepEqual(downsample(points, 10_000), points);
});

test("thinning keeps every gap marker", () => {
  // 400 samples over four runs of 100, so three breaks; thin to 20 points.
  const points = series([
    Array(100).fill(1),
    Array(100).fill(1),
    Array(100).fill(1),
    Array(100).fill(1),
  ]);
  const gapsBefore = points.filter((p) => p.total === null).length;
  assert.equal(gapsBefore, 4);

  const thinned = downsample(points, 20);
  const gapsAfter = thinned.filter((p) => p.total === null).length;

  assert.equal(
    gapsAfter,
    gapsBefore,
    "a break must not be dropped when the series is thinned",
  );
  assert.ok(thinned.length <= 40, `expected a real reduction, got ${thinned.length}`);
});

test("the naive last-point-per-bucket approach would have lost gaps", () => {
  // This is the regression the fix addresses: a gap followed by real samples in
  // the same bucket. Keeping only the bucket's last point loses the null.
  const points: ChartPoint[] = [];
  for (let i = 0; i < 50; i += 1) points.push({ t: i, total: i });
  points.push({ t: 50, total: null });
  for (let i = 51; i < 100; i += 1) points.push({ t: i, total: i });

  const thinned = downsample(points, 2);
  assert.ok(
    thinned.some((point) => point.total === null),
    "the single break in this series must survive aggressive thinning",
  );
});

test("output stays in ascending time order", () => {
  const points = series([Array(60).fill(1), Array(60).fill(1), Array(60).fill(1)]);
  const thinned = downsample(points, 12);

  for (let i = 1; i < thinned.length; i += 1) {
    assert.ok(
      thinned[i].t > thinned[i - 1].t,
      `time went backwards at ${i}: ${thinned[i - 1].t} then ${thinned[i].t}`,
    );
  }
});

test("an all-real series still reduces without inventing nulls", () => {
  const points: ChartPoint[] = [];
  for (let i = 0; i < 1000; i += 1) points.push({ t: i, total: i });

  const thinned = downsample(points, 100);
  assert.ok(thinned.length <= 100, `expected <= 100 points, got ${thinned.length}`);
  assert.equal(thinned.filter((p) => p.total === null).length, 0);
  assert.equal(thinned[thinned.length - 1].total, 999, "the last sample is kept");
});
