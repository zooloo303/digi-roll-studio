// The bassline.
//
// Port of `js/gen/parts/bass.js`. Generated first in a row's own place in
// the arrangement order, which makes it the part later rows react to: what
// it returns goes into the shared rhythm map, and a lead's density is
// penalised on the steps it owns.
//
// Pitch vocabulary is deliberately small — roots, fifths, octaves, the
// chord's own seventh, and an approach tone into a chord change. A bassline
// is a rhythm part that happens to have notes in it, so the interest lives
// in the trig list, the velocities and the ghosts rather than in the
// melody.

use std::collections::HashSet;

use crate::context::ResolvedContext;
use crate::genres::RoleProfile;
use crate::rhythm::{gap_after, micro_for, rhythm_for, trig_feel_for, velocity_for, RhythmOpts};
use crate::rng::{chance, weighted, Rng};
use crate::theory::{chord_tones, fold_into_window, slot_root_pitch, snap_to_scale_pitch, window_for, ChordWindow, Key};

use super::{len_bounds, GeneratedPart, NoteSpec};

/// The pitches a bassline reaches for beyond the root, weighted so the
/// nearest chord tone (a third or a fifth) wins more often than the rest.
fn pick_chord_tone(rng: &mut Rng, tones: &[u8], root: i32) -> i32 {
    let above: Vec<i32> = tones.iter().map(|&p| i32::from(p)).filter(|&p| p != root).collect();
    if above.is_empty() {
        return root;
    }
    let items: Vec<(i32, f64)> = above.iter().enumerate().map(|(i, &p)| (p, if i == 0 { 0.5 } else { 1.0 })).collect();
    weighted(rng, &items, |it| it.1).map(|it| it.0).unwrap_or(root)
}

pub fn generate_bass(ctx: &ResolvedContext, profile: &RoleProfile, octave: u8, density: u8, rng: &mut Rng, busy: &HashSet<u32>) -> GeneratedPart {
    let (min, max) = window_for(profile.span, i32::from(octave));
    let total = ctx.length_steps;
    let key = Key { root: ctx.key_root, intervals: ctx.key_intervals };

    let trigs = rhythm_for(
        RhythmOpts {
            weights: &profile.weights,
            density: u32::from(density),
            bars: ctx.bars,
            busy,
            avoid: 0.0,
            // The 1 always plays. A bassline that can be missing its downbeat
            // is a different feature, and one the density slider shouldn't
            // be able to reach.
            anchors: if profile.anchor_len.is_some() { &[0] } else { &[] },
            trigs_per_bar: profile.trigs_per_bar,
        },
        rng,
    );

    let feel = trig_feel_for(&trigs, profile.conditions, ctx.feel.looseness as u32, ctx.bars, rng);
    let (len_normal, len_ghost, len_max) = len_bounds(&profile.len);

    let mut notes = Vec::with_capacity(trigs.len());
    for (i, trig) in trigs.iter().enumerate() {
        let slot = ctx.bar_slots[trig.bar as usize];
        let root = slot_root_pitch(&slot, key, i32::from(octave), min, max);
        let tones = chord_tones(&slot, key, ChordWindow { octave: i32::from(octave), min: min.clamp(0, 127) as u8, max: max.clamp(0, 127) as u8, ..Default::default() });

        // The pitch decisions read only the genre profile and the seed,
        // never the feel sliders: Motion is about p-lock automation and
        // Looseness about trig conditions, so moving either must not
        // rewrite the notes.
        let next_slot = ctx.bar_slots.get(trig.bar as usize + 1).copied().unwrap_or(ctx.bar_slots[0]);
        let last_of_bar = trigs.get(i + 1).map(|t| t.bar) != Some(trig.bar);

        let pitch = if last_of_bar && next_slot != slot && chance(rng, profile.approach.unwrap_or(0.0)) {
            // An approach tone into the next chord: a scale tone a step away
            // from where the next bar starts, which is what makes a loop
            // turn over instead of restarting.
            let target = slot_root_pitch(&next_slot, key, i32::from(octave), min, max);
            let offset = if chance(rng, 0.5) { -2 } else { -1 };
            snap_to_scale_pitch(target + offset, key.root, key.intervals)
        } else if trig.accent {
            root
        } else if chance(rng, profile.octave_leap.unwrap_or(0.0)) {
            root + if chance(rng, 0.75) { 12 } else { -12 }
        } else if !trig.ghost && chance(rng, 0.45) {
            pick_chord_tone(rng, &tones, root)
        } else {
            root
        };
        let pitch = fold_into_window(pitch, min, max);

        // Length: the anchor holds, everything else plays to the next trig
        // at most, and every value is snapped to the boxes' own LEN scale so
        // what the roll draws is what the hardware stores.
        let gap = gap_after(&trigs, i, total);
        // The anchor's own length when this is the anchor and the profile gives
        // one; otherwise the ordinary length. One `match` rather than an
        // `is_some()` guard and an `unwrap()` — same answer, and no unwrap on the
        // generator's hot path for a future edit to get wrong.
        let want = match profile.anchor_len.filter(|_| trig.step == 0) {
            Some(anchor) => anchor.min(gap),
            None => (if trig.ghost { len_ghost } else { len_normal }).min(gap).min(len_max),
        };
        let len = digi_core::snap_len_fine(want, f64::from(total - trig.step));

        let t = feel.get(&trig.step);
        notes.push(NoteSpec {
            step: trig.step,
            pitch: pitch.clamp(0, 127) as u8,
            len,
            velocity: velocity_for(trig.accent, trig.ghost, profile.velocity, u32::from(ctx.feel.humanize), rng),
            micro: micro_for(trig.step, &ctx.profile.groove, u32::from(ctx.feel.humanize), rng),
            prob: t.and_then(|t| t.prob),
            fill: t.and_then(|t| t.fill),
            cond: t.and_then(|t| t.cond),
        });
    }

    GeneratedPart { notes, trigs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{resolve_context, GenContext};
    use crate::genres::{genre_profile, role_profile, GenreId, Role};
    use crate::rng::rng_for;

    fn ctx_for(genre: GenreId, seed: u32, bars: u32) -> ResolvedContext {
        let ctx = GenContext { genre, seed, bars, ..GenContext::default() };
        resolve_context(&ctx).unwrap()
    }

    fn generate(genre: GenreId, seed: u32, bars: u32, density: u8, octave: u8) -> GeneratedPart {
        let ctx = ctx_for(genre, seed, bars);
        let profile = role_profile(genre, Role::Bass);
        let mut rng = rng_for(seed, "bass");
        generate_bass(&ctx, &profile, octave, density, &mut rng, &HashSet::new())
    }

    fn expect_playable(part: &GeneratedPart, ctx: &ResolvedContext, min: i32, max: i32) {
        for n in &part.notes {
            assert!(n.step < ctx.length_steps);
            assert!(i32::from(n.pitch) >= min && i32::from(n.pitch) <= max);
            assert!(n.len >= digi_core::LEN_MIN);
            assert!(n.len <= f64::from(ctx.length_steps - n.step));
            assert!((1..=127).contains(&n.velocity));
            assert!(n.micro.abs() <= crate::rhythm::MICRO_LIMIT + 1e-9);
            let ticks = n.micro / crate::rhythm::MICRO_TICK;
            assert!((ticks - ticks.round()).abs() < 1e-9);
        }
    }

    #[test]
    fn produces_notes_the_hardware_can_hold_at_every_density() {
        for genre in GenreId::ALL {
            for density in [0u8, 40, 100] {
                for seed in 0..6u32 {
                    let ctx = ctx_for(genre, seed, 2);
                    let (min, max) = window_for(role_profile(genre, Role::Bass).span, 2);
                    let mut rng = rng_for(seed, "bass");
                    let part = generate_bass(&ctx, &role_profile(genre, Role::Bass), 2, density, &mut rng, &HashSet::new());
                    assert!(!part.notes.is_empty());
                    expect_playable(&part, &ctx, min, max);
                }
            }
        }
    }

    #[test]
    fn is_deterministic_for_a_seed_and_different_for_another() {
        let a = generate(GenreId::Dnb, 4242, 2, 55, 2);
        let b = generate(GenreId::Dnb, 4242, 2, 55, 2);
        assert_eq!(a.notes, b.notes);
        let c = generate(GenreId::Dnb, 4243, 2, 55, 2);
        assert_ne!(a.notes, c.notes);
    }

    #[test]
    fn writes_no_conditions_at_all_at_looseness_0() {
        let ctx = GenContext { genre: GenreId::Dnb, seed: 11, feel: crate::context::Feel { motion: 50, looseness: 0, humanize: 30 }, ..GenContext::default() };
        let resolved = resolve_context(&ctx).unwrap();
        let mut rng = rng_for(11, "bass");
        let part = generate_bass(&resolved, &role_profile(GenreId::Dnb, Role::Bass), 2, 55, &mut rng, &HashSet::new());
        for n in &part.notes {
            assert_eq!((n.prob, n.fill, n.cond), (None, None, None));
        }
    }

    #[test]
    fn always_plays_the_1() {
        for genre in GenreId::ALL {
            for seed in 0..10u32 {
                let part = generate(genre, seed, 2, 55, 2);
                assert!(part.notes.iter().any(|n| n.step == 0));
            }
        }
    }

    #[test]
    fn is_one_note_per_step() {
        let part = generate(GenreId::Dnb, 3, 2, 55, 2);
        let steps: HashSet<u32> = part.notes.iter().map(|n| n.step).collect();
        assert_eq!(steps.len(), part.notes.len());
    }

    #[test]
    fn sits_mostly_on_the_root_of_whatever_chord_the_bar_is_on() {
        let ctx = GenContext { genre: GenreId::Dnb, seed: 5, progression: "i VI".into(), ..GenContext::default() };
        let resolved = resolve_context(&ctx).unwrap();
        let mut rng = rng_for(5, "bass");
        let part = generate_bass(&resolved, &role_profile(GenreId::Dnb, Role::Bass), 2, 55, &mut rng, &HashSet::new());
        let mut on_root = 0;
        for n in &part.notes {
            let bar = (n.step / 16) as usize;
            let degree = resolved.bar_slots[bar].degree;
            let root_class = (resolved.key_root + resolved.key_intervals[(degree - 1) as usize]).rem_euclid(12);
            if i32::from(n.pitch).rem_euclid(12) == root_class {
                on_root += 1;
            }
        }
        assert!(on_root as f64 / part.notes.len() as f64 > 0.4);
    }

    #[test]
    fn puts_houses_bass_on_the_off_beats() {
        let part = generate(GenreId::House, 2, 1, 55, 2);
        let offbeat = part.notes.iter().filter(|n| n.step % 4 == 2).count();
        assert!(offbeat as f64 / part.notes.len() as f64 > 0.5);
    }

    #[test]
    fn keeps_electros_bass_staccato_and_busy() {
        let dnb = generate(GenreId::Dnb, 8, 2, 55, 2);
        let electro = generate(GenreId::Electro, 8, 2, 55, 2);
        assert!(electro.notes.len() > dnb.notes.len());
        assert!(electro.notes.iter().map(|n| n.len).fold(0.0, f64::max) <= 1.0);
    }

    #[test]
    fn holds_every_genres_bassline_long_enough_to_join_up() {
        // The JS's bass lengths made a line that read as short in every
        // genre — notes stopping well inside the gap to the next trig — so
        // `genres.rs` doubles them. Two things pinned here, because both
        // were wrong before and neither is visible from a note count.
        for genre in GenreId::ALL {
            let profile = role_profile(genre, Role::Bass);
            let (normal, ghost, _) = len_bounds(&profile.len);
            // An ordinary note is at least half a step. Below that a
            // bassline is a click track with pitches on it.
            assert!(normal >= 0.5, "{genre:?}: normal length {normal}");
            assert!(ghost <= normal, "{genre:?}: ghost {ghost} outlasts normal {normal}");
            // The anchor on the 1 is never shorter than the notes around
            // it: three genres' anchors used to be, once the ordinary
            // lengths came up.
            if let Some(anchor) = profile.anchor_len {
                assert!(anchor >= normal, "{genre:?}: anchor {anchor} shorter than normal {normal}");
            }
        }
    }

    #[test]
    fn holds_dnbs_anchor_note_on_the_1() {
        let part = generate(GenreId::Dnb, 6, 2, 20, 2);
        let first = part.notes.iter().find(|n| n.step == 0).unwrap();
        assert!(first.len > 1.0);
    }

    #[test]
    fn works_at_every_pattern_length() {
        for bars in [1u32, 2, 4, 8] {
            let part = generate(GenreId::Dnb, 7, bars, 55, 2);
            let (min, max) = window_for(role_profile(GenreId::Dnb, Role::Bass).span, 2);
            let ctx = ctx_for(GenreId::Dnb, 7, bars);
            expect_playable(&part, &ctx, min, max);
            assert!(part.notes.iter().map(|n| n.step).max().unwrap() < bars * 16);
        }
    }

    #[test]
    fn every_genre_default_profile_resolves() {
        for genre in GenreId::ALL {
            let _ = genre_profile(genre);
        }
    }
}
