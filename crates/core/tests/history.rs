// The undo stack, and the line it draws between the music and the desk.
//
// No oracle: `js/main.js`'s history is untested on the far side too, and its
// snapshot is one pattern slot where this is every box's. So these are the claims
// the module header makes, each turned into a test — including the two that are
// the whole reason the scoping question in PLAN.md §9 needed an answer: what an
// undo step contains, and what closes one.

use digi_core::device::{model_for_key, Device};
use digi_core::history::{Content, History, HISTORY_MAX};
use digi_core::{default_session, Note, PortRef, Session};

/// Two boxes, and a session where both play A01. The default the app opens with.
fn session() -> Session {
    default_session()
}

fn first_track(session: &mut Session) -> &mut digi_core::Track {
    let device = session.devices[0].id;
    session.device_mut(device).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap()
}

fn note_count(session: &Session) -> usize {
    session.devices[0].pattern(0).unwrap().track(0).unwrap().notes.len()
}

/// One whole gesture: snapshot, edit, commit.
fn gesture(history: &mut History, session: &mut Session, edit: impl FnOnce(&mut Session)) -> bool {
    history.begin(Content::of(session));
    edit(session);
    history.commit(session)
}

#[test]
fn a_gesture_becomes_one_step_and_undoes_as_one() {
    let mut session = session();
    let mut history = History::default();
    assert!(gesture(&mut history, &mut session, |s| {
        first_track(s).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    }));
    assert_eq!(note_count(&session), 1);
    assert!(history.can_undo());

    assert!(history.undo(&mut session));
    assert_eq!(note_count(&session), 0);
    assert!(!history.can_undo());
    assert!(history.can_redo());

    assert!(history.redo(&mut session));
    assert_eq!(note_count(&session), 1);
}

#[test]
fn many_frames_of_one_drag_are_still_one_step() {
    // The claim that matters most for the sliders and the roll's drags: `begin` is
    // idempotent while a step is open, so forty frames of movement push one entry.
    let mut session = session();
    let mut history = History::default();
    history.begin(Content::of(&session));
    first_track(&mut session).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    for velocity in 1..=40u8 {
        history.begin(Content::of(&session)); // every subsequent frame of the drag
        first_track(&mut session).notes[0].velocity = velocity;
        assert!(history.is_open());
    }
    assert!(history.commit(&session));
    assert_eq!(history.depth(), (1, 0));

    history.undo(&mut session);
    assert_eq!(note_count(&session), 0, "the whole drag went, not its last frame");
}

#[test]
fn a_gesture_that_changed_nothing_leaves_no_step() {
    // `dropUnchangedUndo`. Clicking a note sets the app's `edited` flag on the
    // frame the trig is adopted, and an undo stack full of no-ops is an undo
    // button that has to be pressed several times before anything happens.
    let mut session = session();
    let mut history = History::default();
    assert!(!gesture(&mut history, &mut session, |_| {}));
    assert!(!history.can_undo());
}

#[test]
fn a_gesture_that_changed_something_back_again_leaves_no_step() {
    let mut session = session();
    let mut history = History::default();
    let pushed = gesture(&mut history, &mut session, |s| {
        let track = first_track(s);
        track.notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
        track.notes.clear();
    });
    assert!(!pushed, "a drag that ended where it started is not a step");
}

#[test]
fn commit_without_begin_pushes_nothing() {
    let mut session = session();
    let mut history = History::default();
    first_track(&mut session).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    assert!(!history.commit(&session));
    assert!(!history.can_undo());
}

#[test]
fn a_new_step_throws_away_the_redo_stack() {
    let mut session = session();
    let mut history = History::default();
    gesture(&mut history, &mut session, |s| {
        first_track(s).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    });
    history.undo(&mut session);
    assert!(history.can_redo());

    gesture(&mut history, &mut session, |s| {
        first_track(s).notes.push(Note::new(8.0, 64, 1.0, 100, 0.0));
    });
    assert!(!history.can_redo(), "there is no longer one future to redo into");
}

#[test]
fn the_stack_stops_growing_at_the_limit_and_drops_the_oldest() {
    let mut session = session();
    let mut history = History::default();
    for step in 0..HISTORY_MAX + 20 {
        gesture(&mut history, &mut session, |s| {
            first_track(s).notes.push(Note::new(step as f64 % 16.0, 60, 1.0, 100, 0.0));
        });
    }
    assert_eq!(history.depth(), (HISTORY_MAX, 0));
    // The oldest steps are gone, so undoing all the way back does not reach empty.
    while history.undo(&mut session) {}
    assert_eq!(note_count(&session), 20);
}

// --- the line between the music and the desk ---------------------------------

#[test]
fn the_tempo_is_not_undoable() {
    let mut session = session();
    let mut history = History::default();
    assert!(!gesture(&mut history, &mut session, |s| s.tempo_bpm = 174.0));
    assert_eq!(session.tempo_bpm, 174.0, "and it stays where it was put");
    assert!(!history.can_undo());
}

#[test]
fn a_port_binding_is_not_undoable() {
    // The case the module header argues from: undo must not silently stop a box.
    let mut session = session();
    let mut history = History::default();
    let device = session.devices[0].id;
    let port = PortRef { id: String::from("out-1"), name: String::from("Digitakt II") };
    assert!(!gesture(&mut history, &mut session, |s| {
        s.device_mut(device).unwrap().io.output = Some(port.clone());
    }));
    assert!(session.devices[0].io.output.is_some());
    assert!(!history.can_undo());
}

#[test]
fn the_scenes_are_not_undoable() {
    let mut session = session();
    let mut history = History::default();
    assert!(!gesture(&mut history, &mut session, |s| {
        s.add_scene("Scene 2", None);
    }));
    assert_eq!(session.scenes.len(), 2);
    assert!(!history.can_undo());
}

#[test]
fn everything_in_a_pattern_is_undoable_including_swing_and_the_lanes() {
    // The other side of the same line: a pattern's own bytes are music.
    for (what, edit) in [
        ("swing", (|s: &mut Session| {
            let d = s.devices[0].id;
            s.device_mut(d).unwrap().pattern_mut(0).unwrap().swing = 66;
        }) as fn(&mut Session)),
        ("track length", |s: &mut Session| first_track(s).length_steps = 32),
        ("the PROB default", |s: &mut Session| first_track(s).track_prob = 40),
        ("mute", |s: &mut Session| first_track(s).mute = true),
        ("a p-lock lane", |s: &mut Session| {
            first_track(s).plocks.push(
                digi_core::PLockLane::new(
                    Some(String::from("filter.cutoff")),
                    None,
                    Some(String::from("DT2")),
                    false,
                    vec![Some(64)],
                )
                .unwrap(),
            );
        }),
    ] {
        let mut session = session();
        let mut history = History::default();
        assert!(gesture(&mut history, &mut session, edit), "{what} should be a step");
        assert!(history.undo(&mut session), "{what} should undo");
    }
}

#[test]
fn a_pattern_in_a_slot_nobody_is_looking_at_still_undoes() {
    // The roll edits one of 32 tracks, but an import or a generator can change a
    // slot that is not on screen, and a step that only undid the visible slot
    // would leave the rest of the change behind.
    let mut session = session();
    let mut history = History::default();
    let second = session.devices[1].id;
    assert!(gesture(&mut history, &mut session, |s| {
        s.device_mut(second)
            .unwrap()
            .pattern_mut(7)
            .unwrap()
            .track_mut(3)
            .unwrap()
            .notes
            .push(Note::new(0.0, 60, 1.0, 100, 0.0));
    }));
    history.undo(&mut session);
    assert!(session.devices[1].pattern(7).unwrap().track(3).unwrap().notes.is_empty());
}

// --- the awkward cases -------------------------------------------------------

#[test]
fn a_step_over_a_box_that_has_since_been_removed_is_skipped_not_applied() {
    // Setup can remove a box while there are steps behind it. Putting its
    // patterns back is impossible; getting stuck on the step is worse.
    let mut session = session();
    let mut history = History::default();
    gesture(&mut history, &mut session, |s| {
        first_track(s).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    });
    let gone = session.devices[0].id;
    session.remove_device(gone);

    // The step is consumed and the cursor moves, even though nothing was put back.
    assert!(!history.undo(&mut session));
    assert!(!history.can_undo(), "the step was spent rather than left to jam the stack");
    assert!(history.can_redo());
}

#[test]
fn a_box_added_after_a_step_keeps_its_patterns_through_an_undo() {
    // The snapshot names devices by id, so a box that was not in it is not
    // touched — an undo cannot delete a box's music by not knowing about it.
    let mut session = session();
    let mut history = History::default();
    gesture(&mut history, &mut session, |s| {
        first_track(s).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    });
    let model = model_for_key("DT2").unwrap();
    let newcomer = session.add_device(Device::new("Adeel's DT2", model, 16));
    session
        .device_mut(newcomer)
        .unwrap()
        .pattern_mut(0)
        .unwrap()
        .track_mut(0)
        .unwrap()
        .notes
        .push(Note::new(4.0, 48, 1.0, 100, 0.0));

    history.undo(&mut session);
    assert_eq!(
        session.device(newcomer).unwrap().pattern(0).unwrap().track(0).unwrap().notes.len(),
        1,
        "the newcomer's note is not in the step, so the step leaves it alone"
    );
    assert_eq!(note_count(&session), 0, "and the step it does name still undid");
}

#[test]
fn a_step_over_a_box_whose_slot_count_has_changed_is_skipped() {
    // **Found by a plant that failed nothing.** The guard in `Content::restore` had
    // no test, and without it the restore replaces the whole pattern vector — so a
    // box holding eight slots would silently come back holding sixteen, half of them
    // from a shape it no longer has.
    //
    // Nothing in the app can reach this today: a device's slot count is fixed when
    // it is built and no panel resizes it. It is constructed by hand here because a
    // guard whose absence is invisible is a guard nobody can trust, and this is
    // cheaper than deleting it and finding out later.
    let mut session = session();
    let mut history = History::default();
    gesture(&mut history, &mut session, |s| {
        first_track(s).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    });

    let device = session.devices[0].id;
    session.device_mut(device).unwrap().patterns.truncate(8);

    assert!(!history.undo(&mut session), "nothing was put back");
    assert_eq!(
        session.devices[0].patterns.len(),
        8,
        "and the box was not grown back to a shape it no longer has"
    );
}

#[test]
fn undo_and_redo_on_an_empty_stack_do_nothing_rather_than_panicking() {
    let mut session = session();
    let mut history = History::default();
    assert!(!history.undo(&mut session));
    assert!(!history.redo(&mut session));
}

#[test]
fn abandoning_an_open_step_pushes_nothing() {
    // Opening a project mid-gesture: the step would be measured against music
    // that is no longer in the window.
    let mut session = session();
    let mut history = History::default();
    history.begin(Content::of(&session));
    first_track(&mut session).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    history.abandon();
    assert!(!history.is_open());
    assert!(!history.commit(&session));
    assert!(!history.can_undo());
}

#[test]
fn clearing_the_history_leaves_the_session_alone() {
    let mut session = session();
    let mut history = History::default();
    gesture(&mut history, &mut session, |s| {
        first_track(s).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    });
    history.clear();
    assert_eq!(history.depth(), (0, 0));
    assert_eq!(note_count(&session), 1, "the music in hand is not the history");
}

#[test]
fn a_snapshot_shares_its_patterns_until_one_is_edited() {
    // The claim that makes taking one of these every frame affordable. Not a
    // micro-optimisation: it is why `begin` can run unconditionally.
    let session = session();
    let snapshot = Content::of(&session);
    assert!(snapshot.matches(&session));

    let mut edited = session.clone();
    first_track(&mut edited).notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    assert!(!snapshot.matches(&edited));
    // And the untouched box is still the same allocation on both sides.
    assert!(std::sync::Arc::ptr_eq(&session.devices[1].patterns[0], &edited.devices[1].patterns[0]));
}
