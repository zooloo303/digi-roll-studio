//! The safe write flow, end to end against a fake box.
//!
//! Ported from `test/safe-write.test.js`, which is Phase 6's exit criterion. The
//! point of the JS suite — and of this one — is that PLAN.md §7's safety rules are
//! *provable* here rather than only observable on hardware: backup before send,
//! always re-fetch, allowlist at send time, verify by re-read. Every one of them
//! is an ordering claim, so the fake box logs every exchange and the tests assert
//! on the log.
//!
//! Every expected value below was derived by running the JS original under node
//! against **these** fixture files, on these slots and tracks, before being
//! written down (`node /tmp/safe-write-derive.mjs`, the recipe preserved in this
//! header) — so the suite pins digi-roll's hardware-verified behaviour rather
//! than this port's output.
//!
//! ## Where this suite differs from the JS, and why
//!
//! * **The fixtures.** The JS runs against the ~16 MB DT2 project dump and its
//!   128 slots; that dump is deliberately not committed (PLAN.md Phase 1). Two
//!   committed single-pattern captures stand in as slots 0 and 1, and the
//!   substitution makes the p-lock cases *stronger*: slot 1 is the Phase 0
//!   plock-final capture, so the destination track already holds ten
//!   box-written lanes and "the caller's lanes are the truth" is exercised
//!   against real automation rather than an empty pool.
//! * **"Refuses to write at all without a backup hook" has no body here.** The JS
//!   checks `typeof onBackup !== 'function'` at run time; in Rust `on_backup` is
//!   the one required method of [`WriteHooks`], so a caller without a backup path
//!   does not compile. The reachable half of that test is kept: a hook that
//!   *fails* aborts before anything is sent.
//! * **There is no download, so the wording and the hook both changed.** The
//!   browser's backup *was* a file download and the stash was a second copy;
//!   here the stash is the backup, so `BACKUP_LINE` names it, `on_backup` is an
//!   optional extra copy rather than rule 1's carrier, and a stash that cannot be
//!   written **aborts the write**. That last one is the inversion worth watching:
//!   the tests below assert the abort, where an earlier draft of this suite
//!   asserted the write went through anyway.
//! * **`safe_restore_pattern_kit` takes consent through its own hook.** See the
//!   note on [`WriteHooks::confirm_restore`] — a restore must not need the bytes
//!   it is replacing to decode.


use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::common::payload;
use digi_protocol::backup_stash::{Stash, StashEntry, StashError};
use digi_protocol::device::{identity_from_responses, product_for_family, DeviceIdentity, DeviceResponse};
use digi_protocol::pattern::{
    decode_pattern_kit, dn2_spec, dt2_spec, encode_track_notes, track_notes, Note,
};
use digi_protocol::pattern_settings::read_swing;
use digi_protocol::plocks::{read_track_plocks, LaneWrite};
use digi_protocol::protocol::{
    split_sysex_stream, SysExKind, DUMP_PATTERN_KIT, FAMILY_DIGITAKT_2, FAMILY_DIGITONE_2,
};
use digi_protocol::safe_write::{
    pattern_kit_backup, pattern_kit_file, restore_result_message, safe_restore_pattern_kit,
    safe_write_track, safe_write_tracks, write_impact_lines, write_result_message, ConfirmArgs,
    ImpactArgs, PatternIo, PatternKitFile, Timestamp, TrackWrite, WriteError, WriteHooks,
    WriteResult, BACKUP_LINE,
};
use digi_protocol::trig_cond::TrigSetting;

/// Slot 0: a real DT2 pattern with 15 trigs on track 1 and an empty lane pool.
const A01: &str = "digitakt2-A01-conditions-2026-08-02.syx";
/// Slot 1: the Phase 0 capture — 4 trigs on track 1, ten p-lock lanes on it, one
/// more on track 2, 69 lanes free.
const A02: &str = "digitakt2-A01-plock-final-2026-08-04.syx";
/// A DN2 pattern that is **not straight**: swing 65. The only committed capture
/// that can catch a write which resets swing, and the reason the DN2 box below
/// exists — see `a_write_that_does_not_model_swing_leaves_a_swung_pattern_swung`.
const DN2_SWUNG: &str = "dn2-swing-65.syx";
/// A second DN2 pattern, at swing 78, so "only the written slot moved" has teeth
/// on this box too.
const DN2_FRESH: &str = "dn2-fresh-A01.syx";

/// The instant every backup name in this suite is stamped with: 2026-08-01
/// 12:34:56 UTC, the one the JS suite uses.
const NOW: Timestamp = Timestamp {
    year: 2026,
    month: 8,
    day: 1,
    hour: 12,
    minute: 34,
    second: 56,
};

fn dt2_identity(build: &str) -> DeviceIdentity {
    let dev = DeviceResponse {
        product_id: 42,
        supported_ids: vec![0x60],
        reported_name: String::new(),
    };
    identity_from_responses(&dev, build.into(), "1.15B".into())
}

/// The exchange log, shared by the box and the hooks.
///
/// Shared rather than owned by the box because the strongest ordering claim in
/// this suite — the backup hook ran *between* the fetch and the send — needs one
/// timeline with entries from both, which is what the JS gets for free by having
/// its hook close over the fake box.
type Log = Rc<RefCell<Vec<String>>>;

fn dn2_identity(build: &str) -> DeviceIdentity {
    let dev = DeviceResponse {
        product_id: 43,
        supported_ids: vec![0x60],
        reported_name: String::new(),
    };
    identity_from_responses(&dev, build.into(), "1.10D".into())
}

/// A box that records every exchange, so the tests can assert on ordering.
struct FakeBox {
    identity: Option<DeviceIdentity>,
    slots: BTreeMap<u8, Vec<u8>>,
    log: Log,
    /// The box stores something other than what it was sent — the verify step's
    /// whole reason for existing.
    corrupt_on_store: bool,
}

impl FakeBox {
    fn new() -> Self {
        Self {
            identity: Some(dt2_identity("0070")),
            slots: BTreeMap::from([(0, payload(A01)), (1, payload(A02))]),
            log: Log::default(),
            corrupt_on_store: false,
        }
    }

    fn corrupting() -> Self {
        Self { corrupt_on_store: true, ..Self::new() }
    }

    fn on_build(build: &str) -> Self {
        Self { identity: Some(dt2_identity(build)), ..Self::new() }
    }

    /// A DN2 holding two patterns that are not straight — swing 65 in slot 0 and
    /// 78 in slot 1.
    fn dn2() -> Self {
        Self {
            identity: Some(dn2_identity("0049")),
            slots: BTreeMap::from([(0, payload(DN2_SWUNG)), (1, payload(DN2_FRESH))]),
            log: Log::default(),
            corrupt_on_store: false,
        }
    }

    fn slot(&self, index: u8) -> &[u8] {
        &self.slots[&index]
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// Hooks that write into this box's timeline.
    fn hooks(&self) -> Recorder {
        Recorder { log: Some(Rc::clone(&self.log)), ..Default::default() }
    }
}

impl PatternIo for FakeBox {
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
            stored[20_000] ^= 0xff;
        }
        self.slots.insert(index, stored);
        Ok(())
    }
}

/// Hooks that record what they were handed. The default `Recorder` consents and
/// its backup hook succeeds; the fields turn each behaviour off.
#[derive(Default)]
struct Recorder {
    statuses: Vec<String>,
    logs: Vec<String>,
    backups: Vec<PatternKitFile>,
    /// Every confirm's `(label, track_index, existing_trigs, note_count, swing,
    /// box_plock_count, free_lanes)` — everything a dialog would say.
    ///
    /// Single-track writes only, because that is the shape all but a handful of
    /// these tests drive and flattening a slot write into it would lose which
    /// numbers belonged to which track. `slot_confirms` is the general one.
    confirms: Vec<(String, usize, usize, usize, u8, usize, usize)>,
    /// Every confirm's `(label, swing, free_lanes, per-track (track_index,
    /// existing_trigs, note_count, box_plock_count))`.
    ///
    /// The pattern-wide half is recorded *once* per confirm on purpose: a slot
    /// write shares one swing byte and one pool of 80 across every track it
    /// names, and a recorder that repeated them per track would let a dialog
    /// that budgets the pool sixteen times over still pass.
    slot_confirms: Vec<(String, u8, usize, Vec<(usize, usize, usize, usize)>)>,
    restore_confirms: Vec<(String, u8)>,
    /// Kit name from the last confirm, which is the proof the *pattern* came
    /// through and not just numbers about it.
    kit_name: Option<String>,
    cancel: bool,
    cancel_restore: bool,
    backup_fails: Option<String>,
    /// The box's own timeline, so the backup hook can mark where in it it ran.
    log: Option<Log>,
    /// Filled by the backup hook from the stash, to prove the stash went first.
    stash_when_backup_ran: Option<Vec<StashEntry>>,
    stash: Option<Stash>,
}

impl WriteHooks for Recorder {
    fn on_backup(&mut self, backup: &PatternKitFile) -> Result<(), String> {
        if let Some(log) = &self.log {
            log.borrow_mut().push("backup".into());
        }
        if let Some(stash) = &self.stash {
            self.stash_when_backup_ran = Some(stash.backups(Some("digitakt2")));
        }
        self.backups.push(backup.clone());
        match &self.backup_fails {
            Some(why) => Err(why.clone()),
            None => Ok(()),
        }
    }

    fn confirm(&mut self, args: &ConfirmArgs) -> bool {
        // This recorder only ever sits under the gen-2 flow, which always
        // decodes the destination and reads its swing and lane pool — so the
        // three `Option`s (they are `None` only on the A4's gen-1 flow) are
        // unwrapped rather than threaded through every assertion's tuple.
        let swing = args.swing.expect("a gen-2 confirm carries the box's swing");
        let free_lanes = args.free_lanes.expect("a gen-2 confirm carries the pool");
        self.slot_confirms.push((
            args.label.clone(),
            swing,
            free_lanes,
            args.tracks
                .iter()
                .map(|t| (t.track_index, t.existing_trigs, t.note_count, t.box_plocks.len()))
                .collect(),
        ));
        if let [track] = args.tracks.as_slice() {
            self.confirms.push((
                args.label.clone(),
                track.track_index,
                track.existing_trigs,
                track.note_count,
                swing,
                track.box_plocks.len(),
                free_lanes,
            ));
        }
        self.kit_name = Some(
            args.pattern_kit
                .expect("a gen-2 confirm carries the decoded destination")
                .kit
                .name
                .clone(),
        );
        !self.cancel
    }

    fn confirm_restore(&mut self, label: &str, index: u8) -> bool {
        self.restore_confirms.push((label.to_string(), index));
        !self.cancel_restore
    }

    fn on_status(&mut self, status: &str) {
        self.statuses.push(status.to_string());
    }

    fn on_log(&mut self, line: &str) {
        self.logs.push(line.to_string());
    }
}

/// A stash in its own temp directory, cleaned up by the caller.
fn tmp_stash(tag: &str) -> Stash {
    let dir = std::env::temp_dir().join(format!(
        "digi-roll-safe-write-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Stash::at(dir)
}

/// The JS suite's two notes.
fn bassline() -> Vec<(Note, TrigSetting)> {
    vec![
        (
            Note { step: 0, pitch: 36, velocity: 110, len_steps: 2.0, micro: 0.0 },
            TrigSetting::default(),
        ),
        (
            Note { step: 6, pitch: 41, velocity: 127, len_steps: 4.0, micro: 5.0 / 24.0 },
            TrigSetting::default(),
        ),
    ]
}

fn write_to(index: u8, track_index: usize) -> TrackWrite {
    TrackWrite { index, track_index, notes: bassline(), ..Default::default() }
}

fn lane_of(param_id: u8, by_step: &[(usize, u16)]) -> LaneWrite {
    let mut values = vec![None; 128];
    for &(step, v) in by_step {
        values[step] = Some(v);
    }
    LaneWrite::new(param_id, values)
}

fn lane_ids(spec: &digi_protocol::pattern::Spec, payload: &[u8], track: usize) -> Vec<(usize, u8)> {
    read_track_plocks(spec, payload, track)
        .unwrap()
        .iter()
        .map(|l| (l.lane, l.param_id))
        .collect()
}

// --- the sequence ------------------------------------------------------------

#[test]
fn it_fetches_backs_up_writes_then_reads_back_to_verify_in_that_order() {
    // The one test the other rules hang off: every safety rule in PLAN.md §7 rule
    // 2's list is a claim about *when* something happens.
    let stash = tmp_stash("order");
    let mut box_ = FakeBox::new();
    let mut hooks = box_.hooks();
    let result =
        safe_write_track(&mut box_, &stash, &write_to(1, 2), &mut hooks, NOW).expect("a clean write");

    // One timeline, and every safety rule about *when* is legible in it: the
    // target is re-fetched, the backup happens before a byte is sent, and the
    // slot is read back afterwards to be compared.
    assert_eq!(box_.log(), ["fetch 1", "backup", "send 1", "fetch 1"]);
    assert_eq!(hooks.backups.len(), 1);
    assert!(result.ok);
    assert!(!result.cancelled);
    assert_eq!(result.diffs, vec![]);
    assert_eq!(result.written, 2);
    assert_eq!(result.dropped, 0);
    assert_eq!(result.warnings, Vec::<String>::new());
    assert_eq!(result.label, "A02");
    assert!(
        hooks.statuses.iter().any(|s| s.contains("Verifying")),
        "the verify step announces itself: {:?}",
        hooks.statuses
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn the_backup_gets_the_untouched_bytes_before_anything_is_sent() {
    let stash = tmp_stash("backup-bytes");
    let mut box_ = FakeBox::new();
    let original = box_.slot(1).to_vec();
    let mut hooks = Recorder::default();
    safe_write_track(&mut box_, &stash, &write_to(1, 2), &mut hooks, NOW).unwrap();

    let backup = &hooks.backups[0];
    assert_eq!(backup.payload, original);
    assert_eq!(backup.index, 1);
    assert_eq!(backup.name, "digitakt2-A02-backup-2026-08-01T12-34-56.syx");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_backup_that_fails_aborts_the_write_before_a_byte_is_sent() {
    // The reachable half of the JS's "refuses to write without a backup hook":
    // the other half is a compile error now. See the file header.
    let stash = tmp_stash("backup-fails");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder { backup_fails: Some("disk full".into()), ..box_.hooks() };
    let err = safe_write_track(&mut box_, &stash, &write_to(1, 2), &mut hooks, NOW)
        .expect_err("a failed backup is not a write");

    assert_eq!(err, WriteError::Backup("disk full".into()));
    assert!(err.to_string().contains("nothing was written"));
    assert_eq!(
        box_.log(),
        ["fetch 1", "backup"],
        "fetched, tried to back up, then stopped — no send"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn it_re_fetches_the_target_instead_of_trusting_an_earlier_read() {
    // Something — the user, on the box — changes the pattern after an import.
    // The write must build on *those* bytes, or it silently reverts them.
    let stash = tmp_stash("refetch");
    let mut box_ = FakeBox::new();
    let spec = dt2_spec();
    let (moved, _) = encode_track_notes(
        &spec,
        box_.slot(0),
        10,
        &[Note { step: 5, pitch: 72, velocity: 42, len_steps: 1.0, micro: 0.0 }],
    )
    .unwrap();
    box_.slots.insert(0, moved);

    let mut hooks = Recorder::default();
    safe_write_track(&mut box_, &stash, &write_to(0, 2), &mut hooks, NOW).unwrap();

    let after = decode_pattern_kit(&spec, box_.slot(0)).unwrap();
    assert_eq!(
        track_notes(&after, 2).iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![36, 41],
        "our track landed"
    );
    assert_eq!(
        track_notes(&after, 10),
        vec![Note { step: 5, pitch: 72, velocity: 42, len_steps: 1.0, micro: 0.0 }],
        "and the change made on the box survived untouched"
    );
    assert_eq!(track_notes(&after, 0).len(), 15, "as did the fixture's own track 1");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn it_actually_puts_the_notes_on_the_box() {
    let stash = tmp_stash("notes");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    safe_write_track(&mut box_, &stash, &write_to(1, 2), &mut hooks, NOW).unwrap();

    let kit = decode_pattern_kit(&dt2_spec(), box_.slot(1)).unwrap();
    assert_eq!(
        track_notes(&kit, 2),
        vec![
            Note { step: 0, pitch: 36, velocity: 110, len_steps: 2.0, micro: 0.0 },
            Note { step: 6, pitch: 41, velocity: 127, len_steps: 4.0, micro: 5.0 / 24.0 },
        ]
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_verify_mismatch_is_reported_loudly_rather_than_claimed_as_success() {
    let stash = tmp_stash("corrupt");
    let mut box_ = FakeBox::corrupting();
    let mut hooks = Recorder::default();
    let result = safe_write_track(&mut box_, &stash, &write_to(1, 2), &mut hooks, NOW).unwrap();

    assert!(!result.ok);
    assert_eq!(result.diffs.len(), 1);
    assert_eq!(result.diffs[0].offset, 20_000);
    assert_eq!((result.diffs[0].sent, result.diffs[0].read), (Some(255), Some(0)));
    let message = write_result_message(&result);
    assert!(message.is_error);
    assert!(message.text.contains("Write verify FAILED for A02 T3"), "{}", message.text);
    assert!(
        message.text.contains("digitakt2-A02-backup-2026-08-01T12-34-56.syx"),
        "the report names the backup that can undo it: {}",
        message.text
    );
    assert!(
        message.text.contains("in Backups"),
        "and says where to find it, now that it is a list and not a file in Downloads: {}",
        message.text
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn cancelling_writes_nothing_and_takes_no_backup() {
    let stash = tmp_stash("cancel");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder { cancel: true, ..Default::default() };
    let result = safe_write_track(&mut box_, &stash, &write_to(1, 2), &mut hooks, NOW).unwrap();

    assert!(result.cancelled);
    assert!(!result.ok);
    assert_eq!(result.written, 0);
    assert!(result.backup.is_none());
    assert!(result.payload.is_none());
    assert!(hooks.backups.is_empty(), "no backup was taken");
    assert_eq!(box_.log(), ["fetch 1"]);
    assert_eq!(write_result_message(&result).text, "Write cancelled");
    assert!(!write_result_message(&result).is_error);
    // A cancel must not leave a stash entry either — the JS stashes after the
    // confirm for exactly this reason.
    assert_eq!(stash.backups(None), vec![]);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn an_os_build_that_has_never_been_write_verified_is_refused() {
    let stash = tmp_stash("gate");
    let mut box_ = FakeBox::on_build("9999");
    let mut hooks = Recorder::default();
    let err = safe_write_track(&mut box_, &stash, &write_to(1, 0), &mut hooks, NOW)
        .expect_err("an unverified build cannot be written to");

    assert_eq!(
        err,
        WriteError::Gate("OS build 9999 isn't write-verified yet — read-only".into())
    );
    assert_eq!(box_.log(), Vec::<String>::new(), "not even a fetch");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_box_with_no_identity_cannot_be_written_to_at_all() {
    let stash = tmp_stash("no-id");
    let mut box_ = FakeBox { identity: None, ..FakeBox::new() };
    let mut hooks = Recorder::default();
    let err = safe_write_track(&mut box_, &stash, &write_to(1, 0), &mut hooks, NOW).unwrap_err();
    assert_eq!(err, WriteError::Gate("no device connected".into()));
    assert_eq!(box_.log(), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- what the confirm hook is told -------------------------------------------

#[test]
fn the_confirm_hook_is_told_exactly_what_is_about_to_be_overwritten() {
    let stash = tmp_stash("confirm");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    safe_write_track(&mut box_, &stash, &write_to(0, 0), &mut hooks, NOW).unwrap();

    // (label, track, existing trigs, notes offered, swing on the box, lanes the
    // track holds, lanes free in the pool)
    assert_eq!(hooks.confirms, vec![("A01".to_string(), 0, 15, 2, 50, 0, 80)]);
    assert_eq!(hooks.kit_name.as_deref(), Some("KIT 1"));
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn the_confirm_hook_sees_the_lanes_the_track_already_has_on_the_box() {
    // Slot 1 is the Phase 0 capture: ten lanes on track 1, one on track 2, 69
    // free. Only knowable after the re-fetch, which is why it is handed over.
    let stash = tmp_stash("confirm-lanes");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    let write = TrackWrite {
        plocks: Some(vec![lane_of(44, &[(0, 4096)])]),
        ..write_to(1, 0)
    };
    safe_write_track(&mut box_, &stash, &write, &mut hooks, NOW).unwrap();

    assert_eq!(hooks.confirms, vec![("A02".to_string(), 0, 4, 2, 50, 10, 69)]);
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- several tracks, one slot --------------------------------------------------
//
// Phase 10's mass send is why `safe_write_tracks` exists. The claim these tests
// hold down is that writing six tracks of a slot is **one** of everything —
// fetch, backup, send, verify — rather than six, because six backups a box would
// put 32 entries of a fifty-entry ring into one press and leave the recovery path
// the feature depends on full of the same slot.

#[test]
fn several_tracks_of_one_slot_are_one_fetch_one_backup_one_send_one_verify() {
    let stash = tmp_stash("slot-once");
    let mut box_ = FakeBox::new();
    let mut hooks = box_.hooks();
    let writes = vec![write_to(0, 0), write_to(0, 4), write_to(0, 9)];
    let result = safe_write_tracks(&mut box_, &stash, &writes, &mut hooks, NOW).unwrap();

    assert!(result.ok);
    assert_eq!(result.tracks, vec![0, 4, 9]);
    assert_eq!(result.written, 6, "two notes on each of three tracks");
    // The whole timeline. Three tracks, and still exactly one of each step — the
    // ordering claim of `it_fetches_backs_up_writes_then_reads_back_to_verify`
    // scaled up, which is the thing a per-track loop would fail.
    assert_eq!(box_.log(), vec!["fetch 0", "backup", "send 0", "fetch 0"]);
    assert_eq!(stash.backups(Some("digitakt2")).len(), 1, "one press, one backup");

    // And all three tracks actually hold the notes.
    let after = decode_pattern_kit(&dt2_spec(), box_.slot(0)).unwrap();
    for track in [0, 4, 9] {
        let notes = track_notes(&after, track);
        assert_eq!(notes.len(), 2, "track {track}");
        assert_eq!(notes[0].pitch, 36, "track {track}");
    }
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_slot_write_asks_once_about_every_track_it_names() {
    // One dialog, not three. The per-track numbers are per track; the swing and
    // the lane budget are stated once because the slot has one of each.
    let stash = tmp_stash("slot-confirm");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    let writes = vec![write_to(1, 0), write_to(1, 1)];
    safe_write_tracks(&mut box_, &stash, &writes, &mut hooks, NOW).unwrap();

    // Slot 1 is the Phase 0 capture: 4 trigs and ten lanes on track 1, one trig
    // and one lane on track 2, 69 lanes free across the whole pattern.
    assert_eq!(
        hooks.slot_confirms,
        vec![("A02".to_string(), 50, 69, vec![(0, 4, 2, 10), (1, 1, 2, 1)])]
    );
    assert_eq!(hooks.kit_name.as_deref(), Some("KIT 1"));
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_cancelled_slot_write_names_every_track_it_was_going_to_touch() {
    let stash = tmp_stash("slot-cancel");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder { cancel: true, ..box_.hooks() };
    let writes = vec![write_to(0, 0), write_to(0, 1)];
    let result = safe_write_tracks(&mut box_, &stash, &writes, &mut hooks, NOW).unwrap();

    assert!(result.cancelled);
    assert_eq!(result.tracks, vec![0, 1], "a refusal still says what was refused");
    assert_eq!(box_.log(), vec!["fetch 0"], "nothing sent, nothing backed up");
    assert!(stash.backups(Some("digitakt2")).is_empty());
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn one_backup_for_a_whole_slot_names_no_single_track() {
    // The restore list's row is the same row either way — it puts all sixteen
    // tracks back — so a slot write reads as "before a write" rather than
    // claiming one track it happened to start with.
    let stash = tmp_stash("slot-backup-label");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    safe_write_tracks(&mut box_, &stash, &[write_to(0, 2), write_to(0, 5)], &mut hooks, NOW)
        .unwrap();
    let entry = &stash.backups(Some("digitakt2"))[0];
    assert_eq!(entry.track_index, None);
    assert!(entry.summary().contains("before a write"), "{}", entry.summary());

    // A one-track write is still specific, because there it can be.
    let stash = tmp_stash("slot-backup-label-one");
    let mut box_ = FakeBox::new();
    safe_write_track(&mut box_, &stash, &write_to(0, 2), &mut hooks, NOW).unwrap();
    let entry = &stash.backups(Some("digitakt2"))[0];
    assert_eq!(entry.track_index, Some(2));
    assert!(entry.summary().contains("before writing T3"), "{}", entry.summary());
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn the_lane_pool_is_spent_once_across_the_tracks_of_one_write() {
    // **The claim a per-track loop over `original` would fail silently.** Slot 0
    // has all 80 lanes free. Two tracks asking for 45 each cannot both fit; a
    // second track encoded against the *original* payload would see 80 free
    // again, report no shortfall, and the write would land with lanes missing
    // and nothing said.
    let stash = tmp_stash("slot-pool");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    let lanes = |from: u8| -> Vec<LaneWrite> {
        (from..from + 45).map(|k| lane_of(k, &[(0, k as u16)])).collect()
    };
    let writes = vec![
        TrackWrite { plocks: Some(lanes(1)), ..write_to(0, 2) },
        TrackWrite { plocks: Some(lanes(50)), ..write_to(0, 3) },
    ];
    let result = safe_write_tracks(&mut box_, &stash, &writes, &mut hooks, NOW).unwrap();

    assert!(result.ok, "the bytes verified — the shortfall is a warning, not a failure");
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert!(
        result.warnings[0].contains("all in use") && result.warnings[0].contains("10 lanes"),
        "45 + 45 into 80 leaves ten unwritten, and it has to be said: {:?}",
        result.warnings
    );

    let spec = dt2_spec();
    assert_eq!(lane_ids(&spec, box_.slot(0), 2).len(), 45, "the first track got all of its");
    assert_eq!(lane_ids(&spec, box_.slot(0), 3).len(), 35, "the second got what was left");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn the_result_line_counts_the_tracks_rather_than_listing_them() {
    // "A01 T1, T2, T5, T9, T11, T14" is a line nobody reads to the end, in a row
    // 300px wide. One track is still named, because there it fits and it is what
    // the single-track panel has always said.
    let stash = tmp_stash("slot-message");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    let result =
        safe_write_tracks(&mut box_, &stash, &[write_to(0, 0), write_to(0, 1)], &mut hooks, NOW)
            .unwrap();
    assert_eq!(
        write_result_message(&result).text,
        "Wrote 4 notes to 2 tracks of A01 — verified byte-identical"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

// The four ways a caller can describe a write that is not one write. Every one
// of them is refused before the box is touched: they are all mistakes about what
// the caller meant, and none of them needs a fetch to be recognised.

#[test]
fn a_write_with_no_tracks_in_it_is_refused_before_the_box_is_touched() {
    // Not a harmless no-op: it would fetch the slot, take a backup, and send the
    // bytes straight back — a ring entry and a 127 KB round trip for nothing.
    let stash = tmp_stash("slot-empty");
    let mut box_ = FakeBox::new();
    let mut hooks = box_.hooks();
    let err = safe_write_tracks(&mut box_, &stash, &[], &mut hooks, NOW).unwrap_err();
    assert!(matches!(err, WriteError::Encode(ref m) if m.contains("nothing to write")), "{err}");
    assert!(box_.log().is_empty(), "not even a fetch");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn one_write_is_one_slot() {
    let stash = tmp_stash("slot-two-slots");
    let mut box_ = FakeBox::new();
    let mut hooks = box_.hooks();
    let err = safe_write_tracks(&mut box_, &stash, &[write_to(0, 0), write_to(1, 1)], &mut hooks, NOW)
        .unwrap_err();
    assert!(matches!(err, WriteError::Encode(ref m) if m.contains("A01") && m.contains("A02")), "{err}");
    assert!(box_.log().is_empty());
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_track_named_twice_in_one_write_is_refused_rather_than_resolved() {
    // The second would win, silently, and the notes the caller thought they were
    // sending would never have existed.
    let stash = tmp_stash("slot-dupe-track");
    let mut box_ = FakeBox::new();
    let mut hooks = box_.hooks();
    let err = safe_write_tracks(&mut box_, &stash, &[write_to(0, 3), write_to(0, 3)], &mut hooks, NOW)
        .unwrap_err();
    assert!(matches!(err, WriteError::Encode(ref m) if m.contains("track 4 is named twice")), "{err}");
    assert!(box_.log().is_empty());
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn two_swings_for_one_pattern_are_refused_and_no_swing_is_not_a_second_answer() {
    let stash = tmp_stash("slot-two-swings");
    let mut box_ = FakeBox::new();
    let mut hooks = box_.hooks();
    let writes = vec![
        TrackWrite { swing: Some(50.0), ..write_to(0, 0) },
        TrackWrite { swing: Some(65.0), ..write_to(0, 1) },
    ];
    let err = safe_write_tracks(&mut box_, &stash, &writes, &mut hooks, NOW).unwrap_err();
    assert!(matches!(err, WriteError::Encode(ref m) if m.contains("two swings")), "{err}");
    assert!(box_.log().is_empty());

    // `None` means "leave the byte alone", which is not a competing answer — a
    // mass send builds one write per track from one pattern, and a track that
    // does not model swing must not veto the one that does.
    let writes = vec![
        TrackWrite { swing: Some(65.0), ..write_to(0, 0) },
        TrackWrite { swing: None, ..write_to(0, 1) },
    ];
    let result = safe_write_tracks(&mut box_, &stash, &writes, &mut hooks, NOW).unwrap();
    assert!(result.ok);
    assert_eq!(read_swing(&dt2_spec(), box_.slot(0)), 65);
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- p-lock lanes -------------------------------------------------------------

#[test]
fn p_lock_lanes_are_written_through_the_flow() {
    let stash = tmp_stash("plocks");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    let write = TrackWrite {
        plocks: Some(vec![lane_of(0x2a, &[(6, 4096)])]),
        ..write_to(0, 2)
    };
    let result = safe_write_track(&mut box_, &stash, &write, &mut hooks, NOW).unwrap();

    assert!(result.ok);
    assert_eq!(result.warnings, Vec::<String>::new());
    let spec = dt2_spec();
    let lanes = read_track_plocks(&spec, box_.slot(0), 2).unwrap();
    assert_eq!(lanes.len(), 1);
    assert_eq!((lanes[0].lane, lanes[0].param_id), (0, 0x2a));
    assert_eq!(lanes[0].values[6], Some(4096));
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn none_leaves_the_lane_pool_completely_alone_and_an_empty_vec_frees_it() {
    // `None` is different from `Some(vec![])`, which means "this track has no
    // lanes". A caller that does not model p-locks must not have an opinion about
    // them; a caller replacing a track's notes is saying its automation went too.
    let spec = dt2_spec();
    let seeded = lane_ids(&spec, &payload(A02), 0);
    assert_eq!(seeded.len(), 10, "the fixture has lanes for this to be a real claim");

    let stash = tmp_stash("plocks-none");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    safe_write_track(&mut box_, &stash, &write_to(1, 0), &mut hooks, NOW).unwrap();
    assert_eq!(lane_ids(&spec, box_.slot(1), 0), seeded, "None left all ten alone");

    let mut box_ = FakeBox::new();
    let write = TrackWrite { plocks: Some(Vec::new()), ..write_to(1, 0) };
    safe_write_track(&mut box_, &stash, &write, &mut hooks, NOW).unwrap();
    assert_eq!(lane_ids(&spec, box_.slot(1), 0), vec![], "an empty vec freed all ten");
    assert_eq!(
        lane_ids(&spec, box_.slot(1), 1),
        vec![(1, 44)],
        "and the neighbouring track's lane is still there"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_full_lane_pool_is_a_warning_on_an_otherwise_good_write() {
    // The notes landed and the bytes verified, but not everything was written —
    // so the result line has to shout rather than say "verified" and stop.
    let stash = tmp_stash("plocks-full");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    let lanes: Vec<LaneWrite> = (0u8..81).map(|k| lane_of(k, &[(0, k as u16)])).collect();
    let write = TrackWrite { plocks: Some(lanes), ..write_to(0, 2) };
    let result = safe_write_track(&mut box_, &stash, &write, &mut hooks, NOW).unwrap();

    assert!(result.ok, "the write itself verified");
    assert_eq!(
        result.warnings,
        vec!["the pattern's 80 p-lock lanes are all in use, so 1 lane (parameter 80) was not \
              written — free some p-locks on the box first"]
    );
    let message = write_result_message(&result);
    assert!(message.is_error, "a partial write must not read as a clean one");
    assert!(message.text.contains("all in use"), "{}", message.text);
    assert!(message.text.starts_with("Wrote 2 notes to A01 T3 — verified byte-identical — but "));
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- swing --------------------------------------------------------------------

#[test]
fn swing_travels_with_the_write_and_the_hook_is_told_what_it_is_replacing() {
    // Swing reaches every track in the slot, so the hook is handed the value the
    // box currently holds — a UI cannot warn about what it cannot see.
    let stash = tmp_stash("swing");
    let mut box_ = FakeBox::new();
    let mut hooks = Recorder::default();
    let write = TrackWrite { swing: Some(66.0), ..write_to(1, 2) };
    safe_write_track(&mut box_, &stash, &write, &mut hooks, NOW).unwrap();

    let spec = dt2_spec();
    assert_eq!(hooks.confirms[0].4, 50, "the fixture pattern is straight");
    assert_eq!(read_swing(&spec, box_.slot(1)), 66);
    assert_eq!(read_swing(&spec, box_.slot(0)), 50, "and only the written slot moved");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn swing_is_left_alone_when_the_caller_does_not_model_it() {
    let stash = tmp_stash("swing-none");
    let mut box_ = FakeBox::new();
    let spec = dt2_spec();
    let before = read_swing(&spec, box_.slot(1));
    let mut hooks = Recorder::default();
    safe_write_track(&mut box_, &stash, &write_to(1, 2), &mut hooks, NOW).unwrap();
    assert_eq!(read_swing(&spec, box_.slot(1)), before);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_write_that_does_not_model_swing_leaves_a_swung_pattern_swung() {
    // **The test the DT2 fixtures could not be.** `apply_swing`'s own `None` writes
    // the *minimum*, 50 — so a `safe_write_track` that called it unconditionally
    // would quietly straighten the destination, changing the feel of all sixteen
    // tracks. Every DT2 fixture in this repo is already at 50, which makes that bug
    // byte-invisible: it was planted deliberately on 2026-08-18 and the suite above
    // caught nothing.
    //
    // Same escape class as the trig-write session's scrub-every-track bug and the
    // params table's sentinel clamp — a guard that is real, and untested because
    // every case the fixtures reached was covered by something else. This is the one
    // committed capture that is not straight.
    let stash = tmp_stash("swing-swung");
    let mut box_ = FakeBox::dn2();
    let spec = dn2_spec();
    assert_eq!(read_swing(&spec, box_.slot(0)), 65, "the fixture is the evidence");

    let mut hooks = Recorder::default();
    let result = safe_write_track(&mut box_, &stash, &write_to(0, 3), &mut hooks, NOW).unwrap();

    assert!(result.ok);
    assert_eq!(result.written, 2);
    assert_eq!(read_swing(&spec, box_.slot(0)), 65, "swing did not move");
    assert_eq!(read_swing(&spec, box_.slot(1)), 78, "and neither did the other slot's");
    assert_eq!(hooks.confirms[0].4, 65, "the hook was told what the box holds");
    // The notes still landed, on the other box's struct — which this suite would
    // otherwise never exercise, the JS having had only a DT2 dump to run against.
    let kit = decode_pattern_kit(&spec, box_.slot(0)).unwrap();
    assert_eq!(
        track_notes(&kit, 3),
        vec![
            Note { step: 0, pitch: 36, velocity: 110, len_steps: 2.0, micro: 0.0 },
            Note { step: 6, pitch: 41, velocity: 127, len_steps: 4.0, micro: 5.0 / 24.0 },
        ]
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn straightening_a_swung_pattern_is_something_a_caller_can_still_ask_for() {
    // The other half of the pair: `Some(50)` on a pattern at 65 really does move it,
    // so the guard above is "only when asked" and not "never".
    let stash = tmp_stash("swing-flatten");
    let mut box_ = FakeBox::dn2();
    let spec = dn2_spec();
    let mut hooks = Recorder::default();
    let write = TrackWrite { swing: Some(50.0), ..write_to(0, 3) };
    safe_write_track(&mut box_, &stash, &write, &mut hooks, NOW).unwrap();
    assert_eq!(read_swing(&spec, box_.slot(0)), 50);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_dn2_write_is_gated_on_its_own_verified_build() {
    // The allowlist is per box, and until now nothing asserted that through the
    // flow rather than through `write_gate` alone.
    let stash = tmp_stash("dn2-gate");
    let mut box_ = FakeBox { identity: Some(dn2_identity("0070")), ..FakeBox::dn2() };
    let mut hooks = Recorder::default();
    let err = safe_write_track(&mut box_, &stash, &write_to(0, 3), &mut hooks, NOW).unwrap_err();
    assert_eq!(
        err,
        WriteError::Gate("OS build 0070 isn't write-verified yet — read-only".into()),
        "0070 is the DT2's verified build, not the DN2's"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- the backup file ----------------------------------------------------------

#[test]
fn a_backup_is_a_file_the_box_could_actually_be_sent_back() {
    let original = payload(A01);
    let backup = pattern_kit_backup("digitakt2", FAMILY_DIGITAKT_2, 0, &original, NOW);
    assert_eq!(backup.name, "digitakt2-A01-backup-2026-08-01T12-34-56.syx");

    let messages = split_sysex_stream(&backup.bytes);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, SysExKind::Dump);
    let dump = messages[0].dump.as_ref().unwrap();
    assert_eq!(dump.dump_type, DUMP_PATTERN_KIT);
    assert_eq!(dump.family, FAMILY_DIGITAKT_2);
    assert_eq!(dump.index, 0);
    assert!(dump.checksum_ok && dump.count_ok);
    assert_eq!(dump.payload, original);
}

#[test]
fn one_pattern_saves_as_a_file_named_for_what_it_is() {
    // `kind` is the word in the filename, so a pre-write backup and a plain "save
    // this pattern" sit apart on disk.
    let original = payload(A02);
    let file = pattern_kit_file(
        "digitakt2",
        FAMILY_DIGITAKT_2,
        2,
        &original,
        "pattern",
        Timestamp::from_unix_seconds(1_785_834_000),
    );
    assert_eq!(file.name, "digitakt2-A03-pattern-2026-08-04T09-00-00.syx");
    let messages = split_sysex_stream(&file.bytes);
    let dump = messages[0].dump.as_ref().unwrap();
    assert_eq!(dump.index, 2);
    assert!(dump.checksum_ok && dump.count_ok);
    assert_eq!(dump.payload, original);
}

#[test]
fn naming_a_file_needs_nothing_but_a_slug_and_a_family() {
    // A pattern decoded from a `.syx` never had a handshake, so the signature
    // must not ask for one — which is why this takes two values and not a
    // `DeviceIdentity`.
    let original = payload(A01);
    let file = pattern_kit_file("digitakt2", FAMILY_DIGITAKT_2, 0, &original, "pattern", NOW);
    assert_eq!(split_sysex_stream(&file.bytes)[0].dump.as_ref().unwrap().payload, original);
}

#[test]
fn a_box_can_be_named_from_a_dump_family_alone() {
    // The port of `PRODUCT_BY_FAMILY`, for the paths that read a file and so never
    // get a handshake to ask.
    let dt2 = product_for_family(FAMILY_DIGITAKT_2).expect("the DT2 family is known");
    assert_eq!((dt2.slug, dt2.product_id, dt2.name), ("digitakt2", 42, "Digitakt II"));
    let dn2 = product_for_family(FAMILY_DIGITONE_2).expect("the DN2 family is known");
    assert_eq!((dn2.slug, dn2.product_id, dn2.name), ("digitone2", 43, "Digitone II"));
    assert!(product_for_family(0x7f).is_none());
}

// --- the stash ----------------------------------------------------------------

#[test]
fn the_pre_write_bytes_are_stashed_before_the_backup_hook_runs() {
    // Rule 1's teeth: the caller's backup path can fail, so the stash copy goes
    // in *before* the hook is offered the bytes.
    let stash = tmp_stash("stash-first");
    let mut box_ = FakeBox::new();
    let original = box_.slot(0).to_vec();
    let mut hooks = Recorder { stash: Some(stash.clone()), ..box_.hooks() };
    safe_write_track(&mut box_, &stash, &write_to(0, 0), &mut hooks, NOW).unwrap();

    let seen = hooks.stash_when_backup_ran.expect("the hook looked");
    assert_eq!(seen.len(), 1, "already stashed when the hook ran");
    assert_eq!(stash.payload(&seen[0].file).as_deref(), Some(&original[..]));
    // And the row carries what a restore list needs, which the bytes do not give:
    // the box by name, what the box called the pattern, and the track the write
    // was about.
    assert_eq!(seen[0].device_name, "Digitakt II");
    assert_eq!(seen[0].kit_name, "KIT 1");
    assert_eq!(seen[0].track_index, Some(0));
    assert_eq!(seen[0].bank, "A01");
    assert_eq!(seen[0].kind, "backup");
    assert_eq!(seen[0].at, "2026-08-01T12:34:56Z");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_backup_hook_that_fails_aborts_the_write_but_leaves_the_stash_copy() {
    let stash = tmp_stash("stash-survives");
    let mut box_ = FakeBox::new();
    let original = box_.slot(0).to_vec();
    let mut hooks = Recorder { backup_fails: Some("blocked".into()), ..box_.hooks() };
    safe_write_track(&mut box_, &stash, &write_to(0, 0), &mut hooks, NOW).unwrap_err();

    assert_eq!(box_.log(), ["fetch 0", "backup"], "the backup was tried; nothing was sent");
    let stashed = stash.backups(Some("digitakt2"));
    assert_eq!(stashed.len(), 1);
    assert_eq!(
        stash.payload(&stashed[0].file).as_deref(),
        Some(&original[..]),
        "the bytes are still recoverable"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_stash_that_cannot_be_written_aborts_the_write() {
    // **The inversion, asserted.** While the backup was a browser download the
    // stash was a second copy, so its failure was a log line and the write went
    // on. It is now the only automatic copy, which makes its failure a failure of
    // rule 1 — so nothing is sent, and the destination pattern is still on the
    // box where it can be read again.
    //
    // A file where the directory should be is the cheapest way to make
    // `create_dir_all` fail.
    let blocked = std::env::temp_dir().join(format!("digi-roll-stash-blocked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&blocked);
    std::fs::write(&blocked, b"not a directory").unwrap();
    let stash = Stash::at(&blocked);

    let mut box_ = FakeBox::new();
    let untouched = box_.slot(0).to_vec();
    let mut hooks = box_.hooks();
    let err = safe_write_track(&mut box_, &stash, &write_to(0, 0), &mut hooks, NOW)
        .expect_err("no backup, no write");

    assert!(matches!(err, WriteError::Stash(StashError::Dir { .. })), "{err:?}");
    assert!(err.to_string().contains("nothing was written"), "{err}");
    assert_eq!(box_.log(), ["fetch 0"], "not even the backup hook ran");
    assert_eq!(box_.slot(0), &untouched[..], "and the slot is exactly as it was");
    let _ = std::fs::remove_file(&blocked);
}

#[test]
fn a_write_needs_no_backup_hook_at_all_now_that_the_stash_carries_rule_one() {
    // `on_backup` used to be the one required method of `WriteHooks`; the stash
    // does that job now, so a caller that does not want a second copy of every
    // backup simply does not implement it. `Bare` is the whole implementation.
    struct Bare;
    impl WriteHooks for Bare {}

    let stash = tmp_stash("bare-hooks");
    let mut box_ = FakeBox::new();
    let original = box_.slot(0).to_vec();
    let result = safe_write_track(&mut box_, &stash, &write_to(0, 0), &mut Bare, NOW).unwrap();

    assert!(result.ok);
    assert_eq!(result.written, 2);
    // Rule 1 held anyway, which is the point.
    let stashed = stash.backups(None);
    assert_eq!(stashed.len(), 1);
    assert_eq!(stash.payload(&stashed[0].file).as_deref(), Some(&original[..]));
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn fifty_writes_leave_fifty_restorable_backups() {
    // The ring's depth is the feature: a session's worth of mistakes stays
    // walkable back. Beyond fifty the oldest goes, and its file goes with it.
    let stash = tmp_stash("fifty");
    let mut box_ = FakeBox::new();
    for i in 0..(digi_protocol::backup_stash::STASH_MAX + 3) {
        // A distinct instant each time, so the ring's order is the write order.
        let now = Timestamp::from_unix_seconds(1_785_587_696 + i as i64);
        let mut hooks = Recorder::default();
        safe_write_track(&mut box_, &stash, &write_to(0, 0), &mut hooks, now).unwrap();
    }
    let stashed = stash.backups(None);
    assert_eq!(stashed.len(), digi_protocol::backup_stash::STASH_MAX);
    assert_eq!(stashed[0].at, "2026-08-01T12:35:48Z", "newest first");
    // Every row still resolves to bytes a restore could send.
    for row in &stashed {
        assert!(stash.payload(&row.file).is_some(), "{} is unreadable", row.file);
    }
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- restore ------------------------------------------------------------------

/// A backup taken the way the app takes one: `safe_write_track`'s own result.
fn written_slot(box_: &mut FakeBox, stash: &Stash) -> PatternKitFile {
    let mut hooks = Recorder::default();
    let result = safe_write_track(box_, stash, &write_to(0, 0), &mut hooks, NOW).unwrap();
    result.backup.expect("a write that was not cancelled has a backup")
}

#[test]
fn a_restore_puts_the_backed_up_bytes_back_and_verifies_by_re_read() {
    let stash = tmp_stash("restore");
    let mut box_ = FakeBox::new();
    let before = box_.slot(0).to_vec();
    let backup = written_slot(&mut box_, &stash);
    assert_ne!(box_.slot(0), &before[..], "the slot really did change first");

    box_.log.borrow_mut().clear();
    let mut hooks = Recorder::default();
    let result =
        safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
            .unwrap();

    assert!(result.ok);
    assert_eq!(result.diffs, vec![]);
    assert_eq!(result.label, "A01");
    assert_eq!(box_.slot(0), &before[..], "the slot is back to what it held");
    // Fetch-current (for the pre-restore backup), send, fetch again (verify).
    assert_eq!(box_.log(), ["fetch 0", "send 0", "fetch 0"]);
    assert_eq!(hooks.restore_confirms, vec![("A01".to_string(), 0)]);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_restore_backs_up_what_the_slot_holds_now_before_overwriting_it() {
    // The pre-restore backup is the botched state — the evidence — not the bytes
    // being restored.
    let stash = tmp_stash("restore-backup");
    let mut box_ = FakeBox::new();
    let backup = written_slot(&mut box_, &stash);
    let current = box_.slot(0).to_vec();

    let mut hooks = Recorder::default();
    safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
        .unwrap();

    let saved = &hooks.backups[0];
    assert_eq!(saved.payload, current);
    assert_eq!(saved.name, "digitakt2-A01-pre-restore-2026-08-01T12-34-56.syx");
    assert!(saved.name.contains("pre-restore"));
    // Stored, for the same reason the write's is: the state being reverted away
    // from may be the evidence of what went wrong. **And kept out of the restore
    // list**, because a list of patterns to restore is not the place for the one
    // you just decided was wrong — Neil's call, 2026-08-18.
    let snapshots = stash.snapshots(Some("digitakt2"));
    assert_eq!(snapshots.len(), 1);
    assert_eq!(stash.payload(&snapshots[0].file).as_deref(), Some(&current[..]));
    assert!(snapshots[0].is_snapshot());
    assert_eq!(snapshots[0].track_index, None, "a restore replaces the whole slot");
    assert!(snapshots[0].summary().contains("before a restore"), "{}", snapshots[0].summary());
    // The restore list holds the write's own pre-write backup and nothing else.
    let listed = stash.backups(Some("digitakt2"));
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].is_snapshot());
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_restore_re_checks_the_allowlist_gate_at_send_time() {
    // Not just when a button was enabled: the OS can be updated mid-session.
    let stash = tmp_stash("restore-gate");
    let mut box_ = FakeBox::new();
    let backup = written_slot(&mut box_, &stash);
    box_.identity = Some(dt2_identity("9999"));
    box_.log.borrow_mut().clear();

    let mut hooks = Recorder::default();
    let err =
        safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
            .unwrap_err();
    assert_eq!(
        err,
        WriteError::Gate("OS build 9999 isn't write-verified yet — read-only".into())
    );
    assert!(!box_.log().contains(&"send 0".to_string()));
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn cancelling_a_restore_sends_nothing() {
    let stash = tmp_stash("restore-cancel");
    let mut box_ = FakeBox::new();
    let backup = written_slot(&mut box_, &stash);
    let written = box_.slot(0).to_vec();
    box_.log.borrow_mut().clear();

    let mut hooks = Recorder { cancel_restore: true, ..Default::default() };
    let result =
        safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
            .unwrap();

    assert!(result.cancelled);
    assert_eq!(box_.log(), ["fetch 0"]);
    assert_eq!(box_.slot(0), &written[..]);
    assert!(hooks.backups.is_empty());
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_restore_reports_a_verify_mismatch_loudly_too() {
    let stash = tmp_stash("restore-corrupt");
    let mut box_ = FakeBox::new();
    let backup = written_slot(&mut box_, &stash);
    box_.corrupt_on_store = true;

    let mut hooks = Recorder::default();
    let result =
        safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
            .unwrap();
    assert!(!result.ok);
    assert_eq!(result.diffs.len(), 1);
    assert_eq!(result.diffs[0].offset, 20_000);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_restore_does_not_need_the_bytes_it_is_replacing_to_decode() {
    // The deviation this port makes deliberately: a restore exists to recover a
    // slot from a botched write, so requiring the botched bytes to decode would
    // lock the door from the inside. Half a payload is as undecodable as it gets.
    let stash = tmp_stash("restore-garbage");
    let good = payload(A01);
    let mut box_ = FakeBox::new();
    box_.slots.insert(0, good[..good.len() / 2].to_vec());

    let mut hooks = Recorder::default();
    let result =
        safe_restore_pattern_kit(&mut box_, &stash, 0, &good, &mut hooks, NOW).unwrap();
    assert!(result.ok, "the good bytes went back");
    assert_eq!(box_.slot(0), &good[..]);
    assert_eq!(hooks.restore_confirms, vec![("A01".to_string(), 0)]);
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- what a restore is reported as --------------------------------------------

#[test]
fn a_finished_restore_is_reported_as_a_restore() {
    let stash = tmp_stash("restore-message");
    let mut box_ = FakeBox::new();
    let backup = written_slot(&mut box_, &stash);

    let mut hooks = Recorder::default();
    let result =
        safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
            .unwrap();

    let message = restore_result_message(&result);
    assert_eq!(message.text, "Restored A01 — verified byte-identical");
    assert!(!message.is_error);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn the_writes_own_result_line_cannot_describe_a_restore() {
    // **Why `restore_result_message` exists at all**, pinned rather than argued.
    // A restore's result carries `written: 0` and an empty `tracks` — both
    // correct, because it replaces a whole slot and counts no notes — and
    // `write_result_message` reads both. Left to word the app's whole-pattern
    // revert, it makes three false claims in one line: a note count, a track
    // count, and the word "Wrote", when a restore puts all sixteen tracks back.
    //
    // **Phase 10 moved one of them and did not fix it.** `track_index: usize`
    // became `tracks: Vec<usize>` so a slot write could name several, which
    // turned "A01 T1" — a track the restore was not writing — into "0 tracks of
    // A01", a count that is wrong in the other direction. Better prose, same
    // three lies, same reason for the function below to exist.
    //
    // This test asserts the *wrong* wording deliberately. If someone widens
    // `write_result_message` to handle a restore properly, this failing is the
    // signal to delete it and the function it guards.
    let stash = tmp_stash("restore-wrong-message");
    let mut box_ = FakeBox::new();
    let backup = written_slot(&mut box_, &stash);

    let mut hooks = Recorder::default();
    let result =
        safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
            .unwrap();

    assert_eq!(
        write_result_message(&result).text,
        "Wrote 0 notes to 0 tracks of A01 — verified byte-identical",
        "the write's wording is wrong about a restore in three ways, which is the point"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_restore_that_did_not_verify_says_where_the_previous_state_went() {
    let stash = tmp_stash("restore-message-failed");
    let mut box_ = FakeBox::new();
    let backup = written_slot(&mut box_, &stash);
    box_.corrupt_on_store = true;

    let mut hooks = Recorder::default();
    let result =
        safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
            .unwrap();

    let message = restore_result_message(&result);
    assert!(message.is_error);
    assert!(message.text.starts_with("Restore verify FAILED for A01: 1+ bytes differ"), "{}", message.text);
    // The snapshot named is what the slot held before this attempt — the
    // pre-restore file, not the backup being restored from.
    assert!(
        message.text.contains("pre-restore"),
        "the line has to name a file that exists: {}",
        message.text
    );
    // And it stops there. A failed restore whose remedy is another restore is a
    // loop, and the result line is not where someone should be sent round it.
    assert!(!message.text.contains("restore it"), "{}", message.text);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_cancelled_restore_says_so_and_is_not_an_error() {
    let stash = tmp_stash("restore-message-cancel");
    let mut box_ = FakeBox::new();
    let backup = written_slot(&mut box_, &stash);

    let mut hooks = Recorder { cancel_restore: true, ..Default::default() };
    let result =
        safe_restore_pattern_kit(&mut box_, &stash, backup.index, &backup.payload, &mut hooks, NOW)
            .unwrap();

    let message = restore_result_message(&result);
    assert_eq!(message.text, "Restore cancelled");
    assert!(!message.is_error);
    let _ = std::fs::remove_dir_all(stash.dir());
}

// --- the wording --------------------------------------------------------------

fn impact<'a>(over: impl FnOnce(&mut ImpactArgs<'a>)) -> Vec<String> {
    let mut args = ImpactArgs { label: "A02", track: Some(1), ..Default::default() };
    over(&mut args);
    write_impact_lines(&args)
}

#[test]
fn a_write_that_only_replaces_the_tracks_trigs_says_nothing_extra() {
    assert_eq!(
        impact(|a| {
            a.track_prob = Some(100);
            a.swing = Some(50);
            a.box_swing = Some(50);
        }),
        Vec::<String>::new()
    );
}

#[test]
fn the_impact_lines_name_the_lanes_going_on_and_the_ones_being_cleared() {
    // The console's "Write to box" row once had its own inline copy of the write
    // sequence and so silently dropped conditions, PROB, p-locks and swing. These
    // sentences are the shared wording that stops any write path doing that again.
    let lanes = [LaneWrite::new(74, vec![])];
    let box_plocks = read_track_plocks(&dt2_spec(), &payload(A02), 0).unwrap();
    let text = impact(|a| {
        a.lanes = &lanes;
        a.box_plocks = &box_plocks[..2]; // lane 0 is param 44, lane 2 is param 65
        a.free_lanes = Some(40);
    })
    .join("\n");
    assert!(
        text.contains("This also writes 1 p-lock lane and clears 2 p-lock lanes that track has on the box."),
        "{text}"
    );
}

#[test]
fn clearing_lanes_is_named_even_when_the_write_has_none_of_its_own() {
    let box_plocks = read_track_plocks(&dt2_spec(), &payload(A02), 1).unwrap();
    assert_eq!(
        impact(|a| a.box_plocks = &box_plocks),
        vec!["This also clears 1 p-lock lane that track has on the box."]
    );
}

#[test]
fn a_pool_without_room_for_every_lane_is_warned_about_before_the_write() {
    let lanes: Vec<LaneWrite> = (0u8..5).map(|i| LaneWrite::new(i, vec![])).collect();
    let text = impact(|a| {
        a.lanes = &lanes;
        a.free_lanes = Some(2);
    })
    .join("\n");
    assert!(text.contains("This also writes 5 p-lock lanes."), "{text}");
    assert!(
        text.contains(
            "Careful: the pattern only has 2 spare p-lock lanes, so some of them won't fit — \
             you'll be told which."
        ),
        "{text}"
    );
}

#[test]
fn a_prob_default_that_is_not_100_is_named_and_100_stays_quiet() {
    assert_eq!(
        impact(|a| a.track_prob = Some(40)),
        vec!["That track's PROB default is also set to 40% — trigs without their own PROB lock \
              will play at those odds."]
    );
    assert_eq!(impact(|a| a.track_prob = Some(100)), Vec::<String>::new());
    assert_eq!(impact(|a| a.track_prob = None), Vec::<String>::new());
}

#[test]
fn swing_is_spelled_out_as_reaching_all_16_tracks_only_when_it_would_change() {
    let text = impact(|a| {
        a.swing = Some(62);
        a.box_swing = Some(50);
    })
    .join("\n");
    assert!(text.contains("Swing goes from 50 to 62"), "{text}");
    assert!(text.contains("all 16 tracks in A02, not just track 2"), "{text}");
    assert_eq!(
        impact(|a| {
            a.swing = Some(62);
            a.box_swing = Some(62);
        }),
        Vec::<String>::new()
    );
    // A caller not touching swing at all.
    assert_eq!(
        impact(|a| {
            a.swing = None;
            a.box_swing = Some(50);
        }),
        Vec::<String>::new()
    );
}

#[test]
fn a_whole_slot_write_drops_the_not_just_track_n_tail() {
    // `track: None` is a mass send, where every track in the slot is going —
    // so "not just track 2" would be picking one of sixteen that are all moving
    // and implying the other fifteen are not.
    let text = impact(|a| {
        a.track = None;
        a.swing = Some(62);
        a.box_swing = Some(50);
    })
    .join("\n");
    assert!(text.contains("all 16 tracks in A02."), "{text}");
    assert!(!text.contains("not just"), "{text}");
}

#[test]
fn there_is_one_backup_line_every_confirm_ends_with() {
    // The JS says "downloads first", because a browser download *was* the backup.
    // Here the backup is a list inside the app, so the line says where — which is
    // the difference between telling someone a backup exists and telling them
    // what to do with it. The sentence exists so no dialog can imply the backup
    // is optional, and that is the part that has to be pinned.
    assert_eq!(
        BACKUP_LINE,
        "The whole destination pattern is backed up first, and can be restored from Backups."
    );
    assert!(BACKUP_LINE.contains("backed up first"));
    assert!(BACKUP_LINE.contains("restored"), "and says what can be done about it");
}

// --- the result line ----------------------------------------------------------

fn result(ok: bool, written: usize, dropped: usize, warnings: Vec<String>) -> WriteResult {
    WriteResult {
        ok,
        cancelled: false,
        diffs: Vec::new(),
        dropped,
        written,
        warnings,
        label: "A02".into(),
        index: 1,
        tracks: vec![1],
        backup: None,
        payload: None,
    }
}

#[test]
fn a_clean_write_reports_itself_in_one_line() {
    let m = write_result_message(&result(true, 5, 0, Vec::new()));
    assert_eq!(m.text, "Wrote 5 notes to A02 T2 — verified byte-identical");
    assert!(!m.is_error);
}

#[test]
fn dropped_notes_are_never_hidden() {
    assert_eq!(
        write_result_message(&result(true, 4, 1, Vec::new())).text,
        "Wrote 4 notes to A02 T2 — verified byte-identical (1 note didn't fit and was dropped)"
    );
    assert_eq!(
        write_result_message(&result(true, 4, 3, Vec::new())).text,
        "Wrote 4 notes to A02 T2 — verified byte-identical (3 notes didn't fit and were dropped)"
    );
}

#[test]
fn the_singulars_are_right() {
    assert!(write_result_message(&result(true, 1, 0, Vec::new())).text.contains("Wrote 1 note to"));
}
