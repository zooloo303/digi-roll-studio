//! A Syntakt pattern into a session slot, and one of this session's tracks
//! described as a Syntakt write.
//!
//! The third of these files, after [`crate::import`] + [`crate::export`] for the
//! digis and [`crate::a4_transfer`] for the Analog Four, and separate from both
//! for the reason those two are separate from each other: what arrives is
//! 31,744 bytes whose layout is [`digi_protocol::syntakt_pattern`]'s, mapped
//! from captures rather than from a published struct, and there is no `Spec`.
//!
//! `core` still parses no bytes (PLAN.md §3): every read below is a
//! `syntakt_pattern` call, and this file only moves the answers into and out of
//! the model.
//!
//! # What crosses, and what does not
//!
//! In both directions: **which steps have trigs, what note each plays, and the
//! velocity, length, micro timing and trig condition of each.** Those are the
//! five lanes hardware named on 2026-09-10, and the pattern's swing.
//!
//! **P-locks do not cross, in either direction.** The pool is mapped on this box
//! — `syntakt_pattern` finds it, and the slot sweep proved the sorted insert —
//! but no `param_id` is known except `29 = FLTR RESO`, so a lane would travel as
//! a number nobody can name. Worse for the write: the flow leaves the
//! destination's pool exactly as fetched, so importing lanes into the roll would
//! show the user automation that a write-back then silently fails to carry.
//! They are **counted in the report instead**, so "this pattern has automation
//! you cannot see here" is something the panel can say.
//!
//! # The write is a plan, and the bytes are the ceremony's
//!
//! [`syntakt_track_write`] mirrors [`crate::export::track_write`]: one session
//! track becomes a [`SyntaktTrackWrite`] plus the warnings for everything that
//! could not be said in that vocabulary. It touches no payload. The payload work
//! happens in `digi_protocol::safe_write::syntakt_safe_write_tracks`, which
//! re-fetches the destination (`0x60`) and edits the named tracks into *that* —
//! so the kit, the pool and every unnamed lane are the destination's own, read
//! moments before the send. On this box that matters more than on the others:
//! it stores `0x50` and nothing else, so a pattern write reaches sounds whether
//! it wants to or not.
//!
//! # This box is not in the app's write picker yet
//!
//! `device::SYNTAKT` is still [`PatternRoute::RequestReadOnly`]. Everything here
//! works and is tested; what is missing is the panel dispatch, and promoting the
//! route before that exists would put a button on screen that calls the gen-2
//! flow and fails. So the checks below name the box by slug rather than by
//! route — the route that would describe it does not exist yet, and inventing an
//! unreachable variant to check against would be worse than saying so.

use digi_protocol::pattern::{
    length_byte_to_steps, micro_byte_to_steps, micro_steps_to_byte, steps_to_length_byte,
};
use digi_protocol::safe_write::{SyntaktStep, SyntaktTrackWrite};
use digi_protocol::syntakt_pattern::{self as st, SyntaktCond};

use crate::device::{DeviceId, DeviceModel};
use crate::model::{Note, Pattern, Source};
use crate::session::{PatternRef, Session};

/// The slug that names this box's format, and the one check every entry point
/// here makes. See the module doc for why it is a slug and not a route.
const SLUG: &str = "syntakt";

/// What the box calls each block.
///
/// Twelve tracks, and a thirteenth block whose role is **unresolved** — see
/// `syntakt_pattern`'s header. It gets a number rather than a name, because
/// calling it FX would be a guess printed on a track header where a user would
/// read it as a fact.
pub const TRACK_NAMES: [&str; st::NUM_BLOCKS] = [
    "T1", "T2", "T3", "T4", "T5", "T6", "T7", "T8", "T9", "T10", "T11", "T12", "B13",
];

/// What an import carried, and what it could not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyntaktImportReport {
    pub pattern_name: String,
    pub from_slot: u8,
    pub notes: usize,
    pub tracks_with_notes: usize,
    /// Steps holding a trig that sounds no note. This model holds notes, not
    /// trigs, so they have no representation and are counted rather than
    /// imported. **A write-back leaves them exactly as they are** — see
    /// `syntakt_pattern::set_step` — so this is a display gap and not a
    /// destruction risk.
    pub trigless_dropped: usize,
    /// Trigs whose note lane read `FF` and took the track's default.
    pub notes_from_track_default: usize,
    /// Trigs that arrived carrying a condition. A count, not a loss.
    pub conditions: usize,
    /// Condition bytes past the menu this decoder knows. Not carried.
    pub conditions_off_the_menu: usize,
    /// Parameter-lock lanes the pattern holds, across every track.
    ///
    /// **Not carried, deliberately** — see the module doc. Reported so the
    /// panel can say the pattern has automation the roll is not showing.
    pub plock_lanes_not_carried: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaktImportError {
    NoSuchDevice(DeviceId),
    NoSuchSlot { device: DeviceId, slot: PatternRef },
    /// The destination does not speak this format. A digi slot must not receive
    /// Syntakt bytes: reading them at gen-2 addresses would import plausible
    /// nonsense.
    NotThisBox { expected: &'static str },
    /// The payload does not announce this format's struct version, or is short.
    BadPayload(String),
}

impl std::fmt::Display for SyntaktImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDevice(id) => write!(f, "no device {} in this session", id.0),
            Self::NoSuchSlot { slot, .. } => write!(f, "that box has no slot {}", slot.label()),
            Self::NotThisBox { expected } => {
                write!(f, "that is a Syntakt pattern and this device is a {expected}")
            }
            Self::BadPayload(why) => write!(f, "{why}"),
        }
    }
}

/// A fetched Syntakt payload as a pattern in this app's model.
///
/// `slot` is the dump's index byte, which on this box is also where a write
/// goes back to.
pub fn syntakt_pattern_to_model(
    model: &'static DeviceModel,
    slot: u8,
    payload: &[u8],
) -> Result<(Pattern, SyntaktImportReport), SyntaktImportError> {
    if model.slug != Some(SLUG) {
        return Err(SyntaktImportError::NotThisBox { expected: model.display });
    }
    if !st::looks_like_pattern(payload) {
        return Err(SyntaktImportError::BadPayload(format!(
            "{} bytes that do not announce struct version {} — not a Syntakt pattern",
            payload.len(),
            st::STRUCT_VERSION
        )));
    }

    let mut pattern = Pattern::for_model(model);
    // Nothing in the mapped layout is a pattern name, so the slot is the honest
    // label — the same fallback a digi import takes when the box's name is blank.
    pattern.name = slot_name(slot);
    pattern.swing = st::swing_percent(payload).unwrap_or(st::SWING_STRAIGHT_PERCENT);
    pattern.source = Some(Source {
        device_slug: SLUG.to_owned(),
        bank: slot / 16,
        index: slot % 16,
    });

    let mut report = SyntaktImportReport {
        pattern_name: pattern.name.clone(),
        from_slot: slot,
        ..Default::default()
    };
    let length = st::pattern_length_steps(payload).unwrap_or(st::NUM_STEPS as u8);

    for (t, track_name) in
        TRACK_NAMES.iter().enumerate().take(st::NUM_BLOCKS.min(model.num_tracks))
    {
        let mut notes = Vec::new();
        for trig in st::track_notes(payload, t) {
            // `track_notes` has already resolved an unset lane to the track's
            // default, which is what the box itself sounds; the raw `FF` would
            // drop a step that plays.
            if !trig.locked.note {
                report.notes_from_track_default += 1;
            }
            let (prob, fill, cond) = match st::step_condition(payload, t, trig.step) {
                None if trig.condition_byte != st::NO_LOCK => {
                    report.conditions_off_the_menu += 1;
                    (None, None, None)
                }
                None => (None, None, None),
                Some(c) => {
                    report.conditions += 1;
                    model_condition(&c)
                }
            };
            let mut note = Note::new(
                trig.step as f64,
                trig.note,
                length_byte_to_steps(trig.length_byte),
                trig.velocity,
                micro_byte_to_steps(trig.micro_ticks as u8),
            );
            note.prob = prob;
            note.fill = fill;
            note.cond = cond;
            notes.push(note);
        }
        report.trigless_dropped += (0..st::NUM_STEPS)
            .filter(|&s| !st::step_is_empty(payload, t, s) && !st::plays_note(payload, t, s))
            .count();
        report.notes += notes.len();
        report.tracks_with_notes += usize::from(!notes.is_empty());

        let track = pattern.track_mut(t).expect("built for this model a moment ago");
        track.length_steps = length as u16;
        track.name = (*track_name).to_owned();
        track.notes = notes;
    }

    Ok((pattern, report))
}

/// A01–H16, the box's eight banks of sixteen.
pub fn slot_name(slot: u8) -> String {
    format!("{}{:02}", (b'A' + slot / 16) as char, slot % 16 + 1)
}

/// One box condition as the three fields this model spreads it across.
fn model_condition(cond: &SyntaktCond) -> (Option<u8>, Option<bool>, Option<String>) {
    match cond {
        SyntaktCond::Probability(p) => (Some(*p), None, None),
        SyntaktCond::Logic { name: "FILL", negated } => (None, Some(!negated), None),
        SyntaktCond::Logic { name, negated } => {
            (None, None, Some(if *negated { format!("!{name}") } else { (*name).to_owned() }))
        }
        SyntaktCond::Ratio { a, b } => (None, None, Some(format!("{a}:{b}"))),
    }
}

/// What a Syntakt could not take from one note's trig settings.
enum ConditionLoss {
    /// A probability that is not one of the ladder's 22 rungs.
    Rounded,
    /// A COND this box's menu does not contain.
    NoEquivalent(String),
    /// More than one of PROB, FILL and COND was set. The digis keep three
    /// independent lanes; this box has one, so two of the three cannot go.
    OnlyOneFits,
}

/// One note's trig settings as a Syntakt condition byte.
///
/// **This box holds exactly one of PROB, FILL and COND**, so where a trig has
/// set more than one this has to choose. It takes them in the order the A4's
/// twin does, and for the same reason: a COND names an exact pass, a FILL names
/// a mode, a probability names a chance.
fn condition_for(note: &Note) -> (u8, Option<ConditionLoss>) {
    let set = usize::from(note.prob.is_some())
        + usize::from(note.fill.is_some())
        + usize::from(note.cond.is_some());
    let crowded = (set > 1).then_some(ConditionLoss::OnlyOneFits);

    if let Some(key) = &note.cond {
        let cond = match key.split_once(':') {
            Some((a, b)) => match (a.parse::<u8>(), b.parse::<u8>()) {
                (Ok(a), Ok(b)) => Some(SyntaktCond::Ratio { a, b }),
                _ => None,
            },
            None => {
                let (negated, name) = match key.strip_prefix('!') {
                    Some(rest) => (true, rest),
                    None => (false, key.as_str()),
                };
                st::CONDITION_LOGIC
                    .iter()
                    .find(|n| **n == name)
                    .map(|n| SyntaktCond::Logic { name: n, negated })
            }
        };
        return match cond.as_ref().and_then(st::condition_byte) {
            Some(byte) => (byte, crowded),
            None => (st::NO_LOCK, Some(ConditionLoss::NoEquivalent(key.clone()))),
        };
    }
    if let Some(on) = note.fill {
        let cond = SyntaktCond::Logic { name: "FILL", negated: !on };
        return (st::condition_byte(&cond).unwrap_or(st::NO_LOCK), crowded);
    }
    if let Some(p) = note.prob {
        return match st::condition_byte(&SyntaktCond::Probability(p)) {
            Some(byte) => (byte, crowded),
            // The ladder's rungs are the only percentages the box holds, so an
            // off-ladder value takes the nearest rather than being dropped —
            // and says it did.
            None => {
                let nearest = digi_protocol::a4_conditions::PERCENTAGES
                    .iter()
                    .enumerate()
                    .skip(1)
                    .min_by_key(|(_, v)| v.abs_diff(p))
                    .map(|(i, _)| i as u8)
                    .unwrap_or(st::NO_LOCK);
                (nearest, Some(ConditionLoss::Rounded))
            }
        };
    }
    (st::NO_LOCK, None)
}

/// One track described as a write, plus what could not be said.
#[derive(Debug, Clone)]
pub struct SyntaktTrackExport {
    pub write: SyntaktTrackWrite,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaktExportError {
    NoSuchDevice(DeviceId),
    NoSuchSlot { device: DeviceId, slot: PatternRef },
    NotThisBox { expected: &'static str },
    NoSuchTrack { track: usize, tracks: usize },
    /// A slot past the box's last bank. The dump index is linear 0–127.
    NotOnTheWire(PatternRef),
}

impl std::fmt::Display for SyntaktExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDevice(id) => write!(f, "no device {} in this session", id.0),
            Self::NoSuchSlot { slot, .. } => write!(f, "that box has no slot {}", slot.label()),
            Self::NotThisBox { expected } => {
                write!(f, "that is a Syntakt write and this device is a {expected}")
            }
            Self::NoSuchTrack { track, tracks } => {
                write!(f, "no track {}; this pattern has {tracks}", track + 1)
            }
            Self::NotOnTheWire(slot) => {
                write!(f, "{} is past the last slot this box answers for", slot.label())
            }
        }
    }
}

/// Turn one track of a pattern into a Syntakt write.
///
/// Touches no payload: what comes back is which steps get a trig and what each
/// one plays, which
/// `digi_protocol::safe_write::syntakt_safe_write_tracks` then edits into a
/// freshly fetched destination.
pub fn syntakt_track_write(
    pattern: &Pattern,
    track_index: usize,
    into: PatternRef,
) -> Result<SyntaktTrackExport, SyntaktExportError> {
    let tracks = pattern.tracks().len();
    let track = pattern
        .tracks()
        .get(track_index)
        .ok_or(SyntaktExportError::NoSuchTrack { track: track_index, tracks })?;
    let index = u8::try_from(into.slot()).map_err(|_| SyntaktExportError::NotOnTheWire(into))?;
    if index as usize >= SYNTAKT_WIRE_SLOTS {
        return Err(SyntaktExportError::NotOnTheWire(into));
    }

    let mut steps: Vec<Option<SyntaktStep>> = vec![None; st::NUM_STEPS];
    let mut past_end = 0usize;
    let mut off_grid = 0usize;
    let mut stacked = 0usize;
    let mut rounded_prob = 0usize;
    let mut crowded = 0usize;
    let mut dropped_conds: Vec<String> = Vec::new();

    for note in &track.notes {
        let step = note.step.round();
        if (step - note.step).abs() > f64::EPSILON {
            off_grid += 1;
        }
        if step < 0.0 || step as usize >= st::NUM_STEPS {
            past_end += 1;
            continue;
        }
        let step = step as usize;
        // **One note per trig on this box** (`notes_per_trig: 1`), so a chord
        // drawn in the roll cannot go as a chord. The lowest is kept because
        // that is the note a monophonic voice would sound.
        if let Some(existing) = &steps[step] {
            stacked += 1;
            if existing.note <= note.pitch {
                continue;
            }
        }
        let (condition_byte, loss) = condition_for(note);
        match loss {
            None => {}
            Some(ConditionLoss::Rounded) => rounded_prob += 1,
            Some(ConditionLoss::OnlyOneFits) => crowded += 1,
            Some(ConditionLoss::NoEquivalent(key)) => dropped_conds.push(key),
        }
        steps[step] = Some(SyntaktStep {
            note: note.pitch,
            velocity: note.velocity,
            length_byte: steps_to_length_byte(note.len),
            micro_ticks: micro_steps_to_byte(note.micro) as i8,
            condition_byte,
        });
    }

    let mut warnings = Vec::new();
    if past_end > 0 {
        warnings.push(format!(
            "{past_end} note{} sit{} past step {}, where this box's pattern ends — not sent",
            if past_end == 1 { "" } else { "s" },
            if past_end == 1 { "s" } else { "" },
            st::NUM_STEPS
        ));
    }
    if off_grid > 0 {
        warnings.push(format!(
            "{off_grid} note{} sat between steps and {} rounded onto the nearest one — the box \
             stores a trig on a whole step, with micro timing as its own offset",
            if off_grid == 1 { "" } else { "s" },
            if off_grid == 1 { "was" } else { "were" },
        ));
    }
    if stacked > 0 {
        warnings.push(format!(
            "{stacked} note{} shared a step with another — this box holds one note per trig, so \
             the lowest went and the rest did not",
            if stacked == 1 { "" } else { "s" },
        ));
    }
    if rounded_prob > 0 {
        warnings.push(format!(
            "{rounded_prob} trig{} had a probability this box's ladder does not hold and {} \
             rounded to the nearest of its 22 steps",
            if rounded_prob == 1 { "" } else { "s" },
            if rounded_prob == 1 { "was" } else { "were" },
        ));
    }
    if !dropped_conds.is_empty() {
        let mut named = dropped_conds.clone();
        named.sort();
        named.dedup();
        warnings.push(format!(
            "{} trig{} carry {}, which this box's condition menu does not have — sent without a \
             condition",
            dropped_conds.len(),
            if dropped_conds.len() == 1 { "" } else { "s" },
            named.join(", "),
        ));
    }
    if crowded > 0 {
        warnings.push(format!(
            "{crowded} trig{} set more than one of PROB, FILL and COND — this box holds one of \
             the three per trig, so the condition went and the rest did not",
            if crowded == 1 { "" } else { "s" },
        ));
    }
    // Said on every write rather than only when the destination has lanes,
    // because this side cannot see the destination. See the module doc.
    if !track.plocks.is_empty() {
        warnings.push(format!(
            "{} parameter-lock lane{} drawn on this track — this box's automation does not \
             travel yet, and the destination keeps its own",
            track.plocks.len(),
            if track.plocks.len() == 1 { "" } else { "s" },
        ));
    }

    Ok(SyntaktTrackExport {
        write: SyntaktTrackWrite {
            index,
            track_index,
            steps,
            swing: Some(pattern.swing as f64),
        },
        warnings,
    })
}

/// The slots this box answers dump requests for — eight banks of sixteen.
const SYNTAKT_WIRE_SLOTS: usize = 128;

impl Session {
    /// Land a received Syntakt dump in a slot of this session.
    ///
    /// The mirror of [`Session::import_pattern`], and it keeps the same promise:
    /// **the slot's studio state survives.** Ports, channels, mute and solo are
    /// the session's, not the box's.
    pub fn import_syntakt_pattern(
        &mut self,
        device: DeviceId,
        into: PatternRef,
        slot: u8,
        payload: &[u8],
    ) -> Result<SyntaktImportReport, SyntaktImportError> {
        let model = self
            .device(device)
            .ok_or(SyntaktImportError::NoSuchDevice(device))?
            .model;
        let (mut pattern, report) = syntakt_pattern_to_model(model, slot, payload)?;

        let d = self.device_mut(device).expect("checked just above");
        let old = d
            .pattern(into.slot())
            .ok_or(SyntaktImportError::NoSuchSlot { device, slot: into })?;
        let studio: Vec<_> = old
            .tracks()
            .iter()
            .map(|t| (t.out_port.clone(), t.channel, t.mute, t.solo))
            .collect();
        for (t, (out_port, channel, mute, solo)) in studio.into_iter().enumerate() {
            let track = pattern.track_mut(t).expect("same model, same track count");
            track.out_port = out_port;
            track.channel = channel;
            track.mute = mute;
            track.solo = solo;
        }

        *d.pattern_mut(into.slot()).expect("checked just above") = pattern;
        Ok(report)
    }

    /// One track of one of this session's slots, described as a Syntakt write.
    pub fn syntakt_track_write(
        &self,
        device: DeviceId,
        from: PatternRef,
        track_index: usize,
        into: PatternRef,
    ) -> Result<SyntaktTrackExport, SyntaktExportError> {
        let d = self.device(device).ok_or(SyntaktExportError::NoSuchDevice(device))?;
        if d.model.slug != Some(SLUG) {
            return Err(SyntaktExportError::NotThisBox { expected: d.model.display });
        }
        let pattern = d
            .pattern(from.slot())
            .ok_or(SyntaktExportError::NoSuchSlot { device, slot: from })?;
        syntakt_track_write(pattern, track_index, into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Device, DT2, SYNTAKT};

    /// The captures this decoder was measured against, read straight out of the
    /// evidence folder rather than copied into a fixtures directory — the same
    /// thing `protocol`'s Syntakt suite does, and for the same reason: two
    /// copies of a capture can drift and the one that drifts is the one nothing
    /// compares.
    fn dump(name: &str) -> Vec<u8> {
        let path = format!("{}/../../dumps/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// The end of the 2026-09-10 session: two trigs on track 7, one fully
    /// locked and one at the track's defaults, and 100 other trigs elsewhere.
    fn final_state() -> Vec<u8> {
        dump("syntakt-2026-09-10/stride-H01/vel-64.bin")
    }

    fn session_with_syntakt() -> (Session, DeviceId) {
        let mut s = Session::default();
        let id = s.add_device(Device::new("ST", &SYNTAKT, 16));
        (s, id)
    }

    // --- import --------------------------------------------------------------

    /// The trig counts the box's own screen showed, block by block.
    #[test]
    fn every_blocks_trigs_arrive_as_notes() {
        let (pattern, report) = syntakt_pattern_to_model(&SYNTAKT, 112, &final_state()).unwrap();
        let per_track: Vec<usize> =
            pattern.tracks().iter().map(|t| t.notes.len()).collect();
        assert_eq!(per_track, vec![8, 4, 6, 8, 9, 1, 2, 0, 16, 0, 0, 48, 0]);
        assert_eq!(report.notes, 102);
        assert_eq!(report.tracks_with_notes, 9);
    }

    /// The two trigs hardware named, with the values read off its screen.
    #[test]
    fn a_locked_trig_and_a_default_one_arrive_as_the_box_showed_them() {
        let (pattern, report) = syntakt_pattern_to_model(&SYNTAKT, 112, &final_state()).unwrap();
        let notes = &pattern.track(6).unwrap().notes;
        assert_eq!(notes.iter().map(|n| n.step).collect::<Vec<_>>(), vec![4.0, 15.0]);

        assert_eq!(notes[0].pitch, 64, "E5");
        assert_eq!(notes[0].velocity, 64);
        assert_eq!(notes[0].len, 0.5, "the box showed 1/32");
        assert!((notes[0].micro - 1.0 / 3.0).abs() < 1e-9, "+1/48 of a bar is a third of a step");

        // The second trig locked nothing, so all three lanes read the track's
        // defaults — which is what the box sounds and therefore what arrives.
        assert_eq!(notes[1].pitch, 62, "D5, the track default");
        assert_eq!(notes[1].velocity, 100);
        assert_eq!(notes[1].len, 1.0);
        // Pattern-wide, and worth having a number on: two thirds of this
        // pattern's trigs never locked their note lane, so two thirds of what
        // the roll draws is the track default rather than a stored pitch. An
        // import that read the raw `FF` would have shown an empty roll.
        assert_eq!(report.notes_from_track_default, 66);
        assert_eq!(report.notes, 102);
    }

    #[test]
    fn the_slot_names_the_pattern_and_the_source_records_which_one() {
        let (pattern, report) = syntakt_pattern_to_model(&SYNTAKT, 112, &final_state()).unwrap();
        assert_eq!(pattern.name, "H01");
        assert_eq!(report.pattern_name, "H01");
        assert_eq!(report.from_slot, 112);
        let source = pattern.source.as_ref().unwrap();
        assert_eq!((source.device_slug.as_str(), source.bank, source.index), ("syntakt", 7, 0));
    }

    #[test]
    fn slot_names_run_a01_to_h16() {
        assert_eq!(slot_name(0), "A01");
        assert_eq!(slot_name(15), "A16");
        assert_eq!(slot_name(112), "H01");
        assert_eq!(slot_name(127), "H16");
    }

    #[test]
    fn swing_and_pattern_length_come_across() {
        let (pattern, _) =
            syntakt_pattern_to_model(&SYNTAKT, 0, &dump("syntakt-2026-09-10/stride-H01/swing-60.bin"))
                .unwrap();
        assert_eq!(pattern.swing, 60);
        assert!(pattern.tracks().iter().all(|t| t.length_steps == 64));
    }

    /// A digi slot must not receive these bytes: the offsets are a different
    /// format and would import plausible nonsense.
    #[test]
    fn another_boxs_model_is_refused() {
        let err = syntakt_pattern_to_model(&DT2, 0, &final_state()).unwrap_err();
        assert!(matches!(err, SyntaktImportError::NotThisBox { .. }), "{err}");
    }

    #[test]
    fn bytes_that_are_not_a_pattern_are_refused() {
        let err = syntakt_pattern_to_model(&SYNTAKT, 0, &[0u8; 64]).unwrap_err();
        assert!(err.to_string().contains("not a Syntakt pattern"), "{err}");
    }

    /// The promise every import in this app makes: the desk survives.
    #[test]
    fn an_import_keeps_the_slots_ports_channels_mute_and_solo() {
        let (mut s, id) = session_with_syntakt();
        let slot = PatternRef::from_slot(0);
        {
            let d = s.device_mut(id).unwrap();
            let t = d.pattern_mut(slot.slot()).unwrap().track_mut(0).unwrap();
            t.out_port = Some("Elektron Syntakt".into());
            t.channel = 9;
            t.mute = true;
        }
        s.import_syntakt_pattern(id, slot, 112, &final_state()).unwrap();
        let t = s.device(id).unwrap().pattern(slot.slot()).unwrap().track(0).unwrap();
        assert_eq!(t.out_port.as_deref(), Some("Elektron Syntakt"));
        assert_eq!(t.channel, 9);
        assert!(t.mute);
        assert_eq!(t.notes.len(), 8, "and the pattern still arrived");
    }

    // --- export --------------------------------------------------------------

    /// What went in comes back out. The lane is all 64 steps either way, so the
    /// two ends are comparable without reconciling anything.
    #[test]
    fn a_track_imported_and_described_as_a_write_keeps_its_trigs() {
        let (pattern, _) = syntakt_pattern_to_model(&SYNTAKT, 112, &final_state()).unwrap();
        let export = syntakt_track_write(&pattern, 6, PatternRef::from_slot(112)).unwrap();
        assert_eq!(export.write.steps.len(), st::NUM_STEPS);
        let authored: Vec<usize> = export
            .write
            .steps
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|_| i))
            .collect();
        assert_eq!(authored, vec![4, 15]);
        let first = export.write.steps[4].as_ref().unwrap();
        assert_eq!((first.note, first.velocity, first.micro_ticks), (64, 64, 8));
        assert_eq!(length_byte_to_steps(first.length_byte), 0.5);
        assert_eq!(export.write.index, 112);
        assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    }

    /// **One note per trig on this box.** A chord drawn in the roll cannot go as
    /// one, and the write says so rather than dropping the extra notes quietly.
    #[test]
    fn a_chord_sends_its_lowest_note_and_warns_about_the_rest() {
        let mut pattern = Pattern::for_model(&SYNTAKT);
        let track = pattern.track_mut(0).unwrap();
        track.notes = vec![
            Note::new(0.0, 67, 1.0, 100, 0.0),
            Note::new(0.0, 60, 1.0, 100, 0.0),
            Note::new(0.0, 64, 1.0, 100, 0.0),
        ];
        let export = syntakt_track_write(&pattern, 0, PatternRef::from_slot(0)).unwrap();
        assert_eq!(export.write.steps[0].as_ref().unwrap().note, 60);
        assert!(export.warnings.iter().any(|w| w.contains("one note per trig")), "{:?}", export.warnings);
    }

    #[test]
    fn notes_past_the_last_step_are_left_behind_and_counted() {
        let mut pattern = Pattern::for_model(&SYNTAKT);
        pattern.track_mut(0).unwrap().notes =
            vec![Note::new(0.0, 60, 1.0, 100, 0.0), Note::new(64.0, 60, 1.0, 100, 0.0)];
        let export = syntakt_track_write(&pattern, 0, PatternRef::from_slot(0)).unwrap();
        assert_eq!(export.write.steps.iter().filter(|s| s.is_some()).count(), 1);
        assert!(export.warnings.iter().any(|w| w.contains("past step 64")), "{:?}", export.warnings);
    }

    /// Automation is mapped on this box and does not travel. A write that
    /// silently left it behind would be the worse half of that.
    #[test]
    fn parameter_lock_lanes_are_named_rather_than_dropped_quietly() {
        use crate::model::PLockLane;
        let mut pattern = Pattern::for_model(&SYNTAKT);
        let track = pattern.track_mut(0).unwrap();
        track.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
        track.plocks = vec![PLockLane::new(Some("FLTR RESO".into()), Some(29), None, false, vec![]).unwrap()];
        let export = syntakt_track_write(&pattern, 0, PatternRef::from_slot(0)).unwrap();
        assert!(
            export.warnings.iter().any(|w| w.contains("does not travel yet")),
            "{:?}",
            export.warnings
        );
    }

    // --- conditions ----------------------------------------------------------

    /// Every condition the box can hold survives the model, which spreads one
    /// byte across three fields and has to put it back in one.
    #[test]
    fn a_condition_survives_the_three_fields_this_model_spreads_it_across() {
        for byte in 0..=255u8 {
            let Some(cond) = st::condition(byte) else { continue };
            let (prob, fill, key) = model_condition(&cond);
            let mut note = Note::new(0.0, 60, 1.0, 100, 0.0);
            note.prob = prob;
            note.fill = fill;
            note.cond = key;
            assert_eq!(condition_for(&note).0, byte, "{cond:?} did not come back");
        }
    }

    /// This box holds one of PROB, FILL and COND. A trig with two says so.
    #[test]
    fn a_trig_carrying_two_kinds_of_condition_keeps_one_and_warns() {
        let mut pattern = Pattern::for_model(&SYNTAKT);
        let mut note = Note::new(0.0, 60, 1.0, 100, 0.0);
        note.prob = Some(50);
        note.cond = Some("1ST".into());
        pattern.track_mut(0).unwrap().notes = vec![note];
        let export = syntakt_track_write(&pattern, 0, PatternRef::from_slot(0)).unwrap();
        let step = export.write.steps[0].as_ref().unwrap();
        assert_eq!(st::condition(step.condition_byte), Some(SyntaktCond::Logic { name: "1ST", negated: false }));
        assert!(export.warnings.iter().any(|w| w.contains("more than one")), "{:?}", export.warnings);
    }

    /// The Syntakt's menu has `LST`, which the A4's does not — so a COND the
    /// digis carry reaches this box where it would not reach that one.
    #[test]
    fn lst_reaches_this_box_where_it_would_not_reach_an_analog_four() {
        let mut pattern = Pattern::for_model(&SYNTAKT);
        let mut note = Note::new(0.0, 60, 1.0, 100, 0.0);
        note.cond = Some("!LST".into());
        pattern.track_mut(0).unwrap().notes = vec![note];
        let export = syntakt_track_write(&pattern, 0, PatternRef::from_slot(0)).unwrap();
        let step = export.write.steps[0].as_ref().unwrap();
        assert_eq!(
            st::condition(step.condition_byte),
            Some(SyntaktCond::Logic { name: "LST", negated: true })
        );
        assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    }

    #[test]
    fn a_condition_this_box_has_no_entry_for_is_named_rather_than_guessed() {
        let mut pattern = Pattern::for_model(&SYNTAKT);
        let mut note = Note::new(0.0, 60, 1.0, 100, 0.0);
        note.cond = Some("9:9".into());
        pattern.track_mut(0).unwrap().notes = vec![note];
        let export = syntakt_track_write(&pattern, 0, PatternRef::from_slot(0)).unwrap();
        assert_eq!(export.write.steps[0].as_ref().unwrap().condition_byte, st::NO_LOCK);
        assert!(export.warnings.iter().any(|w| w.contains("9:9")), "{:?}", export.warnings);
    }

    // --- the session's two entry points --------------------------------------

    #[test]
    fn a_write_asked_for_by_slot_comes_back_aimed_at_that_slot() {
        let (mut s, id) = session_with_syntakt();
        s.import_syntakt_pattern(id, PatternRef::from_slot(0), 112, &final_state()).unwrap();
        let export = s
            .syntakt_track_write(id, PatternRef::from_slot(0), 6, PatternRef::from_slot(3))
            .unwrap();
        assert_eq!(export.write.index, 3);
        assert_eq!(export.write.track_index, 6);
        assert_eq!(export.write.steps.iter().filter(|s| s.is_some()).count(), 2);
    }

    #[test]
    fn a_write_for_a_device_that_is_not_this_box_is_refused() {
        let mut s = Session::default();
        let dt2 = s.add_device(Device::new("DT2", &DT2, 16));
        let err = s
            .syntakt_track_write(dt2, PatternRef::from_slot(0), 0, PatternRef::from_slot(0))
            .unwrap_err();
        assert!(matches!(err, SyntaktExportError::NotThisBox { .. }), "{err}");
    }
}
