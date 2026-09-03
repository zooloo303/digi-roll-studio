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
//! so the unmapped 10 KB of the message (sounds, six unnamed lanes) is the
//! destination's own, read moments before the send. The pool is no longer among
//! them, and it is the first thing this app writes that moves bytes outside the
//! named tracks' lanes — see
//! [`digi_protocol::a4_plocks::apply_track_plocks`] for why, and what is held
//! invariant instead.
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
//! In both directions it also carries **velocity, length and micro timing**,
//! since 2026-09-01 — the three lanes a hand on a knob named, and all three in
//! the digis' own encodings, so nothing is converted at this boundary that a
//! gen-2 import does not also convert.
//!
//! And the **trig condition**, since the menu behind it was read off the box
//! the same day ([`digi_protocol::a4_conditions`]). It is one enum where this
//! app models three fields, so the translation is explicit in both directions:
//! an A4 percentage becomes `prob`, `FILL`/`!FILL` becomes `fill`, and the
//! logic and ratio entries become `cond` — exactly one of the three, because
//! exactly one is what the box can hold.
//!
//! And **p-locks, since 2026-09-01**, in both directions. A lane travels on the
//! strength of the box's own `param_id` rather than a curated name — three A4
//! ids are measured out of an unknown total, so a table lookup would drop nearly
//! every lane the box has — which means the values carried are raw stored words
//! and the round trip is lossless without a parameter table. What this cannot
//! do is *author* a new lane from a named knob: that needs the id, and the id is
//! what is missing.
//!
//! **And chords, since 2026-09-02.** An A4 step holds one note, and the A4 is
//! not polyphonic per track — but its ARP menu's NO2/NO3/NO4 are three per-step
//! note offsets, and on a polyphonic kit with the arp MOD off the box plays them
//! *with* the note. That is how the factory A01 sounds three-note chords, and it
//! is how a same-step chord travels here: the lowest note is the trig, the rest
//! are offsets from it. Up to four notes, the same ceiling the roll's chord draw
//! already keeps for the digis; a fifth or a note more than 63 semitones above
//! the root is past what the menu holds, and the warning says so. The one thing
//! this cannot promise is the sound: whether the box plays a chord or the root
//! alone is the kit's poly config, which lives in the kit's unmapped tail.
//!
//! Not the unnamed `+459`; a write leaves the destination's own bytes there.

use digi_protocol::a4_pattern::{
    effective_length, effective_note, effective_velocity, read_track_trigs, A4Pattern,
    ARP_OFFSET_MAX, NUM_STEPS, NUM_TRACKS, PAYLOAD_LEN, TRACK_NAMES,
};
use digi_protocol::a4_conditions::{self, A4Cond};
use digi_protocol::a4_kit::{A4Kit, NUM_SOUNDS};
use digi_protocol::a4_plocks::{read_track_plocks, A4Lane, A4LaneWrite};
use digi_protocol::params;
use digi_protocol::pattern::{
    length_byte_to_steps, micro_byte_to_steps, micro_steps_to_byte, steps_to_length_byte,
};
use digi_protocol::safe_write::{A4Step, A4TrackWrite};

use crate::device::{DeviceId, DeviceModel, PatternRoute, A4};
use crate::model::{Note, PLockLane, PatchSound, Pattern, Source};
use crate::session::{PatternRef, Session};

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
    /// Trigs that arrived carrying a trig condition — a percentage, a fill, or
    /// a logic or ratio condition. Not a loss; a count, so the panel can say
    /// the pattern has conditions in it.
    pub conditions: usize,
    /// Notes that arrived as ARP NO2/NO3/NO4 offsets and were drawn as the
    /// upper notes of a chord on their trig's step. Included in `notes`. Worth
    /// saying because they sound only on a polyphonic kit with the arp off, and
    /// a person hearing a single line from a roll full of chords needs to know
    /// where to look.
    pub chord_notes: usize,
    /// Offsets that could not become a note: off the keyboard once added to the
    /// root, or landing on a pitch the step already holds. The box would still
    /// sound the latter as a doubled voice; this model has no doubled voice.
    pub chord_notes_dropped: usize,
    /// P-lock lanes carried in, across every track. An extension is not counted
    /// separately: it is half of a value, not a lane of automation.
    pub plock_lanes: usize,
    /// Of those, the ones holding a value on a step with no trig. Carried
    /// read-only — this model has no trigless lock — and counted because a
    /// track full of them looks like a track full of nothing.
    pub trigless_plock_lanes: usize,
    /// Condition bytes past the end of the menu, which no box has ever been
    /// seen to write.
    ///
    /// Zero on every capture we hold. A nonzero here means the menu is longer
    /// than the four labels read on 2026-09-01 imply — evidence about the box,
    /// not a bad import — so it is counted rather than clamped away.
    pub conditions_off_the_menu: usize,
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
        conditions: 0,
        chord_notes: 0,
        chord_notes_dropped: 0,
        conditions_off_the_menu: 0,
        plock_lanes: 0,
        trigless_plock_lanes: 0,
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
            // One A4 byte becomes exactly one of the three fields a gen-2 trig
            // spreads across three lanes — see `a4_conditions` for why that
            // asymmetry is the shape of the box rather than of this code.
            let (prob, fill, cond) = match trig.condition {
                None => (None, None, None),
                Some(byte) => match a4_conditions::from_byte(byte) {
                    Some(c) => {
                        report.conditions += 1;
                        match c {
                            A4Cond::Probability(p) => (Some(p), None, None),
                            A4Cond::Fill(on) => (None, Some(on), None),
                            other => (None, None, a4_conditions::digi_cond_key(other)),
                        }
                    }
                    None => {
                        report.conditions_off_the_menu += 1;
                        (None, None, None)
                    }
                },
            };
            // All three resolve the way the note does — an unset lane means
            // "take the track's" — and all three are then read with the *gen-2*
            // codecs, because the A4 shares those encodings: length is the same
            // piecewise curve (0x00 = .125, 0x0e = one step, 0x7f = INF, all
            // three read off the box's screen), micro timing is the same signed
            // 1/24-step tick, and velocity is the same 1-127.
            let velocity = effective_velocity(&dump.payload, t, trig)
                .map_err(A4ImportError::BadPayload)?;
            let length = effective_length(&dump.payload, t, trig)
                .map_err(A4ImportError::BadPayload)?;
            let make = |pitch: u8| {
                let mut note = Note::new(
                    // `Trig::step` is one-based, as the box counts; the model is
                    // zero-based, as the roll draws.
                    (trig.step - 1) as f64,
                    pitch,
                    length_byte_to_steps(length),
                    velocity,
                    micro_byte_to_steps(trig.micro_timing as u8),
                );
                note.prob = prob;
                note.fill = fill;
                note.cond = cond.clone();
                note
            };
            notes.push(make(pitch));
            // NO2/NO3/NO4 become the chord's upper notes, sharing everything
            // but pitch with the trig — velocity, length, micro timing and the
            // condition are the trig's on the box, so they are the trig's here.
            let mut pitches = vec![pitch];
            for offset in trig.arp_notes.into_iter().flatten() {
                let sounded = i16::from(pitch) + i16::from(offset);
                let sounded = match u8::try_from(sounded) {
                    Ok(p) if p <= 127 => p,
                    _ => {
                        report.chord_notes_dropped += 1;
                        continue;
                    }
                };
                if pitches.contains(&sounded) {
                    report.chord_notes_dropped += 1;
                    continue;
                }
                pitches.push(sounded);
                notes.push(make(sounded));
                report.chord_notes += 1;
            }
        }

        report.notes += notes.len();
        report.tracks_with_notes += usize::from(!notes.is_empty());

        // The pool, keyed off the steps that actually hold a trig so a lane can
        // say whether it is trigless. `trigs` is the box's own one-based count
        // with residue already excluded, which is the same set the roll draws.
        let live_steps: Vec<usize> = trigs.iter().map(|t| t.step - 1).collect();
        let lanes: Vec<PLockLane> = read_track_plocks(&dump.payload, t)
            .map_err(A4ImportError::BadPayload)?
            .iter()
            .map(|l| a4_lane_to_model(l, t, &live_steps))
            .collect();
        report.plock_lanes += lanes.len();
        report.trigless_plock_lanes += lanes.iter().filter(|l| l.trigless).count();

        let track = pattern.track_mut(t).expect("built for this model a moment ago");
        track.length_steps = NUM_STEPS as u16;
        track.name = (*track_name).to_owned();
        track.notes = notes;
        track.plocks = lanes;
    }

    Ok((pattern, report))
}

/// One pool lane as this app's model holds it.
///
/// The gen-1 twin of `import::lane_to_model`, and it differs in the one place
/// that matters: **nothing is resolved through [`digi_protocol::params`]**.
///
/// **Ninety-two synth parameter ids are measured** (`params::A4_SYNTH_PLOCKS`)
/// and **all thirteen `A4_PARAMS` entries also have a measured scaling**
/// (`a4_scale_probe`, both 2026-09-01), which is what `A4_PARAMS` needs before
/// it will carry a `plock`.
/// So a lane arrives in one of three states, and the difference is the whole
/// shape of this function:
///
/// * **curated** — a synth track, an id in `A4_PARAMS`. It takes the canonical
///   name, its values convert to the display axis, and it is editable.
/// * **named** — a synth track, an id in `A4_SYNTH_PLOCKS` only. Raw stored
///   words, the box's own four-character label, read-only.
/// * **raw** — the FX and CV tracks, whose id space has never been swept. Raw
///   stored words, a hex stand-in, read-only.
///
/// **The track kind is consulted here and nowhere else**, and that is deliberate
/// rather than incidental. `PLockLane` does not carry its track, so if this
/// function did not resolve the id, nothing downstream safely could —
/// `params::plock_id_identifies_parameter` is the rule that makes the rest of
/// the app refuse a bare A4 id, and the canonical `name` set here is the answer
/// it trusts instead.
///
/// For the two uncurated states `values` are **raw stored words** — the coarse
/// byte in the high half and the extension lane's fine byte, 128ths of a display
/// unit, in the low — which is what [`crate::model::PLockLane::values`]
/// documents for a lane off a box whose knob we cannot scale. They go back out
/// unchanged, so those round trips stay lossless with no parameter table at all.
///
/// This block said "three ids measured" and "256ths in the low" until
/// 2026-09-01, and then "none of them is curated" until later the same day. The
/// id sweep corrected the first; OSC TUNE, the only parameter whose fine byte
/// the box shows a number for, corrected the second; and the scaling probe,
/// read against the box's own screen, corrected the third.
fn a4_lane_to_model(lane: &A4Lane, track: usize, live_steps: &[usize]) -> PLockLane {
    // **Only a synth track's id may be resolved, and that is what makes the FX
    // and CV tracks safe.** The id space is per track kind, so `param_by_plock_id`
    // is consulted here — where the track is in hand — and nowhere else; the
    // answer travels as the lane's canonical `name`, which is the only evidence
    // `PLockLane::param` will accept for this box.
    //
    // A track without one keeps raw stored words and stays read-only, byte-exact
    // through the round trip. That is the FX and CV tracks' whole story for this
    // release: their trigs and locks are preserved exactly as fetched and nothing
    // here pretends to know what their knobs are.
    let curated = (track < A4_SYNTH_TRACKS)
        .then(|| params::param_by_plock_id(params::param_table_for("A4"), u16::from(lane.param_id)))
        .flatten();

    // Stored word → display value for a curated lane, raw pass-through for
    // everything else — the same split `import::lane_to_model` makes on the
    // digis, and the reason a lane nothing has scaled still round-trips
    // untouched.
    let desc = curated.map(|p| p.describe(Some("A4")));
    let values = (0..NUM_STEPS).map(|step| {
        let word = lane.word(step);
        match (&desc, word) {
            (Some(d), Some(w)) => d.display_from_stored(w).map(|v| v.max(0) as u16),
            _ => word,
        }
    });

    let modelled = PLockLane::new(
        curated.map(|p| p.name.to_owned()),
        Some(u16::from(lane.param_id)),
        Some("A4".to_owned()),
        lane.has_trigless_values(live_steps),
        values.collect(),
    )
    .expect("a pool lane always has a param id");
    if curated.is_some() {
        // Named from the curated table, which carries its own label — the
        // measured-id stand-in below is for lanes that table cannot name.
        return modelled;
    }

    // **The track kind decides which table names this id, and only a synth
    // track has one.** Measured 2026-09-01: an FX-track lock landed on `0x1a`
    // and `0x29`, both of which name synth parameters — so the id space is per
    // track kind and labelling an FX or CV lane from the synth table would
    // produce a confident wrong name rather than no name. Neither of those two
    // tracks has been swept, so their lanes keep their hex stand-in, which is
    // the honest answer and is visible on screen if anyone meets one.
    //
    // A label is not curation: the lane is named and stays read-only, because
    // editing needs a stored-to-display scaling and only OSC TUNE has one.
    match (track < A4_SYNTH_TRACKS)
        .then(|| params::a4_synth_plock_full_label(lane.param_id))
        .flatten()
    {
        Some(label) => modelled.with_label(label),
        None => modelled,
    }
}

/// The UI's editability rule, restated where `core` can assert it.
///
/// `ui::plocklane::lane_is_editable` is the real one and lives in the app crate,
/// which `core` cannot depend on. Kept in sync by being one line and by the
/// test that uses it naming what it mirrors.
#[cfg(test)]
pub(crate) fn tests_lane_is_editable(lane: &PLockLane) -> bool {
    lane.param().curated && !lane.trigless
}

/// SYN1–SYN4. Track 4 is the FX track and 5 is CV, and neither shares the synth
/// tracks' p-lock parameter numbering — see [`a4_lane_to_model`].
const A4_SYNTH_TRACKS: usize = 4;

/// Model p-lock lanes → the lanes [`digi_protocol::a4_plocks::apply_track_plocks`]
/// writes.
///
/// The gen-1 counterpart to `export::lanes_for_device`, and shorter for the same
/// reason [`a4_lane_to_model`] is: the param id travels, so there is no table to
/// resolve against and no lane dropped for want of a measured p-lock slot.
///
/// Two lanes still do not make it, both reported:
///
/// * one belonging to another box's parameter numbering — a DT2's `0x22` is not
///   this box's, and crossing boxes is copy-track's job, by name;
/// * one that names a parameter this app can play but cannot place. Ninety-two
///   of this box's synth ids are measured (`params::A4_SYNTH_PLOCKS`), and since
///   2026-09-01 thirteen of them are bound to a canonical name in `A4_PARAMS`
///   with a scaling read off the box behind each one — so an authored
///   `filter.cutoff` now travels, where this bullet used to say none could. A
///   lane naming anything outside those thirteen still cannot say which byte the
///   A4 stores it under, and is refused here rather than aimed at a guess.
pub fn a4_lanes_for_write(lanes: &[PLockLane]) -> (Vec<A4LaneWrite>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for lane in lanes {
        if let Some(kind) = lane.device_kind.as_deref() {
            if kind != "A4" {
                warnings.push(format!(
                    "a p-lock lane wasn't sent — it belongs to a {kind}'s parameter numbering, \
                     not the Analog Four's"
                ));
                continue;
            }
        }
        // A lane authored in the roll carries a canonical name and no id, so the
        // id is resolved from the curated table — which is what makes "+ add
        // lane…" and the Generate panel's A4 rows work on this box at all. All
        // thirteen curated parameters have a measured id and scaling
        // (2026-09-01); a name outside that table still refuses below.
        let id = lane.param_id.or_else(|| {
            lane.name
                .as_deref()
                .and_then(|n| params::param_by_name(params::param_table_for("A4"), n))
                .and_then(|p| p.plock)
                .map(|pl| pl.id)
        });
        let Some(id) = id else {
            warnings.push(format!(
                "the {} lane wasn't sent — digi-roll can play that parameter over MIDI but \
                 hasn't measured which byte an A4 pattern stores it under, so it can't write \
                 it into the pool",
                lane.name.as_deref().unwrap_or("unnamed"),
            ));
            continue;
        };
        let Ok(param_id) = u8::try_from(id) else {
            // Unreachable from a box, whose ids are bytes; this is the
            // hand-edited project file, where truncating would aim the lane at a
            // real parameter, silently and wrongly.
            warnings.push(format!(
                "a p-lock lane wasn't sent — parameter number {id} is past the 255 an A4 lane \
                 header can hold"
            ));
            continue;
        };
        // Display value → stored word for a curated lane; raw words straight
        // through for everything else. The `max(0)` is `stored_from_display`'s
        // own clamp reaching the u16 axis the pool holds.
        //
        // **A curated lane loses the box's fine byte**, which is the same
        // accepted loss `ParamDesc::display_from_stored` documents for the
        // digis: the box records sub-display-unit resolution from a knob landing
        // between integers, and this app's axis is integers. It costs 1/128 of
        // one display unit on a lane the user is editing, and it is why an
        // *unnamed* lane is still passed through untouched rather than routed
        // through the same conversion.
        let desc = lane.param();
        let values: Vec<Option<u16>> = if desc.curated {
            lane.values
                .iter()
                .map(|v| v.and_then(|d| desc.stored_from_display(f64::from(d))))
                .collect()
        } else {
            lane.values.clone()
        };
        out.push(A4LaneWrite::new(param_id, values));
    }
    (out, warnings)
}

// --- PATCH NAMES -------------------------------------------------------------

/// What the box's kit calls each of this model's tracks' sounds.
///
/// The gen-1 twin of `import::patch_sound_for_track`, and it exists for the one
/// reason that function cannot serve here: naming a track's sound on a digi
/// goes through a [`digi_protocol::pattern::PatternKit`] decoded with a `Spec`,
/// and the A4 has neither. It has a 2,410-byte kit dump with four sound
/// containers in it (`digi_protocol::a4_kit`), which is all a patch-names read
/// ever needed.
///
/// **Four sounds against six tracks, and the extra two are not empty — they are
/// a different kind of thing.** SYN1–SYN4 have sounds; the FX and CV tracks are
/// the sequencer's and the kit holds nothing for them. That is
/// [`PatchSound::NoSound`], added for exactly this, and not
/// [`PatchSound::Unnamed`] — an unnamed slot is one a later read might name,
/// and no read will ever name these two.
///
/// Returns one entry per track of `model`, which is what
/// [`Session::apply_patch_sounds`] requires and refuses without. A model with
/// more tracks than the A4's six would take `NoSound` for the surplus rather
/// than running off the end of the kit; nothing constructs one today, and the
/// arithmetic is written so that it could not be the interesting failure if
/// something ever did.
pub fn a4_patch_sounds(model: &DeviceModel, kit: &A4Kit) -> Vec<PatchSound> {
    (0..model.num_tracks)
        .map(|t| match kit.sound_name(t) {
            None => PatchSound::NoSound,
            Some("") => PatchSound::Unnamed,
            Some(name) => PatchSound::Named(name.to_owned()),
        })
        .collect()
}

/// The sounds a kit holds, as tracks. `NUM_SOUNDS` is re-stated here because
/// this module's own arithmetic depends on it and a silent change to the kit
/// format should break a test in this crate too, not only in `protocol`.
pub const A4_KIT_SOUNDS: usize = NUM_SOUNDS;

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
/// Pure and payload-free — the module doc has the split. A chord goes as its
/// lowest note plus ARP NO2/NO3/NO4 offsets, ascending — the lowest is the trig
/// because note order in a track is an artefact of editing history and "the
/// root of the chord" is at least a musical answer a person can predict, and
/// the trig's velocity, length and condition are that note's, since taking the
/// lowest pitch but some other note's velocity would author a trig neither note
/// describes. A track with no notes is a valid write that clears the track,
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

    let mut per_step: Vec<Vec<&Note>> = vec![Vec::new(); NUM_STEPS];
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
        per_step[step as usize].push(note);
    }

    let mut steps: Vec<Option<A4Step>> = vec![None; NUM_STEPS];
    let mut rounded_prob = 0usize;
    let mut crowded = 0usize;
    let mut dropped_conds: Vec<String> = Vec::new();
    let mut over_four = 0usize;
    let mut out_of_reach = 0usize;
    for (step, notes) in per_step.iter_mut().enumerate() {
        if notes.is_empty() {
            continue;
        }
        // Lowest first, one of each pitch: the box has no doubled voice, and a
        // step holding the same pitch twice is an editing accident, not a chord.
        notes.sort_by_key(|n| n.pitch);
        notes.dedup_by_key(|n| n.pitch);
        let root = notes[0];
        // The gen-2 codecs, because the A4 shares the encodings — the same
        // functions `core::export` hands a Digitakt II note to. The condition
        // is the root's; notes on one step agree on it in this model, so reading
        // it once is reading it for the chord.
        let (condition, loss) = a4_condition_for(root);
        match loss {
            Some(ConditionLoss::Rounded) => rounded_prob += 1,
            Some(ConditionLoss::NoEquivalent(key)) => dropped_conds.push(key),
            Some(ConditionLoss::OnlyOneFits) => crowded += 1,
            None => {}
        }
        // The upper notes as NO2/NO3/NO4, ascending. The root is the lowest, so
        // an offset is never negative and the only edge is the menu's top.
        let mut arp_notes = [None; 3];
        let mut filled = 0usize;
        for upper in &notes[1..] {
            let offset = upper.pitch - root.pitch;
            if offset > ARP_OFFSET_MAX as u8 {
                out_of_reach += 1;
                continue;
            }
            if filled == arp_notes.len() {
                over_four += 1;
                continue;
            }
            arp_notes[filled] = Some(offset as i8);
            filled += 1;
        }
        steps[step] = Some(A4Step {
            note: root.pitch,
            velocity: root.velocity,
            length: steps_to_length_byte(root.len),
            micro_timing: micro_steps_to_byte(root.micro) as i8,
            condition,
            arp_notes,
        });
    }

    let mut warnings = Vec::new();
    if over_four > 0 {
        warnings.push(format!(
            "{over_four} note{} sat fifth or higher in a chord — the A4 plays a note plus \
             NO2–NO4, four at most, so the highest did not go",
            if over_four == 1 { "" } else { "s" },
        ));
    }
    if out_of_reach > 0 {
        warnings.push(format!(
            "{out_of_reach} note{} sat more than 63 semitones above the chord's root, past \
             what NO2–NO4 can reach — not sent",
            if out_of_reach == 1 { "" } else { "s" },
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
            "{off_grid} note{} sat between steps and {} rounded onto the nearest one — the box \
             stores a trig on a whole step, with micro-timing as its own offset",
            if off_grid == 1 { "" } else { "s" },
            if off_grid == 1 { "was" } else { "were" },
        ));
    }
    if rounded_prob > 0 {
        warnings.push(format!(
            "{rounded_prob} trig{} had a probability the A4's ladder does not hold and {} \
             rounded to the nearest of its 22 steps",
            if rounded_prob == 1 { "" } else { "s" },
            if rounded_prob == 1 { "was" } else { "were" },
        ));
    }
    if !dropped_conds.is_empty() {
        let mut named: Vec<String> = dropped_conds.clone();
        named.sort();
        named.dedup();
        warnings.push(format!(
            "{} trig{} carry {}, which the A4's TRC menu does not have — sent without a \
             condition",
            dropped_conds.len(),
            if dropped_conds.len() == 1 { "" } else { "s" },
            named.join(", "),
        ));
    }
    if crowded > 0 {
        warnings.push(format!(
            "{crowded} trig{} set more than one of PROB, FILL and COND — the A4 holds one of \
             the three per trig, so the condition went and the rest did not",
            if crowded == 1 { "" } else { "s" },
        ));
    }
    // Lanes drawn in the roll on this track. Since 2026-09-01 these travel:
    // `a4_plocks::apply_track_plocks` is the encoder and this is where the model
    // is put into its vocabulary. What it cannot say is said here, where every
    // other per-track loss is said.
    let (plocks, lane_warnings) = a4_lanes_for_write(&track.plocks);
    warnings.extend(lane_warnings);

    Ok(A4TrackExport {
        // **`Some`, always — including for a track with no lanes.** This says
        // "the track's lanes are the truth", which is the same contract
        // `export::track_write` gives a digi, and it is what makes deleting the
        // last lane in the roll something a write can express. `None` would mean
        // "leave the pool alone" and there would then be no way to clear one.
        //
        // The cost is on the other side and it is worth naming: a project file
        // written before this date holds A4 patterns whose `plocks` are empty
        // because the *import* did not carry them, and writing one of those back
        // removes lanes the user was never shown. `apply_track_plocks` warns on
        // exactly that shape — a track asking for nothing over a destination
        // that has something — which is the one signature that separates it from
        // an ordinary deletion.
        write: A4TrackWrite { index, track_index, steps, plocks: Some(plocks) },
        warnings,
    })
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

/// What an A4 could not take from one note's trig settings.
enum ConditionLoss {
    /// A probability that is not one of the ladder's 22 rungs.
    Rounded,
    /// A COND the A4's menu does not contain — `LST`, or any negated ratio.
    NoEquivalent(String),
    /// More than one of PROB, FILL and COND was set. The digis keep three
    /// independent lanes; the A4 has one knob, so two of the three cannot go.
    OnlyOneFits,
}

/// One note's trig settings as an A4 condition byte.
///
/// **The A4 holds exactly one of PROB, FILL and COND**, so where a gen-2 trig
/// has set more than one this has to choose. It takes them in order of how
/// specifically they describe *when* a trig fires — a COND names an exact pass,
/// a FILL names a mode, a probability names a chance — and reports that it had
/// to, rather than picking silently.
fn a4_condition_for(note: &Note) -> (Option<u8>, Option<ConditionLoss>) {
    let set = usize::from(note.prob.is_some())
        + usize::from(note.fill.is_some())
        + usize::from(note.cond.is_some());
    let crowded = (set > 1).then_some(ConditionLoss::OnlyOneFits);

    if let Some(key) = &note.cond {
        return match a4_conditions::from_digi_cond_key(key) {
            // `to_byte` can still refuse — a ratio outside 1:2..8:8 parses as a
            // ratio and is not on the menu — so both refusals land here.
            Some(cond) => match a4_conditions::to_byte(cond) {
                Some(byte) => (Some(byte), crowded),
                None => (None, Some(ConditionLoss::NoEquivalent(key.clone()))),
            },
            None => (None, Some(ConditionLoss::NoEquivalent(key.clone()))),
        };
    }
    if let Some(on) = note.fill {
        let byte = a4_conditions::to_byte(A4Cond::Fill(on)).expect("FILL is on the menu");
        return (Some(byte), crowded);
    }
    if let Some(prob) = note.prob {
        let nearest = a4_conditions::nearest_percentage(prob);
        let byte = a4_conditions::to_byte(A4Cond::Probability(nearest))
            .expect("the nearest rung is by construction a rung");
        let loss = if nearest == prob { crowded } else { Some(ConditionLoss::Rounded) };
        return (Some(byte), loss);
    }
    (None, None)
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
        // got wrong: SYN1 32 trigs, SYN4 4, nothing else. SYN1's 32 trigs carry
        // 57 NO2/NO3 offsets between them, so the roll draws 89 notes there.
        let syn1 = pattern.track(0).unwrap();
        assert_eq!(syn1.notes.iter().filter(|n| n.step as usize % 2 == 0).count(), 89, "SYN1");
        assert_eq!(syn1.notes.len(), 89, "SYN1");
        assert_eq!(pattern.track(3).unwrap().notes.len(), 4, "SYN4");
        assert_eq!(pattern.track(1).unwrap().notes.len(), 0, "SYN2");
        assert_eq!(report.notes, 93);
        assert_eq!(report.chord_notes, 57);
        assert_eq!(report.chord_notes_dropped, 0);
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

    /// A01 SYN1 was played in rather than stepped in, so its velocity and
    /// length lanes hold what a hand played — and this is the assertion that
    /// they now *arrive*. Before 2026-09-01 every note of this import came in
    /// at 100 and one step, and the panel said so in a caution line.
    #[test]
    fn an_import_brings_the_boxs_own_velocity_and_length_across() {
        let (pattern, report) = a4_pattern_to_model(&A4, &a01()).unwrap();
        let syn1 = pattern.track(0).unwrap();
        assert!(syn1.notes.iter().all(|n| n.velocity == 127), "recorded at full velocity");
        // 0x1b and 0x1a on the box: just under two steps, and the seven
        // shorter ones really are shorter — a sixteenth of a step shorter,
        // which is the resolution the curve has up there and exactly what a
        // played-in phrase looks like.
        // One length per trig: a chord's upper notes share their trig's.
        let mut seen = std::collections::HashSet::new();
        let lengths: Vec<f64> =
            syn1.notes.iter().filter(|n| seen.insert(n.step as usize)).map(|n| n.len).collect();
        assert_eq!(lengths.len(), 32);
        assert_eq!(lengths.iter().filter(|&&l| l == 1.8125).count(), 25);
        assert_eq!(lengths.iter().filter(|&&l| l == 1.75).count(), 7);

        // SYN4 was recorded too, and at its own length: 0x3e is eight steps,
        // which is also the value SYN4's per-track default carries.
        let syn4 = pattern.track(3).unwrap();
        assert!(syn4.notes.iter().all(|n| n.velocity == 127), "SYN4 recorded at full too");
        assert!(syn4.notes.iter().all(|n| n.len == 8.0), "0x3e is eight steps");

        assert_eq!(report.conditions, 0, "A01 has no conditions on it");
    }

    /// An unset lane is not silence — it is "take the track's", and the import
    /// has to resolve it or every such trig arrives at the wrong level. No
    /// fixture contains one, which is exactly why this authors one: a reader
    /// that returned the raw `FF` would be right about all our captures and
    /// wrong on the box.
    #[test]
    fn a_trig_with_no_velocity_of_its_own_arrives_at_the_tracks() {
        let mut dump = a01();
        let base = digi_protocol::a4_pattern::track_base(0);
        dump.payload[base + digi_protocol::a4_pattern::VELOCITY_LANE] = 0xFF;
        dump.payload[base + digi_protocol::a4_pattern::LENGTH_LANE] = 0xFF;

        let (pattern, _) = a4_pattern_to_model(&A4, &dump).unwrap();
        let first = &pattern.track(0).unwrap().notes[0];
        assert_eq!(first.velocity, 100, "SYN1's default velocity, 0x64");
        assert_eq!(first.len, 1.0, "SYN1's default length, 0x0e — one step");
    }

    /// A condition on the box becomes exactly one of the three fields this app
    /// models, and which one depends on where in the menu it sits. That split
    /// is the whole asymmetry: the A4 has one knob, a digi has three lanes.
    #[test]
    fn each_kind_of_a4_condition_lands_in_the_field_that_holds_it() {
        for (byte, expected) in [
            // 0x0d is 75%, 0x16 is FILL, 0x18 is PRE, 0x1e is 1:2.
            (0x0d, (Some(75), None, None)),
            (0x16, (None, Some(true), None)),
            (0x17, (None, Some(false), None)),
            (0x18, (None, None, Some("PRE".to_string()))),
            (0x1e, (None, None, Some("1:2".to_string()))),
        ] {
            let mut dump = a01();
            digi_protocol::a4_pattern::set_trig_condition(&mut dump.payload, 0, 0, Some(byte))
                .unwrap();
            let (pattern, report) = a4_pattern_to_model(&A4, &dump).unwrap();
            let first = &pattern.track(0).unwrap().notes[0];
            assert_eq!(
                (first.prob, first.fill, first.cond.clone()),
                expected,
                "{byte:#04x}"
            );
            assert_eq!(report.conditions, 1, "{byte:#04x} counted");
            assert_eq!(report.conditions_off_the_menu, 0);
        }
    }

    /// A byte past the end of the menu is evidence about the box rather than a
    /// bad import — the menu's length rests on four labels — so it is counted
    /// and the note comes in without a condition.
    #[test]
    fn a_condition_byte_past_the_menu_is_counted_rather_than_guessed_at() {
        let mut dump = a01();
        // 0x41, one past 8:8. No box has been seen to write it.
        digi_protocol::a4_pattern::set_trig_condition(&mut dump.payload, 0, 0, Some(0x41))
            .unwrap();
        let (pattern, report) = a4_pattern_to_model(&A4, &dump).unwrap();
        assert_eq!(report.conditions_off_the_menu, 1);
        assert_eq!(report.conditions, 0);
        assert_eq!(pattern.track(0).unwrap().notes[0].prob, None);
    }

    /// The export's three named losses, each on a trig the A4 cannot fully
    /// take: a probability off the ladder, a COND the menu lacks, and two
    /// fields set where one fits.
    #[test]
    fn an_export_says_which_conditions_the_a4_could_not_take() {
        let (mut s, id) = imported();
        {
            let d = s.device_mut(id).unwrap();
            let t = d.pattern_mut(0).unwrap().track_mut(1).unwrap();
            let mut off_ladder = Note::new(0.0, 60, 1.0, 100, 0.0);
            off_ladder.prob = Some(55);
            let mut no_equivalent = Note::new(1.0, 60, 1.0, 100, 0.0);
            no_equivalent.cond = Some("LST".to_string());
            let mut crowded = Note::new(2.0, 60, 1.0, 100, 0.0);
            crowded.prob = Some(50);
            crowded.cond = Some("PRE".to_string());
            t.notes.extend([off_ladder, no_equivalent, crowded]);
        }
        let export = s.a4_track_write(id, PatternRef::new(0, 0), 1, PatternRef::new(0, 0)).unwrap();

        // 55% rounds up to 59%, the nearest rung.
        assert_eq!(
            export.write.steps[0].and_then(|s| s.condition),
            digi_protocol::a4_conditions::to_byte(
                digi_protocol::a4_conditions::A4Cond::Probability(59)
            )
        );
        assert_eq!(export.write.steps[1].and_then(|s| s.condition), None, "LST cannot go");
        // The COND wins where both are set: it names an exact pass.
        assert_eq!(export.write.steps[2].and_then(|s| s.condition), Some(0x18), "PRE");

        let all = export.warnings.join(" | ");
        assert!(all.contains("rounded to the nearest"), "{all}");
        assert!(all.contains("LST"), "{all}");
        assert!(all.contains("holds one of the three"), "{all}");
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
        assert_eq!(t.notes.len(), 89, "and must still have imported — 32 trigs, 57 arp notes");
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

    /// A chord is the lowest note as the trig and the rest as NO2/NO3/NO4,
    /// ascending — whatever order the notes were drawn in — and it is not a
    /// loss any more, so there is nothing to warn about.
    #[test]
    fn a_chord_goes_as_its_root_plus_arp_offsets_in_ascending_order() {
        let (mut s, id) = imported();
        {
            let d = s.device_mut(id).unwrap();
            let t = d.pattern_mut(0).unwrap().track_mut(1).unwrap();
            for pitch in [64, 60, 67] {
                t.notes.push(Note::new(0.0, pitch, 1.0, 100, 0.0));
            }
            // A doubled pitch is an editing accident, not a fourth voice.
            t.notes.push(Note::new(0.0, 64, 1.0, 100, 0.0));
        }
        let export = s.a4_track_write(id, PatternRef::new(0, 0), 1, PatternRef::new(0, 0)).unwrap();
        let step = export.write.steps[0].expect("a trig on step 1");
        assert_eq!(step.note, 60, "the root survives, whatever the edit order");
        assert_eq!(step.arp_notes, [Some(4), Some(7), None]);
        assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    }

    /// The menu's two edges: a fifth distinct note has no lane, and a note more
    /// than 63 semitones over the root is past the top of NO2–NO4. Both are
    /// said, neither is silently folded into range.
    #[test]
    fn a_fifth_note_and_a_note_past_the_menus_reach_are_warned_about() {
        let (mut s, id) = imported();
        {
            let d = s.device_mut(id).unwrap();
            let t = d.pattern_mut(0).unwrap().track_mut(1).unwrap();
            for pitch in [36, 40, 43, 47, 50] {
                t.notes.push(Note::new(0.0, pitch, 1.0, 100, 0.0));
            }
            t.notes.push(Note::new(2.0, 24, 1.0, 100, 0.0));
            t.notes.push(Note::new(2.0, 100, 1.0, 100, 0.0));
        }
        let export = s.a4_track_write(id, PatternRef::new(0, 0), 1, PatternRef::new(0, 0)).unwrap();
        assert_eq!(export.write.steps[0].unwrap().arp_notes, [Some(4), Some(7), Some(11)]);
        assert_eq!(export.write.steps[2].unwrap().arp_notes, [None; 3]);
        assert_eq!(export.warnings.len(), 2, "{:?}", export.warnings);
        assert!(export.warnings[0].contains("1 note sat fifth"), "{}", export.warnings[0]);
        assert!(export.warnings[1].contains("63 semitones"), "{}", export.warnings[1]);
    }

    /// The factory A01's SYN1 comes in as the chords it plays — A4 with E6 and
    /// C7 over it on step 1 — and goes back out as the same offsets. What does
    /// not survive is the box's own NO2/NO3 order on the steps where it stored
    /// the higher offset first; the write is ascending, which sounds the same
    /// with the arp off and differs only if someone turns it on.
    #[test]
    fn a01_syn1_imports_as_chords_and_exports_as_the_same_offsets() {
        let (s, id) = imported();
        let step_one: Vec<u8> = {
            let d = s.device(id).unwrap();
            let t = d.pattern(0).unwrap().track(0).unwrap();
            let mut p: Vec<u8> = t.notes.iter().filter(|n| n.step == 0.0).map(|n| n.pitch).collect();
            p.sort_unstable();
            p
        };
        // The box labels these A4, E6 and C7 (`a4_pattern::note_name`, which
        // counts C5 as 60); the model holds the bytes.
        assert_eq!(step_one, [57, 76, 84], "A4 E6 C7 in the box's own labelling");

        let export = s.a4_track_write(id, PatternRef::new(0, 0), 0, PatternRef::new(0, 0)).unwrap();
        let first = export.write.steps[0].unwrap();
        assert_eq!((first.note, first.arp_notes), (57, [Some(19), Some(27), None]));
        // Step 7 is stored on the box as +27 then +19; this writes it ascending.
        let seventh = export.write.steps[6].unwrap();
        assert_eq!((seventh.note, seventh.arp_notes), (57, [Some(19), Some(27), None]));
        assert!(export.warnings.is_empty(), "{:?}", export.warnings);
        // Every trig on SYN1 has at least one offset in A01, none has three.
        let authored: Vec<A4Step> = export.write.steps.iter().flatten().copied().collect();
        assert_eq!(authored.len(), 32);
        assert!(authored.iter().all(|s| s.arp_notes[0].is_some() && s.arp_notes[2].is_none()));
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
        assert_eq!(export.write.steps[4].map(|s| s.note), Some(60));
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


#[cfg(test)]
mod plock_routing_tests {
    use super::*;
    use crate::device::Device;

    fn fixture(name: &str) -> A4Pattern {
        let path = format!("{}/../protocol/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        digi_protocol::a4_pattern::parse_pattern(&raw).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// SYN1 `0x22` + `0x23`, SYN2 `0x24` — the two-track pool captured 2026-09-01.
    fn two_track() -> A4Pattern {
        fixture("analogfour-A16-plock-two-track-2026-09-01.syx")
    }

    #[test]
    fn an_import_carries_the_pool_onto_the_tracks_that_own_it() {
        let (pattern, report) = a4_pattern_to_model(&A4, &two_track()).unwrap();
        assert_eq!(report.plock_lanes, 3);
        assert_eq!(report.trigless_plock_lanes, 0, "every lock sits on a trig");

        let syn1 = &pattern.track(0).unwrap().plocks;
        assert_eq!(syn1.iter().map(|l| l.param_id).collect::<Vec<_>>(), [Some(0x22), Some(0x23)]);
        let syn2 = &pattern.track(1).unwrap().plocks;
        assert_eq!(syn2.iter().map(|l| l.param_id).collect::<Vec<_>>(), [Some(0x24)]);

        // **Display values, since 2026-09-01**: these three parameters have a
        // measured scaling now, so the import converts as the digis' does and
        // the model holds what the box's screen shows — FREQ 50, RESO 100,
        // OVERDRIVE 127. They read `0x3200`/`0x6400`/`0x7f00` here until that
        // measurement landed, which is the raw word the same numbers pack into.
        //
        // **This fixture carries the scar of the experiment that produced it.**
        // It was captured after the detached-extension write, which cost FREQ
        // the fine byte it had (`0x3b`) and left RESO holding an all-zero
        // extension it would never have allocated. So FREQ reads as an exact 50
        // here where every 2026-08-31 capture has it fractional — which is also
        // why the conversion is exact rather than rounding.
        assert_eq!(syn1[0].values[0], Some(50), "FREQ 50, its fine byte spent on variant C");
        assert_eq!(syn1[1].values[4], Some(100), "RESO 100, and the zero fine byte reads as 0");
        assert_eq!(syn2[0].values[8], Some(127), "OVERDRIVE max");
        assert_eq!(
            syn1.iter().map(|l| l.name.as_deref()).collect::<Vec<_>>(),
            [Some("filter.cutoff"), Some("filter.resonance")],
            "a measured id resolves to its canonical name, which is the only evidence \
             PLockLane::param accepts on this box",
        );
        assert!(syn1.iter().all(|l| l.device_kind.as_deref() == Some("A4")));
    }

    /// A lane holding a value on a step with no trig is marked, so the roll can
    /// show it read-only rather than edit it into a lie — the same promise the
    /// gen-2 import makes, and the reason a write can be trusted not to
    /// invent one.
    #[test]
    fn a_lock_on_a_step_with_no_trig_imports_as_trigless() {
        let mut dump = two_track();
        // SYN1's FREQ lane locks step 1. Take the trig off it and the lane is
        // holding a value nothing plays.
        digi_protocol::a4_pattern::clear_trig(&mut dump.payload, 0, 0).unwrap();
        let (pattern, report) = a4_pattern_to_model(&A4, &dump).unwrap();
        let freq = &pattern.track(0).unwrap().plocks[0];
        assert!(freq.trigless, "the FREQ lane now holds a value on a trigless step");
        assert_eq!(report.trigless_plock_lanes, 1);
    }

    /// **The end-to-end round trip: import, describe as a write, apply, and the
    /// pool must be the bytes it started as.**
    ///
    /// The two halves of the routing checked against each other with the box's
    /// own payload in the middle, which is what `a4_track_write` claims when it
    /// says the track's lanes are the truth.
    #[test]
    fn a_pattern_read_off_the_box_and_written_straight_back_keeps_its_pool() {
        let dump = two_track();
        let (mut s, dev) = {
            let mut s = Session::default();
            let id = s.add_device(Device::new("A4", &A4, 16));
            (s, id)
        };
        let slot = PatternRef::new(0, 0);
        s.import_a4_pattern(dev, slot, &dump).unwrap();

        let mut payload = dump.payload.clone();
        for track in 0..2 {
            let export = s.a4_track_write(dev, slot, track, slot).unwrap();
            let lanes = export.write.plocks.expect("the track's lanes are the truth");
            assert!(
                export.warnings.iter().all(|w| !w.contains("p-lock")),
                "{:?}", export.warnings
            );
            digi_protocol::a4_plocks::apply_track_plocks(&mut payload, track, &lanes).unwrap();
        }

        let before = digi_protocol::a4_plocks::read_all_plocks(&dump.payload).unwrap();
        let after = digi_protocol::a4_plocks::read_all_plocks(&payload).unwrap();
        assert_eq!(after.len(), before.len(), "no lane gained or lost");
        for (a, b) in after.iter().zip(&before) {
            assert_eq!((a.param_id, a.track), (b.param_id, b.track));
            assert_eq!(a.values, b.values);
        }
    }

    /// **A lane authored in the roll from a name is sent, for the five
    /// parameters that have a measured id and scaling.** This asserted the
    /// opposite until 2026-09-01, when `a4_scale_probe` measured them on the
    /// box — the id comes from the curated table, and the display values are
    /// converted to stored words on the way out.
    #[test]
    fn a_lane_authored_from_a_measured_name_is_sent() {
        let lane = PLockLane::new(
            Some("filter.cutoff".to_owned()),
            None,
            Some("A4".to_owned()),
            false,
            vec![Some(64)],
        )
        .unwrap();
        let (out, warnings) = a4_lanes_for_write(&[lane]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].param_id, 0x22, "resolved from the name, not guessed");
        assert_eq!(out[0].values[0], Some(0x4000), "display 64 stored as coarse 64, fine 0");
    }

    /// And a lane naming a parameter **this box does not have** is refused by
    /// name rather than aimed at a guess — said out loud, because a lane
    /// silently not arriving is a pattern that plays flat.
    ///
    /// This used to reach the same branch through an A4 parameter whose scaling
    /// was unmeasured, and there is no longer one: all thirteen were measured on
    /// 2026-09-01. What is left is the genuinely absent knob — `lfo3.depth` is a
    /// digi parameter and the A4 has two LFOs — which is what a cross-device
    /// copy or a hand-edited project file can still put in front of this.
    #[test]
    fn a_lane_naming_a_knob_this_box_lacks_is_refused_and_reported() {
        assert!(
            digi_protocol::params::param_by_name(
                digi_protocol::params::param_table_for("A4"),
                "lfo3.depth",
            )
            .is_none(),
            "the A4 has two LFOs — if it ever gains a third this test needs another name",
        );
        let lane = PLockLane::new(
            Some("lfo3.depth".to_owned()),
            None,
            Some("A4".to_owned()),
            false,
            vec![Some(64)],
        )
        .unwrap();
        let (out, warnings) = a4_lanes_for_write(&[lane]);
        assert!(out.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("lfo3.depth"), "{}", warnings[0]);
        assert!(warnings[0].contains("can't write"), "{}", warnings[0]);
    }

    /// A DT2's `0x22` is not an A4's. Crossing boxes is copy-track's job and it
    /// translates by name; a lane carrying another box's numbering is refused.
    #[test]
    fn another_box_s_lane_does_not_go_to_the_a4() {
        let lane =
            PLockLane::new(None, Some(0x22), Some("DT2".to_owned()), false, vec![Some(0x4000)])
                .unwrap();
        let (out, warnings) = a4_lanes_for_write(&[lane]);
        assert!(out.is_empty());
        assert!(warnings[0].contains("DT2"), "{}", warnings[0]);
    }
}

#[cfg(test)]
mod plock_naming_tests {
    use super::*;

    fn fixture(name: &str) -> A4Pattern {
        let path = format!("{}/../protocol/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        digi_protocol::a4_pattern::parse_pattern(&raw).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// A synth track's lanes arrive named. **Which name depends on whether the
    /// parameter has a measured *scaling*, and the two are different names on
    /// purpose**: a curated lane takes `A4_PARAMS`' label, and a lane the
    /// scaling sweep has not reached keeps the four-character name the box
    /// prints, out of `A4_SYNTH_PLOCKS`.
    #[test]
    fn a_synth_lane_is_named_from_the_measured_table() {
        let dump = fixture("analogfour-A16-plock-two-track-2026-09-01.syx");
        let (pattern, _) = a4_pattern_to_model(&A4, &dump).unwrap();

        // Measured 2026-09-01, so curated and carrying a canonical name.
        let syn1 = &pattern.track(0).unwrap().plocks;
        assert_eq!(syn1[0].name.as_deref(), Some("filter.cutoff"));
        assert_eq!(syn1[0].param().label.as_ref(), "FLTR1 FREQ");
        assert_eq!(syn1[1].param().label.as_ref(), "FLTR1 RESO");

        let syn2 = &pattern.track(1).unwrap().plocks;
        assert_eq!(syn2[0].param().label.as_ref(), "FLTR OVERDRIVE");

        // And one the sweep has not reached: `0x25` is FLTR1 TRK, named by the
        // box and in no curated table, so it keeps the stand-in label.
        let mut dump = fixture("analogfour-A16-plock-two-track-2026-09-01.syx");
        let o = digi_protocol::a4_plocks::POOL_BASE;
        assert_eq!(dump.payload[o], 0x22, "the fixture's first lane");
        dump.payload[o] = 0x25;
        let (pattern, _) = a4_pattern_to_model(&A4, &dump).unwrap();
        let lane = &pattern.track(0).unwrap().plocks[0];
        assert_eq!(lane.label.as_deref(), Some("FLTR1 TRK"));
        assert_eq!(lane.name, None, "no canonical name, because no measured scaling");
    }

    /// **A named lane is editable only once its scaling is measured**, and the
    /// split is the whole point.
    ///
    /// This test asserted that *no* A4 lane was ever curated, which was right
    /// while only ids had been measured. `a4_scale_probe` measured five
    /// scalings on the box on 2026-09-01, so the rule it was really protecting —
    /// **a label is not a scaling** — now has to be shown on a lane that has one
    /// and a lane that does not, rather than on the absence of the whole set.
    #[test]
    fn a_lane_is_curated_only_where_the_scaling_was_measured() {
        let dump = fixture("analogfour-A16-plock-two-track-2026-09-01.syx");
        let (pattern, _) = a4_pattern_to_model(&A4, &dump).unwrap();
        let measured = &pattern.track(0).unwrap().plocks[0];
        assert!(measured.param().curated, "0x22's scaling was read off the box");
        assert!(measured.param().writable(), "so it can be written into the pool");

        // FLTR1 TRK: named by the box, no measured scaling, still read-only.
        let mut dump = fixture("analogfour-A16-plock-two-track-2026-09-01.syx");
        dump.payload[digi_protocol::a4_plocks::POOL_BASE] = 0x25;
        let (pattern, _) = a4_pattern_to_model(&A4, &dump).unwrap();
        let unmeasured = &pattern.track(0).unwrap().plocks[0];
        assert!(!unmeasured.param().curated, "a label is not a measured scaling");
        assert!(unmeasured.name.is_none());
    }

    /// **The FX and CV tracks are read-only, and their locks survive a round
    /// trip byte for byte.** Decided for the first release that ships A4
    /// features: their p-lock id space has never been swept, so this app cannot
    /// name a single one of their knobs, and the honest thing is to carry what
    /// the box has rather than guess at it.
    ///
    /// The trap this pins is specific and it is new as of 2026-09-01. `0x22` is
    /// now a *curated* id — `filter.cutoff`, with a measured scaling — so a
    /// resolution that consulted the id alone would hand an FX lane the synth
    /// table's answer, convert its stored word onto the 0..127 display axis,
    /// clamp `0x4000` to 127, and write back `0x7f00`. A lane the user never
    /// touched would come back changed and wrong.
    #[test]
    fn an_fx_lane_is_read_only_and_survives_the_round_trip_unchanged() {
        let mut dump = fixture("analogfour-A16-plock-two-track-2026-09-01.syx");
        // Move SYN1's 0x22 lane onto the FX track: a curated id, a track kind
        // that id does not belong to.
        let o = digi_protocol::a4_plocks::POOL_BASE + 1;
        assert_eq!(dump.payload[o - 1], 0x22, "the fixture's first lane");
        dump.payload[o] = 4;

        let (pattern, _) = a4_pattern_to_model(&A4, &dump).unwrap();
        let fx = &pattern.track(4).unwrap().plocks;
        assert_eq!(fx.len(), 1);

        let desc = fx[0].param();
        assert!(!desc.curated, "0x22 is curated on a synth track and must not be here");
        assert!(!crate::a4_transfer::tests_lane_is_editable(&fx[0]), "so it cannot be dragged");
        assert_eq!(fx[0].name, None, "and it carries no canonical name to be curated by");

        // Raw stored words in, the same raw stored words out.
        let raw = digi_protocol::a4_plocks::read_all_plocks(&dump.payload).unwrap();
        let before = raw.iter().find(|l| l.track == 4).expect("the moved lane");
        assert_eq!(fx[0].values[0], before.word(0), "carried as the box stored it");

        let (out, warnings) = a4_lanes_for_write(&fx[0..1]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].param_id, 0x22);
        assert_eq!(
            out[0].values, fx[0].values,
            "no conversion on the way out either — byte-exact is the whole promise",
        );
    }

    /// **An FX or CV lane keeps its hex stand-in.** The p-lock id space is per
    /// track kind — an FX lock was measured on `0x1a` and `0x29`, both synth
    /// parameters — so naming one from the synth table would be a confident
    /// wrong answer. Neither track has been swept.
    #[test]
    fn an_fx_track_lane_is_not_named_from_the_synth_table() {
        let mut dump = fixture("analogfour-A16-plock-two-track-2026-09-01.syx");
        // Move SYN1's 0x22 lane onto the FX track (index 4) by hand: the same
        // id, a different track kind.
        let o = digi_protocol::a4_plocks::POOL_BASE + 1;
        assert_eq!(dump.payload[o - 1], 0x22, "the fixture's first lane");
        dump.payload[o] = 4;

        let (pattern, _) = a4_pattern_to_model(&A4, &dump).unwrap();
        let fx = &pattern.track(4).unwrap().plocks;
        assert_eq!(fx.len(), 1);
        assert_eq!(fx[0].param_id, Some(0x22), "the lane still arrives");
        assert_eq!(fx[0].label, None, "but unnamed — 0x22 is not FLTR1 FRQ here");
        assert!(fx[0].param().label.contains("0x22"), "{}", fx[0].param().label);
    }
}
