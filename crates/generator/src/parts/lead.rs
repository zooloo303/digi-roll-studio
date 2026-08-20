// The lead.
//
// Port of `js/gen/parts/lead.js`. Generated last in a row's own place in the
// arrangement order, so it can hear the rows before it. Two things make it a
// part rather than a sprinkle of notes:
//
//   * **it is motif-based.** One short idea (`crate::motif`) is stated, then
//     developed phrase by phrase — transposed, inverted, retrograded,
//     displaced. The density slider decides how much of each development
//     survives.
//   * **it answers the bass.** A note landing on a step another part already
//     owns is nudged a step later where there is room, and dropped where
//     there isn't. That one rule is why a generated lead sits in the gaps
//     instead of doubling the bassline.
//
// Pitch resolution: the motif's scale-degree offsets are walked along the
// scale (so the contour is the same shape in any key), and then a note on a
// beat is pulled onto the nearest chord tone of that bar. Strong beats agree
// with the harmony; weak beats are free to pass through.

use std::collections::HashSet;

use crate::context::ResolvedContext;
use crate::genres::RoleProfile;
use crate::motif::{develop_motif, make_motif, thin_motif, MakeMotifOpts, MotifNote};
use crate::rhythm::{is_beat, micro_for, snap_micro, trig_feel_for, velocity_for, Trig};
use crate::rng::{chance, Rng};
use crate::theory::{chord_tones, fold_into_window, scale_pitches_in_window, slot_root_pitch, window_for, ChordWindow, Key};

use super::{len_bounds, GeneratedPart, NoteSpec};

/// Walk a pitch list by whole degrees, carrying into the next octave at the
/// ends — the same wrap `core::chords::chord_pitches` uses for thirds past
/// the octave, so a degree offset always means something. `None` for an
/// empty palette, which a caller skips the note for.
fn walk(list: &[i32], base_index: i32, degrees: i32) -> Option<i32> {
    if list.is_empty() {
        return None;
    }
    let len = list.len() as i32;
    let i = base_index + degrees;
    let octave = i.div_euclid(len);
    Some(list[i.rem_euclid(len) as usize] + 12 * octave)
}

fn nearest_index(list: &[i32], pitch: i32) -> usize {
    let mut best = 0;
    for (i, &p) in list.iter().enumerate().skip(1) {
        if (p - pitch).abs() < (list[best] - pitch).abs() {
            best = i;
        }
    }
    best
}

/// Pull a pitch onto the nearest chord tone, but only if one is close:
/// dragging a passing note a tritone to "fix" it would destroy the motif's
/// shape.
fn to_chord_tone(pitch: i32, tones: &[u8], reach: i32) -> i32 {
    if tones.is_empty() {
        return pitch;
    }
    let near = tones
        .iter()
        .map(|&t| i32::from(t))
        .reduce(|a, b| if (b - pitch).abs() < (a - pitch).abs() { b } else { a })
        .unwrap();
    if (near - pitch).abs() <= reach {
        near
    } else {
        pitch
    }
}

/// A generated lead, plus the motif it stated — kept alongside the notes
/// because it is what the rest of the phrase development read, and a future
/// caller (song mode, or a "show the idea" UI) may want it too.
#[derive(Debug, Clone, Default)]
pub struct LeadPart {
    pub notes: Vec<NoteSpec>,
    pub trigs: Vec<Trig>,
    pub motif: Vec<MotifNote>,
}

impl From<LeadPart> for GeneratedPart {
    fn from(l: LeadPart) -> Self {
        GeneratedPart { notes: l.notes, trigs: l.trigs }
    }
}

struct Placed {
    step: u32,
    pitch: i32,
    want: f64,
    bar: u32,
}

pub fn generate_lead(ctx: &ResolvedContext, profile: &RoleProfile, octave: u8, density: u8, rng: &mut Rng, busy: &HashSet<u32>) -> LeadPart {
    let (min, max) = window_for(profile.span, i32::from(octave));
    let total = ctx.length_steps;
    let key = Key { root: ctx.key_root, intervals: ctx.key_intervals };

    let (motif_notes, motif_window) = match profile.motif {
        Some(m) => (m.notes, m.window),
        None => ((3, 5), 8),
    };
    let window = motif_window.max(2).min(16) as u32;
    let phrases = (total / window).max(1);

    let motif = make_motif(
        rng,
        MakeMotifOpts { notes: (i64::from(motif_notes.0), i64::from(motif_notes.1)), window, weights: &profile.weights, spread: 2 },
    );
    let plan = crate::motif::motif_plan(rng, phrases, f64::from(ctx.feel.looseness));

    let palette = scale_pitches_in_window(key.root, key.intervals, min, max);
    let mut taken: HashSet<u32> = HashSet::new();
    let mut placed: Vec<Placed> = Vec::new();

    for p in 0..phrases {
        let start = p * window;
        let bar = start / 16;
        let slot = ctx.bar_slots.get(bar as usize).copied().unwrap_or(ctx.bar_slots[0]);

        // Space is a musical answer: at low density whole phrases are left
        // out, but never the first, which is where the idea gets stated.
        if p > 0 && chance(rng, (1.0 - f64::from(density) / 100.0) * 0.35) {
            continue;
        }

        let developed = develop_motif(&motif, plan[p as usize], window, rng);
        let phrase = thin_motif(&developed, f64::from(density), rng);
        let window_bounds = ChordWindow { octave: i32::from(octave), min: min.clamp(0, 127) as u8, max: max.clamp(0, 127) as u8, ..Default::default() };
        let tones = chord_tones(&slot, key, window_bounds);
        let root = slot_root_pitch(&slot, key, i32::from(octave), min, max);
        let root_index = nearest_index(&palette, root) as i32;

        for n in &phrase {
            let mut step = start + n.step;
            if step >= total {
                continue;
            }

            // Answer, don't double: a step another part owns is nudged one
            // later where there's room, and given up where there isn't.
            if busy.contains(&step) {
                let nudged = step + 1;
                if nudged < total && !busy.contains(&nudged) && !taken.contains(&nudged) {
                    step = nudged;
                } else if chance(rng, 0.7) {
                    continue;
                }
            }
            if taken.contains(&step) {
                continue;
            }
            taken.insert(step);

            let Some(walked) = walk(&palette, root_index, n.deg) else { continue };
            let pitch = fold_into_window(if is_beat(step) { to_chord_tone(walked, &tones, 2) } else { walked }, min, max);
            placed.push(Placed { step, pitch, want: n.len, bar: step / 16 });
        }
    }

    placed.sort_by_key(|p| p.step);

    // The trig list the conditions and the dynamics read. `accent` is a
    // beat, and a lead's ghosts are the notes squeezed between two others —
    // the ones a player would throw away.
    let trigs: Vec<Trig> = placed
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let next_step = placed.get(i + 1).map(|p| p.step).unwrap_or(total);
            let gap = i64::from(next_step) - i64::from(n.step);
            Trig {
                step: n.step,
                bar: n.bar,
                weight: profile.weights.get((n.step % 16) as usize).copied().unwrap_or(0.5),
                accent: is_beat(n.step),
                ghost: !is_beat(n.step) && gap <= 1,
            }
        })
        .collect();

    let feel = trig_feel_for(&trigs, profile.conditions, ctx.feel.looseness as u32, ctx.bars, rng);
    let (_, _, len_max) = len_bounds(&profile.len);

    let notes: Vec<NoteSpec> = placed
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let trig = trigs[i];
            let next_step = placed.get(i + 1).map(|p| p.step).unwrap_or(total);
            let gap = (f64::from(next_step) - f64::from(n.step)).max(0.125);
            let len = digi_core::snap_len_fine(n.want.min(gap).min(len_max), f64::from(total - n.step));
            let t = feel.get(&n.step);
            NoteSpec {
                step: n.step,
                pitch: n.pitch.clamp(0, 127) as u8,
                len,
                velocity: velocity_for(trig.accent, trig.ghost, profile.velocity, u32::from(ctx.feel.humanize), rng),
                micro: snap_micro(micro_for(n.step, &ctx.profile.groove, u32::from(ctx.feel.humanize), rng)),
                prob: t.and_then(|t| t.prob),
                fill: t.and_then(|t| t.fill),
                cond: t.and_then(|t| t.cond),
            }
        })
        .collect();

    LeadPart { notes, trigs, motif }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{resolve_context, Feel, GenContext};
    use crate::genres::{role_profile, GenreId, Role};
    use crate::rng::rng_for;

    fn generate(over: GenContext, genre: GenreId, density: u8, octave: u8, busy: &HashSet<u32>) -> LeadPart {
        let seed = over.seed;
        let ctx = resolve_context(&over).unwrap();
        let profile = role_profile(genre, Role::Lead);
        let mut rng = rng_for(seed, "lead");
        generate_lead(&ctx, &profile, octave, density, &mut rng, busy)
    }

    #[test]
    fn answers_the_bass_rather_than_doubling_it() {
        use crate::parts::bass::generate_bass;
        let mut on_bass = 0;
        let mut free = 0;
        for seed in 0..25u32 {
            let over = GenContext { genre: GenreId::Dnb, seed, bars: 2, ..GenContext::default() };
            let ctx = resolve_context(&over).unwrap();
            let mut bass_rng = rng_for(seed, "bass");
            let bass = generate_bass(&ctx, &role_profile(GenreId::Dnb, Role::Bass), 2, 55, &mut bass_rng, &HashSet::new());
            let busy: HashSet<u32> = bass.trigs.iter().map(|t| t.step).collect();
            let mut lead_rng = rng_for(seed, "lead");
            let lead = generate_lead(&ctx, &role_profile(GenreId::Dnb, Role::Lead), 5, 40, &mut lead_rng, &busy);
            for n in &lead.notes {
                if busy.contains(&n.step) {
                    on_bass += 1;
                } else {
                    free += 1;
                }
            }
        }
        assert!(free > on_bass * 2);
    }

    #[test]
    fn lands_on_chord_tones_on_the_beats() {
        let over = GenContext { genre: GenreId::Electro, seed: 6, progression: "i".into(), bars: 1, ..GenContext::default() };
        let lead = generate(over, GenreId::Electro, 40, 5, &HashSet::new());
        let on_beat: Vec<&NoteSpec> = lead.notes.iter().filter(|n| n.step % 4 == 0).collect();
        assert!(!on_beat.is_empty());
        for n in on_beat {
            assert!([0, 3, 7].contains(&(i32::from(n.pitch).rem_euclid(12))));
        }
    }

    #[test]
    fn stays_in_the_scale() {
        use digi_core::chords::Scale;
        let over = GenContext { genre: GenreId::Dnb, seed: 7, scale: Scale::Minor, root: 2, bars: 2, ..GenContext::default() };
        let classes: HashSet<i32> = Scale::Minor.intervals().iter().map(|i| (i + 2).rem_euclid(12)).collect();
        let lead = generate(over, GenreId::Dnb, 40, 5, &HashSet::new());
        for n in &lead.notes {
            assert!(classes.contains(&i32::from(n.pitch).rem_euclid(12)));
        }
    }

    #[test]
    fn is_one_note_per_step() {
        for seed in 0..10u32 {
            let over = GenContext { genre: GenreId::Breaks, seed, ..GenContext::default() };
            let lead = generate(over, GenreId::Breaks, 40, 5, &HashSet::new());
            let steps: HashSet<u32> = lead.notes.iter().map(|n| n.step).collect();
            assert_eq!(steps.len(), lead.notes.len());
        }
    }

    #[test]
    fn develops_the_motif_rather_than_repeating_it_four_times() {
        let over = GenContext {
            genre: GenreId::Dnb,
            seed: 2,
            bars: 4,
            feel: Feel { motion: 0, looseness: 80, humanize: 0 },
            ..GenContext::default()
        };
        let lead = generate(over, GenreId::Dnb, 40, 5, &HashSet::new());
        let bars: HashSet<String> = (0..4)
            .map(|b| {
                let mut notes: Vec<String> =
                    lead.notes.iter().filter(|n| n.step / 16 == b).map(|n| format!("{}:{}", n.step % 16, n.pitch)).collect();
                notes.sort();
                notes.join(",")
            })
            .collect();
        assert!(bars.len() > 1);
    }

    #[test]
    fn plays_less_at_low_density_than_at_high() {
        let at = |density: u8| -> usize {
            let mut total = 0;
            for seed in 0..10u32 {
                let over = GenContext { genre: GenreId::Breaks, seed, bars: 2, ..GenContext::default() };
                total += generate(over, GenreId::Breaks, density, 5, &HashSet::new()).notes.len();
            }
            total
        };
        assert!(at(100) > at(10));
    }

    #[test]
    fn produces_notes_the_hardware_can_hold() {
        for genre in GenreId::ALL {
            for density in [0u8, 40, 100] {
                for seed in 0..6u32 {
                    let over = GenContext { genre, seed, bars: 2, ..GenContext::default() };
                    let (min, max) = window_for(role_profile(genre, Role::Lead).span, 5);
                    let lead = generate(over, genre, density, 5, &HashSet::new());
                    assert!(!lead.notes.is_empty());
                    for n in &lead.notes {
                        assert!(i32::from(n.pitch) >= min && i32::from(n.pitch) <= max);
                        assert!((1..=127).contains(&n.velocity));
                    }
                }
            }
        }
    }
}
