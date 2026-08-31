//! The Send button, driven end to end against a box that is a `BTreeMap`.
//!
//! `core/tests/export.rs` proves an unedited import written back is the pattern
//! the box already had. This proves the *button* around that: the job the press
//! builds, the refusals that happen before anything is fetched, the confirm round
//! trip across the thread boundary, and the line the row ends up showing. Between
//! them, the only thing left untested by the time a cable is involved is the
//! cable.
//!
//! **Nothing here can reach hardware.** [`ui::write::run`] takes the box as a
//! [`PatternIo`], which is exactly why that trait is two methods wide; the stash
//! is a temp directory, and no port is opened. What is *not* covered is
//! `ui::write`'s `worker` — the six lines that open real ports and identify — and
//! `egui`'s side of the dialog.
//!
//! The fixtures are `protocol/tests/fixtures`, read by relative path rather than
//! copied, on the same bargain as `core/tests/export.rs`: one repository, one
//! copy of a ~100 KB capture.

use std::collections::BTreeMap;
use std::sync::mpsc::channel;

use digi_core::device::{model_for_key, DeviceIo, PortRef};
use digi_core::import::Fetched;
use digi_core::model::{PLockLane, Source};
use digi_core::{two_box_session, DeviceId, PatternRef, Session};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::{identity_from_responses, DeviceIdentity, DeviceResponse};
use digi_protocol::pattern::{decode_pattern_kit, Spec};
use digi_protocol::protocol::{split_sysex_stream, SysExKind, DUMP_PATTERN_KIT};
use digi_protocol::safe_write::{PatternIo, Timestamp};
use digi_roll_studio::ui::write::{plan, run, Event, Job, PortsPresent, Report};

/// 15 trigs on track 1, every one carrying PROB/FILL/COND.
const DT2_CONDITIONS: &str = "digitakt2-A01-conditions-2026-08-02.syx";

/// 2026-08-01 12:34:56 UTC, the instant the JS suite stamps its backups with.
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

fn identity(product_id: u8, build: &str) -> DeviceIdentity {
    identity_from_responses(
        &DeviceResponse { product_id, supported_ids: vec![0x60], reported_name: String::new() },
        build.into(),
        "1.15B".into(),
    )
}

/// A box that keeps its slots in a map, and counts what was asked of it — so a
/// refusal can be shown to have refused *before* the wire rather than after.
struct FakeBox {
    identity: DeviceIdentity,
    slots: BTreeMap<u8, Vec<u8>>,
    fetches: usize,
    sends: usize,
}

impl FakeBox {
    /// A Digitakt II on the write allowlist.
    fn dt2() -> Self {
        Self {
            identity: identity(42, "0070"),
            slots: BTreeMap::from([(0, payload(DT2_CONDITIONS))]),
            fetches: 0,
            sends: 0,
        }
    }

    fn slot(&self, index: u8) -> &[u8] {
        &self.slots[&index]
    }
}

impl PatternIo for FakeBox {
    fn identity(&self) -> Option<&DeviceIdentity> {
        Some(&self.identity)
    }

    fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
        self.fetches += 1;
        self.slots.get(&index).cloned().ok_or_else(|| format!("no slot {index}"))
    }

    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String> {
        self.sends += 1;
        self.slots.insert(index, payload.to_vec());
        Ok(())
    }
}

fn tmp_stash(tag: &str) -> Stash {
    let dir =
        std::env::temp_dir().join(format!("digi-roll-app-write-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stash::at(dir)
}

/// A session holding the DT2 capture in A01, with the box's ports set so a press
/// can build a job at all.
fn session_with_import() -> (Session, DeviceId, &'static Spec) {
    let mut session = two_box_session();
    let id = session
        .devices
        .iter()
        .find(|d| d.model.key == "DT2")
        .expect("two_box_session has both boxes")
        .id;
    let spec = model_for_key("DT2").and_then(|m| m.spec()).expect("a box with a spec");
    let bytes = payload(DT2_CONDITIONS);
    let kit = decode_pattern_kit(spec, &bytes).expect("the fixture decodes");
    session
        .import_pattern(
            id,
            PatternRef::new(0, 0),
            &Fetched { spec, kit: &kit, payload: &bytes, from: PatternRef::new(0, 0) },
        )
        .expect("a capture into a slot of its own model");
    let device = session.device_mut(id).expect("just found it");
    device.io = DeviceIo {
        input: Some(PortRef { id: "in".into(), name: "Digitakt II".into() }),
        output: Some(PortRef { id: "out".into(), name: "Digitakt II".into() }),
        ..DeviceIo::default()
    };
    (session, id, spec)
}

/// What the UI thread did while the write ran.
struct Seen {
    asks: Vec<Vec<String>>,
    statuses: Vec<String>,
    logs: Vec<String>,
}

/// Run the real flow with a UI that answers `consent` to whatever it is asked —
/// or, for `None`, drops the question on the floor the way a closing window does.
///
/// The confirm genuinely crosses a thread here rather than being called
/// directly: the reply channel is the part of decision 1 that could deadlock, and
/// a test that answered the hook in place would not touch it.
fn drive(
    box_: &mut FakeBox,
    job: &Job,
    stash: &Stash,
    consent: Option<bool>,
) -> (Result<Report, String>, Seen) {
    let (tx, rx) = channel::<Event>();
    let ui = std::thread::spawn(move || {
        let mut seen = Seen { asks: Vec::new(), statuses: Vec::new(), logs: Vec::new() };
        while let Ok(event) = rx.recv() {
            match event {
                Event::Ask(ask) => {
                    seen.asks.push(ask.lines.clone());
                    match consent {
                        Some(answer) => {
                            let _ = ask.reply.send(answer);
                        }
                        // Dropped without an answer, which is what a window
                        // closing mid-dialog does to the worker's `recv`.
                        None => drop(ask),
                    }
                }
                Event::Status(s) => seen.statuses.push(s),
                Event::Log(s) => seen.logs.push(s),
                Event::Done(_) => unreachable!("`run` returns the report; only `worker` sends it"),
            }
        }
        seen
    });
    let out = run(box_, stash, job, &tx, NOW);
    drop(tx);
    (out, ui.join().expect("the UI thread panicked"))
}

fn job_for(session: &Session, id: DeviceId, track: usize) -> Job {
    plan(session, PortsPresent::unknown(), id, PatternRef::new(0, 0), track, PatternRef::new(0, 0), false)
        .expect("a wired box, a slot this session has, and a track it has")
}

// --- the write ------------------------------------------------------------------

#[test]
fn a_consented_write_lands_verified_and_says_so_in_one_line() {
    let (session, id, spec) = session_with_import();
    let mut box_ = FakeBox::dt2();
    let before = box_.slot(0).to_vec();

    let (result, seen) =
        drive(&mut box_, &job_for(&session, id, 0), &tmp_stash("consented"), Some(true));

    let report = result.expect("the fake box is on the allowlist");
    assert!(!report.cancelled);
    assert!(!report.message.is_error, "{}", report.message.text);
    assert!(
        report.message.text.starts_with("Wrote 15 notes to A01 T1 — verified byte-identical"),
        "{}",
        report.message.text
    );
    assert_eq!(box_.sends, 1);
    // The re-fetch and the verify read: rule 2's "never write back bytes captured
    // earlier" and rule 4's read-back, both of them the flow's, not this file's.
    assert_eq!(box_.fetches, 2);

    // The pattern came home. `core/tests/export.rs` is where this is dissected
    // field by field; here it is the end-to-end sanity that the button wrote the
    // pattern rather than something that merely verified.
    let after = box_.slot(0);
    let kit_before = decode_pattern_kit(spec, &before).unwrap();
    let kit_after = decode_pattern_kit(spec, after).unwrap();
    assert_eq!(kit_after.kit.name, kit_before.kit.name);
    // Sixteen records went in and fifteen came back, which is the scrub working
    // rather than a note lost: this capture holds the leftovers of a trig deleted
    // on the box at step 16, the import dropped them with the trig, and a write
    // that left them would let the next trig drawn there inherit a dead one's
    // probability. `core/tests/export.rs` pins the byte-level version of this.
    assert_eq!(kit_before.tracks[0].trigs.len(), 16);
    assert_eq!(kit_after.tracks[0].trigs.len(), 15);

    // And the person was told what was happening while it happened.
    assert!(seen.statuses.iter().any(|s| s.contains("backup")), "{:?}", seen.statuses);
    assert!(seen.statuses.iter().any(|s| s.contains("Verifying")), "{:?}", seen.statuses);
    assert!(seen.logs.iter().any(|l| l.contains("Backed up")), "{:?}", seen.logs);
}

#[test]
fn the_dialog_is_asked_once_after_the_re_fetch_and_names_what_the_box_holds() {
    let (session, id, _) = session_with_import();
    let mut box_ = FakeBox::dt2();

    let (result, seen) =
        drive(&mut box_, &job_for(&session, id, 0), &tmp_stash("asked"), Some(true));
    result.expect("consented");

    assert_eq!(seen.asks.len(), 1, "one write, one question");
    let lines = &seen.asks[0];
    // Every one of these is knowable only from the destination's own bytes,
    // which is the whole reason the dialog is answered mid-flow rather than
    // before it: the trig count and the kit name are the box's, this second.
    assert!(
        lines[0].contains("to A01 “KIT 1” track 1"),
        "the destination's kit name is what a person recognises the slot by: {}",
        lines[0]
    );
    assert!(
        lines.iter().any(|l| l == "This replaces the 15 trigs already on that track."),
        "{lines:#?}"
    );
    assert_eq!(
        lines.last().map(String::as_str),
        Some(digi_protocol::safe_write::BACKUP_LINE),
        "no dialog may bury the backup line"
    );
}

#[test]
fn cancelling_the_dialog_leaves_the_box_and_the_backup_store_untouched() {
    // The important half of consent. A cancel has to stop *before* the stash, or
    // the restore list fills up with patterns nobody overwrote.
    let (session, id, _) = session_with_import();
    let mut box_ = FakeBox::dt2();
    let before = box_.slot(0).to_vec();
    let stash = tmp_stash("cancelled");

    let (result, seen) = drive(&mut box_, &job_for(&session, id, 0), &stash, Some(false));

    let report = result.expect("a cancel is an answer, not an error");
    assert!(report.cancelled);
    assert_eq!(report.message.text, "Write cancelled");
    assert!(!report.message.is_error);
    assert_eq!(seen.asks.len(), 1);
    assert_eq!(box_.sends, 0, "nothing may be sent after a cancel");
    assert_eq!(box_.slot(0), &before[..]);
    assert!(stash.backups(None).is_empty(), "a cancelled write is not a backup");
}

#[test]
fn a_dialog_that_is_never_answered_consents_to_nothing() {
    // The window closing, or the panel being dropped, while a worker waits on the
    // reply. There is no answer to `recv` on a dead channel except "no", and this
    // is the one line in the flow where the wrong default would send bytes nobody
    // agreed to.
    let (session, id, _) = session_with_import();
    let mut box_ = FakeBox::dt2();
    let before = box_.slot(0).to_vec();
    let stash = tmp_stash("dropped");

    let (result, seen) = drive(&mut box_, &job_for(&session, id, 0), &stash, None);

    let report = result.expect("a dropped dialog is a cancel, not a crash");
    assert!(report.cancelled);
    assert_eq!(seen.asks.len(), 1);
    assert_eq!(box_.sends, 0);
    assert_eq!(box_.slot(0), &before[..]);
    assert!(stash.backups(None).is_empty());
}

#[test]
fn what_the_box_holds_and_what_is_going_to_it_are_read_from_different_ends() {
    // The LEN sentence needs both: the destination track's length off the dump
    // the flow just re-fetched, and the source track's off the session. Getting
    // either from the wrong end would still produce a plausible line.
    let (mut session, id, _) = session_with_import();
    session
        .device_mut(id)
        .unwrap()
        .pattern_mut(0)
        .unwrap()
        .track_mut(0)
        .unwrap()
        .length_steps = 64;

    let mut box_ = FakeBox::dt2();
    let (_, seen) = drive(&mut box_, &job_for(&session, id, 0), &tmp_stash("len"), Some(false));

    let len = seen.asks[0]
        .iter()
        .find(|l| l.contains("steps long"))
        .expect("the LEN line");
    assert!(len.contains("16 steps long on the box and this one is 64"), "{len}");
}

#[test]
fn a_box_that_is_not_the_one_the_row_names_is_refused_before_a_byte_is_read() {
    // Decision 3, and the reason it is checked in `run` rather than in the
    // hooks: a mis-cabled desk must not get as far as fetching, let alone as far
    // as a dialog offering to overwrite the wrong box.
    let (session, id, _) = session_with_import();
    let mut box_ = FakeBox::dt2();
    box_.identity = identity(43, "0049"); // a Digitone II answering the DT2's row

    let (result, seen) =
        drive(&mut box_, &job_for(&session, id, 0), &tmp_stash("wrong"), Some(true));

    let refusal = result.expect_err("a DN2 on the DT2's row");
    assert!(refusal.contains("refusing to write"), "{refusal}");
    assert!(seen.asks.is_empty(), "nothing may be offered for a box we are refusing");
    assert_eq!((box_.fetches, box_.sends), (0, 0));
}

#[test]
fn a_firmware_the_format_was_never_verified_against_is_refused_the_same_way() {
    // Rule 3, and this one is `safe_write_track`'s refusal rather than the
    // button's: the gate is the flow's first act, before the re-fetch, which is
    // why `ui::write::run` does not check it a second time. The assertions below
    // are that it reaches the row unaltered and that nothing was read or sent.
    let (session, id, _) = session_with_import();
    let mut box_ = FakeBox::dt2();
    box_.identity = identity(42, "9999");

    let (result, seen) =
        drive(&mut box_, &job_for(&session, id, 0), &tmp_stash("build"), Some(true));

    let refusal = result.expect_err("an unverified build");
    assert!(refusal.contains("9999"), "{refusal}");
    assert!(refusal.contains("read-only"), "{refusal}");
    assert!(seen.asks.is_empty());
    assert_eq!((box_.fetches, box_.sends), (0, 0));
}

#[test]
fn a_lane_that_could_not_be_written_reaches_the_dialog_and_the_result_line() {
    // The loss that is easiest to ship silently: the notes all land, the write
    // verifies byte-identical, and one lane of automation quietly did not go.
    // `js/main.js` folds the export's warnings into the result for this reason,
    // and `write_result_message` shouts about a warning for the same one.
    let (mut session, id, _) = session_with_import();
    let slot = PatternRef::new(0, 0);
    session
        .device_mut(id)
        .unwrap()
        .pattern_mut(slot.slot())
        .unwrap()
        .track_mut(0)
        .unwrap()
        .plocks = vec![PLockLane {
        name: None,
        param_id: Some(44),
        // A lane numbered against the *other* box: unwritable here, and aiming it
        // at whatever paramId 44 happens to be on this one is the guess `export`
        // refuses to make.
        device_kind: Some("DN2".into()),
        trigless: false,
        values: vec![Some(64); 16],
    }];

    let job = job_for(&session, id, 0);
    assert_eq!(job.warnings.len(), 1, "{:?}", job.warnings);
    let mut box_ = FakeBox::dt2();
    let (result, seen) = drive(&mut box_, &job, &tmp_stash("lane"), Some(true));

    let report = result.expect("the write itself is fine");
    assert!(seen.asks[0].iter().any(|l| l.starts_with("Note: ") && l.contains("DN2")),
        "the loss is agreed to before the write, not discovered after: {:#?}", seen.asks[0]);
    assert!(
        report.message.text.contains("— but ") && report.message.text.contains("DN2"),
        "{}",
        report.message.text
    );
    assert!(
        report.message.is_error,
        "a write that did not go entirely as asked must not read as one that did"
    );
}

// --- what the press captured ------------------------------------------------------

#[test]
fn the_press_captures_the_track_it_was_aimed_at_and_everything_travelling_with_it() {
    let (session, id, _) = session_with_import();
    let job = job_for(&session, id, 0);

    assert_eq!(job.write.index, 0);
    assert_eq!(job.write.track_index, 0);
    assert_eq!(job.write.notes.len(), 15);
    // The three that reach past the notes, each of which the dialog has to name:
    // the track's PROB default, its lanes (`Some`, so lanes the box holds and
    // this track does not are freed) and the pattern's swing.
    assert_eq!(job.write.track_prob, Some(100));
    assert_eq!(job.write.plocks.as_deref(), Some(&[][..]));
    assert_eq!(job.write.swing, Some(50.0));
    assert_eq!(job.source_label, "A01 T1");
    assert_eq!(job.from_other_box, None);
}

#[test]
fn a_pattern_that_came_off_another_box_is_carried_as_a_note_rather_than_a_refusal() {
    // Aiming a copy somewhere on purpose is allowed; the dialog says whose
    // pattern is going where. It is the box on the *cable* being wrong that gets
    // refused, and that cannot be known until the handshake.
    let (mut session, id, _) = session_with_import();
    session.device_mut(id).unwrap().pattern_mut(0).unwrap().source =
        Some(Source { device_slug: "digitone2".into(), bank: 0, index: 3 });

    let job = job_for(&session, id, 0);
    assert_eq!(job.from_other_box.as_deref(), Some("digitone2"));
}

#[test]
fn a_track_the_pattern_does_not_have_is_refused_where_the_press_happened() {
    let (session, id, _) = session_with_import();
    let refusal = plan(&session, PortsPresent::unknown(), id, PatternRef::new(0, 0), 99, PatternRef::new(0, 0), false)
        .expect_err("a DT2 pattern has 16 tracks");
    assert!(refusal.contains("track 100 does not exist"), "{refusal}");
}

#[test]
fn a_box_with_no_ports_is_refused_before_a_thread_exists() {
    let (mut session, id, _) = session_with_import();
    session.device_mut(id).unwrap().io.output = None;

    let refusal = plan(&session, PortsPresent::unknown(), id, PatternRef::new(0, 0), 0, PatternRef::new(0, 0), false)
        .expect_err("a write goes out on the output");
    assert!(refusal.contains("out port"), "{refusal}");
}
