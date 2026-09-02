//! The Backups group, driven end to end against a box that is a `BTreeMap`.
//!
//! `protocol/tests/all/safe_write.rs` proves `safe_restore_pattern_kit` — the fetch
//! order, the snapshot, the gate at send time, the verify. This proves the *button*
//! around it: the refusals that happen before anything is read, the confirm round
//! trip across the thread boundary, and the line the block ends up showing.
//!
//! **The test worth reading first is the round trip.** A write goes on through the
//! same `safe_write_track` the Send button uses, the backup it left is taken out of
//! the store the way the panel lists it, and restoring it puts the slot back
//! byte-for-byte. That is the claim the whole group exists to make, and it is not
//! one either side could make alone.
//!
//! **Nothing here can reach hardware.** [`run`] takes the box as a [`PatternIo`],
//! the stash is a temp directory, and no port is opened. What is *not* covered is
//! `ui::restore`'s `worker` — the lines that open real ports and identify — and
//! `egui`'s side of the dialog.

use std::collections::BTreeMap;
use std::sync::mpsc::channel;

use digi_core::device::{model_for_key, DeviceIo, PortRef};
use digi_core::import::Fetched;
use digi_core::{two_box_session, DeviceId, PatternRef, Session};
use digi_protocol::backup_stash::{Stash, StashEntry};
use digi_protocol::device::{identity_from_responses, DeviceIdentity, DeviceResponse};
use digi_protocol::pattern::{decode_pattern_kit, Spec};
use digi_protocol::protocol::{split_sysex_stream, SysExKind, DUMP_PATTERN_KIT};
use digi_protocol::safe_write::{
    safe_write_track, PatternIo, Timestamp, WriteHooks, SNAPSHOT_LINE,
};
use digi_roll_studio::ui::restore::{pick, plan, run, Event, Job, Report};
use digi_roll_studio::ui::write::PortsPresent;

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
        std::env::temp_dir().join(format!("digi-roll-app-restore-{tag}-{}", std::process::id()));
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

/// A caller that has already asked. The write being seeded is scaffolding, not the
/// thing under test — `app/tests/all/write.rs` is where the consent around it lives.
struct Consented;
impl WriteHooks for Consented {}

/// Overwrite track 1 of A01 through the real write path, and hand back the backup
/// it left in the store — which is exactly the row the panel lists.
///
/// **The seeding is a real write on purpose.** A hand-built `StashEntry` over a
/// hand-stashed file would test the restore against a fixture of this test's own
/// making; going through `safe_write_track` means the bytes being restored are
/// bytes the app actually replaced, and the round trip below is a claim about the
/// two buttons rather than about one.
fn overwrite_a01(
    box_: &mut FakeBox,
    stash: &Stash,
    session: &Session,
    id: DeviceId,
    spec: &'static Spec,
) -> StashEntry {
    let export = session
        .track_write(spec, id, PatternRef::new(0, 0), 0, PatternRef::new(0, 0))
        .expect("a track this session has");
    safe_write_track(box_, stash, &export.write, &mut Consented, NOW).expect("a consented write");
    box_.fetches = 0;
    box_.sends = 0;
    stash
        .backups(Some("digitakt2"))
        .into_iter()
        .next()
        .expect("a write that was not cancelled leaves a backup")
}

/// What the UI thread did while the restore ran.
struct Seen {
    asks: Vec<Vec<String>>,
    buttons: Vec<String>,
    statuses: Vec<String>,
    logs: Vec<String>,
}

/// Run the real flow with a UI that answers `consent` to whatever it is asked —
/// or, for `None`, drops the question on the floor the way a closing window does.
///
/// The confirm genuinely crosses a thread here rather than being called directly:
/// the reply channel is the part that could deadlock, and a test that answered the
/// hook in place would not touch it.
fn drive(box_: &mut FakeBox, job: &Job, consent: Option<bool>) -> (Result<Report, String>, Seen) {
    let (tx, rx) = channel::<Event>();
    let ui = std::thread::spawn(move || {
        let mut seen =
            Seen { asks: Vec::new(), buttons: Vec::new(), statuses: Vec::new(), logs: Vec::new() };
        while let Ok(event) = rx.recv() {
            match event {
                Event::Ask(ask) => {
                    seen.asks.push(ask.lines.clone());
                    seen.buttons.push(ask.button.clone());
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
    let out = run(box_, job, &tx, NOW);
    drop(tx);
    (out, ui.join().expect("the UI thread panicked"))
}

// --- the round trip ---------------------------------------------------------------

#[test]
fn a_write_then_a_restore_puts_the_whole_slot_back_byte_for_byte() {
    // The claim the group exists to make, and neither half can make it alone.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("roundtrip");
    let mut box_ = FakeBox::dt2();
    let original = box_.slot(0).to_vec();

    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    assert_ne!(box_.slot(0), &original[..], "the write really did change the slot first");

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).expect("a wired box and its own capture");
    let (result, seen) = drive(&mut box_, &job, Some(true));

    let report = result.expect("the fake box is on the allowlist");
    assert!(!report.cancelled);
    assert!(!report.message.is_error, "{}", report.message.text);
    assert_eq!(report.message.text, "Restored A01 — verified byte-identical");
    assert_eq!(box_.slot(0), &original[..], "every byte of the slot is back");

    // One send, and two fetches: what the slot holds now (for the snapshot) and
    // the read-back that verifies. The flow's, not this file's.
    assert_eq!(box_.sends, 1);
    assert_eq!(box_.fetches, 2);

    // And the person was told what was happening while it happened.
    assert!(seen.statuses.iter().any(|s| s.contains("backing up")), "{:?}", seen.statuses);
    assert!(seen.statuses.iter().any(|s| s.contains("Verifying")), "{:?}", seen.statuses);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn what_the_slot_held_before_the_restore_is_kept_where_it_can_be_found_again() {
    // `SNAPSHOT_LINE` promises the restore can be undone. This is that promise
    // being kept: the botched state is stored, it is *not* in the restore list,
    // and it did not push the real backup out of it.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("snapshot");
    let mut box_ = FakeBox::dt2();

    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    let written = box_.slot(0).to_vec();

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    let (result, seen) = drive(&mut box_, &job, Some(true));
    result.expect("consented");

    let snapshots = stash.snapshots(Some("digitakt2"));
    assert_eq!(snapshots.len(), 1);
    assert!(snapshots[0].is_snapshot());
    assert_eq!(
        stash.payload(&snapshots[0].file).as_deref(),
        Some(&written[..]),
        "the snapshot is what the slot held at the moment of the restore"
    );
    // The list the panel shows is unchanged: still the one pattern this app
    // overwrote, with the snapshot counted in its own ring.
    let listed = stash.backups(Some("digitakt2"));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].file, entry.file);

    // And the block says where it went, in `protocol`'s own wording for a row.
    let log = seen.logs.last().expect("the snapshot is logged");
    assert!(log.contains("before a restore"), "{log}");
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- consent -----------------------------------------------------------------------

#[test]
fn the_dialog_names_the_capture_and_admits_it_cannot_describe_what_it_replaces() {
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("dialog");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    let (result, seen) = drive(&mut box_, &job, Some(false));
    result.expect("a cancel is an answer");

    assert_eq!(seen.asks.len(), 1, "one restore, one question");
    assert_eq!(seen.buttons, ["Restore A01"], "a button that says OK is one you press by reflex");
    let lines = &seen.asks[0];

    assert!(lines[0].contains("Restore A01 on the Digitakt II"), "{}", lines[0]);
    // The capture, in the wording `protocol` gives a row — so the dialog and the
    // list cannot come to describe the same backup differently.
    assert!(
        lines.contains(&entry.summary()),
        "the dialog has to name which capture is going: {lines:#?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("all sixteen tracks, the kit and its sounds")),
        "a restore is bigger than a write and the dialog must say so: {lines:#?}"
    );
    // Decision 4: the silence about the destination is stated rather than left to
    // be read as reassurance.
    assert!(
        lines.iter().any(|l| l.contains("isn't decoded first")),
        "{lines:#?}"
    );
    assert_eq!(
        lines.last().map(String::as_str),
        Some(SNAPSHOT_LINE),
        "no dialog may bury where the previous state went"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn cancelling_leaves_the_box_and_the_store_untouched() {
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("cancel");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    let written = box_.slot(0).to_vec();

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    let (result, _) = drive(&mut box_, &job, Some(false));

    let report = result.expect("a cancel is an answer, not an error");
    assert!(report.cancelled);
    assert_eq!(report.message.text, "Restore cancelled");
    assert!(!report.message.is_error);
    assert_eq!(box_.sends, 0, "nothing may be sent after a cancel");
    assert_eq!(box_.slot(0), &written[..]);
    assert!(
        stash.snapshots(None).is_empty(),
        "a cancelled restore has nothing to snapshot"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_dialog_that_is_never_answered_consents_to_nothing() {
    // The window closing while a worker waits on the reply. There is no answer to
    // `recv` on a dead channel except "no", and this is the one line in the flow
    // where the wrong default would replace a whole slot nobody agreed to.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("dropped");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    let written = box_.slot(0).to_vec();

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    let (result, seen) = drive(&mut box_, &job, None);

    let report = result.expect("a dropped dialog is a cancel, not a crash");
    assert!(report.cancelled);
    assert_eq!(seen.asks.len(), 1);
    assert_eq!(box_.sends, 0);
    assert_eq!(box_.slot(0), &written[..]);
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- the refusals -------------------------------------------------------------------

#[test]
fn a_box_that_is_not_the_one_the_block_names_is_refused_before_a_byte_is_read() {
    // Decision 1's other half. The list cannot offer another family's capture, so
    // this is the mis-cabled desk: a DN2 answering the DT2's block must not get as
    // far as a dialog offering to replace all sixteen of its tracks.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("wrong-box");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    box_.identity = identity(43, "0049"); // a Digitone II

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    let (result, seen) = drive(&mut box_, &job, Some(true));

    let refusal = result.expect_err("a DN2 on the DT2's block");
    assert!(refusal.contains("refusing to write"), "{refusal}");
    assert!(seen.asks.is_empty(), "nothing may be offered for a box we are refusing");
    assert_eq!((box_.fetches, box_.sends), (0, 0));
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_firmware_the_format_was_never_verified_against_is_refused() {
    // Rule 3, re-checked by `safe_restore_pattern_kit` at send time rather than
    // when a button was enabled — the OS can be updated mid-session.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("build");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    box_.identity = identity(42, "9999");

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    let (result, seen) = drive(&mut box_, &job, Some(true));

    let refusal = result.expect_err("an unverified build");
    assert!(refusal.contains("9999"), "{refusal}");
    assert!(refusal.contains("read-only"), "{refusal}");
    assert!(seen.asks.is_empty());
    assert_eq!((box_.fetches, box_.sends), (0, 0));
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_backup_whose_file_has_gone_is_refused_and_nothing_is_sent() {
    // The store is a directory a user can tidy up, and the ring evicts. A row that
    // cannot be read has to be a legible refusal before the box is touched — not a
    // fetch, a snapshot and then a discovery.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("gone");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    std::fs::remove_file(stash.dir().join(&entry.file)).expect("the backup was there");

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    let (result, seen) = drive(&mut box_, &job, Some(true));

    let refusal = result.expect_err("the bytes are gone");
    assert!(refusal.contains(&entry.file), "{refusal}");
    assert!(refusal.contains("Nothing was sent"), "{refusal}");
    assert!(seen.asks.is_empty());
    assert_eq!((box_.fetches, box_.sends), (0, 0));
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- what the press captured ----------------------------------------------------------

#[test]
fn a_capture_from_another_family_is_refused_where_the_press_happened() {
    // Decision 1. Not reachable through the panel, because the list is filtered by
    // slug — but `entry` is an argument, and a Digitone capture aimed at a Digitakt
    // is a whole-slot write of the wrong family's bytes.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("family");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    let foreign = StashEntry { slug: "digitone2".into(), ..entry };

    let refusal = plan(&session, PortsPresent::unknown(), id, &stash, &foreign, false)
        .expect_err("a DN2 capture on the DT2's block");
    assert!(refusal.contains("came off a digitone2"), "{refusal}");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn the_press_aims_at_the_slot_the_capture_came_from() {
    // Decision 2: there is no destination picker, and the capture's own index is
    // the destination.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("aim");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    assert_eq!(job.entry.index, 0);
    assert_eq!(job.entry.bank, "A01");
    assert_eq!(job.slug, Some("digitakt2"));
    // The two ends, the right way round. Nothing else in this file can tell them
    // apart — a fake box has no ports — and crossed bindings would fail on a real
    // desk as "the box did not answer", which is the least helpful way to be told.
    assert_eq!(job.input.id, "in");
    assert_eq!(job.output.id, "out");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_capture_of_a_slot_that_is_not_the_first_goes_back_to_its_own_slot() {
    // **Every other test in this file restores A01, and a hardcoded `0` would pass
    // all of them.** That is the escape class this repo keeps finding — a witness
    // that no fixture happens to contain — so here is one: a backup of the sixth
    // slot, which must come back to the sixth slot and be named as such.
    let (session, id, spec) = session_with_import();
    let stash = tmp_stash("slot-six");
    let mut box_ = FakeBox::dt2();
    // The box has to hold something at index 5 for the write to back it up. A
    // second copy of the capture is fine: what matters is that the index the
    // restore aims at is not the one every other test uses.
    box_.slots.insert(5, payload(DT2_CONDITIONS));
    let original = box_.slot(5).to_vec();

    let export = session
        .track_write(spec, id, PatternRef::new(0, 0), 0, PatternRef::new(0, 5))
        .expect("a track this session has, aimed at the sixth slot");
    safe_write_track(&mut box_, &stash, &export.write, &mut Consented, NOW).expect("consented");
    box_.fetches = 0;
    box_.sends = 0;
    assert_ne!(box_.slot(5), &original[..], "the write changed the sixth slot");
    assert_eq!(box_.slot(0), &payload(DT2_CONDITIONS)[..], "and left the first alone");

    let entry = stash.backups(Some("digitakt2")).into_iter().next().expect("a backup");
    assert_eq!(entry.index, 5);

    let job = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false).unwrap();
    let (result, seen) = drive(&mut box_, &job, Some(true));

    let report = result.expect("consented");
    assert_eq!(report.message.text, "Restored A06 — verified byte-identical");
    assert_eq!(box_.slot(5), &original[..], "the sixth slot is what it was");
    assert_eq!(seen.buttons, ["Restore A06"]);
    assert!(seen.asks[0][0].contains("Restore A06"), "{}", seen.asks[0][0]);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_box_with_no_ports_is_refused_before_a_thread_exists() {
    let (mut session, id, spec) = session_with_import();
    let stash = tmp_stash("ports");
    let mut box_ = FakeBox::dt2();
    let entry = overwrite_a01(&mut box_, &stash, &session, id, spec);
    session.device_mut(id).unwrap().io.input = None;

    let refusal = plan(&session, PortsPresent::unknown(), id, &stash, &entry, false)
        .expect_err("the read-back that verifies a restore comes in on the input");
    assert!(refusal.contains("in port"), "{refusal}");
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- which row the button is aimed at ---------------------------------------------------

fn row(file: &str) -> StashEntry {
    StashEntry {
        file: file.into(),
        slug: "digitakt2".into(),
        device_name: "Digitakt II".into(),
        bank: "A01".into(),
        index: 0,
        kind: "backup".into(),
        kit_name: file.into(),
        track_index: Some(0),
        at: "2026-08-01T12:34:56Z".into(),
    }
}

#[test]
fn the_newest_backup_is_the_one_offered_until_a_row_is_picked() {
    // Decision 3: "put it back" almost always means the last thing this app
    // overwrote, and the list is newest first.
    let (newest, older) = (row("three.syx"), row("two.syx"));
    let rows = [&newest, &older];
    assert_eq!(pick(&rows, None).map(|e| e.file.as_str()), Some("three.syx"));
    assert_eq!(pick(&rows, Some("two.syx")).map(|e| e.file.as_str()), Some("two.syx"));
}

#[test]
fn a_picked_row_the_ring_has_evicted_falls_back_to_the_newest() {
    // **The bug this shape exists to prevent.** Held as an index, a selection
    // whose row had been evicted would still point at position 1 — which is now a
    // *different capture*, still drawn as picked, with a button offering to write
    // it over a whole slot. Held as a file, a selection that is gone is gone, and
    // the fallback is the same one a fresh block gets.
    let (newest, older) = (row("three.syx"), row("two.syx"));
    let rows = [&newest, &older];
    assert_eq!(
        pick(&rows, Some("one.syx")).map(|e| e.file.as_str()),
        Some("three.syx"),
        "an evicted selection must not resolve to whatever took its place"
    );
}

#[test]
fn a_box_with_nothing_in_the_store_offers_nothing() {
    assert!(pick(&[], None).is_none());
    assert!(pick(&[], Some("gone.syx")).is_none());
}
