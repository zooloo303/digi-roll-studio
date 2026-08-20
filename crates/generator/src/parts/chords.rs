// The chord part.
//
// Port of `js/gen/parts/chords.js`. One voicing per progression slot,
// stamped through the app's own chord code: `theory::chord_tones` (→
// `core::chords::chord_pitches`) for the notes and `core::chords::voice_chord`
// for the strum stagger and the velocity taper. So a generated chord is
// byte-for-byte the same kind of thing chord draw makes, including the
// 4-note hardware ceiling.
//
// The session-musician touch is **voice leading**: each chord is built in
// all four inversions (and a drop-2 spread), and the one that moves least
// from the chord before it wins. That single rule is the difference between
// a part that walks and a part that jumps an octave every bar.

use std::collections::HashSet;

use digi_core::chords::{voice_chord, VoiceOpts};

use crate::context::ResolvedContext;
use crate::genres::{LenMode, LenProfile, RoleProfile};
use crate::rhythm::{gap_after, micro_for, rhythm_for, snap_micro, trig_feel_for, velocity_for, RhythmOpts};
use crate::rng::{chance, Rng};
use crate::theory::{best_voicing, voicing_candidates, window_for, Key};

use super::{GeneratedPart, NoteSpec};

/// The chords role's length data is always [`LenProfile::Mode`] in every
/// shipped genre — never `Plain` — so this reads that shape directly rather
/// than going through the bass-shaped `len_bounds` helper. A `Plain` profile
/// falls back to a stab-length reading rather than panicking, in case a
/// future genre gives chords a plain length.
fn mode_len(len: &LenProfile) -> (LenMode, f64, f64) {
    match *len {
        LenProfile::Mode { mode, normal, max } => (mode, normal, max),
        LenProfile::Plain { normal, max, .. } => (LenMode::Stab, normal, max),
    }
}

pub fn generate_chords(ctx: &ResolvedContext, profile: &RoleProfile, octave: u8, density: u8, rng: &mut Rng, busy: &HashSet<u32>) -> GeneratedPart {
    let (min, max) = window_for(profile.span, i32::from(octave));
    let total = ctx.length_steps;
    let key = Key { root: ctx.key_root, intervals: ctx.key_intervals };

    let trigs = rhythm_for(
        RhythmOpts {
            weights: &profile.weights,
            density: u32::from(density),
            bars: ctx.bars,
            // Chords landing with the bass is a band, not a collision, so
            // the busy map costs them almost nothing — unlike the lead.
            busy,
            avoid: 0.15,
            anchors: &[],
            trigs_per_bar: profile.trigs_per_bar,
        },
        rng,
    );

    let feel = trig_feel_for(&trigs, profile.conditions, ctx.feel.looseness as u32, ctx.bars, rng);
    let (mode, len_normal, len_max) = mode_len(&profile.len);

    let mut notes = Vec::new();
    let mut previous: Vec<i32> = Vec::new();
    for (i, trig) in trigs.iter().enumerate() {
        let slot = ctx.bar_slots[trig.bar as usize];
        // Whether this genre opens its voicings up is a per-chord coin toss
        // weighted by the profile, so a part isn't uniformly blocky or
        // uniformly wide.
        let spread = chance(rng, profile.spread.unwrap_or(0.0));
        let candidates = voicing_candidates(&slot, key, i32::from(octave), min, max, &[spread]);
        let centre = f64::from(min + max) / 2.0;
        let pitches = best_voicing(&previous, &candidates, Some(centre));
        if pitches.is_empty() {
            continue;
        }
        previous = pitches.clone();

        // Sustain holds to the next chord (a pad); stab is the genre's own
        // short length whatever the gap (house, breaks). Either way it is
        // snapped to the box's LEN scale and can't run past the end of the
        // pattern.
        let gap = gap_after(&trigs, i, total);
        let want = if mode == LenMode::Sustain { len_max.min(gap) } else { len_normal.min(gap).min(len_max) };
        let len = digi_core::snap_len_fine(want, f64::from(total - trig.step));

        let velocity = velocity_for(trig.accent, trig.ghost, profile.velocity, u32::from(ctx.feel.humanize), rng);
        let micro = micro_for(trig.step, &ctx.profile.groove, u32::from(ctx.feel.humanize), rng);
        let t = feel.get(&trig.step);

        let pitches_u8: Vec<u8> = pitches.iter().map(|&p| p.clamp(0, 127) as u8).collect();
        // Strum is real per-note micro-timing, so it survives write-back. It
        // rides on top of the groove offset and the sum is re-snapped to the
        // box's 1/24-step grid — otherwise a strum of 0.06 per note would be
        // three values the hardware rounds on the way in.
        let voiced = voice_chord(&pitches_u8, &VoiceOpts { velocity, strum: profile.strum.unwrap_or(0.0), taper: true });
        for v in voiced {
            notes.push(NoteSpec {
                step: trig.step,
                pitch: v.pitch,
                len,
                velocity: v.velocity,
                micro: snap_micro(micro + v.micro),
                // Every note on a step shares the trig's conditions — the
                // step-uniformity rule the encoder relies on, and the reason
                // `feel` is keyed by step.
                prob: t.and_then(|t| t.prob),
                fill: t.and_then(|t| t.fill),
                cond: t.and_then(|t| t.cond),
            });
        }
    }

    GeneratedPart { notes, trigs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{resolve_context, GenContext};
    use crate::genres::{role_profile, GenreId, Role};
    use crate::rng::rng_for;
    use digi_core::chords::MAX_CHORD_NOTES;

    fn ctx_for(over: GenContext) -> ResolvedContext {
        resolve_context(&over).unwrap()
    }

    fn generate(genre: GenreId, seed: u32, over: GenContext, density: u8, octave: u8) -> GeneratedPart {
        let ctx = ctx_for(over);
        let profile = role_profile(genre, Role::Chords);
        let mut rng = rng_for(seed, "chords");
        generate_chords(&ctx, &profile, octave, density, &mut rng, &HashSet::new())
    }

    #[test]
    fn never_exceeds_the_hardwares_four_notes_per_trig() {
        for genre in GenreId::ALL {
            let over = GenContext { genre, seed: 1, progression: "i7 iv7 VI7 v7".into(), bars: 4, ..GenContext::default() };
            let part = generate(genre, 1, over, 40, 4);
            let mut per_step: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for n in &part.notes {
                *per_step.entry(n.step).or_default() += 1;
            }
            for count in per_step.values() {
                assert!(*count <= MAX_CHORD_NOTES);
            }
        }
    }

    #[test]
    fn walks_between_chords_instead_of_jumping_an_octave() {
        let over =
            GenContext { genre: GenreId::House, seed: 3, progression: "i7 iv7 VI7 v7".into(), bars: 4, ..GenContext::default() };
        let part = generate(GenreId::House, 3, over, 40, 4);
        let mut steps: Vec<u32> = part.notes.iter().map(|n| n.step).collect();
        steps.sort_unstable();
        steps.dedup();
        let mean = |step: u32| -> f64 {
            let on: Vec<i32> = part.notes.iter().filter(|n| n.step == step).map(|n| i32::from(n.pitch)).collect();
            on.iter().sum::<i32>() as f64 / on.len() as f64
        };
        for i in 1..steps.len() {
            assert!((mean(steps[i]) - mean(steps[i - 1])).abs() < 9.0);
        }
    }

    #[test]
    fn plays_the_notes_of_the_bars_chord() {
        let over = GenContext { genre: GenreId::Dnb, seed: 4, progression: "i".into(), bars: 1, ..GenContext::default() };
        let part = generate(GenreId::Dnb, 4, over, 40, 4);
        for n in &part.notes {
            assert!([0, 3, 7].contains(&(i32::from(n.pitch).rem_euclid(12))));
        }
    }

    #[test]
    fn staggers_a_strummed_chord_with_real_micro_timing() {
        let over = GenContext {
            genre: GenreId::Breaks,
            seed: 9,
            feel: crate::context::Feel { motion: 0, looseness: 0, humanize: 0 },
            ..GenContext::default()
        };
        let part = generate(GenreId::Breaks, 9, over, 40, 4);
        let step = part.notes[0].step;
        let chord: Vec<&NoteSpec> = part.notes.iter().filter(|n| n.step == step).collect();
        assert!(chord.len() > 1);
        let micros: HashSet<u64> = chord.iter().map(|n| n.micro.to_bits()).collect();
        assert!(micros.len() > 1);
    }

    #[test]
    fn tapers_a_chords_velocity_so_the_top_note_sings() {
        let over = GenContext {
            genre: GenreId::Dnb,
            seed: 12,
            feel: crate::context::Feel { motion: 0, looseness: 0, humanize: 0 },
            ..GenContext::default()
        };
        let part = generate(GenreId::Dnb, 12, over, 40, 4);
        let step = part.notes[0].step;
        let mut chord: Vec<&NoteSpec> = part.notes.iter().filter(|n| n.step == step).collect();
        chord.sort_by_key(|n| n.pitch);
        assert!(chord.last().unwrap().velocity >= chord[0].velocity);
    }

    #[test]
    fn produces_notes_the_hardware_can_hold_at_every_density() {
        for genre in GenreId::ALL {
            for density in [0u8, 40, 100] {
                for seed in 0..6u32 {
                    let over = GenContext { genre, seed, bars: 2, ..GenContext::default() };
                    let part = generate(genre, seed, over, density, 4);
                    assert!(!part.notes.is_empty());
                    for n in &part.notes {
                        assert!((1..=127).contains(&n.velocity));
                        assert!(n.len >= digi_core::LEN_MIN);
                    }
                }
            }
        }
    }
}
