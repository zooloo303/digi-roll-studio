// The rules a live take follows, and nothing else.
//
// MIDI_RECORD_DESIGN.md §4.3. This is stage 3 of the three the design splits
// recording into, and it is the only one with an opinion about music: pairing
// note-ons with note-offs, the overdub rule, the polyphony cap, and what a
// release does to a length. It owns no thread, opens no port, reads no pointer
// and knows nothing about egui — every rule below is pinned by a test in
// `core/tests/all/record.rs` that runs in microseconds.
//
// # Why [`PlacedEvent`] lives here and not in `engine`
//
// The design puts it in `engine::record`, and that cannot compile: `Take` is in
// `core`, `core` depends on neither `engine` nor `midi`, and a function in
// `core` cannot name a type from either. So the placed event moves down to the
// crate that consumes it, and `engine::record` — which *can* see both sides —
// builds one and re-exports the type, so `engine::record::PlacedEvent` still
// resolves for anyone following the design. The same argument gives
// [`PlacedKind`] its existence beside `midi::live_input::LiveKind`: two
// two-variant enums either side of a layer boundary, converted once in the one
// crate that sees both.
//
// # What a provisional note is for
//
// A key going down inserts a real [`Note`] into the track *immediately*, one
// step long, and a key coming up sets its length. That is §9 decision 4, taken
// the way it is because the alternative — nothing on screen until the key is
// released — makes a held chord look like a dropped one, and the roll is
// redrawn every frame while the transport runs anyway. The cost is that a take
// abandoned mid-hold leaves one-step notes, which [`Take::close`] states
// plainly rather than tidying away.

use crate::edit_ops::{adopt_step_trig, clamp_micro, clamp_velocity};
use crate::lengths::{snap_len_fine, LEN_MIN};
use crate::model::{Note, Track};

/// The two things a take listens for, after the engine has placed them.
///
/// Not `midi::live_input::LiveKind`, which is the same two variants — see the
/// module header for the crate-graph reason there are two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacedKind {
    NoteOn { pitch: u8, velocity: u8 },
    NoteOff { pitch: u8 },
}

impl PlacedKind {
    pub fn pitch(&self) -> u8 {
        match self {
            PlacedKind::NoteOn { pitch, .. } | PlacedKind::NoteOff { pitch } => *pitch,
        }
    }
}

/// A note-on or note-off after the engine has converted the moment it arrived
/// into a position on the armed track's own grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedEvent {
    pub kind: PlacedKind,
    /// Whole step within the armed track's pattern, 0-based.
    pub step: u64,
    /// Fraction of a step from that grid point, in (−0.5, 0.5]. Zero when
    /// QUANTIZE is on.
    pub micro: f64,
    /// Which trip through the pattern this fell in. The take reports the
    /// highest it saw; nothing else reads it.
    pub pass: u64,
    /// Seconds since the transport started. Kept so a held length is the
    /// difference of two exact moments rather than of two rounded steps — the
    /// difference between a note that lasted 1.03 steps and one that lasted 1.
    pub at: f64,
}

/// Everything the rules below need that is not in the track or the event.
///
/// `step_secs` is **not** in the design's field list and has to be: `at` is in
/// seconds, a length is in steps, and `core` has no tempo and no SCALE to
/// convert between them. The caller has both.
#[derive(Clone, Copy, Debug)]
pub struct TakeOptions {
    /// `DeviceModel::notes_per_trig` — 4 on all three boxes.
    pub notes_per_trig: usize,
    /// The longest a recorded note may be: the armed track's `length_steps`.
    pub max_steps: f64,
    /// Snap new notes to the step and their lengths to whole steps.
    pub quantize: bool,
    /// Seconds per step on the armed track, at this tempo and this SCALE.
    pub step_secs: f64,
}

impl Default for TakeOptions {
    fn default() -> Self {
        Self { notes_per_trig: 4, max_steps: 16.0, quantize: false, step_secs: 0.125 }
    }
}

/// How long a note is while its key is still down. One step, so a held chord
/// reads as a chord on the roll rather than as four slivers.
pub const PROVISIONAL_LEN: f64 = 1.0;

/// What a take did, for the one console line it gets when it closes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TakeReport {
    /// Notes that reached the track. A replacement counts here too — a note
    /// landed either way.
    pub placed: usize,
    /// Arrivals that landed on a `(step, pitch)` that already held a note.
    pub replaced: usize,
    /// Arrivals refused because their step was already at `notes_per_trig`.
    pub dropped_full: usize,
    /// Which steps those were, 0-based and in the order they first filled up.
    /// Named in the console, because "one note was dropped" without saying
    /// where is a message nobody can act on.
    pub full_steps: Vec<u64>,
    /// How many trips through the pattern the take covered: the highest `pass`
    /// seen, plus one. Zero for a take that placed nothing.
    pub passes: u64,
    /// Events the glue could not resolve a track for. Counted by the caller —
    /// nothing in this file can produce one — and reported here so the take has
    /// one place that says what became of every arrival.
    pub dropped_no_target: usize,
}

impl TakeReport {
    /// Whether this take is worth a history step and a console line. A take
    /// that placed nothing is not a take.
    pub fn is_empty(&self) -> bool {
        self.placed == 0
    }

    /// The console line, `target` being how the track should be named —
    /// `"DT2 T3"`.
    ///
    /// Steps are printed 1-based, matching the box's own count and
    /// `track_clip::describe_chord_drops`.
    pub fn line(&self, target: &str) -> String {
        let mut out = format!(
            "REC: {} note{} onto {target} over {} pass{}",
            self.placed,
            if self.placed == 1 { "" } else { "s" },
            self.passes,
            if self.passes == 1 { "" } else { "es" },
        );
        if self.replaced > 0 {
            out.push_str(&format!(" · {} replaced", self.replaced));
        }
        if self.dropped_full > 0 {
            let steps: Vec<String> =
                self.full_steps.iter().map(|s| (s + 1).to_string()).collect();
            out.push_str(&format!(
                " · {} dropped (step{} {} full)",
                self.dropped_full,
                if steps.len() == 1 { "" } else { "s" },
                steps.join(", "),
            ));
        }
        if self.dropped_no_target > 0 {
            out.push_str(&format!(" · {} with no track to land on", self.dropped_no_target));
        }
        out
    }
}

/// A key that is down: which note in the track it put there, and when.
#[derive(Clone, Copy, Debug)]
struct Held {
    id: u32,
    at: f64,
}

/// One take: everything between the first note-on and STOP.
pub struct Take {
    /// Open notes by pitch. An array rather than a map because there are 128
    /// pitches, this is written from the UI thread once per arrival, and a
    /// `None` slot is the cheapest possible "that key is up".
    held: [Option<Held>; 128],
    pub report: TakeReport,
}

impl Default for Take {
    fn default() -> Self {
        Self::new()
    }
}

impl Take {
    pub fn new() -> Self {
        Self { held: [None; 128], report: TakeReport::default() }
    }

    /// Whether any key is still down. The shell asks, because a take that is
    /// mid-chord must not have its undo step committed under it.
    pub fn holding(&self) -> bool {
        self.held.iter().any(Option::is_some)
    }

    /// Take one placed event into `track`. Returns whether the track changed.
    pub fn push(&mut self, ev: PlacedEvent, track: &mut Track, o: &TakeOptions) -> bool {
        match ev.kind {
            PlacedKind::NoteOn { pitch, velocity } => self.note_on(ev, pitch, velocity, track, o),
            PlacedKind::NoteOff { pitch } => self.note_off(ev, pitch, track, o),
        }
    }

    /// End the take.
    ///
    /// **A key still down keeps its provisional length**, which is the honest
    /// answer: nothing released it, so nothing measured it. Tidying those notes
    /// away instead would delete music somebody played, and stretching them to
    /// the stop would invent a length off the moment a button was pressed.
    pub fn close(self, _track: &mut Track) -> TakeReport {
        self.report
    }

    fn note_on(
        &mut self,
        ev: PlacedEvent,
        pitch: u8,
        velocity: u8,
        track: &mut Track,
        o: &TakeOptions,
    ) -> bool {
        let step = ev.step as f64;
        let micro = clamp_micro(ev.micro);
        let velocity = clamp_velocity(i32::from(velocity));
        let provisional = snap_len_fine(PROVISIONAL_LEN, o.max_steps);

        // **Replacement is checked before the cap**, and the order is the whole
        // of decision 3: overdub means playing the same note again on the same
        // step *corrects* it, so re-playing one of the four notes on a full step
        // must not be refused as a fifth.
        if let Some(note) = track
            .notes
            .iter_mut()
            .find(|n| n.pitch == pitch && n.step.floor() == step)
        {
            note.step = step;
            note.micro = micro;
            note.velocity = velocity;
            note.len = provisional;
            let id = note.id;
            self.held[pitch as usize] = Some(Held { id, at: ev.at });
            self.report.replaced += 1;
            self.count_placed(ev.pass);
            return true;
        }

        // The cap, and it is the import's rule verbatim (§4.6 there, decision 6
        // here): keep the first `notes_per_trig`, drop the rest, count it.
        // `step.floor()` rather than equality, because a note nudged off its
        // step by micro-timing is still that step's trig — which is what the box
        // stores and what `export::notes_for_device` reads back.
        let on_step = track.notes.iter().filter(|n| n.step.floor() == step).count();
        if on_step >= o.notes_per_trig {
            self.report.dropped_full += 1;
            if !self.report.full_steps.contains(&ev.step) {
                self.report.full_steps.push(ev.step);
            }
            return false;
        }

        let note = Note::new(step, pitch, provisional, velocity, micro);
        let id = note.id;
        track.notes.push(note);
        // PROB/FILL/COND are per trig on the box, so a note joining an occupied
        // step adopts that step's conditions — exactly as a click, a paste or a
        // drag into that step does. The roll calls this; so does a take.
        adopt_step_trig(&mut track.notes, &[id]);
        self.held[pitch as usize] = Some(Held { id, at: ev.at });
        self.count_placed(ev.pass);
        true
    }

    fn note_off(&mut self, ev: PlacedEvent, pitch: u8, track: &mut Track, o: &TakeOptions) -> bool {
        // **A release with nothing held is ignored**, not an error. Two ways to
        // get one, both ordinary: the key was already down when the take opened,
        // and a keyboard that sends a note-off for a key it never sent an on for
        // after a panic.
        let Some(held) = self.held[pitch as usize].take() else {
            return false;
        };
        let Some(note) = track.notes.iter_mut().find(|n| n.id == held.id) else {
            // The note was deleted under the take — the roll is live while this
            // runs. Nothing to measure.
            return false;
        };

        let raw = (ev.at - held.at) / o.step_secs.max(f64::MIN_POSITIVE);
        let len = if o.quantize {
            // Whole steps, and never zero: a staccato tap is a one-step trig,
            // which is what the box's own LIVE REC gives you.
            snap_len_fine(raw.round().max(1.0), o.max_steps)
        } else {
            snap_len_fine(raw.max(LEN_MIN), o.max_steps)
        };
        if note.len == len {
            return false;
        }
        note.len = len;
        true
    }

    fn count_placed(&mut self, pass: u64) {
        self.report.placed += 1;
        self.report.passes = self.report.passes.max(pass + 1);
    }
}
