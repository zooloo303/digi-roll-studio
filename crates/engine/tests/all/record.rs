//! Where a played note lands — PLAN.md §12.4.2, through the
//! scheduler.
//!
//! `engine::record`'s own unit tests pin the arithmetic in isolation: nearest
//! step, the negative micro at a wrap, quantize. What only this file can say is
//! that [`Scheduler::place_live`] finds the *right* three numbers to hand it —
//! the armed track's `origin_at`, its SCALE and its `length_steps` — because
//! every one of those is per track and none of them is the transport bar's
//! readout.
//!
//! **There is no JS oracle for any of it.** `js/midi.js` never listened to an
//! input, so nothing in the original has an opinion about which step a played
//! note belongs to. These come from the design and from what the box does.
//!
//! No thread, no `Instant`, no port. Seconds since the transport started, the
//! same footing `scheduler.rs` is on.

use digi_core::device::{Device, DeviceIo, DeviceId, PortRef, DT2};
use digi_core::model::TrackScale;
use digi_core::session::Session;
use digi_engine::event::PortTable;
use digi_engine::scheduler::Scheduler;

const BPM: f64 = 120.0;
/// A 16th at 120 bpm, which is a 1x track's step.
const STEP: f64 = 0.125;

fn one_box() -> (Session, DeviceId) {
    let mut device = Device::new("DT2", &DT2, 16);
    device.io = DeviceIo {
        output: Some(PortRef { id: "id-DT2".into(), name: "DT2".into() }),
        ..device.io
    };
    let id = device.id;
    let mut session = Session { tempo_bpm: BPM, ..Session::default() };
    session.add_device(device);
    (session, id)
}

/// Shape one track of the slot a scene points at: its length and its SCALE.
fn shape(
    session: &mut Session,
    device: DeviceId,
    slot: usize,
    track: usize,
    length: u16,
    scale: TrackScale,
) {
    let t = session
        .device_mut(device)
        .expect("the device is in the session")
        .pattern_mut(slot)
        .expect("the slot exists")
        .track_mut(track)
        .expect("the model has this track");
    t.length_steps = length;
    t.scale = scale;
}

fn prepared(session: &Session) -> Scheduler {
    let mut ports = PortTable::new();
    let mut s = Scheduler::new(BPM);
    s.prepare(session, &mut ports);
    s
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn a_note_played_on_the_beat_lands_on_that_step() {
    let (mut session, id) = one_box();
    shape(&mut session, id, 0, 0, 16, TrackScale::One);
    let sched = prepared(&session);

    let p = sched
        .place_live(&session, id, 0, 4.0 * STEP, false)
        .expect("the track is being played, so it has a cursor");
    assert_eq!((p.step, p.pass), (4, 0));
    assert!(close(p.micro, 0.0));
}

/// The rounding rule, stated where the track's own numbers come from: a hit
/// 30% of a step early is still aiming at that step. Rounding down instead
/// would put every human note one step early, every take.
#[test]
fn the_nearest_step_wins_rather_than_the_one_just_gone() {
    let (mut session, id) = one_box();
    shape(&mut session, id, 0, 0, 16, TrackScale::One);
    let sched = prepared(&session);

    let early = sched.place_live(&session, id, 0, 3.7 * STEP, false).expect("placed");
    assert_eq!(early.step, 4);
    assert!(close(early.micro, -0.3), "{}", early.micro);

    let late = sched.place_live(&session, id, 0, 4.3 * STEP, false).expect("placed");
    assert_eq!(late.step, 4);
    assert!(close(late.micro, 0.3), "{}", late.micro);
}

/// The design's named case. A hit just after the last step of a 16-step track
/// belongs to step 0 of the next pass, carrying a negative micro — which is
/// what the box does, and it is why `micro` is signed.
#[test]
fn a_hit_just_late_of_the_last_step_wraps_onto_the_next_pass() {
    let (mut session, id) = one_box();
    shape(&mut session, id, 0, 0, 16, TrackScale::One);
    let sched = prepared(&session);

    let p = sched.place_live(&session, id, 0, 15.8 * STEP, false).expect("placed");
    assert_eq!((p.step, p.pass), (0, 1));
    assert!(close(p.micro, -0.2), "{}", p.micro);
}

/// SCALE is read off the track, not assumed. A 2x track's steps are half as
/// long, so one second of playing covers twice as many of them.
#[test]
fn a_track_at_double_scale_counts_its_own_steps() {
    let (mut session, id) = one_box();
    shape(&mut session, id, 0, 0, 16, TrackScale::One);
    shape(&mut session, id, 0, 1, 16, TrackScale::Two);
    let sched = prepared(&session);

    let at = 4.0 * STEP;
    let plain = sched.place_live(&session, id, 0, at, false).expect("placed");
    let fast = sched.place_live(&session, id, 1, at, false).expect("placed");
    assert_eq!(plain.step, 4);
    assert_eq!(fast.step, 8, "twice as far into the pattern in the same second");
}

/// Polymeter, from the recorder's side: two tracks, one clock, one moment, and
/// two different answers because each divides by its own length.
#[test]
fn a_64_step_track_and_a_128_step_one_divide_the_same_clock_differently() {
    let (mut session, id) = one_box();
    shape(&mut session, id, 0, 0, 64, TrackScale::One);
    shape(&mut session, id, 0, 1, 128, TrackScale::One);
    let sched = prepared(&session);

    let at = 100.0 * STEP;
    let short = sched.place_live(&session, id, 0, at, false).expect("placed");
    let long = sched.place_live(&session, id, 1, at, false).expect("placed");
    assert_eq!((short.step, short.pass), (36, 1));
    assert_eq!((long.step, long.pass), (100, 0));
}

/// **The reason placement is measured off `origin_at` and not off the transport
/// bar's readout.** A scene change re-anchors every cursor to the moment the
/// switch was taken, so a note played four steps after it is on step 4 of the
/// new pattern — not on step 4 + wherever the old one had got to.
#[test]
fn placement_follows_a_scene_switch_that_moved_the_origin() {
    let (mut session, id) = one_box();
    shape(&mut session, id, 0, 0, 16, TrackScale::One);
    session.add_scene("Scene 2", Some(0));
    assert!(session.set_slot_in_scene(1, id, digi_core::PatternRef::from_slot(1)));
    shape(&mut session, id, 1, 0, 16, TrackScale::One);

    let mut sched = prepared(&session);
    // Before the switch, ten steps in, the answer is measured from zero.
    let before = sched.place_live(&session, id, 0, 10.0 * STEP, false).expect("placed");
    assert_eq!(before.step, 10);

    // Take the switch. `commit_scene` re-dates every cursor's origin to the
    // moment the incoming pattern started.
    sched.commit_scene(&session, 1);
    let origin = sched.cursors()[0].origin_at;
    let after = sched
        .place_live(&session, id, 0, origin + 4.0 * STEP, false)
        .expect("placed");
    assert_eq!(
        (after.step, after.pass),
        (4, 0),
        "step 4 of the pattern that is now playing"
    );
}

#[test]
fn quantize_zeroes_the_micro_and_moves_nothing_else() {
    let (mut session, id) = one_box();
    shape(&mut session, id, 0, 0, 16, TrackScale::One);
    let sched = prepared(&session);

    let loose = sched.place_live(&session, id, 0, 6.35 * STEP, false).expect("placed");
    let tight = sched.place_live(&session, id, 0, 6.35 * STEP, true).expect("placed");
    assert_eq!(loose.step, 6);
    assert!(loose.micro > 0.3);
    assert_eq!(tight.step, 6);
    assert_eq!(tight.micro, 0.0);
}

/// A track the sounding scene is not playing has no cursor and therefore no
/// grid to be placed on. `None` rather than a guessed step: a take counts these
/// and says so, which is the only honest thing to do with a note aimed at
/// nowhere.
#[test]
fn a_track_with_no_cursor_has_nowhere_to_place_a_note() {
    let (mut session, id) = one_box();
    shape(&mut session, id, 0, 0, 16, TrackScale::One);
    let sched = prepared(&session);

    assert!(sched.place_live(&session, id, 99, 0.0, false).is_none(), "no such track");
    assert!(
        sched.place_live(&session, DeviceId(4242), 0, 0.0, false).is_none(),
        "no such device"
    );
}
