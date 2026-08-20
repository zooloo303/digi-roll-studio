// The BACKUPS group of the Setup panel: put one of the patterns this app
// overwrote back on the box it came off.
//
// **This is the second thing in `app/` that can store bytes on hardware**, and it
// is the larger of the two: `ui::write` replaces one track of a slot, and every
// path below replaces a *whole* slot — sixteen tracks, the kit and its sounds. As
// with that file, the safety is not here. Every path ends at
// `digi_protocol::safe_write::safe_restore_pattern_kit`, and a second route to a
// store anywhere in this crate is a bug rather than a shortcut.
//
// Everything under the button already existed and had no caller:
// `Stash::backups` is documented in `protocol` as *"this is the restore list"*,
// `Stash::payload` as *"ready for `safe_restore_pattern_kit`"*, and the function
// itself has been tested since the store landed. What was missing was any way to
// see the list, which made `write_result_message`'s "restore it from Backups" a
// promise the UI could not keep — and by the twelfth session there were real
// backups of real overwrites sitting in the store with nothing able to show them.
//
// ## Six decisions
//
//  1. **The list is per box, filtered by slug, and that is the safety rule made
//     invisible rather than enforced.** A Digitakt pattern cannot go on a
//     Digitone: different lengths, different kit, different everything. Filtering
//     `Stash::backups(Some(slug))` into each box's own block means a
//     cross-family restore is not a thing the UI can express, which is better
//     than a thing it refuses. `write::wrong_box` still runs at send time, because
//     that one is about the *cable* rather than about the list.
//  2. **A backup goes back to the slot it came from, and there is no picker.**
//     `StashEntry::index` is the destination. `safe_restore_pattern_kit` will take
//     any index, so sending a capture of A01 to A05 is a thing the layer beneath
//     can do — but that is a copy, not a restore, and inventing it here would put
//     the whole-slot write path's only untested shape in the UI. Same argument as
//     `ui::write`'s one track picker.
//  3. **The newest backup for the box is selected by default**, because "put it
//     back" almost always means the last thing this app overwrote. A picked row
//     sticks. A selection whose file the ring has since evicted falls back to the
//     newest rather than quietly becoming whichever row moved into its place —
//     which is the one way a list-plus-button can restore something nobody chose.
//  4. **The confirm dialog says less than the write's, on purpose.** It cannot
//     say "this replaces the 15 trigs on that track", because nothing decodes the
//     destination: a restore has to work on a slot whose bytes may not decode at
//     all, which is `WriteHooks::confirm_restore`'s whole reason for not being
//     `confirm`. So the dialog names the *capture* — box, slot, kit, when it was
//     taken and what it was taken before — and says outright that it cannot
//     describe what is being replaced. A dialog that is quiet about that reads as
//     a dialog that checked.
//  5. **The store travels in the job.** `ui::write`'s worker resolves
//     `Stash::default_stash()` for itself, which is right when the stash is only
//     somewhere to put a file. Here the stash is where the bytes are *coming
//     from*, so the list and the restore have to be the same directory or a row
//     could name a file the restore then looks for somewhere else. `Stash` is a
//     `PathBuf` and clones for nothing.
//  6. **Pre-restore snapshots are behind a toggle, off by default.** The store
//     keeps `SNAPSHOT_MAX` of them and `Stash::backups` deliberately hides them,
//     for the reason its module doc gives: rows saying "here is the thing you
//     just decided was wrong" are noise between the patterns you are looking for.
//     But with no way to see them at all, the ring would be dead weight and
//     `SNAPSHOT_LINE`'s promise that a restore "can be undone" would be the same
//     kind of unkeepable promise this file exists to fix.
//
// **What this deliberately does not do.** It does not touch the session: a
// restored slot is what the *box* holds, and nothing decodes it here, so claiming
// anything about our own model would be a guess. Fetch is one group up the panel
// for anyone who wants the restored pattern back in the app. It also does not stop
// the transport, for the reason `ui::write` does not.
//
// **Nothing here has met hardware.** The flow is driven end to end against a fake
// box in `app/tests/restore.rs`, including the confirm crossing a thread, and
// `protocol/tests/safe_write.rs` has had the function itself covered since it
// landed — but no backup has been sent to a real box from this button, and the
// first press is Neil's, at the desk, on a slot he is willing to lose.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

use digi_core::{DeviceId, Session};
use digi_midi::{ElektronDevice, PortBinding};
use digi_protocol::backup_stash::{Stash, StashEntry};
use digi_protocol::safe_write::{
    restore_result_message, safe_restore_pattern_kit, ConfirmArgs, PatternIo, ResultMessage,
    Timestamp, WriteHooks, SNAPSHOT_LINE,
};
use eframe::egui::{self, Ui};

use crate::ui::session::{export_backup, Chooser, NativeChooser};
use crate::ui::transfer::binding;
use crate::ui::write::{blocker, wrong_box, PortsPresent};

/// How many rows a box shows before the list has to be asked for in full.
///
/// The Setup panel scrolls as one — no nested scroll areas in a 300px column —
/// so a hundred rows of two boxes' full rings would bury the ports panel under
/// them. Five is the depth of a mistake you are still holding in your head.
const PREVIEW_ROWS: usize = 5;

// --- what crosses the thread ---------------------------------------------------

/// Everything the press captured, so the worker never borrows the session.
#[derive(Debug)]
pub struct Job {
    pub device: DeviceId,
    /// The slug of the box this block names — what the identity handshake has to
    /// come back with. `None` for a model with no dumps, which cannot get here.
    pub slug: Option<&'static str>,
    pub display: &'static str,
    pub input: PortBinding,
    pub output: PortBinding,
    /// The store the row was listed from, and the one the payload is read back
    /// out of — decision 5.
    pub stash: Stash,
    /// The row being restored. Its `index` is the destination (decision 2) and
    /// its `file` is the key the payload is read by.
    pub entry: StashEntry,
    pub playing: bool,
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
    /// What the button that goes through with it says — named after the slot it
    /// replaces, because a button that says "OK" is one you press by reflex.
    pub button: String,
    pub reply: Sender<bool>,
}

/// A finished restore, as the block shows it.
#[derive(Debug)]
pub struct Report {
    pub message: ResultMessage,
    pub cancelled: bool,
    /// The snapshot line `safe_restore_pattern_kit` logged, so the block can say
    /// where the state being reverted away from went.
    pub log: Option<String>,
}

// --- the blocking half ----------------------------------------------------------

/// Everything after the ports are open, with the box injected.
///
/// Generic over [`PatternIo`] for the reason the trait exists: `app/tests/restore.rs`
/// drives this exact function against a box that is a `BTreeMap`, so the only thing
/// left untested by the time a cable is involved is the cable.
pub fn run(
    device: &mut impl PatternIo,
    job: &Job,
    events: &Sender<Event>,
    now: Timestamp,
) -> Result<Report, String> {
    let identity = device
        .identity()
        .cloned()
        .ok_or_else(|| "the box did not answer the identity handshake".to_string())?;
    // Decision 1's other half, before anything is read and long before anything
    // is sent. The list cannot offer another family's backup; this catches the
    // cable that is not plugged where the panel thinks it is.
    if let Some(refusal) = wrong_box(job.slug, job.display, &identity) {
        return Err(refusal);
    }

    // Read before the flow starts rather than at the press: it is ~125 KB off
    // disk, and a frame is not the place for it. A file the ring evicted or a
    // user deleted has to be a legible refusal rather than a `None` unwrapped
    // somewhere inside the write path.
    let payload = job.stash.payload(&job.entry.file).ok_or_else(|| {
        format!(
            "that backup can't be read any more: {} is gone from the store, or isn't a pattern \
             dump. Nothing was sent.",
            job.entry.file
        )
    })?;

    let mut hooks = UiHooks { events, job, device_name: identity.name.clone(), log: None };
    let result =
        safe_restore_pattern_kit(device, &job.stash, job.entry.index, &payload, &mut hooks, now)
            .map_err(|e| e.to_string())?;

    Ok(Report {
        message: restore_result_message(&result),
        cancelled: result.cancelled,
        log: hooks.log,
    })
}

/// The hooks `safe_restore_pattern_kit` calls, forwarded to the UI thread.
struct UiHooks<'a> {
    events: &'a Sender<Event>,
    job: &'a Job,
    device_name: String,
    log: Option<String>,
}

impl WriteHooks for UiHooks<'_> {
    /// Never called on this path, and that is the point of it being a separate
    /// hook: `ConfirmArgs` describes a pattern that decoded, and a restore does
    /// not decode one. Left at the trait's default, which consents — harmless
    /// here because nothing on this path can reach it, and the one that *is*
    /// reached is below.
    fn confirm(&mut self, _args: &ConfirmArgs) -> bool {
        debug_assert!(false, "a restore consents through confirm_restore");
        false
    }

    fn confirm_restore(&mut self, label: &str, _index: u8) -> bool {
        let lines = confirm_lines(&Facts {
            device_name: &self.device_name,
            label,
            entry: &self.job.entry,
            playing: self.job.playing,
        });
        let (reply, answer) = channel();
        let ask = Ask { lines, button: format!("Restore {label}"), reply };
        if self.events.send(Event::Ask(ask)).is_err() {
            return false;
        }
        // Blocks until the dialog is answered — or forever less one frame, if the
        // window has gone, in which case the channel closes and this is a no.
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
        let mut device =
            ElektronDevice::open(&job.input, &job.output).map_err(|e| e.to_string())?;
        let _ = events.send(Event::Status("Asking the box who it is…".into()));
        device.identify().map_err(|e| e.to_string())?;
        run(&mut device, &job, &events, Timestamp::now())
    })();
    let _ = events.send(Event::Done(outcome));
}

// --- the wording ----------------------------------------------------------------

/// Everything the confirm dialog says, as plain values — so the sentences a
/// person agrees to are testable without a box, a thread or a window.
pub struct Facts<'a> {
    /// The box that answered, by its own name.
    pub device_name: &'a str,
    /// The destination slot as the box labels it, from the flow rather than from
    /// the row: it is derived from the index being sent to, and having the dialog
    /// and the send disagree about which slot is the one bug this cannot have.
    pub label: &'a str,
    pub entry: &'a StashEntry,
    pub playing: bool,
}

/// The sentences a restore is agreed to on.
///
/// **The interesting half is what is missing.** `ui::write`'s dialog opens by
/// counting the trigs it is about to replace, and this one cannot: nothing decodes
/// the destination, because a restore exists to rescue a slot whose bytes may not
/// decode at all. So the capture is named in full and the silence about the
/// destination is stated rather than left to be read as reassurance — decision 4.
pub fn confirm_lines(f: &Facts) -> Vec<String> {
    let mut lines = vec![
        format!("Restore {} on the {} from a backup?", f.label, f.device_name),
        String::new(),
        // `protocol`'s own wording for a row, so the dialog, the list and any log
        // line cannot come to describe the same capture differently.
        f.entry.summary(),
        String::new(),
        format!(
            "This replaces the whole of {} — all sixteen tracks, the kit and its sounds — not one \
             track.",
            f.label
        ),
        format!(
            "Whatever is in {} now goes, including anything written to it since that backup was \
             taken.",
            f.label
        ),
        "What's in there isn't decoded first, so this can't tell you what's being replaced: a \
         restore has to work on a slot whose bytes may not make sense at all."
            .to_string(),
    ];

    // Said, not refused — the same bargain `ui::write` strikes, and for the same
    // reason: what a box does with a 127 KB dump while it is sequencing is not
    // something this repo has measured.
    if f.playing {
        lines.push(
            "The transport is running — this app keeps clocking the box while the dump goes \
             across, and pressing this does not stop it."
                .to_string(),
        );
    }

    lines.push(String::new());
    lines.push(SNAPSHOT_LINE.to_string());
    lines
}

// --- the panel --------------------------------------------------------------------

/// One box's block: which row is picked, and the last answer.
#[derive(Default)]
struct Row {
    /// The picked row's `file`, which is the stash's own key. **Not an index** —
    /// the ring shifts under a list, and an index would silently come to mean a
    /// different capture (decision 3).
    selected: Option<String>,
    /// Whether this box's full ring is on screen rather than the newest few.
    expanded: bool,
    outcome: Option<Outcome>,
}

enum Outcome {
    Done(Report),
    /// Anything that stopped it before `safe_restore_pattern_kit` returned: a
    /// port that would not open, the wrong box, a firmware the format was never
    /// verified against, a backup file that had gone.
    Failed(String),
}

/// One box's rows, as last read off the store.
#[derive(Default)]
struct Lists {
    backups: Vec<StashEntry>,
    snapshots: Vec<StashEntry>,
}

/// A restore in flight.
struct Pending {
    device: DeviceId,
    rx: Receiver<Event>,
    status: String,
}

/// What is on screen instead of the panel, if anything.
enum Dialog {
    Confirm { ask: Ask },
    /// A finished restore that must not be scrolled past.
    Alert { text: String },
}

pub struct RestorePanel {
    rows: HashMap<DeviceId, Row>,
    lists: HashMap<DeviceId, Lists>,
    /// The store, or why there isn't one. Resolved once and kept: it is a couple
    /// of environment variables and a path join, and it fails the same way every
    /// frame if it fails at all.
    store: Option<Result<Stash, String>>,
    /// What [`Stash::generation`] said when the lists were last read.
    ///
    /// The outer `None` is "never read, or asked to read again" — what the refresh
    /// button sets. The inner one is the store's own answer, which is `None` for a
    /// store with nothing in it yet, so the two cannot be collapsed.
    listed: Option<Option<std::time::SystemTime>>,
    show_snapshots: bool,
    pending: Option<Pending>,
    dialog: Option<Dialog>,
    /// How Export asks for a path. Its own rather than the Session panel's: this
    /// group is drawn inside Setup and never sees that panel, and the seam is a
    /// trait precisely so two owners cost nothing.
    chooser: Box<dyn Chooser>,
    /// Where the last export went, or why it did not. Per panel rather than per
    /// box: one dialog is open at a time, so one answer is what there is to say.
    exported: Option<String>,
}

impl Default for RestorePanel {
    fn default() -> Self {
        Self {
            rows: HashMap::new(),
            lists: HashMap::new(),
            store: None,
            listed: None,
            show_snapshots: false,
            pending: None,
            dialog: None,
            chooser: Box::new(NativeChooser),
            exported: None,
        }
    }
}

impl RestorePanel {
    /// A panel reading a store the caller chose rather than the platform's.
    ///
    /// Exists for the reason [`Stash::at`] exists beside [`Stash::default_stash`]:
    /// a test must not read the real backup folder. On the machine this was written
    /// on, that folder holds the backups of the first hardware write — so a test
    /// asserting what a list contains would be asserting something about Neil's
    /// desk, and would have started failing the day he used the app.
    pub fn at(stash: Stash) -> Self {
        Self { store: Some(Ok(stash)), ..Self::default() }
    }

    /// Whether a restore is in flight, so the other two transfer buttons can be
    /// held off. One transfer at a time, in any direction.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Show or hide the pre-restore snapshots — decision 6's toggle, as state
    /// rather than as a click.
    ///
    /// A display preference with a setter, so a headless pass can assert the thing
    /// worth asserting about it: that both rings are read whatever it says, and
    /// ticking it therefore reveals rows rather than starting a re-read.
    pub fn show_snapshots(&mut self, on: bool) {
        self.show_snapshots = on;
    }

    /// The rows one box's block is offering, newest first.
    ///
    /// The panel's own view of the store, which is what makes the freshness rules
    /// above assertable from a headless pass — they live inside a draw, and without
    /// this there is nothing to look at afterwards. Reflects the snapshot toggle,
    /// because what a block *shows* is the thing worth being sure about.
    pub fn showing(&self, device: DeviceId) -> Vec<&StashEntry> {
        let Some(lists) = self.lists.get(&device) else { return Vec::new() };
        if self.show_snapshots {
            lists.backups.iter().chain(lists.snapshots.iter()).collect()
        } else {
            lists.backups.iter().collect()
        }
    }

    /// Take whatever the worker has said, and put up the dialog it is waiting on.
    ///
    /// **Called from the window rather than from the panel**, for the reason
    /// `ui::write::tick` is: a collapsed panel's body does not run, so a dialog
    /// living beside the rows would leave a worker blocked forever on a question
    /// nobody could be shown the moment Setup was closed.
    pub fn tick(&mut self, ui: &mut Ui) {
        self.poll();
        if self.pending.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.dialog_ui(ui);
    }

    /// Draw the group. Never edits the session.
    ///
    /// **The lists follow the store rather than being told when to re-read.** Every
    /// frame compares [`Stash::generation`] — one `stat` — against what the lists
    /// were read at, so the backup of a write that has just finished is in the list
    /// by the time anyone looks, and so is a snapshot a restore left, and so is
    /// anything a second instance or a tidy-up did to the folder. The first design
    /// took a count of finished writes from `ui::write` and re-read when it moved;
    /// it worked, and it put the freshness rule in three places that each had to
    /// remember to say so. Two of them were wrong in a deliberate-bug pass and
    /// nothing noticed.
    pub fn ui(&mut self, ui: &mut Ui, session: &Session, present: PortsPresent<'_>, blocked: bool, playing: bool) {
        if session.devices.is_empty() {
            ui.weak("No boxes in this session.");
            return;
        }

        let store = self
            .store
            .get_or_insert_with(|| Stash::default_stash().map_err(|e| e.to_string()));
        let Ok(stash) = store.clone() else {
            // Nowhere to keep backups is the same condition that stops a write
            // happening at all, so it is stated rather than shown as an empty
            // list — an empty list would read as "you have never overwritten
            // anything", which is a different and much more comforting thing.
            ui.colored_label(
                super::CAUTION,
                store.as_ref().err().cloned().unwrap_or_default(),
            );
            return;
        };

        let generation = stash.generation();
        if self.listed != Some(generation) {
            self.reload(session, &stash);
            self.listed = Some(generation);
        }

        let devices: Vec<DeviceId> = session.devices.iter().map(|d| d.id).collect();
        let last = devices.len().saturating_sub(1);
        for (position, id) in devices.into_iter().enumerate() {
            self.device_ui(ui, session, present, &stash, id, blocked, playing);
            if position != last {
                ui.add_space(6.0);
            }
        }

        ui.add_space(4.0);
        // Decision 6. Worded as what they are rather than as "show all", because
        // the two kinds are not the same thing seen at different depths.
        //
        // **Purely a display filter**, which is why it needs no invalidation:
        // `reload` reads both rings every time. The draft that read snapshots only
        // while the box was ticked needed this checkbox to remember to re-list, and
        // a deliberate-bug pass showed that forgetting it left the toggle doing
        // nothing at all with no test able to see it. Two directory reads a
        // generation is a cheaper price than a rule that can be forgotten.
        ui.checkbox(&mut self.show_snapshots, "Show pre-restore snapshots")
            .on_hover_text(
                "What each restore found in the slot before it ran, kept separately so it cannot \
                 push a real backup out of the list. This is the way back from a restore you \
                 didn't mean.",
            );

        ui.horizontal(|ui| {
            if ui
                .small_button("Refresh")
                .on_hover_text("Read the backup folder again")
                .clicked()
            {
                self.listed = None;
            }
            ui.weak(format!("{} kept per box", digi_protocol::backup_stash::STASH_MAX));
        });
        ui.label(
            egui::RichText::new(format!(
                "Each one is a replayable dump in {} — any MIDI utility can send one at a box, \
                 even if this app won't start.",
                stash.dir().display()
            ))
            .weak()
            .italics(),
        );
    }

    /// One box's block.
    fn device_ui(
        &mut self,
        ui: &mut Ui,
        session: &Session,
        present: PortsPresent<'_>,
        stash: &Stash,
        id: DeviceId,
        blocked: bool,
        playing: bool,
    ) {
        let Some(device) = session.device(id) else { return };
        ui.label(egui::RichText::new(&device.name).strong());

        // The same two ends as a write, and they fail for the same reasons: the
        // dump goes out on the output and the verify read comes back on the input.
        if let Some(reason) = blocker(device, present) {
            ui.weak(reason);
            return;
        }

        let busy = blocked || self.pending.is_some() || self.dialog.is_some();
        let in_flight = self.pending.as_ref().is_some_and(|p| p.device == id);
        let show_snapshots = self.show_snapshots;
        let (was_selected, was_expanded) = self
            .rows
            .get(&id)
            .map(|r| (r.selected.clone(), r.expanded))
            .unwrap_or_default();

        // **The rows are borrowed rather than taken out of the map and put back.**
        // The first draft did take them, because the button below needs `&mut
        // self` — and a take-and-return leaves one line (`insert`) whose loss would
        // empty a box's list for the rest of the session, silently, with nothing
        // able to test it. So the block below borrows `self.lists`, produces only
        // owned values, and every mutation happens after the borrow has gone. The
        // shape makes the bug unrepresentable instead of guarding it.
        let (selected, expanded, clicked, export) = {
            let lists = self.lists.get(&id);
            let rows: Vec<&StashEntry> = match lists {
                Some(l) if show_snapshots => l.backups.iter().chain(l.snapshots.iter()).collect(),
                Some(l) => l.backups.iter().collect(),
                None => Vec::new(),
            };

            if rows.is_empty() {
                ui.weak(if show_snapshots {
                    "Nothing from this box is in the store yet."
                } else {
                    "Nothing from this box has been overwritten yet."
                });
                return;
            }

            let mut selected = pick(&rows, was_selected.as_deref()).map(|e| e.file.clone());
            let mut expanded = was_expanded;

            let shown = if expanded { rows.len() } else { rows.len().min(PREVIEW_ROWS) };
            for entry in &rows[..shown] {
                let picked = selected.as_deref() == Some(entry.file.as_str());
                // The whole summary, wrapped, rather than a truncated line: the
                // point of a row is telling one capture from another, and the
                // fields that do that are at the end of it.
                if ui
                    .selectable_label(picked, egui::RichText::new(entry.summary()).small())
                    .on_hover_text(&entry.file)
                    .clicked()
                {
                    selected = Some(entry.file.clone());
                }
            }
            if rows.len() > PREVIEW_ROWS {
                let text = if expanded {
                    format!("Show the newest {PREVIEW_ROWS}")
                } else {
                    format!("Show all {}", rows.len())
                };
                if ui.small_button(text).clicked() {
                    expanded = !expanded;
                }
            }

            // The destination is the capture's own slot, so the button can name it
            // before the box has been asked anything — decision 2.
            let mut clicked = None;
            let mut export = None;
            if let Some(entry) = pick(&rows, selected.as_deref()) {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if ui
                            .button(format!("Restore {}", entry.bank))
                            .on_hover_text(
                                "Put this capture back in the slot it came from. What that slot \
                                 holds now is read and saved first, you get to read what it \
                                 replaces before anything is sent, and it is verified byte for \
                                 byte afterwards.",
                            )
                            .clicked()
                        {
                            clicked = Some(entry.clone());
                        }
                    });
                    // **Not held off by `busy`.** Every other button in this group
                    // can reach a box; this one is a file copy that cannot, so
                    // disabling it during a transfer would be theatre.
                    if ui
                        .button("Export…")
                        .on_hover_text(
                            "Copy this capture to a file you choose. A plain copy of the same \
                             replayable dump, so any MIDI utility can send it at a box.",
                        )
                        .clicked()
                    {
                        export = Some(entry.file.clone());
                    }
                });
            }
            (selected, expanded, clicked, export)
        };

        let row = self.rows.entry(id).or_default();
        row.selected = selected;
        row.expanded = expanded;

        if let Some(file) = export {
            // `None` is a cancelled dialog, which leaves the last answer standing
            // rather than replacing it with silence.
            if let Some(result) = export_backup(self.chooser.as_mut(), stash, &file) {
                self.exported = Some(match result {
                    Ok(dest) => format!("Exported to {}", dest.display()),
                    Err(why) => why,
                });
            }
        }
        if let Some(text) = &self.exported {
            ui.label(egui::RichText::new(text).small().weak());
        }

        if let Some(entry) = clicked {
            self.start(id, session, present, stash, &entry, playing);
        }

        if in_flight {
            ui.horizontal(|ui| {
                ui.spinner();
                let status =
                    self.pending.as_ref().map(|p| p.status.clone()).unwrap_or_default();
                ui.label(status);
            });
            return;
        }

        self.outcome_ui(ui, id);
    }

    /// The last answer on this box.
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

        let response = egui::Modal::new(egui::Id::new("restore-dialog")).show(ui.ctx(), |ui| {
            ui.set_max_width(520.0);
            match dialog {
                Dialog::Confirm { ask } => {
                    ui.label(egui::RichText::new("Restore a backup to the box").strong());
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
                    ui.label(egui::RichText::new("The restore did not go as asked").strong());
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
        // because there is no dialog here whose destructive answer is the default.
        if response.should_close() {
            answer = Some(false);
        }

        let Some(answer) = answer else { return };
        match self.dialog.take() {
            // A send that fails means the worker has gone, and a worker that has
            // gone consented to nothing.
            Some(Dialog::Confirm { ask }) => {
                let _ = ask.reply.send(answer);
            }
            Some(Dialog::Alert { .. }) | None => {}
        }
    }

    /// Put a restore out. Does nothing if anything is already in flight.
    fn start(
        &mut self,
        id: DeviceId,
        session: &Session,
        present: PortsPresent<'_>,
        stash: &Stash,
        entry: &StashEntry,
        playing: bool,
    ) {
        if self.pending.is_some() || self.dialog.is_some() {
            return;
        }
        let job = match plan(session, present, id, stash, entry, playing) {
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
                Event::Log(s) => pending.status = s,
                Event::Ask(ask) => {
                    self.dialog = Some(Dialog::Confirm { ask });
                    return;
                }
                Event::Done(outcome) => {
                    let outcome = match outcome {
                        Ok(report) => {
                            // A verify mismatch on a *restore* is the worst news
                            // this app can deliver — the recovery itself did not
                            // land — so it goes in front of the person who asked.
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
                    // Nothing to invalidate: a restore that got as far as the box
                    // left a snapshot, which moved the store's generation, which
                    // the next frame notices on its own.
                    return;
                }
            }
        }
    }

    /// Re-read every box's rows off the store.
    fn reload(&mut self, session: &Session, stash: &Stash) {
        self.lists.clear();
        for device in &session.devices {
            let Some(slug) = device.model.slug else { continue };
            self.lists.insert(
                device.id,
                Lists { backups: stash.backups(Some(slug)), snapshots: stash.snapshots(Some(slug)) },
            );
        }
    }
}

/// Which row a block is offering to restore: the one that was picked, or the
/// newest.
///
/// **The `or_else` is decision 3 and it is not a convenience.** A selection is
/// held as a `file`, and the ring evicts — so by the time a frame is drawn, the
/// picked row may be gone. Falling back to the newest is the only answer that
/// cannot surprise anyone; the shape to avoid is holding an *index*, where a list
/// that shifted under it would leave the button aimed at whichever capture moved
/// into that position, still looking picked. Pure and out here so that property has
/// somewhere to be tested, because inside a draw closure it has nowhere.
pub fn pick<'a>(rows: &[&'a StashEntry], chosen: Option<&str>) -> Option<&'a StashEntry> {
    rows.iter()
        .copied()
        .find(|e| Some(e.file.as_str()) == chosen)
        .or_else(|| rows.first().copied())
}

/// Everything the press captures, in one function, so the thing a test drives is
/// the thing the button builds.
///
/// The only refusals that happen before a thread exists: a box with no ports, a
/// model with no dumps, and a capture that did not come off this family of box.
/// Everything past here needs the box to have answered.
pub fn plan(
    session: &Session,
    present: PortsPresent<'_>,
    id: DeviceId,
    stash: &Stash,
    entry: &StashEntry,
    playing: bool,
) -> Result<Job, String> {
    let device = session.device(id).ok_or("that box is not in this session")?;
    let (Some(input), Some(output)) = (device.io.input.clone(), device.io.output.clone()) else {
        return Err(blocker(device, present).unwrap_or_else(|| "that box has no ports".into()));
    };
    let slug = device
        .model
        .slug
        .ok_or_else(|| format!("{} has no patterns to restore", device.model.display))?;
    // Decision 1. The list this row came from was filtered by slug, so this
    // cannot be reached through the panel — and it is not a copy of that filter,
    // it is this function's own precondition: `entry` is an argument, and a
    // caller handing it a Digitone capture for a Digitakt would otherwise get a
    // whole-slot write of the wrong family's bytes.
    if entry.slug != slug {
        return Err(format!(
            "that backup came off a {} and this is the {} — refusing. A pattern cannot be moved \
             between families, and a restore is not the place to try.",
            entry.slug, device.model.display
        ));
    }

    Ok(Job {
        device: id,
        slug: device.model.slug,
        display: device.model.display,
        input: binding(&input),
        output: binding(&output),
        stash: stash.clone(),
        entry: entry.clone(),
        playing,
    })
}
