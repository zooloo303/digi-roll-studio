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

use digi_core::device::DeviceId;
use digi_core::session::Session;
use digi_midi::live_input::{LiveEvent, LiveKind};

use crate::event::{MidiMsg, PortId, PortTable, ScheduledEvent};
use crate::record::{placed_kind, PlacedEvent};
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
    /// Walk the song, or stop walking it — PLAN.md §6 phase 12.
    ///
    /// Entering starts at `row` and commits that row's scene at once rather than
    /// queuing it: the song is now what decides which scene plays, and a pointer
    /// naming a row that is not yet sounding would be the display lying. Leaving
    /// commits nothing, so whatever the last row put up keeps playing.
    SetSongMode { on: bool, row: usize },
    /// Move the walk to a row. Ignored in pattern mode.
    JumpToSongRow(usize),
    /// Where thru goes: the selected track's resolved port and channel, or
    /// `None` to go quiet — PLAN.md §12.4.2.
    ///
    /// Re-sent by the UI whenever the selection moves, and remembered by
    /// `EngineLink` across rebuilds, because a rebuild is a new thread that
    /// knows none of this. Changing it releases anything the old monitor is
    /// holding: without that, moving track with a chord under your hands leaves
    /// it ringing on the box you just left, with nothing that could stop it.
    SetMonitor(Option<(PortId, u8)>),
    /// Arm or disarm, naming the track a take would land on, and QUANTIZE.
    ///
    /// Arming alone captures nothing — the transport also has to be running,
    /// which is decision 5. `target` is `None` when nothing is selected, which
    /// is armed-but-aimed-nowhere and is a state the UI shows rather than one
    /// this thread has to resolve.
    SetRecord {
        armed: bool,
        target: Option<(DeviceId, usize)>,
        quantize: bool,
    },
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
    /// Send these, now, in the order given — no deadline and no queue.
    ///
    /// For a control the *user* is turning rather than one the sequencer plays:
    /// a fader moved while the transport is stopped has to reach the box anyway,
    /// and one moved while it is playing must not wait for a step boundary that
    /// may be two bars off. Scheduling it at `at = 0.0` would be worse than
    /// either, since 0.0 is the top of the run and every event before now is
    /// sent immediately — a fader move would land in the middle of whatever the
    /// queue was already holding.
    ///
    /// The messages are resolved out here, by the caller that knows which
    /// controller number a knob lives at (`app::plocks`, `core::audition`) —
    /// `PLAN.md` §3 keeps that knowledge out of this crate, and a command
    /// carrying finished [`MidiMsg`]s is how it stays out.
    SendNow(Vec<(PortId, MidiMsg)>),
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

/// "The song is not playing" — same sentinel argument as [`NO_SCENE`]: row 0 is
/// a real row, and the SONG panel has to be able to tell "on row 1" from "not
/// walking the song at all", because one of those draws a playhead.
pub const NO_ROW: usize = usize::MAX;

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
    /// The box's SONG POINTER: the row playing, or [`NO_ROW`] in pattern mode.
    pub song_row: AtomicUsize,
    /// Which pass of that row's ROW PLAY COUNT, 0-based.
    pub song_repeat: AtomicUsize,
    /// Whether the transport is walking the song at all. Not derivable from
    /// `song_row`: song mode with an empty song has no row to be on and is still
    /// song mode, and the panel says so rather than looking switched off.
    pub song_mode: AtomicBool,
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
            song_row: AtomicUsize::new(NO_ROW),
            song_repeat: AtomicUsize::new(0),
            song_mode: AtomicBool::new(false),
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

    pub fn song_mode(&self) -> bool {
        self.song_mode.load(Ordering::Relaxed)
    }

    /// The row playing and which pass of it, or `None` in pattern mode.
    pub fn song_position(&self) -> Option<(usize, u16)> {
        match self.song_row.load(Ordering::Relaxed) {
            NO_ROW => None,
            row => Some((row, self.song_repeat.load(Ordering::Relaxed) as u16)),
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
    ///
    /// `live_rx` and `placed_tx` are recording's two ends of this thread
    /// (PLAN.md §12.4.2): notes played on the record input come *in*
    /// on the first, and the ones that fell inside a take go *out* on the second
    /// with a step, a micro and a pass attached. Both are plain `mpsc`s and both
    /// are allowed to be dead — a session with no keyboard has a `live_rx` that
    /// never yields and a `placed_tx` nobody drains, and neither is an error
    /// worth a branch anywhere else in this file.
    pub fn spawn(
        session: Arc<Session>,
        scheduler: Scheduler,
        sink: Box<dyn PortSink>,
        state: Arc<TransportState>,
        plocks: Box<dyn PLockMap + Send>,
        live_rx: Receiver<LiveEvent>,
        placed_tx: Sender<PlacedEvent>,
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
                    live_rx,
                    placed_tx,
                    monitor: None,
                    held_thru: [false; 128],
                    armed: false,
                    record_target: None,
                    quantize: false,
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

    // --- recording, PLAN.md §12.4.2 ------------------------------
    /// Notes off the record input, stamped by the driver callback.
    live_rx: Receiver<LiveEvent>,
    /// The same notes, placed on the armed track's grid. One heap node per
    /// note-on and one per note-off; a fast player makes perhaps twenty a
    /// second, which is why this is an `mpsc` and not the fixed-capacity ring
    /// the design names as its replacement if [`JitterStats`] ever moves.
    placed_tx: Sender<PlacedEvent>,
    /// Where thru goes, or nowhere.
    monitor: Option<(PortId, u8)>,
    /// Which pitches thru is currently holding down on `monitor`. A 128-slot
    /// array rather than a list, because it is written from this thread on every
    /// arrival and read whole only when something has to release it.
    held_thru: [bool; 128],
    armed: bool,
    record_target: Option<(DeviceId, usize)>,
    quantize: bool,
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

            // **Before the stopped check, not after it.** Thru has to work
            // with the transport stopped — that is decision 2, and it is the
            // whole reason thru lives on this thread rather than in the UI,
            // which may not draw a frame for seconds at a time.
            self.drain_live();

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

            // **`END: STOP`, taken here and not in the scheduler.** The scheduler
            // has no `Instant` and cannot stop anything; it records the moment the
            // song ran out and emits nothing past it. This is where the timeline
            // reaching that moment becomes a stop — after `send_due`, so the last
            // row's trigs and their note-offs are already on the wire.
            if let Some(stop) = self.scheduler.song_stop_at() {
                if elapsed >= stop {
                    self.flush_stop();
                    self.publish_scene();
                    continue;
                }
            }

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
        // The song pointer travels with the scene, and for the same reason: in
        // song mode the row is *what chose* the scene, so publishing one without
        // the other would let the transport bar name a row and a scene that do
        // not belong together.
        self.state
            .song_mode
            .store(self.scheduler.song_mode(), Ordering::Relaxed);
        let (row, repeat) = match self.scheduler.song_position() {
            Some((row, repeat)) => (row, repeat as usize),
            None => (NO_ROW, 0),
        };
        self.state.song_row.store(row, Ordering::Relaxed);
        self.state.song_repeat.store(repeat, Ordering::Relaxed);
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
                self.scheduler.rewind(&self.session);
                self.queue.clear();
                self.scheduled_to = 0.0;
                self.started_at = Some(Instant::now());
                self.scratch.clear();
                self.scheduler.start_messages(0.0, &mut self.scratch);
                self.queue.append(&mut self.scratch);
            }
            TransportCommand::Continue => {
                // A song that ran out under `END: STOP` has nothing to resume
                // into: `song_stop_at` still stands, so this starts the thread,
                // finds the timeline already past the stop and stops again. That
                // is the honest answer — the set is over — and PLAY is what plays
                // it again, clearing the stop through `rewind`.
                if self.started_at.is_none() {
                    // Resume where the cursors are: back-date the start so the
                    // timeline the cursors already sit on stays continuous.
                    let resumed_from = self.scheduled_to;
                    self.started_at =
                        Some(Instant::now() - Duration::from_secs_f64(resumed_from.max(0.0)));
                }
            }
            TransportCommand::Stop => self.flush_stop(),
            TransportCommand::SetMonitor(monitor) => {
                if monitor != self.monitor {
                    // Released on the *old* monitor, before the new one is
                    // adopted: these pitches are sounding on the box we are
                    // leaving, and an off sent to the box we are joining would
                    // leave them there for good.
                    self.release_thru();
                    self.monitor = monitor;
                }
            }
            TransportCommand::SetRecord { armed, target, quantize } => {
                self.armed = armed;
                self.record_target = target;
                self.quantize = quantize;
            }
            TransportCommand::SendNow(msgs) => {
                // Through `scratch` and `send_now`, which is the same path a
                // panic takes: one place that turns events into bytes on a
                // port. `at` is unused — nothing waits — and is written 0.0
                // rather than being made optional, because these never enter
                // the queue where a deadline would mean anything.
                self.scratch.clear();
                self.scratch
                    .extend(msgs.into_iter().map(|(port, msg)| ScheduledEvent::new(0.0, port, msg)));
                self.send_now();
            }
            TransportCommand::Panic => {
                // Thru's held pitches are not in the scheduler's active table —
                // nothing scheduled them — so a panic that only asked the
                // scheduler would leave exactly the notes a user's hands are on.
                self.release_thru();
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
            TransportCommand::SetSongMode { on, row } => {
                // From `scheduled_to`, not from zero: that is how far the queue
                // has already been dated, and the same point a scene switch is
                // measured against. Stopped, it is where a Continue would resume.
                self.scheduler.set_song_mode(&self.session, on, row, self.scheduled_to);
                self.publish_scene();
            }
            TransportCommand::JumpToSongRow(row) => {
                self.scheduler.jump_to_row(&self.session, row, self.scheduled_to);
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
                // **A song that has just come into existence starts being
                // walked.** `prepare` can drop the walker — a song emptied under
                // it has no row to be on — but it cannot start one, because it
                // does not know what time it is. Song mode is a standing request,
                // so the first snapshot that gives it something to walk is where
                // that request is honoured: build a song with SONG lit and it
                // plays, rather than needing the toggle pressed twice.
                if self.scheduler.song_mode() && self.scheduler.song_position().is_none() {
                    self.scheduler.jump_to_row(&self.session, 0, self.scheduled_to);
                }
                self.publish_scene();
            }
            TransportCommand::Quit => {}
        }
    }

    /// Stop: release everything sounding, tell the boxes, and go idle.
    fn flush_stop(&mut self) {
        // **Above the early return.** Thru is held outside the scheduler's
        // active table, so a stop pressed while the transport was never running
        // — which takes that return — still has to let go of the keyboard.
        self.release_thru();
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

    // --- recording, PLAN.md §12.4.2 ------------------------------

    /// Everything that arrived on the record input since the last pass: echoed
    /// to the monitor, and — while armed and playing — placed on the armed
    /// track's grid and sent back to the UI.
    ///
    /// Never blocks. A disconnected sender means no keyboard is open, which is
    /// the ordinary case and not a condition worth reporting.
    fn drain_live(&mut self) {
        loop {
            match self.live_rx.try_recv() {
                Ok(event) => {
                    self.thru(event.kind);
                    self.place(event);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return,
            }
        }
    }

    /// Echo one note to the selected track's port and channel, **immediately**.
    ///
    /// Straight at the sink rather than into the queue, for exactly the reason
    /// [`TransportCommand::SendNow`] gives: this is a person's hands, not the
    /// sequencer, and a note that waited for a step boundary two bars off would
    /// be unplayable. The incoming channel is already gone — `parse_live`
    /// discarded it — so what goes out is the track's, which is what makes
    /// "select a track, play the keyboard, hear that box" true.
    fn thru(&mut self, kind: LiveKind) {
        let Some((port, channel)) = self.monitor else {
            return;
        };
        let msg = match kind {
            LiveKind::NoteOn { pitch, velocity } => {
                self.held_thru[pitch as usize] = true;
                MidiMsg::NoteOn { channel, pitch, velocity }
            }
            LiveKind::NoteOff { pitch } => {
                self.held_thru[pitch as usize] = false;
                MidiMsg::NoteOff { channel, pitch }
            }
        };
        self.bytes.clear();
        msg.write_bytes(&mut self.bytes);
        self.sink.send(port, &self.bytes);
    }

    /// Let go of every pitch thru is holding, on whatever monitor is set.
    ///
    /// Called from Stop, from Panic, and from a monitor change. Without it,
    /// holding a chord and changing track leaves that chord ringing on the box
    /// you left, with nothing in the app that could release it — the one class
    /// of failure a user cannot fix from the UI.
    fn release_thru(&mut self) {
        let Some((port, channel)) = self.monitor else {
            // Nothing could have been sent, so nothing can be sounding. The
            // table is cleared anyway: a monitor that goes to `None` and comes
            // back must not resurrect stale pitches.
            self.held_thru = [false; 128];
            return;
        };
        for pitch in 0..128u8 {
            if !self.held_thru[pitch as usize] {
                continue;
            }
            self.held_thru[pitch as usize] = false;
            self.bytes.clear();
            MidiMsg::NoteOff { channel, pitch }.write_bytes(&mut self.bytes);
            self.sink.send(port, &self.bytes);
        }
    }

    /// Convert one arrival into *this track, this step, this micro, this pass*
    /// and send it back to the UI.
    ///
    /// Three conditions, all of them required: armed, playing, and a target
    /// whose cursor exists. Arming alone captures nothing (decision 5), and a
    /// target with no cursor is a track the sounding scene is not playing.
    fn place(&mut self, event: LiveEvent) {
        if !self.armed {
            return;
        }
        let (Some(started_at), Some((device, track))) = (self.started_at, self.record_target)
        else {
            return;
        };
        // The arrival's own moment, on the engine's clock. `saturating_sub` for
        // the key struck in the instant before `Start` re-based the clock: zero
        // is the honest answer, and `place` puts it on step 0.
        let at = event.at.saturating_duration_since(started_at).as_secs_f64();
        let Some(placement) =
            self.scheduler.place_live(&self.session, device, track, at, self.quantize)
        else {
            return;
        };
        // A closed receiver means the UI has rebuilt the engine around us and
        // this thread is about to be joined. Nothing useful can be done with the
        // event, and nothing here may block.
        let _ = self.placed_tx.send(PlacedEvent {
            kind: placed_kind(event.kind),
            step: placement.step,
            micro: placement.micro,
            pass: placement.pass,
            at,
        });
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
        // Both recording ends are dropped immediately: nothing here plays a
        // keyboard, and a dead channel is exactly what a session with no record
        // input has.
        let (_live_tx, live_rx) = std::sync::mpsc::channel();
        let (placed_tx, _placed_rx) = std::sync::mpsc::channel();
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
            live_rx,
            placed_tx,
            monitor: None,
            held_thru: [false; 128],
            armed: false,
            record_target: None,
            quantize: false,
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
