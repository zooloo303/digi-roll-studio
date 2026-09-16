//! The Syntakt write path the way the app drives it — fetch, import, edit,
//! export, safe write — over the captures from the 0.5.5-beta.2 hardware test.
//!
//! On 2026-09-15 four writes went to the box through the beta, and each slot
//! was read back independently afterwards (`dumps/syntakt-2026-09-15/beta2-test/`).
//! Every write verified, and every one changed more than its edit: the note,
//! velocity and length lanes of each untouched trig that had followed its
//! track's default came back locked at that default. These tests replay the
//! same edits over the same baselines and hold the write to changing what was
//! edited and nothing else.

use std::cell::RefCell;
use std::rc::Rc;

use digi_core::device::SYNTAKT;
use digi_core::syntakt_transfer::{syntakt_pattern_to_model, syntakt_track_write};
use digi_core::{Pattern, PatternRef};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::{identity_from_responses, DeviceIdentity, DeviceResponse};
use digi_protocol::safe_write::{
    syntakt_safe_write_tracks, write_result_message, PatternIo, Timestamp, WriteHooks, WriteResult,
};
use digi_protocol::syntakt_pattern as st;

const NOW: Timestamp =
    Timestamp { year: 2026, month: 9, day: 15, hour: 12, minute: 0, second: 0 };

fn capture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../dumps/syntakt-2026-09-15/beta2-test/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// One slot's worth of box: answers a fetch with what it holds and keeps what
/// it is sent.
struct OneSlot {
    identity: DeviceIdentity,
    index: u8,
    held: Rc<RefCell<Vec<u8>>>,
}

impl PatternIo for OneSlot {
    fn identity(&self) -> Option<&DeviceIdentity> {
        Some(&self.identity)
    }

    fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
        assert_eq!(index, self.index, "the write asked for a slot it was not aimed at");
        Ok(self.held.borrow().clone())
    }

    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String> {
        assert_eq!(index, self.index, "the write sent to a slot it was not aimed at");
        *self.held.borrow_mut() = payload.to_vec();
        Ok(())
    }
}

struct Consent;
impl WriteHooks for Consent {}

fn tmp_stash(tag: &str) -> Stash {
    let dir = std::env::temp_dir()
        .join(format!("digi-roll-core-syntakt-write-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stash::at(dir)
}

fn import(slot: u8, payload: &[u8]) -> Pattern {
    syntakt_pattern_to_model(&SYNTAKT, slot, payload).expect("a beta-2 capture imports").0
}

/// Send `tracks` of `pattern` to `slot`, whose box holds `destination`, and
/// return what the box holds afterwards.
fn write_back(
    destination: &[u8],
    slot: u8,
    pattern: &Pattern,
    tracks: &[usize],
    tag: &str,
) -> (Vec<u8>, WriteResult) {
    let dev =
        DeviceResponse { product_id: 30, supported_ids: vec![0x01], reported_name: String::new() };
    let held = Rc::new(RefCell::new(destination.to_vec()));
    let mut box_ = OneSlot {
        identity: identity_from_responses(&dev, "0082".into(), "1.40".into()),
        index: slot,
        held: held.clone(),
    };
    let writes: Vec<_> = tracks
        .iter()
        .map(|&t| {
            let export = syntakt_track_write(pattern, t, PatternRef::from_slot(slot as usize))
                .expect("the track exports");
            assert!(export.warnings.is_empty(), "{:?}", export.warnings);
            export.write
        })
        .collect();
    let result =
        syntakt_safe_write_tracks(&mut box_, &tmp_stash(tag), &writes, &mut Consent, NOW)
            .expect("the write completes");
    assert!(result.ok, "verify failed: {:?}", result.diffs);
    let after = held.borrow().clone();
    (after, result)
}

/// Every offset where two payloads differ.
fn changed(a: &[u8], b: &[u8]) -> Vec<usize> {
    assert_eq!(a.len(), b.len());
    (0..a.len()).filter(|&i| a[i] != b[i]).collect()
}

/// Where a step's lane byte sits in the payload.
fn lane_at(track: usize, lane: usize, step: usize) -> usize {
    st::BLOCK_BASE + st::BLOCK_STRIDE * track + lane + step
}

fn nudge_pitch(pattern: &mut Pattern, track: usize, step: f64, by: i8) {
    let note = pattern
        .track_mut(track)
        .expect("the track exists")
        .notes
        .iter_mut()
        .find(|n| n.step == step)
        .expect("a note on that step");
    note.pitch = note.pitch.checked_add_signed(by).expect("still a MIDI note");
}

/// The floor under everything else: a track fetched and sent straight back
/// leaves the slot byte-identical. The beta-2 write path rewrote 24 lane bytes
/// doing this to A03's T2.
#[test]
fn a03_t2_written_back_untouched_is_byte_identical() {
    let a03 = capture("A03-baseline.bin");
    let (after, _) = write_back(&a03, 2, &import(2, &a03), &[1], "a03-untouched");
    assert_eq!(changed(&a03, &after), Vec::<usize>::new());
}

/// **Round 1.** One pitch on A03's T2, step 5, up a semitone. Beta 2 changed 24
/// bytes (`A03-after-write-round1.bin`); the edit is one of them.
#[test]
fn round_1_one_pitch_on_a03_changes_one_byte() {
    let a03 = capture("A03-baseline.bin");
    let mut pattern = import(2, &a03);
    nudge_pitch(&mut pattern, 1, 4.0, 1);
    let (after, _) = write_back(&a03, 2, &pattern, &[1], "round-1");

    let note = lane_at(1, st::NOTE_LANE, 4);
    assert_eq!(changed(&a03, &after), vec![note]);
    assert_eq!((a03[note], after[note]), (st::NO_LOCK, 61));
    assert_eq!(
        changed(&capture("A03-after-write-round1.bin"), &after).len(),
        23,
        "what beta 2 locked at the default, and this leaves following it"
    );
}

/// What beta 2 left on the box — seven trigs locked at 60/100/14, the track's
/// defaults — goes back as locks. A lock at the default is not a default.
#[test]
fn the_locks_beta_2_wrote_stay_locks_when_written_back() {
    let written = capture("A03-after-write-round1.bin");
    let (after, _) = write_back(&written, 2, &import(2, &written), &[1], "round-1-locks");
    assert_eq!(changed(&written, &after), Vec::<usize>::new());
}

/// **Round 2.** B01's T1 step 5, E5 to F5. That note's velocity was locked at
/// 80 and stays; its length followed the default and still does. The pool, the
/// residue on step 11 and T2 are not the write's to touch.
#[test]
fn round_2_one_pitch_on_b01_changes_one_byte() {
    let b01 = capture("B01-baseline.bin");
    let mut pattern = import(16, &b01);
    nudge_pitch(&mut pattern, 0, 4.0, 1);
    let (after, _) = write_back(&b01, 16, &pattern, &[0], "round-2");

    let note = lane_at(0, st::NOTE_LANE, 4);
    assert_eq!(changed(&b01, &after), vec![note]);
    assert_eq!((b01[note], after[note]), (64, 65));
    assert_eq!(changed(&capture("B01-after-write-round2.bin"), &after).len(), 9);
}

/// **Round 3.** A02's T12 plays eight trigs in 16 steps and stores 24 more past
/// them. One pitch moved, then the bar duplicated so eight notes sit past the
/// length. Every T12 note on the box was already locked, so beta 2 changed only
/// the edit here — and this write must land on exactly what the box stored.
#[test]
fn round_3_matches_what_the_box_stored_and_counts_the_skip_without_a_warning() {
    let a02 = capture("A02-baseline.bin");
    let mut pattern = import(1, &a02);
    nudge_pitch(&mut pattern, 11, 1.0, 1);
    let track = pattern.track_mut(11).unwrap();
    let copies: Vec<_> = track
        .notes
        .iter()
        .map(|n| {
            let mut copy = n.clone();
            copy.step += 16.0;
            copy
        })
        .collect();
    track.notes.extend(copies);
    track.length_steps = 32;

    let (after, result) = write_back(&a02, 1, &pattern, &[11], "round-3");
    assert_eq!(after, capture("A02-after-write-round3.bin"));
    assert_eq!(changed(&a02, &after), vec![lane_at(11, st::NOTE_LANE, 1)]);

    assert_eq!((result.written, result.skipped, result.dropped), (8, 8, 0));
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let message = write_result_message(&result);
    assert!(!message.is_error, "{}", message.text);
    assert!(
        message.text.ends_with("(8 notes beyond the destination length were skipped)"),
        "{}",
        message.text
    );
}

/// **Round 4.** B01 as a whole-pattern sync: T1 and T2, with T2's step 11 —
/// the note carrying the RESO lock — up a semitone. T2's lanes followed their
/// defaults; only the moved pitch locks.
#[test]
fn round_4_a_two_track_sync_changes_one_byte() {
    let b01 = capture("B01-baseline.bin");
    let mut pattern = import(16, &b01);
    nudge_pitch(&mut pattern, 1, 10.0, 1);
    let (after, _) = write_back(&b01, 16, &pattern, &[0, 1], "round-4");

    let note = lane_at(1, st::NOTE_LANE, 10);
    assert_eq!(changed(&b01, &after), vec![note]);
    assert_eq!((b01[note], after[note]), (st::NO_LOCK, 61));
    assert_eq!(changed(&capture("B01-after-write-round4.bin"), &after).len(), 17);
    assert_eq!(st::plock_lanes(&after), st::plock_lanes(&b01), "the pool, empty record and all");
}

/// B01's pool holds two records and one of them has no value in it. The panel
/// says one lane stays on the box, because one does.
#[test]
fn an_empty_pool_record_is_not_counted_as_a_lane() {
    let b01 = capture("B01-baseline.bin");
    assert_eq!(st::plock_lanes(&b01).len(), 2, "the record is there");
    let (_, report) = syntakt_pattern_to_model(&SYNTAKT, 16, &b01).unwrap();
    assert_eq!(report.plock_lanes_not_carried, 1);
}
