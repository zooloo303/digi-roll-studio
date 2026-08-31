//! The Session panel's decisions, driven without a window.
//!
//! Phase 8's exit criteria live here. What is deliberately *not* here is the
//! native file dialog: [`ScriptedChooser`] stands in for it, which is the whole
//! reason `ui::session::Chooser` is a trait. The interesting half of "save the
//! session" is which bytes are written, when the dirty flag moves and what a
//! refusal says — none of which needs a human to click OK.
//!
//! The one claim no test in this file can make is that the *native* dialog
//! opens. That is a run-the-app-and-look check, like the glyph table in
//! `ui::mod`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use digi_core::device::PortRef;
use digi_core::model::{Note, PLockLane, TrackScale, PLOCK_STEPS};
use digi_core::project::Project;
use digi_core::session::PatternRef;
use digi_core::{two_box_session, Session};
use digi_roll_studio::ui::session::{
    export_backup, Chooser, CloseGuard, NewGuard, SessionPanel, Status,
};

// ------------------------------------------------------------- the harness

/// What the scripted chooser was asked, and what it answered.
#[derive(Default)]
struct Script {
    save_answers: Vec<Option<PathBuf>>,
    open_answers: Vec<Option<PathBuf>>,
    export_answers: Vec<Option<PathBuf>>,
    /// Every call, in order: "save", "open" or "export".
    calls: Vec<&'static str>,
    /// The filename offered to the last Save As.
    suggested: Option<String>,
}

/// A [`Chooser`] that answers from a queue instead of opening a window.
///
/// Answers are popped from the front, and an exhausted queue answers `None` —
/// which is "the user cancelled", the case most worth not forgetting.
struct ScriptedChooser(Rc<RefCell<Script>>);

impl Chooser for ScriptedChooser {
    fn save_as(&mut self, suggested: &str) -> Option<PathBuf> {
        let mut s = self.0.borrow_mut();
        s.calls.push("save");
        s.suggested = Some(suggested.to_string());
        if s.save_answers.is_empty() { None } else { s.save_answers.remove(0) }
    }

    fn open(&mut self) -> Option<PathBuf> {
        let mut s = self.0.borrow_mut();
        s.calls.push("open");
        if s.open_answers.is_empty() { None } else { s.open_answers.remove(0) }
    }

    fn export_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        let mut s = self.0.borrow_mut();
        s.calls.push("export");
        if s.export_answers.is_empty() { None } else { s.export_answers.remove(0) }
    }

    // The MIDI-file half of the trait, which the Edit panel drives and this panel
    // never touches. Answering `None` — "cancelled" — is the honest stub: if the
    // session panel ever reached for one of these, the test would show a cancelled
    // dialog rather than a path it was never given.
    fn export_midi_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        self.0.borrow_mut().calls.push("export-midi");
        None
    }

    fn open_midi(&mut self) -> Option<PathBuf> {
        self.0.borrow_mut().calls.push("open-midi");
        None
    }
}

fn panel() -> (SessionPanel, Rc<RefCell<Script>>) {
    let script = Rc::new(RefCell::new(Script::default()));
    let panel = SessionPanel::with_chooser(Box::new(ScriptedChooser(Rc::clone(&script))));
    (panel, script)
}

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("digi-roll-session-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    dir
}

/// A session with the things most likely to be lost in translation actually set
/// — the same argument as `core/tests/session.rs`'s seeded fixture, and for the
/// same reason: a round-trip assertion only witnesses what the fixture sets.
fn seeded() -> Session {
    let mut s = two_box_session();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);

    s.name = "Desk".into();
    s.tempo_bpm = 137.0;

    let d = s.device_mut(dt2).unwrap();
    d.io.takes_clock = false;
    let t = d.pattern_mut(0).unwrap().track_mut(3).unwrap();
    t.length_steps = 12;
    t.scale = TrackScale::ThreeHalves;
    t.mute = true;
    let mut n = Note::new(4.0, 60, 1.0, 100, 0.25);
    n.cond = Some("1:2".into());
    n.prob = Some(60);
    t.notes.push(n);
    let mut values = vec![None; PLOCK_STEPS];
    values[4] = Some(96);
    t.plocks.push(
        PLockLane::new(Some("filter.cutoff".into()), Some(44), Some("DT2".into()), false, values)
            .unwrap(),
    );

    s.device_mut(dn2).unwrap().pattern_mut(2).unwrap().swing = 65;

    let verse = s.add_scene("Verse", None);
    s.set_slot_in_scene(verse, dt2, PatternRef::new(0, 4));
    s.current_scene = verse;
    s
}

/// Assert `session` is a just-launched session.
///
/// Since discovery-first (2026-08-24) that means **empty**: no boxes, one
/// scene, house tempo. New and first launch are the same state, and both leave
/// filling the desk to auto-connect and to Setup's "Add a box" — so a fresh
/// session asserting two specific boxes would be asserting the pre-2026-08-24
/// launch state this release removed.
fn assert_is_fresh_empty_session(session: &Session) {
    assert_eq!(session.name, "Session");
    assert_eq!(session.tempo_bpm, 120.0);
    assert_eq!(session.current_scene, 0);
    assert_eq!(session.scenes.len(), 1);
    assert!(
        session.devices.is_empty(),
        "a new session has no boxes — discovery and Add a box fill the desk"
    );
}

// ------------------------------------------------------------- save and open

#[test]
fn a_saved_session_opens_back_as_the_same_session() {
    // Phase 8's headline claim, through the panel rather than through `core`:
    // the bytes the button writes are the bytes the button reads.
    let dir = tmp_dir("roundtrip");
    let path = dir.join("desk.json");
    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(path.clone()));

    let original = seeded();
    assert!(p.save(&original), "the save should reach the disk");

    let mut opened = two_box_session();
    let (mut q, script2) = panel();
    script2.borrow_mut().open_answers.push(Some(path.clone()));
    assert!(q.open(&mut opened, &[], &[]));

    assert_eq!(opened, original);
}

#[test]
fn a_session_saved_and_opened_and_saved_again_is_byte_identical() {
    // The exit criterion in full: through the file, back into a panel, and out
    // again. A field that survives equality but is re-serialised differently
    // shows up here and nowhere else.
    let dir = tmp_dir("bytes");
    let first_path = dir.join("a.json");
    let second_path = dir.join("b.json");

    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(first_path.clone()));
    assert!(p.save(&seeded()));

    let mut reopened = two_box_session();
    let (mut q, script2) = panel();
    script2.borrow_mut().open_answers.push(Some(first_path.clone()));
    script2.borrow_mut().save_answers.push(Some(second_path.clone()));
    assert!(q.open(&mut reopened, &[], &[]));
    assert!(q.save_as(&reopened));

    assert_eq!(
        std::fs::read_to_string(&first_path).unwrap(),
        std::fs::read_to_string(&second_path).unwrap()
    );
}

#[test]
fn saving_adopts_the_file_so_the_next_save_does_not_ask_again() {
    // A Save that reopened the dialog every time would be a Save As wearing the
    // wrong label.
    let dir = tmp_dir("adopt");
    let path = dir.join("desk.json");
    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(path.clone()));

    let s = seeded();
    assert!(p.save(&s));
    assert_eq!(p.path(), Some(path.as_path()));

    // The queue is empty now, so a second dialog would answer None and fail.
    p.mark_edited(true);
    assert!(p.save(&s), "the second save should reuse the adopted path");
    assert_eq!(script.borrow().calls, vec!["save"], "only the first save may ask");
}

#[test]
fn the_suggested_filename_comes_off_the_session_name() {
    let (mut p, script) = panel();
    let mut s = two_box_session();
    s.name = "Tuesday Desk".into();
    p.save_as(&s);
    assert_eq!(script.borrow().suggested.as_deref(), Some("Tuesday-Desk.json"));
}

// ------------------------------------------------------------- the dirty flag

#[test]
fn an_edit_makes_the_session_dirty_and_a_save_makes_it_clean() {
    let dir = tmp_dir("dirty");
    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(dir.join("d.json")));

    assert!(!p.is_dirty(), "a session nobody has touched is not unsaved work");
    p.mark_edited(false);
    assert!(!p.is_dirty(), "a frame that changed nothing must not dirty it");
    p.mark_edited(true);
    assert!(p.is_dirty());

    assert!(p.save(&seeded()));
    assert!(!p.is_dirty(), "a successful save is what clears it");
}

#[test]
fn an_edit_stays_unsaved_through_every_quiet_frame_after_it() {
    // **Found by a deliberate bug that failed nothing.** `mark_edited` is `|=`
    // and not `=`, and every other test in this file happened to call it with
    // `false` *before* `true` — so a version that simply assigned the flag passed
    // the whole suite. What that bug does in the app is the worst kind: the app
    // redraws at least once more after any edit, that frame is quiet, and the
    // close guard then waves the window shut over work nobody saved.
    let (mut p, _script) = panel();
    p.mark_edited(true);
    for _ in 0..10 {
        p.mark_edited(false);
    }
    assert!(p.is_dirty(), "a quiet frame must not un-edit the session");
    assert!(!p.allow_close(), "and the close guard has to still be watching");
}

#[test]
fn a_cancelled_save_leaves_the_work_unsaved() {
    // The case that matters: if a cancelled dialog cleared the flag, the close
    // guard would wave the window shut over work that was never written.
    let (mut p, _script) = panel(); // no answers queued: every dialog cancels
    p.mark_edited(true);

    assert!(!p.save(&seeded()));
    assert!(p.is_dirty(), "cancelling must not clear the dirty flag");
    assert_eq!(p.path(), None);
    assert!(p.status().is_none(), "a cancel is not an event worth reporting");
}

#[test]
fn a_save_that_cannot_be_written_says_so_and_stays_dirty() {
    let dir = tmp_dir("unwritable");
    // A directory is not a file, so the write fails without needing permissions.
    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(dir.clone()));
    p.mark_edited(true);

    assert!(!p.save(&seeded()));
    assert!(p.is_dirty());
    assert!(matches!(p.status(), Some(Status::Failed(_))));
}

// ------------------------------------------------------------- opening

#[test]
fn opening_a_session_does_not_mark_it_as_unsaved_work() {
    let dir = tmp_dir("openclean");
    let path = dir.join("s.json");
    std::fs::write(&path, Project::new(seeded()).to_json_pretty().unwrap()).unwrap();

    let (mut p, script) = panel();
    script.borrow_mut().open_answers.push(Some(path));
    p.mark_edited(true);

    let mut s = two_box_session();
    assert!(p.open(&mut s, &[], &[]));
    assert!(!p.is_dirty(), "a session straight off disk is not unsaved work");
}

#[test]
fn a_file_from_a_newer_build_is_refused_and_the_session_in_hand_survives() {
    // Two claims in one, and the second is the one that would hurt: a load that
    // failed halfway would leave the app holding neither session.
    let dir = tmp_dir("future");
    let path = dir.join("future.json");
    let json = Project::new(seeded()).to_json().unwrap();
    std::fs::write(&path, json.replacen("\"format\":1", "\"format\":99", 1)).unwrap();

    let (mut p, script) = panel();
    script.borrow_mut().open_answers.push(Some(path));

    let before = seeded();
    let mut s = before.clone();
    assert!(!p.open(&mut s, &[], &[]));
    assert_eq!(s, before, "a refused load must not touch the session in hand");

    let Some(Status::Failed(why)) = p.status() else { panic!("expected a failure status") };
    // Actionable, not a stack trace: it has to name both formats and say what to
    // do about it.
    assert!(why.contains("99"), "{why}");
    assert!(why.contains('1'), "{why}");
    assert!(
        why.contains("Update") || why.contains("save the file again"),
        "the message must say what to do next, got {why:?}"
    );
}

#[test]
fn a_file_that_is_not_a_session_is_refused_and_the_session_in_hand_survives() {
    let dir = tmp_dir("garbage");
    let path = dir.join("notes.txt");
    std::fs::write(&path, "this is not a session").unwrap();

    let (mut p, script) = panel();
    script.borrow_mut().open_answers.push(Some(path));

    let before = seeded();
    let mut s = before.clone();
    assert!(!p.open(&mut s, &[], &[]));
    assert_eq!(s, before);
    assert!(matches!(p.status(), Some(Status::Failed(_))));
}

#[test]
fn a_file_that_is_not_there_is_refused_rather_than_emptying_the_session() {
    let (mut p, script) = panel();
    script.borrow_mut().open_answers.push(Some(PathBuf::from("/nowhere/at/all.json")));

    let before = seeded();
    let mut s = before.clone();
    assert!(!p.open(&mut s, &[], &[]));
    assert_eq!(s, before);
}

#[test]
fn a_box_whose_port_is_gone_is_named_and_keeps_its_patterns() {
    // PLAN.md Phase 8: "a box that silently lost its port is a box that silently
    // stopped playing". `from_json_with_ports` hands back ids; the panel has to
    // turn them into something a person can look for on the desk.
    let dir = tmp_dir("lostport");
    let path = dir.join("s.json");

    let mut saved = seeded();
    let dt2 = saved.devices[0].id;
    saved.device_mut(dt2).unwrap().io.input =
        Some(PortRef { id: "gone-in".into(), name: "Digitakt II".into() });
    saved.device_mut(dt2).unwrap().io.output =
        Some(PortRef { id: "gone-out".into(), name: "Digitakt II".into() });
    let notes_before = saved.device(dt2).unwrap().pattern(0).unwrap().track(3).unwrap().notes.len();
    std::fs::write(&path, Project::new(saved).to_json_pretty().unwrap()).unwrap();

    let (mut p, script) = panel();
    script.borrow_mut().open_answers.push(Some(path));

    let mut s = two_box_session();
    // Nothing plugged in that matches, by id or by name.
    let elsewhere = [PortRef { id: "other".into(), name: "Some Other Box".into() }];
    assert!(p.open(&mut s, &elsewhere, &elsewhere));

    assert_eq!(p.lost_ports(), &["DT2".to_string()], "the box has to be named");
    let dt2 = s.devices[0].id;
    assert_eq!(
        s.device(dt2).unwrap().pattern(0).unwrap().track(3).unwrap().notes.len(),
        notes_before,
        "a missing box costs you its I/O and nothing else"
    );
}

#[test]
fn a_box_whose_port_is_back_is_not_reported_as_lost() {
    let dir = tmp_dir("foundport");
    let path = dir.join("s.json");

    let mut saved = seeded();
    let dt2 = saved.devices[0].id;
    let port = PortRef { id: "dt2-1".into(), name: "Digitakt II".into() };
    saved.device_mut(dt2).unwrap().io.input = Some(port.clone());
    saved.device_mut(dt2).unwrap().io.output = Some(port.clone());
    std::fs::write(&path, Project::new(saved).to_json_pretty().unwrap()).unwrap();

    let (mut p, script) = panel();
    script.borrow_mut().open_answers.push(Some(path));

    let mut s = two_box_session();
    // One list, handed to both ends: the port is an input *and* an output here,
    // which is what the clone used to say less directly.
    let ports = [port];
    assert!(p.open(&mut s, &ports, &ports));
    assert!(p.lost_ports().is_empty(), "the port is there, so nothing was lost");
}

// ------------------------------------------------------------- the close guard

#[test]
fn a_clean_session_closes_without_a_question() {
    let (mut p, _s) = panel();
    assert!(p.allow_close());
    assert_eq!(p.guard(), CloseGuard::Idle, "nothing to ask about");
}

#[test]
fn unsaved_work_stops_the_window_closing_and_raises_the_question() {
    let (mut p, _s) = panel();
    p.mark_edited(true);
    assert!(!p.allow_close(), "the close must be refused");
    assert_eq!(p.guard(), CloseGuard::Asking);
}

#[test]
fn discarding_lets_the_next_close_through_rather_than_asking_forever() {
    // Without the Confirmed arm the guard re-asks on the very close it just
    // agreed to, and the window can never shut.
    let (mut p, _s) = panel();
    p.mark_edited(true);
    assert!(!p.allow_close());

    p.discard_and_close();
    assert!(p.allow_close(), "the agreed close must go through");
    assert!(p.is_dirty(), "discarding does not save — it gives the work up");
}

#[test]
fn cancelling_the_guard_leaves_the_app_open_and_still_dirty() {
    let (mut p, _s) = panel();
    p.mark_edited(true);
    assert!(!p.allow_close());

    p.cancel_close();
    assert_eq!(p.guard(), CloseGuard::Idle);
    assert!(!p.allow_close(), "still unsaved, so a second close asks again");
}

#[test]
fn saving_from_the_guard_makes_the_session_closable() {
    let dir = tmp_dir("guardsave");
    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(dir.join("g.json")));
    p.mark_edited(true);
    assert!(!p.allow_close());

    assert!(p.save(&seeded()));
    assert!(!p.is_dirty());
    // The guard is still Asking — the modal is up — but the answer has changed.
    p.discard_and_close();
    assert!(p.allow_close());
}

// ------------------------------------------------------------- the New guard

#[test]
fn new_on_a_clean_session_replaces_it_immediately_and_reports_it() {
    // Nothing to lose, so no modal — `request_new`'s return value is exactly
    // what `ui()` folds into `Outcome::reloaded`, the same way `open`'s
    // return value already is (see that method's caller in `ui::session`).
    let (mut p, _s) = panel();
    let mut session = seeded();

    assert!(p.request_new(&mut session), "a clean session needs no guard");
    assert_is_fresh_empty_session(&session);
    assert_eq!(p.guard_new(), NewGuard::Idle);
    assert_eq!(p.status(), Some(&Status::New));
}

#[test]
fn new_on_a_dirty_session_waits_for_the_guard_to_be_answered() {
    let (mut p, _s) = panel();
    p.mark_edited(true);
    let mut session = seeded();
    let before = session.clone();

    assert!(!p.request_new(&mut session), "dirty work must be asked about first");
    assert_eq!(p.guard_new(), NewGuard::Asking);
    assert_eq!(session, before, "nothing changes until the guard is answered");
    assert!(p.is_dirty(), "and the dirty flag itself must not move either");
}

#[test]
fn keeping_working_on_new_leaves_the_session_the_path_and_the_dirty_flag_untouched() {
    let dir = tmp_dir("newkeep");
    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(dir.join("d.json")));
    let mut session = seeded();
    assert!(p.save(&session), "give the panel a path to lose track of if this goes wrong");
    p.mark_edited(true);
    let before = session.clone();
    let path_before = p.path().map(|p| p.to_path_buf());

    assert!(!p.request_new(&mut session));
    p.cancel_new();

    assert_eq!(session, before, "Keep working must not touch the session");
    assert_eq!(p.path().map(|p| p.to_path_buf()), path_before, "nor the adopted path");
    assert!(p.is_dirty(), "nor the dirty flag — nothing was saved or discarded");
    assert_eq!(p.guard_new(), NewGuard::Idle);
}

#[test]
fn discarding_on_new_replaces_the_session_and_resets_the_panel() {
    let (mut p, _s) = panel();
    p.mark_edited(true);
    let mut session = seeded();

    assert!(!p.request_new(&mut session));
    p.confirm_new(&mut session);

    assert_is_fresh_empty_session(&session);
    assert_eq!(p.path(), None);
    assert!(!p.is_dirty());
    assert!(p.lost_ports().is_empty());
    assert_eq!(p.guard_new(), NewGuard::Idle);
}

#[test]
fn saving_then_new_writes_first_and_only_then_replaces_the_session() {
    let dir = tmp_dir("newsave");
    let path = dir.join("desk.json");
    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(path.clone()));
    let mut session = seeded();
    p.mark_edited(true);

    assert!(!p.request_new(&mut session));
    assert_eq!(p.guard_new(), NewGuard::Asking);

    // The Save exit: save, and only replace the session if the save actually
    // reached the disk (mirrors the close guard's "Save and close").
    assert!(p.save(&session), "the write must happen before anything is thrown away");
    assert!(std::fs::read_to_string(&path).is_ok(), "bytes must be on disk already");
    p.confirm_new(&mut session);

    assert_is_fresh_empty_session(&session);
    assert!(!p.is_dirty());
    assert_eq!(p.guard_new(), NewGuard::Idle);
}

#[test]
fn after_new_a_save_asks_for_a_filename_instead_of_silently_reusing_the_old_one() {
    // The one that matters most: if `path` survived a New, the very next Save
    // would overwrite the *previous* session's file with the empty one,
    // without ever asking.
    let dir = tmp_dir("newpathreset");
    let old_path = dir.join("old.json");
    let new_path = dir.join("new.json");
    let (mut p, script) = panel();
    script.borrow_mut().save_answers.push(Some(old_path.clone()));
    let mut session = seeded();
    assert!(p.save(&session));
    assert_eq!(p.path(), Some(old_path.as_path()));
    // Captured right after the first save, so the later check is a byte
    // comparison against what was actually written — not against a second,
    // independently-constructed `seeded()` call, whose device ids would never
    // match the first call's even if nothing were wrong.
    let old_bytes = std::fs::read_to_string(&old_path).unwrap();

    assert!(p.request_new(&mut session), "clean right after a save, so New applies at once");
    assert_eq!(p.path(), None, "New must forget the old file");

    script.borrow_mut().save_answers.push(Some(new_path.clone()));
    assert!(p.save(&session));
    assert_eq!(
        script.borrow().calls,
        vec!["save", "save"],
        "the second save must ask the chooser again rather than reuse old.json silently"
    );
    assert_eq!(p.path(), Some(new_path.as_path()));
    assert_eq!(
        std::fs::read_to_string(&old_path).unwrap(),
        old_bytes,
        "the previous session's file on disk must be untouched by the New that followed it"
    );
}

#[test]
fn new_clears_a_stale_lost_ports_warning_from_whatever_was_open_before_it() {
    let dir = tmp_dir("newlostports");
    let path = dir.join("s.json");
    let mut saved = seeded();
    let dt2 = saved.devices[0].id;
    saved.device_mut(dt2).unwrap().io.input =
        Some(PortRef { id: "gone-in".into(), name: "Digitakt II".into() });
    saved.device_mut(dt2).unwrap().io.output =
        Some(PortRef { id: "gone-out".into(), name: "Digitakt II".into() });
    std::fs::write(&path, Project::new(saved).to_json_pretty().unwrap()).unwrap();

    let (mut p, script) = panel();
    script.borrow_mut().open_answers.push(Some(path));
    let mut session = two_box_session();
    assert!(p.open(&mut session, &[], &[]));
    assert!(!p.lost_ports().is_empty(), "setup: the open must have flagged something");

    assert!(p.request_new(&mut session));
    assert!(
        p.lost_ports().is_empty(),
        "a fresh session with no boxes wired up must not still show a warning \
         left over from what used to be open"
    );
}

// The two guards must not be able to stack a second modal over the one
// already asking about the identical unsaved work — see `NewGuard`'s header
// comment in `ui::session` for the reasoning. Both directions are tested
// because each is a separate `if` in a different method.

#[test]
fn a_close_request_does_not_open_its_own_modal_while_the_new_guard_is_up() {
    let (mut p, _s) = panel();
    p.mark_edited(true);
    let mut session = seeded();
    assert!(!p.request_new(&mut session));
    assert_eq!(p.guard_new(), NewGuard::Asking);

    assert!(!p.allow_close(), "still unsaved, so the close itself must be refused");
    assert_eq!(
        p.guard(),
        CloseGuard::Idle,
        "but the close guard must not raise its own modal over the New one"
    );
    assert_eq!(p.guard_new(), NewGuard::Asking, "the New question must still be the one showing");
}

#[test]
fn a_new_request_is_refused_rather_than_stacking_while_the_close_guard_is_up() {
    let (mut p, _s) = panel();
    p.mark_edited(true);
    assert!(!p.allow_close());
    assert_eq!(p.guard(), CloseGuard::Asking);

    let mut session = seeded();
    let before = session.clone();
    assert!(!p.request_new(&mut session), "the close guard already has the floor");
    assert_eq!(p.guard_new(), NewGuard::Idle, "New must not raise its own modal on top");
    assert_eq!(session, before, "and nothing about the session may change either");
}

// ------------------------------------------------------------- backup export

#[test]
fn exporting_a_backup_copies_the_stored_dump_to_the_chosen_path() {
    // `Stash::export` has had no caller since it was written. This is it.
    use digi_protocol::backup_stash::Stash;

    let dir = tmp_dir("export");
    let stash_dir = dir.join("stash");
    std::fs::create_dir_all(&stash_dir).unwrap();
    let bytes: &[u8] = &[0xF0, 0x00, 0x20, 0x3C, 0x0D, 0x00, 0xF7];
    std::fs::write(stash_dir.join("backup.syx"), bytes).unwrap();
    let stash = Stash::at(stash_dir);

    let dest = dir.join("somewhere-else.syx");
    let script = Rc::new(RefCell::new(Script::default()));
    script.borrow_mut().export_answers.push(Some(dest.clone()));
    let mut chooser = ScriptedChooser(Rc::clone(&script));

    assert_eq!(export_backup(&mut chooser, &stash, "backup.syx"), Some(Ok(dest.clone())));
    assert_eq!(std::fs::read(&dest).unwrap(), bytes, "a plain copy of the dump");
}

#[test]
fn a_cancelled_export_copies_nothing_and_reports_nothing() {
    use digi_protocol::backup_stash::Stash;

    let dir = tmp_dir("exportcancel");
    let stash = Stash::at(dir.join("stash"));
    let script = Rc::new(RefCell::new(Script::default()));
    let mut chooser = ScriptedChooser(Rc::clone(&script));

    // No answer queued, so the dialog cancels. The outer `None` is what says
    // "nothing happened" rather than "the copy failed".
    assert_eq!(export_backup(&mut chooser, &stash, "backup.syx"), None);
    assert_eq!(script.borrow().calls, vec!["export"], "it did ask");
}

#[test]
fn a_backup_that_cannot_be_copied_reports_a_failure_rather_than_a_cancel() {
    // The two `None`-ish answers must not collapse into one: a cancel is silence
    // and a failed copy is a message, and the row shows only the second.
    use digi_protocol::backup_stash::Stash;

    let dir = tmp_dir("exportmissing");
    let stash = Stash::at(dir.join("stash"));
    let script = Rc::new(RefCell::new(Script::default()));
    script.borrow_mut().export_answers.push(Some(dir.join("out.syx")));
    let mut chooser = ScriptedChooser(Rc::clone(&script));

    let answer = export_backup(&mut chooser, &stash, "not-there.syx");
    let Some(Err(why)) = answer else { panic!("expected a reported failure, got {answer:?}") };
    assert!(why.contains("not-there.syx"), "the message must name the file: {why}");
}
