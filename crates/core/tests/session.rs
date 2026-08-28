// Phase 2's exit criteria, as tests.
//
// A session holding a 16-track DT2 pattern and a 16-track DN2 pattern round-trips
// through the project file; a scene switch selects the right slot on both; the
// track-count invariant is enforced; and a 4-track, no-SysEx model constructs
// correctly, which is what proves the device table is data rather than a shape
// baked into the model.

use digi_core::chords::{ChordSettings, Harmony, Quality, QualityChoice, Scale};
use digi_core::device::{model_for_slug, Device, DeviceIo, DeviceModel, PortRef, DN2, DT2};
use digi_core::model::{Note, PLockLane, Source, TrackKind, TrackScale, PLOCK_STEPS};
use digi_core::project::Project;
use digi_core::session::{PatternRef, Scene, Session};
use digi_core::{two_box_session, BindError, PortEnd, ProjectError};
use digi_protocol::device::DeviceIdentity;

fn dt2_and_dn2() -> Session {
    two_box_session()
}

// ------------------------------------------------------------- the shape

#[test]
fn a_session_holds_several_boxes_of_sixteen_tracks_each() {
    let s = dt2_and_dn2();
    assert_eq!(s.devices.len(), 2);
    let total: usize = s
        .devices
        .iter()
        .map(|d| d.pattern(0).unwrap().num_tracks())
        .sum();
    // The target case from PLAN.md §2: 16 tracks each, 32 in a session.
    assert_eq!(total, 32);
    assert_eq!(s.devices[0].model.key, "DT2");
    assert_eq!(s.devices[1].model.key, "DN2");
}

#[test]
fn track_count_comes_from_the_model_not_a_constant() {
    for d in &dt2_and_dn2().devices {
        for p in &d.patterns {
            assert_eq!(p.num_tracks(), d.model.num_tracks);
        }
    }
}

#[test]
fn an_unshipped_live_only_model_constructs_correctly() {
    // Deliberately not shipped in MODELS: this proves the table is data without
    // claiming a Syntakt profile we have not verified. This test's model was
    // an "Analog Four" from Phase 2 until 2026-08-24, when the real A4 row
    // graduated into the shipped table — the Syntakt takes over as the next
    // box the plan names and this build does not ship.
    static SYNTAKT: DeviceModel = DeviceModel {
        key: "ST",
        display: "Syntakt",
        slug: None,
        num_tracks: 12,
        max_steps: 64,
        default_track_kind: TrackKind::Audio,
        sysex: None,
        answers_identity: false,
    };

    let d = Device::new("ST", &SYNTAKT, 8);
    assert_eq!(d.patterns.len(), 8);
    for p in &d.patterns {
        assert_eq!(p.num_tracks(), 12);
    }
    // sysex: None means sequence-live-only — no fetch, no write.
    assert!(!d.can_sysex());
    assert!(SYNTAKT.spec().is_none());
    d.validate().expect("a 12-track model is coherent");
}

#[test]
fn the_shipped_a4_is_live_only_and_six_tracks() {
    // The first `sysex: None` row to ship. Six tracks — four voices, FX, CV —
    // and 64 steps, both off the box's own manual; nothing here is a guess a
    // dump has to verify, because no dump is ever read for it.
    assert_eq!(digi_core::A4.num_tracks, 6);
    assert_eq!(digi_core::A4.max_steps, 64);
    assert!(!digi_core::A4.can_sysex());
    assert!(digi_core::A4.spec().is_none());
    // Corrected 2026-08-28 against the box: it answers 0x01 on the first try,
    // product id 4, OS 1.55B. `sysex: None` survives that correction for its
    // own reason — the same reply lists no `0x6x` dump request at all.
    assert!(digi_core::A4.answers_identity, "the mk1 answers 0x01 — verified on hardware");
    let d = Device::new("A4", &digi_core::A4, 16);
    d.validate().expect("the shipped A4 model is coherent");
}

#[test]
fn the_shipped_models_can_do_sysex_and_say_so_truthfully() {
    // Guards the trap of a model whose `sysex` field disagrees with reality.
    // The A4 is deliberately absent: its truth is the test above.
    for m in [&DT2, &DN2] {
        assert!(m.can_sysex(), "{} should do SysEx", m.key);
        assert!(m.spec().is_some(), "{} should resolve a Spec", m.key);
    }
}

#[test]
fn an_identity_reply_binds_to_the_right_model() {
    // The slug is the link between protocol's wire identity and core's musical
    // table — the binding Phase 3 could not do without a Session. The A4
    // reaches it from a real handshake like the others, since 2026-08-28.
    assert_eq!(model_for_slug("digitakt2").unwrap().key, "DT2");
    assert_eq!(model_for_slug("digitone2").unwrap().key, "DN2");
    assert_eq!(model_for_slug("analogfour").unwrap().key, "A4");
    assert!(model_for_slug("microfreak").is_none());
}

// ------------------------------------------------------------ round trip

/// The round-trip fixture, seeded so that `assert_eq!(back, s)` can actually
/// fail.
///
/// Equality only witnesses fields the fixture *sets*, and every field seeded
/// below is `#[serde(default)]` — so a bug that dropped a p-lock lane, a trig
/// condition or a track's mute passed this test untouched for five phases. The
/// rule is that each one is set to something that is **not** its default, which
/// for `takes_clock` means `false`: its default is `true`, so a box that takes
/// clock cannot witness the field going missing.
fn seeded_session() -> Session {
    let mut s = two_box_session();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);

    s.name = "Seeded".into();
    s.tempo_bpm = 138.5;

    // --- the DT2: a note carrying all three trig-condition fields, and a lane.
    {
        let d = s.device_mut(dt2).unwrap();
        d.io.build = Some("0070".into());
        d.io.version = Some("1.15B".into());
        d.io.input = Some(PortRef {
            id: "in-dt2".into(),
            name: "Digitakt II MIDI In".into(),
        });
        d.io.output = Some(PortRef {
            id: "out-dt2".into(),
            name: "Digitakt II MIDI Out".into(),
        });
        d.io.takes_clock = false;

        let p = d.pattern_mut(0).unwrap();
        p.name = "Kick".into();
        p.swing = 58;
        p.source = Some(Source {
            device_slug: "digitakt2".into(),
            bank: 0,
            index: 0,
        });

        let t = p.track_mut(3).unwrap();
        t.name = "BD".into();
        t.length_steps = 12; // polymeter against the DN2 track below
        t.scale = TrackScale::ThreeHalves;
        t.track_prob = 80;
        t.kind = TrackKind::Midi;
        t.out_port = Some("out-dt2".into());
        t.channel = 4;
        t.mute = true;

        let mut n = Note::new(4.0, 60, 1.5, 100, 0.25);
        n.prob = Some(60);
        n.fill = Some(true);
        n.cond = Some("1:2".into());
        t.notes.push(n);

        // `paramId` 44 is filter cutoff on a DT2 and 74 is overdrive, so a lane
        // carries its own box's numbering — which is what `device_kind` is for.
        let mut values = vec![None; PLOCK_STEPS];
        values[4] = Some(96);
        values[7] = Some(12);
        t.plocks.push(
            PLockLane::new(
                Some("filter.cutoff".into()),
                Some(44),
                Some("DT2".into()),
                false,
                values,
            )
            .unwrap(),
        );
    }

    // --- the DN2: the same fields at different values, and a trigless lane.
    {
        let d = s.device_mut(dn2).unwrap();
        d.io.build = Some("0049".into());
        d.io.version = Some("1.10D".into());

        let p = d.pattern_mut(2).unwrap();
        p.name = "Pad".into();
        p.swing = 65;
        p.source = Some(Source {
            device_slug: "digitone2".into(),
            bank: 0,
            index: 2,
        });

        let t = p.track_mut(11).unwrap();
        t.length_steps = 32;
        t.scale = TrackScale::Half;
        t.track_prob = 55;
        t.channel = 12;
        t.solo = true;

        let mut n = Note::new(2.5, 67, 0.125, 42, -0.5);
        n.prob = Some(25);
        n.cond = Some("PRE".into());
        t.notes.push(n);

        // Trigless: the box held a value on a step with no trig. v1 does not
        // model that, so the lane is passed through untouched — which it can
        // only be if the flag survives the file.
        let mut values = vec![None; PLOCK_STEPS];
        values[0] = Some(64);
        t.plocks.push(
            PLockLane::new(
                Some("filter.cutoff".into()),
                Some(74),
                Some("DN2".into()),
                true,
                values,
            )
            .unwrap(),
        );
    }

    // --- the key and the chord settings, every field off its default.
    //
    // **All six of the chord fields, and both key fields, set to something that is
    // not what `Default` gives.** These are `#[serde(default)]`, and the measured
    // lesson from this very test is that a defaulted field left at its default is
    // a field whose disappearance the round-trip assertion cannot see. `strum` and
    // `inversion` are the sharp ones: `0` is both their default and a perfectly
    // ordinary value, so only a fixture that moves them witnesses them at all.
    s.harmony = Harmony {
        root: 7, // G
        scale: Some(Scale::Dorian),
        chord: ChordSettings {
            on: true,
            quality: QualityChoice::Fixed(Quality::Sus4),
            seventh: true,
            inversion: 2,
            spread: true,
            strum: 3,
        },
    };

    // --- two scenes, each box on a different slot, and not sitting on the first.
    let verse = s.add_scene("Verse", None);
    assert!(s.set_slot_in_scene(verse, dt2, PatternRef::new(0, 4)));
    assert!(s.set_slot_in_scene(verse, dn2, PatternRef::new(0, 9)));
    s.current_scene = verse;

    s
}

#[test]
fn a_session_round_trips_through_the_project_file() {
    let s = seeded_session();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);

    let json = Project::new(s.clone()).to_json_pretty().unwrap();
    let back = Project::from_json(&json).unwrap().session;

    assert_eq!(back, s);

    // Named one at a time as well, because a whole-`Session` `assert_eq!` says
    // "these differ" without saying which field went missing.
    assert_eq!(back.name, "Seeded");
    assert_eq!(back.tempo_bpm, 138.5);

    let dt2_track = back
        .device(dt2)
        .unwrap()
        .pattern(0)
        .unwrap()
        .track(3)
        .unwrap();
    assert_eq!(dt2_track.name, "BD");
    assert_eq!(dt2_track.length_steps, 12);
    assert_eq!(dt2_track.scale, TrackScale::ThreeHalves);
    assert_eq!(dt2_track.track_prob, 80);
    assert_eq!(dt2_track.kind, TrackKind::Midi);
    assert_eq!(dt2_track.out_port.as_deref(), Some("out-dt2"));
    assert_eq!(dt2_track.channel, 4);
    assert!(dt2_track.mute);

    let n = &dt2_track.notes[0];
    assert_eq!(
        (n.step, n.pitch, n.len, n.velocity, n.micro),
        (4.0, 60, 1.5, 100, 0.25)
    );
    assert_eq!(n.prob, Some(60));
    assert_eq!(n.fill, Some(true));
    assert_eq!(n.cond.as_deref(), Some("1:2"));

    let lane = &dt2_track.plocks[0];
    assert_eq!(lane.name.as_deref(), Some("filter.cutoff"));
    assert_eq!(lane.param_id, Some(44));
    assert_eq!(lane.device_kind.as_deref(), Some("DT2"));
    assert!(!lane.trigless);
    // Every step, not just the two that hold a value: a lane that came back
    // shorter would still compare equal on the steps that were set.
    assert_eq!(lane.values.len(), PLOCK_STEPS);
    assert_eq!(lane.values[4], Some(96));
    assert_eq!(lane.values[7], Some(12));
    assert_eq!(lane.values[5], None);

    let dn2_pattern = back.device(dn2).unwrap().pattern(2).unwrap();
    assert_eq!(dn2_pattern.swing, 65);
    let dn2_track = dn2_pattern.track(11).unwrap();
    assert_eq!(dn2_track.scale, TrackScale::Half);
    assert!(dn2_track.solo);
    assert!(dn2_track.plocks[0].trigless);
    assert_eq!(dn2_track.plocks[0].param_id, Some(74));
    let n = &dn2_track.notes[0];
    assert_eq!(
        (n.step, n.pitch, n.len, n.velocity, n.micro),
        (2.5, 67, 0.125, 42, -0.5)
    );
    assert_eq!(n.cond.as_deref(), Some("PRE"));
    // Left unset on this note on purpose: `None` has to survive as `None`.
    assert_eq!(n.fill, None);

    // A box that does not take the session's clock must not come back taking
    // it. `takes_clock` defaults to `true`, so this is the one field whose
    // disappearance is silent in both directions.
    assert!(!back.device(dt2).unwrap().io.takes_clock);
    assert!(back.device(dn2).unwrap().io.takes_clock);
    assert_eq!(back.device(dt2).unwrap().io.build.as_deref(), Some("0070"));
    assert_eq!(back.device(dn2).unwrap().io.version.as_deref(), Some("1.10D"));
    assert_eq!(
        back.device(dt2).unwrap().io.output.as_ref().unwrap().name,
        "Digitakt II MIDI Out"
    );

    // The key and the chord settings, field by field rather than as one struct:
    // the whole point of setting each of them off its default is being told which
    // one went missing.
    assert_eq!(back.harmony.root, 7);
    assert_eq!(back.harmony.scale, Some(Scale::Dorian));
    assert!(back.harmony.chord.on);
    assert_eq!(back.harmony.chord.quality, QualityChoice::Fixed(Quality::Sus4));
    assert!(back.harmony.chord.seventh);
    assert_eq!(back.harmony.chord.inversion, 2);
    assert!(back.harmony.chord.spread);
    assert_eq!(back.harmony.chord.strum, 3);
    assert_eq!(
        back.device(dt2)
            .unwrap()
            .pattern(0)
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .device_slug,
        "digitakt2"
    );

    // Two scenes, each box re-slotted, and the session not sitting on the first.
    assert_eq!(back.scenes.len(), 2);
    assert_eq!(back.current_scene, 1);
    assert_eq!(back.scenes[1].name, "Verse");
    assert_eq!(back.slot_in_scene(1, dt2), Some(PatternRef::new(0, 4)));
    assert_eq!(back.slot_in_scene(1, dn2), Some(PatternRef::new(0, 9)));
    // Scene 1 still holds what `add_device` gave it.
    assert_eq!(back.slot_in_scene(0, dt2), Some(PatternRef::new(0, 0)));
}

#[test]
fn saving_an_unchanged_project_twice_produces_identical_bytes() {
    // The determinism lesson from Phase 1, applied to scenes: a HashMap here
    // would make a project file's bytes depend on the run.
    let s = dt2_and_dn2();
    let a = Project::new(s.clone()).to_json().unwrap();
    let b = Project::new(s).to_json().unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_session_reopened_and_saved_again_is_byte_identical() {
    // Phase 8's exit criterion, and a stronger claim than saving the same
    // in-memory session twice: this one goes through the parser, so a field
    // that survives equality but is re-serialised differently — a padded lane,
    // a reordered scene map — shows up as a diff in the bytes.
    let first = Project::new(seeded_session()).to_json_pretty().unwrap();
    let reopened = Project::from_json(&first).unwrap();
    let second = reopened.to_json_pretty().unwrap();
    assert_eq!(first, second);
}

#[test]
fn the_session_tempo_is_never_part_of_a_pattern() {
    // PLAN.md §7 rule 8. The DT2/DN2 pattern struct has a tempo field and this
    // model deliberately does not mirror it, so minimal diff leaves those bytes
    // alone. If a tempo ever appears inside a pattern, that rule is broken.
    let json = Project::new(dt2_and_dn2()).to_json().unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v["session"]["tempoBpm"].is_number());

    let pattern = &v["session"]["devices"][0]["patterns"][0];
    assert!(pattern.is_object());
    for key in pattern.as_object().unwrap().keys() {
        assert!(
            !key.to_lowercase().contains("tempo"),
            "a pattern must not carry tempo, found {key:?}"
        );
    }
    // Swing, by contrast, genuinely is a per-pattern byte on the box.
    assert!(pattern["swing"].is_number());
}

#[test]
fn a_project_from_a_newer_build_is_refused_rather_than_misread() {
    let json = Project::new(dt2_and_dn2()).to_json().unwrap();
    let bumped = json.replacen("\"format\":1", "\"format\":99", 1);
    assert!(matches!(
        Project::from_json(&bumped),
        Err(ProjectError::FromTheFuture { found: 99, .. })
    ));
}

#[test]
fn an_unknown_device_model_is_refused_rather_than_defaulted() {
    let json = Project::new(dt2_and_dn2()).to_json().unwrap();
    let swapped = json.replacen("\"model\":\"DT2\"", "\"model\":\"OctatrackIII\"", 1);
    // Must not quietly become a DT2.
    assert!(Project::from_json(&swapped).is_err());
}

// ------------------------------------------------------- the invariant

#[test]
fn a_pattern_whose_track_count_disagrees_with_its_model_is_refused() {
    let json = Project::new(dt2_and_dn2()).to_json().unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();

    // Hand-edit a DT2 pattern down to four tracks, as a corrupted or
    // hand-tweaked file would.
    let tracks = v["session"]["devices"][0]["patterns"][0]["tracks"]
        .as_array()
        .unwrap()
        .clone();
    v["session"]["devices"][0]["patterns"][0]["tracks"] = serde_json::Value::Array(tracks[..4].to_vec());

    let err = Project::from_json(&v.to_string()).unwrap_err();
    // Rejected, not repaired: padding would invent tracks, truncating would
    // throw notes away.
    assert!(
        matches!(err, ProjectError::Model(_)),
        "expected a model error, got {err:?}"
    );
    assert!(err.to_string().contains("16 tracks"), "{err}");
}

#[test]
fn the_track_vec_cannot_be_resized_through_the_public_api() {
    // `tracks` is private and `track_mut` hands back one track, so there is no
    // push, truncate or swap to reach. This test exists to fail loudly if that
    // field is ever made public again.
    let mut s = dt2_and_dn2();
    let d = s.devices[0].id;
    let p = s.device_mut(d).unwrap().pattern_mut(0).unwrap();
    let before = p.num_tracks();
    p.track_mut(0).unwrap().notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    assert_eq!(p.num_tracks(), before);
    s.validate().expect("editing a track must not disturb the invariant");
}

// ----------------------------------------------------------------- scenes

#[test]
fn a_scene_names_one_slot_per_box_and_switching_selects_both() {
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);

    // Name the slots so we can tell which one got selected.
    s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().name = "DT2 verse".into();
    s.device_mut(dt2).unwrap().pattern_mut(5).unwrap().name = "DT2 chorus".into();
    s.device_mut(dn2).unwrap().pattern_mut(0).unwrap().name = "DN2 verse".into();
    s.device_mut(dn2).unwrap().pattern_mut(9).unwrap().name = "DN2 chorus".into();

    s.scenes = vec![
        Scene::new("Verse")
            .with_slot(dt2, PatternRef::new(0, 0))
            .with_slot(dn2, PatternRef::new(0, 0)),
        Scene::new("Chorus")
            .with_slot(dt2, PatternRef::new(0, 5))
            .with_slot(dn2, PatternRef::new(0, 9)),
    ];

    s.current_scene = 0;
    assert_eq!(s.current_pattern(dt2).unwrap().name, "DT2 verse");
    assert_eq!(s.current_pattern(dn2).unwrap().name, "DN2 verse");

    // One switch moves both boxes.
    s.current_scene = 1;
    assert_eq!(s.current_pattern(dt2).unwrap().name, "DT2 chorus");
    assert_eq!(s.current_pattern(dn2).unwrap().name, "DN2 chorus");
}

#[test]
fn slots_are_addressed_the_way_the_box_addresses_them() {
    assert_eq!(PatternRef::new(0, 0).slot(), 0);
    assert_eq!(PatternRef::new(0, 0).label(), "A01");
    assert_eq!(PatternRef::new(1, 3).slot(), 19);
    assert_eq!(PatternRef::new(1, 3).label(), "B04");
    assert_eq!(PatternRef::from_slot(19), PatternRef::new(1, 3));
}

#[test]
fn a_pattern_label_parses_back_to_the_slot_it_names() {
    assert_eq!(PatternRef::from_label("A01"), Some(PatternRef::new(0, 0)));
    assert_eq!(PatternRef::from_label("A03"), Some(PatternRef::new(0, 2)));
    assert_eq!(PatternRef::from_label("A16"), Some(PatternRef::new(0, 15)));
    assert_eq!(PatternRef::from_label("B01"), Some(PatternRef::new(1, 0)));
    assert_eq!(PatternRef::from_label("a01"), Some(PatternRef::new(0, 0)));

    // Every slot either box has round-trips through its own label. This is the
    // property that matters — the two directions cannot drift apart.
    for bank in 0..16u8 {
        for index in 0..16u8 {
            let r = PatternRef::new(bank, index);
            assert_eq!(PatternRef::from_label(&r.label()), Some(r), "{}", r.label());
        }
    }

    // A mistyped slot refuses rather than becoming A01: the boxes number
    // patterns from 1, so "A00" is not a slot, and "A17" is past the bank.
    assert_eq!(PatternRef::from_label("A00"), None);
    assert_eq!(PatternRef::from_label("A17"), None);
    assert_eq!(PatternRef::from_label("01"), None);
    assert_eq!(PatternRef::from_label("A"), None);
    assert_eq!(PatternRef::from_label(""), None);
    assert_eq!(PatternRef::from_label("A1x"), None);
}

#[test]
fn every_slot_either_box_has_fits_the_one_byte_a_dump_request_carries() {
    // A01 is 0 and P16 is 255: the whole of both boxes' pattern space fits the
    // wire exactly, with nothing to spare.
    assert_eq!(PatternRef::new(0, 0).wire_index(), Some(0));
    assert_eq!(PatternRef::new(0, 2).wire_index(), Some(2));
    assert_eq!(PatternRef::new(15, 15).wire_index(), Some(255));
    for bank in 0..16u8 {
        for index in 0..16u8 {
            let r = PatternRef::new(bank, index);
            assert_eq!(r.wire_index(), Some(r.slot() as u8), "{}", r.label());
        }
    }

    // Past P16 there is no byte to send, so this refuses rather than wrapping
    // round and fetching a different pattern than the one asked for.
    assert_eq!(PatternRef::new(16, 0).wire_index(), None);
    assert_eq!(PatternRef::from_slot(256).wire_index(), None);
}

#[test]
fn adding_a_device_gives_every_existing_scene_a_slot_for_it() {
    let mut s = Session {
        scenes: vec![Scene::new("One"), Scene::new("Two")],
        ..Session::default()
    };
    let id = s.add_device(Device::new("DT2", &DT2, 16));
    for scene in &s.scenes {
        assert_eq!(scene.slots.get(&id), Some(&PatternRef::new(0, 0)));
    }
}

#[test]
fn removing_a_device_takes_it_out_of_every_scene() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.remove_device(dt2);
    assert!(s.devices.iter().all(|d| d.id != dt2));
    assert!(s.scenes.iter().all(|sc| !sc.slots.contains_key(&dt2)));
}

#[test]
fn a_scene_boundary_is_the_longest_track_across_every_box() {
    // Polymeter across devices: the switch waits for the longest track in the
    // outgoing scene so nothing is cut mid-cycle (PLAN.md §4).
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);
    s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap().length_steps = 12;
    s.device_mut(dn2).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap().length_steps = 48;

    assert_eq!(s.scene_boundary_steps(0), Some(48));
}

#[test]
fn a_new_scene_starts_where_the_one_it_was_added_from_is() {
    // The case this exists for: building a variation. A blank scene would put
    // every box back on A01, which is a scene nobody asked for.
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);
    s.set_slot_in_scene(0, dt2, PatternRef::new(0, 4));
    s.set_slot_in_scene(0, dn2, PatternRef::new(1, 2));

    let next = s.add_scene("Chorus", Some(0));
    assert_eq!(next, 1);
    assert_eq!(s.slot_in_scene(next, dt2), Some(PatternRef::new(0, 4)));
    assert_eq!(s.slot_in_scene(next, dn2), Some(PatternRef::new(1, 2)));

    // And moving the new one leaves the one it was copied from alone.
    s.set_slot_in_scene(next, dt2, PatternRef::new(0, 7));
    assert_eq!(s.slot_in_scene(0, dt2), Some(PatternRef::new(0, 4)));
}

#[test]
fn a_scene_added_with_nothing_to_copy_still_names_every_box() {
    let mut s = dt2_and_dn2();
    let added = s.add_scene("From nothing", None);
    for device in &s.devices {
        assert_eq!(s.slot_in_scene(added, device.id), Some(PatternRef::new(0, 0)));
    }
}

#[test]
fn a_scene_copied_from_one_that_predates_a_box_still_names_that_box() {
    // The gap: `add_device` fills the scenes that exist, and a scene copied from
    // an older one would inherit the hole rather than the device.
    let mut s = Session::default();
    s.add_device(Device::new("DT2", &DT2, 16));
    let stale = s.add_scene("Stale", Some(0));
    let dn2 = s.add_device(Device::new("DN2", &DN2, 16));
    // `add_device` filled the scenes that existed...
    assert_eq!(s.slot_in_scene(stale, dn2), Some(PatternRef::new(0, 0)));
    // ...and a copy of one of them must not lose it again.
    let copy = s.add_scene("Copy", Some(stale));
    assert_eq!(s.slot_in_scene(copy, dn2), Some(PatternRef::new(0, 0)));
}

#[test]
fn removing_a_scene_keeps_the_current_one_pointing_at_a_scene_that_exists() {
    let mut s = dt2_and_dn2();
    s.add_scene("Two", Some(0));
    s.add_scene("Three", Some(0));
    s.current_scene = 2;

    // Removing one *before* the current scene shifts it down, so the same scene
    // stays selected rather than the same index.
    assert!(s.remove_scene(0));
    assert_eq!(s.current_scene, 1);
    assert_eq!(s.scenes[s.current_scene].name, "Three");

    // Removing the current one lands on what took its place — here, the last.
    assert!(s.remove_scene(1));
    assert_eq!(s.current_scene, 0);

    // The last scene cannot go: `current_scene` is an index, not an option, and
    // every path that sounds anything resolves a pattern through a scene.
    assert!(!s.remove_scene(0), "the last scene stays");
    assert_eq!(s.scenes.len(), 1);
    assert!(!s.remove_scene(9), "and there is no scene 10 to remove");
}

#[test]
fn a_slot_is_never_written_for_a_box_that_is_not_in_the_session() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    let gone = s.devices[1].id;
    s.remove_device(gone);

    assert!(!s.set_slot_in_scene(0, gone, PatternRef::new(0, 3)));
    assert_eq!(s.slot_in_scene(0, gone), None, "and nothing was written");
    assert!(!s.set_slot_in_scene(4, dt2, PatternRef::new(0, 3)), "no scene 5 either");
    assert!(s.set_slot_in_scene(0, dt2, PatternRef::new(0, 3)));
    assert_eq!(s.slot_in_scene(0, dt2), Some(PatternRef::new(0, 3)));
}

#[test]
fn solo_is_session_wide_so_soloing_a_dt2_track_silences_dn2_tracks() {
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);

    assert!(!s.any_solo());
    let dn2_track = s.device(dn2).unwrap().pattern(0).unwrap().track(0).unwrap().clone();
    assert!(s.track_audible(&dn2_track));

    s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().track_mut(2).unwrap().solo = true;

    assert!(s.any_solo());
    // The DN2 track is on a different box and is silenced anyway.
    assert!(!s.track_audible(&dn2_track));
    let soloed = s.device(dt2).unwrap().pattern(0).unwrap().track(2).unwrap().clone();
    assert!(s.track_audible(&soloed));
}

#[test]
fn a_muted_track_stays_silent_even_when_it_is_soloed() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    let t = s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap();
    t.mute = true;
    t.solo = true;
    let t = t.clone();
    assert!(!s.track_audible(&t));
}

// ------------------------------------------------------------------ ports

fn port(id: &str, name: &str) -> PortRef {
    PortRef {
        id: id.into(),
        name: name.into(),
    }
}

#[test]
fn ports_rebind_by_id_first() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.device_mut(dt2).unwrap().io.input = Some(port("111", "Elektron Digitakt II"));
    s.device_mut(dt2).unwrap().io.output = Some(port("222", "Elektron Digitakt II"));

    let unbound = s.rebind_ports(
        &[port("111", "Renamed By The OS")],
        &[port("222", "Renamed By The OS")],
    );
    assert!(unbound.is_empty());
    assert_eq!(s.device(dt2).unwrap().io.input.as_ref().unwrap().name, "Renamed By The OS");
}

#[test]
fn ports_fall_back_to_matching_by_name_when_the_id_has_changed() {
    // A replug can renumber ports; the name survives it.
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.device_mut(dt2).unwrap().io.input = Some(port("111", "Elektron Digitakt II"));
    s.device_mut(dt2).unwrap().io.output = Some(port("222", "Elektron Digitakt II"));

    let unbound = s.rebind_ports(
        &[port("999", "Elektron Digitakt II")],
        &[port("888", "Elektron Digitakt II")],
    );
    assert!(unbound.is_empty());
    assert_eq!(s.device(dt2).unwrap().io.input.as_ref().unwrap().id, "999");
}

#[test]
fn a_missing_port_disables_that_devices_io_and_touches_nothing_else() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.device_mut(dt2).unwrap().io.input = Some(port("111", "Elektron Digitakt II"));
    s.device_mut(dt2).unwrap().io.output = Some(port("222", "Elektron Digitakt II"));
    s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap()
        .notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));

    let unbound = s.rebind_ports(&[port("777", "Some Other Box")], &[port("778", "Some Other Box")]);

    assert_eq!(unbound, vec![dt2]);
    assert!(s.device(dt2).unwrap().io.input.is_none());
    assert!(s.device(dt2).unwrap().io.output.is_none());
    // The patterns are untouched: a missing box costs you its I/O and nothing else.
    assert_eq!(s.device(dt2).unwrap().pattern(0).unwrap().track(0).unwrap().notes.len(), 1);
}

#[test]
fn ports_are_rematched_on_load() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.device_mut(dt2).unwrap().io.input = Some(port("old", "Elektron Digitakt II"));
    s.device_mut(dt2).unwrap().io.output = Some(port("old", "Elektron Digitakt II"));
    let json = Project::new(s).to_json().unwrap();

    let (project, unbound) = Project::from_json_with_ports(
        &json,
        &[port("new", "Elektron Digitakt II")],
        &[port("new", "Elektron Digitakt II")],
    )
    .unwrap();

    assert!(unbound.is_empty());
    assert_eq!(
        project.session.device(dt2).unwrap().io.input.as_ref().unwrap().id,
        "new"
    );
}

#[test]
fn a_device_added_after_a_load_cannot_collide_with_a_loaded_id() {
    let json = Project::new(dt2_and_dn2()).to_json().unwrap();
    let mut loaded = Project::from_json(&json).unwrap().session;
    let existing: Vec<_> = loaded.devices.iter().map(|d| d.id).collect();

    let fresh = loaded.add_device(Device::new("Another DT2", &DT2, 16));
    assert!(!existing.contains(&fresh), "id {fresh:?} collided with {existing:?}");
}

// ------------------------------------------------- binding an identity reply
//
// Phase 3's third exit criterion: "the session binds each reply to the right
// Device". `model_for_slug` answers *which model*; these answer *which box*,
// which is the harder half — identity is the instance, and two DT2s on one host
// are told apart only by their ports.

fn identity(slug: &str, name: &str, build: &str, version: &str) -> DeviceIdentity {
    DeviceIdentity {
        product_id: 42,
        supported_ids: vec![0x60],
        name: name.into(),
        slug: slug.into(),
        family: Some(0x14),
        build: build.into(),
        version: version.into(),
    }
}

fn dt2_identity() -> DeviceIdentity {
    // The real DT2 in this studio, 2026-08-13.
    identity("digitakt2", "Digitakt II", "0070", "1.15B")
}

fn dn2_identity() -> DeviceIdentity {
    identity("digitone2", "Digitone II", "0049", "1.10D")
}

#[test]
fn each_reply_binds_to_the_box_of_its_own_model() {
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);

    let bound_dt2 = s
        .bind_identity(
            &dt2_identity(),
            port("111", "Elektron Digitakt II"),
            port("222", "Elektron Digitakt II"),
        )
        .unwrap();
    let bound_dn2 = s
        .bind_identity(
            &dn2_identity(),
            port("333", "Elektron Digitone II"),
            port("444", "Elektron Digitone II"),
        )
        .unwrap();

    assert_eq!(bound_dt2, dt2);
    assert_eq!(bound_dn2, dn2);
    let io = &s.device(dt2).unwrap().io;
    assert_eq!(io.input.as_ref().unwrap().id, "111");
    assert_eq!(io.output.as_ref().unwrap().id, "222");
    assert_eq!(io.build.as_deref(), Some("0070"));
    assert_eq!(io.version.as_deref(), Some("1.15B"));
    assert_eq!(s.device(dn2).unwrap().io.build.as_deref(), Some("0049"));
}

#[test]
fn binding_writes_io_and_touches_nothing_else() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap()
        .notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
    let before = s.device(dt2).unwrap().patterns.clone();

    s.bind_identity(&dt2_identity(), port("111", "DT2 in"), port("222", "DT2 out")).unwrap();

    // An identity reply is session state. It must never reach a pattern byte.
    assert_eq!(s.device(dt2).unwrap().patterns, before);
    assert_eq!(s.device(dt2).unwrap().name, "DT2");
    assert_eq!(s.tempo_bpm, 120.0);
}

#[test]
fn re_identifying_the_same_ports_updates_that_box_rather_than_claiming_another() {
    // Two identical boxes: the first identify takes the free one, and a second
    // identify on the same ports must land on it again — not on the other DT2.
    let mut s = dt2_and_dn2();
    let second_dt2 = s.add_device(Device::new("DT2 b", &DT2, 16));

    let first = s
        .bind_identity(&dt2_identity(), port("111", "Elektron Digitakt II"), port("222", "Elektron Digitakt II"))
        .unwrap();
    let again = s
        .bind_identity(
            &identity("digitakt2", "Digitakt II", "0071", "1.15C"),
            port("111", "Elektron Digitakt II"),
            port("222", "Elektron Digitakt II"),
        )
        .unwrap();

    assert_eq!(first, again);
    assert_ne!(again, second_dt2);
    assert_eq!(s.device(first).unwrap().io.build.as_deref(), Some("0071"));
    assert!(!s.device(second_dt2).unwrap().has_ports());
}

#[test]
fn two_identical_boxes_are_told_apart_by_their_ports() {
    let mut s = dt2_and_dn2();
    let a = s.devices[0].id;
    let b = s.add_device(Device::new("DT2 b", &DT2, 16));

    let first = s
        .bind_identity(&dt2_identity(), port("111", "Elektron Digitakt II"), port("222", "Elektron Digitakt II"))
        .unwrap();
    let second = s
        .bind_identity(&dt2_identity(), port("333", "Elektron Digitakt II #2"), port("444", "Elektron Digitakt II #2"))
        .unwrap();

    assert_eq!(first, a);
    assert_eq!(second, b, "the second box must take the still-unbound device");
}

#[test]
fn the_only_box_of_its_model_follows_a_replug() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.bind_identity(&dt2_identity(), port("111", "Elektron Digitakt II"), port("222", "Elektron Digitakt II")).unwrap();

    // Same box, different socket: nothing else could be meant.
    let again = s
        .bind_identity(&dt2_identity(), port("999", "Elektron Digitakt II"), port("888", "Elektron Digitakt II"))
        .unwrap();

    assert_eq!(again, dt2);
    assert_eq!(s.device(dt2).unwrap().io.input.as_ref().unwrap().id, "999");
}

#[test]
fn several_boxes_of_one_model_all_bound_elsewhere_refuse_to_guess() {
    let mut s = dt2_and_dn2();
    let a = s.devices[0].id;
    let b = s.add_device(Device::new("DT2 b", &DT2, 16));
    s.bind_identity(&dt2_identity(), port("1", "A in"), port("2", "A out")).unwrap();
    s.bind_identity(&dt2_identity(), port("3", "B in"), port("4", "B out")).unwrap();

    let err = s
        .bind_identity(&dt2_identity(), port("5", "C in"), port("6", "C out"))
        .unwrap_err();

    match err {
        BindError::Ambiguous { model, candidates } => {
            assert_eq!(model.key, "DT2");
            assert_eq!(candidates, vec![a, b]);
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
    // Nothing moved: refusing must not half-bind.
    assert_eq!(s.device(a).unwrap().io.input.as_ref().unwrap().id, "1");
    assert_eq!(s.device(b).unwrap().io.input.as_ref().unwrap().id, "3");

    // Naming one resolves it.
    s.bind_identity_to(b, &dt2_identity(), port("5", "C in"), port("6", "C out"))
        .expect("naming a device resolves the ambiguity");
    assert_eq!(s.device(b).unwrap().io.input.as_ref().unwrap().id, "5");
}

#[test]
fn a_port_belongs_to_one_box() {
    // The DN2 was on these ports; a DT2 answers on them now. Leaving both bound
    // would have two devices sending to one socket.
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);
    s.device_mut(dn2).unwrap().io.input = Some(port("111", "Elektron Digitakt II"));
    s.device_mut(dn2).unwrap().io.output = Some(port("222", "Elektron Digitakt II"));

    let bound = s
        .bind_identity(&dt2_identity(), port("111", "Elektron Digitakt II"), port("222", "Elektron Digitakt II"))
        .unwrap();

    assert_eq!(bound, dt2);
    assert!(!s.device(dn2).unwrap().has_ports(), "the DN2 must have lost the ports it does not own");
}

#[test]
fn an_unknown_box_stays_unbound_rather_than_becoming_the_nearest_model() {
    let mut s = dt2_and_dn2();
    // What `identity_from_responses` produces for a product id we do not know.
    let unknown = identity("elektron", "Syntakt", "0001", "1.0");

    let err = s.bind_identity(&unknown, port("1", "Elektron Syntakt"), port("2", "Elektron Syntakt")).unwrap_err();

    assert_eq!(err, BindError::UnknownModel { slug: "elektron".into(), name: "Syntakt".into() });
    assert!(s.devices.iter().all(|d| !d.has_ports()));
}

#[test]
fn a_session_with_no_box_of_that_model_says_so() {
    let mut s = Session::default();
    s.add_device(Device::new("DN2", &DN2, 16));

    let err = s.bind_identity(&dt2_identity(), port("1", "in"), port("2", "out")).unwrap_err();

    assert_eq!(err, BindError::NoDeviceOfModel(&DT2));
}

#[test]
fn a_reply_is_refused_for_a_device_of_another_model() {
    let mut s = dt2_and_dn2();
    let dn2 = s.devices[1].id;

    let err = s
        .bind_identity_to(dn2, &dt2_identity(), port("1", "in"), port("2", "out"))
        .unwrap_err();

    match err {
        BindError::ModelMismatch { expected, found, .. } => {
            assert_eq!(expected.key, "DT2");
            assert_eq!(found.key, "DN2");
        }
        other => panic!("expected ModelMismatch, got {other:?}"),
    }
    assert!(!s.device(dn2).unwrap().has_ports());
}

#[test]
fn a_bound_identity_survives_the_project_file() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.bind_identity(&dt2_identity(), port("111", "Elektron Digitakt II"), port("222", "Elektron Digitakt II")).unwrap();

    let json = Project::new(s).to_json().unwrap();
    let loaded = Project::from_json(&json).unwrap().session;

    let io = &loaded.device(dt2).unwrap().io;
    assert_eq!(io.build.as_deref(), Some("0070"));
    assert_eq!(io.version.as_deref(), Some("1.15B"));
    assert_eq!(io.input.as_ref().unwrap().name, "Elektron Digitakt II");
}

#[test]
fn a_box_added_in_the_app_takes_the_session_clock_like_a_reloaded_one() {
    // `#[derive(Default)]` on `DeviceIo` gave `takes_clock: false` while its
    // serde default gives `true`. So a box added in the app sat silently off the
    // clock — no 0xF8 reaches it, and a box on external clock simply never
    // starts — while the same box saved and reloaded came back on it. The two
    // defaults now agree, and this is what says so.
    let fresh = Device::new("DT2", &DT2, 1);
    assert!(fresh.io.takes_clock, "a new box follows the session clock");

    let json = serde_json::to_string(&fresh).unwrap();
    let reloaded: Device = serde_json::from_str(&json).unwrap();
    assert_eq!(reloaded.io, fresh.io);

    // And the same for a file written before the field existed.
    let older: DeviceIo = serde_json::from_str("{}").unwrap();
    assert_eq!(older, DeviceIo::default());
}

// ------------------------------------------------- picking a port by hand
//
// Identify used to be the only thing that ever gave a device a port, so the app
// could not be pointed at an IAC bus or a soft synth and could not make a sound
// without an Elektron on the desk — which put the whole UI beyond reach of the
// dev loop that PLAN.md §7 rule 1 insists on. `set_device_port` is that path,
// and it carries the two rules `bind_identity_to` already established rather
// than inventing its own.

#[test]
fn a_port_can_be_picked_by_hand_with_no_box_to_identify() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;

    assert!(s.set_device_port(dt2, PortEnd::Output, Some(port("iac1", "IAC Driver Bus 1"))));

    assert_eq!(s.device(dt2).unwrap().io.output.as_ref().unwrap().name, "IAC Driver Bus 1");
    // Only the end that was asked for.
    assert!(s.device(dt2).unwrap().io.input.is_none());
}

#[test]
fn a_hand_picked_port_belongs_to_one_box_like_an_identified_one() {
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);
    let bus = port("iac1", "IAC Driver Bus 1");

    s.set_device_port(dt2, PortEnd::Output, Some(bus.clone()));
    s.set_device_port(dn2, PortEnd::Output, Some(bus));

    // Two devices on one socket is a DT2 trig coming out of the DN2.
    assert!(s.device(dt2).unwrap().io.output.is_none());
    assert_eq!(s.device(dn2).unwrap().io.output.as_ref().unwrap().id, "iac1");
}

#[test]
fn releasing_an_output_leaves_an_input_of_the_same_name_alone() {
    // The two ends are separate namespaces: a box's input and output usually
    // *do* share a name, so a release that matched on name alone would unbind
    // the wrong half.
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);
    s.set_device_port(dt2, PortEnd::Input, Some(port("in-1", "Elektron Digitakt II")));
    s.set_device_port(dt2, PortEnd::Output, Some(port("out-1", "Elektron Digitakt II")));

    // The DN2 takes the *output* of that name.
    s.set_device_port(dn2, PortEnd::Output, Some(port("out-1", "Elektron Digitakt II")));

    assert!(s.device(dt2).unwrap().io.output.is_none(), "the output moved");
    assert!(s.device(dt2).unwrap().io.input.is_some(), "the input is a different port");
}

#[test]
fn a_port_can_be_unset_by_hand() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.set_device_port(dt2, PortEnd::Output, Some(port("iac1", "IAC Driver Bus 1")));

    assert!(s.set_device_port(dt2, PortEnd::Output, None));
    assert!(s.device(dt2).unwrap().io.output.is_none());
}

#[test]
fn moving_a_device_off_its_ports_drops_the_os_it_reported() {
    // `build`/`version` are what answered on the ports the handshake went out
    // on. Once either end moves they describe a box that is not there, and "OS
    // 0070" beside a hand-picked IAC bus is exactly the plausible-looking lie
    // that leaving a device visibly unbound exists to avoid.
    let mut s = dt2_and_dn2();
    let dt2 = s
        .bind_identity(&dt2_identity(), port("111", "Elektron Digitakt II"), port("222", "Elektron Digitakt II"))
        .unwrap();
    assert_eq!(s.device(dt2).unwrap().io.build.as_deref(), Some("0070"));

    s.set_device_port(dt2, PortEnd::Output, Some(port("iac1", "IAC Driver Bus 1")));

    assert!(s.device(dt2).unwrap().io.build.is_none());
    assert!(s.device(dt2).unwrap().io.version.is_none());
}

#[test]
fn picking_the_port_already_there_changes_nothing() {
    // The UI calls this from a combo box every frame it is touched, and an edit
    // costs a session snapshot down the channel to the engine. Re-picking what
    // is already set must not report a change — nor quietly drop the OS report.
    let mut s = dt2_and_dn2();
    let dt2 = s
        .bind_identity(&dt2_identity(), port("111", "Elektron Digitakt II"), port("222", "Elektron Digitakt II"))
        .unwrap();
    let before = s.clone();

    assert!(!s.set_device_port(dt2, PortEnd::Output, Some(port("222", "Elektron Digitakt II"))));

    assert_eq!(s.device(dt2).unwrap().io, before.device(dt2).unwrap().io);
    assert_eq!(s.device(dt2).unwrap().io.build.as_deref(), Some("0070"));
}

#[test]
fn setting_a_port_on_a_device_that_is_not_there_changes_nothing() {
    let mut s = dt2_and_dn2();
    let before = s.clone();
    let gone = digi_core::DeviceId(9_999);

    assert!(!s.set_device_port(gone, PortEnd::Output, Some(port("iac1", "IAC Driver Bus 1"))));
    assert_eq!(s.devices.len(), before.devices.len());
    for (a, b) in s.devices.iter().zip(&before.devices) {
        assert_eq!(a.io, b.io);
    }
}

#[test]
fn a_hand_picked_port_survives_the_project_file() {
    // Nothing new in the format — `DeviceIo` already round-trips — but the point
    // of the picker is a soft-synth setup you do not have to rebuild each time.
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.set_device_port(dt2, PortEnd::Output, Some(port("iac1", "IAC Driver Bus 1")));

    let json = Project::new(s).to_json().unwrap();
    let (project, unbound) = Project::from_json_with_ports(
        &json,
        &[],
        &[port("iac1", "IAC Driver Bus 1")],
    )
    .unwrap();
    let loaded = project.session;

    assert_eq!(unbound, vec![loaded.devices[0].id], "the input never was bound");
    assert_eq!(loaded.devices[0].io.output.as_ref().unwrap().name, "IAC Driver Bus 1");
}

// -------------------------------------------------------------- the song

// Phase 12. A song is rows of *scenes*, so most of what a row has to survive is
// the scene list moving under it — and a project written before song mode has to
// keep loading, and keep saving byte-identically.

#[test]
fn a_session_starts_with_no_song_at_all() {
    // `None` and not an empty song: "nobody has built an arrangement" is a state
    // the transport asks about, and it is what keeps the project file unchanged.
    assert!(dt2_and_dn2().song().is_none());
}

#[test]
fn a_song_less_session_writes_no_song_key() {
    let json = Project::new(dt2_and_dn2()).to_json().unwrap();
    assert!(!json.contains("song"), "{json}");
}

#[test]
fn a_project_written_before_song_mode_still_loads() {
    // The same file a pre-Phase-12 build wrote: no `song` key anywhere.
    let json = Project::new(dt2_and_dn2()).to_json().unwrap();
    let loaded = Project::from_json(&json).unwrap().session;
    assert!(loaded.song().is_none());
}

#[test]
fn a_song_round_trips_through_the_project_file() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.add_scene("Chorus", Some(0));
    s.song_mut().name = "Sad Song".into();
    s.add_song_row(0).unwrap();
    s.add_song_row(1).unwrap();
    s.song_mut().row_mut(1).unwrap().label = "CHORUS".into();
    s.song_mut().row_mut(1).unwrap().repeats = 4;
    s.song_mut().row_mut(1).unwrap().length_steps = Some(32);
    s.song_mut().row_mut(1).unwrap().set_mute(dt2, 5, true);
    s.song_mut().end = digi_core::EndAction::Stop;

    let json = Project::new(s.clone()).to_json().unwrap();
    let loaded = Project::from_json(&json).unwrap().session;

    assert_eq!(loaded.song, s.song);
    let row = loaded.song_row(1).unwrap();
    assert_eq!(row.label, "CHORUS");
    assert_eq!(row.plays(), 4);
    assert_eq!(row.length_steps, Some(32));
    assert_eq!(row.mutes(dt2, 5), Some(true));
    assert_eq!(loaded.song().unwrap().end, digi_core::EndAction::Stop);
}

#[test]
fn saving_a_session_with_a_song_twice_produces_identical_bytes() {
    // The `BTreeMap` argument, extended to a row's mute masks: a save that is not
    // byte-stable makes every project file look edited.
    let mut s = dt2_and_dn2();
    let (dt2, dn2) = (s.devices[0].id, s.devices[1].id);
    s.add_song_row(0).unwrap();
    s.song_mut().row_mut(0).unwrap().set_mute(dn2, 2, true);
    s.song_mut().row_mut(0).unwrap().set_mute(dt2, 9, true);

    let a = Project::new(s.clone()).to_json_pretty().unwrap();
    let b = Project::new(s).to_json_pretty().unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_new_row_takes_its_mutes_from_the_pattern_it_plays() {
    // The box's rule: "when selecting a pattern for the row, the row's mute state
    // initially reflects the pattern's mute state".
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    let dn2 = s.devices[1].id;
    s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().track_mut(3).unwrap().mute = true;

    let row = s.add_song_row(0).unwrap();
    let row = s.song_row(row).unwrap();
    assert_eq!(row.mutes(dt2, 3), Some(true));
    assert_eq!(row.mutes(dt2, 4), Some(false));
    // The DN2's pattern had nothing muted, so that box keeps inheriting rather
    // than gaining a mask of zeroes the panel would draw as an override.
    assert_eq!(row.mutes(dn2, 0), None);
}

#[test]
fn removing_a_scene_carries_the_song_rows_with_it() {
    let mut s = dt2_and_dn2();
    s.add_scene("B", Some(0));
    s.add_scene("C", Some(0));
    s.add_song_row(0).unwrap();
    s.add_song_row(1).unwrap();
    s.add_song_row(2).unwrap();

    assert!(s.remove_scene(1));
    let scenes: Vec<usize> = s.song().unwrap().rows.iter().map(|r| r.scene).collect();
    // Scene 2's row followed the list down; the row that named the scene that
    // went landed on the one the session is now on, so no row names nothing.
    assert_eq!(scenes, vec![0, 0, 1]);
    assert!(s.song().unwrap().broken_rows(s.scenes.len()).is_empty());
}

#[test]
fn removing_a_device_takes_its_row_mutes_with_it() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.add_song_row(0).unwrap();
    s.song_mut().row_mut(0).unwrap().set_mute(dt2, 1, true);

    s.remove_device(dt2);
    // A re-added box gets a fresh id, so a left-behind mask could only ever be
    // read by the wrong device.
    assert_eq!(s.song_row(0).unwrap().mutes(dt2, 1), None);
}

#[test]
fn a_row_mute_substitutes_for_the_patterns_mute_rather_than_stacking() {
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().track_mut(0).unwrap().mute = true;
    let mut row = digi_core::SongRow::new(0);
    // The row sounds a track the pattern mutes — a substitution, not a second
    // mute stage.
    row.set_mute(dt2, 0, false);

    let track = s.device(dt2).unwrap().pattern(0).unwrap().track(0).unwrap();
    assert!(digi_core::song::audible(row.mutes(dt2, 0), track, false));
    assert!(!digi_core::song::audible(None, track, false));
}

#[test]
fn a_row_mute_does_not_undo_a_solo() {
    // Solo is the desk, not the arrangement (PLAN.md §2).
    let mut s = dt2_and_dn2();
    let dt2 = s.devices[0].id;
    s.device_mut(dt2).unwrap().pattern_mut(0).unwrap().track_mut(1).unwrap().solo = true;
    let mut row = digi_core::SongRow::new(0);
    row.set_mute(dt2, 0, false);

    let pattern = s.device(dt2).unwrap().pattern(0).unwrap();
    assert!(!digi_core::song::audible(row.mutes(dt2, 0), pattern.track(0).unwrap(), true));
    assert!(digi_core::song::audible(row.mutes(dt2, 1), pattern.track(1).unwrap(), true));
}
