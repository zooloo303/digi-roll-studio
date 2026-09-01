//! Loading a fetched pattern into a session, against the hardware fixtures.
//!
//! This is the join `PLAN.md` §5 left open: `fetch_pattern_kit` →
//! `decode_pattern_kit` → `read_swing` reached a `PatternKit` and stopped there,
//! and nothing turned one into a `Pattern` in a slot.
//!
//! The fixtures live in `protocol/tests/fixtures` and are read from here by a
//! relative path rather than copied. They are ~100 KB each of real capture and
//! there is one repository; a second copy would be a second thing to keep true.
//! Everything expected of them was derived from the JS original first — the
//! note lists came out of `trackNotes` + `attachTrigSettings` under node,
//! against these same files.
//!
//! What the fixtures are: one pattern-kit dump each, captured 2026-08-02 with
//! PROB/FILL/COND set by hand on the first steps of track 1. The DT2 one also
//! carries a trig deleted before the capture (step 16), which is why its
//! imported track has 15 notes and 16 steps' worth of stored lane bytes.

use digi_core::device::{model_for_key, PatternRoute};
use digi_core::import::{patch_read_source, patch_read_source_named, Fetched, PatchReadError};
use digi_core::model::{PatchSound, Source, TrackPatch};
use digi_core::{two_box_session, DeviceId, ImportError, Note, PatternRef, Project, Session, TrackKind};
use digi_protocol::pattern::{
    decode_pattern_kit, dn2_spec, dt2_spec, KitInfo, PatternKit, Spec, TrackData,
};
use digi_protocol::protocol::{split_sysex_stream, SysExKind, DUMP_PATTERN_KIT};

const DT2_FIXTURE: &str = "digitakt2-A01-conditions-2026-08-02.syx";
const DN2_FIXTURE: &str = "digitone2-A01-conditions-2026-08-02.syx";
/// The Phase 0 p-lock captures, 2026-08-04: eleven knobs locked one at a time on
/// track 1, plus one on track 2 in the DT2's final pass. The condition fixtures
/// above hold an empty pool, so these are the only ones with lanes to import.
const DT2_PLOCKS: &str = "digitakt2-A01-plock-final-2026-08-04.syx";
const DN2_PLOCKS: &str = "digitone2-A01-plock-final-2026-08-04.syx";

/// The one pattern-kit payload in a single-pattern capture.
fn payload(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../protocol/tests/fixtures")
        .join(name);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("reading fixture {}: {e}", path.display()));
    let mut payloads = split_sysex_stream(&bytes)
        .into_iter()
        .filter(|m| m.kind == SysExKind::Dump)
        .filter_map(|m| m.dump)
        .inspect(|d| assert!(d.checksum_ok && d.count_ok, "{name}: a dump did not verify"))
        .filter(|d| d.dump_type == DUMP_PATTERN_KIT)
        .map(|d| d.payload);
    let first = payloads.next().unwrap_or_else(|| panic!("{name}: no pattern-kit dump"));
    assert!(payloads.next().is_none(), "{name}: expected exactly one pattern-kit dump");
    first
}

/// A session, a device of that model in it, and the fixture ready to import.
fn ready(model_key: &str, fixture: &str) -> (Session, DeviceId, Spec, Vec<u8>) {
    let session = two_box_session();
    let device = session
        .devices
        .iter()
        .find(|d| d.model.key == model_key)
        .expect("two_box_session has both boxes")
        .id;
    let spec = if model_key == "DT2" { dt2_spec() } else { dn2_spec() };
    (session, device, spec, payload(fixture))
}

/// `(step, pitch, velocity, len, prob, fill, cond)` — the fields an import is
/// answerable for. Ids are per process and micro is zero throughout these
/// captures, so neither is compared.
fn shape(n: &Note) -> (f64, u8, u8, f64, Option<u8>, Option<bool>, Option<&str>) {
    (n.step, n.pitch, n.velocity, n.len, n.prob, n.fill, n.cond.as_deref())
}

// --- the DT2 fixture ---------------------------------------------------------

#[test]
fn a_fetched_pattern_lands_in_a_slot_with_its_trig_locks_intact() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).expect("the fixture decodes");
    let report = session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .expect("a DT2 pattern into a DT2 slot");

    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    let track = pattern.track(0).unwrap();
    let got: Vec<_> = track.notes.iter().map(shape).collect();
    // Every value below came out of the JS: trackNotes() stamped with
    // attachTrigSettings(readTrackTrigSettings()), run against this fixture.
    // Step 16's trig was deleted on the box, so it is not here — its FILL and
    // PROB bytes survive in the payload and have no note to ride on.
    assert_eq!(
        got,
        vec![
            (0.0, 60, 100, 1.0, Some(0), Some(true), Some("1:4")),
            (1.0, 60, 100, 1.0, Some(5), Some(true), Some("!2:5")),
            (2.0, 60, 100, 1.0, Some(10), Some(true), Some("6:6")),
            (3.0, 60, 100, 1.0, Some(15), Some(true), Some("4:7")),
            (4.0, 60, 100, 1.0, Some(20), Some(true), Some("!8:8")),
            (5.0, 60, 100, 1.0, Some(25), Some(true), None),
            (6.0, 60, 100, 1.0, None, Some(true), Some("LST")),
            (7.0, 60, 100, 1.0, Some(35), None, Some("!LST")),
            (8.0, 60, 100, 1.0, Some(40), Some(false), Some("1:2")),
            (9.0, 60, 100, 1.0, Some(100), Some(false), Some("2:2")),
            (10.0, 60, 100, 1.0, Some(50), Some(false), Some("1:3")),
            (11.0, 60, 100, 1.0, Some(55), Some(false), Some("!1:3")),
            (12.0, 60, 100, 1.0, Some(60), Some(false), Some("2:3")),
            (13.0, 60, 100, 1.0, Some(65), Some(false), Some("!2:3")),
            (14.0, 60, 100, 1.0, Some(70), Some(false), Some("3:3")),
        ],
    );
    assert_eq!(report.notes, 15);
    assert_eq!(report.tracks_with_notes, 1);
    assert_eq!(report.trimmed_past_len, 0);
}

#[test]
fn the_fifteen_tracks_that_were_never_touched_come_in_empty() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();

    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    assert_eq!(pattern.num_tracks(), 16);
    for t in 1..16 {
        let track = pattern.track(t).unwrap();
        assert!(track.notes.is_empty(), "track {}", t + 1);
        // Length and the track PROB default still come across for a track with
        // no trigs on it — they are what the box was set to, not what it played.
        assert_eq!(track.length_steps, 16, "track {}", t + 1);
        assert_eq!(track.track_prob, 100, "track {}", t + 1);
    }
}

#[test]
fn the_imported_pattern_is_still_the_shape_its_model_demands() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    // The invariant `Device::validate` and `Project::load` both check: a slot
    // whose track count drifted would be a project file that will not open.
    assert_eq!(session.validate(), Ok(()));
}

#[test]
fn tracks_are_named_after_the_kits_sounds() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    // This project was never renamed on the box, so its sounds are still the
    // factory names — which is exactly what the box shows above each track.
    assert_eq!(pattern.track(0).unwrap().name, "PRESET 1");
    assert_eq!(pattern.track(15).unwrap().name, "PRESET 16");
    assert_eq!(pattern.track(0).unwrap().kind, TrackKind::Audio);
}

#[test]
fn a_named_track_carries_a_patch_record_naming_the_sound_and_the_kit_it_came_from() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    let patch = pattern.track(0).unwrap().patch.clone().expect("a named sound gets a patch record");
    // The record names the same sound `Track.name` was just set to — the
    // whole point is that a later rename cannot take this with it.
    assert_eq!(patch.sound, PatchSound::Named("PRESET 1".into()));
    assert_eq!(patch.kit_name, kit.kit.name);
    assert_eq!(patch.kit_index, kit.kit_index);
    assert_eq!(patch.from, pattern.source.clone().unwrap());
    assert!(patch.seen_at >= before, "seen_at should be no earlier than the import call");
}

// --- reading patch names, stage 2 (packet E, 2026-08-20) --------------------
//
// `Session::apply_patch_read` and `patch_read_source` are the pieces
// `ui::sync::read_patch_kit` and `ui::sync::patch_read_job` build on — this is
// the box-free half, tested here rather than in `app` because both functions
// are `core`'s and take a `PatternKit` a caller has already decoded, not a
// `PatternIo`.

/// The patch each of a fresh import's sixteen tracks holds, cloned so it can
/// be compared against after a refused `apply_patch_read` call — the only way
/// to prove a refusal really did leave every record exactly as it was rather
/// than merely returning `Err` while still mutating something.
fn all_patches(session: &Session, device: DeviceId, slot: PatternRef) -> Vec<Option<digi_core::model::TrackPatch>> {
    session
        .device(device)
        .unwrap()
        .pattern(slot.slot())
        .unwrap()
        .tracks()
        .iter()
        .map(|t| t.patch.clone())
        .collect()
}

#[test]
fn a_track_count_mismatch_refuses_apply_patch_read_without_touching_any_existing_record() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    let before = all_patches(&session, device, PatternRef::new(0, 0));
    assert!(before.iter().all(Option::is_some), "the import above already gave every track a record");

    // A kit with the wrong number of tracks is the one failure
    // `apply_patch_read` can hit entirely on its own, with no fetch involved —
    // exactly the plant this packet's test has to catch: a first draft that
    // clears every record before checking the track count, rather than after,
    // wipes all sixteen on a refusal instead of leaving them alone.
    let mut short_kit = kit.clone();
    short_kit.tracks.pop();
    let source = Source { device_slug: "digitakt2".into(), bank: 0, index: 0 };
    let err = session
        .apply_patch_read(device, PatternRef::new(0, 0), &short_kit, &source, 1_787_184_000)
        .unwrap_err();
    assert_eq!(err, PatchReadError::TrackCountMismatch { expected: 16, found: 15 });

    let after = all_patches(&session, device, PatternRef::new(0, 0));
    assert_eq!(before, after, "a refused read must not have touched a single existing patch record");
}

#[test]
fn apply_patch_read_touches_patch_and_nothing_else_on_the_track() {
    // Unlike a full import, a patch-names read must not rename a track — the
    // whole reason `TrackPatch` exists apart from `Track::name` is that a
    // rename survives it, and a read that overwrote the name on every press
    // would defeat that (this section's header explains the split).
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    session
        .device_mut(device)
        .unwrap()
        .pattern_mut(0)
        .unwrap()
        .track_mut(0)
        .unwrap()
        .name = "My Kick".into();

    let source = Source { device_slug: "digitakt2".into(), bank: 0, index: 0 };
    let n = session.apply_patch_read(device, PatternRef::new(0, 0), &kit, &source, 1_787_184_000).unwrap();
    assert_eq!(n, 16);

    let track = session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap();
    assert_eq!(track.name, "My Kick", "a patch-names read must not rename a track");
    assert_eq!(track.patch.as_ref().unwrap().sound, PatchSound::Named("PRESET 1".into()));
}

#[test]
fn patch_read_source_refuses_a_pattern_with_no_provenance_rather_than_guessing_a01() {
    // "Ask rather than assume" (packet E's addendum), tested at the pure
    // function `ui::sync::patch_read_job` calls to resolve which slot to
    // fetch: a pattern nobody has ever fetched carries no `Source`, and this
    // must refuse rather than default to A01 or to the slot it happens to sit
    // in.
    let session = two_box_session();
    let device = session.devices[0].id;
    let pattern = session.device(device).unwrap().pattern(0);
    assert_eq!(pattern.and_then(|p| p.source.as_ref()), None, "a fresh session's slot has no source");
    assert_eq!(
        patch_read_source(pattern, session.device(device).unwrap().model.slug),
        Err(PatchReadError::UnknownSlot)
    );
}

#[test]
fn patch_read_source_refuses_a_pattern_provenanced_to_a_different_box() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    // Hand-edit the provenance to name a box this device is not — the
    // mis-cabled-desk case one level up from the wire, which this pure
    // function refuses before anyone opens a port.
    session.device_mut(device).unwrap().pattern_mut(0).unwrap().source =
        Some(Source { device_slug: "digitone2".into(), bank: 0, index: 0 });
    let pattern = session.device(device).unwrap().pattern(0);
    let slug = session.device(device).unwrap().model.slug;
    assert_eq!(
        patch_read_source(pattern, slug),
        Err(PatchReadError::NotThisBox { pattern_slug: "digitone2".into() })
    );
}

#[test]
fn patch_read_source_resolves_the_slot_a_pattern_actually_came_from() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    // Fetched from A01 in this session but landed in A06 — a real shape
    // `ui::transfer`'s IN block allows (its `into` picker is independent of
    // `from`). `two_box_session` gives each box one bank of sixteen slots, so
    // A06 is the highest-numbered real destination available here. The slot
    // to *read* has to follow the pattern's own record, not wherever it
    // happens to be sitting in the session.
    session
        .import_pattern(
            device,
            PatternRef::new(0, 5),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    let pattern = session.device(device).unwrap().pattern(PatternRef::new(0, 5).slot());
    let slug = session.device(device).unwrap().model.slug;
    let source = patch_read_source(pattern, slug).expect("this pattern has a source");
    assert_eq!(source, Source { device_slug: "digitakt2".into(), bank: 0, index: 0 });
}

#[test]
fn patch_read_source_named_reads_a_slot_a_pattern_made_here_never_came_from() {
    // The other half of "ask rather than assume": refusing is right when
    // nothing has said which slot, and wrong as the only answer available.
    // Neil's case, 2026-08-20 — a pattern built in this app, a box on the desk
    // with sixteen named tracks, and nothing fetched yet. The slot comes from a
    // picker on screen, so it is said rather than guessed, and this function is
    // where "said" is spelled out.
    let session = two_box_session();
    let device = session.devices[0].id;
    let pattern = session.device(device).unwrap().pattern(0);
    assert_eq!(pattern.and_then(|p| p.source.as_ref()), None, "nothing here was ever fetched");
    assert_eq!(
        patch_read_source_named(pattern, "digitakt2", PatternRef::new(1, 2)),
        Ok(Source { device_slug: "digitakt2".into(), bank: 1, index: 2 })
    );
}

#[test]
fn patch_read_source_named_still_refuses_a_pattern_provenanced_to_a_different_box() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    session.device_mut(device).unwrap().pattern_mut(0).unwrap().source =
        Some(Source { device_slug: "digitone2".into(), bank: 0, index: 0 });
    let pattern = session.device(device).unwrap().pattern(0);
    // A slot number is the user's to name. Which box a pattern came off is
    // not, so naming a slot does not get past this one.
    assert_eq!(
        patch_read_source_named(pattern, "digitakt2", PatternRef::new(0, 3)),
        Err(PatchReadError::NotThisBox { pattern_slug: "digitone2".into() })
    );
}

// --- the DN2 fixture ---------------------------------------------------------

#[test]
fn the_other_box_imports_the_same_way() {
    let (mut session, device, spec, payload) = ready("DN2", DN2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    let report = session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .expect("a DN2 pattern into a DN2 slot");

    let track = session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap();
    let got: Vec<_> = track.notes.iter().map(shape).collect();
    assert_eq!(
        got,
        vec![
            (0.0, 60, 100, 1.0, None, None, Some("PRE")),
            (1.0, 60, 100, 1.0, None, None, Some("!8:8")),
            (2.0, 60, 100, 1.0, None, None, Some("2:4")),
            (3.0, 60, 100, 1.0, None, None, Some("!2:4")),
            (4.0, 60, 100, 1.0, Some(45), None, None),
            (5.0, 60, 100, 1.0, None, Some(true), None),
            (6.0, 60, 100, 1.0, None, Some(false), None),
            // The plain control trig: a live step with nothing locked on it.
            (7.0, 60, 100, 1.0, None, None, None),
        ],
    );
    assert_eq!(report.notes, 8);
}

// --- what an import must not disturb -----------------------------------------

#[test]
fn an_import_keeps_the_slots_own_routing() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();

    // A desk that was already working: this slot's tracks have been routed,
    // muted and soloed by hand. None of that is in a pattern dump.
    {
        let pattern = session.device_mut(device).unwrap().pattern_mut(0).unwrap();
        let t0 = pattern.track_mut(0).unwrap();
        t0.channel = 9;
        t0.mute = true;
        t0.out_port = Some("IAC Bus 1".to_owned());
        pattern.track_mut(3).unwrap().solo = true;
    }

    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();

    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    let t0 = pattern.track(0).unwrap();
    assert_eq!(t0.channel, 9);
    assert!(t0.mute);
    assert_eq!(t0.out_port.as_deref(), Some("IAC Bus 1"));
    assert!(pattern.track(3).unwrap().solo);
    // …and the notes did arrive, so this is not passing by doing nothing.
    assert_eq!(t0.notes.len(), 15);
}

#[test]
fn an_import_never_takes_the_boxs_tempo() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session.tempo_bpm = 132.0;
    let report = session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    // One clock, the studio's (PLAN.md §7 rule 8). The box's tempo is reported
    // so a caller can offer it, and applied by nothing.
    assert_eq!(session.tempo_bpm, 132.0);
    assert_eq!(report.box_tempo_bpm, 120.0);
}

#[test]
fn only_the_slot_that_was_imported_into_changes() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    let before = session.clone();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 5),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    for slot in 0..16 {
        let now = session.device(device).unwrap().pattern(slot).unwrap();
        let then = before.device(device).unwrap().pattern(slot).unwrap();
        assert_eq!(now == then, slot != 5, "slot {slot}");
    }
    // And the other box in the session is untouched.
    let dn2 = session.devices.iter().find(|d| d.model.key == "DN2").unwrap().id;
    assert_eq!(session.device(dn2), before.device(dn2));
}

// --- provenance ---------------------------------------------------------------

#[test]
fn the_pattern_remembers_the_slot_it_came_off_the_box_from() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    // Fetched from A01, landed in B03: the source is where it came *from*, which
    // is what a write-back has to aim at.
    session
        .import_pattern(
            device,
            PatternRef::new(0, 2),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    let source = session
        .device(device)
        .unwrap()
        .pattern(2)
        .unwrap()
        .source
        .clone()
        .expect("an imported pattern knows where it came from");
    assert_eq!(source.device_slug, "digitakt2");
    assert_eq!((source.bank, source.index), (0, 0));
}

#[test]
fn an_unnamed_pattern_takes_the_label_of_the_slot_it_came_from() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    // Nothing in this fixture was ever named on the box, so the dump carries an
    // empty pattern name — which must not become an empty name in the UI.
    assert_eq!(kit.name, "");
    let report = session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 3) },
        )
        .unwrap();
    assert_eq!(report.pattern_name, "");
    assert_eq!(session.device(device).unwrap().pattern(0).unwrap().name, "A04");
}

#[test]
fn swing_comes_across_because_the_box_holds_it_per_pattern() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    let report = session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    // This capture is straight; `protocol`'s swing suite is where the values
    // are pinned. What matters here is that the byte reached the model at all —
    // it is the last of the three things `PLAN.md` §5 said were connected to
    // nothing.
    assert_eq!(report.swing, 50);
    assert_eq!(session.device(device).unwrap().pattern(0).unwrap().swing, 50);
}

// --- refusals -----------------------------------------------------------------

#[test]
fn refuses_a_pattern_fetched_from_the_other_box() {
    // A DN2 pattern, aimed at the DT2 that is sitting in the same session.
    let (mut session, _, spec, payload) = ready("DN2", DN2_FIXTURE);
    let dt2 = session.devices.iter().find(|d| d.model.key == "DT2").unwrap().id;
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    // The two boxes' lane offsets differ. Reading a DN2 payload at DT2 offsets
    // would produce plausible-looking nonsense rather than an error, so this is
    // refused up front. Moving a track between boxes is cross-device copy,
    // which is a different feature and not this one.
    let err = session.import_pattern(
        dt2,
        PatternRef::new(0, 0),
        &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
    );
    assert_eq!(err, Err(ImportError::NotThisBox { expected: "DT2", found: "DN2" }));
}

#[test]
fn refuses_a_device_or_a_slot_that_is_not_there() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    let fetched =
        Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) };

    let ghost = DeviceId(9999);
    assert_eq!(
        session.import_pattern(ghost, PatternRef::new(0, 0), &fetched),
        Err(ImportError::NoSuchDevice(ghost)),
    );
    // The session gives each box one bank; B01 is past the end of it.
    let past_the_end = PatternRef::new(1, 0);
    assert_eq!(
        session.import_pattern(device, past_the_end, &fetched),
        Err(ImportError::NoSuchSlot { device, slot: past_the_end }),
    );
}

// --- the cases no capture has ------------------------------------------------

/// A hand-built kit, for the rules the fixtures cannot exercise. Its payload is
/// empty on purpose: every lane read then takes the "nothing stored" answer a
/// truncated payload gets, which is a path worth walking too.
fn synthetic_kit(spec: &Spec, length_steps: u16, steps_with_trigs: &[usize]) -> PatternKit {
    let tracks = (0..spec.pattern.num_tracks)
        .map(|_| {
            let mut steps = vec![0u16; spec.track.num_steps];
            for &s in steps_with_trigs {
                steps[s] = 1;
            }
            TrackData {
                steps,
                sound_p_locks: vec![0; spec.track.num_steps],
                default_note: 60,
                default_velocity: 100,
                // Length byte 14 is one step — the bottom of the octave scale.
                default_length: 14,
                length_steps,
                trigs: Default::default(),
            }
        })
        .collect();
    PatternKit {
        version: 3,
        name: "SYNTH".into(),
        tempo_bpm: 90.0,
        kit_index: 0,
        tracks,
        kit: KitInfo {
            version: 3,
            name: "KIT".into(),
            sound_names: vec![String::new(); spec.pattern.num_tracks],
            midi_mask: 0,
        },
    }
}

#[test]
fn trigs_stored_past_the_tracks_own_len_are_dropped() {
    let (mut session, device, spec, _) = ready("DT2", DT2_FIXTURE);
    // The box stores a trig beyond LEN and never plays it: raising LEN brings it
    // back, which is why it is in the dump at all. An import takes what plays.
    let kit = synthetic_kit(&spec, 16, &[0, 15, 16, 40]);
    let report = session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &[], from: PatternRef::new(0, 0) },
        )
        .unwrap();
    let track = session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap();
    assert_eq!(track.notes.iter().map(|n| n.step).collect::<Vec<_>>(), vec![0.0, 15.0]);
    // Sixteen tracks, two dropped trigs each.
    assert_eq!(report.trimmed_past_len, 32);
    assert_eq!(report.notes, 32);
    assert_eq!(report.tracks_with_notes, 16);
}

#[test]
fn an_unnamed_sound_leaves_the_tracks_default_label_alone() {
    let (mut session, device, spec, _) = ready("DT2", DT2_FIXTURE);
    let kit = synthetic_kit(&spec, 16, &[0]);
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &[], from: PatternRef::new(0, 0) },
        )
        .unwrap();
    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    assert_eq!(pattern.track(0).unwrap().name, "T1");
    // A named pattern keeps its own name rather than the slot label.
    assert_eq!(pattern.name, "SYNTH");
    // The track's own *label* is untouched — an unnamed kit slot has nothing
    // useful to rename it to. But it was still fetched, and packet E's
    // addendum is explicit that a fetched track with nothing to say still gets
    // a record saying exactly that, rather than reading as "never fetched"
    // (which is what `patch: None` claims). `PatchSound::Unnamed` is the
    // third shape, distinct from a named sound and from a MIDI track.
    let patch = pattern.track(0).unwrap().patch.clone().expect("a fetched track always gets a record");
    assert_eq!(patch.sound, digi_core::model::PatchSound::Unnamed);
    assert_eq!(patch.kit_name, kit.kit.name);
}

#[test]
fn renaming_a_track_does_not_disturb_its_patch_record() {
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    let before = session
        .device(device)
        .unwrap()
        .pattern(0)
        .unwrap()
        .track(0)
        .unwrap()
        .patch
        .clone()
        .expect("imported with a named sound");

    let d = session.device_mut(device).unwrap();
    let pattern = d.pattern_mut(0).unwrap();
    pattern.track_mut(0).unwrap().name = "My Kick".into();

    let track = session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap();
    assert_eq!(track.name, "My Kick");
    // The rename touched `name` and nothing else — `patch` is what a rename
    // cannot erase, which is the entire reason it exists apart from `name`.
    assert_eq!(track.patch, Some(before));
}

#[test]
fn a_kit_with_the_wrong_track_count_is_refused_rather_than_half_loaded() {
    let (mut session, device, spec, _) = ready("DT2", DT2_FIXTURE);
    let mut kit = synthetic_kit(&spec, 16, &[0]);
    kit.tracks.truncate(4);
    assert_eq!(
        session.import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &[], from: PatternRef::new(0, 0) },
        ),
        Err(ImportError::TrackCountMismatch { expected: 16, found: 4 }),
    );
    // Nothing landed: the slot is still the empty one the session started with.
    assert!(session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap().notes.is_empty());
}

#[test]
fn a_live_only_model_has_nothing_to_import_into() {
    // Constructed here rather than shipped: the point is that the refusal is a
    // property of `sysex: None`, not of any box this build knows.
    static LIVE_ONLY: digi_core::DeviceModel = digi_core::DeviceModel {
        key: "LIVE",
        display: "A live-only box",
        slug: None,
        num_tracks: 4,
        max_steps: 64,
        default_track_kind: TrackKind::Audio,
        sysex: None,
        pattern_route: PatternRoute::LiveOnly,
        wire_slots: 0,
    };
    let spec = dt2_spec();
    let kit = synthetic_kit(&spec, 16, &[0]);
    let err = digi_core::pattern_from_kit(
        &LIVE_ONLY,
        &Fetched { spec: &spec, kit: &kit, payload: &[], from: PatternRef::new(0, 0) },
    )
    .expect_err("a model with no spec cannot be imported into");
    assert_eq!(err, ImportError::LiveOnly(&LIVE_ONLY));
    assert!(model_for_key("LIVE").is_none(), "and it is not in the shipped table");
}

// --- p-lock lanes ------------------------------------------------------------

/// `(name, param_id, trigless, [(step, display value)])` — what a lane is once
/// the import has converted it off the wire.
fn lane_shape(
    l: &digi_core::model::PLockLane,
) -> (Option<&str>, Option<u16>, bool, Vec<(usize, u16)>) {
    let held = l
        .values
        .iter()
        .enumerate()
        .filter_map(|(s, v)| v.map(|v| (s, v)))
        .collect();
    (l.name.as_deref(), l.param_id, l.trigless, held)
}

#[test]
fn an_imported_pattern_arrives_with_its_lanes_named_and_on_the_display_axis() {
    // Every value here came out of the JS: `devicePLocksToRoll(readTrackPLocks(
    // SPEC, payload, 0), 'DT2', liveSteps)` against this fixture. The stored
    // words are ×256 of these — 0x4002 comes back as 64, which is the box's
    // sub-MIDI fine resolution rounding onto this app's integer axis.
    let (mut session, device, spec, payload) = ready("DT2", DT2_PLOCKS);
    let kit = decode_pattern_kit(&spec, &payload).expect("a DT2 capture decodes");
    let report = session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .expect("a DT2 pattern into a DT2 slot");

    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    let got: Vec<_> = pattern.track(0).unwrap().plocks.iter().map(lane_shape).collect();
    assert_eq!(
        got,
        [
            (Some("filter.cutoff"), Some(44), false, vec![(0, 0), (4, 64), (8, 127)]),
            (Some("amp.pan"), Some(65), false, vec![(4, 0)]),
            (Some("filter.envDepth"), Some(46), false, vec![(8, 32)]),
            (Some("fx.overdrive"), Some(74), false, vec![(12, 127)]),
            (Some("fx.delaySend"), Some(63), false, vec![(0, 127)]),
            (Some("fx.reverbSend"), Some(64), false, vec![(4, 64)]),
            (Some("fx.chorusSend"), Some(62), false, vec![(8, 32)]),
            (Some("lfo1.depth"), Some(29), false, vec![(12, 72)]),
            (Some("lfo2.depth"), Some(30), false, vec![(0, 72)]),
            (Some("lfo3.depth"), Some(31), false, vec![(4, 72)]),
        ]
    );

    // Track 2's single lane belongs to track 2 and nowhere else — one pool,
    // sixteen tracks, and the import has to sort them out.
    assert_eq!(
        pattern.track(1).unwrap().plocks.iter().map(lane_shape).collect::<Vec<_>>(),
        [(Some("filter.cutoff"), Some(44), false, vec![(0, 100)])]
    );
    for t in 2..16 {
        assert!(pattern.track(t).unwrap().plocks.is_empty(), "track {}", t + 1);
    }

    assert_eq!(report.plock_lanes, 11);
    assert_eq!(report.unnamed_plock_lanes, 0);
}

#[test]
fn every_lane_remembers_which_box_it_came_off() {
    // The DN2 numbers the same knobs differently — cutoff is 74 here and 44 on
    // the DT2, where 74 is overdrive. A lane that forgot this would be
    // auditioned on the wrong parameter, so the audition path refuses a lane
    // whose `device_kind` is not the destination's, and this is where the field
    // gets set.
    let (mut session, device, spec, payload) = ready("DN2", DN2_PLOCKS);
    let kit = decode_pattern_kit(&spec, &payload).expect("a DN2 capture decodes");
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .expect("a DN2 pattern into a DN2 slot");

    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    let lanes = &pattern.track(0).unwrap().plocks;
    assert_eq!(lanes.len(), 10);
    assert!(lanes.iter().all(|l| l.device_kind.as_deref() == Some("DN2")));
    assert_eq!(
        lane_shape(&lanes[0]),
        (Some("filter.cutoff"), Some(74), false, vec![(0, 0), (4, 64), (8, 127), (12, 32)])
    );
}

#[test]
fn a_pattern_with_an_empty_pool_imports_no_lanes_and_says_so() {
    // The ordinary case, and the one every earlier import test ran under: the
    // 2026-08-02 captures predate the p-lock experiments entirely.
    let (mut session, device, spec, payload) = ready("DT2", DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &payload).expect("a DT2 capture decodes");
    let report = session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &payload, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    assert_eq!(report.plock_lanes, 0);
    assert_eq!(report.unnamed_plock_lanes, 0);
    let pattern = session.device(device).unwrap().pattern(0).unwrap();
    assert!((0..16).all(|t| pattern.track(t).unwrap().plocks.is_empty()));
}

#[test]
fn importing_over_a_slot_replaces_its_lanes_rather_than_adding_to_them() {
    // An import replaces a slot wholesale — the transfer panel says so in amber
    // before the button is pressed — and lanes are part of "wholesale". Importing
    // the lane-bearing capture and then the empty one must leave no lanes.
    let (mut session, device, spec, with_lanes) = ready("DT2", DT2_PLOCKS);
    let kit = decode_pattern_kit(&spec, &with_lanes).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &with_lanes, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    assert!(!session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap().plocks.is_empty());

    let empty = payload(DT2_FIXTURE);
    let kit = decode_pattern_kit(&spec, &empty).unwrap();
    session
        .import_pattern(
            device,
            PatternRef::new(0, 0),
            &Fetched { spec: &spec, kit: &kit, payload: &empty, from: PatternRef::new(0, 0) },
        )
        .unwrap();
    assert!(session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap().plocks.is_empty());
}

// --- Track.patch round-trips through the project file -------------------------

fn a_patch() -> TrackPatch {
    TrackPatch {
        sound: PatchSound::Named("BD HARD".into()),
        kit_name: "KIT 1".into(),
        kit_index: 7,
        from: Source { device_slug: "digitakt2".into(), bank: 1, index: 5 },
        seen_at: 1_787_184_000, // 2026-08-20T00:00:00Z — not the default of nothing
    }
}

#[test]
fn a_track_with_a_patch_round_trips_through_the_project_file() {
    let mut session = two_box_session();
    let device = session.devices[0].id;
    session
        .device_mut(device)
        .unwrap()
        .pattern_mut(0)
        .unwrap()
        .track_mut(0)
        .unwrap()
        .patch = Some(a_patch());

    let json = Project::new(session.clone()).to_json_pretty().unwrap();
    let back = Project::from_json(&json).unwrap().session;

    assert_eq!(back, session);
    let round_tripped = back.device(device).unwrap().pattern(0).unwrap().track(0).unwrap().patch.clone();
    assert_eq!(round_tripped, Some(a_patch()));
}

#[test]
fn a_track_without_a_patch_round_trips_through_the_project_file() {
    let session = two_box_session();
    let device = session.devices[0].id;
    assert_eq!(
        session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap().patch,
        None,
        "a fresh session's tracks start with no patch record"
    );

    let json = Project::new(session.clone()).to_json_pretty().unwrap();
    let back = Project::from_json(&json).unwrap().session;
    assert_eq!(back.device(device).unwrap().pattern(0).unwrap().track(0).unwrap().patch, None);
}

#[test]
fn a_project_file_written_before_the_patch_field_existed_still_loads() {
    let mut session = two_box_session();
    let device = session.devices[0].id;
    session
        .device_mut(device)
        .unwrap()
        .pattern_mut(0)
        .unwrap()
        .track_mut(0)
        .unwrap()
        .patch = Some(a_patch());

    let json = Project::new(session.clone()).to_json_pretty().unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();

    // Strip every "patch" key from every track, in every pattern of every
    // device — a file with no key at all, not a file holding `"patch":null`.
    // That is what a project written before this change actually looks like
    // (DEVELOPMENT.md's lesson 2: a fixture that only ever holds `null` never
    // exercises the missing-key path at all).
    //
    // **The plant for this test cannot fail, and that is worth saying rather
    // than faking.** Removing `#[serde(default)]` from `Track.patch` was
    // planted on 2026-08-20 and this test still passed: serde's derive treats
    // a missing `Option<T>` field as `None` on its own, with or without the
    // attribute. So the attribute is inert here and is kept only for
    // consistency with every other `Option<T>` in the model. What this test
    // does pin is the thing that *can* break — that an old file still loads
    // at all, which a future non-`Option` field or a `deny_unknown_fields`
    // would take out.
    let mut stripped = 0usize;
    for device in v["session"]["devices"].as_array_mut().unwrap() {
        for pattern in device["patterns"].as_array_mut().unwrap() {
            for track in pattern["tracks"].as_array_mut().unwrap() {
                if track.as_object_mut().unwrap().remove("patch").is_some() {
                    stripped += 1;
                }
            }
        }
    }
    assert!(stripped > 0, "the fixture must actually have had a patch key to strip");

    let old_style_json = serde_json::to_string(&v).unwrap();
    let loaded = Project::from_json(&old_style_json)
        .expect("a project file with no patch key at all must still load");
    let track = loaded.session.device(device).unwrap().pattern(0).unwrap().track(0).unwrap();
    assert_eq!(track.patch, None, "with no patch key in the file, the track must load with no patch");
}
