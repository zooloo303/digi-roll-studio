// The MIDI ports section of the Setup panel: what is plugged in, who each box
// says it is, and which device in the session that reply belongs to.
//
// It is the *lower* half of that panel — the boxes and their routing come first,
// because that is the order the questions come in, and because once a session is
// routed this section is only visited when something is unplugged. The two lists
// fold away for the same reason.
//
// Enumeration is cheap but not free, so the list is cached and refreshed on
// demand rather than rebuilt every frame. The identity handshake blocks for up
// to 5 s per request with two retries, so it runs on a worker thread and the
// panel polls the result — a wedged box must not freeze the UI.
//
// The binding itself is `Session::bind_identity`, in `core`, so it is unit-
// tested without a port in sight. This file only supplies the reply and the two
// ports it came in on, and renders whichever answer comes back — including the
// refusals, which are the point: a box that cannot be placed says so instead of
// landing on the wrong device.

use std::sync::mpsc::{channel, Receiver};

use digi_core::device::PortRef;
use digi_core::{BindError, DeviceId, Session};
use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding, PortInfo};
use digi_protocol::device::DeviceIdentity;
use eframe::egui::{self, Ui};

pub struct PortsPanel {
    inputs: Vec<PortInfo>,
    outputs: Vec<PortInfo>,
    /// `None` until the first refresh; `Some(Err)` if MIDI itself would not start.
    error: Option<String>,
    selected_input: Option<usize>,
    selected_output: Option<usize>,
    identity: Option<Result<DeviceIdentity, String>>,
    pending: Option<Receiver<Result<DeviceIdentity, String>>>,
    /// The ports the in-flight (or last) handshake went out on. Kept because the
    /// binding needs them, and because the selection may move while it is in
    /// flight — a reply belongs to the ports it was asked on, not to whatever is
    /// highlighted when it lands.
    asked_on: Option<(PortRef, PortRef)>,
    bind: Option<Result<DeviceId, BindError>>,
}

impl Default for PortsPanel {
    fn default() -> Self {
        let mut panel = Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            error: None,
            selected_input: None,
            selected_output: None,
            identity: None,
            pending: None,
            asked_on: None,
            bind: None,
        };
        panel.refresh();
        panel
    }
}

impl PortsPanel {
    pub fn refresh(&mut self) {
        match (list_inputs(), list_outputs()) {
            (Ok(i), Ok(o)) => {
                // Keep a selection only if that exact port is still present.
                let prev_in = self.selected_input.and_then(|k| self.inputs.get(k)).cloned();
                let prev_out = self.selected_output.and_then(|k| self.outputs.get(k)).cloned();
                self.selected_input = prev_in.and_then(|p| i.iter().position(|q| q.id == p.id));
                self.selected_output = prev_out.and_then(|p| o.iter().position(|q| q.id == p.id));
                self.inputs = i;
                self.outputs = o;
                self.error = None;
                // Default to the first port pair that *looks* like an Elektron box.
                if self.selected_input.is_none() {
                    self.selected_input = self.inputs.iter().position(|p| p.slug.is_some());
                }
                if self.selected_output.is_none() {
                    self.selected_output = self.outputs.iter().position(|p| p.slug.is_some());
                }
            }
            (Err(e), _) | (_, Err(e)) => self.error = Some(e.to_string()),
        }
    }

    /// The identity of the selected box, once the handshake has answered.
    pub fn identity(&self) -> Option<&DeviceIdentity> {
        self.identity.as_ref().and_then(|r| r.as_ref().ok())
    }

    /// The ports enumerated as of the last refresh, for the device strip's manual
    /// pickers. Handed out rather than re-enumerated there because enumeration is
    /// cheap but not free, and this panel already caches it — one list per frame,
    /// and both views agree about what is plugged in.
    pub fn inputs(&self) -> &[PortInfo] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[PortInfo] {
        &self.outputs
    }

    fn start_identify(&mut self) {
        let (Some(i), Some(o)) = (self.selected_input, self.selected_output) else { return };
        let (Some(input), Some(output)) = (self.inputs.get(i), self.outputs.get(o)) else { return };
        self.asked_on = Some((port_ref(input), port_ref(output)));
        let input = PortBinding::from(input);
        let output = PortBinding::from(output);

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = ElektronDevice::open(&input, &output)
                .and_then(|mut d| d.identify())
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.pending = Some(rx);
        self.identity = None;
        self.bind = None;
    }

    /// Take a landed handshake. Returns whether one landed on this call — a
    /// binding gives a device its ports, and the device strip above this section
    /// has already been drawn for the frame, so the caller owes the window one
    /// more repaint before the two agree.
    fn poll(&mut self, session: &mut Session) -> bool {
        let Some(rx) = &self.pending else { return false };
        let Ok(result) = rx.try_recv() else { return false };
        self.pending = None;
        // A reply is only half the job: the session has to know which of its
        // boxes just spoke. Phase 3's third exit criterion.
        if let (Ok(identity), Some((input, output))) = (&result, self.asked_on.clone()) {
            self.bind = Some(session.bind_identity(identity, input, output));
        }
        self.identity = Some(result);
        true
    }

    pub fn ui(&mut self, ui: &mut Ui, session: &mut Session) {
        if self.poll(session) {
            ui.ctx().request_repaint();
        }

        ui.horizontal(|ui| {
            super::caption(ui, "MIDI PORTS");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Refresh").clicked() {
                    self.refresh();
                    // The pickers above were drawn from the old list this frame.
                    ui.ctx().request_repaint();
                }
            });
        });

        if let Some(err) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
            return;
        }

        if self.inputs.is_empty() && self.outputs.is_empty() {
            ui.label("No MIDI ports found. Connect a device and press Refresh.");
            return;
        }

        port_list(ui, "In", &self.inputs, &mut self.selected_input);
        port_list(ui, "Out", &self.outputs, &mut self.selected_output);

        ui.add_space(6.0);

        let busy = self.pending.is_some();
        let ready = self.selected_input.is_some() && self.selected_output.is_some();
        ui.add_enabled_ui(ready && !busy, |ui| {
            if ui.button("Identify").on_hover_text("Read-only: asks the box who it is").clicked() {
                self.start_identify();
            }
        });

        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("waiting for the box …");
            });
            // No callback wakes the UI thread, so keep polling while in flight.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }

        match &self.identity {
            Some(Ok(id)) => {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&id.name).strong());
                ui.label(format!("OS build {} ({})", id.build, id.version));
                if id.supported() {
                    ui.label("Pattern dumps: supported");
                } else {
                    ui.colored_label(
                        super::CAUTION,
                        "Unknown dump protocol — read-only",
                    );
                }
                self.bind_ui(ui, session);
            }
            Some(Err(e)) => {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            None => {}
        }
    }

    /// Where the reply landed in the session — or why it did not land.
    fn bind_ui(&mut self, ui: &mut Ui, session: &mut Session) {
        // Cloned so the Ambiguous arm can overwrite `self.bind` with the choice
        // the user just made.
        let Some(bind) = self.bind.clone() else { return };
        ui.add_space(4.0);
        match &bind {
            Ok(id) => {
                let name = session
                    .device(*id)
                    .map(|d| format!("{} ({})", d.name, d.model.display))
                    .unwrap_or_else(|| "a device that has since gone".into());
                ui.colored_label(egui::Color32::LIGHT_GREEN, format!("Bound to {name}"));
            }
            // Several boxes of one model, all already on other ports. `core`
            // refuses to guess, so the choice is offered here rather than made
            // for the user.
            Err(BindError::Ambiguous { model, candidates }) => {
                ui.colored_label(
                    super::CAUTION,
                    format!("Which {}?", model.display),
                );
                let choices: Vec<(DeviceId, String)> = candidates
                    .iter()
                    .map(|id| {
                        let label = session
                            .device(*id)
                            .map(|d| d.name.clone())
                            .unwrap_or_else(|| format!("device {}", id.0));
                        (*id, label)
                    })
                    .collect();
                let identity = self.identity.as_ref().and_then(|r| r.as_ref().ok()).cloned();
                for (id, label) in choices {
                    if ui.button(&label).clicked() {
                        if let (Some(identity), Some((input, output))) =
                            (&identity, self.asked_on.clone())
                        {
                            self.bind = Some(
                                session
                                    .bind_identity_to(id, identity, input, output)
                                    .map(|()| id),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                ui.colored_label(super::CAUTION, e.to_string());
            }
        }
    }
}

fn port_ref(p: &PortInfo) -> PortRef {
    PortRef { id: p.id.clone(), name: p.name.clone() }
}

/// One end's list, folded away behind a header: eighteen ports is the whole
/// height of a panel, and after a session is routed nobody is reading them.
///
/// The header counts what it is hiding, so a folded list still answers "is the
/// box even plugged in".
fn port_list(ui: &mut Ui, label: &str, ports: &[PortInfo], selected: &mut Option<usize>) {
    egui::CollapsingHeader::new(format!("{label} — {} port(s)", ports.len()))
        .id_salt(("port-list", label))
        .default_open(true)
        .show(ui, |ui| {
            for (i, port) in ports.iter().enumerate() {
                let text = match port.slug {
                    Some(slug) => format!("{}  ({})", port.name, slug),
                    None => port.name.clone(),
                };
                if ui.selectable_label(*selected == Some(i), text).clicked() {
                    *selected = Some(i);
                }
            }
            if ports.is_empty() {
                ui.weak("none");
            }
        });
}
