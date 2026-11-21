/**
 * EXP needed to fill the bar at each level of Classic World, 1 to 99 (index
 * `level - 1`). Kept in step with `src-tauri/src/exp_table.rs`; see there for the
 * source. Levels 50-89 are planning numbers, not yet confirmed in play, and are
 * labelled estimated.
 */
const REQUIRED: readonly number[] = [
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

const CONFIRMED_THROUGH = 49;
const ESTIMATED_THROUGH = 89;

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
