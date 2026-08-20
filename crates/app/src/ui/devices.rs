// The device strip — PLAN.md §5: "one row per box in the session: name, model,
// in/out port, connection state, current slot, and whether it takes clock."
//
// A group per box rather than §5's single row: it lives in the right-hand Setup
// panel now, roughly 300px wide, and a row that wide can hold a name or a port
// picker but not both. The order inside a group is the order the questions come
// in — who is this, what does it play, what is it plugged into, does it follow us.
//
// **Why the port pickers matter more than they look.** Until this existed,
// `Identify` was the only thing that ever gave a device a port. That made the
// app unable to reach an IAC bus or a soft synth, so it could not make a sound
// without an Elektron on the desk — which put every part of the UI downstream of
// the roll outside the dev loop that PLAN.md §7 rule 1 insists on. A dropdown
// here is the difference between testing the sequencer and owning the hardware.
//
// The model rule these pickers obey is `Session::set_device_port`, in `core`, so
// it is unit-tested without a port in sight: a port belongs to one box, and
// moving a device off its ports drops the OS it reported. This file only offers
// the choices and says what the engine did with them.
//
// **A picker names the box that currently holds a port**, rather than listing it
// as if it were free. Picking it does work — the other box loses it, which is
// what `set_device_port` guarantees — but silently stealing the DT2's socket when
// the row you are editing says DN2 is the sort of thing you discover later,
// through a trig coming out of the wrong box.
//
// **Connection state is the engine's answer, not the session's.** A device can
// name a port that will not open — another app holds it — so the strip asks the
// engine which ports it actually has, and a named-but-not-open port reads as
// dim rather than as ready.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{LazyLock, Mutex};

use digi_core::device::PortEnd;
use digi_core::session::PatternRef;
use digi_core::{Device, DeviceId, PortRef, Session};
use digi_midi::{ElektronDevice, PortInfo};
use digi_protocol::pattern::PatternKit;
use digi_protocol::safe_write::PatternIo;
use eframe::egui::{self, Ui};

use crate::engine::EngineLink;
use crate::ui::sync::{patch_read_blocker, patch_read_job, read_patch_kit, PatchJob};
use crate::ui::write::PortsPresent;

/// Whether this box's output is a port the engine actually holds **and the OS
/// can still see** — the one honest summary of "will pressing play reach it".
///
/// Pulled out for the connection status strip (`ui::setup`), which needs the
/// same answer per box to decide whether its dot is green or amber: two views
/// of the desk computing "is this box live" two different ways is how they end
/// up disagreeing about it.
///
/// # Why the engine's own connection is not enough
///
/// Asking only `engine.ports()` is asking whether *we* still hold a connection
/// object, which is a different question from whether the box is there. Pull a
/// DT2's USB on macOS and CoreMIDI hands back a connection that stays alive and
/// quietly goes nowhere: nothing errors, nothing closes, and from inside this
/// app absolutely nothing has happened. The row stayed bold and the port count
/// stayed at two while the cable sat on the desk.
///
/// So liveness needs the OS's own list as well, which `PortsPanel` refreshes
/// every three seconds for [`crate::ui::autoconnect`] and which therefore costs
/// nothing new here. `outputs` empty means the enumeration has not run rather
/// than that everything is unplugged — the same rule, and for the same reason,
/// as [`crate::ui::write::PortsPresent::holds`].
pub fn is_live(device: &Device, engine: &EngineLink, outputs: &[PortInfo]) -> bool {
    device.io.output.as_ref().is_some_and(|p| {
        engine.ports().get(&p.name).is_some()
            && crate::ui::write::PortsPresent::holds(outputs, p)
    })
}

/// Draw the strip. Returns whether the session changed.
pub fn ui(
    ui: &mut Ui,
    session: &mut Session,
    engine: &EngineLink,
    inputs: &[PortInfo],
    outputs: &[PortInfo],
) -> bool {
    let mut changed = false;

    if session.devices.is_empty() {
        ui.weak(format!("{} — no boxes in this session", session.name));
        return changed;
    }

    let devices: Vec<DeviceId> = session.devices.iter().map(|d| d.id).collect();
    let last = devices.len().saturating_sub(1);
    for (position, id) in devices.into_iter().enumerate() {
        let Some(device) = session.device(id) else { continue };

        // Whether this box's output is a port the engine actually holds. The one
        // honest summary of "will pressing play reach it".
        let live = is_live(device, engine, outputs);
        let slot = session
            .slot_in_scene(session.current_scene, id)
            .map(|s| s.label())
            .unwrap_or_else(|| "—".into());
        let heading = format!("{} · {}", device.name, device.model.display);
        let text = if live {
            egui::RichText::new(heading).strong()
        } else {
            egui::RichText::new(heading).weak()
        };
        let tracks = device.model.num_tracks;
        let sysex = device.can_sysex();
        let build = device.io.build.clone();

        ui.label(text).on_hover_text(if live {
            "The engine has this box's out port open"
        } else {
            "No out port open for this box — it will not sound"
        });
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!("{tracks} trk · plays {slot}"));
            if let Some(build) = build {
                // Only ever from a handshake, and dropped the moment a port
                // moves, so this is a claim about the box on these ports *now*.
                ui.weak(format!("· OS {build}"));
            }
            if !sysex {
                ui.weak("· live only");
            }
        });

        changed |= picker(ui, session, id, PortEnd::Input, inputs);
        changed |= picker(ui, session, id, PortEnd::Output, outputs);

        // This is our half of sync, and it is the smaller half. Proven on
        // hardware 2026-08-17: with both boxes set to receive, both took sync from
        // here — and with the DT2 set to *send*, neither did, the DN2 included.
        // The reason is the cabling: the boxes are chained to each other over DIN
        // while we reach each of them over USB, so a box with CLOCK SEND on is a
        // second master on every *other* box's DIN input. Both facts a user needs
        // when sync looks wrong are therefore on the boxes, and a tooltip is the
        // only place this app can say them — SYNC is a menu we neither read nor
        // write. Turning this off is the honest way to leave a box that is slaved
        // elsewhere alone (PLAN.md §4).
        // The menu path is spelled with `>` and not `→`. U+2192 is on `ui::mod`'s
        // suspect list, and on 2026-08-20 this very tooltip was photographed
        // reading `SETTINGS □ MIDI CONFIG □ SYNC` — two tofu boxes, in the one
        // sentence someone reads *because* their sync is already wrong. Same
        // decision, and the same reasoning, as `tracks::channel_note`.
        let mut takes_clock = session.device(id).is_some_and(|d| d.io.takes_clock);
        if ui
            .checkbox(&mut takes_clock, "Takes our clock")
            .on_hover_text(
                "Send this box the session's clock and transport.\n\n\
                 On the box: SETTINGS > MIDI CONFIG > SYNC, with CLOCK RECEIVE and \
                 TRANSPORT RECEIVE on — and CLOCK SEND off on every box. On a \
                 DIN-chained desk a box that sends clock feeds it to your other \
                 boxes, and they stop locking to us too.\n\n\
                 Turn this off for a box slaved to something else.",
            )
            .changed()
        {
            if let Some(d) = session.device_mut(id) {
                d.io.takes_clock = takes_clock;
                changed = true;
            }
        }

        let (_, patched, _) = patch_read_row(ui, session, id, inputs, outputs, open_real);
        changed |= patched;

        if position != last {
            ui.add_space(6.0);
            ui.separator();
        }
    }

    changed
}

/// One end's dropdown. Returns whether it changed the session.
fn picker(
    ui: &mut Ui,
    session: &mut Session,
    device: DeviceId,
    end: PortEnd,
    ports: &[PortInfo],
) -> bool {
    let label = match end {
        PortEnd::Input => "in",
        PortEnd::Output => "out",
    };
    let current = session.device(device).and_then(|d| d.port(end)).cloned();
    let choices = port_choices(session, device, end, ports);

    let mut chosen = current.clone();
    ui.horizontal(|ui| {
        ui.weak(label);
        // The picker takes the rest of the row, whatever the panel has been
        // dragged to: a fixed width either overflows a narrow panel or leaves a
        // gap in a wide one, and port names are long.
        let width = (ui.available_width() - 4.0).max(90.0);
        egui::ComboBox::from_id_salt(("device-port", device.0, label))
            .selected_text(selected_text(current.as_ref()))
            .width(width)
            .show_ui(ui, |ui| {
                for (port, text) in &choices {
                    ui.selectable_value(&mut chosen, port.clone(), text);
                }
            });
    });

    // `set_device_port` is the one that decides whether this is a change at all —
    // re-picking what is already set must not cost a snapshot down the channel.
    session.set_device_port(device, end, chosen)
}

/// What a closed picker shows. An unbound end says so rather than showing a
/// plausible default.
fn selected_text(current: Option<&PortRef>) -> String {
    match current {
        Some(p) => p.name.clone(),
        None => "— none —".into(),
    }
}

/// The choices for one end, in the order they are offered: "none" first, then
/// every connected port.
///
/// A port another box in this session holds *on the same end* is labelled with
/// that box's name. Picking it still works — the other box loses it — but the
/// strip says so before the click rather than after it.
fn port_choices(
    session: &Session,
    device: DeviceId,
    end: PortEnd,
    ports: &[PortInfo],
) -> Vec<(Option<PortRef>, String)> {
    let mut out: Vec<(Option<PortRef>, String)> = vec![(None, "— none —".into())];
    for port in ports {
        let port_ref = PortRef { id: port.id.clone(), name: port.name.clone() };
        let holder = session
            .devices
            .iter()
            .filter(|d| d.id != device)
            .find(|d| d.port(end).is_some_and(|p| p.same_port(&port_ref)))
            .map(|d| d.name.clone());
        let text = match holder {
            Some(name) => format!("{}  (on {})", port.name, name),
            None => port.name.clone(),
        };
        out.push((Some(port_ref), text));
    }
    out
}

// --- reading patch names off one box --------------------------------------------
//
// Packet E, stage 2 (2026-08-20), the button half. Every rule this obeys, and
// every sentence it can put on screen, belongs to `ui::sync`'s
// `patch_read_blocker`, `patch_read_job` and `read_patch_kit`, and to
// `Session::apply_patch_read` in `core::import` — this section wires those
// functions to a control and reimplements none of their logic (lesson 7: a
// function with no caller is half a feature, and the missing seam is
// invisible from up here until something reads the field it left behind).
//
// One button per box, in that box's own group, beside "Takes our clock" — the
// group this file already draws per box is the natural home the packet named,
// and nothing about it needed a second file.
//
// **A fetch is a real handshake and a real dump request** — up to a few
// seconds on hardware, the same round trip `ui::ports`' identify makes — so it
// runs on a worker thread exactly the way that panel's does, and this section
// polls for the answer. The in-flight receiver and the last result have
// nowhere to live in `patch_read_row`'s own arguments: `ui::setup` calls
// `devices::ui` fresh every frame and is not a file this packet may touch, so
// they live in this module instead, in a small map keyed on the box.
//
// **Every one of the read path's failures reaches the screen.** The blocker —
// no ports, a cable the OS no longer lists, a box with no patch names to read
// at all — is shown live, before a click, the same way a disabled control
// with a reason beats one that is merely absent. Everything a click itself
// can turn up — a handshake that never answers, the wrong box on the cable, a
// firmware this build cannot decode, a track-count mismatch — is shown after
// it, in the words those functions already chose. A read that fails silently
// and leaves the old patch names in place is the one bug this whole feature
// exists to fix, one level up.
//
// **The slot to read sits next to the button, in a picker.** Added 2026-08-20,
// the day after the button shipped: as first built, the slot was resolved from
// the pattern's own `source` and a pattern that had none was refused with
// "fetch it from a named slot first". Neil read that message on a pattern he
// had just built here and pointed out what it costs — he had not fetched
// anything, did not want to, and only wanted the names the box has on its
// sixteen tracks. The refusal was guarding the right rule (do not silently
// read A01) with the wrong remedy. A picker keeps the rule and drops the
// remedy: the slot is on screen before the click, so nothing is inferred, and
// [`patch_read_slot_default`] starts it where the answer almost always is.
//
// **Generic over how a job becomes a live `PatternIo`**, for the reason
// `ui::sync::run` is: production hands this a real `ElektronDevice`
// (`open_real`, below), and a test hands it a box built by hand from a
// fixture. The button, the click handling, the spawn and every frame of
// polling in between run identically either way — the same seam `ui::sync`'s
// own tests already use, rather than a second one invented here.

/// One box's patch-names read, mid-flight or just finished.
#[derive(Default)]
struct PatchRead {
    pending: Option<Receiver<PatchReadOutcome>>,
    /// The last thing this box's read said, and whether it was a failure —
    /// kept on screen until the next attempt overwrites it, rather than
    /// vanishing the frame after it lands.
    last: Option<(String, bool)>,
    /// The slot the user picked, once they have picked one. `None` means the
    /// picker is still showing [`patch_read_slot_default`]'s answer, which
    /// tracks the pattern rather than freezing at whatever it said the first
    /// frame this box was drawn — the same "pinned only once touched" rule
    /// `ui::transfer`'s `into_pinned` follows, for the same reason.
    from: Option<PatternRef>,
}

/// What the worker thread hands back: the job it was given, so a landed fetch
/// can be applied to the slot it was resolved against, and either a decoded
/// kit or the reason it could not get one.
struct PatchReadOutcome {
    job: PatchJob,
    result: Result<PatternKit, String>,
}

/// Keyed on the box rather than threaded through this function's arguments —
/// see this section's header for why.
static PATCH_READS: LazyLock<Mutex<HashMap<DeviceId, PatchRead>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Real hardware's route into [`read_patch_kit`]: identify, then let that
/// function make the one dump request. Mirrors `ui::sync::worker`'s own
/// opening closure exactly — the same two round trips, the same
/// `.to_string()` on a `MidiError`, aimed at one box's current kit instead of
/// a whole session's tracks.
fn open_real(job: &PatchJob) -> Result<ElektronDevice, String> {
    let mut device = ElektronDevice::open(&job.input, &job.output).map_err(|e| e.to_string())?;
    device.identify().map_err(|e| e.to_string())?;
    Ok(device)
}

/// Open the box and fetch, on a worker thread so a wedged box cannot freeze
/// the UI — the same reason `ui::ports`' identify does the same thing.
fn spawn_patch_read<D: PatternIo + Send + 'static>(
    job: PatchJob,
    open: impl FnOnce(&PatchJob) -> Result<D, String> + Send + 'static,
) -> Receiver<PatchReadOutcome> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let result = open(&job).and_then(|mut device| read_patch_kit(&mut device, &job));
        let _ = tx.send(PatchReadOutcome { job, result });
    });
    rx
}

/// "Read 16 track names from A01 on DT2" — the three facts a success owes:
/// how many tracks, which slot, which box.
fn patch_read_success(job: &PatchJob, count: usize) -> String {
    format!(
        "Read {count} track name{} from {} on {}",
        if count == 1 { "" } else { "s" },
        job.from().label(),
        job.name,
    )
}

/// What the slot picker shows until the user says otherwise: the slot the
/// pattern on screen came off the box from, or — when it never came off a box —
/// its own position in this session, which is the slot of the same name.
///
/// Not a guess dressed up as a default. The distinction that matters is that
/// this is *visible*: it sits in a picker next to the button with `A01` written
/// on it, so a read is aimed by whoever presses it rather than resolved out of
/// sight. A session pattern in A01 next to a box on A01 is the overwhelmingly
/// common desk, and starting the picker anywhere else would make the common
/// case take an extra click to say something the layout already said.
fn patch_read_slot_default(session: &Session, id: DeviceId) -> PatternRef {
    let at = session
        .slot_in_scene(session.current_scene, id)
        .unwrap_or_else(|| PatternRef::new(0, 0));
    session
        .device(id)
        .and_then(|d| d.pattern(at.slot()))
        .and_then(|p| p.source.as_ref())
        .map(|s| PatternRef::new(s.bank, s.index))
        .unwrap_or(at)
}

/// Take a landed fetch, if one has, and apply it. Returns whether the session
/// actually changed — only true on a successful [`Session::apply_patch_read`],
/// never on a refusal, so a caller's dirty flag is not set by a read that
/// touched nothing.
fn poll_patch_read(session: &mut Session, id: DeviceId) -> bool {
    let outcome = {
        let mut reads = PATCH_READS.lock().unwrap();
        let Some(state) = reads.get_mut(&id) else { return false };
        let landed = match &state.pending {
            Some(rx) => rx.try_recv().ok(),
            None => None,
        };
        let Some(outcome) = landed else { return false };
        state.pending = None;
        outcome
    };

    let (message, is_error, applied) = match outcome.result {
        Ok(kit) => {
            let seen_at = digi_core::import::now_unix_seconds();
            match session.apply_patch_read(outcome.job.device, outcome.job.at, &kit, &outcome.job.source, seen_at) {
                Ok(count) => (patch_read_success(&outcome.job, count), false, true),
                Err(e) => (e.to_string(), true, false),
            }
        }
        Err(why) => (why, true, false),
    };
    PATCH_READS.lock().unwrap().entry(id).or_default().last = Some((message, is_error));
    applied
}

/// The button and its status line, drawn alongside this box's other per-box
/// actions. Returns the button's own response — so a test can find where it
/// landed and click it for real — and whether this call changed the session.
fn patch_read_row<D: PatternIo + Send + 'static>(
    ui: &mut Ui,
    session: &mut Session,
    id: DeviceId,
    inputs: &[PortInfo],
    outputs: &[PortInfo],
    open: impl FnOnce(&PatchJob) -> Result<D, String> + Send + 'static,
) -> (egui::Response, bool, Option<(String, bool)>) {
    // A landed fetch is applied before this frame draws its own status line,
    // the same order `ui::ports`' panel polls in.
    let changed = poll_patch_read(session, id);

    let present = PortsPresent { inputs, outputs };
    let blocked = session.device(id).and_then(|d| patch_read_blocker(d, present));

    let mut reads = PATCH_READS.lock().unwrap();
    let busy = reads.get(&id).is_some_and(|s| s.pending.is_some());
    if busy {
        // The same 100ms cadence `ui::sync::SyncPanel::tick` polls a run at —
        // often enough that a landed fetch does not sit unseen for a visible
        // beat, cheap enough that it costs nothing while nothing is happening.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    let enabled = !busy && blocked.is_none();

    // What the picker is showing this frame — the user's pick if they have made
    // one, otherwise the default, recomputed every frame so it follows the
    // pattern on screen.
    let shown = reads
        .get(&id)
        .and_then(|s| s.from)
        .unwrap_or_else(|| patch_read_slot_default(session, id));
    let mut picked = shown;

    // `horizontal_wrapped` rather than `horizontal`, for `ui::transfer`'s
    // reason: the Setup panel is resizable and at its narrow end this folds
    // onto two lines instead of pushing the picker off the edge.
    let button = ui
        .horizontal_wrapped(|ui| {
            let button = ui
                .add_enabled(enabled, egui::Button::new("Read patch names").small())
                .on_hover_text(
                    "Fetch the kit in the slot beside this and fill in every track's \
                     patch — what sound each one is named after, not its notes.\n\n\
                     Read-only, and narrower than a fetch: it touches the patch names \
                     and nothing else. No notes, no lengths, no swing.",
                );
            ui.weak("from");
            egui::ComboBox::from_id_salt(("patch-read-from", id.0))
                .selected_text(egui::RichText::new(shown.label()).color(super::TEXT_DIMMER))
                .width(56.0)
                .show_ui(ui, |ui| {
                    for slot in crate::ui::transfer::wire_slots() {
                        ui.selectable_value(&mut picked, slot, slot.label());
                    }
                })
                .response
                .on_hover_text(
                    "Which of the box's slots to read the names from.\n\n                     This reads the kit saved in that slot, which is what the box \
                     has live while it is sitting on that pattern. Starts on the slot \
                     this pattern was fetched from, or on the slot of the same name \
                     when the pattern was made here.",
                );
            button
        })
        .inner;
    if picked != shown {
        reads.entry(id).or_default().from = Some(picked);
    }

    if button.clicked() {
        match patch_read_job(session, id, present, Some(shown)) {
            Ok(job) => {
                let rx = spawn_patch_read(job, open);
                // The pick survives its own read: `insert` would drop it back
                // to the default, and a box read at B03 must still say B03
                // afterwards.
                let state = reads.entry(id).or_default();
                state.pending = Some(rx);
                state.last = None;
            }
            Err(why) => {
                reads.entry(id).or_default().last = Some((why, true));
            }
        }
    }

    // The live blocker takes priority over a stale result — a box that was
    // readable a minute ago and just lost its cable must not go on showing
    // last minute's success.
    let status = match blocked {
        Some(why) => Some((why, true)),
        None => reads.get(&id).and_then(|s| s.last.clone()),
    };
    drop(reads);

    if let Some((text, is_error)) = &status {
        if *is_error {
            ui.colored_label(egui::Color32::LIGHT_RED, text);
        } else {
            ui.weak(text);
        }
    }

    // `status` is handed back too, not just drawn — so a test can assert on
    // exactly the string this frame put on screen, rather than on state no
    // control actually reached (`ui::edit`'s own reasoning for the same
    // choice). `ui::devices::ui`'s caller ignores it.
    (button, changed, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, name: &str) -> PortInfo {
        PortInfo { id: id.into(), name: name.into(), slug: None }
    }

    #[test]
    fn a_picker_offers_none_first_then_every_connected_port() {
        // "None" has to be reachable: the way to stop a box sounding without
        // unplugging it is to take its port away.
        let session = digi_core::default_session();
        let dt2 = session.devices[0].id;
        let ports = [info("iac1", "IAC Driver Bus 1"), info("out-1", "Elektron Digitakt II")];

        let choices = port_choices(&session, dt2, PortEnd::Output, &ports);

        assert_eq!(choices.len(), 3);
        assert!(choices[0].0.is_none());
        assert_eq!(choices[0].1, "— none —");
        assert_eq!(choices[1].1, "IAC Driver Bus 1");
        assert_eq!(choices[2].1, "Elektron Digitakt II");
    }

    #[test]
    fn a_picker_names_the_box_already_holding_a_port() {
        let mut session = digi_core::default_session();
        let (dt2, dn2) = (session.devices[0].id, session.devices[1].id);
        session.set_device_port(
            dt2,
            PortEnd::Output,
            Some(PortRef { id: "out-1".into(), name: "Elektron Digitakt II".into() }),
        );

        // Offered to the *DN2*: this is the row that would steal it.
        let choices = port_choices(&session, dn2, PortEnd::Output, &[info("out-1", "Elektron Digitakt II")]);

        assert_eq!(choices[1].1, "Elektron Digitakt II  (on DT2)");
    }

    #[test]
    fn a_box_does_not_report_itself_as_holding_its_own_port() {
        let mut session = digi_core::default_session();
        let dt2 = session.devices[0].id;
        session.set_device_port(
            dt2,
            PortEnd::Output,
            Some(PortRef { id: "out-1".into(), name: "Elektron Digitakt II".into() }),
        );

        let choices = port_choices(&session, dt2, PortEnd::Output, &[info("out-1", "Elektron Digitakt II")]);

        assert_eq!(choices[1].1, "Elektron Digitakt II", "no \"(on DT2)\" in the DT2's own row");
    }

    #[test]
    fn the_other_ends_holder_is_not_reported_on_this_end() {
        // A box's input and output usually share a name. Listing the outputs must
        // not claim the DT2 holds one because it holds the *input* of that name.
        let mut session = digi_core::default_session();
        let (dt2, dn2) = (session.devices[0].id, session.devices[1].id);
        session.set_device_port(
            dt2,
            PortEnd::Input,
            Some(PortRef { id: "in-1".into(), name: "Elektron Digitakt II".into() }),
        );

        let choices = port_choices(&session, dn2, PortEnd::Output, &[info("out-1", "Elektron Digitakt II")]);

        assert_eq!(choices[1].1, "Elektron Digitakt II", "the input is a different port");
    }

    #[test]
    fn an_unbound_end_says_so_rather_than_showing_a_plausible_default() {
        assert_eq!(selected_text(None), "— none —");
        assert_eq!(
            selected_text(Some(&PortRef { id: "iac1".into(), name: "IAC Driver Bus 1".into() })),
            "IAC Driver Bus 1"
        );
    }

    #[test]
    fn a_picker_offers_nothing_but_none_when_no_ports_are_connected() {
        let session = digi_core::default_session();
        let dt2 = session.devices[0].id;
        let choices = port_choices(&session, dt2, PortEnd::Output, &[]);
        assert_eq!(choices.len(), 1);
        assert!(choices[0].0.is_none());
    }
}

// --- the read-patch-names button: presence, driven clicks, every refusal ---------
//
// Packet E, stage 2. The engine's own tests (`ui::sync::patch_read_tests`)
// already cover the fetch itself — a real fixture decoded through a real
// `read_patch_kit`, every one of its four ordinary failures, and the
// no-source refusal. Nothing here repeats those assertions; this module is
// about the seam this file adds on top: does the button exist, does clicking
// it actually run that engine, and does every one of its outcomes reach the
// screen in words rather than silently doing (or not doing) something to the
// session.
#[cfg(test)]
mod patch_read_button_tests {
    use std::collections::BTreeMap;

    use digi_core::device::{model_for_key, Device as CoreDevice, DeviceIo, DT2};
    use digi_core::import::Fetched;
    use digi_core::session::PatternRef as Slot;
    use digi_protocol::device::{identity_from_responses, DeviceIdentity, DeviceResponse};
    use digi_protocol::pattern::decode_pattern_kit;
    use digi_protocol::protocol::{split_sysex_stream, SysExKind, DUMP_PATTERN_KIT};

    use super::*;

    const DT2_FIXTURE: &str = "digitakt2-A01-conditions-2026-08-02.syx";

    /// The same fixture, and the same extraction, `ui::sync::patch_read_tests`
    /// uses — a real captured pattern-kit dump, not a hand-built one, because
    /// `read_patch_kit`'s decode step only exercises for real against real
    /// bytes.
    fn fixture_payload() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../protocol/tests/fixtures")
            .join(DT2_FIXTURE);
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| panic!("reading fixture {}: {e}", path.display()));
        split_sysex_stream(&bytes)
            .into_iter()
            .filter(|m| m.kind == SysExKind::Dump)
            .filter_map(|m| m.dump)
            .find(|d| d.dump_type == DUMP_PATTERN_KIT)
            .map(|d| d.payload)
            .unwrap_or_else(|| panic!("{DT2_FIXTURE}: no pattern-kit dump"))
    }

    fn identity(product_id: u8, build: &str) -> DeviceIdentity {
        identity_from_responses(
            &DeviceResponse { product_id, supported_ids: vec![0x60], reported_name: String::new() },
            build.into(),
            "1.15B".into(),
        )
    }

    fn dt2_identity() -> DeviceIdentity {
        identity(42, "0070")
    }

    /// A box that answers [`PatternIo`] from an in-memory map — everything
    /// `read_patch_kit` needs and nothing it does not, `Send` so it can
    /// actually cross the worker thread `spawn_patch_read` opens, the same
    /// way a real `ElektronDevice` would.
    #[derive(Clone)]
    struct TestBox {
        identity: Option<DeviceIdentity>,
        slots: BTreeMap<u8, Vec<u8>>,
    }

    impl TestBox {
        fn new(identity: DeviceIdentity, index: u8, bytes: Vec<u8>) -> Self {
            Self { identity: Some(identity), slots: BTreeMap::from([(index, bytes)]) }
        }

        fn silent() -> Self {
            Self { identity: None, slots: BTreeMap::new() }
        }
    }

    impl PatternIo for TestBox {
        fn identity(&self) -> Option<&DeviceIdentity> {
            self.identity.as_ref()
        }

        fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
            self.slots.get(&index).cloned().ok_or_else(|| format!("no slot {index}"))
        }

        fn send_pattern_kit(&mut self, _index: u8, _payload: &[u8]) -> Result<(), String> {
            unreachable!("a patch-names read never sends")
        }
    }

    /// A never-called opener, for frames this test does not expect to click —
    /// a click that reached it would be the test's own bug, not the code
    /// under test's, so it panics rather than quietly answering something.
    fn unreachable_open(_job: &PatchJob) -> Result<TestBox, String> {
        unreachable!("this frame is not expected to click")
    }

    /// One DT2, ports set (so the blocker has nothing to refuse), with the
    /// fixture imported into A01 — so the pattern's own `source` names A01,
    /// exactly what a real fetch-then-import would leave behind. Mirrors
    /// `ui::sync::patch_read_tests::session_with_fixture`.
    fn session_with_fixture() -> (Session, DeviceId) {
        let mut session = Session::default();
        let id = session.add_device(CoreDevice::new("DT2", &DT2, 16));
        let spec = model_for_key("DT2").and_then(|m| m.spec()).expect("DT2 has a spec");
        let bytes = fixture_payload();
        let kit = decode_pattern_kit(spec, &bytes).expect("fixture decodes");
        session
            .import_pattern(
                id,
                Slot::new(0, 0),
                &Fetched { spec, kit: &kit, payload: &bytes, from: Slot::new(0, 0) },
            )
            .expect("a DT2 fixture into a DT2 slot");
        session.device_mut(id).expect("just added").io = DeviceIo {
            input: Some(PortRef { id: "dt2-in".into(), name: "DT2 in".into() }),
            output: Some(PortRef { id: "dt2-out".into(), name: "DT2 out".into() }),
            ..DeviceIo::default()
        };
        (session, id)
    }

    fn present_ports() -> (Vec<PortInfo>, Vec<PortInfo>) {
        (
            vec![PortInfo { id: "dt2-in".into(), name: "DT2 in".into(), slug: None }],
            vec![PortInfo { id: "dt2-out".into(), name: "DT2 out".into(), slug: None }],
        )
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    fn patches(session: &Session, id: DeviceId) -> Vec<Option<digi_core::model::TrackPatch>> {
        session.device(id).unwrap().pattern(0).unwrap().tracks().iter().map(|t| t.patch.clone()).collect()
    }

    // --- presence ----------------------------------------------------------------

    #[test]
    fn the_read_patch_names_button_is_present_for_a_box() {
        let ctx = egui::Context::default();
        let (mut session, id) = session_with_fixture();
        let (inputs, outputs) = present_ports();

        let mut rect = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let (response, changed, _) =
                patch_read_row(ui, &mut session, id, &inputs, &outputs, unreachable_open);
            assert!(!changed, "drawing the row alone must not touch the session");
            rect = Some(response.rect);
        });
        out.textures_delta.clear();
        let rect = rect.expect("the row draws a button every frame");
        assert!(rect.width() > 0.0 && rect.height() > 0.0, "the button occupies real space in the panel");
    }

    // --- driven: a real click, a fake that answers --------------------------------

    #[test]
    fn clicking_it_with_a_box_that_answers_populates_all_sixteen_tracks() {
        let ctx = egui::Context::default();
        let (mut session, id) = session_with_fixture();
        let (inputs, outputs) = present_ports();
        let fake = TestBox::new(dt2_identity(), 0, fixture_payload());

        // Frame 1: draw, and measure where the button actually landed —
        // `ui::edit`'s own established shape for driving a click rather than
        // asserting on state no control could reach.
        let center = {
            let mut rect = None;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let (response, _, _) =
                    patch_read_row(ui, &mut session, id, &inputs, &outputs, unreachable_open);
                rect = Some(response.rect);
            });
            out.textures_delta.clear();
            rect.unwrap().center()
        };

        // Frame 2: press down.
        let mut out = ctx.run_ui(
            egui::RawInput { events: vec![egui::Event::PointerMoved(center), press(center, true)], ..Default::default() },
            |ui| {
                let _ = patch_read_row(ui, &mut session, id, &inputs, &outputs, unreachable_open);
            },
        );
        out.textures_delta.clear();

        // Frame 3: release — the frame `.clicked()` fires on, and the one
        // that is handed the fake box that will actually answer. `run_ui`
        // wants an `FnMut` even though this frame body only ever runs once,
        // so the opener sits behind an `Option` and is taken rather than
        // moved directly — a plain move would demand the closure be
        // re-runnable, which an `FnOnce` opener is not.
        let mut opener = Some(move |_: &PatchJob| Ok(fake));
        let mut out = ctx.run_ui(egui::RawInput { events: vec![press(center, false)], ..Default::default() }, |ui| {
            let opener = opener.take().expect("this frame body runs exactly once");
            let _ = patch_read_row(ui, &mut session, id, &inputs, &outputs, opener);
        });
        out.textures_delta.clear();

        // The fetch runs on a worker thread — an in-memory lookup against
        // `TestBox`, so it lands within a frame or two in practice. Polled
        // rather than joined directly because the receiver lives inside this
        // module's own state, not in the test's hands; capped so a genuine
        // regression fails the assertion below instead of hanging the run
        // (the trap lesson 6 names: a `while` with no bound turns a stuck
        // worker into a hang the harness mistakes for a slow build).
        let mut applied = false;
        for _ in 0..200 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let (_, changed, _) =
                    patch_read_row(ui, &mut session, id, &inputs, &outputs, unreachable_open);
                applied |= changed;
            });
            out.textures_delta.clear();
            if applied {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(applied, "the fetch landed and apply_patch_read ran within the poll budget");

        let pattern = session.device(id).unwrap().pattern(0).unwrap();
        for t in 0..16 {
            assert!(pattern.track(t).unwrap().patch.is_some(), "track {t} must carry a record — every track was fetched");
        }
    }

    // --- a pattern that never came off a box ------------------------------------
    //
    // This used to be the refusal test: a pattern with no `source` had no slot
    // to resolve, so the click was turned away with "fetch it from a named slot
    // first". Neil hit exactly that on 2026-08-20 building a pattern here from
    // scratch and asked the question it deserved — what if I have not fetched,
    // and just want the names the box has on its tracks right now? So the row
    // grew a slot picker and this test grew with it: no provenance is no longer
    // a refusal, because the picker shows the slot and the person pressing the
    // button is the one naming it. The refusal itself still exists and is still
    // tested, one level down, where nothing names a slot at all
    // (`ui::sync::patch_read_tests` and `core`'s own import tests).

    #[test]
    fn clicking_it_on_a_pattern_that_never_came_off_a_box_reads_the_slot_the_picker_shows() {
        let ctx = egui::Context::default();
        let mut session = Session::default();
        let id = session.add_device(CoreDevice::new("DT2", &DT2, 16));
        session.device_mut(id).expect("just added").io = DeviceIo {
            input: Some(PortRef { id: "dt2-in".into(), name: "DT2 in".into() }),
            output: Some(PortRef { id: "dt2-out".into(), name: "DT2 out".into() }),
            ..DeviceIo::default()
        };
        let (inputs, outputs) = present_ports();
        // The picker is showing this session slot's own name, A01, so the box
        // is asked for slot 0 — and answers, from the real fixture.
        let fake = TestBox::new(dt2_identity(), 0, fixture_payload());
        assert!(
            patches(&session, id).iter().all(Option::is_none),
            "nothing here has ever been fetched — that is the whole point of this case"
        );

        let center = {
            let mut rect = None;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let (response, _, _) =
                    patch_read_row(ui, &mut session, id, &inputs, &outputs, unreachable_open);
                rect = Some(response.rect);
            });
            out.textures_delta.clear();
            rect.unwrap().center()
        };
        let mut out = ctx.run_ui(
            egui::RawInput { events: vec![egui::Event::PointerMoved(center), press(center, true)], ..Default::default() },
            |ui| {
                let _ = patch_read_row(ui, &mut session, id, &inputs, &outputs, unreachable_open);
            },
        );
        out.textures_delta.clear();

        let mut opener = Some(move |_: &PatchJob| Ok(fake));
        let mut out = ctx.run_ui(egui::RawInput { events: vec![press(center, false)], ..Default::default() }, |ui| {
            let opener = opener.take().expect("this frame body runs exactly once");
            let _ = patch_read_row(ui, &mut session, id, &inputs, &outputs, opener);
        });
        out.textures_delta.clear();

        let mut applied = false;
        let mut status = None;
        for _ in 0..200 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let (_, changed, s) =
                    patch_read_row(ui, &mut session, id, &inputs, &outputs, unreachable_open);
                applied |= changed;
                if changed {
                    status = s;
                }
            });
            out.textures_delta.clear();
            if applied {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(applied, "a pattern with no provenance is readable now, not refused");

        let (text, is_error) = status.expect("a success says so, and says which slot");
        assert!(!is_error, "{text}");
        assert!(text.contains("from A01"), "the success names the slot it read: {text}");
        let pattern = session.device(id).unwrap().pattern(0).unwrap();
        for t in 0..16 {
            let patch = pattern.track(t).unwrap().patch.as_ref();
            let patch = patch.unwrap_or_else(|| panic!("track {t} must carry a record"));
            assert_eq!(
                patch.from,
                digi_core::model::Source { device_slug: "digitakt2".into(), bank: 0, index: 0 },
                "the record says which slot on which box these names came from"
            );
        }
    }

    // --- what the picker starts on ------------------------------------------------

    #[test]
    fn the_picker_starts_on_where_the_pattern_came_from_or_on_its_own_slot() {
        // Fetched from B03 and landed in A01: the read follows the pattern, not
        // its position here, the same way `ui::write::aim` does.
        let (mut session, id) = session_with_fixture();
        let spec = model_for_key("DT2").and_then(|m| m.spec()).expect("DT2 has a spec");
        let bytes = fixture_payload();
        let kit = decode_pattern_kit(spec, &bytes).expect("fixture decodes");
        session
            .import_pattern(
                id,
                Slot::new(0, 0),
                &Fetched { spec, kit: &kit, payload: &bytes, from: Slot::new(1, 2) },
            )
            .expect("a DT2 fixture into a DT2 slot");
        assert_eq!(patch_read_slot_default(&session, id), Slot::new(1, 2));

        // Made here, never fetched: the slot of the same name, which is what a
        // box sitting alongside this session is almost always on.
        let mut fresh = Session::default();
        let fresh_id = fresh.add_device(CoreDevice::new("DT2", &DT2, 16));
        assert_eq!(patch_read_slot_default(&fresh, fresh_id), Slot::new(0, 0));
    }

    // --- the blocker: shown live, before any click --------------------------------

    #[test]
    fn no_ports_set_is_shown_live_and_the_button_is_disabled() {
        let ctx = egui::Context::default();
        let mut session = Session::default();
        let id = session.add_device(CoreDevice::new("DT2", &DT2, 16));
        let inputs: Vec<PortInfo> = Vec::new();
        let outputs: Vec<PortInfo> = Vec::new();

        let mut status = None;
        let mut enabled = true;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let (response, changed, s) =
                patch_read_row(ui, &mut session, id, &inputs, &outputs, unreachable_open);
            assert!(!changed);
            enabled = response.enabled();
            status = s;
        });
        out.textures_delta.clear();

        let (text, is_error) = status.expect("the blocker is shown without needing a click");
        assert!(is_error);
        assert_eq!(text, "No ports set — pick an in and an out for this box above");
        // `add_enabled(false, ..)`'s own effect: a disabled response reports
        // itself disabled, which is what stands between "drawn" and "a press
        // can reach it".
        assert!(!enabled, "no ports means nothing to click");
    }

    #[test]
    fn a_port_the_os_no_longer_lists_is_shown_live() {
        let ctx = egui::Context::default();
        let (mut session, id) = session_with_fixture();
        // A non-empty list that does not contain this box's ports — what
        // makes a port read as gone rather than "not enumerated yet"
        // (`PortsPresent::holds`'s own documented rule).
        let elsewhere = vec![PortInfo { id: "elsewhere".into(), name: "Some Other Port".into(), slug: None }];

        let mut status = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let (_, changed, s) =
                patch_read_row(ui, &mut session, id, &elsewhere, &elsewhere, unreachable_open);
            assert!(!changed);
            status = s;
        });
        out.textures_delta.clear();

        let (text, is_error) = status.expect("the blocker is shown without needing a click");
        assert!(is_error);
        assert!(text.contains("is no longer plugged in — reconnect the box to read its patch names"), "{text}");
    }

    // --- the fetch's own failures, driven through a real click --------------------

    /// Click through to a landed (`Err`) outcome and return what the status
    /// line said, polling the same way the success test does.
    fn click_and_wait_for_outcome(
        ctx: &egui::Context,
        session: &mut Session,
        id: DeviceId,
        inputs: &[PortInfo],
        outputs: &[PortInfo],
        fake: TestBox,
    ) -> (String, bool) {
        let center = {
            let mut rect = None;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let (response, _, _) = patch_read_row(ui, session, id, inputs, outputs, unreachable_open);
                rect = Some(response.rect);
            });
            out.textures_delta.clear();
            rect.unwrap().center()
        };
        let mut out = ctx.run_ui(
            egui::RawInput { events: vec![egui::Event::PointerMoved(center), press(center, true)], ..Default::default() },
            |ui| {
                let _ = patch_read_row(ui, session, id, inputs, outputs, unreachable_open);
            },
        );
        out.textures_delta.clear();
        // Same `Option`-take shape as the success test's frame 3, and for the
        // same reason: `run_ui` demands `FnMut`, and an `FnOnce` opener
        // cannot be moved out of a captured closure that must look
        // re-runnable to the type system even though it only runs once here.
        let mut opener = Some(move |_: &PatchJob| Ok(fake));
        let mut out = ctx.run_ui(egui::RawInput { events: vec![press(center, false)], ..Default::default() }, |ui| {
            let opener = opener.take().expect("this frame body runs exactly once");
            let _ = patch_read_row(ui, session, id, inputs, outputs, opener);
        });
        out.textures_delta.clear();

        for _ in 0..200 {
            let mut status = None;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let (_, _, s) = patch_read_row(ui, session, id, inputs, outputs, unreachable_open);
                status = s;
            });
            out.textures_delta.clear();
            if let Some(s) = status {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("no outcome landed within the poll budget");
    }

    #[test]
    fn handshake_refused_reaches_the_screen_and_touches_no_record() {
        let ctx = egui::Context::default();
        let (mut session, id) = session_with_fixture();
        let (inputs, outputs) = present_ports();
        let before = patches(&session, id);

        let (text, is_error) =
            click_and_wait_for_outcome(&ctx, &mut session, id, &inputs, &outputs, TestBox::silent());

        assert!(is_error);
        assert_eq!(text, "the box did not answer the identity handshake");
        assert_eq!(before, patches(&session, id), "a failed read must leave every existing record exactly as it was");
    }

    #[test]
    fn wrong_firmware_reaches_the_screen_and_touches_no_record() {
        let ctx = egui::Context::default();
        let (mut session, id) = session_with_fixture();
        let (inputs, outputs) = present_ports();
        let before = patches(&session, id);

        let mut bytes = fixture_payload();
        bytes[0..4].copy_from_slice(&999u32.to_be_bytes());
        let fake = TestBox::new(dt2_identity(), 0, bytes);

        let (text, is_error) = click_and_wait_for_outcome(&ctx, &mut session, id, &inputs, &outputs, fake);

        assert!(is_error);
        assert!(text.contains("unsupported") && text.contains("version"), "{text}");
        assert_eq!(before, patches(&session, id), "a failed read must leave every existing record exactly as it was");
    }

    #[test]
    fn the_wrong_box_on_the_cable_reaches_the_screen_and_touches_no_record() {
        let ctx = egui::Context::default();
        let (mut session, id) = session_with_fixture();
        let (inputs, outputs) = present_ports();
        let before = patches(&session, id);

        // A DN2 answering identity on the DT2's cabled ports.
        let dn2 = identity(43, "0049");
        let fake = TestBox::new(dn2, 0, fixture_payload());

        let (text, is_error) = click_and_wait_for_outcome(&ctx, &mut session, id, &inputs, &outputs, fake);

        assert!(is_error);
        assert!(text.contains("says it's a"), "{text}");
        assert!(text.contains("refusing to read"), "{text}");
        assert_eq!(before, patches(&session, id), "a failed read must leave every existing record exactly as it was");
    }
}
