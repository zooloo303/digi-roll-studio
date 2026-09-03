//! The Edit panel's decisions, driven without a window.
//!
//! Phase 9's exit criteria that are not in the roll live here. As with
//! `session_panel.rs`, the native file dialog is stood in for by a
//! [`ScriptedChooser`] — the interesting half of "export this as a MIDI file" is
//! which bytes are written and what the panel then says, not the syscall.
//!
//! **The undo boundary is tested here rather than in `core`** because `core` can
//! only say what a step *contains*; what closes one is the shell's rule, and the
//! part worth pinning is that an undo does not itself become a step.
//!
//! What no test in this file can claim: that the native dialog opens, or that any
//! of these controls is legible on screen. Both are run-the-app-and-look checks,
//! like the glyph table in `ui::mod`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use digi_core::edit_ops::{clear_track, duplicate_last_bar, transpose_track, Transposed, OCTAVE};
use digi_core::history::{Content, History};
use digi_core::midifile::{midi_file_to_notes, track_to_midi_file};
use digi_core::{two_box_session, Note, Session};
use digi_roll_studio::ui::edit::{EditPanel, Status};
use digi_roll_studio::ui::pianoroll::PianoRoll;
use digi_roll_studio::ui::tracks::{track, track_mut, Selection};
use eframe::egui;

// ------------------------------------------------------------- the harness

#[derive(Default)]
struct Script {
    export_answers: Vec<Option<PathBuf>>,
    open_answers: Vec<Option<PathBuf>>,
    calls: Vec<&'static str>,
    /// The filename offered to the last MIDI export.
    suggested: Option<String>,
}

struct ScriptedChooser(Rc<RefCell<Script>>);

impl digi_roll_studio::ui::session::Chooser for ScriptedChooser {
    fn save_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        self.0.borrow_mut().calls.push("save");
        None
    }

    fn open(&mut self) -> Option<PathBuf> {
        self.0.borrow_mut().calls.push("open");
        None
    }

    fn export_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        self.0.borrow_mut().calls.push("export");
        None
    }

    fn export_midi_as(&mut self, suggested: &str) -> Option<PathBuf> {
        let mut s = self.0.borrow_mut();
        s.calls.push("export-midi");
        s.suggested = Some(suggested.to_string());
        if s.export_answers.is_empty() { None } else { s.export_answers.remove(0) }
    }

    fn open_midi(&mut self) -> Option<PathBuf> {
        let mut s = self.0.borrow_mut();
        s.calls.push("open-midi");
        if s.open_answers.is_empty() { None } else { s.open_answers.remove(0) }
    }
}

fn panel() -> (EditPanel, Rc<RefCell<Script>>) {
    let script = Rc::new(RefCell::new(Script::default()));
    let panel = EditPanel::with_chooser(Box::new(ScriptedChooser(Rc::clone(&script))));
    (panel, script)
}

fn tmp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("digi-roll-edit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    dir
}

const FIRST: Selection = Selection { device: 0, track: 0 };

/// The default session, with three notes on the track the panel will be aimed at
/// and a swing that is not the default — so a test can see a value being read
/// rather than a default being coincidentally right.
///
/// **One of the notes is on an odd step, and that is load-bearing.** Swing only
/// moves odd steps, so a fixture whose notes all sit on even ones exports the same
/// bytes at swing 50 as at swing 66 — which is `DEVELOPMENT.md` lesson 2 exactly: a
/// committed witness no test can see. The first version of this fixture had notes
/// at 0, 4 and 12, and the swing test passed by being unable to fail.
fn seeded() -> Session {
    let mut session = two_box_session();
    let device = session.devices[0].id;
    let pattern = session.device_mut(device).unwrap().pattern_mut(0).unwrap();
    pattern.swing = 66;
    let t = pattern.track_mut(0).unwrap();
    t.notes = vec![
        Note::new(0.0, 60, 1.0, 100, 0.0),
        Note::new(5.0, 63, 2.0, 40, 0.25),
        Note::new(12.0, 67, 1.0, 127, 0.0),
    ];
    session
}

// ------------------------------------------------------------- MIDI export

#[test]
fn an_export_writes_a_file_the_import_can_read_back() {
    let (mut panel, script) = panel();
    let session = seeded();
    let path = tmp_dir("export").join("out.mid");
    script.borrow_mut().export_answers.push(Some(path.clone()));

    assert!(panel.export_midi(&session, FIRST));
    assert_eq!(script.borrow().calls, ["export-midi"]);

    let bytes = std::fs::read(&path).expect("the file the panel wrote");
    let back = midi_file_to_notes(&bytes, 128).expect("and it is a MIDI file");
    assert_eq!(back.notes.len(), 3);
    assert!(matches!(panel.status(), Some(Status::Exported { notes: 3, .. })));
}

#[test]
fn the_exported_filename_is_the_slot_and_the_track() {
    // What makes a file recognisable a week later. `A01 T1`, not `pattern.mid`.
    let (mut panel, script) = panel();
    let session = seeded();
    let _ = panel.export_midi(&session, FIRST);
    assert_eq!(script.borrow().suggested.as_deref(), Some("A01 T1.mid"));
}

#[test]
fn the_export_carries_the_patterns_swing_not_a_default() {
    // Swing is a per-pattern byte and the export bakes it into tick positions, so
    // an export that read 50 off the wrong place would silently straighten the
    // music. The fixture is at 66 to make that visible.
    let (mut panel, script) = panel();
    let session = seeded();
    let path = tmp_dir("swing").join("out.mid");
    script.borrow_mut().export_answers.push(Some(path.clone()));
    panel.export_midi(&session, FIRST);

    let straight = {
        let mut s = session.clone();
        let device = s.devices[0].id;
        s.device_mut(device).unwrap().pattern_mut(0).unwrap().swing = 50;
        let track = track(&s, FIRST).unwrap();
        track_to_midi_file(track, "A01 T1", 50, s.tempo_bpm)
    };
    let written = std::fs::read(&path).unwrap();
    assert_ne!(written, straight, "the export used the pattern's own swing");
}

#[test]
fn a_cancelled_export_writes_nothing_and_says_nothing() {
    // A cancelled dialog is a normal answer, never an error — the same rule the
    // Session panel's chooser follows.
    let (mut panel, script) = panel();
    let session = seeded();
    assert!(!panel.export_midi(&session, FIRST));
    assert_eq!(script.borrow().calls, ["export-midi"]);
    assert!(panel.status().is_none());
}

#[test]
fn an_export_to_a_path_that_cannot_be_written_says_which() {
    let (mut panel, script) = panel();
    let session = seeded();
    let path = tmp_dir("unwritable").join("no-such-dir").join("out.mid");
    script.borrow_mut().export_answers.push(Some(path));
    assert!(!panel.export_midi(&session, FIRST));
    let Some(Status::Failed(why)) = panel.status() else { panic!("expected a refusal") };
    assert!(why.contains("could not write"), "{why}");
}

// ------------------------------------------------------------- MIDI import

#[test]
fn an_import_replaces_the_tracks_notes_and_its_length() {
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    // A file whose music needs two bars, built by the codec so the fixture cannot
    // drift from what the parser expects.
    let source = {
        let mut s = two_box_session();
        let device = s.devices[0].id;
        let t = s.device_mut(device).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap();
        t.length_steps = 32;
        t.notes = vec![Note::new(0.0, 48, 1.0, 90, 0.0), Note::new(20.0, 55, 4.0, 70, 0.0)];
        track_to_midi_file(t, "src", 50, 120.0)
    };
    let path = tmp_dir("import").join("in.mid");
    std::fs::write(&path, source).unwrap();

    assert!(panel.import_midi_from(&path, &mut session, FIRST, &mut roll));
    let t = track(&session, FIRST).unwrap();
    assert_eq!(t.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [48, 55]);
    assert_eq!(t.length_steps, 32, "the file needed two bars, so the track is two bars");
    assert!(matches!(panel.status(), Some(Status::Imported { notes: 2, dropped: 0, .. })));
}

#[test]
fn an_import_takes_the_p_lock_lanes_and_the_provenance_with_the_notes() {
    // Locks ride on trigs. Lanes left behind after the music is replaced would be
    // locked to notes that no longer exist, and a pattern that still claimed to be
    // a copy of A01 would send this file back to the box as if it were that slot.
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let device = session.devices[0].id;
    {
        let pattern = session.device_mut(device).unwrap().pattern_mut(0).unwrap();
        pattern.source = Some(digi_core::Source {
            device_slug: String::from("digitakt2"),
            bank: 0,
            index: 0,
        });
        let t = pattern.track_mut(0).unwrap();
        t.plocks.push(
            digi_core::PLockLane::new(
                Some(String::from("filter.cutoff")),
                None,
                Some(String::from("DT2")),
                false,
                vec![Some(64)],
            )
            .unwrap(),
        );
    }

    let source = track_to_midi_file(track(&session, FIRST).unwrap(), "src", 50, 120.0);
    let path = tmp_dir("import-lanes").join("in.mid");
    std::fs::write(&path, source).unwrap();
    panel.import_midi_from(&path, &mut session, FIRST, &mut roll);

    assert!(track(&session, FIRST).unwrap().plocks.is_empty(), "the lanes went");
    assert!(
        session.device(device).unwrap().pattern(0).unwrap().source.is_none(),
        "and so did the claim to be a copy of a slot"
    );
}

#[test]
fn an_import_forgets_the_selection_because_those_ids_name_nothing_now() {
    // **This test could not fail once.** Its first version asserted the selection
    // was empty on a `PianoRoll::default()`, which starts empty — so the plant that
    // took `clear_selection()` out of the import passed. `PianoRoll::select` exists
    // because of that: the roll has to actually be holding a selection for the
    // claim to mean anything.
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let source = track_to_midi_file(track(&session, FIRST).unwrap(), "src", 50, 120.0);
    let path = tmp_dir("import-selection").join("in.mid");
    std::fs::write(&path, source).unwrap();

    let held: Vec<u32> = track(&session, FIRST).unwrap().notes.iter().map(|n| n.id).collect();
    roll.select(held.clone());
    assert_eq!(roll.selection(), held, "the fixture really is holding a selection");

    panel.import_midi_from(&path, &mut session, FIRST, &mut roll);
    assert!(
        roll.selection().is_empty(),
        "those ids name notes that are no longer in the track"
    );
}

#[test]
fn a_file_that_is_not_a_midi_file_leaves_the_track_exactly_as_it_was() {
    // The rule `session::open_from` follows: nothing is touched until the file has
    // been proved good, because a half-applied import leaves the slot holding
    // neither the old music nor the new.
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let before = track(&session, FIRST).unwrap().clone();
    let path = tmp_dir("not-midi").join("in.mid");
    std::fs::write(&path, b"this is not a MIDI file").unwrap();

    assert!(!panel.import_midi_from(&path, &mut session, FIRST, &mut roll));
    assert_eq!(track(&session, FIRST).unwrap(), &before);
    let Some(Status::Failed(why)) = panel.status() else { panic!("expected a refusal") };
    assert!(why.contains("not a MIDI file"), "{why}");
}

#[test]
fn a_missing_file_says_so_rather_than_emptying_the_track() {
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let path = tmp_dir("missing").join("nope.mid");

    assert!(!panel.import_midi_from(&path, &mut session, FIRST, &mut roll));
    assert_eq!(track(&session, FIRST).unwrap().notes.len(), 3);
    let Some(Status::Failed(why)) = panel.status() else { panic!("expected a refusal") };
    assert!(why.contains("could not read"), "{why}");
}

#[test]
fn a_midi_file_with_no_notes_is_refused_rather_than_emptying_the_track() {
    // The case that would be most annoying to discover afterwards: importing a
    // tempo-only track and finding the slot wiped.
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let mut empty = Vec::new();
    empty.extend_from_slice(b"MThd");
    empty.extend_from_slice(&6u32.to_be_bytes());
    empty.extend_from_slice(&[0, 0, 0, 1]);
    empty.extend_from_slice(&96u16.to_be_bytes());
    let body = [0x00u8, 0xff, 0x2f, 0x00];
    empty.extend_from_slice(b"MTrk");
    empty.extend_from_slice(&(body.len() as u32).to_be_bytes());
    empty.extend_from_slice(&body);
    let path = tmp_dir("no-notes").join("in.mid");
    std::fs::write(&path, empty).unwrap();

    assert!(!panel.import_midi_from(&path, &mut session, FIRST, &mut roll));
    assert_eq!(track(&session, FIRST).unwrap().notes.len(), 3);
    let Some(Status::Failed(why)) = panel.status() else { panic!("expected a refusal") };
    assert!(why.contains("no notes found"), "{why}");
}

#[test]
fn a_file_whose_notes_all_land_past_the_limit_says_so_rather_than_no_notes_found() {
    // The fault a hardware session found, in miniature. A ten-track Star Wars
    // arrangement reported `no notes found` for a file holding ~690 notes: the
    // first note-bearing track was a bass that does not enter until step 139, a
    // box holds 128, so every one of its notes was dropped as out of range and
    // the message named the wrong cause entirely.
    //
    // This is the sibling of the test above, and the pair is the point: that one
    // has `dropped == 0` and this one has `dropped > 0`, so a single message for
    // both cases cannot pass them both. Restoring the old one-line version fails
    // exactly here.
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let source = {
        let mut s = two_box_session();
        let device = s.devices[0].id;
        let t = s.device_mut(device).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap();
        // Past MAX_STEPS (128), the way the real file's first track was. The
        // exporter writes what the note vec holds rather than what the track
        // length allows, so this reaches the file and comes back dropped.
        t.notes = vec![Note::new(139.0, 43, 1.0, 90, 0.0), Note::new(150.0, 45, 1.0, 90, 0.0)];
        track_to_midi_file(t, "far-out", 50, 120.0)
    };
    let path = tmp_dir("import-all-dropped").join("in.mid");
    std::fs::write(&path, source).unwrap();

    assert!(!panel.import_midi_from(&path, &mut session, FIRST, &mut roll));
    assert_eq!(track(&session, FIRST).unwrap().notes.len(), 3, "the track is untouched");
    let Some(Status::Failed(why)) = panel.status() else { panic!("expected a refusal") };
    assert!(why.contains('2'), "it names how many notes were actually found: {why}");
    assert!(why.contains("past 8 bars"), "and why they did not make it: {why}");
    assert!(
        !why.contains("no notes found"),
        "the message that sent someone looking for an empty file: {why}"
    );
}

#[test]
fn a_cancelled_import_touches_nothing() {
    let (mut panel, script) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    assert!(!panel.import_midi(&mut session, FIRST, &mut roll));
    assert_eq!(script.borrow().calls, ["open-midi"]);
    assert_eq!(track(&session, FIRST).unwrap().notes.len(), 3);
    assert!(panel.status().is_none());
}

#[test]
fn a_round_trip_through_a_file_keeps_the_notes_and_loses_the_conditions() {
    // The claim the panel makes on screen before the button is pressed, end to end
    // through two real files. If this ever stops being true the panel's wording is
    // what has to change.
    let (mut panel, script) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    {
        let device = session.devices[0].id;
        let t = session.device_mut(device).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap();
        t.notes[0].cond = Some(String::from("2:4"));
        t.notes[0].prob = Some(50);
    }
    let path = tmp_dir("roundtrip").join("rt.mid");
    script.borrow_mut().export_answers.push(Some(path.clone()));
    assert!(panel.export_midi(&session, FIRST));

    assert!(panel.import_midi_from(&path, &mut session, FIRST, &mut roll));
    let t = track(&session, FIRST).unwrap();
    assert_eq!(t.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [60, 63, 67]);
    assert!(t.notes.iter().all(|n| n.cond.is_none() && n.prob.is_none()));
}

// ------------------------------------------------------------- the undo boundary
//
// One gesture is one step, and an undo is not one. Driven the way the shell drives
// it, because that ordering *is* the rule — `core::history` cannot check it.

/// The shell's frame, reduced to the parts that decide a step: snapshot before the
/// edits, `begin` if anything changed, `commit` when the pointer is up.
fn frame(
    history: &mut History,
    session: &mut Session,
    pointer_down: bool,
    edit: impl FnOnce(&mut Session) -> bool,
) {
    let before = (!history.is_open()).then(|| Content::of(session));
    let edited = edit(session);
    if edited {
        if let Some(before) = before {
            history.begin(before);
        }
    }
    if !pointer_down {
        history.commit(session);
    }
}

#[test]
fn a_duplicate_bar_is_one_step_and_undoes_whole() {
    let mut session = seeded();
    let mut history = History::default();
    frame(&mut history, &mut session, false, |s| {
        duplicate_last_bar(track_mut(s, FIRST).unwrap()).is_some()
    });

    assert_eq!(track(&session, FIRST).unwrap().length_steps, 32);
    assert!(history.undo(&mut session));
    let t = track(&session, FIRST).unwrap();
    assert_eq!(t.length_steps, 16, "the length came back with the notes");
    assert_eq!(t.notes.len(), 3);
}

#[test]
fn a_transpose_is_one_step_and_undoes_whole() {
    let mut session = seeded();
    let mut history = History::default();
    let before: Vec<u8> = track(&session, FIRST).unwrap().notes.iter().map(|n| n.pitch).collect();

    frame(&mut history, &mut session, false, |s| {
        matches!(
            transpose_track(track_mut(s, FIRST).unwrap(), OCTAVE),
            Transposed::Moved { .. }
        )
    });
    let moved: Vec<u8> = track(&session, FIRST).unwrap().notes.iter().map(|n| n.pitch).collect();
    assert_eq!(moved, before.iter().map(|p| p + 12).collect::<Vec<_>>());

    assert!(history.undo(&mut session));
    let back: Vec<u8> = track(&session, FIRST).unwrap().notes.iter().map(|n| n.pitch).collect();
    assert_eq!(back, before, "one press, one step, all the way back");
}

/// A refused transpose must not open a step of its own. Otherwise Cmd+Z after a
/// press that visibly did nothing would take back whatever came *before* it.
#[test]
fn a_transpose_with_no_room_leaves_no_undo_step() {
    let mut session = seeded();
    let mut history = History::default();
    track_mut(&mut session, FIRST).unwrap().notes = vec![Note::new(0.0, 126, 1.0, 100, 0.0)];

    frame(&mut history, &mut session, false, |s| {
        matches!(
            transpose_track(track_mut(s, FIRST).unwrap(), OCTAVE),
            Transposed::Moved { .. }
        )
    });

    assert_eq!(history.depth(), (0, 0));
    assert_eq!(track(&session, FIRST).unwrap().notes[0].pitch, 126);
}

#[test]
fn a_clear_undoes_the_lanes_along_with_the_notes() {
    let mut session = seeded();
    let mut history = History::default();
    {
        let t = track_mut(&mut session, FIRST).unwrap();
        t.plocks.push(
            digi_core::PLockLane::new(
                Some(String::from("filter.cutoff")),
                None,
                Some(String::from("DT2")),
                false,
                vec![Some(64)],
            )
            .unwrap(),
        );
    }
    frame(&mut history, &mut session, false, |s| clear_track(track_mut(s, FIRST).unwrap()));
    assert!(track(&session, FIRST).unwrap().notes.is_empty());

    history.undo(&mut session);
    let t = track(&session, FIRST).unwrap();
    assert_eq!(t.notes.len(), 3);
    assert_eq!(t.plocks.len(), 1, "the automation came back with the trigs it rode on");
}

#[test]
fn a_slider_dragged_across_frames_is_one_step() {
    // The shape every control in the panel relies on. `js/main.js` gets this from a
    // latch per widget (`velGesture`, `lenGesture`, `trackProbGesture`); the shell
    // gets it once, from the pointer.
    let mut session = seeded();
    let mut history = History::default();
    for swing in 51..=70u8 {
        frame(&mut history, &mut session, true, |s| {
            let device = s.devices[0].id;
            let pattern = s.device_mut(device).unwrap().pattern_mut(0).unwrap();
            let moved = pattern.swing != swing;
            pattern.swing = swing;
            moved
        });
    }
    // The release frame: the pointer is up and nothing further changed.
    frame(&mut history, &mut session, false, |_| false);

    assert_eq!(history.depth(), (1, 0), "twenty frames of dragging, one step");
    history.undo(&mut session);
    assert_eq!(
        session.device(session.devices[0].id).unwrap().pattern(0).unwrap().swing,
        66,
        "back to where the drag started, not to its second-last frame"
    );
}

#[test]
fn an_undo_does_not_itself_become_a_step() {
    // **The bug `Outcome::stepped` exists to prevent.** If the shell folded a
    // history move into `edited`, the next commit would push the post-undo state
    // and the button would alternate between two states forever.
    let mut session = seeded();
    let mut history = History::default();
    frame(&mut history, &mut session, false, |s| {
        clear_track(track_mut(s, FIRST).unwrap())
    });
    assert_eq!(history.depth(), (1, 0));

    // The shell's undo frame: `stepped`, not `edited`, so no `begin` happens — but
    // `commit` still runs, as it does on every frame the pointer is up.
    history.abandon();
    assert!(history.undo(&mut session));
    history.commit(&session);

    assert_eq!(history.depth(), (0, 1), "one step forward to redo into, none behind");
    assert_eq!(track(&session, FIRST).unwrap().notes.len(), 3);

    // And redo goes back the other way rather than getting stuck.
    history.abandon();
    assert!(history.redo(&mut session));
    history.commit(&session);
    assert_eq!(history.depth(), (1, 0));
    assert!(track(&session, FIRST).unwrap().notes.is_empty());
}

#[test]
fn an_undo_mid_gesture_abandons_the_open_step_rather_than_committing_it() {
    // Reachable from the keyboard: Cmd+Z while a drag is still held. Committing the
    // half-finished gesture would leave a step measured against music that is about
    // to be replaced.
    let mut session = seeded();
    let mut history = History::default();
    frame(&mut history, &mut session, false, |s| {
        clear_track(track_mut(s, FIRST).unwrap())
    });
    // A drag begins and is still held.
    frame(&mut history, &mut session, true, |s| {
        track_mut(s, FIRST).unwrap().track_prob = 40;
        true
    });
    assert!(history.is_open());

    history.abandon();
    history.undo(&mut session);
    assert!(!history.is_open());
    assert_eq!(track(&session, FIRST).unwrap().notes.len(), 3, "the clear undid");
}

#[test]
fn an_import_is_an_ordinary_step_and_undoes() {
    // It replaces notes, length, lanes and provenance in one call, so it has to
    // come back the same way.
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let mut history = History::default();
    let source = {
        let mut s = two_box_session();
        let device = s.devices[0].id;
        let t = s.device_mut(device).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap();
        t.notes = vec![Note::new(0.0, 48, 1.0, 90, 0.0)];
        track_to_midi_file(t, "src", 50, 120.0)
    };
    let path = tmp_dir("import-undo").join("in.mid");
    std::fs::write(&path, source).unwrap();

    frame(&mut history, &mut session, false, |s| {
        panel.import_midi_from(&path, s, FIRST, &mut roll)
    });
    assert_eq!(track(&session, FIRST).unwrap().notes.len(), 1);

    history.undo(&mut session);
    let t = track(&session, FIRST).unwrap();
    assert_eq!(t.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [60, 63, 67]);
}

#[test]
fn a_frame_that_only_moved_the_desk_leaves_no_step() {
    // Not a special case anywhere — it falls out of `commit` comparing content. It
    // is asserted here because it is the mechanism the whole "the desk is not
    // history" rule rests on.
    let mut session = seeded();
    let mut history = History::default();
    frame(&mut history, &mut session, false, |s| {
        s.tempo_bpm = 174.0;
        true // the transport panel really does report this as an edit
    });
    assert_eq!(history.depth(), (0, 0));
    assert_eq!(session.tempo_bpm, 174.0);
}

// ------------------------------------------------------------- drawing it
//
// **What this can and cannot say.** It runs the whole panel body through a real
// egui pass, so it catches a layout that panics, an `id_salt` that clashes and a
// widget handed a value outside its own range — the three ways a panel fails
// without anyone touching it. It cannot say the panel is *legible*, and it cannot
// reach the clear-confirmation modal, which is behind a click at a position this
// harness does not know. That one is a run-the-app-and-look check, listed in
// `DEVELOPMENT.md` the way the Session panel's close guard was.

/// Draw the panel once, in whatever state it is in, and hand back what changed.
fn draw(
    ctx: &egui::Context,
    panel: &mut EditPanel,
    session: &mut Session,
    roll: &mut PianoRoll,
    history: &mut History,
    events: Vec<egui::Event>,
) -> digi_roll_studio::ui::edit::Outcome {
    let mut out = digi_roll_studio::ui::edit::Outcome::default();
    let input = egui::RawInput {
        events,
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(272.0, 900.0),
        )),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        out = panel.ui(ui, session, FIRST, roll, history);
    });
    // Same reason the roll's tests clear it: epaint's debug assert fires on a
    // dropped font-atlas delta.
    output.textures_delta.clear();
    out
}

#[test]
fn the_panel_draws_every_group_without_panicking() {
    let ctx = egui::Context::default();
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let mut history = History::default();

    // Nothing selected, no lanes, no history — the state the app opens on.
    for _ in 0..2 {
        let out = draw(&ctx, &mut panel, &mut session, &mut roll, &mut history, vec![]);
        assert!(!out.close && !out.edited && !out.stepped, "drawing is not editing");
    }

    // With a selection, a lane, a full-length track and a step on the stack — the
    // other end of every branch in the body.
    let held: Vec<u32> = track(&session, FIRST).unwrap().notes.iter().map(|n| n.id).collect();
    roll.select(held);
    {
        let t = track_mut(&mut session, FIRST).unwrap();
        t.length_steps = 128;
        t.plocks.push(
            digi_core::PLockLane::new(
                Some(String::from("filter.cutoff")),
                None,
                Some(String::from("DT2")),
                false,
                vec![Some(64)],
            )
            .unwrap(),
        );
        // A read-only lane too: its row takes the other arm of the remove tooltip.
        t.plocks.push(
            digi_core::PLockLane::new(None, Some(200), Some(String::from("DT2")), true, vec![])
                .unwrap(),
        );
    }
    history.begin(Content::of(&session));
    history.commit(&session);
    for _ in 0..2 {
        draw(&ctx, &mut panel, &mut session, &mut roll, &mut history, vec![]);
    }
}

/// The transpose row's other arm: a track pinned against both ends of the MIDI
/// range, so every one of the four buttons draws disabled and takes the
/// no-room branch of its own hover text.
#[test]
fn the_panel_draws_a_transpose_row_with_no_room_in_either_direction() {
    let ctx = egui::Context::default();
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let mut history = History::default();
    track_mut(&mut session, FIRST).unwrap().notes =
        vec![Note::new(0.0, 0, 1.0, 100, 0.0), Note::new(4.0, 127, 1.0, 100, 0.0)];
    for _ in 0..2 {
        draw(&ctx, &mut panel, &mut session, &mut roll, &mut history, vec![]);
    }

    // And the empty-track arm, which is a third branch again: nothing to move
    // rather than nowhere to move it.
    track_mut(&mut session, FIRST).unwrap().notes.clear();
    for _ in 0..2 {
        draw(&ctx, &mut panel, &mut session, &mut roll, &mut history, vec![]);
    }
}

#[test]
fn the_panel_draws_over_a_selection_that_names_nothing() {
    // Reachable: a selection is ids, and an undo can take the notes they name out
    // from under it. Every group has to survive that rather than unwrapping on it.
    let ctx = egui::Context::default();
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let mut history = History::default();
    roll.select([999_999, 1_000_000]);
    for _ in 0..2 {
        draw(&ctx, &mut panel, &mut session, &mut roll, &mut history, vec![]);
    }
}

#[test]
fn the_panel_draws_when_the_selection_points_at_no_track_at_all() {
    // A device removed from Setup while the roll was on one of its tracks. The
    // panel says so instead of drawing controls aimed at nothing.
    let ctx = egui::Context::default();
    let (mut panel, _) = panel();
    let mut session = seeded();
    let mut roll = PianoRoll::default();
    let mut history = History::default();
    session.devices.clear();
    for _ in 0..2 {
        let out = draw(&ctx, &mut panel, &mut session, &mut roll, &mut history, vec![]);
        assert!(!out.edited && !out.stepped);
    }
}
