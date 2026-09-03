//! The spacebar, driven through a real headless egui pass into a real engine.
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
use digi_roll_studio::engine::{EngineLink, SinkFactory};
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
    let mut took = false;
    let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        took |= digi_roll_studio::ui::transport::shortcuts(ui, engine, session);
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
