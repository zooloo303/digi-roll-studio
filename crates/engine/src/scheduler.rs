//! Turning a session into a sorted queue of timed MIDI events.
//!
//! This is the whole of the engine's decision-making, and it is a **pure
//! function of the session, the tempo and a window of time** — no threads, no
//! `Instant`, no `midir`. The transport thread does nothing but ask for the next
//! window and hit the deadlines. That split is deliberate: timing is the part of
//! Phase 4 that cannot be unit-tested, so everything else is kept out of it.
//!
//! PLAN.md §4's ordering, and where each piece lives:
//!
//! - *one event queue whose entries carry a port* — [`crate::event`]
//! - *positions as fractional `f64` steps* — [`crate::time`]
//! - *note lifetime* — [`crate::notes`]
//! - *conditions* — [`crate::conditions`]
//! - the rest is here: cursors, polymeter, clock, mutes and solo.

use digi_core::device::DeviceId;
use digi_core::model::{PLockLane, Pattern, Track};
use digi_core::session::Session;

use crate::conditions::{should_play, CondHistory};
use crate::event::{MidiMsg, PortId, PortTable, ScheduledEvent};
use crate::notes::ActiveNotes;
use crate::rng::Rng;
use crate::time::{
    clock_tick_seconds, note_duration_seconds, swing_offset_steps, track_step_seconds,
    PLOCK_LEAD_SECONDS,
};

/// Where one track has got to.
///
/// `next_step` is **absolute and monotonic** — it never wraps, and it is what
/// dates every event, so it can never be rewound without sending the whole
/// timeline backwards.
///
/// `origin` is the step at which the pattern this track is playing *started*.
/// The step within the pattern and the loop number are both derived from the
/// difference, which is what makes polymeter fall out for free — two tracks of
/// different lengths divide the same counter differently, and neither needs to
/// know about the other — and what lets a scene change restart every track at
/// step 1 of its new pattern without touching the clock.
#[derive(Debug, Clone)]
pub struct TrackCursor {
    pub device: DeviceId,
    /// Index into `session.devices`, resolved once in [`Scheduler::prepare`].
    pub device_index: usize,
    pub track: usize,
    /// `None` when neither the track nor its device names a port that exists —
    /// the track is then silent rather than an error, the same way a device with
    /// no ports keeps its patterns.
    pub port: Option<PortId>,
    pub channel: u8,
    pub next_step: u64,
    /// Where the current pattern began, on the same absolute counter. Zero until
    /// a scene change moves it.
    pub origin: u64,
}

impl TrackCursor {
    /// How far into its pattern this track is: the position `next_step` occupies
    /// within a pattern that began at `origin`.
    fn elapsed_steps(&self) -> u64 {
        self.next_step.saturating_sub(self.origin)
    }
}

/// Turns a p-lock lane into the parameter messages that carry it.
///
/// Injected, exactly as `js/midi.js` injects `plockMessages`, and for the same
/// reason: the engine stays device-agnostic. Which CC or NRPN number a parameter
/// lives at is a per-box table (`digi_protocol::params`) that belongs nowhere
/// near the scheduler — pan is CC 90 on a DT2 and CC 89 on a DN2, where 89 is
/// Volume.
///
/// **This crate cannot implement it, by design.** `PLAN.md` §3 forbids `engine`
/// from depending on `protocol`, so the resolution lives in
/// `digi_core::audition` and the implementation in `app` — the same shape as
/// `js/main.js` injecting `js/roll-bridge.js`'s resolver into `js/midi.js`.
pub trait PLockMap {
    /// Messages for every lane that has a value at `step`, appended to `out`.
    fn messages(&self, device: DeviceId, channel: u8, lanes: &[PLockLane], step: u64, out: &mut Vec<MidiMsg>);
}

/// A `PLockMap` that emits nothing — the default, and what "the parameter tables
/// are unported" looks like at runtime.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPLocks;

impl PLockMap for NoPLocks {
    fn messages(&self, _: DeviceId, _: u8, _: &[PLockLane], _: u64, _: &mut Vec<MidiMsg>) {}
}

/// Every port name a session routes to, interned in the order
/// [`Scheduler::prepare`] expects to find them.
///
/// Split out of `prepare` because the two callers need it at different moments.
/// The UI has to answer *which connections does this session need?* before there
/// is a scheduler at all: it owns the sink, the sink is a vector indexed by the
/// ids this assigns, and it can only open a port once. So the UI interns first,
/// opens against the result, and hands the same table to `prepare`.
///
/// Interning is idempotent, so calling this on a table that already holds these
/// names changes nothing — which is how a session can be re-prepared mid-play
/// without the queue's port ids moving underneath it.
///
/// A device's own out port comes first, then any track that overrides it, in
/// track order. Two devices on one port share an id, which is the point: one
/// connection, one clock.
///
/// **Every scene, not just the one playing.** A scene change must not change the
/// port table, because the UI compares this table against the connections it has
/// open and rebuilds the engine when they differ — so a scene whose patterns
/// route somewhere new would tear the engine down and restart the set at the
/// exact moment the switch was supposed to be seamless. Interning the whole
/// session up front costs one connection per port a scene *could* use, which is
/// the price of switching without a gap.
pub fn intern_ports(session: &Session, ports: &mut PortTable) {
    for device in &session.devices {
        if let Some(port) = &device.io.output {
            ports.intern(&port.name);
        }
        for scene in 0..session.scenes.len() {
            let Some(pattern) = session.pattern_in_scene(scene, device.id) else {
                continue;
            };
            for track in pattern.tracks() {
                if let Some(name) = &track.out_port {
                    ports.intern(name);
                }
            }
        }
    }
}

/// How long one full cycle of `scene` lasts, in seconds: the longest track in
/// it, across every device.
///
/// This is [`Session::scene_boundary_steps`] answered in the unit a scene change
/// actually needs. The model can only count *steps*, and steps are not all the
/// same length: SCALE is per track, so a 16-step track at 2x wraps twice as often
/// as a 16-step track at 1x, and the longest track by step count is not
/// necessarily the longest by time. Tempo and SCALE both live out here, so this
/// does too — and where every track is at 1x the two agree exactly, which is what
/// the tests pin.
///
/// `None` when the scene names no track that can wrap at all, which is the
/// "switch at once, there is no boundary to wait for" case.
pub fn scene_cycle_seconds(session: &Session, scene: usize, bpm: f64) -> Option<f64> {
    session
        .devices
        .iter()
        .filter_map(|d| session.pattern_in_scene(scene, d.id))
        .flat_map(|p| p.tracks().iter())
        .filter(|t| t.length_steps > 0)
        .map(|t| t.length_steps as f64 * track_step_seconds(bpm, t.scale))
        .fold(None, |longest: Option<f64>, cycle| {
            Some(longest.map_or(cycle, |l| l.max(cycle)))
        })
}

/// A scene change that has been asked for and not yet taken.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingScene {
    scene: usize,
    /// Seconds since the transport started, the moment the switch takes effect.
    at: f64,
}

/// The engine's playback state, between windows.
#[derive(Debug)]
pub struct Scheduler {
    pub bpm: f64,
    /// The transport's FILL control. Read by every trig carrying a FILL lock.
    pub fill_active: bool,
    /// Whether MIDI clock goes out at all. Per device beyond this — a box slaved
    /// to something else must not be fought over — but this is the master switch.
    pub send_clock: bool,
    /// **The scene that is sounding, which is the engine's and not the
    /// session's.** `Session::current_scene` is what the UI shows and edits; this
    /// is what plays. They differ for exactly as long as a switch is queued, and
    /// they have to: a scene change is taken at a boundary (PLAN.md §4), while an
    /// edit snapshot arrives whenever the user moves a note. If playback read
    /// `current_scene`, every snapshot would carry the switch with it and the
    /// queue would mean nothing.
    scene: usize,
    pending: Option<PendingScene>,
    cursors: Vec<TrackCursor>,
    /// One per device, in `session.devices` order. Never shared between boxes:
    /// `NEI` is a physical neighbour on one machine.
    histories: Vec<CondHistory>,
    /// Each device's own out port, by `session.devices` index — what a track
    /// falls back to when it does not override the routing. Kept so a scene
    /// change can re-resolve a cursor's port without re-interning anything.
    device_ports: Vec<Option<PortId>>,
    /// A copy of the table [`Scheduler::prepare`] resolved against, so a scene
    /// taken mid-window can look a port name up without the caller handing one
    /// back. Read only, and never interned into: the ids in it are indexes into
    /// connections somebody else has already opened.
    ports: PortTable,
    /// The out ports of every device that takes clock. One tick counter feeds
    /// all of them, so nothing gets its own clock.
    clock_ports: Vec<PortId>,
    next_clock_tick: u64,
    active: ActiveNotes,
    /// Scratch, reused across windows so the p-lock path allocates once.
    plock_scratch: Vec<MidiMsg>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new(120.0)
    }
}

impl Scheduler {
    pub fn new(bpm: f64) -> Self {
        Self {
            bpm,
            fill_active: false,
            send_clock: true,
            scene: 0,
            pending: None,
            cursors: Vec::new(),
            histories: Vec::new(),
            device_ports: Vec::new(),
            ports: PortTable::new(),
            clock_ports: Vec::new(),
            next_clock_tick: 0,
            active: ActiveNotes::new(),
            plock_scratch: Vec::with_capacity(32),
        }
    }

    pub fn active_notes(&self) -> &ActiveNotes {
        &self.active
    }

    pub fn cursors(&self) -> &[TrackCursor] {
        &self.cursors
    }

    /// The scene that is sounding.
    pub fn scene(&self) -> usize {
        self.scene
    }

    /// The scene that has been asked for and is waiting for a boundary.
    pub fn pending_scene(&self) -> Option<usize> {
        self.pending.map(|p| p.scene)
    }

    /// When a queued switch takes effect, in seconds since the transport
    /// started. The UI does not show this yet; the tests read it, because "at
    /// which boundary" is the whole question a queued scene asks.
    pub fn scene_switch_at(&self) -> Option<f64> {
        self.pending.map(|p| p.at)
    }

    /// The pattern a device plays in the scene that is sounding.
    fn pattern<'a>(&self, session: &'a Session, device: DeviceId) -> Option<&'a Pattern> {
        session.pattern_in_scene(self.scene, device)
    }

    /// Where a track is within its pattern right now, for the UI's playhead.
    pub fn step_in_pattern(&self, session: &Session, cursor: &TrackCursor) -> Option<u64> {
        let track = self.pattern(session, cursor.device)?.track(cursor.track)?;
        (track.length_steps > 0).then(|| cursor.elapsed_steps() % track.length_steps as u64)
    }

    /// Resolve a session into cursors, histories and clock ports.
    ///
    /// Called on the UI thread whenever the session snapshot changes — this is
    /// the step that allocates, so the engine thread does not have to. Cursors
    /// keep their `next_step` across a rebuild wherever the same
    /// `(device, track)` still exists, so editing a note mid-play does not
    /// restart every track.
    ///
    /// Ports are resolved through [`intern_ports`] and then read with
    /// `PortTable::get`, never interned again here. That is what stops a
    /// re-prepare renumbering a table the caller has already opened a sink
    /// against: a name this does not find is a silent track, which is visible,
    /// rather than an id past the end of the connections, which would be a track
    /// sending to the wrong box.
    pub fn prepare(&mut self, session: &Session, ports: &mut PortTable) {
        let previous: Vec<(DeviceId, usize, u64, u64)> = self
            .cursors
            .iter()
            .map(|c| (c.device, c.track, c.next_step, c.origin))
            .collect();

        self.cursors.clear();
        self.histories.clear();
        self.clock_ports.clear();
        self.device_ports.clear();
        intern_ports(session, ports);
        self.ports = ports.clone();

        for (device_index, device) in session.devices.iter().enumerate() {
            self.histories.push(CondHistory::new(device.model.num_tracks));

            let device_port = device.io.output.as_ref().and_then(|p| ports.get(&p.name));
            self.device_ports.push(device_port);
            if device.io.takes_clock {
                if let Some(id) = device_port {
                    if !self.clock_ports.contains(&id) {
                        self.clock_ports.push(id);
                    }
                }
            }

            let Some(pattern) = session.pattern_in_scene(self.scene, device.id) else {
                continue;
            };
            for (track_index, track) in pattern.tracks().iter().enumerate() {
                // A track's own port wins over its device's: routing is studio
                // state and a MIDI track may well go somewhere else entirely.
                let port = match &track.out_port {
                    Some(name) => ports.get(name),
                    None => device_port,
                };
                let (next_step, origin) = previous
                    .iter()
                    .find(|(d, t, _, _)| *d == device.id && *t == track_index)
                    .map(|(_, _, s, o)| (*s, *o))
                    .unwrap_or((0, 0));
                self.cursors.push(TrackCursor {
                    device: device.id,
                    device_index,
                    track: track_index,
                    port,
                    channel: track.channel,
                    next_step,
                    origin,
                });
            }
        }
    }

    /// Rewind everything to the top and forget all history. Start does this;
    /// continue does not.
    pub fn rewind(&mut self) {
        for c in &mut self.cursors {
            c.next_step = 0;
            c.origin = 0;
        }
        for h in &mut self.histories {
            h.clear();
        }
        self.next_clock_tick = 0;
        // A queued switch does not survive a rewind: its boundary was a moment on
        // the timeline that is about to stop existing.
        self.pending = None;
    }

    /// Ask for a scene, taken at the next boundary of the scene that is playing.
    ///
    /// `not_before` is the point on the timeline the caller has already committed
    /// to the wire — its scheduling horizon. The switch lands at the first
    /// boundary at or after it, never inside it: the transport has up to
    /// [`crate::transport::LOOKAHEAD`] of the outgoing scene already queued and
    /// dated, and taking a switch behind that would mean unpicking events that
    /// are on their way out. So a scene asked for in the last 50 ms of a bar
    /// takes the *following* boundary, which is a lot better than half a bar of
    /// the wrong pattern.
    ///
    /// `immediate` is PLAN.md §4's "immediate setting" — it still means "at the
    /// end of what is already committed", because nothing can be taken back off
    /// the wire, and that is at most 50 ms.
    ///
    /// A scene this session does not have is ignored rather than clamped: the
    /// caller asking for scene 4 of three has a bug, and playing scene 3 would
    /// hide it.
    pub fn queue_scene(
        &mut self,
        session: &Session,
        scene: usize,
        not_before: f64,
        immediate: bool,
    ) {
        if scene >= session.scenes.len() {
            return;
        }
        let cycle = scene_cycle_seconds(session, self.scene, self.bpm);
        match cycle {
            Some(cycle) if cycle > 0.0 && !immediate => {
                // The next multiple of the cycle at or after the horizon. An
                // exact multiple is already a boundary and is taken there.
                let at = (not_before / cycle).ceil() * cycle;
                self.pending = Some(PendingScene { scene, at });
            }
            // Nothing to wait for: a scene whose every track is zero-length has
            // no boundary, and asking to wait for one would queue a switch that
            // never came.
            _ => self.commit_scene(session, scene),
        }
    }

    /// Take a queued scene now, wherever the cursors are.
    ///
    /// Every track restarts at step 1 of its new pattern, which is what a pattern
    /// change does on the box, and is why [`TrackCursor::origin`] exists: the
    /// absolute counter dates every event and cannot go backwards, so the
    /// *pattern* is moved to meet it rather than the other way round. `1ST` fires
    /// again for the same reason, and the condition history is cleared — carrying
    /// one pattern's `PRE` chain into the next would make the first bar of a scene
    /// depend on what happened to be playing before it.
    pub fn commit_scene(&mut self, session: &Session, scene: usize) {
        // Cleared first, and unconditionally. A scene deleted while its switch
        // was queued would otherwise leave a pending that can never be taken and
        // that `advance` would try to take on every pass — a spin, not a silence.
        self.pending = None;
        if scene >= session.scenes.len() {
            return;
        }
        self.scene = scene;
        for cursor in &mut self.cursors {
            cursor.origin = cursor.next_step;
            // Channel and out port are the *pattern's*, so two scenes on one box
            // can route differently. The port table is the one already opened
            // against, never re-interned: a name it does not hold is a silent
            // track, which is visible, rather than an id past the end of the
            // connections, which would be a trig coming out of the wrong box.
            let Some(track) = session
                .pattern_in_scene(scene, cursor.device)
                .and_then(|p| p.track(cursor.track))
            else {
                continue;
            };
            cursor.channel = track.channel;
            cursor.port = match &track.out_port {
                Some(name) => self.ports.get(name),
                None => self.device_ports.get(cursor.device_index).copied().flatten(),
            };
        }
        for h in &mut self.histories {
            h.clear();
        }
    }

    /// Every `(port, channel)` the session can currently sound on — what a panic
    /// has to shout at.
    pub fn channels_in_use(&self) -> Vec<(PortId, u8)> {
        let mut out: Vec<(PortId, u8)> = Vec::new();
        for c in &self.cursors {
            if let Some(port) = c.port {
                if !out.contains(&(port, c.channel)) {
                    out.push((port, c.channel));
                }
            }
        }
        out
    }

    /// Emit everything falling before `to`, sorted.
    ///
    /// `to` is seconds since the transport started. There is no `from`: the
    /// scheduler remembers where every track and the clock got to, so a window is
    /// defined by its far edge alone and no caller can accidentally emit a step
    /// twice or skip one by miscomputing the near edge.
    ///
    /// The caller walks the returned events in order and sends each at its
    /// deadline; nothing here knows what time it is.
    pub fn advance(
        &mut self,
        session: &Session,
        to: f64,
        rng: &mut dyn Rng,
        plocks: &dyn PLockMap,
        out: &mut Vec<ScheduledEvent>,
    ) {
        self.emit_clock(to, out);

        // Solo is session-wide (PLAN.md §2: soloing a DT2 track silences DN2
        // tracks too), and answering it walks every pattern — so it is answered
        // once per window rather than once per trig.
        let any_solo = session.any_solo();

        // **Tracks advance in deadline order, not one track at a time.**
        //
        // Running each track to the horizon before starting the next would be
        // simpler and is wrong twice over. `NEI` reads the last condition result
        // on the neighbouring track, so a track that has already run to the end
        // of the window hands its neighbour the *wrong bar's* answer. And the
        // result would then depend on how the caller chopped the timeline up,
        // which is exactly what the transport thread cannot guarantee — a window
        // is however long the last wake-up took.
        //
        // Picking the earliest pending step across every cursor makes both
        // problems go away: the engine walks the timeline once, in order, whatever
        // the window boundaries are. Ties go to the lower cursor index — track
        // order within a device, device order across the session — so a run is
        // reproducible.
        loop {
            let mut next: Option<(usize, f64)> = None;
            for index in 0..self.cursors.len() {
                let Some(deadline) = self.cursor_deadline(session, index) else {
                    continue;
                };
                if deadline < to && next.is_none_or(|(_, best)| deadline < best) {
                    next = Some((index, deadline));
                }
            }

            // **A queued scene is taken here, in the same walk as the trigs.**
            // The boundary is a point on the timeline like any other, so the
            // switch competes for the earliest deadline and the events either
            // side of it come out in the right order whatever the window
            // boundaries were. A trig landing exactly on the boundary belongs to
            // the *incoming* scene — that trig is step 1 of the new pattern —
            // which is why the comparison is `<=` and not `<`.
            if let Some(pending) = self.pending {
                if pending.at < to && next.is_none_or(|(_, deadline)| pending.at <= deadline) {
                    self.commit_scene(session, pending.scene);
                    continue;
                }
            }

            let Some((index, deadline)) = next else { break };
            self.step_cursor(session, index, deadline, any_solo, rng, plocks, out);
        }

        // Note-offs whose deadline has arrived. After the tracks, so a note-on
        // scheduled in this same window is already in the table and a note that
        // starts and ends inside one window still gets its off.
        self.active.drain_due(to, out);

        crate::event::sort_events(out);
    }

    /// MIDI clock: one tick counter, every device that takes it, in step.
    ///
    /// Nothing gets its own clock — that is what stops two boxes drifting apart
    /// while both believe they are following us.
    fn emit_clock(&mut self, to: f64, out: &mut Vec<ScheduledEvent>) {
        if !self.send_clock || self.clock_ports.is_empty() {
            // Keep the counter aligned with the timeline in case clock is turned
            // back on mid-play, rather than resuming from an old tick number.
            if !self.send_clock {
                self.next_clock_tick = (to / clock_tick_seconds(self.bpm)).ceil().max(0.0) as u64;
            }
            return;
        }
        let tick = clock_tick_seconds(self.bpm);
        while (self.next_clock_tick as f64) * tick < to {
            let at = self.next_clock_tick as f64 * tick;
            for &port in &self.clock_ports {
                out.push(ScheduledEvent::new(at, port, MidiMsg::Clock));
            }
            self.next_clock_tick += 1;
        }
    }

    /// When this cursor's next step falls, or `None` if the track cannot play at
    /// all — no port, no pattern in the current scene, or zero length.
    ///
    /// A zero-length track is `None` rather than a panic: it cannot wrap, so
    /// deriving the step within the pattern would divide by zero. It plays
    /// nothing.
    fn cursor_deadline(&self, session: &Session, index: usize) -> Option<f64> {
        let cursor = &self.cursors[index];
        cursor.port?;
        let pattern = self.pattern(session, cursor.device)?;
        let track = pattern.track(cursor.track)?;
        let length = track.length_steps as u64;
        if length == 0 {
            return None;
        }
        let step_secs = track_step_seconds(self.bpm, track.scale);
        let step_in_pattern = cursor.elapsed_steps() % length;
        Some(
            cursor.next_step as f64 * step_secs
                + swing_offset_steps(pattern.swing, step_in_pattern) * step_secs,
        )
    }

    /// Play one step of one track and move its cursor on.
    #[allow(clippy::too_many_arguments)]
    fn step_cursor(
        &mut self,
        session: &Session,
        index: usize,
        deadline: f64,
        any_solo: bool,
        rng: &mut dyn Rng,
        plocks: &dyn PLockMap,
        out: &mut Vec<ScheduledEvent>,
    ) {
        let cursor = &self.cursors[index];
        let (device_id, device_index, track_index, channel, step) =
            (cursor.device, cursor.device_index, cursor.track, cursor.channel, cursor.elapsed_steps());
        // `cursor_deadline` answered `Some` for this index a moment ago, so every
        // one of these is present; the `let else`s are how that is stated rather
        // than assumed.
        let Some(port) = cursor.port else { return };
        let Some(pattern) = self.pattern(session, device_id) else { return };
        let Some(track) = pattern.track(track_index) else { return };
        let length = track.length_steps as u64;
        if length == 0 {
            return;
        }

        self.cursors[index].next_step += 1;

        let audible = !track.mute && (!any_solo || track.solo);
        let step_secs = track_step_seconds(self.bpm, track.scale);
        self.play_trig(
            track, track_index, device_id, device_index, port, channel, step % length,
            step / length, deadline, step_secs, audible, rng, plocks, out,
        );
    }

    /// One step of one track: the trig, its condition, its notes and its lanes.
    #[allow(clippy::too_many_arguments)]
    fn play_trig(
        &mut self,
        track: &Track,
        track_index: usize,
        device_id: DeviceId,
        device_index: usize,
        port: PortId,
        channel: u8,
        step_in_pattern: u64,
        loop_index: u64,
        deadline: f64,
        step_secs: f64,
        audible: bool,
        rng: &mut dyn Rng,
        plocks: &dyn PLockMap,
        out: &mut Vec<ScheduledEvent>,
    ) {
        // Notes sitting on this step. The box has trigs on whole steps and
        // carries the sub-step offset in `micro`, so the step is matched by
        // rounding rather than by an `f64` equality nobody should rely on.
        let has_note = track
            .notes
            .iter()
            .any(|n| n.step.round() as i64 == step_in_pattern as i64);
        if !has_note {
            return;
        }

        // **One roll per trig, not per note.** `js/midi.js` filters note by note
        // and therefore draws the dice once per note, which lets a 50% chord fire
        // three of its five notes. On the box a trig is one roll: notes sharing a
        // step *are* one trig and carry identical prob/fill/cond — `Note`'s own
        // doc comment in `core` says so, and `edit_ops::adopt_step_trig` enforces
        // it. Rolling once is the hardware behaviour; rolling per note was an
        // artifact of the browser having no trig concept.
        let trig = track
            .notes
            .iter()
            .find(|n| n.step.round() as i64 == step_in_pattern as i64)
            .expect("checked by has_note");

        let ctx =
            self.histories[device_index].context_for(track_index, loop_index, self.fill_active);
        let outcome = should_play(
            trig.prob,
            trig.fill,
            trig.cond.as_deref(),
            track.track_prob,
            &ctx,
            rng,
        );

        // The history records what the *condition* did, whether or not the track
        // is audible. Muting is a mixing decision and must not rewrite what a
        // later PRE on that track reads — unmuting mid-bar would otherwise
        // resume a different pattern of conditions than the one that was running.
        self.histories[device_index].record(track_index, outcome);

        if !outcome.plays || !audible {
            return;
        }

        // P-lock lanes go out just ahead of the step's notes, so the parameter is
        // already where the lane says by the time the trig sounds — and only when
        // something is actually playing, because a trig silenced by probability
        // does not apply its locks on the box either.
        if !track.plocks.is_empty() {
            self.plock_scratch.clear();
            plocks.messages(device_id, channel, &track.plocks, step_in_pattern, &mut self.plock_scratch);
            let at = (deadline - PLOCK_LEAD_SECONDS).max(0.0);
            for msg in self.plock_scratch.drain(..) {
                out.push(ScheduledEvent::new(at, port, msg));
            }
        }

        for note in track
            .notes
            .iter()
            .filter(|n| n.step.round() as i64 == step_in_pattern as i64)
        {
            let at = deadline + note.micro * step_secs;
            let off_at = at + note_duration_seconds(note.len, step_secs);
            if let Some(displaced) = self.active.note_on(port, channel, note.pitch, at, off_at) {
                out.push(displaced);
            }
            out.push(ScheduledEvent::new(
                at,
                port,
                MidiMsg::NoteOn { channel, pitch: note.pitch, velocity: note.velocity },
            ));
        }
    }

    /// Everything still sounding, released at `at`, and the table emptied.
    pub fn stop(&mut self, at: f64, out: &mut Vec<ScheduledEvent>) {
        self.active.flush(at, out);
        if self.send_clock {
            for &port in &self.clock_ports {
                out.push(ScheduledEvent::new(at, port, MidiMsg::Stop));
            }
        }
        crate::event::sort_events(out);
    }

    /// All Notes Off and All Sound Off on every channel in use, after the
    /// note-offs we do know about.
    pub fn panic(&mut self, at: f64, out: &mut Vec<ScheduledEvent>) {
        let channels = self.channels_in_use();
        self.active.panic(at, &channels, out);
        crate::event::sort_events(out);
    }

    /// The transport messages that open a run.
    ///
    /// Stop before Start, as `js/midi.js` does: if a box is already running —
    /// slaved to another master moments ago — Stop → Start forces a clean restart
    /// at step 1 rather than a second sequence beginning mid-bar.
    pub fn start_messages(&self, at: f64, out: &mut Vec<ScheduledEvent>) {
        if !self.send_clock {
            return;
        }
        for &port in &self.clock_ports {
            out.push(ScheduledEvent::new(at, port, MidiMsg::Stop));
            out.push(ScheduledEvent::new(at, port, MidiMsg::Start));
        }
    }
}
