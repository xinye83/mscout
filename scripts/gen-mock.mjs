// Dev-only mock data generator. Writes plausible Classic World samples so the
// chart and history window can be exercised without the game.
//
// It never touches real captures: it writes to $MSCOUT_DATA_DIR/samples, or to
// data-mock/samples when that is unset, refuses the real data/ folder, and
// refuses to overwrite any existing day file. Run the app against the same
// folder to see the rows:
//
//   node scripts/gen-mock.mjs
//   $env:MSCOUT_DATA_DIR = "$PWD\data-mock"; npm run tauri dev

import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dataDir = resolve(process.env.MSCOUT_DATA_DIR || join(root, "data-mock"));
if (dataDir.toLowerCase() === resolve(root, "data").toLowerCase()) {
  console.error(`Refusing to write mock rows into the real data folder (${dataDir}).`);
  console.error("Unset MSCOUT_DATA_DIR or point it at a separate folder.");
  process.exit(1);
}
const outDir = join(dataDir, "samples");

// Same rows as src-tauri/src/exp_table.rs (index level - 1).
const REQUIRED = [
  15, 34, 57, 92, 135, 372, 560, 840,
  1_242, 1_716, 2_360, 3_216, 4_200, 5_460, 7_050, 8_840,
  11_040, 13_716, 16_680, 20_216, 24_402, 28_980, 34_320, 40_512,
  47_216, 54_900, 63_666, 73_080, 83_720, 95_700, 108_480, 122_760,
  138_666, 155_540, 174_216, 194_832, 216_600, 240_500, 266_682, 294_216,
  324_240, 356_916, 391_160, 428_280, 468_450, 510_420, 555_680, 604_416,
  655_200, 709_716, 748_608, 789_631, 832_902, 878_545, 926_689, 977_471,
  1_031_036, 1_087_536, 1_147_132, 1_209_994, 1_276_301, 1_346_242, 1_420_016, 1_497_832,
  1_579_913, 1_666_492, 1_757_815, 1_854_143, 1_955_750, 2_062_925, 2_175_973, 2_295_216,
  2_420_993, 2_553_663, 2_693_603, 2_841_212, 2_996_910, 3_161_140, 3_334_370, 3_517_093,
  3_709_829, 3_913_127, 4_127_566, 4_353_756, 4_592_341, 4_844_001, 5_109_452, 5_389_449,
  5_684_790, 11_938_059, 25_069_923, 52_646_838, 110_558_359, 232_172_553, 487_562_361, 1_023_880_958,
  2_150_150_011, 4_515_315_023, 9_482_161_548,
];

function required(level) {
  return REQUIRED[level - 1];
}

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const rand = mulberry32(20261111);
const MAPS = [
  "Henesys Hunting Ground I",
  "The Pig Beach",
  "Kerning City Construction Site",
  "Sleepywood Ant Tunnel I",
];

function dayStamp(date) {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

const now = new Date();
const state = { lv: 68, exp: 0 };

function simulate(startSec, endSec, stepSec, map) {
  const rows = [];
  let nextKill = startSec + 6;
  let gainSinceWrite = false;

  for (let t = startSec; t <= endSec; t += 1) {
    const hour = (t - startSec) / 3600;
    // Take breaks: a long idle stretch each session, plus a short pause.
    const longBreak = hour >= 1.6 && hour < 2.4;
    const shortPause = hour >= 3.0 && hour < 3.15;

    if (!longBreak && !shortPause && t >= nextKill) {
      const req = required(state.lv);
      state.exp += 2600 + Math.floor(rand() * 900);
      if (state.exp >= req) {
        state.lv += 1;
        state.exp -= req;
      }
      gainSinceWrite = true;
      nextKill = t + 6 + Math.floor(rand() * 6);
    }

    if ((t - startSec) % stepSec === 0) {
      rows.push({
        t,
        lv: state.lv,
        exp: state.exp,
        pct: Number(((state.exp / required(state.lv)) * 100).toFixed(2)),
        map,
        state: gainSinceWrite ? "gaining" : "idle",
        conf: 1,
      });
      gainSinceWrite = false;
    }
  }

  return rows;
}

const sessions = [
  { daysAgo: 2, hours: 5, step: 60 },
  { daysAgo: 1, hours: 6, step: 60 },
  { daysAgo: 0, hours: 1.5, step: 1 },
];

// Collect every session first: a session that runs past midnight spills into the
// next day, and writing per session would let a later session clobber it.
const byDay = new Map();
for (const session of sessions) {
  const start = new Date(now);
  start.setDate(start.getDate() - session.daysAgo);
  start.setHours(19, 0, 0, 0);

  const startSec = Math.floor(start.getTime() / 1000);
  const endSec = startSec + Math.round(session.hours * 3600);
  const limitedEnd = Math.min(endSec, Math.floor(now.getTime() / 1000));

  const rows = simulate(startSec, limitedEnd, session.step, MAPS[session.daysAgo % MAPS.length]);
  for (const row of rows) {
    const key = dayStamp(new Date(row.t * 1000));
    if (!byDay.has(key)) byDay.set(key, []);
    byDay.get(key).push(row);
  }
}

const targets = [...byDay.keys()].map((day) => join(outDir, `${day}.jsonl`));
const existing = targets.filter((path) => existsSync(path));
if (existing.length > 0) {
  console.error("Refusing to overwrite existing day files:");
  for (const path of existing) console.error(`  ${path}`);
  console.error(`Delete ${outDir} (mock data only) and run again.`);
  process.exit(1);
}

mkdirSync(outDir, { recursive: true });
for (const [day, list] of [...byDay].sort(([a], [b]) => a.localeCompare(b))) {
  list.sort((a, b) => a.t - b.t);
  writeFileSync(join(outDir, `${day}.jsonl`), list.map((r) => JSON.stringify(r)).join("\n") + "\n", {
    flag: "wx",
  });
  console.log(`${day}: ${list.length} samples, lv ${list[0].lv} -> ${list[list.length - 1].lv}`);
}
console.log(`Wrote mock samples to ${outDir}`);
