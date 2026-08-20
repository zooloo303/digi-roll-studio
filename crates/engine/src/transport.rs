//! The engine thread: the only part of the engine that knows what time it is.
//!
//! # Why this exists at all
//!
//! `js/midi.js` never has to hit a deadline. It runs a coarse 25 ms pump and
//! hands `MIDIOutput.send()` a future `DOMHighResTimeStamp`; the browser's MIDI
//! stack does the scheduling, and the interval timer's jitter never reaches the
//! wire. **`midir::MidiOutputConnection::send` is immediate** — there is no
//! timestamp parameter and no driver-side queue. So the scheduling the browser
//! got for free has to happen here, in userspace, on a thread that wakes up at
//! the right moment.
//!
//! Which is why this module contains no musical decisions whatsoever. It asks
//! [`Scheduler`] for the next window, converts each event's `f64` seconds to an
//! `Instant`, sleeps to just before it and spins to the deadline. Everything
//! about *what* plays is in [`crate::scheduler`], where it can be tested.
//!
//! # Sleep, then spin
//!
//! `thread::sleep` on macOS and Linux is accurate to something like a
//! millisecond on a good day and much worse under load — not good enough for a
//! sequencer. So the thread sleeps to [`SPIN_MARGIN`] *before* the deadline and
//! then busy-waits the remainder. The spin costs a core for well under a
//! millisecond per event and is what buys the ~1 ms jitter PLAN.md §4 targets.
//!
//! The later optimisation PLAN.md §4 names — CoreMIDI *does* accept scheduled
//! packet timestamps, so on macOS the scheduling could move back into the driver
//! — is not taken here, and [`JitterStats`] exists to say whether it needs to be.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use digi_core::session::Session;

use crate::event::{MidiMsg, PortId, PortTable, ScheduledEvent};
use crate::rng::{Rng, XorShift64};
use crate::scheduler::{PLockMap, Scheduler};

/// How far ahead the scheduler is asked to compute. PLAN.md §4's "~50 ms
/// horizon": long enough that a slow wake-up does not run the queue dry, short
/// enough that a tempo change or a scene switch is not already committed.
pub const LOOKAHEAD: Duration = Duration::from_millis(50);

/// How long before a deadline the thread stops sleeping and starts spinning.
pub const SPIN_MARGIN: Duration = Duration::from_micros(1500);

/// The longest the thread will sleep with nothing to do, so a command is never
/// waiting more than this to be noticed.
pub const IDLE_POLL: Duration = Duration::from_millis(5);

/// What the UI tells the engine to do. Everything crosses this channel; the UI
/// never touches the scheduler.
pub enum TransportCommand {
    /// Rewind to the top and play. Sends MIDI Stop then Start to every device
    /// that takes clock.
    Start,
    /// Stop, flushing every pending note-off.
    Stop,
    /// Resume from where the cursors are, without rewinding.
    Continue,
    /// All Notes Off + All Sound Off on every channel in use.
    Panic,
    SetTempo(f64),
    SetFill(bool),
    SetSendClock(bool),
    /// Play this scene, from the next boundary of the one playing — PLAN.md §4's
    /// queued scene change. `immediate` is its "immediate" setting.
    ///
    /// Stopped, there is no boundary to wait for and this takes effect at once,
    /// so that picking a scene to edit does what it looks like it does.
    SelectScene { scene: usize, immediate: bool },
    /// A new whole-session snapshot. One `Arc` for the entire session, not one
    /// per device, so the boxes can never pick up halves of an edit
    /// (PLAN.md §4).
    ///
    /// The port table travels with it. Re-interning against a fresh table would
    /// renumber the ports out from under both the queue and the sink, so the
    /// sender — the UI, which owns the sink — sends the table it opened those
    /// connections against. A session that has since grown a port the sink does
    /// not have is the UI's problem to notice, and it rebuilds rather than
    /// snapshotting.
    Snapshot { session: Arc<Session>, ports: PortTable },
    Quit,
}

/// How far each send missed its deadline, per port.
///
/// PLAN.md §8 flags this as a risk that has to be *measured*, not extrapolated:
/// clock and notes go to two USB endpoints from one thread, and a slow `send()`
/// on one port delays the other. Counting lateness per port is what tells the
/// difference between "one thread is fine" and "fall back to a sender thread per
/// port".
///
/// Microseconds, as atomics, so the UI can read them without a lock while the
/// engine keeps writing.
#[derive(Debug, Default)]
pub struct JitterStats {
    pub sends: AtomicU64,
    pub total_late_us: AtomicU64,
    pub max_late_us: AtomicU64,
    /// Sends that missed their deadline by more than a millisecond — the number
    /// that decides whether the CoreMIDI fallback is needed.
    pub over_1ms: AtomicU64,
}

impl JitterStats {
    fn record(&self, late: Duration) {
        let us = late.as_micros().min(u64::MAX as u128) as u64;
        self.sends.fetch_add(1, Ordering::Relaxed);
        self.total_late_us.fetch_add(us, Ordering::Relaxed);
        self.max_late_us.fetch_max(us, Ordering::Relaxed);
        if us > 1000 {
            self.over_1ms.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn mean_late_us(&self) -> f64 {
        let n = self.sends.load(Ordering::Relaxed);
        if n == 0 {
            return 0.0;
        }
        self.total_late_us.load(Ordering::Relaxed) as f64 / n as f64
    }

    pub fn reset(&self) {
        self.sends.store(0, Ordering::Relaxed);
        self.total_late_us.store(0, Ordering::Relaxed);
        self.max_late_us.store(0, Ordering::Relaxed);
        self.over_1ms.store(0, Ordering::Relaxed);
    }
}

/// "No scene is queued", as an atomic can say it. There is no `AtomicOption`, and
/// a sentinel of 0 would be a real scene.
pub const NO_SCENE: usize = usize::MAX;

/// What the engine publishes back for display.
///
/// Atomics rather than a mutex, per PLAN.md §4: the engine thread must never
/// block on the UI, and a playhead that is one frame stale is not a problem.
#[derive(Debug)]
pub struct TransportState {
    pub playing: AtomicBool,
    /// Playhead in steps × 1000, since there is no atomic `f64` and the UI only
    /// wants it to draw a line.
    pub position_millisteps: AtomicU64,
    pub active_notes: AtomicUsize,
    /// The scene actually sounding. The UI follows this rather than deciding it:
    /// a scene change is taken at a boundary the engine owns, so the engine is
    /// the only thing that knows when the switch happened.
    pub playing_scene: AtomicUsize,
    /// The scene queued behind it, or [`NO_SCENE`].
    pub pending_scene: AtomicUsize,
    /// One per port, indexed by [`PortId`].
    pub jitter: Vec<JitterStats>,
}

/// Written out rather than derived: a derived `Default` would give
/// `pending_scene: 0`, which reads as "scene 1 is queued" from the moment the app
/// opens. Same class of bug as `DeviceIo`'s two disagreeing defaults.
impl Default for TransportState {
    fn default() -> Self {
        Self {
            playing: AtomicBool::new(false),
            position_millisteps: AtomicU64::new(0),
            active_notes: AtomicUsize::new(0),
            playing_scene: AtomicUsize::new(0),
            pending_scene: AtomicUsize::new(NO_SCENE),
            jitter: Vec::new(),
        }
    }
}

impl TransportState {
    pub fn with_ports(n: usize) -> Self {
        Self {
            jitter: (0..n).map(|_| JitterStats::default()).collect(),
            ..Default::default()
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn position_steps(&self) -> f64 {
        self.position_millisteps.load(Ordering::Relaxed) as f64 / 1000.0
    }

    pub fn playing_scene(&self) -> usize {
        self.playing_scene.load(Ordering::Relaxed)
    }

    pub fn pending_scene(&self) -> Option<usize> {
        match self.pending_scene.load(Ordering::Relaxed) {
            NO_SCENE => None,
            scene => Some(scene),
        }
    }
}

/// Where the engine puts bytes.
///
/// A trait, not a `midir` connection, so the transport loop can be driven in a
/// test against a recording sink — the timing behaviour is then observable
/// without a box, a driver, or a person listening.
pub trait PortSink: Send {
    fn send(&mut self, port: PortId, bytes: &[u8]);
}

/// A sink that records what it was given and when. The test double, and also
/// what the jitter example measures against when no hardware is connected.
#[derive(Debug, Default)]
pub struct RecordingSink {
    pub sent: Vec<(PortId, Vec<u8>, Instant)>,
}

impl PortSink for RecordingSink {
    fn send(&mut self, port: PortId, bytes: &[u8]) {
        self.sent.push((port, bytes.to_vec(), Instant::now()));
    }
}

/// A handle on a running engine thread.
pub struct Transport {
    tx: Sender<TransportCommand>,
    state: Arc<TransportState>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Transport {
    /// Spawn the engine thread.
    ///
    /// `sink` is moved onto it and never shared: the only thing that touches a
    /// MIDI connection is the thread that hits the deadlines.
    /// `plocks` is how the caller supplies the per-box parameter tables this
    /// crate is not allowed to know about — see [`PLockMap`]. Pass
    /// [`NoPLocks`] to play a session's notes and none of its lanes.
    pub fn spawn(
        session: Arc<Session>,
        scheduler: Scheduler,
        sink: Box<dyn PortSink>,
        state: Arc<TransportState>,
        plocks: Box<dyn PLockMap + Send>,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_state = Arc::clone(&state);
        let join = std::thread::Builder::new()
            .name("digi-engine".into())
            .spawn(move || {
                let mut engine = EngineThread {
                    session,
                    scheduler,
                    sink,
                    state: thread_state,
                    rng: Box::new(XorShift64::new(0x5eed_1234_9876_abcd)),
                    plocks,
                    queue: Vec::with_capacity(1024),
                    scratch: Vec::with_capacity(1024),
                    bytes: Vec::with_capacity(16),
                    started_at: None,
                    scheduled_to: 0.0,
                };
                engine.run(rx);
            })
            .expect("spawning the engine thread");
        Self { tx, state, join: Some(join) }
    }

    pub fn send(&self, cmd: TransportCommand) {
        // A closed channel means the thread is already gone, which is only
        // reachable during shutdown. Nothing useful can be done about it here.
        let _ = self.tx.send(cmd);
    }

    pub fn state(&self) -> &Arc<TransportState> {
        &self.state
    }
}

impl Drop for Transport {
    /// Stop the box before dropping the handle. A sequencer whose window closes
    /// while a note is held leaves the box droning with nothing left to release
    /// it — the one failure a user cannot fix from the UI.
    fn drop(&mut self) {
        let _ = self.tx.send(TransportCommand::Stop);
        let _ = self.tx.send(TransportCommand::Quit);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

struct EngineThread {
    session: Arc<Session>,
    scheduler: Scheduler,
    sink: Box<dyn PortSink>,
    state: Arc<TransportState>,
    rng: Box<dyn Rng + Send>,
    plocks: Box<dyn PLockMap + Send>,
    /// Events computed but not yet due, in send order.
    queue: Vec<ScheduledEvent>,
    scratch: Vec<ScheduledEvent>,
    bytes: Vec<u8>,
    /// `None` when stopped.
    started_at: Option<Instant>,
    scheduled_to: f64,
}

impl EngineThread {
    fn run(&mut self, rx: Receiver<TransportCommand>) {
        loop {
            match self.drain_commands(&rx) {
                ControlFlow::Quit => {
                    self.flush_stop();
                    return;
                }
                ControlFlow::Continue => {}
            }

            let Some(started_at) = self.started_at else {
                std::thread::sleep(IDLE_POLL);
                continue;
            };

            let now = Instant::now();
            let elapsed = now.duration_since(started_at).as_secs_f64();

            // Top the queue up to the horizon. Asking for a window that is
            // already partly in the past is normal — a late wake-up — and the
            // scheduler handles it by dating those events in the past, which
            // sends them immediately rather than dropping them.
            let horizon = elapsed + LOOKAHEAD.as_secs_f64();
            if horizon > self.scheduled_to {
                self.scratch.clear();
                self.scheduler.advance(
                    &self.session,
                    horizon,
                    self.rng.as_mut(),
                    self.plocks.as_ref(),
                    &mut self.scratch,
                );
                self.queue.append(&mut self.scratch);
                crate::event::sort_events(&mut self.queue);
                self.scheduled_to = horizon;
            }

            self.send_due(started_at);
            self.publish(elapsed);
            self.park(started_at);
        }
    }

    /// Everything in the queue whose deadline has arrived, sent at its deadline.
    fn send_due(&mut self, started_at: Instant) {
        let mut sent = 0;
        for event in &self.queue {
            let deadline = started_at + Duration::from_secs_f64(event.at.max(0.0));
            let now = Instant::now();
            if deadline > now + SPIN_MARGIN {
                break;
            }
            // Sleep is over; close the last of the gap by spinning. `spin_loop`
            // is the hint that lets the core back off without yielding the
            // timeslice, which a `yield_now` here would.
            while Instant::now() < deadline {
                std::hint::spin_loop();
            }
            self.bytes.clear();
            event.msg.write_bytes(&mut self.bytes);
            self.sink.send(event.port, &self.bytes);
            if let Some(stats) = self.state.jitter.get(event.port.0) {
                stats.record(Instant::now().saturating_duration_since(deadline));
            }
            sent += 1;
        }
        self.queue.drain(..sent);
    }

    /// Sleep until just before the next deadline, or [`IDLE_POLL`], whichever is
    /// sooner — so a command never waits long to be noticed.
    fn park(&self, started_at: Instant) {
        let next = self
            .queue
            .first()
            .map(|e| started_at + Duration::from_secs_f64(e.at.max(0.0)));
        let now = Instant::now();
        let wait = match next {
            Some(deadline) if deadline > now + SPIN_MARGIN => {
                (deadline - now - SPIN_MARGIN).min(IDLE_POLL)
            }
            Some(_) => Duration::ZERO,
            None => IDLE_POLL,
        };
        if wait > Duration::ZERO {
            std::thread::sleep(wait);
        }
    }

    fn publish(&self, elapsed: f64) {
        self.state.playing.store(true, Ordering::Relaxed);
        self.state
            .active_notes
            .store(self.scheduler.active_notes().len(), Ordering::Relaxed);
        let steps = elapsed / crate::time::step_seconds(self.scheduler.bpm);
        self.state
            .position_millisteps
            .store((steps * 1000.0).max(0.0) as u64, Ordering::Relaxed);
        self.publish_scene();
    }

    /// Which scene is sounding and which is queued behind it. Published from
    /// every frame of the loop *and* the moment a scene command lands, because a
    /// scene picked while stopped never sees another frame of the loop.
    fn publish_scene(&self) {
        self.state
            .playing_scene
            .store(self.scheduler.scene(), Ordering::Relaxed);
        self.state
            .pending_scene
            .store(self.scheduler.pending_scene().unwrap_or(NO_SCENE), Ordering::Relaxed);
    }

    fn drain_commands(&mut self, rx: &Receiver<TransportCommand>) -> ControlFlow {
        loop {
            match rx.try_recv() {
                Ok(TransportCommand::Quit) => return ControlFlow::Quit,
                Ok(cmd) => self.apply(cmd),
                Err(TryRecvError::Empty) => return ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => return ControlFlow::Quit,
            }
        }
    }

    fn apply(&mut self, cmd: TransportCommand) {
        match cmd {
            TransportCommand::Start => {
                self.flush_stop();
                self.scheduler.rewind();
                self.queue.clear();
                self.scheduled_to = 0.0;
                self.started_at = Some(Instant::now());
                self.scratch.clear();
                self.scheduler.start_messages(0.0, &mut self.scratch);
                self.queue.append(&mut self.scratch);
            }
            TransportCommand::Continue => {
                if self.started_at.is_none() {
                    // Resume where the cursors are: back-date the start so the
                    // timeline the cursors already sit on stays continuous.
                    let resumed_from = self.scheduled_to;
                    self.started_at =
                        Some(Instant::now() - Duration::from_secs_f64(resumed_from.max(0.0)));
                }
            }
            TransportCommand::Stop => self.flush_stop(),
            TransportCommand::Panic => {
                self.scratch.clear();
                self.scheduler.panic(0.0, &mut self.scratch);
                self.send_now();
            }
            TransportCommand::SetTempo(bpm) if bpm > 0.0 => self.scheduler.bpm = bpm,
            TransportCommand::SetTempo(_) => {}
            TransportCommand::SetFill(on) => self.scheduler.fill_active = on,
            TransportCommand::SetSendClock(on) => self.scheduler.send_clock = on,
            TransportCommand::SelectScene { scene, immediate } => {
                // `scheduled_to` is how far the queue has already been dated —
                // the earliest point a switch can land without unpicking events
                // that are on their way out. Stopped, it is where a Continue
                // would resume from, and there is no boundary worth waiting for.
                let immediate = immediate || self.started_at.is_none();
                self.scheduler.queue_scene(
                    &self.session,
                    scene,
                    self.scheduled_to,
                    immediate,
                );
                self.publish_scene();
            }
            TransportCommand::Snapshot { session, mut ports } => {
                self.session = session;
                // **The tempo comes across with everything else, and for eleven
                // commits it did not.** `Snapshot` means *here is the session as
                // it now stands*; taking every field of it except `tempo_bpm`
                // made the clock a second source of truth that only an explicit
                // `SetTempo` could move, so "a tempo edit is two calls" was a
                // rule living in each of the app's callers rather than here.
                // Two of the three remembered it; the Generate panel's SET
                // button set the transport to 174 and left the boxes at 120.
                //
                // Same guard `SetTempo` has, and not a theoretical one: a
                // snapshot carries whatever the session held, a session comes
                // off disk, and `track_step_seconds` divides by this number.
                if self.session.tempo_bpm > 0.0 {
                    self.scheduler.bpm = self.session.tempo_bpm;
                }
                // Prepared against the caller's table, so the ids in the queue
                // and the ids the sink is indexed by stay the same numbers.
                // Cursors keep their `next_step`, so editing a note mid-play
                // moves that note and nothing else.
                self.scheduler.prepare(&self.session, &mut ports);
            }
            TransportCommand::Quit => {}
        }
    }

    /// Stop: release everything sounding, tell the boxes, and go idle.
    fn flush_stop(&mut self) {
        if self.started_at.is_none() && self.scheduler.active_notes().is_empty() {
            return;
        }
        // **Note-offs already in the queue belong to notes that are sounding.**
        // `ActiveNotes::drain_due` emits an off as soon as its deadline falls
        // inside the window — up to [`LOOKAHEAD`] early — and forgets the note as
        // it does, so for those 50 ms the note is in neither the active table nor
        // on the wire. Dropping the queue without sending them left exactly those
        // notes droning with nothing that could release them: the one failure a
        // user cannot fix from the UI, reached by pressing Stop rather than by
        // closing the window.
        self.scratch.clear();
        self.scratch.extend(
            self.queue
                .iter()
                .filter(|e| matches!(e.msg, MidiMsg::NoteOff { .. }))
                .copied(),
        );
        self.queue.clear();
        self.send_now();

        self.scratch.clear();
        self.scheduler.stop(0.0, &mut self.scratch);
        self.send_now();
        self.started_at = None;
        self.state.playing.store(false, Ordering::Relaxed);
        self.state.active_notes.store(0, Ordering::Relaxed);
    }

    /// Send everything in `scratch` immediately, deadlines ignored. Stop and
    /// panic both mean *now*.
    fn send_now(&mut self) {
        for event in self.scratch.drain(..) {
            self.bytes.clear();
            event.msg.write_bytes(&mut self.bytes);
            self.sink.send(event.port, &self.bytes);
        }
    }
}

enum ControlFlow {
    Continue,
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::NoPLocks;

    #[test]
    fn jitter_stats_track_the_worst_case_not_just_the_mean() {
        let s = JitterStats::default();
        s.record(Duration::from_micros(100));
        s.record(Duration::from_micros(2500));
        s.record(Duration::from_micros(200));
        assert_eq!(s.sends.load(Ordering::Relaxed), 3);
        assert_eq!(s.max_late_us.load(Ordering::Relaxed), 2500);
        assert_eq!(s.over_1ms.load(Ordering::Relaxed), 1, "one send missed by over a ms");
        assert!((s.mean_late_us() - 933.333).abs() < 0.01);
        s.reset();
        assert_eq!(s.mean_late_us(), 0.0);
    }

    #[test]
    fn transport_state_reports_a_fractional_playhead() {
        let st = TransportState::with_ports(2);
        assert!(!st.is_playing());
        st.position_millisteps.store(3500, Ordering::Relaxed);
        assert_eq!(st.position_steps(), 3.5);
        assert_eq!(st.jitter.len(), 2);
    }

    /// A stopped engine thread, off the thread, at `bpm`. `apply` is what the
    /// real one calls for every command it drains, so driving it directly tests
    /// the command handling and nothing about timing.
    fn engine_at(bpm: f64) -> EngineThread {
        let session = Arc::new(Session::default());
        EngineThread {
            session: Arc::clone(&session),
            scheduler: Scheduler::new(bpm),
            sink: Box::new(RecordingSink::default()),
            state: Arc::new(TransportState::with_ports(0)),
            rng: Box::new(XorShift64::new(1)),
            plocks: Box::new(NoPLocks),
            queue: Vec::new(),
            scratch: Vec::new(),
            bytes: Vec::new(),
            started_at: None,
            scheduled_to: 0.0,
        }
    }

    #[test]
    fn a_snapshot_carries_the_sessions_tempo() {
        // Neil, 2026-08-20: the transport read 174 BPM and the boxes played 120.
        //
        // `Snapshot` means *here is the session as it now stands*, and it took
        // every field of that session except the one the user had just changed.
        // The only thing that had ever moved the clock was an explicit
        // `SetTempo` sent beside the session — so the rule "a tempo edit is two
        // calls" lived in every caller instead of here, and of the three callers
        // that write `session.tempo_bpm` exactly two remembered the second call.
        // The Generate panel's SET button was the one that did not.
        let mut engine = engine_at(120.0);
        let session = Session { tempo_bpm: 174.0, ..Session::default() };
        engine.apply(TransportCommand::Snapshot {
            session: Arc::new(session),
            ports: PortTable::new(),
        });
        assert_eq!(engine.scheduler.bpm, 174.0, "the snapshot's tempo is the tempo");
    }

    #[test]
    fn a_snapshot_with_no_tempo_at_all_is_ignored_rather_than_dividing_by_it() {
        // The same guard `SetTempo` has always had, and it is not theoretical
        // here: a `Snapshot` carries whatever was in the session, and a session
        // read off disk is a file anyone can edit. `track_step_seconds` divides
        // by this number.
        let mut engine = engine_at(174.0);
        let session = Session { tempo_bpm: 0.0, ..Session::default() };
        engine.apply(TransportCommand::Snapshot {
            session: Arc::new(session),
            ports: PortTable::new(),
        });
        assert_eq!(engine.scheduler.bpm, 174.0, "a nonsense tempo leaves the clock alone");
    }

    #[test]
    fn a_recording_sink_keeps_what_it_was_given() {
        let mut sink = RecordingSink::default();
        sink.send(PortId(1), &[0xf8]);
        assert_eq!(sink.sent.len(), 1);
        assert_eq!(sink.sent[0].1, vec![0xf8]);
    }
}
