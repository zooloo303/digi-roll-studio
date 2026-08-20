// The p-lock strip: stacked automation rows under the trig lane, one per lane.
//
// Ported from `js/plocklane.js`. Same skeleton as the trig lane above it —
// step-aligned hit testing, drag-painting, selection-aware edits — but a bar
// graph rather than a text cell, because a p-lock is a value in a range and the
// shape of a filter sweep is the thing you actually want to see.
//
// Rows are taller than the trig lane's 18 px for the same reason: a bar you can
// aim at is worth the pixels. One row per lane on the track, stacked in lane
// order, and the whole strip is absent when a track has no lanes.
//
// Editing, on a lane whose parameter we know:
//
// ```text
//   press in a cell   -> sets that step's value from where you pressed
//                        (absolute, not relative: the bar follows the pointer,
//                        which is what a bar graph implies)
//   drag up/down      -> keeps setting it as you move
//   drag sideways     -> paints the same value across the live steps passed over
//   alt/right-click   -> clears the lock on that step
//   with a selection  -> a press reaches every selected step
// ```
//
// **Two kinds of lane are read-only**, drawn dimmed and hatched, and say why:
//
// * a lane whose `param_id` is in no curated table. Phase 0 measured eleven
//   parameters per box, and a box can p-lock far more knobs than that, so lanes
//   captured off hardware still land here. The app can see such a lane, name the
//   byte and carry it through byte-exact; it cannot honestly draw "cutoff 64" or
//   let you drag it, because it does not know which parameter it is.
// * a lane the box filled on a step with no trig — a trigless lock. v1 does not
//   model those, and passing it through untouched keeps what the box has instead
//   of editing it into something else.
//
// **Values are on the parameter's display axis**, which is where
// `core::import` converts them to. That is the same axis `core::audition` sends
// from, so what the bar says and what the box hears cannot drift.
//
// Geometry comes from the roll, through the same [`Cols`] the trig lane uses, so
// all three surfaces share their step columns by construction.

use digi_core::model::{PLockLane, Track};
use digi_protocol::params::ParamDesc;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::ui::pianoroll::KEY_W;
use crate::ui::triglane::{target_steps, Cols};

/// Taller than the trig lane's row: a bar you have to aim at is worth the
/// pixels, and this one is dragged rather than clicked.
pub const ROW_H: f32 = 30.0;

/// Headroom above a full bar, so "at maximum" does not read as "clipped by the
/// row above". Part of the geometry contract between drawing and dragging: the
/// pointer sits on the top edge of the bar it is setting, so a full bar's top
/// edge is `BAR_PAD` from the top of the row and that is where the maximum
/// value lives.
const BAR_PAD: f32 = 4.0;

/// Each editable lane gets its own hue, by row order — adjacent lanes always
/// differ, which is what tells three stacked bar graphs apart at a glance. Grey
/// is reserved for read-only lanes, so colour keeps meaning "you can drag this".
const LANE_COLORS: [(Color32, Color32); 6] = [
    (Color32::from_rgb(0x8d, 0x6f, 0xd1), Color32::from_rgb(0xc9, 0xb6, 0xf2)), // purple
    (Color32::from_rgb(0x4a, 0xa8, 0xa0), Color32::from_rgb(0xa8, 0xde, 0xd9)), // teal
    (Color32::from_rgb(0xc9, 0x97, 0x4a), Color32::from_rgb(0xed, 0xd2, 0xa8)), // amber
    (Color32::from_rgb(0x5b, 0x8d, 0xd6), Color32::from_rgb(0xb3, 0xcc, 0xf0)), // blue
    (Color32::from_rgb(0xc7, 0x6a, 0x8d), Color32::from_rgb(0xea, 0xb6, 0xca)), // rose
    (Color32::from_rgb(0x7a, 0xa8, 0x4a), Color32::from_rgb(0xc8, 0xe0, 0xa8)), // green
];

const READ_ONLY_BAR: Color32 = Color32::from_gray(70);
const READ_ONLY_CAP: Color32 = Color32::from_gray(105);

pub fn lane_color(row: usize) -> (Color32, Color32) {
    LANE_COLORS[row % LANE_COLORS.len()]
}

/// How tall the strip wants to be for this track. Zero when it has no lanes,
/// which is how the strip disappears rather than leaving an empty band under the
/// roll.
///
/// **What it gets may be less.** A fully p-locked track holds eleven lanes —
/// the Phase 0 captures do — and eleven rows plus the trig lane would leave the
/// roll a sliver. The caller clamps; [`rows_that_fit`] is where the strip finds
/// out, and it says how many it had to hide rather than truncating in silence.
pub fn strip_height(track: &Track) -> f32 {
    track.plocks.len() as f32 * ROW_H
}

/// How many rows fit in `rect`, never more than there are lanes.
///
/// Every part of the strip goes through this — drawing, hit testing and the
/// overflow note — so a row you cannot see is a row you cannot edit. A strip that
/// let you drag an invisible lane would be worse than one that hides it.
fn rows_that_fit(rect: Rect, lanes: usize) -> usize {
    lanes.min((rect.height() / ROW_H).floor().max(0.0) as usize)
}

// --- The lane's rules, as plain functions -----------------------------------
//
// Kept off the widget so they can be tested without an egui context. The
// read-only rule is the one that has to be right: everything else is pixels, but
// that one decides whether the app edits bytes it does not understand.

/// The parameter a lane automates — curated when we know which knob it is, and a
/// raw stand-in when we do not. Never absent: a lane always draws and always
/// labels itself.
///
/// The resolution itself is `PLockLane::param`, in `core`, because the write path
/// has to name a lane it is refusing in exactly the words the strip labels it
/// with. This stays as the name the widget reads by.
pub fn lane_param(lane: &PLockLane) -> ParamDesc {
    lane.param()
}

/// May this lane be edited? Only when we know which parameter it is, and only
/// when the box was not holding trigless values in it.
///
/// **Not the same question as "can it be written to the box"** — a curated
/// parameter can be drawn and heard while its slot in the pattern format is
/// unmeasured. [`ParamDesc::writable`] is that other question, and the two are
/// deliberately separate.
pub fn lane_is_editable(lane: &PLockLane) -> bool {
    lane_param(lane).curated && !lane.trigless
}

/// Why a lane cannot be edited, for the tooltip. `None` when it can be.
pub fn lane_read_only_reason(lane: &PLockLane) -> Option<String> {
    let p = lane_param(lane);
    if !p.curated {
        return Some(format!(
            "{} isn't a parameter this app has mapped, so the lane is shown read-only \
             and written back to the box exactly as it came",
            p.label
        ));
    }
    if lane.trigless {
        return Some(format!(
            "{} has locks on steps with no trig, which this app doesn't edit — \
             the lane is written back exactly as it came",
            p.label
        ));
    }
    None
}

/// A y position inside a row → a value on the parameter's axis.
///
/// Absolute: the top of the row is the parameter's maximum and the bottom its
/// minimum, which is what a bar graph promises. Clamped onto the parameter's own
/// resolution, so every position the pointer can reach is a value the box can
/// hold.
pub fn value_from_row_y(param: &ParamDesc, y_in_row: f32, row_h: f32) -> i32 {
    let usable = (row_h - BAR_PAD).max(1.0);
    let frac = 1.0 - ((y_in_row - BAR_PAD) / usable).clamp(0.0, 1.0);
    param.clamp_value(param.min as f64 + f64::from(frac) * f64::from(param.max - param.min))
}

/// How tall a bar is, 0..1, for a display value.
pub fn bar_fraction(param: &ParamDesc, value: u16) -> f32 {
    let span = param.max - param.min;
    if span <= 0 {
        return 0.0;
    }
    ((f32::from(value) - param.min as f32) / span as f32).clamp(0.0, 1.0)
}

/// Write one display value across whole steps of a lane. Returns whether
/// anything changed, so a gesture that moved nothing leaves nothing to send.
pub fn set_lane_value(lane: &mut PLockLane, steps: &[f64], value: i32) -> bool {
    let v = lane_param(lane).clamp_value(f64::from(value)).max(0) as u16;
    let mut changed = false;
    for &step in steps {
        let Some(slot) = step_slot(lane, step) else { continue };
        if lane.values[slot] != Some(v) {
            lane.values[slot] = Some(v);
            changed = true;
        }
    }
    changed
}

/// Take the locks off whole steps.
pub fn clear_lane_value(lane: &mut PLockLane, steps: &[f64]) -> bool {
    let mut changed = false;
    for &step in steps {
        let Some(slot) = step_slot(lane, step) else { continue };
        if lane.values[slot].is_some() {
            lane.values[slot] = None;
            changed = true;
        }
    }
    changed
}

/// Which entry of `values` a step addresses, or `None` for a step no lane can
/// hold. A lane is indexed by whole steps; a note dragged to a fractional step
/// is on no lane step at all, the same rule the trig lane follows.
fn step_slot(lane: &PLockLane, step: f64) -> Option<usize> {
    if step < 0.0 || step.fract() != 0.0 {
        return None;
    }
    let slot = step as usize;
    (slot < lane.values.len()).then_some(slot)
}

/// A lane's one-line summary: `FLTR CUTOFF · 6 steps`.
///
/// A lane that can be edited but not stored in a pattern says so. Since Phase 0
/// measured every curated parameter that state should not occur — it survives as
/// the honest label for a lane whose name ever stops resolving to a measured
/// slot, rather than letting such a lane silently fail to travel.
pub fn describe_lane(lane: &PLockLane) -> String {
    let param = lane_param(lane);
    let n = lane.values.iter().filter(|v| v.is_some()).count();
    let state = if !lane_is_editable(lane) {
        " · read-only"
    } else if !param.writable() {
        " · preview only"
    } else {
        ""
    };
    format!("{} · {n} step{}{state}", param.label, if n == 1 { "" } else { "s" })
}

/// Which lane row a point is in, and where inside it.
fn row_at(rect: Rect, lanes: usize, pos: Pos2) -> Option<(usize, f32)> {
    if !rect.contains(pos) {
        return None;
    }
    let row = ((pos.y - rect.min.y) / ROW_H).floor();
    if row < 0.0 || row as usize >= lanes {
        return None;
    }
    let row = row as usize;
    Some((row, pos.y - rect.min.y - row as f32 * ROW_H))
}

/// The steps that have trigs. A lock with no trig to ride on is the trigless
/// case v1 does not author, so a cell is only live where a note sits.
fn live_steps(track: &Track) -> Vec<f64> {
    let mut v: Vec<f64> = track.notes.iter().map(|n| n.step).collect();
    v.sort_by(f64::total_cmp);
    v.dedup();
    v
}

struct Drag {
    row: usize,
    anchor_step: f64,
}

#[derive(Default)]
pub struct PLockStrip {
    drag: Option<Drag>,
}

impl PLockStrip {
    /// Draw and edit `track`'s lanes in `rect`, columns shared with the roll.
    /// Returns whether the track changed.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        cols: Cols,
        track: &mut Track,
        selected: &[u32],
    ) -> bool {
        if track.plocks.is_empty() {
            self.drag = None;
            return false;
        }
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let mut changed = false;
        let alt = ui.input(|i| i.modifiers.alt);
        let lanes = rows_that_fit(rect, track.plocks.len());
        if lanes == 0 {
            self.drag = None;
            return false;
        }
        let live = live_steps(track);
        let notes = track.notes.clone();

        // -- interaction, before painting, so what is drawn is this frame's --

        if response.secondary_clicked() || (response.clicked() && alt) {
            self.drag = None;
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some((row, _)) = row_at(rect, lanes, pos) {
                    let step = cols.step_at(pos.x);
                    if live.contains(&step) && lane_is_editable(&track.plocks[row]) {
                        let steps = target_steps(&notes, step, selected);
                        changed |= clear_lane_value(&mut track.plocks[row], &steps);
                    }
                }
            }
        } else if response.drag_started() || response.clicked() {
            // **Set on press, no movement threshold.** The trig lane above makes
            // you drag instead, because there a click means something else — open
            // the COND picker, cycle FILL — and a click that silently changed the
            // odds would be a nasty surprise. Here a bare click has no other
            // meaning, so a threshold would only create a dead zone around the
            // press point and swallow small adjustments.
            self.drag = None;
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some((row, y_in_row)) = row_at(rect, lanes, pos) {
                    let step = cols.step_at(pos.x);
                    if lane_is_editable(&track.plocks[row]) && live.contains(&step) {
                        let value =
                            value_from_row_y(&lane_param(&track.plocks[row]), y_in_row, ROW_H);
                        let steps = target_steps(&notes, step, selected);
                        changed |= set_lane_value(&mut track.plocks[row], &steps, value);
                        self.drag = Some(Drag { row, anchor_step: step });
                    }
                }
            }
        }

        if response.dragged() {
            if let (Some(drag), Some(pos)) = (&self.drag, response.interact_pointer_pos()) {
                let row = drag.row;
                // The value follows the pointer's height in the *lane's own row*,
                // wherever the cursor has wandered to vertically — dragging
                // sideways to paint must not change the value because the pointer
                // strayed into the next row.
                let y_in_row = pos.y - rect.min.y - row as f32 * ROW_H;
                let value = value_from_row_y(&lane_param(&track.plocks[row]), y_in_row, ROW_H);
                let here = cols.step_at(pos.x);
                let (from, to) = (drag.anchor_step.min(here), drag.anchor_step.max(here));
                let painted: Vec<f64> =
                    live.iter().copied().filter(|s| *s >= from && *s <= to).collect();
                let steps = if painted.is_empty() { vec![drag.anchor_step] } else { painted };
                changed |= set_lane_value(&mut track.plocks[row], &steps, value);
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }

        // A read-only lane explains itself rather than ignoring the pointer,
        // which is the difference between "broken" and "deliberate".
        if let Some(pos) = response.hover_pos() {
            if let Some((row, _)) = row_at(rect, lanes, pos) {
                if let Some(reason) = lane_read_only_reason(&track.plocks[row]) {
                    response.clone().on_hover_text(reason);
                }
            }
        }

        self.paint(ui, rect, cols, track, &live, lanes);
        changed
    }

    fn paint(&self, ui: &Ui, rect: Rect, cols: Cols, track: &Track, live: &[f64], rows: usize) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(22));

        for (row, lane) in track.plocks.iter().enumerate().take(rows) {
            let top = rect.min.y + row as f32 * ROW_H;
            let param = lane_param(lane);
            let editable = lane_is_editable(lane);
            let (bar, cap) = if editable { lane_color(row) } else { (READ_ONLY_BAR, READ_ONLY_CAP) };

            painter.line_segment(
                [Pos2 { x: rect.min.x, y: top }, Pos2 { x: rect.max.x, y: top }],
                Stroke::new(1.0, Color32::from_gray(40)),
            );

            for (slot, value) in lane.values.iter().enumerate() {
                let Some(value) = value else { continue };
                let step = slot as f64;
                let x = cols.x_of_step(step);
                if x + cols.cell_w < rect.min.x + KEY_W || x > rect.max.x {
                    continue;
                }
                let frac = bar_fraction(&param, *value);
                let usable = ROW_H - BAR_PAD;
                let h = frac * usable;
                let cell = Rect::from_min_size(
                    Pos2 { x: x + 1.0, y: top + BAR_PAD + (usable - h) },
                    Vec2::new((cols.cell_w - 2.0).max(1.0), h.max(1.0)),
                );
                painter.rect_filled(cell, 0.0, bar);
                // The cap marks the value's own height, which is what you are
                // aiming at when the bar is short.
                painter.line_segment(
                    [cell.left_top(), cell.right_top()],
                    Stroke::new(1.5, cap),
                );
                // A lock on a step with no trig is the box's, not ours: hatched
                // so it reads as "held, not authored".
                if !live.contains(&step) {
                    painter.line_segment(
                        [cell.left_bottom(), cell.right_top()],
                        Stroke::new(1.0, Color32::from_gray(140)),
                    );
                }
            }

            // The gutter label, in the roll's key column, so a stack of bars
            // says which knob each row is.
            let gutter = Rect::from_min_max(
                Pos2 { x: rect.min.x, y: top },
                Pos2 { x: rect.min.x + KEY_W, y: top + ROW_H },
            );
            painter.rect_filled(gutter, 0.0, Color32::from_gray(22));
            painter.text(
                Pos2 { x: rect.min.x + 6.0, y: top + ROW_H / 2.0 },
                Align2::LEFT_CENTER,
                &param.short,
                FontId::proportional(10.0),
                if editable { cap } else { Color32::from_gray(120) },
            );
        }

        // **No silent caps.** A track can hold more lanes than there is room for,
        // and a strip that just stopped drawing would read as "that lane did not
        // import". Say the number instead — the same rule the transfer panel
        // follows when it drops a trig past LEN.
        let hidden = track.plocks.len() - rows;
        if hidden > 0 {
            painter.text(
                Pos2 { x: rect.min.x + 6.0, y: rect.max.y - 6.0 },
                Align2::LEFT_BOTTOM,
                format!("+{hidden} more lane{}", if hidden == 1 { "" } else { "s" }),
                FontId::proportional(10.0),
                Color32::from_rgb(0xc9, 0x97, 0x4a),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(name: Option<&str>, param_id: Option<u16>, trigless: bool, at: &[(usize, u16)]) -> PLockLane {
        let mut values = vec![None; 128];
        for (step, v) in at {
            values[*step] = Some(*v);
        }
        PLockLane::new(
            name.map(str::to_string),
            param_id,
            Some("DT2".into()),
            trigless,
            values,
        )
        .unwrap()
    }

    fn cutoff() -> PLockLane {
        lane(Some("filter.cutoff"), Some(44), false, &[(0, 64)])
    }

    #[test]
    fn a_curated_lane_is_editable_and_a_raw_one_is_not() {
        assert!(lane_is_editable(&cutoff()));
        assert!(lane_read_only_reason(&cutoff()).is_none());

        // A knob in no table: carried, drawn, never edited.
        let raw = lane(None, Some(0x2A), false, &[(0, 40000)]);
        assert!(!lane_is_editable(&raw));
        assert!(lane_read_only_reason(&raw).unwrap().contains("read-only"));
        assert!(lane_read_only_reason(&raw).unwrap().contains("exactly as it came"));
    }

    #[test]
    fn a_trigless_lane_is_read_only_even_though_we_know_the_knob() {
        // The two read-only reasons are different and the tooltip has to say
        // which: this one we *could* draw, and decline to edit because the box is
        // holding something v1 does not model.
        let trigless = lane(Some("filter.cutoff"), Some(44), true, &[(3, 64)]);
        assert!(!lane_is_editable(&trigless));
        let why = lane_read_only_reason(&trigless).unwrap();
        assert!(why.contains("no trig"), "{why}");
        assert!(why.contains("FLTR CUTOFF"), "{why}");
    }

    #[test]
    fn the_top_of_a_row_is_the_maximum_and_the_bottom_is_the_minimum() {
        // The geometry contract: BAR_PAD of headroom at the top, and the pointer
        // sits on the top edge of the bar it is setting.
        let p = lane_param(&cutoff());
        assert_eq!(value_from_row_y(&p, BAR_PAD, ROW_H), 127);
        assert_eq!(value_from_row_y(&p, 0.0, ROW_H), 127, "above the pad still reads as full");
        assert_eq!(value_from_row_y(&p, ROW_H, ROW_H), 0);
        assert_eq!(value_from_row_y(&p, ROW_H * 2.0, ROW_H), 0, "below the row clamps");
        // Halfway down the usable height is halfway up the range.
        let mid = value_from_row_y(&p, BAR_PAD + (ROW_H - BAR_PAD) / 2.0, ROW_H);
        assert!((63..=64).contains(&mid), "{mid}");
    }

    #[test]
    fn a_bar_is_as_tall_as_the_value_is_through_the_range() {
        let p = lane_param(&cutoff());
        assert_eq!(bar_fraction(&p, 0), 0.0);
        assert_eq!(bar_fraction(&p, 127), 1.0);
        assert!((bar_fraction(&p, 64) - 0.5039).abs() < 0.001);
        // A raw lane is drawn over the whole word range, so the same number is a
        // sliver rather than a full bar — honest about relative height and about
        // nothing else.
        let raw = lane_param(&lane(None, Some(0x2A), false, &[]));
        assert!(bar_fraction(&raw, 127) < 0.01);
    }

    #[test]
    fn setting_a_value_reports_whether_anything_moved() {
        let mut l = cutoff();
        assert!(set_lane_value(&mut l, &[1.0], 100));
        assert_eq!(l.values[1], Some(100));
        assert!(!set_lane_value(&mut l, &[1.0], 100), "the same value twice is not a change");
        assert!(set_lane_value(&mut l, &[1.0], 101));
    }

    #[test]
    fn a_value_is_clamped_onto_the_parameters_own_range() {
        let mut l = cutoff();
        set_lane_value(&mut l, &[2.0], 999);
        assert_eq!(l.values[2], Some(127));
        set_lane_value(&mut l, &[3.0], -40);
        assert_eq!(l.values[3], Some(0));
    }

    #[test]
    fn a_fractional_or_out_of_range_step_addresses_no_slot() {
        // A note dragged off the grid must not silently write to the lane's step
        // 3 because 3.5 truncates.
        let mut l = cutoff();
        assert!(!set_lane_value(&mut l, &[3.5], 100));
        assert!(!set_lane_value(&mut l, &[-1.0], 100));
        assert!(!set_lane_value(&mut l, &[128.0], 100));
        assert!(l.values[3].is_none());
    }

    #[test]
    fn clearing_takes_the_lock_off_and_says_whether_it_had_to() {
        let mut l = cutoff();
        assert!(clear_lane_value(&mut l, &[0.0]));
        assert_eq!(l.values[0], None);
        assert!(!clear_lane_value(&mut l, &[0.0]));
    }

    #[test]
    fn a_lane_describes_itself_by_knob_and_by_how_much_it_holds() {
        assert_eq!(describe_lane(&cutoff()), "FLTR CUTOFF · 1 step");
        let two = lane(Some("filter.cutoff"), Some(44), false, &[(0, 1), (4, 2)]);
        assert_eq!(describe_lane(&two), "FLTR CUTOFF · 2 steps");
        let raw = lane(None, Some(0x2A), false, &[(0, 1)]);
        assert_eq!(describe_lane(&raw), "DT2 param 0x2a · 1 step · read-only");
    }

    #[test]
    fn the_strip_is_absent_rather_than_empty_when_a_track_has_no_lanes() {
        let mut track = Track::new(0, digi_core::TrackKind::Audio);
        assert_eq!(strip_height(&track), 0.0);
        track.plocks.push(cutoff());
        assert_eq!(strip_height(&track), ROW_H);
        track.plocks.push(cutoff());
        assert_eq!(strip_height(&track), 2.0 * ROW_H);
    }

    #[test]
    fn a_strip_with_no_room_for_a_row_shows_none_of_it() {
        // Found by running the app rather than by a test: five lanes were seeded,
        // four fitted, and the fifth was drawn past the bottom of the window
        // where it could neither be seen nor reached. Rows now go through one
        // function so drawing and hit testing cannot disagree about which exist.
        let wide = Vec2::new(400.0, 0.0);
        let at = |h: f32| Rect::from_min_size(Pos2 { x: 0.0, y: 0.0 }, wide + Vec2::new(0.0, h));
        assert_eq!(rows_that_fit(at(5.0 * ROW_H), 5), 5);
        assert_eq!(rows_that_fit(at(5.0 * ROW_H), 3), 3, "never more rows than lanes");
        assert_eq!(rows_that_fit(at(4.0 * ROW_H), 5), 4, "the fifth had nowhere to go");
        assert_eq!(rows_that_fit(at(4.0 * ROW_H - 1.0), 5), 3, "a part-row is not a row");
        assert_eq!(rows_that_fit(at(0.0), 5), 0);
    }

    #[test]
    fn adjacent_lanes_never_share_a_colour() {
        for row in 0..LANE_COLORS.len() * 3 {
            assert_ne!(lane_color(row).0, lane_color(row + 1).0, "row {row}");
        }
    }

    #[test]
    fn a_row_is_found_by_where_the_pointer_is_down_the_strip() {
        let rect = Rect::from_min_size(Pos2 { x: 0.0, y: 100.0 }, Vec2::new(400.0, 3.0 * ROW_H));
        assert_eq!(row_at(rect, 3, Pos2 { x: 10.0, y: 101.0 }), Some((0, 1.0)));
        assert_eq!(row_at(rect, 3, Pos2 { x: 10.0, y: 100.0 + ROW_H + 2.0 }), Some((1, 2.0)));
        // Past the last lane is no row, even though the rect may be taller.
        assert_eq!(row_at(rect, 2, Pos2 { x: 10.0, y: 100.0 + 2.0 * ROW_H + 1.0 }), None);
        assert_eq!(row_at(rect, 3, Pos2 { x: 10.0, y: 99.0 }), None);
    }
}
