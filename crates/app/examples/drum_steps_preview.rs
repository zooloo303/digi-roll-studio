//! Offline editor preview. Synthetic session, inert MIDI sink, no port scans.
//! Run with `cargo run -p digi_roll_studio --example drum_steps_preview`.
use digi_core::device::{Device, DT2};
use digi_core::history::{Content, History};
use digi_core::{Note, PLockLane, Session, TrackScale};
use digi_engine::{event::PortId, transport::PortSink};
use digi_roll_studio::ui::{
    edit,
    pianoroll::PianoRoll,
    tracks::Selection,
    workspace::{EditorView, Workspace},
};
use digi_roll_studio::EngineLink;
use eframe::egui;

struct Silent;
impl PortSink for Silent {
    fn send(&mut self, _: PortId, _: &[u8]) {}
}

struct Preview {
    session: Session,
    workspace: Workspace,
    roll: PianoRoll,
    selection: Selection,
    engine: EngineLink,
    history: History,
}

impl eframe::App for Preview {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| self.content(ui));
    }
}

impl Preview {
    fn content(&mut self, ui: &mut egui::Ui) {
        let before = (!self.history.is_open()).then(|| Content::of(&self.session));
        let mut stepped = edit::shortcuts(ui, &mut self.session, &mut self.roll, &mut self.history);
        ui.horizontal_wrapped(|ui| {
            ui.strong("DRUM STEPS");
            ui.weak("Offline preview · synthetic pattern · no MIDI ports");
            if ui
                .button(if self.engine.is_playing() {
                    "STOP"
                } else {
                    "PLAY"
                })
                .clicked()
            {
                if self.engine.is_playing() {
                    self.engine.stop();
                } else {
                    self.engine.play(&self.session);
                }
            }
            if ui.button("Undo").clicked() {
                stepped |= self.history.undo(&mut self.session);
            }
            if ui.button("Redo").clicked() {
                stepped |= self.history.redo(&mut self.session);
            }
            for (label, size) in [("Laptop", [1100.0, 800.0]), ("Narrow", [700.0, 680.0])] {
                if ui.button(label).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(size.into()));
                }
            }
        });
        if stepped {
            self.workspace.cancel_gesture();
            self.roll.clear_selection();
        }
        let changed = self.workspace.ui(
            ui,
            &mut self.session,
            &mut self.engine,
            &mut self.selection,
            &mut self.roll,
            false,
        );
        if changed {
            if let Some(before) = before {
                self.history.begin(before);
            }
        }
        if !ui.input(|i| i.pointer.any_down()) {
            self.history.commit(&self.session);
        }
        if changed || stepped {
            self.engine.sync(&self.session);
        }
    }
}

fn main() -> eframe::Result {
    let mut session = Session::default();
    let device = session.add_device(Device::new("Digitakt II", &DT2, 16));
    session.tempo_bpm = 126.0;
    let pattern = session.device_mut(device).unwrap().pattern_mut(0).unwrap();
    pattern.name = "Night shift".into();
    let names = [
        "Deep kick",
        "Dry snare",
        "Closed hat",
        "Open hat",
        "Rim",
        "Shaker",
        "Low tom",
        "Clap",
        "Ride",
        "Metal",
        "Perc A",
        "Perc B",
        "Texture",
        "Fill",
        "Accent",
        "Spare",
    ];
    let steps: [&[usize]; 8] = [
        &[0, 4, 8, 10, 12],
        &[4, 12],
        &[0, 2, 4, 6, 8, 10, 12, 14],
        &[6, 14],
        &[3, 11],
        &[1, 3, 5, 7, 9, 11, 13, 15],
        &[7, 15],
        &[12],
    ];
    for (i, name) in names.iter().enumerate() {
        let track = pattern.track_mut(i).unwrap();
        track.name = name.to_string();
        track.length_steps = if i == 4 { 12 } else { 64 };
        if i == 5 {
            track.scale = TrackScale::Half;
        }
        if let Some(steps) = steps.get(i) {
            for page in 0..track.length_steps.div_ceil(16) {
                for &s in *steps {
                    let step = usize::from(page) * 16 + s;
                    if step < usize::from(track.length_steps) {
                        let mut note = Note::new(
                            step as f64,
                            60,
                            1.0,
                            if i == 2 || i == 5 {
                                if s % 4 == 0 {
                                    96
                                } else {
                                    58
                                }
                            } else {
                                112
                            },
                            0.0,
                        );
                        if i == 4 {
                            note.prob = Some(75);
                        }
                        if i == 6 {
                            note.cond = Some("1:2".into());
                        }
                        track.notes.push(note);
                    }
                }
            }
        }
    }
    pattern.track_mut(0).unwrap().plocks.push(
        PLockLane::new(
            Some("filter.cutoff".into()),
            None,
            Some("DT2".into()),
            false,
            vec![
                Some(100),
                None,
                None,
                None,
                Some(80),
                None,
                None,
                None,
                Some(110),
                None,
                Some(68),
                None,
                Some(90),
            ],
        )
        .unwrap(),
    );
    let mut workspace = Workspace::default();
    workspace.view = EditorView::DrumSteps;
    eframe::run_native(
        "Drum Steps — Offline Preview",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 800.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            digi_roll_studio::ui::install_style(&cc.egui_ctx);
            Ok(Box::new(Preview {
                session,
                workspace,
                roll: PianoRoll::default(),
                selection: Selection::default(),
                engine: EngineLink::with_sinks(Box::new(|_| (Box::new(Silent), Vec::new()))),
                history: History::default(),
            }))
        }),
    )
}
