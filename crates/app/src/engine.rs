// The join between the UI thread and the engine thread.
//
// PLAN.md §4 puts the split at a channel: the UI never touches the scheduler,
// and the engine thread never blocks on the UI. What is left over is the
// bookkeeping neither side owns — which ports this session needs, whether the
// connections that are open still match them, and turning the `Session` the user
// is editing into the `Arc` snapshot the engine reads. That is this file.
//
// No egui here. The decisions are testable without a window (see
// `tests/all/engine_link.rs`) and the widget in `ui::transport` is only buttons.

use crate::plocks::CuratedPLocks;
use std::sync::Arc;

use digi_core::audition::track_level_message;
use digi_core::device::DeviceId;
use digi_core::Session;
use digi_engine::event::{MidiMsg, PortTable};
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
    /// Whether the transport is walking the song, and which row the SONG panel is
    /// pointing at.
    ///
    /// Both live here rather than in the session for the reason the scene does:
    /// *which arrangement exists* is the session's, *whether we are playing it*
    /// is the engine's. And both have to be remembered here, because a rebuild is
    /// a whole new scheduler that knows neither.
    song_mode: bool,
    song_row: usize,
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
            song_mode: false,
            song_row: 0,
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
        // After `prepare`, because the walker commits a scene and that has to
        // land on cursors that exist. A rebuild restarts the set from the top
        // (see below), so the song starts at the row the panel is on rather than
        // wherever the old scheduler had got to — there is no timeline left to
        // resume into.
        if self.song_mode {
            scheduler.set_song_mode(session, true, self.song_row, 0.0);
        }
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

    /// Send a track's LEVEL to the box it plays on, right now.
    ///
    /// **The port is resolved the way the scheduler resolves it** — the track's
    /// own `out_port` if it has one, else its device's output — because a fader
    /// that reached a different port from the notes would move some other box's
    /// track. `scheduler::prepare` writes that rule for playback; this is the
    /// same rule for a control the user is turning, and the only other place it
    /// is spelled.
    ///
    /// Returns whether anything went. `false` covers all four ways it cannot:
    /// no engine yet, a device this session cannot name a box for, a box with no
    /// published controller for level, and a track routed nowhere. A caller that
    /// wants to say "the fader moved but nothing heard it" has this to say it
    /// from; nothing here writes to the session.
    pub fn send_track_level(&self, session: &Session, device: DeviceId, track: usize) -> bool {
        let Some(device) = session.devices.iter().find(|d| d.id == device) else {
            return false;
        };
        // **Keyed off the param tables, not the SysEx spec** — the same
        // correction `plocks::CuratedPLocks` took on 2026-08-24 and the same
        // reason: hearing a fader and parsing a dump are different
        // capabilities. `model.spec()?.device` named the same set of boxes
        // right up to the moment a live-only one shipped, and then it made the
        // A4's VOL field a control that drags and sends nothing — the box has
        // a published chart (CC 95 / NRPN 1/100) and no dump format at all.
        //
        // Fixed here on 2026-08-28, against the box, four days after the
        // identical fix went into `CuratedPLocks` and did not travel. Two
        // places holding one rule, and the second one forgotten: lesson 5.
        let Some(kind) = digi_protocol::params::device_kind_key(device.model.key) else {
            return false;
        };
        let Some(track) = session
            .current_pattern(device.id)
            .and_then(|p| p.tracks().get(track).cloned())
        else {
            return false;
        };
        let Some(level) = track.level else {
            return false;
        };
        let Some(message) = track_level_message(kind, level) else {
            return false;
        };
        let name = match &track.out_port {
            Some(name) => Some(name.as_str()),
            None => device.io.output.as_ref().map(|p| p.name.as_str()),
        };
        let Some(port) = name.and_then(|n| self.ports.get(n)) else {
            return false;
        };
        // NRPN first, CC as the fallback — `plocks::CuratedPLocks` chooses in
        // this order and gives the three reasons. Level is a case where it
        // matters for a fourth: the boxes share the CC (95) and differ on the
        // NRPN, so the NRPN is the one that cannot be sent to the wrong box by
        // accident.
        let msg = match (message.nrpn, message.cc) {
            (Some((msb, lsb)), _) => MidiMsg::Nrpn {
                channel: track.channel,
                msb,
                lsb,
                value14: message.value14,
            },
            (None, Some(cc)) => MidiMsg::ControlChange {
                channel: track.channel,
                controller: cc,
                value: message.value7,
            },
            (None, None) => return false,
        };
        self.send(TransportCommand::SendNow(vec![(port, msg)]));
        self.transport.is_some()
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

    // ------------------------------------------------------------- song mode

    /// Whether the transport is walking the song.
    pub fn song_mode(&self) -> bool {
        self.song_mode
    }

    /// Walk the song, or stop walking it.
    ///
    /// Turning it on with no song built is allowed and does nothing audible: the
    /// mode is a standing request, and the first snapshot that gives the engine
    /// rows to walk starts it. That is what makes building a song with SONG lit
    /// behave the way it looks, rather than needing the toggle pressed twice.
    pub fn set_song_mode(&mut self, session: &Session, on: bool) {
        self.song_mode = on;
        if let Some(song) = session.song() {
            self.song_row = self.song_row.min(song.len().saturating_sub(1));
        }
        self.send(TransportCommand::SetSongMode { on, row: self.song_row });
    }

    /// Which row the panel is pointing at — the box's selected row, the one the
    /// editors write to and the one PLAY starts from.
    pub fn selected_row(&self) -> usize {
        self.song_row
    }

    /// Point the panel at a row without moving the playhead. The box's `[UP]`
    /// and `[DOWN]`: selecting a row is not jumping to it.
    pub fn select_row(&mut self, row: usize) {
        self.song_row = row;
    }

    /// Move the playhead to a row, and point the panel at it.
    pub fn jump_to_row(&mut self, row: usize) {
        self.song_row = row;
        self.send(TransportCommand::JumpToSongRow(row));
    }

    /// The row playing and which pass of it — the box's SONG POINTER. `None` in
    /// pattern mode, and while song mode has no rows to walk.
    pub fn song_position(&self) -> Option<(usize, u16)> {
        match self.transport {
            Some(_) => self.state.song_position(),
            None => None,
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
