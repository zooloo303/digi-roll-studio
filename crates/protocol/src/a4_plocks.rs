//! The Analog Four's **gen-1** p-lock pool: 128 lanes of 66 bytes, read off a
//! pattern payload.
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
//!   built on that. **A gen-1 lane index does not survive an edit on the box.**
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
//! [`crate::plocks`] records gen-2 storing display × 256 in a `u16be`, so **both
//! generations store the same 16-bit quantity** — gen-2 inline, gen-1 split
//! across a lane and its extension, because a gen-1 lane has 64 value bytes
//! where gen-2's has 128. `0x8080` is a continuation marker rather than a
//! parameter id, which is why it never looked like a plausible one.
//!
//! **The fractional reading is inference, and [`A4Lane::word`] is where it
//! lives.** That the coarse byte equals the display is measured; that the fine
//! byte is 256ths of a display unit is imported from gen-2.
//!
//! # There is no write path here, deliberately
//!
//! [`apply_track_plocks`](crate::plocks::apply_track_plocks) has a gen-1
//! counterpart only when two things are known. **The first was answered on
//! 2026-08-31 and the second was not**, so there is still no writer:
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
//! 2. **Whether the box requires the compacted, `(param_id, track)`-sorted
//!    order it produces.** That it produces that order is measured. That it
//!    *needs* it is not, and a pool written in some other order is a guess
//!    delivered to hardware.
//!
//! [`crate::a4_pattern::build_pattern`] refuses to encode a ragged tail it
//! cannot measure rather than pick one of two orders. This is the same refusal
//! one level up: the reader is complete, and the writer waits for the capture.
//!
//! **A third fact the writer will want, caught free on 2026-09-01.** The lane
//! probe was watching when A16 was cleared from the front panel, so the box's
//! own way of *freeing* a lane is on record: it writes [`FREE`] into both id
//! bytes and **zeroes all 64 value bytes**, rather than filling them with
//! [`NO_VALUE`]. An extension lane between two used lanes (`80 80`) is freed the
//! same way. So a writer removing a lock should zero the values, which is not
//! what the two opposite fills elsewhere in this format would lead anyone to
//! guess — and an untouched free lane in a cleared pattern reads the same way,
//! so the two are indistinguishable afterwards, which is presumably the point.
//! The capture is `local/a4-check/lanes/a4-working-change002.syx` against
//! `change001`.

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
    /// The stored 16-bit value at a zero-based step, in the same units
    /// [`crate::plocks`] reports for gen-2: display × 256.
    ///
    /// **Half measured, half inferred.** The coarse byte being the displayed
    /// value is measured on this box. That the fine byte is 256ths of a display
    /// unit is imported from the gen-2 scaling in [`crate::params`] — it is
    /// consistent with every capture and it is not independently established
    /// here, which is why the raw bytes stay reachable on
    /// [`values`](A4Lane::values) and [`fine`](A4Lane::fine).
    ///
    /// A lane with no extension reads its fine byte as zero, which is what an
    /// absent extension has always meant so far and is the other half of open
    /// item 3.
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

/// Is the pool in the compacted, `(param_id, track)`-sorted form the box
/// produces?
///
/// Every allocated lane below every free one, each extension immediately after
/// the lane it extends, and the allocated lanes non-decreasing by
/// `(param_id, track)`. **Measured, not required**: that the box produces this
/// is established, that it demands it is not — which is why this is a predicate
/// a caller can check rather than an invariant anything here enforces. It is the
/// second of the two things a gen-1 pool writer is waiting on.
pub fn is_compacted(payload: &[u8]) -> Result<bool, String> {
    check_payload(payload)?;
    let mut seen_free = false;
    let mut last_key: Option<(u8, u8)> = None;
    for lane in 0..NUM_LANES {
        if is_free(payload, lane) {
            seen_free = true;
            continue;
        }
        if seen_free {
            return Ok(false);
        }
        if is_extension(payload, lane) {
            // An extension must follow a real lane, and `read_all_plocks`
            // already treats a leading one as an orphan.
            if lane == 0 || is_extension(payload, lane - 1) {
                return Ok(false);
            }
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
}
