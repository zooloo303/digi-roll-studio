// The Edit panel: the rail's first slot, and the one that had been a labelled
// empty box since the rail shipped.
//
// This is `js/main.js`'s Edit aside, and PLAN.md §9's four groups in its order:
//
//   NOTES     velocity, length, and the track's PROB default
//   PATTERN   swing, duplicate bar, clear
//   P-LOCKS   add a lane, and the lane list
//   MIDI FILES import and export a Standard MIDI File
//   HISTORY   undo and redo
//
// **The headline is velocity.** `Note.velocity` has been in the model since Phase
// 2, is carried through `core::export`, and has been written to real hardware —
// and until this file existed nothing in the app could set it. Every note this
// app ever sent to a box went out at 100 because that was the literal in the
// roll's `Note::new` call.
//
// ## The three controls that are not what they look like
//
// **Velocity is a control *and* a readout.** It sets `PianoRoll`'s default for new
// notes and, when there is a selection, levels every selected note onto it — and
// it moves on its own when your hand touches a note in the roll, because
// `js/main.js`'s `onSelect` writes `state.defaultVelocity = note.velocity`. So the
// number under the pointer and the number in the panel are always the same number.
// It lives on the roll, not here; see `PianoRoll::default_velocity`.
//
// **Length has no default.** The slider edits the selection and sets nothing for
// new notes, which is deliberate and `js/main.js` says why: "all the same length"
// is a deliberate act, while drawing wants the plain one-step note it always got.
// With nothing selected it is a readout of the last note in the selection and
// otherwise parks on one step.
//
// **The slider position *is* the length byte.** Every stop it can reach is a
// length the hardware can store, so a number shown here cannot round on write —
// which is the same guarantee the roll's fine resize gives, reached the same way.
//
// ## Why the PROB default is in NOTES
//
// It is per *pattern*, not per selection, so it does not belong in a group about
// the notes you have picked. `js/main.js` puts it there anyway because the box
// keeps velocity, length and PROB together on one page — and PLAN.md §9 says to
// say so in the panel rather than leave it reading as a bug. The caption does.
//
// ## What the MIDI FILES group has to admit
//
// **Trig conditions do not survive a MIDI file.** The format has no PROB, no FILL
// and no COND, so an export drops every one and an import cannot invent any. The
// export button says so *before* it is pressed, per §9, rather than leaving it to
// be discovered on the far side. The p-lock lanes go the same way, and an import
// clears them along with the pattern's provenance: locks ride on trigs, so
// automation left over from music that has been replaced would be riding on notes
// that no longer exist.
//
// ## What is drawn, and what only exists
//
// `ui::tools`' rule — anything shipped gets its state line rewritten in the change
// that ships it. Everything in this file is operable. There are no descriptions of
// features here, because there are no features left in this panel to describe.
//
// ## The 2026-08-19 side-panel pass
//
// `design_handoff_digi_roll_ui_v2/README.md`'s six panel rules land here as:
// [`super::panel_title_bar`] carries the slot·track context the body used to
// restate; the eighteen-line "IN THE ROLL" reference moves entirely behind the
// title bar's `?`, which is the same flag the new KEYS & GESTURES row at the
// bottom opens and closes (one flag, not two, since both reveal the same text —
// see [`EditPanel::reference_visible`]); HISTORY becomes a disclosure row to
// match; Velocity, Length, Prob and Swing move onto the shared
// [`super::slider_row`]; a p-lock lane's chip now shares
// [`crate::ui::plocklane::lane_color`] with the bar graph under the roll instead
// of inventing its own; the add-lane dropdown-plus-button pair collapses into one
// dashed affordance; and MIDI FILE's Import takes the amber destructive
// treatment with a [`super::destructive_note`] under it. None of the decisions
// above change what a control *does* — see each function's own doc comment for
// what, if anything, was judgment rather than direct translation.

use std::path::Path;

use digi_core::device::DeviceId;
use digi_core::edit_ops::{
    clear_track, duplicate_last_bar, set_selection_length, LenEntry, ResizeOpts, VEL_MAX, VEL_MIN,
};
use digi_core::history::History;
use digi_core::lengths::snap_len_fine;
use digi_core::midifile::{midi_file_name, midi_file_to_notes, track_to_midi_file};
use digi_core::model::PLockLane;
use digi_core::{Session, Track};
use digi_protocol::pattern::{length_byte_to_steps, steps_to_length_byte};
use digi_protocol::params::writable_params_for;
use eframe::egui::{self, Color32, Ui};

use crate::ui::pianoroll::{PianoRoll, ZOOM_MAX, ZOOM_MIN};
use crate::ui::plocklane::{self, describe_lane};
use crate::ui::session::{Chooser, NativeChooser};
use crate::ui::tracks::Selection;

/// The highest byte on the boxes' LEN scale that is a real length. 127 is
/// `INFINITY` — the box's `INF`, a note that holds until something stops it —
/// which this app does not model, so the slider stops one short of it rather than
/// offering a value the model cannot carry.
const LEN_BYTE_MAX: u8 = 126;

/// A read-only lane's chip colour, matching the grey `PLockStrip::paint` gives a
/// read-only lane's bars under the roll. Not `pub` in `plocklane.rs`, so kept
/// here as the same literal rather than reaching into that module for a constant
/// it does not export — the two are visually locked together by eye, the way the
/// editable chip colours are locked together by [`plocklane::lane_color`].
const READ_ONLY_LANE_CHIP: Color32 = Color32::from_gray(70);

/// MIDI FILE's [`destructive_note`](super::destructive_note) body — a `const`
/// rather than an inline literal so the text lives in exactly one place, per
/// the 2026-08-20 packet brief: "the wording is Neil's decision, not yours...
/// do not scatter the string." `midi_group` draws it; the test
/// `the_destructive_note_admits_the_first_track_limit` pins it to the claims
/// it has to keep making. Three sentences, in this order:
///
/// 1. **Already there before this packet.** Import replaces the track's
///    notes, its p-lock lanes and its provenance — the destructive warning,
///    and why the button is amber.
/// 2. **Already there before this packet.** A MIDI file has no PROB, no FILL
///    and no COND, so trig conditions do not survive either direction; swing
///    and micro-timing are baked into the note positions.
/// 3. **New, and the reason this packet exists.** Import reads only the
///    first note-bearing track and cannot offset it, so a multi-track DAW
///    file will likely bring in the wrong part, or nothing. A track chooser
///    and a from-bar control are coming; round-tripping a file this app
///    exported works today. The measured numbers behind this claim are
///    `PLAN.md`'s Parked entry "MIDI import against a file this app did not
///    write" — a ten-track, 384 PPQN file whose first note-bearing track
///    (bass, entering at step 139) imported as nothing, because the box only
///    holds 128 steps and there is no offset control to move it into range.
const MIDI_IMPORT_WARNING: &str = "Import replaces this track's notes, its p-lock lanes and its \
     provenance. A MIDI file has no PROB, no FILL and no COND, so trig conditions do not \
     survive either direction, and swing and micro-timing are baked into the note positions. \
     Import currently reads only the first note-bearing track in the file and cannot offset \
     it, so a multi-track file from a DAW will likely bring in the wrong part, or nothing — a \
     track chooser and a from-bar control are coming, and round-tripping a file this app \
     exported works today.";

/// What one frame of the panel did.
#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    pub close: bool,
    /// An ordinary edit: the shell opens a history step around it.
    pub edited: bool,
    /// **The history itself moved.** The session changed and the engine needs a
    /// snapshot, but the shell must *not* record a step for it — folding this into
    /// `edited` would push the post-undo state onto the stack and make the undo
    /// button alternate between two states forever. Same shape, and the same
    /// reason, as `session::Outcome::reloaded`.
    pub stepped: bool,
}

/// The last thing the MIDI FILES group did, shown until the next thing does.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Exported { path: std::path::PathBuf, notes: usize },
    Imported { name: String, notes: usize, dropped: usize },
    /// Already worded for a person.
    Failed(String),
}

pub struct EditPanel {
    chooser: Box<dyn Chooser>,
    status: Option<Status>,
    /// The clear button's confirmation. A clear is undoable now, so this is not a
    /// safety rail so much as a stop on the reflex — it empties a whole track from
    /// a button that sits next to `Duplicate bar`.
    confirm_clear: bool,
    /// Whether the add-lane picker is expanded in place of the dashed
    /// affordance. Replaces the old `pick: Option<&'static str>`: the 2b redesign
    /// folds "choose a parameter" and "add it" into the one click that picks a
    /// row, so there is nothing left to remember *between* two clicks — only
    /// whether the list is currently showing.
    adding_lane: bool,
    /// Whether the panel's reference text is showing. **Shared by two
    /// affordances rather than one each**: the title bar's `?`
    /// ([`super::panel_title_bar`]) and the KEYS & GESTURES disclosure row at
    /// the bottom both reveal the exact same "IN THE ROLL" text in the exact
    /// same place, so two independent flags could only ever agree by
    /// coincidence or show two panels disagreeing about their own state.
    reference_visible: bool,
    /// HISTORY's own disclosure state, closed by default — Undo and Redo still
    /// work from the keyboard via [`shortcuts`] whether this row is open or not.
    history_open: bool,
    /// MIDI FILE's own disclosure state, closed by default per Neil's
    /// 2026-08-20 decision (`PLAN.md`'s five decisions, item 3). Import stays
    /// enabled either way — collapsing the box hides the buttons behind a
    /// click, it does not switch anything off.
    midi_open: bool,
}

impl Default for EditPanel {
    fn default() -> Self {
        Self::with_chooser(Box::new(NativeChooser))
    }
}

impl EditPanel {
    pub fn with_chooser(chooser: Box<dyn Chooser>) -> Self {
        Self {
            chooser,
            status: None,
            confirm_clear: false,
            adding_lane: false,
            reference_visible: false,
            history_open: false,
            midi_open: false,
        }
    }

    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    // --- the decisions, all reachable without a window ------------------------

    /// Write the selected track out as a MIDI file. Returns whether bytes reached
    /// the disk; a cancelled dialog is `false` and is not an error.
    pub fn export_midi(&mut self, session: &Session, selection: Selection) -> bool {
        let Some(ctx) = context(session, selection) else {
            self.status = Some(Status::Failed(String::from("no track is selected")));
            return false;
        };
        let suggested = midi_file_name(&ctx.label);
        let Some(path) = self.chooser.export_midi_as(&suggested) else {
            return false;
        };
        let bytes = track_to_midi_file(ctx.track, &ctx.label, ctx.swing, session.tempo_bpm);
        let notes = ctx.track.notes.len();
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                self.status = Some(Status::Exported { path, notes });
                true
            }
            Err(e) => {
                self.status =
                    Some(Status::Failed(format!("could not write {}: {e}", path.display())));
                false
            }
        }
    }

    /// Ask for a MIDI file and read it over the selected track.
    pub fn import_midi(
        &mut self,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
    ) -> bool {
        let Some(path) = self.chooser.open_midi() else {
            return false;
        };
        self.import_midi_from(&path, session, selection, roll)
    }

    /// Read `path` over the selected track.
    ///
    /// **The track is not touched until the file has parsed**, the same rule
    /// `session::open_from` follows: a half-applied import would leave the slot
    /// holding neither the music that was there nor the music in the file.
    pub fn import_midi_from(
        &mut self,
        path: &Path,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
    ) -> bool {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.status =
                    Some(Status::Failed(format!("could not read {}: {e}", path.display())));
                return false;
            }
        };
        let imported = match midi_file_to_notes(&bytes, digi_core::edit_ops::MAX_STEPS) {
            Ok(imported) => imported,
            Err(e) => {
                self.status = Some(Status::Failed(format!("could not read {name}: {e}")));
                return false;
            }
        };
        if imported.notes.is_empty() {
            // Two different failures reach here and they had one message between
            // them, which is why a ten-track file full of music reported as
            // empty. `dropped` separates them and is already in hand: it counts
            // notes that parsed fine and then fell past the longest track a box
            // can hold, so a non-zero one means the file has music in it and
            // none of it is inside the first 8 bars.
            self.status = Some(Status::Failed(match imported.dropped {
                0 => format!("no notes found in {name}"),
                1 => format!("nothing imported from {name} — its 1 note lands past 8 bars"),
                n => format!("nothing imported from {name} — all {n} notes land past 8 bars"),
            }));
            return false;
        }
        let Some(track) = crate::ui::tracks::track_mut(session, selection) else {
            self.status = Some(Status::Failed(String::from("no track is selected")));
            return false;
        };
        let notes = imported.notes.len();
        track.length_steps = imported.length_steps;
        track.notes = imported.notes;
        // The music was replaced, so the automation that was riding on it goes with
        // it — locks ride on trigs, and lanes left behind would be locked to notes
        // that no longer exist. `js/main.js` clears both for the same reason.
        track.plocks.clear();
        // And so does the provenance: these notes came from a file, not from the
        // box's track, so the pattern can no longer claim to be a copy of a slot.
        clear_source(session, selection);
        // Ids from the music that was there name nothing in the music that is.
        roll.clear_selection();
        self.status = Some(Status::Imported { name, notes, dropped: imported.dropped });
        true
    }

    // --- drawing ---------------------------------------------------------------

    /// Draw the panel over the track `selection` names.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
        history: &mut History,
    ) -> Outcome {
        // Resolved once, per `Context`'s own doc comment, and read twice here:
        // the title bar wants "A01 · T1" before anything else in the panel
        // draws, and the groups below want the track, its swing and its device.
        let resolved = context(session, selection);
        let context_str = resolved
            .as_ref()
            .map(|ctx| format!("{} \u{b7} {}", ctx.pattern_name, ctx.track.name))
            .unwrap_or_default();
        let mut out = Outcome {
            close: super::panel_title_bar(ui, "Edit", &context_str, &mut self.reference_visible),
            edited: false,
            stepped: false,
        };
        let Some(ctx) = resolved else {
            ui.weak("no track selected");
            return out;
        };
        let (track_name, device, swing) = (ctx.track.name.clone(), ctx.device, ctx.swing);
        let selected = roll.selection();

        egui::ScrollArea::vertical()
            .id_salt("edit-panel")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                out.edited |= self.notes_group(ui, session, selection, roll, &selected);
                ui.add_space(10.0);
                // **No `out.edited`, deliberately.** See `view_group`: the zoom is
                // the only control in this panel that does not change the music.
                self.view_group(ui, roll);
                ui.add_space(10.0);
                out.edited |= self.pattern_group(ui, session, selection, roll, &track_name, swing);
                ui.add_space(10.0);
                out.edited |= self.plock_group(ui, session, selection, device);
                ui.add_space(10.0);
                self.midi_group(ui, session, selection, roll, &mut out);
                ui.add_space(10.0);
                out.stepped |= self.history_group(ui, session, roll, history);
                ui.add_space(4.0);
                self.keys_and_gestures_row(ui);
            });
        out
    }

    /// NOTES: velocity, length, and the track's PROB default.
    fn notes_group(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
        selected: &[u32],
    ) -> bool {
        // Per 2b rule 4, the group's own caption carries the selection-state
        // sentence that used to sit under the Velocity slider as a separate
        // weak label — `selection_caption` is the pure half of that, kept
        // outside the `Ui` so it can be tested on its own.
        let caption = selection_caption(selected.len());
        super::section_header(ui, "NOTES", Some(&caption));
        let mut changed = false;

        // --- velocity ---
        let mut velocity_f = f32::from(roll.default_velocity());
        let velocity_hover = if selected.is_empty() {
            "The velocity a note drawn in the roll will get. \
             Shift-drag a note's body to set it by hand."
        } else {
            "Sets every selected note, and the velocity a new one will get. \
             Shift-drag a note's body to nudge a selection without levelling it."
        };
        if tooltip_slider_row(
            ui,
            "Velocity",
            &mut velocity_f,
            f32::from(VEL_MIN)..=f32::from(VEL_MAX),
            |v| format!("{}", v.round() as i32),
            velocity_hover,
        ) {
            let velocity = velocity_f.round().clamp(f32::from(VEL_MIN), f32::from(VEL_MAX)) as u8;
            roll.set_default_velocity(velocity);
            // **Levels, where the drag deltas.** One number on one control can only
            // honestly mean "all of them, this". The group-delta rule belongs to
            // the gesture, which has a direction and a distance to work with.
            if let Some(track) = crate::ui::tracks::track_mut(session, selection) {
                for note in track.notes.iter_mut().filter(|n| selected.contains(&n.id)) {
                    if note.velocity != velocity {
                        note.velocity = velocity;
                        changed = true;
                    }
                }
            }
        }

        // --- length ---
        ui.add_space(6.0);
        let current = length_of(session, selection, selected);
        let mut byte_f = f32::from(steps_to_length_byte(current).min(LEN_BYTE_MAX));
        let length_hover = "In steps, on the boxes' own LEN scale — every stop this cannot \
             round on write. Edits the selection and sets no default for new notes. \
             With nothing selected this is a readout.";
        if tooltip_slider_row(
            ui,
            "Length",
            &mut byte_f,
            0.0..=f32::from(LEN_BYTE_MAX),
            |v| trim(length_byte_to_steps(v.round() as u8)),
            length_hover,
        ) && !selected.is_empty()
        {
            let byte = byte_f.round().clamp(0.0, f32::from(LEN_BYTE_MAX)) as u8;
            let len = length_byte_to_steps(byte);
            if let Some(track) = crate::ui::tracks::track_mut(session, selection) {
                let entries: Vec<LenEntry> = track
                    .notes
                    .iter()
                    .filter(|n| selected.contains(&n.id))
                    .map(LenEntry::from)
                    .collect();
                // Clamped **per note** here, unlike the roll's drag: asking for four
                // steps when one note has two left means that note takes two, rather
                // than holding the whole selection back to the shortest room going.
                let opts = ResizeOpts::fine(
                    f64::from(track.length_steps),
                    snap_len_fine,
                    snap_len_fine(0.0, f64::from(track.length_steps)),
                );
                let lens = set_selection_length(&entries, len, &opts);
                let ids: Vec<u32> = track
                    .notes
                    .iter()
                    .filter(|n| selected.contains(&n.id))
                    .map(|n| n.id)
                    .collect();
                for (id, len) in ids.iter().zip(lens) {
                    if let Some(note) = track.notes.iter_mut().find(|n| n.id == *id) {
                        if note.len != len {
                            note.len = len;
                            changed = true;
                        }
                    }
                }
            }
        }

        // --- the track's PROB default ---
        ui.add_space(6.0);
        if let Some(track) = crate::ui::tracks::track_mut(session, selection) {
            let mut prob_f = f32::from(track.track_prob);
            let prob_hover = "The odds an unlocked trig runs at. Per *pattern*, not per \
                 selection — it is in this group because the box keeps velocity, \
                 length and PROB together on one page. Per-step PROB is in the trig \
                 lane under the roll.";
            if tooltip_slider_row(
                ui,
                "Prob",
                &mut prob_f,
                0.0..=100.0,
                |v| format!("{}%", v.round() as i32),
                prob_hover,
            ) {
                let prob = prob_f.round().clamp(0.0, 100.0) as u8;
                if track.track_prob != prob {
                    track.track_prob = prob;
                    changed = true;
                }
            }
        }

        changed
    }

    /// VIEW: how big the roll draws a step, which is the other half of the
    /// wheel gesture in `pianoroll.rs`.
    ///
    /// **It returns nothing, where every other group in this panel returns
    /// whether it changed something.** A zoom is not an edit: reporting one would
    /// mark the session unsaved, open a history step and re-snapshot the engine
    /// for looking closer at a bar. That is the distinction `Wheel::Aimed` draws
    /// in the roll, and it is the reason this group's signature is the odd one
    /// out rather than an oversight.
    ///
    /// **Why the panel carries it at all**, when cmd+wheel over the grid is the
    /// gesture anyone would reach for first: because `PianoRoll::zoom` sat there
    /// for eleven phases, multiplied into the grid on every frame, with nothing
    /// in the app able to move it — `DEVELOPMENT.md` lesson 7, and lesson 7's
    /// answer is a control, not a comment. A modifier-and-wheel that nothing on
    /// screen names is also the shape lesson 8 warns about: this is where the
    /// number lives, and KEYS & GESTURES below names the gesture.
    ///
    /// **The slider's number is percent, not the multiple the roll stores.** So
    /// typing 200 into the value box means 200%, which is what the box's own
    /// formatter shows — `slider_row`'s doc comment flags exactly this trap, and
    /// PROB solved it the same way. The rounding is on the way *in*, so what is
    /// stored is what is drawn rather than a display rounded off a value that
    /// kept its fraction.
    fn view_group(&mut self, ui: &mut Ui, roll: &mut PianoRoll) {
        super::section_header(ui, "VIEW", None);
        let mut zoom_pct = zoom_percent(roll.zoom());
        let zoom_hover = "How big the roll draws a step. Cmd-scroll or pinch over the grid \
             does the same and holds the cell under the pointer still; this grows it \
             from step 1. Nothing here reaches the box \u{2014} it is what you see, not \
             what plays.";
        if tooltip_slider_row(
            ui,
            "Zoom",
            &mut zoom_pct,
            zoom_range(),
            |v| format!("{}%", v.round() as i32),
            zoom_hover,
        ) {
            roll.set_zoom(zoom_from_percent(zoom_pct));
        }
    }

    /// PATTERN: swing, duplicate bar, clear.
    fn pattern_group(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
        track_name: &str,
        current_swing: u8,
    ) -> bool {
        super::section_header(ui, "PATTERN", None);
        let mut changed = false;

        let mut swing_f = f32::from(current_swing);
        // The thing that matters about swing, kept where `js/main.js` keeps it: it
        // is a per-pattern byte, so it is sent with the pattern and re-times every
        // track in the slot rather than the one you wrote.
        let swing_hover = "50 is straight and 66 is a triplet feel. One byte per pattern on \
             the box, so it is sent with the pattern and re-times all sixteen tracks \
             in the slot — not just the track you wrote.";
        if tooltip_slider_row(
            ui,
            "Swing",
            &mut swing_f,
            50.0..=80.0,
            |v| format!("{}", v.round() as i32),
            swing_hover,
        ) {
            let swing = swing_f.round().clamp(50.0, 80.0) as u8;
            if let Some(pattern) = pattern_mut(session, selection) {
                if pattern.swing != swing {
                    pattern.swing = swing;
                    changed = true;
                }
            }
        }

        ui.add_space(8.0);
        let (length, notes) = crate::ui::tracks::track(session, selection)
            .map(|t| (t.length_steps, t.notes.len()))
            .unwrap_or((0, 0));
        // Two `flex: 1` outline buttons, `gap: 6px` apart, per 2b's Edit spec —
        // the click behaviour underneath is untouched, only the sizing is new.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let width = (ui.available_width() - 6.0) / 2.0;
            let full = length >= digi_core::edit_ops::MAX_STEPS;
            let duplicate = egui::Button::new("Duplicate bar").min_size(egui::vec2(width, 0.0));
            let button = ui.add_enabled(!full, duplicate);
            if button.clicked() {
                if let Some(track) = crate::ui::tracks::track_mut(session, selection) {
                    changed |= duplicate_last_bar(track).is_some();
                }
            }
            button.on_hover_text(if full {
                String::from("Already 8 bars — a track cannot be longer than a box holds.")
            } else {
                format!(
                    "Copy bar {} onto the end, making the track {} bars.",
                    (length / 16).max(1),
                    length / 16 + 1
                )
            });

            let clear_button = egui::Button::new("Clear").min_size(egui::vec2(width, 0.0));
            let clear = ui.add_enabled(notes > 0, clear_button);
            if clear.clicked() {
                self.confirm_clear = true;
            }
            clear.on_hover_text(
                "Empty this track of notes and p-lock lanes. Length, scale, PROB, \
                 channel and port stay as they are.",
            );
        });

        if self.confirm_clear {
            let mut clear_now = false;
            egui::Modal::new(egui::Id::new("edit-clear-guard")).show(ui.ctx(), |ui| {
                ui.set_max_width(420.0);
                ui.label(egui::RichText::new(format!("Clear {track_name}?")).strong());
                ui.separator();
                ui.label(format!(
                    "{notes} note{} and every p-lock lane on this track go. Locks ride \
                     on trigs, so the automation goes with the notes.",
                    if notes == 1 { "" } else { "s" }
                ));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Nothing on a box changes — this empties the track in this \
                         window. Undo brings it back.",
                    )
                    .weak(),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    // Cancel leftmost, as every other dialog in this app has it: it
                    // is the answer a hesitating hand should land on.
                    if ui.button("Keep it").clicked() {
                        self.confirm_clear = false;
                    }
                    if ui.button(format!("Clear {track_name}")).clicked() {
                        clear_now = true;
                    }
                });
            });
            if clear_now {
                self.confirm_clear = false;
                if let Some(track) = crate::ui::tracks::track_mut(session, selection) {
                    changed |= clear_track(track);
                }
                roll.clear_selection();
            }
        }

        changed
    }

    /// P-LOCK LANES: add a lane, and the lane list.
    ///
    /// **The box is not guessed.** `js/main.js` resolves it through four fallbacks
    /// — the pattern's provenance, the connected box's identity, the output port's
    /// name, then both boxes at once — because a browser holding one pattern has no
    /// idea which machine it belongs to. Here the track *is* a track of a device in
    /// the session, so its numbering is known outright. That matters: 74 is
    /// overdrive on a DT2 and filter frequency on a DN2, so a lane authored against
    /// the wrong table is not unlabelled, it is wrong.
    ///
    /// **The add-lane affordance is one click, not two.** The old pair — a
    /// "— pick a parameter —" dropdown plus a separately-enabled "Add lane"
    /// button — is gone; the dashed "+ add lane…" strip below the list opens an
    /// inline row of the available parameters, and picking one adds it on the
    /// spot. `self.adding_lane` only remembers whether that list is open.
    fn plock_group(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        selection: Selection,
        device: DeviceId,
    ) -> bool {
        super::section_header(ui, "P-LOCK LANES", None);
        let mut changed = false;

        // **The box's own key, not its `Spec`'s.** This read `model.spec()` until
        // 2026-09-01, and `spec()` is `None` on the Analog Four and always will be
        // — so an A4 track carrying sixty-one lanes off the box listed none of
        // them and said "this box has no SysEx spec" instead. That sentence was
        // answering a question nobody asked here: *listing* a lane needs only
        // `PLockLane::param`, which resolves against the lane's own recorded
        // `device_kind` and no spec at all. What a spec was standing in for is the
        // **authoring** half, and that is now gated on its own terms below.
        let Some(kind) = session.device(device).map(|d| d.model.key) else {
            return false;
        };

        // **`writable_params_for`, not `auditable_params_for`.** A lane is
        // authored to be *written into a pattern*, so the question is whether the
        // parameter's p-lock slot has been measured — not whether it can be heard
        // over MIDI. The two sets are identical on all three boxes today (eleven
        // on each digi, thirteen on the A4), so the call picks nothing different
        // from what a menu built the other way would offer. It is written this
        // way for the day they diverge again: this menu offering a parameter
        // whose scaling nobody has read off the box would produce a lane the
        // write path then refuses by name, which is the exact trade `params.rs`'s
        // own split at the top of the file exists to prevent. The A4 spent a week
        // in that gap — auditable everywhere, writable nowhere — and came out of
        // it on 2026-09-01 by measurement, not by the menu relaxing.
        let params = writable_params_for(kind);
        let taken: Vec<&'static str> = crate::ui::tracks::track(session, selection)
            .map(|t| t.plocks.iter().filter_map(|l| l.param().name).collect())
            .unwrap_or_default();
        // A parameter already on this track is not offered again: two lanes for one
        // knob would fight over the same step on the way to the box.
        let available: Vec<_> = params.iter().filter(|p| !taken.contains(&p.name)).collect();

        let lanes: Vec<LaneRow> = crate::ui::tracks::track(session, selection)
            .map(|t| t.plocks.iter().enumerate().map(lane_row).collect())
            .unwrap_or_default();

        let mut remove = None;
        for row in &lanes {
            let response = egui::Frame::new()
                .fill(super::INSET_BG)
                .stroke(egui::Stroke::new(1.0, super::PANEL_BORDER))
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        // The chip is the same colour `PLockStrip::paint` gives this
                        // lane's bars under the roll, so the panel list and the
                        // graph read as one object rather than two colour systems
                        // that happen to agree.
                        let (chip, _) =
                            ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                        ui.painter().rect_filled(chip, 0.0, row.colour);
                        ui.label(
                            egui::RichText::new(&row.label)
                                .monospace()
                                .size(11.0)
                                .color(super::TEXT_PRIMARY),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (x_rect, x_response) = ui
                                .allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
                            let x_colour = if x_response.hovered() {
                                super::WARN_AMBER
                            } else {
                                super::TEXT_DIMMER
                            };
                            ui.painter().text(
                                x_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "\u{d7}",
                                egui::FontId::monospace(11.0),
                                x_colour,
                            );
                            if x_response.clicked() {
                                remove = Some(row.index);
                            }
                            x_response.on_hover_text(if row.editable {
                                "Take this lane off the track."
                            } else {
                                "Take this lane off the track. It holds locks this app \
                                 does not edit — removing it loses them."
                            });
                            ui.label(
                                egui::RichText::new(row.steps.to_string())
                                    .monospace()
                                    .size(10.0)
                                    .color(super::TEXT_DIM),
                            );
                        });
                    });
                })
                .response;
            response.on_hover_text(row.summary.clone());
            ui.add_space(4.0);
        }
        if let Some(index) = remove {
            if let Some(track) = crate::ui::tracks::track_mut(session, selection) {
                if index < track.plocks.len() {
                    track.plocks.remove(index);
                    changed = true;
                }
            }
        }

        // **Two different emptinesses, and they say different things.** `params`
        // empty means no parameter on this box can be authored into a lane *at
        // all*; `available` empty means every one that can is already on this
        // track. The A4 is the first box to reach the first state, and telling
        // someone "every parameter already has a lane" in front of an empty list
        // would be a flat contradiction.
        let can_author = !params.is_empty();

        if lanes.is_empty() {
            ui.label(
                egui::RichText::new(if can_author {
                    "No lanes on this track. Add one and its bars appear under the \
                     roll — press in a bar to set a step, drag to paint."
                        .to_string()
                } else {
                    format!(
                        "No lanes on this track. Fetch a pattern off the {kind} and any \
                         p-locks it carries arrive here and under the roll."
                    )
                })
                .weak()
                .small(),
            );
            ui.add_space(4.0);
        }

        if !can_author {
            ui.label(
                egui::RichText::new(format!(
                    "No {kind} parameter has a measured p-lock scaling yet, so a lane off \
                     the box is drawn and sent back exactly as it came — and there is \
                     nothing to author one from here."
                ))
                .weak()
                .small(),
            );
        } else if available.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "Every {kind} parameter this app has mapped already has a lane here."
                ))
                .weak()
                .small(),
            );
        } else if self.adding_lane {
            egui::Frame::new()
                .fill(super::INSET_BG)
                .stroke(egui::Stroke::new(1.0, super::PANEL_BORDER))
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    for param in &available {
                        if ui.selectable_label(false, param.label).clicked() {
                            if let Some(track) = crate::ui::tracks::track_mut(session, selection) {
                                // Empty: a lane arrives with no locks in it and is
                                // filled by dragging on the strip. `PLockLane::new`
                                // refuses a lane with neither a name nor a paramId,
                                // which is why this cannot be constructed nameless.
                                if let Ok(lane) = PLockLane::new(
                                    Some(param.name.to_string()),
                                    None,
                                    Some(kind.to_string()),
                                    false,
                                    Vec::new(),
                                ) {
                                    track.plocks.push(lane);
                                    changed = true;
                                }
                            }
                            self.adding_lane = false;
                        }
                    }
                });
        } else {
            let opened = ui
                .scope(|ui| {
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 22.0),
                        egui::Sense::click(),
                    );
                    paint_dashed_rect(ui.painter(), rect, super::PANEL_BORDER);
                    let colour =
                        if response.hovered() { super::TEXT_PRIMARY } else { super::TEXT_DIMMER };
                    ui.painter().text(
                        rect.left_center() + egui::vec2(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        "+ add lane\u{2026}",
                        egui::FontId::monospace(10.5),
                        colour,
                    );
                    response
                })
                .inner
                .clicked();
            if opened {
                self.adding_lane = true;
            }
        }

        if !lanes.is_empty() {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Lanes travel with the pattern when you send it.")
                    .weak()
                    .small(),
            );
        }

        changed
    }

    /// MIDI FILE: import and export, and the warning that has to come first.
    ///
    /// **Order changed from the pre-2b panel on purpose.** The old body put the
    /// warning above both buttons; the 2b spec puts EXPORT and IMPORT first and
    /// the [`super::destructive_note`] after, directly under the button it
    /// guards — the same "attach to the cause" instinct 2b applies to Generate's
    /// conflicts, just applied to a fixed pair of buttons instead of a list of
    /// cards.
    fn midi_group(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
        out: &mut Outcome,
    ) {
        // Closed by default per Neil's 2026-08-20 decision (`PLAN.md`'s five
        // decisions, item 3): keep Import enabled, collapse the box, and say
        // what import can and cannot do. `midi_open` cannot be borrowed
        // alongside the body closure below — the closure needs `self` for
        // `export_midi`/`import_midi`/`status` — so it is copied out and
        // written back, the same shape `history_group` and
        // `keys_and_gestures_row` would need if their bodies touched `self`
        // too.
        let mut open = self.midi_open;
        super::disclosure_row(ui, &mut open, "MIDI FILE", "import & export", |ui| {
            ui.horizontal(|ui| {
                let notes = crate::ui::tracks::track(session, selection)
                    .map(|t| t.notes.len())
                    .unwrap_or(0);
                if ui
                    .add_enabled(notes > 0, egui::Button::new("Export\u{2026}"))
                    .on_hover_text("Write this track out as a Standard MIDI File")
                    .on_disabled_hover_text("Nothing to export — this track has no notes")
                    .clicked()
                {
                    self.export_midi(session, selection);
                }
                // Amber destructive treatment: this button replaces the track's
                // notes, lanes and provenance, the same weight Setup's SEND buttons
                // carry for writing to a box.
                if super::colored_button(
                    ui,
                    "Import\u{2026}",
                    super::WARN_AMBER_FILL,
                    super::WARN_AMBER_TEXT,
                    super::WARN_AMBER_BORDER,
                    super::WARN_AMBER,
                    super::WARN_AMBER_INK,
                )
                .on_hover_text("Read a MIDI file over this track, replacing what is in it")
                .clicked()
                    && self.import_midi(session, selection, roll)
                {
                    out.edited = true;
                }
            });
            ui.add_space(6.0);
            // The third sentence is the reason this box exists: import is not
            // missing, it is narrow, and a panel that doesn't say so sends
            // someone looking for a feature that isn't the one that's broken —
            // `ui::mod`'s lesson 3, applied to understating rather than
            // overstating. Numbers and shape are `PLAN.md`'s Parked entry "MIDI
            // import against a file this app did not write". Pending Neil's
            // sign-off per the 2026-08-20 packet brief — see this file's test
            // `the_destructive_note_admits_the_first_track_limit` for the
            // words this is pinned to.
            super::destructive_note(ui, "IMPORT REPLACES THE TRACK", MIDI_IMPORT_WARNING);

            if let Some(status) = &self.status {
                ui.add_space(4.0);
                let (text, colour) = match status {
                    Status::Exported { path, notes } => (
                        format!(
                            "Exported {} note{} to {}",
                            notes,
                            if *notes == 1 { "" } else { "s" },
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        ),
                        egui::Color32::from_rgb(0x7a, 0xa8, 0x4a),
                    ),
                    Status::Imported { name, notes, dropped } => (
                        format!(
                            "Imported {notes} note{} from {name}{}",
                            if *notes == 1 { "" } else { "s" },
                            match dropped {
                                0 => String::new(),
                                1 => String::from(" — 1 note landed past 8 bars and was dropped"),
                                n => format!(" — {n} notes landed past 8 bars and were dropped"),
                            }
                        ),
                        if *dropped > 0 {
                            super::CAUTION
                        } else {
                            egui::Color32::from_rgb(0x7a, 0xa8, 0x4a)
                        },
                    ),
                    Status::Failed(why) => (why.clone(), super::CAUTION),
                };
                ui.label(egui::RichText::new(text).color(colour));
            }
        });
        self.midi_open = open;
    }

    /// HISTORY: undo and redo, now a disclosure row like Setup's BACKUPS rather
    /// than a permanently-open group — closed by default, since [`shortcuts`]
    /// gives Undo and Redo to the keyboard whether this row is open or not.
    fn history_group(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        roll: &mut PianoRoll,
        history: &mut History,
    ) -> bool {
        let mut stepped = false;
        let (undo, redo) = history.depth();
        super::disclosure_row(ui, &mut self.history_open, "HISTORY", "undo & redo", |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(history.can_undo(), egui::Button::new("Undo"))
                    .on_hover_text(format!(
                        "{undo} step{} back (Cmd+Z)",
                        if undo == 1 { "" } else { "s" }
                    ))
                    .on_disabled_hover_text("Nothing to undo")
                    .clicked()
                {
                    stepped |= step_back(session, roll, history);
                }
                if ui
                    .add_enabled(history.can_redo(), egui::Button::new("Redo"))
                    .on_hover_text(format!(
                        "{redo} step{} forward (Cmd+Shift+Z)",
                        if redo == 1 { "" } else { "s" }
                    ))
                    .on_disabled_hover_text("Nothing to redo")
                    .clicked()
                {
                    stepped |= step_forward(session, roll, history);
                }
            });
            ui.label(
                egui::RichText::new(
                    "One step is one gesture. The music undoes — notes, lengths, swing, \
                     p-locks; the desk does not — the tempo, the ports and the scenes stay \
                     where you put them.",
                )
                .weak()
                .small(),
            );
        });
        stepped
    }

    /// KEYS & GESTURES: the disclosure row 2b rule 1 asks every panel to carry
    /// at the bottom, its hint the live count of what [`in_the_roll`] lists.
    /// Opens and closes [`Self::reference_visible`] — the same flag the title
    /// bar's `?` uses — rather than a row of its own, per that field's doc
    /// comment.
    fn keys_and_gestures_row(&mut self, ui: &mut Ui) {
        let hint = format!("{} shortcuts", gesture_count());
        super::disclosure_row(ui, &mut self.reference_visible, "KEYS & GESTURES", &hint, |ui| {
            in_the_roll(ui);
        });
    }
}

/// A p-lock lane row's drawing-time facts, resolved once per lane per frame so
/// the row loop reads straight through rather than re-deriving each field
/// inline. Kept next to [`EditPanel::plock_group`] rather than promoted to
/// `plocklane.rs`: everything here is about how the *panel* lists a lane, not
/// how the strip under the roll draws or edits one.
struct LaneRow {
    index: usize,
    editable: bool,
    colour: Color32,
    label: String,
    steps: usize,
    summary: String,
}

fn lane_row((index, lane): (usize, &PLockLane)) -> LaneRow {
    let editable = plocklane::lane_is_editable(lane);
    let colour = if editable { plocklane::lane_color(index).0 } else { READ_ONLY_LANE_CHIP };
    LaneRow {
        index,
        editable,
        colour,
        label: plocklane::lane_param(lane).label.into_owned(),
        steps: lane.values.iter().filter(|v| v.is_some()).count(),
        summary: describe_lane(lane),
    }
}

/// A dashed rectangle border — the "+ add lane…" affordance's `border: 1px
/// dashed` in the 2b spec, which `Painter::rect_stroke` cannot draw on its own
/// (it only takes a solid [`egui::Stroke`]). Built the same way
/// [`super::paint_fold_arrow`] draws a shape no font has: plain line segments,
/// one per edge, each turned to dashes by [`egui::Shape::dashed_line`].
fn paint_dashed_rect(painter: &egui::Painter, rect: egui::Rect, colour: Color32) {
    let stroke = egui::Stroke::new(1.0, colour);
    for edge in [
        [rect.left_top(), rect.right_top()],
        [rect.right_top(), rect.right_bottom()],
        [rect.right_bottom(), rect.left_bottom()],
        [rect.left_bottom(), rect.left_top()],
    ] {
        painter.extend(egui::Shape::dashed_line(&edge, stroke, 3.0, 3.0));
    }
}

/// The VIEW slider's number, and the zoom it means.
///
/// **Percent, not the multiple the roll stores**, so the digits typed into the
/// value box mean what the box's own formatter prints. `super::slider_row`'s doc
/// comment flags exactly this trap — its `DragValue` parses with egui's default
/// parser, which knows nothing about a `%` suffix — and the PROB row solved it
/// the same way. Store the multiple and "200" would arrive as 200x, clamped to
/// 4x, while the box went on saying 400%.
///
/// **Rounded on the way in**, so what is stored is what is drawn rather than a
/// display rounded off a value that kept its fraction. That is Phase 9's
/// velocity slider in miniature, and the clamp on `PianoRoll::set_zoom` is the
/// same lesson's other half.
fn zoom_from_percent(percent: f32) -> f32 {
    percent.round() / 100.0
}

/// The inverse, and what the slider's range is built out of so the track's ends
/// cannot drift from the roll's clamp.
fn zoom_percent(zoom: f32) -> f32 {
    zoom * 100.0
}

/// The VIEW slider's track, in the percent its number is in.
///
/// **A function so the range is reachable from a test**, rather than two
/// literals inline in the widget call. `PianoRoll::set_zoom` clamps to the same
/// two constants, and a track wider than that clamp gives a handle that can be
/// dragged to a number the roll refuses; a track narrower than it makes part of
/// the roll's own range unreachable. Either way it is one rule written twice,
/// which `DEVELOPMENT.md` lesson 5 says will be forgotten in one of them.
fn zoom_range() -> std::ops::RangeInclusive<f32> {
    zoom_percent(ZOOM_MIN)..=zoom_percent(ZOOM_MAX)
}

/// The NOTES section's caption: which selection state a control's number
/// belongs to, since Velocity in particular is a readout as much as it is a
/// control. Pure so it can be checked without a `Ui` — see the tests below.
fn selection_caption(selected: usize) -> String {
    match selected {
        0 => String::from("nothing selected \u{2014} sets new notes"),
        1 => String::from("1 selected"),
        n => format!("{n} selected"),
    }
}

/// [`super::slider_row`] plus the hover text every slider in this panel had
/// before the move to that shared widget. `slider_row` returns only whether the
/// value changed, not an `egui::Response`, so the tooltip hangs off this
/// function's own `ui.scope` wrapper — which is itself a normal `Response`,
/// with `Sense::hover()` by default — rather than off the widget directly.
fn tooltip_slider_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    format: impl Fn(f32) -> String,
    hover: &str,
) -> bool {
    let mut changed = false;
    ui.scope(|ui| changed = super::slider_row(ui, label, value, range, format))
        .response
        .on_hover_text(hover);
    changed
}

/// What the roll and the two lanes under it can do, because none of it is
/// discoverable by looking.
///
/// `ui::tools` used to carry the trig lane's and the p-lock lanes' instructions —
/// it was the only place in the app that said a PROB cell could be dragged. Those
/// entries moved here rather than being deleted with the Edit slot: five of the
/// roll's fifteen gestures are modifier-and-drag, so a panel about editing notes
/// that did not list them would leave most of this app's editing invisible.
///
/// A hover would not do. `ui::tools`' own rule is that the honest state of a thing
/// is not something to hide behind one, and a gesture list is exactly the thing
/// someone opens a panel to find.
///
/// **No longer captioned in its own right.** Per 2b rule 1 this text now lives
/// entirely inside the KEYS & GESTURES disclosure row (and behind the title
/// bar's `?`, the same flag) — the row's own header supplies the heading this
/// function used to draw with `super::caption`.
fn in_the_roll(ui: &mut Ui) {
    let green = egui::Color32::from_rgb(0x7a, 0xa8, 0x4a);
    // Every mark here is ASCII, per `ui::mod`'s glyph rule — no arrows, no bullets.
    for (what, how) in ROLL_GESTURES {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new(*what).strong().small());
            ui.label(egui::RichText::new(*how).small().weak());
        });
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Under the roll: the trig lane holds per-step PROB, FILL and COND — drag \
             a PROB cell, click COND for the picker, click FILL to cycle it. Below \
             that, one bar graph per p-lock lane: press in a bar to set a step, drag \
             to paint, right-click to clear. A grey lane is read-only; hover it to \
             find out why. Either lane's edit reaches every selected step at once.",
        )
        .small()
        .color(green),
    );
}

/// The itemized half of [`in_the_roll`]'s reference text — pulled into its own
/// array so the KEYS & GESTURES row's "N shortcuts" hint ([`gesture_count`])
/// counts exactly what is drawn, rather than a hand-maintained number that can
/// drift from the list under it.
///
/// **This counts nine, not the original mock's eighteen.** (Eight until zoom
/// landed — the count is the array's length, so this sentence is the only place
/// that has to be edited by hand.) The mock's
/// "IN THE ROLL" block enumerated mouse *and* keyboard shortcuts as separate
/// lines; this app's version of that block (see the top-of-file doc comment's
/// history) already condensed them into eight named gestures plus one
/// paragraph describing the trig and p-lock lanes in prose rather than as a
/// list. Counting the paragraph's gestures individually would mean inventing
/// list items this file has never drawn as one; counting only what is
/// itemized is the honest number for what is actually here.
const ROLL_GESTURES: &[(&str, &str)] = &[
    (
        "Draw",
        "Click an empty cell for a one-step note, or drag right to set its length. \
         With chord draw on — the Harmony panel — a click stamps the whole chord \
         under the ghost instead.",
    ),
    ("Move", "Drag a note's body. A selection travels with it."),
    (
        "Length",
        "Drag a note's right edge; hold shift while dragging for a fine resize, \
         snapped to what the box stores.",
    ),
    ("Velocity", "Shift-drag a note's body up or down. A selection moves by one delta."),
    ("Micro-timing", "Cmd-drag a note's body sideways. That note only, so a chord can be strummed."),
    ("Copy", "Alt-drag a note or a selection. Alt-click deletes instead."),
    ("Select", "Cmd-drag empty space to band-select; shift-click to add or drop one."),
    ("Delete", "Right-click a note, or press Delete with a selection."),
    (
        "Zoom",
        "Cmd-scroll or pinch over the grid. The cell under the pointer stays \
         where it is, so you zoom into what you are looking at; the VIEW slider \
         above sets the same number from the panel.",
    ),
];

fn gesture_count() -> usize {
    ROLL_GESTURES.len()
}

/// Undo and redo from the keyboard, whether the Edit panel is open or not.
///
/// Drawn from the shell for the same reason the session panel's close guard is:
/// this panel can be closed, and a shortcut that only works while a panel is
/// showing is a shortcut nobody finds. Returns whether the history moved.
///
/// **Guarded on nothing being typed into**, which is egui's equivalent of
/// `js/pianoroll.js`'s "not typing" check: without it Cmd+Z inside the tempo field
/// would undo a note edit instead of the text.
///
/// The guard is `tracks::typing_elsewhere` rather than a bare
/// `focused().is_some()` because a clicked TRACKS cell holds keyboard focus —
/// that is how its Delete is armed — and undo has to keep working on the frame
/// after one clears a track, which is precisely when it is wanted most.
pub fn shortcuts(
    ui: &Ui,
    session: &mut Session,
    roll: &mut PianoRoll,
    history: &mut History,
) -> bool {
    if crate::ui::tracks::typing_elsewhere(ui.ctx(), session) {
        return false;
    }
    let (undo, redo) = ui.ctx().input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
            i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z),
        )
    });
    // Redo first: `Cmd+Shift+Z` also satisfies a plain `Cmd+Z` matcher on some
    // platforms, and consuming the more specific one first is what keeps a redo
    // from arriving as an undo.
    if redo {
        return step_forward(session, roll, history);
    }
    if undo {
        return step_back(session, roll, history);
    }
    false
}

fn step_back(session: &mut Session, roll: &mut PianoRoll, history: &mut History) -> bool {
    // Any gesture part-way through is abandoned rather than committed: it would be
    // measured against music that is about to be replaced.
    history.abandon();
    // The selection names notes in the music being stepped away from.
    roll.clear_selection();
    history.undo(session)
}

fn step_forward(session: &mut Session, roll: &mut PianoRoll, history: &mut History) -> bool {
    history.abandon();
    roll.clear_selection();
    history.redo(session)
}

// --- resolving the selection --------------------------------------------------

/// Everything the panel needs resolved once: the track, the label to call it by,
/// the pattern's swing, and the device whose parameter numbering applies.
///
/// **One resolve rather than one per reader.** Swing used to have its own lookup
/// with an `unwrap_or(50)` on the end, and a deliberate bug replacing that default
/// with a panic failed nothing — because every caller checks this function first,
/// so the default was unreachable. An unreachable fallback is a claim that the
/// value can be missing, which then has to be believed everywhere. Folding it in
/// deletes the claim instead of testing it.
struct Context<'a> {
    track: &'a Track,
    /// `A01 T1`, which is what makes an exported file recognisable as the slot it
    /// came off rather than as `pattern.mid`.
    label: String,
    /// The pattern's own name alone — `label` without the track name folded
    /// in — for the title bar's "A01 · T1" context string, which wants its own
    /// separator rather than `label`'s space.
    pattern_name: String,
    swing: u8,
    device: DeviceId,
}

fn context(session: &Session, selection: Selection) -> Option<Context<'_>> {
    let device = session.devices.get(selection.device)?;
    let pattern = session.current_pattern(device.id)?;
    let track = pattern.track(selection.track)?;
    Some(Context {
        track,
        label: format!("{} {}", pattern.name, track.name),
        pattern_name: pattern.name.clone(),
        swing: pattern.swing,
        device: device.id,
    })
}

/// The pattern the roll is editing a track of. `tracks::track_mut`'s sibling, and
/// resolved the same way — through the scene rather than remembered, so switching
/// scene moves the panel onto the pattern that is now playing.
fn pattern_mut(session: &mut Session, selection: Selection) -> Option<&mut digi_core::Pattern> {
    let device = session.devices.get(selection.device)?.id;
    let slot = session.slot_in_scene(session.current_scene, device)?.slot();
    session.device_mut(device)?.pattern_mut(slot)
}

fn clear_source(session: &mut Session, selection: Selection) {
    if let Some(pattern) = pattern_mut(session, selection) {
        pattern.source = None;
    }
}

/// What the Length slider shows: the last selected note's length, or one step when
/// nothing is selected.
///
/// The *last*, matching `js/main.js`'s `selectedNotes().at(-1)?.len` — with a
/// mixed selection the slider has to show one of them, and the most recently
/// touched is the least surprising one.
fn length_of(session: &Session, selection: Selection, selected: &[u32]) -> f64 {
    crate::ui::tracks::track(session, selection)
        .and_then(|t| t.notes.iter().rfind(|n| selected.contains(&n.id)))
        .map(|n| n.len)
        .unwrap_or(1.0)
}

/// A length as `js/main.js` prints it: `+len.toFixed(3)`, which drops trailing
/// zeroes so `1` is `1` and `1.125` is `1.125`.
fn trim(len: f64) -> String {
    let s = format!("{len:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { String::from("0") } else { s.to_string() }
}

/// The note a length readout is about. Not used by the panel — kept out of it so
/// `length_of` has one job — but it is what a test asserts against.
#[cfg(test)]
fn last_selected<'a>(track: &'a Track, selected: &[u32]) -> Option<&'a digi_core::Note> {
    track.notes.iter().rfind(|n| selected.contains(&n.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::{Note, TrackKind};
    use std::path::PathBuf;

    /// A [`Chooser`] that hands back fixed paths instead of opening a real
    /// dialog, so the MIDI FILE group's Export and Import buttons can be
    /// driven end to end in a headless test. Export's path only needs to be
    /// writable; Import's has to already hold bytes `midi_file_to_notes` can
    /// parse, which the test that uses this writes with
    /// [`track_to_midi_file`] before the panel ever sees it.
    struct StubChooser {
        export_path: PathBuf,
        import_path: PathBuf,
    }

    impl Chooser for StubChooser {
        fn save_as(&mut self, _suggested: &str) -> Option<PathBuf> {
            None
        }
        fn open(&mut self) -> Option<PathBuf> {
            None
        }
        fn export_as(&mut self, _suggested: &str) -> Option<PathBuf> {
            None
        }
        fn export_midi_as(&mut self, _suggested: &str) -> Option<PathBuf> {
            Some(self.export_path.clone())
        }
        fn open_midi(&mut self) -> Option<PathBuf> {
            Some(self.import_path.clone())
        }
    }

    /// One pass through [`EditPanel::midi_group`] with the given input events,
    /// mirroring `setup.rs`'s own `frame` helper for `status_strip` — the
    /// established shape in this codebase for driving real frames through
    /// `Context::run_ui` rather than asserting on state no control could
    /// actually reach.
    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        panel: &mut EditPanel,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
        out: &mut Outcome,
    ) {
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |u| {
            panel.midi_group(u, session, selection, roll, out);
        });
        output.textures_delta.clear();
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn the_midi_file_box_is_closed_on_a_freshly_constructed_panel() {
        // Neil's 2026-08-20 decision (`PLAN.md`'s five decisions, item 3):
        // collapse the box by default, the same treatment HISTORY already
        // gets in this panel. This is the plant target — defaulting
        // `midi_open` to `true` must fail exactly this assertion.
        assert!(!EditPanel::default().midi_open, "MIDI FILE starts collapsed");
    }

    #[test]
    fn the_midi_file_box_opens_on_click_and_import_and_export_are_reachable_inside_it() {
        // Closed-by-default plus an unreachable body is a regression, not a
        // fix (per the packet brief). This drives real frames rather than
        // flipping the field by hand: the disclosure header is clicked open
        // exactly as a user would, and Export and Import are then clicked at
        // their own measured positions — proof the body is interactive, not
        // merely drawn.
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let selection = Selection::default();
        let mut roll = PianoRoll::default();
        let mut out = Outcome::default();

        // A note on the track so Export is enabled rather than greyed out —
        // a click that lands on a disabled button proves nothing.
        if let Some(track) = crate::ui::tracks::track_mut(&mut session, selection) {
            track.notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
        }
        // A real Standard MIDI File for Import to read — generated with the
        // same `track_to_midi_file` the Export button itself calls, so
        // Import's stub path is exactly what a round trip through this app's
        // own exporter produces (the one case the packet's new sentence says
        // works today).
        let dir = std::env::temp_dir()
            .join(format!("digi-roll-edit-midi-group-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let import_path = dir.join("roundtrip.mid");
        let export_path = dir.join("export.mid");
        {
            let track = Track::new(0, TrackKind::Audio);
            let mut track = track;
            track.notes.push(Note::new(0.0, 60, 1.0, 100, 0.0));
            let bytes = track_to_midi_file(&track, "probe", 50, 120.0);
            std::fs::write(&import_path, bytes).expect("write fixture midi file");
        }

        let mut panel = EditPanel::with_chooser(Box::new(StubChooser {
            export_path: export_path.clone(),
            import_path: import_path.clone(),
        }));

        frame(&ctx, vec![], &mut panel, &mut session, selection, &mut roll, &mut out);
        assert!(!panel.midi_open, "starts closed, same as HISTORY");

        // The disclosure header, measured 2026-08-20 the same way `setup.rs`'s
        // own fold-row test targets BACKUPS: it is the first and only thing
        // drawn in this frame, so (10, 10) lands inside its row.
        let header = egui::Pos2 { x: 10.0, y: 10.0 };
        frame(
            &ctx,
            vec![egui::Event::PointerMoved(header), press(header, true)],
            &mut panel,
            &mut session,
            selection,
            &mut roll,
            &mut out,
        );
        frame(&ctx, vec![press(header, false)], &mut panel, &mut session, selection, &mut roll, &mut out);
        assert!(panel.midi_open, "clicking the row opens it");

        // Export and Import's own rects, measured 2026-08-20 against this
        // exact fixture (one note on the track, the box freshly opened, a
        // default headless `Context`): Export centres near (38.9, 50.0),
        // Import near (107.2, 50.0). If a future layout change moves them,
        // this assertion pair is what will need re-measuring — not a reason
        // to stop measuring.
        let export_centre = egui::Pos2 { x: 38.9, y: 50.0 };
        frame(
            &ctx,
            vec![egui::Event::PointerMoved(export_centre), press(export_centre, true)],
            &mut panel,
            &mut session,
            selection,
            &mut roll,
            &mut out,
        );
        frame(
            &ctx,
            vec![press(export_centre, false)],
            &mut panel,
            &mut session,
            selection,
            &mut roll,
            &mut out,
        );
        assert!(
            matches!(panel.status(), Some(Status::Exported { .. })),
            "Export is reachable and does its job with the box open, got {:?}",
            panel.status()
        );
        assert!(export_path.exists(), "the click actually reached the export path");

        let import_centre = egui::Pos2 { x: 107.2, y: 50.0 };
        frame(
            &ctx,
            vec![egui::Event::PointerMoved(import_centre), press(import_centre, true)],
            &mut panel,
            &mut session,
            selection,
            &mut roll,
            &mut out,
        );
        frame(
            &ctx,
            vec![press(import_centre, false)],
            &mut panel,
            &mut session,
            selection,
            &mut roll,
            &mut out,
        );
        assert!(
            matches!(panel.status(), Some(Status::Imported { .. })),
            "Import is reachable and does its job with the box open, got {:?}",
            panel.status()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_destructive_note_admits_the_first_track_limit() {
        // Pins `MIDI_IMPORT_WARNING` — the actual string `midi_group` draws,
        // not a hand-typed copy of it — to the claims its third sentence has
        // to keep making, so a future edit cannot quietly soften it back
        // toward "coming soon" without this test noticing. Wording is
        // pending Neil's sign-off per the 2026-08-20 packet brief; if he
        // changes it, this is the one place that has to change with it.
        let body = MIDI_IMPORT_WARNING;
        assert!(body.contains("replaces this track's notes, its p-lock lanes and its provenance"));
        assert!(body.contains("no PROB, no FILL and no COND"));
        assert!(body.contains("only the first note-bearing track in the file"));
        assert!(body.contains("cannot offset it"));
        assert!(
            body.contains("multi-track file from a DAW will likely bring in the wrong part, or nothing")
        );
        assert!(body.contains("a track chooser and a from-bar control are coming"));
        assert!(body.contains("round-tripping a file this app exported works today"));
        assert!(!body.to_lowercase().contains("coming soon"), "lesson 3 cuts both ways");
    }

    #[test]
    fn a_length_is_printed_the_way_the_js_prints_it() {
        // `+len.toFixed(3)` in the JS: trailing zeroes gone, so the LEN scale's
        // fractional stops read as `1.125` and a whole step reads as `1`.
        assert_eq!(trim(1.0), "1");
        assert_eq!(trim(1.125), "1.125");
        assert_eq!(trim(0.125), "0.125");
        assert_eq!(trim(16.0), "16");
        assert_eq!(trim(0.0), "0");
    }

    #[test]
    fn the_len_slider_never_offers_the_hold_forever_byte() {
        // Byte 127 is `INFINITY` — the box's `INF` — which this app's model does
        // not carry, so the slider stops one short rather than showing a value a
        // write would have to invent something for.
        assert_eq!(LEN_BYTE_MAX, 126);
        assert!(length_byte_to_steps(LEN_BYTE_MAX).is_finite());
        assert!(length_byte_to_steps(127).is_infinite());
        // And its top stop is the longest track a box holds, so the slider can
        // reach a note that fills a whole pattern.
        assert_eq!(length_byte_to_steps(LEN_BYTE_MAX), 128.0);
    }

    #[test]
    fn every_stop_on_the_len_slider_is_a_length_the_box_stores() {
        // The guarantee the group's caption makes. Snapping is idempotent over the
        // whole range, which is what "the slider position *is* the length byte"
        // comes to.
        for byte in 0..=LEN_BYTE_MAX {
            let steps = length_byte_to_steps(byte);
            assert_eq!(
                steps_to_length_byte(steps),
                byte,
                "byte {byte} ({steps} steps) does not round-trip"
            );
        }
    }

    #[test]
    fn the_length_readout_follows_the_last_note_in_the_selection() {
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![
            Note::new(0.0, 60, 1.0, 100, 0.0),
            Note::new(4.0, 62, 4.0, 100, 0.0),
            Note::new(8.0, 64, 2.0, 100, 0.0),
        ];
        let ids: Vec<u32> = track.notes.iter().map(|n| n.id).collect();
        assert_eq!(last_selected(&track, &ids).map(|n| n.len), Some(2.0));
        assert_eq!(last_selected(&track, &ids[..2]).map(|n| n.len), Some(4.0));
        assert_eq!(last_selected(&track, &[]).map(|n| n.len), None);
    }

    #[test]
    fn the_notes_caption_names_the_selection_state() {
        // What the NOTES section header shows instead of the old per-slider
        // "Nothing selected..." label — per the 2b design spec's own examples.
        assert_eq!(selection_caption(0), "nothing selected \u{2014} sets new notes");
        assert_eq!(selection_caption(1), "1 selected");
        assert_eq!(selection_caption(3), "3 selected");
    }

    #[test]
    fn the_zoom_sliders_ends_are_the_rolls_own_clamp_and_its_number_is_percent() {
        // **Two statements of one rule the moment they can disagree**
        // (`DEVELOPMENT.md` lesson 5): a slider that could ask for a zoom
        // `set_zoom` refuses would sit with its handle at one end and its number
        // saying something else, and a slider that could not reach the range's
        // ends would make part of the roll's own clamp unreachable.
        let range = zoom_range();
        assert_eq!(zoom_from_percent(*range.start()), ZOOM_MIN, "the track's low end");
        assert_eq!(zoom_from_percent(*range.end()), ZOOM_MAX, "and its high end");

        // Which the roll then keeps, rather than clamping the slider's own ends
        // away under it.
        let mut roll = PianoRoll::default();
        for end in [*range.start(), *range.end()] {
            roll.set_zoom(zoom_from_percent(end));
            assert_eq!(zoom_percent(roll.zoom()), end, "the slider reaches {end}%, and no further");
        }

        // And a value a drag can actually land on: the number shown is the
        // number stored, to the percent.
        assert_eq!(zoom_from_percent(137.4), 1.37);
        assert!((zoom_percent(1.37) - 137.0).abs() < 1e-3);
    }

    #[test]
    fn the_shortcut_count_matches_what_the_reference_actually_lists() {
        // The KEYS & GESTURES row's hint is this number, not a hand-typed one —
        // so a gesture added to or removed from `ROLL_GESTURES` cannot silently
        // leave the hint saying something else.
        assert_eq!(gesture_count(), ROLL_GESTURES.len());
        assert_eq!(gesture_count(), 9, "see this const's own doc comment if this changes");
    }

    #[test]
    fn a_read_only_lane_gets_the_grey_chip_and_an_editable_one_gets_the_roll_s_own_colour() {
        // The chip is only worth drawing if it actually matches
        // `plocklane::lane_color`'s row-order palette for an editable lane, and
        // is visibly different for a lane the strip under the roll greys out.
        let editable = PLockLane::new(
            Some(String::from("filter.cutoff")),
            None,
            Some(String::from("DT2")),
            false,
            vec![Some(64)],
        )
        .unwrap();
        let read_only = PLockLane::new(None, Some(200), Some(String::from("DT2")), true, vec![])
            .unwrap();

        let row0 = lane_row((0, &editable));
        assert_eq!(row0.colour, plocklane::lane_color(0).0);
        assert!(row0.editable);

        let row1 = lane_row((1, &read_only));
        assert_eq!(row1.colour, READ_ONLY_LANE_CHIP);
        assert!(!row1.editable);
        assert_ne!(row1.colour, plocklane::lane_color(1).0);
    }

    /// **The panel lists a lane it cannot author, and the A4 is the box where
    /// those two answers first differ.** Listing needs only the lane's own
    /// recorded `device_kind`; authoring needs a measured p-lock scaling. The
    /// section used to be gated as a whole on `model.spec()`, which is `None`
    /// here, so a track carrying lanes off the box listed nothing at all.
    ///
    /// The lane below carries an **id and no name**, which on this box is not
    /// enough to curate — its id space is per track kind, so a bare `0x22`
    /// could be an FX-track lane. It lists, read-only, with the name the box
    /// prints.
    #[test]
    fn the_panel_lists_an_a4_lane_it_cannot_author_from() {
        let lane =
            PLockLane::new(None, Some(0x22), Some(String::from("A4")), false, vec![Some(0x4000)])
                .unwrap()
                .with_label("FLTR1 FRQ");

        let row = lane_row((0, &lane));
        assert_eq!(row.label, "FLTR1 FRQ", "the box's own name, not the hex stand-in");
        assert_eq!(row.steps, 1);
        assert!(!row.editable, "an id with no name is not curated on this box");
        assert_eq!(row.colour, READ_ONLY_LANE_CHIP);
    }

    /// **And the picker offers every parameter this app knows on the A4.**
    /// This asserted `writable_params_for("A4").is_empty()` until 2026-09-01,
    /// when `a4_scale_probe` read all thirteen scalings off the A4's own screen
    /// across four runs — so the A4's picker is now exactly as full as a digi's.
    #[test]
    fn the_picker_offers_the_a4_parameters_that_were_measured() {
        assert_eq!(writable_params_for("A4").len(), 13);
        // A lane resolved by canonical name — what the import gives a synth
        // track — is curated, editable and draws in colour.
        let lane = PLockLane::new(
            Some(String::from("filter.cutoff")),
            Some(0x22),
            Some(String::from("A4")),
            false,
            vec![Some(64)],
        )
        .unwrap();
        let row = lane_row((0, &lane));
        assert!(row.editable, "a measured scaling is what makes a lane draggable");
        assert_eq!(row.colour, plocklane::lane_color(0).0);
        assert_eq!(row.label, "FLTR1 FREQ");
    }

    /// The switch from `auditable_params_for` to `writable_params_for` costs the
    /// digis nothing — every parameter they can hear also has a measured p-lock
    /// slot. If that ever stops being true the picker quietly loses an entry,
    /// so it is asserted rather than assumed.
    #[test]
    fn every_auditable_digi_parameter_is_also_authorable() {
        for kind in ["DT2", "DN2"] {
            assert_eq!(
                writable_params_for(kind).len(),
                digi_protocol::params::auditable_params_for(kind).len(),
                "{kind}",
            );
        }
    }
}
