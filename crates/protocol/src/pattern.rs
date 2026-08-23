//! Shared pattern + kit dump decoding/encoding for the Digitakt II family.
//!
//! Port of `js/elektron/pattern-core.js`. The DT2 and DN2 are sibling boxes on
//! the same OS generation and their pattern structs are near-identical: same
//! regions in the same order, differing only in a handful of sizes and offsets.
//! Each device supplies a [`Spec`] of those numbers.
//!
//! Struct knowledge comes from elk-herd's `Elektron/Digitakt/{Dump,CppStructs}.elm`
//! (BSD-2-Clause, © mzero) for the DT2 pattern/track/kit skeleton, plus digi-roll's
//! own reverse engineering: the note-trig record pool (hardware-verified) and the
//! entire DN2 mapping.

// **Clippy is off for three cosmetic lints in this file, per PLAN.md §7 rule 3.**
// A redundant cast, a `.get(0)`, and an indexed loop over a byte range: all three
// suggestions are behaviour-identical and all three are refused for the reason
// `sevenbit` refuses its own three. This is a byte-for-byte port pinned against
// hardware captures, and keeping its shape is what makes it diffable against the
// JS when a capture disagrees. Rule 3 does not extend past the decode/encode
// internals, so these are named rather than blanket.
#![allow(clippy::unnecessary_cast, clippy::get_first, clippy::needless_range_loop)]

use std::collections::{BTreeMap, HashMap};

pub const TRIG_ENABLED: u16 = 0x0001;
pub const TRIG_SET_HI: u8 = 0x03;
pub const TRIG_SET_LO: u8 = 0x81;

pub fn u16_be(b: &[u8], o: usize) -> u16 {
    ((b[o] as u16) << 8) | b[o + 1] as u16
}
pub fn u32_be(b: &[u8], o: usize) -> u32 {
    ((b[o] as u32) << 24) | ((b[o + 1] as u32) << 16) | ((b[o + 2] as u32) << 8) | b[o + 3] as u32
}
pub fn chars16(bytes: &[u8], offset: usize) -> String {
    let mut end = offset;
    let limit = (offset + 16).min(bytes.len());
    while end < limit && bytes[end] != 0 {
        end += 1;
    }
    String::from_utf8_lossy(&bytes[offset..end]).into_owned()
}
pub fn length_byte_to_steps(v: u8) -> f64 {
    if v >= 127 {
        return f64::INFINITY;
    }
    if v < 14 {
        return 0.125 + v as f64 * 0.0625;
    }
    let octave = ((v as i32 - 14) / 16) as i32;
    let base = 2f64.powi(octave);
    base + (v as f64 - 14.0 - octave as f64 * 16.0) * (base / 16.0)
}
pub fn steps_to_length_byte(steps: f64) -> u8 {
    if !steps.is_finite() {
        return 127;
    }
    let mut best = 0u8;
    let mut best_err = f64::INFINITY;
    for v in 0u8..=126 {
        let err = (length_byte_to_steps(v) - steps).abs();
        if err < best_err {
            best_err = err;
            best = v;
        }
    }
    best
}
pub fn micro_byte_to_steps(v: u8) -> f64 {
    v as i8 as f64 / 24.0
}
/// Micro-timing offset (fraction of a step) → the signed byte the box stores.
///
/// Rounds halves toward +∞, which is what JS `Math.round` does. Rust's
/// `f64::round` rounds halves *away from zero*, so the two disagree at exactly
/// −n.5 ticks — the kind of silent one-tick deviation rule 3 exists to prevent.
///
/// **The `.max().min()` chain is not a worse spelling of `clamp`, and clippy is
/// wrong here.** They differ on `NaN`: `f64::max` and `f64::min` return the
/// non-`NaN` operand, so a `NaN` arriving in `micro` comes out of this chain as
/// −23 ticks — a real byte, at the bottom of the range. `f64::clamp` propagates
/// `NaN` instead, and `NaN as i16` saturates to 0, so the same input would
/// silently become *no offset at all* while looking like a deliberate one.
/// Neither is a value anybody wants, but one of them is a trig 23 ticks early
/// and the other is a trig that reads as untouched. `micro` is `f64` arithmetic
/// off a UI drag, and this function is on the write path to a box.
#[allow(clippy::manual_clamp)]
pub fn micro_steps_to_byte(micro: f64) -> u8 {
    let ticks = (micro * 24.0 + 0.5).floor();
    (ticks.max(-23.0).min(23.0) as i16 as i8) as u8
}
pub fn bank_name(index: usize) -> String {
    let a = b'A' + ((index >> 4) % 8) as u8;
    let n = index % 16 + 1;
    format!("{}{:02}", a as char, n)
}

#[derive(Debug, Clone)]
pub struct PatternSpec {
    pub size: usize,
    pub tracks_offset: usize,
    pub num_tracks: usize,
    pub trig_pool: usize,
    pub trig_pool_records: usize,
    pub p_locks_index: usize,
    pub num_p_locks: usize,
    pub p_lock_size: usize,
    pub name_offset: usize,
    pub tempo_offset: usize,
    pub kit_index_offset: usize,
}
#[derive(Debug, Clone)]
pub struct TrackSpec {
    pub size: usize,
    pub num_steps: usize,
    pub steps: usize,
    pub trig_cond: usize,
    pub trig_fill: usize,
    pub trig_prob: usize,
    pub sound_p_locks: usize,
    pub defaults: usize,
    pub length_steps: usize,
    pub track_prob: usize,
}
#[derive(Debug, Clone)]
pub struct TrigSpec {
    pub layout: &'static str,
    pub max_notes: usize,
}
#[derive(Debug, Clone)]
pub struct KitSpec {
    pub size: usize,
    pub sounds_offset: usize,
    pub sound_size: usize,
    pub midi_mask_offset: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct Spec {
    pub device: &'static str,
    pub pattern_versions: Vec<u32>,
    pub pattern: PatternSpec,
    pub track: TrackSpec,
    pub trig: TrigSpec,
    pub kits: HashMap<u32, KitSpec>,
    pub track_kind_fallback: &'static str,
}

#[derive(Debug, Clone)]
pub struct TrigSlot {
    pub note: u8,
    pub velocity: u8,
    pub length: u8,
    pub micro: u8,
}
#[derive(Debug, Clone)]
pub struct TrackData {
    pub steps: Vec<u16>,
    pub sound_p_locks: Vec<u8>,
    pub default_note: u8,
    pub default_velocity: u8,
    pub default_length: u8,
    pub length_steps: u16,
    /// step → the trig's note-slot records. A `BTreeMap`, not a `HashMap`:
    /// callers walk it, and Rust randomises `HashMap` order per process.
    pub trigs: BTreeMap<u8, Vec<TrigSlot>>,
}
#[derive(Debug, Clone)]
pub struct KitInfo {
    pub version: u32,
    pub name: String,
    pub sound_names: Vec<String>,
    pub midi_mask: u16,
}
#[derive(Debug, Clone)]
pub struct PatternKit {
    pub version: u32,
    pub name: String,
    pub tempo_bpm: f64,
    pub kit_index: u8,
    pub tracks: Vec<TrackData>,
    pub kit: KitInfo,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub step: u8,
    pub pitch: u8,
    pub velocity: u8,
    pub len_steps: f64,
    pub micro: f64,
}

pub fn dt2_spec() -> Spec {
    let mut kits = HashMap::new();
    kits.insert(
        3,
        KitSpec {
            size: 10240,
            sounds_offset: 60,
            sound_size: 341,
            midi_mask_offset: Some(9972),
        },
    );
    kits.insert(
        4,
        KitSpec {
            size: 22528,
            sounds_offset: 60,
            sound_size: 1109,
            midi_mask_offset: Some(22260),
        },
    );
    Spec {
        device: "DT2",
        pattern_versions: vec![3, 4],
        pattern: PatternSpec {
            size: 89088,
            tracks_offset: 4,
            num_tracks: 16,
            trig_pool: 18948,
            trig_pool_records: 8192,
            p_locks_index: 68100,
            num_p_locks: 80,
            p_lock_size: 2 + 128 * 2,
            name_offset: 88740,
            tempo_offset: 88756,
            kit_index_offset: 88768,
        },
        track: TrackSpec {
            size: 1184,
            num_steps: 128,
            steps: 0,
            trig_cond: 256,
            trig_fill: 384,
            trig_prob: 512,
            sound_p_locks: 1024,
            defaults: 1152,
            length_steps: 1164,
            track_prob: 1168,
        },
        trig: TrigSpec {
            layout: "quad",
            max_notes: 4,
        },
        kits,
        track_kind_fallback: "sample",
    }
}
pub fn dn2_spec() -> Spec {
    let mut kits = HashMap::new();
    kits.insert(
        3,
        KitSpec {
            size: 10752,
            sounds_offset: 60,
            sound_size: 359,
            midi_mask_offset: None,
        },
    );
    Spec {
        device: "DN2",
        pattern_versions: vec![3],
        pattern: PatternSpec {
            size: 89088,
            tracks_offset: 4,
            num_tracks: 16,
            trig_pool: 18996,
            trig_pool_records: 8192,
            p_locks_index: 68148,
            num_p_locks: 80,
            p_lock_size: 2 + 128 * 2,
            name_offset: 88788,
            tempo_offset: 88804,
            kit_index_offset: 88816,
        },
        track: TrackSpec {
            size: 1187,
            num_steps: 128,
            steps: 0,
            trig_cond: 256,
            trig_fill: 384,
            trig_prob: 512,
            sound_p_locks: 1024,
            defaults: 1152,
            length_steps: 1164,
            track_prob: 1168,
        },
        trig: TrigSpec {
            layout: "perNote",
            max_notes: 4,
        },
        kits,
        track_kind_fallback: "synth",
    }
}

pub fn decode_pattern_kit(spec: &Spec, payload: &[u8]) -> Result<PatternKit, String> {
    let pattern_spec = &spec.pattern;
    let track_spec = &spec.track;
    if payload.len() < pattern_spec.size + 8 {
        return Err(format!(
            "pattern-kit payload too short ({} bytes)",
            payload.len()
        ));
    }
    let version = u32_be(payload, 0);
    if !spec.pattern_versions.contains(&version) {
        return Err(format!(
            "unsupported {} pattern struct version {} — needs a digi-roll update",
            spec.device, version
        ));
    }

    let mut tracks = Vec::with_capacity(pattern_spec.num_tracks);
    for t in 0..pattern_spec.num_tracks {
        let base = pattern_spec.tracks_offset + t * track_spec.size;
        let mut steps = Vec::with_capacity(track_spec.num_steps);
        for s in 0..track_spec.num_steps {
            steps.push(u16_be(payload, base + track_spec.steps + s * 2));
        }
        let sound_p_locks_start = base + track_spec.sound_p_locks;
        let sound_p_locks =
            payload[sound_p_locks_start..sound_p_locks_start + track_spec.num_steps].to_vec();
        let default_note = payload[base + track_spec.defaults];
        let default_velocity = payload[base + track_spec.defaults + 1];
        let default_length = payload[base + track_spec.defaults + 2];
        let length_steps = u16_be(payload, base + track_spec.length_steps);
        tracks.push(TrackData {
            steps,
            sound_p_locks,
            default_note,
            default_velocity,
            default_length,
            length_steps,
            trigs: BTreeMap::new(),
        });
    }

    if spec.trig.layout == "quad" {
        let slots_per = spec.trig.max_notes;
        for r in (0..pattern_spec.trig_pool_records).step_by(slots_per) {
            let o = pattern_spec.trig_pool + r * 6;
            if o + 5 >= payload.len() {
                break;
            }
            let track_idx = payload[o] as usize;
            let step = payload[o + 1] as usize;
            if track_idx >= pattern_spec.num_tracks || step >= track_spec.num_steps {
                continue;
            }
            let mut slots = Vec::with_capacity(slots_per);
            for n in 0..slots_per {
                let s = o + n * 6;
                slots.push(TrigSlot {
                    note: payload[s + 2],
                    velocity: payload[s + 3],
                    length: payload[s + 4],
                    micro: payload[s + 5],
                });
            }
            tracks[track_idx].trigs.insert(step as u8, slots);
        }
    } else {
        for r in 0..pattern_spec.trig_pool_records {
            let o = pattern_spec.trig_pool + r * 6;
            if o + 5 >= payload.len() {
                break;
            }
            let track_idx = payload[o] as usize;
            let step = payload[o + 1] as usize;
            if track_idx >= pattern_spec.num_tracks || step >= track_spec.num_steps {
                continue;
            }
            let slot = TrigSlot {
                note: payload[o + 2],
                velocity: payload[o + 3],
                length: payload[o + 4],
                micro: payload[o + 5],
            };
            tracks[track_idx]
                .trigs
                .entry(step as u8)
                .or_default()
                .push(slot);
        }
    }

    let kit_base = pattern_spec.size;
    if u32_be(payload, kit_base) != 0xBEEFBACE {
        return Err("kit magic 0xBEEFBACE not found where expected — struct drift?".to_string());
    }
    let kit_version = u32_be(payload, kit_base + 4);
    let kit_spec = spec.kits.get(&kit_version).ok_or_else(|| {
        format!(
            "unsupported {} kit struct version {}",
            spec.device, kit_version
        )
    })?;

    let name = chars16(payload, pattern_spec.name_offset);
    let tempo_bpm = u32_be(payload, pattern_spec.tempo_offset) as f64 / 120.0;
    let kit_index = payload[pattern_spec.kit_index_offset];

    let kit_name = chars16(payload, kit_base + 8);
    let mut sound_names = Vec::with_capacity(pattern_spec.num_tracks);
    for t in 0..pattern_spec.num_tracks {
        let off = kit_base + kit_spec.sounds_offset + t * kit_spec.sound_size + 12;
        sound_names.push(chars16(payload, off));
    }
    let midi_mask = if let Some(off) = kit_spec.midi_mask_offset {
        u16_be(payload, kit_base + off)
    } else {
        0
    };

    Ok(PatternKit {
        version,
        name,
        tempo_bpm,
        kit_index,
        tracks,
        kit: KitInfo {
            version: kit_version,
            name: kit_name,
            sound_names,
            midi_mask,
        },
    })
}

pub fn track_notes(pattern_kit: &PatternKit, track_index: usize) -> Vec<Note> {
    if track_index >= pattern_kit.tracks.len() {
        return Vec::new();
    }
    let track = &pattern_kit.tracks[track_index];
    let mut notes = Vec::new();
    for s in 0..track.steps.len() {
        if (track.steps[s] & TRIG_ENABLED) == 0 {
            continue;
        }
        let step_u8 = s as u8;
        let slots = track.trigs.get(&step_u8).cloned().unwrap_or_default();
        let filled: Vec<_> = slots.iter().filter(|sl| sl.note != 0xff).collect();
        if !filled.is_empty() {
            for sl in filled {
                let velocity = if sl.velocity == 0xff {
                    track.default_velocity
                } else {
                    sl.velocity & 0x7f
                };
                let len_byte = if sl.length == 0xff {
                    track.default_length
                } else {
                    sl.length
                };
                let len_steps = length_byte_to_steps(len_byte);
                let len_steps = if len_steps.is_finite() {
                    len_steps
                } else {
                    track.length_steps as f64
                };
                let micro = micro_byte_to_steps(sl.micro);
                notes.push(Note {
                    step: step_u8,
                    pitch: sl.note & 0x7f,
                    velocity: velocity & 0x7f,
                    len_steps,
                    micro,
                });
            }
        } else {
            let sl_opt = slots.get(0);
            let velocity = match sl_opt {
                Some(sl) if sl.velocity != 0xff => sl.velocity,
                _ => track.default_velocity,
            };
            let len_byte = match sl_opt {
                Some(sl) if sl.length != 0xff => sl.length,
                _ => track.default_length,
            };
            let len_steps = length_byte_to_steps(len_byte);
            let len_steps = if len_steps.is_finite() {
                len_steps
            } else {
                track.length_steps as f64
            };
            let micro = sl_opt
                .map(|sl| micro_byte_to_steps(sl.micro))
                .unwrap_or(0.0);
            notes.push(Note {
                step: step_u8,
                pitch: track.default_note & 0x7f,
                velocity: velocity & 0x7f,
                len_steps,
                micro,
            });
        }
    }
    notes
}

pub fn track_trig_count(pattern_kit: &PatternKit, track_index: usize) -> usize {
    pattern_kit.tracks[track_index]
        .steps
        .iter()
        .filter(|w| *w & TRIG_ENABLED != 0)
        .count()
}

pub fn encode_track_notes(
    spec: &Spec,
    payload: &[u8],
    track_index: usize,
    notes: &[Note],
) -> Result<(Vec<u8>, usize), String> {
    let pattern_spec = &spec.pattern;
    let track_spec = &spec.track;
    if track_index >= pattern_spec.num_tracks {
        return Err(format!("no track {}", track_index));
    }
    let version = u32_be(payload, 0);
    if !spec.pattern_versions.contains(&version) {
        return Err(format!(
            "unsupported {} pattern struct version {} — refusing to write",
            spec.device, version
        ));
    }

    let mut out = payload.to_vec();
    let base = pattern_spec.tracks_offset + track_index * track_spec.size;
    for s in 0..track_spec.num_steps {
        let idx = base + s * 2 + 1;
        out[idx] &= !0x01u8;
    }

    let group_size = 6 * if spec.trig.layout == "quad" {
        spec.trig.max_notes
    } else {
        1
    };
    for o in (pattern_spec.trig_pool..pattern_spec.p_locks_index).step_by(group_size) {
        if o >= out.len() {
            break;
        }
        if out[o] == track_index as u8 {
            let end = (o + group_size).min(out.len());
            for i in o..end {
                out[i] = 0xff;
            }
        }
    }

    let mut sorted = notes.to_vec();
    sorted.sort_by(|a, b| a.step.cmp(&b.step).then(a.pitch.cmp(&b.pitch)));
    let max_notes = spec.trig.max_notes;
    let num_steps = track_spec.num_steps;
    // A `BTreeMap`, deliberately. The JS groups into a `Map`, which iterates in
    // insertion order, and insertion is pre-sorted by (step, pitch) — so groups
    // claim pool records in ascending step order, every run. A `HashMap` here
    // randomises that order per process, which means the same pattern encodes
    // to a different byte layout each time and the minimal-diff contract and
    // the read-back verify both stop meaning anything.
    let mut by_step: BTreeMap<u8, Vec<Note>> = BTreeMap::new();
    let mut dropped = 0usize;
    for n in sorted {
        let step = n.step;
        if (step as usize) >= num_steps {
            dropped += 1;
            continue;
        }
        let group = by_step.entry(step).or_default();
        if group.len() >= max_notes {
            dropped += 1;
            continue;
        }
        group.push(n);
    }

    let mut next_search = pattern_spec.trig_pool;
    let pool_end = pattern_spec.p_locks_index;

    let find_free = |out: &mut Vec<u8>,
                     start: usize,
                     pool_start: usize,
                     pool_end: usize,
                     group_size: usize|
     -> Option<usize> {
        let mut idx = if start > pool_start {
            start
        } else {
            pool_start
        };
        // align to group size
        if idx < pool_start {
            idx = pool_start;
        }
        loop {
            if idx + group_size > pool_end {
                return None;
            }
            if out[idx] == 0xff {
                return Some(idx);
            }
            idx += group_size;
            if idx >= pool_end {
                return None;
            }
        }
    };

    if spec.trig.layout == "quad" {
        for (step, group) in by_step {
            let o = find_free(
                &mut out,
                next_search,
                pattern_spec.trig_pool,
                pool_end,
                group_size,
            )
            .ok_or("pattern trig storage is full — too many trigs across all tracks")?;
            next_search = o + group_size;
            // write quad
            for slot in 0..max_notes {
                let s_off = o + slot * 6;
                let (pitch, note_ref) = if slot < group.len() {
                    (group[slot].pitch & 0x7f, &group[slot])
                } else {
                    (0xff, &group[0])
                };
                out[s_off] = track_index as u8;
                out[s_off + 1] = step;
                out[s_off + 2] = pitch;
                out[s_off + 3] = note_ref.velocity & 0x7f;
                out[s_off + 4] = steps_to_length_byte(note_ref.len_steps);
                out[s_off + 5] = micro_steps_to_byte(note_ref.micro);
            }
            let step_idx = base + step as usize * 2;
            out[step_idx] |= TRIG_SET_HI;
            out[step_idx + 1] |= TRIG_SET_LO;
        }
    } else {
        for (step, group) in by_step {
            for n in group {
                let o = find_free(
                    &mut out,
                    next_search,
                    pattern_spec.trig_pool,
                    pool_end,
                    group_size,
                )
                .ok_or("pattern trig storage is full — too many trigs across all tracks")?;
                next_search = o + group_size;
                out[o] = track_index as u8;
                out[o + 1] = step;
                out[o + 2] = n.pitch & 0x7f;
                out[o + 3] = n.velocity & 0x7f;
                out[o + 4] = steps_to_length_byte(n.len_steps);
                out[o + 5] = micro_steps_to_byte(n.micro);
                let step_idx = base + step as usize * 2;
                out[step_idx] |= TRIG_SET_HI;
                out[step_idx + 1] |= TRIG_SET_LO;
            }
        }
    }

    Ok((out, dropped))
}

// --- Diffing and annotation ---------------------------------------------------

/// One differing byte. `sent`/`read` are `None` past the end of a short payload,
/// which is how a length mismatch surfaces — the JS carries a separate
/// `lengthMismatch` marker, but trailing `None`s already say the same thing and
/// leave "empty means byte-identical" intact.
#[derive(Debug, Clone, PartialEq)]
pub struct ByteDiff {
    pub offset: usize,
    pub sent: Option<u8>,
    pub read: Option<u8>,
}

/// Byte-diff two payloads for the verify layer. Returns up to `cap` differing
/// offsets with both values — empty means byte-identical.
pub fn diff_payloads(a: &[u8], b: &[u8], cap: usize) -> Vec<ByteDiff> {
    let mut diffs = Vec::new();
    let len = a.len().max(b.len());
    for i in 0..len {
        if diffs.len() >= cap {
            break;
        }
        let (sent, read) = (a.get(i).copied(), b.get(i).copied());
        if sent != read {
            diffs.push(ByteDiff { offset: i, sent, read });
        }
    }
    diffs
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffRange {
    pub start: usize,
    pub end: usize,
    pub label: String,
}

/// Group the differing offsets of two payloads into contiguous ranges, each
/// annotated by `describe`. Adjacent changed bytes merge only while their region
/// label matches, so one range never spans two struct regions.
pub fn diff_annotated_ranges(
    a: &[u8],
    b: &[u8],
    describe: impl Fn(usize) -> String,
) -> Vec<DiffRange> {
    let mut ranges: Vec<DiffRange> = Vec::new();
    let mut open = false;
    for i in 0..a.len().max(b.len()) {
        if a.get(i) == b.get(i) {
            open = false;
            continue;
        }
        let label = describe(i);
        if open {
            let cur = ranges.last_mut().unwrap();
            if i == cur.end + 1 && label == cur.label {
                cur.end = i;
                continue;
            }
        }
        ranges.push(DiffRange { start: i, end: i, label });
        open = true;
    }
    ranges
}

/// Human annotation for a pattern-kit payload offset — the diff lab's map from
/// "byte 19003 changed" to "pool record #1, note". Everything the spec knows gets
/// a name; anything else is labelled unknown so unexplained diffs stand out
/// instead of hiding.
const POOL_FIELDS: [&str; 6] = ["track", "step", "note", "velocity", "length", "micro"];

pub fn describe_offset(spec: &Spec, offset: usize) -> String {
    let p = &spec.pattern;
    let t_spec = &spec.track;

    if offset < p.tracks_offset {
        return "pattern struct version".to_string();
    }
    if offset < p.trig_pool {
        let t = (offset - p.tracks_offset) / t_spec.size;
        let rel = (offset - p.tracks_offset) % t_spec.size;
        if rel < t_spec.num_steps * 2 {
            let half = if rel % 2 == 1 { "lo" } else { "hi" };
            return format!("track {} step word, step {} ({} byte)", t + 1, rel / 2 + 1, half);
        }
        if rel < t_spec.sound_p_locks {
            // The first three per-step arrays are the trig-condition lanes; the
            // rest are still unmapped.
            let start = 256 + (rel - 256) / 128 * 128;
            let step = (rel - 256) % 128 + 1;
            let lane = if start == t_spec.trig_cond {
                Some("COND")
            } else if start == t_spec.trig_fill {
                Some("FILL")
            } else if start == t_spec.trig_prob {
                Some("PROB")
            } else {
                None
            };
            return match lane {
                Some(l) => format!("track {} trig {}, step {}", t + 1, l, step),
                None => format!(
                    "track {} unknown per-step array {}, step {}",
                    t + 1,
                    (rel - 256) / 128 + 1,
                    step
                ),
            };
        }
        if rel < t_spec.defaults {
            return format!("track {} sound p-lock, step {}", t + 1, rel - t_spec.sound_p_locks + 1);
        }
        let d = rel - t_spec.defaults;
        let named = match d {
            0 => "default note".to_string(),
            1 => "default velocity".to_string(),
            2 => "default length".to_string(),
            _ if d == t_spec.length_steps - t_spec.defaults
                || d == t_spec.length_steps - t_spec.defaults + 1 =>
            {
                "track length (u16)".to_string()
            }
            _ if rel == t_spec.track_prob => "track PROB default".to_string(),
            _ => format!("+{}", d),
        };
        return format!("track {} defaults, {}", t + 1, named);
    }
    if offset < p.p_locks_index {
        let rec = (offset - p.trig_pool) / 6;
        return format!(
            "trig-record pool, record #{}, {}",
            rec,
            POOL_FIELDS[(offset - p.trig_pool) % 6]
        );
    }
    if offset < p.name_offset {
        let lane = (offset - p.p_locks_index) / p.p_lock_size;
        let rel = (offset - p.p_locks_index) % p.p_lock_size;
        let part = match rel {
            0 => "paramId".to_string(),
            1 => "track".to_string(),
            _ => format!(
                "step {} value ({} byte)",
                (rel - 2) / 2 + 1,
                if rel % 2 == 1 { "hi" } else { "lo" }
            ),
        };
        return format!("p-lock lane {}, {}", lane, part);
    }
    if offset < p.name_offset + 16 {
        return "pattern name".to_string();
    }
    if offset < p.tempo_offset + 4 {
        return "pattern tempo (u32, BPM × 120)".to_string();
    }
    if offset == p.kit_index_offset {
        return "kit index".to_string();
    }
    if offset < p.size {
        return format!("pattern settings tail +{}", offset - p.name_offset - 16);
    }
    format!("kit +{}", offset - p.size)
}
