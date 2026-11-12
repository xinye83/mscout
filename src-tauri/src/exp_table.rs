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

#[cfg(test)]
mod tests {
    use super::*;

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
