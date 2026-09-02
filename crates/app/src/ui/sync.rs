// SEND: one button per box that puts a whole pattern onto one of its slots.
//
// **This is the riskiest thing in the app, multiplied.** `ui::write` overwrites
// one track of one slot behind a dialog naming it; this does the same to up to
// sixteen tracks behind *one* press. Read `ui::write`'s header first — every
// safety rule this obeys is that file's, reached through the same one function,
// and nothing here encodes a byte or decides a refusal of its own.
//
// **It was a desk-wide sync until 2026-08-26, and what changed it was a wrong
// write.** One button sent every box at once and — decision 4, as it then stood
// — aimed each one by provenance, deliberately having no pickers. The SEND rows
// three inches up *did* have pickers. So a DN2 row reading `to A02` and a
// pattern imported from A01 were two answers to one question, and the button
// used the one nobody could see: Neil pressed `Overwrite 7 tracks on 2 boxes`
// and the DN2's tracks landed on A01. The fix is not a tie-break between the
// two answers, it is that a box now has one destination and one place it is
// chosen — this row's picker, which *starts* where provenance says and stays
// where it is put. The whole-desk button went with it: OUT is per box now,
// exactly as IN is, and the per-track panel it used to sit beside is behind
// `setup::PER_TRACK_LABEL`.
//
// ## Six decisions
//
//  1. **One dialog, enumerating every track, with a per-row opt-out.**
//     Rule 4 says nothing is written without a dialog naming the slot, the
//     track and the trigs being replaced. Sixteen of those dialogs is not
//     consent, it is a mash-through: by the fourth one nobody is reading. So
//     the whole intent is one modal, one row per track, each row tickable, and
//     its confirm button names the count it is about to do — `Overwrite 12
//     tracks` — and re-counts as rows are unticked.
//
//  2. **One backup per *slot*, not per track**, which is why
//     `protocol::safe_write::safe_write_tracks` exists. Rule 3 scales badly on
//     purpose: every write takes its own backup, and a backup that cannot be
//     stored is a write that does not happen. At one backup per track, a
//     32-track send would put 32 entries into a fifty-entry ring — the feature
//     destroying the recovery path it depends on, on the run where you most
//     need it. Grouped by slot it is two backups, and the ring still holds the
//     last fifty things this app overwrote. Decided with Neil 2026-08-18.
//
//  3. **Empty tracks are skipped, and the dialog says which.** A session track
//     with no notes could mean "clear that track on the box", and as a *default*
//     that is a press wiping tracks nobody looked at. So a track with no notes
//     is not sent — and it is listed, greyed, with the reason, because an
//     omission you can see is a decision and an omission you cannot is a bug.
//     `ui::write`'s per-track button is still there for a deliberate clear.
//     Decided with Neil 2026-08-18.
//
//  4. **Two pickers, and they are the two `ui::transfer` has, read backwards.**
//     `send <this session's slot> to <the box's slot>`. What they *start* on is
//     unchanged from the rules this file has always had: the scene's slot for
//     the source, and `write::aim`'s answer for the destination — back where the
//     pattern was imported from, or the slot of the same name when it has no
//     provenance (`ui::write` decision 6). Each pins the moment it is touched,
//     and an untouched destination still follows the source as it moves. The
//     pickers exist because the alternative was two mechanisms disagreeing in
//     silence; see the note at the top about how that was found.
//
//  5. **There are two fetches per slot, and the second one is the write's.**
//     The dialog has to say how many trigs each destination track holds, and
//     only the box knows that — so a read-only survey pass runs first, across
//     every box, before the one dialog. The write that follows re-fetches, per
//     rule 2, and builds its payload from *that*: the survey is wording, never a
//     payload. What the survey buys is worth the second dump, and what it costs
//     is closed by [`changed_since_survey`] — if the box moved between the two
//     reads, that slot refuses rather than writing against consent given for
//     different numbers.
//
//  6. **A backup that cannot be stored stops the run**, and everything after it
//     in that run. Rule 3 says such a write does not happen; with a store that
//     has just failed, neither does the next one, and carrying on to try would
//     be asking the same question and expecting a different answer while
//     overwriting a second slot unbacked. At one box per press that rule is
//     about the box itself — but [`run`] still carries it across a list of jobs,
//     because that is the shape the flow was verified in and `app/tests/sync.rs`
//     drives it with two boxes to state the rule at all. The panel hands it one.
//
// **What this deliberately does not do.** It does not stop the transport, for
// `ui::write`'s reason. It does not touch the session — a whole-pattern send
// claims no provenance, per that file's decision 5, which is why moving the
// destination picker does not re-aim the *next* send: provenance is what the
// picker starts from, and only an import writes it. And it never sends to a box
// that is not the box the row names: `write::wrong_box` refuses a mis-cabled
// desk here exactly as it does there, which matters more at this scale, because
// one wrong cable would otherwise take sixteen tracks with it.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

use digi_core::device::{Device, PatternRoute};
use digi_core::session::PatternRef;
use digi_core::{DeviceId, Session, Source};
use digi_midi::{ElektronDevice, PortBinding};
use digi_protocol::a4_pattern::{
    read_track_trigs as a4_read_track_trigs, PAYLOAD_LEN as A4_PAYLOAD_LEN,
    TRACK_NAMES as A4_TRACK_NAMES,
};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::DeviceIdentity;
use digi_protocol::a4_kit::{parse_working_kit, A4Kit, DUMP_A4_KIT_WORKING};
use digi_protocol::pattern::{decode_pattern_kit, track_trig_count, PatternKit, Spec};
use digi_protocol::protocol::{build_dump_message, FAMILY_ANALOG_FOUR};
use digi_protocol::plocks::{free_lane_count, read_track_plocks, PoolLane};
use digi_protocol::safe_write::{
    a4_safe_write_tracks, safe_write_tracks, write_gate, write_impact_lines,
    write_result_message, A4TrackWrite, ConfirmArgs, ImpactArgs, PatternIo, Timestamp,
    TrackWrite, WriteError, WriteHooks, BACKUP_LINE,
};
use eframe::egui::{self, Ui};

use crate::ui::transfer::{binding, slot_choices, wire_slots};
use crate::ui::write::{aim, blocker, is_home, track_kind_label, wrong_box, PortsPresent};

// --- the plan, which needs no box ------------------------------------------------

/// The tracks going to one box, in whichever format that box speaks. Decided
/// once, in [`plan_box`], from the model's `pattern_route`; matched once more,
/// in [`run`], to pick the ceremony — everything between is format-blind.
#[derive(Debug)]
pub enum JobWrites {
    Gen2(Vec<TrackWrite>),
    A4(Vec<A4TrackWrite>),
}

impl JobWrites {
    pub fn len(&self) -> usize {
        match self {
            Self::Gen2(w) => w.len(),
            Self::A4(w) => w.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One box's share of a sync: which of its tracks are going, and where.
#[derive(Debug)]
pub struct BoxJob {
    pub device: DeviceId,
    /// The user's label for this box in the session — "DT2", "Adeel's DN2".
    pub name: String,
    pub display: &'static str,
    /// The slug the identity handshake has to come back with, per decision 3 of
    /// `ui::write`.
    pub slug: Option<&'static str>,
    pub input: PortBinding,
    pub output: PortBinding,
    /// `None` on the Analog Four, whose format has no `Spec`; [`Self::writes`]
    /// says which flow the job takes instead.
    pub spec: Option<&'static Spec>,
    /// The scene's slot on this box — where the notes are coming from.
    pub from: PatternRef,
    /// Where they are going, by the provenance rule.
    pub into: PatternRef,
    pub pattern_name: String,
    /// One per track being sent, same order and same length as [`Self::aims`].
    pub writes: JobWrites,
    pub aims: Vec<TrackAim>,
    /// Tracks not being sent, and why — never dropped silently (decision 3).
    pub skipped: Vec<Skipped>,
}

/// What is going onto one track, as the dialog says it before the box is asked.
#[derive(Debug, Clone)]
pub struct TrackAim {
    pub track_index: usize,
    /// The track's name in this session, when it has stopped being "T3".
    pub name: Option<String>,
    pub notes: usize,
    pub lanes: usize,
    /// What `core::export` could not carry off this track.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Skipped {
    pub track_index: usize,
    pub why: String,
}

/// A box that cannot take part at all, and the sentence saying so.
#[derive(Debug, Clone)]
pub struct Blocked {
    pub device: DeviceId,
    pub name: String,
    pub why: String,
}

#[derive(Debug, Default)]
pub struct MassPlan {
    pub jobs: Vec<BoxJob>,
    pub blocked: Vec<Blocked>,
}

impl MassPlan {
    /// How many tracks the whole plan would send.
    pub fn tracks(&self) -> usize {
        self.jobs.iter().map(|j| j.writes.len()).sum()
    }

    /// Whether there is anything at all to press the button for.
    pub fn is_empty(&self) -> bool {
        self.tracks() == 0
    }
}

/// Everything one box's press captures, before a thread exists.
///
/// Pure: no ports, no I/O, no clock. The same bargain `ui::write::plan` makes,
/// and the reason the whole shape of a send is testable without a box —
/// including the two things easiest to get wrong at this scale, which tracks are
/// skipped and where the slot is aimed.
///
/// **`into` is the caller's, not this function's** — decision 4. The panel's
/// picker decides it, and what that picker *starts* at is [`aim`]'s answer. A
/// second opinion computed down here is the bug this signature exists to make
/// unrepresentable.
pub fn plan_box(
    session: &Session,
    present: PortsPresent<'_>,
    id: DeviceId,
    from: PatternRef,
    into: PatternRef,
) -> MassPlan {
    let mut plan = MassPlan::default();
    let Some(device) = session.device(id) else { return plan };
    if let Some(why) = blocker(device, present) {
        plan.blocked.push(Blocked { device: id, name: device.name.clone(), why });
        return plan;
    }
    // `blocker` already refused a box without both ports and without a pattern
    // route, so these cannot fail — but they are refused through the same list
    // rather than `expect`, because a refusal that reaches the panel is always
    // better than a window that closes.
    let (Some(input), Some(output)) = (device.io.input.clone(), device.io.output.clone())
    else {
        plan.blocked.push(Blocked {
            device: id,
            name: device.name.clone(),
            why: "this box has no ports or no pattern format".into(),
        });
        return plan;
    };
    let gen1 = device.model.pattern_route() == PatternRoute::RequestGen1;
    let spec = device.model.spec();
    if !gen1 && spec.is_none() {
        plan.blocked.push(Blocked {
            device: id,
            name: device.name.clone(),
            why: "this box has no ports or no pattern format".into(),
        });
        return plan;
    }

    let pattern = device.pattern(from.slot());
    let mut job = BoxJob {
        device: id,
        name: device.name.clone(),
        display: device.model.display,
        slug: device.model.slug,
        input: binding(&input),
        output: binding(&output),
        spec,
        from,
        into,
        pattern_name: pattern.map(|p| p.name.clone()).unwrap_or_default(),
        writes: if gen1 { JobWrites::A4(Vec::new()) } else { JobWrites::Gen2(Vec::new()) },
        aims: Vec::new(),
        skipped: Vec::new(),
    };

    for track_index in 0..device.model.num_tracks {
        let track = pattern.and_then(|p| p.track(track_index));
        let notes = track.map(|t| t.notes.len()).unwrap_or(0);
        if notes == 0 {
            // Decision 3, and said out loud rather than left out.
            job.skipped.push(Skipped {
                track_index,
                why: "nothing here — left as it is on the box".into(),
            });
            continue;
        }
        // Each format plans through its own `core` seam; the aim rows they
        // produce are identical, which is what keeps the dialog format-blind.
        let planned = match (&mut job.writes, spec) {
            (JobWrites::A4(writes), _) => {
                session.a4_track_write(id, from, track_index, into).map(|export| {
                    writes.push(export.write);
                    // `lanes: 0` even when the roll holds lanes on this track:
                    // an A4 write cannot carry them, and the count in this row
                    // is a promise of what travels. The loss is in
                    // `export.warnings` instead, which the dialog prints.
                    (0, export.warnings)
                })
                .map_err(|e| e.to_string())
            }
            (JobWrites::Gen2(writes), Some(spec)) => {
                session.track_write(spec, id, from, track_index, into).map(|export| {
                    writes.push(export.write);
                    (track.map(|t| t.plocks.len()).unwrap_or(0), export.warnings)
                })
                .map_err(|e| e.to_string())
            }
            (JobWrites::Gen2(_), None) => unreachable!("refused above"),
        };
        match planned {
            Ok((lanes, warnings)) => {
                job.aims.push(TrackAim {
                    track_index,
                    name: track_name(device, from, track_index),
                    notes,
                    lanes,
                    warnings,
                });
            }
            // `core` refusing a track is not a reason to refuse the box: the
            // other fifteen are still describable, and the one that is not
            // says so in the same list the empty ones do.
            Err(why) => job.skipped.push(Skipped { track_index, why }),
        }
    }

    if job.writes.is_empty() {
        plan.blocked.push(Blocked {
            device: id,
            name: job.name.clone(),
            why: format!("{} has no notes to send", job.pattern_name_or_slot()),
        });
        return plan;
    }
    plan.jobs.push(job);
    plan
}

/// Where this box's send row points when nobody has touched its pickers: the
/// scene's slot as the source, and [`aim`]'s answer as the destination.
pub fn defaults(session: &Session, id: DeviceId) -> (PatternRef, PatternRef) {
    let from = session
        .slot_in_scene(session.current_scene, id)
        .unwrap_or_else(|| PatternRef::new(0, 0));
    let into = session
        .device(id)
        .map(|d| aim(d.pattern(from.slot()), d.model.slug, from))
        .unwrap_or(from);
    (from, into)
}

impl BoxJob {
    /// How to name this pattern in a sentence about it.
    ///
    /// **The slot is not repeated when the name *is* the slot.** A pattern
    /// nobody has renamed is called "A01", so the obvious wording produced
    /// `“A01” has no notes in A01` — which was tolerable buried in a whole-desk
    /// summary and is not, now that this line sits under one box's own row as
    /// the everyday answer to "why is that button grey". Seen on screen
    /// 2026-08-26.
    fn pattern_name_or_slot(&self) -> String {
        let slot = self.from.label();
        match self.pattern_name.trim() {
            "" => slot,
            name if name == slot => slot,
            name => format!("“{name}” ({slot})"),
        }
    }
}

/// A track's name in this session, when the box gave it one.
fn track_name(device: &Device, from: PatternRef, track_index: usize) -> Option<String> {
    let default = format!("T{}", track_index + 1);
    device
        .pattern(from.slot())
        .and_then(|p| p.track(track_index))
        .map(|t| t.name.trim().to_string())
        .filter(|n| !n.is_empty() && *n != default)
}

// --- what the box says before the dialog -----------------------------------------

/// One box's destination, read before anyone is asked anything.
///
/// Decision 5: this is the *wording's* copy. Nothing here becomes a payload.
#[derive(Debug, Clone)]
pub struct Survey {
    /// Empty on the A4, whose mapped format has no kit name to read.
    pub kit_name: String,
    /// `None` on the A4 — swing and the lane pool are not in its mapped format,
    /// so the dialog's slot-wide lines have nothing true to say about them.
    pub box_swing: Option<u8>,
    pub free_lanes: Option<usize>,
    /// Per track being sent, in the job's order: what the destination holds now.
    pub existing: Vec<TrackSurvey>,
}

#[derive(Debug, Clone)]
pub struct TrackSurvey {
    pub track_index: usize,
    pub existing_trigs: usize,
    /// The destination track's name on the box — a sound name, or "MIDI".
    pub kind: String,
    /// What that track has locked on the box right now. Kept whole rather than
    /// counted, because `write_impact_lines` decides which of them a write
    /// *clears* by comparing parameter ids — a count could only be guessed with.
    pub box_plocks: Vec<PoolLane>,
}

impl Survey {
    fn of(&self, track_index: usize) -> Option<&TrackSurvey> {
        self.existing.iter().find(|t| t.track_index == track_index)
    }
}

/// Read one box's destination slot and describe it. Read-only.
pub fn survey(device: &mut impl PatternIo, job: &BoxJob) -> Result<Survey, String> {
    let identity = device
        .identity()
        .cloned()
        .ok_or_else(|| "the box did not answer the identity handshake".to_string())?;
    // The same refusal as the single-track panel, and it matters more here: one
    // wrong cable would otherwise take sixteen tracks with it.
    if let Some(refusal) = wrong_box(job.slug, job.display, &identity) {
        return Err(refusal);
    }
    // **A copy of `safe_write_tracks`' own gate, and it earns its place.**
    // `ui::write` deliberately does not repeat this, because there the refusal
    // arrives from inside the flow with the same words and nobody could tell.
    // Here it changes what the *dialog* says: a box on a firmware the format was
    // never verified against is listed as refused rather than as sixteen rows
    // you can tick and consent to.
    let gate = write_gate(Some(&identity));
    if !gate.ok {
        return Err(gate.reason);
    }

    let index = job
        .into
        .wire_index()
        .ok_or_else(|| format!("{} is not a slot this box has", job.into.label()))?;
    let bytes = device.fetch_pattern_kit(index)?;

    // The A4's survey is smaller because its mapped format is: per-track trig
    // counts — `a4_read_track_trigs` counts what the box shows, the same call
    // `a4_safe_write_tracks`' confirm counts with, so `changed_since_survey`
    // compares like with like — and nothing slot-wide at all.
    if matches!(job.writes, JobWrites::A4(_)) {
        if bytes.len() != A4_PAYLOAD_LEN {
            return Err(format!(
                "the box answered {} bytes for {}, an A4 pattern is {A4_PAYLOAD_LEN}",
                bytes.len(),
                job.into.label()
            ));
        }
        let existing = job
            .aims
            .iter()
            .map(|aim| {
                Ok(TrackSurvey {
                    track_index: aim.track_index,
                    existing_trigs: a4_read_track_trigs(&bytes, aim.track_index)?.len(),
                    kind: A4_TRACK_NAMES.get(aim.track_index).copied().unwrap_or("?").into(),
                    box_plocks: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        return Ok(Survey { kit_name: String::new(), box_swing: None, free_lanes: None, existing });
    }

    let spec = job.spec.ok_or_else(|| "this box has no pattern format".to_string())?;
    let kit = decode_pattern_kit(spec, &bytes)?;
    let existing = job
        .aims
        .iter()
        .map(|aim| TrackSurvey {
            track_index: aim.track_index,
            existing_trigs: trigs_on(&kit, aim.track_index),
            kind: track_kind_label(&kit, aim.track_index, spec.track_kind_fallback),
            box_plocks: read_track_plocks(spec, &bytes, aim.track_index).unwrap_or_default(),
        })
        .collect();
    Ok(Survey {
        kit_name: kit.kit.name.clone(),
        box_swing: Some(digi_protocol::pattern_settings::read_swing(spec, &bytes)),
        free_lanes: Some(free_lane_count(spec, &bytes)),
        existing,
    })
}

/// How many trigs the destination track holds.
///
/// **`protocol`'s own count, not a second one.** A first draft here counted
/// `trigs.len()`, which is one *higher* on the DT2 fixture: that capture holds
/// the leftovers of a trig deleted on the box, and `track_trig_count` reads the
/// enabled bit instead. The survey and `safe_write_tracks` compare their answers
/// to each other (see [`changed_since_survey`]), so two ways of counting the
/// same thing did not merely word the dialog differently — it made every write
/// refuse itself. Lesson 5, found by the guard it broke.
fn trigs_on(kit: &PatternKit, track_index: usize) -> usize {
    if track_index >= kit.tracks.len() {
        return 0;
    }
    track_trig_count(kit, track_index)
}

// --- the dialog's words, as values ------------------------------------------------

/// The whole intent, in the shape the modal draws and a test can read.
#[derive(Debug, Clone)]
pub struct AskBox {
    pub device: DeviceId,
    /// "DT2 · Digitakt II — A01 “KIT 1”".
    pub heading: String,
    /// Everything this box's write touches beyond the named tracks' trigs.
    pub lines: Vec<String>,
    pub rows: Vec<AskRow>,
    /// The tracks not going, and why (decision 3).
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AskRow {
    pub track_index: usize,
    /// "T1 BD — 4 notes and 2 p-lock lanes, replacing 8 trigs".
    pub label: String,
    /// Anything `core::export` could not carry off this track, one per line.
    pub warnings: Vec<String>,
}

/// The dialog, and the wire back to the thread waiting on it.
///
/// Dropping this without sending is a refusal, exactly as `ui::write::Ask` is:
/// closing the window mid-question consents to nothing.
pub struct Ask {
    pub boxes: Vec<AskBox>,
    /// Boxes that cannot take part, said in the same modal so the count in the
    /// button is not silently smaller than the desk.
    pub blocked: Vec<String>,
    /// The last line, always: the backup promise (`BACKUP_LINE`).
    pub backup_line: &'static str,
    /// `None` is a cancel; otherwise the `(box, track)` pairs still ticked.
    pub reply: Sender<Option<Vec<(DeviceId, usize)>>>,
}

/// The dialog's confirm button, which has to name the count rather than say OK.
///
/// Recomputed as rows are unticked, because a button that says twelve while nine
/// are ticked is the same lie as one that says OK.
pub fn headline(tracks: usize) -> String {
    format!("Overwrite {}", plural(tracks, "track"))
}

/// One row: what is going onto this track, and what it lands on.
pub fn row_label(aim: &TrackAim, survey: Option<&TrackSurvey>) -> String {
    let mut label = format!("T{}", aim.track_index + 1);
    // The session's name for the track first — it is what the roll is showing —
    // then the box's, when the two differ and the box has one worth saying.
    if let Some(name) = &aim.name {
        label.push(' ');
        label.push_str(name);
    }
    match survey.map(|s| s.kind.trim()) {
        Some(kind) if !kind.is_empty() && Some(kind) != aim.name.as_deref() => {
            // `>`, not `→` (U+2192): pre-existing tofu, flagged from a real
            // window capture 2026-08-20 and fixed in passing while this file
            // was open for packet E. `ui::tracks::channel_note`'s doc comment
            // already made this same call for the same reason — `>` is the
            // house answer, not a third convention.
            label.push_str(&format!(" > {kind}"));
        }
        _ => {}
    }
    label.push_str(&format!(" — {}", plural(aim.notes, "note")));
    if aim.lanes > 0 {
        label.push_str(&format!(" and {}", plural(aim.lanes, "p-lock lane")));
    }
    match survey.map(|s| s.existing_trigs) {
        Some(0) | None => label.push_str(", onto an empty track"),
        Some(n) => label.push_str(&format!(", replacing {}", plural(n, "trig"))),
    }
    label
}

/// One box's block of the dialog.
pub fn ask_box(job: &BoxJob, survey: &Survey, playing: bool) -> AskBox {
    let kit = match survey.kit_name.trim() {
        "" => String::new(),
        name => format!(" “{name}”"),
    };
    // `plural`, not "{} tracks": with one box per press this heading is the first
    // line of the everyday confirm dialog, and it read "1 tracks into A01" there.
    let heading = format!(
        "{} · {} — {} into {}{}",
        job.name,
        job.display,
        plural(job.writes.len(), "track"),
        job.into.label(),
        kit
    );

    let mut lines = Vec::new();
    if job.into != job.from {
        lines.push(format!(
            "Coming from {} in this session, going to {} on the box.",
            job.from.label(),
            job.into.label()
        ));
    }
    match &job.writes {
        JobWrites::Gen2(writes) => {
            // Per track, the lanes it writes and clears and its PROB default —
            // the parts of the impact that really are per track. Swing is left
            // out here because it is one byte for the whole slot, and repeating
            // it sixteen times would read as sixteen changes.
            const NO_LANES: &[PoolLane] = &[];
            for (aim, write) in job.aims.iter().zip(writes) {
                let per_track = write_impact_lines(&ImpactArgs {
                    label: &job.into.label(),
                    track: Some(aim.track_index),
                    lanes: write.plocks.as_deref().unwrap_or(&[]),
                    box_plocks: survey
                        .of(aim.track_index)
                        .map(|t| t.box_plocks.as_slice())
                        .unwrap_or(NO_LANES),
                    // Left to the slot-wide line below: the pool is one budget
                    // for all sixteen tracks, so a per-track "won't fit" would
                    // be counted once per track out of the same eighty.
                    free_lanes: None,
                    track_prob: write.track_prob,
                    swing: None,
                    box_swing: None,
                });
                for line in per_track {
                    lines.push(format!("T{}: {line}", aim.track_index + 1));
                }
            }
            // The pool, once, against everything this box is about to spend out
            // of it.
            let wanted: usize = writes
                .iter()
                .map(|w| w.plocks.as_ref().map(|l| l.len()).unwrap_or(0))
                .sum();
            let freed: usize = job
                .aims
                .iter()
                .map(|a| survey.of(a.track_index).map(|t| t.box_plocks.len()).unwrap_or(0))
                .sum();
            let free_lanes = survey.free_lanes.unwrap_or(0);
            if wanted > free_lanes + freed {
                lines.push(format!(
                    "Careful: these tracks want {} between them and the pattern only has {} — \
                     some of them won't fit, and you'll be told which.",
                    plural(wanted, "p-lock lane"),
                    plural(free_lanes + freed, "spare lane")
                ));
            }
            // Swing, once, because it is the slot's and not a track's.
            let swing = writes.iter().find_map(|w| w.swing).map(|s| s.round() as u8);
            lines.extend(write_impact_lines(&ImpactArgs {
                label: &job.into.label(),
                track: None,
                lanes: &[],
                box_plocks: &[],
                free_lanes: None,
                track_prob: None,
                swing,
                box_swing: survey.box_swing,
            }));
        }
        // The A4's whole impact beyond the named tracks' trigs is that there is
        // none, and saying so is this dialog's version of the enumeration
        // above: the sounds and p-locks a person might expect to travel with
        // the pattern stay exactly as the destination slot has them.
        JobWrites::A4(_) => lines.push(
            "Only the trigs move: sounds, p-locks, velocity and length stay as the \
             destination slot holds them right now — the write is composed on a fresh read \
             of that slot."
                .to_string(),
        ),
    }
    if playing {
        lines.push(
            "The transport is running — this app keeps clocking the box while the dumps go \
             across, and pressing this does not stop it."
                .to_string(),
        );
    }

    AskBox {
        device: job.device,
        heading,
        lines,
        rows: job
            .aims
            .iter()
            .map(|aim| AskRow {
                track_index: aim.track_index,
                label: row_label(aim, survey.of(aim.track_index)),
                warnings: aim.warnings.clone(),
            })
            .collect(),
        skipped: job
            .skipped
            .iter()
            .map(|s| format!("T{} — {}", s.track_index + 1, s.why))
            .collect(),
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

// --- the run --------------------------------------------------------------------

/// What the worker says while it works.
pub enum Event {
    Status(String),
    Log(String),
    Ask(Ask),
    Done(MassReport),
}

/// One box's end state, as its row shows it.
#[derive(Debug, Clone)]
pub struct BoxOutcome {
    pub device: DeviceId,
    pub name: String,
    pub text: String,
    pub is_error: bool,
    /// Whether bytes were actually stored. A refusal, a cancel and a box nobody
    /// ticked are all `false`, and each says something different in `text`.
    pub wrote: bool,
    /// The backup line, so the row can say where the previous contents went.
    pub log: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MassReport {
    pub boxes: Vec<BoxOutcome>,
    /// The whole run was called off at the dialog: nothing was surveyed further
    /// and nothing was sent.
    pub cancelled: bool,
}

impl MassReport {
    pub fn tracks_written(&self) -> usize {
        self.boxes.iter().filter(|b| b.wrote).count()
    }
}

/// The whole flow after the plan, with the boxes injected.
///
/// Generic over how a job becomes a [`PatternIo`] for the reason `ui::write::run`
/// is generic over the trait: `app/tests/sync.rs` drives this exact function —
/// the survey, the one dialog, the per-row opt-out, the per-slot write and the
/// store failing mid-run — against boxes that are `BTreeMap`s, so the only thing
/// left untested by the time a cable is involved is the cable.
pub fn run<D: PatternIo>(
    plan: &MassPlan,
    stash: &Stash,
    mut open: impl FnMut(&BoxJob) -> Result<D, String>,
    events: &Sender<Event>,
    now: Timestamp,
    playing: bool,
) -> MassReport {
    let mut report = MassReport::default();
    for blocked in &plan.blocked {
        report.boxes.push(BoxOutcome {
            device: blocked.device,
            name: blocked.name.clone(),
            text: blocked.why.clone(),
            is_error: false,
            wrote: false,
            log: None,
        });
    }

    // --- the survey pass, read-only ---------------------------------------------
    let mut ready: Vec<(usize, D, Survey)> = Vec::new();
    for (position, job) in plan.jobs.iter().enumerate() {
        let _ = events.send(Event::Status(format!("Reading {} off the {}…", job.into.label(), job.name)));
        let opened = open(job).and_then(|mut device| {
            let s = survey(&mut device, job)?;
            Ok((device, s))
        });
        match opened {
            Ok((device, s)) => ready.push((position, device, s)),
            Err(why) => report.boxes.push(BoxOutcome {
                device: job.device,
                name: job.name.clone(),
                text: why,
                is_error: true,
                wrote: false,
                log: None,
            }),
        }
    }
    if ready.is_empty() {
        let _ = events.send(Event::Done(report.clone()));
        return report;
    }

    // --- one dialog, about everything -------------------------------------------
    let ask_boxes: Vec<AskBox> = ready
        .iter()
        .map(|(position, _, s)| ask_box(&plan.jobs[*position], s, playing))
        .collect();
    let (reply, answer) = channel();
    let ask = Ask {
        boxes: ask_boxes,
        blocked: plan
            .blocked
            .iter()
            .map(|b| format!("{} — {}", b.name, b.why))
            .collect(),
        backup_line: BACKUP_LINE,
        reply,
    };
    if events.send(Event::Ask(ask)).is_err() {
        report.cancelled = true;
        return report;
    }
    // A dropped channel is a window that has gone, and a window that has gone
    // consented to nothing.
    let Some(accepted) = answer.recv().unwrap_or(None) else {
        report.cancelled = true;
        for (position, _, _) in &ready {
            let job = &plan.jobs[*position];
            report.boxes.push(BoxOutcome {
                device: job.device,
                name: job.name.clone(),
                text: "Send cancelled".into(),
                is_error: false,
                wrote: false,
                log: None,
            });
        }
        let _ = events.send(Event::Done(report.clone()));
        return report;
    };

    // --- the writes, one slot at a time -----------------------------------------
    let mut store_failed: Option<String> = None;
    for (position, mut device, survey) in ready {
        let job = &plan.jobs[position];
        if let Some(why) = &store_failed {
            // Decision 6: the store is broken, so this write is one that does not
            // happen either — said rather than attempted.
            report.boxes.push(BoxOutcome {
                device: job.device,
                name: job.name.clone(),
                text: format!("Not attempted — {why}"),
                is_error: true,
                wrote: false,
                log: None,
            });
            continue;
        }

        // The per-row opt-out, applied in whichever format the job planned. The
        // two arms are the same filter; the ceremony they feed differs.
        let ticked = |track_index: usize| accepted.contains(&(job.device, track_index));
        let writes = match &job.writes {
            JobWrites::Gen2(all) => JobWrites::Gen2(
                all.iter().filter(|w| ticked(w.track_index)).cloned().collect(),
            ),
            JobWrites::A4(all) => JobWrites::A4(
                all.iter().filter(|w| ticked(w.track_index)).cloned().collect(),
            ),
        };
        if writes.is_empty() {
            report.boxes.push(BoxOutcome {
                device: job.device,
                name: job.name.clone(),
                text: "Nothing ticked for this box".into(),
                is_error: false,
                wrote: false,
                log: None,
            });
            continue;
        }

        let mut hooks = SlotHooks { events, survey: &survey, log: None, refusal: None };
        let outcome = match &writes {
            JobWrites::Gen2(writes) => {
                safe_write_tracks(&mut device, stash, writes, &mut hooks, now)
            }
            JobWrites::A4(writes) => {
                a4_safe_write_tracks(&mut device, stash, writes, &mut hooks, now)
            }
        };
        let (text, is_error, wrote) = match outcome {
            Ok(result) if result.cancelled => (
                hooks
                    .refusal
                    .clone()
                    .unwrap_or_else(|| "Write cancelled".to_string()),
                hooks.refusal.is_some(),
                false,
            ),
            Ok(result) => {
                // What `core::export` could not carry belongs in the result line
                // too, or a successful send reads as if everything went — the
                // same rule `ui::write::run` follows.
                let mut warnings = result.warnings.clone();
                for aim in &job.aims {
                    if accepted.contains(&(job.device, aim.track_index)) {
                        warnings.extend(aim.warnings.iter().cloned());
                    }
                }
                let message = write_result_message(&digi_protocol::safe_write::WriteResult {
                    warnings,
                    ..result.clone()
                });
                (message.text, message.is_error, result.ok)
            }
            Err(e) => {
                if let WriteError::Stash(_) = &e {
                    store_failed = Some(format!(
                        "the backup store failed on the {} ({e}), and a backup that cannot be \
                         stored is a write that does not happen",
                        job.name
                    ));
                }
                (e.to_string(), true, false)
            }
        };
        report.boxes.push(BoxOutcome {
            device: job.device,
            name: job.name.clone(),
            text,
            is_error,
            wrote,
            log: hooks.log,
        });
    }

    let _ = events.send(Event::Done(report.clone()));
    report
}

/// Whether the destination moved between the survey the dialog described and the
/// re-fetch the write is built from.
///
/// Decision 5's other half. Consent was given for "replacing 8 trigs on T1"; if
/// the box now says 12, the sentence agreed to is not the sentence about to
/// happen. Returns the line explaining the refusal, or `None`.
pub fn changed_since_survey(survey: &Survey, args: &ConfirmArgs) -> Option<String> {
    for track in &args.tracks {
        let Some(seen) = survey.of(track.track_index) else {
            return Some(format!(
                "T{} was not in what this dialog described — nothing was written",
                track.track_index + 1
            ));
        };
        if seen.existing_trigs != track.existing_trigs {
            return Some(format!(
                "the box changed while the dialog was open: T{} had {} when you were asked and \
                 has {} now — nothing was written",
                track.track_index + 1,
                seen.existing_trigs,
                track.existing_trigs
            ));
        }
    }
    None
}

/// The hooks for one slot of a run that has already been consented to.
struct SlotHooks<'a> {
    events: &'a Sender<Event>,
    survey: &'a Survey,
    log: Option<String>,
    /// Set when [`changed_since_survey`] turned the consent down.
    refusal: Option<String>,
}

impl WriteHooks for SlotHooks<'_> {
    /// **Consent was given upstairs, once, for the whole desk** — so this does
    /// not ask again. What it does is check that the thing consented to is still
    /// the thing about to happen.
    fn confirm(&mut self, args: &ConfirmArgs) -> bool {
        match changed_since_survey(self.survey, args) {
            Some(why) => {
                self.refusal = Some(why);
                false
            }
            None => true,
        }
    }

    fn on_status(&mut self, status: &str) {
        let _ = self.events.send(Event::Status(status.to_string()));
    }

    fn on_log(&mut self, line: &str) {
        self.log = Some(line.to_string());
        let _ = self.events.send(Event::Log(line.to_string()));
    }
}

/// Open the ports, identify, and run the flow. The whole of the thread.
fn worker(plan: MassPlan, playing: bool, events: Sender<Event>) {
    // Rule 1's first gate, before anything is even read: the store has to exist,
    // because it holds the only automatic copy of everything about to be
    // overwritten — and at this scale that is up to thirty-two tracks.
    let stash = match Stash::default_stash() {
        Ok(stash) => stash,
        Err(e) => {
            let mut report = MassReport::default();
            for job in &plan.jobs {
                report.boxes.push(BoxOutcome {
                    device: job.device,
                    name: job.name.clone(),
                    text: format!(
                        "nothing was written: there is nowhere to keep the backup ({e}) — a \
                         backup that cannot be stored is a write that does not happen"
                    ),
                    is_error: true,
                    wrote: false,
                    log: None,
                });
            }
            let _ = events.send(Event::Done(report));
            return;
        }
    };

    let report = run(
        &plan,
        &stash,
        |job| {
            let mut device = ElektronDevice::open(&job.input, &job.output).map_err(|e| e.to_string())?;
            device.identify().map_err(|e| e.to_string())?;
            Ok(device)
        },
        &events,
        Timestamp::now(),
        playing,
    );
    // `run` already sent this on every path it returns from; the worker resends
    // nothing. Kept as a binding so a future early return here cannot drop it.
    let _ = report;
}

// --- the panel --------------------------------------------------------------------

/// A run in flight, and which box's row it belongs to — the spinner belongs on
/// the row that was pressed, not on every row in the group.
struct Pending {
    device: DeviceId,
    rx: Receiver<Event>,
    status: String,
}

/// What is on screen instead of the panel, if anything.
enum Dialog {
    Confirm {
        ask: Ask,
        /// One flag per row, in the ask's own order. Everything starts ticked:
        /// the plan is what the button offered to do.
        ticked: Vec<Vec<bool>>,
    },
    /// A finished run that must not be scrolled past.
    Alert { lines: Vec<String> },
}

/// One box's row: where the pattern is coming from and where it is going.
///
/// Both are `None` until someone picks, and `None` means *following* — the
/// scene's slot for the source, [`aim`]'s answer for the destination. The same
/// pin-on-touch rule `ui::transfer::Row` and `ui::write::Row` both keep, so all
/// three rows of the panel behave identically under the same gesture.
#[derive(Default)]
struct Row {
    from: Option<PatternRef>,
    into: Option<PatternRef>,
}

#[derive(Default)]
pub struct SyncPanel {
    pending: Option<Pending>,
    dialog: Option<Dialog>,
    rows: HashMap<DeviceId, Row>,
    outcomes: HashMap<DeviceId, BoxOutcome>,
}

impl SyncPanel {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Take whatever the worker has said, and put up the dialog it is waiting on.
    ///
    /// Called from the window rather than from the panel, for `ui::write::tick`'s
    /// reason: the Setup panel collapses, a collapsed panel's body does not run,
    /// and a worker blocked on a question nobody can be shown never comes back.
    pub fn tick(&mut self, ui: &mut Ui) {
        self.poll();
        if self.pending.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.dialog_ui(ui);
    }

    /// Draw the group: one row per box. Never edits the session.
    pub fn ui(&mut self, ui: &mut Ui, session: &Session, present: PortsPresent<'_>, blocked: bool, playing: bool) {
        if session.devices.is_empty() {
            ui.weak("No boxes in this session.");
            return;
        }
        // Every box in the session, the A4 included: its whole-pattern send
        // goes through the same ceremony as a digi's since 2026-08-31, so the
        // FRONT-PANEL DUMP group this used to filter it out toward is gone.
        let devices: Vec<DeviceId> = session.devices.iter().map(|d| d.id).collect();
        let last = devices.len().saturating_sub(1);
        for (position, id) in devices.into_iter().enumerate() {
            self.device_ui(ui, session, present, id, blocked, playing);
            if position != last {
                ui.add_space(6.0);
            }
        }
    }

    /// One box's row: `send A01 to A02`, and the button that does it.
    ///
    /// Deliberately the same two pickers and the same layout as `ui::transfer`'s
    /// fetch row, read the other way round — decision 1. The only asymmetry is
    /// the verb on the button, which is `ui::write::is_home`'s and says whether
    /// this is going back where the pattern came from.
    fn device_ui(
        &mut self,
        ui: &mut Ui,
        session: &Session,
        present: PortsPresent<'_>,
        id: DeviceId,
        blocked: bool,
        playing: bool,
    ) {
        let Some(device) = session.device(id) else { return };
        ui.label(egui::RichText::new(&device.name).size(11.0).color(super::TEXT_SECONDARY));

        // Why this box cannot be written to, if it cannot. Said before the
        // pickers rather than behind a dead button — `ui::transfer`'s rule.
        if let Some(reason) = blocker(device, present) {
            ui.weak(reason);
            return;
        }

        let (scene_slot, _) = defaults(session, id);
        let row = self.rows.entry(id).or_default();
        let (mut from, pinned_into) = (row.from.unwrap_or(scene_slot), row.into);

        let busy = blocked || self.pending.is_some() || self.dialog.is_some();
        let in_flight = self.pending.as_ref().is_some_and(|p| p.device == id);
        let from_choices = slot_choices(device);
        let mut send_clicked = false;

        // **Planned before the row is drawn, because the button's enabled state
        // is one of the plan's answers.** A pattern with no notes in it has
        // nothing to send, and an enabled button that does nothing when pressed
        // is worse than a disabled one that says why underneath. Built from
        // where the pickers stand *now*; if one of them moves in this frame the
        // plan is rebuilt below, which happens on the frames a person is
        // choosing and on no others.
        // Computed exactly as the closure below computes it, off the *row's*
        // source rather than the scene's: a row whose source has been pinned
        // elsewhere has its own provenance answer, and the two must agree or the
        // button's enabled state is deciding on a destination the row is not
        // showing.
        let standing = (
            from,
            pinned_into
                .unwrap_or_else(|| aim(device.pattern(from.slot()), device.model.slug, from)),
        );
        let mut plan = plan_box(session, present, id, standing.0, standing.1);
        let sendable = !plan.is_empty();

        // The destination is read *after* the source picker below, for
        // `ui::write::device_ui`'s reason: `from` may move in this very frame,
        // and provenance read before that would pin the destination to where the
        // *previous* source pointed.
        let mut into = standing.1;
        let mut home = false;

        // `horizontal_wrapped`, so the narrow end of a resizable panel folds the
        // button onto a second line instead of pushing it off the edge.
        ui.horizontal_wrapped(|ui| {
            ui.weak("send");
            let mut picked = from;
            egui::ComboBox::from_id_salt(("sync-from", id.0))
                .selected_text(egui::RichText::new(from.label()).color(super::TEXT_DIMMER))
                .width(56.0)
                .show_ui(ui, |ui| {
                    for (slot, text) in &from_choices {
                        ui.selectable_value(&mut picked, *slot, text);
                    }
                });
            from = picked;

            let pattern = device.pattern(from.slot());
            into = pinned_into.unwrap_or_else(|| aim(pattern, device.model.slug, from));
            home = is_home(pattern, device.model.slug, into);

            ui.weak("to");
            let mut picked = into;
            egui::ComboBox::from_id_salt(("sync-into", id.0))
                .selected_text(egui::RichText::new(into.label()).color(super::TEXT_DIMMER))
                .width(56.0)
                .show_ui(ui, |ui| {
                    for slot in wire_slots(device.model) {
                        ui.selectable_value(&mut picked, slot, slot.label());
                    }
                });
            into = picked;

            ui.add_enabled_ui(!busy && sendable, |ui| {
                // "Write back" when it is going home, "Send" when it is not —
                // `ui::write`'s distinction, kept word for word, because the two
                // rows now differ only in how much they send and a person
                // switching between them should not have to relearn the button.
                let verb = if home { "Write back to" } else { "Send to" };
                send_clicked = super::colored_button(
                    ui,
                    format!("{verb} {}", into.label()),
                    super::WARN_AMBER_FILL,
                    super::WARN_AMBER_TEXT,
                    super::WARN_AMBER_BORDER,
                    super::WARN_AMBER,
                    super::WARN_AMBER_INK,
                )
                .on_hover_text(
                    "Put every track of this pattern that has notes onto the box, in the slot \
                     picked here. One dialog lists every track and you can untick any of them; \
                     the slot is re-read and backed up whole before anything is sent, and \
                     verified byte for byte afterwards.",
                )
                .clicked();
            });
        });

        // Pin what the pickers ended up on. `into` is only *pinned* when it has
        // been moved off what it was following, so a source change still drags
        // an untouched destination with it.
        let following = aim(device.pattern(from.slot()), device.model.slug, from);
        let row = self.rows.entry(id).or_default();
        row.from = Some(from);
        if pinned_into.is_some() || into != following {
            row.into = Some(into);
        }

        if (from, into) != standing {
            plan = plan_box(session, present, id, from, into);
        }
        if !in_flight && self.dialog.is_none() {
            // What the button would do, before the press — the same promise
            // `ui::transfer` makes with its "A01 has 17 notes" line.
            match plan.jobs.first() {
                // `>`, not `→`: pre-existing tofu, see `row_label`.
                Some(job) => ui.weak(format!(
                    "{} > {}",
                    plural(job.writes.len(), "track"),
                    job.into.label()
                )),
                None => ui.weak(match plan.blocked.first() {
                    Some(first) => first.why.clone(),
                    None => "Nothing to send from this box.".into(),
                }),
            };
        }

        if send_clicked && !plan.is_empty() {
            self.start(plan, playing);
        }

        if let Some(pending) = &self.pending {
            if in_flight {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&pending.status);
                });
                return;
            }
        }
        self.outcome_ui(ui, id);
    }

    /// The last run's line for one box, if it was this box's run.
    fn outcome_ui(&mut self, ui: &mut Ui, id: DeviceId) {
        let Some(outcome) = self.outcomes.get(&id) else { return };
        let colour = if outcome.is_error {
            egui::Color32::LIGHT_RED
        } else if outcome.wrote {
            egui::Color32::LIGHT_GREEN
        } else {
            super::CAUTION
        };
        ui.colored_label(colour, &outcome.text);
        if let Some(log) = &outcome.log {
            ui.weak(log);
        }
    }

    fn start(&mut self, plan: MassPlan, playing: bool) {
        if self.pending.is_some() || self.dialog.is_some() {
            return;
        }
        // Only this box's last line goes. The other row's result is still true —
        // clearing the panel because a *different* box is being sent would throw
        // away the byte-identical verdict a person may still be reading.
        let Some(device) = plan.jobs.first().map(|j| j.device) else { return };
        self.outcomes.remove(&device);
        let (tx, rx) = channel();
        std::thread::spawn(move || worker(plan, playing, tx));
        self.pending = Some(Pending { device, rx, status: "Opening the box…".into() });
    }

    fn poll(&mut self) {
        let Some(pending) = &mut self.pending else { return };
        loop {
            let Ok(event) = pending.rx.try_recv() else { return };
            match event {
                Event::Status(s) | Event::Log(s) => pending.status = s,
                Event::Ask(ask) => {
                    let ticked = ask.boxes.iter().map(|b| vec![true; b.rows.len()]).collect();
                    self.dialog = Some(Dialog::Confirm { ask, ticked });
                    return;
                }
                Event::Done(report) => {
                    // Anything that did not go as asked is put in front of the
                    // person who pressed, per `ui::write` decision 7 — and at
                    // this scale a failure buried under a green row is worse.
                    let loud: Vec<String> = report
                        .boxes
                        .iter()
                        .filter(|b| b.is_error)
                        .map(|b| format!("{}: {}", b.name, b.text))
                        .collect();
                    if !loud.is_empty() {
                        self.dialog = Some(Dialog::Alert { lines: loud });
                    }
                    // Merged, not replaced: each row keeps its own last answer,
                    // and a send of one box says nothing about the other.
                    self.outcomes.extend(report.boxes.into_iter().map(|b| (b.device, b)));
                    self.pending = None;
                    return;
                }
            }
        }
    }

    fn dialog_ui(&mut self, ui: &mut Ui) {
        let Some(dialog) = &mut self.dialog else { return };
        let mut answer: Option<Option<Vec<(DeviceId, usize)>>> = None;
        let mut dismiss = false;

        let response = egui::Modal::new(egui::Id::new("sync-dialog")).show(ui.ctx(), |ui| {
            ui.set_max_width(620.0);
            match dialog {
                Dialog::Confirm { ask, ticked } => {
                    ui.label(egui::RichText::new("Send every track to the box").strong());
                    ui.separator();

                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for (b, block) in ask.boxes.iter().enumerate() {
                            ui.label(egui::RichText::new(&block.heading).strong());
                            for (r, row) in block.rows.iter().enumerate() {
                                ui.checkbox(&mut ticked[b][r], &row.label);
                                for warning in &row.warnings {
                                    ui.indent(("w", b, r), |ui| {
                                        ui.colored_label(super::CAUTION, format!("Note: {warning}"));
                                    });
                                }
                            }
                            for line in &block.lines {
                                ui.weak(line);
                            }
                            for line in &block.skipped {
                                ui.weak(egui::RichText::new(format!("Not sent — {line}")).italics());
                            }
                            ui.add_space(8.0);
                        }
                        for line in &ask.blocked {
                            ui.colored_label(super::CAUTION, format!("Left out — {line}"));
                        }
                    });

                    ui.separator();
                    ui.label(ask.backup_line);
                    ui.separator();

                    let mut picked: Vec<(DeviceId, usize)> = Vec::new();
                    for (b, block) in ask.boxes.iter().enumerate() {
                        for (r, row) in block.rows.iter().enumerate() {
                            if ticked[b][r] {
                                picked.push((block.device, row.track_index));
                            }
                        }
                    }

                    ui.horizontal(|ui| {
                        // Cancel first and on the left, because it is the answer
                        // a hesitating hand should land on.
                        if ui.button("Cancel").clicked() {
                            answer = Some(None);
                        }
                        ui.add_enabled_ui(!picked.is_empty(), |ui| {
                            if ui.button(headline(picked.len())).clicked() {
                                answer = Some(Some(picked.clone()));
                            }
                        });
                    });
                }
                Dialog::Alert { lines } => {
                    ui.label(egui::RichText::new("The send did not go as asked").strong());
                    ui.separator();
                    for line in lines.iter() {
                        ui.label(line);
                    }
                    ui.separator();
                    if ui.button("OK").clicked() {
                        dismiss = true;
                    }
                }
            }
        });
        // Clicking away or pressing Escape is a cancel, which is only correct
        // because the destructive answer is never the default one here.
        if response.should_close() {
            match dialog {
                Dialog::Confirm { .. } => answer = Some(None),
                Dialog::Alert { .. } => dismiss = true,
            }
        }

        if dismiss {
            self.dialog = None;
            return;
        }
        let Some(answer) = answer else { return };
        if let Some(Dialog::Confirm { ask, .. }) = self.dialog.take() {
            // A send that fails means the worker has gone, and a worker that has
            // gone consented to nothing.
            let _ = ask.reply.send(answer);
        }
    }
}

// --- reading patch names off one box ---------------------------------------------
//
// Packet E, stage 2 (2026-08-20). Smaller and safer than everything above it in
// this file: this asks a box for its current kit and updates `Track.patch` on
// whichever pattern is already showing for that device — nothing else. No
// notes, no lengths, no swing, no dialog with an `Overwrite` button: rules 1
// and 4 are the write path's and do not apply to a read (packet E's own text
// says so explicitly). What *does* apply is that every ordinary way this can
// fail has to say so on screen, in its own words, because a read that fails
// silently and leaves the old patch names in place is the exact bug this
// feature exists to fix, one level up — see `ui::tracks::patch_line`'s "none"
// branch.
//
// **Which slot to ask for is named, never guessed.** [`patch_read_job`] takes
// it from one of two places and no third: the pattern's own `source` — the same
// field `ui::write::aim` consults, and where `aim` falls back to a
// same-numbered slot this refuses instead (`PatchReadError::UnknownSlot`) — or
// the `asked` slot its caller's picker is showing. The second was added on
// 2026-08-20 after Neil read the first one's refusal on a pattern he had built
// in this app and asked the obvious question back: nothing had been fetched,
// so the refusal told him to fetch first, when what he wanted was simply the
// names the box has on its sixteen tracks right now. A picker answers that
// without either half of the bad trade — the app still guesses nothing, and
// the person who knows which pattern the box is sitting on gets to say.
//
// What it cannot claim is "live": a dump request names a stored slot, and
// nothing in the Elektron dump protocol as implemented here asks a box what it
// is currently playing. Reading A01 gets A01's saved kit, which is what is
// live when the box is on A01 with no unsaved kit edits — so the picker's
// tooltip says that rather than promising more.

/// Why a patch-names read cannot even be attempted, decided without opening a
/// port. Covers two of the four ordinary failures packet E's addendum names:
/// **no box** (no ports assigned at all) and **cable gone** (a port is
/// assigned but the OS no longer lists it — `PortsPresent::holds` is the same
/// check `ui::write::blocker` makes for a write, because a macOS connection
/// object stays alive and goes nowhere once the cable is pulled).
///
/// The other two — **handshake refused** and **wrong firmware** — can only be
/// discovered by actually asking the box, so they belong to [`read_patch_kit`]
/// instead.
pub fn patch_read_blocker(device: &Device, present: PortsPresent<'_>) -> Option<String> {
    // **The route, not `can_sysex`.** This asked `can_sysex` until 2026-09-01,
    // and that field means "has a gen-2 `Spec`" — so it refused the Analog Four
    // with a sentence that was wrong in both halves. The box has not been
    // live-only since 2026-08-31, and its kit carries the four synth tracks'
    // sound names all along (`protocol::a4_kit`, mapped from captures that were
    // already on disk). What actually cannot be read is a box whose patterns do
    // not move at all, and `PatternRoute::LiveOnly` is the row that says so.
    if !device.pattern_route().transfers() {
        return Some(format!(
            "{} plays over MIDI but has no patch names to read",
            device.model.display
        ));
    }
    match (&device.io.input, &device.io.output) {
        (Some(input), Some(output)) => {
            let gone = match (
                PortsPresent::holds(present.inputs, input),
                PortsPresent::holds(present.outputs, output),
            ) {
                (true, true) => return None,
                (true, false) => output.name.clone(),
                _ => input.name.clone(),
            };
            Some(format!(
                "{gone} is no longer plugged in — reconnect the box to read its patch names"
            ))
        }
        (None, None) => Some("No ports set — pick an in and an out for this box above".into()),
        (None, Some(_)) => Some("No in port — the box's answer comes back on it".into()),
        (Some(_), None) => Some("No out port — the request goes out on it".into()),
    }
}

/// One box's patch-names read, resolved and ready to open a port — everything
/// [`read_patch_kit`] needs, decided in advance so "ask rather than assume" is
/// testable without a port in sight.
#[derive(Debug, Clone)]
pub struct PatchJob {
    pub device: DeviceId,
    pub name: String,
    pub display: &'static str,
    pub slug: Option<&'static str>,
    /// How this box's patterns move, which is also what decides which kit
    /// request [`read_patch_kit`] sends. See [`PatchKit`].
    pub route: PatternRoute,
    /// `None` on a gen-1 box: the A4 has no gen-2 `Spec` and never will, and it
    /// does not need one to name its sounds.
    pub spec: Option<&'static Spec>,
    pub input: PortBinding,
    pub output: PortBinding,
    /// The session slot whose tracks get the patch records — the pattern on
    /// screen for this box right now (the scene's slot, same default
    /// [`defaults`] offers a send's `from`).
    pub at: PatternRef,
    /// Which of the box's slots to ask for. Either the slot the pattern says
    /// it came from, or the one the caller named in the row's picker — never a
    /// slot this app worked out on its own. See [`patch_read_job`]'s `asked`.
    pub source: Source,
}

impl PatchJob {
    /// The box's own slot to fetch — `source` translated into the wire's
    /// vocabulary, since a box does not know what a `Source` is.
    pub fn from(&self) -> PatternRef {
        PatternRef::new(self.source.bank, self.source.index)
    }
}

/// Resolve one box's patch-names read, or say why it cannot be attempted yet.
///
/// Pure and box-free: everything it decides — no ports, a cable the OS no
/// longer lists, a pattern with no provenance to read a slot from — is decided
/// without a fetch in sight, which is what makes the "ask rather than assume"
/// rule testable dry.
///
/// `asked` is the slot the caller's own picker is showing, when it has one:
/// with `None` the slot is resolved from the pattern's `source` and a pattern
/// that has none refuses (`UnknownSlot`), and with `Some` that slot is read
/// because a person named it. The UI passes `Some` — see
/// [`digi_core::import::patch_read_source_named`] for why "said out loud" is a
/// different thing from "guessed", and `ui::devices`' patch-read section for
/// what the picker defaults to.
pub fn patch_read_job(
    session: &Session,
    device: DeviceId,
    present: PortsPresent<'_>,
    asked: Option<PatternRef>,
) -> Result<PatchJob, String> {
    let d = session.device(device).ok_or_else(|| "no such box in this session".to_string())?;
    if let Some(why) = patch_read_blocker(d, present) {
        return Err(why);
    }
    let (Some(input), Some(output)) = (d.io.input.clone(), d.io.output.clone()) else {
        return Err("this box has no ports set".into());
    };
    // **A `Spec` is no longer required to get here.** It was, until 2026-09-01,
    // and that is the same `can_sysex` mistake one layer down: the spec is how a
    // *gen-2* kit is decoded, not what says a box has kit names. A gen-1 box
    // carries `None` and [`read_patch_kit`] dispatches on the route instead.
    let spec = d.model.spec();
    let route = d.model.pattern_route();
    let at = session.slot_in_scene(session.current_scene, device).unwrap_or_else(|| PatternRef::new(0, 0));
    let pattern = d.pattern(at.slot());
    let source = match asked {
        Some(slot) => {
            // A box with no slug is a box with no dump format, which is
            // `patch_wrong_box`'s own wording for the same fact — reused
            // rather than reworded, since the read is refused for one reason.
            let slug = d
                .model
                .slug
                .ok_or_else(|| format!("{} has no pattern dumps to read", d.model.display))?;
            digi_core::import::patch_read_source_named(pattern, slug, slot)
        }
        None => digi_core::import::patch_read_source(pattern, d.model.slug),
    }
    .map_err(|e| e.to_string())?;
    Ok(PatchJob {
        device,
        name: d.name.clone(),
        display: d.model.display,
        slug: d.model.slug,
        route,
        spec,
        input: binding(&input),
        output: binding(&output),
        at,
        source,
    })
}

/// Why the box that answered is not the one `job.source` says the pattern came
/// from — the read path's twin of `ui::write::wrong_box`, worded for a read
/// rather than borrowing that function's "refusing to write" sentence, which
/// would be a lie on this path.
fn patch_wrong_box(job: &PatchJob, identity: &DeviceIdentity) -> Option<String> {
    match job.slug {
        Some(slug) if slug == identity.slug => None,
        Some(_) => Some(format!(
            "this row is the {} and the box on those ports says it's a {} — refusing to read its \
             patch names. The pattern's own record says where it came from, and this is not that \
             box.",
            job.display, identity.name
        )),
        None => Some(format!("{} has no pattern dumps to read", job.display)),
    }
}

/// Ask the box for the kit at `job.source`, read-only: identify, then one
/// pattern-kit request. The two round trips carry the other two ordinary
/// failures packet E's addendum names — **handshake refused**
/// (`device.identity()` answers nothing) and **wrong firmware** (the box
/// answered, but this build's decoder does not recognise the pattern struct
/// version it sent back, which `decode_pattern_kit` reports by itself).
///
/// Nothing here touches a [`Session`]. A caller applies the result with
/// [`Session::apply_patch_read`][digi_core::Session::apply_patch_read] only on
/// `Ok`, so a failure at any point — including the decode — leaves every
/// existing patch record exactly as it was.
pub fn read_patch_kit(device: &mut impl PatternIo, job: &PatchJob) -> Result<PatchKit, String> {
    let identity = device
        .identity()
        .cloned()
        .ok_or_else(|| "the box did not answer the identity handshake".to_string())?;
    if let Some(refusal) = patch_wrong_box(job, &identity) {
        return Err(refusal);
    }
    match job.route {
        // **The A4 asks a different question, and gets a better answer.** The
        // paragraph above this function's section says a dump request can only
        // name a *stored* slot and that nothing in this protocol asks a box what
        // it is playing. On this box something does: `0x68` returns the edit
        // buffer, unsaved kit edits included. So the gen-1 read takes no slot at
        // all — see `PatchKit::Gen1` and `TrackPatch::live`.
        PatternRoute::RequestGen1 => {
            let bytes = device.fetch_a4_working_kit()?;
            parse_working_kit(&build_dump_message(
                FAMILY_ANALOG_FOUR,
                DUMP_A4_KIT_WORKING,
                0,
                &bytes,
            ))
            .map(PatchKit::Gen1)
        }
        _ => {
            let spec = job.spec.ok_or_else(|| {
                format!("{} has no pattern format this build can decode", job.display)
            })?;
            let index = job.from().wire_index().ok_or_else(|| {
                format!("{} is past the last slot a dump request can name", job.from().label())
            })?;
            let bytes = device.fetch_pattern_kit(index)?;
            decode_pattern_kit(spec, &bytes).map(PatchKit::Gen2)
        }
    }
}

/// A kit read for its patch names, in whichever shape its box speaks.
///
/// **Two variants rather than one normalised struct**, because what a caller
/// does with them differs beyond the names: a gen-2 kit names a stored slot and
/// can go stale against the pattern on screen, and a gen-1 one is the box's edit
/// buffer and cannot. Flattening them here would push that distinction into a
/// boolean beside a struct that had already lost it.
#[derive(Debug, Clone)]
pub enum PatchKit {
    /// A digi's kit, out of the combined pattern-kit dump at a named slot.
    Gen2(PatternKit),
    /// The Analog Four's working kit — `protocol::a4_kit`, four sounds, no slot.
    Gen1(A4Kit),
}

impl PatchKit {
    /// Was this read off the box's edit buffer rather than a stored slot?
    /// [`digi_core::model::TrackPatch::live`] is what this becomes.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Gen1(_))
    }

    /// The kit's own name, which both shapes have.
    pub fn name(&self) -> &str {
        match self {
            Self::Gen2(k) => &k.kit.name,
            Self::Gen1(k) => &k.name,
        }
    }
}

#[cfg(test)]
mod patch_read_tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use digi_core::device::{model_for_key, Device as CoreDevice, DeviceIo, DT2};
    use digi_core::import::Fetched;
    use digi_core::session::PatternRef as Slot;
    use digi_protocol::device::{identity_from_responses, DeviceResponse};
    use digi_protocol::protocol::{split_sysex_stream, SysExKind, DUMP_PATTERN_KIT};

    use super::*;

    const DT2_FIXTURE: &str = "digitakt2-A01-conditions-2026-08-02.syx";

    fn payload(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../protocol/tests/fixtures")
            .join(name);
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| panic!("reading fixture {}: {e}", path.display()));
        split_sysex_stream(&bytes)
            .into_iter()
            .filter(|m| m.kind == SysExKind::Dump)
            .filter_map(|m| m.dump)
            .find(|d| d.dump_type == DUMP_PATTERN_KIT)
            .map(|d| d.payload)
            .unwrap_or_else(|| panic!("{name}: no pattern-kit dump"))
    }

    fn identity(product_id: u8, build: &str) -> DeviceIdentity {
        identity_from_responses(
            &DeviceResponse { product_id, supported_ids: vec![0x60], reported_name: String::new() },
            build.into(),
            "1.15B".into(),
        )
    }

    fn dt2_identity() -> DeviceIdentity {
        identity(42, "0070")
    }

    fn port(name: &str) -> digi_core::PortRef {
        digi_core::PortRef { id: name.into(), name: name.into() }
    }

    /// A fake box holding one slot's raw bytes, answering [`PatternIo`] the
    /// same way `app/tests/sync.rs`'s `FakeBox` does — no port, no thread,
    /// just the two round trips the trait names.
    #[derive(Clone)]
    struct FakeBox {
        identity: Option<DeviceIdentity>,
        slots: Rc<RefCell<BTreeMap<u8, Vec<u8>>>>,
        fetches: Rc<RefCell<usize>>,
    }

    impl FakeBox {
        fn new(identity: DeviceIdentity, index: u8, bytes: Vec<u8>) -> Self {
            Self {
                identity: Some(identity),
                slots: Rc::new(RefCell::new(BTreeMap::from([(index, bytes)]))),
                fetches: Rc::new(RefCell::new(0)),
            }
        }

        fn silent() -> Self {
            Self { identity: None, slots: Rc::new(RefCell::new(BTreeMap::new())), fetches: Rc::new(RefCell::new(0)) }
        }

        fn fetches(&self) -> usize {
            *self.fetches.borrow()
        }
    }

    impl PatternIo for FakeBox {
        fn identity(&self) -> Option<&DeviceIdentity> {
            self.identity.as_ref()
        }

        fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
            *self.fetches.borrow_mut() += 1;
            self.slots.borrow().get(&index).cloned().ok_or_else(|| format!("no slot {index}"))
        }

        fn send_pattern_kit(&mut self, _index: u8, _payload: &[u8]) -> Result<(), String> {
            unreachable!("a patch-names read never sends")
        }

        /// The A4's edit buffer, keyed off the same map under a sentinel that
        /// is not a slot — because the whole point of `0x68` is that it names
        /// no slot, and a fake that served it out of index 0 would let a read
        /// that wrongly asked for slot 0 pass.
        fn fetch_a4_working_kit(&mut self) -> Result<Vec<u8>, String> {
            *self.fetches.borrow_mut() += 1;
            self.slots
                .borrow()
                .get(&WORKING_KIT)
                .cloned()
                .ok_or_else(|| "this fake holds no working kit".to_string())
        }
    }

    /// Not a wire index: the key `FakeBox` files its edit buffer under. `0xFF`
    /// is past the last slot any dump request can name.
    const WORKING_KIT: u8 = 0xFF;

    /// A session with one DT2, the A01 fixture imported into slot A01 — so the
    /// pattern's own `source` names A01, exactly what a real fetch-then-read
    /// round trip would leave behind.
    fn session_with_fixture() -> (Session, DeviceId) {
        let mut session = Session::default();
        let id = session.add_device(CoreDevice::new("DT2", &DT2, 16));
        let spec = model_for_key("DT2").and_then(|m| m.spec()).expect("DT2 has a spec");
        let bytes = payload(DT2_FIXTURE);
        let kit = decode_pattern_kit(spec, &bytes).expect("fixture decodes");
        session
            .import_pattern(
                id,
                Slot::new(0, 0),
                &Fetched { spec, kit: &kit, payload: &bytes, from: Slot::new(0, 0) },
            )
            .expect("a DT2 fixture into a DT2 slot");
        let device = session.device_mut(id).expect("just added");
        device.io = DeviceIo {
            input: Some(port("dt2-in")),
            output: Some(port("dt2-out")),
            ..DeviceIo::default()
        };
        (session, id)
    }

    fn present<'a>(inputs: &'a [digi_midi::PortInfo], outputs: &'a [digi_midi::PortInfo]) -> PortsPresent<'a> {
        PortsPresent { inputs, outputs }
    }

    // --- the success path -------------------------------------------------------

    #[test]
    fn a_successful_read_populates_all_sixteen_tracks_including_a_forced_midi_one() {
        let (mut session, id) = session_with_fixture();
        let job = patch_read_job(&session, id, PortsPresent::unknown(), None).expect("A01 has a source");
        assert_eq!(job.from(), PatternRef::new(0, 0));

        // Force track 2 to read as MIDI, and leave track 3's sound name blank,
        // so one read exercises all three `PatchSound` shapes at once — the
        // requirement this packet names by name: "including MIDI tracks".
        let spec = job.spec.expect("a DT2 has a gen-2 spec");
        let mut kit = decode_pattern_kit(spec, &payload(DT2_FIXTURE)).unwrap();
        kit.kit.midi_mask |= 1 << 1;
        kit.kit.sound_names[2] = String::new();

        // The fake answers with the *real, unmodified* fixture bytes, so the
        // fetch round trip — identity, the wrong-box check, one
        // `fetch_pattern_kit` call, one real decode — is exercised end to end
        // against genuine bytes rather than anything hand-built.
        let mut fake = FakeBox::new(dt2_identity(), 0, payload(DT2_FIXTURE));
        let fetched_kit = read_patch_kit(&mut fake, &job).expect("the fake answers");
        assert_eq!(fake.fetches(), 1);
        // Real fixture kits have no MIDI-masked or unnamed tracks to fetch (the
        // condition captures are a plain audio kit with all sixteen sounds
        // named), so `apply_patch_read` is driven against the mutated `kit`
        // above rather than `fetched_kit` — there is no encoder in this crate
        // to turn a hand-edited mask bit back into bytes, and hand-rolling one
        // would only be re-deriving `decode_pattern_kit`. The two are checked
        // against each other below so this substitution cannot quietly drift
        // from what a real decode produces.
        let PatchKit::Gen2(fetched_kit) = fetched_kit else {
            panic!("a DT2 reads as a gen-2 kit");
        };
        assert_eq!(fetched_kit.tracks.len(), kit.tracks.len());
        let count = session
            .apply_patch_read(id, job.at, &kit, &job.source, 1_787_184_000)
            .expect("all sixteen tracks patch");
        assert_eq!(count, 16);

        let pattern = session.device(id).unwrap().pattern(0).unwrap();
        for t in 0..16 {
            let patch = pattern.track(t).unwrap().patch.clone();
            assert!(patch.is_some(), "track {t} must carry a record — every track was fetched");
        }
        assert_eq!(
            pattern.track(1).unwrap().patch.as_ref().unwrap().sound,
            digi_core::model::PatchSound::Midi,
            "the forced MIDI track gets the MIDI shape, not a blank sound name"
        );
        assert_eq!(
            pattern.track(2).unwrap().patch.as_ref().unwrap().sound,
            digi_core::model::PatchSound::Unnamed,
            "the forced blank sound name gets Unnamed, not an empty-string sentinel"
        );
        assert!(matches!(
            pattern.track(0).unwrap().patch.as_ref().unwrap().sound,
            digi_core::model::PatchSound::Named(_)
        ));
    }

    // --- the Analog Four reads its edit buffer ----------------------------------
    //
    // The last thing this box could not do that both digis could, closed
    // 2026-09-01. Everything here runs against a real `0x58` capture off the
    // box; nothing is hand-built but the session around it.

    const A4_KIT_FIXTURE: &str = "analogfour-kit-00-working-2026-08-31.syx";

    fn a4_working_kit_message() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../protocol/tests/fixtures")
            .join(A4_KIT_FIXTURE);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The payload inside that message, which is what a real `fetch_dump`
    /// hands back and therefore what the fake must serve.
    fn a4_working_kit_payload() -> Vec<u8> {
        digi_protocol::protocol::parse_sysex(&a4_working_kit_message())
            .dump
            .expect("a dump message")
            .payload
    }

    fn a4_session() -> (Session, DeviceId) {
        let mut session = Session::default();
        let id = session.add_device(CoreDevice::new("A4", &digi_core::device::A4, 16));
        let device = session.device_mut(id).expect("just added");
        device.io = DeviceIo {
            input: Some(port("a4-in")),
            output: Some(port("a4-out")),
            ..DeviceIo::default()
        };
        (session, id)
    }

    fn a4_identity() -> DeviceIdentity {
        identity_from_responses(
            &DeviceResponse {
                product_id: 4,
                supported_ids: vec![0x60],
                reported_name: "Analog Four".into(),
            },
            "0195".into(),
            "1.55B".into(),
        )
    }

    /// **The refusal that was wrong in both halves.** `patch_read_blocker`
    /// asked `can_sysex`, which means "has a gen-2 `Spec`", and told anyone
    /// with an A4 that it "plays over MIDI but has no patch names to read".
    /// The box has moved patterns since 2026-08-31 and its kit carries the
    /// names; what the blocker is actually about is a box whose patterns do not
    /// move at all.
    #[test]
    fn an_a4_is_no_longer_refused_a_patch_names_read() {
        let (session, id) = a4_session();
        let d = session.device(id).unwrap();
        assert!(!d.can_sysex(), "and it still has no gen-2 spec — that was never the question");
        assert_eq!(patch_read_blocker(d, PortsPresent::unknown()), None);

        let job = patch_read_job(&session, id, PortsPresent::unknown(), Some(Slot::new(0, 0)))
            .expect("a gen-1 box resolves a job without a spec");
        assert_eq!(job.route, PatternRoute::RequestGen1);
        assert!(job.spec.is_none(), "and needs none");
    }

    /// The read itself: one `0x68`, four sound names off the box's own capture,
    /// and no slot request at all.
    #[test]
    fn an_a4_read_asks_for_the_edit_buffer_and_names_four_sounds() {
        let (session, id) = a4_session();
        let job = patch_read_job(&session, id, PortsPresent::unknown(), Some(Slot::new(0, 0)))
            .expect("a job");

        let mut fake = FakeBox::new(a4_identity(), WORKING_KIT, a4_working_kit_payload());
        let kit = read_patch_kit(&mut fake, &job).expect("the fake answers");
        assert_eq!(fake.fetches(), 1, "one round trip");

        let PatchKit::Gen1(kit) = kit else { panic!("an A4 reads as a gen-1 kit") };
        assert_eq!(kit.name, "POLYTRON");
        assert_eq!(
            (0..4).map(|t| kit.sound_name(t).unwrap()).collect::<Vec<_>>(),
            ["ARPME", "WAVE MOD LEAD", "ALONE", "BRE"],
        );
        assert!(PatchKit::Gen1(kit).is_live(), "the edit buffer is not a stored slot");
    }

    /// **A gen-1 read never asks for a slot**, which is what makes hiding the
    /// picker honest rather than cosmetic. The fake holds a payload at slot 0
    /// *as well as* its edit buffer; a read that fell through to
    /// `fetch_pattern_kit` would find it and quietly succeed with the wrong
    /// bytes, so the slot map is loaded on purpose here.
    #[test]
    fn an_a4_read_never_reaches_the_slot_request() {
        let (session, id) = a4_session();
        let job = patch_read_job(&session, id, PortsPresent::unknown(), Some(Slot::new(0, 0)))
            .expect("a job");

        let mut fake = FakeBox::new(a4_identity(), 0, payload(DT2_FIXTURE));
        let err = read_patch_kit(&mut fake, &job).unwrap_err();
        assert!(err.contains("no working kit"), "it asked the slot map instead: {err}");
    }

    /// The four sounds land on six tracks, and the two the kit holds nothing
    /// for say so rather than reading as unnamed slots a later read might fill.
    #[test]
    fn an_a4_patches_six_tracks_and_the_last_two_have_no_sound() {
        use digi_core::model::PatchSound;

        let (mut session, id) = a4_session();
        let job = patch_read_job(&session, id, PortsPresent::unknown(), Some(Slot::new(0, 0)))
            .expect("a job");
        let mut fake = FakeBox::new(a4_identity(), WORKING_KIT, a4_working_kit_payload());
        let PatchKit::Gen1(kit) = read_patch_kit(&mut fake, &job).unwrap() else {
            panic!("gen-1")
        };

        let model = session.device(id).unwrap().model;
        let sounds = digi_core::a4_transfer::a4_patch_sounds(model, &kit);
        let count = session
            .apply_patch_sounds(
                id,
                job.at,
                &sounds,
                &kit.name,
                kit.index,
                &job.source,
                1_787_184_000,
                true,
            )
            .expect("six tracks patch");
        assert_eq!(count, 6, "SYN1-4, FX, CV");

        let pattern = session.device(id).unwrap().pattern(0).unwrap();
        let sound = |t: usize| pattern.track(t).unwrap().patch.as_ref().unwrap().sound.clone();
        assert_eq!(sound(0), PatchSound::Named("ARPME".into()));
        assert_eq!(sound(3), PatchSound::Named("BRE".into()));
        assert_eq!(sound(4), PatchSound::NoSound, "FX is the sequencer's track, not the kit's");
        assert_eq!(sound(5), PatchSound::NoSound, "and so is CV");

        for t in 0..6 {
            assert!(
                pattern.track(t).unwrap().patch.as_ref().unwrap().live,
                "track {t}: read off the edit buffer, so it claims no slot",
            );
        }
    }

    // --- the "ask rather than assume" refusal -----------------------------------

    #[test]
    fn a_pattern_with_no_source_refuses_rather_than_guessing_a01() {
        let mut session = Session::default();
        let id = session.add_device(CoreDevice::new("DT2", &DT2, 16));
        let device = session.device_mut(id).expect("just added");
        device.io =
            DeviceIo { input: Some(port("in")), output: Some(port("out")), ..DeviceIo::default() };
        // A pattern written in this app from scratch: never fetched, so it has
        // no `source` at all. Slot A01 happens to be empty too, which is the
        // trap — a careless resolver could default to it silently.
        let err = patch_read_job(&session, id, PortsPresent::unknown(), None).unwrap_err();
        assert_eq!(
            err,
            digi_core::import::PatchReadError::UnknownSlot.to_string(),
            "{err}"
        );
        assert!(
            err.contains("no record of which slot"),
            "the refusal has to say why, not just that it failed: {err}"
        );
    }

    // --- a slot the caller named ------------------------------------------------

    #[test]
    fn a_named_slot_is_read_even_when_the_pattern_has_no_source() {
        let mut session = Session::default();
        let id = session.add_device(CoreDevice::new("DT2", &DT2, 16));
        let device = session.device_mut(id).expect("just added");
        device.io =
            DeviceIo { input: Some(port("in")), output: Some(port("out")), ..DeviceIo::default() };
        // The same pattern the test above refuses to resolve a slot for. The
        // difference is not the pattern, it is that someone said which slot.
        let job = patch_read_job(&session, id, PortsPresent::unknown(), Some(PatternRef::new(1, 2)))
            .expect("a named slot needs no provenance");
        assert_eq!(job.from(), PatternRef::new(1, 2), "the slot asked for is the slot fetched");
        assert_eq!(job.at, PatternRef::new(0, 0), "and it lands on the pattern that is on screen");
        assert_eq!(job.source.device_slug, "digitakt2", "the record still names the box it came off");
    }

    #[test]
    fn a_named_slot_does_not_override_which_box_a_pattern_came_off() {
        // A slot number is the user's to choose; the box is not. A DN2-sourced
        // pattern under a DT2 row is still refused, however clearly the slot
        // was asked for — reading anyway would file the DT2's kit names under a
        // pattern whose own record says DN2.
        let (mut session, id) = session_with_fixture();
        let at = Slot::new(0, 0);
        let pattern = session.device_mut(id).unwrap().pattern_mut(at.slot()).unwrap();
        pattern.source = Some(Source { device_slug: "digitone2".into(), bank: 0, index: 0 });

        let err = patch_read_job(&session, id, PortsPresent::unknown(), Some(PatternRef::new(0, 5)))
            .unwrap_err();
        assert_eq!(
            err,
            digi_core::import::PatchReadError::NotThisBox { pattern_slug: "digitone2".into() }.to_string(),
            "{err}"
        );
    }

    // --- the four ordinary failures ----------------------------------------------

    #[test]
    fn no_box_no_ports_set_at_all() {
        let (session, id) = session_with_fixture();
        let device = session.device(id).unwrap();
        assert!(patch_read_blocker(device, PortsPresent::unknown()).is_none(), "unknown present never blocks");

        let mut session = session;
        session.device_mut(id).unwrap().io = DeviceIo::default();
        let device = session.device(id).unwrap();
        assert_eq!(
            patch_read_blocker(device, PortsPresent::unknown()),
            Some("No ports set — pick an in and an out for this box above".to_string())
        );
    }

    #[test]
    fn cable_gone_a_port_the_os_no_longer_lists() {
        let (session, id) = session_with_fixture();
        let device = session.device(id).unwrap();
        // `PortsPresent::holds` treats a genuinely *empty* list as "not
        // enumerated yet" rather than "unplugged" (its own documented rule),
        // so the test has to give it a list that is non-empty but does not
        // contain this box's ports — that is what makes a port read as gone.
        let elsewhere = [digi_midi::PortInfo {
            id: "elsewhere".into(),
            name: "Some Other Port".into(),
            slug: None,
        }];
        let why = patch_read_blocker(device, present(&elsewhere, &elsewhere))
            .expect("this box's ports are not in a non-empty list that lacks them");
        assert!(
            why.contains("is no longer plugged in — reconnect the box to read its patch names"),
            "{why}"
        );
    }

    #[test]
    fn handshake_refused_the_box_does_not_answer_identity() {
        let (session, id) = session_with_fixture();
        let job = patch_read_job(&session, id, PortsPresent::unknown(), None).expect("A01 has a source");
        let mut fake = FakeBox::silent();
        let err = read_patch_kit(&mut fake, &job).unwrap_err();
        assert_eq!(err, "the box did not answer the identity handshake");
        // Nothing was ever applied — the failure happened before there was
        // anything to apply.
        let unaffected = session.device(id).unwrap().pattern(0).unwrap().track(0).unwrap().patch.clone();
        assert!(unaffected.is_some(), "the fixture's own import already gave this a record");
        // The record is exactly what the import left, not a second write.
        assert_eq!(unaffected.unwrap().from, Source { device_slug: "digitakt2".into(), bank: 0, index: 0 });
    }

    #[test]
    fn wrong_firmware_a_pattern_struct_version_this_build_does_not_decode() {
        let (session, id) = session_with_fixture();
        let job = patch_read_job(&session, id, PortsPresent::unknown(), None).expect("A01 has a source");
        let mut bytes = payload(DT2_FIXTURE);
        // The pattern struct version is the first four bytes, big-endian
        // (`decode_pattern_kit`'s own read). A version this build's
        // `spec.pattern_versions` does not list is exactly what a newer
        // firmware would send.
        bytes[0..4].copy_from_slice(&999u32.to_be_bytes());
        let mut fake = FakeBox::new(dt2_identity(), 0, bytes);
        let err = read_patch_kit(&mut fake, &job).unwrap_err();
        assert!(err.contains("unsupported") && err.contains("version"), "{err}");
        assert_eq!(fake.fetches(), 1, "the fetch happened; only the decode refused");
    }

    #[test]
    fn a_box_that_answers_as_the_wrong_model_is_refused_without_a_write_worded_sentence() {
        let (session, id) = session_with_fixture();
        let job = patch_read_job(&session, id, PortsPresent::unknown(), None).expect("A01 has a source");
        // A DN2 answering on the DT2's cabled ports — the mis-cabled-desk case,
        // read straight off `identity_from_responses` with the DN2's product id.
        let dn2 = identity_from_responses(
            &DeviceResponse { product_id: 43, supported_ids: vec![0x60], reported_name: String::new() },
            "0049".into(),
            "1.10D".into(),
        );
        let mut fake = FakeBox::new(dn2, 0, payload(DT2_FIXTURE));
        let err = read_patch_kit(&mut fake, &job).unwrap_err();
        assert!(err.contains("says it's a"), "{err}");
        assert!(!err.contains("refusing to write"), "a read's refusal must not say \"write\": {err}");
        assert!(err.contains("refusing to read"), "{err}");
    }

    // --- no patch record changes on any failure ----------------------------------

    #[test]
    fn no_failure_mode_touches_an_existing_patch_record() {
        let (session, id) = session_with_fixture();
        let before: Vec<_> = session
            .device(id)
            .unwrap()
            .pattern(0)
            .unwrap()
            .tracks()
            .iter()
            .map(|t| t.patch.clone())
            .collect();

        // Run every failure path that can be reached with a `PatternIo` alone
        // (the blocker cases never open a device at all, so they are covered
        // by construction) and confirm the session is byte-for-byte the same
        // afterwards — `read_patch_kit` never touches a `Session`, so this is
        // really pinning that `apply_patch_read` is only ever called on `Ok`.
        let job = patch_read_job(&session, id, PortsPresent::unknown(), None).expect("A01 has a source");
        let mut silent = FakeBox::silent();
        assert!(read_patch_kit(&mut silent, &job).is_err());

        let mut bad_version = FakeBox::new(dt2_identity(), 0, {
            let mut b = payload(DT2_FIXTURE);
            b[0..4].copy_from_slice(&999u32.to_be_bytes());
            b
        });
        assert!(read_patch_kit(&mut bad_version, &job).is_err());

        let after: Vec<_> = session
            .device(id)
            .unwrap()
            .pattern(0)
            .unwrap()
            .tracks()
            .iter()
            .map(|t| t.patch.clone())
            .collect();
        assert_eq!(before, after, "a session nothing was ever applied to must not have moved");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_names_the_count_and_gets_the_singulars_right() {
        assert_eq!(headline(12), "Overwrite 12 tracks");
        assert_eq!(headline(1), "Overwrite 1 track");
        // Nothing ticked is a button that is disabled rather than one that lies,
        // but the words still have to be right when it is drawn.
        assert_eq!(headline(0), "Overwrite 0 tracks");
    }

    fn aim(notes: usize, lanes: usize, name: Option<&str>) -> TrackAim {
        TrackAim {
            track_index: 0,
            name: name.map(str::to_string),
            notes,
            lanes,
            warnings: Vec::new(),
        }
    }

    fn seen(existing_trigs: usize, kind: &str) -> TrackSurvey {
        TrackSurvey {
            track_index: 0,
            existing_trigs,
            kind: kind.into(),
            box_plocks: Vec::new(),
        }
    }

    #[test]
    fn a_row_says_what_is_going_and_what_it_lands_on() {
        assert_eq!(
            row_label(&aim(4, 2, Some("BASS")), Some(&seen(8, "BD"))),
            "T1 BASS > BD — 4 notes and 2 p-lock lanes, replacing 8 trigs"
        );
        // An empty destination is stated rather than left silent: "onto an empty
        // track" is the difference between adding and replacing.
        assert_eq!(row_label(&aim(1, 0, None), Some(&seen(0, ""))), "T1 — 1 note, onto an empty track");
    }

    #[test]
    fn a_track_the_box_and_the_session_call_the_same_thing_is_not_said_twice() {
        assert_eq!(
            row_label(&aim(2, 0, Some("BD")), Some(&seen(1, "BD"))),
            "T1 BD — 2 notes, replacing 1 trig"
        );
    }

    #[test]
    fn a_box_that_moved_between_the_dialog_and_the_write_refuses() {
        // Decision 5's other half. Consent was given for eight trigs; the write's
        // own re-fetch says twelve, so the sentence agreed to is not the one
        // about to happen.
        let survey = Survey {
            kit_name: "KIT 1".into(),
            box_swing: Some(50),
            free_lanes: Some(80),
            existing: vec![seen(8, "BD")],
        };
        let kit = PatternKit {
            version: 3,
            name: String::new(),
            tempo_bpm: 120.0,
            kit_index: 0,
            tracks: Vec::new(),
            kit: digi_protocol::pattern::KitInfo {
                version: 3,
                name: "KIT 1".into(),
                sound_names: Vec::new(),
                midi_mask: 0,
            },
        };
        let track = digi_protocol::safe_write::TrackConfirm {
            track_index: 0,
            existing_trigs: 12,
            note_count: 4,
            box_plocks: Vec::new(),
        };
        let args = ConfirmArgs {
            pattern_kit: Some(&kit),
            label: "A01".into(),
            index: 0,
            swing: Some(50),
            free_lanes: Some(80),
            tracks: vec![track.clone()],
        };
        let why = changed_since_survey(&survey, &args).expect("eight is not twelve");
        assert!(why.contains("had 8 when you were asked and has 12 now"), "{why}");

        // And the same numbers consent.
        let args = ConfirmArgs {
            tracks: vec![digi_protocol::safe_write::TrackConfirm { existing_trigs: 8, ..track }],
            ..args
        };
        assert_eq!(changed_since_survey(&survey, &args), None);
    }
}
