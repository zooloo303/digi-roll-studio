// The join between the UI thread and the engine thread.
//
// PLAN.md §4 puts the split at a channel: the UI never touches the scheduler,
// and the engine thread never blocks on the UI. What is left over is the
// bookkeeping neither side owns — which ports this session needs, whether the
// connections that are open still match them, and turning the `Session` the user
// is editing into the `Arc` snapshot the engine reads. That is this file.
//
// No egui here. The decisions are testable without a window (see
// `tests/engine_link.rs`) and the widget in `ui::transport` is only buttons.

use crate::plocks::CuratedPLocks;
use std::sync::Arc;

use digi_core::Session;
use digi_engine::event::PortTable;
use digi_engine::scheduler::{intern_ports, Scheduler};
use digi_engine::sink::MidirSink;
use digi_engine::transport::{PortSink, Transport, TransportCommand, TransportState};

/// Opens the connections a port table names.
///
/// Injected rather than called directly so the link can be driven in a test
/// against a recording sink: nothing here should need a MIDI stack to be
/// exercised, which is the same reason `PortSink` is a trait at all.
///
/// Returns the sink plus a line per port that would not open. A box silent
/// because another app holds its port has to say so, or it reads as a sequencer
/// bug.
pub type SinkFactory = Box<dyn Fn(&PortTable) -> (Box<dyn PortSink>, Vec<String>)>;

/// The real one: a `midir` connection per port.
pub fn midir_sinks() -> SinkFactory {
    Box::new(|ports| {
        let (sink, failed) = MidirSink::open(ports);
        let failed = failed
            .iter()
            .map(|(id, e)| format!("{}: {e}", ports.name(*id).unwrap_or("?")))
            .collect();
        (Box::new(sink), failed)
    })
}

/// A running engine, and everything needed to rebuild it when the routing moves.
pub struct EngineLink {
    open_sinks: SinkFactory,
    /// `None` until the first `reroute`, and between a shutdown and the next one.
    transport: Option<Transport>,
    state: Arc<TransportState>,
    /// The table the open sink is indexed by. Every `PortId` in flight is a
    /// position in this.
    ports: PortTable,
    failed: Vec<String>,
    send_clock: bool,
    fill: bool,
    /// The scene the user has asked for. Not necessarily the one sounding — that
    /// is the engine's answer, and it only changes at a boundary — but it is the
    /// one a rebuilt engine starts on, alongside `send_clock` and `fill`, because
    /// a rebuild is a new scheduler that knows none of them.
    scene: usize,
    scene_immediate: bool,
    rebuilds: u64,
}

impl Default for EngineLink {
    fn default() -> Self {
        Self::with_sinks(midir_sinks())
    }
}

impl EngineLink {
    pub fn with_sinks(open_sinks: SinkFactory) -> Self {
        Self {
            open_sinks,
            transport: None,
            // Stands in until there is an engine, so the UI can read a playhead
            // and a playing flag before anything has been spawned.
            state: Arc::new(TransportState::default()),
            ports: PortTable::new(),
            failed: Vec::new(),
            send_clock: true,
            fill: false,
            scene: 0,
            scene_immediate: false,
            rebuilds: 0,
        }
    }

    /// Rebuild the engine if this session no longer routes to the ports the open
    /// sink was built for. Returns whether it rebuilt.
    ///
    /// Cheap enough to call every frame: it interns a handful of names and
    /// compares. That is deliberate — identifying a box gives it an out port, and
    /// the engine should pick that up without anyone remembering to tell it.
    ///
    /// A rebuild is a whole new engine thread, because the sink is owned by that
    /// thread and a `midir` connection cannot be added to one already running.
    /// It is also the only correct answer: a new port means a new id, and every
    /// id already in the queue is an index into the connections that are open.
    pub fn reroute(&mut self, session: &Session) -> bool {
        let mut wanted = PortTable::new();
        intern_ports(session, &mut wanted);
        if self.transport.is_some() && wanted == self.ports {
            return false;
        }

        let was_playing = self.is_playing();
        // Drop the old handle *before* opening the new sink. Its `Drop` stops the
        // boxes and joins the thread, and the ports it holds are usually the ones
        // about to be reopened — two connections to one port is how a note gets
        // stuck with nothing left owning it.
        self.transport = None;
        self.state = Arc::new(TransportState::with_ports(wanted.len()));

        let (sink, failed) = (self.open_sinks)(&wanted);
        let mut scheduler = Scheduler::new(session.tempo_bpm);
        scheduler.send_clock = self.send_clock;
        scheduler.fill_active = self.fill;
        // Before `prepare`, because the scene decides which pattern each cursor
        // is built against. A queued switch does not survive a rebuild: the
        // rebuild restarts from the top anyway, so the scene that was asked for
        // is the one that starts.
        self.scene = self.scene.min(session.scenes.len().saturating_sub(1));
        scheduler.commit_scene(session, self.scene);
        self.state
            .playing_scene
            .store(self.scene, std::sync::atomic::Ordering::Relaxed);
        scheduler.prepare(session, &mut wanted);
        self.transport = Some(Transport::spawn(
            Arc::new(session.clone()),
            scheduler,
            sink,
            Arc::clone(&self.state),
            // The per-box parameter tables the engine is not allowed to know
            // about. Built here rather than held, because it is only correct for
            // the session this transport is being spawned against.
            Box::new(CuratedPLocks::new(session)),
        ));
        self.ports = wanted;
        self.failed = failed;
        self.rebuilds += 1;

        // Plugging a box in mid-set should not end the set. It does restart it
        // from the top rather than resuming: the cursors live in the scheduler,
        // and this is a new scheduler. Continue would claim to resume and play
        // from step 0 anyway, which is the worse of the two.
        if was_playing {
            self.send(TransportCommand::Start);
        }
        true
    }

    /// Hand the engine the session as it now stands.
    ///
    /// Cheap: cloning a `Session` shares every track until one is written, since
    /// `Pattern` holds `Arc<Track>`. Editing one note copies one track.
    pub fn sync(&mut self, session: &Session) {
        if self.reroute(session) {
            // The rebuild spawned against this very session; a snapshot now would
            // only repeat it.
            return;
        }
        self.send(TransportCommand::Snapshot {
            session: Arc::new(session.clone()),
            ports: self.ports.clone(),
        });
    }

    /// From the top.
    pub fn play(&mut self, session: &Session) {
        self.sync(session);
        self.send(TransportCommand::Start);
    }

    /// From where the cursors are.
    pub fn resume(&mut self, session: &Session) {
        self.sync(session);
        self.send(TransportCommand::Continue);
    }

    pub fn stop(&mut self) {
        self.send(TransportCommand::Stop);
    }

    /// All Notes Off and All Sound Off on every channel in use — the button for
    /// when something is sounding and nothing else will release it.
    pub fn panic(&mut self) {
        self.send(TransportCommand::Panic);
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.send(TransportCommand::SetTempo(bpm));
    }

    pub fn fill(&self) -> bool {
        self.fill
    }

    pub fn set_fill(&mut self, on: bool) {
        self.fill = on;
        self.send(TransportCommand::SetFill(on));
    }

    /// Ask for a scene. Taken at the next boundary of the one playing, or at once
    /// if the transport is stopped or the immediate setting is on.
    ///
    /// Out of range is ignored rather than clamped — the engine ignores it too,
    /// and a caller asking for a scene that is not there has a bug that landing
    /// on the last one would hide.
    pub fn select_scene(&mut self, session: &Session, scene: usize) {
        if scene >= session.scenes.len() {
            return;
        }
        self.scene = scene;
        self.send(TransportCommand::SelectScene {
            scene,
            immediate: self.scene_immediate,
        });
    }

    /// Re-point the engine after the scene *list* changed under it.
    ///
    /// Removing a scene shifts every index above it, so the number the engine is
    /// holding stops naming the scene it was playing. Never queued: waiting for a
    /// boundary to correct an index that is already wrong would mean a bar of
    /// whatever that index now happens to point at.
    pub fn rebase_scene(&mut self, session: &Session, scene: usize) {
        if scene >= session.scenes.len() {
            return;
        }
        self.scene = scene;
        self.send(TransportCommand::SelectScene { scene, immediate: true });
    }

    /// The scene the user last asked for, which is the queued one while a switch
    /// is waiting for its boundary.
    pub fn selected_scene(&self) -> usize {
        self.scene
    }

    /// The scene actually sounding. The engine's answer while there is an engine:
    /// only it knows when the boundary went past.
    pub fn playing_scene(&self) -> usize {
        match self.transport {
            Some(_) => self.state.playing_scene(),
            None => self.scene,
        }
    }

    /// The scene waiting for a boundary, if one is.
    pub fn queued_scene(&self) -> Option<usize> {
        self.transport.as_ref()?;
        self.state.pending_scene()
    }

    /// PLAN.md §4's "immediate" setting: take a scene change without waiting for
    /// the boundary.
    pub fn scene_immediate(&self) -> bool {
        self.scene_immediate
    }

    pub fn set_scene_immediate(&mut self, on: bool) {
        self.scene_immediate = on;
    }

    pub fn send_clock(&self) -> bool {
        self.send_clock
    }

    pub fn set_send_clock(&mut self, on: bool) {
        self.send_clock = on;
        self.send(TransportCommand::SetSendClock(on));
    }

    pub fn is_playing(&self) -> bool {
        self.state.is_playing()
    }

    /// The playhead, in pattern steps since the transport started. Fractional and
    /// unwrapped: a track wraps it by its own length, which is what lets two
    /// tracks of different lengths share one number.
    pub fn position_steps(&self) -> f64 {
        self.state.position_steps()
    }

    pub fn active_notes(&self) -> usize {
        self.state
            .active_notes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The ports the engine is currently sending to, in `PortId` order.
    pub fn ports(&self) -> &PortTable {
        &self.ports
    }

    /// What would not open, one line each.
    pub fn failures(&self) -> &[String] {
        &self.failed
    }

    /// How many engine threads have been spawned. The UI does not show this; the
    /// tests count it, because "did that change rebuild the engine?" is the whole
    /// question this file answers.
    pub fn rebuilds(&self) -> u64 {
        self.rebuilds
    }

    pub fn running(&self) -> bool {
        self.transport.is_some()
    }

    /// Stop the engine and release its ports. The next `play` or `sync` spawns a
    /// new one.
    pub fn shutdown(&mut self) {
        self.transport = None;
    }

    fn send(&self, cmd: TransportCommand) {
        if let Some(transport) = &self.transport {
            transport.send(cmd);
        }
    }
}
