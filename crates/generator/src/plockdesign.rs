// P-lock lanes for a generated part.
//
// Port of `js/gen/plockdesign.js`. The one module here that knows a device
// exists — and it only ever **reads** `digi_protocol::params::writable_params_for`,
// the list of parameters whose paramId has been measured on real hardware.
// Nothing is encoded, no byte is chosen, and the lanes leave for the box
// through the same seam a hand-drawn lane does.
//
// Two rules the design leans on:
//
//   * **No resolvable box, no lanes.** A lane belongs to one box's parameter
//     numbering (74 is overdrive on a DT2 and filter frequency on a DN2), so
//     guessing a device kind would mean writing the wrong knob. The
//     generator returns nothing and says so in a warning instead.
//   * **Values only on steps that have trigs**, which is the v1 p-lock rule
//     anyway — so a lane leaves here already obeying it rather than being
//     scrubbed afterwards.
//
// Values sit on the parameter's **display axis** (MIDI 0–127), the axis the
// lane strip draws and the audition path sends; the uint16 only happens at
// the roll↔device seam. So nothing in this module knows about scaling
// either.

use digi_protocol::params::{writable_params_for, MIDI_MAX, MIDI_MIN};

use crate::genres::{LaneRecipe, LaneShape, RoleProfile};
use crate::rhythm::Trig;
use crate::rng::{range, Rng};

fn clamp_midi(v: f64) -> i32 {
    (v.round() as i32).clamp(MIDI_MIN, MIDI_MAX)
}

/// `v` held to 0..=1.
///
/// **`.max().min()` and not `clamp`, the last of the four sites that make this
/// choice** — and the one every lane shape passes through, so it is the one that
/// matters most. The two differ on `NaN`: this chain returns 0.0, a real value at
/// the bottom of the range, while `clamp` would hand `NaN` on to
/// `clamp_midi`, where `NaN.round() as i32` saturates to 0 and then clamps to
/// `MIDI_MIN` — the same answer by accident, through two casts nobody would look
/// at. `protocol::pattern::micro_steps_to_byte` has the full argument.
#[allow(clippy::manual_clamp)]
fn clamp01(v: f64) -> f64 {
    v.max(0.0).min(1.0)
}

/// What a [`LaneShape`] reads besides the phrase position `t` (0..1).
#[derive(Debug, Clone, Copy)]
pub struct ShapeCtx {
    pub step: u32,
    pub accent: bool,
    pub walk: f64,
}

/// The shapes a recipe can ask for, `(t, ctx) → 0..1` — so a shape is
/// written once and works at any pattern length.
///
///   Rise    opens across the phrase; the classic filter contour
///   Fall    the reverse — closes as the loop goes on
///   Arc     opens to the middle and closes again
///   Swell   flat, then lifts over the last quarter: a send building into the turnaround
///   Accent  high on the accented trigs, low on the rest — not a contour at all
///   Pulse   alternates high/low per beat, for LFO depth and pan movement
///   Wander  a random walk, the shape a hand on a knob actually makes
pub fn lane_shape(shape: LaneShape, t: f64, ctx: ShapeCtx) -> f64 {
    match shape {
        LaneShape::Rise => t,
        LaneShape::Fall => 1.0 - t,
        LaneShape::Accent => {
            if ctx.accent {
                1.0
            } else {
                0.15
            }
        }
        LaneShape::Swell => {
            if t < 0.75 {
                0.15 * (t / 0.75)
            } else {
                0.15 + 0.85 * ((t - 0.75) / 0.25)
            }
        }
        LaneShape::Arc => 1.0 - (2.0 * t - 1.0).abs(),
        LaneShape::Wander => ctx.walk,
        LaneShape::Pulse => {
            if (ctx.step / 4) % 2 == 0 {
                1.0
            } else {
                0.25
            }
        }
    }
}

/// Motion decides two things at once: how many of a role's recipes are
/// used, and how far each one travels. At 0 there are no lanes at all; at
/// 100 every recipe in the profile is drawn over its full range.
pub fn lanes_wanted(recipe_count: usize, motion: u32) -> usize {
    // **Clamped as an integer, and this is the one place in the file where the
    // `NaN` argument for `.min().max()` does not apply**: `motion` is a `u32`, so
    // `f64::from` cannot produce a `NaN` and cannot produce a negative — the
    // `.max(0.0)` this used to carry was dead on an unsigned value. Compare
    // `clamp01`, whose input is real `f64` arithmetic and which keeps the chain.
    let m = f64::from(motion.min(100));
    if m <= 0.0 {
        return 0;
    }
    ((recipe_count as f64 * (0.34 + 0.66 * m / 100.0)).round() as i64).max(1) as usize
}

/// One lane value: which step, and what it holds on the display axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneValue {
    pub step: u32,
    pub value: i32,
}

/// Lane values for one recipe over one part's trigs.
pub fn lane_values(recipe: &LaneRecipe, trigs: &[Trig], total: u32, motion: u32, rng: &mut Rng) -> Vec<LaneValue> {
    let from = clamp_midi(f64::from(recipe.from));
    let to = clamp_midi(f64::from(recipe.to));
    let centre = f64::from(from + to) / 2.0;
    // Integer clamp, for the reason `lanes_wanted` gives: `motion` is a `u32`.
    let depth = f64::from(motion.min(100)) / 100.0;

    // The random walk is one shared series across the lane, so `wander`
    // reads as one hand moving rather than as noise per step.
    let mut walk = 0.5;
    let mut values = Vec::with_capacity(trigs.len());
    for trig in trigs {
        walk = clamp01(walk + range(rng, -0.28, 0.28));
        let t = if total > 1 { f64::from(trig.step) / f64::from(total - 1) } else { 0.0 };
        let f = lane_shape(recipe.shape, t, ShapeCtx { step: trig.step, accent: trig.accent, walk });
        let full = f64::from(from) + f64::from(to - from) * clamp01(f);
        // Motion scales the movement about the middle of the recipe's
        // range, so a low Motion is a gentle version of the same gesture
        // rather than a different one.
        values.push(LaneValue { step: trig.step, value: clamp_midi(centre + (full - centre) * depth) });
    }
    values
}

/// One designed lane: a canonical parameter name, and its values sparse
/// over the pattern's own step count.
#[derive(Debug, Clone)]
pub struct DesignedLane {
    pub name: &'static str,
    pub device_kind: &'static str,
    pub values: Vec<Option<i32>>,
}

/// Lanes for one role.
///
///   `role`         the role profile (its `lanes` field is the recipe list)
///   `trigs`        the part's trig list — lanes only get values where there are trigs
///   `device_kind`  `"DT2"` / `"DN2"`, or `None` when no box could be resolved
///
/// Returns the lanes and any warnings — a lane is sparse over `steps` (128,
/// the full pattern memory a box's lane pool addresses), matching
/// [`digi_core::model::PLockLane`]'s own shape.
pub fn design_lanes(
    role: &RoleProfile,
    device_kind: Option<&'static str>,
    trigs: &[Trig],
    total: u32,
    motion: u32,
    steps: usize,
    rng: &mut Rng,
) -> (Vec<DesignedLane>, Vec<String>) {
    let mut warnings = Vec::new();
    let recipes = role.lanes;
    if recipes.is_empty() || trigs.is_empty() || motion == 0 {
        return (Vec::new(), warnings);
    }
    let Some(device_kind) = device_kind else {
        warnings.push(
            "no p-lock lanes — digi-roll can't tell which box this is for, and a lane belongs to \
             one box's parameter numbering. Pick your box in the MIDI output menu (or import a \
             track from it) and generate again."
                .to_string(),
        );
        return (Vec::new(), warnings);
    };

    let writable: std::collections::HashSet<&str> = writable_params_for(device_kind).iter().map(|p| p.name).collect();
    let usable: Vec<&LaneRecipe> = recipes.iter().filter(|r| writable.contains(r.name)).collect();
    if usable.is_empty() {
        warnings.push(format!(
            "no p-lock lanes — none of the {device_kind}'s measured parameters match this genre's recipe"
        ));
        return (Vec::new(), warnings);
    }

    let want = usable.len().min(lanes_wanted(usable.len(), motion));
    (build_lanes(&usable[..want], device_kind, trigs, total, steps, motion, rng), warnings)
}

fn build_lanes(
    recipes: &[&LaneRecipe],
    device_kind: &'static str,
    trigs: &[Trig],
    total: u32,
    steps: usize,
    motion: u32,
    rng: &mut Rng,
) -> Vec<DesignedLane> {
    let mut lanes = Vec::new();
    for recipe in recipes {
        let mut values: Vec<Option<i32>> = vec![None; steps];
        for v in lane_values(recipe, trigs, total, motion, rng) {
            if (v.step as usize) < steps {
                values[v.step as usize] = Some(v.value);
            }
        }
        if values.iter().all(Option::is_none) {
            continue;
        }
        lanes.push(DesignedLane { name: recipe.name, device_kind, values });
    }
    lanes
}

// --- The shared pool, across parts ---------------------------------------------
//
// One slot has one pool of lane records shared by every track in it — 80 on
// both boxes (`digi_protocol::pattern::Spec::num_p_locks`). `design_lanes`
// above picks a per-part lane count exactly the way `js/gen/plockdesign.js`
// does, with no awareness that anything else might want the same pool —
// which was safe for the JS's fixed three parts (nine lanes at most) and
// stops being safe once a generate can aim sixteen parts at one slot.
//
// PLAN.md Phase 7 Decision 1 raises the question without answering it: which
// part loses its lanes when the pool runs out, and how does the panel say
// so. What follows is *a* policy — row order, first served — chosen because
// it is the same order the busy-map threading already reads notes in, so
// "what answers what" and "who keeps their lanes" agree. It is a judgment
// call, not something Neil settled, and a caller is free to arbitrate
// differently; `design_lanes_capped` is the seam that lets it.

/// How many lanes one part would want, before anything else competes for the
/// pool — `usable.len().min(lanes_wanted(..))`, exposed so a caller can
/// arbitrate across every part before designing any of them.
pub fn wanted_lane_count(role: &RoleProfile, device_kind: Option<&str>, trigs: &[Trig], motion: u32) -> usize {
    if role.lanes.is_empty() || trigs.is_empty() || motion == 0 {
        return 0;
    }
    let Some(device_kind) = device_kind else { return 0 };
    let usable = writable_params_for(device_kind).iter().map(|p| p.name).collect::<std::collections::HashSet<_>>();
    let count = role.lanes.iter().filter(|r| usable.contains(r.name)).count();
    if count == 0 {
        return 0;
    }
    count.min(lanes_wanted(count, motion))
}

/// One part's claim on the shared pool, for [`arbitrate_pool`].
pub struct LaneClaim<Id> {
    pub id: Id,
    /// A name for the warning message — the destination or the role, not
    /// the internal id.
    pub label: String,
    pub wanted: usize,
}

/// What each part was granted, and what to tell the panel about it.
pub struct LaneBudget<Id> {
    pub granted: std::collections::HashMap<Id, usize>,
    pub warnings: Vec<String>,
}

/// Divide a slot's remaining pool capacity among every part's claim, in the
/// order given — first served, so a caller passing claims in row order gets
/// "earlier rows keep their lanes first", matching the row order the busy
/// map already threads through. A part that has to be cut gets a warning
/// naming it; a part that fits gets none.
pub fn arbitrate_pool<Id: std::hash::Hash + Eq>(claims: Vec<LaneClaim<Id>>, capacity: usize) -> LaneBudget<Id> {
    let mut remaining = capacity;
    let mut granted = std::collections::HashMap::with_capacity(claims.len());
    let mut warnings = Vec::new();
    for claim in claims {
        let give = claim.wanted.min(remaining);
        if give < claim.wanted {
            warnings.push(format!(
                "{}: only {give} of {} p-lock lanes fit the slot's shared pool of {capacity}",
                claim.label, claim.wanted
            ));
        }
        remaining -= give;
        granted.insert(claim.id, give);
    }
    LaneBudget { granted, warnings }
}

/// [`design_lanes`], with the lane count fixed by [`arbitrate_pool`] instead
/// of computed from Motion alone. `max_lanes` is a hard ceiling on top of
/// whatever Motion would have picked — the smaller of the two wins, so
/// arbitration can only ever take lanes away, never grant more than Motion
/// asked for.
pub fn design_lanes_capped(
    role: &RoleProfile,
    device_kind: Option<&'static str>,
    trigs: &[Trig],
    total: u32,
    motion: u32,
    steps: usize,
    max_lanes: usize,
    rng: &mut Rng,
) -> (Vec<DesignedLane>, Vec<String>) {
    let mut warnings = Vec::new();
    if role.lanes.is_empty() || trigs.is_empty() || motion == 0 || max_lanes == 0 {
        return (Vec::new(), warnings);
    }
    let Some(device_kind) = device_kind else {
        warnings.push(
            "no p-lock lanes — digi-roll can't tell which box this is for, and a lane belongs to \
             one box's parameter numbering. Pick your box in the MIDI output menu (or import a \
             track from it) and generate again."
                .to_string(),
        );
        return (Vec::new(), warnings);
    };
    let writable: std::collections::HashSet<&str> = writable_params_for(device_kind).iter().map(|p| p.name).collect();
    let usable: Vec<&LaneRecipe> = role.lanes.iter().filter(|r| writable.contains(r.name)).collect();
    if usable.is_empty() {
        warnings.push(format!(
            "no p-lock lanes — none of the {device_kind}'s measured parameters match this genre's recipe"
        ));
        return (Vec::new(), warnings);
    }
    let want = usable.len().min(lanes_wanted(usable.len(), motion)).min(max_lanes);
    (build_lanes(&usable[..want], device_kind, trigs, total, steps, motion, rng), warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genres::{genre_profile, role_profile, GenreId, Role};
    use digi_protocol::params::{param_table_for, DEVICE_KINDS};

    fn trigs_at(steps: &[u32]) -> Vec<Trig> {
        steps
            .iter()
            .map(|&step| Trig { step, bar: step / 16, weight: 1.0, accent: step % 4 == 0, ghost: false })
            .collect()
    }

    fn sample_trigs() -> Vec<Trig> {
        trigs_at(&[0, 3, 6, 8, 11, 14, 16, 20, 22, 27])
    }

    fn bass_role() -> RoleProfile {
        role_profile(GenreId::Dnb, Role::Bass)
    }

    #[test]
    fn makes_none_at_motion_0() {
        let (lanes, _) = design_lanes(&bass_role(), Some("DT2"), &sample_trigs(), 32, 0, 128, &mut Rng::new(1));
        assert!(lanes.is_empty());
    }

    #[test]
    fn makes_none_with_no_resolvable_box_and_says_why() {
        let (lanes, warnings) = design_lanes(&bass_role(), None, &sample_trigs(), 32, 100, 128, &mut Rng::new(2));
        assert!(lanes.is_empty());
        assert!(warnings.iter().any(|w| w.contains("can't tell which box")));
    }

    #[test]
    fn makes_none_for_a_part_with_no_trigs() {
        let (lanes, _) = design_lanes(&bass_role(), Some("DN2"), &[], 32, 100, 128, &mut Rng::new(3));
        assert!(lanes.is_empty());
    }

    #[test]
    fn is_none_at_0_and_everything_in_the_recipe_at_100() {
        assert_eq!(lanes_wanted(3, 0), 0);
        assert_eq!(lanes_wanted(3, 100), 3);
    }

    #[test]
    fn rises_with_motion_and_always_leaves_at_least_one_lane_once_on() {
        let mut last = 0;
        for motion in [1, 25, 50, 75, 100] {
            let n = lanes_wanted(3, motion);
            assert!(n >= last.max(1));
            last = n;
        }
    }

    #[test]
    fn only_ever_automates_a_parameter_measured_on_that_box() {
        for kind in DEVICE_KINDS {
            let writable: std::collections::HashSet<&str> = writable_params_for(kind).iter().map(|p| p.name).collect();
            for genre in GenreId::ALL {
                for role in Role::ALL {
                    let profile = role_profile(genre, role);
                    let (lanes, _) = design_lanes(&profile, Some(kind), &sample_trigs(), 32, 100, 128, &mut Rng::new(7));
                    for lane in &lanes {
                        assert!(writable.contains(lane.name), "{} on {kind}", lane.name);
                        assert_eq!(lane.device_kind, *kind);
                    }
                }
            }
        }
    }

    #[test]
    fn holds_values_only_on_steps_that_have_trigs() {
        for kind in DEVICE_KINDS {
            let trigs = sample_trigs();
            let live: std::collections::HashSet<u32> = trigs.iter().map(|t| t.step).collect();
            let (lanes, _) = design_lanes(&bass_role(), Some(kind), &trigs, 32, 80, 128, &mut Rng::new(8));
            // A kind with nothing writable — the A4, whose whole table is
            // audition-only until a paramId is measured — must design *no*
            // lanes rather than lanes the write seam then refuses.
            if writable_params_for(kind).is_empty() {
                assert!(lanes.is_empty(), "{kind} has nothing writable to design with");
                continue;
            }
            assert!(!lanes.is_empty());
            for lane in &lanes {
                assert_eq!(lane.values.len(), 128);
                for (step, v) in lane.values.iter().enumerate() {
                    if v.is_some() {
                        assert!(live.contains(&(step as u32)), "value on trigless step {step}");
                    }
                }
                assert_eq!(lane.values.iter().filter(|v| v.is_some()).count(), live.len());
            }
        }
    }

    #[test]
    fn stays_on_the_midi_display_axis() {
        for kind in DEVICE_KINDS {
            for motion in [10, 50, 100] {
                let (lanes, _) = design_lanes(&bass_role(), Some(kind), &sample_trigs(), 32, motion, 128, &mut Rng::new(9));
                for lane in &lanes {
                    for v in lane.values.iter().flatten() {
                        assert!(*v >= MIDI_MIN && *v <= MIDI_MAX);
                    }
                }
            }
        }
    }

    #[test]
    fn all_shapes_stay_inside_0_1_across_a_whole_pattern() {
        for shape in [
            LaneShape::Rise,
            LaneShape::Fall,
            LaneShape::Accent,
            LaneShape::Swell,
            LaneShape::Arc,
            LaneShape::Wander,
            LaneShape::Pulse,
        ] {
            for step in 0..32u32 {
                let v = lane_shape(shape, f64::from(step) / 31.0, ShapeCtx { step, accent: step % 4 == 0, walk: 0.5 });
                assert!((0.0..=1.0).contains(&v), "{shape:?} at step {step} gave {v}");
            }
        }
    }

    fn recipe(shape: LaneShape, from: u8, to: u8) -> LaneRecipe {
        LaneRecipe { name: "filter.cutoff", shape, from, to }
    }

    #[test]
    fn rise_really_does_open_across_the_pattern() {
        let values = lane_values(&recipe(LaneShape::Rise, 20, 100), &sample_trigs(), 32, 100, &mut Rng::new(11));
        assert!(values.last().unwrap().value > values[0].value);
    }

    #[test]
    fn fall_closes_across_it() {
        let values = lane_values(&recipe(LaneShape::Fall, 20, 100), &sample_trigs(), 32, 100, &mut Rng::new(12));
        assert!(values.last().unwrap().value < values[0].value);
    }

    #[test]
    fn accent_puts_the_high_value_on_the_accented_trigs_and_nowhere_else() {
        let values = lane_values(&recipe(LaneShape::Accent, 10, 90), &sample_trigs(), 32, 100, &mut Rng::new(13));
        let trigs = sample_trigs();
        let on: Vec<i32> = values.iter().filter(|v| trigs.iter().find(|t| t.step == v.step).unwrap().accent).map(|v| v.value).collect();
        let off: Vec<i32> =
            values.iter().filter(|v| !trigs.iter().find(|t| t.step == v.step).unwrap().accent).map(|v| v.value).collect();
        assert!(on.iter().min().unwrap() > off.iter().max().unwrap());
    }

    #[test]
    fn swell_saves_its_lift_for_the_end_of_the_loop() {
        let values = lane_values(&recipe(LaneShape::Swell, 20, 90), &sample_trigs(), 32, 100, &mut Rng::new(14));
        let early: Vec<i32> = values.iter().filter(|v| v.step < 24).map(|v| v.value).collect();
        assert!(values.last().unwrap().value > *early.iter().max().unwrap());
    }

    #[test]
    fn a_lower_motion_is_the_same_gesture_gentler() {
        let spread = |motion: u32| -> i32 {
            let values = lane_values(&recipe(LaneShape::Rise, 20, 100), &sample_trigs(), 32, motion, &mut Rng::new(15));
            values.iter().map(|v| v.value).max().unwrap() - values.iter().map(|v| v.value).min().unwrap()
        };
        assert!(spread(100) > spread(40));
        assert!(spread(40) > spread(10));
    }

    #[test]
    fn is_deterministic_for_a_seed_wander_included() {
        let once = |seed: u32| -> Vec<i32> {
            lane_values(&recipe(LaneShape::Wander, 30, 90), &sample_trigs(), 32, 100, &mut Rng::new(seed))
                .iter()
                .map(|v| v.value)
                .collect()
        };
        assert_eq!(once(16), once(16));
        assert_ne!(once(16), once(17));
    }

    #[test]
    fn produces_lanes_the_existing_write_seam_would_accept() {
        // The safety story: values stay on the display axis (0..=127) and
        // every lane names a parameter the box actually has — the two
        // things `rollPLocksToDevice`'s Rust equivalent
        // (`digi_protocol::params`) would refuse otherwise.
        for kind in DEVICE_KINDS {
            let (lanes, _) = design_lanes(&bass_role(), Some(kind), &sample_trigs(), 32, 100, 128, &mut Rng::new(10));
            // Same rule as above: the write seam accepts nothing for a kind
            // with no measured paramIds, and the honest design is no lanes.
            if writable_params_for(kind).is_empty() {
                assert!(lanes.is_empty(), "{kind} has nothing writable to design with");
                continue;
            }
            assert!(!lanes.is_empty());
            for lane in &lanes {
                let table = param_table_for(kind);
                assert!(table.iter().any(|p| p.name == lane.name));
                for v in lane.values.iter().flatten() {
                    assert!((0..=127).contains(v));
                }
            }
        }
    }

    #[test]
    fn arbitrate_pool_grants_everyone_when_the_pool_is_not_exhausted() {
        let claims = vec![
            LaneClaim { id: 1, label: "bass".into(), wanted: 3 },
            LaneClaim { id: 2, label: "chords".into(), wanted: 2 },
            LaneClaim { id: 3, label: "lead".into(), wanted: 2 },
        ];
        let budget = arbitrate_pool(claims, 80);
        assert_eq!(budget.granted[&1], 3);
        assert_eq!(budget.granted[&2], 2);
        assert_eq!(budget.granted[&3], 2);
        assert!(budget.warnings.is_empty());
    }

    #[test]
    fn arbitrate_pool_cuts_later_claims_first_and_says_so() {
        let claims = vec![
            LaneClaim { id: 1, label: "row 1".into(), wanted: 5 },
            LaneClaim { id: 2, label: "row 2".into(), wanted: 5 },
            LaneClaim { id: 3, label: "row 3".into(), wanted: 5 },
        ];
        let budget = arbitrate_pool(claims, 8);
        assert_eq!(budget.granted[&1], 5);
        assert_eq!(budget.granted[&2], 3);
        assert_eq!(budget.granted[&3], 0);
        assert_eq!(budget.warnings.len(), 2);
        assert!(budget.warnings[0].contains("row 2"));
        assert!(budget.warnings[1].contains("row 3"));
    }

    #[test]
    fn design_lanes_capped_never_exceeds_the_cap_even_when_motion_asks_for_more() {
        for cap in [0usize, 1, 2] {
            let (lanes, _) =
                design_lanes_capped(&bass_role(), Some("DT2"), &sample_trigs(), 32, 100, 128, cap, &mut Rng::new(20));
            assert!(lanes.len() <= cap);
        }
    }

    #[test]
    fn design_lanes_capped_never_grants_more_than_motion_would_have() {
        let uncapped = design_lanes(&bass_role(), Some("DT2"), &sample_trigs(), 32, 30, 128, &mut Rng::new(21)).0.len();
        let (capped, _) =
            design_lanes_capped(&bass_role(), Some("DT2"), &sample_trigs(), 32, 30, 128, 99, &mut Rng::new(21));
        assert_eq!(capped.len(), uncapped);
    }

    #[test]
    fn wanted_lane_count_matches_what_design_lanes_actually_produces() {
        for kind in DEVICE_KINDS {
            for motion in [0u32, 30, 100] {
                let trigs = sample_trigs();
                let want = wanted_lane_count(&bass_role(), Some(kind), &trigs, motion);
                let (lanes, _) = design_lanes(&bass_role(), Some(kind), &trigs, 32, motion, 128, &mut Rng::new(22));
                // A lane recipe that lands on all-`None` values is dropped
                // after the fact, so `wanted` is an upper bound, not always
                // an exact count.
                assert!(lanes.len() <= want);
            }
        }
    }

    #[test]
    fn every_genres_recipe_names_only_parameters_both_boxes_actually_have() {
        // A typo in genres.rs would otherwise mean a lane that silently
        // never appears.
        for genre in GenreId::ALL {
            for role in Role::ALL {
                let profile = role_profile(genre, role);
                for recipe in profile.lanes {
                    for kind in DEVICE_KINDS {
                        let known = param_table_for(kind).iter().any(|p| p.name == recipe.name);
                        assert!(known, "{genre:?}/{role:?}: {} on {kind}", recipe.name);
                    }
                }
            }
        }
        let _ = genre_profile(GenreId::Dnb);
    }
}
