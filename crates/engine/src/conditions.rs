//! Per-trig conditions: whether a trig fires this pass.
//!
//! Ported from `shouldPlay` in `js/midi.js`, keeping its two-level PROB model
//! exactly — a trig's own PROB lock overrides the track default, *including* an
//! explicit 100 — and then extended where PLAN.md §4 says a multi-track engine
//! can be honest about conditions the browser could not evaluate.
//!
//! # What changed from the browser, and why it is allowed to
//!
//! `js/midi.js` documents `PRE`, `NEI`, `LST` and `FILL` as unsimulable, and
//! says why: digi-roll plays one track at a time, keeps no history of previous
//! conditional results, has no FILL button and never knows a pattern change is
//! coming. Three of those four stop being true here:
//!
//! | Condition | Browser | Here |
//! |---|---|---|
//! | `PROB`, `1ST`, `A:B` | simulated | simulated, byte-identically |
//! | `PRE` | not simulable | simulated — last condition result on this track |
//! | `NEI` | not simulable | simulated — last condition result on track *n−1* of the same device |
//! | `FILL` | no FILL button | simulated — the transport's FILL toggle |
//! | `LST` | unknowable | simulated **in song mode** — the track's last pass before the row changes |
//!
//! `LST` was the fourth, and it stopped being unsimulable when song mode landed
//! (PLAN.md §6 phase 12): a song row knows when it ends, so "is this the last
//! pass of this track before the pattern changes" has an answer. In *pattern*
//! mode it still has none — nothing knows whether the next scene switch is a bar
//! away or an hour — so [`CondContext::last_pass`] is `None` there and the trig
//! plays.
//!
//! **The rule that survives all of it: anything unsimulated plays**, so the
//! sequencer is never quieter than the box. That covers `LST` in pattern mode,
//! `PRE` before this track has evaluated any condition, `NEI` on track 1, and
//! any condition string this build does not recognise.
//!
//! `NEI` reads track *n−1* **of the same device**, never across boxes — a
//! neighbour is a physical neighbour on one machine (PLAN.md §2).

use crate::rng::Rng;

/// A parsed trig condition.
///
/// Parsed rather than matched on a `&str` at evaluation time so the engine
/// thread does no string work per trig, and so an unrecognised condition is a
/// single named case instead of a fall-through nobody notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondKind {
    /// The previous evaluated condition on this track was true.
    Pre,
    /// The previous evaluated condition on track *n−1* of this device was true.
    Nei,
    /// First pass of the pattern.
    First,
    /// Last pass before a pattern change. Unknowable in pattern mode; answered
    /// in song mode, where the row says when the scene changes.
    Last,
    /// `A:B` — plays on pass `A` of every `B`.
    Ratio { a: u32, b: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cond {
    pub kind: CondKind,
    pub negated: bool,
}

impl Cond {
    /// Parse the condition strings the boxes' COND menu uses, as digi-roll
    /// stores them: `PRE`, `!PRE`, `NEI`, `!NEI`, `1ST`, `!1ST`, `LST`, `!LST`,
    /// and `A:B` / `!A:B`.
    ///
    /// `None` for anything unrecognised — including the empty string, which the
    /// JS's `if (!cond)` also treats as no condition at all. An unrecognised
    /// condition then plays, per the rule above.
    pub fn parse(s: &str) -> Option<Self> {
        let (negated, key) = match s.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let kind = match key {
            "PRE" => CondKind::Pre,
            "NEI" => CondKind::Nei,
            "1ST" => CondKind::First,
            "LST" => CondKind::Last,
            _ => {
                // The JS gate is /^\d+:\d+$/ — digits both sides, nothing else.
                let (a, b) = key.split_once(':')?;
                if a.is_empty() || b.is_empty() {
                    return None;
                }
                if !a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()) {
                    return None;
                }
                CondKind::Ratio {
                    a: a.parse().ok()?,
                    b: b.parse().ok()?,
                }
            }
        };
        Some(Cond { kind, negated })
    }
}

/// What the engine knows at the moment a trig is evaluated.
///
/// `prev_on_track` and `prev_on_neighbour` are the two facts the browser could
/// not supply. Both are `Option`: `None` means "no condition has been evaluated
/// there yet", which is a real state (the first bar of a pattern; track 1, which
/// has no neighbour) and not a missing value to paper over.
#[derive(Debug, Clone, Copy, Default)]
pub struct CondContext {
    /// How many complete passes of this track have already played. `1ST` is
    /// `loop_index == 0`.
    pub loop_index: u64,
    /// The transport's FILL toggle.
    pub fill_active: bool,
    pub prev_on_track: Option<bool>,
    pub prev_on_neighbour: Option<bool>,
    /// Whether this is the track's last pass before the pattern changes —
    /// `LST`. `None` outside song mode, where nothing knows a change is coming,
    /// and the trig then plays.
    ///
    /// Not filled in by [`CondHistory::context_for`]: it is not history, it is
    /// the arrangement, and the scheduler is the only thing that knows when the
    /// row it is on ends.
    pub last_pass: Option<bool>,
}

/// The result of evaluating one trig.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrigOutcome {
    pub plays: bool,
    /// `Some(result)` when this trig carried a condition the engine could
    /// actually evaluate — which is what a later `PRE` or `NEI` consults.
    ///
    /// `None` for an unconditional trig, and for one whose condition could not be
    /// evaluated — `LST` in pattern mode, and nothing else now. Neither touches
    /// the history: an unconditional trig does not
    /// participate in `PRE` on the box either, and recording a *guess* for `LST`
    /// would propagate it into every downstream `PRE`, which is worse than the
    /// gap it fills.
    ///
    /// This is the *condition's* result, not whether the note sounded. A trig
    /// silenced by probability still had its condition come out true or false,
    /// and PROB is a separate lane from COND on both boxes.
    pub cond_result: Option<bool>,
}

/// Whether a trig fires, and what it contributes to the condition history.
///
/// The order of business is `js/midi.js`'s, unchanged:
///
/// 1. `prob` is the trig's own PROB lock, or the track default if it has none.
///    100 (the box default) is indistinguishable from no probability at all.
/// 2. **`rng` is drawn exactly once, always** — even at 100%, where the draw
///    cannot change the answer. The JS does the same, and it matters: skipping
///    the draw as an optimisation would make a seeded run diverge from the same
///    seed on a pattern whose only difference is a trig at 100%.
/// 3. FILL, then the condition.
///
/// The one structural difference from the JS is that the condition is evaluated
/// even when probability has already silenced the trig, so `cond_result` is
/// well-defined for the history. Condition evaluation is pure, so `plays` comes
/// out identical to the JS's short-circuit either way.
pub fn should_play(
    prob: Option<u8>,
    fill: Option<bool>,
    cond: Option<&str>,
    track_prob: u8,
    ctx: &CondContext,
    rng: &mut dyn Rng,
) -> TrigOutcome {
    let effective_prob = prob.unwrap_or(track_prob);
    let prob_passes = rng.next_f64() * 100.0 < effective_prob as f64;

    // FILL is its own per-step lane on both boxes, not a COND menu entry, so it
    // is checked separately and never feeds the PRE/NEI history.
    let fill_passes = match fill {
        None => true,
        Some(want) => want == ctx.fill_active,
    };

    let cond_result = cond.filter(|c| !c.is_empty()).and_then(|c| {
        let parsed = Cond::parse(c)?;
        let raw = match parsed.kind {
            CondKind::First => Some(ctx.loop_index == 0),
            // `loop % b == a - 1`. `b == 0` is not a condition any box can
            // produce; the JS gets NaN and falls to false, so this does too.
            CondKind::Ratio { a, b } => {
                Some(b != 0 && a >= 1 && ctx.loop_index % b as u64 == (a - 1) as u64)
            }
            CondKind::Pre => ctx.prev_on_track,
            CondKind::Nei => ctx.prev_on_neighbour,
            CondKind::Last => ctx.last_pass,
        }?;
        Some(if parsed.negated { !raw } else { raw })
    });

    TrigOutcome {
        plays: prob_passes && fill_passes && cond_result.unwrap_or(true),
        cond_result,
    }
}

/// Per-device condition history: the last evaluated condition result on each
/// track, which is what `PRE` and `NEI` read.
///
/// One per device, never shared between boxes — `NEI` is a physical neighbour on
/// one machine. Sized from the device's track count at construction, so a
/// 4-track box gets four slots and not sixteen.
#[derive(Debug, Clone)]
pub struct CondHistory {
    last: Vec<Option<bool>>,
}

impl CondHistory {
    pub fn new(num_tracks: usize) -> Self {
        Self {
            last: vec![None; num_tracks],
        }
    }

    /// The context a trig on `track` evaluates against.
    ///
    /// Track 0 has no neighbour, so its `NEI` reads `None` and therefore plays —
    /// PLAN.md §4 states that case explicitly.
    pub fn context_for(&self, track: usize, loop_index: u64, fill_active: bool) -> CondContext {
        CondContext {
            loop_index,
            fill_active,
            prev_on_track: self.last.get(track).copied().flatten(),
            prev_on_neighbour: track
                .checked_sub(1)
                .and_then(|n| self.last.get(n).copied().flatten()),
            // Not history. The scheduler fills this in from the song row it is
            // on, because nothing in a per-device history could know.
            last_pass: None,
        }
    }

    /// Record what a trig's condition came out as. A `None` outcome — an
    /// unconditional trig, or one nothing could evaluate — leaves the slot alone
    /// rather than clearing it, so `PRE` keeps reading the last condition that
    /// genuinely resolved.
    pub fn record(&mut self, track: usize, outcome: TrigOutcome) {
        if let (Some(slot), Some(result)) = (self.last.get_mut(track), outcome.cond_result) {
            *slot = Some(result);
        }
    }

    /// Forget everything. Stop and a scene change both do this: carrying one
    /// pattern's `PRE` chain into the next one would make the first bar of a
    /// scene depend on what happened to be playing before it.
    pub fn clear(&mut self) {
        self.last.fill(None);
    }
}
