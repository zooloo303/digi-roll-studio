// The scene controls — PLAN.md §5: "the scenes from §2, showing which slot each
// box is on and what is queued".
//
// A scene is one pattern per box, chosen together, so this is the control that
// makes a session more than two sequencers side by side: one click moves the DT2
// and the DN2 to their next pattern at the same boundary.
//
// **Where this draws changed on 2026-08-19.** It was a floating pane beside the
// patterns, and `design_handoff_digi_roll_ui_v2/README.md` §2c's structural
// argument is that the pane was in the wrong place: it split the centre column,
// so the track lanes and the piano roll underneath them no longer shared a left
// and a right edge, and the playhead in the roll stopped sitting under the
// progress bars driving it. So the *live state* — which scene is sounding, what
// is queued, which slot each box is on — moved into the transport bar's zone 5,
// and this function is now what that zone's pill opens: the same three rows,
// unchanged, in a popup. Nothing here was reimplemented compactly; a compact
// copy is a second implementation that drifts.
//
// Three rows, in the order the work goes — what the engine is doing, which
// scenes there are, and what the one being edited holds — because a scene is
// built out of patterns that already exist: you draw, then you group.
//
// Two things here are not obvious, and both come from PLAN.md §4.
//
// **Clicking a scene queues it; it does not switch it.** The switch is taken by
// the engine at the boundary of the longest track in the outgoing scene, which
// only the engine knows the moment of. So this bar *asks*, and then follows what
// the engine publishes — the highlight moves when the boxes move, not when the
// mouse does. Stopped, there is no boundary and the answer is immediate, so
// picking a scene to edit does what it looks like it does.
//
// **`Session::current_scene` means the scene that is sounding.** `main` copies
// the engine's answer into it every frame, which is what makes the track strip
// and the roll follow a switch onto the patterns that are now playing without
// either of them knowing scenes exist.
//
// Editing a scene's slots *is* an ordinary edit and goes down as a snapshot, at
// once and without a boundary — the same bargain as moving a note while the
// transport runs.

use digi_core::{PatternRef, Session};
use eframe::egui::{self, Ui};

use crate::engine::EngineLink;

/// Draw the full scene controls. Returns whether the session changed.
///
/// Called from `transport.rs`'s zone 5, inside the popup its scene pill opens.
pub fn ui(ui: &mut Ui, session: &mut Session, engine: &mut EngineLink) -> bool {
    let mut changed = false;
    // A project file is the only way to get here with no scenes — nothing in the
    // app can remove the last one — and the rest of this indexes freely.
    if session.scenes.is_empty() {
        ui.weak("this session has no scenes, so nothing can play");
        return changed;
    }
    let playing = engine.playing_scene();
    let queued = engine.queued_scene();
    // The slot pickers edit the scene the user last asked for, so a scene can be
    // set up on the way in rather than only once it is already sounding.
    let editing = engine.selected_scene().min(session.scenes.len() - 1);

    ui.horizontal(|ui| {
        super::caption(ui, "SCENES");

        if ui
            .small_button("+")
            .on_hover_text("Add a scene, starting where this one is")
            .clicked()
        {
            let name = format!("Scene {}", session.scenes.len() + 1);
            session.add_scene(name, Some(editing));
            changed = true;
        }
        if ui
            .add_enabled(session.scenes.len() > 1, egui::Button::new("−").small())
            .on_hover_text("Remove this scene. The last one cannot go: every box plays through a scene")
            .clicked()
            && session.remove_scene(editing)
        {
            // The engine is holding an index into a list that just got shorter.
            // `remove_scene` has already shifted `current_scene` — which is the
            // scene the engine says is sounding, copied in at the top of the
            // frame — so that is the corrected number for it.
            engine.rebase_scene(session, session.current_scene);
            changed = true;
        }

        ui.separator();
        let mut immediate = engine.scene_immediate();
        if ui
            .toggle_value(&mut immediate, "NOW")
            .on_hover_text(
                "Switch without waiting for the boundary — as soon as what is \
                 already on its way out has gone",
            )
            .changed()
        {
            engine.set_scene_immediate(immediate);
        }

        if let Some(q) = queued.filter(|q| *q != playing) {
            let name = session.scenes.get(q).map(|s| s.name.as_str()).unwrap_or("?");
            ui.colored_label(super::ACCENT, format!("» {name} queued"));
        }
    });

    // The scenes themselves. Wrapped rather than a column: they are few, their
    // names are short, and the pane's height is the roll's.
    ui.horizontal_wrapped(|ui| {
        for index in 0..session.scenes.len() {
            let label = scene_label(session, index, playing, queued);
            let response = ui
                .selectable_label(index == editing, label)
                .on_hover_text(slots_summary(session, index));
            if response.clicked() {
                engine.select_scene(session, index);
            }
        }
    });

    // The slots: which pattern each box plays in the scene being edited. This is
    // the whole content of a scene, so it is worth showing rather than hiding
    // behind the name.
    ui.horizontal_wrapped(|ui| {
        let Some(scene) = session.scenes.get(editing) else {
            return;
        };
        let mut name = scene.name.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(96.0)
                    .hint_text("scene name"),
            )
            .changed()
        {
            if let Some(scene) = session.scenes.get_mut(editing) {
                scene.name = name;
                changed = true;
            }
        }

        let devices: Vec<(digi_core::DeviceId, String, usize)> = session
            .devices
            .iter()
            .map(|d| (d.id, d.name.clone(), d.patterns.len()))
            .collect();
        for (id, device_name, slots) in devices {
            ui.weak(format!("· {device_name}"));
            let current = session
                .slot_in_scene(editing, id)
                .unwrap_or(PatternRef::new(0, 0));
            let mut chosen = current;
            egui::ComboBox::from_id_salt(("scene-slot", editing, id.0))
                .selected_text(current.label())
                .width(52.0)
                .show_ui(ui, |ui| {
                    for slot in 0..slots {
                        let candidate = PatternRef::from_slot(slot);
                        ui.selectable_value(&mut chosen, candidate, candidate.label());
                    }
                });
            if chosen != current && session.set_slot_in_scene(editing, id, chosen) {
                changed = true;
            }
        }
    });

    changed
}

/// A scene's button: its name, plus a mark for what the engine is doing with it.
///
/// The marks are the two states a queued switch has: `▶` is sounding now, `»` is
/// waiting for a boundary. A scene that is neither gets neither.
///
/// **Both marks are glyphs this build has been seen to draw.** The first pair
/// were `●` (U+25CF) and `▸` (U+25B8), and egui bundles neither, so this bar
/// shipped showing two missing-glyph boxes — the whole of what it existed to say,
/// invisible, with every test passing. `▶` is what the transport's Play button
/// already draws and `»` is Latin-1; both were then read off the screen to be
/// sure. Anything added here wants the same treatment, for the reason set out in
/// [`super`]: the compiler cannot see a tofu box, and neither can a unit test.
fn scene_label(
    session: &Session,
    index: usize,
    playing: usize,
    queued: Option<usize>,
) -> egui::RichText {
    let name = session
        .scenes
        .get(index)
        .map(|s| s.name.as_str())
        .unwrap_or("?");
    if index == playing {
        egui::RichText::new(format!("▶ {name}")).strong()
    } else if queued == Some(index) {
        egui::RichText::new(format!("» {name}")).color(super::ACCENT)
    } else {
        egui::RichText::new(format!("  {name}"))
    }
}

/// "DT2 A01 · DN2 B03" — what this scene actually does.
///
/// The tooltip on each scene button here, and — since the v2 pass — the dim
/// slot readout in the transport bar's scene zone, which is the same fact for
/// the scene that is sounding. One function, so the two cannot disagree.
pub fn slots_summary(session: &Session, index: usize) -> String {
    let parts: Vec<String> = session
        .devices
        .iter()
        .map(|d| {
            let slot = session
                .slot_in_scene(index, d.id)
                .map(|s| s.label())
                .unwrap_or_else(|| "—".into());
            format!("{} {}", d.name, slot)
        })
        .collect();
    if parts.is_empty() {
        return "no boxes in this session".into();
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_button_says_what_the_engine_is_doing_with_it() {
        let mut session = digi_core::default_session();
        session.add_scene("Chorus", Some(0));

        // Playing scene 0 with 1 queued behind it: one mark each, and they are
        // different marks, because "sounding" and "next" are different answers.
        assert!(scene_label(&session, 0, 0, Some(1)).text().starts_with('▶'));
        assert!(scene_label(&session, 1, 0, Some(1)).text().starts_with('»'));
        // Nothing queued: the other scene carries no mark at all.
        assert!(scene_label(&session, 1, 0, None).text().starts_with(' '));
    }

    #[test]
    fn the_tooltip_names_every_box_and_its_slot() {
        let mut session = digi_core::default_session();
        let dn2 = session.devices[1].id;
        session.set_slot_in_scene(0, dn2, PatternRef::new(1, 2));
        assert_eq!(slots_summary(&session, 0), "DT2 A01 · DN2 B03");
    }

    #[test]
    fn a_slot_picker_offers_the_labels_printed_on_the_box() {
        // The combo box is built from `from_slot` over the device's slot count,
        // so these are the strings a user picks between.
        assert_eq!(PatternRef::from_slot(0).label(), "A01");
        assert_eq!(PatternRef::from_slot(15).label(), "A16");
        assert_eq!(PatternRef::from_slot(16).label(), "B01");
    }
}
