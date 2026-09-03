//! The two shortcuts the *window* owns and no panel does: the rail's letters and
//! `Cmd+S`.
//!
//! Each has its own tests next to the code that reads it — `ui::rail`'s say what
//! a letter does to the sidebars, `session_panel.rs` says what a save does to the
//! disk. What only this file can say is that the two live together: `S` opens
//! Song and does not save, `Cmd+S` saves and does not open Song, and neither
//! eats the other's event on the way past. They are read one after the other in
//! `main.rs`, from the same queue, and that is the seam worth a test.
//!
//! The events are built the way `egui-winit` really sends them — a bare letter
//! with the printable character beside it, a command chord with none (`lib.rs`
//! ~1060 suppresses text while ctrl or cmd is down) — for the reason
//! `tracks_clipboard.rs` spells out: a test that feeds the input the code
//! expects rather than the input the platform produces cannot fail.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use digi_core::{two_box_session, Session};
use digi_roll_studio::ui::console::Console;
use digi_roll_studio::ui::rail::{self, Sidebars, Tool};
use digi_roll_studio::ui::session::{Chooser, SessionPanel, Status};
use eframe::egui;

// ------------------------------------------------------------- the harness

/// A [`Chooser`] that answers with one path and records whether it was asked.
/// The native dialog is the one thing no test here can open; what matters is
/// whether the shortcut reached for it at all.
struct OnePath {
    answer: Option<PathBuf>,
    asked: Rc<RefCell<usize>>,
}

impl Chooser for OnePath {
    fn save_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        *self.asked.borrow_mut() += 1;
        self.answer.clone()
    }
    fn open(&mut self) -> Option<PathBuf> {
        None
    }
    fn export_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        None
    }
    fn export_midi_as(&mut self, _suggested: &str) -> Option<PathBuf> {
        None
    }
    fn open_midi(&mut self) -> Option<PathBuf> {
        None
    }
}

/// A panel that will save to `path` when it is first asked for one.
fn panel_saving_to(path: Option<PathBuf>) -> (SessionPanel, Rc<RefCell<usize>>) {
    let asked = Rc::new(RefCell::new(0));
    let chooser = OnePath { answer: path, asked: Rc::clone(&asked) };
    (SessionPanel::with_chooser(Box::new(chooser)), asked)
}

/// A directory of this test run's own, the way `session_panel.rs` makes one.
fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("digi-roll-shell-keys-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    dir
}

/// A bare letter: the key, and the character `egui-winit` pushes beside it.
fn letter(key: egui::Key) -> Vec<egui::Event> {
    vec![
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Text(key.name().to_ascii_lowercase()),
    ]
}

/// A command chord. No `Event::Text`: `egui-winit` drops the character while
/// ctrl or cmd is held, which is why this shortcut has no twin to clean up.
fn chord(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }]
}

fn release(key: egui::Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

/// One shell pass, in `main.rs`'s order: the rail's letters, then the save.
fn frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    bars: &mut Sidebars,
    panel: &mut SessionPanel,
    session: &Session,
) {
    let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        rail::shortcuts(ui, bars, session);
        panel.save_shortcut(ui.ctx(), session);
    });
    output.textures_delta.clear();
}

/// A press and the release after it.
fn tap(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    key: egui::Key,
    bars: &mut Sidebars,
    panel: &mut SessionPanel,
    session: &Session,
) {
    frame(ctx, events, bars, panel, session);
    frame(ctx, release(key), bars, panel, session);
}

/// What the console was told this frame.
fn said(ctx: &egui::Context) -> Option<String> {
    let mut console = Console::default();
    console.collect(ctx);
    console.latest().map(|entry| entry.text.clone())
}

// ------------------------------------------------------------- the tests

#[test]
fn a_bare_s_opens_song_and_saves_nothing() {
    let path = tmp_dir("bare-s").join("never-written.json");
    let ctx = egui::Context::default();
    let session = two_box_session();
    let mut bars = Sidebars::default();
    let (mut panel, asked) = panel_saving_to(Some(path.clone()));

    tap(&ctx, letter(egui::Key::S), egui::Key::S, &mut bars, &mut panel, &session);

    assert_eq!((bars.tool, bars.tool_open), (Tool::Song, true), "S is Song's");
    assert_eq!(*asked.borrow(), 0, "and it must not have gone looking for a file");
    assert!(!path.exists(), "nothing was written");
    assert_eq!(panel.status(), None);
}

#[test]
fn cmd_s_saves_and_leaves_the_rail_where_it_was() {
    let path = tmp_dir("cmd-s").join("session.json");
    let ctx = egui::Context::default();
    let session = two_box_session();
    let mut bars = Sidebars::default();
    let (mut panel, asked) = panel_saving_to(Some(path.clone()));

    tap(
        &ctx,
        chord(egui::Key::S, egui::Modifiers::COMMAND),
        egui::Key::S,
        &mut bars,
        &mut panel,
        &session,
    );

    assert!(!bars.tool_open, "the save must not open a panel — that is the whole point of it");
    assert_eq!(*asked.borrow(), 1, "no path yet, so it asked for one");
    assert!(path.exists(), "and the bytes reached the disk");
    assert_eq!(panel.status(), Some(&Status::Saved(path.clone())));
    assert!(!panel.is_dirty());
    // With the Session panel closed there is nowhere else this could be said.
    assert_eq!(said(&ctx).as_deref(), Some(format!("Saved to {}", path.display()).as_str()));
}

#[test]
fn a_second_cmd_s_writes_to_the_same_file_without_asking_again() {
    let path = tmp_dir("twice").join("session.json");
    let ctx = egui::Context::default();
    let mut session = two_box_session();
    let mut bars = Sidebars::default();
    let (mut panel, asked) = panel_saving_to(Some(path.clone()));

    tap(&ctx, chord(egui::Key::S, egui::Modifiers::COMMAND), egui::Key::S, &mut bars, &mut panel, &session);
    session.tempo_bpm = 133.0;
    panel.mark_edited(true);
    tap(&ctx, chord(egui::Key::S, egui::Modifiers::COMMAND), egui::Key::S, &mut bars, &mut panel, &session);

    assert_eq!(*asked.borrow(), 1, "the file is known now — a save key that re-asks is a save key nobody uses");
    assert!(!panel.is_dirty());
    let json = std::fs::read_to_string(&path).expect("the second save landed");
    assert!(json.contains("133"), "and it is the *later* session that is on disk");
}

#[test]
fn a_cancelled_dialog_says_nothing_and_leaves_the_work_unsaved() {
    let ctx = egui::Context::default();
    let session = two_box_session();
    let mut bars = Sidebars::default();
    let (mut panel, asked) = panel_saving_to(None);
    panel.mark_edited(true);

    tap(&ctx, chord(egui::Key::S, egui::Modifiers::COMMAND), egui::Key::S, &mut bars, &mut panel, &session);

    assert_eq!(*asked.borrow(), 1);
    assert!(panel.is_dirty(), "cancelling is a normal answer, and it saves nothing");
    assert_eq!(panel.status(), None, "and it is not a failure either");
    assert_eq!(said(&ctx), None, "so there is nothing to say about it");
}

#[test]
fn cmd_shift_s_is_left_free_for_the_save_as_it_looks_like() {
    let ctx = egui::Context::default();
    let session = two_box_session();
    let mut bars = Sidebars::default();
    let (mut panel, asked) =
        panel_saving_to(Some(tmp_dir("shift").join("should-not-happen.json")));

    tap(
        &ctx,
        chord(egui::Key::S, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT),
        egui::Key::S,
        &mut bars,
        &mut panel,
        &session,
    );

    // A `Modifiers::COMMAND` pattern would have swallowed this, and a Save As
    // reflex quietly doing a plain Save is the wrong kind of surprise.
    assert_eq!(*asked.borrow(), 0);
    assert_eq!(panel.status(), None);
    assert!(!bars.tool_open, "nor is it a rail letter");
}

#[test]
fn a_focused_field_keeps_both_of_them() {
    let ctx = egui::Context::default();
    let session = two_box_session();
    let mut bars = Sidebars::default();
    let (mut panel, asked) =
        panel_saving_to(Some(tmp_dir("focus").join("should-not-happen.json")));
    // Renaming a track, say. Every shortcut in this app stands down for this.
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("a-text-field")));

    frame(&ctx, letter(egui::Key::S), &mut bars, &mut panel, &session);
    frame(&ctx, chord(egui::Key::S, egui::Modifiers::COMMAND), &mut bars, &mut panel, &session);

    assert!(!bars.tool_open, "the letter belongs to the field");
    assert_eq!(*asked.borrow(), 0, "and so does the chord");
}

#[test]
fn a_save_that_could_not_be_written_says_so_where_it_can_be_seen() {
    let ctx = egui::Context::default();
    let session = two_box_session();
    let mut bars = Sidebars::default();
    // A directory that is not there. The panel's LAST line would carry this too,
    // but the whole premise of the key is that the panel is closed.
    let nowhere = tmp_dir("failed").join("no-such-directory").join("session.json");
    let (mut panel, _asked) = panel_saving_to(Some(nowhere));
    panel.mark_edited(true);

    tap(&ctx, chord(egui::Key::S, egui::Modifiers::COMMAND), egui::Key::S, &mut bars, &mut panel, &session);

    assert!(panel.is_dirty(), "a save that did not happen must not clear the flag");
    assert!(matches!(panel.status(), Some(Status::Failed(_))));
    let said = said(&ctx).expect("a failed save is the one thing here that must not be quiet");
    assert!(said.starts_with("Not saved — "), "{said}");
}

#[test]
fn neither_key_is_taken_while_a_dialog_is_waiting_for_an_answer() {
    // A write, sync or restore dialog is a question. Answering it with a key
    // that runs a *second* save underneath it is two saves and one answer.
    let ctx = egui::Context::default();
    let session = two_box_session();
    let mut bars = Sidebars::default();
    let (mut panel, asked) = panel_saving_to(Some(tmp_dir("modal").join("should-not-happen.json")));

    // The modal is drawn every pass, because that is what a dialog on screen
    // does — `top_modal_layer` reports the *previous* frame's, so a modal shown
    // once and then dropped is a modal that has been answered.
    let mut pass = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            egui::Modal::new(egui::Id::new("a-question")).show(ui.ctx(), |ui| {
                ui.label("Overwrite A01 on the box?");
            });
            rail::shortcuts(ui, &mut bars, &session);
            panel.save_shortcut(ui.ctx(), &session);
        });
        output.textures_delta.clear();
    };
    pass(Vec::new());
    pass(letter(egui::Key::G));
    pass(chord(egui::Key::S, egui::Modifiers::COMMAND));

    assert!(!bars.tool_open, "the modal has the keyboard until it is answered");
    assert_eq!(*asked.borrow(), 0);
    assert_eq!(panel.status(), None);
}
