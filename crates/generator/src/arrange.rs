// The orchestrator: a song context in, one result per row out.
//
// Port of `js/gen/arrange.js`, restructured for PLAN.md Phase 7 Decision 1 —
// see `context`'s module header for the whole argument. The two properties
// this module exists to guarantee are unchanged from the JS:
//
//   * **Each part draws from its own stream** (`rng_for(seed, stream_tag(id,
//     variation))`), so nudging one row's density doesn't reshuffle another.
//     The tag is keyed by the row's stable [`PartId`], not its role or its
//     position in the list — the "stream-tag trap" `context`'s header warns
//     about.
//   * **Every part is always generated, even the ones whose checkbox is
//     off.** Their notes still feed the shared rhythm map, so unchecking a
//     bass row doesn't move a lead row that answers it. A caller applies
//     only the parts marked `on`.
//
// Order is row order — the order the parts appear in `GenContext::parts` —
// each handed the accumulated set of steps every earlier row claimed. That
// one piece of shared state is the whole of what buys call-and-response,
// and it generalises the JS's fixed bass→chords→lead order: with N rows of
// any role, "what answers what" is simply "what comes after it in the
// list", which the panel's row order already controls.
//
// Nothing here encodes a byte. A result is ordinary pattern state — roll
// notes, and (from Stage 4) p-lock lanes — which leaves for the box through
// the existing `safe_write_tracks` path unchanged.

use std::collections::HashSet;

use digi_core::model::Note;

use crate::context::{resolve_context, GenContext, Part, PartId, ResolvedContext};
use crate::genres::{role_profile, Role};
use crate::parts::{bass::generate_bass, chords::generate_chords, drums::generate_drums, lead::generate_lead, GeneratedPart};
use crate::plockdesign::design_lanes;
use crate::rng::rng_for;
use crate::theory::ChordParseError;

/// A part's stream tag. `variation` is what "Generate this part" bumps: it
/// gives that one part a different stream while every other part keeps the
/// song seed's, so re-rolling a lead leaves a bass exactly as it is *and*
/// the new lead still answers the bassline actually sitting in the slot.
/// Rolling the seed instead would move every part, which is what
/// regenerating the whole arrangement is for.
pub fn stream_tag(id: PartId, variation: u32) -> String {
    if variation == 0 {
        format!("part{}", id.0)
    } else {
        format!("part{}#{variation}", id.0)
    }
}

fn generate_for_role(ctx: &ResolvedContext, role: Role, octave: u8, density: u8, rng: &mut crate::rng::Rng, busy: &HashSet<u32>) -> GeneratedPart {
    let profile = role_profile(ctx.profile.id, role);
    match role {
        Role::Bass => generate_bass(ctx, &profile, octave, density, rng, busy),
        Role::Chords => generate_chords(ctx, &profile, octave, density, rng, busy),
        Role::Lead => generate_lead(ctx, &profile, octave, density, rng, busy).into(),
        // Every drum voice takes the identical path — the only thing that
        // differs between a kick and a ride is the profile fetched above,
        // which is why adding a voice costs a weight table and a name here.
        // Spelled out rather than guarded on `is_drum_voice`, so that a
        // future role that is neither melodic nor a drum fails to compile
        // here instead of being quietly generated as a rhythm.
        Role::Kick
        | Role::Snare
        | Role::Clap
        | Role::Rimshot
        | Role::ClosedHat
        | Role::OpenHat
        | Role::Ride
        | Role::Tom => generate_drums(ctx, &profile, density, rng, busy),
    }
}

/// One row, generated: real roll notes (ids and all), so a caller can drop
/// them straight onto a track.
#[derive(Debug, Clone)]
pub struct ArrangedPart {
    pub part_id: PartId,
    pub role: Role,
    pub on: bool,
    pub destination: crate::context::Destination,
    pub length_steps: u32,
    pub notes: Vec<Note>,
    /// From `plockdesign::design_lanes`, unarbitrated against the shared
    /// pool — see `generate_arrangement`'s doc comment.
    pub plocks: Vec<digi_core::model::PLockLane>,
    pub trig_count: usize,
    /// Why lanes came back empty (no resolvable box, or none of a genre's
    /// recipe matches that box's measured parameters). Empty when lanes
    /// were not wanted at all (Motion 0, or no trigs).
    pub warnings: Vec<String>,
}

/// 128 steps, the full pattern memory a box's lane pool addresses —
/// `digi_core::model::PLOCK_STEPS`.
const PLOCK_STEPS: usize = digi_core::model::PLOCK_STEPS;

fn build_part(ctx: &ResolvedContext, part: &Part, busy: &HashSet<u32>, device_kind: Option<&'static str>) -> ArrangedPart {
    let tag = stream_tag(part.id, part.variation);
    let mut rng = rng_for(ctx.seed, &tag);
    let generated = generate_for_role(ctx, part.role, part.octave, part.density, &mut rng, busy);

    let trig_count = generated.notes.iter().map(|n| n.step).collect::<HashSet<_>>().len();
    let notes: Vec<Note> = generated
        .notes
        .iter()
        .map(|spec| {
            let mut note = Note::new(f64::from(spec.step), spec.pitch, spec.len, spec.velocity, spec.micro);
            note.prob = spec.prob.map(|p| p.clamp(0, 100) as u8);
            note.fill = spec.fill;
            note.cond = spec.cond.map(str::to_string);
            note
        })
        .collect();

    // The lane rng is its own stream: drawing lanes must not shift the
    // notes, so that turning Motion up doesn't rewrite the music.
    let mut lane_rng = rng_for(ctx.seed, &format!("{tag}.lanes"));
    let profile = role_profile(ctx.profile.id, part.role);
    let (designed, warnings) =
        design_lanes(&profile, device_kind, &generated.trigs, ctx.length_steps, u32::from(ctx.feel.motion), PLOCK_STEPS, &mut lane_rng);
    let plocks: Vec<digi_core::model::PLockLane> = designed
        .into_iter()
        .map(|lane| {
            let values: Vec<Option<u16>> = lane.values.into_iter().map(|v| v.map(|x| x as u16)).collect();
            digi_core::model::PLockLane::new(Some(lane.name.to_string()), None, Some(lane.device_kind.to_string()), false, values)
                .expect("a named lane always constructs")
        })
        .collect();

    ArrangedPart {
        part_id: part.id,
        role: part.role,
        on: part.on,
        destination: part.destination,
        length_steps: ctx.length_steps,
        notes,
        plocks,
        trig_count,
        warnings,
    }
}

/// The whole generated arrangement, and anything worth telling the panel
/// about across every row.
#[derive(Debug, Clone)]
pub struct Arrangement {
    pub parts: Vec<ArrangedPart>,
    pub warnings: Vec<String>,
}

/// Why an arrangement, or one part of it, could not be generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrangeError {
    Progression(ChordParseError),
    /// `generate_part` was asked for a [`PartId`] the context does not have
    /// — the JS's "unknown part" throw, adapted to N parts identified by id
    /// rather than by role.
    UnknownPart(PartId),
}

impl From<ChordParseError> for ArrangeError {
    fn from(e: ChordParseError) -> Self {
        Self::Progression(e)
    }
}

/// Generate the whole arrangement: every row in `ctx.parts`, in row order,
/// each answering the rows before it.
///
/// `device_kind` is `"DT2"` / `"DN2"` / `None` — which box's parameter
/// numbering the p-lock lanes belong to. `None` means no lanes, and a
/// warning saying why. Lane demand is **not** arbitrated against the
/// shared pool here — see `plockdesign`'s module header — so a wide
/// generate aimed at one slot should call `plockdesign::wanted_lane_count`
/// and `arbitrate_pool` itself before trusting these lane counts.
pub fn generate_arrangement(ctx: &GenContext, device_kind: Option<&'static str>) -> Result<Arrangement, ArrangeError> {
    let resolved = resolve_context(ctx)?;
    let mut busy: HashSet<u32> = HashSet::new();
    let mut parts = Vec::with_capacity(ctx.parts.len());
    let mut warnings: Vec<String> = Vec::new();

    for part in &ctx.parts {
        let arranged = build_part(&resolved, part, &busy, device_kind);
        // Only a part that is actually being used should claim steps in the
        // busy map… except that it must claim them whether or not it is
        // applied, or a later row would move when an earlier row's checkbox
        // changed. So every part registers, and `on` decides what a caller
        // writes.
        busy.extend(arranged.notes.iter().map(|n| n.step as u32));
        if arranged.on {
            for w in &arranged.warnings {
                if !warnings.contains(w) {
                    warnings.push(w.clone());
                }
            }
        }
        parts.push(arranged);
    }

    Ok(Arrangement { parts, warnings })
}

/// One part, against the same context — "Generate this part". It runs the
/// *whole* arrangement and returns one row of it, which is the point: the
/// lead you re-roll on its own is exactly the lead the full arrangement
/// would have produced, because the rhythm map it answered is the same one.
pub fn generate_part(ctx: &GenContext, id: PartId, device_kind: Option<&'static str>) -> Result<ArrangedPart, ArrangeError> {
    let arrangement = generate_arrangement(ctx, device_kind)?;
    arrangement.parts.into_iter().find(|p| p.part_id == id).ok_or(ArrangeError::UnknownPart(id))
}

/// Apply a generated part to a track, in place.
///
/// **The list of fields this touches is the feature's safety story**, so it
/// lives in one function rather than at each call site:
///
///   * `notes` and `plocks` are replaced, and `length_steps` with them —
///     that is the generation;
///   * `name` is set when a label is given, so the slot dropdown says what's
///     in it;
///   * swing has no equivalent field on a `Track` at all — it lives on the
///     `Pattern` a track belongs to, one level up, so there is nothing here
///     that could touch it even by accident. Genre groove is per-note
///     micro-timing instead (see `genres::GenreProfile::groove`).
///   * `track_prob`, `channel`, `mute`, `solo`, `kind`, `scale` and
///     `out_port` are *not* touched. Track PROB is a default the person sets;
///     the generator expresses chance through per-trig PROB locks, which is
///     the hardware's own model.
pub fn apply_part_to_track(track: &mut digi_core::model::Track, part: ArrangedPart, label: Option<&str>) {
    track.length_steps = part.length_steps.min(u32::from(u16::MAX)) as u16;
    track.notes = part.notes;
    track.plocks = part.plocks;
    if let Some(label) = label {
        track.name = label.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{default_parts, Feel};
    use crate::genres::GenreId;

    fn ctx(over: GenContext) -> GenContext {
        over
    }

    fn shape(notes: &[Note]) -> String {
        notes.iter().map(|n| format!("{}:{}:{}:{}:{}", n.step, n.pitch, n.len, n.velocity, n.micro)).collect::<Vec<_>>().join("|")
    }

    #[test]
    fn is_n_parts_in_row_order() {
        let c = ctx(GenContext { seed: 1, ..GenContext::default() });
        let arrangement = generate_arrangement(&c, None).unwrap();
        assert_eq!(arrangement.parts.len(), c.parts.len());
        for (arranged, part) in arrangement.parts.iter().zip(&c.parts) {
            assert_eq!(arranged.part_id, part.id);
            assert_eq!(arranged.role, part.role);
            assert_eq!(arranged.destination, part.destination);
            assert!(!arranged.notes.is_empty());
        }
    }

    #[test]
    fn gives_every_note_a_unique_id() {
        let c = ctx(GenContext { seed: 2, ..GenContext::default() });
        let arrangement = generate_arrangement(&c, None).unwrap();
        let ids: Vec<u32> = arrangement.parts.iter().flat_map(|p| p.notes.iter().map(|n| n.id)).collect();
        let set: HashSet<u32> = ids.iter().copied().collect();
        assert_eq!(set.len(), ids.len());
    }

    #[test]
    fn takes_its_length_from_the_bar_count() {
        for bars in [1u32, 2, 4, 8] {
            let c = ctx(GenContext { bars, seed: 3, ..GenContext::default() });
            let arrangement = generate_arrangement(&c, None).unwrap();
            for part in &arrangement.parts {
                assert_eq!(part.length_steps, bars * 16);
                for n in &part.notes {
                    assert!(n.step < f64::from(bars * 16));
                }
            }
        }
    }

    #[test]
    fn counts_trigs_not_notes() {
        let c = ctx(GenContext { seed: 4, ..GenContext::default() });
        let arrangement = generate_arrangement(&c, None).unwrap();
        let chords = &arrangement.parts[1];
        assert_eq!(chords.role, Role::Chords);
        assert!(chords.trig_count < chords.notes.len());
        let distinct: HashSet<u64> = chords.notes.iter().map(|n| n.step.to_bits()).collect();
        assert_eq!(chords.trig_count, distinct.len());
    }

    #[test]
    fn throws_the_parsers_message_for_a_malformed_progression() {
        let c = ctx(GenContext { progression: "i nonsense".into(), ..GenContext::default() });
        let err = generate_arrangement(&c, None).unwrap_err();
        match err {
            ArrangeError::Progression(e) => assert!(e.0.contains("roman numerals")),
            _ => panic!("expected a progression error"),
        }
    }

    #[test]
    fn generates_something_for_every_genre() {
        for genre in GenreId::ALL {
            let c = ctx(GenContext { genre, seed: 5, ..GenContext::default() });
            let arrangement = generate_arrangement(&c, None).unwrap();
            for part in &arrangement.parts {
                assert!(!part.notes.is_empty());
            }
        }
    }

    #[test]
    fn a_drum_row_flows_through_the_full_arrangement_pipeline() {
        // Kick, Snare and ClosedHat are three of the six default rows since
        // Neil's 2026-08-20 ask (`context::default_parts`); OpenHat is not,
        // so it is what this test uses to prove a caller (the Generate
        // panel) can still add a drum voice by hand like any other part —
        // this is that path, exercised end to end rather than only at
        // `parts::drums`'s own level.
        let mut c = ctx(GenContext { seed: 13, ..GenContext::default() });
        let mut hat = crate::context::default_parts()[0];
        hat.id = crate::context::PartId::next();
        hat.role = Role::OpenHat;
        hat.destination.track = 6; // past the six default rows' own tracks 0..=5
        c.parts.push(hat);

        let arrangement = generate_arrangement(&c, Some("DT2")).unwrap();
        let drum_part = arrangement.parts.last().unwrap();
        assert_eq!(drum_part.role, Role::OpenHat);
        assert!(!drum_part.notes.is_empty());
        // No lanes were invented for the first cut of drums — see
        // `genres::drum_profile`'s header.
        assert!(drum_part.plocks.is_empty());
        for n in &drum_part.notes {
            assert_eq!(n.pitch, crate::parts::drums::DRUM_TRIGGER_PITCH);
        }
    }

    #[test]
    fn the_seed_makes_a_result_reproducible() {
        // `GenContext::default()` assigns each part a fresh id off a global
        // counter, so it must be called once and shared — two independent
        // calls are two different (if identical-looking) contexts, not the
        // same context generated twice. Comparing across two `default()`
        // calls is exactly the stream-tag trap `context`'s header warns
        // about, aimed at a test instead of a caller.
        let c = ctx(GenContext { seed: 12345, ..GenContext::default() });
        let a = generate_arrangement(&c, None).unwrap();
        let b = generate_arrangement(&c, None).unwrap();
        for (pa, pb) in a.parts.iter().zip(&b.parts) {
            assert_eq!(shape(&pb.notes), shape(&pa.notes));
        }
    }

    #[test]
    fn the_seed_changes_everything_when_it_changes() {
        let base = GenContext::default();
        let a = generate_arrangement(&GenContext { seed: 12345, ..base.clone() }, None).unwrap();
        let b = generate_arrangement(&GenContext { seed: 12346, ..base.clone() }, None).unwrap();
        let different = a.parts.iter().zip(&b.parts).filter(|(pa, pb)| shape(&pb.notes) != shape(&pa.notes)).count();
        // All six of the default rows, now that there are six — each part's
        // stream is keyed by its own `PartId` plus the song seed, so every
        // row's stream moves when the seed does.
        assert_eq!(different, base.parts.len());
    }

    #[test]
    fn keeps_other_parts_still_when_only_one_parts_density_moves() {
        // The structural guarantee is per-part stream independence: bass and
        // chords must tie *exactly*, every time, because their RNG streams
        // never read the lead's density. Whether the lead's *own* output
        // moves for any single seed is a statistical question, not a
        // per-seed guarantee — the busy set it answers depends on what bass
        // and chords rolled, so an unlucky seed can have every note that
        // thinning would drop be one the busy-step filter would have
        // dropped anyway, tying by coincidence. Proven across a spread of
        // seeds instead of trusting one.
        let mut lead_differed = false;
        for seed in 0..12u32 {
            let base = ctx(GenContext { seed, ..GenContext::default() });
            let mut moved = base.clone();
            moved.parts[2].density = moved.parts[2].density.saturating_add(30).min(100);
            let a = generate_arrangement(&base, None).unwrap();
            let b = generate_arrangement(&moved, None).unwrap();
            assert_eq!(shape(&b.parts[0].notes), shape(&a.parts[0].notes), "seed {seed}: bass moved");
            assert_eq!(shape(&b.parts[1].notes), shape(&a.parts[1].notes), "seed {seed}: chords moved");
            if shape(&b.parts[2].notes) != shape(&a.parts[2].notes) {
                lead_differed = true;
            }
        }
        assert!(lead_differed, "the lead's density never changed its own output across any seed tried");
    }

    #[test]
    fn leaves_a_later_part_alone_when_an_earlier_one_is_unchecked() {
        // Every part is generated whether or not it is applied, precisely
        // so that turning one off doesn't reshuffle the ones that answer it.
        let base = ctx(GenContext { seed: 31337, ..GenContext::default() });
        let mut off = base.clone();
        off.parts[0].on = false;
        let a = generate_arrangement(&base, None).unwrap();
        let b = generate_arrangement(&off, None).unwrap();
        assert_eq!(shape(&b.parts[2].notes), shape(&a.parts[2].notes));
        assert!(!b.parts[0].on);
        assert!(!b.parts[0].notes.is_empty());
    }

    #[test]
    fn threads_the_rhythm_map_forward_so_the_lead_answers_the_bass() {
        let mut doubled = 0;
        let mut total = 0;
        for seed in 0..25u32 {
            let c = ctx(GenContext { seed, bars: 2, ..GenContext::default() });
            let arrangement = generate_arrangement(&c, None).unwrap();
            let bass: HashSet<u64> = arrangement.parts[0].notes.iter().map(|n| n.step.to_bits()).collect();
            for n in &arrangement.parts[2].notes {
                total += 1;
                if bass.contains(&n.step.to_bits()) {
                    doubled += 1;
                }
            }
        }
        assert!((doubled as f64) / (total as f64) < 0.35);
    }

    #[test]
    fn regenerating_one_part_produces_exactly_the_part_the_whole_arrangement_would_have() {
        let c = ctx(GenContext { seed: 24680, ..GenContext::default() });
        let whole = generate_arrangement(&c, None).unwrap();
        for part in &c.parts {
            let got = generate_part(&c, part.id, None).unwrap();
            let want = whole.parts.iter().find(|p| p.part_id == part.id).unwrap();
            assert_eq!(shape(&got.notes), shape(&want.notes));
        }
    }

    #[test]
    fn refuses_a_part_nobody_has_written() {
        let c = ctx(GenContext::default());
        let bogus = crate::context::PartId(999_999);
        assert_eq!(generate_part(&c, bogus, None).unwrap_err(), ArrangeError::UnknownPart(bogus));
    }

    #[test]
    fn re_rolls_one_part_and_leaves_the_others_exactly_where_they_were() {
        let mut base = ctx(GenContext { seed: 1357, ..GenContext::default() });
        let lead_id = base.parts[2].id;
        let a = generate_arrangement(&base, None).unwrap();
        base.bump_variation(lead_id);
        let b = generate_arrangement(&base, None).unwrap();
        assert_eq!(shape(&b.parts[0].notes), shape(&a.parts[0].notes));
        assert_eq!(shape(&b.parts[1].notes), shape(&a.parts[1].notes));
        assert_ne!(shape(&b.parts[2].notes), shape(&a.parts[2].notes));
    }

    #[test]
    fn goes_back_to_the_canonical_arrangement_when_variations_are_reset() {
        let base = ctx(GenContext { seed: 2468, ..GenContext::default() });
        let mut messy = base.clone();
        messy.bump_variation(messy.parts[2].id);
        messy.bump_variation(messy.parts[0].id);
        let a = generate_arrangement(&base, None).unwrap();
        messy.reset_variations();
        let b = generate_arrangement(&messy, None).unwrap();
        for (pa, pb) in a.parts.iter().zip(&b.parts) {
            assert_eq!(shape(&pb.notes), shape(&pa.notes));
        }
    }

    #[test]
    fn applying_a_part_replaces_the_music_and_leaves_every_other_field_alone() {
        let c = ctx(GenContext { seed: 9, bars: 4, ..GenContext::default() });
        let arrangement = generate_arrangement(&c, None).unwrap();
        let bass = arrangement.parts.into_iter().next().unwrap();
        let notes_len = bass.notes.len();

        let mut track = digi_core::model::Track {
            name: "T1".into(),
            length_steps: 16,
            scale: digi_core::model::TrackScale::One,
            track_prob: 40,
            kind: digi_core::model::TrackKind::Midi,
            notes: Vec::new(),
            plocks: Vec::new(),
            level: None,
            out_port: None,
            channel: 9,
            mute: false,
            solo: false,
            patch: None,
        };
        apply_part_to_track(&mut track, bass, Some("DnB bass"));

        assert_eq!(track.notes.len(), notes_len);
        assert_eq!(track.length_steps, 64);
        assert_eq!(track.name, "DnB bass");
        assert_eq!(track.track_prob, 40);
        assert_eq!(track.channel, 9);
    }

    #[test]
    fn lanes_are_real_lanes_keyed_by_name_and_box_spanning_the_pattern_memory() {
        let c = ctx(GenContext {
            seed: 6,
            feel: Feel { motion: 100, looseness: 30, humanize: 20 },
            ..GenContext::default()
        });
        let arrangement = generate_arrangement(&c, Some("DT2")).unwrap();
        let lanes = &arrangement.parts[0].plocks;
        assert!(!lanes.is_empty());
        for lane in lanes {
            assert!(lane.name.is_some());
            assert_eq!(lane.param_id, None); // named lanes resolve their byte on the way out
            assert_eq!(lane.device_kind.as_deref(), Some("DT2"));
            assert!(!lane.trigless);
            assert_eq!(lane.values.len(), digi_core::model::PLOCK_STEPS);
        }
    }

    #[test]
    fn lanes_only_sit_on_steps_the_part_actually_trigs() {
        let c = ctx(GenContext {
            seed: 7,
            feel: Feel { motion: 90, looseness: 40, humanize: 10 },
            ..GenContext::default()
        });
        let arrangement = generate_arrangement(&c, Some("DN2")).unwrap();
        for part in &arrangement.parts {
            let live: HashSet<u32> = part.notes.iter().map(|n| n.step as u32).collect();
            for lane in &part.plocks {
                for (step, v) in lane.values.iter().enumerate() {
                    if v.is_some() {
                        assert!(live.contains(&(step as u32)), "{:?} lane value on trigless step {step}", part.role);
                    }
                }
            }
        }
    }

    #[test]
    fn lanes_are_absent_with_a_warning_when_no_box_can_be_resolved() {
        let c = ctx(GenContext {
            seed: 8,
            feel: Feel { motion: 100, looseness: 20, humanize: 0 },
            ..GenContext::default()
        });
        let arrangement = generate_arrangement(&c, None).unwrap();
        for part in &arrangement.parts {
            assert!(part.plocks.is_empty());
        }
        assert_eq!(arrangement.warnings.len(), 1);
        assert!(arrangement.warnings[0].contains("which box"));
    }

    #[test]
    fn that_warning_is_reported_once_not_once_per_part() {
        let c = ctx(GenContext { feel: Feel { motion: 50, looseness: 0, humanize: 0 }, ..GenContext::default() });
        let arrangement = generate_arrangement(&c, None).unwrap();
        let unique: HashSet<&String> = arrangement.warnings.iter().collect();
        assert_eq!(unique.len(), arrangement.warnings.len());
    }

    #[test]
    fn nothing_is_said_about_a_box_when_motion_is_off() {
        let c = ctx(GenContext { feel: Feel { motion: 0, looseness: 20, humanize: 0 }, ..GenContext::default() });
        let arrangement = generate_arrangement(&c, None).unwrap();
        assert!(arrangement.warnings.is_empty());
    }

    #[test]
    fn applying_a_part_keeps_the_tracks_name_when_none_is_given() {
        let c = ctx(GenContext { seed: 10, ..GenContext::default() });
        let arrangement = generate_arrangement(&c, None).unwrap();
        let lead = arrangement.parts.into_iter().nth(2).unwrap();
        let mut track = digi_core::model::Track {
            name: "Track 3".into(),
            length_steps: 16,
            scale: digi_core::model::TrackScale::One,
            track_prob: 100,
            kind: digi_core::model::TrackKind::Midi,
            notes: Vec::new(),
            plocks: Vec::new(),
            level: None,
            out_port: None,
            channel: 3,
            mute: false,
            solo: false,
            patch: None,
        };
        apply_part_to_track(&mut track, lead, None);
        assert_eq!(track.name, "Track 3");
    }

    #[test]
    fn stream_tag_is_stable_across_reordering_but_distinct_per_variation() {
        let parts = default_parts();
        let a = stream_tag(parts[0].id, 0);
        let b = stream_tag(parts[0].id, 1);
        assert_ne!(a, b);
        // The tag depends only on the id, so reordering the row list (which
        // never changes an id) can never change it — the whole point of
        // Decision 1's redesign.
        assert_eq!(stream_tag(parts[0].id, 0), a);
    }

    #[test]
    fn writes_no_conditions_at_looseness_0() {
        let c = ctx(GenContext { seed: 11, feel: Feel { motion: 50, looseness: 0, humanize: 30 }, ..GenContext::default() });
        let arrangement = generate_arrangement(&c, None).unwrap();
        for part in &arrangement.parts {
            for n in &part.notes {
                assert_eq!((n.prob, n.fill, n.cond.clone()), (None, None, None));
            }
        }
    }
}
