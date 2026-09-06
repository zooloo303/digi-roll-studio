//! What a live take does to a track — MIDI_RECORD_DESIGN.md §4.3.
//!
//! Every rule the design lists gets a test here, and every one of them runs
//! without a thread, a port, a clock or an `Instant`. That is the whole reason
//! the rules were pulled out of `app::record` into `core`: "what happens when a
//! fifth note lands on a full step" should be answerable in microseconds rather
//! than by playing a chord at a box.
//!
//! **No JS oracle.** `js/midi.js` never listened to a MIDI input, so there is
//! nothing in the browser app to derive an overdub rule from. These come from
//! the design's decision list — 3 (overdub replaces), 4 (as played, with a
//! QUANTIZE toggle), 6 (keep the first four, count the rest) — and from what the
//! box's own LIVE REC does.

use digi_core::edit_ops::MICRO_MAX;
use digi_core::lengths::LEN_MIN;
use digi_core::model::{Note, Track, TrackKind};
use digi_core::record::{PlacedEvent, PlacedKind, Take, TakeOptions, PROVISIONAL_LEN};

/// A 16th at 120 bpm — the step length every `at` below is counted in.
const STEP: f64 = 0.125;

fn track() -> Track {
    Track::new(0, TrackKind::Audio)
}

fn options() -> TakeOptions {
    TakeOptions { notes_per_trig: 4, max_steps: 16.0, quantize: false, step_secs: STEP }
}

fn on(step: u64, pitch: u8, velocity: u8, at: f64) -> PlacedEvent {
    PlacedEvent {
        kind: PlacedKind::NoteOn { pitch, velocity },
        step,
        micro: 0.0,
        pass: 0,
        at,
    }
}

fn off(pitch: u8, at: f64) -> PlacedEvent {
    PlacedEvent { kind: PlacedKind::NoteOff { pitch }, step: 0, micro: 0.0, pass: 0, at }
}

fn note_at(track: &Track, step: f64, pitch: u8) -> &Note {
    track
        .notes
        .iter()
        .find(|n| n.step == step && n.pitch == pitch)
        .unwrap_or_else(|| panic!("no note at step {step} pitch {pitch}"))
}

// --- the ordinary case -------------------------------------------------------

/// §9 decision 4: the note appears the moment the key goes down, one step long,
/// so a held chord reads as a chord on the roll rather than as nothing at all.
#[test]
fn a_key_going_down_puts_a_note_in_the_track_at_once() {
    let (mut t, mut take) = (track(), Take::new());
    assert!(take.push(on(4, 60, 100, 0.0), &mut t, &options()));
    assert_eq!(t.notes.len(), 1);
    let note = note_at(&t, 4.0, 60);
    assert_eq!(note.len, PROVISIONAL_LEN, "provisional until the key comes up");
    assert_eq!(note.velocity, 100);
    assert!(take.holding(), "and the take knows a key is down");
}

/// The length is the difference of two exact moments, not of two rounded steps.
/// Two and a bit steps held is a two-and-a-bit-step note, which is what
/// "as played" means.
#[test]
fn a_key_coming_up_sets_the_length_from_how_long_it_was_held() {
    let (mut t, mut take) = (track(), Take::new());
    take.push(on(0, 60, 100, 0.0), &mut t, &options());
    take.push(off(60, 2.0 * STEP), &mut t, &options());
    assert_eq!(note_at(&t, 0.0, 60).len, 2.0);
    assert!(!take.holding());
}

/// Off the grid on purpose: a length no box can store is snapped to the nearest
/// one it can, so what the roll shows is what a write would land.
#[test]
fn a_held_length_is_snapped_to_something_a_box_can_store() {
    let (mut t, mut take) = (track(), Take::new());
    take.push(on(0, 60, 100, 0.0), &mut t, &options());
    take.push(off(60, 1.1 * STEP), &mut t, &options());
    assert_eq!(note_at(&t, 0.0, 60).len, digi_core::snap_len_fine(1.1, 16.0));
}

/// A stab shorter than the shortest storable note is still a note.
#[test]
fn the_shortest_possible_tap_is_floored_rather_than_lost() {
    let (mut t, mut take) = (track(), Take::new());
    take.push(on(0, 60, 100, 0.0), &mut t, &options());
    take.push(off(60, 0.0001), &mut t, &options());
    assert_eq!(note_at(&t, 0.0, 60).len, LEN_MIN);
}

/// A note held longer than the whole pattern is capped at it. Anything else
/// would be a note that laps itself.
#[test]
fn a_note_held_longer_than_the_track_is_capped_at_its_length() {
    let (mut t, mut take) = (track(), Take::new());
    take.push(on(0, 60, 100, 0.0), &mut t, &options());
    take.push(off(60, 40.0 * STEP), &mut t, &options());
    assert!(note_at(&t, 0.0, 60).len <= 16.0);
}

// --- the rules ---------------------------------------------------------------

/// Decision 3, the whole of overdub: playing the same note again on the same
/// step corrects it rather than stacking a second one.
#[test]
fn a_second_note_on_the_same_step_and_pitch_replaces_the_first() {
    let mut t = track();
    let mut take = Take::new();
    take.push(on(4, 60, 40, 0.0), &mut t, &options());
    take.push(off(60, 1.0 * STEP), &mut t, &options());

    let first_id = note_at(&t, 4.0, 60).id;
    take.push(on(4, 60, 120, 8.0 * STEP), &mut t, &options());
    take.push(off(60, 12.0 * STEP), &mut t, &options());

    assert_eq!(t.notes.len(), 1, "one note, corrected — not two on one step");
    let note = note_at(&t, 4.0, 60);
    assert_eq!(note.id, first_id, "the same note, so a selection holding it survives");
    assert_eq!(note.velocity, 120);
    assert_eq!(note.len, 4.0, "and its length came from the second pass's hold");
    assert_eq!(take.report.replaced, 1);
    assert_eq!(take.report.placed, 2, "both arrivals landed");
}

/// A different pitch on the same step is a chord, not a replacement.
#[test]
fn a_different_pitch_on_the_same_step_is_a_chord() {
    let (mut t, mut take) = (track(), Take::new());
    for pitch in [60, 64, 67] {
        take.push(on(4, pitch, 100, 0.0), &mut t, &options());
    }
    assert_eq!(t.notes.len(), 3);
    assert_eq!(take.report.replaced, 0);
}

/// Decision 6, and the import's §4.6 rule verbatim: keep the first four, drop
/// the rest, and *say which step* — a count with no location is a message
/// nobody can act on.
#[test]
fn a_fifth_note_on_one_step_is_dropped_and_the_step_is_named() {
    let (mut t, mut take) = (track(), Take::new());
    for pitch in [60, 63, 67, 70] {
        assert!(take.push(on(5, pitch, 100, 0.0), &mut t, &options()));
    }
    assert!(!take.push(on(5, 72, 100, 0.0), &mut t, &options()), "the fifth is refused");
    assert_eq!(t.notes.len(), 4);
    assert_eq!(take.report.dropped_full, 1);
    assert_eq!(take.report.full_steps, vec![5]);
    assert!(take.report.line("DT2 T1").contains("step 6 full"), "1-based, as the box counts");
}

/// The order of the two checks, stated on its own. Re-playing one of the four
/// notes already on a full step is a *correction*, and refusing it as a fifth
/// would make overdub stop working exactly where it is most needed.
#[test]
fn replaying_a_note_on_a_full_step_is_a_replace_and_not_a_drop() {
    let (mut t, mut take) = (track(), Take::new());
    for pitch in [60, 63, 67, 70] {
        take.push(on(5, pitch, 60, 0.0), &mut t, &options());
    }
    assert!(take.push(on(5, 63, 127, 8.0 * STEP), &mut t, &options()));
    assert_eq!(take.report.dropped_full, 0);
    assert_eq!(take.report.replaced, 1);
    assert_eq!(note_at(&t, 5.0, 63).velocity, 127);
    assert_eq!(t.notes.len(), 4);
}

/// PROB/FILL/COND are per trig on the box, so a note joining an occupied step
/// takes that step's conditions — exactly as a click, a paste or a drag does.
/// A recorded note that quietly kept its own blank conditions would break the
/// step-uniformity rule the encoder resolves by lowest pitch.
#[test]
fn a_note_joining_a_step_that_has_a_trig_adopts_its_conditions() {
    let mut t = track();
    let mut incumbent = Note::new(6.0, 48, 1.0, 100, 0.0);
    incumbent.prob = Some(50);
    incumbent.cond = Some("2:4".into());
    t.notes.push(incumbent);

    let mut take = Take::new();
    take.push(on(6, 72, 100, 0.0), &mut t, &options());
    let arrived = note_at(&t, 6.0, 72);
    assert_eq!(arrived.prob, Some(50));
    assert_eq!(arrived.cond.as_deref(), Some("2:4"));
}

/// Velocity through `clamp_velocity`, so nothing this app writes is a 0 — which
/// is a note-off on the wire.
#[test]
fn velocity_is_stored_as_played_within_the_range_the_wire_allows() {
    let (mut t, mut take) = (track(), Take::new());
    take.push(on(0, 60, 1, 0.0), &mut t, &options());
    take.push(on(1, 61, 127, 0.0), &mut t, &options());
    assert_eq!(note_at(&t, 0.0, 60).velocity, 1);
    assert_eq!(note_at(&t, 1.0, 61).velocity, 127);
}

/// Micro through `clamp_micro`. `place` can hand back a −0.5, and the roll's
/// own window stops at ±0.49 — so what a take stores is a value the gesture can
/// also produce, rather than one only a recording can hold.
#[test]
fn micro_is_clamped_to_what_the_roll_can_hold() {
    let mut t = track();
    let mut take = Take::new();
    let mut event = on(4, 60, 100, 0.0);
    event.micro = -0.5;
    take.push(event, &mut t, &options());
    assert_eq!(note_at(&t, 4.0, 60).micro, -MICRO_MAX);
}

/// Decision 4's toggle: with QUANTIZE on the engine has already zeroed the
/// micro, and this end rounds the length to whole steps. A staccato tap is one
/// step, never zero.
#[test]
fn quantize_rounds_lengths_to_whole_steps_and_never_to_none() {
    let quantized = TakeOptions { quantize: true, ..options() };
    let (mut t, mut take) = (track(), Take::new());

    take.push(on(0, 60, 100, 0.0), &mut t, &quantized);
    take.push(off(60, 2.4 * STEP), &mut t, &quantized);
    assert_eq!(note_at(&t, 0.0, 60).len, 2.0);

    take.push(on(4, 64, 100, 4.0 * STEP), &mut t, &quantized);
    take.push(off(64, 4.01 * STEP), &mut t, &quantized);
    assert_eq!(note_at(&t, 4.0, 64).len, 1.0, "a stab is a one-step trig");
}

/// Two ordinary ways to get a release with nothing held: the key was already
/// down when the take opened, and a keyboard sending an off after a panic. Both
/// are ignored rather than counted or crashed on.
#[test]
fn a_release_with_nothing_held_is_ignored() {
    let (mut t, mut take) = (track(), Take::new());
    assert!(!take.push(off(60, 1.0), &mut t, &options()));
    assert!(t.notes.is_empty());
    assert_eq!(take.report.placed, 0);
}

/// Passes are the highest one seen plus one, so "over 2 passes" means the take
/// really went round twice.
#[test]
fn passes_are_counted_from_the_highest_one_a_note_landed_in() {
    let (mut t, mut take) = (track(), Take::new());
    take.push(on(0, 60, 100, 0.0), &mut t, &options());
    let mut second = on(4, 64, 100, 20.0 * STEP);
    second.pass = 1;
    take.push(second, &mut t, &options());
    assert_eq!(take.report.passes, 2);
}

/// STOP with keys still down. The provisional length stands: nothing released
/// those notes, so nothing measured them, and inventing a length off the moment
/// a button was pressed would be worse than the honest one step.
#[test]
fn closing_a_take_mid_hold_leaves_the_held_notes_one_step_long() {
    let (mut t, mut take) = (track(), Take::new());
    take.push(on(0, 60, 100, 0.0), &mut t, &options());
    take.push(on(0, 64, 100, 0.0), &mut t, &options());
    let report = take.close(&mut t);
    assert_eq!(t.notes.len(), 2);
    assert!(t.notes.iter().all(|n| n.len == PROVISIONAL_LEN));
    assert_eq!(report.placed, 2);
}

// --- the report --------------------------------------------------------------

#[test]
fn a_take_that_placed_nothing_is_not_a_take() {
    let take = Take::new();
    assert!(take.report.is_empty());
}

#[test]
fn the_console_line_says_what_landed_and_what_did_not() {
    let (mut t, mut take) = (track(), Take::new());
    for pitch in [60, 63, 67, 70] {
        take.push(on(5, pitch, 100, 0.0), &mut t, &options());
    }
    take.push(on(5, 72, 100, 0.0), &mut t, &options());
    take.push(on(5, 63, 110, 8.0 * STEP), &mut t, &options());
    let mut second_pass = on(9, 48, 100, 20.0 * STEP);
    second_pass.pass = 1;
    take.push(second_pass, &mut t, &options());

    assert_eq!(
        take.report.line("DT2 T3"),
        "REC: 6 notes onto DT2 T3 over 2 passes · 1 replaced · 1 dropped (step 6 full)"
    );
}

/// The quiet case: nothing to apologise for, so the line does not manufacture
/// clauses saying so.
#[test]
fn a_clean_take_says_only_what_it_did() {
    let (mut t, mut take) = (track(), Take::new());
    take.push(on(0, 60, 100, 0.0), &mut t, &options());
    assert_eq!(take.report.line("A4 T2"), "REC: 1 note onto A4 T2 over 1 pass");
}
