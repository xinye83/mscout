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

const CONFIRMED = {
  1: 15, 2: 34, 3: 57, 4: 92, 5: 135, 6: 372, 7: 560, 8: 840, 9: 1242,
  10: 1716, 11: 2360, 12: 3216, 13: 4200, 14: 5460, 15: 7050, 16: 8840,
  17: 11040, 18: 13716, 19: 16680, 20: 20216, 21: 24402, 22: 28980,
  23: 34320, 24: 40512, 25: 47216, 26: 54900, 27: 63666, 28: 73080,
  29: 83720, 30: 95700, 31: 108480, 32: 122760, 33: 138666, 34: 155540,
  35: 174216, 36: 194832, 37: 216600, 38: 240500, 39: 266682, 40: 294216,
  41: 324240, 42: 356916, 43: 391160, 44: 428280, 45: 468450, 46: 510420,
  47: 555680, 48: 604416, 49: 655200,
};
const GROWTH = Math.pow(5_684_790 / CONFIRMED[49], 1 / 40);

function required(level) {
  if (CONFIRMED[level]) return CONFIRMED[level];
  if (level <= 89) return Math.round(CONFIRMED[49] * Math.pow(GROWTH, level - 49));
  return Math.round(11_938_059 * Math.pow(2.1, level - 90));
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
