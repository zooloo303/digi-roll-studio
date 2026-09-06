//! The spacebar and `R`, driven through a real headless egui pass into a real
//! engine.
//!
//! `ui::transport`'s own tests say what the key *read* does — that a plain space
//! is taken whole, that a held one is one tap, that a focused field or an open
//! dialog keeps it. This says the other half: that the tap reaches the transport
//! and toggles it, with a scheduler running on a thread and MIDI leaving through
//! a sink. Nothing is stubbed between the `Event::Key` and the bytes.
//!
//! The events are built the way `egui-winit` really sends a space — the key
//! event and the `Event::Text(" ")` beside it — for the reason
//! `tracks_clipboard.rs` spells out at length: a test that feeds the input the
//! code expects rather than the input the platform produces cannot fail.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use digi_core::device::PortRef;
use digi_core::model::Note;
use digi_core::Session;
use digi_engine::event::{PortId, PortTable};
use digi_engine::transport::PortSink;
use digi_roll_studio::engine::{EngineLink, InputFactory, SinkFactory};
use digi_roll_studio::record::Recorder;
use digi_roll_studio::ui::tracks::Selection;
use eframe::egui;

#[derive(Default)]
struct Log {
    sent: Vec<(PortId, Vec<u8>)>,
}

struct SharedSink(Arc<Mutex<Log>>);

impl PortSink for SharedSink {
    fn send(&mut self, port: PortId, bytes: &[u8]) {
        self.0.lock().expect("sink log").sent.push((port, bytes.to_vec()));
    }
}

fn recording() -> (Arc<Mutex<Log>>, SinkFactory) {
    let log = Arc::new(Mutex::new(Log::default()));
    let mine = Arc::clone(&log);
    let factory: SinkFactory = Box::new(move |_ports: &PortTable| {
        let sink: Box<dyn PortSink> = Box::new(SharedSink(Arc::clone(&mine)));
        (sink, Vec::new())
    });
    (log, factory)
}

fn notes_sounded(log: &Arc<Mutex<Log>>) -> usize {
    log.lock()
        .expect("sink log")
        .sent
        .iter()
        .filter(|(_, b)| matches!(b[..], [status, _, _] if status & 0xf0 == 0x90))
        .count()
}

/// A plain spacebar as the platform sends it: the key, then the printable
/// character that comes with it.
fn spacebar() -> Vec<egui::Event> {
    vec![
        egui::Event::Key {
            key: egui::Key::Space,
            physical_key: Some(egui::Key::Space),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Text(" ".to_owned()),
    ]
}

/// Letting go of it, in a frame of its own.
///
/// **Not optional, and not cosmetic.** `InputState::begin_pass` rewrites every
/// key event's `repeat` flag from its own `keys_down` set, so a second press
/// with no release between arrives as a repeat — which the transport ignores by
/// design. Without this a second tap in a test is a held-down key, which is
/// what the first cut of this file was accidentally asserting on.
fn release() -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key: egui::Key::Space,
        physical_key: Some(egui::Key::Space),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

/// One pass of the shell's shortcut read. Returns whether the key was taken.
fn frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    engine: &mut EngineLink,
    session: &Session,
) -> bool {
    frame_rec(ctx, events, engine, session, &mut Recorder::default())
}

/// The same, against a recorder the caller holds — needed by the `R` tests,
/// since arming is the recorder's state and not the engine's.
fn frame_rec(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    engine: &mut EngineLink,
    session: &Session,
    recorder: &mut Recorder,
) -> bool {
    let mut took = false;
    let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        took |= digi_roll_studio::ui::transport::shortcuts(
            ui,
            engine,
            session,
            recorder,
            Selection::default(),
        );
    });
    output.textures_delta.clear();
    took
}

/// A whole tap — press, then let go — and whether the press was taken.
fn tap(ctx: &egui::Context, engine: &mut EngineLink, session: &Session) -> bool {
    let took = frame(ctx, spacebar(), engine, session);
    frame(ctx, release(), engine, session);
    took
}

// ------------------------------------------------------------------- `R`

/// A bare letter as the platform sends it: the key, and the character
/// `egui-winit` pushes beside it. Same shape as `shell_keys.rs`'s, and for the
/// same reason — a test that feeds the input the code expects rather than the
/// input the platform produces cannot fail.
fn letter(key: egui::Key) -> Vec<egui::Event> {
    vec![
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Text(key.name().to_ascii_lowercase()),
    ]
}

fn release_letter(key: egui::Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

fn tap_letter(
    ctx: &egui::Context,
    key: egui::Key,
    engine: &mut EngineLink,
    session: &Session,
    recorder: &mut Recorder,
) -> bool {
    let took = frame_rec(ctx, letter(key), engine, session, recorder);
    frame_rec(ctx, release_letter(key), engine, session, recorder);
    took
}

/// An input factory that opens nothing and never fails. A keyboard is the one
/// piece of hardware no test in this repo may need, and `InputFactory` exists so
/// that stays true — the handle is opaque, so `()` is a perfectly good open
/// port as far as `EngineLink` is concerned.
fn stub_input() -> InputFactory {
    Box::new(|_port, _tx| Ok(Box::new(()) as Box<dyn Send>))
}

/// A session with a keyboard picked in Setup, so REC has something to listen to.
fn session_with_keyboard() -> Session {
    let mut session = session_with_trigs();
    session.record_input = Some(PortRef { id: "kbd".into(), name: "A Keyboard".into() });
    session
}

#[test]
fn r_arms_recording_and_starts_the_transport_from_stopped() {
    let ctx = egui::Context::default();
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks_and_input(factory, stub_input());
    let mut recorder = Recorder::default();
    let session = session_with_keyboard();
    engine.reroute(&session);

    assert!(!recorder.armed());
    assert!(tap_letter(&ctx, egui::Key::R, &mut engine, &session, &mut recorder));
    assert!(recorder.armed(), "R arms");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        engine.is_playing(),
        "and REC while stopped starts the transport — decision 5, no count-in"
    );
    engine.stop();
}

/// Disarming is not a stop. One control silently doing the other's job is how a
/// set ends by accident.
#[test]
fn a_second_r_disarms_and_leaves_the_transport_running() {
    let ctx = egui::Context::default();
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks_and_input(factory, stub_input());
    let mut recorder = Recorder::default();
    let session = session_with_keyboard();
    engine.reroute(&session);

    tap_letter(&ctx, egui::Key::R, &mut engine, &session, &mut recorder);
    std::thread::sleep(Duration::from_millis(200));
    assert!(engine.is_playing());

    tap_letter(&ctx, egui::Key::R, &mut engine, &session, &mut recorder);
    assert!(!recorder.armed(), "the second press disarms");
    std::thread::sleep(Duration::from_millis(80));
    assert!(engine.is_playing(), "and the transport is still running");
    engine.stop();
}

/// A held key is one press, exactly as the spacebar is: `InputState` rewrites
/// `repeat` from its own `keys_down` set, and without this rule REC would arm
/// and disarm at the key-repeat rate for as long as a finger rested on it.
#[test]
fn a_held_r_is_one_tap_and_not_a_stutter() {
    let ctx = egui::Context::default();
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks_and_input(factory, stub_input());
    let mut recorder = Recorder::default();
    let session = session_with_keyboard();
    engine.reroute(&session);

    assert!(frame_rec(&ctx, letter(egui::Key::R), &mut engine, &session, &mut recorder));
    for _ in 0..8 {
        assert!(
            !frame_rec(&ctx, letter(egui::Key::R), &mut engine, &session, &mut recorder),
            "a repeat is the same press still held down"
        );
    }
    assert!(recorder.armed(), "armed once, and still armed");
    engine.stop();
}

/// With no record input there is nothing to record *from*, so the key does
/// nothing rather than lighting REC over a keyboard that is not there. The
/// button says the same thing in a tooltip; this is the key half of it.
#[test]
fn r_does_nothing_with_no_record_input_picked() {
    let ctx = egui::Context::default();
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks_and_input(factory, stub_input());
    let mut recorder = Recorder::default();
    let session = session_with_trigs();
    engine.reroute(&session);

    tap_letter(&ctx, egui::Key::R, &mut engine, &session, &mut recorder);
    assert!(!recorder.armed());
    std::thread::sleep(Duration::from_millis(100));
    assert!(!engine.is_playing(), "and it did not start the transport either");
}

/// The two keys live in one read, off one queue, and **neither eats the other's
/// event** — `shell_keys.rs` makes the same claim about `S` and `Cmd+S`. Each
/// read matches its own key and its modifiers exactly, so a frame carrying both
/// arms *and* moves the transport, and the queue is empty afterwards.
///
/// What this deliberately does not assert is which state the transport lands
/// in. `R` while stopped calls `play`, and the space that follows it in the same
/// frame reads `is_playing()` off an atomic the engine thread has not written
/// yet — so it calls `play` again rather than `stop`. That is the same one-frame
/// lag the bar already lives with when it draws PLAY as disabled, and it is not
/// what this test is about.
#[test]
fn r_and_space_do_not_eat_each_others_keypress() {
    let ctx = egui::Context::default();
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks_and_input(factory, stub_input());
    let mut recorder = Recorder::default();
    let session = session_with_keyboard();
    engine.reroute(&session);

    let mut both = letter(egui::Key::R);
    both.extend(spacebar());
    let mut left = Vec::new();
    let mut output = ctx.run_ui(
        egui::RawInput { events: both, ..Default::default() },
        |ui| {
            digi_roll_studio::ui::transport::shortcuts(
                ui,
                &mut engine,
                &session,
                &mut recorder,
                Selection::default(),
            );
            left = ui.ctx().input(|i| i.events.clone());
        },
    );
    output.textures_delta.clear();

    assert!(recorder.armed(), "R was read");
    assert!(
        left.is_empty(),
        "and both keypresses were consumed whole, characters included: {left:?}"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(engine.is_playing(), "the transport moved, so the space reached it too");
    engine.stop();
}

/// A session whose first box is bound to a port and has something to play.
fn session_with_trigs() -> Session {
    let mut session = digi_core::two_box_session();
    session.devices[0].io.output =
        Some(PortRef { id: "a port".into(), name: "a port".into() });
    let id = session.devices[0].id;
    let slot = session
        .slot_in_scene(session.current_scene, id)
        .expect("every scene names a slot for every device")
        .slot();
    session
        .device_mut(id)
        .expect("just looked it up")
        .pattern_mut(slot)
        .expect("slot exists")
        .track_mut(0)
        .expect("the model has track 1")
        .notes = (0..16).map(|s| Note::new(s as f64, 60, 1.0, 100, 0.0)).collect();
    session
}

#[test]
fn the_spacebar_starts_the_transport_and_the_next_one_stops_it() {
    let ctx = egui::Context::default();
    let (log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let session = session_with_trigs();
    engine.reroute(&session);

    assert!(!engine.is_playing(), "nothing is running until the key is pressed");

    assert!(tap(&ctx, &mut engine, &session), "the key was taken");
    std::thread::sleep(Duration::from_millis(300));
    assert!(engine.is_playing(), "a space on a stopped transport is PLAY");
    assert!(notes_sounded(&log) > 0, "and the boxes heard it, not just the atomic");

    assert!(tap(&ctx, &mut engine, &session), "the key was taken again");
    std::thread::sleep(Duration::from_millis(80));
    assert!(!engine.is_playing(), "a space on a running transport is STOP");

    // And it goes back the other way, rather than being a one-shot latch.
    tap(&ctx, &mut engine, &session);
    std::thread::sleep(Duration::from_millis(300));
    assert!(engine.is_playing());
    engine.stop();
}

#[test]
fn a_frame_with_no_spacebar_leaves_the_transport_alone() {
    let ctx = egui::Context::default();
    let (_log, factory) = recording();
    let mut engine = EngineLink::with_sinks(factory);
    let session = session_with_trigs();
    engine.reroute(&session);

    // Every other key in the app passes through this read on its way to whoever
    // wants it, and none of them is the transport.
    for key in [egui::Key::C, egui::Key::V, egui::Key::Z, egui::Key::Enter, egui::Key::Escape] {
        let event = egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(!frame(&ctx, vec![event], &mut engine, &session), "{key:?} is not the transport");
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(!engine.is_playing());
}
