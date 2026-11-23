//! A second editor over the session's ordinary notes: one hardware track per
//! row, sixteen steps per page. No drum-map conversion or second pattern model.
//!
//! A cell groups note starts in [step, step + 1), including chords and off-grid
//! imports. Switching views never quantises them. Toggle/erase removes that
//! cell's notes only, leaving p-locks intact just as Piano Roll deletion does.
//! Shift-click selects the cell's notes for the shared Edit panel. Drag painting
//! fixes its add/erase intent at the first cell, so revisiting a cell is harmless.

use std::collections::HashMap;

use digi_core::device::DeviceId;
use digi_core::{Note, Session, Track};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::pianoroll::{note_name, PianoRoll, KEY_W};
use super::plocklane::{self, PLockStrip};
use super::tracks::{self, Selection};
use super::triglane::{self, Cols, TrigLane};

const PAGE: usize = 16;
const GUTTER: f32 = 188.0;
const ROW_H: f32 = 30.0;
const RULER_H: f32 = 23.0;
const VELOCITY_H: f32 = 52.0;
const MIN_WIDTH: f32 = GUTTER + 16.0 * 22.0;

#[derive(Clone, Copy)]
struct Paint {
    row: usize,
    last: usize,
    add: bool,
}

#[cfg(test)]
mod tests {
    // This editor has no JS oracle. Assertions come from the shared model's
    // invariants and real pointer/key events through egui, not a second editor.
    use super::*;
    use digi_core::history::{Content, History};
    use digi_core::{two_box_session, PLockLane, TrackScale};

    struct Harness {
        ctx: egui::Context,
        drums: DrumSequencer,
        session: Session,
        selection: Selection,
        roll: PianoRoll,
        history: History,
    }

    impl Harness {
        fn new() -> Self {
            let mut this = Self {
                ctx: egui::Context::default(),
                drums: DrumSequencer::default(),
                session: two_box_session(),
                selection: Selection::default(),
                roll: PianoRoll::default(),
                history: History::default(),
            };
            this.frame(vec![], egui::Modifiers::NONE);
            this
        }

        fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> bool {
            let mut all_events = vec![egui::Event::ModifiersChanged(modifiers)];
            all_events.extend(events);
            let before = (!self.history.is_open()).then(|| Content::of(&self.session));
            let mut changed = false;
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))),
                    events: all_events,
                    ..Default::default()
                },
                |ui| {
                    changed = self.drums.ui(
                        ui,
                        &mut self.session,
                        &mut self.selection,
                        &mut self.roll,
                        None,
                        false,
                    );
                },
            );
            output.textures_delta.clear();
            if changed {
                if let Some(before) = before {
                    self.history.begin(before);
                }
            }
            if !self.ctx.input(|i| i.pointer.any_down()) {
                self.history.commit(&self.session);
            }
            changed
        }

        fn cell(&self, row: usize, col: usize) -> Pos2 {
            let rect = self
                .ctx
                .read_response(egui::Id::new("digi-drum-grid"))
                .unwrap()
                .rect;
            Pos2::new(
                rect.left() + (col as f32 + 0.5) * rect.width() / 16.0,
                rect.top() + (row as f32 + 0.5) * ROW_H,
            )
        }

        fn button(&mut self, pos: Pos2, pressed: bool, secondary: bool, shift: bool) -> bool {
            let modifiers = if shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            };
            self.frame(
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        pressed,
                        modifiers,
                        button: if secondary {
                            egui::PointerButton::Secondary
                        } else {
                            egui::PointerButton::Primary
                        },
                    },
                ],
                modifiers,
            )
        }

        fn click(&mut self, row: usize, col: usize, shift: bool) {
            let pos = self.cell(row, col);
            self.button(pos, true, false, shift);
            self.button(pos, false, false, shift);
        }

        fn track(&self) -> &Track {
            tracks::track(&self.session, self.selection).unwrap()
        }
    }

    #[test]
    fn click_edits_shared_track_and_undo_redo_restore_it() {
        let mut h = Harness::new();
        h.click(1, 4, false);
        assert_eq!(h.selection.track, 1);
        assert_eq!(h.track().notes.len(), 1);
        assert_eq!(h.track().notes[0].step, 4.0);
        assert_eq!(h.roll.selection(), vec![h.track().notes[0].id]);
        assert!(h.history.undo(&mut h.session));
        assert!(h.track().notes.is_empty());
        assert!(h.history.redo(&mut h.session));
        assert_eq!(h.track().notes[0].step, 4.0);
        h.click(1, 4, false);
        assert!(h.track().notes.is_empty());
        assert!(tracks::track(
            &h.session,
            Selection {
                device: 1,
                track: 1
            }
        )
        .unwrap()
        .notes
        .is_empty());
    }

    #[test]
    fn fast_drag_paints_intervening_steps_once_as_one_undo() {
        let mut h = Harness::new();
        let start = h.cell(0, 0);
        let end = h.cell(0, 7);
        h.button(start, true, false, false);
        h.frame(vec![egui::Event::PointerMoved(end)], egui::Modifiers::NONE);
        h.frame(
            vec![egui::Event::PointerMoved(start)],
            egui::Modifiers::NONE,
        );
        h.button(start, false, false, false);
        assert_eq!(h.track().notes.len(), 8);
        assert!(h.history.undo(&mut h.session));
        assert!(h.track().notes.is_empty());
        assert!(!h.history.undo(&mut h.session));
    }

    #[test]
    fn right_drag_erases_without_adding_or_crossing_track_lengths() {
        let mut h = Harness::new();
        h.click(0, 2, false);
        h.click(0, 4, false);
        let start = h.cell(0, 0);
        let end = h.cell(0, 5);
        h.button(start, true, true, false);
        h.frame(vec![egui::Event::PointerMoved(end)], egui::Modifiers::NONE);
        h.button(end, false, true, false);
        assert!(h.track().notes.is_empty());
        tracks::track_mut(&mut h.session, h.selection)
            .unwrap()
            .length_steps = 5;
        h.click(0, 5, false);
        assert!(h.track().notes.is_empty());
        h.click(0, 4, false);
        assert_eq!(h.track().notes[0].step, 4.0);
    }

    #[test]
    fn shift_click_keeps_chords_offsets_and_locks_then_delete_is_scoped() {
        let mut h = Harness::new();
        let mut chord = vec![
            Note::new(3.0, 36, 2.0, 90, -0.125),
            Note::new(3.0, 43, 1.0, 65, -0.125),
            Note::new(3.5, 47, 0.5, 50, 0.0),
        ];
        chord[0].prob = Some(75);
        let t = tracks::track_mut(&mut h.session, h.selection).unwrap();
        t.notes = chord.clone();
        t.notes.push(Note::new(8.0, 60, 1.0, 100, 0.0));
        t.plocks.push(
            PLockLane::new(
                Some("filter.cutoff".into()),
                None,
                Some("DT2".into()),
                false,
                vec![None, None, None, Some(64)],
            )
            .unwrap(),
        );
        let locks = t.plocks.clone();
        h.click(0, 3, true);
        assert_eq!(&h.track().notes[..3], chord.as_slice());
        assert_eq!(h.roll.selection().len(), 3);
        h.frame(
            vec![egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            egui::Modifiers::NONE,
        );
        assert_eq!(h.track().notes.len(), 1);
        assert_eq!(h.track().notes[0].step, 8.0);
        assert_eq!(h.track().plocks, locks);
    }

    #[test]
    fn page_two_addresses_step_seventeen_and_short_rows_remain_disabled() {
        let mut h = Harness::new();
        tracks::track_mut(&mut h.session, h.selection)
            .unwrap()
            .length_steps = 64;
        h.drums.page = 1;
        h.frame(vec![], egui::Modifiers::NONE);
        h.click(0, 0, false);
        assert_eq!(h.track().notes[0].step, 16.0);
        h.click(1, 0, false);
        assert!(h.track().notes.is_empty());
        assert_eq!(
            h.drums.page, 1,
            "short selected rows do not change the pattern's available pages"
        );
    }

    #[test]
    fn device_switch_cancels_a_held_brush() {
        let mut h = Harness::new();
        let start = h.cell(0, 0);
        h.button(start, true, false, false);
        h.selection.device = 1;
        h.frame(
            vec![egui::Event::PointerMoved(h.cell(0, 5))],
            egui::Modifiers::NONE,
        );
        h.button(start, false, false, false);
        assert!(h.track().notes.is_empty());
        assert!(h.drums.paint.is_none());
    }

    #[test]
    fn velocity_lane_updates_selected_notes_without_changing_pitch_or_timing() {
        let mut h = Harness::new();
        h.click(0, 0, false);
        h.click(0, 4, false);
        h.click(0, 0, true);
        let before = h.track().notes.clone();
        let rect = h
            .ctx
            .read_response(egui::Id::new("digi-drum-velocity"))
            .unwrap()
            .rect;
        let pos = Pos2::new(
            rect.left() + rect.width() / 32.0,
            rect.top() + rect.height() * 0.5,
        );
        h.button(pos, true, false, false);
        h.button(pos, false, false, false);
        assert!(h.track().notes.iter().all(|n| n.velocity == 64));
        for (old, new) in before.iter().zip(&h.track().notes) {
            assert_eq!(
                (old.step, old.pitch, old.len, old.micro),
                (new.step, new.pitch, new.len, new.micro)
            );
        }
    }

    #[test]
    fn opening_view_is_read_only_and_playheads_respect_polymeter() {
        let mut h = Harness::new();
        let before = Content::of(&h.session);
        assert!(!h.frame(vec![], egui::Modifiers::NONE));
        assert_eq!(before, Content::of(&h.session));
        let t = tracks::track_mut(&mut h.session, h.selection).unwrap();
        t.length_steps = 12;
        t.scale = TrackScale::Half;
        assert_eq!(play_step(t, 30.0), 3);
        t.length_steps = 64;
        t.scale = TrackScale::Two;
        assert_eq!(play_step(t, 40.0), 16);
    }
}

#[derive(Default)]
pub struct DrumSequencer {
    page: usize,
    follow: bool,
    paint: Option<Paint>,
    // Scene/slot changes cancel held gestures; no held mouse can edit a new song.
    context: Option<(DeviceId, usize)>,
    detail_track: Option<Selection>,
    reveal_track: bool,
    pitches: HashMap<(DeviceId, usize), u8>,
    trig: TrigLane,
    plocks: PLockStrip,
}

fn in_cell(note: &Note, step: usize) -> bool {
    note.step >= step as f64 && note.step < (step + 1) as f64
}

fn cell_notes(track: &Track, step: usize) -> Vec<u32> {
    track
        .notes
        .iter()
        .filter(|n| in_cell(n, step))
        .map(|n| n.id)
        .collect()
}

fn set_cell(track: &mut Track, step: usize, add: bool, pitch: u8, velocity: u8) -> bool {
    if step >= usize::from(track.length_steps) {
        return false;
    }
    let occupied = track.notes.iter().any(|n| in_cell(n, step));
    if add && !occupied {
        track.notes.push(Note::new(
            step as f64,
            pitch.min(127),
            1.0,
            velocity.clamp(1, 127),
            0.0,
        ));
        true
    } else if !add && occupied {
        track.notes.retain(|n| !in_cell(n, step));
        true
    } else {
        false
    }
}

fn play_step(track: &Track, position: f64) -> usize {
    ((position * track.scale.multiplier()).rem_euclid(f64::from(track.length_steps.max(1)))).floor()
        as usize
}

fn page_count(session: &Session, selection: Selection) -> usize {
    session
        .devices
        .get(selection.device)
        .and_then(|d| session.current_pattern(d.id))
        .map(|p| {
            p.tracks()
                .iter()
                .map(|t| usize::from(t.length_steps))
                .max()
                .unwrap_or(1)
        })
        .unwrap_or(1)
        .div_ceil(PAGE)
        .max(1)
}

fn scale_text(track: &Track) -> String {
    format!("{}\n{}x", track.length_steps, track.scale.multiplier())
}

impl DrumSequencer {
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        selection: &mut Selection,
        roll: &mut PianoRoll,
        position: Option<f64>,
        recording: bool,
    ) -> bool {
        let Some(device) = session.devices.get(selection.device) else {
            ui.weak("Add a box in Setup to start a drum pattern.");
            return false;
        };
        let device_id = device.id;
        let Some(slot) = session.slot_in_scene(session.current_scene, device_id) else {
            ui.weak("No pattern in this scene.");
            return false;
        };
        let context = (device_id, slot.slot());
        if self.context != Some(context) {
            self.context = Some(context);
            self.page = 0;
            self.paint = None;
            self.reset_details();
        }
        self.sync_details(*selection);
        let pages = page_count(session, *selection);
        self.page = self.page.min(pages - 1);
        if self.follow && self.paint.is_none() && !ui.input(|i| i.pointer.any_down()) {
            if let (Some(position), Some(track)) = (position, tracks::track(session, *selection)) {
                let page = play_step(track, position) / PAGE;
                if self.page != page {
                    self.page = page;
                    self.reset_details();
                }
            }
        }
        let owner = format!("{} · {}", device.name, slot.label());
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(owner).strong());
            ui.label("PAGE");
            for page in 0..pages {
                if ui
                    .selectable_label(self.page == page, (page + 1).to_string())
                    .on_hover_text(format!("Steps {}–{}", page * PAGE + 1, (page + 1) * PAGE))
                    .clicked()
                {
                    self.page = page;
                    self.follow = false;
                    self.paint = None;
                    self.reset_details();
                }
            }
            ui.checkbox(&mut self.follow, "Follow")
                .on_hover_text("Follow the selected track's playhead");
            ui.label("NEW NOTE");
            let pitch = self
                .pitches
                .entry((device_id, selection.track))
                .or_insert_with(|| {
                    tracks::track(session, *selection)
                        .and_then(|t| t.notes.first())
                        .map_or(60, |n| n.pitch)
                });
            ui.add(egui::DragValue::new(pitch).range(0..=127))
                .on_hover_text("Pitch for new steps on this track");
            ui.label(note_name(*pitch));
            ui.label("VEL");
            let mut velocity = roll.default_velocity();
            if ui
                .add(egui::DragValue::new(&mut velocity).range(1..=127))
                .changed()
            {
                roll.set_default_velocity(velocity);
            }
        });

        let mut changed = false;
        // The shared roll selection lets the existing Edit/Harmony panels and
        // undo shortcuts work in either view. Stale ids never select new notes.
        if let Some(track) = tracks::track(session, *selection) {
            let selected = roll.selection();
            roll.select(
                track
                    .notes
                    .iter()
                    .filter(|n| selected.contains(&n.id))
                    .map(|n| n.id),
            );
        }
        if !ui.ctx().memory(|m| m.focused().is_some()) {
            let delete = ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                    | i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
            });
            if delete {
                if let Some(track) = tracks::track_mut(session, *selection) {
                    let selected = roll.selection();
                    let before = track.notes.len();
                    track.notes.retain(|n| !selected.contains(&n.id));
                    changed |= before != track.notes.len();
                    roll.clear_selection();
                }
            }
        }

        ui.label(egui::RichText::new("Click toggles · drag paints · right-drag erases · Shift-click selects · bars show velocity")
            .small().color(super::TEXT_MUTED));
        let lane_h = tracks::track(session, *selection)
            .map_or(0.0, |t| plocklane::strip_height(t).min(68.0));
        let details_h = VELOCITY_H + triglane::LANE_H + lane_h + 35.0;
        // Keep the details reachable in short windows: the whole editor can
        // scroll vertically, while the track matrix gets its own bounded area.
        let matrix_h = (ui.available_height() - details_h - RULER_H - 40.0).max(100.0);
        egui::ScrollArea::both()
            .id_salt("drum-editor-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(MIN_WIDTH);
                let width = ui.available_width().max(MIN_WIDTH);
                let cell_w = (width - GUTTER) / PAGE as f32;
                let (ruler, _) = ui.allocate_exact_size(Vec2::new(width, RULER_H), Sense::hover());
                ui.painter().text(
                    ruler.left_center() + Vec2::new(3.0, 0.0),
                    Align2::LEFT_CENTER,
                    "TRACK",
                    FontId::proportional(10.0),
                    super::TEXT_MUTED,
                );
                ui.painter().text(
                    Pos2::new(ruler.left() + 131.0, ruler.center().y),
                    Align2::CENTER_CENTER,
                    "LEN / x",
                    FontId::proportional(10.0),
                    super::TEXT_MUTED,
                );
                for col in 0..PAGE {
                    let pos = Pos2::new(
                        ruler.left() + GUTTER + (col as f32 + 0.5) * cell_w,
                        ruler.center().y,
                    );
                    ui.painter().text(
                        pos,
                        Align2::CENTER_CENTER,
                        (self.page * PAGE + col + 1).to_string(),
                        FontId::proportional(11.0),
                        if col % 4 == 0 {
                            super::TEXT_BRIGHT
                        } else {
                            super::TEXT_MUTED
                        },
                    );
                }
                egui::ScrollArea::vertical()
                    .id_salt("drum-rows")
                    .max_height(matrix_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        changed |= self.matrix(
                            ui, width, cell_w, session, selection, roll, position, recording,
                        );
                    });
                self.sync_details(*selection);
                if let Some(track) = tracks::track_mut(session, *selection) {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "T{:02}  {} · {} selected",
                            selection.track + 1,
                            track.name,
                            roll.selection().len()
                        ))
                        .color(super::TEXT_PRIMARY),
                    );
                    let selected = roll.selection();
                    let (velocity, _) =
                        ui.allocate_exact_size(Vec2::new(width, VELOCITY_H), Sense::hover());
                    let cols = Cols {
                        origin_x: velocity.left() + GUTTER - (self.page * PAGE) as f32 * cell_w,
                        cell_w,
                    };
                    changed |= self.velocity(ui, velocity, cols, track, &selected, roll);
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(width, triglane::LANE_H), Sense::hover());
                    let rect = Rect::from_min_max(
                        Pos2::new(rect.left() + GUTTER - KEY_W, rect.top()),
                        rect.max,
                    );
                    changed |= self.trig.ui(ui, rect, cols, track, &selected);
                    if lane_h > 0.0 {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(width, lane_h), Sense::hover());
                        let rect = Rect::from_min_max(
                            Pos2::new(rect.left() + GUTTER - KEY_W, rect.top()),
                            rect.max,
                        );
                        changed |= self.plocks.ui(ui, rect, cols, track, &selected);
                    }
                }
            });
        if !ui.input(|i| i.pointer.any_down()) {
            self.paint = None;
        }
        if position.is_some() {
            ui.ctx().request_repaint();
        }
        changed
    }

    fn reset_details(&mut self) {
        self.trig = TrigLane::default();
        self.plocks = PLockStrip::default();
    }

    pub fn cancel_gesture(&mut self) {
        self.paint = None;
        self.reset_details();
    }

    fn sync_details(&mut self, selection: Selection) {
        if self.detail_track != Some(selection) {
            self.detail_track = Some(selection);
            self.reveal_track = true;
            self.reset_details();
        }
    }

    fn matrix(
        &mut self,
        ui: &mut Ui,
        width: f32,
        cell_w: f32,
        session: &mut Session,
        selection: &mut Selection,
        roll: &mut PianoRoll,
        position: Option<f64>,
        recording: bool,
    ) -> bool {
        let Some(device) = session.devices.get(selection.device) else {
            return false;
        };
        let device_id = device.id;
        let Some(pattern) = session.current_pattern(device_id) else {
            return false;
        };
        let rows = pattern.num_tracks();
        let (_, full) = ui.allocate_space(Vec2::new(width, rows as f32 * ROW_H));
        if self.reveal_track && selection.track < rows {
            ui.scroll_to_rect(
                Rect::from_min_size(
                    Pos2::new(full.left(), full.top() + selection.track as f32 * ROW_H),
                    Vec2::new(width, ROW_H),
                ),
                None,
            );
            self.reveal_track = false;
        }
        let grid = Rect::from_min_max(Pos2::new(full.left() + GUTTER, full.top()), full.max);
        let response = ui.interact(
            grid,
            egui::Id::new("digi-drum-grid"),
            Sense::click_and_drag(),
        );
        let cell_at = |pos: Pos2| -> Option<(usize, usize)> {
            if !grid.contains(pos) {
                return None;
            }
            let row = ((pos.y - grid.top()) / ROW_H).floor() as usize;
            let col = ((pos.x - grid.left()) / cell_w).floor() as usize;
            (row < rows && col < PAGE).then_some((row, col))
        };
        let mut changed = false;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if response.is_pointer_button_down_on() {
            if let Some((row, col)) = pointer
                .filter(|p| ui.clip_rect().contains(*p))
                .and_then(cell_at)
            {
                let step = self.page * PAGE + col;
                let target = Selection {
                    device: selection.device,
                    track: row,
                };
                let pressed = ui.input(|i| i.pointer.any_pressed());
                if pressed {
                    // Drop TRACKS' clear-track focus before handling any note selection.
                    ui.ctx().memory_mut(|m| {
                        if let Some(id) = m.focused() {
                            m.surrender_focus(id);
                        }
                    });
                    if *selection != target {
                        roll.clear_selection();
                    }
                    *selection = target;
                    let track = tracks::track(session, target).unwrap();
                    if step < usize::from(track.length_steps) {
                        let ids = cell_notes(track, step);
                        if ui.input(|i| i.modifiers.shift) {
                            let mut selected = roll.selection();
                            if !ids.is_empty() && ids.iter().all(|id| selected.contains(id)) {
                                selected.retain(|id| !ids.contains(id));
                            } else {
                                selected.extend(
                                    ids.iter()
                                        .filter(|id| !selected.contains(id))
                                        .copied()
                                        .collect::<Vec<_>>(),
                                );
                            }
                            roll.select(selected);
                            if let Some(note) = track.notes.iter().find(|n| ids.contains(&n.id)) {
                                roll.set_default_velocity(note.velocity);
                            }
                        } else {
                            self.paint = Some(Paint {
                                row,
                                last: step,
                                add: ids.is_empty() && !ui.input(|i| i.pointer.secondary_down()),
                            });
                            roll.clear_selection();
                        }
                    }
                }
                if let Some(paint) = self.paint.as_mut().filter(|p| p.row == row) {
                    let track = tracks::track_mut(session, target).unwrap();
                    let pitch = *self
                        .pitches
                        .entry((device_id, row))
                        .or_insert_with(|| track.notes.first().map_or(60, |n| n.pitch));
                    for s in paint.last.min(step)..=paint.last.max(step) {
                        changed |= set_cell(track, s, paint.add, pitch, roll.default_velocity());
                    }
                    if paint.add {
                        let mut selected = roll.selection();
                        for s in paint.last.min(step)..=paint.last.max(step) {
                            for id in cell_notes(track, s) {
                                if !selected.contains(&id) {
                                    selected.push(id);
                                }
                            }
                        }
                        roll.select(selected);
                    }
                    paint.last = step;
                }
            }
        }

        for row in 0..rows {
            let target = Selection {
                device: selection.device,
                track: row,
            };
            let track = tracks::track(session, target).unwrap();
            let top = full.top() + row as f32 * ROW_H;
            let row_rect =
                Rect::from_min_size(Pos2::new(full.left(), top), Vec2::new(width, ROW_H));
            if !ui.is_rect_visible(row_rect) {
                continue;
            }
            let active = row == selection.track;
            let painter = ui.painter_at(ui.clip_rect().intersect(row_rect));
            painter.rect_filled(
                row_rect,
                0.0,
                if active {
                    super::CYAN_WASH
                } else {
                    super::PANEL_BG
                },
            );
            let name_rect = Rect::from_min_size(
                Pos2::new(full.left() + 3.0, top + 2.0),
                Vec2::new(105.0, ROW_H - 4.0),
            );
            let label = format!("{:02}  {}", row + 1, track.name);
            let selected_name = ui
                .put(
                    name_rect,
                    egui::Button::new(egui::RichText::new(label).size(12.0))
                        .selected(active)
                        .truncate(),
                )
                .on_hover_text(&track.name);
            let settings_rect =
                Rect::from_min_size(Pos2::new(full.left() + 110.0, top), Vec2::new(42.0, ROW_H));
            painter.text(
                settings_rect.center(),
                Align2::CENTER_CENTER,
                scale_text(track),
                FontId::proportional(11.0),
                super::TEXT_MUTED,
            );
            let mut mute = track.mute;
            let mute_rect = Rect::from_min_size(
                Pos2::new(full.left() + 157.0, top + 4.0),
                Vec2::new(25.0, ROW_H - 8.0),
            );
            let muted = ui
                .put(mute_rect, egui::Button::new("M").selected(mute))
                .on_hover_text("Mute this track")
                .clicked();
            let selected = roll.selection();
            for col in 0..PAGE {
                let step = self.page * PAGE + col;
                let cell = Rect::from_min_size(
                    Pos2::new(grid.left() + col as f32 * cell_w, top),
                    Vec2::new(cell_w, ROW_H),
                );
                let pad = cell.shrink2(Vec2::new(2.5, 4.0));
                let live = step < usize::from(track.length_steps);
                let notes: Vec<_> = track.notes.iter().filter(|n| in_cell(n, step)).collect();
                let occupied = !notes.is_empty();
                let fill = if !live {
                    super::INSET_BG
                } else if occupied {
                    if track.mute {
                        super::TEXT_DIMMEST
                    } else {
                        super::CYAN_FILL
                    }
                } else if col / 4 % 2 == 0 {
                    super::PANEL_BG_RAISED
                } else {
                    super::INSET_BG_HOVER
                };
                painter.rect_filled(pad, 3.0, fill);
                if !live {
                    painter.line_segment(
                        [pad.left_bottom(), pad.right_top()],
                        Stroke::new(1.0, super::PANEL_BORDER),
                    );
                } else if occupied {
                    let velocity = notes.iter().map(|n| n.velocity).max().unwrap_or(0);
                    let bar = Rect::from_min_size(
                        Pos2::new(pad.left() + 3.0, pad.bottom() - 4.0),
                        Vec2::new((pad.width() - 6.0) * f32::from(velocity) / 127.0, 2.0),
                    );
                    painter.rect_filled(bar, 0.0, super::CYAN_TEXT);
                    let marked = notes
                        .iter()
                        .any(|n| n.prob.is_some() || n.cond.is_some() || n.fill.is_some());
                    if marked {
                        painter.circle_filled(
                            pad.center_top() + Vec2::new(0.0, 4.0),
                            2.0,
                            super::TEXT_BRIGHT,
                        );
                    }
                    let label = if notes.len() > 1 {
                        notes.len().to_string()
                    } else if notes
                        .iter()
                        .any(|n| n.step.fract() != 0.0 || n.micro != 0.0)
                    {
                        "~".into()
                    } else {
                        String::new()
                    };
                    painter.text(
                        pad.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(10.0),
                        super::CYAN_TEXT,
                    );
                    if active && notes.iter().any(|n| selected.contains(&n.id)) {
                        painter.rect_stroke(
                            pad,
                            3.0,
                            Stroke::new(1.5, super::TEXT_BRIGHT),
                            StrokeKind::Inside,
                        );
                    }
                }
                if position.is_some_and(|p| live && play_step(track, p) == step) {
                    painter.rect_stroke(
                        pad.expand(1.0),
                        3.0,
                        Stroke::new(
                            1.5,
                            if recording && active {
                                Color32::LIGHT_RED
                            } else {
                                super::ACCENT
                            },
                        ),
                        StrokeKind::Inside,
                    );
                }
                if response.hovered() && pointer.is_some_and(|p| cell.contains(p)) {
                    let text = if !live {
                        "Outside this track's length".into()
                    } else if let Some(note) = notes.first() {
                        format!("Step {} · {} note(s) · {} · velocity {}\nClick removes this cell's notes. Shift-click selects.\n~ = timing offset; dot = trig rule; number = multiple notes.", step + 1, notes.len(), note_name(note.pitch), note.velocity)
                    } else {
                        format!("Step {} · click to add a note", step + 1)
                    };
                    response.clone().on_hover_text(text);
                }
            }
            if selected_name.clicked() {
                if *selection != target {
                    roll.clear_selection();
                }
                *selection = target;
            }
            if muted {
                mute = !mute;
                tracks::track_mut(session, target).unwrap().mute = mute;
                changed = true;
            }
        }
        changed
    }

    fn velocity(
        &self,
        ui: &mut Ui,
        rect: Rect,
        cols: Cols,
        track: &mut Track,
        selected: &[u32],
        roll: &mut PianoRoll,
    ) -> bool {
        let grid = Rect::from_min_max(Pos2::new(rect.left() + GUTTER, rect.top()), rect.max);
        let response = ui.interact(
            grid,
            egui::Id::new("digi-drum-velocity"),
            Sense::click_and_drag(),
        );
        let mut changed = false;
        if response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()) {
            ui.ctx().memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            if let Some(pos) = response
                .interact_pointer_pos()
                .filter(|p| grid.contains(*p))
            {
                let step = cols.step_at(pos.x) as usize;
                let ids = cell_notes(track, step);
                if step < usize::from(track.length_steps) && !ids.is_empty() {
                    let velocity = (((grid.bottom() - pos.y) / grid.height()) * 127.0)
                        .round()
                        .clamp(1.0, 127.0) as u8;
                    let group = ids.iter().any(|id| selected.contains(id));
                    for note in &mut track.notes {
                        if (group && selected.contains(&note.id)) || ids.contains(&note.id) {
                            changed |= note.velocity != velocity;
                            note.velocity = velocity;
                        }
                    }
                    roll.set_default_velocity(velocity);
                }
            }
        }
        let painter = ui.painter_at(rect);
        painter.text(
            Pos2::new(grid.left() - 9.0, rect.center().y),
            Align2::RIGHT_CENTER,
            "VELOCITY",
            FontId::proportional(10.0),
            super::TEXT_MUTED,
        );
        for col in 0..PAGE {
            let step = self.page * PAGE + col;
            let left = grid.left() + col as f32 * cols.cell_w;
            let cell = Rect::from_min_size(
                Pos2::new(left, grid.top()),
                Vec2::new(cols.cell_w, grid.height()),
            );
            painter.rect_filled(cell.shrink(1.0), 0.0, super::INSET_BG);
            if step >= usize::from(track.length_steps) {
                continue;
            }
            let notes: Vec<_> = track.notes.iter().filter(|n| in_cell(n, step)).collect();
            if let Some(velocity) = notes.iter().map(|n| n.velocity).max() {
                let h = ((grid.height() - 14.0) * f32::from(velocity) / 127.0).max(3.0);
                let bar = Rect::from_min_max(
                    Pos2::new(left + 5.0, grid.bottom() - h),
                    Pos2::new(left + cols.cell_w - 5.0, grid.bottom() - 2.0),
                );
                painter.rect_filled(
                    bar,
                    1.0,
                    if notes.iter().any(|n| selected.contains(&n.id)) {
                        super::CYAN
                    } else {
                        super::CYAN_FILL
                    },
                );
                painter.text(
                    Pos2::new(cell.center().x, grid.top() + 1.0),
                    Align2::CENTER_TOP,
                    velocity.to_string(),
                    FontId::proportional(10.0),
                    super::TEXT_SECONDARY,
                );
            }
        }
        response.on_hover_text(
            "Drag to set velocity. Touching a selected step applies it to the selection.",
        );
        changed
    }
}
