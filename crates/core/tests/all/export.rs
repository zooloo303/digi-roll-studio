//! A track of this session, written back to the box it came from.
//!
//! `import.rs` proves a capture becomes a `Pattern`; this proves the `Pattern`
//! becomes the capture again. Between them is the claim the whole write path
//! rests on and that no unit test can make: **an import nobody edited, written
//! back, is the pattern the box already had.** Anything `core::export` drops,
//! reorders or rescales shows up here as a pattern that came home different.
//!
//! These drive the *real* [`safe_write_track`] rather than re-applying its steps,
//! against a fake box that keeps its slots in a map — the pattern
//! `protocol/tests/all/safe_write.rs` established. Two reasons: the apply order is
//! part of what is being tested (conditions after the encode, lanes after that),
//! and a test that copied the order would keep passing after the real one
//! changed.
//!
//! **Nothing here can reach hardware.** The box is a `BTreeMap` and the stash is
//! a temp directory.
//!
//! The fixtures are `protocol/tests/fixtures`, read by relative path rather than
//! copied — same bargain as `import.rs`, and the same reason: one repository, one
//! copy of a ~100 KB capture.

use std::collections::BTreeMap;

use digi_core::device::model_for_key;
use digi_core::export::track_write;
use digi_core::import::Fetched;
use digi_core::{two_box_session, DeviceId, PatternRef, Session};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::{identity_from_responses, DeviceIdentity, DeviceResponse};
use digi_protocol::pattern::{decode_pattern_kit, track_notes, Note, Spec};
use digi_protocol::pattern_settings::read_swing;
use digi_protocol::plocks::read_track_plocks;
use digi_protocol::protocol::{split_sysex_stream, SysExKind, DUMP_PATTERN_KIT};
use digi_protocol::safe_write::{safe_write_track, PatternIo, Timestamp, WriteHooks, WriteResult};
use digi_protocol::trig_cond::{
    apply_track_prob, read_track_prob, read_track_trig_settings, TrigSetting,
};

/// 15 trigs on track 1, every one carrying PROB/FILL/COND, an empty lane pool —
/// and the leftovers of a trig deleted on the box at step 16.
const DT2_CONDITIONS: &str = "digitakt2-A01-conditions-2026-08-02.syx";
/// The Phase 0 capture: ten p-lock lanes on track 1 and one on track 2.
const DT2_PLOCKS: &str = "digitakt2-A01-plock-final-2026-08-04.syx";
/// A DN2 pattern that is not straight — swing 65.
const DN2_SWUNG: &str = "dn2-swing-65.syx";
/// A second DN2 pattern, swing 78, so a write's swing has something to overwrite.
const DN2_FRESH: &str = "dn2-fresh-A01.syx";

/// 2026-08-01 12:34:56 UTC, the instant the JS suite stamps its backups with.
const NOW: Timestamp =
    Timestamp { year: 2026, month: 8, day: 1, hour: 12, minute: 34, second: 56 };

/// The one pattern-kit payload in a single-pattern capture.
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
        .inspect(|d| assert!(d.checksum_ok && d.count_ok, "{name}: a dump did not verify"))
        .find(|d| d.dump_type == DUMP_PATTERN_KIT)
        .map(|d| d.payload)
        .unwrap_or_else(|| panic!("{name}: no pattern-kit dump"))
}

fn identity(product_id: u8, build: &str, version: &str) -> DeviceIdentity {
    let dev = DeviceResponse {
        product_id,
        supported_ids: vec![0x60],
        reported_name: String::new(),
    };
    identity_from_responses(&dev, build.into(), version.into())
}

/// A box that keeps its slots in a map. Its builds are the two on the write
/// allowlist, because a gate refusal is `safe_write.rs`'s test and not this
/// file's.
struct FakeBox {
    identity: DeviceIdentity,
    slots: BTreeMap<u8, Vec<u8>>,
}

impl FakeBox {
    fn dt2(slots: &[(u8, &str)]) -> Self {
        Self {
            identity: identity(42, "0070", "1.15B"),
            slots: slots.iter().map(|(i, f)| (*i, payload(f))).collect(),
        }
    }

    fn dn2(slots: &[(u8, &str)]) -> Self {
        Self {
            identity: identity(43, "0049", "1.10D"),
            slots: slots.iter().map(|(i, f)| (*i, payload(f))).collect(),
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
        self.slots.get(&index).cloned().ok_or_else(|| format!("no slot {index}"))
    }

    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String> {
        self.slots.insert(index, payload.to_vec());
        Ok(())
    }
}

/// Consents, keeps the warnings, and does nothing else.
#[derive(Default)]
struct Silent;
impl WriteHooks for Silent {}

fn tmp_stash(tag: &str) -> Stash {
    let dir = std::env::temp_dir()
        .join(format!("digi-roll-core-export-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stash::at(dir)
}

/// A session with one box of `model_key` in it, holding `fixture` in slot A01.
fn imported(model_key: &str, fixture: &str) -> (Session, DeviceId, &'static Spec, Vec<u8>) {
    let mut session = two_box_session();
    let device = session
        .devices
        .iter()
        .find(|d| d.model.key == model_key)
        .expect("two_box_session has both boxes")
        .id;
    let spec = model_for_key(model_key).and_then(|m| m.spec()).expect("a box with a spec");
    let bytes = payload(fixture);
    let kit = decode_pattern_kit(spec, &bytes).expect("the fixture decodes");
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec, kit: &kit, payload: &bytes, from: PatternRef::new(0, 0) },
        )
        .expect("a capture into a slot of its own model");
    (session, device, spec, bytes)
}

/// Export A01's track and write it to `into` on the box. Returns the result and
/// the export's own warnings, which are the ones a confirm dialog would show.
fn write_back(
    session: &Session,
    device: DeviceId,
    spec: &Spec,
    track: usize,
    into: PatternRef,
    box_: &mut FakeBox,
    tag: &str,
) -> (WriteResult, Vec<String>) {
    let export = session
        .track_write(spec, device, PatternRef::new(0, 0), track, into)
        .expect("a track of a slot this session has");
    let result = safe_write_track(box_, &tmp_stash(tag), &export.write, &mut Silent, NOW)
        .expect("the fake box is on the allowlist and its stash is writable");
    (result, export.warnings)
}

/// Notes sorted the way the encoder groups them, so a reordered pool record is
/// not read as a changed pattern.
fn by_pitch(notes: &[Note]) -> Vec<(u8, u8, u8, f64, f64)> {
    let mut v: Vec<_> =
        notes.iter().map(|n| (n.step, n.pitch, n.velocity, n.len_steps, n.micro)).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    v
}

/// `(lane index, paramId, [(step, stored word)])` — a track's automation as the
/// pool holds it.
fn lanes(spec: &Spec, payload: &[u8], track: usize) -> Vec<(usize, u8, Vec<(usize, u16)>)> {
    read_track_plocks(spec, payload, track)
        .expect("a track the spec has")
        .iter()
        .map(|l| {
            let held = l
                .values
                .iter()
                .enumerate()
                .filter_map(|(s, v)| v.map(|w| (s, w)))
                .collect();
            (l.lane, l.param_id, held)
        })
        .collect()
}

// --- the round trip -----------------------------------------------------------

#[test]
fn an_unedited_import_writes_back_as_the_pattern_the_box_had() {
    // The claim the write path rests on. Everything `export` does — the step
    // rounding, the condition lookup, the pairing of settings to notes — is
    // wrong if this fails, and unit tests would not notice: they check the
    // conversion against itself, and this checks it against a real capture.
    let (session, device, spec, before) = imported("DT2", DT2_CONDITIONS);
    let mut box_ = FakeBox::dt2(&[(0, DT2_CONDITIONS)]);
    let (result, warnings) =
        write_back(&session, device, spec, 0, PatternRef::new(0, 0), &mut box_, "unedited");

    assert!(result.ok, "verify failed: {:?}", result.diffs.first());
    assert_eq!(result.dropped, 0);
    assert_eq!(result.written, 15);
    assert!(warnings.is_empty(), "{warnings:?}");

    let after = box_.slot(0);
    let kit_before = decode_pattern_kit(spec, &before).unwrap();
    let kit_after = decode_pattern_kit(spec, after).unwrap();
    assert_eq!(by_pitch(&track_notes(&kit_after, 0)), by_pitch(&track_notes(&kit_before, 0)));

    // The trig lanes come home too — they are read from the payload rather than
    // the decoded kit, so a write that landed the notes and lost the conditions
    // would pass the line above.
    let live: Vec<u8> = track_notes(&kit_before, 0).iter().map(|n| n.step).collect();
    let settings_before = read_track_trig_settings(spec, &before, 0).unwrap();
    let settings_after = read_track_trig_settings(spec, after, 0).unwrap();
    for step in &live {
        assert_eq!(
            settings_after.get(step),
            settings_before.get(step),
            "step {} came home different",
            step + 1
        );
    }
    assert_eq!(read_track_prob(spec, after, 0), read_track_prob(spec, &before, 0));
    assert_eq!(read_swing(spec, after), read_swing(spec, &before));
    assert_eq!(kit_after.kit.name, kit_before.kit.name);
    assert_eq!(kit_after.tempo_bpm, kit_before.tempo_bpm);
}

#[test]
fn the_write_scrubs_the_leftovers_of_a_trig_deleted_on_the_box() {
    // The one place an unedited round trip *must* differ from the capture, and
    // the reason `apply_track_trig_settings` scrubs before it writes. Step 16 of
    // this fixture holds FILL and PROB bytes belonging to a trig deleted on the
    // hardware; the import dropped them with the trig, so the write must too —
    // otherwise the next trig drawn on step 16 inherits a dead one's probability.
    let (session, device, spec, before) = imported("DT2", DT2_CONDITIONS);
    let leftover = read_track_trig_settings(spec, &before, 0).unwrap();
    assert_eq!(
        leftover.get(&15),
        Some(&TrigSetting { prob: Some(75), fill: Some(false), cond: None }),
        "the fixture is supposed to carry a deleted trig's leftovers at step 16",
    );

    let mut box_ = FakeBox::dt2(&[(0, DT2_CONDITIONS)]);
    write_back(&session, device, spec, 0, PatternRef::new(0, 0), &mut box_, "scrub");

    let after = read_track_trig_settings(spec, box_.slot(0), 0).unwrap();
    assert_eq!(after.get(&15), None, "the deleted trig's leftovers survived the write");
}

#[test]
fn writing_one_track_leaves_the_other_fifteen_exactly_as_they_were() {
    // A one-track write that disturbs its neighbours is the failure nothing else
    // in this file would catch: the notes it was asked for would all be correct.
    let (session, device, spec, before) = imported("DT2", DT2_PLOCKS);
    let mut box_ = FakeBox::dt2(&[(0, DT2_PLOCKS)]);
    write_back(&session, device, spec, 0, PatternRef::new(0, 0), &mut box_, "neighbours");
    let after = box_.slot(0);

    let kit_before = decode_pattern_kit(spec, &before).unwrap();
    let kit_after = decode_pattern_kit(spec, after).unwrap();
    for track in 1..16 {
        assert_eq!(
            by_pitch(&track_notes(&kit_after, track)),
            by_pitch(&track_notes(&kit_before, track)),
            "track {} changed",
            track + 1
        );
        assert_eq!(
            read_track_trig_settings(spec, after, track).unwrap(),
            read_track_trig_settings(spec, &before, track).unwrap(),
            "track {}'s trig lanes changed",
            track + 1
        );
        assert_eq!(
            read_track_prob(spec, after, track),
            read_track_prob(spec, &before, track),
            "track {}'s PROB default changed",
            track + 1
        );
        // Track 2 owns one of this capture's eleven lanes, in the pool the
        // written track shares with it.
        assert_eq!(
            lanes(spec, after, track),
            lanes(spec, &before, track),
            "track {}'s p-lock lanes changed",
            track + 1
        );
    }
}

// --- p-lock lanes -------------------------------------------------------------

#[test]
fn every_lane_comes_home_in_the_lane_the_box_put_it_in() {
    // Ten lanes off real hardware, through the model, and back. `param_id` and
    // the lane *index* both have to survive: `apply_track_plocks` rewrites a
    // lane where the box left it precisely so a write moves as few bytes as
    // possible, and a lane that changed index would still play the same and
    // would mean the export had renamed its parameters.
    let (session, device, spec, before) = imported("DT2", DT2_PLOCKS);
    let mut box_ = FakeBox::dt2(&[(0, DT2_PLOCKS)]);
    let (result, warnings) =
        write_back(&session, device, spec, 0, PatternRef::new(0, 0), &mut box_, "lanes");
    assert!(result.ok);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert!(warnings.is_empty(), "{warnings:?}");

    let before_lanes = lanes(spec, &before, 0);
    let after_lanes = lanes(spec, box_.slot(0), 0);
    assert_eq!(before_lanes.len(), 10);
    assert_eq!(
        after_lanes.iter().map(|(l, id, _)| (*l, *id)).collect::<Vec<_>>(),
        before_lanes.iter().map(|(l, id, _)| (*l, *id)).collect::<Vec<_>>(),
    );
    // The steps holding locks are the same steps, lane for lane.
    for (a, b) in after_lanes.iter().zip(&before_lanes) {
        assert_eq!(
            a.2.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            b.2.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            "lane {} holds different steps",
            a.0
        );
    }
}

#[test]
fn a_lane_loses_the_box_s_sub_midi_fine_resolution_and_nothing_else() {
    // The one lossy step in the round trip, and it is a documented one
    // (`ParamDesc::display_from_stored`): a curated lane is carried on this app's
    // integer MIDI axis, so a word the box stored *between* two MIDI values comes
    // back quantised. This test exists to keep that loss exactly this size — a
    // whole-value change would mean the scaling drifted.
    let (session, device, spec, before) = imported("DT2", DT2_PLOCKS);
    let mut box_ = FakeBox::dt2(&[(0, DT2_PLOCKS)]);
    write_back(&session, device, spec, 0, PatternRef::new(0, 0), &mut box_, "fine");

    let mut moved = Vec::new();
    for ((_, id, after), (_, _, before)) in
        lanes(spec, box_.slot(0), 0).into_iter().zip(lanes(spec, &before, 0))
    {
        for ((step, a), (_, b)) in after.into_iter().zip(before) {
            if a != b {
                moved.push((id, step, b, a));
            }
        }
    }
    // Four of the twenty-one locks in this capture were stored *between* two
    // MIDI values — cutoff's 0x4002 and the three LFO depths' 0x4801 — and each
    // loses only those low bits. Every other word survives untouched, which is
    // the part that matters: the loss is the axis, not the scaling.
    assert_eq!(
        moved,
        vec![
            (44, 4, 0x4002, 0x4000),
            (29, 12, 0x4801, 0x4800),
            (30, 0, 0x4801, 0x4800),
            (31, 4, 0x4801, 0x4800),
        ]
    );
    assert!(
        moved.iter().all(|(_, _, b, a)| b / 256 == a / 256),
        "a lane's MIDI value changed, not just its fine bits: {moved:?}",
    );
}

// --- swing --------------------------------------------------------------------

#[test]
fn the_patterns_swing_travels_with_the_write_and_replaces_the_destinations() {
    // Swing is one byte per *pattern*, so this is the only thing a track write
    // changes that reaches the other fifteen tracks. It goes because the JS sends
    // it and because a pattern whose feel did not travel is not the pattern that
    // was sent — and it is the reason the confirm dialog has to name the number
    // it is replacing.
    let (session, device, spec, _) = imported("DN2", DN2_SWUNG);
    let mut box_ = FakeBox::dn2(&[(0, DN2_SWUNG), (1, DN2_FRESH)]);
    assert_eq!(read_swing(spec, box_.slot(1)), 78, "the destination starts elsewhere");

    let (result, _) =
        write_back(&session, device, spec, 0, PatternRef::new(0, 1), &mut box_, "swing");
    assert!(result.ok);
    assert_eq!(read_swing(spec, box_.slot(1)), 65);
    // …and the slot it came from was not touched, because a write names one slot.
    assert_eq!(read_swing(spec, box_.slot(0)), 65);
}

// --- the track's own PROB default ---------------------------------------------

#[test]
fn a_tracks_prob_default_travels_rather_than_leaving_the_destinations() {
    // **Every committed fixture is at PROB 100 on every track**, so a write that
    // dropped this byte would be invisible to all of them — the escape class this
    // repo has now hit four times, and the reason the witness below is seeded
    // rather than looked for. The destination starts at 40 and the pattern being
    // written says 55; a write that leaves the byte alone reads back as 40.
    let (mut session, device, spec, _) = imported("DT2", DT2_CONDITIONS);
    session
        .device_mut(device)
        .unwrap()
        .pattern_mut(0)
        .unwrap()
        .track_mut(0)
        .unwrap()
        .track_prob = 55;

    let mut box_ = FakeBox::dt2(&[(0, DT2_CONDITIONS), (1, DT2_PLOCKS)]);
    let seeded = box_.slots.get_mut(&1).unwrap();
    apply_track_prob(spec, seeded, 0, Some(40)).unwrap();
    apply_track_prob(spec, seeded, 1, Some(40)).unwrap();

    let (result, _) =
        write_back(&session, device, spec, 0, PatternRef::new(0, 1), &mut box_, "track-prob");
    assert!(result.ok);
    assert_eq!(read_track_prob(spec, box_.slot(1), 0), Ok(55));
    // One track, one byte: the neighbour keeps the value the destination had.
    assert_eq!(read_track_prob(spec, box_.slot(1), 1), Ok(40));
}

// --- what the model can hold and the wire cannot -------------------------------

#[test]
fn a_note_the_wire_cannot_carry_is_reported_before_the_write_rather_than_dropped_in_it() {
    // Two different losses, counted in two different places, and both have to
    // reach the person: a step past the pattern's own 128 is the *encoder's*
    // drop, and one past what a byte can name never gets that far.
    let (mut session, device, spec, _) = imported("DT2", DT2_CONDITIONS);
    {
        let track = session
            .device_mut(device)
            .unwrap()
            .pattern_mut(0)
            .unwrap()
            .track_mut(0)
            .unwrap();
        track.notes.push(digi_core::Note::new(200.0, 60, 1.0, 100, 0.0));
        track.notes.push(digi_core::Note::new(400.0, 60, 1.0, 100, 0.0));
    }

    let export = track_write(
        spec,
        session.device(device).unwrap().pattern(0).unwrap(),
        0,
        PatternRef::new(0, 0),
    )
    .unwrap();
    assert_eq!(export.write.notes.len(), 16, "step 400 never became a note");
    assert_eq!(export.warnings.len(), 1);
    assert!(export.warnings[0].contains("outside the 0–255 steps"), "{}", export.warnings[0]);

    let mut box_ = FakeBox::dt2(&[(0, DT2_CONDITIONS)]);
    let result = safe_write_track(&mut box_, &tmp_stash("off-the-end"), &export.write, &mut Silent, NOW)
        .expect("the write itself is fine");
    assert!(result.ok);
    assert_eq!(result.dropped, 1, "step 200 is past the pattern's 128 and the encoder said so");
    assert_eq!(result.written, 15);
}
