//! A take, end to end — MIDI_RECORD_DESIGN.md §4.4 and §5.4.
//!
//! `core/tests/all/record.rs` owns the rules and `engine/tests/all/record.rs`
//! owns the placement. What only this file can say is that the three stages are
//! actually wired together: a note played into a `LiveInput`'s `Sender` crosses
//! a driver channel, an engine thread, a placement, an `mpsc` and a frame of the
//! shell, and comes out as a `Note` in the selected track — **and that the whole
//! take is one undo step**, which is the seam §5.4 changes the shell for.
//!
//! Nothing here needs a keyboard. `InputFactory` is a trait object precisely so
//! a test can hold the `Sender` the engine handed out and play into it; the port
//! it "opened" is `()`.
//!
//! The engine thread is real, so the assertions are about *what* landed rather
//! than about which step it landed on: which step depends on how long a thread
//! took to wake up, and pinning that here would be a flaky test asserting
//! something `engine::record::place` already proves exactly.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use digi_core::device::PortRef;
use digi_core::history::{Content, History};
use digi_core::Session;
use digi_engine::event::PortTable;
use digi_engine::transport::PortSink;
use digi_midi::{LiveEvent, LiveKind};
use digi_roll_studio::engine::{EngineLink, InputFactory, SinkFactory};
use digi_roll_studio::record::Recorder;
use digi_roll_studio::ui::tracks::Selection;
use eframe::egui;

// ------------------------------------------------------------- the harness

struct NullSink;

impl PortSink for NullSink {
    fn send(&mut self, _port: digi_engine::event::PortId, _bytes: &[u8]) {}
}

fn sinks() -> SinkFactory {
    Box::new(|_ports: &PortTable| (Box::new(NullSink) as Box<dyn PortSink>, Vec::new()))
}

type Keyboard = Arc<Mutex<Option<std::sync::mpsc::Sender<LiveEvent>>>>;

fn keyboard() -> (Keyboard, InputFactory) {
    let held: Keyboard = Arc::new(Mutex::new(None));
    let mine = Arc::clone(&held);
    let factory: InputFactory = Box::new(move |_port, tx| {
        *mine.lock().expect("keyboard") = Some(tx);
        Ok(Box::new(()) as Box<dyn Send>)
    });
    (held, factory)
}

fn play(keyboard: &Keyboard, kind: LiveKind) {
    keyboard
        .lock()
        .expect("keyboard")
        .as_ref()
        .expect("the record input was opened")
        .send(LiveEvent { at: Instant::now(), kind })
        .expect("the engine thread is listening");
}

/// A DT2 on a port, with a keyboard picked and nothing recorded yet.
fn session() -> Session {
    let mut session = digi_core::two_box_session();
    session.tempo_bpm = 120.0;
    session.devices[0].io.output =
        Some(PortRef { id: "a port".into(), name: "a port".into() });
    session.record_input = Some(PortRef { id: "kbd".into(), name: "A Keyboard".into() });
    session
}

/// One pass of the shell, in `main.rs`'s order and with `main.rs`'s commit
/// guard. This *is* the seam under test: get either half of it wrong and the
/// take is not one undo step.
fn frame(
    ctx: &egui::Context,
    engine: &mut EngineLink,
    session: &mut Session,
    recorder: &mut Recorder,
    history: &mut History,
) {
    let before = (!history.is_open()).then(|| Content::of(session));
    recorder.tick(
        ctx,
        engine,
        session,
        Selection::default(),
        history,
        before.as_ref(),
    );
    // The pointer is never down in a headless pass, so the only thing holding
    // the step open is the take — §5.4's second clause.
    if !recorder.take_open() {
        history.commit(session);
    }
}

/// Several frames, with the engine thread given time to breathe between them.
fn frames(
    ctx: &egui::Context,
    engine: &mut EngineLink,
    session: &mut Session,
    recorder: &mut Recorder,
    history: &mut History,
    n: usize,
) {
    for _ in 0..n {
        std::thread::sleep(Duration::from_millis(20));
        frame(ctx, engine, session, recorder, history);
    }
}

fn notes(session: &Session) -> usize {
    let id = session.devices[0].id;
    session
        .current_pattern(id)
        .and_then(|p| p.track(0))
        .map(|t| t.notes.len())
        .unwrap_or(0)
}

/// Everything the console was told this frame.
fn said(ctx: &egui::Context) -> Vec<String> {
    ctx.data_mut(|d| {
        d.get_temp::<Vec<String>>(egui::Id::new("digi-roll-studio::console::outbox"))
            .unwrap_or_default()
    })
}

// ------------------------------------------------------------------ the tests

/// The whole feature in one test: arm, play, and the notes are in the track.
#[test]
fn a_note_played_while_armed_and_running_lands_in_the_selected_track() {
    let ctx = egui::Context::default();
    let (keys, inputs) = keyboard();
    let mut engine = EngineLink::with_sinks_and_input(sinks(), inputs);
    let mut session = session();
    let mut recorder = Recorder::default();
    let mut history = History::default();

    engine.reroute(&session);
    recorder.set_armed(true);
    engine.play(&session);
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);
    assert!(engine.is_playing());

    play(&keys, LiveKind::NoteOn { pitch: 60, velocity: 100 });
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 2);
    assert_eq!(notes(&session), 1, "the note appears while the key is still down");
    assert!(recorder.take_open());

    play(&keys, LiveKind::NoteOff { pitch: 60 });
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 2);

    let id = session.devices[0].id;
    let note = session.current_pattern(id).unwrap().track(0).unwrap().notes[0].clone();
    assert_eq!(note.pitch, 60);
    assert_eq!(note.velocity, 100);
    assert!(note.len > 0.0);
    engine.stop();
}

/// **One take is one undo step**, which is the whole of §5.4's change to the
/// shell. Before it, the per-frame commit turned a bar of playing into one step
/// per note and Cmd+Z walked it back a note at a time.
#[test]
fn a_whole_take_is_one_undo_step() {
    let ctx = egui::Context::default();
    let (keys, inputs) = keyboard();
    let mut engine = EngineLink::with_sinks_and_input(sinks(), inputs);
    let mut session = session();
    let mut recorder = Recorder::default();
    let mut history = History::default();

    engine.reroute(&session);
    recorder.set_armed(true);
    engine.play(&session);
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);

    // Five notes over several frames — the shape that used to become five steps.
    for pitch in [60, 62, 64, 65, 67] {
        play(&keys, LiveKind::NoteOn { pitch, velocity: 100 });
        frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 1);
        play(&keys, LiveKind::NoteOff { pitch });
        frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 1);
    }
    let placed = notes(&session);
    assert!(placed >= 2, "several notes landed, got {placed}");
    assert_eq!(history.depth(), (0, 0), "and none of them has been committed yet");

    // STOP ends the take.
    engine.stop();
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);
    assert!(!recorder.take_open());
    assert_eq!(history.depth().0, 1, "one step for the whole take");

    assert!(history.undo(&mut session));
    assert_eq!(notes(&session), 0, "one Cmd+Z removes the take");
}

/// §9 decision 1: STOP ends the take *and* switches REC off, so a stray key
/// after stopping does not land in the next PLAY.
#[test]
fn stop_disarms_so_the_next_play_does_not_record_by_accident() {
    let ctx = egui::Context::default();
    let (keys, inputs) = keyboard();
    let mut engine = EngineLink::with_sinks_and_input(sinks(), inputs);
    let mut session = session();
    let mut recorder = Recorder::default();
    let mut history = History::default();

    engine.reroute(&session);
    recorder.set_armed(true);
    engine.play(&session);
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);
    play(&keys, LiveKind::NoteOn { pitch: 60, velocity: 100 });
    play(&keys, LiveKind::NoteOff { pitch: 60 });
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 2);
    let after_take = notes(&session);
    assert!(after_take > 0);

    engine.stop();
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);
    assert!(!recorder.armed(), "STOP switched REC off");

    // Play again and hit a key. Nothing must be captured.
    engine.play(&session);
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);
    play(&keys, LiveKind::NoteOn { pitch: 72, velocity: 100 });
    play(&keys, LiveKind::NoteOff { pitch: 72 });
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);
    assert_eq!(notes(&session), after_take, "the second pass recorded nothing");
    engine.stop();
}

/// A take with nothing in it is not a take: no history step, no console line.
/// Arming and pressing STOP is a thing people do by accident constantly.
#[test]
fn arming_and_stopping_without_playing_a_note_leaves_no_trace() {
    let ctx = egui::Context::default();
    let (_keys, inputs) = keyboard();
    let mut engine = EngineLink::with_sinks_and_input(sinks(), inputs);
    let mut session = session();
    let mut recorder = Recorder::default();
    let mut history = History::default();

    engine.reroute(&session);
    recorder.set_armed(true);
    engine.play(&session);
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);
    engine.stop();
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);

    assert_eq!(history.depth(), (0, 0), "nothing to undo");
    assert!(
        !said(&ctx).iter().any(|line| line.starts_with("REC: ")),
        "and nothing to read: {:?}",
        said(&ctx)
    );
}

/// The report reaches the console when the take ends, naming the track. It is
/// the only place a dropped note is ever mentioned, so it has to be posted.
#[test]
fn the_take_says_what_it_did_when_it_ends() {
    let ctx = egui::Context::default();
    let (keys, inputs) = keyboard();
    let mut engine = EngineLink::with_sinks_and_input(sinks(), inputs);
    let mut session = session();
    let mut recorder = Recorder::default();
    let mut history = History::default();

    engine.reroute(&session);
    recorder.set_armed(true);
    engine.play(&session);
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);
    play(&keys, LiveKind::NoteOn { pitch: 60, velocity: 100 });
    play(&keys, LiveKind::NoteOff { pitch: 60 });
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 2);
    engine.stop();
    frames(&ctx, &mut engine, &mut session, &mut recorder, &mut history, 3);

    let lines = said(&ctx);
    let report = lines
        .iter()
        .find(|line| line.starts_with("REC: "))
        .unwrap_or_else(|| panic!("no report in {lines:?}"));
    assert!(report.contains("onto DT2 T1"), "{report}");
    assert!(report.contains("pass"), "{report}");
}

/// Thru is unconditional — armed or not, playing or stopped — so the recorder
/// points the monitor at the selection on every frame, including the very first
/// one, before anything is armed. Decision 2.
#[test]
fn thru_is_pointed_at_the_selection_without_anything_being_armed() {
    let ctx = egui::Context::default();
    let (_keys, inputs) = keyboard();
    let mut engine = EngineLink::with_sinks_and_input(sinks(), inputs);
    let mut session = session();
    let mut recorder = Recorder::default();
    let mut history = History::default();

    engine.reroute(&session);
    assert_eq!(engine.monitor(), None, "nothing has told it yet");
    frame(&ctx, &mut engine, &mut session, &mut recorder, &mut history);
    assert!(
        engine.monitor().is_some(),
        "one frame of the shell is all it takes, with REC off and the transport stopped"
    );
    assert!(!recorder.armed());
}
