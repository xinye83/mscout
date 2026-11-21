//! Which readings become history.
//!
//! The engine reads the HUD once a second. A reading is shown on the overlay as
//! soon as it parses, but it only becomes a history row once the reader has been
//! consistent for a while, and it is checked against the last recorded row so a
//! misread cannot bend the EXP curve. This module is that decision, kept free of
//! capture and Tauri so every case can be tested directly.
//!
//! The rules:
//! - Rows are written only after [`VERIFY_READS`] accepted readings **in a
//!   row**. Any rejected or unreadable frame starts the count again.
//! - Levels only go up. A lower level is a misread — unless the character
//!   changed (a different name), which starts a fresh baseline.
//! - Within a level, EXP only goes up, except at a death. A drop is accepted
//!   once [`DROP_CONFIRM_READS`] consecutive readings agree on it. A large drop
//!   (at least [`DEATH_MIN_SHARE`] of the level's requirement, or to zero) is
//!   recorded as a death. A small one means the baseline itself was a misread,
//!   and the history simply continues from the confirmed value. Either way,
//!   recording no longer stalls until the lost EXP is earned back.

/// Accepted readings in a row before rows are written.
pub const VERIFY_READS: usize = 3;
/// Consecutive readings that must agree on an EXP drop before it is believed.
/// The misread this guard exists for — a two-EXP blip — held for two reads.
pub const DROP_CONFIRM_READS: usize = 5;
/// The smallest loss, as a share of the level's EXP requirement, recorded as a
/// death. Classic World's penalty is unconfirmed (pre-Big Bang lost roughly
/// 5–10% of the bar); a misread blip is a few EXP.
pub const DEATH_MIN_SHARE: f64 = 0.01;

/// One reading, as far as the guard cares.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub lv: u32,
    pub exp: i64,
    /// The character name when it was read; used to notice a character switch.
    pub name: Option<String>,
}

/// Whether EXP moved on this reading. Recorded as `"gaining"` / `"idle"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Gaining,
    Idle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Show the reading; write it to history when `verified`.
    Accept {
        state: State,
        verified: bool,
        /// The reading confirms a death: EXP fell and stayed down.
        death: bool,
    },
    /// Show the reading but keep it out of history for now (an unrecognised
    /// level, or a large drop that is not confirmed yet). Not a read failure.
    Hold(String),
    /// A misread: keep it out of history and count it as a failed read.
    Reject(String),
}

/// Consecutive readings below the baseline at one level.
#[derive(Clone, Debug)]
struct DropRun {
    lv: u32,
    last_exp: i64,
    count: usize,
}

#[derive(Debug, Default)]
pub struct Guard {
    /// The last recorded reading: what the next one is compared against.
    baseline: Option<Point>,
    good_reads: usize,
    drop: Option<DropRun>,
}

impl Guard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start from a row already in the history, e.g. after a restart.
    pub fn seed(&mut self, point: Point) {
        self.baseline = Some(point);
    }

    /// Forget everything: the capture target went away.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// A frame failed before it reached the guard (parse error, no frame). The
    /// next rows need a fresh run of good reads, and a drop run is broken.
    pub fn miss(&mut self) {
        self.good_reads = 0;
        self.drop = None;
    }

    /// Decide what to do with a parsed reading. `required` is the EXP the
    /// reading's level needs, when known.
    pub fn judge(&mut self, reading: &Point, required: Option<f64>) -> Verdict {
        if reading.lv == 0 {
            self.miss();
            return Verdict::Hold("level not recognised".into());
        }

        if let (Some(base), Some(name)) = (&self.baseline, &reading.name) {
            if base
                .name
                .as_ref()
                .is_some_and(|base_name| base_name != name)
            {
                // Another character: nothing about the last one's numbers applies.
                self.reset();
            }
        }

        let Some(base) = self.baseline.clone() else {
            return self.accept(reading, State::Gaining);
        };

        if reading.lv < base.lv {
            self.miss();
            return Verdict::Reject(format!("level fell from {} to {}", base.lv, reading.lv));
        }
        if reading.lv > base.lv || reading.exp > base.exp {
            return self.accept(reading, State::Gaining);
        }
        if reading.exp == base.exp {
            return self.accept(reading, State::Idle);
        }

        // Same level, less EXP: a death, a misread now, or a misread baseline.
        self.good_reads = 0;
        let count = match &mut self.drop {
            Some(run) if run.lv == reading.lv && reading.exp >= run.last_exp => {
                run.count += 1;
                run.last_exp = reading.exp;
                run.count
            }
            _ => {
                self.drop = Some(DropRun {
                    lv: reading.lv,
                    last_exp: reading.exp,
                    count: 1,
                });
                1
            }
        };

        let loss = base.exp - reading.exp;
        let large = reading.exp == 0
            || required.is_some_and(|required| loss as f64 >= DEATH_MIN_SHARE * required);

        if count < DROP_CONFIRM_READS {
            return if large {
                Verdict::Hold(format!("EXP fell by {loss}; confirming"))
            } else {
                Verdict::Reject("EXP dropped without a level change".into())
            };
        }

        // Confirmed by consecutive agreeing reads: carry on from here.
        self.drop = None;
        self.good_reads = VERIFY_READS;
        self.baseline = Some(reading.clone());
        Verdict::Accept {
            state: State::Idle,
            verified: true,
            death: large,
        }
    }

    fn accept(&mut self, reading: &Point, state: State) -> Verdict {
        self.drop = None;
        self.good_reads = self.good_reads.saturating_add(1);
        let verified = self.good_reads >= VERIFY_READS;
        if verified {
            self.baseline = Some(reading.clone());
        }
        Verdict::Accept {
            state,
            verified,
            death: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LV35: f64 = 174_216.0;

    fn at(lv: u32, exp: i64) -> Point {
        Point {
            lv,
            exp,
            name: None,
        }
    }

    fn named(lv: u32, exp: i64, name: &str) -> Point {
        Point {
            lv,
            exp,
            name: Some(name.into()),
        }
    }

    fn judge(guard: &mut Guard, point: Point) -> Verdict {
        guard.judge(&point, Some(LV35))
    }

    fn recorded(verdict: &Verdict) -> bool {
        matches!(verdict, Verdict::Accept { verified: true, .. })
    }

    /// A guard that has already recorded `exp` at level 35.
    fn settled(exp: i64) -> Guard {
        let mut guard = Guard::new();
        for _ in 0..VERIFY_READS {
            judge(&mut guard, at(35, exp));
        }
        guard
    }

    #[test]
    fn records_only_after_consecutive_good_reads() {
        let mut guard = Guard::new();
        assert!(!recorded(&judge(&mut guard, at(35, 100))));
        assert!(!recorded(&judge(&mut guard, at(35, 110))));
        guard.miss(); // a frame that failed to parse
        assert!(!recorded(&judge(&mut guard, at(35, 120))));
        assert!(!recorded(&judge(&mut guard, at(35, 130))));
        assert!(recorded(&judge(&mut guard, at(35, 140))));
    }

    #[test]
    fn the_two_exp_blip_is_never_recorded_and_is_not_a_death() {
        // The recorded history this guard was written against.
        let mut guard = settled(21_478);
        for _ in 0..2 {
            assert_eq!(
                judge(&mut guard, at(35, 21_476)),
                Verdict::Reject("EXP dropped without a level change".into())
            );
        }
        let next = judge(&mut guard, at(35, 21_500));
        assert!(matches!(next, Verdict::Accept { death: false, .. }));
        assert!(
            !recorded(&next),
            "the rejections restarted the good-read count"
        );
    }

    #[test]
    fn a_confirmed_death_is_recorded_and_recording_continues() {
        let mut guard = settled(100_000);
        let after = 100_000 - 17_421; // a 10% penalty at level 35
        for _ in 1..DROP_CONFIRM_READS {
            assert!(matches!(judge(&mut guard, at(35, after)), Verdict::Hold(_)));
        }
        assert_eq!(
            judge(&mut guard, at(35, after)),
            Verdict::Accept {
                state: State::Idle,
                verified: true,
                death: true
            }
        );
        // Before, every read below 100,000 was rejected until it was earned back.
        assert!(recorded(&judge(&mut guard, at(35, after + 300))));
    }

    #[test]
    fn a_death_is_confirmed_while_the_player_keeps_killing() {
        let mut guard = settled(100_000);
        let mut verdict = Verdict::Hold(String::new());
        for exp in [82_600, 82_600, 82_900, 82_900, 83_100] {
            verdict = judge(&mut guard, at(35, exp));
        }
        assert!(matches!(verdict, Verdict::Accept { death: true, .. }));
    }

    #[test]
    fn a_death_down_to_zero_counts_however_small() {
        let mut guard = settled(500);
        let mut verdict = Verdict::Hold(String::new());
        for _ in 0..DROP_CONFIRM_READS {
            verdict = judge(&mut guard, at(35, 0));
        }
        assert!(matches!(verdict, Verdict::Accept { death: true, .. }));
    }

    #[test]
    fn a_misread_baseline_is_replaced_instead_of_stalling() {
        // 50,010 was a misread that got recorded; the true value is 50,000.
        let mut guard = settled(50_010);
        let mut verdict = Verdict::Hold(String::new());
        for _ in 0..DROP_CONFIRM_READS {
            verdict = judge(&mut guard, at(35, 50_000));
        }
        assert_eq!(
            verdict,
            Verdict::Accept {
                state: State::Idle,
                verified: true,
                death: false
            }
        );
        assert!(recorded(&judge(&mut guard, at(35, 50_100))));
    }

    #[test]
    fn an_interrupted_drop_run_starts_over() {
        let mut guard = settled(100_000);
        for _ in 1..DROP_CONFIRM_READS {
            judge(&mut guard, at(35, 82_600));
        }
        guard.miss();
        assert!(matches!(
            judge(&mut guard, at(35, 82_600)),
            Verdict::Hold(_)
        ));
    }

    #[test]
    fn a_lower_level_is_a_misread_for_the_same_character() {
        let mut guard = settled(21_500);
        assert_eq!(
            judge(&mut guard, at(19, 2_106)),
            Verdict::Reject("level fell from 35 to 19".into())
        );
    }

    #[test]
    fn switching_to_a_lower_level_character_starts_fresh() {
        let mut guard = Guard::new();
        for _ in 0..VERIFY_READS {
            judge(&mut guard, named(35, 120_000, "Euroblood"));
        }
        let first = judge(&mut guard, named(12, 900, "Alt"));
        assert!(matches!(first, Verdict::Accept { .. }));
        judge(&mut guard, named(12, 950, "Alt"));
        assert!(recorded(&judge(&mut guard, named(12, 960, "Alt"))));
    }

    #[test]
    fn same_exp_is_idle_and_an_unknown_level_is_held() {
        let mut guard = settled(5_000);
        assert!(matches!(
            judge(&mut guard, at(35, 5_000)),
            Verdict::Accept {
                state: State::Idle,
                ..
            }
        ));
        assert!(matches!(judge(&mut guard, at(0, 0)), Verdict::Hold(_)));
        assert!(!recorded(&judge(&mut guard, at(35, 5_010))));
    }

    #[test]
    fn a_restart_seed_is_compared_against() {
        let mut guard = Guard::new();
        guard.seed(at(35, 21_478));
        assert!(matches!(
            judge(&mut guard, at(35, 21_476)),
            Verdict::Reject(_)
        ));
    }

    #[test]
    fn the_state_is_recorded_in_the_existing_format() {
        assert_eq!(serde_json::to_value(State::Gaining).unwrap(), "gaining");
        assert_eq!(serde_json::to_value(State::Idle).unwrap(), "idle");
    }
}
