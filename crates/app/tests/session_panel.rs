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
use digi_core::{default_session, Session};
use digi_roll_studio::ui::session::{
    export_backup, Chooser, CloseGuard, SessionPanel, Status,
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
    let mut s = default_session();
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

    let mut opened = default_session();
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

    let mut reopened = default_session();
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
    let mut s = default_session();
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

    let mut s = default_session();
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

    let mut s = default_session();
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

    let mut s = default_session();
    assert!(p.open(&mut s, &[port.clone()], &[port]));
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
