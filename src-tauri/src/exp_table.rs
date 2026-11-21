/// EXP needed to fill the bar at each level of Classic World, from 1 to 99
/// (index `level - 1`). Level 100 is the cap and needs nothing.
///
/// Source: NiaMeowDB's table, <https://meowdb.com/msclassic/guides/exp-table-level-1-to-100>.
/// Levels 1-49 are confirmed against live play. Levels 50-89 are NiaMeowDB's
/// planning numbers, not yet confirmed in Classic World, so the UI labels them
/// estimated and the engine cross-checks them with a wider tolerance. 90-99 come
/// from launch data. Totals: 103,322,764 to reach 90 and 18,194,778,397 to reach
/// 100 (docs/SPEC.md section 11).
///
/// Levels 50-88 used to be a geometric interpolation between 49 and 89, up to
/// 2.6% below these rows, which pushed the cross-check past its 1-point
/// tolerance late in a level and blanked otherwise good frames.
const REQUIRED: [i64; 99] = [
    15,
    34,
    57,
    92,
    135,
    372,
    560,
    840,
    1_242,
    1_716,
    2_360,
    3_216,
    4_200,
    5_460,
    7_050,
    8_840,
    11_040,
    13_716,
    16_680,
    20_216,
    24_402,
    28_980,
    34_320,
    40_512,
    47_216,
    54_900,
    63_666,
    73_080,
    83_720,
    95_700,
    108_480,
    122_760,
    138_666,
    155_540,
    174_216,
    194_832,
    216_600,
    240_500,
    266_682,
    294_216,
    324_240,
    356_916,
    391_160,
    428_280,
    468_450,
    510_420,
    555_680,
    604_416,
    655_200,
    709_716,
    748_608,
    789_631,
    832_902,
    878_545,
    926_689,
    977_471,
    1_031_036,
    1_087_536,
    1_147_132,
    1_209_994,
    1_276_301,
    1_346_242,
    1_420_016,
    1_497_832,
    1_579_913,
    1_666_492,
    1_757_815,
    1_854_143,
    1_955_750,
    2_062_925,
    2_175_973,
    2_295_216,
    2_420_993,
    2_553_663,
    2_693_603,
    2_841_212,
    2_996_910,
    3_161_140,
    3_334_370,
    3_517_093,
    3_709_829,
    3_913_127,
    4_127_566,
    4_353_756,
    4_592_341,
    4_844_001,
    5_109_452,
    5_389_449,
    5_684_790,
    11_938_059,
    25_069_923,
    52_646_838,
    110_558_359,
    232_172_553,
    487_562_361,
    1_023_880_958,
    2_150_150_011,
    4_515_315_023,
    9_482_161_548,
];

/// Levels whose requirement is a planning number, not confirmed in live play
/// (see [`REQUIRED`]). Levels below are confirmed; 90 and up are launch data.
pub const ESTIMATED: std::ops::RangeInclusive<u32> = 50..=89;

pub fn required(level: u32) -> Option<f64> {
    let index = usize::try_from(level.checked_sub(1)?).ok()?;
    REQUIRED.get(index).map(|&exp| exp as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_match_the_frontend_table() {
        assert_eq!(required(1), Some(15.0));
        assert_eq!(required(30), Some(95_700.0));
        assert_eq!(required(49), Some(655_200.0));
        assert_eq!(required(50), Some(709_716.0));
        assert_eq!(required(89), Some(5_684_790.0));
        assert_eq!(required(90), Some(11_938_059.0));
        assert_eq!(required(99), Some(9_482_161_548.0));
        assert_eq!(required(100), None);
        assert_eq!(required(0), None);
    }

    #[test]
    fn totals_match_the_spec() {
        let to = |level: u32| (1..level).filter_map(required).sum::<f64>();
        assert_eq!(to(90), 103_322_764.0);
        assert_eq!(to(100), 18_194_778_397.0);
    }

    #[test]
    fn requirements_rise_every_level() {
        assert!(REQUIRED.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!((*ESTIMATED.start(), *ESTIMATED.end()), (50, 89));
    }
}
