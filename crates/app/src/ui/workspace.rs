// The middle of the window: the track lanes, and the roll under them.
//
// **The centre column runs edge to edge, and that is the point.** Until
// 2026-08-19 this row was two panes side by side — the track lanes and a
// floating Scenes block — because a pattern is what a scene is made of and the
// two steps are one sitting. `design_handoff_digi_roll_ui_v2/README.md` §2c
// makes the case against it: the lanes and the roll beneath them are two views
// of the same sixteen steps, so they have to share a left edge and a right edge
// or the playhead in the roll stops sitting under the progress bars driving it.
// The scenes block broke that alignment and squeezed the lanes at the same time.
// It moved into the transport bar's zone 5 — see `scenes.rs` and `transport.rs` —
// and the lanes took the whole width, which is what the `panes()` split that used
// to live here was standing in the way of.
//
// **The row is a fixed height, not a nested `Panel`.** A `Panel::top` would come
// with a resize handle along the roll's top edge, and the roll allocates its rect
// with `Sense::click_and_drag` — the later widget wins the pointer, so the handle
// would either steal a note-drawing click or be dead. A fixed allocation has no
// handle to fight over. The lanes scroll if their content outgrows the row.

use digi_core::Session;
use eframe::egui::{self, Align, Layout, Ui, Vec2};

use crate::engine::EngineLink;
use crate::ui::pianoroll::PianoRoll;
use crate::ui::tracks::{self, Selection};

/// The most height the track-lanes row may take.
///
/// Grew from 108 to fit the 2026-08-19 track-lanes redesign: a header line, two
/// 46px-tall device rows plus their margins, a divider, and the parameter row —
/// the same two-device default session the old height was sized for, just at
/// the new grid's own per-row cost rather than a wrapped row of buttons. A
/// third box still scrolls its pane rather than growing this further and
/// squeezing the roll, which is `pane()`'s job below, unchanged from before.
///
/// **Unchanged by the scenes move.** The block that used to sit beside the
/// lanes was the same height as them, never taller, so dropping it gave the row
/// width and not height — there was nothing to recompute.
const HEAD_H: f32 = 206.0;

/// Draw the workspace. Returns whether the session changed.
pub fn ui(
    ui: &mut Ui,
    session: &mut Session,
    engine: &mut EngineLink,
    selection: &mut Selection,
    roll: &mut PianoRoll,
) -> bool {
    let mut edited = false;

    // **The row's height is capped here, not left to its contents.** It has to
    // be: `pane` below wraps its content in a `ScrollArea`, which grows to
    // whatever it is given, so an uncapped row would take the whole central
    // panel and leave the roll a rect of no height. That is not hypothetical —
    // it happened once already, when a `Separator` in the horizontal layout this
    // row used to have grew floor to ceiling and the window showed no piano roll
    // at all, with every test passing.
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        Vec2::new(width, HEAD_H),
        Layout::top_down(Align::Min),
        |ui| {
            ui.set_min_height(HEAD_H);
            ui.set_max_height(HEAD_H);
            pane(ui, "patterns", width, |ui| {
                edited |= tracks::ui(ui, session, selection, &*engine);
            });
        },
    );
    ui.separator();

    // The playhead is the engine's, in pattern steps; a track wraps it by its own
    // length and runs it at its own scale, which is exactly what makes two tracks
    // of different lengths draw right against one clock.
    let playhead = engine.is_playing().then(|| {
        let position = engine.position_steps();
        match tracks::track(session, *selection) {
            Some(track) if track.length_steps > 0 => {
                let steps = position * track.scale.multiplier();
                steps % track.length_steps as f64
            }
            _ => position,
        }
    });

    // **The key comes out of the session and goes back in.** The roll needs it
    // `&mut` for one gesture — alt+wheel cycles the chord inversion while aiming —
    // and it cannot borrow the session twice, since the track it edits is inside
    // it. `Harmony` is `Copy` and eight small fields, so a copy out and a compare
    // back costs nothing and keeps the roll ignorant of sessions.
    let mut harmony = session.harmony;
    match tracks::track_mut(session, *selection) {
        Some(track) => edited |= roll.ui(ui, track, playhead, &mut harmony),
        None => {
            ui.weak("no track selected");
        }
    }
    if session.harmony != harmony {
        session.harmony = harmony;
        edited = true;
    }

    edited
}

/// The row's one pane: the centre column's full width, the row's height, and a
/// scrollbar if what is in it does not fit — so a session with more boxes than
/// the row can show scrolls the pane rather than pushing the roll down the
/// window.
fn pane(ui: &mut Ui, id: &'static str, width: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, HEAD_H),
        Layout::top_down(Align::Min),
        |ui| {
            ui.set_max_width(width);
            egui::ScrollArea::vertical()
                .id_salt(id)
                .max_height(HEAD_H)
                .auto_shrink([false, false])
                .show(ui, add);
        },
    );
}

// **No tests here any more, and that is the change rather than an omission.**
// The two that were here proved `panes()` — the width split between the track
// lanes and the floating scenes block — divided the centre column exactly and
// never starved either side. There is no split left to prove: the lanes take
// `ui.available_width()` and the scenes went to the transport bar, whose own
// pure pieces (`beat_of_bar`, `readout_segments`, `meter_lit`, `clock_label`)
// carry their own tests. Everything else in this file is a draw call.
