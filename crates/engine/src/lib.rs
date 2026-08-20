//! The realtime engine: transport, clock, scheduling, condition evaluation and
//! note lifetime. PLAN.md §4.
//!
//! Depends on `core` and `midi`, never on `protocol` (PLAN.md §3). Playing a
//! pattern and writing one to a box are separate concerns, and the missing
//! dependency is what keeps them that way.
//!
//! # The shape, and why it is this shape
//!
//! Timing is the part of Phase 4 that does not port from the browser and cannot
//! be unit-tested. `js/midi.js` runs a coarse 25 ms pump and hands
//! `MIDIOutput.send()` future timestamps, so the driver does the scheduling and
//! the interval timer's jitter never reaches the wire. **`midir` has no
//! timestamped send.** The engine thread must therefore hit the deadlines itself.
//!
//! So everything that *decides* is kept out of the thread that *waits*:
//!
//! - [`scheduler::Scheduler`] turns a session plus a window of time into a sorted
//!   queue of `(deadline, port, message)`. Pure, deterministic given its RNG, and
//!   tested without a clock or a box.
//! - [`transport::Transport`] owns the thread, converts those deadlines to
//!   `Instant`s, sleeps to just before each one and spins to it. It makes no
//!   musical decisions at all.
//!
//! What that buys: every question about *what plays when* — polymeter, swing,
//! micro-timing, conditions, note lifetime, two boxes on two ports — is answered
//! by a test that runs in microseconds, and the untestable part is small enough
//! to read in one sitting.

pub mod conditions;
pub mod event;
pub mod notes;
pub mod rng;
pub mod scheduler;
pub mod sink;
pub mod time;
pub mod transport;

pub use conditions::{should_play, Cond, CondContext, CondHistory, CondKind, TrigOutcome};
pub use event::{sort_events, MidiMsg, PortId, PortTable, ScheduledEvent};
pub use notes::{ActiveNote, ActiveNotes};
pub use rng::{Rng, ScriptedRng, XorShift64};
pub use scheduler::{NoPLocks, PLockMap, Scheduler, TrackCursor};
pub use sink::MidirSink;
pub use transport::{Transport, TransportCommand, TransportState};
