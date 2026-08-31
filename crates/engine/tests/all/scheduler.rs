//! The scheduler: what plays, on which port, at what second.
//!
//! These have no JS oracle — `js/midi.js` schedules one track of one box, so
//! there is nothing to derive polymeter, two-port output or a shared clock from.
//! They are specified from PLAN.md §4, the same footing `Session::bind_identity`'s
//! tests are on.
//!
//! Everything is in seconds since the transport started. Nothing here touches a
//! thread, an `Instant` or `midir`: that is the point of keeping the scheduler
//! pure, and it is why a question like "does a note-off survive its track's wrap"
//! is answerable in microseconds instead of by listening.

use digi_core::device::{Device, DeviceIo, PortRef, DN2, DT2};
use digi_core::model::{Note, TrackScale};
use digi_core::session::Session;
use digi_engine::event::{MidiMsg, PortTable, ScheduledEvent};
use digi_engine::rng::{ScriptedRng, XorShift64};
use digi_engine::scheduler::{scene_cycle_seconds, NoPLocks, Scheduler};

const BPM: f64 = 120.0;
/// A 16th at 120 bpm.
const STEP: f64 = 0.125;

fn port(name: &str) -> PortRef {
    PortRef { id: format!("id-{name}"), name: name.to_string() }
}

/// A session with one box, one 16-track pattern, everything on one out port.
fn one_box(model: &'static digi_core::device::DeviceModel, port_name: &str) -> (Session, Device) {
    let mut device = Device::new(model.display, model, 16);
    device.io = DeviceIo {
        output: Some(port(port_name)),
        takes_clock: true,
        ..device.io
    };
    let mut session = Session { tempo_bpm: BPM, ..Session::default() };
    let d = device.clone();
    session.add_device(device);
    (session, d)
}

/// Put notes on track `t` of the device's current pattern and set its length.
fn set_track(
    session: &mut Session,
    device: digi_core::device::DeviceId,
    t: usize,
    length: u16,
    notes: Vec<Note>,
) {
    let slot = session.slot_in_scene(session.current_scene, device).unwrap().slot();
    let d = session.device_mut(device).unwrap();
    let pattern = d.pattern_mut(slot).unwrap();
    let track = pattern.track_mut(t).unwrap();
    track.length_steps = length;
    track.notes = notes;
}

fn note_at(step: f64, pitch: u8) -> Note {
    Note::new(step, pitch, 1.0, 100, 0.0)
}

/// Prepare a scheduler over a session and return it with its port table.
fn prepared(session: &Session) -> (Scheduler, PortTable) {
    let mut ports = PortTable::new();
    let mut s = Scheduler::new(BPM);
    s.prepare(session, &mut ports);
    (s, ports)
}

fn run(scheduler: &mut Scheduler, session: &Session, to: f64) -> Vec<ScheduledEvent> {
    let mut out = Vec::new();
    let mut rng = ScriptedRng::always();
    scheduler.advance(session, to, &mut rng, &NoPLocks, &mut out);
    out
}

/// Just the note-ons, as `(second, port, pitch)`.
fn note_ons(events: &[ScheduledEvent]) -> Vec<(f64, usize, u8)> {
    events
        .iter()
        .filter_map(|e| match e.msg {
            MidiMsg::NoteOn { pitch, .. } => Some((e.at, e.port.0, pitch)),
            _ => None,
        })
        .collect()
}

fn note_offs(events: &[ScheduledEvent]) -> Vec<(f64, u8)> {
    events
        .iter()
        .filter_map(|e| match e.msg {
            MidiMsg::NoteOff { pitch, .. } => Some((e.at, pitch)),
            _ => None,
        })
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

// --- one track ---------------------------------------------------------------

#[test]
fn a_trig_on_every_step_lands_one_step_apart() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 4, (0..4).map(|s| note_at(s as f64, 60 + s)).collect());
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let events = run(&mut sched, &session, 4.0 * STEP);
    let ons = note_ons(&events);
    assert_eq!(ons.len(), 4);
    for (i, (at, _, pitch)) in ons.iter().enumerate() {
        assert!(close(*at, i as f64 * STEP), "step {i} landed at {at}");
        assert_eq!(*pitch, 60 + i as u8);
    }
}

#[test]
fn a_track_wraps_and_plays_again() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 2, vec![note_at(0.0, 60)]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    // Four steps = two passes of a 2-step track.
    let events = run(&mut sched, &session, 4.0 * STEP);
    let ons = note_ons(&events);
    assert_eq!(ons.len(), 2, "one hit per pass");
    assert!(close(ons[1].0, 2.0 * STEP));
}

#[test]
fn micro_timing_moves_a_note_off_the_grid_without_moving_the_step() {
    let (mut session, d) = one_box(&DT2, "DT2");
    // −1/24 of a step, the finest the hardware stores.
    let mut early = note_at(1.0, 60);
    early.micro = -1.0 / 24.0;
    set_track(&mut session, d.id, 0, 4, vec![early]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let events = run(&mut sched, &session, 4.0 * STEP);
    let ons = note_ons(&events);
    assert_eq!(ons.len(), 1);
    assert!(close(ons[0].0, STEP - STEP / 24.0), "landed at {}", ons[0].0);
}

// --- swing -------------------------------------------------------------------

#[test]
fn swing_pushes_odd_steps_late_and_leaves_even_ones_alone() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 4, (0..4).map(|s| note_at(s as f64, 60)).collect());
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap().swing = 80;
    }
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let ons = note_ons(&run(&mut sched, &session, 4.0 * STEP));
    // Maximum swing is 0.6 of a step late on the odd steps.
    assert!(close(ons[0].0, 0.0));
    assert!(close(ons[1].0, 1.6 * STEP), "{}", ons[1].0);
    assert!(close(ons[2].0, 2.0 * STEP));
    assert!(close(ons[3].0, 3.6 * STEP));
}

#[test]
fn swing_never_reorders_the_steps() {
    // 0.6 of a step is the most swing can displace, so an odd step can never
    // overtake the even one after it. Worth pinning: if it could, the queue's
    // deadline ordering would stop matching the pattern's step ordering.
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 8, (0..8).map(|s| note_at(s as f64, 60 + s)).collect());
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap().swing = 80;
    }
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let ons = note_ons(&run(&mut sched, &session, 8.0 * STEP));
    let pitches: Vec<u8> = ons.iter().map(|o| o.2).collect();
    assert_eq!(pitches, vec![60, 61, 62, 63, 64, 65, 66, 67]);
    assert!(ons.windows(2).all(|w| w[0].0 < w[1].0), "deadlines must stay ascending");
}

// --- polymeter ---------------------------------------------------------------

#[test]
fn tracks_of_different_lengths_wrap_independently() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 3, vec![note_at(0.0, 60)]); // hits every 3 steps
    set_track(&mut session, d.id, 1, 4, vec![note_at(0.0, 72)]); // every 4
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    // 12 steps: the two lines meet again at the top.
    let ons = note_ons(&run(&mut sched, &session, 12.0 * STEP));
    let threes: Vec<f64> = ons.iter().filter(|o| o.2 == 60).map(|o| o.0 / STEP).collect();
    let fours: Vec<f64> = ons.iter().filter(|o| o.2 == 72).map(|o| o.0 / STEP).collect();
    assert_eq!(threes, vec![0.0, 3.0, 6.0, 9.0]);
    assert_eq!(fours, vec![0.0, 4.0, 8.0]);
}

#[test]
fn track_scale_changes_how_fast_a_track_runs() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    set_track(&mut session, d.id, 1, 4, vec![note_at(0.0, 72)]);
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        let p = session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap();
        p.track_mut(1).unwrap().scale = TrackScale::Two; // twice as fast
    }
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let ons = note_ons(&run(&mut sched, &session, 4.0 * STEP));
    let normal: Vec<f64> = ons.iter().filter(|o| o.2 == 60).map(|o| o.0 / STEP).collect();
    let fast: Vec<f64> = ons.iter().filter(|o| o.2 == 72).map(|o| o.0 / STEP).collect();
    assert_eq!(normal, vec![0.0], "a 4-step track hits once in 4 steps");
    assert_eq!(fast, vec![0.0, 2.0], "at 2x it wraps twice in the same time");
}

// --- every track, and the channel it leaves on -------------------------------

/// "Tracks 9 to 16 do not emit MIDI to the boxes" was reported on 2026-08-18, and
/// the scheduler is the first place anyone looks. **It is not here.** Every track
/// of a 16-track box plays, each on its own channel, and this says so cheaply
/// enough that the next report can start somewhere else.
///
/// The cause was the boxes' own MIDI CONFIG. A factory DT2 or DN2 assigns
/// channels to tracks 1–8 only — `TRACK 9–16 CH` are `OFF` — and reserves 9 for
/// `FX CONTROL CH` and 10 for `AUTO CHANNEL`, so a correct note-on for track 9
/// arrives at a box with nothing listening on channel 9. `ui::tracks::channel_note`
/// is the app's half of the answer; this is the engine's.
#[test]
fn every_track_of_a_box_plays_on_its_own_channel() {
    let (mut session, d) = one_box(&DT2, "DT2");
    for t in 0..16 {
        set_track(&mut session, d.id, t, 16, vec![note_at(0.0, 60)]);
    }
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    // One step is enough: every track has a trig on step 0.
    let events = run(&mut sched, &session, STEP);
    let mut channels: Vec<u8> = events
        .iter()
        .filter_map(|e| match e.msg {
            MidiMsg::NoteOn { channel, .. } => Some(channel),
            _ => None,
        })
        .collect();
    channels.sort();
    assert_eq!(
        channels,
        (0..16).collect::<Vec<u8>>(),
        "sixteen trigs on sixteen channels — the default map is track n on channel n"
    );
}

// --- note lifetime -----------------------------------------------------------

#[test]
fn a_note_off_follows_its_note_on_by_the_notes_length_less_the_gap() {
    let (mut session, d) = one_box(&DT2, "DT2");
    let mut long = note_at(0.0, 60);
    long.len = 4.0; // one bar
    set_track(&mut session, d.id, 0, 16, vec![long]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let events = run(&mut sched, &session, 16.0 * STEP);
    let offs = note_offs(&events);
    assert_eq!(offs.len(), 1);
    assert!(close(offs[0].0, 4.0 * STEP - 0.008), "{}", offs[0].0);
}

#[test]
fn a_note_off_that_falls_past_its_tracks_wrap_still_fires() {
    // The polymetric case PLAN.md §4 names: a 2-step track holding a 3-step note.
    let (mut session, d) = one_box(&DT2, "DT2");
    let mut long = note_at(0.0, 60);
    long.len = 3.0;
    set_track(&mut session, d.id, 0, 2, vec![long]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let events = run(&mut sched, &session, 6.0 * STEP);
    // Each pass retriggers pitch 60 while the previous one is still sounding, so
    // every note-on after the first is preceded by a note-off for the same pitch.
    let ons = note_ons(&events);
    let offs = note_offs(&events);
    assert_eq!(ons.len(), 3, "three passes of a 2-step track");
    assert!(offs.len() >= 2, "each retrigger releases the voice first: {offs:?}");
    for (on, off) in ons.iter().skip(1).zip(offs.iter()) {
        assert!(close(on.0, off.0), "the off must be dated with the on that displaced it");
    }
}

#[test]
fn retriggering_a_sounding_pitch_releases_it_first_in_send_order() {
    let (mut session, d) = one_box(&DT2, "DT2");
    let mut long = note_at(0.0, 60);
    long.len = 8.0;
    set_track(&mut session, d.id, 0, 1, vec![long]); // every step, 8 steps long
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let events = run(&mut sched, &session, 2.0 * STEP);
    let at_second_step: Vec<&MidiMsg> = events
        .iter()
        .filter(|e| close(e.at, STEP))
        .map(|e| &e.msg)
        .collect();
    assert!(
        matches!(at_second_step[0], MidiMsg::NoteOff { .. }),
        "off before on at the same deadline: {at_second_step:?}"
    );
    assert!(matches!(at_second_step[1], MidiMsg::NoteOn { .. }));
}

#[test]
fn stop_releases_everything_still_sounding() {
    let (mut session, d) = one_box(&DT2, "DT2");
    let mut long = note_at(0.0, 60);
    long.len = 64.0;
    set_track(&mut session, d.id, 0, 16, vec![long]);
    let (mut sched, _) = prepared(&session);

    run(&mut sched, &session, STEP);
    assert_eq!(sched.active_notes().len(), 1);

    let mut out = Vec::new();
    sched.stop(9.0, &mut out);
    assert!(sched.active_notes().is_empty());
    assert_eq!(note_offs(&out), vec![(9.0, 60)]);
    assert!(out.iter().any(|e| e.msg == MidiMsg::Stop), "the boxes are told too");
}

#[test]
fn panic_shouts_at_every_channel_in_use() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 16, vec![note_at(0.0, 60)]);
    let (mut sched, _) = prepared(&session);

    let mut out = Vec::new();
    sched.panic(0.0, &mut out);
    // 16 tracks default to channels 0..15, all on one port.
    let all_notes_off = out
        .iter()
        .filter(|e| matches!(e.msg, MidiMsg::ControlChange { controller: 123, .. }))
        .count();
    let all_sound_off = out
        .iter()
        .filter(|e| matches!(e.msg, MidiMsg::ControlChange { controller: 120, .. }))
        .count();
    assert_eq!(all_notes_off, 16);
    assert_eq!(all_sound_off, 16);
}

// --- two boxes, one queue ----------------------------------------------------

#[test]
fn a_dt2_and_a_dn2_play_from_one_queue_on_two_ports() {
    // PLAN.md §4's headline case, and the whole reason events carry a port.
    let mut session = Session { tempo_bpm: BPM, ..Session::default() };

    let mut dt2 = Device::new("DT2", &DT2, 16);
    dt2.io = DeviceIo { output: Some(port("Digitakt II")), takes_clock: true, ..dt2.io };
    let dt2_id = session.add_device(dt2);

    let mut dn2 = Device::new("DN2", &DN2, 16);
    dn2.io = DeviceIo { output: Some(port("Digitone II")), takes_clock: true, ..dn2.io };
    let dn2_id = session.add_device(dn2);

    // Same step on both boxes, plus a DN2 track at a different length.
    set_track(&mut session, dt2_id, 0, 4, vec![note_at(0.0, 36)]);
    set_track(&mut session, dn2_id, 0, 4, vec![note_at(0.0, 60)]);
    set_track(&mut session, dn2_id, 1, 3, vec![note_at(0.0, 72)]);

    let (mut sched, ports) = prepared(&session);
    sched.send_clock = false;
    let dt2_port = ports.get("Digitakt II").unwrap();
    let dn2_port = ports.get("Digitone II").unwrap();
    assert_ne!(dt2_port, dn2_port);

    let events = run(&mut sched, &session, 12.0 * STEP);
    let ons = note_ons(&events);

    // Both boxes hit step 0 together, back to back in the queue.
    let at_zero: Vec<usize> = ons.iter().filter(|o| close(o.0, 0.0)).map(|o| o.1).collect();
    assert_eq!(at_zero.len(), 3, "two DN2 tracks and one DT2 track");
    assert!(at_zero.contains(&dt2_port.0) && at_zero.contains(&dn2_port.0));

    // The DN2's 3-step track is on the DN2's port and nowhere else.
    let threes: Vec<(f64, usize)> = ons
        .iter()
        .filter(|o| o.2 == 72)
        .map(|o| (o.0 / STEP, o.1))
        .collect();
    assert_eq!(threes, vec![(0.0, dn2_port.0), (3.0, dn2_port.0), (6.0, dn2_port.0), (9.0, dn2_port.0)]);

    // And the queue is in deadline order across both boxes, not grouped by box.
    assert!(events.windows(2).all(|w| w[0].at <= w[1].at));
}

#[test]
fn a_track_with_its_own_out_port_leaves_its_devices_port() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    set_track(&mut session, d.id, 1, 4, vec![note_at(0.0, 72)]);
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        let p = session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap();
        p.track_mut(1).unwrap().out_port = Some("Some Synth".into());
    }
    let (mut sched, ports) = prepared(&session);
    sched.send_clock = false;

    let ons = note_ons(&run(&mut sched, &session, STEP));
    let dt2 = ports.get("DT2").unwrap().0;
    let other = ports.get("Some Synth").unwrap().0;
    assert_eq!(ons.iter().find(|o| o.2 == 60).unwrap().1, dt2);
    assert_eq!(ons.iter().find(|o| o.2 == 72).unwrap().1, other);
}

#[test]
fn a_device_with_no_out_port_is_silent_rather_than_an_error() {
    let mut session = Session { tempo_bpm: BPM, ..Session::default() };
    let d = Device::new("DT2", &DT2, 16);
    let id = session.add_device(d);
    set_track(&mut session, id, 0, 4, vec![note_at(0.0, 60)]);

    let (mut sched, _) = prepared(&session);
    let events = run(&mut sched, &session, 4.0 * STEP);
    assert!(note_ons(&events).is_empty());
}

// --- clock -------------------------------------------------------------------

#[test]
fn clock_goes_to_every_device_that_takes_it_from_one_counter() {
    let mut session = Session { tempo_bpm: BPM, ..Session::default() };
    let mut dt2 = Device::new("DT2", &DT2, 16);
    dt2.io = DeviceIo { output: Some(port("Digitakt II")), takes_clock: true, ..dt2.io };
    session.add_device(dt2);
    let mut dn2 = Device::new("DN2", &DN2, 16);
    dn2.io = DeviceIo { output: Some(port("Digitone II")), takes_clock: true, ..dn2.io };
    session.add_device(dn2);

    let (mut sched, _) = prepared(&session);
    let events = run(&mut sched, &session, 0.5); // one beat at 120 bpm

    let clocks: Vec<&ScheduledEvent> =
        events.iter().filter(|e| e.msg == MidiMsg::Clock).collect();
    assert_eq!(clocks.len(), 48, "24 PPQN × 2 ports for one beat");

    // Each tick goes to both ports at exactly the same deadline — one counter,
    // not one per box.
    for pair in clocks.chunks(2) {
        assert!(close(pair[0].at, pair[1].at));
        assert_ne!(pair[0].port, pair[1].port);
    }
    assert!(close(clocks[2].at, 0.5 / 24.0), "{}", clocks[2].at);
}

#[test]
fn a_device_that_does_not_take_clock_is_not_sent_any() {
    let mut session = Session { tempo_bpm: BPM, ..Session::default() };
    let mut dt2 = Device::new("DT2", &DT2, 16);
    dt2.io = DeviceIo { output: Some(port("Digitakt II")), takes_clock: true, ..dt2.io };
    session.add_device(dt2);
    let mut dn2 = Device::new("DN2", &DN2, 16);
    // Slaved to something else — it must not be fought over.
    dn2.io = DeviceIo { output: Some(port("Digitone II")), takes_clock: false, ..dn2.io };
    session.add_device(dn2);

    let (mut sched, ports) = prepared(&session);
    let events = run(&mut sched, &session, 0.5);
    let dn2_port = ports.get("Digitone II").unwrap();
    assert!(!events
        .iter()
        .any(|e| e.msg == MidiMsg::Clock && e.port == dn2_port));
    assert_eq!(events.iter().filter(|e| e.msg == MidiMsg::Clock).count(), 24);
}

#[test]
fn start_sends_stop_then_start_so_a_running_box_restarts_cleanly() {
    let (session, _) = one_box(&DT2, "DT2");
    let (sched, _) = prepared(&session);
    let mut out = Vec::new();
    sched.start_messages(0.0, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].msg, MidiMsg::Stop);
    assert_eq!(out[1].msg, MidiMsg::Start);
}

#[test]
fn clock_can_be_turned_off_without_stopping_the_notes() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let events = run(&mut sched, &session, 4.0 * STEP);
    assert!(!events.iter().any(|e| e.msg == MidiMsg::Clock));
    assert_eq!(note_ons(&events).len(), 1);
}

// --- mutes and solo ----------------------------------------------------------

#[test]
fn a_muted_track_is_silent_and_the_others_are_not() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    set_track(&mut session, d.id, 1, 4, vec![note_at(0.0, 72)]);
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        let p = session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap();
        p.track_mut(0).unwrap().mute = true;
    }
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let pitches: Vec<u8> = note_ons(&run(&mut sched, &session, STEP)).iter().map(|o| o.2).collect();
    assert_eq!(pitches, vec![72]);
}

#[test]
fn solo_is_session_wide_and_silences_the_other_box_too() {
    // PLAN.md §2: soloing a DT2 track silences DN2 tracks, which is the only
    // reading that makes sense at a mixing desk.
    let mut session = Session { tempo_bpm: BPM, ..Session::default() };
    let mut dt2 = Device::new("DT2", &DT2, 16);
    dt2.io = DeviceIo { output: Some(port("Digitakt II")), ..dt2.io };
    let dt2_id = session.add_device(dt2);
    let mut dn2 = Device::new("DN2", &DN2, 16);
    dn2.io = DeviceIo { output: Some(port("Digitone II")), ..dn2.io };
    let dn2_id = session.add_device(dn2);

    set_track(&mut session, dt2_id, 0, 4, vec![note_at(0.0, 36)]);
    set_track(&mut session, dn2_id, 0, 4, vec![note_at(0.0, 60)]);
    {
        let slot = session.slot_in_scene(0, dt2_id).unwrap().slot();
        let p = session.device_mut(dt2_id).unwrap().pattern_mut(slot).unwrap();
        p.track_mut(0).unwrap().solo = true;
    }

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    let pitches: Vec<u8> = note_ons(&run(&mut sched, &session, STEP)).iter().map(|o| o.2).collect();
    assert_eq!(pitches, vec![36], "the DN2 track is silenced by a DT2 solo");
}

// --- conditions, through the scheduler ---------------------------------------

#[test]
fn a_1st_trig_plays_on_the_first_pass_only() {
    let (mut session, d) = one_box(&DT2, "DT2");
    let mut first = note_at(0.0, 60);
    first.cond = Some("1ST".into());
    set_track(&mut session, d.id, 0, 2, vec![first]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let ons = note_ons(&run(&mut sched, &session, 8.0 * STEP));
    assert_eq!(ons.len(), 1, "four passes, one hit");
    assert!(close(ons[0].0, 0.0));
}

#[test]
fn a_chord_is_one_trig_and_takes_one_roll_of_the_dice() {
    // The deviation from `js/midi.js` that matters musically: it filters note by
    // note, so a 50% chord fires some of its notes. On the box, notes sharing a
    // step *are* one trig and take one roll — either the whole chord sounds or
    // none of it does.
    let (mut session, d) = one_box(&DT2, "DT2");
    let chord: Vec<Note> = [60u8, 64, 67]
        .iter()
        .map(|&p| {
            let mut n = note_at(0.0, p);
            n.prob = Some(50);
            n
        })
        .collect();
    set_track(&mut session, d.id, 0, 4, chord);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    // A run of many passes: every pass is all three notes or none.
    let mut out = Vec::new();
    let mut rng = XorShift64::new(99);
    sched.advance(&session, 64.0 * STEP, &mut rng, &NoPLocks, &mut out);
    let ons = note_ons(&out);
    let mut by_time = std::collections::BTreeMap::new();
    for (at, _, _) in &ons {
        *by_time.entry(format!("{at:.6}")).or_insert(0) += 1;
    }
    assert!(!by_time.is_empty(), "some passes must have played");
    assert!(
        by_time.values().all(|&n| n == 3),
        "a partial chord means the dice were rolled per note: {by_time:?}"
    );
    assert!(by_time.len() < 16, "at 50% not every pass should have fired");
}

#[test]
fn a_neighbour_condition_reads_the_track_below_it_on_the_same_box() {
    let (mut session, d) = one_box(&DT2, "DT2");
    // Track 0 carries a condition that is false after the first pass...
    let mut src = note_at(0.0, 60);
    src.cond = Some("1ST".into());
    set_track(&mut session, d.id, 0, 2, vec![src]);
    // ...and track 1 asks its neighbour.
    let mut nei = note_at(0.0, 72);
    nei.cond = Some("NEI".into());
    set_track(&mut session, d.id, 1, 2, vec![nei]);

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let ons = note_ons(&run(&mut sched, &session, 6.0 * STEP));
    let neighbour: Vec<f64> = ons.iter().filter(|o| o.2 == 72).map(|o| o.0 / STEP).collect();
    // Pass 0: track 0 evaluated 1ST as true first (tracks advance in order), so
    // NEI plays. Passes 1 and 2: 1ST is false, so NEI is silent.
    assert_eq!(neighbour, vec![0.0]);
}

// --- windows -----------------------------------------------------------------

#[test]
fn advancing_in_small_windows_gives_the_same_events_as_one_big_one() {
    // The property the transport thread relies on: how the timeline is chopped up
    // must not change what comes out of it.
    let build = || {
        let (mut session, d) = one_box(&DT2, "DT2");
        set_track(&mut session, d.id, 0, 3, (0..3).map(|s| note_at(s as f64, 60 + s)).collect());
        set_track(&mut session, d.id, 1, 5, (0..5).map(|s| note_at(s as f64, 72 + s)).collect());
        session
    };

    let session = build();
    let (mut one_shot, _) = prepared(&session);
    let whole = run(&mut one_shot, &session, 30.0 * STEP);

    let session2 = build();
    let (mut chopped, _) = prepared(&session2);
    let mut pieces = Vec::new();
    for i in 1..=30 {
        let mut out = Vec::new();
        let mut rng = ScriptedRng::always();
        chopped.advance(&session2, i as f64 * STEP, &mut rng, &NoPLocks, &mut out);
        pieces.extend(out);
    }

    let a: Vec<(String, usize, MidiMsg)> = whole
        .iter()
        .map(|e| (format!("{:.9}", e.at), e.port.0, e.msg))
        .collect();
    let b: Vec<(String, usize, MidiMsg)> = pieces
        .iter()
        .map(|e| (format!("{:.9}", e.at), e.port.0, e.msg))
        .collect();
    assert_eq!(a, b);
}

#[test]
fn preparing_again_keeps_each_track_where_it_was() {
    // Editing a note mid-play must not restart every track.
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    let (mut sched, mut ports) = prepared(&session);
    sched.send_clock = false;

    run(&mut sched, &session, 8.0 * STEP);
    let before: Vec<u64> = sched.cursors().iter().map(|c| c.next_step).collect();
    assert_eq!(before[0], 8);

    sched.prepare(&session, &mut ports);
    let after: Vec<u64> = sched.cursors().iter().map(|c| c.next_step).collect();
    assert_eq!(before, after);

    sched.rewind(&session);
    assert!(sched.cursors().iter().all(|c| c.next_step == 0));
}

#[test]
fn a_zero_length_track_plays_nothing_rather_than_dividing_by_zero() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 0, vec![note_at(0.0, 60)]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    assert!(note_ons(&run(&mut sched, &session, 4.0 * STEP)).is_empty());
}

#[test]
fn the_playhead_reports_where_a_track_is_within_its_pattern() {
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 3, vec![note_at(0.0, 60)]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    run(&mut sched, &session, 7.0 * STEP);
    let cursor = &sched.cursors()[0];
    assert_eq!(sched.step_in_pattern(&session, cursor), Some(7 % 3));
}

#[test]
fn p_lock_lanes_are_scheduled_ahead_of_the_trig_they_belong_to() {
    use digi_core::device::DeviceId;
    use digi_core::model::PLockLane;
    use digi_engine::scheduler::PLockMap;

    /// A stand-in for the unported parameter tables: one CC per lane.
    struct OneCcPerLane;
    impl PLockMap for OneCcPerLane {
        fn messages(
            &self,
            _: DeviceId,
            channel: u8,
            lanes: &[PLockLane],
            step: u64,
            out: &mut Vec<MidiMsg>,
        ) {
            for lane in lanes {
                if let Some(Some(v)) = lane.values.get(step as usize) {
                    out.push(MidiMsg::ControlChange {
                        channel,
                        controller: lane.param_id.unwrap_or(0) as u8,
                        value: (*v).min(127) as u8,
                    });
                }
            }
        }
    }

    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 4, vec![note_at(1.0, 60)]);
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        let p = session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap();
        let mut values = vec![None; 128];
        values[1] = Some(64);
        p.track_mut(0).unwrap().plocks =
            vec![PLockLane::new(Some("CUTOFF".into()), Some(74), None, false, values).unwrap()];
    }

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    let mut out = Vec::new();
    let mut rng = ScriptedRng::always();
    sched.advance(&session, 4.0 * STEP, &mut rng, &OneCcPerLane, &mut out);

    let cc = out
        .iter()
        .find(|e| matches!(e.msg, MidiMsg::ControlChange { controller: 74, .. }))
        .expect("the lane must have been scheduled");
    let on = out
        .iter()
        .find(|e| matches!(e.msg, MidiMsg::NoteOn { .. }))
        .unwrap();
    assert!(cc.at < on.at, "the parameter must land before the trig sounds");
    assert!(close(on.at - cc.at, 0.002), "2 ms ahead, as js/midi.js does");
}

#[test]
fn a_silenced_trig_does_not_apply_its_p_locks() {
    use digi_core::device::DeviceId;
    use digi_core::model::PLockLane;
    use digi_engine::scheduler::PLockMap;

    struct Always;
    impl PLockMap for Always {
        fn messages(&self, _: DeviceId, channel: u8, _: &[PLockLane], _: u64, out: &mut Vec<MidiMsg>) {
            out.push(MidiMsg::ControlChange { channel, controller: 74, value: 1 });
        }
    }

    let (mut session, d) = one_box(&DT2, "DT2");
    let mut never_plays = note_at(0.0, 60);
    never_plays.prob = Some(0);
    set_track(&mut session, d.id, 0, 4, vec![never_plays]);
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        let p = session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap();
        p.track_mut(0).unwrap().plocks =
            vec![PLockLane::new(Some("CUTOFF".into()), Some(74), None, false, vec![Some(1); 128]).unwrap()];
    }

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    let mut out = Vec::new();
    let mut rng = ScriptedRng::always();
    sched.advance(&session, 4.0 * STEP, &mut rng, &Always, &mut out);
    assert!(
        out.is_empty(),
        "a trig the dice silenced does not move the parameter on the box either"
    );
}

// ---------------------------------------------------------------------------
// The port table, which the UI has to build before there is a scheduler.
//
// `intern_ports` exists because the two halves of the seam need the same
// numbering at different moments: the UI opens one `midir` connection per id
// before spawning the engine, and every `PortId` in the queue is then an index
// into those connections. If a later `prepare` renumbered them, a track would
// send to the wrong box — the failure these pin down.
// ---------------------------------------------------------------------------

#[test]
fn a_devices_own_port_is_interned_before_any_track_that_overrides_it() {
    let (mut session, d) = one_box(&DT2, "DT2");
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        let p = session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap();
        p.track_mut(2).unwrap().out_port = Some("a synth".into());
        p.track_mut(5).unwrap().out_port = Some("another synth".into());
    }

    let mut ports = PortTable::new();
    digi_engine::scheduler::intern_ports(&session, &mut ports);
    assert_eq!(ports.name(digi_engine::event::PortId(0)), Some("DT2"));
    assert_eq!(ports.name(digi_engine::event::PortId(1)), Some("a synth"));
    assert_eq!(ports.name(digi_engine::event::PortId(2)), Some("another synth"));
    assert_eq!(ports.len(), 3);
}

#[test]
fn two_boxes_on_one_cable_share_one_port_and_therefore_one_clock() {
    let (mut session, _) = one_box(&DT2, "shared");
    let mut dn2 = Device::new("DN2", &DN2, 16);
    dn2.io = DeviceIo { output: Some(port("shared")), takes_clock: true, ..dn2.io };
    session.add_device(dn2);

    let (mut sched, ports) = prepared(&session);
    assert_eq!(ports.len(), 1, "one name, one connection");

    // And one clock down it, not two: a box that got its own tick stream would
    // hear every tick twice and run at double tempo.
    sched.rewind(&session);
    let events = run(&mut sched, &session, STEP);
    let ticks = events.iter().filter(|e| e.msg == MidiMsg::Clock).count();
    assert_eq!(ticks, 6, "six ticks to a 16th, once each");
}

#[test]
fn re_preparing_never_renumbers_a_table_the_caller_already_opened() {
    // The invariant the whole split rests on: the UI opens a sink indexed by
    // these ids, then keeps handing the same table back with each snapshot. A
    // re-prepare that reordered or re-interned would leave the queue addressing
    // connections by the wrong number, and a DT2 trig would come out of the DN2.
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 16, vec![note_at(0.0, 60)]);
    let (mut sched, mut ports) = prepared(&session);
    let before = ports.clone();

    // Every kind of edit the UI can make between two snapshots.
    set_track(&mut session, d.id, 0, 12, vec![note_at(0.0, 60), note_at(3.0, 64)]);
    session.tempo_bpm = 145.0;
    sched.prepare(&session, &mut ports);
    assert_eq!(ports, before);

    // A device losing its port does not compact the table either — the ids that
    // are left still mean what the open connections mean.
    session.devices[0].io.output = None;
    sched.prepare(&session, &mut ports);
    assert_eq!(ports, before);
    assert!(
        sched.cursors().iter().all(|c| c.port.is_none()),
        "with no port the tracks go silent rather than landing on port 0"
    );
}

#[test]
fn a_session_with_no_ports_at_all_interns_nothing() {
    // What the app opens with, before any box has been identified: it must be
    // able to spawn an engine and move a playhead against no connections.
    let session = digi_core::two_box_session();
    let mut ports = PortTable::new();
    digi_engine::scheduler::intern_ports(&session, &mut ports);
    assert!(ports.is_empty());
}

// ---------------------------------------------------------------------------
// Scenes: a change is a queued command, taken at the next boundary of the
// outgoing scene (PLAN.md §4).
//
// No oracle here either, and less than anywhere else: `js/midi.js` plays one
// pattern of one box and has no concept of a scene at all, which is exactly why
// its `LST` is documented as unsimulable. These come from PLAN.md §4's three
// sentences on the subject — queued, at the boundary of the longest track across
// all devices, with an immediate setting — plus one thing the hardware settles:
// a pattern change restarts the sequencer at step 1, which is what
// `TrackCursor::origin` is for.
// ---------------------------------------------------------------------------

/// Put `device` on `slot` in `scene`, adding scenes until there are that many.
fn scene_slot(session: &mut Session, scene: usize, device: digi_core::DeviceId, slot: usize) {
    while session.scenes.len() <= scene {
        session.add_scene(format!("Scene {}", session.scenes.len() + 1), Some(0));
    }
    assert!(session.set_slot_in_scene(scene, device, digi_core::PatternRef::from_slot(slot)));
}

/// Shape one slot: **every** track set to `length`, and `notes` on track 0.
///
/// Every track, because the boundary is the longest track in the scene and a
/// pattern's other fifteen tracks wrap whether or not they hold trigs — exactly
/// as they do on the box. Shaping only track 0 would leave fifteen 16-step tracks
/// deciding the boundary, which is what these tests found the first time round.
fn shape_slot(
    session: &mut Session,
    device: digi_core::DeviceId,
    slot: usize,
    length: u16,
    notes: Vec<Note>,
) {
    let pattern = session.device_mut(device).unwrap().pattern_mut(slot).unwrap();
    for t in 0..pattern.num_tracks() {
        pattern.track_mut(t).unwrap().length_steps = length;
    }
    pattern.track_mut(0).unwrap().notes = notes;
}

#[test]
fn a_scene_cycle_is_the_longest_track_in_seconds_not_in_steps() {
    // At 1x this is `scene_boundary_steps` in the unit the engine needs, and the
    // two must agree exactly — that is what makes it the same rule.
    let (mut session, d) = one_box(&DT2, "DT2");
    set_track(&mut session, d.id, 0, 12, vec![]);
    set_track(&mut session, d.id, 1, 48, vec![]);
    assert_eq!(session.scene_boundary_steps(0), Some(48));
    let cycle = scene_cycle_seconds(&session, 0, BPM).expect("a boundary");
    assert!(close(cycle, 48.0 * STEP), "{cycle}");

    // And where they disagree, seconds is the answer that is right. A 16-step
    // track at 1/8 scale runs eight times slower than a 48-step track at 1x, so
    // it is the longer cycle by a distance — while being a third of the length by
    // step count, which is all the model can see.
    {
        let slot = session.slot_in_scene(0, d.id).unwrap().slot();
        let p = session.device_mut(d.id).unwrap().pattern_mut(slot).unwrap();
        let t = p.track_mut(2).unwrap();
        t.length_steps = 16;
        t.scale = TrackScale::Eighth;
    }
    assert_eq!(session.scene_boundary_steps(0), Some(48), "still 48 by step count");
    let cycle = scene_cycle_seconds(&session, 0, BPM).expect("a boundary");
    assert!(close(cycle, 16.0 * STEP * 8.0), "{cycle}");
}

#[test]
fn an_empty_scene_has_no_boundary_to_wait_for() {
    let (mut session, d) = one_box(&DT2, "DT2");
    for t in 0..16 {
        set_track(&mut session, d.id, t, 0, vec![]);
    }
    assert_eq!(scene_cycle_seconds(&session, 0, BPM), None);

    // So a switch out of it is taken at once rather than queued forever.
    scene_slot(&mut session, 1, d.id, 1);
    let (mut sched, _) = prepared(&session);
    sched.queue_scene(&session, 1, 0.0, false);
    assert_eq!(sched.pending_scene(), None);
    assert_eq!(sched.scene(), 1);
}

#[test]
fn a_queued_scene_is_taken_at_the_boundary_and_not_before() {
    // Slot 0 plays C on every step, slot 1 plays G. A four-step track makes the
    // boundary half a second at 120 bpm, so the switch is easy to point at.
    let (mut session, d) = one_box(&DT2, "DT2");
    shape_slot(&mut session, d.id, 0, 4, (0..4).map(|s| note_at(s as f64, 60)).collect());
    shape_slot(&mut session, d.id, 1, 4, (0..4).map(|s| note_at(s as f64, 67)).collect());
    scene_slot(&mut session, 1, d.id, 1);

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    // Asked for one step in, it lands on the next multiple of four steps.
    sched.queue_scene(&session, 1, STEP, false);
    assert_eq!(sched.pending_scene(), Some(1));
    assert!(close(sched.scene_switch_at().expect("a boundary"), 4.0 * STEP));

    let events = run(&mut sched, &session, 8.0 * STEP);
    let pitches: Vec<u8> = note_ons(&events).iter().map(|(_, _, p)| *p).collect();
    assert_eq!(
        pitches,
        vec![60, 60, 60, 60, 67, 67, 67, 67],
        "the outgoing scene finished its pass, and the incoming one started at its own step 1"
    );
    assert_eq!(sched.scene(), 1);
    assert_eq!(sched.pending_scene(), None);
}

#[test]
fn a_trig_landing_exactly_on_the_boundary_belongs_to_the_incoming_scene() {
    // The `<=` in `advance`, stated on its own. A trig on the boundary *is* step
    // 1 of the new pattern; playing the outgoing scene's version of it would be
    // one step of the wrong pattern, every switch.
    let (mut session, d) = one_box(&DT2, "DT2");
    shape_slot(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    shape_slot(&mut session, d.id, 1, 4, vec![note_at(0.0, 67)]);
    scene_slot(&mut session, 1, d.id, 1);

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    sched.queue_scene(&session, 1, 0.0, false);
    assert!(close(sched.scene_switch_at().expect("a boundary"), 0.0));

    let events = run(&mut sched, &session, 4.0 * STEP);
    assert_eq!(
        note_ons(&events).iter().map(|(_, _, p)| *p).collect::<Vec<u8>>(),
        vec![67],
        "the step on the boundary came from the scene that was switched to"
    );
}

#[test]
fn the_immediate_setting_does_not_wait_for_the_boundary() {
    let (mut session, d) = one_box(&DT2, "DT2");
    shape_slot(&mut session, d.id, 0, 4, (0..4).map(|s| note_at(s as f64, 60)).collect());
    shape_slot(&mut session, d.id, 1, 4, (0..4).map(|s| note_at(s as f64, 67)).collect());
    scene_slot(&mut session, 1, d.id, 1);

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    let events = run(&mut sched, &session, 2.0 * STEP);
    assert_eq!(note_ons(&events).len(), 2, "two steps of the outgoing scene");

    // Immediate is taken at the horizon, because what is already dated cannot be
    // taken back off the wire — and that is all it can mean.
    sched.queue_scene(&session, 1, 2.0 * STEP, true);
    assert_eq!(sched.scene(), 1);
    assert_eq!(sched.pending_scene(), None);
    let events = run(&mut sched, &session, 4.0 * STEP);
    assert_eq!(
        note_ons(&events).iter().map(|(_, _, p)| *p).collect::<Vec<u8>>(),
        vec![67, 67],
        "mid-pass, without finishing the outgoing pattern"
    );
}

#[test]
fn the_incoming_pattern_starts_at_its_own_step_one() {
    // What a pattern change does on the box. It matters most when the two
    // patterns are different lengths: the absolute counter dates every event and
    // cannot go backwards, so the *pattern* is moved to meet it. Without that,
    // a 4-step pattern switched to at second 5 would come in at step 4 of its own
    // cycle, which is a different bar of music.
    let (mut session, d) = one_box(&DT2, "DT2");
    shape_slot(&mut session, d.id, 0, 6, vec![note_at(0.0, 60)]);
    shape_slot(&mut session, d.id, 1, 4, vec![note_at(0.0, 67), note_at(1.0, 68)]);
    scene_slot(&mut session, 1, d.id, 1);

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    sched.queue_scene(&session, 1, STEP, false);
    // Six steps is the outgoing cycle, so the switch is at step 6.
    assert!(close(sched.scene_switch_at().expect("a boundary"), 6.0 * STEP));

    let events = run(&mut sched, &session, 10.0 * STEP);
    let ons = note_ons(&events);
    assert_eq!(ons.iter().map(|(_, _, p)| *p).collect::<Vec<u8>>(), vec![60, 67, 68]);
    assert!(close(ons[1].0, 6.0 * STEP), "step 1 of the new pattern is at the boundary");
    assert!(close(ons[2].0, 7.0 * STEP), "and step 2 one step later");
}

#[test]
fn a_scene_change_makes_1st_fire_again_and_forgets_the_condition_history() {
    // A pattern change restarts the pattern, so `1ST` is true again — and a `PRE`
    // chain from the outgoing scene must not decide the first bar of the incoming
    // one. `CondHistory::clear` says the same thing in its own doc comment.
    let (mut session, d) = one_box(&DT2, "DT2");
    let first_only = |pitch: u8| {
        let mut n = note_at(0.0, pitch);
        n.cond = Some("1ST".into());
        n
    };
    shape_slot(&mut session, d.id, 0, 4, vec![first_only(60)]);
    shape_slot(&mut session, d.id, 1, 4, vec![first_only(67)]);
    scene_slot(&mut session, 1, d.id, 1);

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    sched.queue_scene(&session, 1, STEP, false);

    let events = run(&mut sched, &session, 12.0 * STEP);
    assert_eq!(
        note_ons(&events).iter().map(|(_, _, p)| *p).collect::<Vec<u8>>(),
        vec![60, 67],
        "1ST fired once in the outgoing scene and once more when the new pattern began"
    );
}

#[test]
fn a_scene_switch_waits_for_the_longest_track_across_both_boxes() {
    // The reason the boundary is a session-wide question and not a per-track one:
    // a polymetric track must not be cut mid-cycle, and the longest one may be on
    // the other box entirely.
    let (mut session, dt2) = one_box(&DT2, "DT2");
    let mut dn2 = Device::new("DN2", &DN2, 16);
    dn2.io = DeviceIo { output: Some(port("DN2")), takes_clock: true, ..dn2.io };
    let dn2 = session.add_device(dn2);

    shape_slot(&mut session, dt2.id, 0, 4, vec![note_at(0.0, 60)]);
    shape_slot(&mut session, dn2, 0, 7, vec![note_at(0.0, 40)]);
    scene_slot(&mut session, 1, dt2.id, 1);
    scene_slot(&mut session, 1, dn2, 1);

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    sched.queue_scene(&session, 1, STEP, false);
    assert!(
        close(sched.scene_switch_at().expect("a boundary"), 7.0 * STEP),
        "the DN2's seven steps, not the DT2's four"
    );
}

#[test]
fn a_scene_asked_for_inside_the_committed_window_takes_the_following_boundary() {
    // Nothing can be taken back off the wire. The transport has up to 50 ms of
    // the outgoing scene already dated, so a switch behind that horizon would
    // mean unpicking events that are on their way out — it takes the next
    // boundary instead, which is a lot better than half a bar of the wrong
    // pattern.
    let (mut session, d) = one_box(&DT2, "DT2");
    shape_slot(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    scene_slot(&mut session, 1, d.id, 1);
    let (mut sched, _) = prepared(&session);

    // Asked for at 4.1 steps, a hair past the boundary it "should" have taken.
    sched.queue_scene(&session, 1, 4.1 * STEP, false);
    assert!(close(sched.scene_switch_at().expect("a boundary"), 8.0 * STEP));
}

#[test]
fn a_scene_this_session_does_not_have_is_ignored_rather_than_clamped() {
    let (mut session, d) = one_box(&DT2, "DT2");
    shape_slot(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    scene_slot(&mut session, 2, d.id, 1);
    let (mut sched, _) = prepared(&session);

    sched.queue_scene(&session, 9, 0.0, false);
    assert_eq!(sched.pending_scene(), None, "no queue for a scene that is not there");
    assert_eq!(sched.scene(), 0, "and nothing moved");

    // A scene deleted while its switch was queued cannot leave a pending that
    // `advance` retries for ever — which is a spin, not a silence.
    sched.queue_scene(&session, 2, 0.0, false);
    assert_eq!(sched.pending_scene(), Some(2));
    session.remove_scene(2);
    let events = run(&mut sched, &session, 8.0 * STEP);
    assert_eq!(sched.pending_scene(), None);
    assert_eq!(sched.scene(), 0, "it stayed where it was");
    assert!(!note_ons(&events).is_empty(), "and kept playing");
}

#[test]
fn a_scene_can_route_a_track_somewhere_else_and_the_switch_follows_it() {
    // Two scenes on one box, one of them sending track 1 to a different port.
    // The port table is interned across every scene precisely so this needs no
    // new connection at the boundary.
    let (mut session, d) = one_box(&DT2, "DT2");
    shape_slot(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    shape_slot(&mut session, d.id, 1, 4, vec![note_at(0.0, 67)]);
    {
        let p = session.device_mut(d.id).unwrap().pattern_mut(1).unwrap();
        let t = p.track_mut(0).unwrap();
        t.out_port = Some("a synth".into());
        t.channel = 9;
    }
    scene_slot(&mut session, 1, d.id, 1);

    let (mut sched, ports) = prepared(&session);
    sched.send_clock = false;
    assert_eq!(ports.len(), 2, "both scenes' ports, before either is playing");

    sched.queue_scene(&session, 1, STEP, false);
    let events = run(&mut sched, &session, 8.0 * STEP);
    let ons = note_ons(&events);
    assert_eq!(ons[0], (0.0, 0, 60));
    assert!(close(ons[1].0, 4.0 * STEP));
    assert_eq!((ons[1].1, ons[1].2), (1, 67), "the incoming scene's own port");
    assert!(
        matches!(events.iter().find_map(|e| match e.msg {
            MidiMsg::NoteOn { channel, pitch: 67, .. } => Some(channel),
            _ => None,
        }), Some(9)),
        "and its own channel"
    );
}

// --- song mode ---------------------------------------------------------------
//
// PLAN.md §6 phase 12. A row names a scene, so these are all about *when* the
// walker commits the next one, which is the only thing the song decides.
//
// The fixture throughout: two scenes, each a different pitch on step 0 of a
// 4-step track, so which scene is sounding is readable off the pitches alone.

/// Two scenes on one box: scene 0 plays `60` and scene 1 plays `72`, each on
/// step 0 of a 4-step track — one bar of four 16ths, `4 * STEP` long.
fn two_scenes() -> (Session, digi_core::device::DeviceId) {
    let (mut session, d) = one_box(&DT2, "DT2");
    // Scene 0 is on slot 0; point scene 1 at slot 1 so the two play different
    // patterns rather than the same one twice.
    let scene1 = session.add_scene("Scene 2", Some(0));
    session.set_slot_in_scene(scene1, d.id, digi_core::PatternRef::new(0, 1));

    // **Every other track is zeroed, and that is not tidiness.** A row with no
    // length of its own lasts its scene's own cycle, which is the *longest* track
    // in it — and a fresh pattern's fifteen untouched tracks are 16 steps each.
    // Left alone they would set every row boundary in this section, four times
    // later than the test that reads it says.
    for scene in [0, scene1] {
        session.current_scene = scene;
        for t in 1..16 {
            set_track(&mut session, d.id, t, 0, vec![]);
        }
    }

    session.current_scene = 0;
    set_track(&mut session, d.id, 0, 4, vec![note_at(0.0, 60)]);
    // `set_track` writes the pattern the *current* scene names, so move the
    // session onto scene 1 to fill its slot in.
    session.current_scene = scene1;
    set_track(&mut session, d.id, 0, 4, vec![note_at(0.0, 72)]);
    session.current_scene = 0;
    (session, d.id)
}

fn pitches(events: &[ScheduledEvent]) -> Vec<(f64, u8)> {
    note_ons(events).iter().map(|(at, _, p)| (*at, *p)).collect()
}

fn song_mode(session: &Session) -> (Scheduler, PortTable) {
    let (mut sched, ports) = prepared(session);
    sched.send_clock = false;
    sched.set_song_mode(session, true, 0, 0.0);
    (sched, ports)
}

#[test]
fn a_song_walks_its_rows_in_order() {
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    // Two bars: row 1 then row 2, then END: LOOP back to row 1.
    let events = run(&mut sched, &session, 12.0 * STEP);
    assert_eq!(
        pitches(&events),
        vec![(0.0, 60), (4.0 * STEP, 72), (8.0 * STEP, 60)]
    );
}

#[test]
fn the_song_pointer_says_which_row_and_which_pass() {
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().repeats = 2;
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    assert_eq!(sched.song_position(), Some((0, 0)));
    run(&mut sched, &session, 5.0 * STEP);
    assert_eq!(sched.song_position(), Some((0, 1)), "second pass of row 1");
    run(&mut sched, &session, 9.0 * STEP);
    assert_eq!(sched.song_position(), Some((1, 0)));
}

#[test]
fn a_row_play_count_repeats_the_row_before_the_song_moves_on() {
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().repeats = 3;
    session.add_song_row(1).unwrap();
    session.song_mut().end = digi_core::EndAction::Stop;
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 20.0 * STEP);
    // Three bars of scene 0, then one of scene 1, then nothing.
    assert_eq!(
        pitches(&events),
        vec![(0.0, 60), (4.0 * STEP, 60), (8.0 * STEP, 60), (12.0 * STEP, 72)]
    );
}

#[test]
fn a_row_length_cuts_the_pattern_short() {
    // ROW LENGTH in reference steps: two of the four steps, so the second row
    // arrives half a bar early.
    let (mut session, d) = two_scenes();
    set_track(&mut session, d, 0, 4, (0..4).map(|s| note_at(s as f64, 60 + s)).collect());
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().length_steps = Some(2);
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 3.0 * STEP);
    // Steps 0 and 1 of scene 0, and then scene 1 rather than steps 2 and 3.
    assert_eq!(pitches(&events), vec![(0.0, 60), (STEP, 61), (2.0 * STEP, 72)]);
}

#[test]
fn a_row_with_no_length_of_its_own_lasts_the_longest_track_in_its_scene() {
    // The default: `None` means the scene's own cycle, which under polymeter is
    // the longest track — the same boundary a queued scene change waits for.
    let (mut session, d) = two_scenes();
    set_track(&mut session, d, 1, 7, vec![note_at(0.0, 40)]);
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 8.0 * STEP);
    // Seven steps, not four: track 2 is the longest in the scene.
    let scene_two = pitches(&events).into_iter().find(|(_, p)| *p == 72);
    assert_eq!(scene_two, Some((7.0 * STEP, 72)));
}

#[test]
fn a_row_length_in_reference_steps_ignores_track_scale() {
    // The unit is a 1/16 at 1x, session-wide — the same grid the clock and the
    // position readout run on. A 2x track wrapping twice inside the row does not
    // move the row's boundary.
    let (mut session, d) = two_scenes();
    let slot = session.slot_in_scene(0, d).unwrap().slot();
    session
        .device_mut(d)
        .unwrap()
        .pattern_mut(slot)
        .unwrap()
        .track_mut(0)
        .unwrap()
        .scale = TrackScale::Two;
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().length_steps = Some(4);
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 5.0 * STEP);
    let switch = pitches(&events).into_iter().find(|(_, p)| *p == 72);
    assert_eq!(switch, Some((4.0 * STEP, 72)), "four reference steps, not eight");
}

#[test]
fn end_loop_goes_round_again_and_end_stop_does_not() {
    for (end, expected) in [
        (digi_core::EndAction::Loop, 3),
        (digi_core::EndAction::Stop, 1),
    ] {
        let (mut session, _) = two_scenes();
        session.add_song_row(0).unwrap();
        session.song_mut().end = end;
        let (mut sched, _) = song_mode(&session);

        let events = run(&mut sched, &session, 12.0 * STEP);
        assert_eq!(pitches(&events).len(), expected, "{end:?}");
    }
}

#[test]
fn end_stop_records_the_moment_rather_than_stopping_anything_itself() {
    // The scheduler has no thread and no `Instant`, so it cannot stop the
    // transport. It says when, and nothing plays past it.
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.song_mut().end = digi_core::EndAction::Stop;
    let (mut sched, _) = song_mode(&session);

    run(&mut sched, &session, 12.0 * STEP);
    assert_eq!(sched.song_stop_at(), Some(4.0 * STEP));
    // And a second window past the end adds nothing at all.
    let after = run(&mut sched, &session, 40.0 * STEP);
    assert!(pitches(&after).is_empty());
}

#[test]
fn a_stopped_song_plays_again_from_the_top_after_a_rewind() {
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.song_mut().end = digi_core::EndAction::Stop;
    let (mut sched, _) = song_mode(&session);
    run(&mut sched, &session, 12.0 * STEP);
    assert!(sched.song_stop_at().is_some());

    sched.rewind(&session);
    assert_eq!(sched.song_stop_at(), None);
    assert_eq!(sched.song_position(), Some((0, 0)));
    assert_eq!(pitches(&run(&mut sched, &session, STEP)), vec![(0.0, 60)]);
}

#[test]
fn the_clock_stops_with_the_song_rather_than_running_on() {
    // A stopped song must not be followed by 50 ms of clock still telling the
    // boxes to run — the reason `emit_clock` happens after the walk.
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.song_mut().end = digi_core::EndAction::Stop;
    let (mut sched, ports) = prepared(&session);
    let _ = ports;
    sched.set_song_mode(&session, true, 0, 0.0);

    let events = run(&mut sched, &session, 12.0 * STEP);
    let last_tick = events
        .iter()
        .filter(|e| e.msg == MidiMsg::Clock)
        .map(|e| e.at)
        .fold(0.0f64, f64::max);
    assert!(last_tick < 4.0 * STEP, "last tick at {last_tick}");
}

#[test]
fn a_row_whose_scene_has_no_playable_track_parks_rather_than_spinning() {
    // `scene_cycle_seconds` cannot answer for it, and advancing through it would
    // advance through every row in one pass and then keep going.
    let (mut session, d) = two_scenes();
    set_track(&mut session, d, 0, 0, vec![]);
    for t in 0..16 {
        set_track(&mut session, d, t, 0, vec![]);
    }
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    run(&mut sched, &session, 64.0 * STEP);
    assert_eq!(sched.song_position(), Some((0, 0)), "still on row 1");
}

#[test]
fn leaving_song_mode_keeps_the_scene_the_last_row_put_up() {
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);
    run(&mut sched, &session, 5.0 * STEP);
    assert_eq!(sched.scene(), 1);

    sched.set_song_mode(&session, false, 0, 5.0 * STEP);
    assert_eq!(sched.song_position(), None);
    // Still on scene 2, so SONG → PATTERN is a way to stay on a section.
    assert_eq!(sched.scene(), 1);
    let events = run(&mut sched, &session, 9.0 * STEP);
    assert_eq!(pitches(&events), vec![(8.0 * STEP, 72)]);
}

#[test]
fn a_jump_moves_the_pointer_and_the_sound_together() {
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    sched.jump_to_row(&session, 1, 0.0);
    assert_eq!(sched.song_position(), Some((1, 0)));
    assert_eq!(pitches(&run(&mut sched, &session, STEP)), vec![(0.0, 72)]);
}

#[test]
fn a_jump_in_pattern_mode_is_ignored() {
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    let (mut sched, _) = prepared(&session);
    sched.jump_to_row(&session, 0, 0.0);
    assert_eq!(sched.song_position(), None);
}

#[test]
fn the_walker_survives_the_row_it_is_on_being_deleted() {
    // A snapshot arrives whenever the user touches anything, including the song.
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();
    let (mut sched, mut ports) = song_mode(&session);
    run(&mut sched, &session, 5.0 * STEP);
    assert_eq!(sched.song_position(), Some((1, 0)));

    session.song_mut().remove(1);
    sched.prepare(&session, &mut ports);
    // Clamped, not restarted: the set does not jump back to row 1 mid-bar.
    assert_eq!(sched.song_position(), Some((0, 0)));
}

#[test]
fn emptying_the_song_leaves_the_walker_with_nothing_to_point_at() {
    let (mut session, _) = two_scenes();
    session.add_song_row(0).unwrap();
    let (mut sched, mut ports) = song_mode(&session);
    session.song = None;
    sched.prepare(&session, &mut ports);
    assert_eq!(sched.song_position(), None);
    // Still in song mode, so refilling the song starts walking again.
    assert!(sched.song_mode());
}

#[test]
fn a_row_mute_silences_a_track_the_pattern_plays() {
    let (mut session, d) = two_scenes();
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().set_mute(d, 0, true);
    let (mut sched, _) = song_mode(&session);

    assert!(pitches(&run(&mut sched, &session, STEP)).is_empty());
}

#[test]
fn a_row_mute_sounds_a_track_the_pattern_mutes() {
    // A substitution, not a second mute stage.
    let (mut session, d) = two_scenes();
    let slot = session.slot_in_scene(0, d).unwrap().slot();
    session.device_mut(d).unwrap().pattern_mut(slot).unwrap().track_mut(0).unwrap().mute = true;
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().set_mute(d, 0, false);
    let (mut sched, _) = song_mode(&session);

    assert_eq!(pitches(&run(&mut sched, &session, STEP)), vec![(0.0, 60)]);
}

#[test]
fn a_row_saying_nothing_about_a_box_leaves_the_patterns_mutes_alone() {
    let (mut session, d) = two_scenes();
    let slot = session.slot_in_scene(0, d).unwrap().slot();
    session.device_mut(d).unwrap().pattern_mut(slot).unwrap().track_mut(0).unwrap().mute = true;
    // The row is built by hand, so it has no mask at all.
    session.song_mut().push(digi_core::SongRow::new(0)).unwrap();
    let (mut sched, _) = song_mode(&session);

    assert!(pitches(&run(&mut sched, &session, STEP)).is_empty());
}

// --- LST ---------------------------------------------------------------------

#[test]
fn lst_fires_on_the_last_pass_of_a_row_and_not_before() {
    // PLAN.md §4 listed `LST` as unsimulated "until pattern chaining exists".
    let (mut session, d) = two_scenes();
    let mut note = note_at(0.0, 60);
    note.cond = Some("LST".into());
    set_track(&mut session, d, 0, 4, vec![note]);
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().repeats = 3;
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    // Thirteen steps, not twelve: a window's far edge is exclusive, and row 2's
    // first trig falls exactly on step 12.
    let events = run(&mut sched, &session, 13.0 * STEP);
    // Three passes of row 1, and the trig only on the third.
    assert_eq!(
        pitches(&events),
        vec![(8.0 * STEP, 60), (12.0 * STEP, 72)]
    );
}

#[test]
fn lst_is_per_track_not_per_row() {
    // A 2-step track has two passes inside a 4-step row; only its second is the
    // last before the scene changes.
    let (mut session, d) = two_scenes();
    let mut note = note_at(0.0, 50);
    note.cond = Some("LST".into());
    set_track(&mut session, d, 1, 2, vec![note]);
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 4.0 * STEP);
    let lst: Vec<f64> = pitches(&events).into_iter().filter(|(_, p)| *p == 50).map(|(at, _)| at).collect();
    assert_eq!(lst, vec![2.0 * STEP]);
}

#[test]
fn lst_in_pattern_mode_plays_because_nothing_can_answer_it() {
    // The rule that outlives every extension: anything unsimulated plays, so the
    // sequencer is never quieter than the box.
    let (mut session, d) = two_scenes();
    let mut note = note_at(0.0, 60);
    note.cond = Some("LST".into());
    set_track(&mut session, d, 0, 4, vec![note]);
    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;

    let events = run(&mut sched, &session, 9.0 * STEP);
    assert_eq!(pitches(&events).len(), 3, "every pass plays");
}

#[test]
fn a_negated_lst_plays_every_pass_but_the_last() {
    let (mut session, d) = two_scenes();
    let mut note = note_at(0.0, 60);
    note.cond = Some("!LST".into());
    set_track(&mut session, d, 0, 4, vec![note]);
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().repeats = 3;
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 12.0 * STEP);
    let ons: Vec<f64> = pitches(&events).into_iter().filter(|(_, p)| *p == 60).map(|(at, _)| at).collect();
    assert_eq!(ons, vec![0.0, 4.0 * STEP]);
}

#[test]
fn a_repeat_of_a_full_length_row_does_not_re_fire_1st() {
    // The rule the repeats hang off: a row that plays four times is four passes
    // of one pattern, exactly as on the box, so `1ST` fires once.
    let (mut session, d) = two_scenes();
    let mut note = note_at(0.0, 60);
    note.cond = Some("1ST".into());
    set_track(&mut session, d, 0, 4, vec![note]);
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().repeats = 3;
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 12.0 * STEP);
    assert_eq!(pitches(&events), vec![(0.0, 60)]);
}

#[test]
fn a_repeat_of_a_truncated_row_does_re_fire_1st() {
    // The other half of the same rule: an explicit ROW LENGTH cuts the pattern
    // short, so each repeat genuinely re-launches it.
    let (mut session, d) = two_scenes();
    let mut note = note_at(0.0, 60);
    note.cond = Some("1ST".into());
    set_track(&mut session, d, 0, 4, vec![note]);
    session.add_song_row(0).unwrap();
    session.song_mut().row_mut(0).unwrap().repeats = 3;
    session.song_mut().row_mut(0).unwrap().length_steps = Some(2);
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 6.0 * STEP);
    assert_eq!(pitches(&events), vec![(0.0, 60), (2.0 * STEP, 60), (4.0 * STEP, 60)]);
}

#[test]
fn a_new_row_restarts_every_track_at_its_own_step_one() {
    // What `commit_scene` has always done, now reached from the walker: the
    // absolute counter dates every event and cannot go backwards, so the pattern
    // is moved to meet it.
    let (mut session, d) = two_scenes();
    // A 3-step track, so a row boundary at step 4 falls mid-pattern for it.
    set_track(&mut session, d, 1, 3, (0..3).map(|s| note_at(s as f64, 40 + s)).collect());
    session.add_song_row(0).unwrap();
    session.add_song_row(0).unwrap();
    let (mut sched, _) = song_mode(&session);

    let events = run(&mut sched, &session, 6.0 * STEP);
    let low: Vec<(f64, u8)> = pitches(&events).into_iter().filter(|(_, p)| *p < 50).collect();
    // Steps 1, 2, 3, then the track's *own* wrap back to step 1 at step 4 — and
    // then the row boundary at step 5 restarting it at step 1 again rather than
    // letting it carry on to step 2, which is the assertion that matters.
    assert_eq!(
        low,
        vec![
            (0.0, 40),
            (STEP, 41),
            (2.0 * STEP, 42),
            (3.0 * STEP, 40),
            (4.0 * STEP, 40),
            (5.0 * STEP, 41),
        ]
    );
}

#[test]
fn a_scene_switch_between_two_scales_lands_where_it_was_taken() {
    // **Found by a song-mode test on 2026-08-22, and it is not a song-mode bug.**
    // A step's length is per track — SCALE — so dating events as
    // `next_step × step_seconds` reads the incoming pattern's step length off the
    // outgoing pattern's step count. A 2x track switching onto a 1x one four
    // steps in put the incoming pattern's step 1 at eight steps: half a bar of
    // silence, in pattern mode, with nothing but a scene pill involved.
    let (mut session, d) = two_scenes();
    let slot = session.slot_in_scene(0, d).unwrap().slot();
    session
        .device_mut(d)
        .unwrap()
        .pattern_mut(slot)
        .unwrap()
        .track_mut(0)
        .unwrap()
        .scale = TrackScale::Two;

    let (mut sched, _) = prepared(&session);
    sched.send_clock = false;
    // Four reference steps in — eight of the 2x track's own steps — switch.
    run(&mut sched, &session, 4.0 * STEP);
    sched.commit_scene(&session, 1);

    let events = run(&mut sched, &session, 5.0 * STEP);
    assert_eq!(
        pitches(&events).first().copied(),
        Some((4.0 * STEP, 72)),
        "the incoming pattern starts at the switch, not at twice the step count"
    );
}
