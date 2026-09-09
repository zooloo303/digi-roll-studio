//! September OS captures, read directly from the three updated devices.
//! These tests cover decoding and preservation; hardware write/readback evidence
//! is recorded separately in PLAN.md.
use crate::common::{fixture_bytes, payload};
use digi_protocol::a4_kit::parse_kit;
use digi_protocol::a4_pattern::{parse_pattern, read_track_trigs};
use digi_protocol::drive::decode_drive_preset;
use digi_protocol::device::{identity_from_responses, DeviceResponse};
use digi_protocol::pattern::{decode_pattern_kit, dn2_spec, dt2_spec, encode_track_notes, track_notes, Note};
use digi_protocol::safe_write::write_gate;

const DIR: &str = "firmware-2026-09-09";
const DN2: &str = "digitone2-1.11-0059";
const DT2: &str = "digitakt2-1.16-0079";
const A4: &str = "analogfour-1.55D-0201";

#[test]
fn dn2_v4_blank_pattern_keeps_the_v3_layout() {
    let old = payload("dn2-fresh-A01.syx");
    let new = payload(&format!("{DIR}/{DN2}-A16.syx"));
    // Independently compared raw captures: exactly three pattern bytes differ:
    // struct version, a pre-existing setting, and the slot's kit index.
    let differences: Vec<_> = (0..89088).filter(|&i| old[i] != new[i]).collect();
    assert_eq!(differences, [3, 88812, 88816]);
    let kit = decode_pattern_kit(&dn2_spec(), &new).unwrap();
    assert_eq!((kit.version, kit.kit.version), (4, 4));
    assert_eq!(kit.tempo_bpm, 120.0);
    assert_eq!(kit.kit_index, 15);
    assert!(kit.tracks.iter().all(|t| t.trigs.is_empty()));
    assert_eq!(kit.kit.sound_names, (1..=16).map(|i| format!("PRESET {i}")).collect::<Vec<_>>());
}

#[test]
fn new_digi_formats_encode_notes_without_changing_kits_or_other_tracks() {
    for (prefix, spec) in [(DT2, dt2_spec()), (DN2, dn2_spec())] {
        let before = payload(&format!("{DIR}/{prefix}-A01.syx"));
        let decoded = decode_pattern_kit(&spec, &before).unwrap();
        assert_eq!((decoded.version, decoded.kit.version), (4, 4));
        assert_eq!(decoded.kit.name, "DIGI-ROLL!");
        let notes = vec![
            Note { step: 0, pitch: 60, velocity: 100, len_steps: 1.0, micro: 0.0 },
            Note { step: 0, pitch: 64, velocity: 90, len_steps: 2.0, micro: 0.0 },
            Note { step: 4, pitch: 67, velocity: 80, len_steps: 1.0, micro: 1.0 / 24.0 },
        ];
        let (after, dropped) = encode_track_notes(&spec, &before, 0, &notes).unwrap();
        assert_eq!(dropped, 0);
        let reread = decode_pattern_kit(&spec, &after).unwrap();
        assert_eq!(track_notes(&reread, 0), notes);
        assert_eq!(&after[spec.pattern.size..], &before[spec.pattern.size..], "{prefix}: preserve the entire kit, including unmodelled routing settings");
        for track in 1..16 {
            assert_eq!(track_notes(&reread, track), track_notes(&decoded, track));
            let start = spec.pattern.tracks_offset + track * spec.track.size;
            assert_eq!(&after[start..start + spec.track.size], &before[start..start + spec.track.size]);
        }
        assert_eq!(&after[spec.pattern.p_locks_index..], &before[spec.pattern.p_locks_index..]);
    }
}

#[test]
fn a4_updated_pattern_and_kit_still_decode() {
    for slot in [1, 16] {
        let p = parse_pattern(&fixture_bytes(&format!("{DIR}/{A4}-A{slot:02}.syx"))).unwrap();
        assert_eq!(p.slot, slot - 1);
        assert_eq!(p.payload.len(), 12974);
        for track in 0..6 { read_track_trigs(&p.payload, track).unwrap(); }
    }
    let kit = parse_kit(&fixture_bytes(&format!("{DIR}/{A4}-kit-0.syx"))).unwrap();
    assert_eq!(kit.name, "ARPOLYGY");
}

#[test]
fn updated_devices_presets_still_decode() {
    for prefix in [DT2, DN2, A4] {
        let bytes = fixture_bytes(&format!("{DIR}/{prefix}-sound-A-1.bin"));
        decode_drive_preset(&bytes).unwrap_or_else(|e| panic!("{prefix}: {e}"));
    }
}

#[test]
fn future_dn2_struct_versions_remain_rejected() {
    let original = payload(&format!("{DIR}/{DN2}-A01.syx"));
    for offset in [3, 89095] {
        let mut changed = original.clone();
        changed[offset] = 5;
        assert!(decode_pattern_kit(&dn2_spec(), &changed).unwrap_err().contains("version 5"));
    }
}

#[test]
fn verified_builds_are_allowed_and_unknown_builds_stay_blocked() {
    for (product_id, version, builds) in [
        (42, "1.16", vec!["0070", "0071", "0079"]),
        (43, "1.11", vec!["0049", "0050", "0059"]),
        (4, "1.55D", vec!["0195", "0201"]),
    ] {
        let response = DeviceResponse { product_id, supported_ids: vec![], reported_name: String::new() };
        for build in builds {
            let id = identity_from_responses(&response, build.into(), version.into());
            assert!(write_gate(Some(&id)).ok, "{id:?}");
        }
        let unknown = identity_from_responses(&response, "9999".into(), version.into());
        assert!(!write_gate(Some(&unknown)).ok);
    }
}

#[test]
fn digi_hardware_readbacks_contain_the_requested_notes_conditions_and_plocks() {
    use digi_protocol::plocks::read_track_plocks;
    use digi_protocol::trig_cond::{read_track_trig_settings, TrigSetting};
    for (prefix, spec, param_id) in [(DT2, dt2_spec(), 44), (DN2, dn2_spec(), 74)] {
        let bytes = payload(&format!("{DIR}/{prefix}-A16-readback.syx"));
        let p = decode_pattern_kit(&spec, &bytes).unwrap();
        let notes = track_notes(&p, 0);
        assert_eq!(notes.len(), 3);
        assert_eq!(notes.iter().map(|n| (n.step, n.pitch, n.velocity)).collect::<Vec<_>>(), [(0, 60, 100), (0, 64, 90), (4, 67, 80)]);
        assert_eq!(notes.iter().map(|n| n.len_steps).collect::<Vec<_>>(), [1.0, 2.0, 1.0]);
        assert_eq!(notes[2].micro, 1.0 / 24.0);
        let settings = read_track_trig_settings(&spec, &bytes, 0).unwrap();
        for step in [0, 4] {
            assert_eq!(settings[&step], TrigSetting { prob: Some(75), fill: Some(false), cond: Some("2:4") });
        }
        assert_eq!(bytes[spec.pattern.tracks_offset + spec.track.track_prob], 80);
        let lanes = read_track_plocks(&spec, &bytes, 0).unwrap();
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].param_id, param_id);
        assert_eq!(lanes[0].values[0], Some(64 << 8));
        assert!(lanes[0].values[1..].iter().all(Option::is_none));
    }
}

#[test]
fn a4_hardware_readback_contains_the_requested_chord_timing_and_plock() {
    let p = parse_pattern(&fixture_bytes(&format!("{DIR}/{A4}-A16-readback.syx"))).unwrap();
    let trigs = read_track_trigs(&p.payload, 0).unwrap();
    assert_eq!(trigs.len(), 2);
    assert_eq!(trigs[0].note, Some(60));
    assert_eq!(trigs[0].arp_notes, [Some(4), Some(7), None]);
    assert_eq!(trigs[1].note, Some(67));
    assert_eq!(trigs[1].micro_timing, 1);
    for trig in &trigs {
        assert_eq!(trig.velocity, Some(100));
        assert_eq!(trig.length, Some(14));
        assert_eq!(trig.condition, Some(0));
    }
    let lanes = digi_protocol::a4_plocks::read_track_plocks(&p.payload, 0).unwrap();
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0].param_id, 0x22);
    assert_eq!(lanes[0].word(0), Some(64 << 8));
}
