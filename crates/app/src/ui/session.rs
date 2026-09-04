// The Session panel: the rail's fourth slot, and the only thing in this app that
// can save what you have been doing.
//
// `core::project` has been able to round-trip a whole session since Phase 2 —
// boxes, patterns, scenes, tempo, with the device ids and the port bindings — and
// until this file existed nothing called any of it. Closing the window lost
// everything. That is the gap this closes, and it is lesson 7 in `DEVELOPMENT.md`
// arriving on schedule: a function first, a button after.
//
// ## What a session file is not
//
// It holds **no sample data, no kit and no sounds** — only what PLAN.md §2's
// model holds. Opening one puts nothing on a box. It must not be mistaken for a
// backup: backups are `protocol::backup_stash`, they are raw device dumps, they
// live in their own store with its own ring, and they are in Setup rather than
// here. The panel says so on screen rather than in a tooltip, per `ui::tools`'
// rule that the honest state of a thing is not something to hide behind a hover.
//
// ## Why the dialog is behind a trait
//
// [`Chooser`] exists so the decisions in this file — what gets written, when the
// dirty flag clears, what a refusal says, which boxes lost their ports — can be
// tested without a window and without a human clicking Cancel. `rfd` is the only
// implementation that ships ([`NativeChooser`]); `app/tests/all/session_panel.rs`
// drives a scripted one. It is the same move `Rehearsal` makes on the write path,
// and for the same reason: the interesting half is the decision, not the syscall.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Ui};

use digi_core::device::PortRef;
use digi_core::project::{Project, ProjectError};
use digi_core::{DeviceId, Session};

use super::recovery::{Cadence, Found, Recovery};

/// How the app asks the desktop for a path.
///
/// Both methods return `None` for "the user cancelled", which is a normal answer
/// and never an error: a cancelled Save must leave the dirty flag exactly as it
/// was, or the next close guard waves through work that was never written.
pub trait Chooser {
    /// Where to save. `suggested` is the filename to offer, not a directory.
    fn save_as(&mut self, suggested: &str) -> Option<PathBuf>;
    /// Which file to open.
    fn open(&mut self) -> Option<PathBuf>;
    /// Where to copy a backup. Separate from [`Chooser::save_as`] because the
    /// filter and the suggested name differ, and because a stash export is a
    /// `.syx` dump rather than a session.
    fn export_as(&mut self, suggested: &str) -> Option<PathBuf>;

    /// Where to write a Standard MIDI File, for the Edit panel's MIDI FILES group.
    ///
    /// A fourth and fifth method rather than a second trait, because there is one
    /// question being asked here — *which path* — and the reason each of these is
    /// separate is that a file dialog's title and filter are part of what makes it
    /// answerable. A dialog offering `.json` for a MIDI export is a dialog that
    /// has to be fought.
    fn export_midi_as(&mut self, suggested: &str) -> Option<PathBuf>;

    /// Which MIDI file to read.
    fn open_midi(&mut self) -> Option<PathBuf>;
}

/// The shipping [`Chooser`]: a real modal file dialog from `rfd`.
#[derive(Default)]
pub struct NativeChooser;

impl Chooser for NativeChooser {
    fn save_as(&mut self, suggested: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Save session")
            .add_filter("Digi Roll session", &["json"])
            .set_file_name(suggested)
            .save_file()
    }

    fn open(&mut self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Open session")
            .add_filter("Digi Roll session", &["json"])
            .pick_file()
    }

    fn export_as(&mut self, suggested: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Export backup")
            .add_filter("SysEx dump", &["syx"])
            .set_file_name(suggested)
            .save_file()
    }

    fn export_midi_as(&mut self, suggested: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Export MIDI file")
            .add_filter("Standard MIDI File", &["mid", "midi"])
            .set_file_name(suggested)
            .save_file()
    }

    fn open_midi(&mut self) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Import MIDI file")
            .add_filter("Standard MIDI File", &["mid", "midi"])
            .pick_file()
    }
}

/// The last thing that happened, shown in the panel until the next thing does.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Saved(PathBuf),
    Opened(PathBuf),
    /// A fresh session was started, off nothing on disk.
    New,
    /// A crash copy was taken back up. Carries the file it was a copy *of*, or
    /// `None` for work that had never been saved anywhere — which is the case
    /// where the recovery is the only copy there has ever been, and so the one
    /// the panel says the most about.
    Recovered(Option<PathBuf>),
    /// Already worded for a person: see [`describe`].
    Failed(String),
}

/// What the close guard is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloseGuard {
    #[default]
    Idle,
    /// The modal is up and the close has been cancelled for now.
    Asking,
    /// The user chose to lose the work. The next close request goes through
    /// untouched — without this the guard would ask again forever.
    Confirmed,
}

/// What the New guard is doing.
///
/// A **sibling** to [`CloseGuard`] rather than a third arm bolted onto it, and
/// that is the whole of how the two stay out of each other's way: they are
/// different fields, so answering one can never overwrite what the other
/// remembers. There is no `Confirmed` arm here the way there is on
/// `CloseGuard` — nothing outside this file retries a "New" request the way
/// the OS retries a close, so once the modal resolves there is nothing left
/// to remember.
///
/// The two can still both want the floor in the same frame — the OS can send
/// a close request while this modal is up, or (in principle) the reverse —
/// and [`SessionPanel::allow_close`] and [`SessionPanel::request_new`] each
/// check the *other's* state before opening their own modal, so at most one
/// of the two is ever `Asking` at once rather than stacking two questions
/// about the same unsaved work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewGuard {
    #[default]
    Idle,
    /// The modal is up and the New request has been held for now.
    Asking,
}

/// What one frame of the panel did, reported to the app shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    /// The panel's `×` was clicked.
    pub close: bool,
    /// The session in hand was replaced — by one off disk, or by a fresh one
    /// off `New` — so the engine has to be rebuilt around it. **Deliberately
    /// not the same flag as an edit** — a freshly opened or freshly started
    /// session is not unsaved work.
    pub reloaded: bool,
}

pub struct SessionPanel {
    chooser: Box<dyn Chooser>,
    /// Where this session was last saved to or opened from. `None` until a Save
    /// As has chosen one, which is what makes Save fall back to asking.
    path: Option<PathBuf>,
    /// Set by any frame that edited the session, cleared by a successful save.
    dirty: bool,
    status: Option<Status>,
    /// The boxes whose remembered ports were not there on load, by name.
    ///
    /// `from_json_with_ports` hands back ids and the panel has to turn them into
    /// something a person can act on: a box that silently lost its port is a box
    /// that silently stopped playing.
    lost_ports: Vec<String>,
    guard: CloseGuard,
    /// The New button's own guard — see [`NewGuard`] for why this is a
    /// separate field rather than a state shared with `guard`.
    new_guard: NewGuard,
    /// Whether the `?` in the title bar has this panel's reference prose open —
    /// the "WHAT THIS IS NOT" and "PATTERNS" paragraphs that used to sit
    /// permanently in the body. Persisted on the struct rather than threaded
    /// through as a parameter because [`super::panel_title_bar`] needs a place
    /// to toggle it in place, the same way [`disclosure_row`]'s `open` does —
    /// see `design_handoff_digi_roll_ui_v2/README.md`'s 2b rule 1.
    ///
    /// [`disclosure_row`]: super::disclosure_row
    reference_visible: bool,
    /// Where the crash copy goes. `None` in two cases that behave identically —
    /// a machine with no application-data directory, and any panel built by
    /// [`SessionPanel::with_chooser`], which is every panel in the test suite.
    /// **Tests opt in to a store rather than out of one**, so no test can write
    /// into the real user's directory by forgetting to.
    recovery: Option<Recovery>,
    /// When the next crash copy is due. See `ui::recovery` for the two clocks.
    cadence: Cadence,
    /// A copy left behind by a previous run, found at launch and waiting to be
    /// offered. `Some` is exactly "the modal is up".
    offer: Option<Found>,
    /// Whether [`SessionPanel::look_for_recovery`] has already run. The shell
    /// calls it every frame and it must only look once — a second look after the
    /// offer was declined would raise the same modal again.
    looked: bool,
}

impl Default for SessionPanel {
    fn default() -> Self {
        let panel = Self::with_chooser(Box::new(NativeChooser));
        // A machine with no home directory runs without a crash copy rather than
        // refusing to start. The Session panel says so on screen; see `ui`.
        match Recovery::default_store() {
            Ok(store) => panel.with_recovery(store),
            Err(_) => panel,
        }
    }
}

impl SessionPanel {
    pub fn with_chooser(chooser: Box<dyn Chooser>) -> Self {
        Self {
            chooser,
            path: None,
            dirty: false,
            status: None,
            lost_ports: Vec::new(),
            guard: CloseGuard::default(),
            new_guard: NewGuard::default(),
            reference_visible: false,
            recovery: None,
            cadence: Cadence::default(),
            offer: None,
            looked: false,
        }
    }

    /// Give this panel somewhere to put its crash copies.
    ///
    /// [`Default`] wires the real store in; [`SessionPanel::with_chooser`]
    /// deliberately does not, so a test that wants the behaviour asks for a
    /// directory of its own.
    pub fn with_recovery(mut self, recovery: Recovery) -> Self {
        self.recovery = Some(recovery);
        self
    }

    // --- state the shell reads -----------------------------------------------

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    pub fn lost_ports(&self) -> &[String] {
        &self.lost_ports
    }

    pub fn guard(&self) -> CloseGuard {
        self.guard
    }

    pub fn guard_new(&self) -> NewGuard {
        self.new_guard
    }

    /// Fold this frame's edit flag in. Called once per frame from the shell with
    /// the same `edited` the engine snapshot is keyed off, so there is exactly
    /// one change-tracking path in the app rather than two that can disagree.
    pub fn mark_edited(&mut self, edited: bool) {
        self.dirty |= edited;
        // The same flag drives the crash copy's clock, so there is still exactly
        // one change-tracking path in the app rather than a second one that
        // could think the session was quiet while it was not.
        if edited {
            self.cadence.note_edit(std::time::Instant::now());
        }
    }

    /// Auto-connect changed the desk: unsaved work, but not work to recover.
    ///
    /// The one caller is the shell, and the whole of the difference between this
    /// and [`SessionPanel::mark_edited`] is the crash copy's clock. Binding a box
    /// that was plugged in really does change the session, so the dirty flag and
    /// the close guard treat it exactly as they always have; but the next launch
    /// will make the identical change again from the identical cable, so there is
    /// nothing in it worth handing back through a modal. See the call site for
    /// what happens if the two are not told apart.
    pub fn mark_reconnected(&mut self) {
        self.dirty = true;
    }

    // --- the crash copy -------------------------------------------------------

    /// Take a crash copy if one is due, and say whether one was taken.
    ///
    /// Called every frame from the shell. Nearly every call does nothing: an
    /// app with no unsaved work has no clock running, and an app in the middle
    /// of a drag is inside [`recovery::QUIET`]. The write itself is a pretty-
    /// printed session — a `Session` clone is a handful of pointer bumps, per
    /// the `Arc`s `digi_core`'s manifest explains — so at one write per two
    /// seconds of editing this stays off the frame budget.
    ///
    /// [`recovery::QUIET`]: super::recovery::QUIET
    pub fn autosave(&mut self, session: &Session) -> bool {
        if !self.dirty || !self.cadence.due(std::time::Instant::now()) {
            return false;
        }
        self.autosave_now(session)
    }

    /// Take a crash copy regardless of the clock.
    ///
    /// The decision half of [`SessionPanel::autosave`] with the timing taken
    /// out, which is what makes the wiring — what gets written, when the copy is
    /// dropped again — testable without a two-second sleep in the suite.
    ///
    /// A failure is deliberately silent. This is the one write in the app the
    /// user did not ask for, and a modal about a directory they have never
    /// heard of, raised in the middle of their editing, would be the feature
    /// making itself the problem. What it must never do is claim success: the
    /// return value is the truth, and the dirty flag is untouched either way, so
    /// the close guard still stands between the work and the door.
    pub fn autosave_now(&mut self, session: &Session) -> bool {
        let Some(store) = &self.recovery else {
            return false;
        };
        let taken = store.write(session, self.path.as_deref()).is_ok();
        if taken {
            self.cadence.note_write();
        }
        taken
    }

    /// Whether anything is waiting to be copied, for the shell's repaint.
    pub fn autosave_pending(&self) -> bool {
        self.dirty && self.cadence.pending()
    }

    /// Drop the crash copy. Every path that ends the unsaved work calls this.
    ///
    /// Silent on failure for the same reason [`SessionPanel::autosave_now`] is,
    /// and with a smaller consequence: a copy that could not be removed is
    /// offered once on the next launch, next to a session that is already saved.
    fn clear_recovery(&mut self) {
        if let Some(store) = &self.recovery {
            let _ = store.clear();
        }
        self.cadence.note_write();
    }

    /// Look for a copy left by a previous run, once, and return a line for the
    /// console if there is something to say.
    ///
    /// Called every frame from the shell and guarded by `looked`, because the
    /// shell has no other "first frame" hook to hang it on and a second look
    /// would re-raise a modal the user had just answered.
    ///
    /// A copy that is *there and unreadable* is the case worth the console line.
    /// The offer itself needs no announcement — it is a modal.
    pub fn look_for_recovery(&mut self) -> Option<String> {
        if self.looked {
            return None;
        }
        self.looked = true;
        match self.recovery.as_ref()?.find()? {
            Ok(found) => {
                self.offer = Some(found);
                None
            }
            Err(why) => Some(format!("Recovery copy could not be read — {why}")),
        }
    }

    /// The copy waiting to be offered, if there is one.
    pub fn recovery_offer(&self) -> Option<&Found> {
        self.offer.as_ref()
    }

    /// Accept the offer: put the recovered session on screen.
    ///
    /// Everything the file remembered about ports is re-pointed at the desk as
    /// it is *now*, exactly as [`SessionPanel::open_from`] does, and for the
    /// same reason — the copy was taken on a desk that may since have been
    /// unplugged.
    ///
    /// **The result is dirty on purpose.** Recovered work has still never
    /// reached the file it belongs to, so it is unsaved work and the close guard
    /// has to keep standing in front of it. The copy is left on disk for the
    /// same reason: it stops being the last surviving version at the moment a
    /// real save happens, and not before.
    ///
    /// The remembered origin is adopted as this session's path where there was
    /// one, so the first `Cmd+S` after a recovery goes where it would have gone
    /// before the crash instead of opening a dialog.
    pub fn recover(
        &mut self,
        session: &mut Session,
        available_in: &[PortRef],
        available_out: &[PortRef],
    ) -> bool {
        let Some(found) = self.offer.take() else {
            return false;
        };
        match Project::from_json_with_ports(&found.json, available_in, available_out) {
            Ok((project, unbound)) => {
                let loaded = project.session;
                self.lost_ports = names_of(&loaded, &unbound);
                *session = loaded;
                self.path = found.origin.clone();
                self.dirty = true;
                self.status = Some(Status::Recovered(found.origin));
                true
            }
            Err(e) => {
                // `look_for_recovery` already parsed this text once, so reaching
                // here means the file changed underneath a running app. The copy
                // stays on disk: it is still the only version of that work.
                self.status = Some(Status::Failed(describe(&e)));
                false
            }
        }
    }

    /// Decline the offer, and drop the copy.
    ///
    /// The only place in this file that throws work away without the session in
    /// hand being involved, so it asks in the modal rather than here.
    pub fn discard_recovery(&mut self) {
        self.offer = None;
        self.clear_recovery();
    }

    // --- the decisions, all reachable without a window ------------------------

    /// Save to the known path, or ask for one if there is none.
    ///
    /// Returns whether bytes reached the disk. A cancelled dialog is `false` and
    /// leaves the dirty flag alone.
    pub fn save(&mut self, session: &Session) -> bool {
        match self.path.clone() {
            Some(path) => self.save_to(&path, session),
            None => self.save_as(session),
        }
    }

    /// `Cmd+S` (`Ctrl+S` on Windows): save, from anywhere in the window.
    ///
    /// Returns whether the key was taken. Read from the shell rather than from
    /// this panel's `ui` for the reason the close guard is: the panel can be
    /// closed, and the whole point of a save key is that you do not have to open
    /// anything to use it. It is [`SessionPanel::save`] and nothing else — the
    /// known path, or the dialog if there is none — so the key and the Save
    /// button cannot come to mean two different things.
    ///
    /// **`Cmd+Shift+S` is deliberately left unbound.** `Modifiers::COMMAND` as a
    /// `consume_key` pattern would swallow it (`matches_logically` ignores a
    /// shift the pattern does not ask for), and a Save As reflex quietly doing a
    /// plain Save is the wrong kind of surprise — so the match here is
    /// `matches_exact` and that chord stays free for Save As to grow into.
    /// `Cmd+S` itself reaches egui intact: `egui-winit` intercepts only the
    /// cut/copy/paste chords (`lib.rs` ~1022, and see `ui::tracks` for what that
    /// cost the first time), and a command chord carries no `Event::Text` twin
    /// to clean up after.
    ///
    /// Guarded on focus and on modals like every other shortcut in this app. The
    /// modal guard matters more here than most: a close guard asking "save
    /// first?" is already a question about saving, and answering it with a key
    /// that runs a *second* save underneath it is two saves and one answer.
    ///
    /// It says what happened in the console, because with this panel closed
    /// there is nowhere else it could: the LAST line is in the panel you did not
    /// open. A cancelled dialog says nothing — cancelling is a normal answer.
    pub fn save_shortcut(&mut self, ctx: &egui::Context, session: &Session) -> bool {
        if crate::ui::tracks::typing_elsewhere(ctx, session)
            || ctx.memory(|m| m.top_modal_layer().is_some())
        {
            return false;
        }
        let pressed = ctx.input_mut(|i| {
            let mut pressed = false;
            i.events.retain(|event| match event {
                egui::Event::Key { key: egui::Key::S, pressed: true, repeat, modifiers, .. }
                    if modifiers.matches_exact(egui::Modifiers::COMMAND) =>
                {
                    pressed |= !repeat;
                    false
                }
                _ => true,
            });
            pressed
        });
        if !pressed {
            return false;
        }

        // Whether a `false` was a refusal or a cancelled dialog. A save onto a
        // path this panel already knows can only have been a refusal; without
        // one, a status that moved is the tell. (Two identical failures in a row
        // onto a path chosen twice from the dialog would leave the second silent
        // here — the panel's LAST line still carries it, and the dirty flag is
        // still up.)
        let had_path = self.path.is_some();
        let before = self.status.clone();
        let saved = self.save(session);
        match &self.status {
            Some(Status::Saved(path)) if saved => {
                crate::ui::console::post(ctx, format!("Saved to {}", path.display()));
            }
            Some(Status::Failed(why)) if !saved && (had_path || self.status != before) => {
                crate::ui::console::post(ctx, format!("Not saved — {why}"));
            }
            _ => {}
        }
        true
    }

    /// Ask for a path, then save to it.
    pub fn save_as(&mut self, session: &Session) -> bool {
        let suggested = suggested_name(session);
        let Some(path) = self.chooser.save_as(&suggested) else {
            return false;
        };
        self.save_to(&path, session)
    }

    /// Write the session to `path`, and on success adopt it as this session's
    /// file and clear the dirty flag.
    pub fn save_to(&mut self, path: &Path, session: &Session) -> bool {
        // Pretty rather than compact: a project file a person can read in a text
        // editor is a project file a person can salvage.
        let json = match Project::new(session.clone()).to_json_pretty() {
            Ok(json) => json,
            Err(e) => {
                self.status = Some(Status::Failed(describe(&e)));
                return false;
            }
        };
        match std::fs::write(path, json) {
            Ok(()) => {
                self.path = Some(path.to_path_buf());
                self.dirty = false;
                self.status = Some(Status::Saved(path.to_path_buf()));
                // The work is on disk where the user put it, so the shadow copy
                // has nothing left to protect. Dropping it here rather than
                // waiting for exit is what stops a clean quit from leaving an
                // offer behind for the next launch.
                self.clear_recovery();
                true
            }
            Err(e) => {
                self.status = Some(Status::Failed(format!(
                    "could not write {}: {e}",
                    path.display()
                )));
                false
            }
        }
    }

    /// Ask for a file, then open it over `session`.
    pub fn open(
        &mut self,
        session: &mut Session,
        available_in: &[PortRef],
        available_out: &[PortRef],
    ) -> bool {
        let Some(path) = self.chooser.open() else {
            return false;
        };
        self.open_from(&path, session, available_in, available_out)
    }

    /// Read `path` and, only if it parses and validates, replace `session`.
    ///
    /// **The session in hand is not touched until the file has been proved
    /// good.** A half-applied load would leave the app holding neither the
    /// session that was on screen nor the one on disk.
    pub fn open_from(
        &mut self,
        path: &Path,
        session: &mut Session,
        available_in: &[PortRef],
        available_out: &[PortRef],
    ) -> bool {
        let json = match std::fs::read_to_string(path) {
            Ok(json) => json,
            Err(e) => {
                self.status = Some(Status::Failed(format!(
                    "could not read {}: {e}",
                    path.display()
                )));
                return false;
            }
        };
        match Project::from_json_with_ports(&json, available_in, available_out) {
            Ok((project, unbound)) => {
                let loaded = project.session;
                self.lost_ports = names_of(&loaded, &unbound);
                *session = loaded;
                self.path = Some(path.to_path_buf());
                // Freshly off disk, so it is not unsaved work — and so there is
                // nothing for a crash copy to hold either. Whatever it held a
                // moment ago belonged to the session this one just replaced,
                // which the close guard has already asked about.
                self.dirty = false;
                self.clear_recovery();
                self.status = Some(Status::Opened(path.to_path_buf()));
                true
            }
            Err(e) => {
                self.status = Some(Status::Failed(describe(&e)));
                false
            }
        }
    }

    // --- the close guard ------------------------------------------------------

    /// Answer a close request. `true` means let the window go.
    ///
    /// Called every frame the shell sees `close_requested()`. The `Confirmed`
    /// arm is what stops the guard asking again about a close the user has
    /// already agreed to.
    pub fn allow_close(&mut self) -> bool {
        match self.guard {
            CloseGuard::Confirmed => true,
            _ if !self.dirty => {
                // A clean exit, so there is nothing to recover and the shelf is
                // cleared on the way out. Without this, saving and then quitting
                // would leave the last crash copy to be offered on the next
                // launch, and an offer to recover work that is already on disk
                // is an offer that teaches you to ignore the modal.
                self.clear_recovery();
                true
            }
            // The New modal is already asking the identical question — "keep
            // this unsaved work or not?" — from a different button. Refusing
            // here without touching `self.guard` leaves that modal as the one
            // and only thing on screen; a close tried again after it resolves
            // re-enters this function fresh and asks properly if the work is
            // still unsaved.
            _ if self.new_guard != NewGuard::Idle => false,
            _ => {
                self.guard = CloseGuard::Asking;
                false
            }
        }
    }

    /// Give up the unsaved work and let the next close through.
    ///
    /// The crash copy goes with it. `Discard and close` has to mean discarded —
    /// handing the same work back through a modal on the next launch would turn
    /// the one deliberate way to throw work away into a suggestion.
    pub fn discard_and_close(&mut self) {
        self.guard = CloseGuard::Confirmed;
        self.clear_recovery();
    }

    /// Stay open.
    pub fn cancel_close(&mut self) {
        self.guard = CloseGuard::Idle;
    }

    // --- the New guard ---------------------------------------------------------

    /// Answer a New request: ask first if there is unsaved work, otherwise do
    /// it outright. Returns whether the session was just replaced — the
    /// caller (`ui`) folds that straight into [`Outcome::reloaded`], the same
    /// way [`SessionPanel::open`]'s return value does.
    ///
    /// Refuses even to raise its own modal while the close guard already has
    /// one up — see [`NewGuard`]'s header for why, and [`allow_close`] for the
    /// other half of the same rule.
    ///
    /// [`allow_close`]: SessionPanel::allow_close
    pub fn request_new(&mut self, session: &mut Session) -> bool {
        if self.guard != CloseGuard::Idle {
            return false;
        }
        if self.dirty {
            self.new_guard = NewGuard::Asking;
            false
        } else {
            self.confirm_new(session);
            true
        }
    }

    /// Do the reset: a fresh empty session, and the panel put back to its
    /// just-launched state. Called both for a clean session's immediate
    /// New and for the modal's `Discard` and `Save` exits.
    ///
    /// Empty means *no boxes*, same as launch: auto-connect re-adds whatever is
    /// actually plugged in within a scan, so New on a cabled desk comes back to
    /// the same boxes with their ports — minus any it only imagined having.
    pub fn confirm_new(&mut self, session: &mut Session) {
        *session = Session::default();
        self.path = None;
        self.dirty = false;
        self.lost_ports.clear();
        self.clear_recovery();
        self.status = Some(Status::New);
        self.new_guard = NewGuard::Idle;
    }

    /// `Keep working`: stay on the session in hand, untouched.
    pub fn cancel_new(&mut self) {
        self.new_guard = NewGuard::Idle;
    }

    // --- drawing ---------------------------------------------------------------

    /// The launch-time offer of a crash copy, drawn from the shell.
    ///
    /// In the shell for the same reason the close guard is, and one more: this
    /// modal has to be the first thing on screen, before the user has clicked
    /// anything at all, and the Session panel starts closed.
    ///
    /// Returns whether the session in hand was just replaced, which the shell
    /// folds into the same `reloaded` path an `Open` takes — the engine has to
    /// be rebuilt around a session it has not seen, and the undo history that
    /// belonged to the old one has to go.
    ///
    /// **There is no third button.** `Recover it` and `Discard it` are the whole
    /// question, and neither is destructive of anything on a box. A "decide
    /// later" would have to leave the copy on the shelf and keep offering it,
    /// and an offer you can defer forever is one you stop reading.
    pub fn recovery_ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        available_in: &[PortRef],
        available_out: &[PortRef],
    ) -> bool {
        let Some(found) = &self.offer else {
            return false;
        };
        // Read off before the closure, which needs `&mut self` for the answers.
        let when = found
            .age
            .map(super::console::age_label)
            .unwrap_or_else(|| String::from("at some point"));
        let origin = found.origin.clone();

        let mut recovered = false;
        egui::Modal::new(egui::Id::new("session-recovery-offer")).show(ui.ctx(), |ui| {
            ui.set_max_width(520.0);
            ui.label(
                egui::RichText::new("There is unsaved work from a previous run").strong(),
            );
            ui.separator();
            match &origin {
                Some(path) => ui.label(format!(
                    "A copy was taken {when}, with changes that had not reached {}.",
                    path.display()
                )),
                None => ui.label(format!(
                    "A copy was taken {when}. That session had never been saved to \
                     a file, so this copy is the only version of it there is."
                )),
            };
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Recovering puts it on screen as unsaved work — nothing is \
                     written to your session file until you save. Nothing on a \
                     box changes either way.",
                )
                .weak(),
            );
            ui.separator();
            ui.horizontal(|ui| {
                // The keeping answer first and leftmost. Both guards in this file
                // put the cautious button under a hesitating hand, and here the
                // cautious one is the one that keeps the work.
                if ui.button("Recover it").clicked() {
                    recovered = self.recover(session, available_in, available_out);
                }
                if ui.button("Discard it").clicked() {
                    self.discard_recovery();
                }
            });
        });
        recovered
    }

    /// The close-guard modal, drawn from the shell rather than from the panel
    /// body.
    ///
    /// Same reason `write` and `restore` are ticked there: this panel can be
    /// closed, and a question nobody can be shown is a window that will not shut.
    /// Returns whether the app may now close.
    pub fn guard_ui(&mut self, ui: &mut Ui, session: &Session) -> bool {
        if self.guard != CloseGuard::Asking {
            return false;
        }
        let mut close_now = false;
        egui::Modal::new(egui::Id::new("session-close-guard")).show(ui.ctx(), |ui| {
            ui.set_max_width(480.0);
            ui.label(egui::RichText::new("This session has not been saved").strong());
            ui.separator();
            match &self.path {
                Some(path) => ui.label(format!(
                    "There are changes since it was last saved to {}.",
                    path.display()
                )),
                None => ui.label(
                    "It has never been saved, so closing now loses every pattern, \
                     scene and port binding in it.",
                ),
            };
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Nothing on a box changes either way — a session file is not a backup.",
                )
                .weak(),
            );
            ui.separator();
            ui.horizontal(|ui| {
                // Cancel first and leftmost, as the write dialog does: it is the
                // answer a hesitating hand should land on.
                if ui.button("Keep working").clicked() {
                    self.guard = CloseGuard::Idle;
                }
                if ui.button("Discard and close").clicked() {
                    self.discard_and_close();
                    close_now = true;
                }
                if ui.button("Save and close").clicked() && self.save(session) {
                    self.guard = CloseGuard::Confirmed;
                    close_now = true;
                }
            });
        });
        close_now
    }

    /// The New modal, drawn from inside [`ui`](SessionPanel::ui) rather than
    /// from the shell the way [`guard_ui`](SessionPanel::guard_ui) is.
    ///
    /// The close guard has to live in the shell because a window close can
    /// arrive while the Session panel is not even showing; a `New` request
    /// cannot arrive except by way of the button drawn a few lines above this
    /// call, in the same frame, so there is no seam here to hand to
    /// `main.rs` in the first place. Returns whether the session was just
    /// replaced, which `ui` folds into `Outcome::reloaded`.
    fn new_guard_ui(&mut self, ui: &mut Ui, session: &mut Session) -> bool {
        if self.new_guard != NewGuard::Asking {
            return false;
        }
        let mut did_reset = false;
        egui::Modal::new(egui::Id::new("session-new-guard")).show(ui.ctx(), |ui| {
            ui.set_max_width(480.0);
            ui.label(egui::RichText::new("This session has not been saved").strong());
            ui.separator();
            match &self.path {
                Some(path) => ui.label(format!(
                    "There are changes since it was last saved to {}.",
                    path.display()
                )),
                None => ui.label(
                    "It has never been saved, so starting a new one loses every \
                     pattern, scene and port binding in it.",
                ),
            };
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Nothing on a box changes either way — a session file is not a backup.",
                )
                .weak(),
            );
            ui.separator();
            ui.horizontal(|ui| {
                // Same order as the close guard's modal, for the same reason:
                // cancel first and leftmost.
                if ui.button("Keep working").clicked() {
                    self.cancel_new();
                }
                if ui.button("Discard").clicked() {
                    self.confirm_new(session);
                    did_reset = true;
                }
                if ui.button("Save").clicked() && self.save(session) {
                    self.confirm_new(session);
                    did_reset = true;
                }
            });
        });
        did_reset
    }

    /// The panel body, drawn when the rail's Session tool is showing.
    ///
    /// A styling pass over what this used to be, per
    /// `design_handoff_digi_roll_ui_v2/README.md`'s Session bullet: the four
    /// paragraphs of reference prose move behind the title bar's `?`, and what
    /// stays visible is a FILE section — Save / Save As… / Open…, the current
    /// path, and one LAST line folding in the dirty flag and the last save or
    /// open's result. None of the logic above this method changed; a save, an
    /// open and the close guard all decide exactly what they decided before.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        available_in: &[PortRef],
        available_out: &[PortRef],
    ) -> Outcome {
        // The title bar's context string: the current file's stem, or nothing
        // for a session that has never been saved. Short on purpose — the path
        // itself is still spelled out in full in the FILE section below, this
        // is only "which file, at a glance".
        let context: String = self
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let mut outcome = Outcome {
            close: super::panel_title_bar(ui, "Session", &context, &mut self.reference_visible),
            reloaded: false,
        };

        egui::ScrollArea::vertical()
            .id_salt("session-panel")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                super::section_header(ui, "FILE", None);
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Save and open a session: the boxes, their patterns and \
                         p-locks, the scenes, the tempo, and which box is on which \
                         port.",
                    )
                    .size(11.0)
                    .color(super::TEXT_DIM),
                );
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if ui
                        .button("Save")
                        .on_hover_text(
                            "Write to the current file, or ask for one — or press Cmd+S, \
                             which works with this panel closed",
                        )
                        .clicked()
                    {
                        self.save(session);
                    }
                    if ui.button("Save As…").clicked() {
                        self.save_as(session);
                    }
                    if ui
                        .button("Open…")
                        .on_hover_text("Replace everything in this window")
                        .clicked()
                        && self.open(session, available_in, available_out)
                    {
                        outcome.reloaded = true;
                    }
                    if ui
                        .button("New")
                        .on_hover_text(
                            "Start a fresh session — a DT2 and a DN2, 16 tracks \
                             each, no ports bound",
                        )
                        .clicked()
                        && self.request_new(session)
                    {
                        outcome.reloaded = true;
                    }
                });

                if self.new_guard_ui(ui, session) {
                    outcome.reloaded = true;
                }

                ui.add_space(6.0);
                match &self.path {
                    Some(path) => ui.label(
                        egui::RichText::new(path.display().to_string())
                            .size(11.0)
                            .color(super::TEXT_SECONDARY),
                    ),
                    None => ui.label(
                        egui::RichText::new("Not saved to a file yet")
                            .size(11.0)
                            .color(super::TEXT_DIM),
                    ),
                };

                // The ports warning outlives a single status line on purpose: it
                // is a standing condition of the loaded session, not an event —
                // which is why it stays outside the `?` fold along with the FILE
                // section: this is actionable state, not reference prose.
                if !self.lost_ports.is_empty() {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("PORTS NOT FOUND").size(9.5).color(super::WARN_AMBER),
                    );
                    ui.add_space(3.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} kept its patterns but lost its routing — set it \
                             again in Setup, or nothing will reach it.",
                            self.lost_ports.join(", ")
                        ))
                        .size(10.5)
                        .color(super::WARN_AMBER_BODY),
                    );
                }

                // The LAST line: whatever is true right now about unsaved work,
                // folded together with the outcome of the last save or open.
                // Dirty is an ordinary consequence — editing is what a session is
                // for — so it takes the plain `consequence_line` treatment; a
                // failure is the one case here that gets the amber, ruled
                // treatment `destructive_note` otherwise reserves for copy that
                // changes or destroys data, because a save that did not happen is
                // exactly the kind of thing a closed window then loses for good.
                if self.dirty || self.status.is_some() {
                    ui.add_space(10.0);
                    super::caption(ui, "LAST");
                    if self.dirty {
                        super::consequence_line(ui, "Unsaved changes since the last save.");
                    }
                    match &self.status {
                        Some(Status::Saved(path)) => {
                            super::consequence_line(ui, &format!("Saved to {}", path.display()));
                        }
                        Some(Status::Opened(path)) => {
                            super::consequence_line(ui, &format!("Opened {}", path.display()));
                        }
                        Some(Status::New) => {
                            super::consequence_line(ui, "Started a new session.");
                        }
                        Some(Status::Recovered(origin)) => {
                            super::consequence_line(
                                ui,
                                &match origin {
                                    Some(path) => format!(
                                        "Recovered unsaved work on {}. Save to keep it.",
                                        path.display()
                                    ),
                                    None => String::from(
                                        "Recovered unsaved work. It has never been \
                                         saved to a file — save it to keep it.",
                                    ),
                                },
                            );
                        }
                        Some(Status::Failed(why)) => {
                            super::destructive_note(ui, "LAST ATTEMPT FAILED", why);
                        }
                        None => {}
                    }
                }

                // **The crash copy is stated, not silent.** It is the one thing
                // in this app that writes without being asked, and a safety net
                // nobody can see is a safety net nobody trusts — on the day it
                // matters you need to already know it was running and where it
                // put things. Outside the `?` fold for the reason the ports
                // warning is: this is state, not reference prose.
                ui.add_space(10.0);
                super::caption(ui, "CRASH COPY");
                match &self.recovery {
                    Some(store) => {
                        super::consequence_line(
                            ui,
                            "Unsaved work is copied aside every few seconds and \
                             offered back if this app stops unexpectedly. Your \
                             session file is never written without a Save.",
                        );
                        ui.label(
                            egui::RichText::new(store.dir().display().to_string())
                                .size(11.0)
                                .color(super::TEXT_SECONDARY),
                        );
                    }
                    // A machine with no home directory. Amber, because this is a
                    // safety rule that is off rather than a preference.
                    None => super::destructive_note(
                        ui,
                        "NO CRASH COPY",
                        "This machine has no application-data directory to put one \
                         in, so unsaved work is not being copied anywhere. Save often.",
                    ),
                }

                if self.reference_visible {
                    ui.add_space(12.0);
                    super::caption(ui, "WHAT THIS IS NOT");
                    ui.label(
                        "Not a backup. A session file holds no samples, no kit and \
                         no sounds, and opening one puts nothing on a box. The \
                         backups that a write and a restore take for you are raw \
                         dumps off the box itself, and they are in Setup under \
                         BACKUPS.",
                    );
                    ui.add_space(10.0);

                    super::caption(ui, "PATTERNS");
                    ui.label("Copying a pattern between slots or between boxes.");
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(
                            "Not built. `protocol::copy_track` is written and has no \
                             caller — PLAN.md Phase 6.",
                        )
                        .weak()
                        .italics(),
                    );
                }
            });

        outcome
    }
}

/// The filename to offer for a session that has never been saved.
///
/// Off the session's own name, so a renamed session suggests its new name rather
/// than the one it was born with.
fn suggested_name(session: &Session) -> String {
    let stem: String = session
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let stem = stem.trim_matches('-');
    if stem.is_empty() {
        "session.json".into()
    } else {
        format!("{stem}.json")
    }
}

/// Device ids into names a person can look for on the desk.
fn names_of(session: &Session, ids: &[DeviceId]) -> Vec<String> {
    ids.iter()
        .map(|id| match session.device(*id) {
            Some(d) => d.name.clone(),
            // Cannot happen — the ids came out of this session — but naming the
            // id beats dropping the warning on the floor.
            None => format!("device {}", id.0),
        })
        .collect()
}

/// A `ProjectError` worded so a person knows what to do next.
///
/// `ProjectError`'s own `Display` says what is wrong, which is what the terminal
/// wants. This adds the half a window needs: the move that gets you out of it.
fn describe(e: &ProjectError) -> String {
    match e {
        ProjectError::FromTheFuture { found, supported } => format!(
            "This file was written by a newer build of Digi Roll Studio \
             (format {found}; this build reads up to {supported}). Update this \
             build, or save the file again from the one that wrote it."
        ),
        ProjectError::Model(_) => {
            format!("{e}. It was probably edited by hand, or written by a build whose \
                     device table differs from this one's.")
        }
        ProjectError::Json(_) => format!("{e}"),
    }
}

/// Copy one stored backup to a path the user chooses.
///
/// The user-initiated half of what a write does for you automatically. A free
/// function rather than a method because the Backups list owns a [`Chooser`] of
/// its own and this is the whole of the logic between the two: `Stash::export`
/// is a file copy, and the part worth having in one place is that **a cancelled
/// dialog is not a failure** — the outer `None` — so nothing is reported and
/// nothing is copied.
///
/// `Stash::export` had no caller from the day it was written until this.
pub fn export_backup(
    chooser: &mut dyn Chooser,
    stash: &digi_protocol::backup_stash::Stash,
    file: &str,
) -> Option<Result<PathBuf, String>> {
    let dest = chooser.export_as(file)?;
    Some(
        stash
            .export(file, &dest)
            .map(|_| dest)
            .map_err(|e| format!("could not export {file}: {e}")),
    )
}
