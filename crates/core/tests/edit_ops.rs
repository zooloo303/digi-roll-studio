// Ported from test/edit-ops.test.js, case for case.
//
// Every expected value here is the JS suite's, not this port's output. Where a
// value is computed rather than literal (the snap cases), it was read out of
// js/roll-bridge.js with node before being written down — the Phase 1 method.

use digi_core::edit_ops::{
    adopt_step_trig, clamp_micro, clamp_velocity, clear_track, clipboard_anchor,
    duplicate_last_bar, nudge_velocities, place_clipboard, resize_selection_by,
    set_selection_length, Caret, ClipNote, LenEntry, PLockShift, PasteBounds, ResizeOpts, MICRO_MAX,
    MICRO_MIN, PITCH_MAX, PITCH_MIN, VEL_MAX, VEL_MIN,
};
use digi_core::lengths::{snap_len_fine, LEN_MIN};
use digi_core::model::Note;

fn clip(entries: &[(f64, u8, f64)]) -> Vec<ClipNote> {
    entries
        .iter()
        .map(|&(step, pitch, len)| ClipNote {
            step,
            pitch,
            len,
            velocity: 100,
            micro: 0.0,
            prob: None,
            fill: None,
            cond: None,
        })
        .collect()
}

fn bounds(length_steps: f64) -> PasteBounds {
    PasteBounds::new(length_steps)
}

fn shape(notes: &[ClipNote]) -> Vec<(f64, u8, f64)> {
    notes.iter().map(|n| (n.step, n.pitch, n.len)).collect()
}

fn sel(pairs: &[(f64, f64)]) -> Vec<LenEntry> {
    pairs
        .iter()
        .map(|&(step, len)| LenEntry { step, len })
        .collect()
}

// ---------------------------------------------------------------- the anchor

#[test]
fn anchor_is_the_earliest_step() {
    let c = clip(&[(4.0, 60, 1.0), (1.0, 64, 1.0), (7.0, 67, 1.0)]);
    let a = clipboard_anchor(&c).unwrap();
    assert_eq!((a.step, a.pitch), (1.0, 64));
}

#[test]
fn anchor_is_the_highest_pitch_among_notes_sharing_that_step() {
    let c = clip(&[(2.0, 60, 1.0), (2.0, 67, 1.0), (2.0, 64, 1.0)]);
    let a = clipboard_anchor(&c).unwrap();
    assert_eq!((a.step, a.pitch), (2.0, 67));
}

// -------------------------------------------------------- pasting at a caret

#[test]
fn lands_the_anchor_on_the_caret_and_keeps_the_blocks_shape() {
    let placed = place_clipboard(
        &clip(&[(4.0, 60, 1.0), (6.0, 64, 1.0), (8.0, 67, 1.0)]),
        Some(Caret { step: 0.0, pitch: 72 }),
        &bounds(16.0),
    );
    assert_eq!(placed.dropped, 0);
    // Anchor (4, 60) → (0, 72), so everything shifts −4 steps and +12 semitones.
    assert_eq!(
        shape(&placed.notes),
        vec![(0.0, 72, 1.0), (2.0, 76, 1.0), (4.0, 79, 1.0)]
    );
}

#[test]
fn shifts_a_chord_as_one_block_not_note_by_note() {
    let placed = place_clipboard(
        &clip(&[(2.0, 60, 1.0), (2.0, 64, 1.0), (2.0, 67, 1.0)]),
        Some(Caret { step: 9.0, pitch: 60 }),
        &bounds(16.0),
    );
    // The anchor is the top note (67), so the whole chord comes down a fifth.
    assert_eq!(
        shape(&placed.notes),
        vec![(9.0, 53, 1.0), (9.0, 57, 1.0), (9.0, 60, 1.0)]
    );
}

#[test]
fn carries_velocity_micro_timing_and_the_trig_conditions_across() {
    let src = vec![ClipNote {
        step: 1.0,
        pitch: 60,
        len: 2.0,
        velocity: 42,
        micro: 0.25,
        prob: Some(30),
        fill: Some(false),
        cond: Some("2:4".into()),
    }];
    let placed = place_clipboard(&src, Some(Caret { step: 5.0, pitch: 62 }), &bounds(16.0));
    let n = &placed.notes[0];
    assert_eq!(n.velocity, 42);
    assert_eq!(n.micro, 0.25);
    assert_eq!(n.prob, Some(30));
    assert_eq!(n.fill, Some(false));
    assert_eq!(n.cond.as_deref(), Some("2:4"));
}

#[test]
fn drops_notes_whose_start_falls_past_the_end_of_the_pattern() {
    let placed = place_clipboard(
        &clip(&[(0.0, 60, 1.0), (8.0, 62, 1.0)]),
        Some(Caret { step: 12.0, pitch: 60 }),
        &bounds(16.0),
    );
    assert_eq!(shape(&placed.notes), vec![(12.0, 60, 1.0)]);
    assert_eq!(placed.dropped, 1);
}

#[test]
fn drops_notes_pushed_off_the_bottom_of_the_drawable_rows() {
    let placed = place_clipboard(
        &clip(&[(0.0, 72, 1.0), (1.0, 30, 1.0)]),
        Some(Caret {
            step: 0.0,
            pitch: PITCH_MIN + 2,
        }),
        &bounds(16.0),
    );
    // The block drops 46 semitones: the low note would land under PITCH_MIN.
    assert_eq!(shape(&placed.notes), vec![(0.0, PITCH_MIN + 2, 1.0)]);
    assert_eq!(placed.dropped, 1);
}

#[test]
fn shortens_a_note_that_overruns_the_end_rather_than_dropping_it() {
    let placed = place_clipboard(
        &clip(&[(0.0, 60, 8.0)]),
        Some(Caret { step: 12.0, pitch: 60 }),
        &bounds(16.0),
    );
    assert_eq!(shape(&placed.notes), vec![(12.0, 60, 4.0)]);
    assert_eq!(placed.dropped, 0);
}

#[test]
fn reports_everything_dropped_when_the_block_lands_off_the_grid() {
    let placed = place_clipboard(
        &clip(&[(0.0, 60, 1.0), (1.0, 61, 1.0)]),
        Some(Caret { step: 15.0, pitch: 60 }),
        &bounds(16.0),
    );
    assert_eq!(shape(&placed.notes), vec![(15.0, 60, 1.0)]);
    assert_eq!(placed.dropped, 1);
}

// ------------------------------------------------------ pasting with no caret

#[test]
fn with_no_caret_keeps_the_old_absolute_position_behaviour() {
    let placed = place_clipboard(&clip(&[(4.0, 60, 1.0), (6.0, 64, 1.0)]), None, &bounds(16.0));
    assert_eq!(shape(&placed.notes), vec![(4.0, 60, 1.0), (6.0, 64, 1.0)]);
    assert_eq!(placed.dropped, 0);
}

#[test]
fn with_no_caret_backstops_a_note_past_the_end_onto_the_last_step() {
    let placed = place_clipboard(&clip(&[(40.0, 60, 4.0)]), None, &bounds(16.0));
    assert_eq!(shape(&placed.notes), vec![(15.0, 60, 1.0)]);
    assert_eq!(placed.dropped, 0);
}

#[test]
fn an_empty_clipboard_places_nothing() {
    let placed = place_clipboard(&[], Some(Caret { step: 0.0, pitch: 60 }), &bounds(16.0));
    assert!(placed.notes.is_empty());
    assert_eq!(placed.dropped, 0);
}

// ------------------------------------------- dragging one edge of a selection

#[test]
fn drag_moves_every_note_by_the_same_delta_so_the_shape_survives() {
    let lens = resize_selection_by(
        &sel(&[(0.0, 1.0), (4.0, 2.0), (8.0, 4.0)]),
        1.0,
        &ResizeOpts::coarse(16.0),
    );
    assert_eq!(lens, vec![2.0, 3.0, 5.0]);
}

#[test]
fn drag_shrinks_by_the_same_delta_too() {
    let lens = resize_selection_by(&sel(&[(0.0, 4.0), (4.0, 3.0)]), -2.0, &ResizeOpts::coarse(16.0));
    assert_eq!(lens, vec![2.0, 1.0]);
}

#[test]
fn drag_stops_the_whole_group_at_the_first_note_that_runs_out_of_room() {
    // A 1-step note at step 14 of a 16-step pattern can grow by exactly one
    // before it hits the end, so asking for four holds everyone to one rather
    // than letting the others run on and flatten the differences this mode
    // exists to keep.
    let lens = resize_selection_by(&sel(&[(0.0, 1.0), (14.0, 1.0)]), 4.0, &ResizeOpts::coarse(16.0));
    assert_eq!(lens, vec![2.0, 2.0]);
}

#[test]
fn drag_stops_the_whole_group_at_the_shortest_note_when_shrinking() {
    let lens = resize_selection_by(&sel(&[(0.0, 4.0), (4.0, 1.0)]), -3.0, &ResizeOpts::coarse(16.0));
    assert_eq!(lens, vec![4.0, 1.0]);
}

#[test]
fn drag_never_shrinks_past_the_floor_it_is_given() {
    // The 2-step note hits 1, so the delta stops at −1.
    let lens = resize_selection_by(
        &sel(&[(0.0, 2.0), (4.0, 8.0)]),
        -100.0,
        &ResizeOpts::coarse(16.0),
    );
    assert_eq!(lens, vec![1.0, 7.0]);
}

#[test]
fn drag_snaps_every_result_to_what_the_device_can_store() {
    // A fine drag: the delta lands each note on the box's own LEN scale rather
    // than on a value that would quietly round on write.
    let lens = resize_selection_by(
        &sel(&[(0.0, 1.0), (4.0, 2.0)]),
        0.1,
        &ResizeOpts::fine(16.0, snap_len_fine, LEN_MIN),
    );
    assert_eq!(lens, vec![1.125, 2.125]);
    for len in lens {
        // Already representable: snapping again changes nothing.
        assert_eq!(snap_len_fine(len, f64::INFINITY), len);
    }
}

#[test]
fn drag_places_no_lengths_for_an_empty_selection() {
    assert!(resize_selection_by(&[], 2.0, &ResizeOpts::coarse(16.0)).is_empty());
}

// ------------------------------------------------- the LEN control over a set

#[test]
fn len_control_makes_every_note_the_same_length() {
    let lens = set_selection_length(
        &sel(&[(0.0, 1.0), (4.0, 2.0), (8.0, 4.0)]),
        3.0,
        &ResizeOpts::coarse(16.0),
    );
    assert_eq!(lens, vec![3.0, 3.0, 3.0]);
}

#[test]
fn len_control_clamps_per_note_so_a_note_short_of_room_takes_what_it_has() {
    // Deliberately unlike the drag: one cramped note must not hold the rest back
    // from the length that was actually asked for.
    let lens = set_selection_length(&sel(&[(0.0, 1.0), (14.0, 1.0)]), 4.0, &ResizeOpts::coarse(16.0));
    assert_eq!(lens, vec![4.0, 2.0]);
}

#[test]
fn len_control_snaps_to_the_device_scale_and_honours_its_floor() {
    let lens = set_selection_length(
        &sel(&[(0.0, 4.0)]),
        0.01,
        &ResizeOpts::fine(16.0, snap_len_fine, LEN_MIN),
    );
    assert_eq!(lens, vec![LEN_MIN]);
}

// --------------------------------------- notes joining an occupied step

fn note(step: f64, pitch: u8) -> Note {
    Note::new(step, pitch, 1.0, 100, 0.0)
}

fn with_trig(mut n: Note, prob: Option<u8>, fill: Option<bool>, cond: Option<&str>) -> Note {
    n.prob = prob;
    n.fill = fill;
    n.cond = cond.map(String::from);
    n
}

#[test]
fn an_arriving_note_takes_the_incumbent_trigs_conditions() {
    let incumbent = with_trig(note(4.0, 48), Some(40), Some(true), Some("2:4"));
    let arriving = with_trig(note(4.0, 60), None, None, Some("PRE"));
    let arriving_id = arriving.id;
    let incumbent_id = incumbent.id;
    let mut notes = vec![incumbent, arriving];

    assert_eq!(adopt_step_trig(&mut notes, &[arriving_id]), 1);

    let a = notes.iter().find(|n| n.id == arriving_id).unwrap();
    assert_eq!((a.prob, a.fill, a.cond.as_deref()), (Some(40), Some(true), Some("2:4")));
    // The incumbent is the trig; it never moves toward the arrival.
    let i = notes.iter().find(|n| n.id == incumbent_id).unwrap();
    assert_eq!((i.prob, i.fill, i.cond.as_deref()), (Some(40), Some(true), Some("2:4")));
}

#[test]
fn an_arriving_note_keeps_its_own_conditions_on_an_empty_step() {
    let arriving = with_trig(note(6.0, 60), None, None, Some("2:4"));
    let other = with_trig(note(3.0, 48), None, None, Some("PRE"));
    let arriving_id = arriving.id;
    let mut notes = vec![other, arriving];

    assert_eq!(adopt_step_trig(&mut notes, &[arriving_id]), 0);
    let a = notes.iter().find(|n| n.id == arriving_id).unwrap();
    assert_eq!(a.cond.as_deref(), Some("2:4"));
}

#[test]
fn adopts_from_the_lowest_pitch_incumbent_the_note_the_encoder_believes() {
    let low = with_trig(note(4.0, 40), Some(75), None, None);
    let high = with_trig(note(4.0, 70), Some(30), None, None);
    let arriving = note(4.0, 60);
    let arriving_id = arriving.id;
    let mut notes = vec![high, low, arriving];

    adopt_step_trig(&mut notes, &[arriving_id]);
    assert_eq!(notes.iter().find(|n| n.id == arriving_id).unwrap().prob, Some(75));
}

#[test]
fn other_arrivals_are_not_incumbents_so_a_pasted_chord_keeps_its_conditions() {
    let a = with_trig(note(2.0, 60), None, None, Some("2:4"));
    let b = with_trig(note(2.0, 64), None, None, Some("2:4"));
    let (a_id, b_id) = (a.id, b.id);
    let mut notes = vec![a, b];

    assert_eq!(adopt_step_trig(&mut notes, &[a_id, b_id]), 0);
    for id in [a_id, b_id] {
        assert_eq!(notes.iter().find(|n| n.id == id).unwrap().cond.as_deref(), Some("2:4"));
    }
}

#[test]
fn counts_only_notes_that_actually_changed() {
    let incumbent = with_trig(note(4.0, 48), None, None, Some("2:4"));
    let agrees = with_trig(note(4.0, 60), None, None, Some("2:4"));
    let differs = with_trig(note(4.0, 64), None, None, None);
    let (agrees_id, differs_id) = (agrees.id, differs.id);
    let mut notes = vec![incumbent, agrees, differs];

    assert_eq!(adopt_step_trig(&mut notes, &[agrees_id, differs_id]), 1);
}

#[test]
fn explicit_defaults_adopt_too_so_an_all_null_incumbent_strips_an_arriving_lock() {
    // Joining a trig means taking it as it is, including "no locks at all";
    // anything else would leave the step non-uniform in the other direction.
    let incumbent = note(4.0, 48);
    let arriving = with_trig(note(4.0, 60), Some(40), None, Some("2:4"));
    let arriving_id = arriving.id;
    let mut notes = vec![incumbent, arriving];

    assert_eq!(adopt_step_trig(&mut notes, &[arriving_id]), 1);
    let a = notes.iter().find(|n| n.id == arriving_id).unwrap();
    assert_eq!((a.prob, a.fill, a.cond.as_deref()), (None, None, None));
}

#[test]
fn pitch_rows_match_the_js_roll() {
    assert_eq!((PITCH_MIN, PITCH_MAX), (24, 96));
}

// --- Phase 9: velocity, micro-timing, and the two whole-track operations -----
//
// **The oracle covers none of this.** `test/edit-ops.test.js` has no case for a
// velocity drag, and `test/pianoroll.test.js` tests exactly one function
// (`noteName`) — so the shift-drag, the alt-drag, the cmd-drag and the duplicate
// were all untested on the far side of the port too. Checked before writing, per
// `DEVELOPMENT.md`. The expected values below therefore come from *reading*
// `js/pianoroll.js`'s `vel` and `micro` modes and `js/main.js`'s `dup` and
// `clear`, and each test says which line it is pinning.

#[test]
fn a_velocity_drag_moves_the_whole_selection_by_one_delta() {
    // `js/pianoroll.js`: `it.vel + d` for every item, where `d` is one number for
    // the gesture. The differences between the notes survive, which is the point
    // — that is what "the group delta rule, not levelling" means.
    assert_eq!(nudge_velocities(&[100, 60, 40], 10), [110, 70, 50]);
    assert_eq!(nudge_velocities(&[100, 60, 40], -10), [90, 50, 30]);
    assert_eq!(nudge_velocities(&[100, 60, 40], 0), [100, 60, 40]);
}

#[test]
fn a_velocity_drag_clamps_per_note_where_a_resize_clamps_for_the_group() {
    // The divergence PLAN.md §7 rule 3 exists to preserve, asserted so nobody
    // "fixes" it into the resize's rule. 120 and 60 pushed up by 20 land on 127
    // and 80 — the loud note flattens against the ceiling and the quiet one keeps
    // travelling. The resize would have held both back by the same amount.
    assert_eq!(nudge_velocities(&[120, 60], 20), [127, 80]);
    assert_eq!(nudge_velocities(&[10, 60], -20), [1, 40]);

    // And the contrast, on the same shapes: `resize_selection_by` refuses the
    // whole delta when one member cannot take it.
    let entries = [LenEntry { step: 14.0, len: 2.0 }, LenEntry { step: 0.0, len: 2.0 }];
    assert_eq!(resize_selection_by(&entries, 8.0, &ResizeOpts::coarse(16.0)), [2.0, 2.0]);
}

#[test]
fn velocity_never_reaches_zero_because_zero_is_a_note_off() {
    assert_eq!(nudge_velocities(&[1, 5], -100), [VEL_MIN, VEL_MIN]);
    assert_eq!(clamp_velocity(0), VEL_MIN);
    assert_eq!(clamp_velocity(-40), VEL_MIN);
    assert_eq!(clamp_velocity(500), VEL_MAX);
    assert_eq!((VEL_MIN, VEL_MAX), (1, 127));
}

#[test]
fn an_empty_selection_is_a_velocity_drag_that_does_nothing() {
    assert!(nudge_velocities(&[], 40).is_empty());
}

#[test]
fn micro_timing_stops_just_short_of_the_neighbouring_step() {
    // `js/pianoroll.js`: `Math.max(-0.49, Math.min(0.49, ...))`. Narrower than
    // what the box stores on purpose — see the constants' own note.
    assert_eq!(clamp_micro(0.0), 0.0);
    assert_eq!(clamp_micro(0.25), 0.25);
    assert_eq!(clamp_micro(5.0), MICRO_MAX);
    assert_eq!(clamp_micro(-5.0), MICRO_MIN);
    assert_eq!((MICRO_MIN, MICRO_MAX), (-0.49, 0.49));
}

#[test]
fn the_micro_window_is_narrower_than_the_byte_the_box_stores() {
    // Stated in the constants and checked here, because it is the kind of claim
    // that rots: the box holds ±23/24 of a step and the gesture reaches ±0.49,
    // which is 11 ticks of the 23 available. A note imported carrying more keeps
    // it — nothing in this module touches an existing value.
    //
    // **Clippy calls this a constant assertion, and it is — that is the job.**
    // Both sides are constants, so it can only fail when somebody edits one of
    // them, which is precisely the day this test is for. `DEVELOPMENT.md` lesson 2
    // is about the opposite case, a committed witness nothing asserts on; this is
    // the witness.
    #[allow(clippy::assertions_on_constants)]
    {
        assert!(MICRO_MAX < 23.0 / 24.0);
    }
    let imported = Note::new(4.0, 60, 1.0, 100, 20.0 / 24.0);
    assert_eq!(imported.micro, 20.0 / 24.0, "an import is not clamped by the gesture's window");
}

#[test]
fn duplicating_a_bar_copies_the_last_one_onto_the_end() {
    // `js/main.js`'s `dup`: bar 1 into bar 2, and the length grows with it.
    let mut track = track_with(16, &[(0.0, 60, 1.0), (4.0, 62, 2.0), (12.0, 64, 1.0)]);
    assert_eq!(duplicate_last_bar(&mut track), Some((1, 2)));
    assert_eq!(track.length_steps, 32);
    let steps: Vec<f64> = track.notes.iter().map(|n| n.step).collect();
    assert_eq!(steps, [0.0, 4.0, 12.0, 16.0, 20.0, 28.0]);
}

#[test]
fn only_the_last_bar_is_copied_not_the_whole_track() {
    let mut track = track_with(32, &[(0.0, 60, 1.0), (16.0, 62, 1.0), (20.0, 64, 1.0)]);
    assert_eq!(duplicate_last_bar(&mut track), Some((2, 3)));
    let steps: Vec<f64> = track.notes.iter().map(|n| n.step).collect();
    assert_eq!(steps, [0.0, 16.0, 20.0, 32.0, 36.0], "step 0 is in bar 1 and stays there");
}

#[test]
fn a_copied_note_is_clipped_to_the_room_the_new_end_leaves_it() {
    // A four-step note at step 14 of a 16-step track: its copy starts at 30 and
    // the new end is 32, so it comes out two steps long rather than hanging past
    // the wrap line.
    let mut track = track_with(16, &[(14.0, 60, 4.0)]);
    duplicate_last_bar(&mut track);
    let copy = track.notes.iter().find(|n| n.step == 30.0).expect("the copy");
    assert_eq!(copy.len, 2.0);
}

#[test]
fn a_copied_note_gets_its_own_id() {
    // Two notes carrying one id makes the roll's selection ambiguous, which is
    // what `Note::reissue_id` exists for.
    let mut track = track_with(16, &[(0.0, 60, 1.0)]);
    let original = track.notes[0].id;
    duplicate_last_bar(&mut track);
    assert_eq!(track.notes.len(), 2);
    assert_ne!(track.notes[1].id, original);
}

#[test]
fn a_full_length_track_refuses_rather_than_silently_dropping_the_copy() {
    let mut track = track_with(128, &[(120.0, 60, 1.0)]);
    assert_eq!(duplicate_last_bar(&mut track), None);
    assert_eq!(track.length_steps, 128);
    assert_eq!(track.notes.len(), 1, "nothing was added");
}

#[test]
fn a_trig_past_the_wrap_line_is_dropped_rather_than_copied_to_nowhere() {
    // This roll allows a trig past the wrap line and the JS's clamps do not, so
    // this case has no oracle. Its copy would land at or beyond the new end,
    // where the JS's own arithmetic would have given it a length of zero.
    let mut track = track_with(16, &[(0.0, 60, 1.0), (20.0, 62, 1.0)]);
    duplicate_last_bar(&mut track);
    let steps: Vec<f64> = track.notes.iter().map(|n| n.step).collect();
    assert_eq!(steps, [0.0, 20.0, 16.0], "step 20's copy would be step 36, past the new end of 32");
}

#[test]
fn clearing_a_track_keeps_everything_about_it_that_is_not_music() {
    let mut track = track_with(48, &[(0.0, 60, 1.0)]);
    track.track_prob = 70;
    track.channel = 9;
    track.mute = true;
    track.scale = digi_core::TrackScale::Half;
    track.out_port = Some(String::from("port-1"));
    assert!(clear_track(&mut track));
    assert!(track.notes.is_empty());
    assert_eq!(track.length_steps, 48);
    assert_eq!(track.track_prob, 70);
    assert_eq!(track.channel, 9);
    assert!(track.mute);
    assert_eq!(track.scale, digi_core::TrackScale::Half);
    assert_eq!(track.out_port.as_deref(), Some("port-1"));
}

#[test]
fn clearing_a_track_takes_the_p_lock_lanes_with_the_trigs() {
    // Locks ride on trigs, so erasing the trigs erases the automation — which is
    // what writing the cleared track back to the box should do. `js/main.js` says
    // the same thing above its own `clear`.
    let mut track = track_with(16, &[(0.0, 60, 1.0)]);
    track.plocks = vec![digi_core::PLockLane::new(
        Some(String::from("filter.cutoff")),
        None,
        Some(String::from("DT2")),
        false,
        vec![Some(64)],
    )
    .unwrap()];
    assert!(clear_track(&mut track));
    assert!(track.plocks.is_empty());
}

#[test]
fn clearing_an_already_empty_track_reports_nothing_and_leaves_no_undo_step() {
    let mut track = track_with(16, &[]);
    assert!(!clear_track(&mut track));
}

fn track_with(length_steps: u16, notes: &[(f64, u8, f64)]) -> digi_core::Track {
    let mut track = digi_core::Track::new(0, digi_core::TrackKind::Audio);
    track.length_steps = length_steps;
    track.notes = notes
        .iter()
        .map(|&(step, pitch, len)| Note::new(step, pitch, len, 100, 0.0))
        .collect();
    track
}

// ------------------------------------------------- p-locks under a move

/// A lane holding values at the given slots, on the display axis.
fn lane(name: &str, at: &[(usize, u16)]) -> digi_core::PLockLane {
    let mut values = vec![None; 128];
    for &(slot, v) in at {
        values[slot] = Some(v);
    }
    digi_core::PLockLane::new(
        Some(String::from(name)),
        None,
        Some(String::from("DT2")),
        false,
        values,
    )
    .unwrap()
}

/// The slots a lane holds a value on, so an assertion reads as a shape rather
/// than as 128 `None`s.
fn locks(lane: &digi_core::PLockLane) -> Vec<(usize, u16)> {
    lane.values
        .iter()
        .enumerate()
        .filter_map(|(slot, v)| v.map(|v| (slot, v)))
        .collect()
}

#[test]
fn a_lock_travels_with_the_trig_it_belongs_to() {
    // The bug this exists for: the roll moved the trig and left the sweep on the
    // step it came off, so the automation belonged to whatever turned up there
    // next and the moved trig had none.
    let mut track = track_with(16, &[(4.0, 60, 1.0)]);
    track.plocks = vec![lane("filter.cutoff", &[(4, 100)])];
    let moving = vec![track.notes[0].id];

    let shift = PLockShift::capture(&track, &moving);
    track.notes[0].step = 7.0;
    assert!(shift.apply(&mut track, 3.0));
    assert_eq!(locks(&track.plocks[0]), [(7, 100)]);
}

#[test]
fn every_lane_on_the_track_travels_not_just_the_first() {
    let mut track = track_with(16, &[(2.0, 60, 1.0)]);
    track.plocks = vec![
        lane("filter.cutoff", &[(2, 30)]),
        lane("amp.pan", &[(2, 90)]),
    ];
    let moving = vec![track.notes[0].id];

    let shift = PLockShift::capture(&track, &moving);
    assert!(shift.apply(&mut track, 5.0));
    assert_eq!(locks(&track.plocks[0]), [(7, 30)]);
    assert_eq!(locks(&track.plocks[1]), [(7, 90)]);
}

#[test]
fn a_step_that_keeps_a_note_keeps_its_locks_and_the_mover_takes_a_copy() {
    // One note dragged out of a locked chord. The step still has a trig on it,
    // so the box still has a lock there; the note that left lands on a trig of
    // its own, which carries the value it was playing.
    let mut track = track_with(16, &[(4.0, 60, 1.0), (4.0, 67, 1.0)]);
    track.plocks = vec![lane("filter.cutoff", &[(4, 64)])];
    let moving = vec![track.notes[1].id];

    let shift = PLockShift::capture(&track, &moving);
    assert!(shift.apply(&mut track, 2.0));
    assert_eq!(locks(&track.plocks[0]), [(4, 64), (6, 64)]);
}

#[test]
fn the_lock_already_on_the_destination_wins() {
    // The same rule as `adopt_step_trig`: the trig at the destination was
    // already there and the arriving one is joining it.
    let mut track = track_with(16, &[(0.0, 60, 1.0), (4.0, 62, 1.0)]);
    track.plocks = vec![lane("filter.cutoff", &[(0, 10), (4, 120)])];
    let moving = vec![track.notes[0].id];

    let shift = PLockShift::capture(&track, &moving);
    assert!(shift.apply(&mut track, 4.0));
    assert_eq!(
        locks(&track.plocks[0]),
        [(4, 120)],
        "step 4 keeps its own value and step 0 is vacated"
    );
}

#[test]
fn a_run_of_trigs_moving_one_step_does_not_overwrite_its_own_tail() {
    // Cleared before stamped, or step 5's value would be gone by the time step
    // 4's landed on it — three locks becoming one.
    let mut track = track_with(16, &[(4.0, 60, 1.0), (5.0, 60, 1.0), (6.0, 60, 1.0)]);
    track.plocks = vec![lane("filter.cutoff", &[(4, 1), (5, 2), (6, 3)])];
    let moving: Vec<u32> = track.notes.iter().map(|n| n.id).collect();

    let shift = PLockShift::capture(&track, &moving);
    assert!(shift.apply(&mut track, 1.0));
    assert_eq!(locks(&track.plocks[0]), [(5, 1), (6, 2), (7, 3)]);
}

#[test]
fn a_drag_that_reverses_lands_back_exactly_where_it_started() {
    // Every frame recomputes from the captured base, so nothing accumulates —
    // the same contract `resize_selection_by`'s start lengths keep.
    let mut track = track_with(16, &[(4.0, 60, 1.0)]);
    track.plocks = vec![lane("filter.cutoff", &[(4, 100)])];
    let before = track.plocks.clone();
    let moving = vec![track.notes[0].id];

    let shift = PLockShift::capture(&track, &moving);
    for delta in [1.0, 5.0, 9.0, 2.0] {
        shift.apply(&mut track, delta);
    }
    assert!(shift.apply(&mut track, 0.0), "the last frame put it back");
    assert_eq!(track.plocks, before);
    assert!(
        !shift.apply(&mut track, 0.0),
        "and a frame that moves nothing reports nothing, so it costs no undo step"
    );
}

#[test]
fn a_trigless_lane_is_passed_through_untouched() {
    // Trigless values are not attached to any trig, so no trig carries them —
    // and v1's contract is to leave them exactly as the box had them.
    let mut track = track_with(16, &[(4.0, 60, 1.0)]);
    let mut trigless = lane("filter.cutoff", &[(4, 100)]);
    trigless.trigless = true;
    track.plocks = vec![trigless];
    let before = track.plocks.clone();
    let moving = vec![track.notes[0].id];

    let shift = PLockShift::capture(&track, &moving);
    assert!(!shift.apply(&mut track, 3.0));
    assert_eq!(track.plocks, before);
}

#[test]
fn a_lock_dragged_past_the_last_slot_a_lane_has_is_dropped() {
    // There is nowhere to store it: the trig is past what the box can name too,
    // and `export::notes_for_device` is what says so.
    let mut track = track_with(128, &[(126.0, 60, 1.0)]);
    track.plocks = vec![lane("filter.cutoff", &[(126, 100)])];
    let moving = vec![track.notes[0].id];

    let shift = PLockShift::capture(&track, &moving);
    assert!(shift.apply(&mut track, 4.0));
    assert!(locks(&track.plocks[0]).is_empty());
}

#[test]
fn a_track_with_no_lanes_captures_nothing_and_does_nothing() {
    let mut track = track_with(16, &[(4.0, 60, 1.0)]);
    let moving = vec![track.notes[0].id];
    let shift = PLockShift::capture(&track, &moving);
    assert!(shift.is_empty());
    assert!(!shift.apply(&mut track, 3.0));
}

#[test]
fn two_notes_on_one_step_carry_that_steps_lock_once() {
    // A whole chord moving is one trig moving. The slot is captured once, so
    // the value cannot be stamped twice — and the step it left is vacated.
    let mut track = track_with(16, &[(4.0, 60, 1.0), (4.0, 64, 1.0), (4.0, 67, 1.0)]);
    track.plocks = vec![lane("amp.pan", &[(4, 20)])];
    let moving: Vec<u32> = track.notes.iter().map(|n| n.id).collect();

    let shift = PLockShift::capture(&track, &moving);
    assert!(shift.apply(&mut track, 3.0));
    assert_eq!(locks(&track.plocks[0]), [(7, 20)]);
}

#[test]
fn micro_timing_does_not_move_a_lock_onto_a_neighbouring_slot() {
    // A trig at 4.4 is a trig on step 4 with an offset — one lock, on slot 4 —
    // which is `export::notes_for_device`'s rounding, applied here too.
    let mut track = track_with(16, &[(4.4, 60, 1.0)]);
    track.plocks = vec![lane("filter.cutoff", &[(4, 100)])];
    let moving = vec![track.notes[0].id];

    let shift = PLockShift::capture(&track, &moving);
    assert!(shift.apply(&mut track, 1.0));
    assert_eq!(locks(&track.plocks[0]), [(5, 100)]);
}

#[test]
fn a_lane_shorter_than_the_full_step_count_is_indexed_safely() {
    // `PLockLane::values` is a public field and `PLockLane::new`'s padding to 128
    // is a constructor's courtesy, not the type's guarantee — a lane built by
    // hand can be shorter, and `crates/app/tests/write.rs` builds one. A move at
    // the end of a long pattern must not index past it.
    let mut track = track_with(64, &[(40.0, 60, 1.0)]);
    let short = digi_core::PLockLane {
        name: Some(String::from("filter.cutoff")),
        param_id: None,
        device_kind: Some(String::from("DT2")),
        trigless: false,
        values: vec![Some(64); 16],
    };
    track.plocks = vec![short];
    let moving = vec![track.notes[0].id];

    let shift = PLockShift::capture(&track, &moving);
    // Nothing to carry — slot 40 is past the end of this lane — and nothing to
    // panic about either.
    assert!(!shift.apply(&mut track, 4.0));
    assert_eq!(track.plocks[0].values.len(), 16, "and the lane is left as it was");
}
