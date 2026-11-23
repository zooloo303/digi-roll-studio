//! A shared focus control: switching here selects the same box as TRACKS.
use digi_core::Session;
use egui::Ui;

use super::tracks::Selection;

pub fn ui(ui: &mut Ui, session: &Session, selection: &mut Selection) -> bool {
    let before = *selection;
    ui.horizontal_wrapped(|ui| {
        ui.weak("BOX");
        for (index, device) in session.devices.iter().enumerate() {
            if ui
                .push_id(device.id, |ui| {
                    ui.selectable_label(selection.device == index, &device.name)
                        .on_hover_text(format!("Focus {} in all editors", device.model.display))
                })
                .inner
                .clicked()
                && selection.device != index
            {
                selection.device = index;
                let tracks = session
                    .current_pattern(device.id)
                    .map_or(0, |p| p.num_tracks());
                selection.track = selection.track.min(tracks.saturating_sub(1));
            }
        }
    });
    *selection != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::{
        device::{Device, A4, DN2, DT2},
        history::Content,
    };

    #[test]
    fn buttons_switch_shared_focus_and_clamp_to_the_new_boxes_tracks() {
        let mut session = Session::default();
        for (name, model) in [("DT2", &DT2), ("DN2", &DN2), ("A4", &A4)] {
            session.add_device(Device::new(name, model, 16));
        }
        let before = Content::of(&session);
        let ctx = egui::Context::default();
        let mut selection = Selection {
            device: 0,
            track: 13,
        };
        let frame = |selection: &mut Selection, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 100.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    super::ui(ui, &session, selection);
                },
            );
            output.textures_delta.clear();
            output
        };
        let mut output = frame(&mut selection, vec![]);
        for (name, expected) in [
            (
                "DN2",
                Selection {
                    device: 1,
                    track: 13,
                },
            ),
            (
                "A4",
                Selection {
                    device: 2,
                    track: 5,
                },
            ),
            (
                "DT2",
                Selection {
                    device: 0,
                    track: 5,
                },
            ),
        ] {
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == name => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .expect("box button is visible");
            output = frame(
                &mut selection,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert_eq!(selection, expected);
        }
        assert_eq!(Content::of(&session), before, "focus never edits a pattern");
    }
}
