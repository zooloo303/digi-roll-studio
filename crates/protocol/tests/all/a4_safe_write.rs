//! The Analog Four's safe write flow, end to end against a fake box.
//!
//! `safe_write.rs` is the gen-2 suite and this is its gen-1 twin, split the way
//! the flows themselves are: the ceremony's ordering claims are the same —
//! backup before send, always re-fetch, allowlist at send time, verify by
//! re-read — and everything inside the steps differs, so a shared harness would
//! be a `match` per assertion. The fake box here holds `a4_pattern` payloads
//! and its recorder expects the A4's confirm shape (`pattern_kit: None`,
//! `swing: None`, `free_lanes: None`), which the gen-2 suite's recorder
//! deliberately panics on.
//!
//! Unlike the gen-2 suite there is no JS original to derive expectations from —
//! elk-herd documents only gen-2 — so every number below is measured from the
//! committed `analogfour-*.syx` captures, exactly as `a4.rs`'s are.
//!
//! **What no test here can claim: the box.** The full cycle these tests prove
//! against a `BTreeMap` — encode onto a fresh read, send, re-read, byte-compare
//! — has not run against an A4 as one flow. Its pieces have (the 2026-08-31
//! round trip and the `0x64` read-back of a written slot, PLAN.md §10), which
//! is why `analogfour`/`0195` is in the allowlist at all; the first send
//! through this path on hardware is the claim's other half.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::common::fixture_bytes;
use digi_protocol::a4_pattern::{
    parse_pattern, read_track_trigs, track_base, DUMP_A4_PATTERN, SLOT_MARKER, TRACK_STRIDE,
};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::{identity_from_responses, DeviceIdentity, DeviceResponse};
use digi_protocol::safe_write::{
    a4_safe_write_tracks, safe_restore_pattern_kit, safe_write_tracks, A4TrackWrite,
    ConfirmArgs, PatternIo, PatternKitFile, Timestamp, TrackWrite, WriteError, WriteHooks,
};

/// Slot 0 on the fake box: the A01 capture — 32 trigs on SYN1, 4 on SYN4,
/// nothing else.
const A01: &str = "analogfour-A01-2026-08-30.syx";
/// Slot 15: the cleared-A16 capture, so a second slot exists to prove a write
/// to one slot left the other alone.
const A16: &str = "analogfour-A16-clear-2026-08-31.syx";

const NOW: Timestamp =
    Timestamp { year: 2026, month: 8, day: 31, hour: 12, minute: 0, second: 0 };

fn a4_identity(build: &str) -> DeviceIdentity {
    let dev = DeviceResponse {
        product_id: 4,
        supported_ids: vec![0x01],
        reported_name: String::new(),
    };
    identity_from_responses(&dev, build.into(), "1.55B".into())
}

fn dt2_identity() -> DeviceIdentity {
    let dev = DeviceResponse {
        product_id: 42,
        supported_ids: vec![0x60],
        reported_name: String::new(),
    };
    identity_from_responses(&dev, "0070".into(), "1.15B".into())
}

fn a4_payload(name: &str) -> Vec<u8> {
    parse_pattern(&fixture_bytes(name)).unwrap_or_else(|e| panic!("{name}: {e}")).payload
}

type Log = Rc<RefCell<Vec<String>>>;

/// A fake A4: two slots of real capture bytes, and a timeline.
struct FakeA4 {
    identity: Option<DeviceIdentity>,
    slots: BTreeMap<u8, Vec<u8>>,
    log: Log,
    corrupt_on_store: bool,
}

impl FakeA4 {
    fn new() -> Self {
        Self {
            identity: Some(a4_identity("0195")),
            slots: BTreeMap::from([(0, a4_payload(A01)), (15, a4_payload(A16))]),
            log: Log::default(),
            corrupt_on_store: false,
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

impl PatternIo for FakeA4 {
    fn identity(&self) -> Option<&DeviceIdentity> {
        self.identity.as_ref()
    }

    fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
        self.log.borrow_mut().push(format!("fetch {index}"));
        self.slots.get(&index).cloned().ok_or_else(|| format!("no slot {index}"))
    }

    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String> {
        self.log.borrow_mut().push(format!("send {index}"));
        let mut stored = payload.to_vec();
        if self.corrupt_on_store {
            stored[6_000] ^= 0x7f;
        }
        self.slots.insert(index, stored);
        Ok(())
    }
}

/// Hooks that record the A4's confirm shape. Consents unless told not to.
#[derive(Default)]
struct Recorder {
    /// `(label, index, per-track (track_index, existing_trigs, note_count))`,
    /// plus the three fields that must be `None` on this flow.
    confirms: Vec<(String, u8, Vec<(usize, usize, usize)>)>,
    saw_gen2_facts: bool,
    backups: Vec<PatternKitFile>,
    cancel: bool,
    backup_fails: bool,
    log: Option<Log>,
}

impl WriteHooks for Recorder {
    fn on_backup(&mut self, backup: &PatternKitFile) -> Result<(), String> {
        if let Some(log) = &self.log {
            log.borrow_mut().push("backup".into());
        }
        self.backups.push(backup.clone());
        if self.backup_fails {
            return Err("no second copy today".into());
        }
        Ok(())
    }

    fn confirm(&mut self, args: &ConfirmArgs) -> bool {
        self.saw_gen2_facts |=
            args.pattern_kit.is_some() || args.swing.is_some() || args.free_lanes.is_some();
        self.confirms.push((
            args.label.clone(),
            args.index,
            args.tracks.iter().map(|t| (t.track_index, t.existing_trigs, t.note_count)).collect(),
        ));
        !self.cancel
    }
}

fn tmp_stash(tag: &str) -> Stash {
    let dir =
        std::env::temp_dir().join(format!("digi-roll-a4-safe-write-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stash::at(dir)
}

/// A write of SYN2 (empty in A01): three notes on steps 0, 4 and 63.
fn syn2_write(index: u8) -> A4TrackWrite {
    let mut steps = vec![None; 64];
    steps[0] = Some(60);
    steps[4] = Some(64);
    steps[63] = Some(48);
    A4TrackWrite { index, track_index: 1, steps }
}

// --- the ceremony -------------------------------------------------------------

#[test]
fn the_whole_ceremony_runs_in_order_and_verifies_byte_identical() {
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("order");
    let mut hooks = Recorder { log: Some(Rc::clone(&box_.log)), ..Recorder::default() };

    let result =
        a4_safe_write_tracks(&mut box_, &stash, &[syn2_write(0)], &mut hooks, NOW).unwrap();

    assert!(result.ok, "the fake stores faithfully, so the verify must pass: {:?}", result.diffs);
    assert!(!result.cancelled);
    assert_eq!(result.written, 3);
    assert_eq!(result.tracks, vec![1]);
    assert_eq!(result.label, "A01");
    // The five steps in their one legal order: re-fetch, (confirm), backup,
    // send, re-read. The backup lands between the fetch and the send — rule 1's
    // whole point — and nothing is sent twice.
    assert_eq!(box_.log(), vec!["fetch 0", "backup", "send 0", "fetch 0"]);

    // What the box now holds reads back with the trigs that were sent.
    let trigs = read_track_trigs(&box_.slots[&0], 1).unwrap();
    assert_eq!(trigs.len(), 3);
    assert_eq!(trigs[0].note, Some(60));
    assert_eq!(trigs[2].step, 64, "one-based, as the box counts");
}

#[test]
fn the_write_composes_on_the_destination_and_touches_only_the_named_lanes() {
    // The RMW promise, byte for byte: everything outside SYN2's trig and note
    // lanes — the p-lock pool above all, which has no writer — is the
    // destination's own, plus the one slot-marker byte the box itself writes on
    // every save.
    let before = a4_payload(A01);
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("carry");
    let mut hooks = Recorder::default();
    a4_safe_write_tracks(&mut box_, &stash, &[syn2_write(0)], &mut hooks, NOW).unwrap();

    let after = &box_.slots[&0];
    let base = track_base(1);
    let syn2_lanes = base..base + 192;
    let differing: Vec<usize> = (0..before.len())
        .filter(|i| before[*i] != after[*i])
        .filter(|i| !syn2_lanes.contains(i) && *i != SLOT_MARKER)
        .collect();
    assert!(
        differing.is_empty(),
        "a one-track write changed {} bytes outside its lanes: {:?}",
        differing.len(),
        &differing[..differing.len().min(8)]
    );
    // And the untouched slot is untouched entirely.
    assert_eq!(box_.slots[&15], a4_payload(A16), "the other slot must not move");
}

#[test]
fn the_confirm_counts_what_the_box_shows_and_carries_no_gen_2_facts() {
    // SYN1 holds A01's own 32 trigs; the write replaces them with one note. The
    // dialog's "replacing 32 trigs" comes from the ceremony's fetch, counted
    // with the same call the panel's survey uses.
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("confirm");
    let mut hooks = Recorder::default();
    let mut steps = vec![None; 64];
    steps[0] = Some(69);
    let write = A4TrackWrite { index: 0, track_index: 0, steps };

    a4_safe_write_tracks(&mut box_, &stash, &[write], &mut hooks, NOW).unwrap();

    assert_eq!(hooks.confirms, vec![("A01".to_string(), 0, vec![(0, 32, 1)])]);
    assert!(
        !hooks.saw_gen2_facts,
        "kit name, swing and the lane pool are not in the mapped format, and a dialog \
         handed a value would word it"
    );
}

#[test]
fn a_cancel_sends_nothing_and_says_so() {
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("cancel");
    let mut hooks =
        Recorder { cancel: true, log: Some(Rc::clone(&box_.log)), ..Recorder::default() };

    let result =
        a4_safe_write_tracks(&mut box_, &stash, &[syn2_write(0)], &mut hooks, NOW).unwrap();

    assert!(result.cancelled);
    assert_eq!(box_.log(), vec!["fetch 0"], "a cancel happens after the read and before the send");
    assert_eq!(box_.slots[&0], a4_payload(A01), "and the slot did not move");
    assert!(stash.backups(Some("analogfour")).is_empty(), "no consent, no backup entry");
}

#[test]
fn an_empty_track_clears_the_destination_deliberately() {
    // SYN1 has 32 trigs and the write names none: every step is `None`, which
    // is `ui::write`'s deliberate-clear path.
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("clear");
    let mut hooks = Recorder::default();
    let write = A4TrackWrite { index: 0, track_index: 0, steps: vec![None; 64] };

    let result = a4_safe_write_tracks(&mut box_, &stash, &[write], &mut hooks, NOW).unwrap();

    assert!(result.ok);
    assert_eq!(result.written, 0);
    assert!(read_track_trigs(&box_.slots[&0], 0).unwrap().is_empty(), "SYN1 cleared");
}

// --- the backup ----------------------------------------------------------------

#[test]
fn the_backup_is_the_untouched_destination_framed_as_the_message_the_box_would_take() {
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("backup");
    let mut hooks = Recorder::default();
    a4_safe_write_tracks(&mut box_, &stash, &[syn2_write(0)], &mut hooks, NOW).unwrap();

    let [backup] = hooks.backups.as_slice() else { panic!("one backup per slot") };
    assert_eq!(backup.slug, "analogfour");
    assert_eq!(backup.payload, a4_payload(A01), "the bytes before the write, verbatim");
    assert!(backup.name.starts_with("analogfour-A01-backup-"), "{}", backup.name);
    // Framed as an A4 pattern — the `0x54` the box would take again — not as a
    // gen-2 `0x50` pattern-kit, which would replay as a different message.
    let replay = parse_pattern(&backup.bytes).expect("a backup must replay");
    assert_eq!(replay.slot, 0);
    assert_eq!(replay.payload, a4_payload(A01));
    assert_eq!(backup.bytes[6], DUMP_A4_PATTERN);

    // And the stash row is filed under this box's slug, which is what the
    // restore list filters by.
    assert_eq!(stash.backups(Some("analogfour")).len(), 1);
}

#[test]
fn a_failing_backup_hook_stops_the_write_before_a_byte_is_sent() {
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("backup-fails");
    let mut hooks = Recorder {
        backup_fails: true,
        log: Some(Rc::clone(&box_.log)),
        ..Recorder::default()
    };

    let err = a4_safe_write_tracks(&mut box_, &stash, &[syn2_write(0)], &mut hooks, NOW)
        .expect_err("a caller that asked for a copy and did not get one is entitled to stop");
    assert!(matches!(err, WriteError::Backup(_)));
    assert_eq!(box_.log(), vec!["fetch 0", "backup"], "nothing was sent");
}

#[test]
fn a_restore_puts_an_a4_backup_back_and_snapshots_what_it_replaced() {
    // The recovery path a backup exists for, end to end: write, read the backup
    // out of the stash, send it back through the shared restore flow. This is
    // the path `Stash::payload`'s gen-2-only opcode filter silently broke for
    // every A4 backup until 2026-08-31 — listed, never readable.
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("restore");
    a4_safe_write_tracks(&mut box_, &stash, &[syn2_write(0)], &mut Recorder::default(), NOW)
        .unwrap();
    assert_ne!(box_.slots[&0], a4_payload(A01), "the write moved the slot");

    let [entry]: [_; 1] =
        stash.backups(Some("analogfour")).try_into().expect("one backup per slot");
    let stored = stash.payload(&entry.file).expect("an A4 backup the restore path can read");
    let result = safe_restore_pattern_kit(
        &mut box_,
        &stash,
        entry.index,
        &stored,
        &mut Recorder::default(),
        NOW,
    )
    .unwrap();

    assert!(result.ok, "{:?}", result.diffs);
    assert_eq!(box_.slots[&0], a4_payload(A01), "the slot is back to its pre-write bytes");
    // And what the restore replaced was snapshotted first, under the same slug.
    assert_eq!(stash.snapshots(Some("analogfour")).len(), 1);
}

// --- the verify -----------------------------------------------------------------

#[test]
fn a_box_that_stores_something_else_fails_the_verify_with_the_offsets() {
    let mut box_ = FakeA4 { corrupt_on_store: true, ..FakeA4::new() };
    let stash = tmp_stash("verify");
    let mut hooks = Recorder::default();

    let result =
        a4_safe_write_tracks(&mut box_, &stash, &[syn2_write(0)], &mut hooks, NOW).unwrap();

    assert!(!result.ok, "the re-read differs from what was sent");
    assert!(!result.cancelled);
    assert_eq!(result.diffs.len(), 1);
    assert_eq!(result.diffs[0].offset, 6_000);
}

// --- the gate -------------------------------------------------------------------

#[test]
fn each_flow_refuses_the_other_format_by_name() {
    // A digi routed to the A4 flow would have its pattern-kit read at
    // `a4_pattern`'s offsets; an A4 routed to the gen-2 flow would be encoded
    // with a `Spec` it does not have. Both are mis-wirings, refused in words.
    let stash = tmp_stash("cross");

    let mut digi = FakeA4 { identity: Some(dt2_identity()), ..FakeA4::new() };
    let err = a4_safe_write_tracks(&mut digi, &stash, &[syn2_write(0)], &mut Recorder::default(), NOW)
        .expect_err("a DT2 is not a gen-1 box");
    assert!(err.to_string().contains("gen-2"), "{err}");

    let mut a4 = FakeA4::new();
    let gen2_write = TrackWrite { index: 0, track_index: 0, ..TrackWrite::default() };
    let err = safe_write_tracks(&mut a4, &stash, &[gen2_write], &mut Recorder::default(), NOW)
        .expect_err("an A4 is not a gen-2 box");
    assert!(err.to_string().contains("Analog Four flow"), "{err}");
}

#[test]
fn an_unverified_build_is_refused_before_anything_is_read() {
    let mut box_ = FakeA4 { identity: Some(a4_identity("0196")), ..FakeA4::new() };
    let stash = tmp_stash("build");
    let err =
        a4_safe_write_tracks(&mut box_, &stash, &[syn2_write(0)], &mut Recorder::default(), NOW)
            .expect_err("0196 has never been write-verified");
    assert!(matches!(err, WriteError::Gate(_)));
    assert!(err.to_string().contains("0196"), "{err}");
    assert!(box_.log().is_empty(), "the gate runs before the fetch");
}

// --- refusals that need no box ----------------------------------------------------

#[test]
fn a_malformed_write_is_refused_before_the_fetch() {
    let stash = tmp_stash("shape");
    let assert_refused = |writes: &[A4TrackWrite], needle: &str| {
        let mut box_ = FakeA4::new();
        let err = a4_safe_write_tracks(&mut box_, &stash, writes, &mut Recorder::default(), NOW)
            .expect_err(needle);
        assert!(err.to_string().contains(needle), "wanted {needle:?} in {err}");
        assert!(box_.log().is_empty(), "refused before any I/O: {needle}");
    };

    assert_refused(&[], "nothing to write");
    assert_refused(
        &[syn2_write(0), A4TrackWrite { track_index: 2, ..syn2_write(1) }],
        "one write, one slot",
    );
    assert_refused(&[syn2_write(0), syn2_write(0)], "named twice");
    assert_refused(
        &[A4TrackWrite { index: 0, track_index: 1, steps: vec![None; 63] }],
        "63 steps",
    );
    assert_refused(&[A4TrackWrite { index: 0, track_index: 6, steps: vec![None; 64] }], "no track");
}

#[test]
fn two_tracks_go_as_one_slot_write_with_one_backup() {
    // `ui::sync`'s shape: SYN1 replaced and SYN2 filled, one press, one fetch,
    // one backup, one send, one verify.
    let mut box_ = FakeA4::new();
    let stash = tmp_stash("slot");
    let mut hooks = Recorder { log: Some(Rc::clone(&box_.log)), ..Recorder::default() };
    let mut syn1_steps = vec![None; 64];
    syn1_steps[8] = Some(52);
    let writes = [A4TrackWrite { index: 0, track_index: 0, steps: syn1_steps }, syn2_write(0)];

    let result = a4_safe_write_tracks(&mut box_, &stash, &writes, &mut hooks, NOW).unwrap();

    assert!(result.ok);
    assert_eq!(result.written, 4);
    assert_eq!(result.tracks, vec![0, 1]);
    assert_eq!(box_.log(), vec!["fetch 0", "backup", "send 0", "fetch 0"]);
    assert_eq!(hooks.backups.len(), 1, "one backup per slot, not per track");
    assert_eq!(read_track_trigs(&box_.slots[&0], 0).unwrap().len(), 1);
    assert_eq!(read_track_trigs(&box_.slots[&0], 1).unwrap().len(), 3);
    // Six tracks of 751 bytes each: the two written ones aside, nothing moved.
    let before = a4_payload(A01);
    let after = &box_.slots[&0];
    for t in 2..6 {
        let lanes = track_base(t)..track_base(t) + TRACK_STRIDE;
        assert!(
            before[lanes.clone()] == after[lanes],
            "track {t} was not named and must not move"
        );
    }
}
