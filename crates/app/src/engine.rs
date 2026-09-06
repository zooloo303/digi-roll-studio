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
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;

use digi_core::audition::track_level_message;
use digi_core::device::{DeviceId, PortRef};
use digi_core::record::PlacedEvent;
use digi_core::Session;
use digi_engine::event::{MidiMsg, PortId, PortTable};
use digi_engine::scheduler::{intern_ports, Scheduler};
use digi_engine::sink::MidirSink;
use digi_engine::transport::{PortSink, Transport, TransportCommand, TransportState};
use digi_midi::live_input::{LiveEvent, LiveInput};
use digi_midi::PortBinding;

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

/// Opens the record input — MIDI_RECORD_DESIGN.md §5.3.
///
/// Injected for [`SinkFactory`]'s reason and one more of its own: a test that
/// drove this through `midir` would need a keyboard plugged into the machine
/// running it, which no test in this repo may need. The handle is opaque —
/// **dropping it closes the port** and that is the whole of its interface — so a
/// test can hand back anything at all, including the `Sender` it means to push
/// events down.
pub type InputFactory =
    Box<dyn Fn(&PortRef, Sender<LiveEvent>) -> Result<Box<dyn Send>, String>>;

/// The real one: a `midir` input connection, filtered down to notes.
pub fn midir_input() -> InputFactory {
    Box::new(|port, tx| {
        let binding = PortBinding { id: port.id.clone(), name: port.name.clone() };
        LiveInput::open(&binding, tx)
            .map(|input| Box::new(input) as Box<dyn Send>)
            .map_err(|e| format!("{}: {e}", port.name))
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

    // --- recording, MIDI_RECORD_DESIGN.md §4.4 ------------------------------
    open_input: InputFactory,
    /// The open record input. Opaque and held only to keep the port open, the
    /// same contract `SysExInbox`'s connection has; dropped and reopened on
    /// every rebuild, because the `Sender` inside it belongs to the engine
    /// thread that is going away.
    live_input: Option<Box<dyn Send>>,
    /// What `live_input` was opened against, so `reroute` can tell a keyboard
    /// that moved from one that did not.
    record_input: Option<PortRef>,
    /// The open error, if the record input would not open. Joins `failed` so the
    /// status strip and the console see it the way they see a box's port.
    input_failure: Option<String>,
    /// Placed events coming back from the engine thread. `None` until the first
    /// rebuild; replaced by every one after it.
    placed_rx: Option<Receiver<PlacedEvent>>,
    /// The track thru is echoing to, as a *selection* rather than as a resolved
    /// `(PortId, u8)`.
    ///
    /// **Deliberately not the resolved pair.** A `PortId` is an index into the
    /// table the open sink was built against, and a rebuild renumbers it — so
    /// remembering the resolved answer across a rebuild is how thru ends up
    /// playing the wrong box after a cable is plugged in. Remembering the
    /// selection and resolving it again is the same discipline `scheduler::prepare`
    /// keeps for the cursors.
    monitor_track: Option<(DeviceId, usize)>,
    /// The last resolved pair actually sent, so the per-frame re-send in
    /// `Recorder::tick` costs a comparison rather than a command.
    monitor_sent: Option<(PortId, u8)>,
    armed: bool,
    record_target: Option<(DeviceId, usize)>,
    quantize: bool,
}

impl Default for EngineLink {
    fn default() -> Self {
        Self::with_sinks_and_input(midir_sinks(), midir_input())
    }
}

impl EngineLink {
    /// A link with a test's sink and the real record input. The input opens
    /// nothing until a session names one, so this stays hardware-free for every
    /// test that does not set `Session::record_input`.
    pub fn with_sinks(open_sinks: SinkFactory) -> Self {
        Self::with_sinks_and_input(open_sinks, midir_input())
    }

    pub fn with_sinks_and_input(open_sinks: SinkFactory, open_input: InputFactory) -> Self {
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
            open_input,
            live_input: None,
            record_input: None,
            input_failure: None,
            placed_rx: None,
            monitor_track: None,
            monitor_sent: None,
            armed: false,
            record_target: None,
            quantize: false,
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
        // **The record input is compared too.** It is not in the port table —
        // that table is outputs, and this is an input on a different OS
        // namespace — so without this line a keyboard picked in Setup would not
        // reach the engine until something else happened to force a rebuild.
        // Same treatment as a box's output for the same reason: a replugged
        // keyboard comes back on its own.
        if self.transport.is_some()
            && wanted == self.ports
            && session.record_input == self.record_input
        {
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
        // Recording's two channels, fresh per engine. The old `LiveInput` is
        // dropped first: it holds the `Sender` for a thread that is already
        // gone, and two connections to one input port is the same mistake two
        // connections to one output port would be.
        self.live_input = None;
        let (live_tx, live_rx) = std::sync::mpsc::channel();
        let (placed_tx, placed_rx) = std::sync::mpsc::channel();
        self.placed_rx = Some(placed_rx);
        self.record_input = session.record_input.clone();
        self.input_failure = None;
        if let Some(port) = &self.record_input {
            match (self.open_input)(port, live_tx) {
                Ok(input) => self.live_input = Some(input),
                Err(e) => self.input_failure = Some(e),
            }
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
            live_rx,
            placed_tx,
        ));
        self.ports = wanted;
        self.failed = failed;
        if let Some(line) = &self.input_failure {
            self.failed.push(line.clone());
        }
        self.rebuilds += 1;

        // **Everything the new thread cannot know.** A rebuild is a new
        // `EngineThread` with a `None` monitor and REC off, so thru would go
        // silent and a take would stop capturing the moment a box was plugged
        // in. The monitor is *re-resolved* rather than re-sent, because the
        // `PortId` it was last sent as is an index into a table that has just
        // been renumbered.
        self.monitor_sent = None;
        self.push_monitor(session);
        self.send(TransportCommand::SetRecord {
            armed: self.armed,
            target: self.record_target,
            quantize: self.quantize,
        });

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
        let index = track;
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
        let Some((port, _)) = self.resolve_track_port(session, device.id, index) else {
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

    /// The port and channel a track's notes go out on — **the one place that
    /// rule is spelled for a control the user is turning.**
    ///
    /// The track's own `out_port` if it has one, else its device's output,
    /// looked up in the table the open sink is indexed by. That is
    /// `scheduler::prepare`'s rule for playback; this is the same rule for a
    /// fader and for thru, and it is a function rather than three copies
    /// because DEVELOPMENT.md lesson 5 is exactly this shape — one rule, two
    /// places, and the second one forgotten.
    ///
    /// `None` covers a device this session does not have, a track the sounding
    /// pattern does not have, and a track routed to a port that is not open.
    pub fn resolve_track_port(
        &self,
        session: &Session,
        device: DeviceId,
        track: usize,
    ) -> Option<(PortId, u8)> {
        let device = session.devices.iter().find(|d| d.id == device)?;
        let track = session.current_pattern(device.id)?.track(track)?;
        let name = match &track.out_port {
            Some(name) => Some(name.as_str()),
            None => device.io.output.as_ref().map(|p| p.name.as_str()),
        };
        Some((self.ports.get(name?)?, track.channel))
    }

    // ------------------------------------------------------------- recording

    /// Point thru at a track, or at nothing — MIDI_RECORD_DESIGN.md §4.2.
    ///
    /// Cheap to call every frame, and it is: the resolved pair is compared
    /// against the last one sent and only a difference costs a command. That is
    /// what makes "select a track, play the keyboard, hear that box" hold
    /// without any caller remembering to tell the engine.
    ///
    /// A track routed nowhere resolves to `None`, which goes quiet rather than
    /// leaving thru pointed at whatever was selected before.
    pub fn set_monitor(&mut self, session: &Session, track: Option<(DeviceId, usize)>) {
        self.monitor_track = track;
        self.push_monitor(session);
    }

    /// Resolve [`Self::monitor_track`] against the table as it now stands, and
    /// send it if it differs from what the thread was last told.
    fn push_monitor(&mut self, session: &Session) {
        let resolved = self
            .monitor_track
            .and_then(|(device, track)| self.resolve_track_port(session, device, track));
        if resolved == self.monitor_sent {
            return;
        }
        self.monitor_sent = resolved;
        self.send(TransportCommand::SetMonitor(resolved));
    }

    /// Where thru is pointed right now, as the engine has been told it.
    pub fn monitor(&self) -> Option<(PortId, u8)> {
        self.monitor_sent
    }

    /// Arm or disarm, name the track a take lands on, and set QUANTIZE.
    ///
    /// Remembered here for the reason the scene and FILL are: a rebuild is a new
    /// thread that knows none of it, and a take that stopped capturing because
    /// somebody plugged a box in would be the worst possible time to find out.
    pub fn set_record(
        &mut self,
        armed: bool,
        target: Option<(DeviceId, usize)>,
        quantize: bool,
    ) {
        if armed == self.armed && target == self.record_target && quantize == self.quantize {
            return;
        }
        self.armed = armed;
        self.record_target = target;
        self.quantize = quantize;
        self.send(TransportCommand::SetRecord { armed, target, quantize });
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Everything the engine has placed since the last call, appended to `out`.
    ///
    /// `out` is the caller's, reused frame to frame, so a take that is running
    /// costs no allocation on the UI thread either.
    pub fn drain_placed(&mut self, out: &mut Vec<PlacedEvent>) {
        let Some(rx) = &self.placed_rx else { return };
        loop {
            match rx.try_recv() {
                Ok(event) => out.push(event),
                // Disconnected means the thread has gone and a rebuild is about
                // to replace this receiver. Nothing more will arrive on it, and
                // that is not an error.
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return,
            }
        }
    }

    /// Whether a record input is open. The REC button is disabled without one,
    /// and says so.
    pub fn record_input_open(&self) -> bool {
        self.live_input.is_some()
    }

    /// Why the record input would not open, if it would not.
    pub fn input_failure(&self) -> Option<&str> {
        self.input_failure.as_deref()
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
