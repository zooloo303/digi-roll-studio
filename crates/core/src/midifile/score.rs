// Stage 1 of the MIDI import design — **analyse**, MIDI_IMPORT_DESIGN.md §3.
//
// `score_file` reads a whole Standard MIDI File into a [`Score`]: every part
// (one per (MTrk, channel) pair that holds a note), the tempo map, the time-
// signature map, and the markers. It is pure analysis — nothing here decides
// which part matters, which is why [`super::midi_file_to_notes`] can sit on top
// as a thin wrapper while the Stage 2 fitter (`core::midi_import`, not built
// yet) will sit beside it and decide everything.
//
// The parsing rules are §3.2 and the grid arithmetic is §3.3; the doc-comments
// below cite both. Everything byte-level — running status, SysEx skipping, the
// `open`-list note pairing — is shared with the old single-track reader rather
// than reinvented, because the wrapper's byte-for-byte parity depends on it.

use std::collections::BTreeMap;

use super::{js_round, MidiFileError, Reader};

/// One note as the file held it: integer ticks, before any grid is applied.
/// `off` is exclusive of `on`, as the file's note-off time is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawNote {
    pub on: u64,
    pub off: u64,
    pub pitch: u8,
    pub velocity: u8,
}

/// A time signature. `den` is the **actual** denominator — 4, 8, 16 — not the
/// log2 byte the file carries, so `steps_per_bar` never has to un-log it.
/// (§3.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meter {
    pub num: u8,
    pub den: u8,
}

/// The parsed file. Everything a fitter needs to decide from, and nothing it
/// doesn't. (§3.1)
#[derive(Debug, Clone)]
pub struct Score {
    /// Ticks per quarter note. SMPTE divisions never reach here — they are
    /// refused at the header, as they always were.
    pub division: u16,
    /// Every tempo change as `(tick, bpm)`, in file order. Only the first is
    /// ever offered to the session (§5.5); the rest are collected so the
    /// report can count them as `tempo_changes_dropped`.
    pub tempo: Vec<(u64, f64)>,
    /// Time-signature changes. At least one entry, at tick 0 — the default
    /// 4/4 when the file says nothing. (§3.2)
    pub meters: Vec<(u64, Meter)>,
    /// Marker meta events (0x06) and cue points (0x07), trimmed; an empty
    /// one is dropped rather than carried as `""`.
    pub markers: Vec<(u64, String)>,
    /// One part per (MTrk index, MIDI channel) pair with at least one note,
    /// in file order — MTrk order, then first-seen channel order within it.
    pub parts: Vec<Part>,
    /// The last note-off or end-of-track, whichever is later, across the
    /// whole file. The timeline the fitter slices. (§3.2)
    pub end_tick: u64,
}

/// One (MTrk, channel) part — the unit of mapping. (§3.1)
#[derive(Debug, Clone)]
pub struct Part {
    /// Which MTrk chunk this came out of, 0-based over the MTrks in the file.
    pub mtrk: usize,
    /// The MIDI channel, 0–15. Channel 10 as musicians say it is `9` here.
    pub channel: u8,
    /// The MTrk's track-name (0x03) or instrument-name (0x04) meta, with
    /// `" ch N"` (N = channel + 1) appended when its MTrk yielded more than
    /// one part; `Track {n} ch {c+1}` when the MTrk was never named. (§3.2)
    pub name: String,
    /// The last program change seen on this channel in this MTrk.
    pub program: Option<u8>,
    /// Ticks, sorted by `on`. Two notes starting together keep file order.
    pub notes: Vec<RawNote>,
    pub stats: PartStats,
    /// CC number → event count. Collected now because it is cheap and the
    /// deferred CC→p-lock work (§7) will want it.
    pub cc: BTreeMap<u8, usize>,
    /// CC64 (sustain pedal) event count, pulled out of `cc` because the
    /// report will name it directly.
    pub sustain_events: usize,
    pub pitch_bend_events: usize,
}

/// Facts about a part, derived after parsing — the honest answers the mapping
/// dialog will show. (§3.1)
#[derive(Debug, Clone)]
pub struct PartStats {
    pub notes: usize,
    pub pitch_lo: u8,
    pub pitch_hi: u8,
    /// Bars from [`bar_starts`]: the first bar a note starts in, and the bar
    /// the part ends in. Both 0 when the part has no notes.
    pub first_bar: u32,
    pub last_bar: u32,
    /// The most notes sharing one quantised 16th step — what polyphony the
    /// part peaks at.
    pub max_simultaneous: u8,
    /// The share of notes whose on-tick is not within ±1/48 of a step of a
    /// 16th. A swung or humanised file scores high here.
    pub off_grid_ratio: f32,
    /// Channel 10 (channel 9 here). The GM percussion program banks are not
    /// detectable from a program-change byte alone — a file carries no bank
    /// select guarantee — so the channel is the whole signal, honestly.
    pub looks_like_drums: bool,
    /// `off_grid_ratio > 0.5` **and** the off-grid notes sit near thirds of a
    /// step — swing's 1/3 and 2/3 — rather than scattered.
    pub looks_like_triplets: bool,
}

/// Steps per bar for a meter, on a 16th grid: `num * 16 / den`, rounded **up**
/// when it comes out fractional (den = 32 is the only realistic case). The
/// caller that matters — the Stage 2 fitter — is told it rounded via
/// `report.meter_approximated`; this function just answers. (§3.3)
///
/// 3/4 → 12, 6/8 → 12, 7/8 → 14, 5/4 → 20, 4/4 → 16.
pub fn steps_per_bar(meter: Meter) -> f64 {
    (f64::from(meter.num) * 16.0 / f64::from(meter.den).max(1.0)).ceil()
}

/// The bar boundaries of a score: `(tick, meter)` pairs, one per bar, by
/// walking the meter map. **A meter change always starts a new bar**, even
/// mid-bar — the alternative (finishing the old bar in the new meter) is how
/// a 7/8-after-3/4 file ends up with a phantom 15-step bar. (§3.3)
///
/// The fitter slices segments on these, so it is one function, used
/// everywhere, rather than re-walked per caller.
pub fn bar_starts(score: &Score) -> Vec<(u64, Meter)> {
    let per16 = f64::from(score.division) / 4.0;
    let mut out = Vec::new();
    let mut tick = 0u64;
    let mut meter = score
        .meters
        .first()
        .map(|&(_, m)| m)
        .unwrap_or(Meter { num: 4, den: 4 });
    out.push((0, meter));
    // One pass over the meter map; bars are emitted until the timeline ends.
    let mut next_change = score.meters.iter().skip(1).peekable();
    loop {
        // A meter change due before this bar ends always starts a new bar,
        // even mid-bar — the alternative (finishing the old bar in the new
        // meter) is how a 7/8-after-3/4 file ends up with a phantom bar.
        if let Some(&&(change, new_meter)) = next_change.peek() {
            if change > tick {
                tick = change;
                meter = new_meter;
                next_change.next();
                if tick > score.end_tick {
                    break;
                }
                out.push((tick, meter));
                continue;
            }
            // Due now (or overdue): take it without emitting a duplicate bar.
            meter = new_meter;
            next_change.next();
            continue;
        }
        let bar_ticks = (steps_per_bar(meter) * per16) as u64;
        let next = tick.saturating_add(bar_ticks.max(1));
        if next > score.end_tick {
            break;
        }
        tick = next;
        out.push((tick, meter));
    }
    out
}

/// Which bar of `bars` (from [`bar_starts`]) a tick falls in — the count of
/// bar starts at or before it, minus one, clamped to 0.
fn bar_of(bars: &[(u64, Meter)], tick: u64) -> u32 {
    bars.iter()
        .take_while(|&&(start, _)| start <= tick)
        .count()
        .saturating_sub(1) as u32
}

/// Derive a part's stats. Everything here is a fact about the notes, computed
/// once, because the mapping dialog should show answers rather than recompute
/// them per paint. (§3.1)
fn part_stats(ch: &ChannelParse, channel: u8, bars: &[(u64, Meter)], score: &Score) -> PartStats {
    let per16 = f64::from(score.division) / 4.0;
    let notes = ch.notes.len();
    let pitch_lo = ch.notes.iter().map(|n| n.pitch).min().unwrap_or(0);
    let pitch_hi = ch.notes.iter().map(|n| n.pitch).max().unwrap_or(0);
    let first_bar = ch.notes.first().map(|n| bar_of(bars, n.on)).unwrap_or(0);
    let last_bar = ch
        .notes
        .iter()
        .map(|n| bar_of(bars, n.off))
        .max()
        .unwrap_or(0);

    // Most notes sharing one quantised 16th step.
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for n in &ch.notes {
        let step = js_round(n.on as f64 / per16) as i64;
        *counts.entry(step).or_insert(0) += 1;
    }
    let max_simultaneous = counts
        .values()
        .copied()
        .max()
        .unwrap_or(0)
        .min(u8::MAX as usize) as u8;

    // Off-grid: not within ±1/48 of a step of a 16th — the width of one tick
    // at the export's 96 TPQN, so its own micro-timing never reads as off-grid.
    let tolerance = per16 / 48.0;
    let mut off_grid = 0usize;
    let mut near_thirds = 0usize;
    for n in &ch.notes {
        let f = n.on as f64 / per16;
        let frac = f - f.floor();
        let dist = frac.min(1.0 - frac);
        if dist * per16 > tolerance {
            off_grid += 1;
            // Near a third of a step: swing's 1/3 and 2/3, to the same
            // ±1/48-step tolerance the off-grid test uses. Measured from the
            // note's own step, not the wrap-around — a note sitting just
            // *before* a step is not a third late.
            let third = (dist - 1.0 / 3.0).abs().min((dist - 2.0 / 3.0).abs());
            if third <= 1.0 / 48.0 {
                near_thirds += 1;
            }
        }
    }
    let off_grid_ratio = if notes == 0 {
        0.0
    } else {
        off_grid as f32 / notes as f32
    };

    PartStats {
        notes,
        pitch_lo,
        pitch_hi,
        first_bar,
        last_bar,
        max_simultaneous,
        off_grid_ratio,
        looks_like_drums: channel == 9,
        // thirds rather than halves, so a swung file is not a triplets file.
        looks_like_triplets: off_grid_ratio > 0.5 && off_grid > 0 && near_thirds >= off_grid,
    }
}

/// What one MTrk parsed into, before stats: per-channel accumulators and the
/// MTrk's name, so the parts split out of it can inherit it. (§3.2)
struct TrackParse {
    name: Option<String>,
    /// Channel index → channel parse, in first-seen order.
    channels: Vec<ChannelParse>,
    /// Last tick the track's events reached (end-of-track or last event),
    /// for `end_tick`.
    last_tick: u64,
}

struct ChannelParse {
    program: Option<u8>,
    notes: Vec<RawNote>,
    cc: BTreeMap<u8, usize>,
    pitch_bend_events: usize,
    /// Notes currently sounding, `(pitch, on-tick, velocity)`. Kept beside the
    /// finished notes rather than in a separate structure so the channel's
    /// state is one thing.
    open: Vec<(u8, u64, u8)>,
}

/// Parse a type 0 or type 1 SMF into a [`Score`]. Type 2 is refused
/// ([`MidiFileError::IndependentSequences`]): its tracks are independent
/// sequences sharing no timeline, so there is no honest `end_tick` to fit
/// against. SMPTE and zero division are refused as they always were. (§3.2)
///
/// A file with zero notes across all parts is `Ok` with empty `parts` — the
/// callers decide what to say about it.
pub fn score_file(bytes: &[u8]) -> Result<Score, MidiFileError> {
    let mut r = Reader { bytes, i: 0 };
    if bytes.len() < 14 || r.tag()? != *b"MThd" {
        return Err(MidiFileError::NotAMidiFile);
    }
    let header_len = r.u32()?;
    let format = r.u16()?;
    let ntrks = r.u16()?;
    let division = r.u16()?;
    // Header chunks longer than six bytes are legal and carry nothing this reads.
    r.i =
        r.i.checked_add(header_len.saturating_sub(6) as usize)
            .ok_or(MidiFileError::Truncated)?;
    if format == 2 {
        return Err(MidiFileError::IndependentSequences);
    }
    if division & 0x8000 != 0 {
        return Err(MidiFileError::SmpteTimecode);
    }
    if division == 0 {
        return Err(MidiFileError::NotAMidiFile);
    }
    let per16 = f64::from(division) / 4.0;

    let mut tempo: Vec<(u64, f64)> = Vec::new();
    let mut meters: Vec<(u64, Meter)> = Vec::new();
    let mut markers: Vec<(u64, String)> = Vec::new();
    let mut tracks: Vec<TrackParse> = Vec::new();
    let mut end_tick = 0u64;

    for _ in 0..ntrks {
        if r.i >= bytes.len() {
            break;
        }
        let id = r.tag()?;
        let track_len = r.u32()? as usize;
        let end = r.i.checked_add(track_len).ok_or(MidiFileError::Truncated)?;
        if end > bytes.len() {
            return Err(MidiFileError::Truncated);
        }
        if id != *b"MTrk" {
            r.i = end;
            continue;
        }

        let mut track = TrackParse {
            name: None,
            channels: Vec::new(),
            last_tick: 0,
        };
        let mut tick = 0u64;
        let mut status = 0u8;
        while r.i < end {
            tick += u64::from(r.vlen()?);
            track.last_tick = tick;
            let mut s = r.peek()?;
            if s & 0x80 != 0 {
                status = s;
                r.i += 1;
            } else {
                s = status;
            }
            if s == 0xff {
                let meta = r.u8()?;
                match meta {
                    0x03 | 0x04 => {
                        // Track name, or instrument name when no track name has
                        // been seen — both belong to the MTrk. (§3.2)
                        let text = text_of(r.take()?);
                        if !text.is_empty() && track.name.is_none() {
                            track.name = Some(text);
                        }
                    }
                    0x06 | 0x07 => {
                        // Markers, and cue points as markers too; trimmed, and
                        // empty ones dropped. (§3.2)
                        let text = text_of(r.take()?);
                        if !text.is_empty() {
                            markers.push((tick, text));
                        }
                    }
                    0x51 => {
                        // Tempo: µs per quarter, three bytes big-endian. Every
                        // change recorded as (tick, bpm). (§3.2)
                        let body = r.take()?;
                        if body.len() >= 3 {
                            let uspq = (u32::from(body[0]) << 16)
                                | (u32::from(body[1]) << 8)
                                | u32::from(body[2]);
                            if uspq > 0 {
                                tempo.push((tick, 60_000_000.0 / f64::from(uspq)));
                            }
                        }
                    }
                    0x58 => {
                        // Time signature: nn, dd (log2 of the denominator) — the
                        // Meter stores the actual denominator. (§3.1)
                        let body = r.take()?;
                        if body.len() >= 2 {
                            meters.push((
                                tick,
                                Meter {
                                    num: body[0],
                                    den: 1u8 << body[1].min(7),
                                },
                            ));
                        }
                    }
                    _ => {
                        r.skip()?;
                    }
                }
                status = 0;
            } else if s == 0xf0 || s == 0xf7 {
                r.skip()?;
                status = 0;
            } else {
                let hi = s & 0xf0;
                let channel = s & 0x0f;
                let d1 = r.u8()?;
                let d2 = if hi == 0xc0 || hi == 0xd0 { 0 } else { r.u8()? };
                match hi {
                    0x90 if d2 > 0 => {
                        // `open` keeps the old semantics exactly (§3.2): an
                        // in-place replace on a repeated note-on, which is what
                        // `Map.set` on an existing key did in `js/midi.js`.
                        let ch = channel_slot(&mut track, channel);
                        match ch.open.iter_mut().find(|(p, _, _)| *p == d1) {
                            Some(slot) => *slot = (d1, tick, d2),
                            None => ch.open.push((d1, tick, d2)),
                        }
                    }
                    0x80 | 0x90 => {
                        // Note-off, or note-on at velocity 0. (§3.2)
                        let ch = channel_slot(&mut track, channel);
                        if let Some(at) = ch.open.iter().position(|(p, _, _)| *p == d1) {
                            let (pitch, on, velocity) = ch.open.remove(at);
                            ch.notes.push(RawNote {
                                on,
                                off: tick,
                                pitch,
                                velocity,
                            });
                            end_tick = end_tick.max(tick);
                        }
                    }
                    0xb0 => {
                        let ch = channel_slot(&mut track, channel);
                        *ch.cc.entry(d1).or_insert(0) += 1;
                    }
                    0xc0 => {
                        channel_slot(&mut track, channel).program = Some(d1);
                    }
                    0xe0 => {
                        channel_slot(&mut track, channel).pitch_bend_events += 1;
                    }
                    _ => {}
                }
            }
        }
        r.i = end;
        end_tick = end_tick.max(track.last_tick);
        // Never released: one 16th of length, so a file that forgot its
        // note-offs still arrives as music. (§3.2)
        for ch in &mut track.channels {
            let open = std::mem::take(&mut ch.open);
            for (pitch, on, velocity) in open {
                ch.notes.push(RawNote {
                    on,
                    off: on + per16 as u64,
                    pitch,
                    velocity,
                });
            }
            // Stable, so two notes starting on the same tick keep the order the
            // file put them in. (§3.1)
            ch.notes.sort_by_key(|n| n.on);
        }
        tracks.push(track);
    }

    // Default meter when the file says nothing: 4/4 at tick 0. (§3.2)
    if meters.is_empty() {
        meters.push((0, Meter { num: 4, den: 4 }));
    }
    meters.sort_by_key(|&(tick, _)| tick);
    tempo.sort_by_key(|&(tick, _)| tick);
    markers.sort_by_key(|&(tick, _)| tick);

    let score = Score {
        division,
        tempo,
        meters,
        markers,
        parts: Vec::new(),
        end_tick,
    };
    let bars = bar_starts(&score);
    let mut parts = Vec::new();
    for (mtrk, track) in tracks.iter().enumerate() {
        let sounding: Vec<usize> = (0..track.channels.len())
            .filter(|&i| !track.channels[i].notes.is_empty())
            .collect();
        let multi = sounding.len() > 1;
        for i in sounding {
            let ch = &track.channels[i];
            let channel = i as u8;
            let name = match &track.name {
                Some(name) if multi => format!("{name} ch {}", channel + 1),
                Some(name) => name.clone(),
                None => format!("Track {} ch {}", mtrk + 1, channel + 1),
            };
            let sustain_events = ch.cc.get(&64).copied().unwrap_or(0);
            let stats = part_stats(ch, channel, &bars, &score);
            parts.push(Part {
                mtrk,
                channel,
                name,
                program: ch.program,
                notes: ch.notes.clone(),
                stats,
                cc: ch.cc.clone(),
                sustain_events,
                pitch_bend_events: ch.pitch_bend_events,
            });
        }
    }
    Ok(Score { parts, ..score })
}

/// The parse accumulator for one channel of one MTrk, created on first use.
/// The channel's index in `track.channels` **is** the channel number, so a
/// channel never seen takes no space.
fn channel_slot(track: &mut TrackParse, channel: u8) -> &mut ChannelParse {
    while track.channels.len() <= channel as usize {
        track.channels.push(ChannelParse {
            program: None,
            notes: Vec::new(),
            cc: BTreeMap::new(),
            pitch_bend_events: 0,
            open: Vec::new(),
        });
    }
    &mut track.channels[channel as usize]
}

/// Text out of a meta payload: lossy, trimmed. A file that holds junk in a
/// text meta still parses; the junk just isn't a name.
fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

#[cfg(test)]
mod tests {
    //! Hand-built byte fixtures, per MIDI_IMPORT_DESIGN.md §3.4. No files, no
    //! hardware — and a test that shares the code under test cannot fail it,
    //! so every fixture writes its own VLQs.

    use super::*;

    /// A VLQ, written out here rather than reused from the module.
    fn vlq(mut v: u32, body: &mut Vec<u8>) {
        let mut vb = vec![(v & 0x7f) as u8];
        v /= 128;
        while v > 0 {
            vb.insert(0, ((v & 0x7f) | 0x80) as u8);
            v /= 128;
        }
        body.extend_from_slice(&vb);
    }

    /// An SMF around complete MTrk bodies.
    fn smf(format: u16, division: u16, tracks: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MThd");
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&format.to_be_bytes());
        bytes.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&division.to_be_bytes());
        for body in tracks {
            bytes.extend_from_slice(b"MTrk");
            bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
            bytes.extend_from_slice(body);
        }
        bytes
    }

    /// One note (on then off) into a body at absolute ticks.
    fn note(body: &mut Vec<u8>, t: &mut u64, on: u64, off: u64, ch: u8, pitch: u8) {
        vlq((on - *t) as u32, body);
        body.extend_from_slice(&[0x90 | ch, pitch, 100]);
        vlq((off - on) as u32, body);
        body.extend_from_slice(&[0x80 | ch, pitch, 0]);
        *t = off;
    }

    fn eot(body: &mut Vec<u8>) {
        body.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    }

    /// A type-0 file carrying two channels in one MTrk splits into two parts.
    #[test]
    fn a_type_zero_track_with_two_channels_is_two_parts() {
        let mut body = Vec::new();
        let mut t = 0;
        note(&mut body, &mut t, 0, 24, 0, 60);
        note(&mut body, &mut t, 24, 48, 5, 67);
        eot(&mut body);
        let score = score_file(&smf(0, 96, &[&body])).unwrap();
        assert_eq!(score.parts.len(), 2);
        assert_eq!(score.parts[0].channel, 0);
        assert_eq!(score.parts[1].channel, 5);
        // Both parts inherit the MTrk index, and an unnamed MTrk falls back
        // to "Track {n} ch {c+1}".
        assert_eq!(score.parts[0].name, "Track 1 ch 1");
        assert_eq!(score.parts[1].name, "Track 1 ch 6");
        assert_eq!(score.parts[0].notes.len(), 1);
    }

    /// A type-1 file whose one named MTrk holds two channels names the parts
    /// after the track, with " ch N" appended because there is more than one.
    #[test]
    fn a_multi_channel_track_names_its_parts_after_it_with_the_channel() {
        let mut conductor = Vec::new();
        eot(&mut conductor);
        let mut body = Vec::new();
        body.extend_from_slice(&[0x00, 0xff, 0x03, 0x04]);
        body.extend_from_slice(b"Keys");
        let mut t = 0;
        note(&mut body, &mut t, 0, 24, 0, 60);
        note(&mut body, &mut t, 24, 48, 9, 38);
        eot(&mut body);
        let score = score_file(&smf(1, 96, &[&conductor, &body])).unwrap();
        assert_eq!(score.parts.len(), 2);
        assert_eq!(score.parts[0].name, "Keys ch 1");
        assert_eq!(score.parts[1].name, "Keys ch 10");
        // The mtrk index counts real MTrks, so the part track is number 1.
        assert_eq!(score.parts[0].mtrk, 1);
    }

    /// 3/4 and 6/8 both grid out at twelve 16ths per bar. (§3.3)
    #[test]
    fn three_four_and_six_eight_are_twelve_steps_per_bar() {
        assert_eq!(steps_per_bar(Meter { num: 3, den: 4 }), 12.0);
        assert_eq!(steps_per_bar(Meter { num: 6, den: 8 }), 12.0);
        assert_eq!(steps_per_bar(Meter { num: 7, den: 8 }), 14.0);
        assert_eq!(steps_per_bar(Meter { num: 5, den: 4 }), 20.0);
        assert_eq!(steps_per_bar(Meter { num: 4, den: 4 }), 16.0);
    }

    /// A meter change mid-file always starts a new bar. (§3.3)
    #[test]
    fn a_meter_change_mid_file_starts_a_new_bar() {
        let mut body = Vec::new();
        // 4/4 at tick 0, 3/4 at tick 384 (bar 1's end — an honest change).
        body.extend_from_slice(&[0x00, 0xff, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08]);
        vlq(384, &mut body);
        body.extend_from_slice(&[0xff, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08]);
        let mut t = 384;
        note(&mut body, &mut t, 384, 384 + 24 * 12, 0, 60);
        eot(&mut body);
        let score = score_file(&smf(0, 96, &[&body])).unwrap();
        assert_eq!(
            score.meters,
            vec![
                (0, Meter { num: 4, den: 4 }),
                (384, Meter { num: 3, den: 4 })
            ]
        );
        let bars = bar_starts(&score);
        // Bar 0 at tick 0 in 4/4, bar 1 at the change in 3/4, and the change
        // itself starts a bar rather than mid-counting.
        assert!(bars.contains(&(0, Meter { num: 4, den: 4 })));
        assert!(bars.contains(&(384, Meter { num: 3, den: 4 })));
        assert_eq!(bars[1], (384, Meter { num: 3, den: 4 }));
    }

    /// Markers and cue points are collected, trimmed, and empty ones dropped.
    #[test]
    fn markers_are_collected_and_cue_points_count_as_markers() {
        let mut body = Vec::new();
        body.extend_from_slice(&[0x00, 0xff, 0x06, 0x05]);
        body.extend_from_slice(b"Intro");
        vlq(96, &mut body);
        body.extend_from_slice(&[0xff, 0x07, 0x06]);
        body.extend_from_slice(b"Verse ");
        vlq(0, &mut body);
        body.extend_from_slice(&[0xff, 0x06, 0x01]);
        body.extend_from_slice(b" ");
        eot(&mut body);
        let score = score_file(&smf(0, 96, &[&body])).unwrap();
        assert_eq!(
            score.markers,
            vec![(0, "Intro".to_string()), (96, "Verse".to_string())]
        );
    }

    /// A type-2 file is refused: independent sequences share no timeline.
    #[test]
    fn a_type_two_file_is_refused() {
        let mut body = Vec::new();
        eot(&mut body);
        let bytes = smf(2, 96, &[&body, &body]);
        assert_eq!(
            score_file(&bytes).unwrap_err(),
            MidiFileError::IndependentSequences
        );
    }

    /// A file that ends part-way through a track is refused, not read as
    /// rubbish. This one names the new error on purpose: the refusal set grew.
    #[test]
    fn a_truncated_file_is_refused() {
        let mut body = Vec::new();
        let mut t = 0;
        note(&mut body, &mut t, 0, 24, 0, 60);
        eot(&mut body);
        let mut bytes = smf(0, 96, &[&body]);
        bytes.truncate(bytes.len() - 3);
        assert_eq!(score_file(&bytes).unwrap_err(), MidiFileError::Truncated);
    }

    /// Every tempo change is recorded as (tick, bpm). (§3.2)
    #[test]
    fn every_tempo_change_is_recorded() {
        let mut body = Vec::new();
        // 120 bpm = 500000 µs/q at tick 0; 60 bpm = 1000000 µs/q at tick 96.
        body.extend_from_slice(&[0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20]);
        vlq(96, &mut body);
        body.extend_from_slice(&[0xff, 0x51, 0x03, 0x0f, 0x42, 0x40]);
        eot(&mut body);
        let score = score_file(&smf(0, 96, &[&body])).unwrap();
        assert_eq!(score.tempo.len(), 2);
        assert_eq!(score.tempo[0], (0, 120.0));
        assert!((score.tempo[1].1 - 60.0).abs() < 0.001);
        assert_eq!(score.tempo[1].0, 96);
    }

    /// Channel 10 (channel 9 here) looks like drums. (§3.1)
    #[test]
    fn channel_ten_looks_like_drums() {
        let mut body = Vec::new();
        let mut t = 0;
        note(&mut body, &mut t, 0, 24, 9, 38);
        note(&mut body, &mut t, 24, 48, 0, 60);
        eot(&mut body);
        let score = score_file(&smf(0, 96, &[&body])).unwrap();
        // Parts come in channel order, so channel 1 leads and channel 10 —
        // the drums — follows.
        assert!(!score.parts[0].stats.looks_like_drums);
        assert!(score.parts[1].stats.looks_like_drums);
    }

    /// A chord is the most notes sharing one quantised step. (§3.1)
    #[test]
    fn a_chord_counts_as_simultaneous() {
        let mut body = Vec::new();
        for pitch in [60, 64, 67] {
            body.push(0x00);
            body.extend_from_slice(&[0x90, pitch, 100]);
        }
        vlq(24, &mut body);
        body.extend_from_slice(&[0x80, 60, 0]);
        body.push(0x00);
        body.extend_from_slice(&[0x80, 64, 0]);
        body.push(0x00);
        body.extend_from_slice(&[0x80, 67, 0]);
        let mut t = 24;
        note(&mut body, &mut t, 48, 72, 0, 60);
        eot(&mut body);
        let score = score_file(&smf(0, 96, &[&body])).unwrap();
        assert_eq!(score.parts[0].stats.max_simultaneous, 3);
        assert_eq!(score.parts[0].stats.off_grid_ratio, 0.0);
    }

    /// A swung export reads back as off-grid notes near thirds of a step —
    /// swing is a third of a 16th by construction in `track_to_midi_file`.
    #[test]
    fn a_swung_export_reads_as_off_grid_and_triplet_like() {
        use crate::model::{Note, Track, TrackKind};
        let mut track = Track::new(0, TrackKind::Audio);
        track.channel = 0;
        track.length_steps = 16;
        // Three steps of four sat a third of a step late, via fractional
        // steps — the roll's own way of saying "between the grid lines", and
        // where swing lands when it resolves: a third of a 16th is 8 ticks,
        // beyond the ±0.5-tick off-grid tolerance and dead on the 1/3 line.
        track.notes = (0..4)
            .map(|s| {
                Note::new(
                    if s == 0 { 0.0 } else { s as f64 + 1.0 / 3.0 },
                    36 + s,
                    0.5,
                    100,
                    0.0,
                )
            })
            .collect();
        let bytes = crate::midifile::track_to_midi_file(&track, "swung", 50, 120.0);
        let score = score_file(&bytes).unwrap();
        let stats = &score.parts[0].stats;
        // Three of four notes off the grid, all on the 1/3 line: over the
        // half the rule asks for, and nothing scattered elsewhere.
        assert_eq!(stats.off_grid_ratio, 0.75);
        assert!(stats.looks_like_triplets);
        // And the part inherited the exported name.
        assert_eq!(score.parts[0].name, "swung");
    }
}
