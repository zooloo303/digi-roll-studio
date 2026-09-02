// The chord lead.
//
// Not a port of anything in the JS. It is a transcription of the Analog
// Four's factory pattern A01, SYN1 — the pattern Elektron chose to be the
// first thing the box plays — decoded off the `analogfour-A01-*.syx`
// fixtures on 2026-09-02 and settled with Neil the same day. What that track
// does, trig by trig:
//
//   * **straight eighths, machine-flat.** A trig on every odd step, velocity
//     127 throughout, no conditions, no micro timing, and a length just under
//     the two-step gap — 1.75 to 1.8 steps — so the line is legato with a
//     breath between hits.
//   * **two pedal tones held at fixed pitches while the root leaps octaves
//     under them.** Bar 1 is E6 and C7 over A4, A5, A6, A4, A5, A6; bar 2 is
//     E6 and B6 over G6, G4, G5, … The pedals are chord tones and extensions
//     (the fifth and third over A minor, the third and sixth over G, the ninth
//     and seventh over D, the ninth and fifth over F), and common tones carry
//     across the bar line — E6 rings from Am into G6 into D9.
//   * **the voicing thins when the root reaches the top.** Every third trig
//     the root is in the pedals' own register, and there it keeps one pedal
//     rather than two: three notes, three, then two.
//   * **the whole thing is one trig per step**, on a box whose tracks hold
//     one note each. The upper voices are the ARP menu's NO2/NO3/NO4, offsets
//     from the trig's note, and on a polyphonic kit with the arp off the box
//     sounds them together. That is why this role exists: it is a chord part
//     shaped so that a root-plus-offsets box plays it as written.
//
// In this app's model a chord is several notes on one step, which is what
// this emits — the same shape chord draw makes and `core::a4_transfer` turns
// into a root and up to three offsets. Nothing here knows about the A4; a
// DN2 plays the same notes natively. Generic in the way the rest of the
// generator is: the progression comes from the panel, the pedals come from
// the key, and the seed decides which pair and where the voicing thins.

use std::collections::HashSet;

use crate::context::ResolvedContext;
use crate::genres::RoleProfile;
use crate::rhythm::{gap_after, micro_for, rhythm_for, snap_micro, velocity_for, RhythmOpts};
use crate::rng::{chance, pick, Rng};
use crate::theory::{chord_tones, degree_pitch, ChordWindow, Key};

use super::{len_bounds, GeneratedPart, NoteSpec};

/// How many octaves the root cycles through: A01 walks 4, 5, 6 and back.
const ROOT_OCTAVES: i32 = 3;
/// The pedal register, relative to the root's top octave: A01's pedals sit
/// from six below to eleven above the highest root of each bar, in every bar.
const PEDAL_BELOW_TOP: i32 = 6;
const PEDAL_ABOVE_TOP: i32 = 11;
/// How often the trig whose root is in the pedal register keeps one pedal
/// rather than two. A01 thins on most of them and not all.
const THIN_AT_TOP: f64 = 0.65;
/// Breathing room before the next trig, in steps: 1.75–1.8 under a gap of 2.
const BREATH: f64 = 0.25;

/// The pitch classes a bar's pedals may be drawn from, and which of them are
/// colour rather than chord: the chord's own tones, then the ninth, the sixth
/// and the seventh — the palette A01 uses (the ninth and seventh over its D,
/// the sixth over its G). Diatonic extensions, so a `Fixed` quality slot still
/// gets in-key colour.
struct PedalPool {
    classes: Vec<i32>,
    root_class: i32,
    extensions: Vec<i32>,
}

impl PedalPool {
    fn contains(&self, pitch: i32) -> bool {
        self.classes.contains(&pitch.rem_euclid(12))
    }
}

fn pedal_pool(ctx: &ResolvedContext, key: Key, slot_index: usize) -> PedalPool {
    let slot = ctx.bar_slots[slot_index];
    let mut classes: Vec<i32> = chord_tones(&slot, key, ChordWindow { octave: 5, ..ChordWindow::default() })
        .iter()
        .map(|&p| i32::from(p).rem_euclid(12))
        .collect();
    let root_class = degree_pitch(slot.degree, key.root, key.intervals, 5).rem_euclid(12);
    let mut extensions = Vec::new();
    for extension in [1u32, 5, 6] {
        let class = degree_pitch(slot.degree + extension, key.root, key.intervals, 5).rem_euclid(12);
        if !classes.contains(&class) {
            classes.push(class);
            extensions.push(class);
        }
    }
    PedalPool { classes, root_class, extensions }
}

/// Every pitch in the pedal window whose class is in the pool, ascending.
fn pedal_candidates(pool: &PedalPool, lo: i32, hi: i32) -> Vec<i32> {
    (lo.max(0)..=hi.min(127)).filter(|&p| pool.contains(p)).collect()
}

/// Two pedals for a bar. With a previous pair, the pair that moves least —
/// which is what keeps E6 ringing from Am into G6. On the first bar there is
/// nothing to lead from, so the pair sits close around the top root, is chord
/// rather than colour, and is not the root's own class: A01 opens on the
/// fifth and the third, five below and three above its A6. Either way a pair
/// closer than a third is a cluster, not a chord. Near ties are broken by the
/// seed, so two runs of one progression do not voice alike.
fn choose_pedals(candidates: &[i32], pool: &PedalPool, root_top: i32, previous: &[i32], rng: &mut Rng) -> Vec<i32> {
    if candidates.len() < 2 {
        return candidates.to_vec();
    }
    let mut pairs: Vec<(i32, [i32; 2])> = Vec::new();
    for (i, &a) in candidates.iter().enumerate() {
        for &b in &candidates[i + 1..] {
            // Two of one class an octave apart is a doubled voice, not a chord.
            if (a - b).rem_euclid(12) == 0 {
                continue;
            }
            let cluster = if b - a < 3 { 6 } else { 0 };
            let cost = if previous.is_empty() {
                let colour = |p: i32| {
                    let class = p.rem_euclid(12);
                    if class == pool.root_class {
                        6
                    } else if pool.extensions.contains(&class) {
                        2
                    } else {
                        0
                    }
                };
                (a - root_top).abs() + (b - root_top).abs() + colour(a) + colour(b) + cluster
            } else {
                let nearest = |p: i32| previous.iter().map(|q| (p - q).abs()).min().unwrap_or(0);
                nearest(a) + nearest(b) + cluster
            };
            pairs.push((cost, [a, b]));
        }
    }
    if pairs.is_empty() {
        return vec![candidates[0]];
    }
    pairs.sort_by_key(|(cost, pair)| (*cost, pair[0], pair[1]));
    let best = pairs[0].0;
    let near: Vec<[i32; 2]> = pairs.iter().take_while(|(cost, _)| *cost <= best + 2).map(|(_, p)| *p).collect();
    pick(rng, &near).copied().unwrap_or(pairs[0].1).to_vec()
}

pub fn generate_chord_lead(
    ctx: &ResolvedContext,
    profile: &RoleProfile,
    octave: u8,
    density: u8,
    rng: &mut Rng,
    busy: &HashSet<u32>,
) -> GeneratedPart {
    let total = ctx.length_steps;
    let key = Key { root: ctx.key_root, intervals: ctx.key_intervals };
    let octave = i32::from(octave);

    let trigs = rhythm_for(
        RhythmOpts {
            weights: &profile.weights,
            density: u32::from(density),
            bars: ctx.bars,
            // Like the chords: landing with the bass is a band, not a
            // collision.
            busy,
            avoid: 0.15,
            anchors: &[],
            trigs_per_bar: profile.trigs_per_bar,
        },
        rng,
    );
    let (_, _, len_max) = len_bounds(&profile.len);
    let leap = profile.octave_leap.unwrap_or(0.0);

    let mut notes = Vec::new();
    let mut previous_pedals: Vec<i32> = Vec::new();
    let mut pedals_for_bar: Option<(u32, Vec<i32>)> = None;
    for (i, trig) in trigs.iter().enumerate() {
        let slot_index = trig.bar as usize;
        let slot = ctx.bar_slots[slot_index];
        let root_low = degree_pitch(slot.degree, key.root, key.intervals, octave);
        let root_top = root_low + 12 * (ROOT_OCTAVES - 1);

        // Pedals are chosen once per bar and held — that is the whole sound.
        let same_bar = matches!(&pedals_for_bar, Some((bar, _)) if *bar == trig.bar);
        if !same_bar {
            let pool = pedal_pool(ctx, key, slot_index);
            let candidates = pedal_candidates(&pool, root_top - PEDAL_BELOW_TOP, root_top + PEDAL_ABOVE_TOP);
            let chosen = choose_pedals(&candidates, &pool, root_top, &previous_pedals, rng);
            if !chosen.is_empty() {
                previous_pedals = chosen.clone();
            }
            pedals_for_bar = Some((trig.bar, chosen));
        }
        let pedals = &pedals_for_bar.as_ref().expect("set a line above").1;

        // The root leaps octaves in a cycle that runs on across bar lines, as
        // A01's does. At the top it occasionally leaps one further — the D7 in
        // A01's third bar.
        let cycle = (i as i32) % ROOT_OCTAVES;
        let at_top = cycle == ROOT_OCTAVES - 1;
        let mut root = root_low + 12 * cycle;
        if at_top && chance(rng, leap) && root + 12 <= 127 {
            root += 12;
        }
        if !(0..=127).contains(&root) {
            continue;
        }

        // Thin the voicing where the root sits among the pedals.
        let mut voices: Vec<i32> = pedals.iter().copied().filter(|&p| p != root).collect();
        if at_top && voices.len() > 1 && chance(rng, THIN_AT_TOP) {
            let drop = if chance(rng, 0.5) { 0 } else { voices.len() - 1 };
            voices.remove(drop);
        }

        let gap = gap_after(&trigs, i, total);
        let want = (gap - BREATH).clamp(0.5, len_max);
        let len = digi_core::snap_len_fine(want, f64::from(total - trig.step));
        let velocity = velocity_for(trig.accent, trig.ghost, profile.velocity, u32::from(ctx.feel.humanize), rng);
        let micro = snap_micro(micro_for(trig.step, &ctx.profile.groove, u32::from(ctx.feel.humanize), rng));

        // No conditions: A01 is machine-flat, and a chord that drops out on
        // alternate bars is not this part. `profile.conditions` is empty for
        // the same reason, so nothing is being ignored here.
        for pitch in std::iter::once(root).chain(voices) {
            notes.push(NoteSpec {
                step: trig.step,
                pitch: pitch as u8,
                len,
                velocity,
                micro,
                prob: None,
                fill: None,
                cond: None,
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
    use std::collections::BTreeMap;

    fn ctx_for(over: GenContext) -> ResolvedContext {
        resolve_context(&over).unwrap()
    }

    fn generate(genre: GenreId, seed: u32, mut over: GenContext, density: u8, octave: u8) -> GeneratedPart {
        // The genre alone: `for_genre` would also reset the bars and the
        // progression, and these tests are about A01's.
        over.genre = genre;
        let ctx = ctx_for(over);
        let profile = role_profile(genre, Role::ChordLead);
        generate_chord_lead(&ctx, &profile, octave, density, &mut rng_for(seed, "chord-lead"), &HashSet::new())
    }

    fn a01_context() -> GenContext {
        // A minor: i VII IV VI — the first four bars of A01's own progression.
        GenContext {
            root: 9,
            progression: "i VII IV VI".into(),
            bars: 4,
            feel: crate::context::Feel { humanize: 0, ..crate::context::Feel::default() },
            ..GenContext::default()
        }
    }

    /// Every step's pitches, ascending — the shape the A4 export reads.
    fn by_step(part: &GeneratedPart) -> BTreeMap<u32, Vec<u8>> {
        let mut out: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
        for n in &part.notes {
            out.entry(n.step).or_default().push(n.pitch);
        }
        for v in out.values_mut() {
            v.sort_unstable();
        }
        out
    }

    /// Every step's root and pedals as the part *meant* them: the root is
    /// emitted first, and when it leaps into the pedal register it is not the
    /// lowest note — A01's D7 over A6 and D6.
    fn roots_and_pedals(part: &GeneratedPart) -> BTreeMap<u32, (u8, Vec<u8>)> {
        let mut out: BTreeMap<u32, (u8, Vec<u8>)> = BTreeMap::new();
        for n in &part.notes {
            match out.get_mut(&n.step) {
                None => {
                    out.insert(n.step, (n.pitch, Vec::new()));
                }
                Some((_, pedals)) => pedals.push(n.pitch),
            }
        }
        out
    }

    #[test]
    fn plays_straight_eighths_at_full_density() {
        for seed in 0..12u32 {
            let part = generate(GenreId::Dnb, seed, a01_context(), 100, 4);
            let steps: Vec<u32> = part.trigs.iter().map(|t| t.step).collect();
            assert_eq!(steps, (0..64).step_by(2).collect::<Vec<_>>(), "seed {seed}");
        }
    }

    #[test]
    fn never_exceeds_the_hardwares_four_notes_per_trig_and_never_doubles_a_pitch() {
        for genre in GenreId::ALL {
            for seed in 0..20u32 {
                for density in [30u8, 70, 100] {
                    let part = generate(genre, seed, a01_context(), density, 4);
                    for (step, pitches) in by_step(&part) {
                        assert!(pitches.len() <= MAX_CHORD_NOTES, "{genre:?} seed {seed} step {step}: {pitches:?}");
                        let mut distinct = pitches.clone();
                        distinct.dedup();
                        assert_eq!(distinct, pitches, "{genre:?} seed {seed} step {step} doubled a pitch");
                    }
                }
            }
        }
    }

    /// The A4 carries a chord as its lowest note plus offsets of at most 63
    /// semitones; this part is shaped so that every chord it makes fits.
    #[test]
    fn every_chord_fits_the_a4s_arp_offsets() {
        for seed in 0..20u32 {
            for octave in 1..=7u8 {
                let part = generate(GenreId::House, seed, a01_context(), 100, octave);
                for (step, pitches) in by_step(&part) {
                    let root = pitches[0];
                    assert!(pitches.iter().all(|&p| p - root <= 63), "seed {seed} oct {octave} step {step}: {pitches:?}");
                }
            }
        }
    }

    /// The root cycles three octaves, upward, and runs on across the bar line.
    #[test]
    fn the_root_leaps_octaves_in_a_rising_cycle() {
        let mut ctx = a01_context();
        ctx.progression = "i".into();
        ctx.bars = 1;
        let part = generate(GenreId::Techno, 3, ctx, 100, 4);
        let roots: Vec<u8> = roots_and_pedals(&part).values().map(|(root, _)| *root).collect();
        assert_eq!(roots.len(), 8);
        // A4 A5 A6 A4 A5 A6 A4 A5 in the box's labelling: 57, 69, 81, …
        // with the occasional extra leap at the top allowed for.
        for (i, &root) in roots.iter().enumerate() {
            let expected = 57 + 12 * (i as u8 % 3);
            assert!(root == expected || (i % 3 == 2 && root == expected + 12), "trig {i}: {root}");
        }
    }

    /// Two pedal tones hold across a bar while the root moves under them —
    /// the sound of A01 — and they are chord tones or extensions of the bar's
    /// chord.
    #[test]
    fn holds_two_pedal_tones_per_bar_drawn_from_the_bars_chord() {
        for seed in 0..12u32 {
            let part = generate(GenreId::Dnb, seed, a01_context(), 100, 4);
            let steps = roots_and_pedals(&part);
            let ctx = ctx_for(a01_context());
            let key = Key { root: ctx.key_root, intervals: ctx.key_intervals };
            for bar in 0..4usize {
                let pool = pedal_pool(&ctx, key, bar);
                let mut pedals: HashSet<u8> = HashSet::new();
                for (step, (_, uppers)) in steps.range((bar as u32 * 16)..((bar as u32 + 1) * 16)) {
                    for &p in uppers {
                        assert!(pool.contains(i32::from(p)), "seed {seed} step {step}: {p} is not in the chord");
                        pedals.insert(p);
                    }
                }
                assert_eq!(pedals.len(), 2, "seed {seed} bar {bar}: two pedals and no more, got {pedals:?}");
            }
        }
    }

    /// Bar 1 of A01 in its own key: E and C over A. The seed picks among near
    /// ties, so this asks that *some* seed lands on Elektron's exact pair.
    #[test]
    fn can_voice_a01s_first_bar_exactly() {
        let hit = (0..40u32).any(|seed| {
            let part = generate(GenreId::Dnb, seed, a01_context(), 100, 4);
            let steps = by_step(&part);
            let first = &steps[&0];
            first == &vec![57, 76, 84]
        });
        assert!(hit, "no seed in 40 voices A4 E6 C7 on step 1");
    }

    #[test]
    fn breathes_before_the_next_trig() {
        let part = generate(GenreId::Breaks, 5, a01_context(), 100, 4);
        for n in &part.notes {
            assert!(n.len < 2.0 && n.len >= 1.5, "{}", n.len);
        }
    }

    #[test]
    fn carries_no_conditions_because_a01_has_none() {
        let mut ctx = a01_context();
        ctx.feel.looseness = 100;
        let part = generate(GenreId::Electro, 9, ctx, 100, 4);
        assert!(part.notes.iter().all(|n| n.prob.is_none() && n.fill.is_none() && n.cond.is_none()));
    }
}
