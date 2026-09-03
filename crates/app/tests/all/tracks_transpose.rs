//! Transposing a track from the keyboard, driven end to end: real key events
//! into the real pane, and the `Session` that comes out the other side.
//!
//! **The contrast with `tracks_clear.rs` is the point of this file.** Delete is
//! armed by the click, because the roll one pane down spends the same key on
//! the notes it has selected. Shift+Up and Shift+Down collide with nothing —
//! egui moves focus on *unmodified* arrows only — so they act on the selected
//! track from wherever the keyboard happens to be, which is what makes "drop
//! this part an octave" something you do while looking at the roll rather than
//! after going to find the track's cell. A suite that focused a cell first on
//! both sides of that seam would pass with the difference deleted.

use digi_core::history::{Content, History};
use digi_core::{two_box_session, Note};
use digi_roll_studio::ui::tracks::{track, track_mut, Selection};
use digi_roll_studio::EngineLink;
use eframe::egui;

/// DT2 T01 in `two_box_session()`.
const FIRST: Selection = Selection { device: 0, track: 0 };

/// One pass of the real pane, with `tracks_clear.rs`'s two rules: egui
/// hit-tests against the previous pass's layout, so an empty frame comes first,
/// and the font-atlas delta has to be cleared because there is no renderer here.
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

/// A point inside the first box's first cell, from `ui::tracks`'s own layout
/// constants — the same arithmetic `tracks_clear.rs` documents.
fn first_cell() -> egui::Pos2 {
    egui::Pos2::new(14.0 + 46.0 + 8.0 + 2.0, 12.0 + 18.0 + 10.0 + 2.0)
}

fn click(pos: egui::Pos2) -> (Vec<egui::Event>, Vec<egui::Event>) {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    (vec![egui::Event::PointerMoved(pos), button(true)], vec![button(false)])
}

/// Shift+Down, as the platform sends it: an arrow is not text, so winit hands
/// egui a plain key event with the modifier on it — none of the interception
/// that makes Cmd+C unreachable (`tracks::handle_clipboard_shortcuts`) applies
/// to this one.
fn shift_arrow(key: egui::Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::SHIFT,
    }]
}

fn give_it_music(session: &mut digi_core::Session, selection: Selection) {
    track_mut(session, selection).unwrap().notes =
        vec![Note::new(0.0, 60, 1.0, 100, 0.0), Note::new(4.0, 64, 1.0, 90, 0.0)];
}

fn pitches(session: &digi_core::Session, selection: Selection) -> Vec<u8> {
    track(session, selection).unwrap().notes.iter().map(|n| n.pitch).collect()
}

#[test]
fn shift_down_drops_the_selected_track_an_octave_with_nothing_clicked() {
    let ctx = egui::Context::default();
    let mut session = two_box_session();
    let engine = EngineLink::default();
    give_it_music(&mut session, FIRST);

    // Never clicked, so nothing holds the keyboard — the state the pane is in
    // while someone is working in the roll below. Delete would be dead here,
    // and this must not be.
    let mut selection = FIRST;
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    let edited = frame(&ctx, shift_arrow(egui::Key::ArrowDown), &mut session, &mut selection, &engine);

    assert!(edited, "a transpose is an edit, which is what opens the undo step in the shell");
    assert_eq!(pitches(&session, FIRST), [48, 52]);
}

#[test]
fn clicking_a_cell_first_moves_the_track_that_click_picked_out() {
    let ctx = egui::Context::default();
    let mut session = two_box_session();
    let engine = EngineLink::default();
    give_it_music(&mut session, FIRST);

    // Start somewhere else, so the assertion cannot pass on the default
    // selection: the click has to be what redirects the keystroke.
    let elsewhere = Selection { device: 1, track: 3 };
    let mut selection = elsewhere;
    let (press, release) = click(first_cell());
    frame(&ctx, vec![], &mut session, &mut selection, &engine);
    frame(&ctx, press, &mut session, &mut selection, &engine);
    frame(&ctx, release, &mut session, &mut selection, &engine);
    assert_eq!(selection, FIRST);

    frame(&ctx, shift_arrow(egui::Key::ArrowUp), &mut session, &mut selection, &engine);

    assert_eq!(pitches(&session, FIRST), [72, 76]);
    assert!(track(&session, elsewhere).unwrap().notes.is_empty(), "and only that one moved");
}

/// The promise the status line makes — "Cmd+Z takes it back" — under test,
/// through the same `edit::shortcuts` the shell reads it from. Cheap here and
/// worth having: the shortcut's guard is `tracks::typing_elsewhere`, and a
/// clicked cell holds the keyboard, so this is the exemption that keeps the one
/// key that undoes a transpose alive right after one.
#[test]
fn cmd_z_takes_a_transpose_back() {
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
    assert!(frame(&ctx, shift_arrow(egui::Key::ArrowUp), &mut session, &mut selection, &engine));
    history.begin(before);
    assert!(history.commit(&session), "a moved track is a step worth keeping");
    assert_eq!(pitches(&session, FIRST), [72, 76]);

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

    assert!(stepped, "Cmd+Z is not swallowed by the cell the transpose was aimed at");
    assert_eq!(pitches(&session, FIRST), [60, 64]);
}
