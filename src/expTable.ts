const CONFIRMED: Record<number, number> = {
  1: 15,
  2: 34,
  3: 57,
  4: 92,
  5: 135,
  6: 372,
  7: 560,
  8: 840,
  9: 1_242,
  10: 1_716,
  11: 2_360,
  12: 3_216,
  13: 4_200,
  14: 5_460,
  15: 7_050,
  16: 8_840,
  17: 11_040,
  18: 13_716,
  19: 16_680,
  20: 20_216,
  21: 24_402,
  22: 28_980,
  23: 34_320,
  24: 40_512,
  25: 47_216,
  26: 54_900,
  27: 63_666,
  28: 73_080,
  29: 83_720,
  30: 95_700,
  31: 108_480,
  32: 122_760,
  33: 138_666,
  34: 155_540,
  35: 174_216,
  36: 194_832,
  37: 216_600,
  38: 240_500,
  39: 266_682,
  40: 294_216,
  41: 324_240,
  42: 356_916,
  43: 391_160,
  44: 428_280,
  45: 468_450,
  46: 510_420,
  47: 555_680,
  48: 604_416,
  49: 655_200,
};

const LEVEL_89 = 5_684_790;
const LEVEL_90 = 11_938_059;
const WALL_GROWTH = Math.pow(LEVEL_89 / CONFIRMED[49], 1 / 40);

export const LEVEL_CAP = 100;

export type ExpRequirement = { value: number; estimated: boolean } | null;

export function expRequired(level: number): ExpRequirement {
  if (level < 1 || level >= LEVEL_CAP) return null;

  const confirmed = CONFIRMED[level];
  if (confirmed !== undefined) return { value: confirmed, estimated: false };

  if (level <= 89) {
    const value = CONFIRMED[49] * Math.pow(WALL_GROWTH, level - 49);
    return { value: Math.round(value), estimated: true };
  }

  const value = LEVEL_90 * Math.pow(2.1, level - 90);
  return { value: Math.round(value), estimated: false };
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
