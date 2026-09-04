//! The crash copy, driven without a window and without a clock.
//!
//! Two halves, and they are separate on purpose. `ui::recovery::Cadence` holds
//! no clock — every method takes the `Instant` — so the timing tests below drive
//! a minute of continuous editing in no time at all, rather than sleeping
//! through it and making the suite two seconds slower per assertion.
//! `ui::recovery::Recovery` is the disk half, and it gets a temp directory of
//! its own.
//!
//! The wiring tests then check the thing neither half can see alone: **that
//! every path which ends the unsaved work also takes the copy away.** A crash
//! copy that outlives its session is not a harmless leftover — it is an offer,
//! on the next launch, to recover work that is already on disk, and an offer
//! like that is one you learn to dismiss without reading.
//!
//! The one claim no test here makes is that the app actually crashes and comes
//! back. That is a run-it-and-pull-the-plug check, like the glyph table in
//! `ui::mod`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use digi_core::model::Note;
use digi_core::project::Project;
use digi_core::{two_box_session, Session};
use digi_roll_studio::ui::recovery::{Cadence, Recovery, CEILING, QUIET};
use digi_roll_studio::ui::session::{Chooser, SessionPanel, Status};

// ------------------------------------------------------------- the harness

/// A [`Chooser`] that answers Save As from a queue and everything else `None`.
///
/// Smaller than `session_panel.rs`'s scripted chooser because nothing in this
/// file opens a dialog on purpose: the whole point of a crash copy is that it is
/// written without one.
struct Answers(Rc<RefCell<Vec<Option<PathBuf>>>>);

impl Chooser for Answers {
    fn save_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        let mut a = self.0.borrow_mut();
        if a.is_empty() { None } else { a.remove(0) }
    }
    fn open(&mut self) -> Option<PathBuf> {
        None
    }
    fn export_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        None
    }
    fn export_midi_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        None
    }
    fn open_midi(&mut self) -> Option<PathBuf> {
        None
    }
}

fn tmp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("digi-roll-recovery-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    dir
}

/// A session with something in it worth losing.
fn seeded() -> Session {
    let mut s = two_box_session();
    s.name = "Desk".into();
    s.tempo_bpm = 137.0;
    let dt2 = s.devices[0].id;
    s.device_mut(dt2)
        .unwrap()
        .pattern_mut(0)
        .unwrap()
        .track_mut(3)
        .unwrap()
        .notes
        .push(Note::new(4.0, 60, 1.0, 100, 0.25));
    s
}

/// A panel with a chooser that answers nothing and a store of its own.
fn panel(dir: &std::path::Path) -> SessionPanel {
    SessionPanel::with_chooser(Box::new(Answers(Rc::new(RefCell::new(Vec::new())))))
        .with_recovery(Recovery::at(dir))
}

/// A panel whose next Save As answers `path`.
fn panel_saving_to(dir: &std::path::Path, path: PathBuf) -> SessionPanel {
    SessionPanel::with_chooser(Box::new(Answers(Rc::new(RefCell::new(vec![Some(path)])))))
        .with_recovery(Recovery::at(dir))
}

// --------------------------------------------------------------- the cadence

#[test]
fn an_app_nobody_is_touching_never_writes_a_copy() {
    let c = Cadence::default();
    let now = Instant::now();
    assert!(!c.due(now));
    assert!(!c.due(now + CEILING * 10), "an idle app has no clock running");
    assert!(!c.pending());
}

#[test]
fn a_copy_falls_due_once_the_editing_stops() {
    let t0 = Instant::now();
    let mut c = Cadence::default();
    c.note_edit(t0);
    assert!(c.pending());
    assert!(
        !c.due(t0 + QUIET - Duration::from_millis(1)),
        "still inside the debounce — a drag is one edit, not forty"
    );
    assert!(c.due(t0 + QUIET));
}

#[test]
fn a_hand_that_never_stops_is_copied_at_the_ceiling() {
    // The case the debounce alone gets wrong: someone drawing steadily never
    // leaves a two-second gap, so without the ceiling their whole session is
    // the thing at risk.
    let t0 = Instant::now();
    let mut c = Cadence::default();
    let mut t = t0;
    for _ in 0..20 {
        c.note_edit(t);
        assert!(!c.due(t), "a second apart is never a quiet period");
        t += Duration::from_secs(1);
    }
    c.note_edit(t);
    assert_eq!(t - t0, CEILING);
    assert!(c.due(t), "twenty seconds of unbroken editing is copied anyway");
}

#[test]
fn a_copy_taken_starts_both_clocks_from_the_next_edit() {
    let t0 = Instant::now();
    let mut c = Cadence::default();
    c.note_edit(t0);
    c.note_write();
    assert!(!c.pending());
    assert!(
        !c.due(t0 + CEILING * 2),
        "nothing has been edited since the copy, so nothing is due"
    );
    c.note_edit(t0 + CEILING * 2);
    assert!(!c.due(t0 + CEILING * 2), "and the new edit gets its own debounce");
}

// ----------------------------------------------------------------- the store

#[test]
fn a_copy_is_an_ordinary_project_file_and_nothing_more() {
    // The module's one real safety net, asserted rather than claimed: if the
    // offer modal never appears, the recovery is `Open…` on this file.
    let dir = tmp_dir("plain-project");
    let store = Recovery::at(dir.join("recovery"));
    let session = seeded();
    store.write(&session, None).expect("a copy");

    let raw = std::fs::read_to_string(store.snapshot_path()).expect("the snapshot");
    let project = Project::from_json(&raw).expect("an ordinary project file");
    assert_eq!(project.session, session);

    // And the panel's own Open path takes it, with no recovery machinery in it.
    let mut opened = Session::default();
    let mut p = panel(&dir.join("other"));
    assert!(p.open_from(&store.snapshot_path(), &mut opened, &[], &[]));
    assert_eq!(opened.name, "Desk");
}

#[test]
fn the_file_a_copy_was_a_copy_of_comes_back_with_it() {
    let dir = tmp_dir("origin");
    let store = Recovery::at(dir.join("recovery"));
    let origin = dir.join("mytrack.json");
    store.write(&seeded(), Some(&origin)).expect("a copy");

    let found = store.find().expect("something on the shelf").expect("readable");
    assert_eq!(found.origin.as_deref(), Some(origin.as_path()));
    assert!(found.age.is_some(), "the mtime is where the timestamp comes from");
}

#[test]
fn work_that_was_never_saved_anywhere_has_no_origin() {
    let dir = tmp_dir("no-origin");
    let store = Recovery::at(dir.join("recovery"));
    store.write(&seeded(), None).expect("a copy");
    let found = store.find().expect("something").expect("readable");
    assert_eq!(found.origin, None);
}

#[test]
fn an_empty_shelf_is_not_an_error() {
    let store = Recovery::at(tmp_dir("empty").join("recovery"));
    assert!(store.find().is_none());
    assert!(store.clear().is_ok(), "clearing runs on every ordinary exit");
}

#[test]
fn a_copy_that_cannot_be_read_says_so_rather_than_saying_nothing() {
    // Collapsing this into "nothing on the shelf" would mean the one run where
    // the copy is broken is also the run that never mentions it.
    let dir = tmp_dir("broken");
    let store = Recovery::at(dir.join("recovery"));
    store.write(&seeded(), None).expect("a copy");
    std::fs::write(store.snapshot_path(), "{ this is not a session").unwrap();

    match store.find() {
        Some(Err(why)) => assert!(!why.is_empty(), "and it is worded for a person"),
        other => panic!("expected a readable complaint, got {}", other.is_some()),
    }
}

#[test]
fn a_missing_sidecar_costs_the_remembered_path_and_nothing_else() {
    let dir = tmp_dir("no-meta");
    let store = Recovery::at(dir.join("recovery"));
    store.write(&seeded(), Some(&dir.join("x.json"))).expect("a copy");
    std::fs::remove_file(store.meta_path()).unwrap();

    let found = store.find().expect("something").expect("still readable");
    assert_eq!(found.origin, None, "the path is gone");
    assert!(
        Project::from_json(&found.json).is_ok(),
        "and the session it was protecting is not"
    );
}

#[test]
fn a_write_leaves_no_half_written_files_behind() {
    // Both files go down as a `.tmp` and get renamed into place, because the
    // event this module exists for can land in the middle of the write.
    let dir = tmp_dir("atomic");
    let store = Recovery::at(dir.join("recovery"));
    store.write(&seeded(), None).expect("a copy");
    let mut names: Vec<String> = std::fs::read_dir(store.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["session.json", "session.meta.json"]);
}

#[test]
fn clearing_removes_both_files() {
    let dir = tmp_dir("clear");
    let store = Recovery::at(dir.join("recovery"));
    store.write(&seeded(), None).expect("a copy");
    store.clear().expect("cleared");
    assert!(store.find().is_none());
    assert!(!store.meta_path().exists());
}

// ---------------------------------------------------------------- the wiring

#[test]
fn an_edit_is_copied_out_with_the_path_it_belongs_to() {
    let dir = tmp_dir("wiring-write");
    let store = Recovery::at(dir.join("recovery"));
    let file = dir.join("song.json");
    let mut p = panel_saving_to(&dir.join("recovery"), file.clone());
    let mut session = seeded();

    assert!(p.save(&session), "give it a path first");
    session.tempo_bpm = 141.0;
    p.mark_edited(true);
    assert!(p.autosave_now(&session));

    let found = store.find().expect("a copy").expect("readable");
    assert_eq!(found.origin.as_deref(), Some(file.as_path()));
    assert_eq!(Project::from_json(&found.json).unwrap().session.tempo_bpm, 141.0);
    assert!(p.is_dirty(), "a crash copy is not a save and must not clear the flag");
}

#[test]
fn a_save_takes_the_copy_away() {
    let dir = tmp_dir("save-clears");
    let store = Recovery::at(dir.join("recovery"));
    let mut p = panel_saving_to(&dir.join("recovery"), dir.join("song.json"));
    let session = seeded();

    p.mark_edited(true);
    assert!(p.autosave_now(&session));
    assert!(store.find().is_some());

    assert!(p.save(&session));
    assert!(store.find().is_none(), "the work is where the user put it");
}

#[test]
fn discarding_on_close_takes_the_copy_with_it() {
    // Otherwise the one deliberate way to throw work away becomes a suggestion,
    // handed straight back through a modal on the next launch.
    let dir = tmp_dir("discard-close");
    let store = Recovery::at(dir.join("recovery"));
    let mut p = panel(&dir.join("recovery"));
    let session = seeded();

    p.mark_edited(true);
    assert!(p.autosave_now(&session));
    assert!(!p.allow_close(), "unsaved work still stops the window");

    p.discard_and_close();
    assert!(store.find().is_none());
    assert!(p.allow_close());
}

#[test]
fn a_clean_close_clears_the_shelf_on_the_way_out() {
    let dir = tmp_dir("clean-close");
    let store = Recovery::at(dir.join("recovery"));
    let mut p = panel_saving_to(&dir.join("recovery"), dir.join("song.json"));
    let session = seeded();

    p.mark_edited(true);
    assert!(p.autosave_now(&session));
    assert!(p.save(&session));
    // Something edited and saved again, so there is a second copy to leave behind.
    p.mark_edited(true);
    assert!(p.autosave_now(&session));
    assert!(p.save(&session));

    assert!(p.allow_close(), "nothing unsaved, so it goes");
    assert!(store.find().is_none());
}

#[test]
fn new_clears_the_shelf() {
    let dir = tmp_dir("new-clears");
    let store = Recovery::at(dir.join("recovery"));
    let mut p = panel(&dir.join("recovery"));
    let mut session = seeded();

    p.mark_edited(true);
    assert!(p.autosave_now(&session));
    p.confirm_new(&mut session);
    assert!(store.find().is_none());
}

#[test]
fn a_panel_with_nowhere_to_put_a_copy_writes_nothing_and_says_so() {
    // `with_chooser` deliberately hands out no store, so no test can write into
    // the real user's directory by forgetting to ask for a temp one.
    let mut p = SessionPanel::with_chooser(Box::new(Answers(Rc::new(RefCell::new(Vec::new())))));
    p.mark_edited(true);
    assert!(!p.autosave_now(&seeded()), "and it must never claim it wrote one");
    assert!(p.is_dirty(), "so the close guard is still the thing in the way");
    assert!(p.look_for_recovery().is_none());
    assert!(p.recovery_offer().is_none());
}

// ----------------------------------------------------------------- the offer

#[test]
fn a_copy_found_at_launch_is_offered_exactly_once() {
    let dir = tmp_dir("offer-once");
    Recovery::at(dir.join("recovery")).write(&seeded(), None).expect("a copy");

    let mut p = panel(&dir.join("recovery"));
    assert!(p.look_for_recovery().is_none(), "an offer needs no console line");
    assert!(p.recovery_offer().is_some());

    p.discard_recovery();
    assert!(p.recovery_offer().is_none());
    assert!(
        p.look_for_recovery().is_none(),
        "the shell calls this every frame; a second look would re-raise the modal"
    );
    assert!(p.recovery_offer().is_none());
}

#[test]
fn a_broken_copy_found_at_launch_is_a_console_line_and_no_offer() {
    let dir = tmp_dir("offer-broken");
    let store = Recovery::at(dir.join("recovery"));
    store.write(&seeded(), None).expect("a copy");
    std::fs::write(store.snapshot_path(), "not a session").unwrap();

    let mut p = panel(&dir.join("recovery"));
    let line = p.look_for_recovery().expect("something to say");
    assert!(line.contains("Recovery copy"), "worded for a person: {line}");
    assert!(p.recovery_offer().is_none(), "nothing offered that cannot be delivered");
}

#[test]
fn recovering_puts_the_work_back_as_unsaved_work_on_its_own_path() {
    let dir = tmp_dir("recover");
    let store = Recovery::at(dir.join("recovery"));
    let origin = dir.join("song.json");
    let mut crashed = seeded();
    crashed.tempo_bpm = 141.0;
    store.write(&crashed, Some(&origin)).expect("a copy");

    let mut p = panel(&dir.join("recovery"));
    let mut session = Session::default();
    p.look_for_recovery();
    assert!(p.recover(&mut session, &[], &[]));

    assert_eq!(session.tempo_bpm, 141.0);
    assert_eq!(session.name, "Desk");
    assert!(p.is_dirty(), "recovered work has still never reached a file");
    assert_eq!(p.path(), Some(origin.as_path()), "so Cmd+S goes where it would have");
    assert_eq!(p.status(), Some(&Status::Recovered(Some(origin))));
    assert!(
        store.find().is_some(),
        "and the copy stays until a real save — it is still the only version"
    );
    assert!(p.recovery_offer().is_none(), "the question has been answered");
}

#[test]
fn recovering_work_that_had_no_file_leaves_save_asking_for_one() {
    let dir = tmp_dir("recover-unsaved");
    Recovery::at(dir.join("recovery")).write(&seeded(), None).expect("a copy");

    let mut p = panel(&dir.join("recovery"));
    let mut session = Session::default();
    p.look_for_recovery();
    assert!(p.recover(&mut session, &[], &[]));
    assert_eq!(p.path(), None, "there was never a path, so Save must ask for one");
    assert_eq!(p.status(), Some(&Status::Recovered(None)));
}

#[test]
fn saving_recovered_work_is_what_finally_clears_the_shelf() {
    let dir = tmp_dir("recover-then-save");
    let store = Recovery::at(dir.join("recovery"));
    store.write(&seeded(), None).expect("a copy");

    let mut p = panel_saving_to(&dir.join("recovery"), dir.join("kept.json"));
    let mut session = Session::default();
    p.look_for_recovery();
    assert!(p.recover(&mut session, &[], &[]));
    assert!(store.find().is_some());

    assert!(p.save(&session));
    assert!(!p.is_dirty());
    assert!(store.find().is_none());
    assert_eq!(
        Project::from_json(&std::fs::read_to_string(dir.join("kept.json")).unwrap())
            .unwrap()
            .session
            .name,
        "Desk"
    );
}

#[test]
fn declining_the_offer_throws_the_copy_away() {
    let dir = tmp_dir("decline");
    let store = Recovery::at(dir.join("recovery"));
    store.write(&seeded(), None).expect("a copy");

    let mut p = panel(&dir.join("recovery"));
    let session = Session::default();
    p.look_for_recovery();
    p.discard_recovery();

    assert!(store.find().is_none());
    assert!(!p.is_dirty(), "declining leaves a session nobody has touched");
    assert_eq!(session, Session::default(), "and the one on screen untouched");
}

#[test]
fn a_reconnect_is_unsaved_work_but_not_work_to_recover() {
    // The failure this guards against is not a crash, it is a habit: a box
    // plugged in dirties the session on every single launch, so without the
    // split the shelf is never empty and the offer appears every run — until
    // you dismiss it without reading, on the run that mattered.
    let dir = tmp_dir("reconnect");
    let store = Recovery::at(dir.join("recovery"));
    let mut p = panel(&dir.join("recovery"));
    let session = seeded();

    p.mark_reconnected();
    assert!(p.is_dirty(), "the desk really did change, so the close guard still asks");
    assert!(!p.autosave_pending(), "but nothing is waiting to be copied");
    assert!(!p.autosave(&session), "and no copy is taken, however long the app runs");
    assert!(store.find().is_none());

    // One real edit and the clock is running again.
    p.mark_edited(true);
    assert!(p.autosave_pending());
}
