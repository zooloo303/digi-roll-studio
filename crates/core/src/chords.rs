// Chords: the pitch math, the key the roll tints its rows by, and the two
// gestures built on top of both.
//
// Port of `js/chords.js` — 100 lines of pure pitch math — plus the part of
// `js/main.js` that says what the toolbar's settings *mean*: its `chordSpecs`
// (which of the two modes a click is in) and its `harmonize` handler. All three
// are here rather than in `app` because the generator (Phase 7) reads this key
// rather than inventing one, and PLAN.md §6 runs harmony first for exactly that
// reason.
//
// ## Two modes, and the scale menu decides which
//
// * **In scale** — walk up the scale in thirds from the clicked row, so every
//   degree gets its natural quality: ii comes out minor, V with a 7th comes out
//   dominant, vii° diminished, with no chord tables anywhere. A click on an
//   out-of-scale row snaps to the nearest scale tone, preferring the one below.
// * **A fixed quality** — intervals from [`Quality`], whatever row was clicked.
//
// [`QualityChoice::InScale`] with the scale menu off falls back to a plain major
// triad, which is `js/main.js`'s own `diatonic` test read the other way round.
//
// ## What the model already settled, and what that costs
//
// **Four notes to a trig**, which is both boxes' `spec.trig.max_notes` and is
// pinned by a hardware capture in `protocol`. So [`MAX_CHORD_NOTES`] is not a
// taste decision: a fifth note in a voicing is a note the card cannot hold, and
// `encode_track_notes` drops the highest of them on the way out. Every function
// here that puts notes on a step counts what is already there, because the cap is
// per *step* and not per chord — a four-note voicing stamped onto a step that
// already holds a bass note is five notes, and the one that goes is chosen by the
// encoder rather than by anyone looking at the screen.
//
// **Strum is real per-note micro-timing**, so it survives write-back to the box —
// it is the same field Phase 9's cmd+drag moves, and the same one the boxes store
// a signed tick in. That is why the strum control is in *ticks* rather than in the
// JS's 0–100: see [`strum_steps`].
//
// ## Where this deliberately parts from the JS
//
// **A pitch already on the step is never stamped twice.** `js/main.js`'s
// `harmonize` tests `p.notes` for a collision while its additions are still in a
// separate array, so two selected notes one third apart both add the fifth above
// them and the step ends up holding that pitch twice. Two notes at one step and
// pitch are one wasted slot of the four a trig has, and both get written. So the
// check here sees the notes this pass has already added.

use std::collections::BTreeSet;

use digi_protocol::pattern::micro_byte_to_steps;
use serde::{Deserialize, Serialize};

use crate::edit_ops::{adopt_step_trig, clamp_micro, MICRO_MAX};
use crate::model::Note;

/// The most notes one trig can hold, on both boxes — `spec.trig.max_notes` for
/// the DT2 and the DN2 alike, pinned by a Phase 0 capture and asserted against
/// both specs in `tests/chords.rs`. `js/chords.js` names the same number.
pub const MAX_CHORD_NOTES: usize = 4;

/// The scales the roll tints by and the in-scale mode walks, from
/// `js/pianoroll.js`'s `SCALES` table. Eight of them, and the order is that
/// table's order because it is the order the menu is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scale {
    Major,
    Minor,
    Dorian,
    Phrygian,
    Mixolydian,
    HarmonicMinor,
    PentatonicMinor,
    Blues,
}

impl Scale {
    pub const ALL: [Self; 8] = [
        Self::Major,
        Self::Minor,
        Self::Dorian,
        Self::Phrygian,
        Self::Mixolydian,
        Self::HarmonicMinor,
        Self::PentatonicMinor,
        Self::Blues,
    ];

    /// Ascending semitones from the root. Never empty — which is what lets
    /// [`snap_to_scale`] answer for every one of them.
    pub fn intervals(self) -> &'static [i32] {
        match self {
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::PentatonicMinor => &[0, 3, 5, 7, 10],
            Self::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }

    /// As the menu says it, which is as `js/pianoroll.js`'s table keys say it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::Minor => "Minor",
            Self::Dorian => "Dorian",
            Self::Phrygian => "Phrygian",
            Self::Mixolydian => "Mixolydian",
            Self::HarmonicMinor => "Harmonic Minor",
            Self::PentatonicMinor => "Pentatonic Minor",
            Self::Blues => "Blues",
        }
    }
}

/// The twelve pitch classes, sharps only — `js/pianoroll.js`'s `NAMES`, which is
/// also what the roll's key column is built from.
pub const PITCH_CLASSES: [&str; 12] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// A key: a root pitch class and a scale over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyScale {
    /// 0–11.
    pub root: u8,
    pub scale: Scale,
}

/// A chord quality, for the fixed-quality mode. Each is a triad plus the
/// interval its 7th adds.
///
/// **Major takes the major 7th** — the lush pad voicing — which is
/// `js/chords.js`'s call and the reason it gives: a dominant 7th comes out of
/// in-scale mode on degree V naturally, so spending the Major entry on it would
/// lose the one voicing nothing else produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    Major,
    Minor,
    Sus2,
    Sus4,
    Dim,
    Aug,
}

impl Quality {
    pub const ALL: [Self; 6] =
        [Self::Major, Self::Minor, Self::Sus2, Self::Sus4, Self::Dim, Self::Aug];

    pub fn triad(self) -> [i32; 3] {
        match self {
            Self::Major => [0, 4, 7],
            Self::Minor => [0, 3, 7],
            Self::Sus2 => [0, 2, 7],
            Self::Sus4 => [0, 5, 7],
            Self::Dim => [0, 3, 6],
            Self::Aug => [0, 4, 8],
        }
    }

    /// The interval this quality's 7th adds above the root.
    pub fn seventh(self) -> i32 {
        match self {
            Self::Major => 11,
            Self::Minor | Self::Sus2 | Self::Sus4 | Self::Aug => 10,
            Self::Dim => 9,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::Minor => "Minor",
            Self::Sus2 => "Sus2",
            Self::Sus4 => "Sus4",
            Self::Dim => "Dim",
            Self::Aug => "Aug",
        }
    }
}

/// What the quality menu is set to. `js/state.js` keeps this as a string with
/// `'auto'` as one of its values; an enum is the same choice with the unknown
/// case removed — which is why `js/chords.js`'s "fall back to Major for an
/// unknown quality" has no port. The fallback that *is* reachable is
/// [`InScale`](Self::InScale) with no scale set, and it lands on Major too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QualityChoice {
    /// Walk the scale in thirds, so each degree gets its own natural quality.
    InScale,
    Fixed(Quality),
}

impl QualityChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::InScale => "In scale",
            Self::Fixed(q) => q.label(),
        }
    }
}

/// The chord-draw settings: `js/state.js`'s `state.chord`, one field at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChordSettings {
    /// Whether a click on an empty cell stamps a chord rather than one note.
    #[serde(default)]
    pub on: bool,
    #[serde(default = "in_scale")]
    pub quality: QualityChoice,
    #[serde(default)]
    pub seventh: bool,
    /// 0–3, cycled by alt+wheel over the roll. Above the size of the chord it
    /// wraps, which is [`invert`]'s own arithmetic rather than a clamp.
    #[serde(default)]
    pub inversion: u8,
    /// Drop-2: the second note from the top goes down an octave, for an open
    /// voicing.
    #[serde(default)]
    pub spread: bool,
    /// The per-note stagger, in the box's own micro-timing ticks. See
    /// [`strum_steps`] for why this is ticks and not the JS's 0–100.
    #[serde(default)]
    pub strum: u8,
}

fn in_scale() -> QualityChoice {
    QualityChoice::InScale
}

impl Default for ChordSettings {
    fn default() -> Self {
        // Chord draw off, and in-scale selected behind it: the app opens drawing
        // single notes, and turning the mode on should agree with whatever the
        // scale menu says rather than stamping a major triad over a minor key.
        Self {
            on: false,
            quality: QualityChoice::InScale,
            seventh: false,
            inversion: 0,
            spread: false,
            strum: 0,
        }
    }
}

/// The key and the chord settings together: everything the Harmony panel edits.
///
/// **Session-level, and saved.** It is not musical *content* — no note changes
/// when the scale menu moves — so it is deliberately outside `history::Content`
/// and an undo does not take a key change back, exactly as `js/main.js` keeps
/// `state.scale` out of its own snapshots. It is in the project file because the
/// key a session is in is not something to retype every time it is opened, and
/// because Phase 7's Generate panel edits these same two controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Harmony {
    /// The key's root pitch class, 0–11.
    #[serde(default)]
    pub root: u8,
    /// `None` is the menu's `Off`: no tinting, and in-scale mode falls back to a
    /// major triad.
    #[serde(default)]
    pub scale: Option<Scale>,
    #[serde(default)]
    pub chord: ChordSettings,
}

impl Default for Harmony {
    fn default() -> Self {
        // C, no scale: the roll is untinted until someone chooses a key, which is
        // `js/state.js`'s `scale: 'off'`. **Visual only either way** — a scale
        // never restricts what can be drawn, which is the JS's rule and PLAN.md
        // §5 asks for it to stay one.
        Self { root: 0, scale: None, chord: ChordSettings::default() }
    }
}

/// How a row sits in the key, which is the whole of what tinting needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The key's own root, in any octave.
    Root,
    InScale,
    Outside,
}

impl Harmony {
    /// The key, or `None` when the scale menu is off.
    pub fn key(&self) -> Option<KeyScale> {
        self.scale.map(|scale| KeyScale { root: self.root % 12, scale })
    }

    /// Where `pitch` sits in the key, or `None` when there is no key to sit in —
    /// which is the roll drawing no tint at all rather than a row of "outside".
    pub fn row(&self, pitch: u8) -> Option<Row> {
        let key = self.key()?;
        let iv = (i32::from(pitch) - i32::from(key.root)).rem_euclid(12);
        Some(if iv == 0 {
            Row::Root
        } else if key.scale.intervals().contains(&iv) {
            Row::InScale
        } else {
            Row::Outside
        })
    }

    /// The voicing a click on `root_pitch` would stamp, velocities and strum
    /// included — `js/main.js`'s `chordSpecs`, which is the one place the panel's
    /// settings turn into chord options.
    ///
    /// `band` is the roll's own drawable range: a chord tone outside it is dropped
    /// rather than folded back in, because a voicing that quietly re-voices itself
    /// at the edge of the roll is not the chord anyone asked for.
    ///
    /// `taper` eases the lower notes back so the top of the chord sings. Chord
    /// draw wants it; harmonising does not, because there the top note is the
    /// melody and it is already the loudest thing on the step.
    pub fn specs(&self, root_pitch: u8, velocity: u8, taper: bool, band: (u8, u8)) -> Vec<ChordNote> {
        let opts = ChordOpts {
            // In-scale mode needs a scale to be in.
            scale: match self.chord.quality {
                QualityChoice::InScale => self.key(),
                QualityChoice::Fixed(_) => None,
            },
            quality: match self.chord.quality {
                QualityChoice::Fixed(q) => q,
                QualityChoice::InScale => Quality::Major,
            },
            seventh: self.chord.seventh,
            inversion: self.chord.inversion,
            spread: self.chord.spread,
            min: band.0,
            max: band.1,
            max_notes: MAX_CHORD_NOTES,
        };
        let pitches = chord_pitches(root_pitch, &opts);
        voice_chord(
            &pitches,
            &VoiceOpts { velocity, strum: strum_steps(self.chord.strum), taper },
        )
    }

    /// Alt+wheel over the roll: the inversion cycles through all four positions
    /// and wraps, so the gesture never dead-ends — `js/main.js`'s
    /// `(inversion + dir + 4) % 4`.
    pub fn cycle_inversion(&mut self, dir: i32) {
        let n = i32::from(self.chord.inversion) + dir;
        self.chord.inversion = n.rem_euclid(4) as u8;
    }
}

/// The most stagger a strum may ask for, in the boxes' own micro-timing ticks.
///
/// **Three, and the reason is arithmetic rather than taste.** A four-note chord
/// puts three staggers between its bottom and its top, and [`MICRO_MAX`] is 0.49
/// of a step — eleven ticks. At four ticks a four-note chord's top note clamps,
/// which does not merely cap the spread: it puts two notes on the same offset and
/// the strum stops being one. `js/main.js`'s own maximum is 0.12 of a step, which
/// is 2.88 ticks — the same number, arrived at by ear.
pub const STRUM_TICKS_MAX: u8 = 3;

/// The strum control's ticks as a fraction of a step.
///
/// **The control is in ticks, where the JS's is 0–100, and that is deliberate.**
/// The box stores micro-timing as a signed count of 1/24ths of a step, so a
/// stagger that is not a whole number of them is a number the roll shows and the
/// card cannot keep: the JS's 0.12 lands on 2.88 ticks and comes back off a box
/// as 3. Counting in ticks makes the strum drawn in the ghost the strum that will
/// be read back — the same guarantee the Edit panel's length slider gives by
/// stopping only on lengths the hardware stores.
///
/// It goes through `protocol`'s own tick conversion rather than dividing by 24
/// here, so there is one place that knows what a tick is worth.
pub fn strum_steps(ticks: u8) -> f64 {
    micro_byte_to_steps(ticks.min(STRUM_TICKS_MAX))
}

/// How much softer a harmonised note is than the melody note it was built under.
/// `js/main.js`'s 0.85, and the reason for a flat step rather than the chord
/// taper is that the melody has to stay on top of its own harmony.
const HARMONISE_VELOCITY: f64 = 0.85;

/// What [`chord_pitches`] is given. The JS passes an options object with exactly
/// these fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordOpts {
    /// `Some` for in-scale mode, `None` for a fixed quality.
    pub scale: Option<KeyScale>,
    /// Used only when `scale` is `None`.
    pub quality: Quality,
    pub seventh: bool,
    pub inversion: u8,
    /// Drop-2.
    pub spread: bool,
    pub min: u8,
    pub max: u8,
    pub max_notes: usize,
}

impl Default for ChordOpts {
    fn default() -> Self {
        Self {
            scale: None,
            quality: Quality::Major,
            seventh: false,
            inversion: 0,
            spread: false,
            min: 0,
            max: 127,
            max_notes: MAX_CHORD_NOTES,
        }
    }
}

/// One note of a voicing, ready to be stamped: the JS's note spec.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChordNote {
    pub pitch: u8,
    /// Micro-timing offset in fractions of a step, from the strum.
    pub micro: f64,
    pub velocity: u8,
}

/// What [`voice_chord`] is given.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceOpts {
    pub velocity: u8,
    /// The per-note stagger, in fractions of a step. [`strum_steps`] is what
    /// turns the control's ticks into one.
    pub strum: f64,
    pub taper: bool,
}

impl Default for VoiceOpts {
    fn default() -> Self {
        Self { velocity: 100, strum: 0.0, taper: true }
    }
}

/// Where a snap landed: which scale degree, and what to add to the clicked pitch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Snapped {
    index: usize,
    offset: i32,
}

/// Nearest scale tone to an interval-from-root (0–11), searching the octave
/// below and above as well as its own. Ties prefer the tone below.
///
/// **The two extra octaves cannot change the answer for any scale that ships**, and
/// that is worth writing down rather than leaving as an apparent safety net. A
/// wrapped candidate only wins when the nearest tone in the octave itself is more
/// than six semitones away — `iv - v > 6` — and every scale in [`Scale`] has gaps of
/// three semitones at most, so the unwrapped best is never worse than two. The
/// deliberate-bug pass proved it: cutting the search to `[0]` broke nothing in the
/// ported suite, because the JS's own example (B in a minor pentatonic) is found in
/// its own octave and the wrap only ever ties with it.
///
/// It is kept, faithful to `js/chords.js`, because a sparser scale added later — a
/// two- or three-note one, which `js/gen/` has reason to want — would need it
/// immediately, and because a tie that resolves *downwards* is a rule of its own.
/// `the_search_reaches_the_octave_above_for_a_sparse_scale` pins the branch with an
/// interval list no menu can produce, since it is the only way to reach it.
fn snap_to_scale(iv: i32, intervals: &[i32]) -> Snapped {
    // (distance, index, offset, whether the offset points down)
    let mut best: Option<(i32, usize, i32, bool)> = None;
    for (k, v) in intervals.iter().enumerate() {
        for oct in [-12, 0, 12] {
            let off = v + oct - iv;
            let d = off.abs();
            let below = off <= 0;
            let take = match best {
                None => true,
                Some((bd, _, _, b_below)) => d < bd || (d == bd && below && !b_below),
            };
            if take {
                best = Some((d, k, off, below));
            }
        }
    }
    // Every `Scale` has at least five tones, so the `None` case is unreachable;
    // leaving the clicked pitch where it is beats panicking over it.
    let (_, index, offset, _) = best.unwrap_or((0, 0, 0, true));
    Snapped { index, offset }
}

/// Move `times` notes from the bottom of the chord to the top, an octave up.
fn invert(pitches: &[i32], times: u8) -> Vec<i32> {
    let mut out = pitches.to_vec();
    out.sort_unstable();
    if out.is_empty() {
        return out;
    }
    let n = usize::from(times) % out.len();
    for _ in 0..n {
        let lowest = out.remove(0);
        out.push(lowest + 12);
    }
    out.sort_unstable();
    out
}

/// The pitches of one chord, ascending, deduped, inside `[min, max]` and capped
/// at `max_notes`.
///
/// **The cap cannot bind at [`MAX_CHORD_NOTES`], and that is worth saying plainly.**
/// A voicing is three notes or four by construction — `size` is set by `seventh`,
/// inversion rotates and spread transposes, and none of them adds a note — so at four
/// the `take` never drops anything. The deliberate-bug pass proved it: removing the
/// `take` entirely broke nothing. It is kept because `max_notes` is a *parameter*, and
/// a caller passing a smaller one (a triad-only generator, or a box whose trig holds
/// fewer) is the case it is really for — pinned by
/// `the_cap_is_a_parameter_a_caller_can_lower`.
///
/// **The cap that does bind is per *step*, not per chord**, and it lives in
/// [`chord_for_cell`] and [`harmonise`]: four notes on a trig, minus what is already
/// on it. That is the one worth reading twice, because the JS has no equivalent.
///
/// The two modes are [the module header](self)'s two modes. Everything is
/// computed in `i32` and filtered to the range afterwards, so a voicing that
/// reaches below MIDI 0 or above 127 loses the notes that do not fit rather than
/// wrapping into pitches nobody asked for — which is what an unchecked `u8`
/// would do.
pub fn chord_pitches(root_pitch: u8, opts: &ChordOpts) -> Vec<u8> {
    let size = if opts.seventh { 4 } else { 3 };
    let root = i32::from(root_pitch);
    let mut pitches: Vec<i32> = Vec::with_capacity(size);
    match opts.scale {
        Some(key) => {
            let intervals = key.scale.intervals();
            let len = intervals.len() as i32;
            let iv = (root - i32::from(key.root)).rem_euclid(12);
            let snapped = snap_to_scale(iv, intervals);
            let base = root + snapped.offset;
            let k = snapped.index as i32;
            for j in 0..size as i32 {
                // Every other degree, wrapping into the next octave — thirds in
                // the scale rather than thirds in semitones, which is what gives
                // each degree its own quality with no chord table.
                let deg = k + 2 * j;
                let idx = (deg % len) as usize;
                pitches.push(
                    base - intervals[snapped.index] + intervals[idx] + 12 * (deg / len),
                );
            }
        }
        None => {
            pitches.extend(opts.quality.triad().iter().map(|i| root + i));
            if opts.seventh {
                pitches.push(root + opts.quality.seventh());
            }
        }
    }
    let mut pitches = invert(&pitches, opts.inversion);
    if opts.spread && pitches.len() >= 3 {
        let second_from_top = pitches.len() - 2;
        pitches[second_from_top] -= 12;
    }
    // Deduped and sorted in one move, then filtered, then capped — so the notes
    // that survive a cap are the lowest ones, which is the same end of the chord
    // `encode_track_notes` keeps when a step overflows.
    pitches
        .into_iter()
        .collect::<BTreeSet<i32>>()
        .into_iter()
        .filter(|p| *p >= i32::from(opts.min) && *p <= i32::from(opts.max))
        .map(|p| p as u8)
        .take(opts.max_notes)
        .collect()
}

/// Pitches to note specs: the strum stagger and the velocity taper.
///
/// Strum staggers bottom-up, which is a guitarist's downstroke and the direction
/// `js/chords.js` picked. The clamp is [`MICRO_MAX`] — the same limit the roll's
/// own micro-timing drag has, because this writes the same field.
pub fn voice_chord(pitches: &[u8], opts: &VoiceOpts) -> Vec<ChordNote> {
    let top = pitches.len().saturating_sub(1);
    pitches
        .iter()
        .enumerate()
        .map(|(i, &pitch)| ChordNote {
            pitch,
            micro: (i as f64 * opts.strum).min(MICRO_MAX),
            velocity: if opts.taper {
                // `round` then floor at 1: velocity 0 is a note-off on the wire.
                // Rust rounds halves away from zero where JS rounds them up, and
                // the two agree here because every value is positive.
                let factor = 1.0 - 0.07 * (top - i) as f64;
                (f64::from(opts.velocity) * factor).round().clamp(1.0, 127.0) as u8
            } else {
                opts.velocity
            },
        })
        .collect()
}

/// How many notes sit on `step` already.
fn notes_on_step(notes: &[Note], step: f64) -> usize {
    notes.iter().filter(|n| n.step == step).count()
}

/// Whether `pitch` is already on `step`.
fn taken(notes: &[Note], step: f64, pitch: u8) -> bool {
    notes.iter().any(|n| n.step == step && n.pitch == pitch)
}

/// The chord a click on this cell would *actually* stamp: the voicing, minus any
/// pitch the step already holds, capped at the room the trig has left.
///
/// **One function, because the ghost preview and the stamp both read it.** The
/// ghost is this app's only report of what chord draw is about to do — the roll
/// has no status line — so a ghost drawn from the full voicing while the stamp
/// applied a truncated one would be the roll lying about the next click. Same
/// argument as `PianoRoll::press_intent`, which the cursor and the press share
/// for the same reason.
///
/// The notes that survive a cap are the lowest, which is the end of the chord
/// [`chord_pitches`] keeps and the end `encode_track_notes` keeps.
pub fn chord_for_cell(
    notes: &[Note],
    step: f64,
    pitch: u8,
    harmony: &Harmony,
    velocity: u8,
    band: (u8, u8),
) -> Vec<ChordNote> {
    let room = MAX_CHORD_NOTES.saturating_sub(notes_on_step(notes, step));
    harmony
        .specs(pitch, velocity, true, band)
        .into_iter()
        .filter(|c| !taken(notes, step, c.pitch))
        .take(room)
        .collect()
}

/// What one harmonise did, in the terms the panel has to report it in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Harmonised {
    /// The notes added, by id, so the caller can select them — the JS puts them
    /// into the selection alongside the melody, which is what lets an immediate
    /// drag move the whole harmonised phrase.
    pub added: Vec<u32>,
    /// How many selected notes were harmonised.
    pub sources: usize,
    /// Chord tones not added because that pitch was already on the step.
    pub already_there: usize,
    /// Chord tones not added because the step already holds the four notes a trig
    /// can carry. **Reported rather than swallowed**: a note the encoder would
    /// have dropped is an omission, and an omission you can see is a decision.
    pub over_cap: usize,
}

impl Harmonised {
    /// Whether anything changed. `added` empty is the "those chords are already
    /// there" case the JS reports as a non-error.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
    }
}

/// Build a chord under each selected note.
///
/// The melody notes are left exactly as drawn; added notes take their note's
/// length, its micro-timing plus the strum stagger, and come in at
/// [`HARMONISE_VELOCITY`] of its velocity so the line stays on top. Chord-mates
/// join an existing trig, so [`adopt_step_trig`] gives them its PROB/FILL/COND —
/// the step-uniformity rule the encoder relies on.
///
/// `selected` names the melody notes by id; ids that name nothing are skipped
/// rather than refused, because a selection can outlive the note it pointed at.
pub fn harmonise(
    notes: &mut Vec<Note>,
    selected: &[u32],
    harmony: &Harmony,
    band: (u8, u8),
) -> Harmonised {
    let mut out = Harmonised::default();
    // The melody notes as they stand, in track order — which is
    // `roll.selectedNotes()`'s order in the JS. Snapshotted because the loop
    // below pushes into `notes`, and because a note added under one melody note
    // must not then be harmonised itself.
    let sources: Vec<Note> = notes.iter().filter(|n| selected.contains(&n.id)).cloned().collect();
    out.sources = sources.len();
    for source in &sources {
        for spec in harmony.specs(source.pitch, source.velocity, false, band) {
            if spec.pitch == source.pitch {
                continue;
            }
            // Against the live `notes`, so a pitch this pass has already added is
            // seen — see the module header for the JS bug this closes.
            if taken(notes, source.step, spec.pitch) {
                out.already_there += 1;
                continue;
            }
            if notes_on_step(notes, source.step) >= MAX_CHORD_NOTES {
                out.over_cap += 1;
                continue;
            }
            let velocity =
                (f64::from(spec.velocity) * HARMONISE_VELOCITY).round().clamp(1.0, 127.0) as u8;
            let note = Note::new(
                source.step,
                spec.pitch,
                source.len,
                velocity,
                clamp_micro(source.micro + spec.micro),
            );
            out.added.push(note.id);
            notes.push(note);
        }
    }
    adopt_step_trig(notes, &out.added);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_reaches_the_octave_above_for_a_sparse_scale() {
        // The one branch `tests/chords.rs` cannot reach, because it takes an interval
        // list no [`Scale`] provides. Two tones twelve apart: B is one semitone under
        // the root above it and nine over the tone below, so the snap has to look into
        // the next octave to find it. Every shipped scale is too dense for this to
        // arise — see [`snap_to_scale`], and the plant that proved it.
        let sparse = [0, 2];
        let snapped = snap_to_scale(11, &sparse);
        assert_eq!(snapped.offset, 1, "up to the root above, not down nine to the 2");
        assert_eq!(snapped.index, 0);
    }

    #[test]
    fn a_tie_goes_to_the_tone_below() {
        // Which is the rule the octave search exists to *resolve*, and it is reachable:
        // in a minor pentatonic, B sits one semitone above the 10 and one below the
        // root of the octave above.
        let pent = Scale::PentatonicMinor.intervals();
        let snapped = snap_to_scale(11, pent);
        assert_eq!(snapped.offset, -1);
        assert_eq!(pent[snapped.index], 10);
    }
}
