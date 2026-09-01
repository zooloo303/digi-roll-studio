//! The Analog Four's TRC menu: one enum where the digis use three fields.
//!
//! A gen-2 box spends three per-step lanes on this — `PROB`, `FILL` and `COND`,
//! independently settable, which [`crate::conditions`] is the codec for. The A4
//! spends **one knob and one lane**, so a trig here is a percentage *or* a fill
//! *or* a logic condition *or* a ratio, never two at once. That is the whole
//! reason this is a separate module rather than a table added to that one: the
//! shapes do not nest, and a translation between them loses things in named
//! ways that callers have to be told about.
//!
//! # How the order was established
//!
//! The lane is `+384`, named on 2026-09-01 by turning the TRC knob and watching
//! which byte moved (PLAN.md §10, "The knobs answer"). The *menu* took a second
//! session, because a byte is an index and an index says nothing about labels.
//!
//! Neil described the front panel first: from a trig with no lock, the first
//! value the knob lands on is `100`; turning left walks down the percentages to
//! `1%` at the far left; turning right goes to `FILL` and on to `8:8` at the far
//! right. Then four trigs of A16 SYN1 were set to named values and read off the
//! wire:
//!
//! ```text
//!   step 1   1%     0x00
//!   step 5   75%    0x0d
//!   step 9   FILL   0x16
//!   step 13  8:8    0x40
//! ```
//!
//! Those four pin the whole table, and they pin it *over-determinedly* — which
//! is what makes this more than a fit:
//!
//! * `FILL` at `0x16` puts exactly **22** entries before it, and Elektron's
//!   probability list is 22 values long. So the percentages are indices
//!   `0x00`–`0x15`, and `100%` — the value the knob lands on first — is `0x15`,
//!   immediately before `FILL`. That is the front panel's own description,
//!   arrived at from the other end.
//! * `75%` at `0x0d` is index **13**, and 75 is the fourteenth value of that
//!   same list. A different percentage list would have had to put 75 in the
//!   same place by coincidence.
//! * `8:8` at `0x40` is index **64**, which lands only if the ratios carry **no
//!   negations**. With the digis' `!A:B` entries interleaved the list would run
//!   to 97. So the A4 has no `!A:B`, just as it has no `LST` — two structural
//!   differences from [`crate::conditions`], not one menu with gaps.
//!
//! # What is still fitted rather than measured
//!
//! Four labels were read off the box. The other 61 are arithmetic from those
//! four plus a list of percentages taken from Elektron's other boxes, and
//! **`percentage_list_is_unverified_between_the_anchors` is the standing note on
//! that**: `1%`, `75%` and `100%`'s position are hardware, and `41%` at `0x09`
//! is a prediction. If a later reading disagrees, the percentages move and the
//! three structural facts above do not.

use crate::conditions::CondGroup;

/// One entry of the A4's TRC menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A4Cond {
    /// `X%` — an x% chance of the trig firing. The A4 folds probability into
    /// the same menu as the conditions, so this is not a separate field.
    Probability(u8),
    /// `FILL`, or `!FILL` when `false`: true while fill mode is active.
    Fill(bool),
    /// `PRE` / `!PRE` — the previous evaluated condition on this track.
    Pre(bool),
    /// `NEI` / `!NEI` — the neighbouring track's.
    Nei(bool),
    /// `1ST` / `!1ST` — only the first pass of the pattern.
    First(bool),
    /// `A:B` — plays on pass A of every B. **No negations**: the A4 has no
    /// `!A:B`, which is what puts `8:8` at the end of a 65-entry menu.
    Ratio(u8, u8),
}

/// Elektron's probability ladder, and the first 22 entries of the menu.
///
/// Two of these are hardware — `1%` at index 0 and `75%` at index 13 — and the
/// length is fixed by `FILL` sitting at `0x16`. The values in between are this
/// ladder as Elektron's other boxes use it.
pub const PERCENTAGES: [u8; 22] =
    [1, 2, 4, 6, 9, 13, 19, 25, 33, 41, 50, 59, 67, 75, 81, 87, 91, 94, 96, 98, 99, 100];

/// The byte a trig with no condition carries — `FF`, the same "unset" the other
/// per-step lanes use.
pub const NONE: u8 = 0xFF;

/// `100%`, the value the knob lands on first from an unlocked trig.
pub const ALWAYS: u8 = 0x15;

/// The last entry, `8:8`. The menu is `0x00`..=`0x40`.
pub const MAX: u8 = 0x40;

/// Where `FILL` sits, and therefore how many percentages precede it.
const FIRST_LOGIC: u8 = PERCENTAGES.len() as u8;
/// Where `1:2` sits: eight logic entries after `FILL`.
const FIRST_RATIO: u8 = FIRST_LOGIC + 8;

/// Read one menu byte. `None` for [`NONE`] and for anything past [`MAX`].
pub fn from_byte(byte: u8) -> Option<A4Cond> {
    if byte == NONE || byte > MAX {
        return None;
    }
    if byte < FIRST_LOGIC {
        return Some(A4Cond::Probability(PERCENTAGES[byte as usize]));
    }
    if byte < FIRST_RATIO {
        // Each logic pair is the positive then its negation, which is the order
        // the manual lists them in and the order the digis use too.
        let (pair, positive) = ((byte - FIRST_LOGIC) / 2, (byte - FIRST_LOGIC) % 2 == 0);
        return Some(match pair {
            0 => A4Cond::Fill(positive),
            1 => A4Cond::Pre(positive),
            2 => A4Cond::Nei(positive),
            _ => A4Cond::First(positive),
        });
    }
    // Ratios, grouped by denominator: 1:2, 2:2, 1:3, 2:3, 3:3, … 8:8.
    let mut n = byte - FIRST_RATIO;
    for b in 2..=8u8 {
        if n < b {
            return Some(A4Cond::Ratio(n + 1, b));
        }
        n -= b;
    }
    None
}

/// The byte for one menu entry, or `None` for a value the menu does not hold —
/// a percentage off the ladder, or a ratio outside `1:2`..`8:8`.
pub fn to_byte(cond: A4Cond) -> Option<u8> {
    match cond {
        A4Cond::Probability(p) => {
            PERCENTAGES.iter().position(|&v| v == p).map(|i| i as u8)
        }
        A4Cond::Fill(positive) => Some(FIRST_LOGIC + u8::from(!positive)),
        A4Cond::Pre(positive) => Some(FIRST_LOGIC + 2 + u8::from(!positive)),
        A4Cond::Nei(positive) => Some(FIRST_LOGIC + 4 + u8::from(!positive)),
        A4Cond::First(positive) => Some(FIRST_LOGIC + 6 + u8::from(!positive)),
        A4Cond::Ratio(a, b) => {
            if !(2..=8).contains(&b) || a < 1 || a > b {
                return None;
            }
            let before: u8 = (2..b).sum();
            Some(FIRST_RATIO + before + (a - 1))
        }
    }
}

/// The nearest percentage the A4 can actually store.
///
/// The ladder is coarse and uneven — 22 stops between 1 and 100 — so a digi's
/// `PROB 55` has no A4 entry. Rounding to the nearest stop is the only
/// behaviour that keeps a cross-box copy playable; the caller counts how often
/// it had to.
pub fn nearest_percentage(prob: u8) -> u8 {
    *PERCENTAGES
        .iter()
        .min_by_key(|&&v| v.abs_diff(prob))
        .expect("the ladder is never empty")
}

/// The label the box shows, for a screen or a log line.
pub fn label(cond: A4Cond) -> String {
    match cond {
        A4Cond::Probability(p) => format!("{p}%"),
        A4Cond::Fill(true) => "FILL".into(),
        A4Cond::Fill(false) => "!FILL".into(),
        A4Cond::Pre(true) => "PRE".into(),
        A4Cond::Pre(false) => "!PRE".into(),
        A4Cond::Nei(true) => "NEI".into(),
        A4Cond::Nei(false) => "!NEI".into(),
        A4Cond::First(true) => "1ST".into(),
        A4Cond::First(false) => "!1ST".into(),
        A4Cond::Ratio(a, b) => format!("{a}:{b}"),
    }
}

/// Which of the digis' COND groups an A4 entry belongs to, where it has one.
///
/// [`A4Cond::Probability`] and [`A4Cond::Fill`] have none: on a gen-2 box those
/// are separate lanes rather than COND entries, which is the asymmetry the
/// whole module exists to carry.
pub fn digi_group(cond: A4Cond) -> Option<CondGroup> {
    match cond {
        A4Cond::Probability(_) | A4Cond::Fill(_) => None,
        A4Cond::Ratio(..) => Some(CondGroup::Ratio),
        _ => Some(CondGroup::Logic),
    }
}

/// The digi COND key for an A4 entry, where one exists — `PRE`, `!1ST`, `3:7`.
///
/// `None` for the two the digis do not put in COND at all (probability and
/// fill, which have their own lanes there) — so a caller translating an A4 trig
/// has to look at all three of this and [`A4Cond::Probability`] and
/// [`A4Cond::Fill`], not just this.
pub fn digi_cond_key(cond: A4Cond) -> Option<String> {
    match cond {
        A4Cond::Probability(_) | A4Cond::Fill(_) => None,
        A4Cond::Pre(p) => Some(negate("PRE", p)),
        A4Cond::Nei(p) => Some(negate("NEI", p)),
        A4Cond::First(p) => Some(negate("1ST", p)),
        A4Cond::Ratio(a, b) => Some(format!("{a}:{b}")),
    }
}

fn negate(key: &str, positive: bool) -> String {
    if positive { key.to_owned() } else { format!("!{key}") }
}

/// A digi COND key as an A4 entry, or `None` where the A4 has no such thing.
///
/// The two gaps are real and named: **`LST`/`!LST` do not exist on the A4**, and
/// neither does any `!A:B` — a negated ratio. Both are in the digis' 76-entry
/// menu and neither can be carried here, so a cross-box copy has to drop them
/// and say so.
pub fn from_digi_cond_key(key: &str) -> Option<A4Cond> {
    let (positive, bare) = match key.strip_prefix('!') {
        Some(rest) => (false, rest),
        None => (true, key),
    };
    match bare {
        "PRE" => Some(A4Cond::Pre(positive)),
        "NEI" => Some(A4Cond::Nei(positive)),
        "1ST" => Some(A4Cond::First(positive)),
        // LST has no A4 equivalent, in either polarity.
        "LST" => None,
        _ => {
            // A ratio — and only a positive one; `!3:4` is a digi entry the A4
            // menu simply does not contain.
            if !positive {
                return None;
            }
            let (a, b) = bare.split_once(':')?;
            Some(A4Cond::Ratio(a.parse().ok()?, b.parse().ok()?))
        }
    }
}
