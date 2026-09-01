//! An Analog Four pattern into a session slot, and one of this session's tracks
//! described as an A4 write.
//!
//! The gen-2 twin of this is [`crate::import`] + [`crate::export`], and the two
//! stay separate files because almost nothing is shared. Those are handed a
//! `PatternKit` decoded with a `Spec`; there is no `Spec` here and never will
//! be. What arrives is 12,974 bytes whose layout is
//! [`digi_protocol::a4_pattern`]'s, mapped from captures rather than from a
//! published struct.
//!
//! `core` still parses no bytes (PLAN.md §3): every read below is an
//! `a4_pattern` call, and this file only moves the answers into and out of the
//! model.
//!
//! # The write is a plan, and the bytes are the ceremony's
//!
//! [`a4_track_write`] mirrors [`crate::export::track_write`]: it turns one
//! session track into an [`A4TrackWrite`] — which steps get a trig and what
//! note each plays — plus the warnings for everything that could not be said in
//! that vocabulary. It touches no payload. The payload work happens in
//! `digi_protocol::safe_write::a4_safe_write_tracks`, which re-fetches the
//! destination slot (`0x64`) and edits the named tracks' lanes into *that* —
//! so the unmapped 10 KB of the message (sounds, p-locks, six unnamed lanes)
//! is the destination's own, read moments before the send.
//!
//! Until 2026-08-31 this file instead carried a `DumpBaseline` — the received
//! dump kept whole on the pattern, 26 KB of hex per slot in the project file —
//! and a rule that you could not send to a slot you had not received from.
//! Both existed because "the A4 cannot be re-fetched", and it can (PLAN.md §10,
//! "The A4 answers dump requests"), so both are gone: the re-fetch is the same
//! guarantee taken fresh instead of stored, which is exactly `safe_write`'s
//! bargain on the digis.
//!
//! # What crosses, and what does not
//!
//! In both directions this carries **which steps have trigs and what note each
//! one plays**. That is the whole of the mapped musical content.
//!
//! Not velocity, not length, not micro-timing, not PROB/FILL/COND, not p-locks.
//! Those exist on the box — four of the six unnamed per-step lanes are the
//! obvious candidates — but "obvious candidate" is not a measurement, and a
//! writer aimed at a lane by guesswork would corrupt whatever actually lives
//! there. [`A4ImportReport`] says so in fields and [`a4_track_write`] leaves
//! the destination's own bytes alone, so a write cannot lose what it never
//! carried.
//!
//! An A4 step holds **one note**. A session track can hold a chord, so an
//! export has to drop notes, and the warning counts the steps that lost
//! something rather than letting them vanish.

use digi_protocol::a4_pattern::{
    effective_note, read_track_trigs, A4Pattern, NUM_STEPS, NUM_TRACKS, PAYLOAD_LEN, TRACK_NAMES,
};
use digi_protocol::safe_write::A4TrackWrite;

use crate::device::{DeviceId, DeviceModel, PatternRoute, A4};
use crate::model::{Note, Pattern, Source};
use crate::session::{PatternRef, Session};

/// The velocity an imported A4 trig gets.
///
/// The format's velocity lane is unmapped, so this is not read from the box and
/// is not a default the box has: it is the model's own middle value, chosen so
/// an imported pattern plays at a sane level. [`A4ImportReport::velocity_guessed`]
/// is set on every import that uses it, which is every import.
pub const IMPORTED_VELOCITY: u8 = 100;

/// The length an imported A4 trig gets, in steps. Same reasoning as
/// [`IMPORTED_VELOCITY`].
pub const IMPORTED_LENGTH: f64 = 1.0;

// --- IN ----------------------------------------------------------------------

/// What an import brought across, and what it could not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A4ImportReport {
    pub pattern_name: String,
    pub from_slot: u8,
    pub notes: usize,
    pub tracks_with_notes: usize,
    /// Steps the box shows a trig on that sound no note — `TrigState::Trigless`.
    /// They have no representation in this model, which holds notes and not
    /// trigs, so they are counted rather than imported. A track of them imports
    /// as empty, and a user who sees "0 notes" needs to know the box disagrees.
    pub trigless_dropped: usize,
    /// Trigs whose note lane read `FF` and took the track's default. Worth
    /// reporting because no fixture has ever contained one: the first import that
    /// sets this is evidence about the box, not about the import.
    pub notes_from_track_default: usize,
    /// Always true. Velocity is not in the mapped layout, so every imported note
    /// carries [`IMPORTED_VELOCITY`] rather than the box's. A field rather than a
    /// doc line because this has to reach a screen.
    pub velocity_guessed: bool,
    /// Always true, same reasoning: length is unmapped and every note is
    /// [`IMPORTED_LENGTH`].
    pub length_guessed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A4ImportError {
    NoSuchDevice(DeviceId),
    NoSuchSlot { device: DeviceId, slot: PatternRef },
    /// The destination does not speak the gen-1 format. A DT2 slot must not
    /// receive A4 bytes: the trig lane offsets are a different format entirely,
    /// and reading them at gen-2 addresses would import plausible nonsense —
    /// which is `ImportError::NotThisBox`'s reasoning, one format over.
    NotThisBox { expected: &'static str },
    /// The payload is not 12,974 bytes. Only reachable from a hand-built
    /// `A4Pattern`; `parse_pattern` refuses one on the way in.
    BadPayload(String),
}

impl std::fmt::Display for A4ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDevice(id) => write!(f, "no device {} in this session", id.0),
            Self::NoSuchSlot { slot, .. } => write!(f, "that box has no slot {}", slot.label()),
            Self::NotThisBox { expected } => write!(
                f,
                "that is an Analog Four pattern and this device is a {expected}"
            ),
            Self::BadPayload(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for A4ImportError {}

/// A received dump as a [`Pattern`], with nothing of the session in it.
///
/// Pure: the same payload always gives the same pattern, note ids aside. The
/// destination slot's own routing is preserved by [`Session::import_a4_pattern`],
/// which is the same split [`crate::import::pattern_from_kit`] makes.
pub fn a4_pattern_to_model(
    model: &'static DeviceModel,
    dump: &A4Pattern,
) -> Result<(Pattern, A4ImportReport), A4ImportError> {
    if model.pattern_route() != PatternRoute::RequestGen1 {
        return Err(A4ImportError::NotThisBox { expected: model.display });
    }
    if dump.payload.len() != PAYLOAD_LEN {
        return Err(A4ImportError::BadPayload(format!(
            "payload is {} bytes, an A4 pattern is {PAYLOAD_LEN}",
            dump.payload.len()
        )));
    }

    let mut pattern = Pattern::for_model(model);
    // The A4's pattern struct carries no name — nothing in the mapped layout is
    // one — so the slot is the honest label. A digi import falls back to the same
    // thing when the box's name is blank.
    pattern.name = dump.slot_name();
    pattern.source = Some(Source {
        device_slug: model.slug.unwrap_or(model.key).to_owned(),
        // The A4's eight banks of sixteen are the slot index's own two halves,
        // which is what `slot_name` decomposes.
        bank: dump.slot / 16,
        index: dump.slot % 16,
    });

    let mut report = A4ImportReport {
        pattern_name: pattern.name.clone(),
        from_slot: dump.slot,
        notes: 0,
        tracks_with_notes: 0,
        trigless_dropped: 0,
        notes_from_track_default: 0,
        velocity_guessed: true,
        length_guessed: true,
    };

    for (t, track_name) in TRACK_NAMES.iter().enumerate().take(NUM_TRACKS.min(model.num_tracks)) {
        let trigs = read_track_trigs(&dump.payload, t)
            .map_err(A4ImportError::BadPayload)?;
        let mut notes = Vec::new();
        for trig in &trigs {
            // `effective_note` rather than `trig.note`: an unset lane means "take
            // the track default", so the raw byte would drop a step the box
            // sounds. `read_track_trigs` has already excluded residue.
            let Some(pitch) = effective_note(&dump.payload, t, trig)
                .map_err(A4ImportError::BadPayload)?
            else {
                report.trigless_dropped += 1;
                continue;
            };
            if trig.note.is_none() {
                report.notes_from_track_default += 1;
            }
            notes.push(Note::new(
                // `Trig::step` is one-based, as the box counts; the model is
                // zero-based, as the roll draws.
                (trig.step - 1) as f64,
                pitch,
                IMPORTED_LENGTH,
                IMPORTED_VELOCITY,
                0.0,
            ));
        }

        report.notes += notes.len();
        report.tracks_with_notes += usize::from(!notes.is_empty());

        let track = pattern.track_mut(t).expect("built for this model a moment ago");
        track.length_steps = NUM_STEPS as u16;
        track.name = (*track_name).to_owned();
        track.notes = notes;
    }

    Ok((pattern, report))
}

// --- OUT ---------------------------------------------------------------------

/// One track's write, and everything about it that did not fit.
///
/// The A4 twin of [`crate::export::TrackExport`], with the same contract: the
/// warnings are written to be shown verbatim, and they are the only way this
/// reports trouble — a chord losing its upper notes and a note past step 64 are
/// both losses a person should agree to rather than errors that stop a write.
#[derive(Debug, Clone)]
pub struct A4TrackExport {
    pub write: A4TrackWrite,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A4ExportError {
    NoSuchDevice(DeviceId),
    NoSuchSlot { device: DeviceId, slot: PatternRef },
    NotThisBox { expected: &'static str },
    NoSuchTrack { track: usize, tracks: usize },
    /// A slot past the box's last bank. The A4's dump index is linear 0–127
    /// (banks A–H), so I01 is not a smaller mistake than P16 — neither has ever
    /// been seen answered.
    NotOnTheWire(PatternRef),
}

impl std::fmt::Display for A4ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDevice(id) => write!(f, "no device {} in this session", id.0),
            Self::NoSuchSlot { slot, .. } => write!(f, "that box has no slot {}", slot.label()),
            Self::NotThisBox { expected } => {
                write!(f, "a {expected} does not take gen-1 pattern writes")
            }
            Self::NoSuchTrack { track, tracks } => write!(
                f,
                "track {} does not exist — this pattern has {tracks}",
                track + 1
            ),
            Self::NotOnTheWire(slot) => write!(
                f,
                "{} is past the last slot this box has — its banks stop at H",
                slot.label()
            ),
        }
    }
}

impl std::error::Error for A4ExportError {}

/// One session track described as an A4 write: 64 steps, each a note or a
/// clear.
///
/// Pure and payload-free — the module doc has the split. Chords resolve to
/// their lowest note, because note order in a track is an artefact of editing
/// history and "the root of the chord" is at least a musical answer a person
/// can predict. A track with no notes is a valid write that clears the track,
/// exactly as a gen-2 [`crate::export::track_write`] with no notes is.
pub fn a4_track_write(
    pattern: &Pattern,
    track_index: usize,
    into: PatternRef,
) -> Result<A4TrackExport, A4ExportError> {
    let track = pattern
        .track(track_index)
        .ok_or(A4ExportError::NoSuchTrack { track: track_index, tracks: pattern.num_tracks() })?;
    if track_index >= NUM_TRACKS {
        return Err(A4ExportError::NoSuchTrack { track: track_index, tracks: NUM_TRACKS });
    }
    let index = into
        .wire_index()
        .filter(|i| usize::from(*i) < A4.wire_slots)
        .ok_or(A4ExportError::NotOnTheWire(into))?;

    let mut steps: Vec<Option<u8>> = vec![None; NUM_STEPS];
    let mut per_step = vec![0usize; NUM_STEPS];
    let mut past_end = 0usize;
    let mut off_grid = 0usize;
    for note in &track.notes {
        let step = note.step.round();
        if !(0.0..NUM_STEPS as f64).contains(&step) {
            past_end += 1;
            continue;
        }
        if step != note.step {
            off_grid += 1;
        }
        let step = step as usize;
        per_step[step] += 1;
        steps[step] = Some(match steps[step] {
            None => note.pitch,
            Some(existing) => existing.min(note.pitch),
        });
    }

    let mut warnings = Vec::new();
    // Counted per *step*, not per surplus note: two chords of three notes is
    // two steps that lost something, which is what a person is looking at on
    // the box.
    let chord_steps = per_step.iter().filter(|&&n| n > 1).count();
    if chord_steps > 0 {
        warnings.push(format!(
            "{chord_steps} step{} hold{} a chord and an A4 step plays one note — only the \
             lowest goes",
            if chord_steps == 1 { "" } else { "s" },
            if chord_steps == 1 { "s" } else { "" },
        ));
    }
    if past_end > 0 {
        warnings.push(format!(
            "{past_end} note{} sit{} past step 64, where this box's pattern ends — not sent",
            if past_end == 1 { "" } else { "s" },
            if past_end == 1 { "s" } else { "" },
        ));
    }
    if off_grid > 0 {
        warnings.push(format!(
            "{off_grid} note{} sat off the step grid and landed on the nearest step — \
             micro-timing is not in the mapped format",
            if off_grid == 1 { "" } else { "s" },
        ));
    }
    // Lanes drawn in the roll on this track. The A4's p-lock pool is mapped as
    // shape only and a write never touches it, so these cannot travel — said
    // here, where every other per-track loss is said, rather than silently
    // arriving as a pattern that plays flat.
    if !track.plocks.is_empty() {
        warnings.push(format!(
            "{} p-lock lane{} not sent — the A4's pool is not in the mapped format, and the \
             destination keeps its own",
            track.plocks.len(),
            if track.plocks.len() == 1 { "" } else { "s" },
        ));
    }

    Ok(A4TrackExport { write: A4TrackWrite { index, track_index, steps }, warnings })
}

impl Session {
    /// Land a received A4 dump in a slot of this session.
    ///
    /// The mirror of [`Session::import_pattern`], and it keeps the same promise:
    /// **the slot's studio state survives.** Ports, channels, mute and solo are
    /// the session's, not the box's, and an import that reset them would
    /// re-route a working desk.
    pub fn import_a4_pattern(
        &mut self,
        device: DeviceId,
        into: PatternRef,
        dump: &A4Pattern,
    ) -> Result<A4ImportReport, A4ImportError> {
        let model = self
            .device(device)
            .ok_or(A4ImportError::NoSuchDevice(device))?
            .model;
        let (mut pattern, report) = a4_pattern_to_model(model, dump)?;

        let d = self.device_mut(device).expect("checked just above");
        let old = d
            .pattern(into.slot())
            .ok_or(A4ImportError::NoSuchSlot { device, slot: into })?;
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

    /// One track of one of this session's slots, described as an A4 write.
    ///
    /// The mirror of [`Session::track_write`], and the end of the write path on
    /// this side of the wire: `a4_track_write` →
    /// `safe_write::a4_safe_write_tracks` → `PatternIo::send_pattern_kit`.
    /// Nothing here sends anything, and whether the box on the cable is the box
    /// this row names stays the caller's identity handshake to refuse on.
    pub fn a4_track_write(
        &self,
        device: DeviceId,
        from: PatternRef,
        track_index: usize,
        into: PatternRef,
    ) -> Result<A4TrackExport, A4ExportError> {
        let d = self.device(device).ok_or(A4ExportError::NoSuchDevice(device))?;
        if d.model.pattern_route() != PatternRoute::RequestGen1 {
            return Err(A4ExportError::NotThisBox { expected: d.model.display });
        }
        let pattern = d
            .pattern(from.slot())
            .ok_or(A4ExportError::NoSuchSlot { device, slot: from })?;
        a4_track_write(pattern, track_index, into)
    }
}

/// The A4 model, for a caller that has a dump and no session row yet.
pub fn a4_model() -> &'static DeviceModel {
    &A4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Device, DT2};

    fn fixture(name: &str) -> A4Pattern {
        let path = format!(
            "{}/../protocol/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        digi_protocol::a4_pattern::parse_pattern(&raw)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn a01() -> A4Pattern {
        fixture("analogfour-A01-2026-08-30.syx")
    }

    fn session_with_a4() -> (Session, DeviceId) {
        let mut s = Session::default();
        let id = s.add_device(Device::new("A4", &A4, 16));
        (s, id)
    }

    // --- import --------------------------------------------------------------

    #[test]
    fn a01_imports_the_trigs_the_box_shows_and_not_the_residue() {
        let (pattern, report) = a4_pattern_to_model(&A4, &a01()).unwrap();
        // The counts the front panel shows, and the ones two earlier trig models
        // got wrong: SYN1 32, SYN4 4, nothing else.
        assert_eq!(pattern.track(0).unwrap().notes.len(), 32, "SYN1");
        assert_eq!(pattern.track(3).unwrap().notes.len(), 4, "SYN4");
        assert_eq!(pattern.track(1).unwrap().notes.len(), 0, "SYN2");
        assert_eq!(report.notes, 36);
        assert_eq!(report.tracks_with_notes, 2);
    }

    #[test]
    fn the_first_trig_lands_on_step_zero_not_step_one() {
        let (pattern, _) = a4_pattern_to_model(&A4, &a01()).unwrap();
        let first = &pattern.track(0).unwrap().notes[0];
        assert_eq!(first.step, 0.0, "Trig::step is one-based and the model is not");
        // A4 in the example's rehearsal output.
        assert_eq!(digi_protocol::a4_pattern::note_name(first.pitch), "A4");
    }

    #[test]
    fn an_import_says_that_velocity_and_length_are_this_apps_and_not_the_boxs() {
        let (_, report) = a4_pattern_to_model(&A4, &a01()).unwrap();
        assert!(report.velocity_guessed);
        assert!(report.length_guessed);
    }

    #[test]
    fn the_tracks_are_named_as_the_box_labels_them() {
        let (pattern, _) = a4_pattern_to_model(&A4, &a01()).unwrap();
        let names: Vec<&str> = pattern.tracks().iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["SYN1", "SYN2", "SYN3", "SYN4", "FX", "CV"]);
    }

    #[test]
    fn a_digi_slot_refuses_an_a4_dump_rather_than_reading_it_at_gen_2_offsets() {
        let err = a4_pattern_to_model(&DT2, &a01()).expect_err("different format entirely");
        assert!(matches!(err, A4ImportError::NotThisBox { .. }));
    }

    #[test]
    fn importing_into_a_session_keeps_the_slots_routing() {
        let (mut s, id) = session_with_a4();
        let slot = PatternRef::new(0, 0);
        {
            let d = s.device_mut(id).unwrap();
            let p = d.pattern_mut(slot.slot()).unwrap();
            p.track_mut(0).unwrap().channel = 9;
            p.track_mut(0).unwrap().mute = true;
        }
        s.import_a4_pattern(id, slot, &a01()).unwrap();
        let t = s.device(id).unwrap().pattern(slot.slot()).unwrap().track(0).unwrap();
        assert_eq!(t.channel, 9, "an import must not re-route a working desk");
        assert!(t.mute);
        assert_eq!(t.notes.len(), 32, "and must still have imported");
    }

    // --- the write plan --------------------------------------------------------

    /// A01 imported into slot A01, which is what every planning test below
    /// starts from — the everyday fetch-edit-send round trip.
    fn imported() -> (Session, DeviceId) {
        let (mut s, id) = session_with_a4();
        s.import_a4_pattern(id, PatternRef::new(0, 0), &a01()).unwrap();
        (s, id)
    }

    #[test]
    fn a_planned_track_names_every_imported_trig_and_no_others() {
        let (s, id) = imported();
        let export = s.a4_track_write(id, PatternRef::new(0, 0), 0, PatternRef::new(0, 0)).unwrap();
        assert_eq!(export.write.index, 0);
        assert_eq!(export.write.track_index, 0);
        assert_eq!(export.write.steps.len(), NUM_STEPS, "all 64, always");
        assert_eq!(
            export.write.steps.iter().filter(|s| s.is_some()).count(),
            32,
            "SYN1's own trig count"
        );
        assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    }

    #[test]
    fn an_empty_track_is_a_deliberate_clear_rather_than_a_refusal() {
        // The per-track panel is the tool for clearing a track on the box —
        // `ui::sync`'s decision 3 skips empty tracks precisely because this
        // path exists for doing it on purpose.
        let (s, id) = imported();
        let export = s.a4_track_write(id, PatternRef::new(0, 0), 1, PatternRef::new(0, 0)).unwrap();
        assert!(export.write.steps.iter().all(|s| s.is_none()), "SYN2 is empty in A01");
    }

    #[test]
    fn a_chord_resolves_to_its_root_and_the_warning_counts_steps_not_notes() {
        let (mut s, id) = imported();
        {
            let d = s.device_mut(id).unwrap();
            let t = d.pattern_mut(0).unwrap().track_mut(1).unwrap();
            for pitch in [64, 60, 67] {
                t.notes.push(Note::new(0.0, pitch, 1.0, 100, 0.0));
            }
        }
        let export = s.a4_track_write(id, PatternRef::new(0, 0), 1, PatternRef::new(0, 0)).unwrap();
        assert_eq!(export.write.steps[0], Some(60), "the root survives, whatever the edit order");
        assert_eq!(export.warnings.len(), 1);
        assert!(export.warnings[0].contains("1 step holds a chord"), "{}", export.warnings[0]);
    }

    #[test]
    fn notes_past_step_64_are_warned_about_rather_than_wrapped() {
        let (mut s, id) = imported();
        {
            let d = s.device_mut(id).unwrap();
            d.pattern_mut(0).unwrap().track_mut(1).unwrap().notes.push(
                Note::new(70.0, 60, 1.0, 100, 0.0),
            );
        }
        let export = s.a4_track_write(id, PatternRef::new(0, 0), 1, PatternRef::new(0, 0)).unwrap();
        assert!(export.write.steps.iter().all(|s| s.is_none()), "nothing wrapped onto a step");
        assert!(export.warnings[0].contains("past step 64"), "{}", export.warnings[0]);
    }

    #[test]
    fn an_off_grid_note_lands_on_the_nearest_step_and_says_so() {
        let (mut s, id) = imported();
        {
            let d = s.device_mut(id).unwrap();
            d.pattern_mut(0).unwrap().track_mut(1).unwrap().notes.push(
                Note::new(4.4, 60, 1.0, 100, 0.0),
            );
        }
        let export = s.a4_track_write(id, PatternRef::new(0, 0), 1, PatternRef::new(0, 0)).unwrap();
        assert_eq!(export.write.steps[4], Some(60));
        assert!(export.warnings[0].contains("micro-timing"), "{}", export.warnings[0]);
    }

    #[test]
    fn a_slot_past_bank_h_is_refused_because_the_box_has_never_answered_one() {
        let (s, id) = imported();
        // I01 — one slot past the A4's 128.
        let err = s
            .a4_track_write(id, PatternRef::new(0, 0), 0, PatternRef::new(8, 0))
            .expect_err("the A4's banks stop at H");
        assert!(matches!(err, A4ExportError::NotOnTheWire(_)));
        assert!(err.to_string().contains("banks stop at H"), "{err}");
    }

    #[test]
    fn a_digi_is_refused_a_gen_1_write_plan() {
        let mut s = Session::default();
        let id = s.add_device(Device::new("DT2", &DT2, 16));
        let err = s
            .a4_track_write(id, PatternRef::new(0, 0), 0, PatternRef::new(0, 0))
            .expect_err("wrong format entirely");
        assert!(matches!(err, A4ExportError::NotThisBox { .. }));
    }
}
