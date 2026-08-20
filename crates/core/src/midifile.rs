// Standard MIDI File in and out — the Edit panel's MIDI FILES group.
//
// A port of the two halves of `js/midi.js` that have nothing to do with the Web
// MIDI engine around them: `patternToMidiFile` and `midiFileToNotes`. They come
// here rather than to `protocol` because a MIDI file is not an Elektron thing:
// nothing in it is device-specific, and `protocol` is for bytes that only mean
// something to a DT2 or a DN2.
//
// ## What a MIDI file cannot carry, and why the panel has to say so
//
// **Trig conditions do not survive.** A MIDI file has no notion of PROB, FILL,
// `1ST` or `2:4`, so an export drops every one of them and an import cannot
// invent any. That is not a limitation of this code, it is the format, and it is
// the reason the export button carries the warning rather than the docs — PLAN.md
// §9 asks for it up front rather than discovered afterwards. The p-lock lanes go
// the same way, for the same reason.
//
// **What does survive** is what a MIDI file is good at: pitch, velocity, length,
// and timing — including micro-timing and swing, both of which are *baked into
// the tick positions* on the way out, so the file sounds the way the app does
// rather than the way the grid looks. That is a one-way door: re-importing an
// exported file gives back the swung, micro-shifted positions as micro-timing on
// a straight grid, because the file no longer says which of the two it was.
//
// ## No oracle covered this
//
// `test/` in the JS original has **no test for either function** — checked by
// grep before a line was written, the way `DEVELOPMENT.md` says to. So the expected
// values in this file's tests were derived by *running* the JS
// (`node --input-type=module -e` against `js/midi.js`) and are recorded as
// literal byte strings below, with the call that produced them written out in the
// test. That is the Phase 1 method, and it is the strongest ground truth
// available for a function nothing ever asserted on.

use crate::edit_ops::{clamp_micro, clamp_velocity, BAR_STEPS, MAX_STEPS};
use crate::lengths::LEN_MIN;
use crate::model::{Note, Track};

/// Ticks per quarter note in the files this writes. 96 makes a 16th step exactly
/// [`TICKS_PER_STEP`] ticks, which is what keeps a micro-timing offset a whole
/// number of ticks at the resolutions that matter.
pub const TPQN: u16 = 96;

/// One 16th step, in ticks.
pub const TICKS_PER_STEP: f64 = TPQN as f64 / 4.0;

/// `Math.round`, which rounds a half **up** where Rust's `f64::round` rounds it
/// away from zero. The two disagree at exactly −n.5. `protocol`'s
/// `micro_steps_to_byte` carries the same note for the same reason.
///
/// **No test in this crate can tell the two apart, and that is worth writing down
/// rather than papering over** — a deliberate bug swapping this for `f64::round`
/// failed nothing, which `DEVELOPMENT.md` lesson 6 says to treat as the finding.
/// Walking the four call sites says why: the two length roundings and the tempo
/// take strictly positive arguments, the import's two take a non-negative tick
/// count over a positive divisor, and the one place a negative *can* arrive — a
/// note on step 0 nudged backwards — is `.max(0.0)`'d immediately afterwards, and
/// would need a micro-timing of exactly −1/48 of a step, which neither the drag
/// (hundredths) nor a hardware import (24ths) can produce.
///
/// So this is the faithful port rather than a difference anything observes, and it
/// stays because the next thing to reach for it may well hand it a negative.
fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// A MIDI variable-length quantity, low 7 bits last.
fn vlq(mut n: u32, out: &mut Vec<u8>) {
    let mut bytes = vec![(n & 0x7f) as u8];
    n /= 128;
    while n > 0 {
        bytes.insert(0, ((n & 0x7f) | 0x80) as u8);
        n /= 128;
    }
    out.extend_from_slice(&bytes);
}

fn chunk(id: &[u8; 4], body: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
}

/// One track of one pattern as a type-0 Standard MIDI File.
///
/// `swing` is the pattern's, 50–80 as the box stores it, because swing is a
/// per-pattern byte on the box and this is where it stops being one: it is
/// resolved into tick offsets here and the file carries no swing field of its
/// own. `name` is likewise the pattern's, which is what makes an exported file
/// recognisable as `A01 T1` rather than as `pattern.mid`.
///
/// **Swing lands on odd steps**, and on *any* fractional step — `js/midi.js`
/// tests `n.step % 2`, which is truthy for 1.5 as well as for 1. Kept as it is
/// because a fractional step is already off the grid and the box swings by
/// position rather than by index.
pub fn track_to_midi_file(track: &Track, name: &str, swing: u8, bpm: f64) -> Vec<u8> {
    let swing_ticks = (f64::from(swing) - 50.0) / 50.0 * (TICKS_PER_STEP / 3.0);
    let ch = track.channel & 0x0f;

    // (tick, order, bytes). `order` puts a note-off before a note-on that shares
    // a tick, so a note repeating on the next step is two notes rather than one
    // that never released.
    let mut events: Vec<(i64, u8, [u8; 3])> = Vec::with_capacity(track.notes.len() * 2);
    for n in &track.notes {
        let swung = if n.step % 2.0 != 0.0 { swing_ticks } else { 0.0 };
        let start = js_round((n.step + n.micro) * TICKS_PER_STEP + swung).max(0.0) as i64;
        let end = start + (js_round(n.len * TICKS_PER_STEP) as i64).max(1);
        events.push((start, 1, [0x90 | ch, n.pitch & 0x7f, clamp_velocity(i32::from(n.velocity))]));
        events.push((end, 0, [0x80 | ch, n.pitch & 0x7f, 0]));
    }
    events.sort_by_key(|(tick, order, _)| (*tick, *order));

    let mut body = Vec::new();
    // The track name, then the tempo, both at tick 0. Every name byte is masked
    // to 7 bits, as the JS masks it: a meta event's payload is bytes, but a
    // pattern name arriving from a box is not guaranteed to be ASCII and a
    // high-bit byte in a text meta is a file some readers reject.
    body.push(0);
    body.extend_from_slice(&[0xff, 0x03]);
    let name_bytes: Vec<u8> = name.chars().map(|c| (c as u32 & 0x7f) as u8).collect();
    vlq(name_bytes.len() as u32, &mut body);
    body.extend_from_slice(&name_bytes);

    let uspq = js_round(60_000_000.0 / bpm) as u32;
    body.push(0);
    body.extend_from_slice(&[0xff, 0x51, 0x03]);
    body.extend_from_slice(&[(uspq >> 16) as u8, (uspq >> 8) as u8, uspq as u8]);

    let mut t = 0i64;
    for (tick, _, data) in &events {
        vlq((tick - t) as u32, &mut body);
        body.extend_from_slice(data);
        t = *tick;
    }
    // Run the track out to the full pattern length, so a loop keeps its bar
    // count even when nothing plays in the last bar.
    let full = f64::from(track.length_steps) * TICKS_PER_STEP;
    vlq((full as i64 - t).max(0) as u32, &mut body);
    body.extend_from_slice(&[0xff, 0x2f, 0x00]);

    let mut out = Vec::new();
    chunk(
        b"MThd",
        &[0, 0, 0, 1, (TPQN >> 8) as u8, TPQN as u8],
        &mut out,
    );
    chunk(b"MTrk", &body, &mut out);
    out
}

/// What an import found: notes on this app's fractional-step grid, the track
/// length they need, and how many were thrown away.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub notes: Vec<Note>,
    pub length_steps: u16,
    /// Notes that fell past the longest track a box can hold. Reported rather
    /// than clamped, because a note clamped to step 127 lands somewhere nobody
    /// asked for — the same call [`crate::edit_ops::place_clipboard`] makes.
    pub dropped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiFileError {
    NotAMidiFile,
    /// Division with the high bit set: SMPTE timecode, where a tick is a video
    /// frame rather than a fraction of a beat. There is no honest step grid to
    /// map that onto without knowing the frame rate the music was written at.
    SmpteTimecode,
    /// Ran off the end of the buffer. **This has no counterpart in the oracle**:
    /// JS reads `undefined` past the end of a `Uint8Array` and carries on with
    /// `NaN`, which here would be an index panic. So the port is stricter, and
    /// deliberately — a truncated file is a thing that happens.
    Truncated,
}

impl std::fmt::Display for MidiFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAMidiFile => write!(f, "that is not a MIDI file"),
            Self::SmpteTimecode => {
                write!(f, "SMPTE-timecode MIDI files are not supported")
            }
            Self::Truncated => write!(f, "the file ends part-way through a MIDI track"),
        }
    }
}

impl std::error::Error for MidiFileError {}

/// A cursor that refuses to read past the end rather than reading rubbish.
struct Reader<'a> {
    bytes: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, MidiFileError> {
        let b = *self.bytes.get(self.i).ok_or(MidiFileError::Truncated)?;
        self.i += 1;
        Ok(b)
    }

    fn peek(&self) -> Result<u8, MidiFileError> {
        self.bytes.get(self.i).copied().ok_or(MidiFileError::Truncated)
    }

    fn u16(&mut self) -> Result<u16, MidiFileError> {
        Ok((u16::from(self.u8()?) << 8) | u16::from(self.u8()?))
    }

    fn u32(&mut self) -> Result<u32, MidiFileError> {
        Ok((u32::from(self.u16()?) << 16) | u32::from(self.u16()?))
    }

    fn tag(&mut self) -> Result<[u8; 4], MidiFileError> {
        Ok([self.u8()?, self.u8()?, self.u8()?, self.u8()?])
    }

    /// A variable-length quantity. **Never inline this into `i += vlen()`** —
    /// `js/midi.js` carries that warning on its own copy, because the index is
    /// read before the call that moves it.
    fn vlen(&mut self) -> Result<u32, MidiFileError> {
        let mut v: u32 = 0;
        loop {
            let b = self.u8()?;
            v = v.saturating_mul(128) + u32::from(b & 0x7f);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
    }

    /// Skip a length-prefixed payload.
    fn skip(&mut self) -> Result<(), MidiFileError> {
        let n = self.vlen()? as usize;
        self.i = self.i.checked_add(n).ok_or(MidiFileError::Truncated)?;
        if self.i > self.bytes.len() {
            return Err(MidiFileError::Truncated);
        }
        Ok(())
    }
}

/// One note as the file held it, before it is mapped onto steps.
struct RawNote {
    on: f64,
    off: f64,
    pitch: u8,
    velocity: u8,
}

/// Parse a type 0 or type 1 SMF and take **the first track that has notes**.
///
/// One track, because a pattern is one track: a multi-track file's other parts
/// have nowhere to go in a single slot, and silently merging them would stack a
/// bass line on top of a hi-hat. Whichever channel those notes were on is
/// dropped too — the destination track's channel is the desk's business, not the
/// file's.
///
/// Anything that does not land on a step becomes **micro-timing**, which is how a
/// file written at any resolution arrives playable rather than quantised flat.
///
/// **Pitches are not clamped**, unlike `js/main.js`, which squeezes them into
/// C2–C8 at the call site. This roll grows a row for a pitch outside that band
/// (`ui::pianoroll::Band`), so clamping would move a note the app can perfectly
/// well draw — and `core::import` already made that call for notes arriving off a
/// box.
pub fn midi_file_to_notes(bytes: &[u8], max_steps: u16) -> Result<Imported, MidiFileError> {
    let mut r = Reader { bytes, i: 0 };
    if bytes.len() < 14 || r.tag()? != *b"MThd" {
        return Err(MidiFileError::NotAMidiFile);
    }
    let header_len = r.u32()?;
    let _format = r.u16()?; // 0 and 1 are handled the same way here
    let ntrks = r.u16()?;
    let division = r.u16()?;
    // Header chunks longer than six bytes are legal and carry nothing this reads.
    r.i = r
        .i
        .checked_add(header_len.saturating_sub(6) as usize)
        .ok_or(MidiFileError::Truncated)?;
    if division & 0x8000 != 0 {
        return Err(MidiFileError::SmpteTimecode);
    }
    if division == 0 {
        return Err(MidiFileError::NotAMidiFile);
    }
    let per16 = f64::from(division) / 4.0;

    let mut found: Option<Vec<RawNote>> = None;
    for _ in 0..ntrks {
        if found.is_some() || r.i >= bytes.len() {
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
        let mut notes: Vec<RawNote> = Vec::new();
        // `open` is `js/midi.js`'s `Map` keyed by pitch, and the in-place replace
        // is what keeps it one: a second note-on for a pitch already sounding
        // overwrites the first and keeps its position in the list, which is what
        // `Map.set` on an existing key does. That position decides the order of
        // the never-released notes appended below.
        let mut open: Vec<(u8, f64, u8)> = Vec::new();
        let mut tick = 0f64;
        let mut status = 0u8;
        while r.i < end {
            tick += f64::from(r.vlen()?);
            let mut s = r.peek()?;
            if s & 0x80 != 0 {
                status = s;
                r.i += 1;
            } else {
                s = status;
            }
            if s == 0xff {
                r.u8()?; // the meta type
                r.skip()?;
                status = 0;
            } else if s == 0xf0 || s == 0xf7 {
                r.skip()?;
                status = 0;
            } else {
                let hi = s & 0xf0;
                let d1 = r.u8()?;
                let d2 = if hi == 0xc0 || hi == 0xd0 { 0 } else { r.u8()? };
                if hi == 0x90 && d2 > 0 {
                    match open.iter_mut().find(|(p, _, _)| *p == d1) {
                        Some(slot) => *slot = (d1, tick, d2),
                        None => open.push((d1, tick, d2)),
                    }
                } else if hi == 0x80 || (hi == 0x90 && d2 == 0) {
                    if let Some(at) = open.iter().position(|(p, _, _)| *p == d1) {
                        let (pitch, on, velocity) = open.remove(at);
                        notes.push(RawNote { on, off: tick, pitch, velocity });
                    }
                }
            }
        }
        r.i = end;
        // Never released: given one step of length, so a file that forgot its
        // note-offs still arrives as music.
        for (pitch, on, velocity) in open {
            notes.push(RawNote { on, off: on + per16, pitch, velocity });
        }
        if !notes.is_empty() {
            found = Some(notes);
        }
    }

    let Some(mut raw) = found else {
        return Ok(Imported { notes: Vec::new(), length_steps: BAR_STEPS, dropped: 0 });
    };
    // Stable, so two notes starting on the same tick keep the order the file put
    // them in — which is the order `Array.prototype.sort` keeps too.
    raw.sort_by(|a, b| a.on.total_cmp(&b.on));

    let max_steps = max_steps.min(MAX_STEPS);
    let total = raw.len();
    let mut notes = Vec::with_capacity(total);
    for n in &raw {
        let f = n.on / per16;
        let step = js_round(f);
        if step < 0.0 || step >= f64::from(max_steps) {
            continue; // past the longest track a box can hold
        }
        notes.push(Note::new(
            step,
            n.pitch,
            js_round((n.off - n.on) / per16).max(1.0),
            clamp_velocity(i32::from(n.velocity)),
            clamp_micro(f - step),
        ));
    }

    // Whole bars, at least one, never past the limit — a 17-step file gets two
    // bars rather than seventeen steps, because a track length is what the box
    // wraps on and a bar is the unit that means anything musically.
    let highest = notes.iter().fold(0.0f64, |m, n| m.max(n.step));
    let wanted = ((highest / f64::from(BAR_STEPS)).floor() as u16 + 1) * BAR_STEPS;
    let length_steps = wanted.max(BAR_STEPS).min(max_steps);
    for n in &mut notes {
        n.len = n.len.min(f64::from(length_steps) - n.step).max(LEN_MIN);
    }

    let dropped = total - notes.len();
    Ok(Imported { notes, length_steps, dropped })
}

/// A filename for an exported pattern, from the pattern's own name.
///
/// Word characters, spaces and dashes survive; everything else goes, which keeps
/// a name straight off a box (`A01 T1`) recognisable while refusing anything that
/// would make a path mean something else. An empty result falls back to
/// `pattern`, as `js/main.js` does — a file called `.mid` is a hidden file on
/// this platform.
pub fn midi_file_name(pattern_name: &str) -> String {
    let cleaned: String = pattern_name
        .trim()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == ' ' || *c == '-')
        .collect();
    let cleaned = cleaned.trim();
    format!("{}.mid", if cleaned.is_empty() { "pattern" } else { cleaned })
}
