// Stage 2 of the MIDI import design — **fit**, MIDI_IMPORT_DESIGN.md §4.
//
// Pure functions from `(Score, Mapping, Options, &Session)` to `ImportPlan`.
// Nothing here writes to a session; `apply` (§5.6, Phase D) does. The score
// comes from `midifile::score` (Stage 1), which answers what the file holds;
// this module decides where it lands: segments, dedupe, slots, drum fan-out,
// polyphony, timing. The section citations below are all §4.x of the design.

use std::collections::BTreeMap;

use digi_protocol::pattern::steps_to_length_byte;

use crate::device::{DeviceId, DeviceModel};
use crate::edit_ops::{clamp_micro, clamp_velocity};
use crate::lengths::snap_len_fine;
use crate::midifile::score::{bar_starts, steps_per_bar, Meter, RawNote, Score};
use crate::model::{Note, Pattern, TrackScale};
use crate::session::{PatternRef, Session};
use crate::song::MAX_ROWS;

/// Every drum note on a sample track is stamped at this pitch. The
/// destination track already says which drum it is — a DT2 track holds one
/// sample, so its pitch does not choose *which* sound plays — so the value
/// only needs to be a legal, unremarkable MIDI note. 60 is C5, this app's own
/// convention for "the middle of the keyboard" (`chords`'s octave labelling
/// agrees: MIDI 60 is C5 here, not C4).
///
/// Relocated from `digi_generator::parts::drums` per §4.5/§6 so the import
/// and the generator read one number rather than agreeing by coincidence; the
/// generator re-exports it.
pub const DRUM_TRIGGER_PITCH: u8 = 60;

/// The GM percussion names a drum fan-out labels its destination tracks with
/// (§4.5). Anything outside the map is `"Perc {p}"`. The Phase D dialog needs
/// this too, which is why it is public rather than a fitter internal.
pub fn gm_drum_name(pitch: u8) -> String {
    let name = match pitch {
        36 => "Kick",
        38 => "Snare",
        42 => "Closed Hat",
        46 => "Open Hat",
        39 => "Clap",
        37 => "Rimshot",
        41 | 43 | 45 | 47 | 48 | 50 => "Tom",
        49 | 57 => "Crash",
        51 | 59 => "Ride",
        56 => "Cowbell",
        54 => "Tambourine",
        69 | 70 => "Shaker",
        75 => "Clave",
        _ => return format!("Perc {pitch}"),
    };
    name.to_string()
}

/// What the fit is allowed to do (§4.1). The file stem the patterns are named
/// after is a parameter of [`fit`] rather than a field here: it is a fact
/// about the file, like the `Score`, not a choice the dialog toggles.
#[derive(Debug, Clone)]
pub struct Options {
    /// Bars per pattern: 1, 2, 4 or 8. [`default_pattern_bars`] computes the
    /// default; the dialog shows it and lets the user lower it (§4.2).
    pub pattern_bars: u8,
    /// Start at the bar containing the first note of any mapped part rather
    /// than at tick 0 (§4.2). Default true.
    pub trim_leading_silence: bool,
    /// Where each destination box starts filling, in slot order. A box absent
    /// here starts at its first blank slot — or at slot 0 with
    /// `overwrite_occupied`, where blank-only is no longer the rule (§4.4).
    pub start_slot: BTreeMap<DeviceId, PatternRef>,
    /// Take occupied slots too, listing what they held in
    /// `ImportReport::replacing` (§4.4). Default false.
    pub overwrite_occupied: bool,
    /// Offer the file's first tempo as the session tempo. The dialog defaults
    /// this to true only when no track in the session has a note (§4.1).
    pub apply_tempo: bool,
    /// Label song rows from the file's markers (§4.8). Default true.
    pub label_rows_from_markers: bool,
}

/// One entry per `score.parts`, same order (§4.1).
#[derive(Debug, Clone)]
pub struct Mapping {
    pub parts: Vec<PartMapping>,
}

#[derive(Debug, Clone)]
pub enum PartMapping {
    Skip,
    Track {
        device: DeviceId,
        track: usize,
        overflow: Overflow,
        scale: TrackScale,
    },
    /// A drum part fanned out: one destination track per pitch (§4.5).
    Drums {
        device: DeviceId,
        tracks: Vec<(u8 /*pitch*/, usize /*track*/)>,
    },
}

/// What a step holding more notes than `DeviceModel::notes_per_trig` does
/// with the excess (§4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    KeepLowest,
    KeepHighest,
    SplitTo { device: DeviceId, track: usize },
}

/// One scene the plan will add: a name and a slot per destination box (§4.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedScene {
    pub name: String,
    pub slots: BTreeMap<DeviceId, PatternRef>,
}

/// What the fit decided, before anything is written (§4.8).
#[derive(Debug, Clone)]
pub struct ImportPlan {
    /// The unique patterns to write, `(device, slot, pattern)`.
    pub patterns: Vec<(DeviceId, PatternRef, Pattern)>,
    pub scenes: Vec<PlannedScene>,
    /// `(scene index into `scenes`, label, repeats)`.
    pub rows: Vec<(usize, String, u16)>,
    /// The file's first tempo, offered; applied only when
    /// `Options::apply_tempo` (§5.5).
    pub tempo_bpm: Option<f64>,
    pub report: ImportReport,
}

/// The honest account of what fitting cost (§4.8). Every dropped or altered
/// thing is counted here rather than swallowed, because the dialog shows this
/// and a silent drop is indistinguishable from a bug.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub parts_mapped: usize,
    pub parts_skipped: usize,
    pub notes_placed: usize,
    pub segments: usize,
    pub unique_patterns: BTreeMap<DeviceId, usize>,
    pub rows: usize,
    /// Notes whose `off` crossed a segment end and were clamped at it (§4.2).
    pub clamped_at_boundary: usize,
    /// Notes dropped over the trig's note cap (§4.6).
    pub notes_over_polyphony: usize,
    /// Drum pitches with no destination track (§4.5).
    pub drum_pitches_dropped: usize,
    /// Same-step same-pitch drum hits; the later one is kept (§4.5).
    pub same_step_duplicates: usize,
    pub tempo_changes_dropped: usize,
    pub pitch_bend_dropped: usize,
    /// Until the CC→p-lock work (§7) lands, every CC event of a mapped part.
    pub cc_dropped: usize,
    /// Set when any meter's steps-per-bar rounded up (§3.3).
    pub meter_approximated: bool,
    /// `(device, slot, old name, old note count)` for every occupied slot
    /// `overwrite_occupied` took (§4.4).
    pub replacing: Vec<(DeviceId, PatternRef, String, usize)>,
}

/// Why a plan was refused. All three are detected **before anything is
/// placed** — partial imports are not offered (§4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    NoFreeSlots {
        device: DeviceId,
        needed: usize,
        free: usize,
    },
    TooManyRows {
        needed: usize,
    },
    NotEnoughTracks {
        device: DeviceId,
        needed: usize,
        free: usize,
    },
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFreeSlots {
                device,
                needed,
                free,
            } => write!(
                f,
                "device {device:?} needs {needed} pattern slots but only {free} are free"
            ),
            Self::TooManyRows { needed } => {
                write!(f, "the import needs {needed} song rows; the song holds {MAX_ROWS}")
            }
            Self::NotEnoughTracks {
                device,
                needed,
                free,
            } => write!(
                f,
                "device {device:?} needs {needed} tracks for the drum fan-out but only {free} are free"
            ),
        }
    }
}

impl std::error::Error for PlanError {}

/// The largest of 8, 4, 2, 1 with `pattern_bars × max steps_per_bar ≤ min
/// max_steps` over the destination boxes (§4.2). A DT2 alone in 4/4 defaults
/// to 8 bars; a desk with an A4 (64 steps) defaults to 4; 3/4 on a digi still
/// gets 8 (96 steps).
pub fn default_pattern_bars(score: &Score, destinations: &[&DeviceModel]) -> u8 {
    let max_bar = bar_starts(score)
        .iter()
        .map(|&(_, m)| steps_per_bar(m))
        .fold(16.0_f64, f64::max);
    let min_steps = destinations
        .iter()
        .map(|m| m.max_steps)
        .min()
        .unwrap_or(128);
    for bars in [8u8, 4, 2, 1] {
        if f64::from(bars) * max_bar <= f64::from(min_steps) {
            return bars;
        }
    }
    1
}

/// What [`apply_import`] created (§5.6 step 5), handed back so the panel can
/// select the first imported scene and say in the console what just happened
/// without re-deriving any of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedImport {
    /// Index into `Session::scenes` of the first scene the import added.
    pub first_scene: usize,
    /// Index into the song's rows of the first row the import added.
    pub first_row: usize,
    pub scenes: usize,
    pub rows: usize,
    pub patterns: usize,
}

/// Write a plan onto the session (§5.6). One history step, `ui::generate`'s
/// `apply_plan` style: the **caller** brackets this in
/// `history.begin(Content::of(session))` … `history.commit(session)`; this
/// function only writes ordinary session state, in the design's order —
///
/// 1. every planned pattern into its slot, replacing the `Arc<Pattern>` whole
///    (never editing the pattern that was there, so an occupied slot's undo is
///    one pointer swap);
/// 2. the scenes, each copied from the scene that is sounding so boxes the
///    import does not touch keep their slot rather than snapping to A01;
/// 3. the rows, with the plan's labels and repeats — `length_steps` stays
///    `None`, the row inheriting its scene's cycle;
/// 4. the tempo, when the plan carries one **and** `Options::apply_tempo` was
///    set at plan-build time. `fit` is deliberately not told the option — it
///    always offers `tempo_bpm`, because the dialog's checkbox is re-fitted
///    live and a plan that dropped the fact could not answer a toggle — so
///    the flag is re-taken here, where it is acted on.
///
/// `current_scene` is left where it was: selecting the first imported scene
/// is the panel's job, done through the engine like every other scene pick.
///
/// The tempo is a plain field write, which is the same path the transport
/// bar's tempo edit takes for the session half — `history::Content` does not
/// cover it and nothing here pretends otherwise. The engine half (the
/// `SetTempo` command) is also the caller's: `apply_import` never talks to
/// the engine, per the design's "nothing in this feature sends a byte to a
/// box".
pub fn apply_import(session: &mut Session, plan: ImportPlan, apply_tempo: bool) -> AppliedImport {
    let mut applied = AppliedImport {
        first_scene: session.scenes.len(),
        first_row: session.song().map_or(0, |s| s.rows.len()),
        scenes: 0,
        rows: 0,
        patterns: 0,
    };

    // 1. The patterns. `pattern_mut` would copy-on-write into the slot's own
    //    `Arc`; assigning the slot whole is cheaper and says the truer thing —
    //    the slot's old content is gone, not edited.
    for (device, slot, pattern) in plan.patterns {
        if let Some(device) = session.device_mut(device) {
            if let Some(cell) = device.patterns.get_mut(slot.slot()) {
                *cell = std::sync::Arc::new(pattern);
                applied.patterns += 1;
            }
        }
    }

    // 2. The scenes. `add_scene(name, Some(current))` first, then the slots —
    //    a destination box not yet in the session (removed since the plan was
    //    built) is skipped by `set_slot_in_scene` rather than invented.
    for planned in plan.scenes {
        let index = session.add_scene(planned.name, Some(session.current_scene));
        for (device, slot) in planned.slots {
            session.set_slot_in_scene(index, device, slot);
        }
        applied.scenes += 1;
    }

    // 3. The rows, in plan order. `add_song_row` can refuse on a full song;
    //    `fit` already refused plans over MAX_ROWS, so a refusal here would
    //    mean rows added between plan and apply — the plan is one gesture,
    //    but the check costs nothing and a dropped row must not silently
    //    lose its label.
    for (scene, label, repeats) in plan.rows {
        let Some(index) = session.add_song_row(applied.first_scene + scene) else {
            break;
        };
        if let Some(row) = session.song_mut().row_mut(index) {
            row.label = label;
            row.repeats = repeats;
        }
        applied.rows += 1;
    }

    // 4. The tempo, last, so the one non-history field in the session moves
    //    only when the music has actually landed.
    if apply_tempo {
        if let Some(bpm) = plan.tempo_bpm {
            session.tempo_bpm = bpm;
        }
    }

    applied
}

/// One segment of the timeline: `[start, end)` in ticks, and its length in
/// 16th steps. The last segment is trimmed to whole bars of actual content
/// rather than padded to `pattern_bars` (§4.2).
#[derive(Debug, Clone, Copy)]
struct Segment {
    start: u64,
    end: u64,
    steps: u16,
}

/// Cut the score into segments (§4.2): every `pattern_bars` bars **and** at
/// every meter change, from `origin_tick` to the bar containing `end_tick`.
fn segments_of(score: &Score, pattern_bars: u8, origin_tick: u64) -> Vec<Segment> {
    let per16 = f64::from(score.division) / 4.0;
    let bars = bar_starts(score);
    // `bar_starts` emits the bar *containing* `end_tick` but nothing past
    // it, so the last bar's own end is computed from its meter — rounded up
    // to a whole bar of actual content, never padded to `pattern_bars`
    // (§4.2). A bar start exactly at `end_tick` is an empty bar and dropped.
    let mut from: Vec<(u64, Meter)> = bars
        .iter()
        .copied()
        .filter(|&(t, _)| t >= origin_tick && t < score.end_tick)
        .collect();
    if from.is_empty() {
        from.push((
            origin_tick,
            score
                .meters
                .first()
                .map(|&(_, m)| m)
                .unwrap_or(Meter { num: 4, den: 4 }),
        ));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < from.len() {
        let start = from[i].0.max(origin_tick);
        // Take up to `pattern_bars` bars; a meter change is always a bar
        // start in `bar_starts`, so a change within the window ends the
        // segment there.
        let mut end_bar = i;
        while end_bar + 1 < from.len()
            && end_bar + 1 - i < pattern_bars as usize
            && from[end_bar + 1].1 == from[i].1
        {
            end_bar += 1;
        }
        let end = if end_bar + 1 < from.len() {
            from[end_bar + 1].0
        } else {
            let bar_ticks = (steps_per_bar(from[end_bar].1) * per16).max(1.0);
            let content = score.end_tick.saturating_sub(from[end_bar].0) as f64;
            let whole_bars = (content / bar_ticks).ceil().max(1.0);
            from[end_bar].0 + (whole_bars * bar_ticks) as u64
        };
        let end = end.max(start);
        let steps = ((end - start) as f64 / per16).ceil() as u16;
        out.push(Segment {
            start,
            end,
            steps: steps.max(1),
        });
        i = end_bar + 1;
    }
    out
}

/// The canonical form of one (segment, box) pattern (§4.3): every track's
/// notes sorted by `(step, pitch)` as `(step, micro×384, pitch, length byte,
/// velocity)`, plus `length_steps` per track. Ids excluded — the same trick
/// as `ui::generate::music_of`. Compared by equality in a `BTreeMap`; hashing
/// is only an index.
type Canonical = Vec<(u16, Vec<(u16, i16, u8, u8, u8)>)>;

fn canonical_of(pattern: &Pattern) -> Canonical {
    pattern
        .tracks()
        .iter()
        .map(|t| {
            let mut notes: Vec<(u16, i16, u8, u8, u8)> = t
                .notes
                .iter()
                .map(|n| {
                    (
                        n.step as u16,
                        (n.micro * 384.0) as i16,
                        n.pitch,
                        steps_to_length_byte(n.len),
                        n.velocity,
                    )
                })
                .collect();
            notes.sort_by_key(|&(step, _, pitch, _, _)| (step, pitch));
            (t.length_steps, notes)
        })
        .collect()
}

/// A note mid-fit, before it becomes a `model::Note`: the step is exact
/// (fractional), the micro split happens at placement.
#[derive(Debug, Clone, Copy)]
struct FitNote {
    step: f64,
    pitch: u8,
    len: f64,
    velocity: u8,
}

/// Where one mapped part's notes go for one segment: destination track index
/// → notes. Built per (segment, part), then merged per track (§5.4) before
/// the polyphony pass.
struct PlacedPart {
    /// `(track, notes)` — for `Drums`, one entry per mapped pitch.
    tracks: Vec<(usize, Vec<FitNote>)>,
    /// For `Track` mappings: the overflow policy and the split target.
    overflow: Option<Overflow>,
    scale: TrackScale,
}

/// Fit one score into a session's free slots (§4). Pure: the session is read
/// for its models, its blank slots and nothing else; the answer is a plan.
///
/// `file_stem` names the patterns (`"{file_stem} {n}"`, §4.2). It is a
/// parameter rather than an `Options` field because it is a fact about the
/// file, not a choice.
pub fn fit(
    score: &Score,
    mapping: &Mapping,
    options: &Options,
    file_stem: &str,
    session: &Session,
) -> Result<ImportPlan, PlanError> {
    let mut report = ImportReport::default();

    // ---- §4.8 report facts that cost nothing to collect up front ----
    report.tempo_changes_dropped = score.tempo.len().saturating_sub(1);
    let tempo_bpm = score.tempo.first().map(|&(_, bpm)| bpm);
    let per16 = f64::from(score.division) / 4.0;
    report.meter_approximated = score
        .meters
        .iter()
        .any(|&(_, m)| (f64::from(m.num) * 16.0 / f64::from(m.den).max(1.0)).fract() != 0.0);

    // ---- destination boxes, in device order ----
    let mut destinations: Vec<DeviceId> = Vec::new();
    for (i, part_mapping) in mapping.parts.iter().enumerate() {
        let part = &score.parts[i];
        match part_mapping {
            PartMapping::Skip => report.parts_skipped += 1,
            PartMapping::Track { device, .. } | PartMapping::Drums { device, .. } => {
                report.parts_mapped += 1;
                report.cc_dropped += part.cc.values().sum::<usize>();
                report.pitch_bend_dropped += part.pitch_bend_events;
                if !destinations.contains(device) {
                    destinations.push(*device);
                }
            }
        }
    }
    destinations.sort();

    // ---- §4.4: track-count refusal, before anything is placed ----
    for part_mapping in &mapping.parts {
        if let PartMapping::Drums { device, tracks } = part_mapping {
            let model = model_of(session, *device)?;
            let used: std::collections::BTreeSet<usize> = tracks.iter().map(|&(_, t)| t).collect();
            if used.len() > model.num_tracks {
                return Err(PlanError::NotEnoughTracks {
                    device: *device,
                    needed: used.len(),
                    free: model.num_tracks,
                });
            }
        }
    }

    // ---- §4.2: origin and segments ----
    let origin_tick = if options.trim_leading_silence {
        let bars = bar_starts(score);
        let first_on = mapping
            .parts
            .iter()
            .zip(&score.parts)
            .filter(|(m, _)| !matches!(m, PartMapping::Skip))
            .filter_map(|(_, p)| p.notes.first().map(|n| n.on))
            .min();
        match first_on {
            Some(on) => bars
                .iter()
                .take_while(|&&(t, _)| t <= on)
                .last()
                .map(|&(t, _)| t)
                .unwrap_or(0),
            None => 0,
        }
    } else {
        0
    };
    let segments = segments_of(score, options.pattern_bars.max(1), origin_tick);
    report.segments = segments.len();

    // ---- §4.2/§4.3: build every (segment, box) pattern, dedupe by canonical
    // form, allocate slots in first-appearance order ----
    //
    // `canon_slots[device]` maps canonical form → slot; `next_slot[device]` is
    // the fill-forward cursor. The blank pattern for all-empty segments is
    // just the canonical form of a pattern with no notes — it dedupes like
    // everything else, which is what "share one blank pattern per box" means.
    let mut canon_slots: BTreeMap<DeviceId, BTreeMap<Canonical, PatternRef>> = BTreeMap::new();
    let mut patterns: Vec<(DeviceId, PatternRef, Pattern)> = Vec::new();
    let mut cursors: BTreeMap<DeviceId, usize> = BTreeMap::new();
    for &device in &destinations {
        let start = options
            .start_slot
            .get(&device)
            .map(|r| r.slot())
            .unwrap_or_else(|| {
                if options.overwrite_occupied {
                    0
                } else {
                    first_blank_slot(session, device).unwrap_or(0)
                }
            });
        cursors.insert(device, start);
    }

    // scene tuple per segment, then §4.3's collapse/reuse pass.
    let mut segment_scenes: Vec<BTreeMap<DeviceId, PatternRef>> = Vec::new();

    for (seg_i, segment) in segments.iter().enumerate() {
        let mut scene_slots: BTreeMap<DeviceId, PatternRef> = BTreeMap::new();
        for &device in &destinations {
            let model = model_of(session, device)?;
            let built = build_segment_pattern(
                score,
                mapping,
                device,
                model,
                segment,
                seg_i,
                file_stem,
                per16,
                &mut report,
            );
            let canon = canonical_of(&built);
            let slot = match canon_slots.entry(device).or_default().entry(canon.clone()) {
                std::collections::btree_map::Entry::Occupied(e) => *e.get(),
                std::collections::btree_map::Entry::Vacant(e) => {
                    let slot = take_slot(
                        session,
                        device,
                        cursors.get_mut(&device).expect("cursor seeded"),
                        options.overwrite_occupied,
                        &mut report,
                    )?;
                    e.insert(slot);
                    patterns.push((device, slot, built));
                    slot
                }
            };
            scene_slots.insert(device, slot);
        }
        segment_scenes.push(scene_slots);
    }

    for &device in &destinations {
        report
            .unique_patterns
            .insert(device, canon_slots.get(&device).map_or(0, |m| m.len()));
    }

    // ---- §4.3: scenes and rows. Identical consecutive scenes collapse into
    // one row with repeats+1; identical non-consecutive scenes reuse the
    // scene. All-empty segments still occupy a row. ----
    let mut scenes: Vec<PlannedScene> = Vec::new();
    // Keyed by `(device, slot number)` rather than `PatternRef`, which
    // carries no `Ord` — the slot number is the same identity.
    let mut scene_index: BTreeMap<Vec<(DeviceId, usize)>, usize> = BTreeMap::new();
    let mut rows: Vec<(usize, String, u16)> = Vec::new();
    let mut last_label: Option<String> = None;

    for (seg_i, slots) in segment_scenes.iter().enumerate() {
        let key: Vec<(DeviceId, usize)> = slots.iter().map(|(&d, &r)| (d, r.slot())).collect();
        let idx = *scene_index.entry(key).or_insert_with(|| {
            scenes.push(PlannedScene {
                name: format!("{file_stem} {}", scenes.len() + 1),
                slots: slots.clone(),
            });
            scenes.len() - 1
        });
        // Consecutive identical scenes collapse; the first label is kept.
        if let Some(last) = rows.last_mut() {
            if last.0 == idx {
                last.2 += 1;
                continue;
            }
        }
        let label = row_label(score, options, segments[seg_i].start, seg_i, &last_label);
        last_label = Some(label.clone());
        rows.push((idx, label, 1));
    }

    if rows.len() > MAX_ROWS {
        return Err(PlanError::TooManyRows { needed: rows.len() });
    }
    report.rows = rows.len();

    Ok(ImportPlan {
        patterns,
        scenes,
        rows,
        tempo_bpm,
        report,
    })
}

/// The label a row gets (§4.8): the marker at the segment's start tick, else
/// the previous row's label, else `"Row {n}"` (1-based, as the box counts).
fn row_label(
    score: &Score,
    options: &Options,
    seg_start: u64,
    seg_i: usize,
    last_label: &Option<String>,
) -> String {
    if options.label_rows_from_markers {
        if let Some((_, text)) = score.markers.iter().find(|&&(t, _)| t == seg_start) {
            return text.clone();
        }
        if let Some(prev) = last_label {
            return prev.clone();
        }
    }
    format!("Row {}", seg_i + 1)
}

fn model_of(session: &Session, device: DeviceId) -> Result<&'static DeviceModel, PlanError> {
    session
        .device(device)
        .map(|d| d.model)
        .ok_or(PlanError::NoFreeSlots {
            device,
            needed: 1,
            free: 0,
        })
}

/// The first blank slot on a device, in slot order (§4.4).
fn first_blank_slot(session: &Session, device: DeviceId) -> Option<usize> {
    let device = session.device(device)?;
    device.patterns.iter().position(|p| p.is_blank())
}

/// Take the next slot from the cursor: blanks only, or every slot when
/// `overwrite_occupied` (recording what it replaces). Advances the cursor
/// past what it took (§4.4).
fn take_slot(
    session: &Session,
    device: DeviceId,
    cursor: &mut usize,
    overwrite_occupied: bool,
    report: &mut ImportReport,
) -> Result<PatternRef, PlanError> {
    let dev = session.device(device).ok_or(PlanError::NoFreeSlots {
        device,
        needed: 1,
        free: 0,
    })?;
    let total = dev.patterns.len();
    let mut scan = *cursor;
    while scan < total {
        let blank = dev.patterns[scan].is_blank();
        if blank || overwrite_occupied {
            if !blank {
                let old = &dev.patterns[scan];
                let old_notes: usize = old.tracks().iter().map(|t| t.notes.len()).sum();
                report.replacing.push((
                    device,
                    PatternRef::from_slot(scan),
                    old.name.clone(),
                    old_notes,
                ));
            }
            *cursor = scan + 1;
            return Ok(PatternRef::from_slot(scan));
        }
        scan += 1;
    }
    let free = dev
        .patterns
        .iter()
        .skip(*cursor)
        .filter(|p| p.is_blank())
        .count();
    Err(PlanError::NoFreeSlots {
        device,
        needed: 1,
        free,
    })
}

/// Build one (segment, box) pattern (§4.2): fresh from the model's blank
/// constructor, named `"{file_stem} {n}"` (n = segment number, 1-based),
/// swing 50, every track at the segment's length. Then place every mapped
/// part aimed at this box.
#[allow(clippy::too_many_arguments)]
fn build_segment_pattern(
    score: &Score,
    mapping: &Mapping,
    device: DeviceId,
    model: &'static DeviceModel,
    segment: &Segment,
    seg_i: usize,
    file_stem: &str,
    per16: f64,
    report: &mut ImportReport,
) -> Pattern {
    let mut pattern = Pattern::for_model(model);
    pattern.name = format!("{file_stem} {}", seg_i + 1);
    pattern.swing = 50;
    for i in 0..pattern.num_tracks() {
        if let Some(t) = pattern.track_mut(i) {
            t.length_steps = segment.steps;
        }
    }

    // Place each mapped part aimed at this box, merging parts that share a
    // destination track before the polyphony pass (§5.4).
    let mut by_track: BTreeMap<usize, Vec<FitNote>> = BTreeMap::new();
    let mut overflows: BTreeMap<usize, Overflow> = BTreeMap::new();
    let mut scales: BTreeMap<usize, TrackScale> = BTreeMap::new();

    for (part, part_mapping) in score.parts.iter().zip(&mapping.parts) {
        let placed = match part_mapping {
            PartMapping::Skip => None,
            PartMapping::Track {
                device: d,
                track,
                overflow,
                scale,
            } if *d == device => Some(PlacedPart {
                tracks: vec![(
                    *track,
                    fit_notes(&part.notes, segment, per16, *scale, report),
                )],
                overflow: Some(*overflow),
                scale: *scale,
            }),
            PartMapping::Drums { device: d, tracks } if *d == device => {
                let substitute = model.key == "DT2";
                let mut out = Vec::new();
                for &(pitch, track) in tracks {
                    let notes: Vec<RawNote> = part
                        .notes
                        .iter()
                        .copied()
                        .filter(|n| n.pitch == pitch)
                        .collect();
                    let mut fitted = fit_notes(&notes, segment, per16, TrackScale::One, report);
                    if substitute {
                        for n in &mut fitted {
                            n.pitch = DRUM_TRIGGER_PITCH;
                        }
                    }
                    out.push((track, fitted));
                }
                // Unmapped pitches are dropped and counted (§4.5).
                let mapped: std::collections::BTreeSet<u8> =
                    tracks.iter().map(|&(p, _)| p).collect();
                report.drum_pitches_dropped += part
                    .notes
                    .iter()
                    .filter(|n| {
                        !mapped.contains(&n.pitch) && n.on >= segment.start && n.on < segment.end
                    })
                    .count();
                Some(PlacedPart {
                    tracks: out,
                    overflow: None,
                    scale: TrackScale::One,
                })
            }
            _ => None,
        };
        if let Some(placed) = placed {
            for (track, notes) in placed.tracks {
                by_track.entry(track).or_default().extend(notes);
                if let Some(overflow) = placed.overflow {
                    overflows.insert(track, overflow);
                }
                scales.insert(track, placed.scale);
            }
        }
    }

    // ---- §4.5 flams and §4.6 polyphony, per track ----
    let cap = model.notes_per_trig as usize;
    for (track_i, mut notes) in by_track {
        // Same-step same-pitch duplicates keep the later one (§4.5). Sorting
        // stable by (step, pitch) and deduping keeps the last of each pair.
        notes.sort_by(|a, b| {
            a.step
                .partial_cmp(&b.step)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.pitch.cmp(&b.pitch))
        });
        let before = notes.len();
        notes.dedup_by(|b, a| {
            // `dedup_by` removes the earlier of an equal pair when the closure
            // returns true with (later, earlier) — we keep the later.
            a.step == b.step && a.pitch == b.pitch
        });
        report.same_step_duplicates += before - notes.len();

        // Group by quantised step; the trig cap applies per step (§4.6).
        let mut groups: BTreeMap<u64, Vec<FitNote>> = BTreeMap::new();
        for n in notes {
            groups.entry(n.step.round() as u64).or_default().push(n);
        }
        let mut kept: Vec<FitNote> = Vec::new();
        let mut split: Vec<FitNote> = Vec::new();
        let overflow = overflows.get(&track_i).copied();
        for (_, mut group) in groups {
            group.sort_by_key(|n| n.pitch);
            if group.len() <= cap {
                kept.extend(group);
                continue;
            }
            match overflow {
                Some(Overflow::KeepLowest) | None => {
                    report.notes_over_polyphony += group.len() - cap;
                    kept.extend(group.into_iter().take(cap));
                }
                Some(Overflow::KeepHighest) => {
                    report.notes_over_polyphony += group.len() - cap;
                    let skip = group.len() - cap;
                    kept.extend(group.into_iter().skip(skip));
                }
                Some(Overflow::SplitTo {
                    device: d,
                    track: t,
                }) if d == device => {
                    // Lowest-first stay; the rest move to the second track,
                    // which is subject to the same cap (§4.6).
                    kept.extend(group.iter().take(cap));
                    split.extend(group.into_iter().skip(cap));
                    let _ = t;
                }
                Some(Overflow::SplitTo { .. }) => {
                    report.notes_over_polyphony += group.len() - cap;
                    kept.extend(group.into_iter().take(cap));
                }
            }
        }
        if let Some(Overflow::SplitTo {
            track: split_track, ..
        }) = overflow
        {
            if !split.is_empty() {
                let mut split_groups: BTreeMap<u64, Vec<FitNote>> = BTreeMap::new();
                for n in split {
                    split_groups
                        .entry(n.step.round() as u64)
                        .or_default()
                        .push(n);
                }
                let mut split_kept: Vec<FitNote> = Vec::new();
                for (_, mut group) in split_groups {
                    group.sort_by_key(|n| n.pitch);
                    report.notes_over_polyphony += group.len().saturating_sub(cap);
                    split_kept.extend(group.into_iter().take(cap));
                }
                place_notes(&mut pattern, split_track, split_kept, segment, report);
            }
        }
        place_notes(&mut pattern, track_i, kept, segment, report);
    }

    pattern
}

/// Turn raw notes into fitted notes for one segment (§4.2/§4.7): step
/// relative to the segment start, `off` clamped at the segment end and
/// counted, `ThreeHalves` parts reading `per16 × 2/3` ticks per step.
fn fit_notes(
    notes: &[RawNote],
    segment: &Segment,
    per16: f64,
    scale: TrackScale,
    report: &mut ImportReport,
) -> Vec<FitNote> {
    let ticks_per_step = if scale == TrackScale::ThreeHalves {
        per16 * 2.0 / 3.0
    } else {
        per16
    };
    let mut out = Vec::new();
    for n in notes {
        if n.on < segment.start || n.on >= segment.end {
            continue;
        }
        let step = (n.on - segment.start) as f64 / ticks_per_step;
        let mut len = (n.off.saturating_sub(n.on)) as f64 / ticks_per_step;
        if n.off > segment.end {
            len = (segment.end - n.on) as f64 / ticks_per_step;
            report.clamped_at_boundary += 1;
        }
        out.push(FitNote {
            step,
            pitch: n.pitch,
            len,
            velocity: n.velocity,
        });
    }
    out
}

/// Write fitted notes onto a track (§4.7): off-grid becomes micro-timing
/// (clamped), velocity and length through the same clamps the editor uses.
/// `prob`/`fill`/`cond` stay `None` (§4.6).
fn place_notes(
    pattern: &mut Pattern,
    track_i: usize,
    notes: Vec<FitNote>,
    segment: &Segment,
    report: &mut ImportReport,
) {
    let room = f64::from(segment.steps);
    let Some(track) = pattern.track_mut(track_i) else {
        return;
    };
    for n in notes {
        let step = n.step.round();
        let micro = clamp_micro(n.step - step);
        if step < 0.0 || step >= room {
            continue;
        }
        let len = snap_len_fine(n.len, room - step);
        track.notes.push(Note::new(
            step,
            n.pitch,
            len,
            clamp_velocity(i32::from(n.velocity)),
            micro,
        ));
        report.notes_placed += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midifile::score::{Part, PartStats};
    use crate::{two_box_session, DN2, DT2};

    /// A hand-built score: `division` 96 (24 ticks per 16th), 4/4 throughout
    /// unless `meters` says otherwise, one part with the given notes.
    fn score_with(notes: &[(u64, u64, u8)], end_tick: u64) -> Score {
        score_with_meters(notes, end_tick, vec![(0, Meter { num: 4, den: 4 })])
    }

    fn score_with_meters(
        notes: &[(u64, u64, u8)],
        end_tick: u64,
        meters: Vec<(u64, Meter)>,
    ) -> Score {
        Score {
            division: 96,
            tempo: vec![(0, 120.0)],
            meters,
            markers: Vec::new(),
            parts: vec![Part {
                mtrk: 0,
                channel: 0,
                name: "P".into(),
                program: None,
                notes: notes
                    .iter()
                    .map(|&(on, off, pitch)| RawNote {
                        on,
                        off,
                        pitch,
                        velocity: 100,
                    })
                    .collect(),
                stats: PartStats {
                    notes: notes.len(),
                    pitch_lo: 60,
                    pitch_hi: 72,
                    first_bar: 0,
                    last_bar: 0,
                    max_simultaneous: 1,
                    off_grid_ratio: 0.0,
                    looks_like_drums: false,
                    looks_like_triplets: false,
                },
                cc: BTreeMap::new(),
                sustain_events: 0,
                pitch_bend_events: 0,
            }],
            end_tick,
        }
    }

    fn options(pattern_bars: u8) -> Options {
        Options {
            pattern_bars,
            trim_leading_silence: false,
            start_slot: BTreeMap::new(),
            overwrite_occupied: false,
            apply_tempo: false,
            label_rows_from_markers: true,
        }
    }

    /// Map part 0 onto track 0 of the session's first device (the DT2).
    fn track_mapping(session: &Session) -> Mapping {
        Mapping {
            parts: vec![PartMapping::Track {
                device: session.devices[0].id,
                track: 0,
                overflow: Overflow::KeepLowest,
                scale: TrackScale::One,
            }],
        }
    }

    const BAR: u64 = 96 * 4; // one 4/4 bar at 96 TPQN

    // ---- §4.9 segmenting ----

    #[test]
    fn ninety_bars_at_eight_is_twelve_segments_the_last_two_bars() {
        // 90 bars of 4/4, one note per bar so every segment has content.
        let notes: Vec<(u64, u64, u8)> = (0..90).map(|b| (b * BAR, b * BAR + 24, 60)).collect();
        let score = score_with(&notes, 90 * BAR);
        let session = two_box_session();
        let plan = fit(
            &score,
            &track_mapping(&session),
            &options(8),
            "song",
            &session,
        )
        .expect("fits");
        assert_eq!(plan.report.segments, 12);
        // The last segment is trimmed to its 2 bars of content, not padded
        // to 8 (§4.2): 32 steps, not 128.
        let last_scene = plan.scenes.last().expect("a scene");
        let dt2 = session.devices[0].id;
        let slot = last_scene.slots[&dt2];
        let last_pattern = plan
            .patterns
            .iter()
            .find(|(d, r, _)| *d == dt2 && *r == slot)
            .map(|(_, _, p)| p)
            .expect("the pattern");
        assert_eq!(last_pattern.track(0).unwrap().length_steps, 32);
    }

    #[test]
    fn three_four_at_eight_bars_is_ninety_six_steps() {
        let score = score_with_meters(
            &[(0, 24, 60)],
            8 * 3 * 96,
            vec![(0, Meter { num: 3, den: 4 })],
        );
        let session = two_box_session();
        let plan = fit(
            &score,
            &track_mapping(&session),
            &options(8),
            "waltz",
            &session,
        )
        .expect("fits");
        let dt2 = session.devices[0].id;
        let pattern = &plan.patterns.iter().find(|(d, _, _)| *d == dt2).unwrap().2;
        assert_eq!(pattern.track(0).unwrap().length_steps, 96);
    }

    #[test]
    fn a_meter_change_cuts_a_segment() {
        // 4/4 for 4 bars, then 3/4 at bar 4 (tick 4*BAR). At pattern_bars 8
        // the change must cut the first segment to 4 bars.
        let score = score_with_meters(
            &[(0, 24, 60), (5 * BAR, 5 * BAR + 24, 60)],
            8 * BAR,
            vec![
                (0, Meter { num: 4, den: 4 }),
                (4 * BAR, Meter { num: 3, den: 4 }),
            ],
        );
        let session = two_box_session();
        let plan = fit(&score, &track_mapping(&session), &options(8), "m", &session).expect("fits");
        assert_eq!(plan.report.segments, 2);
        let dt2 = session.devices[0].id;
        let first = plan.scenes[0].slots[&dt2];
        let pattern = plan
            .patterns
            .iter()
            .find(|(d, r, _)| *d == dt2 && *r == first)
            .map(|(_, _, p)| p)
            .unwrap();
        assert_eq!(pattern.track(0).unwrap().length_steps, 64);
    }

    // ---- §4.9 dedupe ----

    #[test]
    fn a_loop_four_times_is_one_pattern_one_scene_one_row_of_four() {
        // An 8-bar loop repeated four times: identical notes in each 8-bar
        // window.
        let mut notes = Vec::new();
        for rep in 0..4u64 {
            notes.push((rep * 8 * BAR, rep * 8 * BAR + 24, 60));
            notes.push((rep * 8 * BAR + 4 * BAR, rep * 8 * BAR + 4 * BAR + 24, 64));
        }
        let score = score_with(&notes, 32 * BAR);
        let session = two_box_session();
        let plan = fit(
            &score,
            &track_mapping(&session),
            &options(8),
            "loop",
            &session,
        )
        .expect("fits");
        assert_eq!(plan.report.segments, 4);
        assert_eq!(plan.patterns.len(), 1, "one unique pattern");
        assert_eq!(plan.scenes.len(), 1);
        assert_eq!(plan.rows.len(), 1);
        assert_eq!(plan.rows[0].2, 4, "repeats = 4");
    }

    #[test]
    fn abab_is_two_patterns_two_scenes_four_rows() {
        let mut notes = Vec::new();
        for rep in 0..2u64 {
            // A: note at window start; B: note a bar later.
            notes.push((rep * 16 * BAR, rep * 16 * BAR + 24, 60));
            notes.push((rep * 16 * BAR + 9 * BAR, rep * 16 * BAR + 9 * BAR + 24, 64));
        }
        let score = score_with(&notes, 32 * BAR);
        let session = two_box_session();
        let plan = fit(
            &score,
            &track_mapping(&session),
            &options(8),
            "abab",
            &session,
        )
        .expect("fits");
        assert_eq!(plan.patterns.len(), 2, "A and B");
        assert_eq!(plan.scenes.len(), 2, "non-consecutive reuse adds no scene");
        assert_eq!(plan.rows.len(), 4);
        assert!(plan.rows.iter().all(|&(_, _, r)| r == 1));
        // Rows alternate scene 0, 1, 0, 1.
        let scene_ids: Vec<usize> = plan.rows.iter().map(|r| r.0).collect();
        assert_eq!(scene_ids, vec![0, 1, 0, 1]);
    }

    // ---- §4.9 boundary clamp ----

    #[test]
    fn a_note_crossing_a_segment_end_is_clamped_and_counted() {
        // One note starting in segment 1 and ending two bars into segment 2.
        let score = score_with(&[(7 * BAR, 10 * BAR, 60)], 16 * BAR);
        let session = two_box_session();
        let plan = fit(&score, &track_mapping(&session), &options(8), "x", &session).expect("fits");
        assert_eq!(plan.report.clamped_at_boundary, 1);
        let dt2 = session.devices[0].id;
        let first = plan.scenes[0].slots[&dt2];
        let pattern = plan
            .patterns
            .iter()
            .find(|(d, r, _)| *d == dt2 && *r == first)
            .map(|(_, _, p)| p)
            .unwrap();
        let note = &pattern.track(0).unwrap().notes[0];
        assert_eq!(note.step, 7.0 * 16.0);
        assert!(note.len <= 16.0, "clamped at the segment end");
    }

    // ---- §4.9 slot allocation ----

    #[test]
    fn allocation_skips_occupied_slots() {
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        // Occupy A01 with a note.
        let mut session = session;
        let dev = session.device_mut(dt2).unwrap();
        let pattern = std::sync::Arc::make_mut(&mut dev.patterns[0]);
        pattern
            .track_mut(0)
            .unwrap()
            .notes
            .push(Note::new(0.0, 60, 1.0, 100, 0.0));

        let score = score_with(&[(0, 24, 60), (8 * BAR, 8 * BAR + 24, 64)], 16 * BAR);
        let plan = fit(&score, &track_mapping(&session), &options(8), "s", &session).expect("fits");
        // Two distinct segments → two slots, neither of them A01.
        assert_eq!(plan.patterns.len(), 2);
        assert!(plan.patterns.iter().all(|(_, r, _)| r.slot() != 0));
    }

    #[test]
    fn allocation_refuses_when_slots_run_short() {
        let mut session = two_box_session();
        let dt2 = session.devices[0].id;
        // Fill every slot but one.
        let dev = session.device_mut(dt2).unwrap();
        for i in 1..dev.patterns.len() {
            let pattern = std::sync::Arc::make_mut(&mut dev.patterns[i]);
            pattern
                .track_mut(0)
                .unwrap()
                .notes
                .push(Note::new(0.0, 60, 1.0, 100, 0.0));
        }
        // Two distinct segments need two slots; one is free.
        let score = score_with(&[(0, 24, 60), (8 * BAR, 8 * BAR + 24, 64)], 16 * BAR);
        let err =
            fit(&score, &track_mapping(&session), &options(8), "s", &session).expect_err("refused");
        assert!(matches!(err, PlanError::NoFreeSlots { device, .. } if device == dt2));
    }

    #[test]
    fn overwrite_occupied_lists_what_it_replaces() {
        let mut session = two_box_session();
        let dt2 = session.devices[0].id;
        {
            let dev = session.device_mut(dt2).unwrap();
            let pattern = std::sync::Arc::make_mut(&mut dev.patterns[0]);
            pattern.name = "Keep me not".into();
            pattern
                .track_mut(0)
                .unwrap()
                .notes
                .push(Note::new(0.0, 60, 1.0, 100, 0.0));
            pattern
                .track_mut(1)
                .unwrap()
                .notes
                .push(Note::new(0.0, 62, 1.0, 100, 0.0));
        }
        let mut opts = options(8);
        opts.overwrite_occupied = true;
        let score = score_with(&[(0, 24, 60)], BAR);
        let plan = fit(&score, &track_mapping(&session), &opts, "s", &session).expect("fits");
        assert_eq!(plan.patterns[0].1.slot(), 0, "took A01");
        assert_eq!(plan.report.replacing.len(), 1);
        let (d, r, name, notes) = &plan.report.replacing[0];
        assert_eq!(*d, dt2);
        assert_eq!(r.slot(), 0);
        assert_eq!(name, "Keep me not");
        assert_eq!(*notes, 2);
    }

    // ---- §4.9 drums ----

    fn drum_score(hits: &[(u64, u8)]) -> Score {
        let notes: Vec<(u64, u64, u8)> = hits.iter().map(|&(on, p)| (on, on + 24, p)).collect();
        let mut score = score_with(&notes, BAR);
        score.parts[0].channel = 9;
        score.parts[0].stats.looks_like_drums = true;
        score
    }

    #[test]
    fn drum_fanout_substitutes_the_trigger_pitch_on_a_dt2() {
        let score = drum_score(&[(0, 36), (24, 38)]);
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        let mapping = Mapping {
            parts: vec![PartMapping::Drums {
                device: dt2,
                tracks: vec![(36, 0), (38, 1)],
            }],
        };
        let plan = fit(&score, &mapping, &options(1), "d", &session).expect("fits");
        let pattern = &plan.patterns[0].2;
        let kick = &pattern.track(0).unwrap().notes;
        assert_eq!(kick.len(), 1);
        assert_eq!(kick[0].pitch, DRUM_TRIGGER_PITCH);
        assert_eq!(pattern.track(1).unwrap().notes[0].pitch, DRUM_TRIGGER_PITCH);
    }

    #[test]
    fn drum_fanout_keeps_the_pitch_on_a_dn2() {
        let score = drum_score(&[(0, 36), (24, 38)]);
        let session = two_box_session();
        let dn2 = session.devices[1].id;
        assert_eq!(session.device(dn2).unwrap().model.key, "DN2");
        let mapping = Mapping {
            parts: vec![PartMapping::Drums {
                device: dn2,
                tracks: vec![(36, 0), (38, 1)],
            }],
        };
        let plan = fit(&score, &mapping, &options(1), "d", &session).expect("fits");
        let pattern = plan
            .patterns
            .iter()
            .find(|(d, _, _)| *d == dn2)
            .map(|x| &x.2)
            .unwrap();
        assert_eq!(pattern.track(0).unwrap().notes[0].pitch, 36);
        assert_eq!(pattern.track(1).unwrap().notes[0].pitch, 38);
    }

    #[test]
    fn unmapped_drum_pitches_are_dropped_and_counted() {
        let score = drum_score(&[(0, 36), (24, 99), (48, 99)]);
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        let mapping = Mapping {
            parts: vec![PartMapping::Drums {
                device: dt2,
                tracks: vec![(36, 0)],
            }],
        };
        let plan = fit(&score, &mapping, &options(1), "d", &session).expect("fits");
        assert_eq!(plan.report.drum_pitches_dropped, 2);
    }

    #[test]
    fn same_step_same_pitch_hits_keep_the_later_one() {
        // A flam: two hits of pitch 36 on the same step.
        let score = drum_score(&[(0, 36), (0, 36)]);
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        let mapping = Mapping {
            parts: vec![PartMapping::Drums {
                device: dt2,
                tracks: vec![(36, 0)],
            }],
        };
        let plan = fit(&score, &mapping, &options(1), "d", &session).expect("fits");
        assert_eq!(plan.report.same_step_duplicates, 1);
        assert_eq!(plan.patterns[0].2.track(0).unwrap().notes.len(), 1);
    }

    #[test]
    fn gm_names_cover_the_map_and_fall_back() {
        assert_eq!(gm_drum_name(36), "Kick");
        assert_eq!(gm_drum_name(45), "Tom");
        assert_eq!(gm_drum_name(57), "Crash");
        assert_eq!(gm_drum_name(99), "Perc 99");
    }

    // ---- §4.9 polyphony ----

    fn chord_score(pitches: &[u8]) -> Score {
        let notes: Vec<(u64, u64, u8)> = pitches.iter().map(|&p| (0, 24, p)).collect();
        score_with(&notes, BAR)
    }

    fn fit_chord(pitches: &[u8], overflow: Overflow) -> (ImportPlan, Session) {
        let score = chord_score(pitches);
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        let mapping = Mapping {
            parts: vec![PartMapping::Track {
                device: dt2,
                track: 0,
                overflow,
                scale: TrackScale::One,
            }],
        };
        let plan = fit(&score, &mapping, &options(1), "c", &session).expect("fits");
        (plan, session)
    }

    #[test]
    fn keep_lowest_drops_the_top_of_an_over_cap_chord() {
        let (plan, _) = fit_chord(&[60, 62, 64, 65, 67, 69], Overflow::KeepLowest);
        let notes = &plan.patterns[0].2.track(0).unwrap().notes;
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 62, 64, 65]);
        assert_eq!(plan.report.notes_over_polyphony, 2);
    }

    #[test]
    fn keep_highest_drops_the_bottom_of_an_over_cap_chord() {
        let (plan, _) = fit_chord(&[60, 62, 64, 65, 67, 69], Overflow::KeepHighest);
        let notes = &plan.patterns[0].2.track(0).unwrap().notes;
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![64, 65, 67, 69]);
    }

    #[test]
    fn split_to_moves_the_excess_to_a_second_track_under_the_same_cap() {
        let score = chord_score(&[60, 62, 64, 65, 67, 69, 71, 72, 74]);
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        let mapping = Mapping {
            parts: vec![PartMapping::Track {
                device: dt2,
                track: 0,
                overflow: Overflow::SplitTo {
                    device: dt2,
                    track: 1,
                },
                scale: TrackScale::One,
            }],
        };
        let plan = fit(&score, &mapping, &options(1), "c", &session).expect("fits");
        let pattern = &plan.patterns[0].2;
        let first: Vec<u8> = pattern
            .track(0)
            .unwrap()
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        let second: Vec<u8> = pattern
            .track(1)
            .unwrap()
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        assert_eq!(first, vec![60, 62, 64, 65], "lowest-first stay");
        assert_eq!(second, vec![67, 69, 71, 72], "same cap on the second track");
        assert_eq!(
            plan.report.notes_over_polyphony, 1,
            "74 is beyond two tracks"
        );
    }

    // ---- §4.9 timing ----

    #[test]
    fn sixteenth_triplets_land_on_whole_steps_at_three_halves() {
        // 16th-triplets at 96 TPQN: 16 ticks apart. At ThreeHalves the
        // tick-per-step is 24 × 2/3 = 16, so each lands exactly on a step.
        let notes: Vec<(u64, u64, u8)> =
            (0..6).map(|i| (i * 16, i * 16 + 8, 60 + i as u8)).collect();
        let score = score_with(&notes, BAR);
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        let mapping = Mapping {
            parts: vec![PartMapping::Track {
                device: dt2,
                track: 0,
                overflow: Overflow::KeepLowest,
                scale: TrackScale::ThreeHalves,
            }],
        };
        let plan = fit(&score, &mapping, &options(1), "t", &session).expect("fits");
        let notes = &plan.patterns[0].2.track(0).unwrap().notes;
        assert_eq!(notes.len(), 6);
        for (i, n) in notes.iter().enumerate() {
            assert_eq!(n.step, i as f64);
            assert_eq!(n.micro, 0.0, "triplet {i} lands flat");
        }
    }

    #[test]
    fn off_grid_notes_become_micro_timing() {
        // 6 ticks late of step 1 at 24 ticks/step = +0.25 steps of micro.
        let score = score_with(&[(24 + 6, 24 + 6 + 24, 60)], BAR);
        let session = two_box_session();
        let plan = fit(&score, &track_mapping(&session), &options(1), "m", &session).expect("fits");
        let note = &plan.patterns[0].2.track(0).unwrap().notes[0];
        assert_eq!(note.step, 1.0);
        assert!((note.micro - 0.25).abs() < 1e-9);
    }

    // ---- §4.9 rows and labels ----

    #[test]
    fn a_marker_at_a_segment_start_labels_its_row() {
        let mut score = score_with(&[(0, 24, 60), (8 * BAR, 8 * BAR + 24, 64)], 16 * BAR);
        score.markers = vec![(8 * BAR, "Chorus".into())];
        let session = two_box_session();
        let plan = fit(&score, &track_mapping(&session), &options(8), "s", &session).expect("fits");
        assert_eq!(plan.rows[0].1, "Row 1");
        assert_eq!(plan.rows[1].1, "Chorus");
    }

    #[test]
    fn tempo_is_offered_and_changes_counted() {
        let mut score = score_with(&[(0, 24, 60)], BAR);
        score.tempo = vec![(0, 120.0), (BAR, 90.0)];
        let session = two_box_session();
        let plan = fit(&score, &track_mapping(&session), &options(1), "s", &session).expect("fits");
        assert_eq!(plan.tempo_bpm, Some(120.0));
        assert_eq!(plan.report.tempo_changes_dropped, 1);
    }

    #[test]
    fn trim_leading_silence_starts_at_the_first_notes_bar() {
        // First note in bar 3; trimming makes it segment 1, step 0.
        let score = score_with(&[(3 * BAR, 3 * BAR + 24, 60)], 4 * BAR);
        let session = two_box_session();
        let mut opts = options(4);
        opts.trim_leading_silence = true;
        let plan = fit(&score, &track_mapping(&session), &opts, "s", &session).expect("fits");
        assert_eq!(plan.report.segments, 1);
        let note = &plan.patterns[0].2.track(0).unwrap().notes[0];
        assert_eq!(note.step, 0.0);
    }

    #[test]
    fn default_pattern_bars_picks_the_largest_that_fits() {
        // 4/4: 16 steps/bar. DT2 (128) → 8; with an A4 (64) → 4.
        let score = score_with(&[(0, 24, 60)], BAR);
        assert_eq!(default_pattern_bars(&score, &[&DT2]), 8);
        assert_eq!(default_pattern_bars(&score, &[&DT2, &crate::A4]), 4);
        // 3/4: 12 steps/bar → a digi still gets 8 (96 ≤ 128).
        let waltz = score_with_meters(&[(0, 24, 60)], 3 * 96, vec![(0, Meter { num: 3, den: 4 })]);
        assert_eq!(default_pattern_bars(&waltz, &[&DT2]), 8);
        assert_eq!(default_pattern_bars(&waltz, &[&DN2]), 8);
    }

    // ---- §5.6 apply ----

    /// Four bars of one-note-per-bar 4/4, fitted at 4 bars — one segment, one
    /// scene, one row, one pattern on the DT2.
    fn four_bar_plan(session: &Session) -> ImportPlan {
        let notes: Vec<(u64, u64, u8)> = (0..4).map(|b| (b * BAR, b * BAR + 24, 60)).collect();
        let score = score_with(&notes, 4 * BAR);
        fit(
            &score,
            &track_mapping(session),
            &options(4),
            "song",
            session,
        )
        .expect("fits")
    }

    #[test]
    fn apply_writes_patterns_scenes_and_rows_and_reports_what_it_made() {
        let mut session = two_box_session();
        let dt2 = session.devices[0].id;
        let dn2 = session.devices[1].id;
        let plan = four_bar_plan(&session);
        let slot = plan.patterns[0].1;
        let planned_notes = plan.patterns[0].2.track(0).unwrap().notes.len();

        let applied = apply_import(&mut session, plan, false);

        assert_eq!(
            applied,
            AppliedImport {
                first_scene: 1,
                first_row: 0,
                scenes: 1,
                rows: 1,
                patterns: 1
            }
        );
        // The pattern is in its slot, whole.
        assert_eq!(
            session.device(dt2).unwrap().patterns[slot.slot()]
                .track(0)
                .unwrap()
                .notes
                .len(),
            planned_notes
        );
        // The scene points the DT2 at the new slot and the DN2 — not in the
        // import — at what the current scene already played (§5.6 step 2).
        assert_eq!(session.slot_in_scene(1, dt2), Some(slot));
        assert_eq!(session.slot_in_scene(1, dn2), session.slot_in_scene(0, dn2));
        // The row names the scene and keeps the plan's label and repeats.
        let row = session.song_row(0).expect("a row");
        assert_eq!(row.scene, 1);
        assert_eq!(row.label, "Row 1");
        assert_eq!(row.repeats, 1);
        assert!(row.length_steps.is_none());
        // Untouched by step 5: the scene that is sounding stays put.
        assert_eq!(session.current_scene, 0);
    }

    #[test]
    fn apply_leaves_the_tempo_alone_unless_opted_in() {
        let mut session = two_box_session();
        session.tempo_bpm = 96.0;
        let mut plan = four_bar_plan(&session);
        plan.tempo_bpm = Some(140.0);

        let applied = apply_import(&mut session, plan.clone(), false);
        assert_eq!(session.tempo_bpm, 96.0);

        // The option is a plan-time choice (§5.5); the same plan with it set
        // moves the session clock.
        let plan2 = ImportPlan {
            patterns: Vec::new(),
            scenes: Vec::new(),
            rows: Vec::new(),
            ..plan
        };
        let _ = applied;
        apply_import(&mut session, plan2, true);
        assert_eq!(session.tempo_bpm, 140.0);
    }

    #[test]
    fn an_import_undoes_in_one_step() {
        use crate::history::{Content, History};
        let mut session = two_box_session();
        let dt2 = session.devices[0].id;
        let before = Content::of(&session);
        let mut history = History::default();

        history.begin(Content::of(&session));
        let plan = four_bar_plan(&session);
        let slot = plan.patterns[0].1;
        apply_import(&mut session, plan, false);
        assert!(history.commit(&session), "the import is one step");

        assert!(history.undo(&mut session));
        // The patterns are back to blank — the part `Content` holds. Scenes and
        // rows stay: the history is over the notes, the same line the song
        // panel already draws (ui::song's reference says so).
        assert!(session.device(dt2).unwrap().patterns[slot.slot()].is_blank());
        assert_eq!(session.scenes.len(), 2);
        assert!(before.matches(&session));
    }

    #[test]
    fn an_applied_import_round_trips_through_the_project_file() {
        use crate::project::Project;
        let mut session = two_box_session();
        let plan = four_bar_plan(&session);
        apply_import(&mut session, plan, true);

        let json = Project::new(session.clone()).to_json_pretty().unwrap();
        let back = Project::from_json(&json).unwrap().session;
        assert_eq!(back, session);
        // And the reopened file saves byte-identically, the stronger claim
        // `a_session_reopened_and_saved_again_is_byte_identical` makes.
        let second = Project::new(back).to_json_pretty().unwrap();
        assert_eq!(json, second);
    }
}
