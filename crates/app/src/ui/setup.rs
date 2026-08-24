// The right panel: the desk. Which boxes are in the session, what they are
// plugged into, and who answered when we asked.
//
// **Why the box's in/out lives here and not on the left rail.** digi-roll kept
// its `Box` panel on the rail because it had one box and one panel column, and
// because the panel was really an *action on the pattern in front of you* — send
// this to the box, fetch that off it. A session has several boxes, and a port is
// a property of one of them, which puts routing next to the box it belongs to
// rather than next to the notes. So the rule this panel is built on: **the left
// rail is what you are composing, the right panel is what you are composing
// on.** Everything per-device — ports, clock, slot, identity, and later the
// fetch/write-back that digi-roll's `Box` panel did — is on this side, one group
// per box.
//
// The panel scrolls as one rather than each list scrolling on its own: nested
// scroll areas in a 300px column are worse than a long column.
//
// ## Two registers, Phase 10
//
// Before this the panel was five groups deep — BOXES, TRANSFER, SEND TO BOX,
// BACKUPS, PORTS — each added on its own terms, and **the two that can change
// what is on a device sat in exactly the same visual register as the three that
// cannot**. Fetch and Send were three inches apart and looked alike. That is the
// one thing in this app worth being unmissable about, so a heading and a frame
// were added around the two dangerous groups, and both were folded away by
// default: a write wants a deliberate reach, and opening a folded group was the
// first of the deliberate acts a confirm dialog completed.
//
// ## Transfer first, 2026-08-19
//
// The register still exists, but the shape carrying it changed, following
// `design_handoff_digi_roll_ui/README.md`'s "1a — Setup panel". Two things this
// panel used to spend most of its height on turned out to be rarely-touched
// once a desk is routed — the per-box port pickers and the raw MIDI port lists —
// while the two things a session is *for*, fetching a pattern and sending one
// back, were three folds deep and easy to lose under BOXES.
//
// So the reorganisation is not a new register, it is the same one read the
// other way round:
//
// * **The "who's connected" block collapses behind a one-line status strip**
//   ([`status_strip`]) that shows a dot per box and expands automatically the
//   moment a box is missing or a port will not open — the one time anyone
//   needs to see it. [`crate::ui::devices`] and the auto-connect toggle live
//   inside it now, rather than always being the first thing under the panel
//   title.
// * **IN and OUT sit in one bordered container** ([`transfer_container`]) so a
//   fetch and a send read as two halves of one round trip, both permanently
//   visible — no fold to open before the panel is useful. The old
//   "WRITES TO THE BOX" frame is gone as a *frame*; its warning moved onto the
//   amber rule that guards the OUT block's SEND buttons directly, which is
//   where a reader meets it a sentence before pressing one. The sentence itself
//   is behind that rule's ⓘ as of 2026-08-20 — the eyebrow and the rule are
//   still permanently on screen; see [`OUT_WARNING_EYEBROW`].
// * **SYNC EVERY TRACK stops being a fold and becomes a button**, full width,
//   at the foot of OUT — it already said what it was about to do in its own
//   label (`sync::headline`); a fold in front of it was a second click for no
//   second decision.
// * **BOXES & MIDI PORTS joins BACKUPS as a disclosure row.** The raw port
//   lists ([`crate::ui::ports`]) are diagnostic UI for the one time auto-detect
//   has failed, and the status strip above already catches that case — so the
//   22 rows of port names collapse the same way the backups list always did.
//
// What the reorganisation does **not** touch: every safety rule Phase 10 built
// still lives one call away in `write`/`sync`/`restore`, unchanged. This file
// still holds no byte offsets and no write logic of its own — it is chrome
// around three modules that already had it.
//
// The fold state — the status strip's, and the two disclosure rows' — lives in
// [`SetupPanel`] rather than in egui's memory, so it survives the panel being
// collapsed and reopened, for the same reason Phase 10's did.

use digi_core::Session;
use eframe::egui::{self, Ui};

use crate::engine::EngineLink;
use crate::ui::autoconnect::AutoConnect;
use crate::ui::devices;
use crate::ui::ports::PortsPanel;
use crate::ui::restore::RestorePanel;
use crate::ui::sync::SyncPanel;
use crate::ui::tracks::Selection;
use crate::ui::transfer::TransferPanel;
use crate::ui::write::WritePanel;

/// The OUT block's warning eyebrow and body — the "WRITES TO THE BOX" frame's
/// old sentence, moved onto the amber rule that guards the SEND buttons
/// directly. It is the one sentence in the panel that must not quietly reword
/// itself, so the two edits it has had are both written down:
///
/// * **"card" became "device", 2026-08-20** (Neil, testing). The word was
///   inherited from digi-roll and describes where the pattern physically lands,
///   which is not what a reader of this panel is thinking about — "device" is
///   what the rest of the app's own copy already calls the thing on the other
///   end of the port, so all three lines here now agree with it.
/// * **The body moved behind the eyebrow's ⓘ**, same pass, to buy back the
///   three wrapped lines it cost in a permanently-visible block — see
///   [`super::destructive_note_tip`] for why the eyebrow and the amber rule
///   stayed on screen when the sentence did not.
const OUT_WARNING_EYEBROW: &str = "OVERWRITES THE DEVICE";
const OUT_WARNING_BODY: &str =
    "Everything in here overwrites what is on the device. The destination is re-read and backed \
     up whole first, you agree to what it changes, and it is verified byte for byte afterwards.";

/// The IN block's reassurance line, under the fetch rows.
const IN_REASSURANCE: &str =
    "Reading is safe. It replaces the pattern here, never the one on the device.";

/// Fold state that has to survive the panel closing and reopening.
///
/// `devices_expanded` is the status strip's: it starts closed and a fault
/// forces it open, but a user is still free to close it again once nothing is
/// wrong. `ports_open` and `backups_open` are the two disclosure rows at the
/// foot of the panel.
#[derive(Default)]
pub struct SetupPanel {
    devices_expanded: bool,
    ports_open: bool,
    backups_open: bool,
}

/// Draw the panel. Returns `(the session changed, the × was clicked)`.
///
/// The engine is `&mut` for two reasons: the transfer group offers the tempo a
/// fetched pattern arrived carrying, and taking that offer has to reach the
/// engine the same way the transport's tempo field does — a snapshot alone does
/// not move a running clock — and the send group asks whether the transport is
/// running, because that is something its confirm dialog has to say.
///
/// `selection` is the track the roll is editing, which is what the send group
/// aims at until someone aims it somewhere else.
#[allow(clippy::too_many_arguments)]
pub fn ui(
    ui: &mut Ui,
    session: &mut Session,
    engine: &mut EngineLink,
    panel: &mut SetupPanel,
    ports: &mut PortsPanel,
    autoconnect: &mut AutoConnect,
    transfer: &mut TransferPanel,
    write: &mut WritePanel,
    sync: &mut SyncPanel,
    restore: &mut RestorePanel,
    selection: Selection,
) -> (bool, bool) {
    let close = super::panel_header(ui, "Setup");
    let mut changed = false;
    // One view of what is plugged in, taken once and handed to all three write
    // surfaces below. `PortsPanel` owns the enumeration, so a panel that asked
    // the OS for itself could disagree with the row drawn an inch above it.
    //
    // Copied rather than borrowed because `ports` is taken mutably further down
    // (BOXES & MIDI PORTS draws its own pickers). Two short `Vec`s a frame, on
    // lists that are single digits long — the alternative is re-enumerating per
    // panel, which is both slower and the disagreement this exists to prevent.
    let (present_in, present_out) = (ports.inputs().to_vec(), ports.outputs().to_vec());
    let present = crate::ui::write::PortsPresent { inputs: &present_in, outputs: &present_out };

    egui::ScrollArea::vertical()
        .id_salt("setup-panel")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            status_strip(ui, panel, session, engine, &present_out);
            if session.devices.is_empty() {
                // First launch, and every New: the desk is empty until
                // discovery or the row below fills it. Drawn unconditionally —
                // there is no strip to expand and nothing else to see.
                ui.add_space(4.0);
                empty_desk(ui, autoconnect);
                ui.add_space(6.0);
                changed |= devices::add_box_row(ui, session);
            } else if panel.devices_expanded {
                ui.add_space(4.0);
                let outcome = devices::ui(ui, session, engine, ports.inputs(), ports.outputs());
                changed |= outcome.changed;
                // A removed box must not boomerang back on discovery's next
                // scan while its cable is still in — see `AutoConnect::decline`.
                for (input, output) in outcome.declined {
                    autoconnect.decline(&input, &output);
                }
                ui.add_space(6.0);
                changed |= devices::add_box_row(ui, session);
                ui.add_space(6.0);
                autoconnect.ui(ui);
            }

            ui.add_space(8.0);
            super::section_header(ui, "DATA TRANSFER", None);

            egui::Frame::new()
                .fill(super::INSET_BG)
                .stroke(egui::Stroke::new(1.0, super::PANEL_BORDER))
                .inner_margin(egui::Margin::same(1))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());

                    // --- IN --------------------------------------------------
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(9, 8)).show(ui, |ui| {
                        direction_header(ui, false, super::TRIG_GREEN, "IN", "read a slot off the box");
                        ui.add_space(8.0);
                        // One transfer at a time, in any direction: each group's
                        // button is held off while any other is working.
                        let elsewhere = write.busy() || restore.busy() || sync.busy();
                        changed |= transfer.ui(ui, session, engine, elsewhere);
                        ui.add_space(7.0);
                        ui.label(
                            egui::RichText::new(IN_REASSURANCE).size(10.5).color(super::TEXT_DIMMER),
                        );
                    });

                    ui.painter().hline(
                        ui.min_rect().x_range(),
                        ui.cursor().top(),
                        egui::Stroke::new(1.0, super::PANEL_BORDER),
                    );

                    // --- OUT -------------------------------------------------
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(9, 8)).show(ui, |ui| {
                        direction_header(ui, true, super::WARN_AMBER, "OUT", "write onto the box");
                        ui.add_space(8.0);
                        super::destructive_note_tip(ui, OUT_WARNING_EYEBROW, OUT_WARNING_BODY);
                        ui.add_space(9.0);

                        let busy = transfer.busy();
                        write.ui(
                            ui,
                            session,
                            present,
                            selection,
                            busy || restore.busy() || sync.busy(),
                            engine.is_playing(),
                        );
                        ui.add_space(9.0);
                        sync.ui(
                            ui,
                            session,
                            present,
                            busy || restore.busy() || write.busy(),
                            engine.is_playing(),
                        );
                    });
                });

            ui.add_space(10.0);
            // Disclosure rows: the two things nobody needs on every visit.
            super::disclosure_row(ui, &mut panel.backups_open, "BACKUPS", "put a slot back", |ui| {
                restore.ui(ui, session, present, write.busy() || sync.busy() || transfer.busy(), engine.is_playing());
            });
            super::disclosure_row(ui, &mut panel.ports_open, "BOXES & MIDI PORTS", "auto-detected", |ui| {
                ports.ui(ui, session);
            });
        });

    (changed, close)
}

/// The connection status strip: replaces the old always-visible BOXES block
/// with a dot per box and one line of names, and expands [`SetupPanel::devices_expanded`]
/// itself — rather than only reporting a click for the caller to apply — because
/// the fault rule has to win regardless of what was clicked this frame, and the
/// two would otherwise have to be reconciled by every caller of this function
/// instead of once, here.
///
/// **Auto-detect drives it.** Live is [`devices::is_live`]'s own answer — the one
/// the per-box strip already uses — so the two views of "is this box heard"
/// cannot disagree. A fault turns that box's dot amber and forces the strip
/// open; it does not lock the toggle, so a user who has seen the fault and
/// wants the strip out of the way while they fix it can still close it, and it
/// reopens on the next frame if the fault is still there.
fn status_strip(
    ui: &mut Ui,
    panel: &mut SetupPanel,
    session: &Session,
    engine: &EngineLink,
    outputs: &[digi_midi::PortInfo],
) {
    if session.devices.is_empty() {
        // Nothing to summarise and nothing to fold: the caller draws
        // [`empty_desk`] in this state, and a strip above it saying "no boxes"
        // would be the same sentence twice.
        return;
    }

    let statuses: Vec<(String, bool)> =
        session.devices.iter().map(|d| (d.name.clone(), devices::is_live(d, engine, outputs))).collect();
    let any_fault = statuses.iter().any(|(_, live)| !live);
    if any_fault {
        panel.devices_expanded = true;
    }
    // The number of boxes with both ends assigned — a different fact from the
    // dots, which say whether the engine actually has the *output* open right
    // now. This one answers "how much of the desk have I finished wiring",
    // which is the number worth a glance when the dots above it are amber.
    let fully_wired =
        session.devices.iter().filter(|d| d.io.input.is_some() && d.io.output.is_some()).count();

    let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8));
    let response = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (_, live) in &statuses {
                    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(7.0), egui::Sense::hover());
                    if *live {
                        // The glow the mock gives a connected dot: a soft, wider
                        // translucent fill under the solid one, approximating the
                        // CSS `box-shadow` egui has no direct equivalent for.
                        ui.painter().circle_filled(rect.center(), rect.width(), super::TRIG_GREEN_GLOW);
                        ui.painter().circle_filled(rect.center(), rect.width() / 2.0, super::TRIG_GREEN);
                    } else {
                        ui.painter().circle_filled(rect.center(), rect.width() / 2.0, super::WARN_AMBER);
                    }
                }
                let names: Vec<&str> = statuses.iter().map(|(n, _)| n.as_str()).collect();
                let suffix = if any_fault { "needs attention" } else { "connected" };
                ui.label(
                    egui::RichText::new(format!("{} {suffix}", names.join(" · ")))
                        .size(11.0)
                        .color(super::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (icon, _) =
                        ui.allocate_exact_size(egui::Vec2::splat(9.0), egui::Sense::hover());
                    super::paint_fold_arrow(
                        ui.painter(),
                        icon,
                        !panel.devices_expanded,
                        super::TEXT_DIMMER,
                    );
                    ui.label(
                        egui::RichText::new(format!("{fully_wired} PORTS"))
                            .size(10.0)
                            .color(super::TEXT_DIMMER),
                    );
                });
            });
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_text(if panel.devices_expanded {
            "Hide the per-box port pickers"
        } else {
            "Show which port and clock setting each box has"
        });

    if response.clicked() {
        panel.devices_expanded = !panel.devices_expanded;
    }
}

/// The desk with no boxes on it: what a first launch shows, and what New goes
/// back to since 2026-08-24. One sentence saying what will happen by itself,
/// then the auto-connect controls — whose checkbox is the sentence's honesty:
/// with it off, "watching" would be a lie, so the copy follows the setting.
/// The caller draws `devices::add_box_row` underneath for the uncabled case.
fn empty_desk(ui: &mut Ui, autoconnect: &mut AutoConnect) {
    let copy = if autoconnect.enabled() {
        "No boxes yet. Watching for Elektron boxes — plug one in over USB and it \
         joins the session with its ports set."
    } else {
        "No boxes yet, and auto-connect is off — turn it on to find your boxes \
         over USB, or add one below."
    };
    ui.label(egui::RichText::new(copy).size(11.0).color(super::TEXT_SECONDARY));
    ui.add_space(6.0);
    autoconnect.ui(ui);
}

/// The `←`/`→` label row over each half of the transfer container.
fn direction_header(ui: &mut Ui, pointing_right: bool, colour: egui::Color32, label: &str, description: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(11.0, 13.0), egui::Sense::hover());
        super::paint_direction_arrow(ui.painter(), rect, pointing_right, colour);
        ui.label(egui::RichText::new(label).size(10.0).color(colour));
        ui.label(egui::RichText::new(description).size(11.0).color(super::TEXT_DIM));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        panel: &mut SetupPanel,
        session: &Session,
        engine: &EngineLink,
    ) {
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |u| {
            status_strip(u, panel, session, engine, &[]);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn the_transfer_copy_calls_it_a_device_and_never_a_card() {
        // Neil, 2026-08-20, testing: "the card" is not what anyone calls the
        // thing on the other end of the port. Three lines used it and all three
        // now say "device". The assertion is on the *word* rather than on the
        // finished sentences, because the failure to guard against is a later
        // rewrite reaching for the old vocabulary again — not a reword, which is
        // allowed, and which the doc comment on these constants asks for a note
        // about rather than forbidding.
        for line in [OUT_WARNING_EYEBROW, OUT_WARNING_BODY, IN_REASSURANCE] {
            assert!(
                !line.to_ascii_lowercase().contains("card"),
                "the data transfer copy still says card: {line}"
            );
            assert!(
                line.to_ascii_lowercase().contains("device"),
                "every line here names what it is talking about: {line}"
            );
        }
    }

    #[test]
    fn a_fold_row_toggles_open_and_shut_on_the_same_click() {
        // The mechanics of the click target, isolated from `status_strip`'s
        // fault-forcing rule — which makes "starts collapsed, a click opens it,
        // the same click closes it" untestable headlessly on that function,
        // since `is_live` can never be true without a real MIDI port open, so
        // every session with a device in it reads as a fault there.
        //
        // Exercises `super::disclosure_row` — promoted from this file's own
        // `fold_row` in the 2026-08-19 v2 side-panel pass, unchanged in
        // behaviour — rather than a local copy, so this still covers the
        // click target every panel's disclosure rows now share.
        let ctx = egui::Context::default();
        let mut open = false;
        let pos = egui::Pos2 { x: 10.0, y: 10.0 };
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let pass = |events: Vec<egui::Event>, open: &mut bool| {
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, |u| {
                super::super::disclosure_row(u, open, "BACKUPS", "put a slot back", |_ui| {});
            });
            output.textures_delta.clear();
        };

        pass(vec![], &mut open);
        assert!(!open);

        pass(vec![egui::Event::PointerMoved(pos), press(true)], &mut open);
        pass(vec![press(false)], &mut open);
        assert!(open, "clicking the row opens it");

        pass(vec![egui::Event::PointerMoved(pos), press(true)], &mut open);
        pass(vec![press(false)], &mut open);
        assert!(!open, "and the same click shuts it again");
    }

    #[test]
    fn a_box_with_no_out_port_open_forces_the_strip_open_even_after_a_manual_close() {
        // `two_box_session`'s boxes have no ports at all yet, so `is_live` is
        // false for both of them and the strip has to force itself open — the
        // "missing box" half of "auto-detect drives this".
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();
        let engine = EngineLink::default();
        let mut panel = SetupPanel { devices_expanded: false, ..SetupPanel::default() };

        frame(&ctx, vec![], &mut panel, &session, &engine);
        assert!(panel.devices_expanded, "an unrouted box is a fault, and a fault forces the strip open");

        // A click can still close it for a frame — someone who has seen the
        // fault and wants it out of the way while they fix the cable — but the
        // fault is still true on the very next pass, and it wins again.
        let pos = egui::Pos2 { x: 20.0, y: 10.0 };
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, vec![egui::Event::PointerMoved(pos), press(true)], &mut panel, &session, &engine);
        frame(&ctx, vec![press(false)], &mut panel, &session, &engine);
        // The release toggles it closed *within* the same pass the fault-check
        // already ran for, so the next pass — with no input at all — is the one
        // that shows the fault winning back.
        frame(&ctx, vec![], &mut panel, &session, &engine);
        assert!(panel.devices_expanded, "the fault re-forces the strip open on the next pass");
    }

    #[test]
    fn a_fully_routed_desk_stays_collapsed() {
        // Can't open a real port in a headless test, so this only checks the
        // shape of the rule: no devices at all is the one case `is_live` cannot
        // call a fault, because there is nothing to be unheard.
        let ctx = egui::Context::default();
        let session = digi_core::Session::default();
        let engine = EngineLink::default();
        let mut panel = SetupPanel::default();

        frame(&ctx, vec![], &mut panel, &session, &engine);
        assert!(!panel.devices_expanded, "no boxes at all is not a fault to surface");
    }
}
