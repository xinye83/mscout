const CONFIRMED: [i64; 49] = [
    15, 34, 57, 92, 135, 372, 560, 840, 1242, 1716, 2360, 3216, 4200, 5460, 7050, 8840, 11040,
    13716, 16680, 20216, 24402, 28980, 34320, 40512, 47216, 54900, 63666, 73080, 83720, 95700,
    108480, 122760, 138666, 155540, 174216, 194832, 216600, 240500, 266682, 294216, 324240,
    356916, 391160, 428280, 468450, 510420, 555680, 604416, 655200,
];

const LEVEL_CAP: u32 = 100;
const LEVEL_89: f64 = 5_684_790.0;
const LEVEL_90: f64 = 11_938_059.0;

pub fn required(level: u32) -> Option<f64> {
    if !(1..LEVEL_CAP).contains(&level) {
        return None;
    }

    if level <= 49 {
        return Some(CONFIRMED[level as usize - 1] as f64);
    }

    if level <= 89 {
        let growth = (LEVEL_89 / CONFIRMED[48] as f64).powf(1.0 / 40.0);
        return Some((CONFIRMED[48] as f64 * growth.powi(level as i32 - 49)).round());
    }

    Some((LEVEL_90 * 2.1f64.powi(level as i32 - 90)).round())
}

/// The level implied by an absolute EXP value and the EXP-bar percentage:
/// `required(lv) ≈ exp × 100 / pct`. Lets a profile work without a level box.
/// Returns `None` when the percentage is too small to pin the level down, or no
/// table value is close enough.
pub fn level_for_exp_pct(exp: i64, pct: f64) -> Option<u32> {
    if exp < 0 || !(0.01..=100.0).contains(&pct) {
        return None;
    }

    // The percentage has two decimals, so the derived requirement carries a
    // relative error of about 0.005/pct — a tenth of a percent around 12%.
    // Levels 1-49 have confirmed requirements, so a read that misses one by
    // more than that is not a level at all: a truncated number used to look
    // like level 19 at 1.8% away. Levels above 49 are estimated, so the bound
    // there stays loose.
    if 0.005 / pct > 0.02 {
        return None;
    }

    let target = exp as f64 * 100.0 / pct;
    let mut best: Option<(u32, f64)> = None;
    for level in 1..LEVEL_CAP {
        let Some(value) = required(level) else {
            continue;
        };
        let error = (value - target).abs() / value;
        if best.map_or(true, |(_, closest)| error < closest) {
            best = Some((level, error));
        }
    }

    let (level, error) = best?;
    // Tied to the precision of the percentage itself: two decimals put a floor of
    // 0.005/pct under the derived requirement, so three times that (plus a little)
    // is the most a genuine read can be out. A flat 2% let a truncated 21,476 —
    // read as 2,106 at 12.40% — pass as level 19.
    let tolerance = if level <= 49 {
        3.0 * 0.005 / pct + 0.001
    } else {
        0.02
    };
    if error <= tolerance {
        Some(level)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_level_from_exp_and_percentage() {
        // 959 EXP at 0.55% of level 35's 174,216.
        assert_eq!(level_for_exp_pct(959, 0.55), Some(35));
        // A larger fraction of the bar pins the level harder.
        assert_eq!(level_for_exp_pct(87_108, 50.0), Some(35));
        // Too small a percentage, or no EXP at all, cannot pin it down.
        assert_eq!(level_for_exp_pct(0, 0.0), None);
        assert_eq!(level_for_exp_pct(5, 0.02), None);
    }

    #[test]
    fn rejects_the_truncated_numbers_that_dipped_the_history() {
        // Each of these is a real recorded row: a truncated EXP number whose
        // percentage still pointed at some other level's requirement.
        assert_eq!(level_for_exp_pct(2_106, 12.40), None, "21,476 read as 2,106");
        assert_eq!(level_for_exp_pct(2_808, 16.68), None, "28,978 read as 2,808");
        assert_eq!(level_for_exp_pct(3_841, 22.87), None, "38,883 read as 3,841");
        assert_eq!(level_for_exp_pct(452, 26.21), None, "45,590 read as 452");

        // The genuine rows around them must still resolve.
        assert_eq!(level_for_exp_pct(45_004, 25.83), Some(35));
        assert_eq!(level_for_exp_pct(42_864, 24.60), Some(35));
        assert_eq!(level_for_exp_pct(21_478, 12.33), Some(35));
    }

    #[test]
    fn anchors_match_the_frontend_table() {
        assert_eq!(required(1), Some(15.0));
        assert_eq!(required(30), Some(95_700.0));
        assert_eq!(required(49), Some(655_200.0));
        assert_eq!(required(89), Some(5_684_790.0));
        assert_eq!(required(90), Some(11_938_059.0));
        assert_eq!(required(99), Some((11_938_059.0 * 2.1f64.powi(9)).round()));
        assert_eq!(required(100), None);
        assert_eq!(required(0), None);
    }
}
