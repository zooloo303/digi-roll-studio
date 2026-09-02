//! The Analog Four's **gen-1** p-lock pool: 128 lanes of 66 bytes, read off and
//! written back to a pattern payload.
//!
//! The gen-1 counterpart to [`crate::plocks`], and a separate module rather than
//! a generation parameter on that one. The reasons are in
//! [What gen-1 does not share with gen-2](#what-gen-1-does-not-share-with-gen-2)
//! below; the short version is that the two pools agree on their lane *header*
//! and on nothing else, and threading a generation through the gen-2 code would
//! make a hardware-verified write policy conditional in order to express a
//! structure it cannot represent anyway.
//!
//! Everything here is measured, on 2026-08-31, from six single-variable captures
//! against a cleared A16 — each one differing from the capture before it by
//! exactly one change on the box. The `analogfour-A16-plock-*.syx` fixtures are
//! the evidence.
//!
//! ```text
//!   128 lanes × 66 bytes, from 4,510:
//!     +0    param_id  u8    which parameter this lane automates, FF = free
//!     +1    track     u8    which track it belongs to, FF = free
//!     +2    64 × u8         one coarse value per step, FF = no lock
//! ```
//!
//! # The header is the gen-2 header
//!
//! `[param_id][track]` — the same two bytes [`crate::plocks`] reads on the DT2
//! and DN2, on a box that disagrees with them about everything else in this
//! region. Locking FLTR1 FREQ to 64 on SYN1 step 1 moved lane 0's header from
//! `FF FF` to `22 00` and put `0x40` at step 1. Locking **the same parameter on
//! SYN2** gives `22 01`, which is what makes the second byte the track: SYN1 is
//! track 0, so `22 00` alone cannot separate "the second byte is the track" from
//! "the second byte is always zero".
//!
//! Param ids are per parameter, not per track: `0x22` is FLTR1 FREQ and `0x23`
//! is RESO — adjacent ids for adjacent knobs — and `0x22` appears on both SYN1's
//! and SYN2's lanes.
//!
//! # What gen-1 does not share with gen-2
//!
//! * **The two fills are opposite, and that has already caused one reader bug.**
//!   A free lane is `FF FF` then 64 **zero** bytes; inside an allocated lane
//!   `0xFF` is a step with **no lock**. The first pool reader shipped a `v != 0`
//!   test for "is this step locked", which would have reported all 64 steps of a
//!   real lane as locked. [`FREE`] and [`NO_VALUE`] are the same byte meaning
//!   opposite things two bytes apart, so they are named separately here.
//! * **A value is a `u8` plus an optional extension lane**, where gen-2 stores a
//!   `u16be` inline. See [Extension lanes](#extension-lanes).
//! * **The box compacts the pool**, where gen-2 does not. Adding SYN2's lock
//!   moved SYN1's existing RESO lane from index 2 to index 4, with the new pair
//!   inserted ahead of it, leaving the lanes ordered by `(param_id, track)`.
//!   [`crate::plocks`] documents the opposite for the digis — "the box does not
//!   compact the pool; it clears a freed lane in place and claims the lowest
//!   free lane including holes" — and
//!   [`crate::plocks::apply_track_plocks`]'s whole scrub-then-write policy is
//!   built on that. **A gen-1 lane index does not survive an edit on the box**,
//!   which is why [`A4LaneWrite`] cannot name one and why
//!   [`apply_track_plocks`] rebuilds the pool rather than editing it in place.
//! * 128 lanes of 66 bytes, against 80 of 258.
//!
//! # Extension lanes
//!
//! The first p-lock capture allocated **two** lanes for one knob: lane 0 as
//! above, and a lane 1 whose header read `80 80` with `0x34` at the same step.
//! Three readings were possible — end-of-pool marker, companion field, low byte
//! — and one capture separates them: change only the lock's *value*. 64 → 100
//! changed two bytes in the whole 12,974-byte payload, the coarse byte
//! `0x40 → 0x64` and the extension's `0x17 → 0x60`. RESO's lane came back
//! byte-identical, which is what it was locked for.
//!
//! So `80 80` is bound to the lane before it and carries a second byte of the
//! same value. **The coarse byte is the displayed value** — `0x40` for 64,
//! `0x64` for 100, measured twice — and the extension is sub-unit resolution
//! beneath it: four takes of a displayed "64" produced fine bytes of 23, 52, 113
//! and 116, a knob landing in different places inside one displayed integer.
//! A marker, a count or a companion field could not look like that.
//!
//! `0x8080` is a continuation marker rather than a parameter id, which is why it
//! never looked like a plausible one.
//!
//! # The fine byte is 128ths, and it is not the gen-2 quantity
//!
//! This section said the opposite until 2026-09-01: that the fine byte was
//! 256ths of a display unit and that **both generations store the same 16-bit
//! quantity**, gen-2 inline and gen-1 split. That was inference imported from
//! [`crate::plocks`], flagged as inference, and it was wrong.
//!
//! **OSC TUNE is what settled it, because it is the only parameter whose fine
//! byte the box will show you a number for.** TUN and FIN are not two p-lock
//! parameters — they are the coarse and fine halves of *one* lane, which is why
//! locking either locks both on the front panel and why turning FIN allocates
//! nothing. So FIN is a fine byte with its own on-screen value, and it can be
//! read against the bytes:
//!
//! ```text
//!   TUN  0, FIN   0   ->  coarse 64, fine   0
//!   TUN  0, FIN  +1   ->  coarse 64, fine   1     one click, one byte
//!   TUN  0, FIN +63   ->  coarse 64, fine  63     the top of FIN's range
//!   TUN  0, FIN -64   ->  coarse 63, fine  64     the coarse byte *borrows*
//!   TUN +1, FIN   0   ->  coarse 65, fine   0     and carries at 128
//! ```
//!
//! So a value is fixed point with **128 fractional steps per display unit**:
//!
//! ```text
//!   value = (coarse - 64) + fine / 128        fine is 0..=127
//! ```
//!
//! Three things follow, and each one contradicts something this module used to
//! say.
//!
//! * **The fine byte never sets its top bit.** Measured on tune by watching the
//!   carry from `fine 127` to `coarse + 1, fine 0`, and true of every fine byte
//!   in every capture — 24 of them, highest 116, which
//!   `tests/a4.rs::no_captured_fine_byte_uses_the_top_bit` pins. Under a 256ths
//!   reading roughly half should exceed 127.
//! * **The word is not display × 256, and the generations do not agree.** The
//!   integer part scales by 256 and the fraction by 128, so
//!   [`A4Lane::word`] is not linear in the displayed value and words with a fine
//!   byte of 128–255 are unreachable. It remains a faithful, reversible encoding
//!   of the stored state — which is all the round trip needs — but it is not a
//!   quantity to scale arithmetically without knowing this.
//! * **The coarse byte is not always the displayed value.** `coarse 63, fine 64`
//!   reads on the box as TUN **0** with FIN −64, not TUN −1. The claim held for
//!   FLTR1 FREQ, RESO and OVERDRIVE and was written as "measured twice"; all
//!   three are unipolar, and tune is the first bipolar parameter it met. A UI
//!   showing the coarse byte as the parameter's value would be off by one across
//!   half of tune's range.
//!
//! # The write path, and what it cost to get
//!
//! [`apply_track_plocks`] is the gen-1 counterpart to
//! [`crate::plocks::apply_track_plocks`], and it waited on three things. All
//! three are now measured.
//!
//! 1. ~~**Whether the box omits an extension whose fine bytes are all zero.**~~
//!    **Answered 2026-08-31: FLTR1 RESO is integer-valued.** Four RESO locks on
//!    one lane at 0, 50, 90 and 127 allocated no extension, where the
//!    omit-when-zero reading would need four 1-in-256 accidents. So an encoder
//!    **emits an extension iff some fine byte is non-zero** — which was the rule
//!    under either answer, so this closed confidence rather than the rule.
//!
//!    Two things that capture is worth reading for beyond its verdict.
//!    **`0` and `127` in one lane** say RESO spans the full range as integers.
//!    And its FREQ control lane, four steps wide, holds its fine bytes at exactly
//!    its parent's four steps with [`NO_VALUE`] elsewhere — so **an extension is
//!    indexed per step**, which every previous capture left as inference because
//!    every previous lock sat on step 1. `tests/a4.rs` pins all of it;
//!    `examples/a4_plock_extension_check.rs` is the tool that read it.
//!
//! 2. ~~**Whether the box requires the compacted, `(param_id, track)`-sorted
//!    order it produces.**~~ **Answered 2026-09-01, and the answer inverts the
//!    question.** Three single-variable writes to A16 — keys swapped, a hole
//!    wedged between two used lanes, an extension detached from its parent — and
//!    in every one the box parsed every lane, lost no lock, and wrote back its
//!    own canonical form. **It requires none of the three.** The sorted-compacted
//!    pool is a serialisation artefact of a box that holds p-locks keyed by
//!    `(param_id, track)` and rebuilds them on ingest, not a structure it reads
//!    in place.
//!
//!    **The encoder emits that form anyway, because the verify needs it.** A
//!    write is checked by reading the slot back and comparing byte for byte, and
//!    a box that normalises turns a correct write into a failed-looking one — 10
//!    spurious diffs for the swapped pair, 132 for the hole. See
//!    [`apply_track_plocks`], and `examples/a4_plock_order_probe.rs` for the
//!    instrument.
//!
//!    The detached-extension write paid for itself twice. It settled that **an
//!    `80 80` binds to the lane physically before it** — which this module's
//!    reader has always assumed and nothing had ever tested, because the box had
//!    never produced a pool where the two were apart — and it confirmed the
//!    per-step indexing from the write side, by re-aligning the adopted
//!    extension's fine bytes to its new parent's locked steps unprompted.
//!
//! 3. ~~**How the box frees a lane.**~~ **Answered 2026-09-01, caught free.** The
//!    lane probe was watching when A16 was cleared from the front panel, so the
//!    box's own way of freeing a lane is on record: it writes [`FREE`] into both
//!    id bytes and **zeroes all 64 value bytes**, rather than filling them with
//!    [`NO_VALUE`]. An extension lane between two used lanes (`80 80`) is freed
//!    the same way. That is not what the two opposite fills elsewhere in this
//!    format would lead anyone to guess — and an untouched free lane in a cleared
//!    pattern reads the same way, so the two are indistinguishable afterwards,
//!    which is presumably the point. The capture is
//!    `local/a4-check/lanes/a4-working-change002.syx` against `change001`.
//!
//! [`crate::a4_pattern::build_pattern`] still refuses to encode a ragged tail it
//! cannot measure rather than pick one of two orders. That refusal stands; this
//! one is discharged.

use crate::a4_pattern::{NUM_STEPS, NUM_TRACKS, PAYLOAD_LEN, TRACK_BASE, TRACK_STRIDE};

/// First byte of the pool. The tracks region ends here — `4 + 6 × 751`.
pub const POOL_BASE: usize = TRACK_BASE + NUM_TRACKS * TRACK_STRIDE;
pub const NUM_LANES: usize = 128;
/// `param_id`, `track`, then one value per step.
pub const LANE_SIZE: usize = 2 + NUM_STEPS;

/// In a lane **header** byte: this lane is unallocated. A free lane is `FF FF`
/// followed by 64 *zero* bytes.
pub const FREE: u8 = 0xFF;
/// Inside an allocated lane: this step carries no lock. The same byte as
/// [`FREE`] with the opposite sense — see the module doc.
pub const NO_VALUE: u8 = 0xFF;
/// Both header bytes `0x80`: this lane is the fine half of the lane before it.
pub const EXT: u8 = 0x80;

/// Where the pool ends. The tail region starts here.
pub const POOL_END: usize = POOL_BASE + NUM_LANES * LANE_SIZE;

/// **The geometry is forced by the data, not fitted to it.** The region between
/// the tracks and the 16-byte tail is 8,448 bytes; 128 × 66 is 8,448 exactly;
/// and in a cleared pattern all 256 `FF` bytes sit at predicted header positions
/// with the other 8,192 zero.
const _: () = assert!(POOL_END == 12_958);

/// One allocated lane, with its extension folded in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A4Lane {
    /// Which of the 128 lanes this is. Recorded for diagnosis rather than for a
    /// write: **the box compacts the pool**, so this index does not survive an
    /// edit on the hardware.
    pub lane: usize,
    /// The box's own parameter index. `0x22` is FLTR1 FREQ, `0x23` RESO; the
    /// rest of the table is not mapped for this box.
    pub param_id: u8,
    /// Zero-based track: 0 is SYN1.
    pub track: u8,
    /// The coarse value per step, `None` where the step has no lock. **This is
    /// the number the box displays**, measured twice.
    pub values: Vec<Option<u8>>,
    /// The `80 80` lane immediately after this one, if it has one: sub-unit
    /// resolution beneath [`values`](A4Lane::values), per step.
    pub fine: Option<Vec<Option<u8>>>,
    /// Index of that extension lane, when present.
    pub ext_lane: Option<usize>,
}

impl A4Lane {
    /// The stored 16-bit value at a zero-based step: the coarse byte in the high
    /// half, the fine byte in the low.
    ///
    /// **Not display × 256, and not the gen-2 quantity** — that is what this said
    /// until 2026-09-01, and the module doc has the measurement that refuted it.
    /// The fine byte is **128ths** of a display unit and never exceeds 127, so
    /// the integer part of a value scales by 256 here while its fraction scales
    /// by 128. This number is therefore *not* linear in what the box displays,
    /// and half the `u16` range is unreachable.
    ///
    /// What it is good for is what the write path needs: a faithful, reversible
    /// packing of the two bytes into one comparable value, so a lane read off the
    /// box goes back byte-exact. Anything wanting the *displayed* number wants
    /// `(coarse - 64) + fine / 128` for a bipolar parameter like tune, and the
    /// raw bytes stay reachable on [`values`](A4Lane::values) and
    /// [`fine`](A4Lane::fine) precisely because the scaling is per parameter and
    /// mostly unmeasured.
    ///
    /// A lane with no extension reads its fine byte as zero, which is what an
    /// absent extension has always meant.
    pub fn word(&self, step: usize) -> Option<u16> {
        let coarse = (*self.values.get(step)?)?;
        let fine = self.fine.as_ref().and_then(|f| f.get(step).copied().flatten()).unwrap_or(0);
        Some(u16::from(coarse) << 8 | u16::from(fine))
    }

    /// Does this lane hold a value on a step with no trig — a **trigless
    /// lock**? `live_steps` is zero-based, as [`values`](A4Lane::values) is.
    ///
    /// The gen-2 reader asks the same question for the same reason: such a lane
    /// is shown read-only and passed through byte-exact rather than edited into
    /// a lie.
    pub fn has_trigless_values(&self, live_steps: &[usize]) -> bool {
        self.values
            .iter()
            .enumerate()
            .any(|(step, v)| v.is_some() && !live_steps.contains(&step))
    }
}

fn lane_start(lane: usize) -> usize {
    POOL_BASE + lane * LANE_SIZE
}

fn check_payload(payload: &[u8]) -> Result<(), String> {
    if payload.len() != PAYLOAD_LEN {
        return Err(format!("payload is {} bytes, an A4 pattern is {PAYLOAD_LEN}", payload.len()));
    }
    Ok(())
}

/// Is this lane the fine half of the one before it?
fn is_extension(payload: &[u8], lane: usize) -> bool {
    let o = lane_start(lane);
    payload[o] == EXT && payload[o + 1] == EXT
}

/// Is this lane unallocated — `FF FF` and 64 zeros?
///
/// The value bytes are checked, not just the header. A lane whose header says
/// free but whose values are not zero is not something any capture has shown,
/// and reporting it as free would hide it.
fn is_free(payload: &[u8], lane: usize) -> bool {
    let o = lane_start(lane);
    payload[o] == FREE
        && payload[o + 1] == FREE
        && payload[o + 2..o + LANE_SIZE].iter().all(|&v| v == 0)
}

fn values_of(payload: &[u8], lane: usize) -> Vec<Option<u8>> {
    let o = lane_start(lane);
    payload[o + 2..o + LANE_SIZE]
        .iter()
        .map(|&v| (v != NO_VALUE).then_some(v))
        .collect()
}

/// Every allocated lane in the pattern, in lane order, each with its extension
/// folded in.
///
/// Extension lanes do not appear as lanes of their own: an `80 80` lane is the
/// second half of a value, not an automation of parameter 0x80. A stray one with
/// no lane before it is skipped and counted by
/// [`orphan_extension_count`].
pub fn read_all_plocks(payload: &[u8]) -> Result<Vec<A4Lane>, String> {
    check_payload(payload)?;
    let mut out = Vec::new();
    for lane in 0..NUM_LANES {
        if is_free(payload, lane) || is_extension(payload, lane) {
            continue;
        }
        let o = lane_start(lane);
        let has_ext = lane + 1 < NUM_LANES && is_extension(payload, lane + 1);
        out.push(A4Lane {
            lane,
            param_id: payload[o],
            track: payload[o + 1],
            values: values_of(payload, lane),
            fine: has_ext.then(|| values_of(payload, lane + 1)),
            ext_lane: has_ext.then_some(lane + 1),
        });
    }
    Ok(out)
}

/// One track's lanes, in lane order. `track` is zero-based.
pub fn read_track_plocks(payload: &[u8], track: usize) -> Result<Vec<A4Lane>, String> {
    if track >= NUM_TRACKS {
        return Err(format!("no track {track}; an A4 pattern has {NUM_TRACKS}"));
    }
    Ok(read_all_plocks(payload)?
        .into_iter()
        .filter(|l| usize::from(l.track) == track)
        .collect())
}

/// How many of the 128 lanes are unallocated.
///
/// Counts lanes, so an extension counts as one: a lock that needs a fine byte
/// costs two lanes, and a caller budgeting for a write needs the lane count
/// rather than the lock count.
pub fn free_lane_count(payload: &[u8]) -> Result<usize, String> {
    check_payload(payload)?;
    Ok((0..NUM_LANES).filter(|&l| is_free(payload, l)).count())
}

/// `80 80` lanes with no lane in front of them to extend.
///
/// Zero in every capture, and reported rather than ignored because a non-zero
/// answer would mean the extension rule is not what this module says it is.
pub fn orphan_extension_count(payload: &[u8]) -> Result<usize, String> {
    check_payload(payload)?;
    Ok((0..NUM_LANES)
        .filter(|&l| {
            is_extension(payload, l) && (l == 0 || is_free(payload, l - 1) || is_extension(payload, l - 1))
        })
        .count())
}

/// Every allocated lane sits below every free one — the pool has no holes.
///
/// One of the three independent properties [`is_compacted`] bundles, split out
/// because the box may require some and not others, and a single `false` cannot
/// say which. See [`PoolOrder`].
pub fn is_packed(payload: &[u8]) -> Result<bool, String> {
    check_payload(payload)?;
    let mut seen_free = false;
    for lane in 0..NUM_LANES {
        if is_free(payload, lane) {
            seen_free = true;
        } else if seen_free {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Every `80 80` lane immediately follows the lane it extends.
///
/// The reader depends on this — [`read_all_plocks`] binds an extension to the
/// lane physically before it — and no capture has ever separated the two,
/// because the box has never produced a pool where they are apart. So this is
/// the reader's own assumption stated as a predicate.
pub fn extensions_are_adjacent(payload: &[u8]) -> Result<bool, String> {
    check_payload(payload)?;
    Ok(orphan_extension_count(payload)? == 0)
}

/// The allocated lanes are non-decreasing by `(param_id, track)`.
///
/// Extensions are skipped: `80 80` is a continuation marker, not a key, and
/// including it would make every extended lane look like a sort violation.
pub fn keys_are_sorted(payload: &[u8]) -> Result<bool, String> {
    check_payload(payload)?;
    let mut last_key: Option<(u8, u8)> = None;
    for lane in 0..NUM_LANES {
        if is_free(payload, lane) || is_extension(payload, lane) {
            continue;
        }
        let o = lane_start(lane);
        let key = (payload[o], payload[o + 1]);
        if last_key.is_some_and(|prev| key < prev) {
            return Ok(false);
        }
        last_key = Some(key);
    }
    Ok(true)
}

/// The three properties of the box's own pool order, each answered separately.
///
/// **They are separate because the box may require some and not others**, and
/// that distinction decides the shape of a gen-1 pool writer:
///
/// * holes tolerated → the writer can be gen-2-shaped, editing lanes in place
///   and moving the fewest bytes;
/// * holes refused → the writer must repack, which means touching lanes
///   belonging to tracks the caller never named;
/// * key order required → the writer must sort the whole pool, same problem
///   one step further.
///
/// Which is why [`is_compacted`]'s single boolean is not enough to design
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolOrder {
    pub packed: bool,
    pub extensions_adjacent: bool,
    pub keys_sorted: bool,
}

impl PoolOrder {
    /// All three — the form the box produces.
    pub fn is_canonical(self) -> bool {
        self.packed && self.extensions_adjacent && self.keys_sorted
    }
}

/// All three order properties in one pass over the pool.
pub fn pool_order(payload: &[u8]) -> Result<PoolOrder, String> {
    Ok(PoolOrder {
        packed: is_packed(payload)?,
        extensions_adjacent: extensions_are_adjacent(payload)?,
        keys_sorted: keys_are_sorted(payload)?,
    })
}

/// Is the pool in the compacted, `(param_id, track)`-sorted form the box
/// produces?
///
/// Every allocated lane below every free one, each extension immediately after
/// the lane it extends, and the allocated lanes non-decreasing by
/// `(param_id, track)`. **Measured, not required**: that the box produces this
/// is established, that it demands it is not — which is why this is a predicate
/// a caller can check rather than an invariant anything here enforces. It is the
/// second of the two things a gen-1 pool writer is waiting on.
///
/// The conjunction of [`pool_order`]'s three, and kept as its own name because
/// "is this what the box would have written" is the question most callers have.
/// A caller *designing a writer* wants the three separately.
pub fn is_compacted(payload: &[u8]) -> Result<bool, String> {
    Ok(pool_order(payload)?.is_canonical())
}

// --- Writing -----------------------------------------------------------------

/// The largest word a step can hold, because **both** bytes have a sentinel.
///
/// A coarse byte of [`NO_VALUE`] reads back as a step with no lock, so a value
/// whose display byte is `0xFF` would erase itself — the gen-1 form of the one
/// clamp [`crate::plocks::VALUE_MAX`] keeps. The fine byte needs the same guard
/// for a subtler reason: the box writes [`NO_VALUE`] into an extension at every
/// step its parent does not lock (measured 2026-09-01, variant C), so a fine
/// byte of `0xFF` on a locked step is indistinguishable from "this step has no
/// fine part" and [`A4Lane::word`] would read it back as zero. Clamping costs
/// 1/256 of a display unit; not clamping costs the whole fine byte, silently.
pub const VALUE_MAX: u16 = 0xFEFE;

/// One lane a caller wants this track to end up with.
///
/// The gen-1 twin of [`crate::plocks::LaneWrite`], and narrower in the same way:
/// a caller says *which parameter* and *what values*, never which of the 128
/// lanes. On this box it could not say even if it wanted to — the pool is
/// rebuilt in `(param_id, track)` order on every write, so a lane index is an
/// output of the encoder rather than an input to it.
///
/// **`param_id` is the box's own byte, and that is the whole identity.** Unlike
/// gen-2, nothing here resolves a lane through [`crate::params`] first. Three A4
/// parameter ids are named (`0x22` FLTR1 FREQ, `0x23` RESO, `0x24` OVERDRIVE)
/// out of an unknown total, so a curated-table lookup would drop nearly every
/// lane a box handed us — and the policy below frees what it is not asked to
/// keep, which would turn "we cannot name it" into "we deleted it". A lane read
/// off the box goes back on the strength of its id alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A4LaneWrite {
    pub param_id: u8,
    /// Stored 16-bit words — the displayed value in the high byte, 256ths in
    /// the low. The same quantity gen-2 keeps inline in a `u16be`, and what
    /// [`A4Lane::word`] reports. `None` is a step with no lock; a short array
    /// leaves the remaining steps unlocked.
    pub values: Vec<Option<u16>>,
}

impl A4LaneWrite {
    pub fn new(param_id: u8, values: Vec<Option<u16>>) -> Self {
        Self { param_id, values }
    }

    fn is_empty(&self) -> bool {
        !self.values.iter().any(Option::is_some)
    }
}

/// A lane read off a payload, asked for again as-is — what a caller does when it
/// is rewriting a track it just read, and what keeps another track's lanes
/// byte-exact through a rebuild.
impl From<&A4Lane> for A4LaneWrite {
    fn from(l: &A4Lane) -> Self {
        Self {
            param_id: l.param_id,
            values: (0..NUM_STEPS).map(|step| l.word(step)).collect(),
        }
    }
}

/// One lane's bytes, plus its extension's when it needs one.
///
/// The unit the pool is laid out from: a lane and its `80 80` are adjacent by
/// construction here, because that is what the box does with them and — since
/// the box re-derives an extension's steps from its parent's (variant C) —
/// what it will normalise them to anyway.
struct Encoded {
    key: (u8, u8),
    lane: [u8; LANE_SIZE],
    ext: Option<[u8; LANE_SIZE]>,
}

impl Encoded {
    fn lanes(&self) -> usize {
        1 + usize::from(self.ext.is_some())
    }
}

fn encode_lane(param_id: u8, track: u8, values: &[Option<u16>]) -> Encoded {
    let mut lane = [NO_VALUE; LANE_SIZE];
    let mut ext = [NO_VALUE; LANE_SIZE];
    lane[0] = param_id;
    lane[1] = track;
    ext[0] = EXT;
    ext[1] = EXT;

    let mut any_fine = false;
    for step in 0..NUM_STEPS {
        let Some(word) = values.get(step).copied().flatten() else { continue };
        let word = word.min(VALUE_MAX);
        lane[2 + step] = (word >> 8) as u8;
        let fine = (word & 0xFF) as u8;
        // A fine byte is written at exactly the steps the parent locks, and
        // `NO_VALUE` everywhere else — the box's own alignment, measured when it
        // re-derived a detached extension from its parent on 2026-09-01.
        ext[2 + step] = fine;
        any_fine |= fine != 0;
    }

    Encoded {
        key: (param_id, track),
        lane,
        // The rule open item 1 closed on 2026-08-31: an extension exists iff
        // some fine byte is non-zero. The box stores an all-zero one when handed
        // it and never allocates one itself, so emitting one would be a lane
        // spent to say nothing.
        ext: any_fine.then_some(ext),
    }
}

/// Reset one lane to the form the box leaves an unallocated one in: `FF FF` and
/// 64 **zero** bytes.
///
/// Not [`NO_VALUE`] in the values, which is what this format's two opposite
/// fills would lead anyone to guess. Measured 2026-09-01 by watching the box
/// clear A16 — see the module doc.
fn free_lane(payload: &mut [u8], lane: usize) {
    let o = lane_start(lane);
    payload[o] = FREE;
    payload[o + 1] = FREE;
    payload[o + 2..o + LANE_SIZE].fill(0);
}

/// Write one track's p-lock lanes into a payload, in place.
///
/// Returns the warnings — written to be shown to the user verbatim, and the only
/// way this reports trouble. **A full pool is not an error**: a write that
/// cannot fit every lane should still land the notes, loudly. The `Err` is
/// reserved for a track this pattern does not have or a payload too short to
/// hold a pool, both of which are caller bugs.
///
/// # Why this rebuilds the whole pool, where gen-2 edits lanes in place
///
/// [`crate::plocks::apply_track_plocks`] keeps every lane where the box put it,
/// on the grounds that the safest write moves the fewest bytes. That policy
/// rests on gen-2 not compacting, and it does not port.
///
/// **Measured on hardware 2026-09-01, three single-variable writes to A16.** The
/// box requires *none* of the three properties [`pool_order`] reports: handed a
/// pool with its keys out of order, or with a hole in it, or with an extension
/// detached from its parent, it parsed every lane, lost no lock, and wrote back
/// its own canonical form. The sorted-compacted pool is a serialisation
/// artefact of a box that holds p-locks keyed by `(param_id, track)` and rebuilds
/// them on ingest — not a structure it reads in place.
///
/// **So the box does not require canonical order, and this encoder emits it
/// anyway, because the *verify* requires it.** A write is checked by reading the
/// slot back and comparing byte for byte
/// ([`crate::safe_write::a4_safe_write_tracks`]). The box normalises what it is
/// sent, so a pool written in any other order comes back *different from what
/// was sent* and a correct write reports as a failed one — 10 spurious diffs for
/// a swapped pair, 132 for a single hole. Emitting the box's own form is what
/// keeps the read-back check meaningful.
///
/// Rebuilding means **lanes belonging to other tracks change index**, which is
/// the one thing gen-2's policy exists to avoid. Their *contents* are carried
/// through byte-exact and that is the property to hold onto: on this box a lane
/// index is not a place a lane lives, it is an artefact of the order, and the
/// box itself moves them on every edit.
///
/// # The policy
///
/// * Lanes belonging to **other tracks are carried through unchanged** — same
///   parameter, same values, same fine bytes — and re-laid-out in key order.
/// * The named track's lanes are **replaced wholesale** by `lanes`. A parameter
///   the caller no longer asks for is gone, which is the same scrub-before-write
///   [`crate::plocks::apply_track_plocks`] does and for the same reason: a step
///   that lost its trig must not leave a lock behind for the next trig to
///   inherit.
/// * A lane with no values is not allocated: it would claim one of 128 to say
///   nothing.
/// * One lane per parameter per track. A repeated `param_id` is warned about and
///   the first wins.
/// * An extension is emitted **iff some fine byte is non-zero**, and carries a
///   fine byte at exactly the steps its parent locks.
/// * **When the pool is full, the caller's lanes are what get dropped** — never
///   another track's. A budget failure must not become someone else's data loss.
///
/// **Nothing here consults [`crate::params`].** See [`A4LaneWrite`] for why: the
/// param id is the identity, and a table lookup would silently free every lane
/// whose knob this app cannot name, which on this box is nearly all of them.
pub fn apply_track_plocks(
    payload: &mut [u8],
    track_index: usize,
    lanes: &[A4LaneWrite],
) -> Result<Vec<String>, String> {
    if track_index >= NUM_TRACKS {
        return Err(format!("no track {track_index}; an A4 pattern has {NUM_TRACKS}"));
    }
    check_payload(payload)?;
    let mut warnings = Vec::new();

    // Other tracks first, and they are not negotiable: read before anything is
    // overwritten, carried through byte-exact, and laid out ahead of the
    // caller's lanes when the pool runs short.
    let keep: Vec<Encoded> = read_all_plocks(payload)?
        .iter()
        .filter(|l| usize::from(l.track) != track_index)
        .map(|l| {
            let values: Vec<Option<u16>> = (0..NUM_STEPS).map(|step| l.word(step)).collect();
            encode_lane(l.param_id, l.track, &values)
        })
        .collect();

    // What the caller wants this track to end up with, in the order asked for.
    let mut wanted: Vec<Encoded> = Vec::new();
    for lane in lanes {
        if lane.param_id == FREE || lane.param_id == EXT || lane.is_empty() {
            if lane.param_id == FREE || lane.param_id == EXT {
                warnings.push(format!(
                    "p-lock parameter {:#04x} is a marker byte in this format, not a parameter \
                     — that lane was not written",
                    lane.param_id
                ));
            }
            continue;
        }
        if wanted.iter().any(|e| e.key.0 == lane.param_id) {
            warnings.push(format!(
                "p-lock parameter {} appears twice for track {} — the box holds one lane per \
                 parameter per track, so only the first was written",
                lane.param_id,
                track_index + 1
            ));
            continue;
        }
        wanted.push(encode_lane(lane.param_id, track_index as u8, &lane.values));
    }

    // **A track asking for nothing over a destination that has something.**
    // Not an ordinary deletion: deleting the last lane in the roll arrives the
    // same way, but so does a project file written before the import carried the
    // pool at all — and in that second case the lanes being freed are ones the
    // user was never shown. The two are indistinguishable from here, so the
    // narrow shape is reported and the caller's instruction is still obeyed.
    if wanted.is_empty() {
        let had = read_all_plocks(payload)?
            .iter()
            .filter(|l| usize::from(l.track) == track_index)
            .count();
        if had > 0 {
            warnings.push(format!(
                "track {} had {had} p-lock lane{} on the box and this write asks for none, so \
                 {} removed — if you did not delete them here, the pattern was read in before \
                 digi-roll carried p-locks and the box's own are the ones to keep",
                track_index + 1,
                if had == 1 { "" } else { "s" },
                if had == 1 { "it was" } else { "they were" },
            ));
        }
    }

    // The budget, in *lanes* rather than locks: a lane needing a fine byte costs
    // two. Other tracks are seated first, so an overflow lands on the caller.
    let mut used: usize = keep.iter().map(Encoded::lanes).sum();
    let mut dropped: Vec<u8> = Vec::new();
    let mut seated: Vec<Encoded> = Vec::new();
    for lane in wanted {
        let cost = lane.lanes();
        if used + cost > NUM_LANES {
            dropped.push(lane.key.0);
            continue;
        }
        used += cost;
        seated.push(lane);
    }
    if !dropped.is_empty() {
        let n = dropped.len();
        let list = dropped.iter().map(|id| format!("{id:#04x}")).collect::<Vec<_>>().join(", ");
        warnings.push(format!(
            "the pattern's {NUM_LANES} p-lock lanes are all in use, so {n} lane{} \
             (parameter{} {list}) {} not written — free some p-locks on the box first",
            if n == 1 { "" } else { "s" },
            if n == 1 { "" } else { "s" },
            if n == 1 { "was" } else { "were" },
        ));
    }

    // The box's own form: sorted by (param_id, track), packed from lane zero,
    // each extension immediately after the lane it extends.
    let mut all: Vec<Encoded> = keep;
    all.extend(seated);
    all.sort_by_key(|e| e.key);

    let mut lane = 0usize;
    for e in &all {
        let o = lane_start(lane);
        payload[o..o + LANE_SIZE].copy_from_slice(&e.lane);
        lane += 1;
        if let Some(ext) = &e.ext {
            let o = lane_start(lane);
            payload[o..o + LANE_SIZE].copy_from_slice(ext);
            lane += 1;
        }
    }
    for empty in lane..NUM_LANES {
        free_lane(payload, empty);
    }

    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleared() -> Vec<u8> {
        let mut p = vec![0u8; PAYLOAD_LEN];
        for lane in 0..NUM_LANES {
            let o = lane_start(lane);
            p[o] = FREE;
            p[o + 1] = FREE;
        }
        p
    }

    #[test]
    fn the_geometry_fills_the_region_exactly() {
        assert_eq!(POOL_BASE, 4_510);
        assert_eq!(NUM_LANES * LANE_SIZE, 8_448);
        assert_eq!(POOL_END, 12_958);
    }

    #[test]
    fn a_cleared_pool_reports_nothing_allocated() {
        let p = cleared();
        assert!(read_all_plocks(&p).unwrap().is_empty());
        assert_eq!(free_lane_count(&p).unwrap(), NUM_LANES);
        assert!(is_compacted(&p).unwrap());
    }

    /// The bug the two opposite fills caused once already: a lane whose every
    /// step is locked to zero is a real lane, and a `v != 0` test loses it.
    #[test]
    fn a_zero_valued_lock_is_a_lock() {
        let mut p = cleared();
        let o = lane_start(0);
        p[o] = 0x22;
        p[o + 1] = 0x00;
        p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
        p[o + 2] = 0x00; // step 1 locked to zero
        let lanes = read_all_plocks(&p).unwrap();
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].values[0], Some(0));
        assert_eq!(lanes[0].values[1], None);
        assert_eq!(lanes[0].word(0), Some(0));
    }

    #[test]
    fn an_extension_is_folded_in_not_reported_as_a_lane() {
        let mut p = cleared();
        for (lane, hdr) in [(0usize, (0x22u8, 0x00u8)), (1, (EXT, EXT))] {
            let o = lane_start(lane);
            p[o] = hdr.0;
            p[o + 1] = hdr.1;
            p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
        }
        p[lane_start(0) + 2] = 0x40;
        p[lane_start(1) + 2] = 0x17;

        let lanes = read_all_plocks(&p).unwrap();
        assert_eq!(lanes.len(), 1, "the 80 80 lane is half a value, not a lane");
        assert_eq!(lanes[0].ext_lane, Some(1));
        assert_eq!(lanes[0].values[0], Some(0x40));
        assert_eq!(lanes[0].fine.as_ref().unwrap()[0], Some(0x17));
        // The gen-2 units: display × 256.
        assert_eq!(lanes[0].word(0), Some(0x4017));
        assert_eq!(orphan_extension_count(&p).unwrap(), 0);
    }

    #[test]
    fn a_lane_with_no_extension_reads_its_fine_byte_as_zero() {
        let mut p = cleared();
        let o = lane_start(0);
        p[o] = 0x23;
        p[o + 1] = 0x00;
        p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
        p[o + 2] = 0x64;
        let lanes = read_all_plocks(&p).unwrap();
        assert!(lanes[0].fine.is_none());
        assert_eq!(lanes[0].word(0), Some(0x6400));
    }

    #[test]
    fn a_leading_extension_is_an_orphan() {
        let mut p = cleared();
        let o = lane_start(0);
        p[o] = EXT;
        p[o + 1] = EXT;
        p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
        assert!(read_all_plocks(&p).unwrap().is_empty());
        assert_eq!(orphan_extension_count(&p).unwrap(), 1);
        assert!(!is_compacted(&p).unwrap());
    }

    /// [`is_free`] checks the 64 value bytes and not just the header, and this
    /// is the shape that says so: `FF FF` over values that are not zero.
    ///
    /// No capture has shown one. Reporting it as free would **hide** it, and the
    /// two things this module is most likely to be wrong about — the extension
    /// rule and whether the box compacts — would both show up first as a lane
    /// like this. A predicate that cannot see a malformed lane cannot report
    /// one.
    #[test]
    fn a_free_header_over_values_that_are_not_zero_is_not_a_free_lane() {
        let mut p = cleared();
        let o = lane_start(3);
        p[o + 2 + 9] = 0x40; // header still FF FF

        assert_eq!(free_lane_count(&p).unwrap(), NUM_LANES - 1);
        let lanes = read_all_plocks(&p).unwrap();
        assert_eq!(lanes.len(), 1, "the malformed lane is reported, not skipped");
        assert_eq!((lanes[0].param_id, lanes[0].track), (FREE, FREE));
        // And it is not the compacted form the box produces, because it sits
        // below 125 genuinely free lanes.
        assert!(!is_compacted(&p).unwrap());
    }

    /// The ordering half of [`is_compacted`], which the hole test below cannot
    /// see: lanes packed from zero with no gaps, but keyed out of order.
    ///
    /// The box has never produced this. It is the predicate's whole purpose to
    /// notice if it ever does — that gen-1 *produces* a
    /// `(param_id, track)`-sorted pool is the measured half of why there is no
    /// writer here yet.
    #[test]
    fn lanes_packed_but_out_of_key_order_are_not_compacted() {
        let mut p = cleared();
        for (lane, hdr) in [(0usize, (0x23u8, 0x00u8)), (1, (0x22, 0x00))] {
            let o = lane_start(lane);
            p[o] = hdr.0;
            p[o + 1] = hdr.1;
            p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
            p[o + 2] = 0x40;
        }
        assert_eq!(read_all_plocks(&p).unwrap().len(), 2, "no holes, both lanes read");
        assert!(!is_compacted(&p).unwrap(), "0x23 before 0x22 is not the box's order");

        // The same two lanes the other way round are.
        let mut q = cleared();
        for (lane, hdr) in [(0usize, (0x22u8, 0x00u8)), (1, (0x23, 0x00))] {
            let o = lane_start(lane);
            q[o] = hdr.0;
            q[o + 1] = hdr.1;
            q[o + 2..o + LANE_SIZE].fill(NO_VALUE);
            q[o + 2] = 0x40;
        }
        assert!(is_compacted(&q).unwrap());
    }

    /// The same parameter on two tracks orders by the track byte, which is the
    /// second half of the key and the one the `PLUS_SYN2` fixture exercises.
    #[test]
    fn the_track_byte_is_the_second_half_of_the_sort_key() {
        let mut p = cleared();
        for (lane, hdr) in [(0usize, (0x22u8, 0x01u8)), (1, (0x22, 0x00))] {
            let o = lane_start(lane);
            p[o] = hdr.0;
            p[o + 1] = hdr.1;
            p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
            p[o + 2] = 0x40;
        }
        assert!(!is_compacted(&p).unwrap(), "SYN2 before SYN1 on one parameter");
    }

    #[test]
    fn a_hole_below_an_allocated_lane_is_not_compacted() {
        let mut p = cleared();
        let o = lane_start(1); // lane 0 left free
        p[o] = 0x22;
        p[o + 1] = 0x00;
        p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
        assert!(!is_compacted(&p).unwrap());
    }

    #[test]
    fn trigless_locks_are_visible() {
        let mut p = cleared();
        let o = lane_start(0);
        p[o] = 0x22;
        p[o + 1] = 0x00;
        p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
        p[o + 2 + 5] = 0x40; // step 6, zero-based 5
        let lane = &read_all_plocks(&p).unwrap()[0];
        assert!(lane.has_trigless_values(&[0, 1]));
        assert!(!lane.has_trigless_values(&[5]));
    }

    #[test]
    fn a_short_payload_is_refused_rather_than_indexed() {
        assert!(read_all_plocks(&[0u8; 100]).is_err());
        assert!(free_lane_count(&[0u8; 100]).is_err());
        assert!(is_compacted(&[0u8; 100]).is_err());
    }

    #[test]
    fn there_is_no_track_six() {
        let p = cleared();
        assert!(read_track_plocks(&p, 5).is_ok());
        assert!(read_track_plocks(&p, 6).is_err());
    }

    // --- the write half ------------------------------------------------------

    /// Put a lane on a payload the way the box would, so a test can start from
    /// a pool that is already the box's own form.
    fn place(p: &mut [u8], lane: usize, param_id: u8, track: u8, at: &[(usize, u8, Option<u8>)]) {
        let o = lane_start(lane);
        p[o] = param_id;
        p[o + 1] = track;
        p[o + 2..o + LANE_SIZE].fill(NO_VALUE);
        if at.iter().any(|(_, _, f)| f.is_some_and(|f| f != 0)) {
            let e = lane_start(lane + 1);
            p[e] = EXT;
            p[e + 1] = EXT;
            p[e + 2..e + LANE_SIZE].fill(NO_VALUE);
        }
        for &(step, coarse, fine) in at {
            p[o + 2 + step] = coarse;
            if let Some(f) = fine {
                p[lane_start(lane + 1) + 2 + step] = f;
            }
        }
    }

    fn words(at: &[(usize, u16)]) -> Vec<Option<u16>> {
        let mut v = vec![None; NUM_STEPS];
        for &(step, w) in at {
            v[step] = Some(w);
        }
        v
    }

    /// The property every other write test leans on, and the reason this
    /// encoder rebuilds rather than edits: the box normalises what it is sent,
    /// so anything but its own form makes a correct write read back as a failure.
    #[test]
    fn a_written_pool_is_in_the_box_s_own_form() {
        let mut p = cleared();
        // Deliberately asked for out of key order, and with the higher id first.
        let lanes = [
            A4LaneWrite::new(0x40, words(&[(0, 0x2000)])),
            A4LaneWrite::new(0x22, words(&[(0, 0x3200), (4, 0x6440)])),
        ];
        assert!(apply_track_plocks(&mut p, 0, &lanes).unwrap().is_empty());

        let order = pool_order(&p).unwrap();
        assert!(order.is_canonical(), "{order:?}");
        let read = read_all_plocks(&p).unwrap();
        assert_eq!(read.iter().map(|l| l.param_id).collect::<Vec<_>>(), [0x22, 0x40]);
    }

    /// A lane read off a payload and asked for again comes back byte-identical
    /// — the round trip the same-slot write-back depends on.
    #[test]
    fn a_lane_written_back_unchanged_moves_no_bytes() {
        let mut p = cleared();
        place(&mut p, 0, 0x22, 0, &[(0, 0x32, Some(0x3b)), (4, 0x64, Some(0))]);
        place(&mut p, 2, 0x23, 0, &[(4, 0x64, None)]);
        let before = p.clone();

        let lanes: Vec<A4LaneWrite> =
            read_all_plocks(&p).unwrap().iter().map(A4LaneWrite::from).collect();
        apply_track_plocks(&mut p, 0, &lanes).unwrap();

        assert_eq!(p, before, "a write-back of what was read moved bytes");
    }

    /// **The containment property.** A one-track write rebuilds the whole pool,
    /// so another track's lanes change *index* — and their contents must not
    /// change at all. This is the gen-1 replacement for gen-2's "lanes belonging
    /// to other tracks are never read, moved or written", which cannot hold on a
    /// box that sorts.
    #[test]
    fn another_track_s_lanes_survive_a_rebuild_that_moves_them() {
        let mut p = cleared();
        // SYN2's lane sorts *after* SYN1's 0x22 but *before* a new 0x40, so
        // adding to SYN1 must shift it — index changes, content does not.
        place(&mut p, 0, 0x22, 0, &[(0, 0x32, Some(0x3b))]);
        place(&mut p, 2, 0x24, 1, &[(8, 0x7f, None)]);

        let syn2_before = read_track_plocks(&p, 1).unwrap();
        assert_eq!(syn2_before[0].lane, 2);

        // 0x23 sorts *between* SYN1's 0x22 and SYN2's 0x24, so seating it
        // pushes SYN2's lane down. A parameter above 0x24 would leave it where
        // it is and the test would pass without exercising anything.
        let lanes = [
            A4LaneWrite::new(0x22, words(&[(0, 0x323b)])),
            A4LaneWrite::new(0x23, words(&[(1, 0x1000)])),
        ];
        apply_track_plocks(&mut p, 0, &lanes).unwrap();

        let syn2_after = read_track_plocks(&p, 1).unwrap();
        assert_eq!(syn2_after.len(), 1, "SYN2's lane survived");
        assert_ne!(syn2_after[0].lane, syn2_before[0].lane, "and it moved, as it must");
        assert_eq!(syn2_after[0].param_id, 0x24);
        assert_eq!(syn2_after[0].values, syn2_before[0].values);
        assert_eq!(syn2_after[0].fine, syn2_before[0].fine);
        assert!(pool_order(&p).unwrap().is_canonical());
    }

    /// A parameter the caller stops asking for is gone, and the lane it left
    /// behind is the box's own free form — `FF FF` and 64 **zero** bytes, not
    /// 64 `NO_VALUE`. Measured 2026-09-01; the opposite of what this format's
    /// two opposite fills suggest.
    #[test]
    fn a_dropped_lane_is_freed_the_way_the_box_frees_one() {
        let mut p = cleared();
        place(&mut p, 0, 0x22, 0, &[(0, 0x32, None)]);
        place(&mut p, 1, 0x23, 0, &[(4, 0x64, None)]);

        apply_track_plocks(&mut p, 0, &[A4LaneWrite::new(0x22, words(&[(0, 0x3200)]))]).unwrap();

        assert_eq!(read_all_plocks(&p).unwrap().len(), 1);
        assert_eq!(free_lane_count(&p).unwrap(), NUM_LANES - 1);
        let o = lane_start(1);
        assert_eq!(p[o], FREE);
        assert_eq!(p[o + 1], FREE);
        assert!(p[o + 2..o + LANE_SIZE].iter().all(|&v| v == 0), "freed values must be zero");
    }

    #[test]
    fn an_extension_is_emitted_only_when_some_fine_byte_is_non_zero() {
        let mut integral = cleared();
        apply_track_plocks(&mut integral, 0, &[A4LaneWrite::new(0x23, words(&[(4, 0x6400)]))])
            .unwrap();
        assert!(read_all_plocks(&integral).unwrap()[0].fine.is_none());
        assert_eq!(free_lane_count(&integral).unwrap(), NUM_LANES - 1, "one lane, not two");

        let mut fractional = cleared();
        apply_track_plocks(&mut fractional, 0, &[A4LaneWrite::new(0x22, words(&[(0, 0x323b)]))])
            .unwrap();
        let lane = &read_all_plocks(&fractional).unwrap()[0];
        assert_eq!(lane.fine.as_ref().unwrap()[0], Some(0x3b));
        assert_eq!(lane.ext_lane, Some(1));
        assert_eq!(free_lane_count(&fractional).unwrap(), NUM_LANES - 2);
    }

    /// An extension carries a fine byte at exactly its parent's locked steps and
    /// `NO_VALUE` elsewhere — the alignment the box re-derived for itself when
    /// handed a detached extension on 2026-09-01.
    #[test]
    fn an_extension_holds_fine_bytes_only_where_its_parent_locks() {
        let mut p = cleared();
        apply_track_plocks(
            &mut p,
            0,
            &[A4LaneWrite::new(0x22, words(&[(0, 0x323b), (8, 0x6400)]))],
        )
        .unwrap();
        let fine = read_all_plocks(&p).unwrap()[0].fine.clone().unwrap();
        assert_eq!(fine[0], Some(0x3b));
        assert_eq!(fine[8], Some(0), "a locked step with no fine part is zero, not absent");
        assert_eq!(fine[1], None, "an unlocked step carries NO_VALUE");
        assert_eq!(fine.iter().filter(|f| f.is_some()).count(), 2);
    }

    /// Both bytes have a sentinel, so both need the clamp. A coarse `0xFF`
    /// would read back as an unlocked step and a fine `0xFF` as no fine part —
    /// two ways for an unclamped write to lose data with no error anywhere.
    #[test]
    fn neither_sentinel_can_be_written_as_a_value() {
        let mut p = cleared();
        apply_track_plocks(&mut p, 0, &[A4LaneWrite::new(0x22, words(&[(0, 0xFFFF)]))]).unwrap();
        let lane = &read_all_plocks(&p).unwrap()[0];
        assert_eq!(lane.values[0], Some(0xFE), "the lock survives as a lock");
        assert_eq!(lane.word(0), Some(VALUE_MAX));
        assert_eq!(lane.fine.as_ref().unwrap()[0], Some(0xFE));
    }

    #[test]
    fn a_lane_with_no_values_claims_nothing() {
        let mut p = cleared();
        apply_track_plocks(&mut p, 0, &[A4LaneWrite::new(0x22, vec![None; NUM_STEPS])]).unwrap();
        assert!(read_all_plocks(&p).unwrap().is_empty());
        assert_eq!(free_lane_count(&p).unwrap(), NUM_LANES);
    }

    #[test]
    fn one_parameter_twice_writes_the_first_and_says_so() {
        let mut p = cleared();
        let warnings = apply_track_plocks(
            &mut p,
            0,
            &[
                A4LaneWrite::new(0x22, words(&[(0, 0x3200)])),
                A4LaneWrite::new(0x22, words(&[(4, 0x6400)])),
            ],
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("appears twice"), "{}", warnings[0]);
        let lanes = read_all_plocks(&p).unwrap();
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].values[0], Some(0x32));
        assert_eq!(lanes[0].values[4], None, "the second lane did not merge in");
    }

    /// A marker byte is not a parameter. `FF` would author a lane that reads as
    /// free and `80` one that reads as somebody else's extension — both of which
    /// corrupt the pool rather than filling it.
    #[test]
    fn a_marker_byte_is_refused_as_a_parameter_id() {
        let mut p = cleared();
        let warnings = apply_track_plocks(
            &mut p,
            0,
            &[
                A4LaneWrite::new(FREE, words(&[(0, 0x3200)])),
                A4LaneWrite::new(EXT, words(&[(0, 0x3200)])),
            ],
        )
        .unwrap();
        assert_eq!(warnings.len(), 2);
        assert!(read_all_plocks(&p).unwrap().is_empty());
    }

    /// **When the pool is full it is the caller's lanes that go.** Another
    /// track's data must not be the thing a budget failure spends.
    #[test]
    fn a_full_pool_drops_the_caller_s_lanes_and_never_another_track_s() {
        let mut p = cleared();
        // 126 lanes belonging to SYN2, leaving room for one more lane only.
        for i in 0..126usize {
            place(&mut p, i, i as u8, 1, &[(0, 0x40, None)]);
        }
        assert_eq!(free_lane_count(&p).unwrap(), 2);

        let warnings = apply_track_plocks(
            &mut p,
            0,
            &[
                // Two lanes' worth: this one needs an extension.
                A4LaneWrite::new(0xC8, words(&[(0, 0x323b)])),
                A4LaneWrite::new(0xC9, words(&[(0, 0x3200)])),
                A4LaneWrite::new(0xCA, words(&[(0, 0x3200)])),
            ],
        )
        .unwrap();

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("all in use"), "{}", warnings[0]);
        assert_eq!(read_track_plocks(&p, 1).unwrap().len(), 126, "SYN2 kept every lane");
        // The 0xC8 pair fit; 0xC9 and 0xCA did not.
        let syn1 = read_track_plocks(&p, 0).unwrap();
        assert_eq!(syn1.len(), 1);
        assert_eq!(syn1[0].param_id, 0xC8);
        assert!(pool_order(&p).unwrap().is_canonical());
    }

    #[test]
    fn a_write_to_a_track_this_box_does_not_have_is_refused() {
        let mut p = cleared();
        assert!(apply_track_plocks(&mut p, 6, &[]).is_err());
        assert!(apply_track_plocks(&mut [0u8; 100], 0, &[]).is_err());
    }
}
