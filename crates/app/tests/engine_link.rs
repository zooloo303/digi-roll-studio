//! The UI's half of the engine seam, driven without a UI and without a MIDI
//! stack.
//!
//! There is no oracle for any of this. `js/midi.js` owns one box, opens its port
//! when the page asks and never reopens it, so nothing in the original can say
//! what should happen when a second box is identified mid-set. These assertions
//! come from PLAN.md §4 — one queue whose entries carry a port, and a snapshot
//! that is a whole session at a time — and from what the hardware makes
//! unavoidable: a `midir` connection belongs to the thread that owns the sink,
//! and every `PortId` in flight is an index into the connections that are open.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use digi_core::device::{DeviceModel, PortEnd, PortRef, DN2, DT2};
use digi_core::model::Note;
use digi_core::Session;
use digi_engine::event::{PortId, PortTable};
use digi_engine::transport::PortSink;
use digi_roll_studio::engine::{EngineLink, SinkFactory};

#[derive(Default)]
struct Log {
    /// The port names each rebuild opened against, in `PortId` order — so a test
    /// can say not just *how many* connections were opened but which, and in
    /// which order, which is what the ids mean.
    opened: Vec<Vec<String>>,
    sent: Vec<(PortId, Vec<u8>)>,
}

struct SharedSink(Arc<Mutex<Log>>);

impl PortSink for SharedSink {
    fn send(&mut self, port: PortId, bytes: &[u8]) {
        self.0.lock().expect("sink log").sent.push((port, bytes.to_vec()));
    }
}

/// A sink factory that records what it was asked to open and what was sent.
fn recording() -> (Arc<Mutex<Log>>, SinkFactory) {
    let log = Arc::new(Mutex::new(Log::default()));
    let mine = Arc::clone(&log);
    let factory: SinkFactory = Box::new(move |ports: &PortTable| {
        let names = ports.ids().filter_map(|id| ports.name(id).map(str::to_string)).collect();
        mine.lock().expect("sink log").opened.push(names);
        let sink: Box<dyn PortSink> = Box::new(SharedSink(Arc::clone(&mine)));
        (sink, Vec::new())
    });
    (log, factory)
}

/// Whether every pitch that was switched on was switched off again — the
/// property that matters at a stop, and the one an on/off count does not state.
///
/// Deliberately not an equality. The scheduler registers a note in its active
/// table when it *schedules* it, so a stop landing between the schedule and the
/// send emits an off for a note that never sounded. That is harmless — a
/// note-off for a silent pitch is a no-op on every box, and it is what `panic`
/// does wholesale — but it makes the counts unequal by design.
fn nothing_left_sounding(log: &Log) -> bool {
    let mut sounding: std::collections::BTreeMap<(usize, u8, u8), bool> = Default::default();
    for (port, bytes) in &log.sent {
        // Realtime messages are one byte, so there is no pitch to read.
        let [status, pitch, ..] = bytes[..] else { continue };
        let key = (port.0, status & 0x0f, pitch);
        match status & 0xf0 {
            0x90 => sounding.insert(key, true),
            0x80 => sounding.insert(key, false),
            _ => None,
        };
    }
    sounding.values().all(|on| !on)
}

fn bind_output(session: &mut Session, device: usize, port: &str) {
    session.devices[device].io.output = Some(PortRef { id: port.into(), name: port.into() });
}

fn put_trigs(session: &mut Session, device: usize, track: usize, steps: &[u16]) {
    let id = session.devices[device].id;
    let slot = session
        .slot_in_scene(session.current_scene, id)
        .expect("every scene names a slot for every device")
        .slot();
    let track = session
        .device_mut(id)
        .expect("just looked it up")
        .pattern_mut(slot)
        .expect("slot exists")
        .track_mut(track)
        .expect("the model has this track");
    track.notes = steps
        .iter()
        .map(|s| Note::new(*s as f64, 60, 1.0, 100, 0.0))
        .collect();
}

/// Trigs on track 0 of a named slot, whichever scene happens to point at it.
fn put_trigs_in_slot(session: &mut Session, device: usize, slot: usize, pitch: u8, steps: &[u16]) {
    let id = session.devices[device].id;
    let track = session
        .device_mut(id)
        .expect("the device is in the session")
        .pattern_mut(slot)
        .expect("the slot exists")
        .track_mut(0)
        .expect("the model has track 1");
    track.notes = steps
        .iter()
        .map(|s| Note::new(*s as f64, pitch, 1.0, 100, 0.0))
        .collect();
}

/// Every pitch that was switched on, in the order the wire saw them.
fn pitches_played(log: &Log) -> Vec<u8> {
    log.sent
        .iter()
        .filter_map(|(_, b)| match b[..] {
            [status, pitch, _] if status & 0xf0 == 0x90 => Some(pitch),
            _ => None,
        })
        .collect()
}

fn models() -> (&'static DeviceModel, &'static DeviceModel) {
    (&DT2, &DN2)
}

#[test]
fn the_engine_is_built_once_and_rebuilt_only_when_the_routing_moves() {
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();

    assert!(!engine.running(), "nothing is spawned until something asks");
    assert!(engine.reroute(&session), "the first call builds the engine");
    assert_eq!(engine.rebuilds(), 1);
    assert!(engine.running());
    assert_eq!(engine.ports().len(), 0, "no box has been identified yet");

    assert!(!engine.reroute(&session), "an unchanged session rebuilds nothing");

    // Neither of these is routing, and neither may cost a MIDI connection.
    session.tempo_bpm = 137.0;
    put_trigs(&mut session, 0, 0, &[0, 4, 8, 12]);
    assert!(!engine.reroute(&session));
    assert_eq!(engine.rebuilds(), 1);

    // This is. A box that has just answered the identity handshake has an out
    // port it did not have a frame ago, and the sink has to be opened again.
    bind_output(&mut session, 0, "Elektron Digitakt II");
    assert!(engine.reroute(&session));
    assert_eq!(engine.rebuilds(), 2);
    assert_eq!(engine.ports().len(), 1);
    assert_eq!(
        log.lock().expect("sink log").opened,
        vec![Vec::<String>::new(), vec!["Elektron Digitakt II".to_string()]],
    );
}

#[test]
fn a_second_box_is_a_second_port_and_two_boxes_on_one_port_share_it() {
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();

    bind_output(&mut session, 0, "Elektron Digitakt II");
    bind_output(&mut session, 1, "Elektron Digitone II");
    engine.reroute(&session);
    assert_eq!(engine.ports().len(), 2);
    assert_eq!(engine.ports().get("Elektron Digitakt II"), Some(PortId(0)));
    assert_eq!(engine.ports().get("Elektron Digitone II"), Some(PortId(1)));

    // Both boxes down one cable — a MIDI splitter, or one box thru another.
    // One connection, one clock: nothing gets its own.
    bind_output(&mut session, 1, "Elektron Digitakt II");
    assert!(engine.reroute(&session));
    assert_eq!(engine.ports().len(), 1);
}

#[test]
fn an_edit_snapshots_rather_than_rebuilding() {
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "a port");
    engine.reroute(&session);
    assert_eq!(engine.rebuilds(), 1);

    // The case this exists for: drawing notes while the transport is running.
    // Rebuilding here would drop every connection and restart the set, once per
    // note.
    for step in 0..8u16 {
        put_trigs(&mut session, 0, 0, &[step]);
        engine.sync(&session);
    }
    assert_eq!(engine.rebuilds(), 1);
    assert_eq!(log.lock().expect("sink log").opened.len(), 1, "one sink, opened once");
}

#[test]
fn a_track_pointed_at_its_own_port_gets_one_of_its_own() {
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "the box");

    let id = session.devices[0].id;
    let slot = session.slot_in_scene(0, id).expect("a slot").slot();
    session
        .device_mut(id)
        .expect("device")
        .pattern_mut(slot)
        .expect("pattern")
        .track_mut(3)
        .expect("track")
        .out_port = Some("a synth over MIDI".into());

    engine.reroute(&session);
    // The device's port is interned before any track's, which is the order
    // `Scheduler::prepare` resolves them in.
    assert_eq!(engine.ports().get("the box"), Some(PortId(0)));
    assert_eq!(engine.ports().get("a synth over MIDI"), Some(PortId(1)));
}

#[test]
fn playing_puts_clock_and_notes_on_the_wire_and_stop_releases_every_one() {
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    // 240 bpm: a step is 62.5 ms, so a short run is still several steps. The
    // assertions below are all "at least one" — this test proves the seam
    // carries bytes, not what the timing is. `digi_engine`'s own tests own the
    // timing, and the jitter example owns the hardware measurement.
    session.tempo_bpm = 240.0;
    bind_output(&mut session, 0, "a port");
    put_trigs(&mut session, 0, 0, &[0, 1, 2, 3, 4, 5, 6, 7]);

    assert!(!engine.is_playing());
    engine.play(&session);
    std::thread::sleep(Duration::from_millis(300));
    assert!(engine.is_playing(), "the engine publishes that it is running");
    assert!(engine.position_steps() > 0.0, "the playhead moved");
    engine.stop();
    std::thread::sleep(Duration::from_millis(50));
    assert!(!engine.is_playing());

    let log = log.lock().expect("sink log");
    let count = |f: &dyn Fn(&[u8]) -> bool| log.sent.iter().filter(|(_, b)| f(b)).count();
    let note_ons = count(&|b| b[0] & 0xf0 == 0x90);
    let note_offs = count(&|b| b[0] & 0xf0 == 0x80);

    assert!(note_ons > 0, "the trigs played");
    assert!(note_offs > 0, "and were released");
    assert!(nothing_left_sounding(&log), "the stop left nothing sounding");
    assert!(count(&|b| b == [0xf8]) > 0, "the box was clocked");
    assert!(count(&|b| b == [0xfa]) > 0, "and started");
    assert!(count(&|b| b == [0xfc]) > 0, "and stopped");
    assert!(
        log.sent.iter().all(|(port, _)| *port == PortId(0)),
        "one box, one port"
    );
}

#[test]
fn stopping_releases_a_note_whose_off_was_already_queued() {
    // The window this exists for: the scheduler emits a note-off up to 50 ms
    // ahead of its deadline and forgets the note as it does, so between those two
    // moments the note is sounding and is in neither the active table nor the
    // wire. Stopping there must still release it, or the box drones with nothing
    // left that can stop it.
    //
    // 120 bpm and whole-step notes put a note-off in that window most of the
    // time; the loop makes "most" into "reliably at least once".
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    session.tempo_bpm = 120.0;
    bind_output(&mut session, 0, "a port");
    put_trigs(&mut session, 0, 0, &(0..16).collect::<Vec<u16>>());

    for _ in 0..12 {
        engine.play(&session);
        std::thread::sleep(Duration::from_millis(70));
        engine.stop();
        std::thread::sleep(Duration::from_millis(20));
    }

    let log = log.lock().expect("sink log");
    let ons = log.sent.iter().filter(|(_, b)| b[0] & 0xf0 == 0x90).count();
    assert!(ons > 0, "something played");
    assert!(nothing_left_sounding(&log), "a stop left a note sounding");
}

#[test]
fn the_models_are_the_two_the_session_opens_with() {
    // Guards the fixtures the tests above lean on: `two_box_session` is a DT2 and
    // a DN2, in that order, 16 tracks each.
    let (dt2, dn2) = models();
    let session = digi_core::two_box_session();
    assert_eq!(session.devices[0].model, dt2);
    assert_eq!(session.devices[1].model, dn2);
    assert_eq!(session.devices[0].model.num_tracks, 16);
    assert_eq!(session.devices[1].model.num_tracks, 16);
}

// ---------------------------------------------------------------------------
// Scenes. The link's part in a scene change is small and the two halves of it
// are easy to get backwards: which scene is *asked for* is the UI's, which one
// is *sounding* is the engine's, and the difference between them is exactly a
// queued switch.
// ---------------------------------------------------------------------------

#[test]
fn a_scene_change_is_not_a_routing_change_and_does_not_rebuild_the_engine() {
    // The failure this exists to stop: a rebuild is a new engine thread and a new
    // scheduler, so it restarts the set from the top. A scene change that
    // rebuilt would restart the music at the exact moment the switch was supposed
    // to be seamless — which is why `intern_ports` covers every scene rather than
    // the one playing.
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "the box");

    // The second scene sends track 1 somewhere the first one does not, which is
    // the case that would otherwise want a connection the sink has not opened.
    session.add_scene("Chorus", Some(0));
    let dt2 = session.devices[0].id;
    session.set_slot_in_scene(1, dt2, digi_core::PatternRef::from_slot(1));
    session
        .device_mut(dt2)
        .expect("device")
        .pattern_mut(1)
        .expect("slot")
        .track_mut(0)
        .expect("track")
        .out_port = Some("a synth".into());

    engine.reroute(&session);
    assert_eq!(engine.rebuilds(), 1);
    assert_eq!(engine.ports().len(), 2, "both scenes' ports are open before either plays");

    engine.select_scene(&session, 1);
    assert!(!engine.reroute(&session), "a scene change is not a routing change");
    assert_eq!(engine.rebuilds(), 1);
    assert_eq!(log.lock().expect("sink log").opened.len(), 1, "one sink, opened once");
}

#[test]
fn a_scene_picked_while_stopped_takes_effect_at_once() {
    // There is no boundary to wait for when nothing is moving, and picking a
    // scene to edit has to do what it looks like it does.
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "the box");
    session.add_scene("Chorus", Some(0));
    engine.reroute(&session);

    assert_eq!(engine.playing_scene(), 0);
    engine.select_scene(&session, 1);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.selected_scene(), 1);
    assert_eq!(engine.playing_scene(), 1, "stopped, the switch is immediate");
    assert_eq!(engine.queued_scene(), None, "so there is nothing left queued");
}

#[test]
fn a_scene_asked_for_while_playing_waits_for_the_boundary() {
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    // 480 bpm makes a step 31 ms, so the default 16-step tracks put the boundary
    // half a second out — long enough that "still queued" is not a race, short
    // enough that the test does not sit there.
    session.tempo_bpm = 480.0;
    bind_output(&mut session, 0, "the box");
    session.add_scene("Chorus", Some(0));
    let dt2 = session.devices[0].id;
    session.set_slot_in_scene(1, dt2, digi_core::PatternRef::from_slot(1));
    put_trigs_in_slot(&mut session, 0, 0, 60, &(0..16).collect::<Vec<u16>>());
    put_trigs_in_slot(&mut session, 0, 1, 67, &(0..16).collect::<Vec<u16>>());

    engine.play(&session);
    std::thread::sleep(Duration::from_millis(60));
    engine.select_scene(&session, 1);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(engine.selected_scene(), 1, "the UI knows what was asked for");
    assert_eq!(engine.playing_scene(), 0, "and the boxes are still on the old one");
    assert_eq!(engine.queued_scene(), Some(1), "with the new one queued behind it");

    // Past the boundary.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(engine.playing_scene(), 1);
    assert_eq!(engine.queued_scene(), None);
    engine.stop();
    std::thread::sleep(Duration::from_millis(50));

    // And the wire says the same thing: the outgoing scene played to the
    // boundary, then the incoming one, with no interleaving.
    let log = log.lock().expect("sink log");
    let pitches = pitches_played(&log);
    assert!(pitches.contains(&60) && pitches.contains(&67), "both scenes sounded");
    let first_g = pitches.iter().position(|p| *p == 67).expect("the new scene");
    assert!(
        !pitches[first_g..].contains(&60),
        "the old scene stopped when the new one started: {pitches:?}"
    );
    assert!(nothing_left_sounding(&log), "and the switch left nothing sounding");
}

#[test]
fn asking_for_a_scene_that_is_not_there_changes_nothing() {
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "the box");
    engine.reroute(&session);

    engine.select_scene(&session, 4);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.selected_scene(), 0, "ignored, not clamped onto the last scene");
    assert_eq!(engine.playing_scene(), 0);
}

#[test]
fn a_rebuild_starts_on_the_scene_that_was_asked_for() {
    // A rebuild is a new scheduler, which knows nothing about scenes, clock or
    // FILL — the link carries all three across. Identifying a box mid-set is the
    // case: it must not put the boxes back on scene 1.
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "the box");
    session.add_scene("Chorus", Some(0));
    engine.reroute(&session);
    engine.select_scene(&session, 1);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.playing_scene(), 1);

    bind_output(&mut session, 1, "the other box");
    assert!(engine.reroute(&session), "a second box is a rebuild");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.playing_scene(), 1, "and it comes back on the same scene");
}

#[test]
fn removing_a_scene_below_the_one_playing_moves_the_engine_with_it() {
    // Indices shift. Scene 2 of three becomes scene 1 when scene 0 goes, and an
    // engine still holding "2" would play a scene nobody selected — or, past the
    // end, nothing at all.
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "the box");
    session.add_scene("Two", Some(0));
    session.add_scene("Three", Some(0));
    engine.reroute(&session);

    engine.select_scene(&session, 2);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.playing_scene(), 2);

    // What the bar does: remove, then hand the engine the shifted index that
    // `remove_scene` has already worked out for `current_scene`.
    session.current_scene = engine.playing_scene();
    assert!(session.remove_scene(0));
    assert_eq!(session.current_scene, 1, "the same scene, one index lower");
    engine.rebase_scene(&session, session.current_scene);
    engine.sync(&session);
    std::thread::sleep(Duration::from_millis(50));

    assert_eq!(engine.playing_scene(), 1);
    assert_eq!(session.scenes[engine.playing_scene()].name, "Three");
}

// ------------------------------------------- a port picked by hand, not identified
//
// The device strip's pickers exist so the app can be driven with no Elektron on
// the desk — an IAC bus, a soft synth — which is what puts the rest of the UI back
// inside the dev loop PLAN.md §7 rule 1 asks for. `Session::set_device_port` is
// tested in `core`; what these say is that the *engine* treats a hand-picked port
// exactly as it treats one Identify supplied, because nothing downstream of the
// session can tell the two apart and nothing should be able to.

#[test]
fn a_hand_picked_port_is_a_routing_change_and_opens_a_connection() {
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    engine.reroute(&session);
    assert_eq!(engine.ports().len(), 0, "nothing has been identified or picked");

    let dt2 = session.devices[0].id;
    assert!(session.set_device_port(
        dt2,
        PortEnd::Output,
        Some(PortRef { id: "iac1".into(), name: "IAC Driver Bus 1".into() }),
    ));

    assert!(engine.reroute(&session), "picking an out port moved the routing");
    assert_eq!(engine.ports().len(), 1);
    assert_eq!(
        log.lock().expect("sink log").opened.last().expect("a rebuild happened"),
        &vec!["IAC Driver Bus 1".to_string()],
    );
}

#[test]
fn a_box_pointed_at_a_bus_by_hand_plays_its_trigs_there() {
    // The whole point, end to end: no handshake, no Elektron, and the notes still
    // reach the port the strip named.
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    session.tempo_bpm = 240.0;

    let dt2 = session.devices[0].id;
    session.set_device_port(
        dt2,
        PortEnd::Output,
        Some(PortRef { id: "iac1".into(), name: "IAC Driver Bus 1".into() }),
    );
    put_trigs(&mut session, 0, 0, &[0, 4, 8, 12]);

    engine.play(&session);
    std::thread::sleep(Duration::from_millis(400));
    engine.stop();
    std::thread::sleep(Duration::from_millis(80));

    let log = log.lock().expect("sink log");
    assert!(!pitches_played(&log).is_empty(), "nothing reached the hand-picked port");
    assert!(pitches_played(&log).iter().all(|p| *p == 60));
    assert!(nothing_left_sounding(&log), "a stop must release every note");
}

#[test]
fn taking_a_port_away_by_hand_closes_the_connection() {
    // The way to silence a box without unplugging it. It has to reach the engine,
    // or the strip would say "none" while the notes kept going out.
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    let dt2 = session.devices[0].id;
    session.set_device_port(
        dt2,
        PortEnd::Output,
        Some(PortRef { id: "iac1".into(), name: "IAC Driver Bus 1".into() }),
    );
    engine.reroute(&session);
    assert_eq!(engine.ports().len(), 1);

    assert!(session.set_device_port(dt2, PortEnd::Output, None));
    assert!(engine.reroute(&session), "losing a port is a routing change too");

    assert_eq!(engine.ports().len(), 0);
    assert_eq!(
        log.lock().expect("sink log").opened.last().expect("a rebuild happened"),
        &Vec::<String>::new(),
    );
}

#[test]
fn moving_a_port_from_one_box_to_the_other_leaves_one_connection_open() {
    // `set_device_port` takes the port off whoever held it, so the table the
    // engine opens against still has exactly one entry — it has just changed
    // hands. Two connections to one socket is how a note gets stuck with nothing
    // left owning it, which is the failure `reroute` drops the old handle first to
    // avoid.
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    let (dt2, dn2) = (session.devices[0].id, session.devices[1].id);
    let bus = PortRef { id: "iac1".into(), name: "IAC Driver Bus 1".into() };

    session.set_device_port(dt2, PortEnd::Output, Some(bus.clone()));
    engine.reroute(&session);

    session.set_device_port(dn2, PortEnd::Output, Some(bus));
    engine.reroute(&session);

    assert_eq!(engine.ports().len(), 1, "one socket, one connection");
    assert!(session.device(dt2).expect("still there").io.output.is_none());
    assert_eq!(
        log.lock().expect("sink log").opened.last().expect("a rebuild happened"),
        &vec!["IAC Driver Bus 1".to_string()],
    );
}

/// Set a track's LEVEL, the way the VOL field does.
fn set_level(session: &mut Session, device: usize, track: usize, level: Option<u8>) {
    let id = session.devices[device].id;
    let slot = session.slot_in_scene(session.current_scene, id).expect("a slot").slot();
    session
        .device_mut(id)
        .expect("device")
        .pattern_mut(slot)
        .expect("pattern")
        .track_mut(track)
        .expect("track")
        .level = level;
}

/// Every NRPN on the wire, as `(port, channel, [(controller, value); 4])`.
///
/// **One `send` carries all four control changes**, which is the point of
/// `MidiMsg::Nrpn` being one variant: the parameter select and its value must
/// not be interleaved with anything else. So this parses a 12-byte blob rather
/// than four sends — and a test that split them would be asserting the opposite
/// of the contract.
fn nrpn_sent(log: &Log) -> Vec<(usize, u8, Vec<(u8, u8)>)> {
    log.sent
        .iter()
        .filter(|(_, bytes)| bytes.len() == 12 && bytes[0] & 0xf0 == 0xb0)
        .map(|(port, bytes)| {
            let pairs = bytes.chunks(3).map(|c| (c[1], c[2])).collect();
            (port.0, bytes[0] & 0x0f, pairs)
        })
        .collect()
}

#[test]
fn moving_a_tracks_level_sends_the_boxs_own_fader_on_that_tracks_channel() {
    // The whole of the volume feature at the seam: a number in the session
    // becomes NRPN 1/100 — the DT2's track level — on the port that track's
    // notes go to, with the transport stopped, because stopped is when most
    // mixing happens.
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "a port");
    set_level(&mut session, 0, 2, Some(64));

    engine.reroute(&session);
    let device = session.devices[0].id;
    assert!(engine.send_track_level(&session, device, 2));
    std::thread::sleep(Duration::from_millis(50));

    let log = log.lock().expect("sink log");
    assert_eq!(
        nrpn_sent(&log),
        // Track 3 sits on channel 3 by `Track::new`'s 1:1 map, which is channel
        // byte 2 on the wire. 99/98 = NRPN MSB 1, LSB 100; then 6/38 = 64 in the
        // top seven bits of the 14, which is where a 0–127 axis puts it.
        [(0, 2, vec![(99, 1), (98, 100), (6, 64), (38, 0)])],
        "one NRPN, on the DT2's own number, on the track's channel"
    );
}

#[test]
fn a_dn2_gets_its_own_level_number_not_the_dt2s() {
    // 95 is the CC on both boxes and the NRPN is not: 1/100 on a DT2, 1/110 on
    // a DN2. Sending one box's number to the other is the mistake this whole
    // parameter layer is shaped to prevent.
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 1, "the dn2");
    set_level(&mut session, 1, 0, Some(127));

    engine.reroute(&session);
    let device = session.devices[1].id;
    assert!(engine.send_track_level(&session, device, 0));
    std::thread::sleep(Duration::from_millis(50));

    let log = log.lock().expect("sink log");
    let sent = nrpn_sent(&log);
    assert_eq!(sent.len(), 1, "one message");
    assert_eq!(sent[0].2[1], (98, 110), "the DN2's LSB, not the DT2's 100");
    assert_eq!(sent[0].2[2], (6, 127));
}

#[test]
fn a_track_that_has_never_been_touched_sends_nothing() {
    // `Track::level` is `None` until someone moves the fader, and `None` has to
    // stay silent: the app does not know where the box's fader is, so opening a
    // project must not ride sixteen of them to a number it invented.
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "a port");

    engine.reroute(&session);
    let device = session.devices[0].id;
    assert!(!engine.send_track_level(&session, device, 0), "nothing to send");
    std::thread::sleep(Duration::from_millis(50));
    assert!(log.lock().expect("sink log").sent.is_empty());
}

#[test]
fn a_level_follows_the_track_to_its_own_port_not_its_boxs() {
    // Same rule as the notes — `Scheduler::prepare` gives a track's own
    // `out_port` precedence over its device's — because a fader that reached a
    // different port from the notes would ride some other box's track.
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    bind_output(&mut session, 0, "the box");
    let id = session.devices[0].id;
    let slot = session.slot_in_scene(0, id).expect("a slot").slot();
    {
        let track = session
            .device_mut(id)
            .expect("device")
            .pattern_mut(slot)
            .expect("pattern")
            .track_mut(3)
            .expect("track");
        track.out_port = Some("a synth over MIDI".into());
        track.level = Some(10);
    }

    engine.reroute(&session);
    assert!(engine.send_track_level(&session, id, 3));
    std::thread::sleep(Duration::from_millis(50));

    let log = log.lock().expect("sink log");
    let sent = nrpn_sent(&log);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, PortId(1).0, "the track's own port, not the box's");
}

#[test]
fn a_level_on_a_track_routed_nowhere_is_refused_rather_than_guessed() {
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = digi_core::two_box_session();
    set_level(&mut session, 0, 0, Some(64));

    engine.reroute(&session);
    let device = session.devices[0].id;
    assert!(!engine.send_track_level(&session, device, 0));
    std::thread::sleep(Duration::from_millis(50));
    assert!(log.lock().expect("sink log").sent.is_empty());
}

// --- song mode ---------------------------------------------------------------
//
// PLAN.md §6 phase 12, at the seam. The scheduler's own tests prove the walk;
// these prove the three things only the link can get wrong — that the mode
// survives a rebuild, that a song built while SONG is already lit starts playing
// without the toggle being pressed twice, and that an `END: STOP` reaches the
// transport as an actual stop rather than as silence with the playhead still
// running.

/// A session whose scenes are two different pitches, so the wire says which row
/// is playing. 480 bpm, as the scene tests use: a 16-step track is half a second.
fn song_session() -> Session {
    let mut session = digi_core::two_box_session();
    session.tempo_bpm = 480.0;
    bind_output(&mut session, 0, "the box");
    session.add_scene("Chorus", Some(0));
    let dt2 = session.devices[0].id;
    session.set_slot_in_scene(1, dt2, digi_core::PatternRef::from_slot(1));
    put_trigs_in_slot(&mut session, 0, 0, 60, &(0..16).collect::<Vec<u16>>());
    put_trigs_in_slot(&mut session, 0, 1, 67, &(0..16).collect::<Vec<u16>>());
    session
}

#[test]
fn song_mode_walks_the_rows_and_the_pointer_says_where() {
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = song_session();
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();

    engine.set_song_mode(&session, true);
    engine.play(&session);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(engine.song_position().map(|(row, _)| row), Some(0));

    // Past the first row's boundary — the 16-step track at 480 bpm is 500 ms.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(engine.song_position().map(|(row, _)| row), Some(1));
    engine.stop();
    std::thread::sleep(Duration::from_millis(50));

    let log = log.lock().expect("sink log");
    let pitches = pitches_played(&log);
    assert!(pitches.contains(&60) && pitches.contains(&67), "both rows sounded");
    assert!(nothing_left_sounding(&log));
}

#[test]
fn a_song_built_while_song_mode_is_already_on_starts_walking() {
    // The mode is a standing request, so the snapshot that first gives the engine
    // rows to walk is where it is honoured. Otherwise SONG has to be pressed
    // twice — once before there was a song, once after — which is a bug nobody
    // would report as one.
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = song_session();

    engine.set_song_mode(&session, true);
    engine.play(&session);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.song_position(), None, "nothing to walk yet");

    session.add_song_row(1).unwrap();
    engine.sync(&session);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.song_position(), Some((0, 0)));
    engine.stop();
}

#[test]
fn end_stop_stops_the_transport_and_releases_everything() {
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = song_session();
    session.add_song_row(0).unwrap();
    session.song_mut().end = digi_core::EndAction::Stop;

    engine.set_song_mode(&session, true);
    engine.play(&session);
    std::thread::sleep(Duration::from_millis(50));
    assert!(engine.is_playing());

    // One 16-step row at 480 bpm is 500 ms.
    std::thread::sleep(Duration::from_millis(700));
    assert!(!engine.is_playing(), "the song ran out and the transport stopped");
    let log = log.lock().expect("sink log");
    assert!(nothing_left_sounding(&log), "and nothing was left droning");
}

#[test]
fn a_rebuild_mid_set_keeps_walking_the_song() {
    // A rebuild is a new scheduler that knows nothing of song mode — the link
    // carries it across, the same way it carries the scene, the clock and FILL.
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = song_session();
    session.add_song_row(0).unwrap();
    session.add_song_row(1).unwrap();

    engine.set_song_mode(&session, true);
    engine.play(&session);
    std::thread::sleep(Duration::from_millis(50));

    // Identifying a second box is a new port, which is a rebuild.
    bind_output(&mut session, 1, "the other box");
    assert!(engine.reroute(&session), "a new port rebuilds");
    std::thread::sleep(Duration::from_millis(50));
    assert!(engine.song_mode());
    assert_eq!(engine.song_position(), Some((0, 0)), "from the top, as a rebuild is");
    engine.stop();
}

#[test]
fn leaving_song_mode_stops_the_walk_without_stopping_the_music() {
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let mut session = song_session();
    session.add_song_row(1).unwrap();

    engine.set_song_mode(&session, true);
    engine.play(&session);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(engine.playing_scene(), 1, "the row put its scene up");

    engine.set_song_mode(&session, false);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(engine.song_position(), None);
    assert!(engine.is_playing(), "still running");
    assert_eq!(engine.playing_scene(), 1, "on the scene the last row left up");
    engine.stop();
}
