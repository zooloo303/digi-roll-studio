//! Whole-track copy/paste, driven the way `edit_panel.rs` drives its own
//! panel: a real headless egui pass, real key events, and assertions on the
//! `Session` that comes out the other side.
//!
//! **What this can and cannot say.** It proves the public contract — a
//! keypress reaching `digi_core::track_clip::paste_track` through
//! `ui::tracks::ui` and changing the session the way the brief specifies, with
//! nothing stubbed on either side. It does not touch `tracks::ui`'s private
//! clipboard state (the `Clipboard` struct, its status message, or the
//! copied-cell ring `paint_cell` draws) — those are covered directly by
//! `ui::tracks`'s own `#[cfg(test)] mod tests`, which sits in the same module
//! and can see them. Nor can it say the ring is legible on screen: that is a
//! run-the-app-and-look check, the same class `DEVELOPMENT.md` lesson 8 lists
//! for the rest of this pane's drawing.
//!
//! This is the seam `PLAN.md` and `DEVELOPMENT.md` both named as missing:
//! `protocol::copy_track` and `edit_ops::place_clipboard` shipped complete
//! with no caller, and the TRACKS grid's own click-Cmd+C-click-Cmd+V is the
//! third one, in-app rather than cross-device.

use digi_core::model::TrackScale;
use digi_core::{default_session, Note, PLockLane};
use digi_roll_studio::ui::tracks::{track, track_mut, Selection};
use digi_roll_studio::EngineLink;
use eframe::egui;

/// The chord the pane actually binds, built the way `egui-winit` really
/// delivers it.
///
/// **Deliberately not `Modifiers::COMMAND`.** `egui-winit` intercepts the
/// platform clipboard chord before it can become a key event
/// (`is_copy_command`/`is_paste_command`, 0.36.1 `src/lib.rs` ~1019): it pushes
/// `Event::Copy`/`Event::Paste` and returns, so `Event::Key { key: Key::C, .. }`
/// with Command held is an event **the real app never receives**. The first cut
/// of this feature matched on exactly that and shipped dead while every test
/// here passed, because these tests were handing the context an event the
/// platform does not produce. Building the input the code expects rather than
/// the input the platform sends is a suite that cannot fail.
fn shift_key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::SHIFT,
    }
}

/// A Command-modified letter, for the one test that proves the pane ignores it.
fn cmd_key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    }
}

/// One pass of the real pane. Two rules carried over from `ui::tracks`'s own
/// `frame` helper and `edit_panel.rs`'s `draw`: egui hit-tests against the
/// *previous* pass's layout, so a plain empty-event frame is how a test
/// establishes one before the frame that matters, and the font-atlas delta has
/// to be cleared because there is no renderer here to hand it to.
fn frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    session: &mut digi_core::Session,
    selection: &mut Selection,
    engine: &EngineLink,
) -> bool {
    let input = egui::RawInput { events, ..Default::default() };
    let mut edited = false;
    let mut output = ctx.run_ui(input, |ui| {
        edited = digi_roll_studio::ui::tracks::ui(ui, session, selection, engine);
    });
    output.textures_delta.clear();
    edited
}

/// DT2 T01 in `default_session()`.
const SOURCE: Selection = Selection { device: 0, track: 0 };
/// DT2 T06 — a different track on the same box, the ordinary case.
const DEST: Selection = Selection { device: 0, track: 5 };
/// DN2 T01 — a different *kind* of box, for the cross-device cases.
const DN2_DEST: Selection = Selection { device: 1, track: 0 };

#[test]
fn shift_c_then_shift_v_copies_the_music_and_leaves_the_destination_s_routing_alone() {
    let ctx = egui::Context::default();
    let mut session = default_session();
    let engine = EngineLink::default();

    {
        let source = track_mut(&mut session, SOURCE).unwrap();
        source.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0), Note::new(4.0, 64, 1.0, 90, 0.0)];
        source.plocks = vec![
            PLockLane::new(Some("filter.cutoff".into()), None, Some("DT2".into()), false, vec![Some(90)])
                .unwrap(),
        ];
        source.length_steps = 32;
        source.scale = TrackScale::Two;
        source.track_prob = 60;
    }
    {
        // The destination's own identity, set to values a wholesale
        // replacement would clobber — the plant this test would catch.
        let dest = track_mut(&mut session, DEST).unwrap();
        dest.name = "Snare".into();
        dest.channel = 9;
        dest.mute = true;
        dest.out_port = Some("dest-port".into());
    }

    let mut selection = SOURCE;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    assert!(
        !frame(&ctx, vec![shift_key(egui::Key::C)], &mut session, &mut selection, &engine),
        "a copy alone never edits the session"
    );

    selection = DEST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    assert!(
        frame(&ctx, vec![shift_key(egui::Key::V)], &mut session, &mut selection, &engine),
        "the paste is an edit worth an undo step"
    );

    let dest = track(&session, DEST).unwrap();
    let pitches: Vec<u8> = dest.notes.iter().map(|n| n.pitch).collect();
    assert_eq!(pitches, vec![60, 64]);
    assert_eq!(dest.length_steps, 32);
    assert_eq!(dest.scale, TrackScale::Two);
    assert_eq!(dest.track_prob, 60);
    assert_eq!(dest.plocks.len(), 1);
    assert_eq!(dest.plocks[0].name.as_deref(), Some("filter.cutoff"));

    // Studio state is the destination's own and must survive the paste.
    assert_eq!(dest.name, "Snare");
    assert_eq!(dest.channel, 9);
    assert!(dest.mute);
    assert_eq!(dest.out_port.as_deref(), Some("dest-port"));

    // This was a copy, not a move: the source is untouched.
    let source = track(&session, SOURCE).unwrap();
    assert_eq!(source.notes.len(), 2);

    // And the ids are fresh, not shared with the source.
    let source_ids: Vec<u32> = source.notes.iter().map(|n| n.id).collect();
    let dest_ids: Vec<u32> = dest.notes.iter().map(|n| n.id).collect();
    assert!(source_ids.iter().all(|id| !dest_ids.contains(id)));
}

#[test]
fn pasting_onto_the_copied_cell_itself_is_a_silent_no_op() {
    let ctx = egui::Context::default();
    let mut session = default_session();
    let engine = EngineLink::default();
    {
        let t = track_mut(&mut session, SOURCE).unwrap();
        t.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
    }
    let before = track(&session, SOURCE).unwrap().clone();

    let mut selection = SOURCE;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, vec![shift_key(egui::Key::C)], &mut session, &mut selection, &engine);
    let edited = frame(&ctx, vec![shift_key(egui::Key::V)], &mut session, &mut selection, &engine);

    assert!(!edited, "copying a track onto itself must not flag an edit");
    assert_eq!(track(&session, SOURCE).unwrap(), &before, "and must not touch the track either");
}

#[test]
fn pasting_with_nothing_ever_copied_does_nothing() {
    let ctx = egui::Context::default();
    let mut session = default_session();
    let engine = EngineLink::default();
    let before = track(&session, DEST).unwrap().clone();

    let mut selection = DEST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    let edited = frame(&ctx, vec![shift_key(egui::Key::V)], &mut session, &mut selection, &engine);

    assert!(!edited);
    assert_eq!(track(&session, DEST).unwrap(), &before);
}

#[test]
fn pasting_after_the_copied_device_is_removed_fails_safely() {
    let ctx = egui::Context::default();
    let mut session = default_session(); // [DT2, DN2]
    let engine = EngineLink::default();
    {
        let t = track_mut(&mut session, DN2_DEST).unwrap();
        t.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
    }

    let mut selection = DN2_DEST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, vec![shift_key(egui::Key::C)], &mut session, &mut selection, &engine);

    // The scene changed, or Setup dropped the box — either way, the DN2 this
    // clipboard points at is simply no longer in the session.
    session.devices.truncate(1);

    selection = SOURCE; // still resolves: DT2 T01
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    let before = track(&session, SOURCE).unwrap().clone();
    let edited = frame(&ctx, vec![shift_key(egui::Key::V)], &mut session, &mut selection, &engine);

    assert!(!edited, "a paste that cannot resolve its source must not edit anything");
    assert_eq!(track(&session, SOURCE).unwrap(), &before);
}

#[test]
fn a_cross_device_paste_still_edits_even_though_a_raw_lane_could_not_cross() {
    let ctx = egui::Context::default();
    let mut session = default_session();
    let engine = EngineLink::default();
    {
        let t = track_mut(&mut session, SOURCE).unwrap(); // DT2 T01
        t.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
        // Unnamed and raw: only ever meaningful on a DT2's own numbering.
        t.plocks = vec![PLockLane::new(None, Some(200), Some("DT2".into()), false, vec![Some(10)]).unwrap()];
    }

    let mut selection = SOURCE;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, vec![shift_key(egui::Key::C)], &mut session, &mut selection, &engine);

    selection = DN2_DEST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    let edited = frame(&ctx, vec![shift_key(egui::Key::V)], &mut session, &mut selection, &engine);

    assert!(edited, "the note still crosses even though the lane didn't");
    let dest = track(&session, DN2_DEST).unwrap();
    assert_eq!(dest.notes.len(), 1);
    assert!(dest.plocks.is_empty(), "a raw DT2 lane means nothing on a DN2's numbering");
}

#[test]
fn a_cell_with_no_track_under_it_cannot_be_copied() {
    // A selection past the end of the session — the same "no track selected"
    // case `track`/`track_mut` already handle elsewhere in this pane.
    let ctx = egui::Context::default();
    let mut session = default_session();
    let engine = EngineLink::default();
    let empty_selection = Selection { device: 9, track: 0 };

    let mut selection = empty_selection;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, vec![shift_key(egui::Key::C)], &mut session, &mut selection, &engine);

    selection = DEST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    let before = track(&session, DEST).unwrap().clone();
    let edited = frame(&ctx, vec![shift_key(egui::Key::V)], &mut session, &mut selection, &engine);

    assert!(!edited, "there was never anything on the clipboard to paste");
    assert_eq!(track(&session, DEST).unwrap(), &before);
}

/// The regression test for the way this feature first shipped: bound to
/// `Modifiers::COMMAND` with `Key::C`, which `egui-winit` never delivers.
///
/// It pins the binding from the other side — a Command-modified C or V must do
/// **nothing** here. That matters for two separate reasons. It documents that
/// the platform chord is not this pane's to take, so a later change that
/// "helpfully" also accepts Command cannot pass unnoticed. And it keeps plain
/// Cmd+C/Cmd+V free for text fields and for the note-level clipboard
/// `core::edit_ops::place_clipboard` is waiting to become.
///
/// Note what this test can and cannot claim. It proves the pane ignores a
/// COMMAND-modified letter *if one arrives*. It cannot prove the real platform
/// converts Cmd+C into `Event::Copy` before that — no headless test can, because
/// that translation happens in `egui-winit`, above the `Context` a test drives.
/// That fact is pinned by the quoted source location in
/// `ui::tracks::handle_clipboard_shortcuts`'s doc comment, not by an assertion.
#[test]
fn a_command_modified_letter_is_not_this_pane_s_shortcut() {
    let ctx = egui::Context::default();
    let mut session = digi_core::default_session();
    let engine = EngineLink::default();

    {
        let source = track_mut(&mut session, SOURCE).unwrap();
        source.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
    }

    let mut selection = SOURCE;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    assert!(
        !frame(&ctx, vec![cmd_key(egui::Key::C)], &mut session, &mut selection, &engine),
        "Cmd+C is not this pane's copy and must not edit anything"
    );

    // Nothing was copied, so a Shift+V onto an empty destination has nothing to
    // place — which is how this asserts the Cmd+C above really was ignored,
    // rather than merely not having reported an edit of its own.
    selection = DEST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    assert!(
        !frame(&ctx, vec![shift_key(egui::Key::V)], &mut session, &mut selection, &engine),
        "with nothing on the clipboard there is nothing to paste"
    );
    assert!(
        track(&session, DEST).unwrap().notes.is_empty(),
        "the destination must still be empty"
    );

    // And Cmd+V is likewise not the paste, even with a real copy on the board.
    selection = SOURCE;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, vec![shift_key(egui::Key::C)], &mut session, &mut selection, &engine);
    selection = DEST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    assert!(
        !frame(&ctx, vec![cmd_key(egui::Key::V)], &mut session, &mut selection, &engine),
        "Cmd+V is not this pane's paste"
    );
    assert!(
        track(&session, DEST).unwrap().notes.is_empty(),
        "a Cmd+V must not have pasted the copied track"
    );
}
