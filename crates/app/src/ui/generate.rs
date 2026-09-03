// The Generate panel: the rail's third slot, and the last one that was still a
// labelled empty box (`ui::tools`' placeholder text, gone from this change).
//
// PLAN.md Phase 7's five build stages shipped `crates/generator` — foundation,
// rhythm, the three melodic parts plus drums, p-locks — on 2026-08-19, and
// nothing in `app` called any of it. This is that surface: a Root/Scale that
// reads `Session::harmony` rather than a second pair, a row list of parts with
// add/remove and a box+slot+track picker validated through
// `context::Destination::validate`, the progression field wired to
// `context::check_progression`, the feel sliders, and a Generate button.
//
// ## Session only, not a write — decided 2026-08-19
//
// **Generate touches the session and nothing else.** It replaces the notes and
// p-lock lanes of the tracks its "on" rows name, exactly the way drawing in the
// roll by hand would — undoable with Cmd+Z, playable through the app's own
// engine immediately, and previewable before anything reaches a box. Getting a
// generated track onto hardware is then one press of the already-built OUT
// button in the Setup panel: it already knows how to read a track's notes and
// lanes and put them safely on a card, and it does not need to know its content
// came from a generator rather than a hand.
//
// **A row's ↻ writes on the press, the button asks first.** Both land in the
// same place by the same path ([`GeneratePanel::apply`]); they differ only in
// whether the confirm dialog stands in front. A press of GENERATE replaces
// every "on" row at once and is worth a dialog; a ↻ replaces one row and
// whatever answers it, is the direct answer to "show me another one", and
// would be useless with a modal in the way. The dialog is not skipped
// blindly, though: it still appears when a re-roll would land on a track
// this panel has never written — see `GeneratePanel::written`.
//
// That is a deliberate narrowing of "What to do next" in `DEVELOPMENT.md`, which
// pictured one button calling `safe_write_tracks` behind its own confirm
// dialog. Doing that here would duplicate a fair slice of `ui::sync`'s survey
// and worker-thread machinery inside this file for a second, parallel path to
// a card — and it would mean a first pass at a generated arrangement can only
// be judged by ear on a box, with no chance to look at it in the roll, tweak a
// slider and try again first. Reusing the OUT block's own send keeps PLAN.md's
// rule 3 (reach for the plural write, never a bespoke second one) and costs
// nothing: that button already reads whatever is in a track.
//
// ## Every "on" row's p-locks are designed for its *own* box
//
// `generator::arrange::generate_arrangement` takes one `device_kind` for the
// whole call, because `js/gen/` only ever knew one box. A session here can
// have a DT2 and a DN2 in it at once, and a p-lock lane belongs to one box's
// parameter numbering — 74 is overdrive on a DT2 and filter frequency on a DN2
// — so authoring every row's lanes against one assumed kind would be silently
// wrong for whichever rows are *not* aimed at that box: `core::export::
// lanes_for_device` would see a lane's `device_kind` disagree with the
// destination's and quietly drop it. [`arranged_parts`] runs the whole
// arrangement once un-assumed (`device_kind: None`, which the notes and the
// busy-map threading do not read at all — only `design_lanes` does), then
// re-derives just the rows whose destination resolves to a real box through
// [`generate_part`], asking for lanes in *that* row's own numbering. The notes
// tie exactly across both passes (`the_seed_makes_a_result_reproducible`
// proves determinism, and `keeps_other_parts_still_when_only_one_parts_density_
// moves` proves nothing but `design_lanes` reads `device_kind`), so this never
// re-rolls a row's music — only which parameters its lanes name.
//
// ## The p-lock pool is arbitrated per destination slot, first-served
//
// `plockdesign::design_lanes` picks a per-part lane count from Motion alone,
// with no awareness that another row might be aimed at the same slot's shared
// pool of 80 — safe for the JS's fixed three parts, not safe once N rows can
// share a destination. PLAN.md Phase 7 Decision 1 raised the question and left
// it to whichever layer owns "who gets told their lanes were cut"; this is
// that layer. [`cap_lane_pool`] groups a plan's "on" rows by destination slot,
// works out what is left of the pool once the rows already there but *not*
// being replaced keep theirs, and calls `plockdesign::arbitrate_pool` — row
// order, first served, the same order the busy-map already threads notes in —
// to cap each row's lane count and warn about whoever lost some. It is a
// judgment call, not something Neil settled, and it only ever *removes* lanes
// a row's own design already produced; nothing here invents a lane budget of
// its own.
//
// ## The settings ride in the session — added 2026-09-02
//
// **Genre, progression, seed, feel and every row are saved with the project
// and come back with it**, so a session recalls the arrangement it was written
// by and not only the notes that came out. They live in `Session::generator`,
// which core carries as an opaque `serde_json::Value` and never reads:
// `context::GenContext` is `crates/generator`'s type and `core` cannot name it
// without closing a dependency cycle (`generator` already depends on `core`).
// This file is the one place that depends on both, so this file does the
// encoding — `mirror_into` on the way out, `session_replaced` on the way back.
//
// Moving `GenContext` into `core` instead — the way `Harmony` moved, since it
// was core's own type — would have dragged `genres`, `progressions` and
// `theory` with it, which is most of the generator crate, to make one field
// nameable. The opaque handle is the same bargain `DeviceModel::sysex` already
// strikes with `protocol`.
//
// Two fields do not travel this way and should not: `root` and `scale` mirror
// `Session::harmony`, which is saved in its own place and copied in at the top
// of every frame. What still does not persist is `written` — which tracks this
// panel touched, and therefore may be re-rolled without a confirm — because it
// is a claim about a *sitting*, not about a file: reopening yesterday's project
// is not consent to overwrite yesterday's tracks unasked.
//
// A settings change is unsaved work but not an *edit*: it marks the file dirty
// through `Outcome::settings` and stops there, opening no history step (a
// slider moves no note, and `history::Content` snapshots patterns only) and
// prompting no engine re-sync.

use std::collections::{HashMap, HashSet};

use digi_core::chords::PITCH_CLASSES;
use digi_core::device::DeviceId;
use digi_core::model::{Note, PLockLane};
use digi_core::session::PatternRef;
use digi_core::Session;
use digi_generator::arrange::{apply_part_to_track, generate_arrangement, generate_part, ArrangeError, ArrangedPart};
use digi_generator::context::{
    bpm_suggestion, check_progression, Destination, DestinationError, GenContext, Part, PartId, GEN_BARS,
};
use digi_generator::genres::{genre_profile, GenreId, Role};
use digi_generator::plockdesign::{arbitrate_pool, LaneClaim, NO_BOX_WARNING};
use digi_generator::progressions::{default_progression_for, next_progression_for, progression_note};
use digi_generator::theory::DEFAULT_SCALE;
use eframe::egui::{self, Color32, Ui};

use crate::ui::transfer::slot_choices;
use crate::ui::{
    colored_button, consequence_line, panel_title_bar, section_header, slider_row, CYAN, CYAN_FILL, CYAN_INK,
    CYAN_TEXT, PANEL_BG_RAISED, PANEL_BORDER, TEXT_BRIGHT, TEXT_DIM, TEXT_DIMMER, TEXT_DIMMEST,
    TEXT_SECONDARY, WARN_AMBER, WARN_AMBER_BORDER, WARN_AMBER_FILL, WARN_AMBER_TEXT,
};

/// The conflicting part card's fill — `#221d12` — the one colour this panel
/// needs that isn't in `ui::mod`'s shared token table, because no other
/// panel has a per-row "this is wrong" background yet. If a second panel
/// ever needs it, it should move up to `ui::mod` the way the rest of the v2
/// palette already has; until then it stays here rather than growing the
/// shared table for one caller.
const CARD_BG_CONFLICT: Color32 = Color32::from_rgb(0x22, 0x1d, 0x12);

/// The Bars combo box's width, now that it shares the ARRANGEMENT row with
/// Genre. egui's default (`Spacing::combo_width`, 100px) is sized for a name;
/// these entries are `1`, `2`, `4`, `8`, so 56px is the digit, the arrow and
/// the padding either side — and the rest of the row stays with the genre
/// name, which is the part that can actually run out of room.
const BARS_COMBO_WIDTH: f32 = 56.0;

/// What one frame of the panel did.
#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    pub close: bool,
    /// A generate was applied to the session, or the bpm suggestion was taken.
    /// Either way the shell opens a history step around it exactly as any other
    /// edit — `core::history` snapshots patterns, so a generate that rewrites
    /// several tracks undoes as one step for free.
    pub edited: bool,
    /// This panel's own settings changed and were mirrored into
    /// `Session::generator`. Unsaved work, and nothing else: no note moved, so
    /// there is no history step to open and nothing for the engine to re-sync.
    /// The shell folds this into the dirty flag alone — see `ui::tools`.
    pub settings: bool,
}

/// One "on" row, generated and ready to land on its track — what the confirm
/// dialog shows, and what [`apply_plan`] writes.
pub struct PlannedRow {
    pub part_id: PartId,
    pub role: Role,
    pub destination: Destination,
    pub device_name: String,
    pub slot_label: String,
    pub track_label: String,
    /// What the track is named once this lands — the genre and the role, so
    /// the slot dropdown says what generated it.
    pub track_name: String,
    pub notes: usize,
    pub lanes: usize,
    pub existing_notes: usize,
    pub existing_lanes: usize,
    /// Why lanes came back short of what Motion asked for — no resolvable box,
    /// no measured parameter match, or the pool ran out (added by
    /// [`cap_lane_pool`] on top of whatever `design_lanes` already said).
    pub warnings: Vec<String>,
    arranged: ArrangedPart,
}

/// Everything a press of Generate is about to do.
pub struct GeneratePlan {
    pub rows: Vec<PlannedRow>,
    /// Said once per destination slot rather than per row — the pool is one
    /// budget shared by everything that lands there.
    pub pool_warnings: Vec<String>,
}

/// Why a plan could not be built, worded for the panel to show directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    Progression(String),
    Destination { role: Role, why: String },
    Collision(String),
    NothingOn,
}

impl PlanError {
    pub fn message(&self) -> String {
        match self {
            Self::Progression(e) => e.clone(),
            Self::Destination { role, why } => format!("{}: {why}", role.label()),
            Self::Collision(s) => s.clone(),
            Self::NothingOn => "nothing is switched on — turn a row on before generating".into(),
        }
    }
}

fn destination_error_message(why: DestinationError) -> String {
    match why {
        DestinationError::NoDevice => "pick a box for this row".into(),
        DestinationError::DeviceMissing => "that box is no longer in this session".into(),
        DestinationError::SlotOutOfRange => "that slot isn't one this box has".into(),
        DestinationError::TrackOutOfRange { num_tracks } => format!("this box only has {num_tracks} tracks"),
    }
}

/// The device-kind string `design_lanes` wants, from the actual box a
/// destination names — `None` only when the row names a box this session no
/// longer has, which `design_lanes` already turns into "no lanes, and why"
/// rather than a guess.
///
/// **The box's own key, not its `Spec`'s.** This read `model.spec().device`
/// until 2026-09-02, and `spec()` is `None` on the Analog Four and always will
/// be — so every A4 row came back with "digi-roll can't tell which box this is
/// for", on a box whose thirteen lane parameters have measured p-lock ids and
/// scalings (`params::A4_PARAMS`, 2026-09-01) and whose lanes
/// `core::a4_transfer::a4_lanes_for_write` has been writing since. A `Spec` is
/// the *gen-2* pattern layout; asking for one here was asking a question about
/// byte offsets in order to answer one about parameter numbering.
/// `ui::edit`'s "+ add lane…" menu had the same bug and took the same fix on
/// 2026-09-01 — see its own note. This is the other half of it.
fn device_kind_of(session: &Session, device: DeviceId) -> Option<&'static str> {
    session.device(device).map(|d| d.model.key)
}

/// SYN1–SYN4. The A4's fifth and sixth tracks are the FX and CV tracks, and
/// **their p-lock parameter numbering is not the synth tracks'** — measured
/// 2026-09-01, an FX-track lock landed on `0x1a` and `0x29`, both of which name
/// *synth* parameters. So a lane designed out of `A4_PARAMS` and written onto
/// one of those two would be a confident wrong knob, which is the failure
/// `params::plock_id_identifies_parameter` and `a4_transfer::a4_lane_to_model`
/// both refuse on the way in. The notes still go; only the lanes stop here.
const A4_SYNTH_TRACKS: usize = 4;

/// Why an A4 FX or CV row gets no lanes. It replaces
/// [`digi_generator::plockdesign::NO_BOX_WARNING`], which would send the user to
/// the MIDI output menu for something no picker can fix.
const A4_NON_SYNTH_WARNING: &str = "no p-lock lanes — the A4's FX and CV tracks number their \
     p-lock parameters differently from SYN1–SYN4, and digi-roll hasn't measured that numbering. \
     The notes still go; aim this row at SYN1–SYN4 if you want lanes with them.";

/// The base part, with the "which box?" warning swapped for the one that is
/// true here. Every other warning it carries — a `Lead (response)` with no call
/// above it — is its own and is left alone.
fn explain_non_synth(mut part: ArrangedPart) -> ArrangedPart {
    for warning in &mut part.warnings {
        if warning == NO_BOX_WARNING {
            *warning = A4_NON_SYNTH_WARNING.to_string();
        }
    }
    part
}

/// The whole arrangement, with every part's p-locks designed against *its own*
/// destination's box rather than one assumed kind — see the module header.
fn arranged_parts(ctx: &GenContext, session: &Session) -> Result<Vec<ArrangedPart>, ArrangeError> {
    let base = generate_arrangement(ctx, None)?;
    base.parts
        .into_iter()
        .map(|part| match part.destination.device.and_then(|id| device_kind_of(session, id)) {
            // The base arrangement was generated with no box, so this part
            // already has no lanes; all it needs is the true reason.
            Some("A4") if part.destination.track >= A4_SYNTH_TRACKS => Ok(explain_non_synth(part)),
            Some(kind) => generate_part(ctx, part.part_id, Some(kind)),
            None => Ok(part),
        })
        .collect()
}

/// Cap each destination slot's total lane demand to what the shared pool of 80
/// actually has left, in row order — see the module header. Returns the
/// warnings; mutates `parts` in place, only ever shortening a `plocks` list
/// that `design_lanes` already produced.
fn cap_lane_pool(parts: &mut [ArrangedPart], session: &Session) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut groups: Vec<(DeviceId, usize)> = Vec::new();
    for p in parts.iter().filter(|p| p.on) {
        if let Some(device) = p.destination.device {
            let key = (device, p.destination.slot.slot());
            if !groups.contains(&key) {
                groups.push(key);
            }
        }
    }

    for (device, slot) in groups {
        let Some(dev) = session.device(device) else { continue };
        // `DeviceModel::plock_pool`, not `spec().pattern.num_p_locks`: the A4
        // has a 128-lane pool and no `Spec`, and reading the spec here skipped
        // it entirely — see `device_kind_of` for the same mistake in the same
        // panel.
        let capacity_total = dev.model.plock_pool();
        if capacity_total == 0 {
            continue;
        }

        let touched: Vec<usize> = parts
            .iter()
            .filter(|p| p.on && p.destination.device == Some(device) && p.destination.slot.slot() == slot)
            .map(|p| p.destination.track)
            .collect();
        // What the slot's *other* tracks — the ones this generate is not
        // replacing — are already holding onto. Those lanes stay theirs.
        let reserved: usize = dev
            .pattern(slot)
            .map(|pattern| {
                pattern
                    .tracks()
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !touched.contains(i))
                    .map(|(_, t)| t.plocks.len())
                    .sum()
            })
            .unwrap_or(0);
        let capacity = capacity_total.saturating_sub(reserved);

        let claims: Vec<LaneClaim<PartId>> = parts
            .iter()
            .filter(|p| p.on && p.destination.device == Some(device) && p.destination.slot.slot() == slot)
            .map(|p| LaneClaim {
                id: p.part_id,
                label: format!("{} T{}", p.role.label(), p.destination.track + 1),
                wanted: p.plocks.len(),
            })
            .collect();
        if claims.iter().map(|c| c.wanted).sum::<usize>() <= capacity {
            continue;
        }
        let budget = arbitrate_pool(claims, capacity);
        warnings.extend(budget.warnings);
        for part in parts.iter_mut() {
            if part.on && part.destination.device == Some(device) && part.destination.slot.slot() == slot {
                if let Some(&granted) = budget.granted.get(&part.part_id) {
                    if part.plocks.len() > granted {
                        part.plocks.truncate(granted);
                    }
                }
            }
        }
    }
    warnings
}

/// Build the whole plan a press of Generate would carry out. Pure — no session
/// mutation — so the panel can call it every frame to decide whether the
/// button is enabled and what it would say, the same way `ui::sync::plan_box`
/// is read every frame.
pub fn plan_generate(ctx: &GenContext, session: &Session) -> Result<GeneratePlan, PlanError> {
    for part in ctx.parts.iter().filter(|p| p.on) {
        if let Err(why) = part.destination.validate(session) {
            return Err(PlanError::Destination { role: part.role, why: destination_error_message(why) });
        }
    }

    // Two "on" rows aimed at the same track would have the second silently
    // clobber the first's note count — refused rather than guessed at.
    let mut seen: HashSet<(DeviceId, usize, usize)> = HashSet::new();
    for part in ctx.parts.iter().filter(|p| p.on) {
        let key = (
            part.destination.device.expect("validated above"),
            part.destination.slot.slot(),
            part.destination.track,
        );
        if !seen.insert(key) {
            return Err(PlanError::Collision(format!(
                "more than one row is aimed at {} T{}",
                PatternRef::from_slot(key.1).label(),
                key.2 + 1
            )));
        }
    }

    if !ctx.parts.iter().any(|p| p.on) {
        return Err(PlanError::NothingOn);
    }

    let mut arranged = arranged_parts(ctx, session).map_err(|e| match e {
        ArrangeError::Progression(e) => PlanError::Progression(e.0),
        ArrangeError::UnknownPart(_) => {
            unreachable!("every id handed to generate_part here came from ctx.parts itself")
        }
    })?;
    let pool_warnings = cap_lane_pool(&mut arranged, session);

    let genre = genre_profile(ctx.genre);
    let rows = arranged
        .into_iter()
        .filter(|p| p.on)
        .map(|arranged| {
            let device_id = arranged.destination.device.expect("validated above");
            let dev = session.device(device_id).expect("validated above");
            let existing = dev
                .pattern(arranged.destination.slot.slot())
                .and_then(|pat| pat.track(arranged.destination.track));
            PlannedRow {
                part_id: arranged.part_id,
                role: arranged.role,
                destination: arranged.destination,
                device_name: dev.name.clone(),
                slot_label: arranged.destination.slot.label(),
                track_label: format!("T{}", arranged.destination.track + 1),
                track_name: format!("{} {}", genre.label, arranged.role.label().to_lowercase()),
                notes: arranged.notes.len(),
                lanes: arranged.plocks.len(),
                existing_notes: existing.map(|t| t.notes.len()).unwrap_or(0),
                existing_lanes: existing.map(|t| t.plocks.len()).unwrap_or(0),
                warnings: arranged.warnings.clone(),
                arranged,
            }
        })
        .collect();

    Ok(GeneratePlan { rows, pool_warnings })
}

/// Write every row in the plan onto its track, in place. Ordinary session
/// state — `core::history` already covers it as one step, the same argument
/// PLAN.md makes about `safe_write_tracks` needing no registration step of its
/// own.
pub fn apply_plan(session: &mut Session, plan: GeneratePlan) -> usize {
    let mut applied = 0;
    for row in plan.rows {
        let PlannedRow { destination, track_name, arranged, .. } = row;
        let Some(device) = destination.device else { continue };
        let Some(track) = session
            .device_mut(device)
            .and_then(|d| d.pattern_mut(destination.slot.slot()))
            .and_then(|p| p.track_mut(destination.track))
        else {
            continue;
        };
        apply_part_to_track(track, arranged, Some(&track_name));
        applied += 1;
    }
    applied
}

/// A row's music, with the note ids stripped, for asking whether a re-roll
/// actually moved this row. Ids are the one field that must not count: every
/// generate mints fresh ones (`Note::new`), so two runs of identical music
/// compare unequal on the whole struct.
fn music_of(row: &PlannedRow) -> (Vec<Note>, Vec<PLockLane>) {
    let notes = row.arranged.notes.iter().map(|n| Note { id: 0, ..n.clone() }).collect();
    (notes, row.arranged.plocks.clone())
}

/// A destination as the `(device, slot, track)` triple both `plan_generate`'s
/// collision check and `GeneratePanel::written` key on. `None` for a row with
/// no box yet, which no plan ever reaches anyway.
fn destination_key(d: &Destination) -> Option<(DeviceId, usize, usize)> {
    d.device.map(|device| (device, d.slot.slot(), d.track))
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn row_summary(row: &PlannedRow) -> String {
    let mut s = plural(row.notes, "note");
    if row.lanes > 0 {
        s.push_str(&format!(" and {}", plural(row.lanes, "p-lock lane")));
    }
    match row.existing_notes {
        0 => s.push_str(", onto an empty track"),
        n => s.push_str(&format!(", replacing {}", plural(n, "note"))),
    }
    if row.existing_lanes > 0 {
        s.push_str(&format!(" and {}", plural(row.existing_lanes, "existing p-lock lane")));
    }
    s
}

/// A seed for the "unlocked" case — a fresh number each press, not a value
/// anything tests against. `Rng`'s own determinism is what every reproducibility
/// guarantee rests on; this is only ever the *input* to it.
fn random_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    nanos.wrapping_mul(2_654_435_761).max(1)
}

#[derive(Debug)]
enum Status {
    Applied { rows: usize, row_warnings: Vec<String>, pool_warnings: Vec<String> },
    Failed(String),
}

#[derive(Default)]
pub struct GeneratePanel {
    ctx: GenContext,
    confirm: Option<GeneratePlan>,
    status: Option<Status>,
    /// Every destination this panel has already written to, as
    /// `(device, slot, track)`. The one thing a re-roll needs to know that a
    /// plan cannot tell it: whether the track it is about to overwrite holds
    /// this panel's own output or somebody's hand-drawn notes. Re-rolling
    /// your own output is the whole point of the button and asks nothing;
    /// re-rolling over a track this panel has never touched falls back to
    /// the confirm dialog, because "show me another one" is not consent to
    /// replace something you drew. Not persisted, along with the rest of the
    /// panel's state — see the module header's last section.
    written: HashSet<(DeviceId, usize, usize)>,
    /// The title bar's `?` — see `reference_text`. Toggled by
    /// `panel_title_bar` itself, not by this file, per that function's own
    /// contract.
    reference_visible: bool,
}


impl GeneratePanel {
    /// The session was replaced — a project opened, or New pressed. Take the
    /// settings that came with it, or go back to the defaults if it carries
    /// none.
    ///
    /// **Driven by the shell's `reloaded` rather than noticed here.** Both
    /// paths that replace a session (`ui::session`'s `open_from` and `new`)
    /// run while Session is the tool on screen, so this panel is not drawing
    /// when either happens; a panel that only looked while it was open would
    /// adopt a file's settings the first time you happened to click Generate,
    /// which is a load an arbitrary number of edits late.
    pub fn session_replaced(&mut self, session: &Session) {
        // All three describe the session that just left: which tracks this
        // panel had written, a confirm dialog measured against them, and what
        // the last press did.
        self.written.clear();
        self.confirm = None;
        self.status = None;

        self.ctx = match &session.generator {
            None => GenContext::default(),
            Some(value) => match serde_json::from_value::<GenContext>(value.clone()) {
                Ok(ctx) => ctx.adopted(),
                // Settings this build can no longer read cost the settings and
                // nothing else — see `Session::generator`'s own header. Said
                // out loud rather than swallowed, because the alternative is a
                // panel that silently shows Dnb defaults over somebody's saved
                // arrangement and looks like it lost the file.
                Err(e) => {
                    self.status = Some(Status::Failed(format!(
                        "this session's generate settings could not be read ({e}) — \
                         the panel is back to its defaults, and no note was touched"
                    )));
                    GenContext::default()
                }
            },
        };
    }

    /// Put this frame's settings in the session if they moved, and say whether
    /// they did — the panel's [`Outcome::settings`].
    ///
    /// The whole context goes across on any change rather than a field at a
    /// time: it is one value in the file, and a partial mirror would be a
    /// second definition of what "the settings" are.
    fn mirror_into(&mut self, session: &mut Session, before: &GenContext) -> bool {
        if self.ctx == *before {
            return false;
        }
        match serde_json::to_value(&self.ctx) {
            Ok(value) => {
                session.generator = Some(value);
                true
            }
            // A context that will not encode is a bug in this crate, not a
            // state anybody can type their way into, and it must not cost a
            // person the panel they are standing in: the settings simply do not
            // travel, and the next change tries again.
            Err(_) => false,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, session: &mut Session) -> Outcome {
        // Mirrors the Harmony panel's own Root/Scale rather than a second pair
        // — see the module header. The scale always has to be concrete for the
        // generator even though Harmony's own can be "off".
        self.ctx.root = session.harmony.root;
        self.ctx.scale = session.harmony.scale.unwrap_or(DEFAULT_SCALE);

        // Snapshot *after* the mirror above, so moving the Harmony panel's own
        // Root/Scale does not read as an edit to these settings: those two
        // fields are Harmony's, saved in its own place, and copied here every
        // frame either way.
        let before = self.ctx.clone();

        // The v2 title bar carries the key/scale context that used to be a
        // permanent KEY row in the body — see the module header's point 1.
        let context = key_context_string(session);
        let mut out = Outcome {
            close: panel_title_bar(ui, "Generate", &context, &mut self.reference_visible),
            edited: false,
            // Answered at the end of the frame, once every widget has had its
            // say — see `mirror_into`.
            settings: false,
        };

        egui::ScrollArea::vertical()
            .id_salt("generate-panel")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.reference_visible {
                    reference_text(ui);
                    ui.add_space(10.0);
                }
                out.edited |= self.arrangement_group(ui, session);
                ui.add_space(10.0);
                self.feel_group(ui);
                ui.add_space(10.0);
                out.edited |= self.parts_group(ui, session);
                ui.add_space(10.0);
                out.edited |= self.generate_group(ui, session);
            });
        out.settings = self.mirror_into(session, &before);
        out
    }

    /// ARRANGEMENT: genre, bar count, progression, the bpm it suggests, and
    /// the seed every random decision comes from — one section per the v2
    /// layout, where the old panel gave GENRE and SEED a caption each. The
    /// widgets and the logic behind every row are the pre-restyle GENRE and
    /// SEED groups' own, unchanged; only the header they sit under moved.
    fn arrangement_group(&mut self, ui: &mut Ui, session: &mut Session) -> bool {
        section_header(ui, "ARRANGEMENT", None);
        ui.add_space(4.0);
        let mut changed = false;

        let before_genre = self.ctx.genre;
        // **Genre and Bars share one row.** Two full-width rows for two short
        // combo boxes wasted a line of a 330px panel that has parts cards to
        // fit below it, and the pair belong together anyway: they are the two
        // numbers the whole arrangement is sized by. Bars is given an explicit
        // narrow width — its entries are single digits — so the genre name,
        // which is the longer of the two, keeps the room it needs.
        ui.horizontal(|ui| {
            ui.label("Genre");
            egui::ComboBox::from_id_salt("gen-genre")
                .selected_text(genre_profile(self.ctx.genre).label)
                .show_ui(ui, |ui| {
                    for g in GenreId::ALL {
                        ui.selectable_value(&mut self.ctx.genre, g, genre_profile(g).label);
                    }
                });
            // Applied here, between the two combo boxes, rather than after the
            // row: `for_genre` re-defaults the *bar count* as well as the
            // progression, so doing it after Bars had drawn would show the old
            // genre's number for a frame.
            if self.ctx.genre != before_genre {
                // A progression still equal to the old genre's default has not
                // been hand-typed, so it is fair game to re-default; one that
                // differs is the user's and stays.
                let keep = self.ctx.progression != default_progression_for(before_genre);
                self.ctx = self.ctx.for_genre(self.ctx.genre, keep);
            }

            ui.add_space(4.0);
            ui.label("Bars");
            egui::ComboBox::from_id_salt("gen-bars")
                .selected_text(format!("{}", self.ctx.bars))
                .width(BARS_COMBO_WIDTH)
                .show_ui(ui, |ui| {
                    for b in GEN_BARS {
                        ui.selectable_value(&mut self.ctx.bars, b, format!("{b}"));
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.label("Progression");
            // **An explicit width, because `TextEdit`'s default is
            // `f32::INFINITY`.** Left at the default it asked for every pixel
            // available and pushed the panel's `Ui` out with it — which put
            // what sits beside it off the edge, and then, since everything
            // below inherited the inflated width, took FEEL's value boxes and
            // the PARTS cards off the right edge too. The panel is a fixed
            // 330px, so the field takes what is left of this row after the
            // label and the ↻ button rather than asking for the sky.
            let button_w = 26.0;
            let field_w = (ui.available_width() - button_w - ui.spacing().item_spacing.x).max(60.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.ctx.progression).desired_width(field_w),
            )
            .on_hover_text("Roman numerals, e.g. \"i VI III VII\" — a scale degree per chord.");
            // **The ↻ button digi-roll had here.**
            // `progressions::next_progression_for` was ported with "the ↻
            // button" written above it and then never called from anywhere —
            // the library was in the crate and the only way to reach it was to
            // type a progression out by hand. Same mark as a PARTS card's
            // reroll, because it is the same act: show me another one.
            if ui
                .small_button("↻")
                .on_hover_text(
                    "Another progression from this genre's library, in order and wrapping —                      the seed and everything else stay put",
                )
                .clicked()
            {
                self.ctx.progression =
                    next_progression_for(self.ctx.genre, &self.ctx.progression).to_string();
            }
        });
        // What the progression *is*, under the field rather than beside it: the
        // library's own one-line note plus how long it runs, which is
        // digi-roll's caption and the reason the library entries carry a `note`
        // at all. A progression longer than the pattern is genuinely truncated
        // — `theory::bar_slots` takes `bars` bars and stops — so the caption
        // says so rather than leaving "4 bars" beside a 2-bar pattern to be
        // puzzled over.
        match check_progression(&self.ctx.progression) {
            Ok(bars) => {
                let note = progression_note(&self.ctx.progression);
                let mut caption = if note.is_empty() {
                    String::new()
                } else {
                    format!("{note} · ")
                };
                caption.push_str(&plural(bars as usize, "bar"));
                if bars > self.ctx.bars {
                    caption.push_str(&format!(
                        ", truncated to your {}",
                        plural(self.ctx.bars as usize, "bar")
                    ));
                }
                ui.label(
                    egui::RichText::new(caption)
                        .size(10.0)
                        .line_height(Some(13.0))
                        .color(TEXT_DIMMEST),
                );
            }
            Err(e) => {
                ui.colored_label(super::CAUTION, &e.0);
            }
        }

        let suggestion = bpm_suggestion(self.ctx.genre, session.tempo_bpm.round() as u32);
        ui.horizontal(|ui| {
            ui.weak(format!(
                "{} suggests {} bpm ({}-{})",
                genre_profile(self.ctx.genre).label,
                suggestion.bpm,
                suggestion.range.0,
                suggestion.range.1
            ));
            if !suggestion.in_range
                && colored_button(ui, "SET", CYAN_FILL, CYAN_TEXT, CYAN, CYAN, CYAN_INK)
                    .on_hover_text("Set the transport's tempo to this genre's own.")
                    .clicked()
            {
                session.tempo_bpm = f64::from(suggestion.bpm);
                changed = true;
            }
        });

        ui.horizontal(|ui| {
            ui.label("Seed");
            ui.add(egui::DragValue::new(&mut self.ctx.seed));
            // **The other button digi-roll had and this panel lost**: roll a
            // fresh seed *now*, without pressing Generate. Unlocked, Generate
            // rolls one for you at the moment it writes, which is a different
            // thing — this is for keeping the arrangement you have and asking
            // for a different roll of the same settings, and for filling in a
            // number worth locking. It sets the field, so the lock beside it
            // then holds exactly what you can see.
            if ui
                .small_button("↻")
                .on_hover_text("Roll a fresh seed into the field now")
                .clicked()
            {
                self.ctx.seed = random_seed();
            }
            ui.checkbox(&mut self.ctx.seed_locked, "Lock").on_hover_text(
                "Locked, Generate reuses this exact seed — the same settings make the same music \
                 every time. Unlocked, Generate rolls a fresh one first.",
            );
        });
        changed
    }

    /// FEEL: the three sliders every role reads, now in the shared
    /// label→track→value order every v2 side panel's rows share via
    /// `slider_row` — each field is a `u8` and the row wants an `f32`, so a
    /// local copy crosses the bridge and is written back only when the row
    /// itself reports a change.
    fn feel_group(&mut self, ui: &mut Ui) {
        section_header(ui, "FEEL", None);
        ui.add_space(4.0);

        let mut motion = f32::from(self.ctx.feel.motion);
        if slider_row(ui, "Motion", &mut motion, 0.0..=100.0, |v| format!("{v:.0}")) {
            self.ctx.feel.motion = motion.round().clamp(0.0, 100.0) as u8;
        }
        let mut looseness = f32::from(self.ctx.feel.looseness);
        if slider_row(ui, "Looseness", &mut looseness, 0.0..=100.0, |v| format!("{v:.0}")) {
            self.ctx.feel.looseness = looseness.round().clamp(0.0, 100.0) as u8;
        }
        let mut humanize = f32::from(self.ctx.feel.humanize);
        if slider_row(ui, "Humanize", &mut humanize, 0.0..=100.0, |v| format!("{v:.0}")) {
            self.ctx.feel.humanize = humanize.round().clamp(0.0, 100.0) as u8;
        }
    }

    /// PARTS: the row list — add, remove, re-roll one row, and its
    /// box+slot+track destination — as the two-row card the v2 redesign
    /// asks for: name, destination chip, reroll and remove on the first
    /// line, a compact Density/Octave pair on the second. See
    /// [`part_conflicts`] for how a collision reaches the one card that has
    /// it rather than a message at the bottom of the panel.
    fn parts_group(&mut self, ui: &mut Ui, session: &mut Session) -> bool {
        section_header(ui, "PARTS", Some("call and response, top down"));
        ui.add_space(4.0);

        let conflicts = part_conflicts(&self.ctx.parts, session);

        let mut remove: Option<PartId> = None;
        let mut reroll: Option<PartId> = None;
        let ids: Vec<PartId> = self.ctx.parts.iter().map(|p| p.id).collect();

        // **Measured once, before the first card, and handed to every one of
        // them.** A card that overflows pushes the parent `Ui`'s width out,
        // and the next card reads that inflated width as its own — which is
        // why the rows used to walk further off the right edge the further
        // down the list they were, rather than all overflowing alike. Pinning
        // every card to the width the panel had *before* any of them drew
        // means one card's overflow can no longer be the next one's budget.
        let card_w = ui.available_width();

        for id in ids {
            let Some(index) = self.ctx.parts.iter().position(|p| p.id == id) else { continue };
            let conflict = conflicts.get(&id);
            let in_conflict = conflict.is_some();

            egui::Frame::new()
                .fill(if in_conflict { CARD_BG_CONFLICT } else { super::INSET_BG })
                .stroke(egui::Stroke::new(1.0, if in_conflict { WARN_AMBER_BORDER } else { PANEL_BORDER }))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    // The frame's own margin and stroke come out of the width
                    // the card was given; see `card_w` above for why it is not
                    // read from `ui` here.
                    ui.set_max_width(card_w - 8.0 * 2.0 - 2.0);
                    let part = &mut self.ctx.parts[index];

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        ui.checkbox(&mut part.on, "")
                            .on_hover_text("On — this row is written when you press Generate");
                        egui::ComboBox::from_id_salt(("gen-role", id.0))
                            .selected_text(
                                egui::RichText::new(part.role.label()).monospace().size(11.0).color(TEXT_BRIGHT),
                            )
                            .show_ui(ui, |ui| {
                                for role in Role::ALL {
                                    ui.selectable_value(&mut part.role, role, role.label());
                                }
                            });
                        // Right-to-left so the three trailing widgets land in
                        // mock order (chip, reroll, remove) reading forward
                        // from the role name: added here in the reverse of
                        // that order, since `right_to_left` places each new
                        // child to the *left* of the last.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Remove this row").clicked() {
                                remove = Some(id);
                            }
                            if ui
                                .small_button("↻")
                                .on_hover_text(
                                    "Re-roll just this row and whatever answers it below, into \
                                     the session now — the seed and every row above it stay put",
                                )
                                .clicked()
                            {
                                reroll = Some(id);
                            }
                            destination_chip(ui, id, part, session, in_conflict);
                        });
                    });

                    part_row_two(ui, part);

                    if let Some(message) = conflict {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(egui::RichText::new("▲").color(WARN_AMBER));
                            ui.label(
                                egui::RichText::new(message.as_str())
                                    .size(10.5)
                                    .line_height(Some(14.0))
                                    .color(WARN_AMBER),
                            );
                        });
                    } else if let Err(why) = part.destination.validate(session) {
                        ui.colored_label(super::CAUTION, destination_error_message(why));
                    }
                });
            ui.add_space(5.0);
        }

        let mut edited = false;
        if let Some(id) = reroll {
            edited = self.reroll_now(id, session);
        }
        if let Some(id) = remove {
            self.ctx.parts.retain(|p| p.id != id);
        }

        let add_part = ui
            .add(
                egui::Label::new(egui::RichText::new("+ add part…").monospace().size(10.5).color(TEXT_DIMMER))
                    .sense(egui::Sense::click()),
            )
            .on_hover_text("Add another row: its own role, destination, density and octave.");
        if add_part.clicked() {
            self.ctx.parts.push(Part {
                id: PartId::next(),
                role: Role::Bass,
                on: true,
                destination: Destination {
                    device: session.devices.first().map(|d| d.id),
                    slot: PatternRef::new(0, 0),
                    track: 0,
                },
                density: 40,
                octave: 4,
                variation: 0,
            });
        }

        edited
    }

    /// A ↻ press on one row: re-roll it and everything that answers it,
    /// straight into the session.
    ///
    /// **The counter bump on its own was the whole of this button until
    /// now**, and it wrote nothing, drew nothing and said nothing — a press
    /// was indistinguishable from a press on a dead control, and the
    /// tooltip's present tense ("re-roll just this row") promised an action
    /// the code deferred to the next Generate. This is the tooltip's version.
    ///
    /// The apply set is worked out by generating the arrangement twice —
    /// once before the counter moves, once after — and keeping the rows that
    /// are *below or at* the re-rolled one **and** whose music actually came
    /// out different. Both halves are load-bearing:
    ///
    ///   * **Position alone is too wide.** Row order is call-and-response,
    ///     but only the melodic roles answer: a lead nudges off the steps
    ///     the rows above own and chords weight away from them, while every
    ///     drum voice passes `avoid: 0.0` on purpose — a kick under a hi-hat
    ///     is the point, not a collision. So re-rolling the lead of the
    ///     default six leaves the three drum rows generating byte-identical
    ///     music, and writing them anyway would burn a hand-tweaked kick for
    ///     no musical reason and report "4 tracks" when one moved.
    ///   * **Difference alone is too wide.** The rows *above* also change if
    ///     the seed or a slider moved between generates; position is what
    ///     holds the promise that everything above a re-roll stays put.
    ///
    /// Two more things it deliberately does not do:
    ///
    ///   * **Touch the seed**, locked or not, unlike the Generate button.
    ///     A re-roll moves one row's own stream; rolling a fresh seed here
    ///     would move every row, which is the opposite of what a row-level
    ///     button is for.
    ///   * **Skip the dialog for a track it has never written.** See
    ///     [`Self::written`].
    ///
    /// A row that is switched off still counts as re-rolled: `arrange` puts
    /// every part into the busy map regardless of `on`, so the melodic rows
    /// below it move even though the row itself writes nothing. And the
    /// counter is bumped only once both plans have been built — a re-roll
    /// that cannot plan leaves the row exactly where it was rather than
    /// quietly advancing its stream.
    fn reroll_now(&mut self, id: PartId, session: &mut Session) -> bool {
        let Some(from) = self.ctx.parts.iter().position(|p| p.id == id) else { return false };
        let downstream: HashSet<PartId> = self.ctx.parts[from..].iter().map(|p| p.id).collect();

        let before = match plan_generate(&self.ctx, session) {
            Ok(plan) => plan,
            Err(e) => {
                self.status = Some(Status::Failed(e.message()));
                return false;
            }
        };
        self.ctx.bump_variation(id);
        let after = match plan_generate(&self.ctx, session) {
            Ok(plan) => plan,
            Err(e) => {
                self.status = Some(Status::Failed(e.message()));
                return false;
            }
        };

        let was: HashMap<PartId, (Vec<Note>, Vec<PLockLane>)> =
            before.rows.iter().map(|r| (r.part_id, music_of(r))).collect();
        // `pool_warnings` is kept whole rather than filtered: the lane budget
        // it reports was arbitrated across every "on" row aimed at that slot,
        // so it is still the true reason a re-rolled row got fewer lanes than
        // Motion asked for, even when the row that took them is above this one
        // and is not being rewritten.
        let GeneratePlan { rows, pool_warnings } = after;
        let on_below = rows.iter().any(|r| downstream.contains(&r.part_id));
        let rows: Vec<PlannedRow> = rows
            .into_iter()
            .filter(|r| downstream.contains(&r.part_id))
            .filter(|r| was.get(&r.part_id) != Some(&music_of(r)))
            .collect();
        if rows.is_empty() {
            self.status = Some(Status::Failed(if on_below {
                "that re-roll came out the same — press it again, or move a slider first".into()
            } else {
                "nothing to re-roll into — this row and every row below it are switched off".into()
            }));
            return false;
        }
        let plan = GeneratePlan { rows, pool_warnings };

        if plan.rows.iter().any(|r| destination_key(&r.destination).is_none_or(|k| !self.written.contains(&k))) {
            self.confirm = Some(plan);
            return false;
        }

        let row_warnings: Vec<String> = plan.rows.iter().flat_map(|r| r.warnings.clone()).collect();
        let pool_warnings = plan.pool_warnings.clone();
        let rows = self.apply(session, plan);
        self.status = Some(Status::Applied { rows, row_warnings, pool_warnings });
        true
    }

    /// [`apply_plan`], plus the record of where it landed that
    /// [`Self::written`] is. Every path that writes goes through here, so a
    /// track generated by the button and a track generated by a re-roll are
    /// equally "ours" to re-roll again without asking.
    fn apply(&mut self, session: &mut Session, plan: GeneratePlan) -> usize {
        for row in &plan.rows {
            if let Some(key) = destination_key(&row.destination) {
                self.written.insert(key);
            }
        }
        apply_plan(session, plan)
    }

    /// GENERATE: the plan, the button — which counts parts or conflicts per
    /// the mock rather than reading a bare "Generate" — and the confirm
    /// dialog.
    fn generate_group(&mut self, ui: &mut Ui, session: &mut Session) -> bool {
        let conflicts = part_conflicts(&self.ctx.parts, session);
        let plan = plan_generate(&self.ctx, session);

        // Conflicts win the count over any other `PlanError` — see the module
        // header's point 4 — because a conflict is the one failure mode with
        // a card of its own to blame; the rest still get the plain label.
        let (label, enabled) = if !conflicts.is_empty() {
            (format!("GENERATE — {}", plural(conflicts.len(), "conflict").to_uppercase()), false)
        } else {
            match &plan {
                Ok(p) => (format!("GENERATE — {}", plural(p.rows.len(), "part").to_uppercase()), true),
                Err(_) => ("GENERATE".to_string(), false),
            }
        };

        if generate_button(ui, &label, enabled) {
            if !self.ctx.seed_locked {
                self.ctx.seed = random_seed();
            }
            match plan_generate(&self.ctx, session) {
                Ok(plan) => self.confirm = Some(plan),
                Err(e) => self.status = Some(Status::Failed(e.message())),
            }
        }

        // A conflict already names itself inside the card that has it; only
        // the failure modes with no card of their own still need a line here.
        if conflicts.is_empty() {
            if let Err(err) = &plan {
                ui.add_space(4.0);
                ui.colored_label(super::CAUTION, err.message());
            }
        }

        ui.add_space(4.0);
        consequence_line(
            ui,
            "Writes into the tracks in this session. Nothing reaches a box until you SEND in Setup.",
        );

        if let Some(status) = &self.status {
            let (text, colour) = match status {
                Status::Applied { rows, .. } => {
                    (format!("Generated {}", plural(*rows, "track")), super::ACCENT)
                }
                Status::Failed(why) => (why.clone(), super::CAUTION),
            };
            ui.add_space(4.0);
            ui.label(egui::RichText::new(text).small().color(colour));
            if let Status::Applied { row_warnings, pool_warnings, .. } = status {
                for w in row_warnings.iter().chain(pool_warnings.iter()) {
                    ui.colored_label(super::CAUTION, format!("Note: {w}"));
                }
            }
        }

        self.confirm_ui(ui, session)
    }

    fn confirm_ui(&mut self, ui: &mut Ui, session: &mut Session) -> bool {
        let Some(plan) = &self.confirm else { return false };
        let mut apply_now = false;
        let mut cancel = false;

        let response = egui::Modal::new(egui::Id::new("generate-confirm")).show(ui.ctx(), |ui| {
            ui.set_max_width(520.0);
            ui.label(egui::RichText::new(format!("Generate {}?", plural(plan.rows.len(), "track"))).strong());
            ui.separator();

            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for row in &plan.rows {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {} {} — {}",
                            row.device_name, row.slot_label, row.track_label, row.role.label()
                        ))
                        .strong(),
                    );
                    ui.weak(row_summary(row));
                    for w in &row.warnings {
                        ui.indent(("gen-row-warn", row.part_id.0), |ui| {
                            ui.colored_label(super::CAUTION, format!("Note: {w}"));
                        });
                    }
                    ui.add_space(4.0);
                }
                for w in &plan.pool_warnings {
                    ui.colored_label(super::CAUTION, w);
                }
            });

            ui.separator();
            ui.label(
                "Nothing is sent to a box. This only changes the tracks in this window, and undo \
                 (Cmd+Z) brings it straight back.",
            );
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.button(format!("Generate {}", plural(plan.rows.len(), "track"))).clicked() {
                    apply_now = true;
                }
            });
        });
        if response.should_close() {
            cancel = true;
        }

        if cancel {
            self.confirm = None;
            return false;
        }
        if apply_now {
            let plan = self.confirm.take().expect("just matched Some above");
            let row_warnings: Vec<String> = plan.rows.iter().flat_map(|r| r.warnings.clone()).collect();
            let pool_warnings = plan.pool_warnings.clone();
            let rows = self.apply(session, plan);
            self.status = Some(Status::Applied { rows, row_warnings, pool_warnings });
            return true;
        }
        false
    }
}

/// One row's destination: device, then slot and track once a device is
/// chosen. Changing the device resets slot and track — a track on the box you
/// just left behind is not a sensible default for the one you picked.
fn destination_picker(ui: &mut Ui, id: PartId, part: &mut Part, session: &Session) {
    ui.horizontal_wrapped(|ui| {
        let mut device = part.destination.device;
        egui::ComboBox::from_id_salt(("gen-device", id.0))
            .selected_text(
                device
                    .and_then(|d| session.device(d))
                    .map(|d| d.name.as_str())
                    .unwrap_or("— pick a box —"),
            )
            .show_ui(ui, |ui| {
                for dev in &session.devices {
                    ui.selectable_value(&mut device, Some(dev.id), &dev.name);
                }
            });
        if device != part.destination.device {
            part.destination.device = device;
            part.destination.slot = PatternRef::new(0, 0);
            part.destination.track = 0;
        }

        let Some(dev) = device.and_then(|d| session.device(d)) else { return };

        let mut slot = part.destination.slot;
        egui::ComboBox::from_id_salt(("gen-slot", id.0))
            .selected_text(slot.label())
            .show_ui(ui, |ui| {
                for (choice, text) in slot_choices(dev) {
                    ui.selectable_value(&mut slot, choice, text);
                }
            });
        part.destination.slot = slot;

        let mut track = part.destination.track;
        egui::ComboBox::from_id_salt(("gen-track", id.0))
            .selected_text(format!("T{}", track + 1))
            .show_ui(ui, |ui| {
                for t in 0..dev.model.num_tracks {
                    ui.selectable_value(&mut track, t, format!("T{}", t + 1));
                }
            });
        part.destination.track = track;
    });
}

/// The title bar's context string — "C chromatic · from Harmony" — read from
/// `Session::harmony` the same way the pre-v2 KEY row did, just worded for a
/// title bar instead of a permanent line in the body. Pure so it can be
/// tested without a `Ui`.
fn key_context_string(session: &Session) -> String {
    let root = PITCH_CLASSES[usize::from(session.harmony.root % 12)];
    match session.harmony.scale {
        Some(scale) => format!("{root} {} · from Harmony", scale.label()),
        None => format!("{root} chromatic · from Harmony"),
    }
}

/// The destination chip's text — "DN2 A01 T1" — device name, slot label, and
/// a one-indexed track. Pure so [`destination_chip`] and [`part_conflicts`]
/// (which quotes it in a collision message) agree with the test on the same
/// formatting without a `Ui` in the way. A device the session no longer has,
/// or none chosen yet, prints as "—" rather than going blank or panicking —
/// a chip that renders nothing is a worse failure than one that says "not
/// set".
fn destination_chip_label(session: &Session, destination: &Destination) -> String {
    let device = destination.device.and_then(|id| session.device(id)).map(|d| d.name.as_str()).unwrap_or("—");
    format!("{device} {} T{}", destination.slot.label(), destination.track + 1)
}

/// Which "on" rows share another "on" row's exact box+slot+track, keyed by
/// the *later* row's [`PartId`] and worded for that row's own card.
/// `plan_generate`'s `PlanError::Collision` finds the same clash but stops
/// at the first one and names neither side, which was enough for a single
/// bottom-anchored message and is not enough for a card to blame — see the
/// module header's point 3. This walks every "on" row in order, remembering
/// each destination's first claimant, and reports every later row that
/// lands on one already taken.
///
/// A destination that does not resolve against `session` at all is skipped
/// here rather than folded into a false collision: that row already gets its
/// own message from [`destination_error_message`] in the card itself, and a
/// `None` device would otherwise dedupe against every other unresolved row.
fn part_conflicts(parts: &[Part], session: &Session) -> HashMap<PartId, String> {
    let mut claimed: Vec<(DeviceId, usize, usize, Role)> = Vec::new();
    let mut conflicts = HashMap::new();
    for part in parts.iter().filter(|p| p.on) {
        if part.destination.validate(session).is_err() {
            continue;
        }
        let device = part.destination.device.expect("validated above");
        let key = (device, part.destination.slot.slot(), part.destination.track);
        match claimed.iter().find(|&&(d, s, t, _)| (d, s, t) == key) {
            Some(&(.., earlier_role)) => {
                let label = destination_chip_label(session, &part.destination);
                conflicts
                    .insert(part.id, format!("{} is already aimed at {label}. Pick another track.", earlier_role.label()));
            }
            None => claimed.push((key.0, key.1, key.2, part.role)),
        }
    }
    conflicts
}

/// The destination as one chip — "DN2 A01 T1" — replacing what used to be
/// three always-visible combo boxes. See the module header's point 2: this
/// codebase had no existing "click a compact chip to edit its fields"
/// convention, so clicking the chip opens the same three pickers
/// (`destination_picker`, untouched) in a popup rather than a bespoke floating
/// panel. `conflicted` swaps the chip to the warning treatment the offending
/// card also carries.
///
/// **It opens [`super::working_popup`], not `egui::Popup::menu`, and that is the
/// fix for a chip nobody could use.** `Popup::menu` keeps its open state in
/// egui's memory, which holds exactly one open popup per viewport — so the
/// first click *inside*, on the device combo box, closed the popup that combo
/// box was drawn in, and a part with no box could never be given one. (The
/// scene pill in `transport.rs` had the same bug from the same cause.) The
/// chip's own label is non-selectable for a related reason — see
/// [`super::install_style`]; a selectable label ate the click on the chip
/// itself, so half of "no box picker" was never reaching the popup at all.
fn destination_chip(ui: &mut Ui, id: PartId, part: &mut Part, session: &Session, conflicted: bool) {
    let label = destination_chip_label(session, &part.destination);
    let (bg, text_colour) = if conflicted { (WARN_AMBER_FILL, WARN_AMBER_TEXT) } else { (PANEL_BG_RAISED, TEXT_DIM) };

    let response = ui
        .scope_builder(egui::UiBuilder::new().id_salt(("gen-dest-chip", id.0)).sense(egui::Sense::click()), |ui| {
            egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
                ui.label(egui::RichText::new(label).monospace().size(10.0).color(text_colour));
            });
        })
        .response
        .on_hover_text("Click to change the box, slot or track this part writes to.");

    super::working_popup(&response, 200.0, |ui| destination_picker(ui, id, part, session));
}

/// PARTS row 2: Density and, for a melodic role, Octave — compact enough to
/// share one line, at the mock's own tighter widths (48px label, 22px value
/// with no filled background, then "Oct" and its own 14px value) rather than
/// a second [`slider_row`], which is sized for a full-width row with a
/// filled value box. A drum voice — [`Role::is_drum_voice`] — has no octave,
/// so its two trailing slots are left blank rather than let the row grow to
/// fill them: that is what keeps every card's Density track the same width
/// whether or not that row has an Oct to show.
fn part_row_two(ui: &mut Ui, part: &mut Part) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        ui.add_sized(egui::vec2(48.0, 0.0), egui::Label::new(egui::RichText::new("Density").size(10.5).color(TEXT_DIM)));

        let row_h = 14.0;
        let value_w = 22.0;
        let oct_label_w = 34.0;
        // The mock's own figure is 14px, which is right for the bare numeral
        // it draws and wrong for a `DragValue`: egui gives one button padding
        // on both sides, so at 14 the digit was clipped out entirely and the
        // row read "Oct" with nothing after it. 24 is the numeral plus that
        // padding; the density track absorbs the ten pixels.
        let oct_value_w = 24.0;
        let gap = ui.spacing().item_spacing.x;
        let track_w = (ui.available_width() - value_w - oct_label_w - oct_value_w - gap * 3.0).max(20.0);

        let (rect, response) = ui.allocate_exact_size(egui::vec2(track_w, row_h), egui::Sense::click_and_drag());
        if response.dragged() || response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let fraction = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
                part.density = (fraction * 100.0).round() as u8;
            }
        }
        let painter = ui.painter_at(rect);
        let mid_y = rect.center().y;
        let track_rect = egui::Rect::from_min_size(egui::pos2(rect.left(), mid_y - 1.5), egui::vec2(rect.width(), 3.0));
        painter.rect_filled(track_rect, 0.0, PANEL_BORDER);
        let fill_w = rect.width() * (f32::from(part.density) / 100.0);
        if fill_w > 0.0 {
            painter.rect_filled(egui::Rect::from_min_size(track_rect.min, egui::vec2(fill_w, 3.0)), 0.0, CYAN);
        }

        ui.add_sized(
            egui::vec2(value_w, row_h),
            egui::Label::new(egui::RichText::new(format!("{}", part.density)).monospace().size(10.0).color(TEXT_SECONDARY)),
        );

        if part.role.is_drum_voice() {
            ui.add_space(oct_label_w + gap + oct_value_w);
        } else {
            ui.add_sized(
                egui::vec2(oct_label_w, row_h),
                egui::Label::new(egui::RichText::new("Oct").size(10.5).color(TEXT_DIM)),
            );
            // Allocated and clipped rather than `add_sized`, for the reason
            // `slider_row`'s value box is: `add_sized` is a minimum, and a
            // `DragValue` that outgrows it takes the row over the card's edge.
            let (oct_rect, _) = ui.allocate_exact_size(egui::vec2(oct_value_w, row_h), egui::Sense::hover());
            let mut oct_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(oct_rect)
                    .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            oct_ui.set_clip_rect(oct_rect.intersect(ui.clip_rect()));
            // A `DragValue` with every fill and stroke stripped, so it reads
            // as the plain right-aligned number the mock draws rather than a
            // boxed field — the same "restyle a stock widget's per-state
            // visuals" trick `slider_row`'s own value box and `colored_button`
            // use, just emptied out instead of recoloured.
            let widgets = &mut oct_ui.style_mut().visuals.widgets;
            for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
                state.weak_bg_fill = Color32::TRANSPARENT;
                state.bg_fill = Color32::TRANSPARENT;
                state.bg_stroke = egui::Stroke::NONE;
                state.fg_stroke = egui::Stroke::new(1.0, TEXT_SECONDARY);
            }
            oct_ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
            oct_ui
                .add(egui::DragValue::new(&mut part.octave).range(1..=7))
                .on_hover_text("The register window this part plays in.");
        }
    });
}

/// The full-width Generate button, styled per state the way [`colored_button`]
/// styles its own — duplicated locally rather than widening that shared
/// function's signature for one caller: every other `colored_button` call in
/// this codebase is happy content-sized, and this is the one button in the
/// panel the mock draws edge to edge.
fn generate_button(ui: &mut Ui, text: &str, enabled: bool) -> bool {
    let width = ui.available_width();
    let rich = egui::RichText::new(text).monospace().size(11.0);
    if enabled {
        ui.scope(|ui| {
            let widgets = &mut ui.style_mut().visuals.widgets;
            for state in [&mut widgets.inactive, &mut widgets.active] {
                state.weak_bg_fill = CYAN_FILL;
                state.bg_fill = CYAN_FILL;
                state.fg_stroke = egui::Stroke::new(1.0, CYAN_TEXT);
                state.bg_stroke = egui::Stroke::new(1.0, CYAN);
            }
            widgets.hovered.weak_bg_fill = CYAN;
            widgets.hovered.bg_fill = CYAN;
            widgets.hovered.fg_stroke = egui::Stroke::new(1.0, CYAN_INK);
            widgets.hovered.bg_stroke = egui::Stroke::new(1.0, CYAN);
            ui.add(egui::Button::new(rich).min_size(egui::vec2(width, 24.0)))
        })
        .inner
        .clicked()
    } else {
        ui.add_enabled(
            false,
            egui::Button::new(rich.color(TEXT_DIMMER))
                .fill(super::INSET_BG)
                .stroke(egui::Stroke::new(1.0, PANEL_BORDER))
                .min_size(egui::vec2(width, 24.0)),
        )
        .clicked()
    }
}

/// The `?` reference block: everything this panel used to say permanently in
/// its body, per 2b rule 1 and rule 2 — moved here rather than deleted, since
/// the explanations are still true, just no longer owed a permanent row
/// among the controls.
fn reference_text(ui: &mut Ui) {
    ui.label(
        egui::RichText::new(
            "KEY is set in the Harmony panel, not here — this generator reads it rather than \
             inventing its own, so it always agrees with what the roll tints.\n\n\
             SEED: every random decision in the arrangement comes from this one number. Locked, \
             Generate reuses it exactly; unlocked, Generate rolls a fresh one first. Nudging a \
             slider with the seed locked re-rolls only what that slider actually changed.\n\n\
             MOTION: how many p-lock lanes each part draws, and how far they travel — 0 is none \
             at all.\nLOOSENESS: PROB and trig conditions on top of the notes written — 0 writes \
             none at all, every trig fires every time round.\nHUMANIZE: velocity and micro-timing \
             wobble away from the genre's own groove.\n\n\
             PARTS: one row per track this writes. Row order is call-and-response — whatever \
             comes after a row in this list answers it. Reroll rewrites just that row and \
             whatever answers it below, straight into the session — no Generate press needed, \
             and the seed and every row above it stay put.\n\n\
             LEAD (CALL) / LEAD (RESPONSE): a pair of rows that trade phrases across two \
             tracks. The call states an idea and then rests; the response answers into the \
             space, quoting, inverting or resolving what the call just played. Put the call \
             above the response — a response looks upward for the nearest call to answer. How \
             long a turn lasts follows the pattern: one bar trades at the half bar, two and \
             four bars trade bar for bar, eight bars trade two bars at a time.",
        )
        .weak()
        .small(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::model::Note;
    use digi_core::project::Project;
    use digi_generator::context::Feel;

    fn session_with_two_boxes() -> Session {
        digi_core::two_box_session()
    }

    /// `GenContext::default()`'s six rows, aimed at a real box — the bare
    /// default leaves every row's device as `None`, which is the state a
    /// freshly-added row starts in when there is no session to default it
    /// from (see `context::new_part`), not a state a plan can ever accept.
    fn default_ctx_on(session: &Session) -> GenContext {
        let mut ctx = GenContext::default();
        for part in &mut ctx.parts {
            part.destination.device = Some(session.devices[0].id);
        }
        ctx
    }

    fn aimed(session: &Session, index: usize, role: Role, track: usize) -> Part {
        Part {
            id: PartId::next(),
            role,
            on: true,
            destination: Destination {
                device: Some(session.devices[index].id),
                slot: PatternRef::new(0, 0),
                track,
            },
            density: 50,
            octave: 4,
            variation: 0,
        }
    }

    /// Notes as plain comparable data — `Note` carries an id that differs
    /// between two generates of the same music, so the ids are dropped and
    /// only what the roll draws is compared.
    fn shape(session: &Session, index: usize, track: usize) -> Vec<(f64, u8, f64, u8)> {
        session.devices[index]
            .pattern(0)
            .and_then(|p| p.track(track))
            .map(|t| t.notes.iter().map(|n| (n.step, n.pitch, n.len, n.velocity)).collect())
            .unwrap_or_default()
    }

    /// A panel whose six default rows are aimed at a real box and have
    /// already been generated once, which is the state a re-roll is for.
    fn generated_panel(session: &mut Session) -> GeneratePanel {
        let mut panel = GeneratePanel { ctx: default_ctx_on(session), ..GeneratePanel::default() };
        let plan = plan_generate(&panel.ctx, session).expect("the default six should plan");
        panel.apply(session, plan);
        panel
    }

    #[test]
    fn a_reroll_rewrites_its_own_row_and_everything_below_it_and_nothing_above() {
        let mut session = session_with_two_boxes();
        let mut panel = generated_panel(&mut session);

        // Row 2 of six: bass and chords are above it, lead and the three drum
        // voices below.
        let before: Vec<Vec<_>> = (0..6).map(|t| shape(&session, 0, t)).collect();
        let lead = panel.ctx.parts[2].id;
        assert!(panel.reroll_now(lead, &mut session), "a re-roll of generated tracks should apply");

        for track in [0, 1] {
            assert_eq!(shape(&session, 0, track), before[track], "row above the re-roll moved");
        }
        assert_ne!(shape(&session, 0, 2), before[2], "the re-rolled row did not change");
        // The three drum rows are below the lead but answer nothing — every
        // voice generates with `avoid: 0.0` — so a lead re-roll must leave
        // them alone rather than rewrite them with identical music.
        // Indexed rather than zipped on purpose: `track` is the track *number*,
        // it is what the assertion message has to name, and `before` is indexed
        // by the same number. Zipping would rename the one thing being asserted.
        #[allow(clippy::needless_range_loop)]
        for track in 3..6 {
            assert_eq!(shape(&session, 0, track), before[track], "a drum row was rewritten for nothing");
        }
        assert!(
            matches!(&panel.status, Some(Status::Applied { rows: 1, .. })),
            "it should report the one track it actually moved, got {:?}",
            panel.status.as_ref().map(|s| matches!(s, Status::Applied { .. }))
        );
    }

    #[test]
    fn a_reroll_of_the_top_row_can_carry_down_to_the_melodic_rows_that_answer_it() {
        // **Across seeds, not on one.** Chords weight away from the steps the
        // bass owns (`avoid: 0.15`) and the lead nudges off them, but both
        // only move when the re-rolled bass actually lands somewhere it
        // didn't before — a gentle weighting re-picking the same step is the
        // common case, not a bug. Pinning one seed here failed depending on
        // which other tests had run first, because `PartId::next()` is a
        // process-global counter and the part id is part of the RNG stream
        // tag. So the claim under test is that the threading is live at all,
        // which is what the position half of the apply-set filter exists to
        // carry.
        let carried = (0..12u32).any(|seed| {
            let mut session = session_with_two_boxes();
            let mut panel = generated_panel(&mut session);
            panel.ctx.seed = seed;
            panel.ctx.seed_locked = true;
            // Re-generate at this seed, so `before` is the tracks' real state.
            let plan = plan_generate(&panel.ctx, &session).expect("the default six should plan");
            panel.apply(&mut session, plan);
            let before: Vec<Vec<_>> = (0..3).map(|t| shape(&session, 0, t)).collect();

            let bass = panel.ctx.parts[0].id;
            if !panel.reroll_now(bass, &mut session) {
                return false;
            }
            assert_ne!(shape(&session, 0, 0), before[0], "seed {seed}: the re-rolled row did not change");
            (1..3).any(|t| shape(&session, 0, t) != before[t])
        });
        assert!(carried, "no seed in 0..12 saw a melodic row answer a bass re-roll");
    }

    #[test]
    fn a_reroll_asks_first_before_overwriting_a_track_this_panel_never_wrote() {
        let mut session = session_with_two_boxes();
        // Never generated: `written` is empty, so every destination is
        // somebody else's until proven otherwise.
        let mut panel = GeneratePanel { ctx: default_ctx_on(&session), ..GeneratePanel::default() };
        let before: Vec<Vec<_>> = (0..6).map(|t| shape(&session, 0, t)).collect();

        let bass = panel.ctx.parts[0].id;
        assert!(!panel.reroll_now(bass, &mut session), "it must not report an edit it did not make");
        assert!(panel.confirm.is_some(), "it should have raised the confirm dialog");
        // Indexed for the reason the other loop in this file is: `track` is the
        // number the failure message has to print.
        #[allow(clippy::needless_range_loop)]
        for track in 0..6 {
            assert_eq!(shape(&session, 0, track), before[track], "a track was written without asking");
        }
    }

    #[test]
    fn a_reroll_bumps_only_its_own_rows_variation_and_never_the_seed() {
        let mut session = session_with_two_boxes();
        let mut panel = generated_panel(&mut session);
        panel.ctx.seed_locked = false; // the case Generate would re-roll the seed in
        let seed = panel.ctx.seed;

        let chords = panel.ctx.parts[1].id;
        panel.reroll_now(chords, &mut session);

        assert_eq!(panel.ctx.seed, seed, "a re-roll moved the seed");
        assert_eq!(panel.ctx.parts[1].variation, 1);
        for (i, part) in panel.ctx.parts.iter().enumerate() {
            if i != 1 {
                assert_eq!(part.variation, 0, "row {i} was re-rolled too");
            }
        }
    }

    #[test]
    fn a_reroll_with_nothing_on_below_it_says_so_rather_than_reporting_an_edit() {
        let mut session = session_with_two_boxes();
        let mut panel = generated_panel(&mut session);
        // The last row switched off, the rest left on — so the plan itself
        // still succeeds (`PlanError::NothingOn` would mask this) and the
        // only reason there is nothing to write is that the apply set is
        // empty. Silence here is exactly the dead-button feel this change
        // exists to remove.
        panel.ctx.parts.last_mut().expect("six rows").on = false;
        let last = panel.ctx.parts.last().expect("six rows").id;

        assert!(!panel.reroll_now(last, &mut session));
        assert!(
            matches!(&panel.status, Some(Status::Failed(why)) if why.contains("switched off")),
            "expected a plain explanation, got {:?}",
            matches!(&panel.status, Some(Status::Failed(_)))
        );
    }

    #[test]
    fn a_plan_refuses_when_nothing_is_on() {
        let session = session_with_two_boxes();
        let mut ctx = GenContext::default();
        for p in &mut ctx.parts {
            p.on = false;
        }
        assert_eq!(plan_generate(&ctx, &session).err(), Some(PlanError::NothingOn));
    }

    #[test]
    fn a_plan_refuses_an_unvalidated_destination() {
        let session = session_with_two_boxes();
        let mut ctx = GenContext::default();
        ctx.parts[0].destination.device = None;
        let err = plan_generate(&ctx, &session).err().expect("expected a refusal");
        assert!(matches!(err, PlanError::Destination { .. }), "{err:?}");
        assert!(err.message().contains("pick a box"), "{}", err.message());
    }

    #[test]
    fn a_plan_refuses_two_on_rows_aimed_at_the_same_track() {
        let session = session_with_two_boxes();
        let ctx = GenContext {
            parts: vec![
                aimed(&session, 0, Role::Bass, 0),
                aimed(&session, 0, Role::Chords, 0),
            ],
            ..GenContext::default()
        };
        let err = plan_generate(&ctx, &session).err().expect("expected a refusal");
        assert!(matches!(err, PlanError::Collision(_)), "{err:?}");
    }

    #[test]
    fn the_default_parts_are_conflict_free_out_of_the_box() {
        // Six rows now land on tracks 0..=5 in a row — `default_parts`'s own
        // doc comment promises this, and `part_conflicts` is what a real
        // session would use to notice two rows aimed at the same box+slot+
        // track. A regression here (say, two default rows sharing a track)
        // would otherwise only surface as an amber card the first time
        // anyone opened the panel.
        let session = session_with_two_boxes();
        let ctx = default_ctx_on(&session);
        let conflicts = part_conflicts(&ctx.parts, &session);
        assert!(conflicts.is_empty(), "default parts collided: {conflicts:?}");
        // `plan_generate` runs the same collision check on the way to a plan;
        // this is the version a real Generate press would hit.
        assert!(plan_generate(&ctx, &session).is_ok());
    }

    #[test]
    fn the_default_parts_plan_and_apply_for_every_genre() {
        // The Generate-panel-level version of the every-genre check: not
        // just "does `resolve_context` succeed" (covered in `generator::
        // context`'s own tests) but "does a real plan build and apply from
        // the panel's own default rows", for every genre someone could
        // switch to without touching a single row.
        for genre in GenreId::ALL {
            let mut session = session_with_two_boxes();
            let mut ctx = default_ctx_on(&session);
            ctx = ctx.for_genre(genre, false);
            let plan = plan_generate(&ctx, &session)
                .unwrap_or_else(|e| panic!("{genre:?}: default parts failed to plan: {}", e.message()));
            assert_eq!(plan.rows.len(), 6, "{genre:?}: expected all six default rows");
            let applied = apply_plan(&mut session, plan);
            assert_eq!(applied, 6, "{genre:?}");
        }
    }

    #[test]
    fn a_plan_names_every_on_row_and_skips_the_off_ones() {
        let session = session_with_two_boxes();
        let mut ctx = default_ctx_on(&session);
        ctx.parts[1].on = false; // the chords row
        let plan = plan_generate(&ctx, &session).expect("a default context always resolves");
        assert_eq!(plan.rows.len(), 5, "one of six default rows switched off");
        assert!(plan.rows.iter().all(|r| r.notes > 0));
    }

    #[test]
    fn applying_a_plan_writes_the_named_tracks_and_leaves_the_rest_alone() {
        let mut session = session_with_two_boxes();
        let ctx = default_ctx_on(&session);
        let plan = plan_generate(&ctx, &session).unwrap();
        let expected_notes: Vec<usize> = plan.rows.iter().map(|r| r.notes).collect();
        let applied = apply_plan(&mut session, plan);
        assert_eq!(applied, 6, "all six default rows: bass/chords/lead + kick/snare/closed hat");

        let device = session.devices[0].id;
        let pattern = session.device(device).unwrap().pattern(0).unwrap();
        for (row, expected) in ctx.parts.iter().zip(&expected_notes) {
            let track = pattern.track(row.destination.track).unwrap();
            assert_eq!(track.notes.len(), *expected);
            assert!(track.name.contains(row.role.label().to_lowercase().as_str()) || !track.name.is_empty());
        }
        // A track no row named is untouched — the default rows only claim
        // tracks 0..=5.
        assert!(pattern.track(15).unwrap().notes.is_empty());
    }

    #[test]
    fn each_rows_lanes_are_designed_for_its_own_box_not_a_shared_guess() {
        // Aim bass at the DT2 and lead at the DN2 — regressing to a single
        // assumed device_kind for the whole arrangement would give one of
        // these rows the other box's parameter numbering.
        let session = session_with_two_boxes();
        let mut ctx = GenContext::default();
        ctx.feel.motion = 100;
        ctx.parts = vec![
            aimed(&session, 0, Role::Bass, 0),
            aimed(&session, 1, Role::Lead, 2),
        ];
        let plan = plan_generate(&ctx, &session).unwrap();
        let dt2_row = plan.rows.iter().find(|r| r.role == Role::Bass).unwrap();
        let dn2_row = plan.rows.iter().find(|r| r.role == Role::Lead).unwrap();
        assert!(dt2_row.arranged.plocks.iter().all(|l| l.device_kind.as_deref() == Some("DT2")));
        assert!(dn2_row.arranged.plocks.iter().all(|l| l.device_kind.as_deref() == Some("DN2")));
    }

    /// The two digis and an Analog Four, which is `devices[2]`.
    fn session_with_an_a4() -> Session {
        let mut session = session_with_two_boxes();
        session.add_device(digi_core::device::Device::new("A4", &digi_core::device::A4, 8));
        session
    }

    /// **The A4 had no lanes for a day, and the reason was a `Spec` lookup.**
    /// `device_kind_of` asked `model.spec()`, which is `None` on this box and
    /// always will be, so every A4 row came back with the "can't tell which box
    /// this is for" warning and no lanes — while the roll's own "+ add lane…"
    /// menu, fixed on 2026-09-01, offered the same thirteen parameters happily.
    #[test]
    fn an_a4_row_gets_a4_lanes_rather_than_the_no_box_warning() {
        let session = session_with_an_a4();
        let mut ctx = GenContext::default();
        ctx.feel.motion = 100;
        ctx.parts = vec![aimed(&session, 2, Role::Bass, 0)];
        let plan = plan_generate(&ctx, &session).unwrap();
        let row = &plan.rows[0];
        assert!(row.lanes > 0, "an A4 synth track draws lanes: {:?}", row.warnings);
        assert!(row.arranged.plocks.iter().all(|l| l.device_kind.as_deref() == Some("A4")));
        assert!(!row.warnings.iter().any(|w| w == NO_BOX_WARNING), "{:?}", row.warnings);
        // Every lane the design picked is one the write path can actually put
        // in the pool — a lane named but not measured would be refused by
        // `a4_transfer::a4_lanes_for_write` after the panel promised it.
        let (writes, warnings) = digi_core::a4_transfer::a4_lanes_for_write(&row.arranged.plocks);
        assert_eq!(writes.len(), row.arranged.plocks.len(), "{warnings:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// The FX and CV tracks number their p-lock parameters differently from
    /// SYN1–SYN4 — see [`A4_SYNTH_TRACKS`]. Designing a synth lane for one of
    /// them would aim a measured id at the wrong knob, so the row keeps its
    /// notes and says why the lanes stopped.
    #[test]
    fn the_a4s_fx_track_gets_no_lanes_and_says_why() {
        let session = session_with_an_a4();
        let mut ctx = GenContext::default();
        ctx.feel.motion = 100;
        ctx.parts = vec![aimed(&session, 2, Role::Bass, 4)];
        let plan = plan_generate(&ctx, &session).unwrap();
        let row = &plan.rows[0];
        assert!(row.notes > 0, "the notes still go");
        assert_eq!(row.lanes, 0);
        assert!(row.warnings.iter().any(|w| w == A4_NON_SYNTH_WARNING), "{:?}", row.warnings);
        assert!(!row.warnings.iter().any(|w| w == NO_BOX_WARNING), "{:?}", row.warnings);
    }

    #[test]
    fn the_seed_still_makes_a_plan_reproducible_through_this_layer() {
        let session = session_with_two_boxes();
        let mut ctx = GenContext { seed: 42, ..GenContext::default() };
        for (i, p) in ctx.parts.iter_mut().enumerate() {
            p.destination.device = Some(session.devices[0].id);
            p.destination.track = i;
        }
        let a = plan_generate(&ctx, &session).unwrap();
        let b = plan_generate(&ctx, &session).unwrap();
        for (ra, rb) in a.rows.iter().zip(&b.rows) {
            assert_eq!(ra.notes, rb.notes);
            assert_eq!(ra.lanes, rb.lanes);
        }
    }

    #[test]
    fn the_pool_is_capped_across_rows_sharing_one_slot_and_the_cut_is_named() {
        let mut session = session_with_two_boxes();
        let device = session.devices[0].id;
        // Ten of the slot's eighty lanes are already spoken for by a track
        // this generate does not touch, leaving seventy — and four rows at
        // Motion 100 (three recipes each, all of them writable on a DT2) ask
        // for twelve between them, comfortably inside that. So instead the
        // reservation is set to leave only five: four rows wanting up to
        // twelve cannot all fit, and the ones that lose out have to be named.
        {
            let track = session.device_mut(device).unwrap().pattern_mut(0).unwrap().track_mut(15).unwrap();
            track.notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
            track.plocks = (0..75)
                .map(|_| {
                    digi_core::model::PLockLane::new(
                        None,
                        Some(1),
                        Some("DT2".to_string()),
                        false,
                        vec![Some(64); digi_core::model::PLOCK_STEPS],
                    )
                    .unwrap()
                })
                .collect();
        }
        let mut ctx = GenContext::default();
        ctx.feel.motion = 100;
        ctx.parts = (0..4)
            .map(|t| aimed(&session, 0, [Role::Bass, Role::Chords, Role::Lead, Role::Bass][t], t))
            .collect();
        let plan = plan_generate(&ctx, &session).unwrap();
        let total: usize = plan.rows.iter().map(|r| r.lanes).sum();
        assert!(total <= 5, "total lanes {total} exceeded the five left in the pool");
        assert!(!plan.pool_warnings.is_empty(), "capping something should say so");
    }

    #[test]
    fn the_pool_leaves_room_already_spoken_for_by_an_untouched_track() {
        let mut session = session_with_two_boxes();
        let device = session.devices[0].id;
        // Track 5 is not touched by this generate and already holds 75 lanes —
        // only 5 are left for whatever this run wants to write elsewhere in
        // the same slot.
        {
            let track = session.device_mut(device).unwrap().pattern_mut(0).unwrap().track_mut(4).unwrap();
            track.notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
            track.plocks = (0..75)
                .map(|_| {
                    digi_core::model::PLockLane::new(
                        None,
                        Some(1),
                        Some("DT2".to_string()),
                        false,
                        vec![Some(64); digi_core::model::PLOCK_STEPS],
                    )
                    .unwrap()
                })
                .collect();
        }
        let mut ctx = GenContext::default();
        ctx.feel.motion = 100;
        ctx.parts = vec![aimed(&session, 0, Role::Lead, 2)];
        let plan = plan_generate(&ctx, &session).unwrap();
        assert!(plan.rows[0].lanes <= 5, "only 5 of the 80 lanes were free: got {}", plan.rows[0].lanes);
    }

    #[test]
    fn an_add_and_a_remove_do_not_disturb_the_other_rows_identity() {
        let session = session_with_two_boxes();
        let mut panel = GeneratePanel::default();
        let bass_id = panel.ctx.parts[0].id;
        panel.ctx.parts.push(Part {
            id: PartId::next(),
            role: Role::Kick,
            on: true,
            destination: Destination {
                device: session.devices.first().map(|d| d.id),
                slot: PatternRef::new(0, 0),
                track: 3,
            },
            density: 40,
            octave: 4,
            variation: 0,
        });
        panel.ctx.parts.remove(1); // drop the chords row
        assert_eq!(panel.ctx.parts[0].id, bass_id);
    }

    /// A real headless pass through `GeneratePanel::ui`, the way `ui::tracks`'
    /// own tests drive a frame: the logic tests above never call a single egui
    /// widget, so a duplicate `Id` between two rows' combo boxes — the class of
    /// bug the tuple-keyed `from_id_salt` calls in this file are exactly built
    /// to avoid — would pass every one of them and only show up here. Drives
    /// three frames: draw, press Generate, draw the confirm modal, press it,
    /// and confirm the session actually changed.
    /// Where a frame painted `label`, straight out of the paint list.
    ///
    /// The sibling tests in this file press their buttons by reaching into
    /// `panel.confirm` directly, on the argument that they are after an egui Id
    /// collision rather than a hitbox. This one is the opposite: the whole claim
    /// is that *pressing SET* moves the tempo, so it presses SET — and the paint
    /// list is how you find a button whose position depends on the width of the
    /// sentence in front of it.
    fn painted_at(ctx: &egui::Context, label: &str, mut body: impl FnMut(&mut Ui)) -> egui::Pos2 {
        let input = egui::RawInput::default();
        let mut output = ctx.run_ui(input, &mut body);
        output.textures_delta.clear();

        fn find(shape: &egui::Shape, label: &str, found: &mut Option<egui::Pos2>) {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    *found = Some(text.pos + text.galley.size() / 2.0);
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| find(s, label, found)),
                _ => {}
            }
        }
        let mut found = None;
        for clipped in &output.shapes {
            find(&clipped.shape, label, &mut found);
        }
        found.unwrap_or_else(|| panic!("nothing in this frame painted {label:?}"))
    }

    #[test]
    fn pressing_set_moves_the_tempo_and_says_the_session_changed() {
        // Neil, 2026-08-20: the transport read 174 and the boxes played 120.
        //
        // The engine end of that is fixed and pinned in `digi_engine` — a
        // `Snapshot` now carries the session's tempo instead of every field but
        // that one. This is the app end of the same round trip, and it is the
        // link that fix now rests on: SET has to write the tempo **and** report
        // `edited`, because `edited` is what makes `main` hand the engine a
        // snapshot at all. A SET that quietly stopped reporting would put the
        // symptom straight back with the engine still correct.
        let ctx = egui::Context::default();
        let mut session = session_with_two_boxes();
        session.tempo_bpm = 120.0;
        let mut panel = GeneratePanel::default();
        // 120 is inside House's own range, so the button only exists on a genre
        // that disagrees with the tempo — which is the only time it is offered.
        panel.ctx.genre = GenreId::Dnb;
        for p in &mut panel.ctx.parts {
            p.destination.device = Some(session.devices[0].id);
        }

        let mut edited = false;
        let mut draw = |ui: &mut Ui| {
            edited |= panel.ui(ui, &mut session).edited;
        };
        // egui hit-tests against the previous pass's layout, so the button has
        // to have been drawn once before a press can land on it.
        let set = painted_at(&ctx, "SET", &mut draw);
        for events in [
            vec![
                egui::Event::PointerMoved(set),
                egui::Event::PointerButton {
                    pos: set,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            vec![egui::Event::PointerButton {
                pos: set,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ] {
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, &mut draw);
            output.textures_delta.clear();
        }

        assert_eq!(session.tempo_bpm, 174.0, "SET sets the transport to the genre's own tempo");
        assert!(edited, "and says so, which is what gets the new tempo to the engine");
    }

    /// Every position a frame painted `label`, in paint order. The panel draws
    /// three ↻ buttons — progression, seed, and one per PARTS card — so
    /// `painted_at`'s "the last one wins" is not enough to press a chosen one.
    fn painted_all(ctx: &egui::Context, label: &str, mut body: impl FnMut(&mut Ui)) -> Vec<egui::Pos2> {
        let input = egui::RawInput::default();
        let mut output = ctx.run_ui(input, &mut body);
        output.textures_delta.clear();

        fn walk(shape: &egui::Shape, label: &str, found: &mut Vec<egui::Pos2>) {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    found.push(text.pos + text.galley.size() / 2.0);
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, label, found)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, label, &mut found);
        }
        found
    }

    /// Press at `pos`: the two-frame press/release egui needs, hit-testing
    /// against the layout the caller has already drawn.
    fn press(ctx: &egui::Context, pos: egui::Pos2, mut body: impl FnMut(&mut Ui)) {
        for events in [
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ] {
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, &mut body);
            output.textures_delta.clear();
        }
    }

    #[test]
    fn the_progression_reroll_button_walks_the_genres_library() {
        // digi-roll had this button and the port lost it:
        // `progressions::next_progression_for` was written with "the ↻ button"
        // in its own doc comment and called from nowhere, so the library was
        // unreachable without typing an entry out by hand.
        let ctx = egui::Context::default();
        let mut session = session_with_two_boxes();
        let mut panel = GeneratePanel::default();
        panel.ctx.genre = GenreId::Dnb;
        panel.ctx.progression = default_progression_for(GenreId::Dnb).to_string();
        let first = panel.ctx.progression.clone();

        let mut draw = |ui: &mut Ui| {
            panel.ui(ui, &mut session);
        };
        // The progression's is the first ↻ the panel paints; the seed's is the
        // second and the PARTS cards' come after.
        let buttons = painted_all(&ctx, "↻", &mut draw);
        assert!(buttons.len() >= 2, "the progression and seed buttons both drew");
        press(&ctx, buttons[0], &mut draw);

        let second = panel.ctx.progression.clone();
        assert_ne!(second, first, "the field moved on to another entry");
        assert_eq!(second, next_progression_for(GenreId::Dnb, &first));
        assert!(
            check_progression(&second).is_ok(),
            "and what it landed on parses, which is what the library guarantees"
        );
    }

    #[test]
    fn the_seed_button_rolls_a_fresh_seed_into_the_field() {
        let ctx = egui::Context::default();
        let mut session = session_with_two_boxes();
        let mut panel = GeneratePanel::default();
        panel.ctx.seed = 0;
        panel.ctx.seed_locked = true;

        let mut draw = |ui: &mut Ui| {
            panel.ui(ui, &mut session);
        };
        let buttons = painted_all(&ctx, "↻", &mut draw);
        press(&ctx, buttons[1], &mut draw);

        assert_ne!(panel.ctx.seed, 0, "a number went into the field");
        assert!(
            panel.ctx.seed_locked,
            "and the lock is untouched — this fills the field the lock then holds"
        );
    }

    #[test]
    fn the_progression_caption_says_what_it_is_and_how_long_it_runs() {
        // The library carries a one-line note per entry for this caption and
        // nothing else, and a progression longer than the pattern really is cut
        // short — `theory::bar_slots` takes `bars` bars and stops — so the
        // caption says so rather than printing "4 bars" beside a 2-bar pattern.
        let ctx = egui::Context::default();
        let mut session = session_with_two_boxes();
        let mut panel = GeneratePanel::default();
        panel.ctx.genre = GenreId::Dnb;
        panel.ctx.progression = String::from("i VI III VII");
        panel.ctx.bars = 2;

        let note = progression_note("i VI III VII");
        assert!(!note.is_empty(), "the entry has a note to draw");
        let wanted = format!("{note} · 4 bars, truncated to your 2 bars");
        assert_eq!(
            painted_all(&ctx, &wanted, |ui| {
                panel.ui(ui, &mut session);
            })
            .len(),
            1,
            "the caption is drawn, once, as written"
        );

        // Four bars into a four-bar pattern: nothing is truncated, so nothing
        // claims to be.
        panel.ctx.bars = 4;
        let plain = format!("{note} · 4 bars");
        assert_eq!(
            painted_all(&ctx, &plain, |ui| {
                panel.ui(ui, &mut session);
            })
            .len(),
            1
        );
    }

    #[test]
    fn a_headless_pass_draws_every_row_and_the_confirm_dialog_without_panicking() {
        let ctx = egui::Context::default();
        let mut session = session_with_two_boxes();
        let mut panel = GeneratePanel::default();
        for p in &mut panel.ctx.parts {
            p.destination.device = Some(session.devices[0].id);
        }

        let frame = |ctx: &egui::Context, panel: &mut GeneratePanel, session: &mut Session| {
            let input = egui::RawInput::default();
            let mut output = ctx.run_ui(input, |u| {
                panel.ui(u, session);
            });
            output.textures_delta.clear();
        };

        frame(&ctx, &mut panel, &mut session);
        // Force the button through directly rather than simulating a click at
        // a screen position: what this test exists to catch is an egui Id
        // collision inside the row list and the confirm modal, not the
        // click-hitbox logic that `tracks.rs`'s own headless test already
        // covers for a different widget.
        panel.confirm = plan_generate(&panel.ctx, &session).ok();
        frame(&ctx, &mut panel, &mut session); // draws the confirm modal open
        let before_notes: usize = session
            .devices
            .iter()
            .flat_map(|d| d.patterns.iter())
            .flat_map(|p| p.tracks().iter())
            .map(|t| t.notes.len())
            .sum();
        if let Some(plan) = panel.confirm.take() {
            apply_plan(&mut session, plan);
        }
        frame(&ctx, &mut panel, &mut session); // draws once more with it applied
        let after_notes: usize = session
            .devices
            .iter()
            .flat_map(|d| d.patterns.iter())
            .flat_map(|p| p.tracks().iter())
            .map(|t| t.notes.len())
            .sum();
        assert!(after_notes > before_notes, "applying the plan should have written some notes");
    }
    // --- the settings ride in the session -----------------------------------

    /// Draw the panel once and hand back what the frame did. `mirror_into`
    /// runs at the *end* of a frame against a snapshot taken at the start, so
    /// only a change made while the panel is drawing counts as a change to
    /// save — which is every change there is, since nothing outside this
    /// panel touches its context.
    fn frame(ctx: &egui::Context, panel: &mut GeneratePanel, session: &mut Session) -> Outcome {
        let mut out = Outcome::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |u| {
            out = panel.ui(u, session);
        });
        output.textures_delta.clear();
        out
    }

    /// The panel's own settings, as they land in `Session::generator`.
    fn stored(session: &Session) -> GenContext {
        serde_json::from_value(session.generator.clone().expect("settings in the session"))
            .expect("what this panel wrote, it can read")
    }

    #[test]
    fn drawing_the_panel_is_not_a_change_to_save() {
        // The counterpart to `mark_edited` in the shell: opening Generate and
        // looking at it must not put a dot on the title bar or turn a quit into
        // a "save first?". Two frames, because a panel that mirrored itself on
        // every pass would only show it from the second one.
        let ctx = egui::Context::default();
        let mut session = session_with_two_boxes();
        let mut panel = GeneratePanel::default();

        for _ in 0..2 {
            let out = frame(&ctx, &mut panel, &mut session);
            assert!(!out.settings, "drawing is not editing");
            assert!(!out.edited);
        }
        assert!(session.generator.is_none(), "nothing was changed, so nothing was written");
    }

    #[test]
    fn a_setting_that_moves_lands_in_the_session_and_says_it_is_unsaved_work() {
        let mut session = session_with_two_boxes();
        let mut panel = GeneratePanel { ctx: default_ctx_on(&session), ..GeneratePanel::default() };
        let before = panel.ctx.clone();

        assert!(!panel.mirror_into(&mut session, &before), "nothing moved yet");
        assert!(session.generator.is_none());

        panel.ctx.feel.motion = 90;
        panel.ctx.seed = 4242;
        assert!(panel.mirror_into(&mut session, &before), "two settings moved");

        let saved = stored(&session);
        assert_eq!(saved.feel.motion, 90);
        assert_eq!(saved.seed, 4242);
    }

    #[test]
    fn a_key_change_is_not_a_settings_change() {
        // `root` and `scale` mirror `Session::harmony`, which is saved in its
        // own field — so turning the key must not also report this panel's
        // settings as unsaved work, or every Harmony edit would mark the file
        // dirty twice over and through the wrong door.
        let ctx = egui::Context::default();
        let mut session = session_with_two_boxes();
        let mut panel = GeneratePanel::default();
        frame(&ctx, &mut panel, &mut session);

        session.harmony.root = 7;
        let out = frame(&ctx, &mut panel, &mut session);
        assert_eq!(panel.ctx.root, 7, "the panel follows the key");
        assert!(!out.settings, "but it is Harmony's change, not this panel's");
    }

    #[test]
    fn the_settings_survive_a_save_and_an_open() {
        // The whole point: a session recalls the arrangement it was written by,
        // not only the notes that came out.
        let mut session = session_with_two_boxes();
        let mut panel = GeneratePanel { ctx: default_ctx_on(&session), ..GeneratePanel::default() };
        let before = panel.ctx.clone();
        panel.ctx.genre = GenreId::Techno;
        panel.ctx.progression = String::from("i iv VI v");
        panel.ctx.seed = 99;
        panel.ctx.seed_locked = true;
        panel.ctx.bars = 4;
        panel.ctx.feel = Feel { motion: 71, looseness: 12, humanize: 3 };
        panel.ctx.parts[2].on = false;
        panel.ctx.parts[0].destination.track = 5;
        panel.mirror_into(&mut session, &before);
        let wanted = panel.ctx.clone();

        let json = Project::new(session).to_json_pretty().unwrap();
        assert!(json.contains("\"generator\""), "and they are in the file under their own key");
        let reopened = Project::from_json(&json).unwrap().session;

        let mut fresh = GeneratePanel::default();
        fresh.session_replaced(&reopened);
        assert_eq!(fresh.ctx, wanted, "every field came back, rows and destinations included");
    }

    #[test]
    fn a_part_added_after_a_load_cannot_take_an_id_that_came_off_disk() {
        // The `DeviceId::reserve_past` hazard, for parts: ids are stream tags
        // rather than indices — see `context`'s header — and the counter behind
        // them starts at 1 every launch. A row added after a load must not
        // collide with one the file already named, or a reroll would move two
        // rows at once.
        let session = session_with_two_boxes();
        let mut panel = GeneratePanel::default();
        let mut off_disk = default_ctx_on(&session);
        for (i, part) in off_disk.parts.iter_mut().enumerate() {
            part.id = PartId(9_000 + i as u64);
        }
        let loaded = Session {
            generator: Some(serde_json::to_value(&off_disk).unwrap()),
            ..session
        };

        panel.session_replaced(&loaded);
        assert!(PartId::next().0 > 9_005, "the counter was pushed past every id the file used");
    }

    #[test]
    fn a_session_carrying_no_settings_puts_the_panel_back_to_its_defaults() {
        // A project written before this field, or one straight off New. The
        // rows on screen belong to the session that just left.
        let mut session = session_with_two_boxes();
        let mut panel = generated_panel(&mut session);
        panel.ctx.genre = GenreId::House;
        assert!(!panel.written.is_empty(), "it has written to this session");

        panel.session_replaced(&Session::default());
        assert_eq!(panel.ctx.genre, GenContext::default().genre);
        assert!(
            panel.written.is_empty(),
            "reopening a project is not consent to overwrite its tracks unasked"
        );
        assert!(panel.confirm.is_none(), "a dialog measured against the old session is gone");
    }

    #[test]
    fn settings_this_build_cannot_read_cost_the_settings_and_nothing_else() {
        // A hand-edited file, or one from a build that knew a genre this one
        // does not. The panel has to open either way — the notes in that
        // session are perfectly good, and refusing to draw would strand them.
        let session = Session {
            generator: Some(serde_json::json!({ "genre": "gabber" })),
            ..session_with_two_boxes()
        };
        let mut panel = GeneratePanel::default();
        panel.session_replaced(&session);

        // Field by field rather than whole: `GenContext::default()` mints six
        // fresh `PartId`s every time it is called, so two defaults are never
        // equal and never should be.
        let want = GenContext::default();
        assert_eq!(panel.ctx.genre, want.genre);
        assert_eq!(panel.ctx.progression, want.progression);
        assert_eq!(panel.ctx.seed, want.seed);
        assert_eq!(panel.ctx.feel, want.feel);
        assert_eq!(
            panel.ctx.parts.iter().map(|p| p.role).collect::<Vec<_>>(),
            want.parts.iter().map(|p| p.role).collect::<Vec<_>>()
        );
        match &panel.status {
            Some(Status::Failed(why)) => {
                assert!(why.contains("could not be read"), "it says so: {why}");
                assert!(why.contains("no note was touched"));
            }
            other => panic!("expected a failure the panel can show, got {other:?}"),
        }
    }
}
