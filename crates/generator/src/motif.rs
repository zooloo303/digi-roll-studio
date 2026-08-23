// The lead's memory: a motif, and the ways a player develops one.
//
// Port of `js/gen/motif.js`. This is the actual difference between the
// generator and a randomiser. A randomiser picks a note per step and
// forgets it; a player states a short idea and then answers it — the same
// shape a tone higher, upside down, backwards, shoved half a beat late. So
// the lead generates **one** motif and develops it across the progression,
// and [`MotifVariant`] is the whole vocabulary.
//
// A motif is scale-degree *offsets*, not pitches: a [`MotifNote`]'s `step`
// is relative to the start of its phrase and `deg` is a number of scale
// steps from wherever the phrase's tonal centre turns out to be. Keeping it
// abstract is what lets the same idea land on a different chord each phrase
// without a transposition table — Stage 3's lead generator resolves degrees
// to pitches against the bar's own chord.

use crate::rng::{chance, int_range, pick, sample_weighted, Rng};

/// One note of a motif: a relative step, a scale-degree offset from the
/// phrase's tonal centre, and a length in steps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotifNote {
    pub step: u32,
    pub deg: i32,
    pub len: f64,
}

pub const MOTIF_VARIANTS: [MotifVariant; 6] = [
    MotifVariant::Repeat,
    MotifVariant::Transpose,
    MotifVariant::Invert,
    MotifVariant::Retrograde,
    MotifVariant::Displace,
    MotifVariant::Sparse,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotifVariant {
    Repeat,
    Transpose,
    Invert,
    Retrograde,
    Displace,
    Sparse,
}

/// What [`make_motif`] needs.
#[derive(Debug, Clone, Copy)]
pub struct MakeMotifOpts<'a> {
    pub notes: (i64, i64),
    pub window: u32,
    pub weights: &'a [f64],
    pub spread: i32,
}

impl<'a> Default for MakeMotifOpts<'a> {
    fn default() -> Self {
        Self { notes: (3, 5), window: 8, weights: &[], spread: 2 }
    }
}

struct Slot {
    step: u32,
    weight: f64,
}

/// A fresh idea. Steps are drawn from the genre's own weight table so the
/// motif sits where that genre's notes sit; the degree contour is a small
/// random walk, because a melody that leaps every note isn't a melody.
pub fn make_motif(rng: &mut Rng, opts: MakeMotifOpts) -> Vec<MotifNote> {
    let raw = int_range(rng, opts.notes.0, opts.notes.1);
    let count = (i64::from(opts.window).min(raw)).max(1) as usize;
    let mut slots = Vec::new();
    for step in 0..opts.window {
        let w = opts.weights.get((step % 16) as usize).copied().unwrap_or(1.0);
        if w > 0.0 {
            slots.push(Slot { step, weight: if step == 0 { w.max(1.0) } else { w } });
        }
    }
    let mut chosen: Vec<&Slot> = sample_weighted(rng, &slots, count, |s| s.weight);
    chosen.sort_by_key(|s| s.step);
    let chosen_steps: Vec<(u32, f64)> = if chosen.is_empty() {
        vec![(0, 1.0)]
    } else {
        chosen.iter().map(|s| (s.step, s.weight)).collect()
    };

    let mut deg: i32 = 0;
    let mut out = Vec::with_capacity(chosen_steps.len());
    for (i, &(step, _)) in chosen_steps.iter().enumerate() {
        if i > 0 {
            // Mostly steps, occasionally a leap — and pulled back toward the
            // centre when the walk has wandered, so a motif keeps a shape
            // instead of drifting.
            //
            // **Two of these arms return `1` and clippy would like them merged.
            // Do not merge them by reordering.** They are two different reasons
            // to go up — forced back toward the centre, and a coin flip — and the
            // coin flip *draws from the rng*. Any rewrite that changes when
            // `chance` is called changes how many numbers this walk consumes, and
            // every seeded pattern in every genre comes out different: the seed
            // is the promise this generator makes. `||` short-circuits and would
            // preserve the draw, but it also hides that the two cases are
            // unrelated, so the arms stay separate and the lint stays off.
            #[allow(clippy::if_same_then_else)]
            let dir = if deg > opts.spread {
                -1
            } else if deg < -opts.spread {
                1
            } else if chance(rng, 0.5) {
                1
            } else {
                -1
            };
            deg += dir * if chance(rng, 0.25) { 2 } else { 1 };
        }
        let next = chosen_steps.get(i + 1).map(|(s, _)| *s).unwrap_or(opts.window);
        // `.min().max()` rather than `clamp`, for the reason
        // `protocol::pattern::micro_steps_to_byte` gives at length: the two differ
        // on `NaN`, and this chain absorbs one into a legal note length where
        // `clamp` would carry it into `MotifNote::len` and out to a box.
        #[allow(clippy::manual_clamp)]
        let len = (f64::from(next) - f64::from(step)).min(4.0).max(0.5);
        out.push(MotifNote { step, deg, len });
    }
    out
}

/// One development of a motif. Every variant returns a *new* motif and can
/// return the empty list for none of them: [`MotifVariant::Sparse`] thins,
/// [`MotifVariant::Displace`] can push notes off the end of the phrase, and
/// a caller that gets nothing back simply has a bar of space — which is a
/// musical answer too.
pub fn develop_motif(motif: &[MotifNote], variant: MotifVariant, window: u32, rng: &mut Rng) -> Vec<MotifNote> {
    match variant {
        MotifVariant::Transpose => {
            let choices = [-2, -1, 1, 2, 3];
            let by = *pick(rng, &choices).unwrap();
            motif.iter().map(|n| MotifNote { deg: n.deg + by, ..*n }).collect()
        }
        MotifVariant::Invert => {
            // Mirrored around the motif's first note, so the opening pitch
            // is shared and the answer is audibly the same idea upside
            // down.
            let pivot = motif.first().map(|n| n.deg).unwrap_or(0);
            motif.iter().map(|n| MotifNote { deg: 2 * pivot - n.deg, ..*n }).collect()
        }
        MotifVariant::Retrograde => {
            // The degree sequence reversed over the motif's own rhythm. A
            // true time-reversal would also mirror the rhythm, which
            // reliably pushes notes off the phrase; this keeps the groove
            // and reverses the tune.
            let degs: Vec<i32> = motif.iter().rev().map(|n| n.deg).collect();
            motif.iter().zip(degs).map(|(n, deg)| MotifNote { deg, ..*n }).collect()
        }
        MotifVariant::Displace => {
            let by = if chance(rng, 0.5) { 1 } else { 2 };
            motif.iter().map(|n| MotifNote { step: n.step + by, ..*n }).filter(|n| n.step < window).collect()
        }
        MotifVariant::Sparse => {
            if motif.len() <= 1 {
                return motif.to_vec();
            }
            let drop = (motif.len() as i64 - 1).min(int_range(rng, 1, 2)) as usize;
            let rest_pool = &motif[1..];
            let want = (motif.len() - 1).saturating_sub(drop);
            let kept: std::collections::HashSet<u32> =
                sample_weighted(rng, rest_pool, want, |_| 1.0).iter().map(|n| n.step).collect();
            motif.iter().enumerate().filter(|(i, n)| *i == 0 || kept.contains(&n.step)).map(|(_, n)| *n).collect()
        }
        MotifVariant::Repeat => motif.to_vec(),
    }
}

/// Which development each phrase gets. Phrase 1 always states the motif
/// plainly — you can't develop an idea nobody has heard yet — and Looseness
/// decides how far the rest travel: low keeps repeating and transposing,
/// high reaches for inversions, retrogrades and displacement.
pub fn motif_plan(rng: &mut Rng, phrases: u32, looseness: f64) -> Vec<MotifVariant> {
    // `.max().min()` and not `clamp`, the third instance of the same argument:
    // this chain turns a `NaN` looseness into 0.0 (all near variants), where
    // `clamp` would carry it into every weight comparison below and pick by
    // accident. See `protocol::pattern::micro_steps_to_byte`.
    #[allow(clippy::manual_clamp)]
    let loose = looseness.max(0.0).min(100.0) / 100.0;
    let near: [(MotifVariant, f64); 3] =
        [(MotifVariant::Repeat, 3.0), (MotifVariant::Transpose, 3.0), (MotifVariant::Sparse, 1.0)];
    let far: [(MotifVariant, f64); 3] =
        [(MotifVariant::Invert, 2.0), (MotifVariant::Retrograde, 1.5), (MotifVariant::Displace, 1.5)];
    let pool: Vec<(MotifVariant, f64)> = near
        .iter()
        .map(|(v, w)| (*v, w * (1.2 - 0.6 * loose)))
        .chain(far.iter().map(|(v, w)| (*v, w * (0.15 + 1.1 * loose))))
        .collect();

    let mut out = vec![MotifVariant::Repeat];
    for i in 1..phrases {
        // A phrase after a plain repeat leans away from repeating again, so
        // a part never sits on the same bar four times in a row.
        let bias: Vec<&(MotifVariant, f64)> = if out[(i - 1) as usize] == MotifVariant::Repeat {
            pool.iter().filter(|(v, _)| *v != MotifVariant::Repeat).collect()
        } else {
            pool.iter().collect()
        };
        let picked = sample_weighted(rng, &bias, 1, |e| e.1).first().map(|e| e.0).unwrap_or(MotifVariant::Transpose);
        out.push(picked);
    }
    out
}

/// How much of a developed motif survives at a given density. A lead at
/// density 20 plays the bones of the idea; at 100 it plays all of it plus
/// passing notes, which Stage 3's lead generator adds. Ordered by step so
/// what survives still reads as the motif.
pub fn thin_motif(motif: &[MotifNote], density: f64, rng: &mut Rng) -> Vec<MotifNote> {
    // `.max().min()`, for the third time in this file and the same reason: a
    // `NaN` density becomes 0.0 here — thin everything — where `clamp` would
    // leave it `NaN` and every comparison below would answer false, keeping
    // everything. See `protocol::pattern::micro_steps_to_byte`.
    #[allow(clippy::manual_clamp)]
    let keep_all = density.max(0.0).min(100.0) / 100.0;
    if motif.len() <= 1 {
        return motif.to_vec();
    }
    let keep = ((motif.len() as f64 * (0.45 + 0.55 * keep_all)).round() as usize).max(1);
    if keep >= motif.len() {
        return motif.to_vec();
    }
    // The first note always survives — it is what makes the phrase
    // recognisable — and the rest are drawn favouring the longer notes,
    // which are the ones an ear hears as the tune rather than as ornament.
    let rest_pool = &motif[1..];
    let rest = sample_weighted(rng, rest_pool, keep - 1, |n| 1.0 + n.len);
    let mut out: Vec<MotifNote> = std::iter::once(motif[0]).chain(rest.into_iter().copied()).collect();
    out.sort_by_key(|n| n.step);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genres::{genre_profile, role_profile, GenreId, Role};

    fn dnb_lead_weights() -> [f64; 16] {
        role_profile(GenreId::Dnb, Role::Lead).weights
    }

    #[test]
    fn making_a_motif_is_deterministic_for_a_seed() {
        let weights = dnb_lead_weights();
        let opts = || MakeMotifOpts { notes: (3, 5), window: 8, weights: &weights, spread: 2 };
        let a = make_motif(&mut Rng::new(1), opts());
        let b = make_motif(&mut Rng::new(1), opts());
        assert_eq!(a, b);
    }

    #[test]
    fn respects_the_note_count_and_the_phrase_window() {
        let weights = dnb_lead_weights();
        for seed in 0..30u32 {
            let m = make_motif(&mut Rng::new(seed), MakeMotifOpts { notes: (3, 5), window: 8, weights: &weights, spread: 2 });
            assert!(!m.is_empty() && m.len() <= 5);
            for n in &m {
                assert!(n.step < 8);
                assert!(n.len > 0.0);
            }
            let steps: Vec<u32> = m.iter().map(|n| n.step).collect();
            let mut sorted = steps.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, steps);
            let set: std::collections::HashSet<u32> = steps.iter().copied().collect();
            assert_eq!(set.len(), steps.len());
        }
    }

    #[test]
    fn keeps_its_contour_within_reach() {
        let weights = dnb_lead_weights();
        for seed in 0..30u32 {
            let m = make_motif(&mut Rng::new(seed), MakeMotifOpts { notes: (4, 6), window: 8, weights: &weights, spread: 2 });
            for n in &m {
                assert!(n.deg.abs() <= 5);
            }
            for i in 1..m.len() {
                assert!((m[i].deg - m[i - 1].deg).abs() <= 2);
            }
        }
    }

    #[test]
    fn never_produces_an_empty_motif_even_from_a_window_with_nothing_in_it() {
        let weights = [0.0; 16];
        let m = make_motif(&mut Rng::new(4), MakeMotifOpts { notes: (3, 4), window: 4, weights: &weights, spread: 2 });
        assert!(!m.is_empty());
    }

    #[test]
    fn always_starts_the_phrase() {
        let weights = dnb_lead_weights();
        let mut on_one = 0;
        for seed in 0..40u32 {
            let m = make_motif(&mut Rng::new(seed), MakeMotifOpts { notes: (3, 5), window: 8, weights: &weights, spread: 2 });
            if m[0].step == 0 {
                on_one += 1;
            }
        }
        assert!(on_one > 20);
    }

    fn sample_motif() -> Vec<MotifNote> {
        vec![
            MotifNote { step: 0, deg: 0, len: 1.0 },
            MotifNote { step: 2, deg: 1, len: 1.0 },
            MotifNote { step: 4, deg: 3, len: 2.0 },
        ]
    }

    #[test]
    fn repeat_gives_back_the_same_idea() {
        let motif = sample_motif();
        let out = develop_motif(&motif, MotifVariant::Repeat, 8, &mut Rng::new(1));
        assert_eq!(out, motif);
    }

    #[test]
    fn transpose_moves_every_degree_by_the_same_amount_keeping_the_rhythm() {
        let motif = sample_motif();
        let out = develop_motif(&motif, MotifVariant::Transpose, 8, &mut Rng::new(2));
        assert_eq!(out.iter().map(|n| n.step).collect::<Vec<_>>(), vec![0, 2, 4]);
        let deltas: std::collections::HashSet<i32> = out.iter().zip(&motif).map(|(o, m)| o.deg - m.deg).collect();
        assert_eq!(deltas.len(), 1);
        assert_ne!(*deltas.iter().next().unwrap(), 0);
    }

    #[test]
    fn invert_mirrors_around_the_first_note() {
        let motif = sample_motif();
        let out = develop_motif(&motif, MotifVariant::Invert, 8, &mut Rng::new(3));
        assert_eq!(out[0].deg, motif[0].deg);
        assert_eq!(out.iter().map(|n| n.deg).collect::<Vec<_>>(), vec![0, -1, -3]);
    }

    #[test]
    fn retrograde_reverses_the_tune_over_the_same_rhythm() {
        let motif = sample_motif();
        let out = develop_motif(&motif, MotifVariant::Retrograde, 8, &mut Rng::new(4));
        assert_eq!(out.iter().map(|n| n.step).collect::<Vec<_>>(), vec![0, 2, 4]);
        assert_eq!(out.iter().map(|n| n.deg).collect::<Vec<_>>(), vec![3, 1, 0]);
    }

    #[test]
    fn displace_pushes_the_whole_phrase_later_and_drops_what_falls_off_the_end() {
        let motif = sample_motif();
        let out = develop_motif(&motif, MotifVariant::Displace, 5, &mut Rng::new(5));
        for n in &out {
            assert!(n.step < 5);
        }
        assert!(out.len() < motif.len());
        assert!(out[0].step > 0);
    }

    #[test]
    fn sparse_thins_the_idea_but_always_keeps_its_first_note() {
        let motif = sample_motif();
        for seed in 0..20u32 {
            let out = develop_motif(&motif, MotifVariant::Sparse, 8, &mut Rng::new(seed));
            assert!(!out.is_empty());
            assert!(out.len() < motif.len() + 1);
            assert_eq!((out[0].step, out[0].deg), (0, 0));
        }
    }

    #[test]
    fn leaves_the_input_untouched_whatever_the_variant() {
        let motif = sample_motif();
        let before = motif.clone();
        for v in MOTIF_VARIANTS {
            develop_motif(&motif, v, 8, &mut Rng::new(6));
        }
        assert_eq!(motif, before);
    }

    #[test]
    fn states_the_motif_plainly_first() {
        for seed in 0..20u32 {
            assert_eq!(motif_plan(&mut Rng::new(seed), 4, 50.0)[0], MotifVariant::Repeat);
        }
    }

    #[test]
    fn is_one_variant_per_phrase_all_known() {
        let plan = motif_plan(&mut Rng::new(2), 8, 60.0);
        assert_eq!(plan.len(), 8);
        for v in plan {
            assert!(MOTIF_VARIANTS.contains(&v));
        }
    }

    #[test]
    fn never_repeats_twice_in_a_row() {
        for seed in 0..30u32 {
            let plan = motif_plan(&mut Rng::new(seed), 8, 50.0);
            for i in 1..plan.len() {
                if plan[i - 1] == MotifVariant::Repeat {
                    assert_ne!(plan[i], MotifVariant::Repeat);
                }
            }
        }
    }

    #[test]
    fn reaches_further_at_high_looseness_than_at_low() {
        let far = [MotifVariant::Invert, MotifVariant::Retrograde, MotifVariant::Displace];
        let count = |looseness: f64| -> usize {
            let mut n = 0;
            for seed in 0..60u32 {
                let plan = motif_plan(&mut Rng::new(seed), 8, looseness);
                n += plan.iter().filter(|v| far.contains(v)).count();
            }
            n
        };
        assert!(count(100.0) > count(5.0) * 2);
    }

    #[test]
    fn motif_plan_is_deterministic_for_a_seed() {
        assert_eq!(motif_plan(&mut Rng::new(9), 6, 40.0), motif_plan(&mut Rng::new(9), 6, 40.0));
    }

    fn thinning_motif() -> Vec<MotifNote> {
        (0..6u32).map(|step| MotifNote { step, deg: (step % 3) as i32, len: 1.0 }).collect()
    }

    #[test]
    fn keeps_more_at_high_density_than_at_low() {
        let motif = thinning_motif();
        let at = |d: f64| thin_motif(&motif, d, &mut Rng::new(3)).len();
        assert!(at(100.0) >= at(0.0));
        assert!(at(0.0) >= 1);
    }

    #[test]
    fn keeps_the_first_note_and_stays_in_order() {
        let motif = thinning_motif();
        for seed in 0..20u32 {
            let out = thin_motif(&motif, 30.0, &mut Rng::new(seed));
            assert_eq!(out[0].step, 0);
            let steps: Vec<u32> = out.iter().map(|n| n.step).collect();
            let mut sorted = steps.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, steps);
        }
    }

    #[test]
    fn leaves_a_one_note_motif_alone() {
        let one = vec![MotifNote { step: 0, deg: 0, len: 1.0 }];
        assert_eq!(thin_motif(&one, 0.0, &mut Rng::new(1)), one);
    }

    #[test]
    fn every_genre_lead_profile_produces_a_usable_motif() {
        // Not in the JS oracle, but cheap insurance: `make_motif` is only
        // ever handed a real genre's lead weights, never `EVEN` or `[0;16]`.
        for genre in GenreId::ALL {
            let weights = role_profile(genre, Role::Lead).weights;
            let m = make_motif(&mut Rng::new(1), MakeMotifOpts { notes: (3, 5), window: 8, weights: &weights, spread: 2 });
            assert!(!m.is_empty());
        }
        let _ = genre_profile(GenreId::Dnb);
    }
}
