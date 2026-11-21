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
///
/// The rows, read from `src-tauri/exp-table.json`: the one copy shared with the
/// UI (`src/expTable.ts`) and the mock generator.
fn rows() -> &'static [i64] {
    static REQUIRED: std::sync::OnceLock<Vec<i64>> = std::sync::OnceLock::new();
    REQUIRED.get_or_init(|| table().required)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Table {
    // Read by the test that keeps [`ESTIMATED`] in step with the file.
    #[cfg_attr(not(test), allow(dead_code))]
    confirmed_through: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    estimated_through: u32,
    required: Vec<i64>,
}

fn table() -> Table {
    serde_json::from_str(include_str!("../exp-table.json")).expect("exp-table.json is valid")
}

/// Levels whose requirement is a planning number, not confirmed in live play
/// (see [`rows`]). Levels below are confirmed; 90 and up are launch data.
pub const ESTIMATED: std::ops::RangeInclusive<u32> = 50..=89;

pub fn required(level: u32) -> Option<f64> {
    let index = usize::try_from(level.checked_sub(1)?).ok()?;
    rows().get(index).map(|&exp| exp as f64)
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
        assert_eq!(rows().len(), 99);
        assert!(rows().windows(2).all(|pair| pair[0] <= pair[1]));
        let table = table();
        assert_eq!(
            (*ESTIMATED.start(), *ESTIMATED.end()),
            (table.confirmed_through + 1, table.estimated_through),
            "ESTIMATED matches the shared table"
        );
    }
}
