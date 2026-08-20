//! Phase 10's exit criterion: a mass send driven end to end against fake boxes.
//!
//! `app/tests/write.rs` proves the single-track button. This proves the one above
//! it — the press that puts every track of the session onto every box — and the
//! three things that only exist at that scale:
//!
//! * **one dialog for the whole desk**, crossing a channel exactly as
//!   `ui::write`'s does, with a **per-row opt-out** that really does leave the
//!   unticked tracks alone;
//! * **one backup per slot rather than per track**, which is the difference
//!   between two ring entries and thirty-two;
//! * **a store that fails mid-run**, where the box it failed on writes nothing
//!   and every box after it refuses without being attempted.
//!
//! **Nothing here can reach hardware.** [`sync::run`] takes its boxes through an
//! `open` closure returning a [`PatternIo`], the stash is a temp directory, and
//! no port is opened. What is *not* covered is `ui::sync`'s `worker` — the lines
//! that open real ports and identify — and `egui`'s side of the modal.
//!
//! The fixtures are `protocol/tests/fixtures`, read by relative path rather than
//! copied, on the same bargain as `app/tests/write.rs`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc::channel;

use digi_core::device::{model_for_key, Device, DeviceIo, PortRef, DN2, DT2};
use digi_core::import::Fetched;
use digi_core::{DeviceId, PatternRef, Session};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::{identity_from_responses, DeviceIdentity, DeviceResponse};
use digi_protocol::pattern::{decode_pattern_kit, Spec};
use digi_protocol::protocol::{split_sysex_stream, SysExKind, DUMP_PATTERN_KIT};
use digi_protocol::safe_write::{PatternIo, Timestamp};
use digi_roll_studio::ui::sync::{self, Event, MassPlan, MassReport};
use digi_roll_studio::ui::write::PortsPresent;

const DT2_CAPTURE: &str = "digitakt2-A01-conditions-2026-08-02.syx";
const DN2_CAPTURE: &str = "digitone2-A01-conditions-2026-08-02.syx";

/// 2026-08-01 12:34:56 UTC, the instant every backup in these suites is stamped
/// with.
const NOW: Timestamp =
    Timestamp { year: 2026, month: 8, day: 1, hour: 12, minute: 34, second: 56 };

fn payload(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../protocol/tests/fixtures")
        .join(name);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("reading fixture {}: {e}", path.display()));
    split_sysex_stream(&bytes)
        .into_iter()
        .filter(|m| m.kind == SysExKind::Dump)
        .filter_map(|m| m.dump)
        .find(|d| d.dump_type == DUMP_PATTERN_KIT)
        .map(|d| d.payload)
        .unwrap_or_else(|| panic!("{name}: no pattern-kit dump"))
}

fn identity(product_id: u8, build: &str, version: &str) -> DeviceIdentity {
    identity_from_responses(
        &DeviceResponse { product_id, supported_ids: vec![0x60], reported_name: String::new() },
        build.into(),
        version.into(),
    )
}

// --- the fake desk ----------------------------------------------------------------

/// What one box holds, shared with the test so it can be read after the run.
///
/// `Rc` rather than `Arc` on purpose: [`sync::run`] is called on this thread and
/// only the dialog crosses a channel, exactly as `app/tests/write.rs` drives its
/// single-track counterpart.
#[derive(Default)]
struct Card {
    slots: BTreeMap<u8, Vec<u8>>,
    fetches: usize,
    sends: usize,
    /// Run just before the nth fetch of this box, whatever n the test picked.
    /// The seam that lets a test break the world *mid-run*, which is the only way
    /// to reach "the store failed while we were writing" without a real disk
    /// going away.
    sabotage: Option<(usize, Box<dyn Fn()>)>,
}

#[derive(Clone)]
struct FakeBox {
    identity: DeviceIdentity,
    card: Rc<RefCell<Card>>,
}

impl FakeBox {
    fn new(identity: DeviceIdentity, capture: &str) -> Self {
        Self {
            identity,
            card: Rc::new(RefCell::new(Card {
                slots: BTreeMap::from([(0, payload(capture))]),
                ..Card::default()
            })),
        }
    }

    fn dt2(capture: &str) -> Self {
        Self::new(identity(42, "0070", "1.15B"), capture)
    }

    fn dn2(capture: &str) -> Self {
        Self::new(identity(43, "0049", "1.10D"), capture)
    }

    fn on_build(mut self, build: &str) -> Self {
        self.identity = identity(42, build, "1.15B");
        self
    }

    fn slot(&self, index: u8) -> Vec<u8> {
        self.card.borrow().slots[&index].clone()
    }

    fn fetches(&self) -> usize {
        self.card.borrow().fetches
    }

    fn sends(&self) -> usize {
        self.card.borrow().sends
    }

    /// Notes on one track of one slot, as the box now holds them.
    fn notes(&self, spec: &Spec, index: u8, track: usize) -> usize {
        let kit = decode_pattern_kit(spec, &self.slot(index)).expect("the fake box holds a pattern");
        digi_protocol::pattern::track_notes(&kit, track).len()
    }
}

impl PatternIo for FakeBox {
    fn identity(&self) -> Option<&DeviceIdentity> {
        Some(&self.identity)
    }

    fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
        let mut card = self.card.borrow_mut();
        card.fetches += 1;
        let n = card.fetches;
        if card.sabotage.as_ref().is_some_and(|(when, _)| *when == n) {
            let (_, act) = card.sabotage.take().expect("just matched");
            // Dropped first: the sabotage reaches back into this same card.
            drop(card);
            act();
            let card = self.card.borrow();
            return card.slots.get(&index).cloned().ok_or_else(|| format!("no slot {index}"));
        }
        card.slots.get(&index).cloned().ok_or_else(|| format!("no slot {index}"))
    }

    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String> {
        let mut card = self.card.borrow_mut();
        card.sends += 1;
        card.slots.insert(index, payload.to_vec());
        Ok(())
    }
}

fn tmp_stash(tag: &str) -> Stash {
    let dir =
        std::env::temp_dir().join(format!("digi-roll-app-sync-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stash::at(dir)
}

// --- the session ------------------------------------------------------------------

fn port(name: &str) -> PortRef {
    PortRef { id: name.into(), name: name.into() }
}

fn spec_of(key: &str) -> &'static Spec {
    model_for_key(key).and_then(|m| m.spec()).expect("a box with a spec")
}

/// A session holding one capture per box, with both boxes wired.
///
/// The DT2 capture has trigs on track 1 only, the DN2's on track 1 only too — so
/// a plan over this session is fifteen skipped tracks per box and one going,
/// which is the *right* shape for the tests below: the skip list is the thing
/// decision 3 could get wrong silently.
fn desk() -> (Session, DeviceId, DeviceId) {
    let mut session = Session::default();
    let dt2 = session.add_device(Device::new("DT2", &DT2, 16));
    let dn2 = session.add_device(Device::new("DN2", &DN2, 16));

    for (id, key, capture, name) in
        [(dt2, "DT2", DT2_CAPTURE, "dt2"), (dn2, "DN2", DN2_CAPTURE, "dn2")]
    {
        let spec = spec_of(key);
        let bytes = payload(capture);
        let kit = decode_pattern_kit(spec, &bytes).expect("the fixture decodes");
        session
            .import_pattern(
                id,
                PatternRef::new(0, 0),
                &Fetched { spec, kit: &kit, payload: &bytes, from: PatternRef::new(0, 0) },
            )
            .expect("a capture into a slot of its own model");
        let device = session.device_mut(id).expect("just added");
        device.io = DeviceIo {
            input: Some(port(&format!("{name}-in"))),
            output: Some(port(&format!("{name}-out"))),
            ..DeviceIo::default()
        };
    }
    (session, dt2, dn2)
}

/// Put notes on a second track of one box, so a partial opt-out has two rows to
/// choose between.
fn add_second_track(session: &mut Session, id: DeviceId) {
    let device = session.device_mut(id).expect("a box in this session");
    let pattern = device.pattern_mut(0).expect("A01");
    let source: Vec<_> = pattern.track(0).expect("T1").notes.clone();
    pattern.track_mut(1).expect("T2").notes = source;
}

// --- driving it -------------------------------------------------------------------

/// What the UI thread saw and did.
struct Seen {
    /// Every dialog, flattened to `(heading, row labels, extra lines, skipped)`.
    asks: Vec<Vec<(String, Vec<String>, Vec<String>, Vec<String>)>>,
    /// Whether the last dialog's backup promise was the last thing in it.
    backup_line: Option<String>,
    blocked: Vec<String>,
    statuses: Vec<String>,
}

/// How the UI answers the one dialog.
enum Answer {
    /// Everything the dialog offered.
    All,
    /// Only these `(box, track)` pairs.
    Only(Vec<(DeviceId, usize)>),
    Cancel,
    /// Dropped without a reply, which is what a closing window does.
    Vanish,
}

/// Run the real flow with a UI on the other end of the channel.
///
/// The dialog genuinely crosses a thread here rather than being answered in
/// place: the reply channel is the part that could deadlock, and a test that
/// called the hook directly would not touch it.
fn drive(
    plan: &MassPlan,
    stash: &Stash,
    boxes: Vec<FakeBox>,
    answer: Answer,
) -> (MassReport, Seen) {
    let (tx, rx) = channel::<Event>();
    let ui = std::thread::spawn(move || {
        let mut seen =
            Seen { asks: Vec::new(), backup_line: None, blocked: Vec::new(), statuses: Vec::new() };
        while let Ok(event) = rx.recv() {
            match event {
                Event::Ask(ask) => {
                    seen.asks.push(
                        ask.boxes
                            .iter()
                            .map(|b| {
                                (
                                    b.heading.clone(),
                                    b.rows.iter().map(|r| r.label.clone()).collect(),
                                    b.lines.clone(),
                                    b.skipped.clone(),
                                )
                            })
                            .collect(),
                    );
                    seen.backup_line = Some(ask.backup_line.to_string());
                    seen.blocked = ask.blocked.clone();
                    match &answer {
                        Answer::All => {
                            let all = ask
                                .boxes
                                .iter()
                                .flat_map(|b| {
                                    b.rows.iter().map(move |r| (b.device, r.track_index))
                                })
                                .collect();
                            let _ = ask.reply.send(Some(all));
                        }
                        Answer::Only(picked) => {
                            let _ = ask.reply.send(Some(picked.clone()));
                        }
                        Answer::Cancel => {
                            let _ = ask.reply.send(None);
                        }
                        Answer::Vanish => drop(ask),
                    }
                }
                Event::Status(s) | Event::Log(s) => seen.statuses.push(s),
                Event::Done(_) => {}
            }
        }
        seen
    });

    let mut queue = boxes.into_iter();
    let report = sync::run(
        plan,
        stash,
        |_job| queue.next().ok_or_else(|| "no box for that job".to_string()),
        &tx,
        NOW,
        false,
    );
    drop(tx);
    (report, ui.join().expect("the UI thread panicked"))
}

// --- the plan ---------------------------------------------------------------------

#[test]
fn the_plan_sends_the_tracks_that_have_notes_and_names_the_ones_it_leaves() {
    // Decision 3, which is the one that can go wrong in silence: a skipped track
    // that is not listed reads as a track that was sent.
    let (session, dt2, _) = desk();
    let plan = sync::plan_all(&session, PortsPresent::unknown());

    assert_eq!(plan.jobs.len(), 2, "both boxes are wired and both have notes");
    assert_eq!(plan.tracks(), 2, "one track each");
    assert!(plan.blocked.is_empty(), "{:?}", plan.blocked);

    let job = plan.jobs.iter().find(|j| j.device == dt2).expect("the DT2 is in the plan");
    assert_eq!(job.aims.iter().map(|a| a.track_index).collect::<Vec<_>>(), vec![0]);
    assert_eq!(job.skipped.len(), 15, "the other fifteen are named, not dropped");
    assert!(
        job.skipped.iter().all(|s| s.why.contains("left as it is on the box")),
        "an empty track is left alone, and the reason is the promise: {:?}",
        job.skipped
    );
    // The provenance rule: this capture came from A01, so it aims back at A01.
    assert_eq!(job.into, PatternRef::new(0, 0));
    assert_eq!(job.from, PatternRef::new(0, 0));
}

#[test]
fn a_slot_aims_where_the_pattern_came_from_rather_than_where_it_is_sitting() {
    // **Found by a deliberate bug.** The test above asserts `into == A01` on a
    // pattern sitting in A01 that came from A01 — so replacing the whole
    // provenance rule with `let into = from` failed nothing. Lesson 4's shape
    // exactly: an assertion whose expected value is also what you would get
    // without the code under test. This is the case that can tell them apart —
    // the pattern is in A03 of the session and came off C06 of the box, which is
    // the everyday fetch-edit-put-it-back round trip.
    let (mut session, dt2, _) = desk();
    {
        let device = session.device_mut(dt2).expect("the DT2");
        let held = device.pattern(0).expect("A01").clone();
        *device.pattern_mut(2).expect("A03") = held;
        device.pattern_mut(2).expect("A03").source = Some(digi_core::model::Source {
            device_slug: "digitakt2".into(),
            bank: 2,
            index: 5,
        });
        device.pattern_mut(0).expect("A01").track_mut(0).expect("T1").notes.clear();
    }
    // Point the scene at A03 so that is the slot the sync reads from.
    session.set_slot_in_scene(0, dt2, PatternRef::new(0, 2));

    let plan = sync::plan_all(&session, PortsPresent::unknown());
    let job = plan.jobs.iter().find(|j| j.device == dt2).expect("the DT2 is in the plan");
    assert_eq!(job.from, PatternRef::new(0, 2), "the scene's slot");
    assert_eq!(job.into, PatternRef::new(2, 5), "and the slot it was fetched from");
}

#[test]
fn a_box_with_no_ports_is_left_out_of_the_plan_by_name_rather_than_ignored() {
    let (mut session, dt2, _) = desk();
    session.device_mut(dt2).expect("the DT2").io.output = None;
    let plan = sync::plan_all(&session, PortsPresent::unknown());

    assert_eq!(plan.jobs.len(), 1, "the DN2 can still go");
    assert_eq!(plan.blocked.len(), 1);
    assert_eq!(plan.blocked[0].device, dt2);
    assert!(plan.blocked[0].why.contains("out port"), "{}", plan.blocked[0].why);
    // And the count in the button is the count that will actually happen.
    assert_eq!(plan.tracks(), 1);
}

#[test]
fn a_box_whose_scene_slot_is_empty_is_left_out_rather_than_sent_nothing() {
    let mut session = Session::default();
    let dt2 = session.add_device(Device::new("DT2", &DT2, 16));
    let device = session.device_mut(dt2).expect("just added");
    device.io = DeviceIo {
        input: Some(port("in")),
        output: Some(port("out")),
        ..DeviceIo::default()
    };
    let plan = sync::plan_all(&session, PortsPresent::unknown());

    assert!(plan.jobs.is_empty());
    assert!(plan.is_empty());
    assert_eq!(plan.blocked.len(), 1);
    assert!(plan.blocked[0].why.contains("no notes"), "{}", plan.blocked[0].why);
}

// --- the run ----------------------------------------------------------------------

#[test]
fn a_consented_sync_writes_every_box_once_and_verifies_each() {
    let (session, dt2, dn2) = desk();
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    let stash = tmp_stash("consented");
    let boxes = vec![FakeBox::dt2(DT2_CAPTURE), FakeBox::dn2(DN2_CAPTURE)];
    let (report, seen) = drive(&plan, &stash, boxes.clone(), Answer::All);

    assert!(!report.cancelled);
    assert_eq!(report.boxes.len(), 2);
    for outcome in &report.boxes {
        assert!(outcome.wrote, "{}: {}", outcome.name, outcome.text);
        assert!(!outcome.is_error, "{}: {}", outcome.name, outcome.text);
        assert!(
            outcome.text.contains("verified byte-identical"),
            "{}: {}",
            outcome.name,
            outcome.text
        );
        assert!(outcome.log.as_ref().is_some_and(|l| l.contains("Backed up")), "{outcome:?}");
    }
    assert_eq!(report.boxes.iter().map(|b| b.device).collect::<Vec<_>>(), vec![dt2, dn2]);

    // Three reads and one send per box: the survey that worded the dialog, the
    // re-fetch rule 2 insists the payload is built from, the send, and the
    // read-back that verifies it.
    for box_ in &boxes {
        assert_eq!(box_.fetches(), 3, "survey, re-fetch, verify");
        assert_eq!(box_.sends(), 1, "one slot, one send");
    }
    // The notes really landed.
    assert_eq!(boxes[0].notes(spec_of("DT2"), 0, 0), 15);
    assert!(boxes[1].notes(spec_of("DN2"), 0, 0) > 0);

    // One dialog, for the whole desk.
    assert_eq!(seen.asks.len(), 1, "one press, one question");
    let ask = &seen.asks[0];
    assert_eq!(ask.len(), 2, "both boxes in it");
    assert!(ask[0].0.starts_with("DT2 · Digitakt II — 1 tracks into A01"), "{}", ask[0].0);
    assert_eq!(ask[0].1.len(), 1, "one row per track going");
    assert!(ask[0].1[0].starts_with("T1"), "{}", ask[0].1[0]);
    assert!(ask[0].3.len() == 15, "the fifteen it is not sending are in the dialog too");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn the_dialog_says_the_slot_wide_changes_once_rather_than_once_per_track() {
    // Swing is one byte for the whole pattern and the lane pool is one budget
    // for all sixteen tracks. Said per track, six tracks would read as six
    // separate changes to the feel of the slot — which is the shape a mass send
    // is uniquely able to get wrong.
    let (mut session, dt2, _) = desk();
    for track in 1..4 {
        let device = session.device_mut(dt2).expect("the DT2");
        let pattern = device.pattern_mut(0).expect("A01");
        let notes = pattern.track(0).expect("T1").notes.clone();
        pattern.track_mut(track).expect("a track").notes = notes;
    }
    // The session's pattern is straight and the box is at 65, so the write moves
    // it — the one impact line that reaches past the tracks being written.
    session.device_mut(dt2).expect("the DT2").pattern_mut(0).expect("A01").swing = 62;

    let plan = sync::plan_all(&session, PortsPresent::unknown());
    let stash = tmp_stash("slot-wide");
    let boxes = vec![FakeBox::dt2(DT2_CAPTURE), FakeBox::dn2(DN2_CAPTURE)];
    let (_, seen) = drive(&plan, &stash, boxes, Answer::Cancel);

    let (_, rows, lines, _) = &seen.asks[0][0];
    assert_eq!(rows.len(), 4, "four tracks going");
    let swing: Vec<&String> = lines.iter().filter(|l| l.contains("Swing goes from")).collect();
    assert_eq!(swing.len(), 1, "said once for the slot, not four times: {lines:#?}");
    assert!(swing[0].contains("all 16 tracks in A01."), "{}", swing[0]);
    assert!(!swing[0].contains("not just"), "the whole slot is going: {}", swing[0]);

    // And the promise every dialog ends on.
    assert_eq!(
        seen.backup_line.as_deref(),
        Some(
            "The whole destination pattern is backed up first, and can be restored from Backups."
        )
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn one_press_leaves_one_backup_per_slot_rather_than_one_per_track() {
    // **The decision the whole plural write exists for.** Six tracks on one box
    // through `safe_write_track` would be six entries of a fifty-entry ring; here
    // it is one, and the row restores all sixteen tracks either way.
    let (mut session, dt2, _) = desk();
    for track in 1..6 {
        let device = session.device_mut(dt2).expect("the DT2");
        let pattern = device.pattern_mut(0).expect("A01");
        let notes = pattern.track(0).expect("T1").notes.clone();
        pattern.track_mut(track).expect("a track").notes = notes;
    }
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    assert_eq!(plan.tracks(), 7, "six on the DT2 and one on the DN2");

    let stash = tmp_stash("one-backup");
    let boxes = vec![FakeBox::dt2(DT2_CAPTURE), FakeBox::dn2(DN2_CAPTURE)];
    let (report, _) = drive(&plan, &stash, boxes.clone(), Answer::All);

    assert!(report.boxes.iter().all(|b| b.wrote), "{:?}", report.boxes);
    assert_eq!(
        stash.backups(None).len(),
        2,
        "seven tracks across two slots is two backups, not seven"
    );
    assert_eq!(boxes[0].sends(), 1, "six tracks went in one dump");
    // And all six are actually on the box.
    for track in 0..6 {
        assert_eq!(boxes[0].notes(spec_of("DT2"), 0, track), 15, "track {track}");
    }
    // The backup for a slot write names no single track, because it puts all
    // sixteen back.
    let dt2_backup = stash
        .backups(Some("digitakt2"))
        .into_iter()
        .next()
        .expect("the DT2's backup");
    assert_eq!(dt2_backup.track_index, None);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn unticking_a_row_leaves_that_track_exactly_as_the_box_had_it() {
    // The per-row opt-out, proved on the bytes rather than on the dialog.
    let (mut session, dt2, dn2) = desk();
    add_second_track(&mut session, dt2);
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    assert_eq!(plan.tracks(), 3, "two on the DT2, one on the DN2");

    let stash = tmp_stash("opt-out");
    let boxes = vec![FakeBox::dt2(DT2_CAPTURE), FakeBox::dn2(DN2_CAPTURE)];
    let before_t2 = boxes[0].notes(spec_of("DT2"), 0, 1);
    // Tick the DT2's T1 only: its T2 is dropped, and the DN2 gets nothing at all.
    let (report, _) = drive(&plan, &stash, boxes.clone(), Answer::Only(vec![(dt2, 0)]));

    let dt2_row = report.boxes.iter().find(|b| b.device == dt2).expect("the DT2 row");
    assert!(dt2_row.wrote, "{}", dt2_row.text);
    assert!(dt2_row.text.contains("A01 T1"), "one track named, not counted: {}", dt2_row.text);

    let dn2_row = report.boxes.iter().find(|b| b.device == dn2).expect("the DN2 row");
    assert!(!dn2_row.wrote);
    assert!(!dn2_row.is_error, "a box nobody ticked is not a failure");
    assert_eq!(dn2_row.text, "Nothing ticked for this box");

    assert_eq!(boxes[0].notes(spec_of("DT2"), 0, 1), before_t2, "T2 was not touched");
    assert_eq!(boxes[1].sends(), 0, "the DN2 was surveyed and then left alone");
    assert_eq!(stash.backups(None).len(), 1, "and only the box that was written got a backup");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn cancelling_the_dialog_writes_nothing_anywhere_and_takes_no_backup() {
    let (session, _, _) = desk();
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    let stash = tmp_stash("cancel");
    let boxes = vec![FakeBox::dt2(DT2_CAPTURE), FakeBox::dn2(DN2_CAPTURE)];
    let (report, _) = drive(&plan, &stash, boxes.clone(), Answer::Cancel);

    assert!(report.cancelled);
    assert!(report.boxes.iter().all(|b| !b.wrote && !b.is_error), "{:?}", report.boxes);
    assert!(report.boxes.iter().all(|b| b.text == "Sync cancelled"), "{:?}", report.boxes);
    for box_ in &boxes {
        assert_eq!(box_.sends(), 0);
        assert_eq!(box_.fetches(), 1, "the survey read, and nothing after it");
    }
    assert!(stash.backups(None).is_empty());
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_dialog_that_is_never_answered_consents_to_nothing() {
    // The window closing mid-question. The worker's `recv` fails, and a failed
    // recv is the only direction that is safe.
    let (session, _, _) = desk();
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    let stash = tmp_stash("vanish");
    let boxes = vec![FakeBox::dt2(DT2_CAPTURE), FakeBox::dn2(DN2_CAPTURE)];
    let (report, _) = drive(&plan, &stash, boxes.clone(), Answer::Vanish);

    assert!(report.cancelled);
    for box_ in &boxes {
        assert_eq!(box_.sends(), 0);
    }
    assert!(stash.backups(None).is_empty());
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_box_that_is_not_the_one_the_row_names_is_refused_and_the_rest_still_go() {
    // A mis-cabled desk. The refusal happens in the *survey*, so the mis-cabled
    // box never reaches the dialog — and the count in the button is the count
    // that can actually happen.
    let (session, dt2, dn2) = desk();
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    let stash = tmp_stash("wrong-box");
    // A DN2 answering on the DT2's ports.
    let boxes = vec![FakeBox::dn2(DN2_CAPTURE), FakeBox::dn2(DN2_CAPTURE)];
    let (report, seen) = drive(&plan, &stash, boxes.clone(), Answer::All);

    let refused = report.boxes.iter().find(|b| b.device == dt2).expect("the DT2 row");
    assert!(refused.is_error);
    assert!(refused.text.contains("refusing to write"), "{}", refused.text);
    assert!(report.boxes.iter().find(|b| b.device == dn2).expect("the DN2 row").wrote);

    assert_eq!(seen.asks[0].len(), 1, "only the box that can be written is in the dialog");
    assert_eq!(boxes[0].sends(), 0, "and nothing went down the mis-cabled pair");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_firmware_the_format_was_never_verified_against_is_refused_before_the_dialog() {
    // The gate is repeated in the survey rather than left to arrive from inside
    // the flow, and this is the difference it makes: a box on an unknown build
    // is *listed as refused* instead of appearing as rows you can tick and
    // consent to.
    let (session, dt2, _) = desk();
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    let stash = tmp_stash("gate");
    let boxes = vec![FakeBox::dt2(DT2_CAPTURE).on_build("9999"), FakeBox::dn2(DN2_CAPTURE)];
    let (report, seen) = drive(&plan, &stash, boxes.clone(), Answer::All);

    let refused = report.boxes.iter().find(|b| b.device == dt2).expect("the DT2 row");
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("9999"), "{}", refused.text);
    assert_eq!(seen.asks[0].len(), 1, "it is not in the dialog at all");
    assert_eq!(boxes[0].fetches(), 0, "and its slot was never even read");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_box_that_changed_while_the_dialog_was_open_refuses_rather_than_writing() {
    // Decision 5's other half. The dialog said "replacing 15 trigs"; between the
    // survey and the write's own re-fetch the box moved, so the sentence agreed
    // to is not the sentence about to happen — and consent does not carry over.
    let (session, dt2, _) = desk();
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    let stash = tmp_stash("moved");
    let dt2_box = FakeBox::dt2(DT2_CAPTURE);
    // On the second fetch — the write's re-fetch — the slot becomes a different
    // pattern, with four trigs on T1 instead of fifteen.
    {
        let card = Rc::clone(&dt2_box.card);
        let other = payload("digitakt2-A01-plock-final-2026-08-04.syx");
        dt2_box.card.borrow_mut().sabotage = Some((
            2,
            Box::new(move || {
                card.borrow_mut().slots.insert(0, other.clone());
            }),
        ));
    }
    let boxes = vec![dt2_box.clone(), FakeBox::dn2(DN2_CAPTURE)];
    let (report, _) = drive(&plan, &stash, boxes, Answer::All);

    let moved = report.boxes.iter().find(|b| b.device == dt2).expect("the DT2 row");
    assert!(!moved.wrote);
    assert!(moved.is_error, "a refusal after consent must not read as a quiet skip");
    assert!(
        moved.text.contains("had 15 when you were asked and has 4 now"),
        "{}",
        moved.text
    );
    assert_eq!(dt2_box.sends(), 0, "nothing was sent");
    assert!(
        stash.backups(Some("digitakt2")).is_empty(),
        "and the backup is not taken until after the confirm"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_store_that_fails_mid_run_stops_every_box_after_it() {
    // **Rule 3 at scale.** The DT2 writes; the store goes away before the DN2's
    // backup; the DN2 writes nothing, and the box after it is not attempted at
    // all — because "a backup that cannot be stored is a write that does not
    // happen" does not stop being true for the next box in the list.
    let (mut session, dt2, dn2) = desk();
    // A third box, so "the remaining writes refuse" has a remainder.
    let third = session.add_device(Device::new("DT2 II", &DT2, 16));
    {
        let spec = spec_of("DT2");
        let bytes = payload(DT2_CAPTURE);
        let kit = decode_pattern_kit(spec, &bytes).expect("the fixture decodes");
        session
            .import_pattern(
                third,
                PatternRef::new(0, 0),
                &Fetched { spec, kit: &kit, payload: &bytes, from: PatternRef::new(0, 0) },
            )
            .expect("a capture into a slot of its own model");
        let device = session.device_mut(third).expect("just added");
        device.io = DeviceIo {
            input: Some(port("third-in")),
            output: Some(port("third-out")),
            ..DeviceIo::default()
        };
    }
    let plan = sync::plan_all(&session, PortsPresent::unknown());
    assert_eq!(plan.jobs.len(), 3);

    let stash = tmp_stash("store-fails");
    let dir = stash.dir().to_path_buf();
    let aside = dir.with_extension("aside");
    let _ = std::fs::remove_dir_all(&aside);

    let dn2_box = FakeBox::dn2(DN2_CAPTURE);
    {
        let dir = dir.clone();
        let aside = aside.clone();
        // On the DN2's *write* re-fetch — after the DT2 has already written and
        // stashed — the store is moved out from under the run and a plain file
        // is left in its place, so `create_dir_all` cannot recreate it. That is
        // the disk going away mid-run, reproduced without a disk going away.
        dn2_box.card.borrow_mut().sabotage = Some((
            2,
            Box::new(move || {
                std::fs::rename(&dir, &aside).expect("moving the store aside");
                std::fs::write(&dir, b"not a directory").expect("blocking the store's path");
            }),
        ));
    }
    let boxes = vec![FakeBox::dt2(DT2_CAPTURE), dn2_box.clone(), FakeBox::dt2(DT2_CAPTURE)];
    let (report, _) = drive(&plan, &stash, boxes.clone(), Answer::All);

    let first = report.boxes.iter().find(|b| b.device == dt2).expect("the DT2 row");
    assert!(first.wrote, "the box before the failure went through: {}", first.text);

    let failed = report.boxes.iter().find(|b| b.device == dn2).expect("the DN2 row");
    assert!(!failed.wrote);
    assert!(failed.is_error);
    assert!(
        failed.text.contains("the backup couldn't be saved"),
        "the refusal names rule 3: {}",
        failed.text
    );
    assert_eq!(dn2_box.sends(), 0, "nothing was sent to the box whose backup failed");

    let untried = report.boxes.iter().find(|b| b.device == third).expect("the third row");
    assert!(!untried.wrote);
    assert!(untried.is_error);
    assert!(untried.text.starts_with("Not attempted — "), "{}", untried.text);
    assert!(
        untried.text.contains("a backup that cannot be stored is a write that does not happen"),
        "{}",
        untried.text
    );
    assert_eq!(boxes[2].fetches(), 1, "the third box was surveyed and never written");
    assert_eq!(boxes[2].sends(), 0);

    // Put the store back, and the box that did go through still has its backup.
    std::fs::remove_file(&dir).expect("the blocking file");
    std::fs::rename(&aside, &dir).expect("putting the store back");
    assert_eq!(stash.backups(None).len(), 1, "the DT2's, and only the DT2's");
    let _ = std::fs::remove_dir_all(&dir);
}
