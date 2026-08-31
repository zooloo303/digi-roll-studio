//! Trig conditions, ported from `test/shouldplay.test.js`.
//!
//! Every expectation in the first half was read out of the JS original first
//! (`node --input-type=module -e` against `~/Projects/digi-roll/js/midi.js`) and
//! then written down here, so it pins digi-roll's behaviour rather than this
//! port's output.
//!
//! The second half has no JS oracle and says so where it starts: `PRE`, `NEI` and
//! `FILL` are conditions the browser explicitly could not evaluate, and PLAN.md
//! §4 is the specification for what this engine does with them instead — the same
//! footing `Session::bind_identity`'s tests are on.
//!
//! **One JS case is deliberately superseded, not dropped.** `ignores fill
//! entirely — there is no FILL button here` asserts that a FILL-locked trig plays
//! regardless. There *is* a FILL button here, so that expectation is now wrong on
//! purpose; `fill_is_simulated_here_unlike_the_browser` below replaces it and
//! shows both toggle states.

use digi_engine::conditions::{should_play, CondContext, CondHistory, TrigOutcome};
use digi_engine::rng::{Rng, ScriptedRng};

/// The JS `note()` helper: no prob lock, no fill, no condition.
fn plays(prob: Option<u8>, cond: Option<&str>, loop_index: u64, rng: &mut impl Rng) -> bool {
    play_at(prob, None, cond, 100, loop_index, false, rng)
}

/// Full form, including the track-level PROB default and the FILL toggle.
fn play_at(
    prob: Option<u8>,
    fill: Option<bool>,
    cond: Option<&str>,
    track_prob: u8,
    loop_index: u64,
    fill_active: bool,
    rng: &mut impl Rng,
) -> bool {
    let ctx = CondContext {
        loop_index,
        fill_active,
        ..Default::default()
    };
    should_play(prob, fill, cond, track_prob, &ctx, rng).plays
}

fn always() -> ScriptedRng {
    ScriptedRng::always()
}

fn never() -> ScriptedRng {
    ScriptedRng::never()
}

fn at(v: f64) -> ScriptedRng {
    ScriptedRng::new(&[v])
}

// --- an unlocked note --------------------------------------------------------

#[test]
fn an_unlocked_note_always_plays() {
    for loop_index in [0, 1, 2, 7] {
        assert!(plays(None, None, loop_index, &mut never()), "loop {loop_index}");
    }
}

// --- probability -------------------------------------------------------------

#[test]
fn plays_when_the_roll_comes_in_under_the_odds() {
    assert!(plays(Some(50), None, 0, &mut at(0.49)));
}

#[test]
fn is_silenced_when_the_roll_comes_in_over() {
    assert!(!plays(Some(50), None, 0, &mut at(0.5)));
}

#[test]
fn never_plays_at_0_and_always_plays_at_100() {
    assert!(!plays(Some(0), None, 0, &mut at(0.0)));
    assert!(plays(Some(100), None, 0, &mut at(0.999)));
}

#[test]
fn probability_combines_with_a_condition_rather_than_replacing_it() {
    // 2:4 is false on loop 0, so it stays silent even with the odds passing.
    assert!(!plays(Some(100), Some("2:4"), 0, &mut always()));
    assert!(plays(Some(100), Some("2:4"), 1, &mut always()));
    // ...and passes the condition but fails the dice.
    assert!(!plays(Some(50), Some("2:4"), 1, &mut never()));
}

// --- the track-level PROB default --------------------------------------------

#[test]
fn the_track_default_is_what_a_trig_with_no_lock_runs_at() {
    assert!(play_at(None, None, None, 30, 0, false, &mut at(0.29)));
    assert!(!play_at(None, None, None, 30, 0, false, &mut at(0.30)));
}

#[test]
fn the_track_default_is_overridden_by_an_explicit_lock_in_either_direction() {
    // The user's case: a 30% track with a few trigs pinned at 100.
    assert!(play_at(Some(100), None, None, 30, 0, false, &mut never()));
    assert!(!play_at(Some(0), None, None, 100, 0, false, &mut always()));
}

#[test]
fn the_track_default_defaults_to_always() {
    assert!(plays(None, None, 0, &mut never()));
    assert!(play_at(None, None, None, 100, 0, false, &mut never()));
}

#[test]
fn the_track_default_silences_an_unlocked_trig_entirely_at_0() {
    assert!(!play_at(None, None, None, 0, 0, false, &mut at(0.0)));
}

#[test]
fn the_track_default_still_lets_the_condition_have_its_say() {
    // Track odds pass, but 2:4 is false on loop 0.
    assert!(!play_at(None, None, Some("2:4"), 50, 0, false, &mut always()));
    assert!(play_at(None, None, Some("2:4"), 50, 1, false, &mut always()));
}

/// Not in the JS suite, but implied by every test in it that passes `always`:
/// the draw happens whatever the odds are. Pinning it is what stops someone
/// "optimising" the 100% case and silently changing every seeded run.
#[test]
fn the_rng_is_drawn_exactly_once_whatever_the_odds() {
    for (prob, track_prob) in [(None, 100), (Some(100), 100), (Some(0), 100), (None, 0)] {
        let mut rng = ScriptedRng::always();
        play_at(prob, None, Some("PRE"), track_prob, 0, false, &mut rng);
        assert_eq!(rng.draws, 1, "prob {prob:?} / track {track_prob}");
    }
}

// --- ratio conditions --------------------------------------------------------

fn ratio_plays(cond: &str, loops: &[u64]) -> Vec<u64> {
    loops
        .iter()
        .copied()
        .filter(|&l| plays(None, Some(cond), l, &mut always()))
        .collect()
}

#[test]
fn plays_1_of_2_on_every_other_loop_starting_with_the_first() {
    assert_eq!(ratio_plays("1:2", &[0, 1, 2, 3, 4, 5]), vec![0, 2, 4]);
}

#[test]
fn plays_2_of_2_on_the_off_loops() {
    assert_eq!(ratio_plays("2:2", &[0, 1, 2, 3, 4, 5]), vec![1, 3, 5]);
}

#[test]
fn plays_2_of_4_on_loop_2_of_every_4() {
    assert_eq!(ratio_plays("2:4", &[0, 1, 2, 3, 4, 5, 6, 7]), vec![1, 5]);
}

#[test]
fn plays_a_negated_ratio_on_exactly_the_loops_the_positive_skips() {
    let loops: Vec<u64> = (0..8).collect();
    for cond in ["1:2", "2:4", "3:5", "8:8"] {
        let yes = ratio_plays(cond, &loops);
        let no = ratio_plays(&format!("!{cond}"), &loops);
        assert!(
            !yes.iter().any(|l| no.contains(l)),
            "{cond}: {yes:?} overlaps {no:?}"
        );
        assert_eq!(yes.len() + no.len(), loops.len(), "{cond}");
    }
}

#[test]
fn plays_full_cycle_conditions_on_the_last_loop_of_every_cycle() {
    // The JS names this "1:1-style full-cycle conditions every loop" and checks
    // loops 7, 15 and 23 — which are precisely the last of each cycle of 8, not
    // every loop. The expectation is the JS's; the name here says what it means.
    assert_eq!(ratio_plays("8:8", &[7, 15, 23]), vec![7, 15, 23]);
    assert_eq!(ratio_plays("8:8", &[0, 1, 2, 3, 4, 5, 6, 7]), vec![7]);
}

// --- first-loop conditions ---------------------------------------------------

#[test]
fn plays_1st_only_on_the_first_pass() {
    assert!(plays(None, Some("1ST"), 0, &mut always()));
    assert!(!plays(None, Some("1ST"), 1, &mut always()));
}

#[test]
fn plays_not_1st_on_every_pass_but_the_first() {
    assert!(!plays(None, Some("!1ST"), 0, &mut always()));
    assert!(plays(None, Some("!1ST"), 3, &mut always()));
}

// --- what stays unsimulated --------------------------------------------------

#[test]
fn plays_lst_rather_than_guessing() {
    // LST needs to know a pattern change is coming, which needs chaining. Until
    // then it plays, so the sequencer is never quieter than the box.
    for cond in ["LST", "!LST"] {
        for loop_index in [0, 1, 5] {
            assert!(plays(None, Some(cond), loop_index, &mut always()), "{cond} @ {loop_index}");
        }
    }
}

#[test]
fn plays_a_condition_string_it_does_not_recognise() {
    for cond in ["", "WAT", "3:", ":4", "a:b", "1:2:3", "1ST!"] {
        assert!(plays(None, Some(cond), 3, &mut always()), "{cond:?}");
    }
}

#[test]
fn still_applies_probability_to_a_note_whose_condition_it_cannot_evaluate() {
    assert!(!plays(Some(50), Some("LST"), 0, &mut never()));
}

// --- PLAN.md §4's extensions. No JS oracle beyond this line. -----------------
//
// `js/midi.js` could not evaluate PRE, NEI or FILL and said so. The three facts
// that changed: this engine plays a whole box of tracks at once, keeps a
// per-track history of condition results, and has a FILL control in the
// transport. What follows is specified from PLAN.md §4, not derived from the JS.

#[test]
fn fill_is_simulated_here_unlike_the_browser() {
    // Supersedes the JS's `ignores fill entirely`. ON plays only while FILL is
    // held; OFF plays only while it is not.
    assert!(play_at(None, Some(true), None, 100, 0, true, &mut always()));
    assert!(!play_at(None, Some(true), None, 100, 0, false, &mut always()));
    assert!(!play_at(None, Some(false), None, 100, 0, true, &mut always()));
    assert!(play_at(None, Some(false), None, 100, 0, false, &mut always()));
    // No FILL lock: the toggle is irrelevant.
    for fill_active in [true, false] {
        assert!(play_at(None, None, None, 100, 0, fill_active, &mut always()));
    }
}

#[test]
fn fill_does_not_rescue_a_trig_its_odds_silenced() {
    // FILL held and the lock says ON, so the fill gate passes — and the trig is
    // still silent, because the dice came first. (`never` is 0.999, which passes
    // at 100%; the odds have to be under it for the roll to bite.)
    assert!(!play_at(Some(50), Some(true), None, 100, 0, true, &mut never()));
}

#[test]
fn pre_reads_the_last_condition_result_on_this_track() {
    let ctx = |prev| CondContext {
        prev_on_track: prev,
        ..Default::default()
    };
    assert!(should_play(None, None, Some("PRE"), 100, &ctx(Some(true)), &mut always()).plays);
    assert!(!should_play(None, None, Some("PRE"), 100, &ctx(Some(false)), &mut always()).plays);
    assert!(!should_play(None, None, Some("!PRE"), 100, &ctx(Some(true)), &mut always()).plays);
    assert!(should_play(None, None, Some("!PRE"), 100, &ctx(Some(false)), &mut always()).plays);
}

#[test]
fn pre_plays_before_this_track_has_evaluated_any_condition() {
    // Nothing to consult is not the same as false. The rule holds: unsimulated
    // plays, in both polarities.
    for cond in ["PRE", "!PRE"] {
        let out = should_play(None, None, Some(cond), 100, &CondContext::default(), &mut always());
        assert!(out.plays, "{cond}");
        assert_eq!(out.cond_result, None, "{cond} must not enter the history");
    }
}

#[test]
fn nei_reads_the_neighbour_track_not_this_one() {
    let ctx = CondContext {
        prev_on_track: Some(false),
        prev_on_neighbour: Some(true),
        ..Default::default()
    };
    assert!(should_play(None, None, Some("NEI"), 100, &ctx, &mut always()).plays);
    assert!(!should_play(None, None, Some("PRE"), 100, &ctx, &mut always()).plays);
}

#[test]
fn nei_on_track_1_has_no_neighbour_and_plays() {
    let history = CondHistory::new(16);
    let ctx = history.context_for(0, 0, false);
    assert_eq!(ctx.prev_on_neighbour, None);
    for cond in ["NEI", "!NEI"] {
        assert!(should_play(None, None, Some(cond), 100, &ctx, &mut always()).plays, "{cond}");
    }
}

// --- the history itself ------------------------------------------------------

#[test]
fn only_a_condition_that_resolved_enters_the_history() {
    let mut h = CondHistory::new(4);
    // An unconditional trig leaves the slot alone...
    h.record(0, TrigOutcome { plays: true, cond_result: None });
    assert_eq!(h.context_for(0, 0, false).prev_on_track, None);
    // ...a resolved one sets it...
    h.record(0, TrigOutcome { plays: false, cond_result: Some(false) });
    assert_eq!(h.context_for(0, 0, false).prev_on_track, Some(false));
    // ...and a later unconditional trig does not clear what is there.
    h.record(0, TrigOutcome { plays: true, cond_result: None });
    assert_eq!(h.context_for(0, 0, false).prev_on_track, Some(false));
}

#[test]
fn the_history_records_the_condition_not_whether_the_note_sounded() {
    // A trig whose condition came out true but whose odds silenced it still
    // feeds a later PRE with `true`: PROB and COND are separate lanes on both
    // boxes, and PRE reads COND.
    let mut h = CondHistory::new(2);
    let out = should_play(Some(0), None, Some("1ST"), 100, &CondContext::default(), &mut always());
    assert!(!out.plays, "0% odds must silence it");
    assert_eq!(out.cond_result, Some(true), "but 1ST was true on loop 0");
    h.record(0, out);
    assert_eq!(h.context_for(0, 0, false).prev_on_track, Some(true));
}

#[test]
fn a_history_is_per_device_and_sized_from_its_track_count() {
    let mut h = CondHistory::new(4);
    h.record(0, TrigOutcome { plays: true, cond_result: Some(true) });
    // Track 1's neighbour is track 0; track 2's is track 1, which is still unset.
    assert_eq!(h.context_for(1, 0, false).prev_on_neighbour, Some(true));
    assert_eq!(h.context_for(2, 0, false).prev_on_neighbour, None);
    // Out of range on a 4-track box is not a panic — a 4-track A4 profile must
    // work without model surgery (PLAN.md §2).
    h.record(9, TrigOutcome { plays: true, cond_result: Some(true) });
    assert_eq!(h.context_for(9, 0, false).prev_on_track, None);
}

#[test]
fn clearing_the_history_makes_pre_play_again() {
    let mut h = CondHistory::new(2);
    h.record(0, TrigOutcome { plays: false, cond_result: Some(false) });
    assert_eq!(h.context_for(0, 0, false).prev_on_track, Some(false));
    h.clear();
    assert_eq!(h.context_for(0, 0, false).prev_on_track, None);
    assert!(should_play(None, None, Some("PRE"), 100, &h.context_for(0, 0, false), &mut always()).plays);
}

/// The whole point of the extension, end to end: a chain of trigs on one track
/// where each `PRE` reads the one before it.
#[test]
fn a_pre_chain_follows_the_track_down_the_pattern() {
    let mut h = CondHistory::new(2);
    // Loop 1, so 1ST is false.
    let first = should_play(None, None, Some("1ST"), 100, &h.context_for(0, 1, false), &mut always());
    assert!(!first.plays);
    h.record(0, first);

    // PRE now reads false → silent, and records false in turn.
    let second = should_play(None, None, Some("PRE"), 100, &h.context_for(0, 1, false), &mut always());
    assert!(!second.plays);
    h.record(0, second);

    // !PRE reads the same false → plays, and records true.
    let third = should_play(None, None, Some("!PRE"), 100, &h.context_for(0, 1, false), &mut always());
    assert!(third.plays);
    h.record(0, third);

    // ...so the next PRE plays.
    let fourth = should_play(None, None, Some("PRE"), 100, &h.context_for(0, 1, false), &mut always());
    assert!(fourth.plays);

    // And a NEI on track 1 sees track 0's last result, which is that `true`.
    assert!(should_play(None, None, Some("NEI"), 100, &h.context_for(1, 1, false), &mut always()).plays);
}
