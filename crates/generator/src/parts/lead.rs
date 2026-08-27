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
//   * **it can take turns with a second lead.** A [`LeadVoice`] other than
//     [`LeadVoice::Solo`] splits the pattern into a grid of turns
//     ([`trade_steps`]) and plays only in every other one, so the call
//     states a phrase and then *stops* — the rests are the feature — and
//     the response answers into the space with [`crate::motif::answer_motif`]
//     applied to what the call actually played. See "Taking turns" below.
//
// ## Taking turns
//
// Settled with Neil 2026-08-26. Three things make this call and response
// rather than two leads that happen to avoid each other:
//
//   * **The turn grid is derived, not configured.** One bar trades at the
//     half bar, two and four bars trade bar for bar, eight bars trade two
//     bars at a time — [`trade_steps`]. There is always room for at least
//     one answer, which a fixed trade length cannot promise across the four
//     pattern lengths the panel offers.
//   * **A turn is a phrase.** For a call or a response the motif window
//     *is* the trade length, rather than the genre's own `motif.window`:
//     an idea should fill the turn it is given. Only [`LeadVoice::Solo`]
//     keeps the profile's window, which is what makes an ordinary
//     [`Role::Lead`](crate::genres::Role::Lead) row generate byte-identical
//     music to before this existed.
//   * **The answer replies to what was heard.** [`CallTurn`] records the
//     call's material as it was *placed* — after thinning, after the
//     busy-step nudge, after anything the arrangement dropped — not the
//     idea behind it. A response to a phrase half of which never sounded
//     would be answering a question nobody asked.
//
// Quiet, but not sealed off: a call writes no trig inside the response's
// turn, though a held note may still ring across the line (its length is
// the gap to *its own* next trig, which is now a long way off), and a
// response may anticipate its entry by a step or two where the call left
// that space empty. Both are what stops the grid sounding like a grid.
//
// Pitch resolution: the motif's scale-degree offsets are walked along the
// scale (so the contour is the same shape in any key), and then a note on a
// beat is pulled onto the nearest chord tone of that bar. Strong beats agree
// with the harmony; weak beats are free to pass through.

use std::collections::HashSet;

use crate::context::ResolvedContext;
use crate::genres::RoleProfile;
use crate::motif::{answer_motif, answer_plan, develop_motif, make_motif, thin_motif, MakeMotifOpts, MotifNote};
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
    /// What a [`LeadVoice::Response`] row below this one answers. `Some`
    /// only for a [`LeadVoice::Call`]; `arrange` is what carries it from
    /// the call row down to the response row.
    pub call: Option<CallVoice>,
}

impl From<LeadPart> for GeneratedPart {
    fn from(l: LeadPart) -> Self {
        GeneratedPart { notes: l.notes, trigs: l.trigs }
    }
}

/// One turn of a call, as it was actually played: where the turn began, and
/// the material that survived onto the grid, in turn-relative steps. See
/// the module header's "The answer replies to what was heard" for why this
/// is the placed material and not the idea behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct CallTurn {
    pub start: u32,
    pub notes: Vec<MotifNote>,
}

/// Everything a response needs to hear from the call above it.
#[derive(Debug, Clone, PartialEq)]
pub struct CallVoice {
    /// The call's own turn length, carried rather than recomputed. Both
    /// voices derive it from the same pattern length today, so this is the
    /// same number either way — passing it is what stops them drifting
    /// apart if that ever stops being true.
    pub trade: u32,
    /// The density the call was generated at — see [`answer_density`] for
    /// the one thing a response needs it for.
    pub density: u8,
    pub turns: Vec<CallTurn>,
}

impl CallVoice {
    /// The turn an ear heard immediately before `start` — what an answer
    /// beginning there is answering. `None` before the call has played
    /// anything, and for a call that rested through its own turn this
    /// reaches back to the last one it *did* play, which is what an ear
    /// does too.
    fn heard_before(&self, start: u32) -> Option<&CallTurn> {
        self.turns.iter().filter(|t| t.start < start).max_by_key(|t| t.start)
    }
}

/// Which side of a call-and-response pair this lead is playing.
#[derive(Debug, Clone, Copy)]
pub enum LeadVoice<'a> {
    /// One lead across the whole pattern — an ordinary
    /// [`Role::Lead`](crate::genres::Role::Lead) row, and the only voice
    /// whose output this module promises is byte-identical to before call
    /// and response existed.
    Solo,
    Call,
    /// `None` is a response row with no call above it. It still trades —
    /// it plays its own turns and rests through the others, so a call added
    /// above it later drops straight into the space it left — but it
    /// develops its own motif, because there is nothing to answer.
    /// `arrange` is what says so on the status line.
    Response(Option<&'a CallVoice>),
}

impl LeadVoice<'_> {
    /// Which half of the turn grid this voice owns; `None` for a solo,
    /// which owns all of it.
    fn parity(&self) -> Option<u32> {
        match self {
            Self::Solo => None,
            Self::Call => Some(0),
            Self::Response(_) => Some(1),
        }
    }
}

/// How long one turn is, in steps.
///
/// Derived from the pattern rather than offered as a control (settled with
/// Neil, 2026-08-26). The arms cover the four lengths `GEN_BARS` offers,
/// and the thing none of them does is leave a voice with no turn at all —
/// which is the trap one fixed trade length falls into, since 16 steps
/// across a one-bar house pattern is a call with nothing after it.
pub fn trade_steps(total_steps: u32) -> u32 {
    match total_steps {
        // One bar: trade at the half bar, or there is no room to answer.
        0..=16 => 8,
        // Two or four bars: bar for bar, the classic.
        17..=64 => 16,
        // Eight bars: two-bar phrases, so an idea has somewhere to go
        // before it is handed over.
        _ => 32,
    }
    // Only reachable for a pattern shorter than a bar, which the panel
    // cannot ask for — but a turn longer than the pattern would silently
    // hand every step to the call.
    .min(total_steps.max(1))
}

/// One bar's harmony, worked out once. A solo phrase never crosses a bar
/// line in any shipped genre, but a turn of up to 32 steps always does, and
/// resolving a whole turn against its first bar's chord would put half of
/// every answer on the wrong harmony.
struct BarHarmony {
    tones: Vec<u8>,
    root: i32,
    root_index: i32,
}

/// The density to thin an answer by.
///
/// **An answer would otherwise be thinned twice.** A [`CallTurn`] holds the
/// call's material *as it was placed*, which means the call's own density
/// slider has already been applied to it — so thinning it again at the
/// response's density applies a slider twice and lands the answer at
/// roughly half the call's note count however the two are set, which reads
/// as a lead with an echo rather than as two voices talking. This converts
/// the response's density into the thinning still *owed*: equal sliders
/// leave the answer whole, and a response set below its call thins by the
/// difference.
///
/// A response set *above* its call clamps at "keep everything", which is
/// the honest ceiling — an answer cannot quote notes the call never played.
fn answer_density(response: u8, call: u8) -> f64 {
    // `thin_motif`'s own curve, inverted, so one number keeps one meaning.
    let keep = |d: u8| 0.45 + 0.55 * f64::from(d) / 100.0;
    (((keep(response) / keep(call) - 0.45) / 0.55) * 100.0).clamp(0.0, 100.0)
}

/// Leave the question open. A call that lands on the root has answered
/// itself, so it steps off to the nearest other chord tone — the third or
/// the fifth — and leaves the response somewhere to go. A call that was
/// already off the root, or a chord offering nothing else within reach, is
/// left exactly where it is.
fn leave_open(pitch: i32, tones: &[u8], root: i32) -> i32 {
    if (pitch - root).rem_euclid(12) != 0 {
        return pitch;
    }
    let others: Vec<u8> = tones.iter().copied().filter(|&t| (i32::from(t) - root).rem_euclid(12) != 0).collect();
    to_chord_tone(pitch, &others, 5)
}

struct Placed {
    step: u32,
    pitch: i32,
    want: f64,
    bar: u32,
    /// The last note of a call's or a response's turn — the one allowed to
    /// ring across the handover. See the note length below.
    closes_turn: bool,
}

/// An ordinary lead: one voice, the whole pattern.
pub fn generate_lead(ctx: &ResolvedContext, profile: &RoleProfile, octave: u8, density: u8, rng: &mut Rng, busy: &HashSet<u32>) -> LeadPart {
    generate_lead_voice(ctx, profile, octave, density, rng, busy, LeadVoice::Solo)
}

/// A lead playing one side of a conversation — see the module header.
/// [`LeadVoice::Solo`] is [`generate_lead`], and takes exactly the path it
/// always did.
pub fn generate_lead_voice(
    ctx: &ResolvedContext,
    profile: &RoleProfile,
    octave: u8,
    density: u8,
    rng: &mut Rng,
    busy: &HashSet<u32>,
    voice: LeadVoice<'_>,
) -> LeadPart {
    let (min, max) = window_for(profile.span, i32::from(octave));
    let total = ctx.length_steps;
    let key = Key { root: ctx.key_root, intervals: ctx.key_intervals };

    let (motif_notes, motif_window) = match profile.motif {
        Some(m) => (m.notes, m.window),
        None => ((3, 5), 8),
    };
    // Integers, so this one really is `clamp` — unlike the four `f64` chains in
    // `motif` and `protocol::pattern`, which keep `.min().max()` on purpose.
    let solo_window = motif_window.clamp(2, 16) as u32;

    // A turn is a phrase: a solo's is the genre's own motif window walked
    // end to end, a call's or a response's is one side of the turn grid.
    // Nothing from here to the first `rng` draw below touches the rng, and
    // the solo arm reproduces `phrases = (total / window).max(1)` exactly,
    // which is what keeps a `Role::Lead` row's seeded output where it was.
    let trade = voice.parity().map(|_| trade_steps(total));
    let window = trade.unwrap_or(solo_window);
    let turns: Vec<u32> = match voice.parity() {
        None => (0..(total / window).max(1)).map(|p| p * window).collect(),
        Some(parity) => (0..total).step_by(window as usize).filter(|s| (s / window) % 2 == parity).collect(),
    };

    // **The note count scales with the turn.** `motif.notes` is a count for
    // the genre's *own* window, so handing it a longer turn unchanged asks
    // for the same few notes to cover twice or four times the ground — a
    // DnB call with three notes to fill a bar where the solo lead beside it
    // plays six, which reads as a part that failed to generate rather than
    // as a phrase. Stretching it by how much longer the turn is keeps each
    // genre's own notes-per-step, whatever its window: electro states its
    // idea in four steps and DnB in eight, and both should sound like
    // themselves. `make_motif` still caps the count at one note per step.
    // A solo's stretch is exactly 1.0 — its window *is* the genre's.
    let stretch = (f64::from(window) / f64::from(solo_window)).max(1.0);
    let stretched = |n: u32| ((f64::from(n) * stretch).round() as i64).max(1);
    let motif = make_motif(
        rng,
        MakeMotifOpts { notes: (stretched(motif_notes.0), stretched(motif_notes.1)), window, weights: &profile.weights, spread: 2 },
    );
    let plan = crate::motif::motif_plan(rng, turns.len() as u32, f64::from(ctx.feel.looseness));
    // **Drawn for both `Response` arms, and used by only one.** A response
    // with no call above it takes the same numbers out of its stream as one
    // with a call above it, so adding the call row later changes what this
    // row *answers* without also reshuffling its motif, its density skips
    // and its pickups. Paying one unused draw for that is the trade.
    let answers = match voice {
        LeadVoice::Response(_) => answer_plan(rng, turns.len() as u32, f64::from(ctx.feel.looseness)),
        LeadVoice::Solo | LeadVoice::Call => Vec::new(),
    };

    let palette = scale_pitches_in_window(key.root, key.intervals, min, max);
    let window_bounds =
        ChordWindow { octave: i32::from(octave), min: min.clamp(0, 127) as u8, max: max.clamp(0, 127) as u8, ..Default::default() };
    let harmony: Vec<BarHarmony> = (0..ctx.bars.max(1))
        .map(|b| {
            let slot = ctx.bar_slots.get(b as usize).copied().unwrap_or(ctx.bar_slots[0]);
            let root = slot_root_pitch(&slot, key, i32::from(octave), min, max);
            BarHarmony { tones: chord_tones(&slot, key, window_bounds), root, root_index: nearest_index(&palette, root) as i32 }
        })
        .collect();
    let bar_of = |step: u32| ((step / 16) as usize).min(harmony.len() - 1);

    // **Two turns is not enough to rest through one.** A call or a response
    // gets at most two turns at any pattern length the panel offers, so the
    // density skip below would cost it half of its half of the
    // conversation — and a pair where one voice sits out reads as a single
    // lead playing every other bar, which is the one thing this is not. A
    // solo keeps the skip whatever its phrase count: its phrases are
    // eighths of a pattern, not halves of a voice.
    let may_rest = trade.is_none() || turns.len() > 2;


    let mut taken: HashSet<u32> = HashSet::new();
    let mut placed: Vec<Placed> = Vec::new();
    let mut call_turns: Vec<CallTurn> = Vec::new();

    for (i, &start) in turns.iter().enumerate() {
        // A solo's phrase runs to the next phrase; a turn stops at the
        // handover, and a note that would land past it belongs to the other
        // voice.
        let turn_end = trade.map(|t| (start + t).min(total)).unwrap_or(total);

        // Space is a musical answer: at low density whole phrases are left
        // out, but never the first, which is where the idea gets stated —
        // and never at all for a voice with only two turns, see `may_rest`.
        if i > 0 && may_rest && chance(rng, (1.0 - f64::from(density) / 100.0) * 0.35) {
            continue;
        }

        let heard = match voice {
            LeadVoice::Response(Some(call)) => call.heard_before(start),
            _ => None,
        };
        let call_density = match voice {
            LeadVoice::Response(Some(call)) => call.density,
            _ => density,
        };
        let (material, thin_by) = match heard {
            Some(turn) => (answer_motif(&turn.notes, answers[i], window, rng), answer_density(density, call_density)),
            None => (develop_motif(&motif, plan[i], window, rng), f64::from(density)),
        };
        let phrase = thin_motif(&material, thin_by, rng);

        // A response may anticipate its entry by a step or two where the
        // call left that space empty — the pickup that stops the turn grid
        // sounding like a grid. Looseness decides how often.
        let pickup = match voice {
            LeadVoice::Response(_) if start > 0 && chance(rng, 0.2 + 0.4 * f64::from(ctx.feel.looseness) / 100.0) => {
                if chance(rng, 0.5) {
                    1
                } else {
                    2
                }
            }
            _ => 0,
        };

        let mut heard_now: Vec<MotifNote> = Vec::new();
        let mut last_placed: Option<usize> = None;

        for (j, n) in phrase.iter().enumerate() {
            let mut step = start + n.step;
            if step >= total || step >= turn_end {
                continue;
            }
            if j == 0 && pickup > 0 && n.step == 0 {
                let early = start.saturating_sub(pickup);
                if early < start && !busy.contains(&early) && !taken.contains(&early) {
                    step = early;
                }
            }

            // Answer, don't double: a step another part owns is nudged one
            // later where there's room, and given up where there isn't. The
            // nudge may not push a note past `turn_end` — the whole point of
            // a turn is that the other voice owns what comes after it. (For
            // a solo `turn_end` *is* `total`, so this is the condition it
            // always had.)
            if busy.contains(&step) {
                let nudged = step + 1;
                if nudged < total && nudged < turn_end && !busy.contains(&nudged) && !taken.contains(&nudged) {
                    step = nudged;
                } else if chance(rng, 0.7) {
                    continue;
                }
            }
            if taken.contains(&step) {
                continue;
            }
            taken.insert(step);

            // A solo resolves a phrase against the bar the phrase starts
            // in, unchanged; a turn is long enough to cross a bar line, so
            // it resolves each note against the bar the note lands in.
            let h = &harmony[bar_of(if matches!(voice, LeadVoice::Solo) { start } else { step })];
            let Some(walked) = walk(&palette, h.root_index, n.deg) else { continue };
            let pitch = fold_into_window(if is_beat(step) { to_chord_tone(walked, &h.tones, 2) } else { walked }, min, max);
            heard_now.push(MotifNote { step: step.saturating_sub(start), ..*n });
            last_placed = Some(placed.len());
            placed.push(Placed { step, pitch, want: n.len, bar: step / 16, closes_turn: false });
        }

        // **A turn must say something.** A voice that loses its whole turn
        // to the busy map has left a hole where its half of the
        // conversation should be, and unlike a solo — which can make a
        // dropped note up anywhere in the pattern — a turn is all the room
        // this voice gets. One bar of house is the case that finds it: the
        // response's turn is eight steps, and the bass, the chords and the
        // lead above it can own all eight. So a turn that placed nothing
        // places its first note anyway, on the best free step it can find,
        // and on a doubled step if there is genuinely no free one — one note
        // in unison reads far better than a silent answer. A turn skipped
        // for density never reaches here: that rest was asked for.
        if trade.is_some() && last_placed.is_none() {
            if let Some(first) = phrase.first() {
                // The phrase's own positions first — they are where the
                // idea wanted to be — then anywhere else in the turn.
                let candidates: Vec<u32> = phrase
                    .iter()
                    .map(|n| start + n.step)
                    .chain(start..turn_end)
                    .filter(|s| *s < turn_end && *s < total)
                    .collect();
                let free = candidates.iter().find(|s| !busy.contains(s) && !taken.contains(s));
                if let Some(&step) = free.or_else(|| candidates.iter().find(|s| !taken.contains(s))) {
                    taken.insert(step);
                    let h = &harmony[bar_of(step)];
                    if let Some(walked) = walk(&palette, h.root_index, first.deg) {
                        // Straight onto a chord tone whatever the beat: this
                        // is the only note of the turn, so it carries the
                        // whole answer and had better land.
                        let pitch = fold_into_window(to_chord_tone(walked, &h.tones, 3), min, max);
                        heard_now.push(MotifNote { step: step.saturating_sub(start), ..*first });
                        last_placed = Some(placed.len());
                        placed.push(Placed { step, pitch, want: first.len, bar: step / 16, closes_turn: false });
                    }
                }
            }
        }

        // The note a phrase ends on is the one an ear grades it by, so the
        // two voices end a turn differently: a response closes onto a chord
        // tone whatever the beat it fell on, and a call steps *off* the root
        // if it landed there, leaving the question open. A solo does
        // neither — it isn't in a conversation.
        if let Some(last) = last_placed {
            let h = &harmony[bar_of(placed[last].step)];
            let closed = match voice {
                LeadVoice::Solo => placed[last].pitch,
                LeadVoice::Call => leave_open(placed[last].pitch, &h.tones, h.root),
                LeadVoice::Response(_) => to_chord_tone(placed[last].pitch, &h.tones, 4),
            };
            placed[last].pitch = fold_into_window(closed, min, max);
            placed[last].closes_turn = trade.is_some();
        }
        if matches!(voice, LeadVoice::Call) {
            call_turns.push(CallTurn { start, notes: heard_now });
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
            // **A note that closes a turn asks for the profile's ceiling,
            // not the motif's own length.** `make_motif` measures a note's
            // length to the next note *inside the phrase window*, so the
            // last note of a turn is capped at the handover by construction
            // — which would make the "quiet, with tails" the module header
            // promises a hard gate instead. The two caps that follow are
            // what keep it honest: `gap` is the distance to this voice's
            // own next trig (a whole turn away, so it does not bite) and
            // `len_max` is the ceiling the genre already sets for every
            // other note. A solo has no turns and so never takes this arm.
            let want = if n.closes_turn { len_max } else { n.want };
            let len = digi_core::snap_len_fine(want.min(gap).min(len_max), f64::from(total - n.step));
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

    // A call's last note has nothing after it to be cut short by, so its
    // tail rings across the handover on its own — see the module header.
    let call = matches!(voice, LeadVoice::Call).then(|| CallVoice { trade: window, density, turns: call_turns });
    LeadPart { notes, trigs, motif, call }
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

    // --- Taking turns ---------------------------------------------------

    /// A call and the response that answers it, from one seed, the way
    /// `arrange` builds them: separate rng streams, the call's steps in the
    /// response's busy map, the call's turns handed down.
    fn pair(over: GenContext, genre: GenreId, density: u8) -> (LeadPart, LeadPart) {
        let seed = over.seed;
        let ctx = resolve_context(&over).unwrap();
        let call = generate_lead_voice(
            &ctx,
            &role_profile(genre, Role::LeadCall),
            6,
            density,
            &mut rng_for(seed, "call"),
            &HashSet::new(),
            LeadVoice::Call,
        );
        let busy: HashSet<u32> = call.notes.iter().map(|n| n.step).collect();
        let response = generate_lead_voice(
            &ctx,
            &role_profile(genre, Role::LeadResponse),
            6,
            density,
            &mut rng_for(seed, "response"),
            &busy,
            LeadVoice::Response(call.call.as_ref()),
        );
        (call, response)
    }

    #[test]
    fn the_turn_grid_always_leaves_room_for_an_answer() {
        // Every pattern length the panel offers, and the property that
        // matters for all four: both voices get a turn. A single fixed trade
        // length cannot promise this — 16 steps across a one-bar house
        // pattern is a call with nothing after it.
        for bars in crate::context::GEN_BARS {
            let total = bars * 16;
            let trade = trade_steps(total);
            assert!(trade > 0 && total % trade == 0, "{bars} bars: {trade} does not divide {total}");
            let turns: Vec<u32> = (0..total).step_by(trade as usize).collect();
            assert!(turns.len() >= 2, "{bars} bars: {} turn(s), nobody answers", turns.len());
            assert!(turns.iter().any(|s| (s / trade) % 2 == 0));
            assert!(turns.iter().any(|s| (s / trade) % 2 == 1));
        }
        // The four lengths, pinned — this is the answer to "how long is a
        // turn", and it is meant to be read off, not re-derived.
        assert_eq!([16, 32, 64, 128].map(trade_steps), [8, 16, 16, 32]);
    }

    #[test]
    fn the_call_and_the_response_take_turns() {
        for genre in GenreId::ALL {
            for bars in crate::context::GEN_BARS {
                for seed in 0..8u32 {
                    let over = GenContext { genre, seed, bars, ..GenContext::default() };
                    let trade = trade_steps(bars * 16);
                    let (call, response) = pair(over, genre, 50);
                    for n in &call.notes {
                        assert_eq!((n.step / trade) % 2, 0, "{genre:?}/{bars}b/{seed}: a call trigged at {} in the response's turn", n.step);
                    }
                    for n in &response.notes {
                        // A response may anticipate its entry by up to two
                        // steps — the pickup — so a trig in the tail of the
                        // call's turn is allowed, and only there.
                        let turn = n.step / trade;
                        let into_next = trade - (n.step % trade);
                        assert!(
                            turn % 2 == 1 || into_next <= 2,
                            "{genre:?}/{bars}b/{seed}: a response trigged at {} inside the call's turn",
                            n.step
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn neither_voice_is_silent() {
        // The rests are the feature, but a voice that never plays is a dead
        // row, and both are one `parity` slip away from being one.
        for genre in GenreId::ALL {
            for bars in crate::context::GEN_BARS {
                for density in [0u8, 50, 100] {
                    for seed in 0..6u32 {
                        let over = GenContext { genre, seed, bars, ..GenContext::default() };
                        let (call, response) = pair(over, genre, density);
                        assert!(!call.notes.is_empty(), "{genre:?}/{bars}b/d{density}/{seed}: the call said nothing");
                        assert!(!response.notes.is_empty(), "{genre:?}/{bars}b/d{density}/{seed}: the response said nothing");
                    }
                }
            }
        }
    }

    #[test]
    fn the_response_answers_the_call_it_actually_heard() {
        // The property that separates this from two leads that avoid each
        // other: change what the call plays, and the answer changes — from
        // the *same* response stream, same seed, same everything else. If
        // the response were only dodging busy steps this would tie most of
        // the time, because a call's steps are in the other voice's turn
        // and were never in its way to begin with.
        let mut differed = 0;
        for seed in 0..30u32 {
            let over = GenContext { genre: GenreId::Dnb, seed, bars: 4, ..GenContext::default() };
            let ctx = resolve_context(&over).unwrap();
            let profile = role_profile(GenreId::Dnb, Role::LeadResponse);
            let answer_to = |call: Option<&CallVoice>| -> Vec<NoteSpec> {
                generate_lead_voice(&ctx, &profile, 6, 50, &mut rng_for(seed, "response"), &HashSet::new(), LeadVoice::Response(call))
                    .notes
            };
            let call_a = generate_lead_voice(
                &ctx,
                &role_profile(GenreId::Dnb, Role::LeadCall),
                6,
                50,
                &mut rng_for(seed, "call-a"),
                &HashSet::new(),
                LeadVoice::Call,
            );
            let call_b = generate_lead_voice(
                &ctx,
                &role_profile(GenreId::Dnb, Role::LeadCall),
                6,
                50,
                &mut rng_for(seed, "call-b"),
                &HashSet::new(),
                LeadVoice::Call,
            );
            if call_a.call == call_b.call {
                continue; // two identical calls have nothing to tell apart
            }
            let pitches = |notes: Vec<NoteSpec>| notes.iter().map(|n| n.pitch).collect::<Vec<_>>();
            if pitches(answer_to(call_a.call.as_ref())) != pitches(answer_to(call_b.call.as_ref())) {
                differed += 1;
            }
        }
        assert!(differed > 20, "only {differed}/30 answers changed when the call did");
    }

    #[test]
    fn a_call_records_what_it_played_not_what_it_meant_to_play() {
        // `CallTurn` is the material that reached the grid. Every recorded
        // note must correspond to a trig the call actually wrote, or the
        // response is answering a phrase nobody heard.
        for seed in 0..20u32 {
            let over = GenContext { genre: GenreId::Breaks, seed, bars: 4, ..GenContext::default() };
            let (call, _) = pair(over, GenreId::Breaks, 60);
            let voice = call.call.expect("a call row leaves its turns behind");
            let steps: HashSet<u32> = call.notes.iter().map(|n| n.step).collect();
            assert!(!voice.turns.is_empty());
            for turn in &voice.turns {
                assert_eq!(turn.start % (voice.trade * 2), 0, "a call turn began in the response's half");
                for n in &turn.notes {
                    assert!(steps.contains(&(turn.start + n.step)), "turn {} note {} never sounded", turn.start, n.step);
                }
            }
        }
    }

    #[test]
    fn a_response_with_no_call_still_keeps_to_its_own_turns() {
        // The orphan case. It trades with itself rather than sprawling
        // across the whole pattern, so dropping a call row in above it later
        // lands in space it already left. `arrange` is what says so on the
        // status line; this is the music half of that promise.
        for genre in GenreId::ALL {
            for seed in 0..8u32 {
                let over = GenContext { genre, seed, bars: 4, ..GenContext::default() };
                let ctx = resolve_context(&over).unwrap();
                let orphan = generate_lead_voice(
                    &ctx,
                    &role_profile(genre, Role::LeadResponse),
                    6,
                    50,
                    &mut rng_for(seed, "response"),
                    &HashSet::new(),
                    LeadVoice::Response(None),
                );
                assert!(!orphan.notes.is_empty());
                let trade = trade_steps(64);
                for n in &orphan.notes {
                    let into_next = trade - (n.step % trade);
                    assert!((n.step / trade) % 2 == 1 || into_next <= 2, "orphan trigged at {}", n.step);
                }
            }
        }
    }

    #[test]
    fn the_response_ends_its_turn_on_a_chord_tone() {
        // The consequent closes. Checked on the last trig of each turn,
        // whatever beat it fell on — an unresolved answer is what makes a
        // pair sound like two leads rather than a conversation.
        let over = GenContext { genre: GenreId::Electro, seed: 4, progression: "i".into(), bars: 4, ..GenContext::default() };
        let (_, response) = pair(over, GenreId::Electro, 60);
        let trade = trade_steps(64);
        let mut checked = 0;
        for turn in (0..64).step_by(trade as usize).filter(|s| (s / trade) % 2 == 1) {
            let Some(last) = response.notes.iter().rfind(|n| n.step >= turn && n.step < turn + trade) else {
                continue;
            };
            assert!(
                [0, 3, 7].contains(&(i32::from(last.pitch).rem_euclid(12))),
                "turn {turn} ended on {} — not a tone of the i chord",
                last.pitch
            );
            checked += 1;
        }
        assert!(checked >= 2, "only {checked} turns had a closing note to check");
    }

    #[test]
    fn a_call_leaves_the_question_open() {
        // The other half of the same idea: a call that ends on the root has
        // answered itself. Measured as a rate across seeds rather than
        // per-seed, because a call whose last note is nowhere near a chord
        // tone is left where it is — `leave_open` moves a note off the root,
        // it does not drag one across the bar to avoid it.
        let mut on_root = 0;
        let mut turns = 0;
        for seed in 0..40u32 {
            let over = GenContext { genre: GenreId::Dnb, seed, progression: "i".into(), bars: 4, ..GenContext::default() };
            let (call, _) = pair(over, GenreId::Dnb, 60);
            let trade = trade_steps(64);
            for turn in (0..64).step_by(trade as usize).filter(|s| (s / trade) % 2 == 0) {
                if let Some(last) = call.notes.iter().rfind(|n| n.step >= turn && n.step < turn + trade) {
                    turns += 1;
                    if i32::from(last.pitch).rem_euclid(12) == 0 {
                        on_root += 1;
                    }
                }
            }
        }
        assert!(turns > 40);
        assert!((on_root as f64) / (turns as f64) < 0.1, "{on_root}/{turns} calls closed on the root anyway");
    }

    #[test]
    fn a_response_speaks_under_the_call() {
        // The one thing `genres::role_profile` decides about the pair. Two
        // identical velocity curves alternating read as one lead with holes
        // in it, so the answer sits a few points under — checked on the
        // profile rather than on generated notes, which Humanize wobbles.
        for genre in GenreId::ALL {
            let call = role_profile(genre, Role::LeadCall).velocity;
            let response = role_profile(genre, Role::LeadResponse).velocity;
            let lead = role_profile(genre, Role::Lead).velocity;
            assert_eq!((call.accent, call.normal, call.ghost), (lead.accent, lead.normal, lead.ghost));
            assert!(response.accent < call.accent && response.normal < call.normal && response.ghost < call.ghost);
            assert!(response.ghost >= 1);
        }
    }

    #[test]
    fn a_call_can_ring_across_the_handover_but_never_trigs_there() {
        // "Quiet, with tails": the length of a call's last note is the gap to
        // its *own* next trig, which is now a turn away, so it sustains past
        // the line under its profile's ceiling. That is the difference
        // between a rest and a hard gate, and it costs no code — this is the
        // test that stops someone adding the gate.
        let mut rang_over = 0;
        for seed in 0..40u32 {
            let over = GenContext { genre: GenreId::Dnb, seed, bars: 2, ..GenContext::default() };
            let (call, _) = pair(over, GenreId::Dnb, 70);
            let trade = trade_steps(32);
            if let Some(last) = call.notes.iter().rfind(|n| n.step < trade) {
                if f64::from(last.step) + last.len > f64::from(trade) {
                    rang_over += 1;
                }
            }
        }
        assert!(rang_over > 0, "no call in 40 seeds held a note across the handover");
    }

    #[test]
    fn taking_turns_is_deterministic_for_a_seed() {
        for genre in GenreId::ALL {
            let over = GenContext { genre, seed: 99, bars: 4, ..GenContext::default() };
            let shape = |p: &LeadPart| p.notes.iter().map(|n| format!("{}:{}:{}", n.step, n.pitch, n.len)).collect::<Vec<_>>();
            let (call_a, response_a) = pair(over.clone(), genre, 55);
            let (call_b, response_b) = pair(over, genre, 55);
            assert_eq!(shape(&call_a), shape(&call_b));
            assert_eq!(shape(&response_a), shape(&response_b));
            assert_eq!(call_a.call, call_b.call);
        }
    }

    #[test]
    fn an_answer_is_not_thinned_twice() {
        // The bug `answer_density` exists to stop. A `CallTurn` holds the
        // call's material *after* the call's own density thinned it, so
        // thinning it again at the response's density applies one slider
        // twice — and with both rows at the panel's default the response
        // came out at roughly half the call's note count, which reads as a
        // lead with an echo rather than as two voices talking.
        //
        // Measured as a ratio across genres and lengths rather than pinned
        // per seed: the exact counts are a function of every weight table
        // in `genres.rs` and should be free to move, but a response
        // systematically half its call's size is the defect returning.
        for genre in GenreId::ALL {
            for bars in crate::context::GEN_BARS {
                let (mut call_notes, mut response_notes) = (0usize, 0usize);
                for seed in 0..24u32 {
                    let over = GenContext { genre, seed, bars, ..GenContext::default() };
                    let (call, response) = pair(over, genre, 40);
                    call_notes += call.notes.len();
                    response_notes += response.notes.len();
                }
                let ratio = response_notes as f64 / call_notes as f64;
                assert!(ratio > 0.7, "{genre:?}/{bars}b: the answer is {ratio:.2} of the call ({response_notes} to {call_notes})");
            }
        }
    }

    #[test]
    fn the_answer_density_curve_is_thinning_still_owed_not_thinning_again() {
        // Equal sliders leave the answer whole; a response under its call
        // thins by the difference; a response over its call stops at "keep
        // everything", because an answer cannot quote what was never played.
        for d in [0u8, 25, 40, 60, 100] {
            assert_eq!(answer_density(d, d).round(), 100.0, "equal sliders should thin nothing");
        }
        assert!(answer_density(0, 40) < answer_density(20, 40));
        assert!(answer_density(20, 40) < answer_density(40, 40));
        assert_eq!(answer_density(100, 40), 100.0);
        // And below the ceiling the compensation is exact: thinning at the
        // owed density leaves the same fraction of the original idea as one
        // pass at the response's own density would have. Only checked for a
        // response at or under its call, which is where no clamp bites —
        // above it the ceiling is the point, and is checked just above.
        let keep = |d: f64| 0.45 + 0.55 * d / 100.0;
        for call in [10u8, 40, 80, 100] {
            for response in [0u8, 30, 60, 100] {
                if response > call {
                    continue;
                }
                let net = keep(f64::from(call)) * keep(answer_density(response, call));
                assert!((net - keep(f64::from(response))).abs() < 1e-9, "call {call}, response {response}: {net}");
            }
        }
    }

    #[test]
    fn a_turn_is_as_busy_as_the_genre_plays() {
        // The other half of the same story: `motif.notes` is a count for the
        // genre's *own* window, so a turn twice that long handed the count
        // unchanged asks three notes to cover a bar where the solo lead
        // beside it plays six. Checked against the solo as the reference,
        // since a call is a lead that plays in half the pattern — per step
        // of its own airtime it should sound like one.
        for genre in GenreId::ALL {
            for bars in [2u32, 4, 8] {
                let (mut solo_notes, mut call_notes) = (0usize, 0usize);
                for seed in 0..24u32 {
                    let over = GenContext { genre, seed, bars, ..GenContext::default() };
                    let ctx = resolve_context(&over).unwrap();
                    solo_notes += generate_lead(
                        &ctx,
                        &role_profile(genre, Role::Lead),
                        6,
                        40,
                        &mut rng_for(seed, "solo"),
                        &HashSet::new(),
                    )
                    .notes
                    .len();
                    call_notes += pair(over, genre, 40).0.notes.len();
                }
                // A call owns half the pattern, so half the solo's count is
                // parity. Anything near a quarter is the un-stretched motif
                // spreading three notes over a bar.
                let per_step = (call_notes * 2) as f64 / solo_notes as f64;
                assert!(per_step > 0.7, "{genre:?}/{bars}b: a call is {per_step:.2} of a solo per step of its own airtime");
            }
        }
    }

    #[test]
    fn both_voices_produce_notes_the_hardware_can_hold() {
        for genre in GenreId::ALL {
            for density in [0u8, 40, 100] {
                for bars in crate::context::GEN_BARS {
                    for seed in 0..4u32 {
                        let over = GenContext { genre, seed, bars, ..GenContext::default() };
                        let (min, max) = window_for(role_profile(genre, Role::LeadCall).span, 6);
                        let (call, response) = pair(over, genre, density);
                        for part in [&call, &response] {
                            for n in &part.notes {
                                assert!(i32::from(n.pitch) >= min && i32::from(n.pitch) <= max, "{} outside {min}..={max}", n.pitch);
                                assert!((1..=127).contains(&n.velocity));
                                assert!(n.step < bars * 16);
                                assert!(n.len > 0.0);
                            }
                        }
                    }
                }
            }
        }
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
