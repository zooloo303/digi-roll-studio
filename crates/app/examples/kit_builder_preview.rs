//! Offline visual harness: synthetic names and all tags; no MIDI ports or engine.
use digi_core::{
    device::{Device, A4, DN2},
    Session,
};
use digi_protocol::preset_index::{BankIndex, IndexEntry, PresetIndex};
use digi_roll_studio::ui::{presets::PresetsPanel, tracks::Selection};
use eframe::egui;

struct Preview {
    session: Session,
    panel: PresetsPanel,
    selection: Selection,
}
impl eframe::App for Preview {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        ui.horizontal(|ui| {
            ui.label("Offline fixture preview");
            ui.selectable_value(&mut self.selection.device, 0, "Digitone II");
            ui.selectable_value(&mut self.selection.device, 1, "Analog Four");
            for (label, size) in [("Laptop", [1100.0, 760.0]), ("Narrow", [640.0, 700.0])] {
                if ui.button(label).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(size.into()));
                }
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            let out = self.panel.ui(ui, &self.session, self.selection, false);
            if let Some(selection) = out.selection {
                self.selection = selection;
            }
        });
    }
}
fn main() -> eframe::Result {
    let store = PresetIndex::at(std::env::temp_dir().join("digi-kit-builder-preview"));
    let mut session = Session::default();
    for model in [&DN2, &A4] {
        session.add_device(Device::new(model.display, model, 16));
        for bank in ["A", "B"] {
            let mut index = BankIndex::new(
                model.slug.unwrap(),
                &format!("/soundbanks/{bank}"),
                "fixture",
                32,
            );
            for i in 0..32 {
                index.insert(
                    i + 1,
                    IndexEntry {
                        name: format!("Preview {bank} preset {:02}", i + 1),
                        tag_mask: (1u32 << i) | 1,
                        size: 319,
                        format: digi_protocol::drive::native_container_magic(model.slug.unwrap()),
                    },
                );
            }
            store.save(&index).unwrap();
        }
    }
    eframe::run_native(
        "Kit builder preview",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 760.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            digi_roll_studio::ui::install_style(&cc.egui_ctx);
            Ok(Box::new(Preview {
                session,
                panel: PresetsPanel::with_store(store),
                selection: Selection::default(),
            }))
        }),
    )
}
