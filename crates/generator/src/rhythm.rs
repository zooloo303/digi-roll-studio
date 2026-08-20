// Which steps get trigs, and how they feel.
//
// Port of `js/gen/rhythm.js`. The step-weight table in a genre profile says
// what is *likely*; the density slider says how much of it fires.
// Everything downstream — pitch, length, p-lock lane values — hangs off the
// trig list this produces, so this module is the shape of the part.
//
// It also owns the two per-trig layers that are about feel rather than
// pitch:
//
//   * **groove micro-timing**, snapped to the 1/24-of-a-step grid the boxes
//     actually store (`digi_protocol::pattern::micro_byte_to_steps`), so
//     what a caller builds is what lands on the hardware — the same bargain
//     `core::lengths::snap_len_fine` strikes for note lengths;
//   * **per-trig PROB/FILL/COND**, from the genre's condition recipe scaled
//     by the Looseness slider. These are per *trig*, so this hands back one
//     setting per step and the parts stamp it on every note sharing that
//     step — the step-uniformity rule the encoder relies on
//     (`core::edit_ops::adopt_step_trig`).

use std::collections::BTreeMap;

use crate::genres::{ConditionRecipe, FillMode, Velocity};
use crate::rng::{chance, int_range, pick, range, sample_weighted, Rng};

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// The boxes store micro-timing in 1/24ths of a step. Snapping here means a
/// groove offset is exactly what the hardware will hold rather than a
/// number that quietly rounds on write.
pub const MICRO_TICK: f64 = 1.0 / 24.0;
pub const MICRO_LIMIT: f64 = 23.0 / 24.0;

pub fn snap_micro(m: f64) -> f64 {
    (clamp(m, -MICRO_LIMIT, MICRO_LIMIT) / MICRO_TICK).round() * MICRO_TICK
}

/// A step is *accented* when it lands on one of the four beats, and a
/// *ghost* when its own weight is in the bottom of the table — the two
/// labels everything else reads: accents get velocity and length, ghosts
/// get PROB locks and a whisper.
pub const GHOST_WEIGHT: f64 = 0.4;

pub fn is_beat(step: u32) -> bool {
    step % 4 == 0
}

/// What [`trig_count_for`] needs. `trigs_per_bar` is the genre's own range,
/// so density 0 is still music (the sparsest version of that part) rather
/// than silence — turning a part *off* is the checkbox's job, not the
/// slider's.
#[derive(Debug, Clone, Copy)]
pub struct TrigCountOpts {
    pub trigs_per_bar: (u32, u32),
    pub density: u32,
    pub bars: u32,
}

/// How many trigs a density asks for.
pub fn trig_count_for(opts: TrigCountOpts) -> u32 {
    let (lo, hi) = opts.trigs_per_bar;
    let per_bar = lo as f64 + (hi as f64 - lo as f64) * clamp(opts.density as f64, 0.0, 100.0) / 100.0;
    ((per_bar * opts.bars as f64).round() as i64).max(1) as u32
}

/// One trig in a generated rhythm: its position and the two labels
/// everything downstream reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trig {
    pub step: u32,
    pub bar: u32,
    pub weight: f64,
    pub accent: bool,
    pub ghost: bool,
}

/// What [`rhythm_for`] needs.
///
///   `weights`   one bar of 16 relative likelihoods (`genres::RoleProfile`)
///   `busy`      steps another part already owns. `avoid` is how much that
///               costs: the lead sets it high so it answers the bass instead
///               of doubling it, the chords leave it near zero because
///               chords and bass landing together is a band, not a
///               collision.
///   `anchors`   steps that always get a trig whatever the density (the
///               bass's 1)
pub struct RhythmOpts<'a> {
    pub weights: &'a [f64],
    pub density: u32,
    pub bars: u32,
    pub busy: &'a std::collections::HashSet<u32>,
    pub avoid: f64,
    pub anchors: &'a [u32],
    pub trigs_per_bar: (u32, u32),
}

struct Candidate {
    step: u32,
    weight: f64,
}

/// The trig list for a part, ascending by step.
pub fn rhythm_for(opts: RhythmOpts, rng: &mut Rng) -> Vec<Trig> {
    let total = opts.bars * 16;
    let want = trig_count_for(TrigCountOpts { trigs_per_bar: opts.trigs_per_bar, density: opts.density, bars: opts.bars });
    let anchored: Vec<u32> = opts.anchors.iter().copied().filter(|&s| s < total).collect();

    let mut candidates = Vec::new();
    for step in 0..total {
        if anchored.contains(&step) {
            continue;
        }
        let base = opts.weights.get((step % 16) as usize).copied().unwrap_or(0.0);
        if base <= 0.0 {
            continue;
        }
        let weight = if opts.busy.contains(&step) { base * (1.0 - clamp(opts.avoid, 0.0, 1.0)) } else { base };
        candidates.push(Candidate { step, weight });
    }

    let want_extra = want.saturating_sub(anchored.len() as u32) as usize;
    let chosen = sample_weighted(rng, &candidates, want_extra, |c| c.weight);

    let mut steps: Vec<(u32, f64)> = anchored
        .iter()
        .map(|&step| (step, opts.weights.get((step % 16) as usize).copied().unwrap_or(1.0).max(1.0)))
        .chain(chosen.iter().map(|c| (c.step, c.weight)))
        .collect();
    steps.sort_by_key(|(step, _)| *step);

    steps
        .into_iter()
        .map(|(step, weight)| Trig {
            step,
            bar: step / 16,
            weight,
            accent: is_beat(step) || weight >= 0.8,
            ghost: !is_beat(step) && weight < GHOST_WEIGHT,
        })
        .collect()
}

/// A trig's velocity: the genre's three levels, plus a humanised wobble so a
/// repeated hit isn't machine-identical. Humanize 0 gives exactly the
/// profile's numbers, which is what makes a generated part reproducible by
/// eye.
pub fn velocity_for(accent: bool, ghost: bool, velocity: Velocity, humanize: u32, rng: &mut Rng) -> u8 {
    let base = if accent { velocity.accent } else if ghost { velocity.ghost } else { velocity.normal };
    let wobble = if humanize > 0 { (range(rng, -1.0, 1.0) * (humanize as f64 / 100.0) * 14.0).round() as i64 } else { 0 };
    clamp(f64::from(base) + wobble as f64, 1.0, 127.0) as u8
}

/// A trig's micro-timing: the genre's groove for that position, plus a
/// humanised wobble, snapped to what the box stores.
pub fn micro_for(step: u32, groove: &[f64], humanize: u32, rng: &mut Rng) -> f64 {
    let g = groove.get((step % 16) as usize).copied().unwrap_or(0.0);
    let wobble = if humanize > 0 { range(rng, -1.0, 1.0) * (humanize as f64 / 100.0) * 0.06 } else { 0.0 };
    snap_micro(g + wobble)
}

/// The gap to the next trig, in steps — what a "play until the next one"
/// length is measured against. The last trig measures to the end of the
/// pattern.
pub fn gap_after(trigs: &[Trig], i: usize, total: u32) -> f64 {
    let next = trigs.get(i + 1).map(|t| t.step).unwrap_or(total);
    (f64::from(next) - f64::from(trigs[i].step)).max(0.125)
}

// --- Per-trig conditions -------------------------------------------------------

/// One trig's PROB/FILL/COND, if it got any. `None` fields are "no lock of
/// that kind" — a step present in [`trig_feel_for`]'s map has at least one
/// `Some`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrigFeel {
    pub prob: Option<i64>,
    pub fill: Option<bool>,
    pub cond: Option<&'static str>,
}

fn cond_for_bar(bar: u32, keys: &'static [&'static str]) -> &'static str {
    keys[(bar as usize) % keys.len()]
}

/// Apply a genre's condition recipe to a trig list. Looseness scales every
/// chance, so 0 writes nothing at all and the parts come out as plain
/// trigs.
///
/// Returns a map of step → lock, only for steps that got something. A step
/// can pick up at most one COND and one FILL — the box holds one of each —
/// and the recipe order decides who wins, which is why a genre lists its
/// alternation rules before its decorations.
pub fn trig_feel_for(
    trigs: &[Trig],
    recipe: &[ConditionRecipe],
    looseness: u32,
    bars: u32,
    rng: &mut Rng,
) -> BTreeMap<u32, TrigFeel> {
    let mut out: BTreeMap<u32, TrigFeel> = BTreeMap::new();
    let loose = clamp(looseness as f64, 0.0, 100.0) / 100.0;
    if loose <= 0.0 || recipe.is_empty() {
        return out;
    }

    for trig in trigs {
        for rule in recipe {
            let (rule_chance, apply): (f64, Box<dyn Fn(&mut TrigFeel, &mut Rng)>) = match *rule {
                ConditionRecipe::AltBar { chance: c, keys } => (
                    c,
                    Box::new(move |feel: &mut TrigFeel, _rng: &mut Rng| {
                        // Alternate bars, so a two-bar loop isn't two identical
                        // bars. Pointless in a one-bar pattern, where "1:2"
                        // would just silence half the loops.
                        if bars >= 2 && feel.cond.is_none() {
                            feel.cond = Some(cond_for_bar(trig.bar, keys));
                        }
                    }),
                ),
                ConditionRecipe::EveryFourth { chance: c, keys } => (
                    c,
                    Box::new(move |feel: &mut TrigFeel, rng: &mut Rng| {
                        // Details that arrive every fourth time round.
                        if feel.cond.is_none() && !trig.accent {
                            feel.cond = pick(rng, keys).copied();
                        }
                    }),
                ),
                ConditionRecipe::Logic { chance: c, keys } => (
                    c,
                    Box::new(move |feel: &mut TrigFeel, rng: &mut Rng| {
                        // A run that answers the trig before it. Never on a
                        // downbeat: the part has to be recognisable on the
                        // first pass.
                        if feel.cond.is_none() && !trig.accent && trig.step > 0 {
                            feel.cond = pick(rng, keys).copied();
                        }
                    }),
                ),
                ConditionRecipe::ProbGhost { chance: c, range: (lo, hi) } => (
                    c,
                    Box::new(move |feel: &mut TrigFeel, rng: &mut Rng| {
                        if trig.ghost && feel.prob.is_none() {
                            feel.prob = Some(int_range(rng, lo, hi));
                        }
                    }),
                ),
                ConditionRecipe::ProbWeak { chance: c, range: (lo, hi) } => (
                    c,
                    Box::new(move |feel: &mut TrigFeel, rng: &mut Rng| {
                        if !trig.accent && feel.prob.is_none() {
                            feel.prob = Some(int_range(rng, lo, hi));
                        }
                    }),
                ),
                ConditionRecipe::Fill { chance: c, mode } => (
                    c,
                    Box::new(move |feel: &mut TrigFeel, rng: &mut Rng| {
                        // ON = only exists while you hold FILL; OFF = steps
                        // aside during one. Never on an accent, so holding
                        // FILL can't gut the groove.
                        if !trig.accent && feel.fill.is_none() {
                            feel.fill = Some(match mode {
                                FillMode::Off => false,
                                FillMode::Either => chance(rng, 0.5),
                                FillMode::On => true,
                            });
                        }
                    }),
                ),
            };
            let p = rule_chance * loose;
            if p <= 0.0 || !chance(rng, p) {
                continue;
            }
            let feel = out.entry(trig.step).or_default();
            apply(feel, rng);
        }
    }

    // Steps that ended up with nothing are not conditions — drop them so
    // callers can treat "in the map" as "has a lock".
    out.retain(|_, s| s.prob.is_some() || s.fill.is_some() || s.cond.is_some());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genres::{genre_profile, role_profile, GenreId, Role};
    use digi_protocol::conditions::is_cond_key;

    fn even() -> [f64; 16] {
        [1.0; 16]
    }
    fn beats_only() -> [f64; 16] {
        std::array::from_fn(|i| if i % 4 == 0 { 1.0 } else { 0.0 })
    }
    fn no_busy() -> std::collections::HashSet<u32> {
        std::collections::HashSet::new()
    }

    #[test]
    fn runs_from_the_genres_own_floor_to_its_ceiling() {
        assert_eq!(trig_count_for(TrigCountOpts { trigs_per_bar: (2, 8), density: 0, bars: 1 }), 2);
        assert_eq!(trig_count_for(TrigCountOpts { trigs_per_bar: (2, 8), density: 100, bars: 1 }), 8);
        assert_eq!(trig_count_for(TrigCountOpts { trigs_per_bar: (2, 8), density: 50, bars: 2 }), 10);
    }

    #[test]
    fn never_asks_for_silence() {
        assert_eq!(trig_count_for(TrigCountOpts { trigs_per_bar: (0, 4), density: 0, bars: 1 }), 1);
    }

    #[test]
    fn rises_with_density_and_with_bars() {
        let mut last = 0;
        for density in [0, 25, 50, 75, 100] {
            let n = trig_count_for(TrigCountOpts { trigs_per_bar: (2, 10), density, bars: 2 });
            assert!(n >= last);
            last = n;
        }
    }

    #[test]
    fn the_trig_list_is_deterministic_for_a_seed() {
        let weights = even();
        let busy = no_busy();
        let opts = || RhythmOpts {
            weights: &weights,
            density: 60,
            bars: 2,
            busy: &busy,
            avoid: 0.0,
            anchors: &[],
            trigs_per_bar: (4, 8),
        };
        let a: Vec<u32> = rhythm_for(opts(), &mut Rng::new(5)).iter().map(|t| t.step).collect();
        let b: Vec<u32> = rhythm_for(opts(), &mut Rng::new(5)).iter().map(|t| t.step).collect();
        assert_eq!(a, b);
        let c: Vec<u32> = rhythm_for(opts(), &mut Rng::new(6)).iter().map(|t| t.step).collect();
        assert_ne!(a, c);
    }

    #[test]
    fn the_trig_list_is_ascending_distinct_and_inside_the_pattern() {
        let weights = even();
        let busy = no_busy();
        for seed in 0..20u32 {
            let trigs = rhythm_for(
                RhythmOpts { weights: &weights, density: 80, bars: 2, busy: &busy, avoid: 0.0, anchors: &[], trigs_per_bar: (4, 12) },
                &mut Rng::new(seed),
            );
            let steps: Vec<u32> = trigs.iter().map(|t| t.step).collect();
            let mut sorted = steps.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, steps);
            let set: std::collections::HashSet<u32> = steps.iter().copied().collect();
            assert_eq!(set.len(), steps.len());
            for s in steps {
                assert!(s < 32);
            }
        }
    }

    #[test]
    fn never_puts_a_trig_on_a_zero_weight_step() {
        let weights = beats_only();
        let busy = no_busy();
        for seed in 0..20u32 {
            let trigs = rhythm_for(
                RhythmOpts { weights: &weights, density: 100, bars: 2, busy: &busy, avoid: 0.0, anchors: &[], trigs_per_bar: (4, 16) },
                &mut Rng::new(seed),
            );
            for t in trigs {
                assert_eq!(t.step % 4, 0);
            }
        }
    }

    #[test]
    fn always_keeps_its_anchors_at_any_density() {
        let weights = even();
        let busy = no_busy();
        for density in [0, 50, 100] {
            let trigs = rhythm_for(
                RhythmOpts { weights: &weights, density, bars: 2, busy: &busy, avoid: 0.0, anchors: &[0], trigs_per_bar: (1, 8) },
                &mut Rng::new(3),
            );
            assert!(trigs.iter().any(|t| t.step == 0));
            assert!(trigs.iter().find(|t| t.step == 0).unwrap().accent);
        }
    }

    #[test]
    fn marks_beats_as_accents_and_low_weight_off_beats_as_ghosts() {
        let weights: [f64; 16] = std::array::from_fn(|i| if i % 4 == 0 { 1.0 } else { 0.2 });
        let busy = no_busy();
        let trigs = rhythm_for(
            RhythmOpts { weights: &weights, density: 100, bars: 1, busy: &busy, avoid: 0.0, anchors: &[], trigs_per_bar: (8, 16) },
            &mut Rng::new(4),
        );
        for t in trigs {
            if t.step % 4 == 0 {
                assert!(t.accent);
                assert!(!t.ghost);
            } else {
                assert!(t.ghost);
            }
        }
    }

    #[test]
    fn avoids_steps_another_part_owns_in_proportion_to_avoid() {
        let weights = even();
        let busy: std::collections::HashSet<u32> = [0, 2, 4, 6, 8, 10, 12, 14].into_iter().collect();
        let mut collisions = 0;
        let mut free = 0;
        for seed in 0..60u32 {
            let trigs = rhythm_for(
                RhythmOpts { weights: &weights, density: 50, bars: 1, busy: &busy, avoid: 0.85, anchors: &[], trigs_per_bar: (4, 4) },
                &mut Rng::new(seed),
            );
            for t in trigs {
                if busy.contains(&t.step) {
                    collisions += 1;
                } else {
                    free += 1;
                }
            }
        }
        assert!(free > collisions * 3);
    }

    #[test]
    fn tags_each_trig_with_its_bar() {
        let weights = even();
        let busy = no_busy();
        let trigs = rhythm_for(
            RhythmOpts { weights: &weights, density: 100, bars: 2, busy: &busy, avoid: 0.0, anchors: &[], trigs_per_bar: (8, 8) },
            &mut Rng::new(8),
        );
        for t in trigs {
            assert_eq!(t.bar, t.step / 16);
        }
    }

    const VELOCITY: Velocity = Velocity { accent: 120, normal: 100, ghost: 60 };

    #[test]
    fn uses_the_profiles_exact_levels_when_humanize_is_0() {
        let mut r = Rng::new(1);
        assert_eq!(velocity_for(true, false, VELOCITY, 0, &mut r), 120);
        assert_eq!(velocity_for(false, false, VELOCITY, 0, &mut r), 100);
        assert_eq!(velocity_for(false, true, VELOCITY, 0, &mut r), 60);
    }

    #[test]
    fn wobbles_with_humanize_but_stays_a_legal_velocity() {
        let mut r = Rng::new(2);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..300 {
            let v = velocity_for(false, false, VELOCITY, 100, &mut r);
            assert!((1..=127).contains(&v));
            seen.insert(v);
        }
        assert!(seen.len() > 5);
    }

    #[test]
    fn clamps_a_hot_accent_rather_than_sending_130() {
        let mut r = Rng::new(3);
        let hot = Velocity { accent: 127, normal: 120, ghost: 90 };
        for _ in 0..200 {
            assert!(velocity_for(true, false, hot, 100, &mut r) <= 127);
        }
    }

    #[test]
    fn snaps_to_the_1_24_step_grid() {
        for m in [0.1, -0.1, 0.333, 0.49, -0.49, 0.0] {
            let snapped = snap_micro(m);
            assert!(((snapped / MICRO_TICK) - (snapped / MICRO_TICK).round()).abs() < 1e-9);
        }
    }

    #[test]
    fn clamps_to_what_a_micro_byte_can_hold() {
        assert!((snap_micro(5.0) - 23.0 / 24.0).abs() < 1e-10);
        assert!((snap_micro(-5.0) - (-23.0 / 24.0)).abs() < 1e-10);
    }

    #[test]
    fn is_exactly_the_genres_groove_when_humanize_is_0() {
        let groove = genre_profile(GenreId::House).groove;
        let mut r = Rng::new(7);
        assert_eq!(micro_for(0, &groove, 0, &mut r), 0.0);
        let m1 = micro_for(1, &groove, 0, &mut r);
        assert!((m1 - snap_micro(groove[1])).abs() < 1e-10);
        let m17 = micro_for(17, &groove, 0, &mut r);
        let m1_again = micro_for(1, &groove, 0, &mut r);
        assert!((m17 - m1_again).abs() < 1e-10);
    }

    #[test]
    fn stays_inside_the_rolls_own_range_with_humanize_at_full() {
        let groove = genre_profile(GenreId::Breaks).groove;
        let mut r = Rng::new(9);
        for step in 0..32u32 {
            let m = micro_for(step, &groove, 100, &mut r);
            assert!(m > -1.0 && m < 1.0);
        }
    }

    #[test]
    fn the_gap_measures_to_the_next_trig_and_to_the_end_for_the_last() {
        let trigs = vec![
            Trig { step: 0, bar: 0, weight: 1.0, accent: true, ghost: false },
            Trig { step: 4, bar: 0, weight: 1.0, accent: true, ghost: false },
            Trig { step: 6, bar: 0, weight: 1.0, accent: false, ghost: false },
        ];
        assert_eq!(gap_after(&trigs, 0, 16), 4.0);
        assert_eq!(gap_after(&trigs, 1, 16), 2.0);
        assert_eq!(gap_after(&trigs, 2, 16), 10.0);
    }

    fn feel_trigs() -> Vec<Trig> {
        vec![
            Trig { step: 0, bar: 0, weight: 1.0, accent: true, ghost: false },
            Trig { step: 3, bar: 0, weight: 0.2, accent: false, ghost: true },
            Trig { step: 6, bar: 0, weight: 0.5, accent: false, ghost: false },
            Trig { step: 18, bar: 1, weight: 0.2, accent: false, ghost: true },
            Trig { step: 20, bar: 1, weight: 1.0, accent: true, ghost: false },
        ]
    }

    fn full_recipe() -> Vec<ConditionRecipe> {
        vec![
            ConditionRecipe::AltBar { chance: 1.0, keys: &["1:2", "2:2"] },
            ConditionRecipe::ProbGhost { chance: 1.0, range: (60, 85) },
            ConditionRecipe::Fill { chance: 1.0, mode: FillMode::On },
        ]
    }

    #[test]
    fn writes_nothing_at_all_at_looseness_0() {
        let feel = trig_feel_for(&feel_trigs(), &full_recipe(), 0, 2, &mut Rng::new(1));
        assert!(feel.is_empty());
    }

    #[test]
    fn only_reports_steps_that_actually_got_a_lock() {
        let recipe = vec![ConditionRecipe::ProbGhost { chance: 1.0, range: (70, 70) }];
        let feel = trig_feel_for(&feel_trigs(), &recipe, 100, 2, &mut Rng::new(2));
        let keys: Vec<u32> = feel.keys().copied().collect();
        assert_eq!(keys, vec![3, 18]);
        let s = &feel[&3];
        assert_eq!((s.prob, s.fill, s.cond), (Some(70), None, None));
    }

    #[test]
    fn alternates_bars_so_a_two_bar_loop_is_not_two_identical_bars() {
        let recipe = vec![ConditionRecipe::AltBar { chance: 1.0, keys: &["1:2", "2:2"] }];
        let feel = trig_feel_for(&feel_trigs(), &recipe, 100, 2, &mut Rng::new(3));
        assert_eq!(feel[&0].cond, Some("1:2"));
        assert_eq!(feel[&18].cond, Some("2:2"));
    }

    #[test]
    fn leaves_alternation_alone_in_a_one_bar_pattern() {
        let trigs = &feel_trigs()[..3];
        let recipe = vec![ConditionRecipe::AltBar { chance: 1.0, keys: &["1:2", "2:2"] }];
        let feel = trig_feel_for(trigs, &recipe, 100, 1, &mut Rng::new(4));
        assert!(feel.is_empty());
    }

    #[test]
    fn keeps_prob_inside_the_recipes_range() {
        let recipe = vec![ConditionRecipe::ProbWeak { chance: 1.0, range: (60, 85) }];
        let feel = trig_feel_for(&feel_trigs(), &recipe, 100, 2, &mut Rng::new(5));
        for s in feel.values() {
            let p = s.prob.unwrap();
            assert!((60..=85).contains(&p));
        }
    }

    #[test]
    fn never_touches_an_accent_with_fill_or_a_ratio() {
        let recipe = vec![
            ConditionRecipe::Fill { chance: 1.0, mode: FillMode::On },
            ConditionRecipe::EveryFourth { chance: 1.0, keys: &["3:4"] },
            ConditionRecipe::Logic { chance: 1.0, keys: &["PRE"] },
        ];
        let feel = trig_feel_for(&feel_trigs(), &recipe, 100, 2, &mut Rng::new(6));
        assert!(!feel.contains_key(&0));
        assert!(!feel.contains_key(&20));
    }

    #[test]
    fn gives_a_step_at_most_one_cond_and_one_fill_and_only_known_conditions() {
        for seed in 0..30u32 {
            let feel = trig_feel_for(&feel_trigs(), &full_recipe(), 100, 2, &mut Rng::new(seed));
            for s in feel.values() {
                if let Some(cond) = s.cond {
                    assert!(is_cond_key(cond));
                }
            }
        }
    }

    #[test]
    fn writes_every_condition_every_genre_asks_for_as_a_key_the_hardware_table_has() {
        let many: Vec<Trig> = (0..32u32)
            .map(|i| Trig { step: i, bar: i / 16, weight: 1.0, accent: false, ghost: i % 3 == 0 })
            .collect();
        for genre in GenreId::ALL {
            for role in Role::ALL {
                let recipe = role_profile(genre, role).conditions;
                let feel = trig_feel_for(&many, recipe, 100, 2, &mut Rng::new(11));
                for s in feel.values() {
                    if let Some(cond) = s.cond {
                        assert!(is_cond_key(cond));
                    }
                }
            }
        }
    }

    #[test]
    fn is_beat_is_the_four_quarters_of_a_bar() {
        for s in [0, 4, 8, 12, 16, 20] {
            assert!(is_beat(s));
        }
        for s in [1, 2, 3, 5, 6, 7, 15] {
            assert!(!is_beat(s));
        }
    }
}
