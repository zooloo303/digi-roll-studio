// A drum voice.
//
// **Not a port.** `js/gen/` has no drums role at all — `GEN_ROLES` is
// `['bass', 'chords', 'lead']`, and every per-genre profile in `genres.js`
// covers exactly those three. This is new design, settled with Neil
// 2026-08-19 (PLAN.md Phase 7 stage 5): drums work on either box, one part
// per voice, exactly like bass/chords/lead — a voice's destination track
// *is* the sound (a DT2 sample slot, or whichever DN2 patch someone points
// it at), so there is no register to choose and no chord to answer. What a
// voice needs is only its own rhythm, which `rhythm.rs`'s engine already
// gives it for free: it is role-agnostic, and was built that way on
// purpose.
//
// A voice's pitch is therefore a constant — [`DRUM_TRIGGER_PITCH`] — rather
// than a decision. Nothing here reads a scale, a key or an octave.

use std::collections::HashSet;

use crate::context::ResolvedContext;
use crate::genres::RoleProfile;
use crate::rhythm::{gap_after, micro_for, rhythm_for, trig_feel_for, velocity_for, RhythmOpts};
use crate::rng::Rng;

use super::{len_bounds, GeneratedPart, NoteSpec};

/// Every drum note is stamped at this pitch. The destination track already
/// says which drum it is — a DT2 track holds one sample, so its pitch does
/// not choose *which* sound plays — so the value only needs to be a legal,
/// unremarkable MIDI note. 60 is C5, this app's own convention for "the
/// middle of the keyboard" (`digi_core::chords`'s octave labelling agrees:
/// MIDI 60 is C5 here, not C4).
pub const DRUM_TRIGGER_PITCH: u8 = 60;

/// Generate one drum voice: a rhythm, and nothing else. Structurally the
/// same shape as [`crate::parts::bass::generate_bass`] minus every pitch
/// decision — `avoid` is always 0, because a kick landing under a hi-hat is
/// the point, not a collision the way a lead doubling a bass would be.
pub fn generate_drums(ctx: &ResolvedContext, profile: &RoleProfile, density: u8, rng: &mut Rng, busy: &HashSet<u32>) -> GeneratedPart {
    let total = ctx.length_steps;

    let trigs = rhythm_for(
        RhythmOpts {
            weights: &profile.weights,
            density: u32::from(density),
            bars: ctx.bars,
            busy,
            avoid: 0.0,
            anchors: &[],
            trigs_per_bar: profile.trigs_per_bar,
        },
        rng,
    );

    let feel = trig_feel_for(&trigs, profile.conditions, ctx.feel.looseness as u32, ctx.bars, rng);
    let (len_normal, len_ghost, len_max) = len_bounds(&profile.len);

    let mut notes = Vec::with_capacity(trigs.len());
    for (i, trig) in trigs.iter().enumerate() {
        let gap = gap_after(&trigs, i, total);
        let want = (if trig.ghost { len_ghost } else { len_normal }).min(gap).min(len_max);
        let len = digi_core::snap_len_fine(want, f64::from(total - trig.step));
        let t = feel.get(&trig.step);
        notes.push(NoteSpec {
            step: trig.step,
            pitch: DRUM_TRIGGER_PITCH,
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
    use crate::genres::{role_profile, GenreId, Role};
    use crate::rng::rng_for;

    fn ctx_for(genre: GenreId, seed: u32, bars: u32) -> ResolvedContext {
        resolve_context(&GenContext { genre, seed, bars, ..GenContext::default() }).unwrap()
    }

    /// `GenContext::default()`'s looseness is 35 (`context.rs`), which is
    /// enough to make PROB/COND tests flaky between runs. The PROB/COND
    /// tests below need looseness pinned at a known value, per the packet's
    /// own "looseness 100" spec — this is that pin.
    fn ctx_for_looseness(genre: GenreId, seed: u32, bars: u32, looseness: u8) -> ResolvedContext {
        resolve_context(&GenContext {
            genre,
            seed,
            bars,
            feel: crate::context::Feel { looseness, ..crate::context::Feel::default() },
            ..GenContext::default()
        })
        .unwrap()
    }

    #[test]
    fn every_voice_of_every_genre_produces_notes_the_hardware_can_hold() {
        for genre in GenreId::ALL {
            for voice in Role::DRUM_VOICES {
                for density in [0u8, 40, 100] {
                    for seed in 0..4u32 {
                        let ctx = ctx_for(genre, seed, 2);
                        let profile = role_profile(genre, voice);
                        let mut rng = rng_for(seed, "drum");
                        let part = generate_drums(&ctx, &profile, density, &mut rng, &HashSet::new());
                        assert!(!part.notes.is_empty(), "{genre:?}/{voice:?} density {density}");
                        for n in &part.notes {
                            assert!(n.step < ctx.length_steps);
                            assert_eq!(n.pitch, DRUM_TRIGGER_PITCH);
                            assert!(n.len >= digi_core::LEN_MIN);
                            assert!(n.len <= f64::from(ctx.length_steps - n.step));
                            assert!((1..=127).contains(&n.velocity));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn is_deterministic_for_a_seed_and_different_for_another() {
        let ctx = ctx_for(GenreId::Dnb, 42, 2);
        let profile = role_profile(GenreId::Dnb, Role::Kick);
        let a = generate_drums(&ctx, &profile, 50, &mut rng_for(42, "kick"), &HashSet::new());
        let b = generate_drums(&ctx, &profile, 50, &mut rng_for(42, "kick"), &HashSet::new());
        assert_eq!(a.notes, b.notes);
        let c = generate_drums(&ctx, &profile, 50, &mut rng_for(43, "kick"), &HashSet::new());
        assert_ne!(a.notes, c.notes);
    }

    #[test]
    fn house_kick_is_steady_four_on_the_floor_at_full_density() {
        let ctx = ctx_for(GenreId::House, 1, 1);
        let profile = role_profile(GenreId::House, Role::Kick);
        let part = generate_drums(&ctx, &profile, 100, &mut rng_for(1, "kick"), &HashSet::new());
        let steps: HashSet<u32> = part.notes.iter().map(|n| n.step).collect();
        for beat in [0, 4, 8, 12] {
            assert!(steps.contains(&beat), "missing beat {beat}");
        }
    }

    #[test]
    fn ignores_the_busy_map_a_kick_and_a_hat_may_share_a_step() {
        // Unlike the lead answering the bass, drum voices are not supposed
        // to avoid each other — a kick under a closed hat on the 1 is the
        // point of a drum kit, not a collision.
        let ctx = ctx_for(GenreId::House, 2, 1);
        let kick_profile = role_profile(GenreId::House, Role::Kick);
        let kick = generate_drums(&ctx, &kick_profile, 100, &mut rng_for(2, "kick"), &HashSet::new());
        let busy: HashSet<u32> = kick.notes.iter().map(|n| n.step).collect();
        let hat_profile = role_profile(GenreId::House, Role::ClosedHat);
        let hat = generate_drums(&ctx, &hat_profile, 100, &mut rng_for(2, "hat"), &busy);
        // House hats sit on 0,2,4,... and the kick sits on 0,4,8,12 — they
        // are expected to overlap on the downbeats, which they may only do
        // if `avoid` truly is 0.
        assert!(hat.notes.iter().any(|n| busy.contains(&n.step)));
    }

    #[test]
    fn house_clap_lands_on_the_backbeat_with_the_snare() {
        // A clap doubling the snare is the point of having both, so the two
        // are expected to agree on steps 4 and 12 rather than avoid them.
        let ctx = ctx_for(GenreId::House, 3, 1);
        let clap = generate_drums(&ctx, &role_profile(GenreId::House, Role::Clap), 100, &mut rng_for(3, "clap"), &HashSet::new());
        let steps: HashSet<u32> = clap.notes.iter().map(|n| n.step).collect();
        for beat in [4, 12] {
            assert!(steps.contains(&beat), "missing backbeat {beat}");
        }
    }

    #[test]
    fn a_rimshot_never_takes_a_beat_and_a_ride_only_takes_eighths() {
        // The two voices added to fill gaps rather than mark them: a rimshot
        // is off-beat by construction (every `weights[n % 4 == 0]` is 0), and
        // a ride is an eighth-note voice, so neither may land on a step its
        // table forbids however high the density goes.
        for genre in GenreId::ALL {
            let ctx = ctx_for(genre, 7, 2);
            let rim = generate_drums(&ctx, &role_profile(genre, Role::Rimshot), 100, &mut rng_for(7, "rim"), &HashSet::new());
            for n in &rim.notes {
                assert!(n.step % 4 != 0, "{genre:?} rimshot on beat step {}", n.step);
            }
            let ride = generate_drums(&ctx, &role_profile(genre, Role::Ride), 100, &mut rng_for(7, "ride"), &HashSet::new());
            for n in &ride.notes {
                assert!(n.step % 2 == 0, "{genre:?} ride off the eighth grid at step {}", n.step);
            }
        }
    }

    #[test]
    fn every_genre_and_voice_resolves_a_profile() {
        // `lanes` is still empty (p-lock lanes for drums are future scope),
        // but `conditions` is not: every drum voice now carries a sprinkle
        // of PROB/COND, per Neil's 2026-08-20 decision. This replaces the
        // old `assert!(profile.conditions.is_empty())` with its opposite —
        // pinning the feature landing rather than pinning it staying off.
        for genre in GenreId::ALL {
            for voice in Role::DRUM_VOICES {
                let profile = role_profile(genre, voice);
                assert_eq!(profile.weights.len(), 16);
                assert!(profile.lanes.is_empty());
                assert!(!profile.conditions.is_empty(), "{genre:?}/{voice:?} has no drum recipe");
            }
        }
    }

    // --- Drum PROB/COND: a sprinkle, not the melodic dose --------------------

    #[test]
    fn the_downbeat_survives_no_spine_voice_ever_carries_altbar() {
        // The safety fact this whole packet turns on: `rhythm::is_beat`
        // steps are always accented (`rhythm.rs:139`), and `ConditionRecipe::AltBar`
        // is the one recipe kind whose closure in `trig_feel_for` does not
        // check `trig.accent`. If the kit's spine (Kick, Snare, Clap) ever
        // picked up `AltBar`, a downbeat could get a `cond` and go silent on
        // alternate loops. This pins the recipe *choice*, not just today's
        // numbers — it must keep failing however the consts are retuned.
        for genre in GenreId::ALL {
            for voice in [Role::Kick, Role::Snare, Role::Clap] {
                for seed in 0..12u32 {
                    let ctx = ctx_for_looseness(genre, seed, 2, 100);
                    let profile = role_profile(genre, voice);
                    let part = generate_drums(&ctx, &profile, 100, &mut rng_for(seed, "spine"), &HashSet::new());
                    for n in &part.notes {
                        if crate::rhythm::is_beat(n.step) {
                            assert_eq!(
                                n.cond, None,
                                "{genre:?}/{voice:?} seed {seed} step {} got a cond on a downbeat",
                                n.step
                            );
                            assert!(
                                n.prob.is_none() || n.prob == Some(100),
                                "{genre:?}/{voice:?} seed {seed} step {} got prob {:?} on a downbeat",
                                n.step,
                                n.prob
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn drum_voices_get_a_sprinkle_lighter_than_the_melodic_roles() {
        // "A sprinkle" as an assertion: something has to arrive (a genre
        // whose eight drum voices produce zero locks over a spread of seeds
        // would mean the recipes never fire), and the per-trig rate has to
        // sit below the melodic roles' on the same genre and seed — Neil's
        // "perhaps it shouldn't be as heavy... but a sprinkle... wouldn't go
        // amiss."
        use crate::parts::bass::generate_bass;
        use crate::parts::chords::generate_chords;
        use crate::parts::lead::generate_lead;

        fn locks(notes: &[NoteSpec]) -> usize {
            notes.iter().filter(|n| n.prob.is_some() || n.fill.is_some() || n.cond.is_some()).count()
        }

        for genre in GenreId::ALL {
            let mut drum_trigs = 0usize;
            let mut drum_locks = 0usize;
            for voice in Role::DRUM_VOICES {
                for seed in 0..8u32 {
                    let ctx = ctx_for_looseness(genre, seed, 2, 100);
                    let profile = role_profile(genre, voice);
                    let part = generate_drums(&ctx, &profile, 100, &mut rng_for(seed, "sprinkle-drum"), &HashSet::new());
                    drum_trigs += part.notes.len();
                    drum_locks += locks(&part.notes);
                }
            }
            assert!(drum_locks > 0, "{genre:?}: drum voices produced no PROB/FILL/COND at all");

            let mut melodic_trigs = 0usize;
            let mut melodic_locks = 0usize;
            for role in Role::MELODIC {
                for seed in 0..8u32 {
                    let ctx = ctx_for_looseness(genre, seed, 2, 100);
                    let profile = role_profile(genre, role);
                    let notes: Vec<NoteSpec> = match role {
                        Role::Bass => generate_bass(&ctx, &profile, 2, 100, &mut rng_for(seed, "sprinkle-mel"), &HashSet::new()).notes,
                        Role::Chords => generate_chords(&ctx, &profile, 2, 100, &mut rng_for(seed, "sprinkle-mel"), &HashSet::new()).notes,
                        Role::Lead => generate_lead(&ctx, &profile, 2, 100, &mut rng_for(seed, "sprinkle-mel"), &HashSet::new()).notes,
                        _ => unreachable!("Role::MELODIC only ever yields these three"),
                    };
                    melodic_trigs += notes.len();
                    melodic_locks += locks(&notes);
                }
            }

            let drum_rate = drum_locks as f64 / drum_trigs as f64;
            let melodic_rate = melodic_locks as f64 / melodic_trigs as f64;
            assert!(
                drum_rate < melodic_rate,
                "{genre:?}: drum lock rate {drum_rate:.4} ({drum_locks}/{drum_trigs}) not lower than melodic {melodic_rate:.4} ({melodic_locks}/{melodic_trigs})"
            );
        }
    }

    #[test]
    fn looseness_0_writes_no_drum_conditions_at_all() {
        // `bass.rs` has `writes_no_conditions_at_all_at_looseness_0`; drums
        // need their own, even though `trig_feel_for`'s early return on
        // `loose <= 0.0` means this passes on day one. It is the guard on
        // the whole feature being opt-out, so it stays pinned rather than
        // assumed.
        for genre in GenreId::ALL {
            for voice in Role::DRUM_VOICES {
                let gen_ctx = GenContext { genre, seed: 21, feel: crate::context::Feel { motion: 50, looseness: 0, humanize: 30 }, ..GenContext::default() };
                let ctx = resolve_context(&gen_ctx).unwrap();
                let profile = role_profile(genre, voice);
                let part = generate_drums(&ctx, &profile, 100, &mut rng_for(21, "loose0"), &HashSet::new());
                for n in &part.notes {
                    assert_eq!((n.prob, n.fill, n.cond), (None, None, None), "{genre:?}/{voice:?}");
                }
            }
        }
    }

    #[test]
    fn drum_conditions_are_deterministic_for_a_seed() {
        // The existing `is_deterministic_for_a_seed_and_different_for_another`
        // above covers the notes; this pins the new PROB/COND path
        // specifically, at a looseness where the recipes actually fire.
        for genre in GenreId::ALL {
            for voice in Role::DRUM_VOICES {
                let ctx = ctx_for_looseness(genre, 99, 2, 100);
                let profile = role_profile(genre, voice);
                let a = generate_drums(&ctx, &profile, 100, &mut rng_for(99, "det"), &HashSet::new());
                let b = generate_drums(&ctx, &profile, 100, &mut rng_for(99, "det"), &HashSet::new());
                let feel_a: Vec<_> = a.notes.iter().map(|n| (n.step, n.prob, n.fill, n.cond)).collect();
                let feel_b: Vec<_> = b.notes.iter().map(|n| (n.step, n.prob, n.fill, n.cond)).collect();
                assert_eq!(feel_a, feel_b, "{genre:?}/{voice:?}");
            }
        }
    }
}
