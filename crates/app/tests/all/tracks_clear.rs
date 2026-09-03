//! Clicking a track cell and pressing Delete, driven end to end: real pointer
//! events into the real pane, a real key event after them, and the `Session`
//! that comes out the other side.
//!
//! **The click is the point of this file.** `ui::tracks`'s own tests can hand
//! the context a focused cell id and prove what the shortcut does with it; only
//! a real press on a real cell can prove that clicking one *is* what focuses
//! it, which is the whole mechanism — Delete is armed by focus, and egui gives
//! a clicked widget no focus of its own accord (only Tab and the arrow keys
//! move it). A suite that focused the cell by hand on both sides of the seam
//! would pass with the `request_focus` call deleted. That is
//! `DEVELOPMENT.md`'s standing lesson, and `tracks_clipboard.rs` says the same
//! thing about the chord it binds.

use digi_core::history::{Content, History};
use digi_core::{two_box_session, Note, PLockLane};
use digi_roll_studio::ui::tracks::{a_track_cell_has_focus, track, track_mut, Selection};
use digi_roll_studio::EngineLink;
use eframe::egui;

/// DT2 T01 in `two_box_session()` — the cell the click below lands on.
const FIRST: Selection = Selection { device: 0, track: 0 };

/// One pass of the real pane, with the same two rules
/// `tracks_clipboard.rs`'s helper carries: egui hit-tests against the
/// *previous* pass's layout, so an empty frame comes first, and the font-atlas
/// delta has to be cleared because there is no renderer here to take it.
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

/// A point inside the first box's first cell, from the same constants
/// `ui::tracks` lays the pane out with: the frame's inner margin (14 left, 12
/// top), then the gutter (46) and its gap (8) across, and the header (18) plus
/// the gap below it (10) down.
fn first_cell() -> egui::Pos2 {
    egui::Pos2::new(14.0 + 46.0 + 8.0 + 2.0, 12.0 + 18.0 + 10.0 + 2.0)
}

/// A whole primary click, as the platform sends it: move, press, release.
fn click(pos: egui::Pos2) -> (Vec<egui::Event>, Vec<egui::Event>) {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    (vec![egui::Event::PointerMoved(pos), button(true)], vec![button(false)])
}

/// The key an Apple keyboard's "delete" actually sends. `Key::Delete` is bound
/// too — `ui::tracks`'s own tests cover both — but this is the one the machine
/// this app is written on produces, so it is the one the end-to-end test uses.
fn delete() -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key: egui::Key::Backspace,
        physical_key: Some(egui::Key::Backspace),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

fn give_it_music(session: &mut digi_core::Session, selection: Selection) {
    let t = track_mut(session, selection).unwrap();
    t.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0), Note::new(4.0, 64, 1.0, 90, 0.0)];
    t.plocks = vec![
        PLockLane::new(Some("filter.cutoff".into()), None, Some("DT2".into()), false, vec![Some(64)]).unwrap(),
    ];
}

#[test]
fn clicking_a_cell_then_pressing_delete_empties_that_track() {
    let ctx = egui::Context::default();
    let mut session = two_box_session();
    let engine = EngineLink::default();
    give_it_music(&mut session, FIRST);

    // Start on a *different* track, so the assertion below cannot pass on the
    // default selection: the click has to be what picks this cell out.
    let mut selection = Selection { device: 1, track: 3 };
    let (press, release) = click(first_cell());

    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, press, &mut session, &mut selection, &engine);
    frame(&ctx, release, &mut session, &mut selection, &engine);
    assert_eq!(selection, FIRST, "the click selects the cell it landed on");

    let edited = frame(&ctx, delete(), &mut session, &mut selection, &engine);

    assert!(edited, "clearing a track is an edit, which is what opens the undo step in the shell");
    let cleared = track(&session, FIRST).unwrap();
    assert!(cleared.notes.is_empty(), "the trigs are gone");
    assert!(cleared.plocks.is_empty(), "and the p-lock lanes with them — locks ride on trigs");
}

/// The fact the piano roll's own Delete is guarded on
/// (`pianoroll::interact`'s "nothing holds focus" check): after a click on a
/// cell, the grid owns the key. Asserted through the public helper the roll's
/// guard and the two shell shortcuts all read, rather than by poking at
/// `ctx.memory()`, so it is the same question they ask.
#[test]
fn clicking_a_cell_is_what_hands_the_keyboard_to_the_grid() {
    let ctx = egui::Context::default();
    let mut session = two_box_session();
    let engine = EngineLink::default();
    let mut selection = FIRST;

    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    assert!(
        !a_track_cell_has_focus(&ctx, &session),
        "a drawn pane nobody has clicked holds no keyboard — the roll's Delete is still the roll's"
    );

    let (press, release) = click(first_cell());
    frame(&ctx, press, &mut session, &mut selection, &engine);
    frame(&ctx, release, &mut session, &mut selection, &engine);

    assert!(a_track_cell_has_focus(&ctx, &session), "the click armed the grid");
}

/// The other half of the same mechanism: clicking off a cell hands the keyboard
/// straight back, so Delete stops meaning "clear this track" the moment you
/// press anywhere else — which in the real window is nearly always the roll,
/// where Delete means "the notes I have selected". The click here lands on the
/// row's own box-id gutter: inside the pane, and not a cell.
#[test]
fn clicking_off_a_cell_gives_the_keyboard_back() {
    let ctx = egui::Context::default();
    let mut session = two_box_session();
    let engine = EngineLink::default();
    give_it_music(&mut session, FIRST);
    let mut selection = FIRST;

    let (press, release) = click(first_cell());
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, press, &mut session, &mut selection, &engine);
    frame(&ctx, release, &mut session, &mut selection, &engine);
    assert!(a_track_cell_has_focus(&ctx, &session), "armed by the click, as the test above proves");

    let gutter = egui::Pos2::new(14.0 + 2.0, 12.0 + 18.0 + 10.0 + 2.0);
    let (press, release) = click(gutter);
    frame(&ctx, press, &mut session, &mut selection, &engine);
    frame(&ctx, release, &mut session, &mut selection, &engine);

    assert!(!a_track_cell_has_focus(&ctx, &session), "a press anywhere else disarms the grid");
    let edited = frame(&ctx, delete(), &mut session, &mut selection, &engine);
    assert!(!edited, "and Delete is the roll's again");
    assert_eq!(track(&session, FIRST).unwrap().notes.len(), 2);
}

#[test]
fn delete_without_clicking_a_cell_leaves_every_track_alone() {
    let ctx = egui::Context::default();
    let mut session = two_box_session();
    let engine = EngineLink::default();
    give_it_music(&mut session, FIRST);
    let before = track(&session, FIRST).unwrap().clone();

    // Selected, drawn, and never clicked: exactly the state the pane is in
    // while someone is working in the roll below, where Delete means "the notes
    // I have selected" and must not also mean "this whole track".
    let mut selection = FIRST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    let edited = frame(&ctx, delete(), &mut session, &mut selection, &engine);

    assert!(!edited);
    assert_eq!(track(&session, FIRST).unwrap(), &before);
}

/// The status line a clear leaves says "Cmd+Z brings them back", and this is
/// that promise under test.
///
/// **It is not free, and it nearly was not true.** `edit::shortcuts` — the
/// window's undo, read from the shell — used to refuse to fire whenever
/// anything held keyboard focus, and a clicked TRACKS cell now does. Without
/// the exemption both shortcuts share (`tracks::typing_elsewhere`), clearing a
/// track would leave the one key that takes it back dead until you clicked
/// somewhere else first — a trap set by the very gesture that needs the escape.
/// The step is opened and committed here the way `main.rs` does it around a
/// frame that edited: snapshot first, `begin` after, `commit` with the pointer
/// up.
#[test]
fn cmd_z_still_undoes_a_clear_while_the_cleared_cell_holds_the_keyboard() {
    let ctx = egui::Context::default();
    let mut session = two_box_session();
    let engine = EngineLink::default();
    let mut roll = digi_roll_studio::ui::pianoroll::PianoRoll::default();
    let mut history = History::default();
    give_it_music(&mut session, FIRST);

    let mut selection = FIRST;
    let (press, release) = click(first_cell());
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, press, &mut session, &mut selection, &engine);
    frame(&ctx, release, &mut session, &mut selection, &engine);

    let before = Content::of(&session);
    assert!(frame(&ctx, delete(), &mut session, &mut selection, &engine), "the clear is the edit");
    history.begin(before);
    assert!(history.commit(&session), "an emptied track is a step worth keeping");
    assert!(track(&session, FIRST).unwrap().notes.is_empty());

    let cmd_z = vec![egui::Event::Key {
        key: egui::Key::Z,
        physical_key: Some(egui::Key::Z),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    }];
    let mut stepped = false;
    let mut output = ctx.run_ui(egui::RawInput { events: cmd_z, ..Default::default() }, |ui| {
        stepped = digi_roll_studio::ui::edit::shortcuts(ui, &mut session, &mut roll, &mut history);
    });
    output.textures_delta.clear();

    assert!(stepped, "Cmd+Z is not swallowed by the cell the clear was aimed at");
    let back = track(&session, FIRST).unwrap();
    assert_eq!(back.notes.len(), 2, "the trigs came back");
    assert_eq!(back.plocks.len(), 1, "and the p-lock lane with them");
}
