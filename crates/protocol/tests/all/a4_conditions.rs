//! The A4's TRC menu, against the four labels read off the box.

use digi_protocol::a4_conditions::{
    digi_cond_key, from_byte, from_digi_cond_key, label, nearest_percentage, to_byte, A4Cond,
    ALWAYS, MAX, NONE, PERCENTAGES,
};

/// **The four anchors.** Neil set these on A16 SYN1 on 2026-09-01 and named
/// each on the front panel; the bytes came off the wire. Everything else in
/// this module is arithmetic from them, so if one of these ever fails the table
/// is not wrong at the edges — it is wrong.
#[test]
fn the_four_labels_read_off_the_box() {
    assert_eq!(label(from_byte(0x00).unwrap()), "1%", "step 1, far left");
    assert_eq!(label(from_byte(0x0d).unwrap()), "75%", "step 5");
    assert_eq!(label(from_byte(0x16).unwrap()), "FILL", "step 9");
    assert_eq!(label(from_byte(0x40).unwrap()), "8:8", "step 13, far right");
}

/// The front panel's own description, arrived at from the byte side: an
/// unlocked trig's knob lands first on `100`, and one click right is `FILL`.
/// Nothing was fitted to make that true — it falls out of `FILL` being at
/// `0x16` with 22 percentages before it.
#[test]
fn the_knob_lands_on_a_hundred_percent_and_one_click_right_is_fill() {
    assert_eq!(label(from_byte(ALWAYS).unwrap()), "100%");
    assert_eq!(label(from_byte(ALWAYS + 1).unwrap()), "FILL");
    assert_eq!(ALWAYS as usize, PERCENTAGES.len() - 1);
}

/// The menu ends at `8:8` and nothing follows it. `0x41` is not a condition,
/// and a byte the box never writes must not decode as one.
#[test]
fn the_menu_stops_at_sixty_five_entries() {
    assert_eq!(MAX, 0x40);
    assert!(from_byte(MAX).is_some());
    assert!(from_byte(MAX + 1).is_none(), "past the end of the menu");
    assert!(from_byte(NONE).is_none(), "FF is no condition, not a condition");
}

/// Every byte in range round-trips through the table, which is the check that
/// the three segments abut exactly — a percentage list one short, or a logic
/// pair miscounted, would show up as a byte that decodes to something whose
/// encoding is a different byte.
#[test]
fn every_byte_round_trips_and_the_segments_abut() {
    for byte in 0..=MAX {
        let cond = from_byte(byte).unwrap_or_else(|| panic!("{byte:#04x} decoded to nothing"));
        assert_eq!(to_byte(cond), Some(byte), "{byte:#04x} is {}", label(cond));
    }
    // And the count is exactly the menu: 22 percentages, 8 logic, 35 ratios.
    assert_eq!(22 + 8 + 35, MAX as usize + 1);
}

/// The ratio segment has **no negations**, which is what puts `8:8` at `0x40`
/// rather than somewhere past a hundred. With the digis' `!A:B` interleaved the
/// menu would run to 97 entries, and the box says otherwise.
#[test]
fn the_ratios_carry_no_negations_which_is_why_the_menu_ends_where_it_does() {
    let ratios: Vec<String> = (0x1e..=MAX).map(|b| label(from_byte(b).unwrap())).collect();
    assert_eq!(ratios.first().map(String::as_str), Some("1:2"));
    assert_eq!(ratios.last().map(String::as_str), Some("8:8"));
    assert_eq!(ratios.len(), 35, "2+3+4+5+6+7+8");
    assert!(!ratios.iter().any(|r| r.starts_with('!')), "no !A:B on this box");
}

/// The eight logic entries, each negation immediately after its positive.
#[test]
fn the_logic_block_is_four_pairs_and_fill_is_one_of_them() {
    let logic: Vec<String> = (0x16..0x1e).map(|b| label(from_byte(b).unwrap())).collect();
    assert_eq!(logic, ["FILL", "!FILL", "PRE", "!PRE", "NEI", "!NEI", "1ST", "!1ST"]);
    // FILL being *in* the menu is the structural difference from the digis,
    // where it is a lane of its own that can be set alongside a COND.
    assert_eq!(from_byte(0x16), Some(A4Cond::Fill(true)));
}

/// **The two things the digis have and this box does not.** `LST` is absent
/// from the A4's manual and from its menu, and so is every negated ratio — so a
/// cross-box copy has to drop them rather than find a near equivalent.
#[test]
fn lst_and_negated_ratios_have_no_a4_equivalent() {
    assert_eq!(from_digi_cond_key("LST"), None);
    assert_eq!(from_digi_cond_key("!LST"), None);
    assert_eq!(from_digi_cond_key("!3:4"), None);
    // While the ones that do exist survive the trip both ways.
    for key in ["PRE", "!PRE", "NEI", "!NEI", "1ST", "!1ST", "1:2", "2:2", "3:7", "8:8"] {
        let cond = from_digi_cond_key(key).unwrap_or_else(|| panic!("{key} should exist"));
        assert_eq!(digi_cond_key(cond).as_deref(), Some(key));
        assert!(to_byte(cond).is_some(), "{key} has a byte");
    }
}

/// Probability and fill are not COND entries on a digi — they are their own
/// lanes there — so translating an A4 trig has to look past `digi_cond_key`.
/// It returning `None` for those two is the signal, not a failure.
#[test]
fn probability_and_fill_are_not_cond_entries_on_a_digi() {
    assert_eq!(digi_cond_key(A4Cond::Probability(50)), None);
    assert_eq!(digi_cond_key(A4Cond::Fill(true)), None);
    assert_eq!(digi_cond_key(A4Cond::Pre(true)).as_deref(), Some("PRE"));
}

/// The ladder is coarse and uneven, so most digi PROB values have no A4 entry
/// and have to round. The ends do not move, and the middle rounds to the
/// nearest rung rather than the nearest multiple of anything.
#[test]
fn a_probability_off_the_ladder_rounds_to_the_nearest_rung() {
    assert_eq!(nearest_percentage(1), 1);
    assert_eq!(nearest_percentage(100), 100);
    assert_eq!(nearest_percentage(50), 50, "on the ladder already");
    assert_eq!(nearest_percentage(55), 59, "between 50 and 59, and 59 is nearer");
    assert_eq!(nearest_percentage(54), 50);
    assert_eq!(nearest_percentage(3), 2, "the bottom of the ladder is fine-grained");
    // And a value that is on the ladder encodes; one that is not, does not.
    assert!(to_byte(A4Cond::Probability(41)).is_some());
    assert_eq!(to_byte(A4Cond::Probability(42)), None, "42% is not a rung");
}

/// A ratio the menu does not hold is refused rather than encoded to something
/// adjacent — `9:9` is not a condition, and writing `0x41` for it would put a
/// byte on the box that the box has never written.
#[test]
fn a_ratio_outside_the_menu_is_refused() {
    assert_eq!(to_byte(A4Cond::Ratio(9, 9)), None);
    assert_eq!(to_byte(A4Cond::Ratio(1, 1)), None, "there is no 1:1");
    assert_eq!(to_byte(A4Cond::Ratio(3, 2)), None, "nor an A greater than B");
    assert_eq!(to_byte(A4Cond::Ratio(1, 8)), Some(0x39));
}


/// The four anchors again, but through the *pattern* rather than the table:
/// the capture Neil made with those four trigs set, decoded end to end.
///
/// The table test above could pass on a table that was internally consistent
/// and had nothing to do with the box. This one cannot.
#[test]
fn the_capture_of_the_four_labelled_trigs_decodes_to_what_the_screen_said() {
    let pattern = crate::common::a4_working_pattern("analogfour-A16-conditions-2026-09-01.syx");
    let trigs = digi_protocol::a4_pattern::read_track_trigs(&pattern.payload, 0).expect("SYN1");
    let at = |step: usize| {
        trigs
            .iter()
            .find(|t| t.step == step)
            .and_then(|t| t.condition)
            .and_then(from_byte)
            .map(label)
    };
    assert_eq!(at(1).as_deref(), Some("1%"), "far left");
    assert_eq!(at(5).as_deref(), Some("75%"));
    assert_eq!(at(9).as_deref(), Some("FILL"));
    assert_eq!(at(13).as_deref(), Some("8:8"), "far right");
    // And the steps between them carry nothing, so no lane is being read one
    // step wide of itself.
    assert_eq!(at(2), None);
    assert_eq!(at(12), None);
}
