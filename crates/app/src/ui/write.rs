// The SEND group of the Setup panel: put one track of this session onto one
// track of one slot of one box.
//
// **This is the first thing in `app/` that can store bytes on hardware.** Read
// that before changing anything here. `ui::transfer`, three inches up the panel,
// is read-only and structurally so; this file is not, and the difference is not
// a flag — it is that every path below ends at
// `digi_protocol::safe_write::safe_write_track`, which is the one function that
// holds all five safety rules (PLAN.md §7 rule 2). A second route to a store
// anywhere in this crate is a bug, not a shortcut.
//
// Everything under the button already existed and was tested:
// `Session::track_write` → `safe_write_track` → `PatternIo::send_pattern_kit`.
// What was missing was any way to ask for it from inside the running app, so a
// write could only happen by running an example from a terminal with `--write`.
// This file is that button and nothing else: it holds no byte offsets, no encode
// rules and no safety logic of its own, and every refusal it renders is one
// `core` or `protocol` made.
//
// ## Seven decisions
//
//  1. **The confirm dialog is answered on the UI thread while the worker
//     blocks.** `safe_write_track` calls `WriteHooks::confirm` *after* its
//     re-fetch, on purpose: the dialog names the trigs that are about to be
//     replaced, the box's current swing and the lanes it is holding, and none of
//     those are knowable until the destination has been read. So the hook sends
//     the wording down a channel with a reply `Sender` and waits on it. The
//     alternative — fetch on the UI thread, ask, then run the flow — would show
//     a dialog about one fetch and write over the top of another, which is the
//     stale-payload bug rule 2's re-fetch exists to prevent, wearing a dialog.
//     A dropped reply channel (the window closing mid-write) reads as *no
//     consent*, which is the only safe direction for a `recv` that failed.
//  2. **One transfer at a time, in either direction.** The Fetch button and this
//     one are disabled while the other is working. There is one desk, one person
//     at it, and two connections to the same box — one of them mid-dump — is a
//     state neither this app nor CoreMIDI has any reason to be good at.
//  3. **The box that answers must be the box the row names.** `js/main.js`
//     refuses only when writing *home* (back to the slot a pattern was imported
//     from). This refuses always, because a row here is per box: if the DT2 row
//     is cabled to a DN2, the honest reading is a mis-cabled desk, not a request
//     to copy a pattern across families. Cross-device copy is its own job with
//     its own translation (PLAN.md §6), and doing it by accident is exactly what
//     provenance exists to prevent. This is the stricter rule `Session::track_write`
//     says belongs with the button.
//  4. **One track picker, not two.** `core::export` names one track index for
//     both ends, so T9 here goes to T9 there. Sending T9 to T3 is copy-track,
//     which is a listed and undesigned piece of work; reaching around the seam to
//     fake it would put the write path's only untested shape in the UI layer.
//  5. **A write claims no provenance.** The JS sets the roll's `source` to where
//     it just sent, because a roll pattern *is* one track. Ours is sixteen, so
//     saying "this pattern now lives at A03" after writing one of them would be a
//     lie the destination picker then acts on. So this panel never edits the
//     session — the only session state a write reads is the notes it sends.
//  6. **The destination starts where the pattern came from.** An imported
//     pattern aims back at its own slot, so the everyday gesture — fetch, edit,
//     put it back — is one press with nothing to line up. A pattern with no
//     provenance aims at the slot of the same name. Either default sticks the
//     moment the picker is touched, exactly as `ui::transfer`'s does.
//  7. **A verify failure, or a write that did not go entirely as asked, gets a
//     modal.** `write_result_message` decides which those are (`is_error`) and
//     the line is shown in the row as well. This is `js/main.js`'s `window.alert`
//     and its reason ports unchanged: a mismatch between what we sent and what
//     the box stored must never be something you scroll past.
//
// **What this deliberately does not do:** it does not stop the transport, and it
// does not refuse to write while the engine is clocking the box. What a box does
// with a 127 KB dump while it is sequencing is not something this repo has
// measured, and refusing on a guess would be inventing a rule no oracle has. The
// confirm dialog says the transport is running and leaves the judgement where the
// evidence is.
//
// **This has met hardware, and it worked** (2026-08-18, hand-run by Neil, PLAN.md
// §9 entry 10): DT2 A01 T1 and DN2 A01 T1, three notes and eight, two p-lock lanes
// with them, both read back byte-identical, both from the defaults this panel
// offered. **It was the first byte this repo ever stored on a box.** The flow is
// also driven end to end against a fake box in `app/tests/write.rs`, which is what
// keeps it honest between hardware runs.
//
// The two things that run did *not* cover, in case they matter to whatever you are
// changing: the transport was stopped, and both rows were left at their defaults,
// so nothing has ever pinned a picker on a real desk.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

use digi_core::device::{Device, PatternRoute};
use digi_core::model::Pattern;
use digi_core::session::PatternRef;
use digi_core::{DeviceId, Session};
use digi_midi::{ElektronDevice, PortBinding};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::DeviceIdentity;
use digi_protocol::pattern::{PatternKit, Spec};
use digi_protocol::plocks::{LaneWrite, PoolLane};
use digi_protocol::a4_pattern::TRACK_NAMES as A4_TRACK_NAMES;
use digi_protocol::safe_write::{
    a4_safe_write_tracks, safe_write_track, write_impact_lines, write_result_message,
    A4TrackWrite, ConfirmArgs, ImpactArgs, PatternIo, ResultMessage, Timestamp, TrackWrite,
    WriteHooks, BACKUP_LINE,
};
use eframe::egui::{self, Ui};

use crate::ui::tracks::Selection;
use crate::ui::transfer::{binding, slot_choices, wire_slots};

// --- what crosses the thread ---------------------------------------------------

/// Everything the press captured, so the worker never borrows the session.
///
/// The notes are a snapshot taken when the button went down, which is the same
/// bargain `ui::transfer` makes with its destination slot: the answer belongs to
/// what was asked for, not to whatever the roll looks like when the dump lands.
#[derive(Debug)]
pub struct Job {
    pub device: DeviceId,
    /// The slug of the box this row names — what the identity handshake has to
    /// come back with. `None` for a model with no dumps, which cannot get here.
    pub slug: Option<&'static str>,
    pub display: &'static str,
    pub input: PortBinding,
    pub output: PortBinding,
    /// The destination's spec, which is the row's own: decision 3 means a box
    /// that would need a different one is refused before this is used. `None`
    /// on the Analog Four, whose format has no `Spec` — the write below says
    /// which flow it takes instead.
    pub spec: Option<&'static Spec>,
    pub write: PlannedWrite,
    /// What `core::export` could not carry — lanes belonging to another box's
    /// numbering, notes off the end of a byte. Shown before the write is agreed
    /// to and again in the result line, per `js/main.js`.
    pub warnings: Vec<String>,
    pub pattern_name: String,
    /// Where it is coming from in *this session*, e.g. "A01 T9".
    pub source_label: String,
    pub source_len: u16,
    /// The slug on the pattern's provenance when it is not this box's — a note in
    /// the dialog rather than a refusal, because aiming a copy somewhere on
    /// purpose is allowed and the dialog is where it gets said.
    pub from_other_box: Option<String>,
    pub into: PatternRef,
    pub playing: bool,
}

/// One track's write in whichever format the destination speaks. Decided by
/// [`plan`] from the model's `pattern_route`, and matched once, in [`run`] —
/// everything between the two (the worker, the dialog plumbing, the report) is
/// format-blind.
#[derive(Debug)]
pub enum PlannedWrite {
    Gen2(TrackWrite),
    A4(A4TrackWrite),
}

/// What the worker says while it works. `Ask` is the one that expects an answer.
pub enum Event {
    Status(String),
    Log(String),
    Ask(Ask),
    Done(Result<Report, String>),
}

/// The confirm dialog, and the wire back to the thread waiting on it.
///
/// Dropping this without sending is a refusal, which is what makes closing the
/// window mid-dialog safe: the worker's `recv` fails and consents to nothing.
pub struct Ask {
    pub lines: Vec<String>,
    /// What the button that goes through with it says. Named after the slot it
    /// overwrites, because a button that says "OK" is one you press by reflex.
    pub button: String,
    pub reply: Sender<bool>,
}

/// A finished write, as the row shows it.
#[derive(Debug)]
pub struct Report {
    pub message: ResultMessage,
    pub cancelled: bool,
    /// The backup line `safe_write_track` logged, so the row can say where the
    /// previous contents went.
    pub log: Option<String>,
}

// --- the blocking half ----------------------------------------------------------

/// Everything after the ports are open, with the box injected.
///
/// Generic over [`PatternIo`] for the reason the trait exists: `app/tests/write.rs`
/// drives this exact function — the refusals, the confirm round trip, the cancel
/// and the result wording — against a box that is a `BTreeMap`, so the only thing
/// left untested by the time a cable is involved is the cable.
pub fn run(
    device: &mut impl PatternIo,
    stash: &Stash,
    job: &Job,
    events: &Sender<Event>,
    now: Timestamp,
) -> Result<Report, String> {
    let identity = device
        .identity()
        .cloned()
        .ok_or_else(|| "the box did not answer the identity handshake".to_string())?;
    // Decision 3, before anything is fetched and long before anything is sent.
    // This is the only refusal this function makes on its own.
    if let Some(refusal) = wrong_box(job.slug, job.display, &identity) {
        return Err(refusal);
    }
    // **There is deliberately no allowlist check here**, and the reason is worth
    // keeping: the first draft had one, "so the refusal is legible rather than
    // arriving from inside the flow", and the deliberate-bug pass found that
    // deleting it failed nothing. `safe_write_track`'s own gate is its first act,
    // before the re-fetch, and its `WriteError::Gate` displays the same words —
    // so the copy could not change what anyone saw. That is not the same as
    // `plan_store`'s copy at the wire, which guards a route this function is not
    // on: a caller that reaches `PatternIo` directly. A copy that guards a bypass
    // earns its place; a copy that guards the path it is already standing on is
    // untestable weight.

    let mut hooks = UiHooks { events, job, device_name: identity.name.clone(), log: None };
    let result = match &job.write {
        PlannedWrite::Gen2(write) => safe_write_track(device, stash, write, &mut hooks, now),
        // The plural flow with a one-element slice, exactly as `safe_write_track`
        // is — the A4 ceremony has no singular alias because this is its only
        // single-track caller.
        PlannedWrite::A4(write) => {
            a4_safe_write_tracks(device, stash, std::slice::from_ref(write), &mut hooks, now)
        }
    }
    .map_err(|e| e.to_string())?;

    // Lanes that could not be written at all were reported before the write; they
    // belong in the result line too, or a successful send reads as if everything
    // went. `js/main.js` does exactly this, for exactly this reason. The
    // `cancelled` guard is its too, and it is currently unobservable —
    // `write_result_message` answers a cancel before it looks at the warnings —
    // but a losses list on a write that never happened would be wrong the moment
    // anything words a cancel more fully than "Write cancelled".
    let mut warnings = result.warnings.clone();
    if !result.cancelled {
        warnings.extend(job.warnings.iter().cloned());
    }
    let message = write_result_message(&digi_protocol::safe_write::WriteResult {
        warnings,
        ..result.clone()
    });
    Ok(Report { message, cancelled: result.cancelled, log: hooks.log })
}

/// Why the box that answered is not the one this row names, or `None`.
pub fn wrong_box(
    expected: Option<&str>,
    display: &str,
    identity: &DeviceIdentity,
) -> Option<String> {
    match expected {
        Some(slug) if slug == identity.slug => None,
        Some(_) => Some(format!(
            "this row is the {display} and the box on those ports says it's a {} — refusing to \
             write. Cross-device copy is its own job; it is not this button with the wrong cable \
             in it.",
            identity.name
        )),
        None => Some(format!("{display} has no pattern dumps to write")),
    }
}

/// The hooks `safe_write_track` calls, forwarded to the UI thread.
struct UiHooks<'a> {
    events: &'a Sender<Event>,
    job: &'a Job,
    device_name: String,
    log: Option<String>,
}

impl WriteHooks for UiHooks<'_> {
    fn confirm(&mut self, args: &ConfirmArgs) -> bool {
        // One track, always: this panel's button is `safe_write_track` (or the
        // A4 flow with a one-element slice), so the plural `args.tracks` is the
        // mass send's to read, in `ui::sync`.
        let track = args.one();
        let lines = match (&self.job.write, args.pattern_kit) {
            (PlannedWrite::Gen2(write), Some(pattern_kit)) => {
                let lanes = write.plocks.clone().unwrap_or_default();
                let spec = self.job.spec.expect("a gen-2 job carries its spec");
                confirm_lines(&Facts {
                    device_name: &self.device_name,
                    pattern_name: &self.job.pattern_name,
                    source_label: &self.job.source_label,
                    notes: track.note_count,
                    label: &args.label,
                    track_index: track.track_index,
                    kit_name: &pattern_kit.kit.name,
                    track_kind: &track_kind_label(
                        pattern_kit,
                        track.track_index,
                        spec.track_kind_fallback,
                    ),
                    existing_trigs: track.existing_trigs,
                    source_len: self.job.source_len,
                    dest_len: pattern_kit
                        .tracks
                        .get(track.track_index)
                        .map(|t| t.length_steps)
                        .unwrap_or(0),
                    lanes: &lanes,
                    box_plocks: &track.box_plocks,
                    free_lanes: args.free_lanes.expect("a gen-2 confirm carries the pool"),
                    track_prob: write.track_prob,
                    swing: write.swing.map(|s| s.round() as u8),
                    box_swing: args.swing.expect("a gen-2 confirm carries the box's swing"),
                    warnings: &self.job.warnings,
                    from_other_box: self.job.from_other_box.as_deref(),
                    playing: self.job.playing,
                })
            }
            (PlannedWrite::A4(_), _) => a4_confirm_lines(&A4Facts {
                device_name: &self.device_name,
                pattern_name: &self.job.pattern_name,
                source_label: &self.job.source_label,
                notes: track.note_count,
                label: &args.label,
                track_index: track.track_index,
                existing_trigs: track.existing_trigs,
                warnings: &self.job.warnings,
                playing: self.job.playing,
            }),
            // A gen-2 write whose confirm carries no decoded destination is a
            // mis-wiring between two flows that both did their own decode; no
            // wording is safe to invent for it.
            (PlannedWrite::Gen2(_), None) => return false,
        };
        let (reply, answer) = channel();
        let ask = Ask {
            lines,
            button: format!("Overwrite {} T{}", args.label, track.track_index + 1),
            reply,
        };
        if self.events.send(Event::Ask(ask)).is_err() {
            return false;
        }
        // Blocks until the dialog is answered — or forever less one frame, if
        // the window has gone, in which case the channel closes and this is a no.
        answer.recv().unwrap_or(false)
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
fn worker(job: Job, events: Sender<Event>) {
    let outcome = (|| {
        // Rule 1's first gate: the store has to exist before a byte moves,
        // because it holds the *only* automatic copy of what is about to be
        // overwritten. `safe_write_track` refuses on a stash that will not
        // write; this refuses on one there is nowhere to put.
        let stash = Stash::default_stash().map_err(|e| {
            format!(
                "nothing was written: there is nowhere to keep the backup ({e}) — a backup that \
                 cannot be stored is a write that does not happen"
            )
        })?;
        let mut device =
            ElektronDevice::open(&job.input, &job.output).map_err(|e| e.to_string())?;
        let _ = events.send(Event::Status("Asking the box who it is…".into()));
        device.identify().map_err(|e| e.to_string())?;
        run(&mut device, &stash, &job, &events, Timestamp::now())
    })();
    let _ = events.send(Event::Done(outcome));
}

// --- the wording ----------------------------------------------------------------

/// Everything the confirm dialog says, as plain values — so the sentences a
/// person agrees to are testable without a box, a thread or a window.
pub struct Facts<'a> {
    /// The box that answered, by its own name.
    pub device_name: &'a str,
    pub pattern_name: &'a str,
    /// Where it is coming from in this session, e.g. "A01 T9".
    pub source_label: &'a str,
    pub notes: usize,
    /// The destination slot, as the box labels it.
    pub label: &'a str,
    pub track_index: usize,
    pub kit_name: &'a str,
    pub track_kind: &'a str,
    pub existing_trigs: usize,
    pub source_len: u16,
    pub dest_len: u16,
    pub lanes: &'a [LaneWrite],
    pub box_plocks: &'a [PoolLane],
    pub free_lanes: usize,
    pub track_prob: Option<u8>,
    pub swing: Option<u8>,
    pub box_swing: u8,
    pub warnings: &'a [String],
    pub from_other_box: Option<&'a str>,
    pub playing: bool,
}

/// The sentences the write is agreed to on.
///
/// The port of `js/main.js`'s confirm block, with `write_impact_lines` doing the
/// same job in the middle of it: everything a write touches *beyond* the named
/// track's trigs is worded once, in `protocol`, so this dialog and the example's
/// terminal prompt cannot come to say different things.
///
/// Two places this differs from the browser, both because the box is the same and
/// the app is not. The **kit** name is quoted rather than the pattern's, because
/// neither of these boxes names a pattern — `PatternKit::name` comes back empty
/// and the kit is what a person reads off the screen, which is also what the
/// backup store's rows are labelled with. And the **source** is named, because
/// this session has sixteen slots of sixteen tracks where the roll had one
/// pattern, so "from where" is a real question here.
pub fn confirm_lines(f: &Facts) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Send {} from “{}” {} to {}{} track {}{} on the {}?",
            plural(f.notes, "note"),
            f.pattern_name,
            f.source_label,
            f.label,
            quoted(f.kit_name),
            f.track_index + 1,
            match f.track_kind.trim() {
                "" => String::new(),
                kind => format!(" ({kind})"),
            },
            f.device_name,
        ),
        String::new(),
        if f.existing_trigs > 0 {
            format!(
                "This replaces the {} already on that track.",
                plural(f.existing_trigs, "trig")
            )
        } else {
            "That track is currently empty.".to_string()
        },
    ];

    // The write never changes a track's length, so a pattern longer than the
    // track it lands on is stored in full and only heard as far as the box's own
    // LEN. Better said here than discovered on playback.
    if f.dest_len < f.source_len {
        lines.push(format!(
            "That track is {} steps long on the box and this one is {} — the rest is stored but \
             won't play until you raise the track's LEN there.",
            f.dest_len, f.source_len
        ));
    }

    lines.extend(write_impact_lines(&ImpactArgs {
        label: f.label,
        track: Some(f.track_index),
        lanes: f.lanes,
        box_plocks: f.box_plocks,
        free_lanes: Some(f.free_lanes),
        track_prob: f.track_prob,
        swing: f.swing,
        box_swing: Some(f.box_swing),
    }));

    for w in f.warnings {
        lines.push(format!("Note: {w}"));
    }
    if let Some(slug) = f.from_other_box {
        lines.push(format!("Note: this pattern came from a {slug}."));
    }
    // Decision: said, not refused. See the header.
    if f.playing {
        lines.push(
            "The transport is running — this app keeps clocking the box while the dump goes \
             across, and pressing this does not stop it."
                .to_string(),
        );
    }

    lines.push(String::new());
    lines.push(BACKUP_LINE.to_string());
    lines
}

/// Everything the A4's confirm dialog says, as plain values — [`Facts`]'s twin,
/// with every field the format cannot answer removed rather than defaulted: no
/// kit name (not in the mapped layout), no LEN comparison (unmapped), no lanes,
/// no PROB, no swing.
pub struct A4Facts<'a> {
    pub device_name: &'a str,
    pub pattern_name: &'a str,
    pub source_label: &'a str,
    pub notes: usize,
    pub label: &'a str,
    pub track_index: usize,
    pub existing_trigs: usize,
    pub warnings: &'a [String],
    pub playing: bool,
}

/// The sentences an A4 write is agreed to on — [`confirm_lines`]'s twin.
///
/// One line here has no gen-2 counterpart and must not be dropped: the
/// read-modify-write sentence. On a digi the dialog's impact lines enumerate
/// what moves beyond the trigs (swing, PROB, lanes); on the A4 nothing does,
/// and *saying so* is the honest version of that enumeration — the sounds and
/// p-locks a person might expect to travel with the pattern stay exactly as
/// the destination slot has them.
pub fn a4_confirm_lines(f: &A4Facts) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Send {} from “{}” {} to {} track {} ({}) on the {}?",
            plural(f.notes, "note"),
            f.pattern_name,
            f.source_label,
            f.label,
            f.track_index + 1,
            A4_TRACK_NAMES.get(f.track_index).copied().unwrap_or("?"),
            f.device_name,
        ),
        String::new(),
        if f.existing_trigs > 0 {
            format!(
                "This replaces the {} already on that track.",
                plural(f.existing_trigs, "trig")
            )
        } else {
            "That track is currently empty.".to_string()
        },
        "Only the trigs move: sounds, p-locks, velocity and length stay as the destination \
         slot holds them right now — the write is composed on a fresh read of that slot."
            .to_string(),
    ];
    for w in f.warnings {
        lines.push(format!("Note: {w}"));
    }
    if f.playing {
        lines.push(
            "The transport is running — this app keeps clocking the box while the dump goes \
             across, and pressing this does not stop it."
                .to_string(),
        );
    }
    lines.push(String::new());
    lines.push(BACKUP_LINE.to_string());
    lines
}

/// What the destination track is called on the box: a MIDI track says so, and a
/// sample/synth track gives its sound's name, falling back to the spec's word for
/// what kind of track this box has.
///
/// The decision itself lives once, in `digi_core::import::kit_track_name` — an
/// import asks it the same question when it populates `Track.patch`, and this
/// only translates the answer into a write dialog's label.
pub fn track_kind_label(kit: &PatternKit, track_index: usize, fallback: &str) -> String {
    match digi_core::import::kit_track_name(kit, track_index) {
        digi_core::import::KitTrackName::Midi => "MIDI".to_string(),
        digi_core::import::KitTrackName::Sound(name) => name.to_string(),
        digi_core::import::KitTrackName::Unnamed => fallback.to_string(),
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn quoted(name: &str) -> String {
    match name.trim() {
        "" => String::new(),
        name => format!(" “{name}”"),
    }
}

// --- the panel --------------------------------------------------------------------

/// One box's row: what to send, where it goes, and the last answer.
///
/// Every picker is an `Option`, and `None` is not "unset" but *following*: the
/// source follows the scene on screen, the track follows the roll's selection,
/// the destination follows the pattern's provenance. A hand-picked value fills the
/// option in and stops following, which is decision 6.
#[derive(Default)]
struct Row {
    from: Option<PatternRef>,
    track: Option<usize>,
    into: Option<PatternRef>,
    /// The last track the roll was editing *on this box*, which is what an
    /// unpinned picker follows. Remembered rather than read live, so selecting a
    /// track on the other box does not snap this row back to T1.
    followed_track: usize,
    outcome: Option<Outcome>,
}

enum Outcome {
    Done(Report),
    /// Anything that stopped it before `safe_write_track` returned: a port that
    /// would not open, the wrong box, a firmware the format was never verified
    /// against, a backup that could not be stored.
    Failed(String),
}

/// A write in flight. The destination is captured at the press, for the reason
/// `ui::transfer`'s is.
struct Pending {
    device: DeviceId,
    rx: Receiver<Event>,
    status: String,
}

/// What is on screen instead of the panel, if anything.
enum Dialog {
    Confirm { ask: Ask },
    /// A finished write that must not be scrolled past — decision 7.
    Alert { text: String },
}

#[derive(Default)]
pub struct WritePanel {
    rows: HashMap<DeviceId, Row>,
    pending: Option<Pending>,
    dialog: Option<Dialog>,
}

impl WritePanel {
    /// Whether a write is in flight, so the fetch button can be held off.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Take whatever the worker has said, and put up the dialog it is waiting on.
    ///
    /// **Called from the window rather than from the panel, and that is not a
    /// style choice.** The Setup panel collapses, and a collapsed panel's body
    /// does not run — so if this lived beside the rows, closing the panel
    /// mid-write would leave a worker blocked forever on a question nobody could
    /// be shown. A modal belongs to the window in any case: it is not a thing
    /// inside a panel, it is the thing in front of everything.
    pub fn tick(&mut self, ui: &mut Ui) {
        self.poll();
        if self.pending.is_some() {
            // Nothing wakes the UI thread when a worker moves, and this one has a
            // dialog to put up in the middle. Same bargain as the fetch panel.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.dialog_ui(ui);
    }

    /// Draw the group. Never edits the session — decision 5.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: &Session,
        present: PortsPresent<'_>,
        selection: Selection,
        blocked: bool,
        playing: bool,
    ) {
        if session.devices.is_empty() {
            ui.weak("No boxes in this session.");
            return;
        }

        // Every box in the session, the A4 included: its per-track write goes
        // through the same ceremony as a digi's since 2026-08-31, so the
        // FRONT-PANEL DUMP group this used to filter it out toward is gone.
        let devices: Vec<DeviceId> = session.devices.iter().map(|d| d.id).collect();
        let last = devices.len().saturating_sub(1);
        for (position, id) in devices.into_iter().enumerate() {
            self.device_ui(ui, session, present, selection, id, blocked, playing);
            if position != last {
                ui.add_space(6.0);
            }
        }
    }

    /// One box's block.
    fn device_ui(
        &mut self,
        ui: &mut Ui,
        session: &Session,
        present: PortsPresent<'_>,
        selection: Selection,
        id: DeviceId,
        blocked: bool,
        playing: bool,
    ) {
        let Some(device) = session.device(id) else { return };
        ui.label(egui::RichText::new(&device.name).size(11.0).color(super::TEXT_SECONDARY));

        if let Some(reason) = blocker(device, present) {
            ui.weak(reason);
            return;
        }

        // What each picker is following while nobody has touched it.
        let scene_slot = session
            .slot_in_scene(session.current_scene, id)
            .unwrap_or_else(|| PatternRef::new(0, 0));
        let selected_track = session
            .devices
            .get(selection.device)
            .filter(|d| d.id == id)
            .map(|_| selection.track);
        let tracks = device.model.num_tracks;

        let row = self.rows.entry(id).or_default();
        if let Some(t) = selected_track {
            row.followed_track = t.min(tracks.saturating_sub(1));
        }
        let track_follow = row.followed_track;
        let (pinned_into, mut from, mut track) =
            (row.into, row.from.unwrap_or(scene_slot), row.track.unwrap_or(track_follow));

        let busy = blocked || self.pending.is_some() || self.dialog.is_some();
        let in_flight = self.pending.as_ref().is_some_and(|p| p.device == id);
        let from_choices = slot_choices(device);
        let mut send_clicked = false;

        ui.horizontal_wrapped(|ui| {
            ui.weak("send");
            let mut picked = from;
            egui::ComboBox::from_id_salt(("send-from", id.0))
                .selected_text(egui::RichText::new(from.label()).color(super::TEXT_DIMMER))
                .width(56.0)
                .show_ui(ui, |ui| {
                    for (slot, text) in &from_choices {
                        ui.selectable_value(&mut picked, *slot, text);
                    }
                });
            if picked != from {
                from = picked;
            }

            let mut picked = track;
            egui::ComboBox::from_id_salt(("send-track", id.0))
                .selected_text(
                    egui::RichText::new(track_label(device, from, track)).color(super::TEXT_DIMMER),
                )
                .width(76.0)
                .show_ui(ui, |ui| {
                    for t in 0..tracks {
                        ui.selectable_value(&mut picked, t, track_label(device, from, t));
                    }
                });
            if picked != track {
                track = picked;
            }
        });

        // **After the source row, not before it.** The destination follows the
        // pattern in `from`, and `from` may have moved a line ago — reading the
        // old slot's provenance here would make the answer differ from the one
        // the pin check below compares against, and changing the source would
        // silently pin the destination to where the *previous* one pointed.
        let pattern = device.pattern(from.slot());
        let into_follow = aim(pattern, device.model.slug, from);
        let mut into = pinned_into.unwrap_or(into_follow);
        let home = is_home(pattern, device.model.slug, into);

        ui.horizontal_wrapped(|ui| {
            ui.weak("to");
            let mut picked = into;
            egui::ComboBox::from_id_salt(("send-into", id.0))
                .selected_text(
                    egui::RichText::new(format!("{} T{}", into.label(), track + 1))
                        .color(super::TEXT_DIMMER),
                )
                .width(76.0)
                .show_ui(ui, |ui| {
                    for slot in wire_slots(device.model) {
                        ui.selectable_value(&mut picked, slot, slot.label());
                    }
                });
            if picked != into {
                into = picked;
            }

            ui.add_enabled_ui(!busy, |ui| {
                // "Write back" when it is going home, "Send" when it is not — the
                // one-glance answer to "am I about to put this somewhere else".
                // Kept as the descriptive verb rather than the design mock's bare
                // "SEND": the distinction is `write::is_home`'s, it is the one
                // thing this button says that the confirm dialog does not
                // restate up front, and genericising it would drop real
                // information behind a colour instead of keeping it in words.
                //
                // The JS writes this as `Send → A01 T9`; the word is spelled out
                // here because `→` is U+2192, in the same neighbourhood as the
                // four marks `ui::mod`'s table records as missing from the
                // bundled fonts, and a button whose label is a tofu box is worse
                // than a button with a longer one.
                let verb = if home { "Write back to" } else { "Send to" };
                send_clicked = super::colored_button(
                    ui,
                    format!("{verb} {} T{}", into.label(), track + 1),
                    super::WARN_AMBER_FILL,
                    super::WARN_AMBER_TEXT,
                    super::WARN_AMBER_BORDER,
                    super::WARN_AMBER,
                    super::WARN_AMBER_INK,
                )
                .on_hover_text(if home {
                    "Re-read this slot from the box, replace that one track with this one, \
                     and verify byte for byte. The whole destination pattern is backed up \
                     first, and you get to read what it changes before anything is sent."
                } else {
                    "Replace that one track of that slot on the box with this one. The whole \
                     destination pattern is re-read and backed up first, you get to read what \
                     it changes before anything is sent, and it is verified byte for byte \
                     afterwards."
                })
                .clicked();
            });
        });

        // What is going, said before the press. The dialog says what is being
        // landed on; only the box knows that, and only after a fetch.
        let notes = pattern.and_then(|p| p.track(track)).map(|t| t.notes.len()).unwrap_or(0);
        // Lanes are only *going* where the format can carry them: on the A4 the
        // pool is unmapped and a write leaves it alone, so counting the roll's
        // lanes here would promise a travel the dialog then warns cannot happen.
        let lanes = match device.model.spec() {
            Some(_) => pattern
                .and_then(|p| p.track(track))
                .map(|t| t.plocks.len())
                .unwrap_or(0),
            None => 0,
        };
        ui.weak(match lanes {
            0 => format!("{} on {} T{}", plural(notes, "note"), from.label(), track + 1),
            n => format!(
                "{} and {} on {} T{}",
                plural(notes, "note"),
                plural(n, "p-lock lane"),
                from.label(),
                track + 1
            ),
        });

        let row = self.rows.entry(id).or_default();
        // Only a value that has *moved off what it was following* is pinned:
        // assigning every frame would pin all three pickers the first time the
        // panel is drawn, and the following in decision 6 would never happen
        // again.
        if from != row.from.unwrap_or(scene_slot) {
            row.from = Some(from);
        }
        if track != row.track.unwrap_or(track_follow) {
            row.track = Some(track);
        }
        if into != pinned_into.unwrap_or(into_follow) {
            row.into = Some(into);
        }

        if send_clicked {
            self.start(id, session, present, track, from, into, playing);
        }

        if in_flight {
            ui.horizontal(|ui| {
                ui.spinner();
                let status = self
                    .pending
                    .as_ref()
                    .map(|p| p.status.clone())
                    .unwrap_or_default();
                ui.label(status);
            });
            return;
        }

        self.outcome_ui(ui, id);
    }

    /// The last answer on this row.
    fn outcome_ui(&mut self, ui: &mut Ui, id: DeviceId) {
        let Some(row) = self.rows.get(&id) else { return };
        match &row.outcome {
            Some(Outcome::Done(report)) => {
                if report.cancelled {
                    ui.weak(&report.message.text);
                    return;
                }
                let colour = if report.message.is_error {
                    egui::Color32::LIGHT_RED
                } else {
                    egui::Color32::LIGHT_GREEN
                };
                ui.colored_label(colour, &report.message.text);
                if let Some(log) = &report.log {
                    ui.weak(log);
                }
            }
            Some(Outcome::Failed(e)) => {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            None => {}
        }
    }

    /// The confirm dialog, or a result that must be read.
    fn dialog_ui(&mut self, ui: &mut Ui) {
        let Some(dialog) = &self.dialog else { return };
        let mut answer: Option<bool> = None;

        let response = egui::Modal::new(egui::Id::new("write-dialog")).show(ui.ctx(), |ui| {
            ui.set_max_width(520.0);
            match dialog {
                Dialog::Confirm { ask } => {
                    ui.label(egui::RichText::new("Write to the box").strong());
                    ui.separator();
                    for line in &ask.lines {
                        if line.is_empty() {
                            ui.add_space(6.0);
                        } else {
                            ui.label(line);
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        // Cancel first, and on the left, because it is the answer
                        // a hesitating hand should land on.
                        if ui.button("Cancel").clicked() {
                            answer = Some(false);
                        }
                        if ui.button(&ask.button).clicked() {
                            answer = Some(true);
                        }
                    });
                }
                Dialog::Alert { text } => {
                    ui.label(egui::RichText::new("The write did not go as asked").strong());
                    ui.separator();
                    ui.label(text);
                    ui.separator();
                    if ui.button("OK").clicked() {
                        answer = Some(false);
                    }
                }
            }
        });
        // Clicking away or pressing Escape is a cancel, which is only correct
        // because there is no dialog here where the destructive answer is the
        // default one.
        if response.should_close() {
            answer = Some(false);
        }

        let Some(answer) = answer else { return };
        match self.dialog.take() {
            Some(Dialog::Confirm { ask }) => {
                // A send that fails means the worker has gone, and a worker that
                // has gone consented to nothing.
                let _ = ask.reply.send(answer);
            }
            Some(Dialog::Alert { .. }) | None => {}
        }
    }

    /// Put a write out. Does nothing if anything is already in flight.
    fn start(
        &mut self,
        id: DeviceId,
        session: &Session,
        present: PortsPresent<'_>,
        track: usize,
        from: PatternRef,
        into: PatternRef,
        playing: bool,
    ) {
        if self.pending.is_some() || self.dialog.is_some() {
            return;
        }
        let job = match plan(session, present, id, from, track, into, playing) {
            Ok(job) => job,
            Err(e) => {
                self.rows.entry(id).or_default().outcome = Some(Outcome::Failed(e));
                return;
            }
        };

        self.rows.entry(id).or_default().outcome = None;
        let (tx, rx) = channel();
        std::thread::spawn(move || worker(job, tx));
        self.pending = Some(Pending { device: id, rx, status: "Opening the box…".into() });
    }

    /// Drain whatever the worker has said since the last frame.
    fn poll(&mut self) {
        let Some(pending) = &mut self.pending else { return };
        let device = pending.device;
        loop {
            let Ok(event) = pending.rx.try_recv() else { return };
            match event {
                Event::Status(s) => pending.status = s,
                // The interesting log line is the backup's, and it is carried
                // home on the report; this keeps the row talking while the
                // 127 KB goes across.
                Event::Log(s) => pending.status = s,
                Event::Ask(ask) => {
                    self.dialog = Some(Dialog::Confirm { ask });
                    return;
                }
                Event::Done(outcome) => {
                    let outcome = match outcome {
                        Ok(report) => {
                            // Decision 7: a verify mismatch, or a lane that did
                            // not fit, is put in front of the person who asked.
                            if report.message.is_error {
                                self.dialog =
                                    Some(Dialog::Alert { text: report.message.text.clone() });
                            }
                            Outcome::Done(report)
                        }
                        Err(e) => Outcome::Failed(e),
                    };
                    self.rows.entry(device).or_default().outcome = Some(outcome);
                    self.pending = None;
                    return;
                }
            }
        }
    }
}

/// Everything the press captures, in one function, so the thing a test drives is
/// the thing the button builds.
///
/// The only refusals that happen before a thread exists: a box with no ports, a
/// model with no dumps, and `core` deciding this track cannot be described as a
/// write at all. Everything past here needs the box to have answered.
pub fn plan(
    session: &Session,
    present: PortsPresent<'_>,
    id: DeviceId,
    from: PatternRef,
    track: usize,
    into: PatternRef,
    playing: bool,
) -> Result<Job, String> {
    let device = session.device(id).ok_or("that box is not in this session")?;
    let (Some(input), Some(output)) = (device.io.input.clone(), device.io.output.clone()) else {
        return Err(blocker(device, present).unwrap_or_else(|| "that box has no ports".into()));
    };

    // The two formats plan through their own `core` seam and meet again at
    // `PlannedWrite`; everything below the match is shared.
    let (spec, write, warnings) = match device.model.pattern_route() {
        PatternRoute::RequestGen1 => {
            let export = session.a4_track_write(id, from, track, into).map_err(|e| e.to_string())?;
            (None, PlannedWrite::A4(export.write), export.warnings)
        }
        _ => {
            let spec = device
                .model
                .spec()
                .ok_or_else(|| format!("{} has no patterns to write to", device.model.display))?;
            let export = session
                .track_write(spec, id, from, track, into)
                .map_err(|e| e.to_string())?;
            (Some(spec), PlannedWrite::Gen2(export.write), export.warnings)
        }
    };
    let pattern = device.pattern(from.slot());
    Ok(Job {
        device: id,
        slug: device.model.slug,
        display: device.model.display,
        input: binding(&input),
        output: binding(&output),
        spec,
        write,
        warnings,
        pattern_name: pattern.map(|p| p.name.clone()).unwrap_or_default(),
        source_label: format!("{} T{}", from.label(), track + 1),
        source_len: pattern.and_then(|p| p.track(track)).map(|t| t.length_steps).unwrap_or(0),
        // Aiming a copy at another box on purpose is allowed, so this is a line
        // in the dialog rather than a refusal. Decision 3 is about the box on the
        // *cable* not being the one this row names, which is a different thing.
        from_other_box: pattern
            .and_then(|p| p.source.as_ref())
            .filter(|s| Some(s.device_slug.as_str()) != device.model.slug)
            .map(|s| s.device_slug.clone()),
        into,
        playing,
    })
}

// --- the small rules --------------------------------------------------------------

/// What the OS says is plugged in right now, for the two port namespaces.
///
/// Built once per frame from the enumeration [`crate::ui::ports::PortsPanel`]
/// owns and handed to every panel that can start a write, so all three of them
/// are answering from one list rather than three — the same invariant
/// `ui::devices` and `AutoConnect` are built on.
#[derive(Debug, Clone, Copy)]
pub struct PortsPresent<'a> {
    pub inputs: &'a [digi_midi::PortInfo],
    pub outputs: &'a [digi_midi::PortInfo],
}

impl PortsPresent<'static> {
    /// "Nobody has enumerated yet", which blocks nothing.
    ///
    /// Every write test in this repo predates the cable-gone check and is about
    /// some other rule, so they all pass this: it keeps the port list out of
    /// their way while naming *why* it is out of their way. The two tests that
    /// are about the check itself build real lists.
    pub fn unknown() -> Self {
        Self { inputs: &[], outputs: &[] }
    }
}

impl PortsPresent<'_> {
    /// Is this bound port still in the OS's list?
    ///
    /// **An empty list means "we do not know", not "everything is unplugged".**
    /// A machine genuinely has zero MIDI ports sometimes, but so does one whose
    /// enumeration has not run yet — `PortsPanel` starts empty and is only
    /// filled by the first `refresh`, which `AutoConnect::tick` skips entirely
    /// when auto-connect is switched off. Refusing writes on that state would
    /// lock a correctly-cabled desk out of its own boxes for a reason nobody
    /// could see, so the unknown case answers `true` and the check below
    /// declines to have an opinion.
    pub fn holds(list: &[digi_midi::PortInfo], port: &digi_core::device::PortRef) -> bool {
        if list.is_empty() {
            return true;
        }
        let seen = digi_core::device::PortRef { id: port.id.clone(), name: port.name.clone() };
        list.iter().any(|p| {
            digi_core::device::PortRef { id: p.id.clone(), name: p.name.clone() }.same_port(&seen)
        })
    }
}

/// Why this box cannot be written to, or `None` if it can.
///
/// The two ends fail differently and for the opposite reasons they do on a fetch:
/// the dump goes *out* on the output, and the verify read — the step that decides
/// whether the box stored what we sent — comes back on the input. A desk with only
/// one of them wired could send and never know.
///
/// **The cable-gone check lives here rather than at the three call sites**, which
/// is lesson 5 applied before the fact: `ui::write`, `ui::restore` and
/// `ui::sync` all ask this one function whether a box can be written to, and a
/// rule restated in three of them would be forgotten in one. A box whose bound
/// port has vanished from the OS is refused for the same reason and by the same
/// sentence as a box that never had one.
pub fn blocker(device: &Device, present: PortsPresent<'_>) -> Option<String> {
    // The route, not `can_sysex`: the A4 writes without a gen-2 `Spec`, so the
    // question is whether patterns move at all.
    if !device.pattern_route().transfers() {
        return Some(format!(
            "{} plays over MIDI but has no patterns to write to",
            device.model.display
        ));
    }
    match (&device.io.input, &device.io.output) {
        (Some(input), Some(output)) => {
            // Rule 2 re-reads the destination pattern immediately before
            // encoding, so a write to a box that is not there fails at the
            // re-fetch anyway — but only *after* rule 1 has taken a backup and
            // the person has agreed to a dialog naming a slot. Refusing up here
            // turns a confusing mid-flight failure into a sentence about a cable.
            let gone = match (
                PortsPresent::holds(present.inputs, input),
                PortsPresent::holds(present.outputs, output),
            ) {
                (true, true) => return None,
                (true, false) => output.name.clone(),
                _ => input.name.clone(),
            };
            Some(format!("{gone} is no longer plugged in — reconnect the box to write to it"))
        }
        (None, None) => Some("No ports set — pick an in and an out for this box above".into()),
        (None, Some(_)) => Some("No in port — the read-back that verifies a write comes in on it".into()),
        (Some(_), None) => Some("No out port — the pattern goes out on it".into()),
    }
}

/// Where a pattern's Send button points before anyone has aimed it.
///
/// At the slot it was imported from, when it was imported from *this* box; at the
/// slot of the same name otherwise. The first is the everyday round trip — fetch,
/// edit, put it back — and the second is the only answer that is not a guess.
pub fn aim(pattern: Option<&Pattern>, slug: Option<&str>, from: PatternRef) -> PatternRef {
    match pattern.and_then(|p| p.source.as_ref()) {
        Some(source) if Some(source.device_slug.as_str()) == slug => {
            PatternRef::new(source.bank, source.index)
        }
        _ => from,
    }
}

/// Is this write going back where the pattern came from?
pub fn is_home(pattern: Option<&Pattern>, slug: Option<&str>, into: PatternRef) -> bool {
    matches!(
        pattern.and_then(|p| p.source.as_ref()),
        Some(source)
            if Some(source.device_slug.as_str()) == slug
                && PatternRef::new(source.bank, source.index) == into
    )
}

/// A track in the picker: its number, and its name when the box gave it one.
fn track_label(device: &Device, from: PatternRef, track: usize) -> String {
    let default = format!("T{}", track + 1);
    match device.pattern(from.slot()).and_then(|p| p.track(track)) {
        Some(t) if t.name != default && !t.name.trim().is_empty() => {
            format!("{default} {}", t.name)
        }
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::device::{model_for_key, DeviceIo, PortRef, DT2};
    use digi_core::model::{Note, Source};
    use digi_protocol::device::{identity_from_responses, DeviceResponse};

    fn port(name: &str) -> PortRef {
        PortRef { id: name.into(), name: name.into() }
    }

    fn wired() -> Device {
        let mut device = Device::new("DT2", &DT2, 16);
        device.io =
            DeviceIo { input: Some(port("in")), output: Some(port("out")), ..DeviceIo::default() };
        device
    }

    fn identity(product_id: u8) -> DeviceIdentity {
        identity_from_responses(
            &DeviceResponse { product_id, supported_ids: vec![0x60], reported_name: String::new() },
            "0070".into(),
            "1.15B".into(),
        )
    }

    fn facts<'a>() -> Facts<'a> {
        Facts {
            device_name: "Digitakt II",
            pattern_name: "JO_KIT",
            source_label: "A01 T9",
            notes: 8,
            label: "A01",
            track_index: 8,
            kit_name: "JO_KIT",
            track_kind: "BD",
            existing_trigs: 0,
            source_len: 16,
            dest_len: 16,
            lanes: &[],
            box_plocks: &[],
            free_lanes: 80,
            track_prob: Some(100),
            swing: Some(50),
            box_swing: 50,
            warnings: &[],
            from_other_box: None,
            playing: false,
        }
    }

    #[test]
    fn the_first_line_names_what_is_going_where_and_onto_which_box() {
        let lines = confirm_lines(&facts());
        assert_eq!(
            lines[0],
            "Send 8 notes from “JO_KIT” A01 T9 to A01 “JO_KIT” track 9 (BD) on the Digitakt II?"
        );
        // One note is one note, not "1 notes" — the dialog is read by a person
        // about to overwrite something.
        let lines = confirm_lines(&Facts { notes: 1, ..facts() });
        assert!(lines[0].starts_with("Send 1 note from"), "{}", lines[0]);
    }

    #[test]
    fn a_box_that_has_never_named_its_kit_leaves_no_empty_quotes() {
        let lines = confirm_lines(&Facts { kit_name: "   ", track_kind: "", ..facts() });
        assert_eq!(
            lines[0],
            "Send 8 notes from “JO_KIT” A01 T9 to A01 track 9 on the Digitakt II?"
        );
    }

    #[test]
    fn what_is_being_replaced_is_always_stated_including_when_it_is_nothing() {
        assert!(confirm_lines(&facts()).contains(&"That track is currently empty.".to_string()));
        let lines = confirm_lines(&Facts { existing_trigs: 15, ..facts() });
        assert!(
            lines.contains(&"This replaces the 15 trigs already on that track.".to_string()),
            "{lines:#?}"
        );
    }

    #[test]
    fn a_pattern_longer_than_the_track_it_lands_on_is_said_before_it_is_discovered() {
        // The write never moves a track's LEN, so the tail is stored and silent.
        let lines = confirm_lines(&Facts { source_len: 64, dest_len: 16, ..facts() });
        let len = lines.iter().find(|l| l.contains("steps long")).expect("the LEN line");
        assert!(len.contains("16 steps long on the box and this one is 64"), "{len}");
        assert!(len.contains("raise the track's LEN"), "{len}");

        // A destination that is longer is not a surprise, so it says nothing.
        let lines = confirm_lines(&Facts { source_len: 16, dest_len: 64, ..facts() });
        assert!(!lines.iter().any(|l| l.contains("steps long")), "{lines:#?}");
    }

    #[test]
    fn the_shared_impact_wording_is_in_here_rather_than_reworded() {
        // Swing is the one that reaches past the track being written, and this
        // dialog is where `safe_write_track` expects it to be said. Compared
        // against `write_impact_lines` itself, so a change there cannot leave
        // this dialog quietly saying the old thing.
        let f = Facts { swing: Some(65), box_swing: 50, ..facts() };
        let lines = confirm_lines(&f);
        for line in write_impact_lines(&ImpactArgs {
            label: f.label,
            track: Some(f.track_index),
            lanes: f.lanes,
            box_plocks: f.box_plocks,
            free_lanes: Some(f.free_lanes),
            track_prob: f.track_prob,
            swing: f.swing,
            box_swing: Some(f.box_swing),
        }) {
            assert!(lines.contains(&line), "missing {line:?} from {lines:#?}");
        }
        assert!(lines.iter().any(|l| l.contains("all 16 tracks")), "{lines:#?}");
    }

    #[test]
    fn everything_the_export_could_not_carry_is_in_the_dialog_that_agrees_to_the_write() {
        let warnings = vec!["the CUTOFF lane wasn't sent — it belongs to a DN2".to_string()];
        let lines = confirm_lines(&Facts { warnings: &warnings, ..facts() });
        assert!(
            lines.contains(&format!("Note: {}", warnings[0])),
            "a lane that cannot be written is a loss to agree to, not a surprise: {lines:#?}"
        );
    }

    #[test]
    fn a_pattern_from_another_box_is_flagged_and_a_running_transport_is_too() {
        let lines = confirm_lines(&Facts {
            from_other_box: Some("digitone2"),
            playing: true,
            ..facts()
        });
        assert!(lines.iter().any(|l| l == "Note: this pattern came from a digitone2."));
        assert!(lines.iter().any(|l| l.contains("transport is running")), "{lines:#?}");

        // Neither is invented when neither is true.
        let quiet = confirm_lines(&facts());
        assert!(!quiet.iter().any(|l| l.contains("came from")));
        assert!(!quiet.iter().any(|l| l.contains("transport")));
    }

    #[test]
    fn the_backup_is_the_last_thing_said_on_every_path() {
        // `BACKUP_LINE` last is the rule every write path in the repo follows —
        // no dialog may imply the backup is optional, and none may bury it.
        for f in [
            facts(),
            Facts { existing_trigs: 9, ..facts() },
            Facts { source_len: 64, playing: true, ..facts() },
        ] {
            assert_eq!(confirm_lines(&f).last().map(String::as_str), Some(BACKUP_LINE));
        }
    }

    #[test]
    fn a_midi_track_says_so_and_a_sound_track_gives_its_name() {
        let spec = model_for_key("DT2").and_then(|m| m.spec()).expect("DT2 has a spec");
        let mut kit = PatternKit {
            version: 3,
            name: String::new(),
            tempo_bpm: 120.0,
            kit_index: 0,
            tracks: Vec::new(),
            kit: digi_protocol::pattern::KitInfo {
                version: 3,
                name: "JO_KIT".into(),
                sound_names: vec!["BD".into(), "  ".into()],
                midi_mask: 0,
            },
        };
        assert_eq!(track_kind_label(&kit, 0, spec.track_kind_fallback), "BD");
        // An unnamed sound falls back to what this box's tracks are.
        assert_eq!(track_kind_label(&kit, 1, spec.track_kind_fallback), "sample");
        // Past the end of the names — a DN2's mask is always 0 and its tracks
        // are synths.
        assert_eq!(track_kind_label(&kit, 9, "synth"), "synth");
        // A MIDI track is a MIDI track whatever the sound name says.
        kit.kit.midi_mask = 1 << 0;
        assert_eq!(track_kind_label(&kit, 0, spec.track_kind_fallback), "MIDI");
    }

    #[test]
    fn a_box_that_is_not_the_one_this_row_names_is_refused() {
        // 42 is the DT2's product id and 43 the DN2's, per `protocol::device`.
        assert_eq!(wrong_box(Some("digitakt2"), "Digitakt II", &identity(42)), None);
        let refusal = wrong_box(Some("digitakt2"), "Digitakt II", &identity(43))
            .expect("a DN2 on the DT2's row is a mis-cabled desk");
        assert!(refusal.contains("refusing to write"), "{refusal}");
        assert!(refusal.contains("Digitone II"), "{refusal}");
    }

    #[test]
    fn a_box_with_both_ports_can_be_written_to_and_each_missing_end_is_named() {
        let nothing_known = PortsPresent::unknown();
        assert_eq!(blocker(&wired(), nothing_known), None);

        let mut device = wired();
        device.io.input = None;
        // Not "you cannot send" — you could; you could not check.
        assert!(
            blocker(&device, nothing_known).unwrap().contains("verifies"),
            "{:?}",
            blocker(&device, nothing_known)
        );

        let mut device = wired();
        device.io.output = None;
        assert!(blocker(&device, nothing_known).unwrap().contains("out port"));
    }

    /// A port list the OS might hand back, for the cable tests below.
    fn plugged(names: &[&str]) -> Vec<digi_midi::PortInfo> {
        names
            .iter()
            .map(|n| digi_midi::PortInfo {
                id: (*n).into(),
                name: (*n).into(),
                slug: digi_protocol::device::slug_from_port_name(n),
            })
            .collect()
    }

    // Pull a box's USB and the write path has to say so. Before this, `blocker`
    // only ever asked whether a port had been *chosen*, so a box whose cable had
    // gone looked identical to one sitting on the desk — and the write went as
    // far as taking a backup and asking for consent before failing at the
    // re-fetch, which named the wrong thing entirely.
    #[test]
    fn a_box_whose_cable_has_gone_is_refused_before_a_backup_is_taken() {
        let device = wired();
        let live = plugged(&["in", "out", "IAC Driver Bus 1"]);
        let present = PortsPresent { inputs: &live, outputs: &live };
        assert_eq!(blocker(&device, present), None, "both ends are plugged in");

        // The out port vanishes: the dump has nowhere to go.
        let without_out = plugged(&["in"]);
        let gone = PortsPresent { inputs: &live, outputs: &without_out };
        let why = blocker(&device, gone).expect("a missing out port must refuse");
        assert!(why.contains("out"), "it names the port that went: {why}");
        assert!(why.contains("no longer plugged in"), "{why}");

        // And the in port: the verify read has nowhere to come back on, which is
        // the end that decides whether a write worked at all.
        let without_in = plugged(&["out"]);
        let gone = PortsPresent { inputs: &without_in, outputs: &live };
        assert!(blocker(&device, gone).unwrap().contains("no longer plugged in"));
    }

    // The false-lockout case, and the reason `holds` answers `true` on an empty
    // list. `PortsPanel` starts with no enumeration at all and only fills on its
    // first refresh — which `AutoConnect::tick` skips entirely when auto-connect
    // is switched off — so a correctly-cabled desk must not be refused its own
    // boxes for want of a list nobody has built yet.
    #[test]
    fn an_unenumerated_port_list_is_unknown_rather_than_unplugged() {
        assert_eq!(blocker(&wired(), PortsPresent::unknown()), None);

        // But a list that exists and does not hold the port is a real answer.
        let elsewhere = plugged(&["Some Other Thing"]);
        let present = PortsPresent { inputs: &elsewhere, outputs: &elsewhere };
        assert!(blocker(&wired(), present).is_some(), "a populated list is believed");
    }

    #[test]
    fn an_imported_pattern_aims_back_at_the_slot_it_came_from() {
        let mut pattern = Pattern::for_model(&DT2);
        pattern.source =
            Some(Source { device_slug: "digitakt2".into(), bank: 2, index: 5 });
        let from = PatternRef::new(0, 0);

        assert_eq!(aim(Some(&pattern), Some("digitakt2"), from), PatternRef::new(2, 5));
        assert!(is_home(Some(&pattern), Some("digitakt2"), PatternRef::new(2, 5)));
        assert!(!is_home(Some(&pattern), Some("digitakt2"), PatternRef::new(0, 0)));
    }

    #[test]
    fn a_pattern_from_another_box_does_not_aim_this_one_anywhere() {
        // Provenance is per box, and C06 on a Digitone is not C06 here. The slot
        // of the same name is the only answer that is not a guess.
        let mut pattern = Pattern::for_model(&DT2);
        pattern.source =
            Some(Source { device_slug: "digitone2".into(), bank: 2, index: 5 });
        let from = PatternRef::new(0, 3);
        assert_eq!(aim(Some(&pattern), Some("digitakt2"), from), from);
        assert!(!is_home(Some(&pattern), Some("digitakt2"), PatternRef::new(2, 5)));

        // And a pattern drawn here, which has never been near a box, aims at the
        // slot of its own name.
        let fresh = Pattern::for_model(&DT2);
        assert_eq!(aim(Some(&fresh), Some("digitakt2"), from), from);
        assert_eq!(aim(None, Some("digitakt2"), from), from);
    }

    #[test]
    fn a_track_the_box_named_shows_that_name_in_the_picker() {
        let mut device = wired();
        let pattern = device.pattern_mut(0).unwrap();
        pattern.track_mut(0).unwrap().name = "BD".into();
        pattern.track_mut(0).unwrap().notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];

        let from = PatternRef::new(0, 0);
        assert_eq!(track_label(&device, from, 0), "T1 BD");
        // An untouched track is called T2, and "T2 T2" is not a label.
        assert_eq!(track_label(&device, from, 1), "T2");
    }
}
