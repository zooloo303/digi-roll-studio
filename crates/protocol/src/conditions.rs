// The canonical trig-condition tables: PROB, FILL and COND.
//
// Ported from `js/elektron/conditions.js` — one place, consulted by everything
// else. The values are hardware-mapped, not guessed: the [V2] sections of
// digi-roll's `docs/dt2-pattern-format.md` and `docs/dn2-pattern-format.md`
// describe the experiment that produced them. Indices 0–15 were walked one at a
// time on hardware; the rest was confirmed at five anchors (16, 27, 44, 52, 75),
// and the tests below pin those same anchors.
//
// The Digitakt II and Digitone II store all three fields identically: the same
// three per-step byte lanes at the same track-relative offsets, the same
// encodings, and the same COND menu in the same order. So there is exactly one
// table and no per-device keying — and cross-device copy can never have to drop
// a value for lack of a target-side equivalent.
//
// All three are per *trig* (per step), not per note. Notes sharing a step form
// one trig, and digi-roll's rule everywhere is that they carry identical
// prob/fill/cond values (`digi_core::edit_ops::adopt_step_trig` upholds it).
//
// The byte lanes that carry these values in a pattern dump are still unported
// (PLAN.md §6); when they arrive, `cond_from_byte` and friends below are the
// codec they must use.

use std::sync::LazyLock;

/// The byte written into any of the three lanes to mean "nothing stored here".
pub const NONE: u8 = 0xFF;

// --- COND -------------------------------------------------------------------

/// Which heading of the box's COND menu an entry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondGroup {
    /// `PRE`/`NEI`/`1ST`/`LST` and their negations.
    Logic,
    /// `A:B` — plays on pass A of every B.
    Ratio,
    /// `!A:B` — plays on every pass of B except A.
    NotRatio,
}

/// One entry of the COND menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CondEntry {
    /// The canonical label, as `Note::cond` stores it: `PRE`, `!1ST`, `3:7`…
    pub key: String,
    pub group: CondGroup,
    /// `Some((a, b))` for the two ratio groups, `None` for logic.
    pub ab: Option<(u8, u8)>,
    /// The stored byte — the zero-based index into the menu.
    pub value: u8,
}

/// The box's own menu order, which *is* the stored encoding: the byte is the
/// zero-based index into this list.
///
/// The order rule, read off the hardware: the four logic pairs first, each
/// negation immediately after its positive; then the ratios grouped by
/// denominator, again with negations interleaved. The `:2` group carries no
/// negations, because `!1:2` would just be `2:2`.
fn build_cond_list() -> Vec<CondEntry> {
    let mut list: Vec<(String, CondGroup, Option<(u8, u8)>)> = Vec::new();
    for key in ["PRE", "!PRE", "NEI", "!NEI", "1ST", "!1ST", "LST", "!LST"] {
        list.push((key.to_owned(), CondGroup::Logic, None));
    }
    // The :2 group is written without negations — `!1:2` would just be `2:2`.
    list.push(("1:2".to_owned(), CondGroup::Ratio, Some((1, 2))));
    list.push(("2:2".to_owned(), CondGroup::Ratio, Some((2, 2))));
    for b in 3..=8 {
        for a in 1..=b {
            list.push((format!("{a}:{b}"), CondGroup::Ratio, Some((a, b))));
            list.push((format!("!{a}:{b}"), CondGroup::NotRatio, Some((a, b))));
        }
    }
    list.into_iter()
        .enumerate()
        .map(|(value, (key, group, ab))| CondEntry {
            key,
            group,
            ab,
            value: value as u8,
        })
        .collect()
}

static CONDITIONS: LazyLock<Vec<CondEntry>> = LazyLock::new(build_cond_list);

/// The full COND menu: 76 entries, `PRE` = 0 … `!8:8` = 75.
pub fn conditions() -> &'static [CondEntry] {
    &CONDITIONS
}

fn by_key(key: &str) -> Option<&'static CondEntry> {
    CONDITIONS.iter().find(|c| c.key == key)
}

/// One tab of the trig lane's picker: every condition on denominator `b`, in
/// menu order — positives and negations interleaved, exactly as the box lists
/// them.
#[derive(Debug)]
pub struct DenomGroup {
    pub b: u8,
    pub items: Vec<&'static CondEntry>,
}

static BY_DENOMINATOR: LazyLock<Vec<DenomGroup>> = LazyLock::new(|| {
    (2..=8)
        .map(|b| DenomGroup {
            b,
            items: CONDITIONS
                .iter()
                .filter(|c| c.ab.map(|(_, denom)| denom) == Some(b))
                .collect(),
        })
        .collect()
});

/// Ratios split by denominator — what the trig lane's picker offers as tabs.
pub fn cond_by_denominator() -> &'static [DenomGroup] {
    &BY_DENOMINATOR
}

/// Stored byte → canonical label, or `None` for "none".
///
/// Unknown values decode to `None` rather than erroring, exactly as the JS
/// warns and carries on: a future OS could extend the menu, and a pattern we
/// cannot fully read must still open.
pub fn cond_from_byte(byte: u8) -> Option<&'static str> {
    if byte == NONE {
        return None;
    }
    CONDITIONS.get(byte as usize).map(|c| c.key.as_str())
}

/// Canonical label → stored byte. `None` and `""` → the "none" sentinel.
///
/// An unknown label is `Err`: it is a programming error rather than device
/// data — the JS throws here — and the write path must refuse to encode it.
pub fn cond_to_byte(key: Option<&str>) -> Result<u8, UnknownCondition> {
    match key {
        None | Some("") => Ok(NONE),
        Some(k) => by_key(k)
            .map(|c| c.value)
            .ok_or_else(|| UnknownCondition(k.to_owned())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCondition(pub String);

impl std::fmt::Display for UnknownCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown trig condition {:?}", self.0)
    }
}

impl std::error::Error for UnknownCondition {}

/// The menu's own spelling of a condition, or `None` for one no box has.
///
/// What a write path needs and [`cond_to_byte`] cannot give it: the model holds
/// a condition as an owned `String` and [`crate::trig_cond::TrigSetting`] holds
/// the `'static` label off this table, so a caller crossing that seam has to
/// look the string up rather than leak it. Going via the byte and back would
/// work and reads as an accident.
pub fn cond_key(key: &str) -> Option<&'static str> {
    by_key(key).map(|c| c.key.as_str())
}

pub fn is_cond_key(key: &str) -> bool {
    cond_key(key).is_some()
}

/// What a condition means, for tooltips. Empty for an unknown key.
pub fn cond_description(key: &str) -> String {
    let Some(c) = by_key(key) else {
        return String::new();
    };
    let neg = c.key.starts_with('!');
    match c.ab {
        None => {
            let base = match c.key.trim_start_matches('!') {
                "PRE" => "the previous trig with a condition on this track evaluated true",
                "NEI" => "the previous trig with a condition on the neighbour track evaluated true",
                "1ST" => "this is the first loop of the pattern",
                "LST" => "this is the last loop of the pattern before it changes",
                _ => return String::new(),
            };
            format!("Plays when {}{base}", if neg { "NOT " } else { "" })
        }
        Some((a, b)) => {
            if neg {
                format!("Plays on every loop of {b} EXCEPT loop {a}")
            } else {
                format!("Plays on loop {a} of every {b} loops")
            }
        }
    }
}

// --- PROB -------------------------------------------------------------------

// The byte is the percentage itself, 0–100. `FF` means no lock, i.e. the track
// default. Note that an explicit 100% lock (`0x64`) is a real, distinct stored
// value — the box writes it when you dial a trig's PROB up to 100 — so decode
// keeps them apart and only the UI collapses 100 to "no lock".

pub const PROB_MIN: u8 = 0;
pub const PROB_MAX: u8 = 100;

/// `None` for the sentinel and for out-of-range values, which the JS also
/// treats (with a warning) as no lock.
pub fn prob_from_byte(byte: u8) -> Option<u8> {
    if byte == NONE || byte > PROB_MAX {
        return None;
    }
    Some(byte)
}

pub fn prob_to_byte(prob: Option<u8>) -> u8 {
    match prob {
        None => NONE,
        Some(p) => p.min(PROB_MAX),
    }
}

// --- FILL -------------------------------------------------------------------

// Tri-state, not a boolean: the box distinguishes "no lock" from an explicit
// OFF, and there is no track-level FILL for an unlocked trig to fall back to.
//
//   None         no lock — the trig ignores fill mode entirely
//   Some(true)   ON  — plays only while FILL is held
//   Some(false)  OFF — does not play while FILL is held

pub const FILL_OFF_BYTE: u8 = 0x00;
pub const FILL_ON_BYTE: u8 = 0x01;

/// Anything that is neither ON nor OFF — including the sentinel — is no lock,
/// exactly as the JS decodes it.
pub fn fill_from_byte(byte: u8) -> Option<bool> {
    match byte {
        FILL_ON_BYTE => Some(true),
        FILL_OFF_BYTE => Some(false),
        _ => None,
    }
}

pub fn fill_to_byte(fill: Option<bool>) -> u8 {
    match fill {
        None => NONE,
        Some(true) => FILL_ON_BYTE,
        Some(false) => FILL_OFF_BYTE,
    }
}

#[cfg(test)]
mod tests {
    // Every expected value below was derived by running the JS oracle
    // (`js/elektron/conditions.js`) under node, not by re-deriving the rule —
    // the same discipline every other ported table in this crate follows.
    use super::*;

    #[test]
    fn the_menu_is_seventy_six_entries_from_pre_to_not_eight_of_eight() {
        let list = conditions();
        assert_eq!(list.len(), 76);
        assert_eq!(list[0].key, "PRE");
        assert_eq!(list[75].key, "!8:8");
    }

    #[test]
    fn the_hardware_anchors_hold() {
        // 0–15 were walked one at a time on hardware; these five anchors are
        // where the rest of the table was confirmed.
        for (value, key) in [(16, "1:4"), (27, "!2:5"), (44, "6:6"), (52, "4:7"), (75, "!8:8")] {
            assert_eq!(conditions()[value].key, key, "anchor {value}");
        }
        // And the seam where logic ends and the negation-free :2 group starts.
        assert_eq!(conditions()[7].key, "!LST");
        assert_eq!(conditions()[8].key, "1:2");
        assert_eq!(conditions()[9].key, "2:2");
        assert_eq!(conditions()[10].key, "1:3");
    }

    #[test]
    fn every_negation_follows_its_positive_and_the_two_group_has_none() {
        for c in conditions() {
            if let Some(stripped) = c.key.strip_prefix('!') {
                let positive = &conditions()[c.value as usize - 1];
                assert_eq!(positive.key, stripped, "{} sits right after its positive", c.key);
            }
        }
        assert!(
            conditions().iter().filter(|c| c.ab.map(|(_, b)| b) == Some(2)).all(|c| c.group == CondGroup::Ratio),
            "!1:2 would just be 2:2"
        );
    }

    #[test]
    fn every_entry_round_trips_through_its_byte() {
        for c in conditions() {
            assert_eq!(cond_to_byte(Some(&c.key)), Ok(c.value));
            assert_eq!(cond_from_byte(c.value), Some(c.key.as_str()));
            assert!(is_cond_key(&c.key));
        }
    }

    #[test]
    fn none_and_the_unknown_decode_to_no_condition() {
        assert_eq!(cond_from_byte(NONE), None);
        // One past the menu: a future OS could extend it, and a pattern we
        // cannot fully read must still open.
        assert_eq!(cond_from_byte(76), None);

        assert_eq!(cond_to_byte(None), Ok(NONE));
        assert_eq!(cond_to_byte(Some("")), Ok(NONE));
        assert_eq!(
            cond_to_byte(Some("9:9")),
            Err(UnknownCondition("9:9".to_owned())),
            "an unknown label is a programming error, not device data"
        );
    }

    #[test]
    fn the_denominator_tabs_carry_the_menu_order() {
        let tabs = cond_by_denominator();
        // (b, count, first, last) — read off the JS oracle.
        let expected = [
            (2, 2, "1:2", "2:2"),
            (3, 6, "1:3", "!3:3"),
            (4, 8, "1:4", "!4:4"),
            (5, 10, "1:5", "!5:5"),
            (6, 12, "1:6", "!6:6"),
            (7, 14, "1:7", "!7:7"),
            (8, 16, "1:8", "!8:8"),
        ];
        assert_eq!(tabs.len(), expected.len());
        for (tab, (b, count, first, last)) in tabs.iter().zip(expected) {
            assert_eq!(tab.b, b);
            assert_eq!(tab.items.len(), count);
            assert_eq!(tab.items[0].key, first);
            assert_eq!(tab.items[count - 1].key, last);
        }
    }

    #[test]
    fn descriptions_match_the_oracle_word_for_word() {
        for (key, want) in [
            ("PRE", "Plays when the previous trig with a condition on this track evaluated true"),
            ("!1ST", "Plays when NOT this is the first loop of the pattern"),
            ("LST", "Plays when this is the last loop of the pattern before it changes"),
            ("1:2", "Plays on loop 1 of every 2 loops"),
            ("3:7", "Plays on loop 3 of every 7 loops"),
            ("!3:7", "Plays on every loop of 7 EXCEPT loop 3"),
        ] {
            assert_eq!(cond_description(key), want);
        }
        assert_eq!(cond_description("bogus"), "");
    }

    #[test]
    fn prob_bytes_round_trip_and_the_lock_at_one_hundred_is_real() {
        assert_eq!(prob_from_byte(NONE), None);
        assert_eq!(prob_from_byte(101), None, "out of range is no lock, as the JS decodes it");
        // An explicit 100% lock is a distinct stored value, not "no lock".
        assert_eq!(prob_from_byte(100), Some(100));
        assert_eq!(prob_from_byte(0), Some(0));

        assert_eq!(prob_to_byte(None), NONE);
        assert_eq!(prob_to_byte(Some(100)), 100);
        assert_eq!(prob_to_byte(Some(255)), 100, "clamped, as the JS clamps");
    }

    #[test]
    fn fill_bytes_round_trip_through_the_tri_state() {
        for fill in [None, Some(true), Some(false)] {
            assert_eq!(fill_from_byte(fill_to_byte(fill)), fill);
        }
        assert_eq!(fill_from_byte(NONE), None);
        assert_eq!(fill_from_byte(0x02), None, "unknown is no lock, as the JS decodes it");
    }
}
