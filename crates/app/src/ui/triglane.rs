// The trig lane: a step-aligned strip under the piano roll for the three
// per-trig condition fields. Ported from `js/triglane.js`.
//
// These are properties of a *step*, not of a note — there is no track-level
// FILL or COND on the boxes at all, and notes sharing a step are one trig. So
// they get their own surface locked to the step grid rather than living in a
// selection panel, where they would read as "a property of the notes I picked".
//
// Rows, top to bottom: PROB, COND, FILL. A cell is live only where the step has
// notes; everything else is inert, because a condition on a step with no trig
// means nothing (the box scrubs those bytes when it creates a trig anyway).
//
// Interactions:
//   drag a PROB cell up/dn -> sets an explicit 0-100 lock; drag sideways to
//                             paint the same value across steps. Unlocked steps
//                             show the track's own PROB default, dimmed.
//   click a COND cell      -> grouped picker popover
//   click a FILL cell      -> cycles  none -> ON -> OFF -> none
//   drag sideways on COND/FILL -> paints the anchor cell's value across steps
//   right-click / alt-click any cell -> clears that field on the step
//
// Editing a step that holds selected notes applies to every selected step at
// once, which is how you get a condition onto a whole phrase in one go. (The
// roll selects one note today; the rule is ported whole so rubber-band
// selection plugs straight in.)
//
// Geometry comes from the roll: `pianoroll::ui` constructs the [`Cols`] handed
// here from the very `Grid` it draws and hit-tests with, so the two surfaces
// cannot drift apart — the same reason the JS lane imports `CELL_W`/`KEY_W`
// from `pianoroll.js`. The picker is built from `digi_protocol::conditions` —
// this module is Elektron-specific by nature, unlike the roll.
//
// The JS's `onBeforeEdit` undo hook has no Rust home yet: there is no undo
// system. Edits report `true` so the caller snapshots to the engine, exactly
// as the roll's do.

use digi_core::{Note, Track};
use digi_protocol::conditions;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Ui, Vec2};

use crate::ui::pianoroll::KEY_W;

pub const ROW_H: f32 = 18.0;
/// Row order, top to bottom. Everything else derives from this — drawing, hit
/// testing and the drag readout all index through it.
const ROWS: [Field; 3] = [Field::Prob, Field::Cond, Field::Fill];
pub const LANE_H: f32 = ROWS.len() as f32 * ROW_H;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Prob,
    Cond,
    Fill,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Prob => "PROB",
            Field::Cond => "COND",
            Field::Fill => "FILL",
        }
    }

    /// The pip colour that makes a set step readable at a glance, before you
    /// read the value itself.
    fn pip(self) -> Color32 {
        match self {
            Field::Prob => Color32::from_rgb(0x4f, 0x8f, 0xd0),
            Field::Cond => Color32::from_rgb(0x6a, 0xa8, 0x4f),
            Field::Fill => Color32::from_rgb(0xc9, 0x77, 0x2f),
        }
    }
}

/// One field's value, carried together so a single setter can hold the
/// step-uniformity rule for all three.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Prob(Option<u8>),
    Fill(Option<bool>),
    Cond(Option<String>),
}

impl FieldValue {
    /// This field's "nothing set" value — what right-click writes.
    fn cleared(field: Field) -> Self {
        match field {
            Field::Prob => FieldValue::Prob(None),
            Field::Fill => FieldValue::Fill(None),
            Field::Cond => FieldValue::Cond(None),
        }
    }
}

// --- The lane's rules, as plain functions -----------------------------------
//
// Kept off the widget so they can be tested without an egui context — and
// because the step-uniformity rule is the part that has to be right.
//
// Steps are `f64` because note positions are: the lane's own gestures only ever
// produce whole steps, but a selected note dragged to a fractional step must
// still round-trip through `target_steps` and match by exact equality, as the
// JS's `===` does. A fractional note is on no lane step at all.

/// The first note on `step`, which speaks for the trig — notes on a step are
/// always in agreement (`edit_ops::adopt_step_trig`).
pub fn step_note(notes: &[Note], step: f64) -> Option<&Note> {
    notes.iter().find(|n| n.step == step)
}

/// Which steps an edit at `step` reaches: the whole selection when the clicked
/// step is part of it, otherwise just that step.
pub fn target_steps(notes: &[Note], step: f64, selected: &[u32]) -> Vec<f64> {
    if !selected.is_empty()
        && notes.iter().any(|n| n.step == step && selected.contains(&n.id))
    {
        let mut steps: Vec<f64> = notes
            .iter()
            .filter(|n| selected.contains(&n.id))
            .map(|n| n.step)
            .collect();
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        steps.dedup();
        return steps;
    }
    vec![step]
}

/// Write one field across whole steps. Every note on a touched step gets the
/// value — the step-uniformity rule the encoder depends on. Returns whether
/// anything actually changed, so callers can skip a needless engine snapshot.
pub fn set_trig_field(notes: &mut [Note], steps: &[f64], value: &FieldValue) -> bool {
    let mut changed = false;
    for n in notes.iter_mut().filter(|n| steps.contains(&n.step)) {
        match value {
            FieldValue::Prob(v) if n.prob != *v => {
                n.prob = *v;
                changed = true;
            }
            FieldValue::Fill(v) if n.fill != *v => {
                n.fill = *v;
                changed = true;
            }
            FieldValue::Cond(v) if n.cond != *v => {
                n.cond = v.clone();
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

/// FILL is a tri-state, so clicking walks all three: none -> ON -> OFF -> none.
pub fn cycle_fill(current: Option<bool>) -> Option<bool> {
    match current {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

/// Vertical drag to probability. Up raises the odds, and the top of the range
/// is an explicit 100% lock rather than "no lock": once a track carries its own
/// PROB default, locking a trig back up to 100 is the only way to say "this one
/// always plays". Clearing a lock is the alt/right-click gesture.
///
/// `f64::round` is half-away-from-zero where JS `Math.round` is half-up; they
/// disagree only below zero, which the clamp erases either way.
pub fn prob_from_drag(start: Option<u8>, dy: f32) -> u8 {
    let raw = (start.unwrap_or(100) as f64 - dy as f64 / 2.0).round();
    raw.clamp(0.0, 100.0) as u8
}

/// Where the lane's step columns are on screen — the roll's `Grid`, reduced to
/// the one axis the lane shares.
#[derive(Debug, Clone, Copy)]
pub struct Cols {
    pub origin_x: f32,
    pub cell_w: f32,
}

impl Cols {
    /// `pub(crate)` since the p-lock strip landed: all three surfaces share
    /// these columns by construction, which is the point of `Cols` existing.
    pub(crate) fn x_of_step(&self, step: f64) -> f32 {
        self.origin_x + step as f32 * self.cell_w
    }

    pub(crate) fn step_at(&self, x: f32) -> f64 {
        ((x - self.origin_x) / self.cell_w).floor() as f64
    }
}

/// A press that may become a drag. egui's `Sense::click_and_drag` already
/// separates the two with its own movement threshold, which is the JS's
/// `DRAG_PX` check for free.
struct Drag {
    anchor_step: f64,
    origin: Pos2,
    /// The anchor cell's value when the press landed. A sideways drag paints
    /// this across steps — including a `None`, exactly as the JS paints the
    /// anchor's null.
    start: FieldValue,
    /// The live value while a PROB drag is moving, for the readout.
    prob_now: Option<u8>,
}

struct Picker {
    anchor_step: f64,
    steps: Vec<f64>,
    tab: Tab,
    /// True on the frame the picker was created. The click that opens it lands
    /// outside the picker's own rect — the picker does not exist yet — so
    /// `clicked_elsewhere` sees that same click and would close the picker on
    /// the frame it opened. It shipped that way: the picker was never visible,
    /// and every test passed. One armed frame is the difference.
    just_opened: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Logic,
    Denom(u8),
}

#[derive(Default)]
pub struct TrigLane {
    drag: Option<Drag>,
    picker: Option<Picker>,
}

impl TrigLane {
    /// Draw and edit the lane for `track` in `rect`, columns shared with the
    /// roll via `cols`. `selected` is the roll's selection, by note id.
    ///
    /// Returns whether the track changed, so the caller can snapshot to the
    /// engine only when there is something to send.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        cols: Cols,
        track: &mut Track,
        selected: &[u32],
    ) -> bool {
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let mut changed = false;

        // -- interaction, before painting, so what is drawn is this frame's --

        let alt = ui.input(|i| i.modifiers.alt);

        // Uniform "take the lock off" gesture, whichever row.
        if response.secondary_clicked() || (response.clicked() && alt) {
            self.picker = None;
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some((field, step)) = live_cell(rect, cols, track, pos) {
                    let steps = target_steps(&track.notes, step, selected);
                    changed |=
                        set_trig_field(&mut track.notes, &steps, &FieldValue::cleared(field));
                }
            }
        }

        if response.drag_started() {
            self.picker = None;
            self.drag = response.interact_pointer_pos().and_then(|pos| {
                let (field, step) = live_cell(rect, cols, track, pos)?;
                let note = step_note(&track.notes, step)?;
                let start = match field {
                    Field::Prob => FieldValue::Prob(note.prob),
                    Field::Fill => FieldValue::Fill(note.fill),
                    Field::Cond => FieldValue::Cond(note.cond.clone()),
                };
                Some(Drag { anchor_step: step, origin: pos, start, prob_now: None })
            });
        }

        if let Some(drag) = &mut self.drag {
            if let Some(pos) = response.interact_pointer_pos() {
                let value = match &drag.start {
                    FieldValue::Prob(start) => {
                        let v = prob_from_drag(*start, pos.y - drag.origin.y);
                        drag.prob_now = Some(v);
                        FieldValue::Prob(Some(v))
                    }
                    // Sideways on COND/FILL paints the anchor cell's value.
                    other => other.clone(),
                };
                // Paint across every live step the cursor has passed over.
                let (from, to) = order(drag.anchor_step, cols.step_at(pos.x));
                let mut steps: Vec<f64> = (from as i64..=to as i64)
                    .map(|s| s as f64)
                    .filter(|s| step_note(&track.notes, *s).is_some())
                    .collect();
                if steps.is_empty() {
                    steps.push(drag.anchor_step);
                }
                changed |= set_trig_field(&mut track.notes, &steps, &value);
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }

        // A click, not a drag. A bare click on PROB does nothing: it is a drag
        // control, and a click that silently changed the odds would be a nasty
        // surprise.
        if response.clicked() && !alt {
            self.picker = None;
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some((field, step)) = live_cell(rect, cols, track, pos) {
                    let steps = target_steps(&track.notes, step, selected);
                    match field {
                        Field::Fill => {
                            let current = step_note(&track.notes, step).and_then(|n| n.fill);
                            changed |= set_trig_field(
                                &mut track.notes,
                                &steps,
                                &FieldValue::Fill(cycle_fill(current)),
                            );
                        }
                        Field::Cond => {
                            let current =
                                step_note(&track.notes, step).and_then(|n| n.cond.clone());
                            self.picker = Some(Picker {
                                anchor_step: step,
                                steps,
                                tab: initial_tab(current.as_deref()),
                                just_opened: true,
                            });
                        }
                        Field::Prob => {}
                    }
                }
            }
        }

        self.paint(ui, rect, cols, track, selected);
        changed |= self.picker_ui(ui, rect, cols, track);
        changed
    }

    // --- drawing -------------------------------------------------------------

    fn paint(&self, ui: &Ui, rect: Rect, cols: Cols, track: &Track, selected: &[u32]) {
        let painter = ui.painter_at(rect);
        let font = FontId::proportional(9.0);

        // What an unlocked trig actually runs at. Only worth drawing when the
        // track isn't at the box default — a lane of dimmed "100"s says nothing.
        let track_prob = track.track_prob;
        let selected_steps: Vec<f64> = track
            .notes
            .iter()
            .filter(|n| selected.contains(&n.id))
            .map(|n| n.step)
            .collect();

        for (r, field) in ROWS.iter().enumerate() {
            let y = rect.min.y + r as f32 * ROW_H;
            for s in 0..track.length_steps {
                let step = s as f64;
                let x = cols.x_of_step(step);
                if x + cols.cell_w < rect.min.x + KEY_W || x > rect.max.x {
                    continue;
                }
                let note = step_note(&track.notes, step);
                let value = note.map(|n| match field {
                    Field::Prob => n.prob.map(|p| p.to_string()),
                    Field::Fill => n.fill.map(|f| if f { "ON" } else { "OFF" }.to_owned()),
                    Field::Cond => n.cond.clone(),
                });

                let cell = Rect::from_min_size(Pos2 { x, y }, Vec2::new(cols.cell_w, ROW_H));
                let bg = match &value {
                    None => Color32::from_rgb(0x15, 0x17, 0x1b),
                    Some(_) if selected_steps.contains(&step) => {
                        Color32::from_rgb(0x25, 0x2a, 0x34)
                    }
                    Some(Some(_)) => Color32::from_rgb(0x20, 0x24, 0x2c),
                    Some(None) => Color32::from_rgb(0x1b, 0x1e, 0x24),
                };
                painter.rect_filled(cell, 0.0, bg);

                let centre = Pos2 { x: x + cols.cell_w / 2.0, y: y + ROW_H / 2.0 };
                match &value {
                    Some(Some(text)) => {
                        // A filled pip makes a set step readable at a glance,
                        // before you read the value itself.
                        painter.rect_filled(
                            Rect::from_min_size(Pos2 { x, y }, Vec2::new(cols.cell_w, 2.0)),
                            0.0,
                            field.pip(),
                        );
                        painter.text(
                            centre,
                            Align2::CENTER_CENTER,
                            text,
                            font.clone(),
                            Color32::from_rgb(0xe4, 0xe7, 0xec),
                        );
                    }
                    Some(None) if *field == Field::Prob && track_prob != 100 => {
                        // Inherited, not stored: this trig has no lock of its own
                        // and runs at the track's odds. Dimmed so it never reads
                        // as a lock.
                        painter.text(
                            centre,
                            Align2::CENTER_CENTER,
                            track_prob.to_string(),
                            font.clone(),
                            Color32::from_rgb(0x59, 0x60, 0x6d),
                        );
                    }
                    Some(None) => {
                        painter.text(
                            centre,
                            Align2::CENTER_CENTER,
                            "·",
                            font.clone(),
                            Color32::from_rgb(0x3a, 0x40, 0x50),
                        );
                    }
                    None => {}
                }

                // Step gridlines, on the roll's own beat/bar rhythm.
                let stroke = egui::Stroke::new(
                    1.0,
                    match () {
                        _ if s % 16 == 0 => Color32::from_rgb(0x4a, 0x50, 0x60),
                        _ if s % 4 == 0 => Color32::from_rgb(0x34, 0x39, 0x45),
                        _ => Color32::from_rgb(0x26, 0x2a, 0x33),
                    },
                );
                painter.line_segment(
                    [Pos2 { x, y }, Pos2 { x, y: y + ROW_H }],
                    stroke,
                );
            }
            painter.line_segment(
                [Pos2 { x: rect.min.x, y }, Pos2 { x: rect.max.x, y }],
                egui::Stroke::new(1.0, Color32::from_rgb(0x26, 0x2a, 0x33)),
            );
        }

        // Row labels, in the gutter that lines up with the roll's key column.
        // Drawn last so cells scrolled under it disappear rather than collide.
        let gutter = Rect::from_min_size(rect.min, Vec2::new(KEY_W, LANE_H));
        painter.rect_filled(gutter, 0.0, Color32::from_rgb(0x17, 0x1a, 0x20));
        for (r, field) in ROWS.iter().enumerate() {
            painter.text(
                Pos2 { x: rect.min.x + 6.0, y: rect.min.y + r as f32 * ROW_H + ROW_H / 2.0 },
                Align2::LEFT_CENTER,
                field.label(),
                font.clone(),
                Color32::from_rgb(0x7d, 0x85, 0x90),
            );
        }
        painter.line_segment(
            [
                Pos2 { x: rect.min.x + KEY_W, y: rect.min.y },
                Pos2 { x: rect.min.x + KEY_W, y: rect.min.y + LANE_H },
            ],
            egui::Stroke::new(1.0, Color32::from_rgb(0x2a, 0x2e, 0x38)),
        );

        // Live readout while dragging probability. The JS draws it inside the
        // PROB row, but our cells are half that size and already hold the value
        // text — the two collided into an unreadable smear, found by hand-
        // testing. So it is a badge floating just above the lane instead, on
        // the unclipped painter, big enough to read mid-gesture.
        if let Some(drag) = &self.drag {
            if let Some(v) = drag.prob_now {
                let x = cols.x_of_step(drag.anchor_step).max(rect.min.x + KEY_W);
                let badge = Rect::from_min_size(
                    Pos2 { x, y: rect.min.y - 22.0 },
                    Vec2::new(44.0, 18.0),
                );
                let over = ui.painter();
                over.rect_filled(badge, 4.0, Color32::from_rgb(0x2a, 0x2e, 0x38));
                over.rect_stroke(
                    badge,
                    4.0,
                    egui::Stroke::new(1.0, Color32::from_rgb(0x4a, 0x50, 0x60)),
                    egui::StrokeKind::Middle,
                );
                over.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    format!("{v}%"),
                    FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
        }
    }

    // --- the COND picker -------------------------------------------------------

    /// The picker floats above the clicked column. Applying writes through the
    /// same setter as everything else; clicking anywhere off it closes it.
    fn picker_ui(&mut self, ui: &mut Ui, rect: Rect, cols: Cols, track: &mut Track) -> bool {
        let Some(picker) = &mut self.picker else {
            return false;
        };
        let just_opened = std::mem::take(&mut picker.just_opened);
        let current = step_note(&track.notes, picker.anchor_step).and_then(|n| n.cond.clone());

        // `Some(new value)` once a button is pressed; the borrow on `picker`
        // has to end before the write.
        let mut apply: Option<Option<String>> = None;
        let x = cols
            .x_of_step(picker.anchor_step)
            .clamp(rect.min.x + KEY_W, (rect.max.x - 280.0).max(rect.min.x + KEY_W));

        let area = egui::Area::new(ui.id().with("trig cond picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2 { x, y: rect.min.y - 4.0 })
            .pivot(Align2::LEFT_BOTTOM)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(280.0);
                    let head = if picker.steps.len() > 1 {
                        format!("Condition · {} steps", picker.steps.len())
                    } else {
                        // 1-based, as the boxes count steps.
                        format!("Condition · step {}", picker.anchor_step as i64 + 1)
                    };
                    ui.label(egui::RichText::new(head).small().strong());
                    ui.separator();

                    ui.horizontal(|ui| {
                        if ui.selectable_label(picker.tab == Tab::Logic, "Logic").clicked() {
                            picker.tab = Tab::Logic;
                        }
                        for group in conditions::cond_by_denominator() {
                            let tab = Tab::Denom(group.b);
                            if ui
                                .selectable_label(picker.tab == tab, format!(":{}", group.b))
                                .clicked()
                            {
                                picker.tab = tab;
                            }
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        let items: Vec<&conditions::CondEntry> = match picker.tab {
                            Tab::Logic => conditions::conditions()
                                .iter()
                                .filter(|c| c.group == conditions::CondGroup::Logic)
                                .collect(),
                            Tab::Denom(b) => conditions::cond_by_denominator()
                                .iter()
                                .find(|g| g.b == b)
                                .map(|g| g.items.clone())
                                .unwrap_or_default(),
                        };
                        for c in items {
                            let on = current.as_deref() == Some(c.key.as_str());
                            if ui
                                .selectable_label(on, &c.key)
                                .on_hover_text(conditions::cond_description(&c.key))
                                .clicked()
                            {
                                apply = Some(Some(c.key.clone()));
                            }
                        }
                    });

                    if ui.selectable_label(current.is_none(), "— none —").clicked() {
                        apply = Some(None);
                    }
                });
            });

        let steps = picker.steps.clone();
        if let Some(value) = apply {
            self.picker = None;
            return set_trig_field(&mut track.notes, &steps, &FieldValue::Cond(value));
        }
        if !just_opened && area.response.clicked_elsewhere() {
            self.picker = None;
        }
        false
    }
}

/// The live cell under `pos`: its row's field and its whole step. `None` in the
/// gutter, past the pattern's end, and on a step with no notes — a condition on
/// a step with no trig means nothing.
fn live_cell(rect: Rect, cols: Cols, track: &Track, pos: Pos2) -> Option<(Field, f64)> {
    let (field, step) = cell_at(rect, cols, track.length_steps, pos)?;
    step_note(&track.notes, step).is_some().then_some((field, step))
}

/// The cell under `pos`, live or not.
fn cell_at(rect: Rect, cols: Cols, length_steps: u16, pos: Pos2) -> Option<(Field, f64)> {
    if pos.x < rect.min.x + KEY_W {
        return None;
    }
    let step = cols.step_at(pos.x);
    let row = ((pos.y - rect.min.y) / ROW_H).floor();
    let in_grid = (0.0..length_steps as f64).contains(&step) && (0.0..ROWS.len() as f32).contains(&row);
    in_grid.then(|| (ROWS[row as usize], step))
}

/// Which tab the picker opens on: the one holding the current value, so
/// re-picking is one click; the first tab otherwise.
fn initial_tab(current: Option<&str>) -> Tab {
    current
        .and_then(|key| conditions::conditions().iter().find(|c| c.key == key))
        .and_then(|c| c.ab)
        .map(|(_, b)| Tab::Denom(b))
        .unwrap_or(Tab::Logic)
}

fn order(a: f64, b: f64) -> (f64, f64) {
    if a <= b { (a, b) } else { (b, a) }
}

#[cfg(test)]
mod tests {
    // Expected values are derived from the JS oracle — `stepValue`,
    // `targetSteps`, `setTrigField`, `cycleFill` and `probFromDrag` in
    // `js/triglane.js`, run under node — not re-reasoned.
    use super::*;
    use digi_core::TrackKind;

    fn note(id_step: f64, pitch: u8) -> Note {
        Note::new(id_step, pitch, 1.0, 100, 0.0)
    }

    #[test]
    fn a_drag_moves_probability_at_half_a_percent_per_pixel_and_clamps() {
        // (start, dy, expected) straight from the oracle.
        for (start, dy, want) in [
            (None, 0.0, 100),
            (None, -20.0, 100),
            (Some(100), 50.0, 75),
            (Some(50), -200.0, 100),
            (Some(50), 200.0, 0),
            (Some(0), -1.0, 1),
            (Some(100), 1.0, 100),
            (None, 199.0, 1),
            (None, 201.0, 0),
            (Some(73), 3.0, 72),
        ] {
            assert_eq!(prob_from_drag(start, dy), want, "start {start:?} dy {dy}");
        }
    }

    #[test]
    fn clicking_fill_walks_the_tri_state() {
        assert_eq!(cycle_fill(None), Some(true));
        assert_eq!(cycle_fill(Some(true)), Some(false));
        assert_eq!(cycle_fill(Some(false)), None);
    }

    #[test]
    fn an_edit_reaches_the_selection_only_through_a_selected_step() {
        let notes = vec![note(0.0, 60), note(0.0, 64), note(4.0, 60), note(8.0, 60)];
        let ids: Vec<u32> = notes.iter().map(|n| n.id).collect();

        // The clicked step holds a selected note: every selected step, sorted.
        assert_eq!(target_steps(&notes, 4.0, &[ids[0], ids[2]]), vec![0.0, 4.0]);
        // The clicked step is outside the selection: just that step.
        assert_eq!(target_steps(&notes, 8.0, &[ids[0], ids[2]]), vec![8.0]);
        // No selection at all: just that step.
        assert_eq!(target_steps(&notes, 4.0, &[]), vec![4.0]);
        // Two selected notes on one step: the step once, not twice.
        assert_eq!(target_steps(&notes, 0.0, &[ids[0], ids[1]]), vec![0.0]);
    }

    #[test]
    fn a_write_stamps_every_note_on_the_step_and_reports_a_second_one_as_a_no_op() {
        let mut notes = vec![note(0.0, 60), note(0.0, 64), note(4.0, 60)];
        assert!(set_trig_field(&mut notes, &[0.0], &FieldValue::Prob(Some(30))));
        assert_eq!(
            notes.iter().map(|n| n.prob).collect::<Vec<_>>(),
            vec![Some(30), Some(30), None],
            "both notes on the step agree; the untouched step is untouched"
        );
        assert!(
            !set_trig_field(&mut notes, &[0.0], &FieldValue::Prob(Some(30))),
            "writing the value already there is not a change"
        );
        assert!(set_trig_field(&mut notes, &[0.0], &FieldValue::Prob(None)), "clearing is");
    }

    #[test]
    fn the_first_note_on_a_step_speaks_for_the_trig_and_a_fractional_note_for_none() {
        let mut notes = vec![note(0.0, 60), note(4.5, 60)];
        notes[0].prob = Some(40);
        assert_eq!(step_note(&notes, 0.0).and_then(|n| n.prob), Some(40));
        // Exact equality, as the JS's `===`: a note at 4.5 is on no lane step.
        assert_eq!(step_note(&notes, 4.0).map(|n| n.id), None);
        assert_eq!(step_note(&notes, 5.0).map(|n| n.id), None);
    }

    #[test]
    fn columns_map_positions_to_the_cells_they_are_drawn_in() {
        // Scrolled somewhere awkward, like the roll's own grid test.
        let cols = Cols { origin_x: 63.0 - 37.0, cell_w: 20.0 };
        for step in [0.0, 1.0, 7.0, 63.0] {
            let x = cols.x_of_step(step);
            assert_eq!(cols.step_at(x), step, "the left edge belongs to its own step");
            assert_eq!(cols.step_at(x + 19.9), step);
            assert_eq!(cols.step_at(x + 20.0), step + 1.0);
        }
    }

    #[test]
    fn the_hit_test_knows_its_rows_its_gutter_and_the_patterns_end() {
        let rect = Rect::from_min_size(Pos2 { x: 10.0, y: 200.0 }, Vec2::new(600.0, LANE_H));
        let cols = Cols { origin_x: 10.0 + KEY_W, cell_w: 20.0 };

        let hit = |x: f32, y: f32| cell_at(rect, cols, 16, Pos2 { x, y });
        let cell0 = 10.0 + KEY_W + 1.0;
        assert_eq!(hit(cell0, 201.0), Some((Field::Prob, 0.0)));
        assert_eq!(hit(cell0, 200.0 + ROW_H + 1.0), Some((Field::Cond, 0.0)));
        assert_eq!(hit(cell0, 200.0 + 2.0 * ROW_H + 1.0), Some((Field::Fill, 0.0)));
        assert_eq!(hit(cell0 + 20.0, 201.0), Some((Field::Prob, 1.0)));

        assert_eq!(hit(10.0 + KEY_W - 2.0, 201.0), None, "the gutter is not a cell");
        assert_eq!(hit(cell0, 200.0 + LANE_H + 1.0), None, "below the lane");
        assert_eq!(hit(cell0 + 16.0 * 20.0, 201.0), None, "past the pattern's end");
    }

    #[test]
    fn a_condition_edit_only_lands_on_a_step_that_has_a_trig() {
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![note(2.0, 60)];
        let rect = Rect::from_min_size(Pos2 { x: 0.0, y: 0.0 }, Vec2::new(600.0, LANE_H));
        let cols = Cols { origin_x: KEY_W, cell_w: 20.0 };

        let on_trig = Pos2 { x: KEY_W + 2.0 * 20.0 + 1.0, y: 1.0 };
        let off_trig = Pos2 { x: KEY_W + 5.0 * 20.0 + 1.0, y: 1.0 };
        assert!(live_cell(rect, cols, &track, on_trig).is_some());
        assert!(live_cell(rect, cols, &track, off_trig).is_none());
    }

    /// One headless egui frame: feed `events`, draw the lane, return.
    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        lane: &mut TrigLane,
        track: &mut Track,
        rect: Rect,
        cols: Cols,
    ) {
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            lane.ui(ui, rect, cols, track, &[]);
        });
        // No renderer here to apply the font-atlas delta to, and epaint's debug
        // assert refuses to let it drop unhandled.
        output.textures_delta.clear();
    }

    #[test]
    fn the_cond_picker_survives_the_click_that_opened_it() {
        // The first simulated click in this repo. The picker shipped opening
        // and closing on the same frame: the click that creates it lands
        // outside a rect that does not exist yet, so `clicked_elsewhere` saw
        // that same click and closed it before it was ever drawn — found by
        // hand-testing, invisible to every pure-rule test.
        let ctx = egui::Context::default();
        let mut lane = TrigLane::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![note(0.0, 60)];
        let rect = Rect::from_min_size(Pos2 { x: 0.0, y: 100.0 }, Vec2::new(400.0, LANE_H));
        let cols = Cols { origin_x: KEY_W, cell_w: 20.0 };
        // The COND row of step 0.
        let pos = Pos2 { x: KEY_W + 10.0, y: 100.0 + ROW_H + 9.0 };
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        // egui hit-tests against the previous pass's layout, so the lane has to
        // have been drawn once before a press can land on it.
        frame(&ctx, vec![], &mut lane, &mut track, rect, cols);
        frame(&ctx, vec![egui::Event::PointerMoved(pos), press(true)], &mut lane, &mut track, rect, cols);
        frame(&ctx, vec![press(false)], &mut lane, &mut track, rect, cols);
        assert!(lane.picker.is_some(), "the release opens the picker");

        frame(&ctx, vec![], &mut lane, &mut track, rect, cols);
        assert!(lane.picker.is_some(), "and it survives past the frame that opened it");

        // A later click somewhere else — the roll, a panel — still closes it.
        // Well clear of the picker itself, which floats above the lane and is
        // up to ~340 px wide from the anchor column.
        let away = Pos2 { x: 600.0, y: 30.0 };
        let press_away = |pressed| egui::Event::PointerButton {
            pos: away,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, vec![egui::Event::PointerMoved(away), press_away(true)], &mut lane, &mut track, rect, cols);
        frame(&ctx, vec![press_away(false)], &mut lane, &mut track, rect, cols);
        assert!(lane.picker.is_none(), "a click elsewhere closes it");
    }

    #[test]
    fn every_condition_the_picker_offers_is_one_the_engine_can_evaluate() {
        // The picker's vocabulary is `digi_protocol::conditions`; the engine's
        // is `Cond::parse`. If they ever disagree, a menu pick would fall into
        // the "unrecognised plays" rule silently.
        for c in conditions::conditions() {
            assert!(
                digi_engine::Cond::parse(&c.key).is_some(),
                "{} is in the menu but the engine cannot parse it",
                c.key
            );
        }
    }
}
