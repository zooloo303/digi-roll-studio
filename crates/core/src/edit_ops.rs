// Editing maths that wants testing without a canvas.
//
// A faithful port of js/edit-ops.js. The piano roll owns gestures and drawing;
// the app owns the clipboard and the undo stack. Deciding *where* pasted notes
// land is neither, and it carries more edge cases than either wants to hide
// inside a mouse handler — off the end of the pattern, off the top or bottom of
// the drawable rows, and the "you haven't clicked anywhere yet" case.
//
// Per PLAN.md §7 rule 3, this does not "improve" on the original: where the JS
// clamps once for a group and per-note elsewhere, so does this, because those
// differences are the behaviour.

use crate::model::Note;

/// The drawable pitch rows, as js/pianoroll.js labels them: C2 to C8.
pub const PITCH_MIN: u8 = 24;
pub const PITCH_MAX: u8 = 96;

/// The device's LEN scale, injected rather than imported — the roll itself
/// stays device-agnostic. `crate::lengths::snap_len_fine` is the one to pass.
/// Takes (wanted length, room left) and returns a storable length.
pub type SnapLen = fn(f64, f64) -> f64;

/// What the resize helpers read off each entry. Both of them return an array of
/// lengths parallel to their input rather than mutating, so the gesture code
/// assigns them and the maths stays testable. A drag passes entries holding the
/// lengths it *began* with, which is what keeps one delta from compounding on
/// every mousemove.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LenEntry {
    pub step: f64,
    pub len: f64,
}

impl From<&Note> for LenEntry {
    fn from(n: &Note) -> Self {
        Self {
            step: n.step,
            len: n.len,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResizeOpts {
    pub length_steps: f64,
    pub snap_len: Option<SnapLen>,
    /// The floor a drag works to: a whole step for a coarse drag, the shortest
    /// representable length for a fine one.
    pub min_len: f64,
}

impl ResizeOpts {
    /// The JS defaults: no snapping, a one-step floor.
    pub fn coarse(length_steps: f64) -> Self {
        Self {
            length_steps,
            snap_len: None,
            min_len: 1.0,
        }
    }

    pub fn fine(length_steps: f64, snap_len: SnapLen, min_len: f64) -> Self {
        Self {
            length_steps,
            snap_len: Some(snap_len),
            min_len,
        }
    }
}

fn lengths_for(entries: &[LenEntry], want: impl Fn(&LenEntry, usize) -> f64, o: &ResizeOpts) -> Vec<f64> {
    entries
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let room = o.length_steps - n.step;
            let len = want(n, i);
            match o.snap_len {
                Some(snap) => snap(len, room),
                None => o.min_len.max(len.min(room)),
            }
        })
        .collect()
}

/// Dragging one note's edge with a selection behind it.
///
/// Every note moves by the same delta, so a mix of long and short notes stays a
/// mix. The delta is clamped **once for the whole group** — the deepest shrink
/// any member can take, the smallest growth any member has room for — because
/// clamping note by note would flatten exactly the differences this mode exists
/// to preserve. The cost is that the grabbed note stops following the pointer
/// once some other member hits its limit, which is the bargain a group move
/// already makes.
pub fn resize_selection_by(entries: &[LenEntry], delta: f64, o: &ResizeOpts) -> Vec<f64> {
    if entries.is_empty() {
        return Vec::new();
    }
    let floor = entries
        .iter()
        .map(|n| o.min_len - n.len)
        .fold(f64::NEG_INFINITY, f64::max);
    let ceil = entries
        .iter()
        .map(|n| o.length_steps - n.step - n.len)
        .fold(f64::INFINITY, f64::min);
    let d = floor.max(delta.min(floor.max(ceil)));
    lengths_for(entries, |n, _| n.len + d, o)
}

/// The LEN control: every selected note takes the same length.
///
/// Clamped **per note** here, unlike the drag — asking for four steps when one
/// note has two steps of room means that note takes two, rather than holding the
/// whole selection back to the shortest room available.
pub fn set_selection_length(entries: &[LenEntry], len: f64, o: &ResizeOpts) -> Vec<f64> {
    lengths_for(entries, |_, _| len, o)
}

/// A note on the clipboard: no id, because ids are reissued when it lands.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipNote {
    pub step: f64,
    pub pitch: u8,
    pub len: f64,
    pub velocity: u8,
    pub micro: f64,
    pub prob: Option<u8>,
    pub fill: Option<bool>,
    pub cond: Option<String>,
}

impl From<&Note> for ClipNote {
    fn from(n: &Note) -> Self {
        Self {
            step: n.step,
            pitch: n.pitch,
            len: n.len,
            velocity: n.velocity,
            micro: n.micro,
            prob: n.prob,
            fill: n.fill,
            cond: n.cond.clone(),
        }
    }
}

impl ClipNote {
    /// Into a real note, with a fresh id.
    pub fn into_note(self) -> Note {
        let mut n = Note::new(self.step, self.pitch, self.len, self.velocity, self.micro);
        n.prob = self.prob;
        n.fill = self.fill;
        n.cond = self.cond;
        n
    }
}

/// The grid cell the caret sits on, or `None` if nothing has been clicked yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caret {
    pub step: f64,
    pub pitch: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct PasteBounds {
    pub length_steps: f64,
    pub pitch_min: u8,
    pub pitch_max: u8,
}

impl PasteBounds {
    pub fn new(length_steps: f64) -> Self {
        Self {
            length_steps,
            pitch_min: PITCH_MIN,
            pitch_max: PITCH_MAX,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub notes: Vec<ClipNote>,
    pub dropped: usize,
}

/// The note a paste hangs off: earliest step, and among ties the highest pitch.
/// That is the top-left corner of the copied block as it looks on the grid,
/// which is what the caret should line up with.
pub fn clipboard_anchor(clip: &[ClipNote]) -> Option<&ClipNote> {
    clip.iter().fold(None, |best: Option<&ClipNote>, n| match best {
        None => Some(n),
        Some(b) if n.step < b.step || (n.step == b.step && n.pitch > b.pitch) => Some(n),
        keep => keep,
    })
}

/// Where a clipboard's notes go on paste.
///
/// With a caret, the whole block is offset so its anchor lands on the caret,
/// preserving relative timing and pitch; anything whose *start* falls outside the
/// pattern or the drawable rows is dropped rather than clamped, because a clamped
/// note lands somewhere you didn't ask for and quietly stacks on a neighbour.
/// Lengths still clamp to the pattern end — that only shortens a note, it never
/// moves one.
///
/// With no caret (nothing clicked yet) the old absolute-position behaviour
/// stands: notes land back on their source steps.
pub fn place_clipboard(clip: &[ClipNote], caret: Option<Caret>, b: &PasteBounds) -> Placement {
    if clip.is_empty() {
        return Placement {
            notes: Vec::new(),
            dropped: 0,
        };
    }
    let anchor = caret.and(clipboard_anchor(clip));
    let (d_step, d_pitch) = match (caret, anchor) {
        (Some(c), Some(a)) => (c.step - a.step, c.pitch as i32 - a.pitch as i32),
        _ => (0.0, 0),
    };

    let mut notes = Vec::with_capacity(clip.len());
    let mut dropped = 0;
    for c in clip {
        let (step, pitch) = if anchor.is_some() {
            let step = c.step + d_step;
            let pitch = c.pitch as i32 + d_pitch;
            if step < 0.0
                || step >= b.length_steps
                || pitch < b.pitch_min as i32
                || pitch > b.pitch_max as i32
            {
                dropped += 1;
                continue;
            }
            (step, pitch as u8)
        } else {
            (c.step.min(b.length_steps - 1.0), c.pitch)
        };
        notes.push(ClipNote {
            step,
            pitch,
            len: c.len.min(b.length_steps - step),
            ..c.clone()
        });
    }
    Placement { notes, dropped }
}

/// Notes joining an occupied step take that trig's conditions.
///
/// PROB/FILL/COND are per *trig* on the box, so every note sharing a step has to
/// agree — the step-uniformity rule the encoder resolves by lowest pitch when it
/// is broken. The chord tools keep it by stamping chord-mates from their root;
/// this is the same adoption for the ways notes land on a step later: paste, a
/// move, an alt-drag copy, a plain click. The incumbent wins — the trig already
/// exists, the arriving note is joining it. On an empty step an arriving note
/// keeps its own conditions.
///
/// `arriving` names the notes that just landed, by id; conditions are copied
/// *onto* those. Returns how many actually changed, so the caller can say so — a
/// note silently shedding its `2:4` is the surprise this exists to prevent.
pub fn adopt_step_trig(notes: &mut [Note], arriving: &[u32]) -> usize {
    // Resolved before any mutation: an arrival is never its own host, and two
    // arrivals on one empty step must not adopt from each other.
    let hosts: Vec<Option<(Option<u8>, Option<bool>, Option<String>)>> = arriving
        .iter()
        .map(|id| {
            let step = notes.iter().find(|n| n.id == *id)?.step;
            // Lowest-pitch incumbent, to match the note the encoder would believe.
            notes
                .iter()
                .filter(|x| !arriving.contains(&x.id) && x.step == step)
                .min_by_key(|x| x.pitch)
                .map(|h| (h.prob, h.fill, h.cond.clone()))
        })
        .collect();

    let mut changed = 0;
    for (id, host) in arriving.iter().zip(hosts) {
        let Some((prob, fill, cond)) = host else {
            continue;
        };
        let Some(n) = notes.iter_mut().find(|n| n.id == *id) else {
            continue;
        };
        if n.prob != prob || n.fill != fill || n.cond != cond {
            n.prob = prob;
            n.fill = fill;
            n.cond = cond;
            changed += 1;
        }
    }
    changed
}

// --- Velocity ----------------------------------------------------------------
//
// The two ways velocity is set are deliberately different operations, and the
// difference is the whole of the JS's behaviour here:
//
// * the **drag** applies one delta to the velocities the selection began with,
//   so a soft note and a loud note stay soft and loud;
// * the **slider** levels — every selected note takes the same value — because
//   "all the same velocity" is a deliberate act, and it is what a control with
//   one number on it can honestly mean.
//
// So the drag is [`nudge_velocities`] and the slider is a plain assignment
// through [`clamp_velocity`]. Nothing here shares the resize's helpers, for the
// reason below.

/// Velocity 0 is a note-off on the wire, so nothing in this app writes one.
/// `js/pianoroll.js`'s drag and `js/main.js`'s slider both stop at 1.
pub const VEL_MIN: u8 = 1;
pub const VEL_MAX: u8 = 127;

/// Onto the wire's range.
pub fn clamp_velocity(v: i32) -> u8 {
    v.clamp(VEL_MIN as i32, VEL_MAX as i32) as u8
}

/// The velocity drag: one delta over the velocities the drag began with.
///
/// **Clamped per note, unlike [`resize_selection_by`], which clamps once for the
/// whole group.** That difference is the JS's and it is deliberate here (PLAN.md
/// §7 rule 3): `js/pianoroll.js`'s `vel` mode runs
/// `Math.max(1, Math.min(127, it.vel + d))` inside the loop, so a selection
/// dragged into the ceiling *does* flatten against it — 120 and 60 pushed up by
/// 20 land on 127 and 80, not on 127 and 67. The resize clamps once because a
/// note that has run out of pattern cannot grow at all and holding the group
/// back is the only honest answer; velocity has no such shared limit, and
/// stopping the whole selection because one note is already at 127 would make
/// the loudest note in a chord veto the rest.
///
/// Returns a vector parallel to `start`, the same contract the resize helpers
/// keep, so the gesture assigns rather than this mutating.
pub fn nudge_velocities(start: &[u8], delta: i32) -> Vec<u8> {
    start
        .iter()
        .map(|v| clamp_velocity(i32::from(*v) + delta))
        .collect()
}

// --- Micro-timing ------------------------------------------------------------

/// The furthest a note may be nudged off its step, from `js/pianoroll.js`.
///
/// **The boxes hold more than this** — `protocol::micro_steps_to_byte` stores
/// ±23/24 of a step — and the narrower window is the gesture's, not the
/// format's: at ±0.5 a note is on top of its neighbouring step, and past that
/// point the thing being asked for is a *move*. So the drag stops just short of
/// where it would start lying about which step the trig is on, and a note
/// imported from a box carrying a larger offset keeps it untouched.
pub const MICRO_MIN: f64 = -0.49;
pub const MICRO_MAX: f64 = 0.49;

pub fn clamp_micro(micro: f64) -> f64 {
    micro.clamp(MICRO_MIN, MICRO_MAX)
}

// --- Whole-track operations --------------------------------------------------

/// One bar, in steps. Both boxes count sixteen steps to the bar at scale 1, and
/// the Edit panel's duplicate works in bars because that is the unit the
/// gesture is for.
pub const BAR_STEPS: u16 = 16;

/// The longest a track can be, which is the boxes' own limit: 128 steps.
pub const MAX_STEPS: u16 = 128;

/// Copy the last bar onto the end, lengthening the track by a bar.
///
/// `js/main.js`'s `dup`, and the two details that are easy to get wrong are both
/// in it: the notes copied are the ones *at or after* the old last bar's first
/// step, and each copy's length is clipped to the room the new end leaves it —
/// so a two-bar note in the source bar does not hang past the wrap line it was
/// just given.
///
/// Returns `(source bar, destination bar)`, both 1-based for saying out loud, or
/// `None` when the track is already [`MAX_STEPS`] long and there is nowhere to
/// put it.
///
/// One case the JS cannot reach and this can: a trig already **past** the wrap
/// line, which this roll allows and the JS's clamps forbid. Its copy would land
/// at or beyond the new end, so it is dropped rather than pushed with a length
/// the JS's arithmetic would have made zero or negative.
pub fn duplicate_last_bar(track: &mut crate::model::Track) -> Option<(u16, u16)> {
    if track.length_steps >= MAX_STEPS {
        return None;
    }
    // A track shorter than a bar still duplicates: the source is everything it
    // has, which is what `lengthSteps - 16` clamped at zero comes to.
    let from = track.length_steps.saturating_sub(BAR_STEPS);
    let copies: Vec<Note> = track
        .notes
        .iter()
        .filter(|n| n.step >= f64::from(from))
        .cloned()
        .collect();
    track.length_steps = (track.length_steps + BAR_STEPS).min(MAX_STEPS);
    let end = f64::from(track.length_steps);
    for mut note in copies {
        note.reissue_id();
        note.step += f64::from(BAR_STEPS);
        // Clipped against the new end. `js/main.js` writes the same clamp as
        // `Math.min(n.len, p.lengthSteps - n.step - 16)`, which is this with the
        // shift already applied.
        note.len = note.len.min(end - note.step).max(crate::lengths::LEN_MIN);
        if note.step < end {
            track.notes.push(note);
        }
    }
    Some((from / BAR_STEPS + 1, track.length_steps / BAR_STEPS))
}

/// Empty a track of music, keeping everything about it that is not music.
///
/// The line is the JS's, and it is the one worth stating: length, scale, the
/// PROB default, the channel, the port and mute/solo all survive, and **the
/// p-lock lanes do not**. Locks ride on trigs — that is the whole of how the box
/// stores them — so erasing the trigs erases the automation with them, which is
/// exactly what writing this back to the box should do. `js/main.js` says the
/// same thing above its own `clear`.
///
/// Returns whether anything went, so a clear on an empty track leaves no undo
/// step and reports nothing.
pub fn clear_track(track: &mut crate::model::Track) -> bool {
    if track.notes.is_empty() && track.plocks.is_empty() {
        return false;
    }
    track.notes.clear();
    track.plocks.clear();
    true
}
