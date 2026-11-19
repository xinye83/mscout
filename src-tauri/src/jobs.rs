//! The job vocabulary: the closed set of job names the client can draw.
//!
//! A character name is open-ended, so the name line has to be transcribed. A job
//! is not: the game only ever prints one of a small set of strings. Turning the
//! job read into *recognition over a known set* removes the failure mode an
//! unconstrained read has, where a plausible-looking wrong string (`Pqe` for
//! `Page`) is returned with no way to tell it apart from a correct one.
//!
//! This list is deliberately **not** a source of truth the reader relies on to
//! invent text. It is a correction layer applied *after* a read:
//!
//! * an exact (case- and punctuation-insensitive) hit means the reader already
//!   produced a real job,
//! * a near miss snaps to the closest entry, which is how the known `Page` →
//!   `age` leading-capital drop is repaired,
//! * anything further away is left exactly as read and reported as unrecognised,
//!   so a mis-drawn box can never be snapped into a confident wrong job.
//!
//! Provenance: the class tree is MapleStory Classic World's as documented at
//! <https://meowdb.com/msclassic/classes> — five trees (Beginner, Warrior,
//! Magician, Bowman, Thief; there is **no Pirate** in this game), ten second-job
//! branches, and ten third jobs. Third job is not in the initial launch, so those
//! names cannot appear yet; they are listed so a level-up into one reads correctly
//! the day it ships, and because a name missing from this list would be rejected
//! as unrecognised when it first appears on screen.
//!
//! See `docs/SHIPPING.md` and `docs/DECISIONS.md` D33 for why the list ships in
//! the binary rather than being read from the client: the client has no job-name
//! table to read (`map_names::tests::probe_string_jobs` scans every candidate
//! `.wz` and finds none).

/// Every job name MapleStory Classic World can print, in advancement order.
///
/// A first job is chosen at level 10 and a second job at 30; ordering only aids
/// readability, because matching is by edit distance and ties are rejected
/// rather than resolved by position.
pub const JOB_NAMES: &[&str] = &[
    // Starting class
    "Beginner",
    // First job
    "Warrior",
    "Magician",
    "Bowman",
    "Thief",
    // Second job
    "Fighter",
    "Page",
    "Spearman",
    "Wizard (Fire,Poison)",
    "Wizard (Ice,Lightning)",
    "Cleric",
    "Hunter",
    "Crossbowman",
    "Assassin",
    "Bandit",
    // Third job — reserved; not in the initial launch (see the module docs).
    "Crusader",
    "White Knight",
    "Dragon Knight",
    "Mage (Fire,Poison)",
    "Mage (Ice,Lightning)",
    "Priest",
    "Ranger",
    "Sniper",
    "Hermit",
    "Chief Bandit",
];

/// How far a read may sit from a known name and still be treated as that name.
///
/// Two edits covers the realistic damage on a rendered 12–19 px line (one dropped
/// character, one confused glyph, or a doubled letter). It is deliberately tight:
/// a third edit is more likely to be a different real word than a misread of a
/// job, and guessing there is exactly the behaviour this module exists to stop.
const MAX_EDITS: usize = 2;

/// Shortest read that may be snapped at all. A one- or two-letter fragment is
/// ambiguous by construction; snapping `ag` to `Page` would be a guess.
const MIN_INPUT: usize = 3;

/// Levenshtein distance with an early exit once the limit is exceeded.
fn distance_within(a: &str, b: &str, limit: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    if a.len().abs_diff(b.len()) > limit {
        return None;
    }

    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];

    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        let mut row_best = current[0];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
            row_best = row_best.min(current[j + 1]);
        }
        if row_best > limit {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }

    let distance = previous[b.len()];
    (distance <= limit).then_some(distance)
}

/// The elemental qualifiers that distinguish otherwise identical names.
///
/// `Wizard (Fire,Poison)` and `Wizard (Ice,Lightning)` differ only here, so these
/// words are matched as their own tokens and never by whole-line edit distance: a
/// swap of one element for another is a *different job*, not a typo, even though
/// the two strings are only two edits apart.
const ELEMENTS: &[&str] = &["fire", "poison", "ice", "lightning"];

/// A single token's tolerance: a long element may carry a transposition
/// (`lightnign`), while a short one must be near-exact so `ice` cannot drift into
/// `fire`.
fn token_limit(token: &str) -> usize {
    if token.chars().count() >= 7 {
        2
    } else {
        1
    }
}

/// Split a name into its elemental qualifiers and everything else.
///
/// Punctuation is only a separator, so `Wizard (Ice,Lightning)`,
/// `Wizard (Ice, Lightning)` and `Wizard Ice Lightning` all yield the same base
/// and the same element list. Returns `None` when a token is close enough to two
/// different elements to be ambiguous — that is not a reading this module should
/// resolve by guessing.
fn split_name(text: &str) -> Option<(String, Vec<&'static str>)> {
    let mut base = String::new();
    let mut elements: Vec<&'static str> = Vec::new();

    for token in text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let lowered = token.to_lowercase();
        let matches: Vec<&'static str> = ELEMENTS
            .iter()
            .copied()
            .filter(|element| distance_within(&lowered, element, token_limit(element)).is_some())
            .collect();

        match matches.as_slice() {
            [] => base.push_str(&lowered),
            [element] => elements.push(element),
            // Two elements explaining one token equally well: refuse to pick.
            _ => return None,
        }
    }

    Some((base, elements))
}

/// Resolve a read job line against the vocabulary.
///
/// Returns the canonical spelling when the read is a known job or a small
/// misspelling of one, and `None` when it is not close enough to anything —
/// the caller keeps whatever it read and reports it as unrecognised.
pub fn canonical(text: &str) -> Option<&'static str> {
    let (base, elements) = split_name(text)?;
    if base.is_empty() {
        return None;
    }

    if base.chars().count() < MIN_INPUT {
        return None;
    }

    let mut best: Option<(&'static str, usize)> = None;
    let mut tie = false;
    for name in JOB_NAMES {
        let Some((known_base, known_elements)) = split_name(name) else {
            continue;
        };

        // The elements are an exact requirement, not a fuzzy one: a read that
        // names a different element is a different branch. The spell-out variants
        // are already folded away by the token match in `split_name`.
        if elements != known_elements {
            continue;
        }

        let Some(distance) = distance_within(&base, &known_base, MAX_EDITS) else {
            continue;
        };

        // An exact base wins outright, so `Page` never has to beat a neighbour.
        if distance == 0 {
            return Some(name);
        }

        match best {
            Some((_, current)) if distance == current => tie = true,
            Some((_, current)) if distance > current => {}
            _ => {
                best = Some((name, distance));
                tie = false;
            }
        }
    }

    // Two names explaining the read equally well is ambiguity, not a read.
    if tie {
        return None;
    }
    best.map(|(name, _)| name)
}

/// The vocabulary's answer for a job *line*: `Some(canonical)` when the read is a
/// known job or a close misspelling of one, `None` when it is not a job.
///
/// This is the gate the read path applies to text it produced itself, because a
/// misread that resolves to nothing must be reported as unrecognised rather than
/// displayed as a confident class. Text the player taught is a different case and
/// is not passed through here.
pub fn resolve(text: &str) -> Option<String> {
    canonical(text).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: the canonical spelling, or the text unchanged.
    ///
    /// The read path deliberately has no such function any more — it uses
    /// [`resolve`] so an unknown line is rejected. This exists only so the
    /// vocabulary's spelling behaviour can be asserted directly.
    fn normalized(text: &str) -> String {
        resolve(text).unwrap_or_else(|| text.trim().to_string())
    }

    #[test]
    fn exact_names_pass_through_canonically() {
        assert_eq!(normalized("Page"), "Page");
        assert_eq!(normalized("  Page  "), "Page");
        assert_eq!(normalized("page"), "Page");
        assert_eq!(normalized("White Knight"), "White Knight");
        assert_eq!(normalized("white knight"), "White Knight");
    }

    #[test]
    fn the_known_leading_capital_drop_is_repaired() {
        // winocr.rs documents that the Default rendering loses the leading `P`.
        assert_eq!(normalized("age"), "Page");
        assert_eq!(canonical("age"), Some("Page"));
    }

    #[test]
    fn single_glyph_confusions_snap() {
        assert_eq!(canonical("Fighfer"), Some("Fighter"));
        assert_eq!(canonical("Clerlc"), Some("Cleric"));
        assert_eq!(canonical("Assassin"), Some("Assassin"));
    }

    #[test]
    fn elemental_names_match_whatever_punctuation_the_reader_emits() {
        // The game prints `Wizard (Ice,Lightning)`. A reader can drop the
        // parentheses, the comma, or the space after it; all of those are the same
        // job, so punctuation and spacing are ignored on both sides.
        for read in [
            "Wizard (Ice,Lightning)",
            "Wizard (Ice, Lightning)",
            "Wizard Ice Lightning",
            "Wizard Ice,Lightning",
            "wizard (ice,lightning)",
        ] {
            assert_eq!(
                canonical(read),
                Some("Wizard (Ice,Lightning)"),
                "read {read:?}"
            );
        }

        for read in [
            "Mage (Fire,Poison)",
            "Mage (Fire, Poison)",
            "Mage Fire Poison",
        ] {
            assert_eq!(canonical(read), Some("Mage (Fire,Poison)"), "read {read:?}");
        }
    }

    #[test]
    fn the_two_elemental_branches_are_never_confused() {
        // Both branches share the `Wizard`/`Mage` prefix, so a read must not be
        // snapped across them, and a bare prefix must not pick one.
        assert_eq!(
            canonical("Wizard (Fire,Poison)"),
            Some("Wizard (Fire,Poison)")
        );
        assert_eq!(
            canonical("Wizard (Ice,Lightning)"),
            Some("Wizard (Ice,Lightning)")
        );
        assert_ne!(
            canonical("Wizard (Ice,Lightning)"),
            Some("Wizard (Fire,Poison)")
        );

        // An elemental word swapped for the other one is a real difference, not a
        // typo, so it must not be silently rewritten.
        assert_eq!(canonical("Wizard (Ice,Poison)"), None);

        // A bare `Wizard` matches neither branch: there are two, so it is unknown.
        assert_eq!(canonical("Wizard"), None);
    }

    #[test]
    fn unknown_text_is_left_alone() {
        // Not a job at all: must stay untouched, never snapped to a neighbour.
        assert_eq!(normalized("Euroblood"), "Euroblood");
        assert_eq!(canonical("Zzzzzz"), None);
        assert_eq!(canonical(""), None);
    }

    #[test]
    fn short_fragments_are_not_guessed() {
        assert_eq!(canonical("Pa"), None);
        assert_eq!(canonical("ag"), None);
    }

    #[test]
    fn a_distant_word_is_not_forced_onto_a_job() {
        // Three-plus edits away from every entry.
        assert_eq!(canonical("Berserker"), None);
        assert_eq!(canonical("Nightwalker"), None);
    }

    #[test]
    fn classes_this_game_does_not_have_are_not_in_the_vocabulary() {
        // Classic World has no Pirate tree, and no post-Big-Bang heroes. A read of
        // one of these must not be canonicalised into something we do have.
        for absent in [
            "Pirate",
            "Brawler",
            "Gunslinger",
            "Buccaneer",
            "Corsair",
            "Noblesse",
            "Legend",
            "Evan",
            "Citizen",
            "Archer",
        ] {
            assert_eq!(canonical(absent), None, "{absent} is not a Classic job");
        }
    }

    #[test]
    fn vocabulary_is_unique_after_normalisation() {
        // Punctuation and spacing carry no meaning, so two entries that differ only
        // there would be the same job listed twice.
        let mut seen: Vec<(String, Vec<&'static str>)> = Vec::new();
        for name in JOB_NAMES {
            let key = split_name(name).expect("every entry must split cleanly");
            assert!(
                !seen.contains(&key),
                "duplicate job name in the vocabulary: {name}"
            );
            seen.push(key);
        }
    }

    #[test]
    fn every_entry_matches_itself() {
        for name in JOB_NAMES {
            assert_eq!(
                canonical(name),
                Some(*name),
                "entry {name} must match itself"
            );
        }
    }
}
