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

/// How a response answers the call it just heard.
///
/// [`MotifVariant`] is what a player does to their *own* idea across a
/// phrase. This is a different list, because an answer has a different job:
/// it must be recognisable as a reply to something the ear heard a bar ago,
/// so it either quotes the call outright ([`Sequence`](Self::Sequence),
/// [`TailEcho`](Self::TailEcho)), turns it over
/// ([`Invert`](Self::Invert), [`Retrograde`](Self::Retrograde)), or keeps
/// its rhythm and walks the tune home ([`Resolve`](Self::Resolve)).
///
/// Three of the five are [`develop_motif`] under another name — an answer
/// by inversion *is* an inversion — and they delegate rather than restate
/// it, so a fix to how inversion pivots reaches both callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnswerVariant {
    /// The call again, moved up or down a step or two — the plainest
    /// "yes, and": same shape, new pitch level. Weighted downward, because
    /// an answer that falls sounds like an answer and one that rises
    /// sounds like a second question.
    Sequence,
    Invert,
    Retrograde,
    /// The last two or three notes of the call, restated at the top of the
    /// answer. The move a horn section makes: quote the end of what was
    /// just played, then go somewhere with it.
    TailEcho,
    /// The call's rhythm with its tune walked stepwise back to the
    /// phrase's own centre — the consequent that closes what the antecedent
    /// left open.
    Resolve,
}

pub const ANSWER_VARIANTS: [AnswerVariant; 5] = [
    AnswerVariant::Sequence,
    AnswerVariant::Invert,
    AnswerVariant::Retrograde,
    AnswerVariant::TailEcho,
    AnswerVariant::Resolve,
];

/// Answer a phrase. `call` is what the other voice actually played in its
/// turn — already developed and thinned, not the bare motif — because an
/// answer replies to what was *heard*, not to the idea behind it.
///
/// Like [`develop_motif`] this may return fewer notes than it was given,
/// and never returns more; a caller that gets an empty list has a turn of
/// space, which is a reply too. Every returned step is inside `window`.
pub fn answer_motif(call: &[MotifNote], variant: AnswerVariant, window: u32, rng: &mut Rng) -> Vec<MotifNote> {
    if call.is_empty() {
        return Vec::new();
    }
    let out = match variant {
        // Two of the five entries fall by one, so a plain `pick` over this
        // slice is the weighting — five outcomes, `-1` twice as likely as
        // any other, and no second rng draw to bias it with.
        AnswerVariant::Sequence => {
            let by = *pick(rng, &[-3, -2, -1, -1, 2]).unwrap();
            call.iter().map(|n| MotifNote { deg: n.deg + by, ..*n }).collect()
        }
        AnswerVariant::Invert => develop_motif(call, MotifVariant::Invert, window, rng),
        AnswerVariant::Retrograde => develop_motif(call, MotifVariant::Retrograde, window, rng),
        AnswerVariant::TailEcho => {
            let take = call.len().min(if chance(rng, 0.5) { 2 } else { 3 }).max(1);
            let tail = &call[call.len() - take..];
            // Re-timed so the quote starts the answer, keeping the gaps
            // between the quoted notes exactly as they were — that spacing
            // is most of what makes an ear recognise the quote.
            let first = tail[0].step;
            tail.iter().map(|n| MotifNote { step: n.step - first, ..*n }).collect()
        }
        AnswerVariant::Resolve => {
            // The call's rhythm, its tune replaced by a walk that picks up
            // where the call left off and takes it home. A *ramp* and not a
            // step-at-a-time descent, because the two differ whenever the
            // distance and the note count disagree: stepping by one arrives
            // early and then sits on the centre for the rest of the phrase,
            // which is a rest with extra notes rather than a resolution.
            // Spreading the distance over the gaps instead means the last
            // note is always the arrival, however far there was to go.
            //
            // A call that already ended on the centre has left nothing to
            // resolve, so the walk starts from wherever its contour reached
            // furthest instead — the peak of the phrase is the thing an ear
            // is still holding on to.
            let from = match call.last().map(|n| n.deg).unwrap_or(0) {
                0 => call.iter().map(|n| n.deg).max_by_key(|d| d.abs()).unwrap_or(0),
                d => d,
            };
            let gaps = (call.len() - 1).max(1) as f64;
            call.iter()
                .enumerate()
                .map(|(i, n)| {
                    let remaining = (call.len() - 1 - i) as f64;
                    MotifNote { deg: (f64::from(from) * remaining / gaps).round() as i32, ..*n }
                })
                .collect()
        }
    };
    out.into_iter().filter(|n| n.step < window).collect()
}

/// Which answer each of the response's turns gets.
///
/// The shape is fixed even though the picks are not: the **first** answer
/// always quotes the call, because an answer nobody recognises as one is
/// just a second lead, and the **last** always resolves, so the loop closes
/// rather than trailing off. Looseness decides everything in between, and
/// with one turn there is no between — a single answer is drawn from the
/// whole pool, quoting at low looseness and turning the idea over at high.
pub fn answer_plan(rng: &mut Rng, turns: u32, looseness: f64) -> Vec<AnswerVariant> {
    // `.max().min()` and not `clamp`, for the reason `motif_plan` gives
    // just above: a `NaN` looseness must land on 0.0 — quote it back —
    // rather than being carried into every weight comparison below.
    #[allow(clippy::manual_clamp)]
    let loose = looseness.max(0.0).min(100.0) / 100.0;
    let pool: [(AnswerVariant, f64); 5] = [
        (AnswerVariant::Sequence, 3.0 * (1.2 - 0.6 * loose)),
        (AnswerVariant::TailEcho, 2.0 * (1.2 - 0.6 * loose)),
        (AnswerVariant::Resolve, 1.5),
        (AnswerVariant::Invert, 2.0 * (0.15 + 1.1 * loose)),
        (AnswerVariant::Retrograde, 1.5 * (0.15 + 1.1 * loose)),
    ];
    let draw = |rng: &mut Rng| {
        sample_weighted(rng, &pool, 1, |e| e.1).first().map(|e| e.0).unwrap_or(AnswerVariant::Sequence)
    };

    if turns <= 1 {
        return (0..turns).map(|_| draw(rng)).collect();
    }
    let quoting = [AnswerVariant::Sequence, AnswerVariant::TailEcho];
    let mut out = vec![*pick(rng, &quoting).unwrap()];
    for _ in 1..turns - 1 {
        out.push(draw(rng));
    }
    out.push(AnswerVariant::Resolve);
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

    fn call_phrase() -> Vec<MotifNote> {
        vec![
            MotifNote { step: 0, deg: 0, len: 1.0 },
            MotifNote { step: 3, deg: 2, len: 1.0 },
            MotifNote { step: 6, deg: 3, len: 2.0 },
            MotifNote { step: 10, deg: 4, len: 2.0 },
        ]
    }

    #[test]
    fn a_sequence_answer_keeps_the_calls_shape_and_moves_it() {
        let call = call_phrase();
        for seed in 0..20u32 {
            let out = answer_motif(&call, AnswerVariant::Sequence, 16, &mut Rng::new(seed));
            assert_eq!(out.len(), call.len());
            assert_eq!(out.iter().map(|n| n.step).collect::<Vec<_>>(), vec![0, 3, 6, 10]);
            let moves: std::collections::HashSet<i32> = out.iter().zip(&call).map(|(o, c)| o.deg - c.deg).collect();
            assert_eq!(moves.len(), 1, "a sequence moves every note by the same amount");
            assert_ne!(*moves.iter().next().unwrap(), 0);
        }
    }

    #[test]
    fn a_sequence_answer_falls_more_often_than_it_rises() {
        // Three of the five entries fall, so a falling answer should be the
        // clear majority — the point of the weighting, not an accident of
        // one seed.
        let call = call_phrase();
        let fell = (0..200u32)
            .filter(|&seed| answer_motif(&call, AnswerVariant::Sequence, 16, &mut Rng::new(seed))[0].deg < 0)
            .count();
        assert!(fell > 120, "only {fell}/200 answers fell");
    }

    #[test]
    fn an_inverted_answer_is_the_call_upside_down() {
        let call = call_phrase();
        let out = answer_motif(&call, AnswerVariant::Invert, 16, &mut Rng::new(1));
        assert_eq!(out.iter().map(|n| n.deg).collect::<Vec<_>>(), vec![0, -2, -3, -4]);
    }

    #[test]
    fn a_tail_echo_quotes_the_end_of_the_call_at_the_top_of_the_answer() {
        let call = call_phrase();
        for seed in 0..20u32 {
            let out = answer_motif(&call, AnswerVariant::TailEcho, 16, &mut Rng::new(seed));
            assert!((2..=3).contains(&out.len()), "quoted {} notes", out.len());
            assert_eq!(out[0].step, 0, "the quote starts the answer");
            // The degrees are the call's own last notes, in order, and the
            // gaps between them are the call's gaps — that spacing is most
            // of what makes an ear recognise a quote.
            let tail = &call[call.len() - out.len()..];
            assert_eq!(out.iter().map(|n| n.deg).collect::<Vec<_>>(), tail.iter().map(|n| n.deg).collect::<Vec<_>>());
            let gaps = |v: &[MotifNote]| -> Vec<u32> { v.windows(2).map(|w| w[1].step - w[0].step).collect() };
            assert_eq!(gaps(&out), gaps(tail));
        }
    }

    #[test]
    fn a_resolving_answer_keeps_the_rhythm_and_walks_the_tune_home() {
        let call = call_phrase();
        let out = answer_motif(&call, AnswerVariant::Resolve, 16, &mut Rng::new(1));
        assert_eq!(out.iter().map(|n| n.step).collect::<Vec<_>>(), call.iter().map(|n| n.step).collect::<Vec<_>>());
        assert_eq!(out[0].deg, call.last().unwrap().deg, "it picks up where the call left off");
        assert_eq!(out.last().unwrap().deg, 0, "the consequent closes on the centre");
        // Monotonic: it walks home rather than wandering there.
        for w in out.windows(2) {
            assert!(w[1].deg.abs() <= w[0].deg.abs());
        }
    }

    #[test]
    fn a_resolving_answer_arrives_on_the_last_note_not_early() {
        // Why the walk is a ramp and not a step-at-a-time descent: moving by
        // one per note reaches the centre on note five of six and then sits
        // there, which is a rest with extra notes rather than a resolution.
        let call: Vec<MotifNote> = (0..6u32).map(|i| MotifNote { step: i * 2, deg: 5, len: 1.0 }).collect();
        let out = answer_motif(&call, AnswerVariant::Resolve, 16, &mut Rng::new(1));
        assert_eq!(out.iter().map(|n| n.deg).collect::<Vec<_>>(), vec![5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn a_resolving_answer_to_a_call_that_already_closed_walks_from_its_peak() {
        // Nothing to resolve, so the answer takes the highest thing the call
        // reached and brings *that* home instead of holding one note.
        let call = vec![
            MotifNote { step: 0, deg: 0, len: 1.0 },
            MotifNote { step: 4, deg: -4, len: 1.0 },
            MotifNote { step: 8, deg: -2, len: 1.0 },
            MotifNote { step: 12, deg: 0, len: 1.0 },
        ];
        let out = answer_motif(&call, AnswerVariant::Resolve, 16, &mut Rng::new(1));
        assert_eq!(out.iter().map(|n| n.deg).collect::<Vec<_>>(), vec![-4, -3, -1, 0]);
    }

    #[test]
    fn every_answer_stays_inside_the_turn_and_never_grows_the_phrase() {
        let call = call_phrase();
        for variant in ANSWER_VARIANTS {
            for seed in 0..20u32 {
                let out = answer_motif(&call, variant, 8, &mut Rng::new(seed));
                assert!(out.len() <= call.len(), "{variant:?} grew the phrase");
                for n in &out {
                    assert!(n.step < 8, "{variant:?} put a note at {} in an 8-step turn", n.step);
                    assert!(n.len > 0.0);
                }
            }
        }
    }

    #[test]
    fn answering_nothing_is_nothing() {
        // A call that rested through its turn: `parts::lead` reaches further
        // back for something to answer, but this must not panic on the way.
        for variant in ANSWER_VARIANTS {
            assert!(answer_motif(&[], variant, 8, &mut Rng::new(1)).is_empty());
        }
    }

    #[test]
    fn an_answer_leaves_the_call_untouched() {
        let call = call_phrase();
        let before = call.clone();
        for variant in ANSWER_VARIANTS {
            answer_motif(&call, variant, 16, &mut Rng::new(7));
        }
        assert_eq!(call, before);
    }

    #[test]
    fn an_answer_plan_quotes_first_and_closes_last() {
        let quoting = [AnswerVariant::Sequence, AnswerVariant::TailEcho];
        for seed in 0..30u32 {
            for turns in 2..6u32 {
                let plan = answer_plan(&mut Rng::new(seed), turns, 50.0);
                assert_eq!(plan.len(), turns as usize);
                assert!(quoting.contains(&plan[0]), "seed {seed}: first answer {:?} quotes nothing", plan[0]);
                assert_eq!(*plan.last().unwrap(), AnswerVariant::Resolve);
            }
        }
    }

    #[test]
    fn a_single_answer_is_drawn_from_the_whole_pool() {
        // Two bars trade bar for bar, which gives the response exactly one
        // turn — the commonest case there is. Forcing it to `Resolve` or to
        // a quote would make every two-bar pattern answer the same way.
        let seen: std::collections::HashSet<AnswerVariant> =
            (0..80u32).map(|seed| answer_plan(&mut Rng::new(seed), 1, 50.0)[0]).collect();
        assert!(seen.len() >= 3, "only {} of the five answers ever came up", seen.len());
        assert!(answer_plan(&mut Rng::new(1), 0, 50.0).is_empty());
    }

    #[test]
    fn an_answer_plan_turns_the_idea_over_more_at_high_looseness_than_at_low() {
        let far = [AnswerVariant::Invert, AnswerVariant::Retrograde];
        let count = |looseness: f64| -> usize {
            (0..80u32).filter(|&seed| far.contains(&answer_plan(&mut Rng::new(seed), 1, looseness)[0])).count()
        };
        assert!(count(100.0) > count(5.0) * 2, "{} vs {}", count(100.0), count(5.0));
    }

    #[test]
    fn an_answer_plan_is_deterministic_for_a_seed() {
        assert_eq!(answer_plan(&mut Rng::new(9), 4, 40.0), answer_plan(&mut Rng::new(9), 4, 40.0));
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
