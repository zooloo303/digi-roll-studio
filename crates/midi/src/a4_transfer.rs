//! Moving a pattern to and from an **Analog Four** — the one transfer in this
//! workspace the app cannot start on its own.
//!
//! > **The premise below fell on 2026-08-31.** The A4 *does* answer dump
//! > requests — `0x64` returns any pattern slot, and `fetch_dump(0x06, 0x64,
//! > index)` works unmodified; the advertised-opcode list this paragraph reads
//! > as testimony describes the API namespace, which never listed the dump
//! > namespace's opcodes for any box (PLAN.md §10, "The A4 answers dump
//! > requests"; `examples/a4_dump_probe.rs` is the sweep). What stays true and
//! > keeps this module: a person can still start a dump from the front panel,
//! > so the listener half is right regardless, and the send half's pacing is
//! > the box's real requirement. The integration that note used to mark is
//! > built: `ElektronDevice::fetch_pattern_kit` speaks `0x64` on an A4, its
//! > store DIN-paces through [`send_pattern`] below, and
//! > `digi_protocol::safe_write::a4_safe_write_tracks` runs the full re-fetch/
//! > backup/verify ceremony over both. This module is now the A4's *wire* —
//! > the pacing, the pre-send verify, and the front-panel listener.
//!
//! Every other read here is request/reply: [`crate::ElektronDevice`] sends a
//! `0x6x` dump request, retries it, and matches a reply against it. ~~The A4
//! answers no `0x6x` at all~~ — its supported-opcode list, captured off the box on
//! 2026-08-28, runs `01,02,03,04,06,07,09` then `50-5e`, and there is no dump
//! request in it (PLAN.md §9, "The Analog Four arrives"). So this module is not
//! a method on `ElektronDevice` ~~and deliberately cannot become one: putting an
//! A4 pattern read next to `fetch_pattern_kit` would imply the app can ask for
//! one, and it cannot~~.
//!
//! # IN is a listener, and that changes the UI as well as the code
//!
//! An A4 dump is started from the box's own front panel — SETTINGS > SYSEX DUMP.
//! The app's half is to be listening when it happens. [`receive_patterns`] is
//! that wait: cancellable, reporting each frame as it lands, and ending on a
//! quiet period rather than on a message count, because a person may dump one
//! pattern or a whole bank and nothing on the wire says which they chose.
//!
//! **Frames that are not A4 patterns are recorded, not dropped.** A desk with
//! three boxes on it can put a DT2 dump into this window, and `0x54` is a
//! *project-settings* dump on a DT2 and a *pattern* here — the same opcode byte,
//! a different message. [`is_a4_pattern`] keys on `(family, dump_type)`, and
//! anything that fails it is counted into [`ReceiveReport::ignored`] with its
//! family so the user is told the wrong box answered rather than left wondering
//! why nothing arrived.
//!
//! # OUT is paced, and the pacing is the protocol rather than politeness
//!
//! DIN MIDI is 31,250 baud — ten bits a byte, so 3,125 bytes a second. A
//! 14,843-byte pattern dump takes **4.75 seconds** to arrive over a cable, and
//! that is the rate a 2013 box was designed against. Over USB a single `send`
//! hands CoreMIDI the whole frame and it lands in microseconds; a receive path
//! built for a trickle has no reason to survive a flood. The first send this
//! project ever made was one unpaced call, and it **did nothing at all** on a box
//! that was demonstrably listening (PLAN.md §9, 2026-08-30). Splitting the frame
//! is legal: `F0 … F7` is one message however many packets carry it, which is
//! the same fact [`crate::sysex_stream`] exists because of.
//!
//! [`Pacing::single`] keeps the unpaced shape reachable, so the difference stays
//! a measurement rather than a story.
//!
//! **This is not [`crate::device`]'s `paced_send` with a new constant**, and the
//! two must not be merged. That one spends elk-herd's *digest* budget —
//! `len / 800 ms`, about 683 KB/s — which is a gen-2 box's rate of swallowing a
//! store. This one is the speed of a **cable**, 3,125 B/s, two hundred times
//! slower. A 14,843-byte pattern delivered at digi pacing takes 22 ms, which is
//! the unpaced shape with extra steps and is the thing that did nothing.
//!
//! # Windows cannot pace at all, and that is a property of the driver
//!
//! `midir`'s WinMM backend decides sysex-versus-short-message from the *first
//! byte of each send*: a chunk beginning `0xF0` goes to `midiOutLongMsg`, and
//! anything else longer than three bytes is refused outright. Only the first
//! chunk of a split frame begins `0xF0`. So a paced A4 send on Windows fails on
//! packet 2 of 58 — and a failure there is the worst outcome available, because
//! the box is left holding half a message and wedges its SysEx API until it is
//! power-cycled. `device::SEND_CHUNK` carries the full derivation; it is the same
//! driver fact reached from the other direction.
//!
//! So [`CAN_PACE`] is false there and [`Pacing::resolve`] collapses to
//! [`Pacing::single`]: the send goes out in one call, which is safe, and
//! [`SendReport::paced`] reports that it was not paced, which is honest. **The
//! expected result on Windows is therefore that nothing happens** — that is what
//! one unpaced call did on 2026-08-30 — and the panel should say so before the
//! user spends a slot on it rather than after. Sending the shape known to do
//! nothing costs nothing; chunking into a driver that refuses continuations
//! costs a power cycle, which is why this collapses rather than refuses.
//!
//! No A4 has met a Windows build. This is reasoned from `midir`'s source and
//! from a driver fact verified on a DN2 in 2026-08-21, and it is written down as
//! reasoning rather than as a measurement.
//!
//! # The five write-safety rules cannot be met here, and this module says so
//!
//! PLAN.md §7 rule 1 makes re-fetch, confirm, backup, send and read-back one
//! function — [`digi_protocol::safe_write`] — so no caller can skip a step. Three
//! of those five need a box that answers a dump request, ~~and this one does not:
//! there is no re-fetch, no backup and no read-back. That is not a gap to be
//! filled later by better code; it is the box~~ — **and as of 2026-08-31 this
//! one does (see the correction at the top), so the gap is filled:
//! `a4_safe_write_tracks` is the A4's copy of the ceremony, and the app's
//! panels reach [`send_pattern`] only through it.**
//!
//! [`send_pattern`] itself still does not pretend. It takes a [`Consent`] the
//! caller had to construct, it refuses a payload it cannot itself verify, and
//! its report confirms nothing — the read-back and the byte-compare are the
//! ceremony's, one layer up, and a caller that reaches this function directly
//! (the example still does) gets exactly what it always got.
//!
//! # Why the traits, and why time is accumulated rather than read off a clock
//!
//! [`A4Listener`] and [`A4Sink`] exist for the same reason
//! [`crate::preset_scan::PresetSource`] does: cancel, timeout, the quiet period
//! and the wrong-box path are branches that otherwise only run with hardware
//! attached, which is `DEVELOPMENT.md` lesson 4 waiting to happen.
//!
//! The subtler half is time. A timeout measured with `Instant::now()` can only be
//! tested by actually waiting for it, so a suite either sleeps for 90 seconds or
//! never covers the branch. Instead both traits' wait methods **return how long
//! they waited**, and the loops accumulate that. The real implementations measure
//! the true elapsed time and are therefore as honest as a clock; a fake returns
//! the nominal figure and a 90-second arm window expires in a microsecond.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use digi_protocol::a4_kit::{parse_working_kit, A4Kit};
use digi_protocol::a4_pattern::{is_a4_pattern, parse_pattern, A4Pattern, PAYLOAD_LEN};
use digi_protocol::protocol::parse_sysex;

use crate::MidiError;

/// DIN MIDI throughput: 31,250 baud, ten bits a byte.
pub const DIN_BYTES_PER_SEC: f64 = 3125.0;

/// Whether this platform's MIDI backend accepts a SysEx split across sends.
///
/// False on Windows, where a chunk that does not begin `0xF0` is refused — see
/// the module doc. A panel should read this *before* offering to send, because
/// on a false it is about to do the thing that has never worked.
pub const CAN_PACE: bool = !cfg!(target_os = "windows");

// --- The wire ----------------------------------------------------------------

/// An input port that collects whole SysEx frames while nobody is looking.
pub trait A4Listener {
    /// Frames completed since the last call. Never blocks; an empty vec means
    /// nothing has arrived, which is the normal case.
    fn drain(&mut self) -> Vec<Vec<u8>>;

    /// True when a frame is open — bytes are arriving *right now*.
    ///
    /// Without this the quiet period would be measured against the gap between
    /// two driver callbacks, and a 14 KB dump delivered in pieces would be
    /// declared finished halfway through.
    fn mid_frame(&self) -> bool;

    /// Wait, and report how long the wait actually took. See the module doc for
    /// why this returns rather than the caller reading a clock.
    fn wait(&mut self, gap: Duration) -> Duration {
        let t = std::time::Instant::now();
        std::thread::sleep(gap);
        t.elapsed()
    }
}

/// An output port that takes a frame in pieces.
pub trait A4Sink {
    /// Put one piece on the wire. Bytes, not messages: a caller is mid-frame.
    fn send_chunk(&mut self, bytes: &[u8]) -> Result<(), MidiError>;

    /// Hold off between pieces, and report how long that took.
    fn pace(&mut self, gap: Duration) -> Duration {
        let t = std::time::Instant::now();
        std::thread::sleep(gap);
        t.elapsed()
    }
}

impl A4Listener for crate::SysExInbox {
    fn drain(&mut self) -> Vec<Vec<u8>> {
        crate::SysExInbox::drain(self)
    }

    fn mid_frame(&self) -> bool {
        crate::SysExInbox::mid_frame(self)
    }
}

impl A4Sink for midir::MidiOutputConnection {
    fn send_chunk(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.send(bytes).map_err(|e| MidiError::Send(e.to_string()))
    }
}

// --- IN ----------------------------------------------------------------------

/// How long [`receive_patterns`] waits, and for what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiveConfig {
    /// How long to wait for the **first** frame. This is a person walking to the
    /// box and finding SETTINGS > SYSEX DUMP, so it is generous by design — and
    /// it is a backstop rather than the control, because cancelling is what the
    /// user actually reaches for.
    pub arm_timeout: Duration,
    /// Silence after the last complete frame that means the dump is over.
    ///
    /// Must comfortably exceed the gap the box leaves *between* patterns when
    /// dumping several, or a bank dump would be reported as one pattern and then
    /// a pile of ignored ones arriving after the window closed.
    pub quiet_after: Duration,
    /// How often to look.
    pub poll: Duration,
}

impl Default for ReceiveConfig {
    fn default() -> Self {
        Self {
            arm_timeout: Duration::from_secs(120),
            quiet_after: Duration::from_secs(3),
            poll: Duration::from_millis(20),
        }
    }
}

/// A frame that arrived in the window and was not an A4 pattern.
///
/// Kept rather than discarded because the likeliest cause is a real mistake a
/// person can fix — the wrong box cabled, or the wrong dump chosen on the right
/// box — and a silent drop presents both as "nothing happened".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredFrame {
    pub bytes: usize,
    /// The dump family byte, when the frame was an Elektron dump at all.
    /// `Some(0x0c)` here is a Digitakt talking into an A4's window.
    pub family: Option<u8>,
    /// Why it was passed over, in the parser's own words.
    pub why: String,
}

/// What a receive reports as it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveProgress {
    /// Complete patterns so far.
    pub patterns: usize,
    /// Frames seen, including ignored ones.
    pub frames: usize,
    /// The slot the last pattern named, `None` if the last frame was ignored.
    pub slot: Option<u8>,
    /// True while nothing has arrived yet — what a panel draws "waiting for the
    /// box" from, as against "receiving".
    pub armed: bool,
}

/// How a receive ended.
#[derive(Debug, Clone)]
pub struct ReceiveReport {
    pub patterns: Vec<A4Pattern>,
    pub ignored: Vec<IgnoredFrame>,
    /// The caller's flag ended it. Whatever arrived is still in `patterns` and
    /// is still worth keeping — the same rule `scan_bank` follows.
    pub cancelled: bool,
    /// Nothing arrived before [`ReceiveConfig::arm_timeout`]. Distinct from
    /// cancelled, because it means the box never sent and the user should be
    /// told to check the cable rather than told they gave up.
    pub timed_out: bool,
}

impl ReceiveReport {
    /// Nothing usable arrived. True for a timeout and for a window that heard
    /// only another box.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// Wait for the box to dump one or more patterns.
///
/// Returns when a quiet period follows at least one frame, when `cancel` is set,
/// or when nothing at all has arrived by [`ReceiveConfig::arm_timeout`]. It never
/// returns `Err` for silence: silence is a [`ReceiveReport`], because "the box
/// said nothing" is an answer a person needs shown, not an error to be logged.
pub fn receive_patterns(
    listener: &mut impl A4Listener,
    cfg: ReceiveConfig,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(ReceiveProgress),
) -> ReceiveReport {
    let mut patterns = Vec::new();
    let mut ignored = Vec::new();
    let mut frames = 0usize;

    // Two accumulators rather than one clock. `since_start` only matters until
    // the first frame; `since_frame` only matters after it.
    let mut since_start = Duration::ZERO;
    let mut since_frame = Duration::ZERO;

    on_progress(ReceiveProgress { patterns: 0, frames: 0, slot: None, armed: true });

    loop {
        if cancel.load(Ordering::Relaxed) {
            return ReceiveReport { patterns, ignored, cancelled: true, timed_out: false };
        }

        let arrived = listener.drain();
        if arrived.is_empty() {
            // A frame half-delivered is not silence. Without this a long dump
            // arriving in driver-sized pieces would trip the quiet period
            // between two of them.
            if listener.mid_frame() {
                since_frame = Duration::ZERO;
            }
            if frames == 0 {
                if since_start >= cfg.arm_timeout {
                    return ReceiveReport {
                        patterns,
                        ignored,
                        cancelled: false,
                        timed_out: true,
                    };
                }
            } else if since_frame >= cfg.quiet_after {
                return ReceiveReport { patterns, ignored, cancelled: false, timed_out: false };
            }
            let waited = listener.wait(cfg.poll);
            since_start += waited;
            since_frame += waited;
            continue;
        }

        for frame in arrived {
            frames += 1;
            since_frame = Duration::ZERO;
            let slot = match classify(&frame) {
                Ok(p) => {
                    let slot = p.slot;
                    patterns.push(p);
                    Some(slot)
                }
                Err(ignore) => {
                    ignored.push(ignore);
                    None
                }
            };
            on_progress(ReceiveProgress {
                patterns: patterns.len(),
                frames,
                slot,
                armed: false,
            });
        }
    }
}

/// One frame: an A4 pattern, or a reason it is not.
///
/// The family is read even when the parse fails, because "a Digitakt answered"
/// and "this pattern is corrupt" are different problems with different fixes and
/// [`parse_pattern`]'s message alone does not separate them for a reader.
fn classify(frame: &[u8]) -> Result<A4Pattern, IgnoredFrame> {
    match parse_pattern(frame) {
        Ok(p) => Ok(p),
        Err(why) => {
            let parsed = parse_sysex(frame);
            let family = parsed.dump.as_ref().map(|d| d.family).filter(|_| !is_a4_pattern(&parsed));
            Err(IgnoredFrame { bytes: frame.len(), family, why })
        }
    }
}

// --- OUT ---------------------------------------------------------------------

/// How a frame is broken up on its way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacing {
    /// Bytes per packet. Zero means "all of it", which is [`Pacing::single`].
    pub chunk: usize,
    /// The hold-off between packets.
    pub gap: Duration,
}

impl Default for Pacing {
    fn default() -> Self {
        Self::din()
    }
}

impl Pacing {
    /// The default: 256-byte packets spaced to arrive at DIN rate.
    ///
    /// The chunk size is not tuned and does not need to be — what mattered on
    /// 2026-08-30 was that the frame arrives over seconds rather than at once.
    pub fn din() -> Self {
        const CHUNK: usize = 256;
        Self {
            chunk: CHUNK,
            gap: Duration::from_secs_f64(CHUNK as f64 / DIN_BYTES_PER_SEC),
        }
    }

    /// The whole frame in one call, with no hold-off — **the shape that did
    /// nothing on 2026-08-30**.
    ///
    /// Kept reachable so the difference stays measurable. A caller choosing this
    /// is running an experiment, not saving 4.75 seconds.
    pub fn single() -> Self {
        Self { chunk: 0, gap: Duration::ZERO }
    }

    /// The pacing this platform will actually deliver.
    ///
    /// `can_pace` is a parameter rather than a read of [`CAN_PACE`] for the
    /// reason `device::paced_send` takes its chunk size as one: a rule that only
    /// compiles on Windows is a rule this repo's Macs can never run. Every caller
    /// passes [`CAN_PACE`]; the tests pass both.
    pub fn resolve(self, can_pace: bool) -> Self {
        if can_pace {
            self
        } else {
            Self::single()
        }
    }

    /// How many packets `len` bytes becomes.
    pub fn packets(&self, len: usize) -> usize {
        if self.chunk == 0 {
            1
        } else {
            len.div_ceil(self.chunk)
        }
    }

    /// Roughly how long delivering `len` bytes will take, for a panel to say so
    /// before the user commits to it.
    pub fn estimate(&self, len: usize) -> Duration {
        self.gap * self.packets(len).saturating_sub(1) as u32
    }
}

/// Proof that a person agreed to overwrite a pattern slot.
///
/// A bool parameter is something a caller passes `true` to. This has to be
/// constructed, and constructing it takes the slot — so a consent obtained for
/// A01 cannot be spent on A16. That is the whole of its job, and it is the only
/// one of §7 rule 1's five steps this path can actually keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    slot: u8,
}

impl Consent {
    /// The caller asserts a human agreed to overwrite this slot, having been
    /// told there is no backup.
    pub fn given_for(slot: u8) -> Self {
        Self { slot }
    }

    pub fn slot(&self) -> u8 {
        self.slot
    }
}

/// What a send reports as it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendProgress {
    pub packets_sent: usize,
    pub packets_total: usize,
    pub bytes_sent: usize,
    pub bytes_total: usize,
}

/// How a send ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReport {
    pub bytes: usize,
    pub packets: usize,
    /// What it actually took, accumulated from [`A4Sink::pace`].
    pub elapsed: Duration,
    pub slot: u8,
    /// Whether the frame was actually paced.
    ///
    /// False means one unpaced call went out — either because the caller asked
    /// for [`Pacing::single`], or because [`CAN_PACE`] is false here. In both
    /// cases the box very likely ignored it, and a report that did not say so
    /// would read as a successful write.
    pub paced: bool,
}

#[derive(Debug)]
pub enum SendError {
    /// These bytes are not a well-formed A4 pattern dump. Checked immediately
    /// before the send and not only at build time — see [`verify_before_send`].
    NotSendable(String),
    /// The consent names a different slot from the message.
    ConsentMismatch { consented: u8, message: u8 },
    /// The user's flag stopped it. **The box now holds a partial message** and
    /// its SysEx API is likely wedged until it is power-cycled.
    Cancelled { packets_sent: usize, packets_total: usize },
    /// The wire failed partway. Same consequence as a cancel.
    Wire { packet: usize, packets_total: usize, source: MidiError },
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSendable(why) => {
                write!(f, "refusing to send bytes this process cannot verify: {why}")
            }
            Self::ConsentMismatch { consented, message } => write!(
                f,
                "consent was given for slot {consented} and the message is for slot {message}"
            ),
            Self::Cancelled { packets_sent, packets_total } => write!(
                f,
                "stopped after {packets_sent} of {packets_total} packets — the box holds a \
                 partial message and should be power-cycled before retrying"
            ),
            Self::Wire { packet, packets_total, source } => write!(
                f,
                "packet {packet} of {packets_total} failed: {source} — the box holds a partial \
                 message and should be power-cycled before retrying"
            ),
        }
    }
}

impl std::error::Error for SendError {}

/// The two checks that belong to *bytes about to go on a cable* rather than to
/// any one format: the frame is `F0 … F7` shaped, and no byte inside it has its
/// high bit set.
///
/// Shared by [`verify_before_send`] and [`verify_kit_before_send`] because it is
/// the half that does not care which object is being sent — and because saying
/// *which* byte is what makes it a diagnosis rather than a rejection.
fn verify_frame(wire: &[u8]) -> Result<(), String> {
    match (wire.first(), wire.last()) {
        (Some(0xf0), Some(0xf7)) => {}
        (Some(0xf0), _) => return Err("starts F0 but does not end F7 — truncated".into()),
        _ => return Err("not an F0 … F7 frame".into()),
    }
    let body = &wire[1..wire.len() - 1];
    if let Some(i) = body.iter().position(|b| b & 0x80 != 0) {
        return Err(format!("byte {i} is {:#04x}: high bit set inside the frame", body[i]));
    }
    Ok(())
}

/// [`verify_before_send`]'s twin for a **working kit** — the `0x58` a preset
/// load sends.
///
/// Parsed as the object it claims to be, immediately before it leaves, for the
/// reason the pattern path documents at length: `a4_kit::build_working_kit`
/// already checked these bytes when it built them, and a `Vec<u8>` can be
/// sliced or edited between the two. The thing that must be true is not "the
/// builder was correct" but "*these* bytes are well-formed", and a body this box
/// cannot parse takes its whole SysEx API down until it is power-cycled
/// (`DEVELOPMENT.md` lesson 13).
pub fn verify_kit_before_send(wire: &[u8]) -> Result<A4Kit, String> {
    verify_frame(wire)?;
    parse_working_kit(wire)
}

/// Everything that must be true of a frame before one of its bytes leaves.
///
/// **Deliberately duplicated.** `a4_pattern::build_pattern` already checked the
/// framing when it built these bytes, and this checks them again. The thing that
/// must be true is not "the builder was correct" but "*these* bytes are
/// well-formed", and a `Vec<u8>` can be sliced, loaded from a file, or edited by
/// hand between the two. A body this box cannot parse takes its whole SysEx API
/// down until it is power-cycled (`DEVELOPMENT.md` lesson 13), so the check that
/// matters is the one immediately before the send.
///
/// The two checks that are not `parse_pattern`'s belong to *bytes about to go on
/// a cable* rather than to the format: the frame must be `F0 … F7` shaped, and no
/// byte inside it may have its high bit set. A parser would reject the second
/// eventually; saying *which* byte is what makes it a diagnosis.
pub fn verify_before_send(wire: &[u8]) -> Result<A4Pattern, String> {
    verify_frame(wire)?;
    let parsed = parse_pattern(wire)?;
    // `parse_pattern` checks this too. Repeated because it is the one invariant
    // every offset in `a4_pattern` is written against.
    if parsed.payload.len() != PAYLOAD_LEN {
        return Err(format!("payload is {} bytes, not {PAYLOAD_LEN}", parsed.payload.len()));
    }
    Ok(parsed)
}

/// Overwrite one pattern slot on an Analog Four.
///
/// There is no backup, no read-back and no re-fetch — see the module doc. What
/// this does keep is the two things it can: bytes verified immediately before
/// they leave, and a [`Consent`] that names the slot it was given for.
///
/// `cancel` is honoured between packets. It is offered because a 4.75-second
/// send is long enough for a person to change their mind, but note what
/// [`SendError::Cancelled`] says: stopping partway leaves the box holding half a
/// message, which is worse than either finishing or never starting.
pub fn send_pattern(
    sink: &mut impl A4Sink,
    wire: &[u8],
    pacing: Pacing,
    consent: Consent,
    cancel: &AtomicBool,
    on_progress: impl FnMut(SendProgress),
) -> Result<SendReport, SendError> {
    send_pattern_with(sink, wire, pacing, consent, cancel, on_progress, CAN_PACE)
}

/// The body of [`send_pattern`], with the platform rule as an argument.
///
/// **`can_pace` is a parameter rather than a read of [`CAN_PACE`] so that both
/// platforms' rules can be tested on either platform**, for the same reason
/// `device::paced_send` takes its chunk size as one. Without this seam the
/// multi-packet failure modes below — a cancel between packets, a wire error on
/// packet 2 — are unreachable on Windows, where [`CAN_PACE`] is false and every
/// frame goes out as a single packet. They were not merely untested there: they
/// asserted the opposite of what happens, and failed the release build twice.
///
/// This is private, so the invariant [`send_pattern`] documents still holds for
/// every real caller: none of them can construct a pacing this platform will
/// refuse mid-frame.
#[allow(clippy::too_many_arguments)]
fn send_pattern_with(
    sink: &mut impl A4Sink,
    wire: &[u8],
    pacing: Pacing,
    consent: Consent,
    cancel: &AtomicBool,
    on_progress: impl FnMut(SendProgress),
    can_pace: bool,
) -> Result<SendReport, SendError> {
    let parsed = verify_before_send(wire).map_err(SendError::NotSendable)?;
    if parsed.slot != consent.slot() {
        return Err(SendError::ConsentMismatch {
            consented: consent.slot(),
            message: parsed.slot,
        });
    }

    send_frame(sink, wire, pacing, cancel, on_progress, can_pace, parsed.slot)
}

/// Overwrite the **working kit** on an Analog Four — its edit buffer, and the
/// destination a preset load splices into.
///
/// [`send_pattern`]'s sibling, and the differences are both about what is being
/// written rather than about how:
///
/// * **No [`Consent`].** Consent names a slot, and this is not one: the working
///   kit is the kit the box is playing, and the box's own undo — reloading the
///   pattern, which discards an unsaved kit — is what makes an audition
///   recoverable. That is the same recovery story `crate::preset_load`
///   documents for a digi's active kit, and its store takes no consent object
///   either. What a caller owes the user here is the sentence, and
///   `ui::presets` says it on screen every time.
/// * **A kit is 2,770 framed bytes against a pattern's 14,843**, so a DIN-paced
///   send lands in under a second rather than in five. Nothing else changes:
///   the pacing is the cable's, because it is the same cable and the same 2013
///   box that has never once accepted an unpaced frame.
pub fn send_working_kit(
    sink: &mut impl A4Sink,
    wire: &[u8],
    pacing: Pacing,
    cancel: &AtomicBool,
    on_progress: impl FnMut(SendProgress),
) -> Result<SendReport, SendError> {
    send_working_kit_with(sink, wire, pacing, cancel, on_progress, CAN_PACE)
}

/// The body of [`send_working_kit`], with the platform rule as an argument —
/// see [`send_pattern_with`] for why that seam exists.
fn send_working_kit_with(
    sink: &mut impl A4Sink,
    wire: &[u8],
    pacing: Pacing,
    cancel: &AtomicBool,
    on_progress: impl FnMut(SendProgress),
    can_pace: bool,
) -> Result<SendReport, SendError> {
    let kit = verify_kit_before_send(wire).map_err(SendError::NotSendable)?;
    send_frame(sink, wire, pacing, cancel, on_progress, can_pace, kit.index)
}

/// The paced loop itself, once the bytes have been verified as whatever they
/// claim to be.
///
/// **Split out on 2026-09-01, when the kit send arrived, and split here on
/// purpose.** Everything above this line is about one format — a pattern's
/// slot, a kit's edit buffer, the consent one of them needs — and everything
/// below it is about a cable: the chunking, the hold-off, the cancel between
/// packets, the platform collapse. The two objects must not each grow their own
/// copy of the second half, because the second half is where the failures cost a
/// power cycle.
///
/// `slot` is only reported, never acted on: [`SendReport::slot`] is the
/// message's own index byte, which is the pattern slot for a `0x54` and zero for
/// a working kit.
#[allow(clippy::too_many_arguments)]
fn send_frame(
    sink: &mut impl A4Sink,
    wire: &[u8],
    pacing: Pacing,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(SendProgress),
    can_pace: bool,
    slot: u8,
) -> Result<SendReport, SendError> {
    // Resolved here rather than at the call site so no caller can construct a
    // pacing this platform will refuse mid-frame.
    let pacing = pacing.resolve(can_pace);
    let packets_total = pacing.packets(wire.len());
    let chunk = if pacing.chunk == 0 { wire.len().max(1) } else { pacing.chunk };
    let mut elapsed = Duration::ZERO;
    let mut bytes_sent = 0usize;

    for (i, piece) in wire.chunks(chunk).enumerate() {
        // Checked before the packet rather than after, so a cancel that arrives
        // during the hold-off costs one fewer packet.
        if cancel.load(Ordering::Relaxed) {
            return Err(SendError::Cancelled { packets_sent: i, packets_total });
        }
        sink.send_chunk(piece).map_err(|source| SendError::Wire {
            packet: i + 1,
            packets_total,
            source,
        })?;
        bytes_sent += piece.len();
        on_progress(SendProgress {
            packets_sent: i + 1,
            packets_total,
            bytes_sent,
            bytes_total: wire.len(),
        });
        // No hold-off after the last packet: it would delay the report without
        // pacing anything.
        if i + 1 < packets_total && !pacing.gap.is_zero() {
            elapsed += sink.pace(pacing.gap);
        }
    }

    Ok(SendReport {
        bytes: wire.len(),
        packets: packets_total,
        elapsed,
        slot,
        paced: packets_total > 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_protocol::a4_pattern::build_pattern;

    // --- fakes ---------------------------------------------------------------

    /// A listener that hands out scripted frames on scripted polls, and whose
    /// `wait` costs the nominal time and no real time at all. A 120-second arm
    /// timeout expires here in a few microseconds.
    #[derive(Default)]
    struct FakeListener {
        /// One entry per `drain` call; anything past the end is silence.
        script: Vec<Vec<Vec<u8>>>,
        calls: usize,
        /// Polls on which a frame is half-delivered.
        mid_frame_on: Vec<usize>,
    }

    impl A4Listener for FakeListener {
        fn drain(&mut self) -> Vec<Vec<u8>> {
            let out = self.script.get(self.calls).cloned().unwrap_or_default();
            self.calls += 1;
            out
        }

        fn mid_frame(&self) -> bool {
            self.mid_frame_on.contains(&self.calls)
        }

        fn wait(&mut self, gap: Duration) -> Duration {
            gap
        }
    }

    #[derive(Default)]
    struct FakeSink {
        packets: Vec<Vec<u8>>,
        gaps: Vec<Duration>,
        /// Fail on this 1-based packet.
        fail_on: Option<usize>,
    }

    impl A4Sink for FakeSink {
        fn send_chunk(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
            if self.fail_on == Some(self.packets.len() + 1) {
                return Err(MidiError::Send("cable".into()));
            }
            self.packets.push(bytes.to_vec());
            Ok(())
        }

        fn pace(&mut self, gap: Duration) -> Duration {
            self.gaps.push(gap);
            gap
        }
    }

    // --- fixtures ------------------------------------------------------------

    fn a4_frame(slot: u8) -> Vec<u8> {
        build_pattern(slot, &vec![0u8; PAYLOAD_LEN]).expect("all-zero payload is seven-bit clean")
    }

    /// A Digitakt II project-settings dump: the same `0x54` opcode byte, a
    /// different family, and the exact frame that must not be read as a pattern.
    fn dt2_frame() -> Vec<u8> {
        use digi_protocol::protocol::{
            build_dump_message, DUMP_PROJECT_SETTINGS, FAMILY_DIGITAKT_2,
        };
        build_dump_message(FAMILY_DIGITAKT_2, DUMP_PROJECT_SETTINGS, 0, &[0u8; 64])
    }

    fn quick() -> ReceiveConfig {
        ReceiveConfig {
            arm_timeout: Duration::from_secs(1),
            quiet_after: Duration::from_millis(100),
            poll: Duration::from_millis(10),
        }
    }

    // --- receive -------------------------------------------------------------

    #[test]
    fn one_pattern_then_silence_is_one_pattern() {
        let mut l = FakeListener { script: vec![vec![], vec![a4_frame(3)]], ..Default::default() };
        let report = receive_patterns(&mut l, quick(), &AtomicBool::new(false), |_| {});
        assert_eq!(report.patterns.len(), 1);
        assert_eq!(report.patterns[0].slot, 3);
        assert!(!report.timed_out && !report.cancelled);
    }

    #[test]
    fn a_bank_dump_is_every_pattern_in_it() {
        let mut l = FakeListener {
            script: vec![
                vec![a4_frame(0), a4_frame(1)],
                vec![],
                vec![a4_frame(2)],
            ],
            ..Default::default()
        };
        let report = receive_patterns(&mut l, quick(), &AtomicBool::new(false), |_| {});
        assert_eq!(
            report.patterns.iter().map(|p| p.slot).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "a quiet poll between two frames must not end a dump the box is still sending"
        );
    }

    #[test]
    fn silence_from_the_start_times_out_rather_than_erroring() {
        let mut l = FakeListener::default();
        let report = receive_patterns(&mut l, quick(), &AtomicBool::new(false), |_| {});
        assert!(report.timed_out);
        assert!(!report.cancelled);
        assert!(report.is_empty());
    }

    #[test]
    fn a_half_delivered_frame_is_not_silence() {
        // Nothing completes until poll 40, well past a 100 ms quiet period at a
        // 10 ms poll — but the reassembler is mid-frame throughout, so the
        // window must stay open.
        let mut script = vec![vec![]; 40];
        script.push(vec![a4_frame(5)]);
        let mut l = FakeListener {
            script,
            mid_frame_on: (1..40).collect(),
            ..Default::default()
        };
        let cfg = ReceiveConfig { arm_timeout: Duration::from_secs(60), ..quick() };
        let report = receive_patterns(&mut l, cfg, &AtomicBool::new(false), |_| {});
        assert_eq!(report.patterns.len(), 1, "a dump still arriving was cut off");
        assert_eq!(report.patterns[0].slot, 5);
    }

    #[test]
    fn a_digitakt_answering_is_recorded_rather_than_read_as_a_pattern() {
        let mut l = FakeListener { script: vec![vec![dt2_frame()]], ..Default::default() };
        let report = receive_patterns(&mut l, quick(), &AtomicBool::new(false), |_| {});
        assert!(report.patterns.is_empty(), "0x54 is a different message on a DT2");
        assert_eq!(report.ignored.len(), 1);
        assert_eq!(
            report.ignored[0].family,
            Some(digi_protocol::protocol::FAMILY_DIGITAKT_2),
            "the user needs to be told which box answered, not just that one did"
        );
    }

    #[test]
    fn cancel_keeps_what_already_arrived() {
        let cancel = AtomicBool::new(false);
        let mut l = FakeListener { script: vec![vec![a4_frame(7)]], ..Default::default() };
        let mut seen = 0;
        let report = receive_patterns(&mut l, quick(), &cancel, |p| {
            seen = p.patterns;
            if p.patterns == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(report.cancelled);
        assert_eq!(report.patterns.len(), 1, "a cancelled receive must not throw away its work");
    }

    #[test]
    fn progress_opens_armed_and_stops_being_armed_once_something_lands() {
        let mut l = FakeListener { script: vec![vec![a4_frame(0)]], ..Default::default() };
        let mut armed = Vec::new();
        let _ = receive_patterns(&mut l, quick(), &AtomicBool::new(false), |p| armed.push(p.armed));
        assert_eq!(armed.first(), Some(&true));
        assert_eq!(armed.last(), Some(&false));
    }

    // --- pacing --------------------------------------------------------------

    #[test]
    fn din_pacing_spreads_a_pattern_over_about_five_seconds() {
        let wire = a4_frame(0);
        let est = Pacing::din().estimate(wire.len());
        // 14,843 bytes at 3,125 a second is 4.75 s. The estimate is one gap
        // short of that, because the last packet is not followed by one.
        assert!(
            (4.5..5.0).contains(&est.as_secs_f64()),
            "a paced send should take about as long as the cable would: {est:?}"
        );
    }

    #[test]
    fn single_is_one_packet_and_no_gap() {
        let p = Pacing::single();
        assert_eq!(p.packets(14_843), 1);
        assert_eq!(p.estimate(14_843), Duration::ZERO);
    }

    // --- send ----------------------------------------------------------------

    #[test]
    fn windows_collapses_to_one_call_rather_than_chunking_into_a_driver_that_refuses() {
        // The rule is tested on whatever platform this runs on, which is the
        // whole reason `resolve` takes a bool. Chunking here would fail on
        // packet 2 and wedge the box.
        assert_eq!(Pacing::din().resolve(false), Pacing::single());
        assert_eq!(Pacing::din().resolve(true), Pacing::din());
    }

    #[test]
    fn an_unpaced_send_reports_that_it_was_not_paced() {
        let wire = a4_frame(0);
        let mut sink = FakeSink::default();
        let report = send_pattern(
            &mut sink,
            &wire,
            Pacing::single(),
            Consent::given_for(0),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert!(
            !report.paced,
            "a report that did not say so would read as a successful write"
        );
    }

    #[test]
    fn a_paced_send_delivers_every_byte_in_order() {
        let wire = a4_frame(2);
        let mut sink = FakeSink::default();
        let report = send_pattern(
            &mut sink,
            &wire,
            Pacing::din(),
            Consent::given_for(2),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("a frame this module built should send");
        assert_eq!(report.bytes, wire.len());
        assert_eq!(report.paced, CAN_PACE);
        assert_eq!(sink.packets.concat(), wire, "the box must receive exactly the frame given");
        assert_eq!(sink.gaps.len(), report.packets - 1, "no hold-off after the last packet");
    }

    #[test]
    fn the_a4_pacing_is_two_orders_slower_than_the_digis_digest_budget() {
        // `device::paced_send` spends `len / 800 ms`. If these two ever converge,
        // one of them has been changed to the other's constant by mistake.
        let wire = a4_frame(0);
        let digi = Duration::from_millis(wire.len().div_ceil(800) as u64);
        let a4 = Pacing::din().estimate(wire.len());
        assert!(
            a4 > digi * 100,
            "an A4 send at digi pacing is the unpaced shape with extra steps: {a4:?} vs {digi:?}"
        );
    }

    #[test]
    fn an_unpaced_send_is_one_call_and_stays_reachable() {
        let wire = a4_frame(0);
        let mut sink = FakeSink::default();
        send_pattern(
            &mut sink,
            &wire,
            Pacing::single(),
            Consent::given_for(0),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(sink.packets.len(), 1);
        assert!(sink.gaps.is_empty());
    }

    #[test]
    fn consent_for_one_slot_cannot_be_spent_on_another() {
        let wire = a4_frame(15);
        let mut sink = FakeSink::default();
        let err = send_pattern(
            &mut sink,
            &wire,
            Pacing::din(),
            Consent::given_for(0),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect_err("A01 consent must not overwrite A16");
        assert!(matches!(err, SendError::ConsentMismatch { consented: 0, message: 15 }));
        assert!(sink.packets.is_empty(), "nothing may leave before the check");
    }

    #[test]
    fn a_frame_edited_after_it_was_built_is_refused_before_any_byte_leaves() {
        let mut wire = a4_frame(0);
        let mid = wire.len() / 2;
        wire[mid] ^= 0x01; // breaks the checksum
        let mut sink = FakeSink::default();
        let err = send_pattern(
            &mut sink,
            &wire,
            Pacing::din(),
            Consent::given_for(0),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect_err("the check that matters is the one immediately before the send");
        assert!(matches!(err, SendError::NotSendable(_)));
        assert!(sink.packets.is_empty());
    }

    #[test]
    fn a_high_bit_inside_the_frame_names_the_byte() {
        let mut wire = a4_frame(0);
        wire[100] = 0x80;
        let err = verify_before_send(&wire).expect_err("0x80 is not legal inside a frame");
        assert!(err.contains("99"), "the diagnosis is which byte, not that there was one: {err}");
    }

    #[test]
    fn a_truncated_frame_says_truncated() {
        let wire = a4_frame(0);
        let err = verify_before_send(&wire[..wire.len() - 1]).expect_err("no F7");
        assert!(err.contains("truncated"), "{err}");
    }

    // The four tests below pass `can_pace` explicitly rather than letting
    // `send_pattern` read `CAN_PACE`. Following the platform constant is what
    // made the first two assert the opposite of the truth on Windows, where a
    // frame is never split and there is no packet 2 to drop and no gap between
    // packets to cancel in. Both rules now run on both platforms.

    #[test]
    fn a_cancelled_send_says_the_box_holds_half_a_message() {
        let wire = a4_frame(0);
        let cancel = AtomicBool::new(false);
        let mut sink = FakeSink::default();
        let err = send_pattern_with(
            &mut sink,
            &wire,
            Pacing::din(),
            Consent::given_for(0),
            &cancel,
            |p| {
                if p.packets_sent == 3 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            true,
        )
        .expect_err("cancelling mid-frame is a failure, not a tidy stop");
        assert!(matches!(err, SendError::Cancelled { packets_sent: 3, .. }));
        assert!(err.to_string().contains("power-cycled"));
    }

    #[test]
    fn a_wire_failure_names_the_packet_and_the_recovery() {
        let wire = a4_frame(0);
        let mut sink = FakeSink { fail_on: Some(2), ..Default::default() };
        let err = send_pattern_with(
            &mut sink,
            &wire,
            Pacing::din(),
            Consent::given_for(0),
            &AtomicBool::new(false),
            |_| {},
            true,
        )
        .expect_err("a dropped packet is not a partial success");
        assert!(matches!(err, SendError::Wire { packet: 2, .. }));
        assert!(err.to_string().contains("power-cycled"));
    }

    #[test]
    fn an_unpaced_wire_failure_still_names_a_packet_and_the_recovery() {
        // Windows' rule: one packet, so the only packet that can fail is the
        // first, and its failure is the whole write failing.
        let wire = a4_frame(0);
        let mut sink = FakeSink { fail_on: Some(1), ..Default::default() };
        let err = send_pattern_with(
            &mut sink,
            &wire,
            Pacing::din(),
            Consent::given_for(0),
            &AtomicBool::new(false),
            |_| {},
            false,
        )
        .expect_err("a refused single packet is not a partial success either");
        assert!(matches!(err, SendError::Wire { packet: 1, packets_total: 1, .. }), "{err:?}");
        assert!(err.to_string().contains("power-cycled"));
    }

    #[test]
    fn an_unpaced_send_cancelled_before_it_starts_sends_nothing() {
        // The one cancel an unpaced send can honour: there is no gap between
        // packets, so a mind changed after the call is a mind changed too late.
        let wire = a4_frame(0);
        let mut sink = FakeSink::default();
        let err = send_pattern_with(
            &mut sink,
            &wire,
            Pacing::din(),
            Consent::given_for(0),
            &AtomicBool::new(true),
            |_| {},
            false,
        )
        .expect_err("a cancel already set before the first packet stops the send");
        assert!(matches!(err, SendError::Cancelled { packets_sent: 0, packets_total: 1 }), "{err:?}");
        assert!(sink.packets.is_empty(), "nothing should have reached the box");
    }
}
