//! The Syntakt's safe write flow, end to end against a fake box.
//!
//! The third of these suites, after `safe_write.rs` (gen-2) and
//! `a4_safe_write.rs` (gen-1), and split from both for the reason those two are
//! split from each other: the ceremony's ordering claims are the same — backup
//! before send, always re-fetch, allowlist at send time, verify by re-read —
//! and everything inside the steps differs.
//!
//! The base payload is the box's own `0x50`, captured off its front panel on
//! 2026-09-11 and committed under `dumps/syntakt-2026-09-11/`. Building one by
//! hand would test the encoder against the encoder.
//!
//! **What no test here can claim: the box.** What hardware did claim, on
//! 2026-09-11, is narrower and is why `syntakt`/`0082` is in the allowlist at
//! all — six sends into six empty slots, each a different edit, each read back
//! and compared, with A02 untouched throughout as the control. Those sends went
//! out through `examples/syntakt_write`, not through this flow. The first send
//! through *this* path on hardware is the claim's other half.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use digi_protocol::backup_stash::Stash;
use digi_protocol::device::{identity_from_responses, DeviceIdentity, DeviceResponse};
use digi_protocol::safe_write::{
    syntakt_safe_write_tracks, ConfirmArgs, PatternIo, PatternKitFile, SyntaktStep,
    SyntaktTrackWrite, Timestamp, WriteError, WriteHooks,
};
use digi_protocol::syntakt_pattern as st;

const NOW: Timestamp =
    Timestamp { year: 2026, month: 9, day: 11, hour: 12, minute: 0, second: 0 };

/// The box's own front-panel `0x50`, unpacked. Empty of trigs, which is what
/// makes an authored one unambiguous.
fn base_payload() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../dumps/syntakt-2026-09-11/panel-send-0x50.bin");
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn syntakt_identity(build: &str) -> DeviceIdentity {
    let dev =
        DeviceResponse { product_id: 30, supported_ids: vec![0x01], reported_name: String::new() };
    identity_from_responses(&dev, build.into(), "1.40".into())
}

fn dt2_identity() -> DeviceIdentity {
    let dev =
        DeviceResponse { product_id: 42, supported_ids: vec![0x60], reported_name: String::new() };
    identity_from_responses(&dev, "0070".into(), "1.15B".into())
}

type Log = Rc<RefCell<Vec<String>>>;

struct FakeSyntakt {
    identity: Option<DeviceIdentity>,
    slots: BTreeMap<u8, Vec<u8>>,
    log: Log,
    corrupt_on_store: bool,
}

impl FakeSyntakt {
    fn new() -> Self {
        Self {
            identity: Some(syntakt_identity("0082")),
            slots: BTreeMap::from([(0, base_payload()), (1, base_payload())]),
            log: Log::default(),
            corrupt_on_store: false,
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

impl PatternIo for FakeSyntakt {
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
            stored[9_000] ^= 0x7f;
        }
        self.slots.insert(index, stored);
        Ok(())
    }
}

#[derive(Default)]
struct Recorder {
    confirms: Vec<(String, u8, Option<u8>, Vec<(usize, usize, usize)>)>,
    backups: Vec<PatternKitFile>,
    cancel: bool,
    log: Option<Log>,
}

impl WriteHooks for Recorder {
    fn on_backup(&mut self, backup: &PatternKitFile) -> Result<(), String> {
        if let Some(log) = &self.log {
            log.borrow_mut().push("backup".into());
        }
        self.backups.push(backup.clone());
        Ok(())
    }

    fn confirm(&mut self, args: &ConfirmArgs) -> bool {
        assert!(args.pattern_kit.is_none(), "this flow has no gen-2 decode to show");
        assert!(args.free_lanes.is_none(), "no pool survey is composed on this box");
        self.confirms.push((
            args.label.clone(),
            args.index,
            args.swing,
            args.tracks
                .iter()
                .map(|t| (t.track_index, t.existing_trigs, t.note_count))
                .collect(),
        ));
        !self.cancel
    }
}

fn tmp_stash(tag: &str) -> Stash {
    let dir = std::env::temp_dir()
        .join(format!("digi-roll-syntakt-safe-write-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stash::at(dir)
}

/// A full lane of 64 steps with notes on the ones named.
fn lane(steps: &[usize]) -> Vec<Option<SyntaktStep>> {
    (0..st::NUM_STEPS)
        .map(|s| {
            steps.contains(&s).then_some(SyntaktStep {
                note: 60 + s as u8,
                velocity: 100,
                length_byte: 12,
                micro_ticks: 0,
                condition_byte: st::NO_LOCK,
            })
        })
        .collect()
}

fn write_for(track: usize, steps: &[usize]) -> SyntaktTrackWrite {
    SyntaktTrackWrite { index: 0, track_index: track, steps: lane(steps), swing: None }
}

#[test]
fn a_write_lands_and_verifies_by_reading_the_slot_back() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("a_write_lands_and_verifies_by_reading_the_slot_back");
    let mut hooks = Recorder::default();
    let result =
        syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[0, 4, 8, 12])], &mut hooks, NOW)
            .expect("should write");
    assert!(result.ok, "diffs: {:?}", result.diffs);
    assert_eq!(result.written, 4);
    assert_eq!(st::trig_count(&box_.slots[&0], 0), 4);
    assert_eq!(st::track_notes(&box_.slots[&0], 0)[0].note, 60);
}

/// Rule 1. A backup that arrives after the send is not a backup.
#[test]
fn the_backup_is_taken_before_anything_is_sent() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("the_backup_is_taken_before_anything_is_sent");
    let mut hooks = Recorder { log: Some(box_.log.clone()), ..Default::default() };
    syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(2, &[0])], &mut hooks, NOW)
        .expect("should write");
    let log = box_.log();
    let backup = log.iter().position(|e| e == "backup").expect("a backup happened");
    let send = log.iter().position(|e| e.starts_with("send")).expect("a send happened");
    assert!(backup < send, "{log:?}");
    assert_eq!(log.first().map(String::as_str), Some("fetch 0"), "re-fetch first: {log:?}");
}

/// **The kit is the destination's own.** This box stores `0x50` whether the
/// caller wants a kit or not, so the one thing standing between a pattern write
/// and somebody's sounds is that nothing here composes those bytes.
#[test]
fn nothing_outside_the_pattern_region_moves() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("nothing_outside_the_pattern_region_moves");
    let before = base_payload();
    let mut hooks = Recorder::default();
    syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(3, &[1, 3, 5])], &mut hooks, NOW)
        .expect("should write");
    let after = &box_.slots[&0];
    assert_eq!(
        after[st::PATTERN_BYTES..],
        before[st::PATTERN_BYTES..],
        "the kit region must come back exactly as it was fetched"
    );
}

/// A step holding a trig that sounds no note is not on screen, so nobody can
/// have meant to delete it — the A4's trigless rule, on this box.
#[test]
fn a_trig_that_sounds_no_note_survives_a_write_to_its_track() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("a_trig_that_sounds_no_note_survives_a_write_to_its_track");
    // Step 20 of block 1: a word that is set and does not play a note.
    let word = st::BLOCK_BASE + st::TRIG_LANE + 20 * 2;
    for slot in box_.slots.values_mut() {
        slot[word] = 0x78;
        slot[word + 1] = 0x00;
    }
    assert!(!st::plays_note(&box_.slots[&0], 0, 20));
    let mut hooks = Recorder::default();
    syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[0, 1])], &mut hooks, NOW)
        .expect("should write");
    let after = &box_.slots[&0];
    assert_eq!((after[word], after[word + 1]), (0x78, 0x00), "the trigless word was cleared");
    assert_eq!(st::trig_count(after, 0), 2, "the authored notes still landed");
}

/// A note trig that *was* on screen is cleared, because deleting the note is
/// exactly what that meant.
#[test]
fn a_step_left_empty_clears_a_note_trig_that_was_there() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("a_step_left_empty_clears_a_note_trig_that_was_there");
    let mut hooks = Recorder::default();
    syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[0, 1, 2])], &mut hooks, NOW)
        .expect("first write");
    assert_eq!(st::trig_count(&box_.slots[&0], 0), 3);
    syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[1])], &mut hooks, NOW)
        .expect("second write");
    assert_eq!(st::trig_count(&box_.slots[&0], 0), 1, "the other two were not cleared");
}

#[test]
fn an_unverified_os_build_is_refused_before_the_box_is_touched() {
    let mut box_ = FakeSyntakt::new();
    box_.identity = Some(syntakt_identity("9999"));
    let stash = tmp_stash("an_unverified_os_build_is_refused_before_the_box_is_touched");
    let mut hooks = Recorder::default();
    let err = syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[0])], &mut hooks, NOW)
        .unwrap_err();
    assert!(matches!(err, WriteError::Gate(_)), "{err:?}");
    assert!(box_.log().is_empty(), "a refused write must not touch the box");
}

#[test]
fn another_boxs_pattern_is_not_edited_at_this_formats_offsets() {
    let mut box_ = FakeSyntakt::new();
    box_.identity = Some(dt2_identity());
    let stash = tmp_stash("another_boxs_pattern_is_not_edited_at_this_formats_offsets");
    let mut hooks = Recorder::default();
    let err = syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[0])], &mut hooks, NOW)
        .unwrap_err();
    assert!(matches!(err, WriteError::Gate(_)), "{err:?}");
    assert!(box_.log().is_empty());
}

#[test]
fn a_cancelled_confirm_sends_nothing_and_takes_no_backup() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("a_cancelled_confirm_sends_nothing_and_takes_no_backup");
    let mut hooks = Recorder { cancel: true, ..Default::default() };
    let result =
        syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[0])], &mut hooks, NOW)
            .expect("a cancel is not an error");
    assert!(result.cancelled);
    assert!(hooks.backups.is_empty());
    assert!(!box_.log().iter().any(|e| e.starts_with("send")));
}

#[test]
fn a_partial_lane_is_refused_rather_than_leaving_steps_nobody_decided_about() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("a_partial_lane_is_refused_rather_than_leaving_steps_nobody_decided_about");
    let mut hooks = Recorder::default();
    let short =
        SyntaktTrackWrite { index: 0, track_index: 0, steps: lane(&[0])[..16].to_vec(), swing: None };
    let err =
        syntakt_safe_write_tracks(&mut box_, &stash, &[short], &mut hooks, NOW).unwrap_err();
    assert!(err.to_string().contains("64"), "{err}");
    assert!(box_.log().is_empty());
}

#[test]
fn two_swings_for_one_pattern_is_a_caller_that_has_not_decided() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("two_swings_for_one_pattern_is_a_caller_that_has_not_decided");
    let mut hooks = Recorder::default();
    let a = SyntaktTrackWrite { swing: Some(55.0), ..write_for(0, &[0]) };
    let b = SyntaktTrackWrite { swing: Some(65.0), ..write_for(1, &[0]) };
    let err = syntakt_safe_write_tracks(&mut box_, &stash, &[a, b], &mut hooks, NOW).unwrap_err();
    assert!(err.to_string().contains("two swings"), "{err}");
    assert!(box_.log().is_empty());
}

#[test]
fn swing_is_one_byte_and_reads_back_as_the_percentage_it_was_given() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("swing_is_one_byte_and_reads_back_as_the_percentage_it_was_given");
    let mut hooks = Recorder::default();
    let write = SyntaktTrackWrite { swing: Some(65.0), ..write_for(0, &[0]) };
    syntakt_safe_write_tracks(&mut box_, &stash, &[write], &mut hooks, NOW).expect("should write");
    assert_eq!(box_.slots[&0][st::SWING], 15);
    // And the confirm showed what the destination held before, not after.
    assert_eq!(hooks.confirms[0].2, Some(50));
}

/// One write, one slot. Two indices in one call is a caller holding two writes.
#[test]
fn tracks_aimed_at_different_slots_are_refused() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("tracks_aimed_at_different_slots_are_refused");
    let mut hooks = Recorder::default();
    let other = SyntaktTrackWrite { index: 1, ..write_for(1, &[0]) };
    let err = syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[0]), other], &mut hooks, NOW)
        .unwrap_err();
    assert!(err.to_string().contains("one write, one slot"), "{err}");
    assert!(box_.log().is_empty());
}

#[test]
fn there_is_no_track_fourteen() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("there_is_no_track_fourteen");
    let mut hooks = Recorder::default();
    let err =
        syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(13, &[0])], &mut hooks, NOW)
            .unwrap_err();
    assert!(err.to_string().contains("13"), "{err}");
}

/// The verify is the only evidence a write worked, so it has to be able to fail.
#[test]
fn a_box_that_stores_something_else_fails_the_verify() {
    let mut box_ = FakeSyntakt::new();
    box_.corrupt_on_store = true;
    let stash = tmp_stash("a_box_that_stores_something_else_fails_the_verify");
    let mut hooks = Recorder::default();
    let result =
        syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(0, &[0])], &mut hooks, NOW)
            .expect("the flow completes and reports");
    assert!(!result.ok);
    assert!(!result.diffs.is_empty(), "a mismatch must be named");
}

/// A write to one slot leaves the other alone, and the confirm says which.
#[test]
fn the_other_slot_is_untouched_and_the_confirm_names_the_one_being_written() {
    let mut box_ = FakeSyntakt::new();
    let stash = tmp_stash("the_other_slot_is_untouched_and_the_confirm_names_the_one_being_written");
    let before_other = box_.slots[&1].clone();
    let mut hooks = Recorder::default();
    syntakt_safe_write_tracks(&mut box_, &stash, &[write_for(5, &[7])], &mut hooks, NOW)
        .expect("should write");
    assert_eq!(box_.slots[&1], before_other);
    assert_eq!(hooks.confirms[0].0, "A01");
    assert_eq!(hooks.confirms[0].3, vec![(5, 0, 1)], "no trigs there before, one authored");
}
