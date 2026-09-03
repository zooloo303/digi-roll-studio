// The transport bar — the first thing in the app that makes the engine run.
//
// PLAN.md §5: "play/stop/continue, tempo, swing, FILL, panic, clock
// master/slave". Swing is per pattern, not per transport, so it sits with the
// pattern controls rather than here; everything else is here.
//
// Every control is one command down the channel. Nothing on this bar reads the
// scheduler, and nothing on it blocks: the playhead and the note count are
// atomics the engine publishes, one frame stale at worst.
//
// It spans the whole window, above the rail and both side panels, because it is
// what you touch while the pattern is running and it must not move when a panel
// opens. That also makes it the right home for the two controls that are not
// transport commands: `SETUP` reopens the right-hand panel — a collapsible panel
// whose only way back is inside itself is a panel that has gone — and, since the
// 2026-08-19 v2 pass, the scene state that used to float beside the patterns.
//
// ## Six zones and a divider between each (v2 §2a)
//
// ```text
// │ ▶ PLAY  ■ STOP  ▶▶ │ 120.0 BPM │ 003.2.07 ●●●● │ CLOCK [INT] FILL │
//   [SONG] 03·1 CHORUS · SCENE [Scene 1] DT2 A01 │  ▍▍▍▍▍ 3 sounding │ PANIC  SETUP │
// ```
//
// The dividers are the whole point. Before this pass the bar was one
// undifferentiated row of similar-looking buttons, four of which were toggles
// wearing the same lit-cyan highlight as the buttons that fire a command — so
// `CLOCK` read as "a thing you can press *now*" rather than as a report of which
// clock the desk is on. The v2 colour rule that fixes it is one line: **filled
// cyan means a thing you can press.** Hence
//
// - `PLAY` is the only *filled* button on the bar and it is trig green, not cyan:
//   the button that makes sound is the colour of sound. `STOP` and `CONTINUE` are
//   outlines beside it.
// - `PANIC` is no longer next to them. It is at the far right beside `SETUP`, in
//   the amber destructive treatment, away from the three buttons a user hits by
//   reflex — this is the one control on the bar that cannot be undone by pressing
//   it again.
// - `CLOCK` is a dim label plus a value pill reading `INT`/`EXT`. It is still
//   clickable, and still exactly `engine.set_send_clock`, but it is shaped like a
//   labelled value because that is what it is.
//
// The spacebar toggles PLAY and STOP ([`shortcuts`], read from the shell before
// this bar is drawn). CONTINUE stays a button, and PANIC is on no key at all.
//
// ## Two things this file approximates, on purpose
//
// **The level meter is derived from the voice count, not from levels.**
// `EngineLink` publishes `active_notes()` and nothing per-voice — there is no
// amplitude anywhere in this app, because the app sends MIDI and the sound is
// made inside a box we cannot hear. So the five bars are a staircase lit by how
// many notes are sounding ([`meter_lit`]), which is an honest picture of the same
// number the text beside it prints, rather than a fake VU. Adding real levels
// would mean the boxes sending audio back, which they do not.
//
// **egui has no letter-spacing**, so the v2 spec's `0.14em` eyebrows and `0.08em`
// button caps are rendered at the right size, weight and colour but at the font's
// own tracking. The size/weight/colour scale is what the handoff says matters.
//
// ## Glyphs
//
// `▶`, `■` and `»` are the three non-ASCII marks here, and all three are on
// [`super`]'s confirmed-rendering list — read off a screen, not reasoned about.
// They are drawn in the **proportional** family: the mono family is Hack, which
// carries far less, and the only monospace strings on this bar are the tempo and
// the position readout, both ASCII digits and dots. The beat dots and the level
// meter are painted shapes for the same reason a fold arrow is.

use digi_core::Session;
use eframe::egui::{self, Color32, Ui};

use crate::engine::EngineLink;
use crate::ui::scenes;

/// The bar's height — v2 §2a's `height: 40px`.
const BAR_H: f32 = 40.0;

/// The tooltip on the CLOCK pill. Kept verbatim from before this redesign: it is
/// a hardware-desk warning that has cost somebody an evening, and the pill it
/// hangs off is the same `send_clock` toggle it was written for. The first line
/// is new, and only says what the two values on the pill mean.
const CLOCK_HINT: &str = "INT — this app is the clock master. EXT — every box runs off its own clock.\n\n\
     Send MIDI clock to every box that takes it.\n\n\
     Each box also has to be set to receive, and no box may have CLOCK \
     SEND on — one master at a time, and on a DIN-chained desk one box \
     sending clock takes the others out with it. See a box's \"Takes \
     our clock\" in SETUP.";

/// Draw the bar. Returns whether the session changed — the tempo lives in the
/// session, so moving it is an edit, and so is anything the scene popup does.
pub fn ui(
    ui: &mut Ui,
    engine: &mut EngineLink,
    session: &mut Session,
    setup_open: &mut bool,
) -> bool {
    let mut changed = false;
    let playing = engine.is_playing();

    let frame = egui::Frame::new().fill(super::PANEL_BG_RAISED);
    let bar = frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), BAR_H),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_height(BAR_H);
                    // Every gap on this bar is spelled out — a zone's own `gap`
                    // and the padding either side of it — rather than left to the
                    // theme's default item spacing, which is what made the old bar
                    // read as one evenly-spaced row.
                    ui.spacing_mut().item_spacing.x = 0.0;

                    zone(ui, 10.0, 4.0, |ui| transport_zone(ui, engine, session, playing));
                    divider(ui);
                    zone(ui, 14.0, 8.0, |ui| changed |= tempo_zone(ui, engine, session));
                    divider(ui);
                    zone(ui, 14.0, 10.0, |ui| position_zone(ui, engine, playing));
                    divider(ui);
                    zone(ui, 12.0, 6.0, |ui| clock_zone(ui, engine));
                    divider(ui);
                    zone(ui, 12.0, 8.0, |ui| changed |= scene_zone(ui, session, engine));
                    divider(ui);

                    // **The right-hand end is drawn right to left**, so the two
                    // right zones stay pinned to the corner however wide the
                    // window is — which means the code below reads backwards:
                    // SETUP first because it is furthest right. Each zone's own
                    // contents are therefore listed in reverse too.
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.add_space(10.0);
                            right_zone_b(ui, engine, setup_open);
                            ui.add_space(12.0);
                            divider(ui);
                            right_zone_a(ui, engine);
                        },
                    );
                },
            );

            // The engine publishes into atomics and nothing wakes the UI when it
            // does, so a moving playhead needs the frames asked for.
            if playing {
                ui.ctx().request_repaint();
            }
        })
        .response;

    // v2 §2a's `border-bottom: 1px solid #2c3237`.
    ui.painter().hline(
        bar.rect.x_range(),
        bar.rect.bottom() - 0.5,
        egui::Stroke::new(1.0, super::PANEL_BORDER),
    );

    changed
}

// -------------------------------------------------------------- the spacebar

/// The spacebar: PLAY when the transport is stopped, STOP when it is running.
///
/// Read from the shell rather than from this bar's own `ui`, for the reason
/// `ui::edit::shortcuts` is: a shortcut belongs to the window, not to a widget,
/// and reading it before the panels are drawn means the frame that starts the
/// transport is also the frame that draws PLAY as unavailable. Returns whether
/// the key was taken — nothing here edits the session, since the transport is
/// the engine's state and not the music's.
///
/// **Plain space only, and it toggles.** It is bound to the two buttons a user
/// hits by reflex and to nothing else: `▶▶` CONTINUE stays a button, because
/// "from the top" and "from where the cursors are" are a distinction one key
/// cannot carry, and because Shift+Space and Alt+Space are left free for it to
/// grow into. PANIC is emphatically not on a key — see [`right_zone_b`] for why
/// it is not even next to the transport buttons.
pub fn shortcuts(ui: &Ui, engine: &mut EngineLink, session: &Session) -> bool {
    if !space_tap(ui.ctx()) {
        return false;
    }
    if engine.is_playing() {
        engine.stop();
    } else {
        engine.play(session);
    }
    true
}

/// Whether a plain spacebar arrived this frame, taking it out of the queue.
///
/// ## Why this is not `consume_key(Modifiers::NONE, Key::Space)`
///
/// Two reasons, both of them things `consume_key` does on purpose and neither of
/// them what a transport wants.
///
/// **It counts key repeats.** `InputState::count_and_consume_key` matches every
/// `Event::Key { pressed: true }` and never looks at `repeat` (egui 0.36.1,
/// `input_state/mod.rs` ~696), so a space held down for half a second arrives as
/// a dozen taps and the transport would start and stop at the key-repeat rate.
/// Only the first press of a hold is a tap here.
///
/// **`Modifiers::NONE` does not mean "no modifiers".** `matches_logically`
/// rejects a pattern's *missing* alt or shift, not the ones the pattern does not
/// ask for (`modifiers.rs` ~211: `if pattern.alt && !self.alt`), because a
/// logical key on some layouts needs shift to type at all. With `NONE` as the
/// pattern that leaves Shift+Space and Alt+Space matching a plain space. So the
/// match here is `matches_exact`, and those two chords stay free.
///
/// Guarded on focus and on modals, in that order. Focus is the same guard
/// `ui::edit::shortcuts` and `ui::tracks`'s clipboard carry: with the tempo
/// `DragValue` or any `TextEdit` focused, space is a character being typed, and
/// egui also gives a focused clickable widget its space as a click
/// (`context.rs` ~1467). The modal guard is this shortcut's own — a write, sync
/// or restore dialog is a question waiting for an answer, and starting the
/// transport underneath one is not an answer. `top_modal_layer` reports the
/// previous frame's modal, which is exactly right for a dialog that was opened
/// by a click and has been on screen since.
///
/// The `Event::Text(" ")` that `egui-winit` pushes beside the key event — a
/// space is printable, so it makes both (`egui-winit` 0.36.1 `lib.rs` ~1064) —
/// goes with it. Consuming half a keypress and leaving the other half for
/// whatever takes focus next is how a stray space ends up in a track name.
fn space_tap(ctx: &egui::Context) -> bool {
    if ctx.memory(|m| m.focused().is_some() || m.top_modal_layer().is_some()) {
        return false;
    }
    ctx.input_mut(|i| {
        let mut tapped = false;
        let mut took_key = false;
        i.events.retain(|event| match event {
            egui::Event::Key { key: egui::Key::Space, pressed: true, repeat, modifiers, .. }
                if modifiers.matches_exact(egui::Modifiers::NONE) =>
            {
                took_key = true;
                tapped |= !repeat;
                false
            }
            egui::Event::Text(text) if took_key && text == " " => false,
            _ => true,
        });
        tapped
    })
}

// ---------------------------------------------------------------- the six zones

/// Zone 1 — transport. PLAY filled and green; STOP and CONTINUE as outlines.
/// Panic is deliberately *not* here; see [`right_zone_b`].
fn transport_zone(ui: &mut Ui, engine: &mut EngineLink, session: &Session, playing: bool) {
    let play = ui
        .add_enabled_ui(!playing, |ui| {
            styled_button(
                ui,
                egui::RichText::new("▶ PLAY").size(12.0).strong(),
                egui::vec2(13.0, 5.0),
                super::TRIG_GREEN,
                super::CYAN_INK,
                super::TRIG_GREEN,
                super::TRIG_GREEN_HOVER,
                super::CYAN_INK,
                super::TRIG_GREEN_HOVER,
            )
        })
        .inner
        // Where the spacebar is announced. A shortcut nobody is told about is a
        // shortcut nobody finds, and the hover text only shows while the button
        // is enabled — which is exactly when that half of the toggle applies.
        .on_hover_text("Play from the top — or press Space");
    if play.clicked() {
        engine.play(session);
    }

    let stop = ui
        .add_enabled_ui(playing, |ui| outline_button(ui, "■ STOP"))
        .inner
        .on_hover_text("Stop — or press Space");
    if stop.clicked() {
        engine.stop();
    }

    let cont = ui
        .add_enabled_ui(!playing, |ui| {
            outline_button(ui, "▶▶").on_hover_text("Resume where the cursors are, without rewinding")
        })
        .inner;
    if cont.clicked() {
        engine.resume(session);
    }
}

/// Zone 2 — tempo. The largest numeral on the bar, and drag-to-change /
/// click-to-type exactly as before: this is still one `DragValue`, restyled.
/// The " BPM" suffix left the field and became its own dim unit label, which
/// also means typing over the number no longer has to survive a suffix.
fn tempo_zone(ui: &mut Ui, engine: &mut EngineLink, session: &mut Session) -> bool {
    let mut changed = false;
    ui.scope(|ui| {
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(18.0));
        ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
        let widgets = &mut ui.style_mut().visuals.widgets;
        for state in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            state.weak_bg_fill = Color32::TRANSPARENT;
            state.bg_fill = Color32::TRANSPARENT;
            state.bg_stroke = egui::Stroke::NONE;
            state.fg_stroke = egui::Stroke::new(1.0, super::TEXT_BRIGHT);
            state.corner_radius = egui::CornerRadius::ZERO;
        }
        let tempo = ui
            .add(
                egui::DragValue::new(&mut session.tempo_bpm)
                    .speed(0.1)
                    .range(20.0..=300.0)
                    .custom_formatter(|v, _| format!("{v:.1}")),
            )
            .on_hover_text("Drag to change the tempo, or click to type one");
        if tempo.changed() {
            engine.set_tempo(session.tempo_bpm);
            changed = true;
        }
    });
    ui.label(egui::RichText::new("BPM").size(9.0).color(super::TEXT_DIMMER));
    changed
}

/// Zone 3 — position. The bar.beat.step readout with its separators dimmed, and
/// four beat dots beside it. New in v2: before this the bar had the readout and
/// nothing to place it in the bar.
fn position_zone(ui: &mut Ui, engine: &EngineLink, playing: bool) {
    let steps = engine.position_steps();
    ui.label(readout_job(&position(steps, playing)));

    // Four 6px circles, painted into one allocation rather than four widgets:
    // a painted dot cannot be a missing glyph, and one rect keeps the 4px gaps
    // exact instead of at the mercy of item spacing.
    let width = 4.0 * DOT_D + 3.0 * DOT_GAP;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, DOT_D), egui::Sense::hover());
    let lit = playing.then(|| beat_of_bar(steps));
    for beat in 0..4 {
        let centre = egui::pos2(
            rect.left() + DOT_D / 2.0 + beat as f32 * (DOT_D + DOT_GAP),
            rect.center().y,
        );
        let colour = if lit == Some(beat) { super::TRIG_GREEN } else { super::PANEL_BORDER };
        ui.painter().circle_filled(centre, DOT_D / 2.0, colour);
    }
}

/// Zone 4 — clock. A dim label, a cyan value pill, and the FILL toggle beside it.
///
/// The pill is a *value* that happens to be clickable, not a lit toggle: it wears
/// the cyan pill treatment in both states and says which one it is in words, so
/// nothing on this bar is lit-versus-unlit any more.
fn clock_zone(ui: &mut Ui, engine: &mut EngineLink) {
    ui.label(egui::RichText::new("CLOCK").size(9.0).color(super::TEXT_DIMMER));

    let send_clock = engine.send_clock();
    if cyan_pill(ui, clock_label(send_clock)).on_hover_text(CLOCK_HINT).clicked() {
        engine.set_send_clock(!send_clock);
    }

    let fill = engine.fill();
    let response = if fill {
        cyan_pill(ui, "FILL")
    } else {
        pill_button(
            ui,
            "FILL",
            super::INSET_BG,
            super::TEXT_DIMMER,
            super::PANEL_BORDER,
            super::INSET_BG,
            super::TEXT_PRIMARY,
            super::BORDER_HOVER,
        )
    };
    if response.on_hover_text("Every trig carrying a FILL condition").clicked() {
        engine.set_fill(!fill);
    }
}

/// Zone 5 — scene, and since 2026-08-22 the song. The compact live state only:
/// the PTN/SONG mode pill, the song pointer while the song is walking, a label,
/// the sounding scene as a cyan pill, the `»` queued mark when a switch is
/// waiting for a boundary, and the per-box slots.
///
/// **Clicking the pill opens the full scene controls in a popup** — add, remove,
/// NOW, pick which scene is being edited, rename it, and its per-box slot
/// pickers. That is [`scenes::ui`] itself, unchanged and called from here, so
/// there is exactly one implementation of every scene capability rather than a
/// compact copy that can drift from the real one.
///
/// [`super::working_popup`] rather than `egui::Popup` directly, and that is not
/// a style choice: this popup's whole content is pickers, and egui's memory
/// holds one open popup per viewport — so with the open state in that memory,
/// clicking a slot's combo box closed the scene box instead of dropping its
/// list down, and the pattern could never be picked. `generate.rs`'s
/// destination chip had the identical bug from the identical cause. The helper
/// keeps the open flag out of egui's popup slot and owns the close behaviour;
/// see its doc comment.
fn scene_zone(ui: &mut Ui, session: &mut Session, engine: &mut EngineLink) -> bool {
    let mut changed = false;
    // **The mode pill, and the song pointer, live in this zone rather than in
    // zone 3.** Zone 3 is the bars-beats-steps readout, six ASCII digits and two
    // dots in the mono family, and the box's SONG POINTER is a different question
    // — *where in the arrangement*, not *where in the bar*. Putting it beside the
    // scene keeps both answers to "what is playing right now" in one place, and
    // leaves the position readout's width fixed so it does not jump when the mode
    // changes.
    let song_mode = engine.song_mode();
    let has_song = session.song().is_some_and(|s| !s.is_empty());
    let mode = if song_mode {
        cyan_pill(ui, "SONG")
    } else {
        pill_button(
            ui,
            "PTN",
            super::INSET_BG,
            super::TEXT_DIMMER,
            super::PANEL_BORDER,
            super::INSET_BG,
            super::TEXT_PRIMARY,
            super::BORDER_HOVER,
        )
    };
    if mode
        .on_hover_text(if has_song {
            "SONG walks the rows in the Song panel; PTN stays on one scene"
        } else {
            "No song built yet — the Song panel on the rail is where rows go.              Turning this on now means the song plays as soon as it has a row"
        })
        .clicked()
    {
        engine.set_song_mode(session, !song_mode);
    }

    if song_mode {
        // The pointer, or why there is none. A row number with no row behind it
        // would be the display inventing an arrangement.
        let text = match engine.song_position() {
            Some((row, repeat)) => {
                let label = session
                    .song_row(row)
                    .map(|r| r.label.clone())
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| String::from("row"));
                format!("{:02}·{} {label}", row + 1, repeat + 1)
            }
            None if has_song => String::from("— stopped"),
            None => String::from("— no rows"),
        };
        ui.label(egui::RichText::new(text).monospace().size(10.0).color(super::TEXT_SECONDARY));
        ui.label(egui::RichText::new("·").size(10.0).color(super::TEXT_DIMMEST));
    }

    ui.label(egui::RichText::new("SCENE").size(9.0).color(super::TEXT_DIMMER));

    // A project file is the only way to get here with no scenes — nothing in the
    // app can remove the last one — and the pill below indexes freely.
    if session.scenes.is_empty() {
        ui.label(
            egui::RichText::new("none — nothing can play").size(10.0).color(super::WARN_AMBER),
        );
        return changed;
    }

    let playing = engine.playing_scene().min(session.scenes.len() - 1);
    let name = session.scenes.get(playing).map(|s| s.name.as_str()).unwrap_or("?");
    let pill = cyan_pill(ui, name.to_owned())
        .on_hover_text("Click for the scene list, the slot each box plays, and NOW");

    if let Some(queued) = engine.queued_scene().filter(|q| *q != playing) {
        let queued_name = session.scenes.get(queued).map(|s| s.name.as_str()).unwrap_or("?");
        ui.label(
            egui::RichText::new(format!("» {queued_name}")).size(10.0).color(super::ACCENT),
        );
    }

    ui.label(
        egui::RichText::new(scenes::slots_summary(session, playing))
            .size(10.0)
            .color(super::TEXT_DIM),
    );

    if let Some(inner) = super::working_popup(&pill, 320.0, |ui| scenes::ui(ui, session, engine)) {
        changed |= inner;
    }

    changed
}

/// Right zone A — activity. The level meter and the voice count it is drawn
/// from. Right-to-left, so the meter is listed last.
fn right_zone_a(ui: &mut Ui, engine: &EngineLink) {
    ui.add_space(12.0);
    let sounding = engine.active_notes();
    ui.label(egui::RichText::new("sounding").size(10.5).color(super::TEXT_DIMMER));
    ui.add_space(7.0);
    ui.label(egui::RichText::new(sounding.to_string()).size(11.0).color(super::TEXT_SECONDARY));
    ui.add_space(7.0);
    meter(ui, sounding);
    ui.add_space(12.0);
}

/// Right zone B — the two buttons that are not transport commands, plus the port
/// diagnostics that have always lived at this end of the bar.
///
/// PANIC is here rather than beside STOP because it is the one control on the bar
/// with no inverse: it sends All Notes Off and All Sound Off on every channel in
/// use, and a user reaching for STOP must not be able to hit it by being 30px
/// out. Amber says the same thing in colour.
///
/// SETUP takes the cyan primary treatment in both states rather than the lit
/// highlight of a toggle: the panel it opens is its own state indicator, and the
/// bar's whole argument is that filled cyan means "pressable".
fn right_zone_b(ui: &mut Ui, engine: &mut EngineLink, setup_open: &mut bool) {
    if pill_button(
        ui,
        egui::RichText::new("SETUP").size(10.0),
        super::CYAN_FILL,
        super::CYAN_TEXT,
        super::CYAN,
        super::CYAN,
        super::CYAN_INK,
        super::CYAN,
    )
    .on_hover_text("The boxes in this session, their ports, and MIDI")
    .clicked()
    {
        *setup_open = !*setup_open;
    }

    ui.add_space(8.0);

    if pill_button(
        ui,
        egui::RichText::new("PANIC").size(10.0),
        super::WARN_AMBER_FILL,
        super::WARN_AMBER_TEXT,
        super::WARN_AMBER_BORDER,
        super::WARN_AMBER,
        super::WARN_AMBER_INK,
        super::WARN_AMBER,
    )
    .on_hover_text("All Notes Off and All Sound Off on every channel in use")
    .clicked()
    {
        engine.panic();
    }

    ui.add_space(10.0);

    // The port count and the failures: real diagnostic state, and this is still
    // the right place for it — SETUP is what you press about it, and it is one
    // space to the right.
    match engine.ports().len() {
        0 => {
            // Identify is no longer the only way to get a port: the device
            // strip's pickers will point a box at anything connected, which is
            // what makes an IAC bus or a soft synth reachable.
            ui.label(egui::RichText::new("no ports").size(10.0).color(super::WARN_AMBER))
                .on_hover_text(
                    "Nothing will sound. Open SETUP and give a box an out \
                     port — pick one by hand, or Identify a box",
                );
        }
        n => {
            let names: Vec<&str> =
                engine.ports().ids().filter_map(|id| engine.ports().name(id)).collect();
            // The names are a tooltip rather than a label: they are two long
            // strings that pushed everything else off a narrow window, and the
            // count is the part you check at a glance.
            ui.label(egui::RichText::new(format!("{n} port(s)")).size(10.0).color(super::TEXT_DIMMER))
                .on_hover_text(names.join("\n"));
        }
    }
    for failure in engine.failures() {
        ui.label(egui::RichText::new(failure).size(10.0).color(super::WARN_AMBER));
    }
}

// ------------------------------------------------------------------- furniture

/// One zone: its own left and right padding, and its own internal gap. Drawn
/// through a scope so the gap cannot leak into the padding either side of it.
fn zone(ui: &mut Ui, pad: f32, gap: f32, content: impl FnOnce(&mut Ui)) {
    ui.add_space(pad);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        content(ui);
    });
    ui.add_space(pad);
}

/// The 1px rule between two zones, full bar height.
fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, BAR_H), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, super::PANEL_BORDER);
}

/// A button with an explicit fill, text colour and border for each of rest and
/// hover, square-cornered, at an explicit padding.
///
/// Not [`super::colored_button`], which this is otherwise a near twin of: that
/// one derives the hovered *border* from the hovered *fill*, which is right for
/// the Setup panel's filled buttons (their hover is a brighter fill of the same
/// colour) and wrong for every outline on this bar, where the whole hover state
/// is a border that lightens while the fill stays put. The extra parameter is the
/// difference, and it is the reason this lives here rather than replacing the
/// shared one — five other panels are drawn with that signature.
#[allow(clippy::too_many_arguments)]
fn styled_button(
    ui: &mut Ui,
    text: impl Into<egui::WidgetText>,
    padding: egui::Vec2,
    fill: Color32,
    text_colour: Color32,
    border: Color32,
    hover_fill: Color32,
    hover_text: Color32,
    hover_border: Color32,
) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = padding;
        let widgets = &mut ui.style_mut().visuals.widgets;
        for state in [&mut widgets.inactive, &mut widgets.active] {
            state.weak_bg_fill = fill;
            state.bg_fill = fill;
            state.fg_stroke = egui::Stroke::new(1.0, text_colour);
            state.bg_stroke = egui::Stroke::new(1.0, border);
            state.corner_radius = egui::CornerRadius::ZERO;
        }
        widgets.hovered.weak_bg_fill = hover_fill;
        widgets.hovered.bg_fill = hover_fill;
        widgets.hovered.fg_stroke = egui::Stroke::new(1.0, hover_text);
        widgets.hovered.bg_stroke = egui::Stroke::new(1.0, hover_border);
        widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
        ui.add(egui::Button::new(text))
    })
    .inner
}

/// STOP and CONTINUE: an inset fill that does not move, a border that lightens.
fn outline_button(ui: &mut Ui, text: &str) -> egui::Response {
    styled_button(
        ui,
        egui::RichText::new(text).size(11.0),
        egui::vec2(10.0, 5.0),
        super::INSET_BG,
        super::TEXT_MUTED,
        super::PANEL_BORDER,
        super::INSET_BG,
        super::TEXT_PRIMARY,
        super::BORDER_HOVER,
    )
}

/// A pill: the small `3px 8px` shape the CLOCK value, FILL, the scene and the two
/// right-hand buttons all share.
#[allow(clippy::too_many_arguments)]
fn pill_button(
    ui: &mut Ui,
    text: impl Into<egui::WidgetText>,
    fill: Color32,
    text_colour: Color32,
    border: Color32,
    hover_fill: Color32,
    hover_text: Color32,
    hover_border: Color32,
) -> egui::Response {
    styled_button(
        ui,
        text,
        egui::vec2(8.0, 3.0),
        fill,
        text_colour,
        border,
        hover_fill,
        hover_text,
        hover_border,
    )
}

/// The cyan pill treatment: a filled value that is also pressable.
fn cyan_pill(ui: &mut Ui, text: impl Into<String>) -> egui::Response {
    pill_button(
        ui,
        egui::RichText::new(text.into()).size(11.0).color(super::CYAN_TEXT),
        super::CYAN_FILL,
        super::CYAN_TEXT,
        super::CYAN,
        super::CYAN,
        super::CYAN_INK,
        super::CYAN,
    )
}

// ------------------------------------------------------------- the pure pieces

/// Beat-dot diameter and the gap between two of them — v2 §2a's 6px circles,
/// `gap: 4px`.
const DOT_D: f32 = 6.0;
const DOT_GAP: f32 = 4.0;

/// The five meter bars' heights, left to right, within the spec's 13px box.
const METER_BAR_H: [f32; 5] = [4.0, 6.0, 8.0, 11.0, 13.0];
/// The voice count at which each bar lights. A staircase, not a scale: two notes
/// is a dyad and eight is a hand on a chord, and the gaps widen accordingly.
const METER_THRESHOLDS: [usize; 5] = [1, 2, 4, 6, 8];
const METER_BAR_W: f32 = 3.0;
const METER_BAR_GAP: f32 = 2.0;

/// How many of the five meter bars are lit for `active_notes`.
///
/// **This is an approximation and the module header says why**: the engine
/// publishes a voice count and nothing per-voice, because the sound is made
/// inside a box this app cannot hear. So the meter is a picture of the same
/// number printed next to it rather than of any signal level.
fn meter_lit(active_notes: usize) -> usize {
    METER_THRESHOLDS.iter().filter(|threshold| active_notes >= **threshold).count()
}

/// The five-bar meter, painted into one allocation.
fn meter(ui: &mut Ui, active_notes: usize) {
    let lit = meter_lit(active_notes);
    let width = 5.0 * METER_BAR_W + 4.0 * METER_BAR_GAP;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 13.0), egui::Sense::hover());
    for (index, height) in METER_BAR_H.iter().enumerate() {
        let left = rect.left() + index as f32 * (METER_BAR_W + METER_BAR_GAP);
        let bar = egui::Rect::from_min_size(
            egui::pos2(left, rect.bottom() - height),
            egui::vec2(METER_BAR_W, *height),
        );
        let colour =
            if index < lit { super::TRIG_GREEN } else { super::PANEL_BORDER };
        ui.painter().rect_filled(bar, 0.0, colour);
    }
}

/// `INT` while this app is the clock master, `EXT` while it is not.
///
/// The engine's own scheduler is always internal; what `send_clock` decides is
/// whether the *desk* runs off it. So `EXT` here means "each box is on its own
/// clock", which is what the pill's tooltip spells out — and it is the state a
/// user has to be able to read at a glance before pressing PLAY on a chained
/// desk.
fn clock_label(send_clock: bool) -> &'static str {
    if send_clock { "INT" } else { "EXT" }
}

/// Which of the four beats of the bar the playhead is in, counting from 0.
///
/// The same arithmetic as the middle field of [`position`], one less: that one
/// prints for a human and counts from 1, this one indexes four dots.
fn beat_of_bar(steps: f64) -> usize {
    let step = steps.max(0.0) as u64;
    ((step % 16) / 4) as usize
}

/// The playhead as bar.beat.step, counting from 1 the way the boxes do.
///
/// These are pattern steps at the session's tempo, not any one track's: a track
/// at half scale passes two of these per step of its own. The roll wraps this
/// number by the length of the track it is drawing.
fn position(steps: f64, playing: bool) -> String {
    if !playing {
        return "  ·  ·  ".into();
    }
    let step = steps.max(0.0) as u64;
    format!("{:>3}.{}.{:02}", step / 16 + 1, (step % 16) / 4 + 1, step % 16 + 1)
}

/// [`position`]'s output split into the runs of digits and the `.` separators
/// between them, so the separators can be painted dimmer than the numbers —
/// v2 §2a asks for exactly that, and it is what stops "003.2.07" reading as one
/// seven-digit number.
///
/// `true` marks a separator. The stopped placeholder has no `.` in it and comes
/// back as one run.
fn readout_segments(text: &str) -> Vec<(String, bool)> {
    let mut segments = Vec::new();
    let mut run = String::new();
    for ch in text.chars() {
        if ch == '.' {
            if !run.is_empty() {
                segments.push((std::mem::take(&mut run), false));
            }
            segments.push((".".into(), true));
        } else {
            run.push(ch);
        }
    }
    if !run.is_empty() {
        segments.push((run, false));
    }
    segments
}

/// The readout as one laid-out line: monospace 14px, digits bright, separators
/// dim.
fn readout_job(text: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (run, separator) in readout_segments(text) {
        job.append(
            &run,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::monospace(14.0),
                color: if separator { super::TEXT_DIMMEST } else { super::TEXT_PRIMARY },
                ..Default::default()
            },
        );
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain spacebar, built the way `egui-winit` really sends one: the key
    /// event *and* the `Event::Text(" ")` beside it, because a space is
    /// printable and the platform pushes both. Feeding only the half this code
    /// reads is the mistake `ui::tracks`'s clipboard shipped dead on.
    ///
    /// **`repeat` is not ours to set.** `InputState::begin_pass` overwrites the
    /// flag on every key event from its own `keys_down` set — a press whose key
    /// is already down *becomes* a repeat, whatever the event said (egui 0.36.1,
    /// `input_state/mod.rs` ~412). So a hold is spelled here the way the platform
    /// spells it, as presses with no [`release`] between them, and the first cut
    /// of these tests handing egui `repeat: false` twice was writing a flag egui
    /// immediately threw away.
    fn space(modifiers: egui::Modifiers) -> Vec<egui::Event> {
        vec![
            egui::Event::Key {
                key: egui::Key::Space,
                physical_key: Some(egui::Key::Space),
                pressed: true,
                repeat: false,
                modifiers,
            },
            egui::Event::Text(" ".to_owned()),
        ]
    }

    /// The other end of a tap. Without it egui still has the key down, and the
    /// next press is a repeat rather than a new tap.
    fn release() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Space,
            physical_key: Some(egui::Key::Space),
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    /// One headless pass that reads the key, plus what was left in the queue
    /// after it — a shortcut that fires and does not consume is a space that
    /// also lands somewhere else.
    fn tap(ctx: &egui::Context, events: Vec<egui::Event>) -> (bool, Vec<egui::Event>) {
        let mut answer = (false, Vec::new());
        let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            let took = space_tap(ui.ctx());
            let left = ui.ctx().input(|i| i.events.clone());
            answer = (took, left);
        });
        output.textures_delta.clear();
        answer
    }

    #[test]
    fn the_spacebar_is_taken_whole() {
        let ctx = egui::Context::default();
        let (took, left) = tap(&ctx, space(egui::Modifiers::NONE));
        assert!(took, "a plain space is a transport tap");
        assert!(left.is_empty(), "both halves of the keypress are consumed, not just the key");
    }

    #[test]
    fn a_held_spacebar_is_one_tap_and_not_a_stutter() {
        // `consume_key` would answer `true` to every one of these: it matches on
        // `pressed` and never looks at `repeat`. A transport on that would start
        // and stop at the key-repeat rate for as long as a thumb rested on the
        // bar. No release goes in between, which is what a hold is.
        let ctx = egui::Context::default();
        assert!(tap(&ctx, space(egui::Modifiers::NONE)).0, "the press that begins the hold");
        for _ in 0..8 {
            let (took, left) = tap(&ctx, space(egui::Modifiers::NONE));
            assert!(!took, "a repeat is the same press still held down");
            assert!(left.is_empty(), "and it is still eaten, rather than left to fall through");
        }
        // Let go, and the next press is a tap again.
        tap(&ctx, release());
        assert!(tap(&ctx, space(egui::Modifiers::NONE)).0, "a second, separate press");
    }

    #[test]
    fn a_modified_space_belongs_to_whoever_wants_it() {
        // The reason this is `matches_exact`: with `Modifiers::NONE` as a
        // `matches_logically` pattern — which is what `consume_key` does — every
        // one of these matches a plain space, because that call only rejects the
        // modifiers a pattern *asks for* and is missing.
        let ctx = egui::Context::default();
        for modifiers in [egui::Modifiers::SHIFT, egui::Modifiers::ALT, egui::Modifiers::COMMAND] {
            let (took, left) = tap(&ctx, space(modifiers));
            assert!(!took, "{modifiers:?}+Space is not the transport");
            assert_eq!(left.len(), 2, "{modifiers:?}+Space is left in the queue untouched");
            tap(&ctx, release());
        }
    }

    #[test]
    fn nothing_is_taken_while_a_field_has_the_keyboard() {
        // The tempo field is the one this is really about: a space typed into a
        // `DragValue` mid-edit must not start the transport. A `TextEdit` is the
        // same focus in one line.
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text)).request_focus();
        });
        output.textures_delta.clear();

        let (took, left) = tap(&ctx, space(egui::Modifiers::NONE));
        assert!(!took, "a space is a character while something is being typed into");
        assert_eq!(left.len(), 2, "and it reaches the field it was typed into");
    }

    #[test]
    fn nothing_is_taken_while_a_dialog_is_waiting_for_an_answer() {
        // A write, sync or restore dialog is a question, and starting the
        // transport underneath one is not an answer to it.
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::Modal::new(egui::Id::new("a-question")).show(ui.ctx(), |ui| {
                ui.label("Send this pattern to the box?");
            });
        });
        output.textures_delta.clear();

        let (took, _) = tap(&ctx, space(egui::Modifiers::NONE));
        assert!(!took, "the modal has the keyboard until it is answered");
    }

    #[test]
    fn the_readout_counts_bars_beats_and_steps_from_one() {
        assert_eq!(position(0.0, true), "  1.1.01");
        assert_eq!(position(4.4, true), "  1.2.05");
        assert_eq!(position(16.0, true), "  2.1.01");
        assert_eq!(position(35.9, true), "  3.1.04");
        assert_eq!(position(12.0, false), "  ·  ·  ", "stopped shows no position");
    }

    #[test]
    fn the_beat_dots_agree_with_the_readouts_middle_field() {
        // The dot that is lit and the digit that is printed are the same beat,
        // one 0-based and one 1-based. Drifting apart is the bug this catches:
        // two indicators of one fact, side by side, disagreeing.
        for steps in [0.0, 3.9, 4.0, 7.5, 8.0, 12.0, 15.9, 16.0, 21.0, 63.0] {
            let printed = position(steps, true);
            let beat_digit: usize = printed.split('.').nth(1).unwrap().parse().unwrap();
            assert_eq!(beat_of_bar(steps) + 1, beat_digit, "at step {steps}");
        }
    }

    #[test]
    fn the_beat_index_stays_in_the_bar_and_never_goes_negative() {
        assert_eq!(beat_of_bar(-9.0), 0, "a negative playhead is the first beat");
        for steps in [0.0, 1.0, 15.0, 16.0, 400.0, 1_000_000.0] {
            assert!(beat_of_bar(steps) < 4, "{steps} landed outside the bar");
        }
    }

    #[test]
    fn the_readout_splits_into_numbers_and_dimmed_separators() {
        let segments = readout_segments("003.2.07");
        assert_eq!(
            segments,
            vec![
                ("003".to_owned(), false),
                (".".to_owned(), true),
                ("2".to_owned(), false),
                (".".to_owned(), true),
                ("07".to_owned(), false),
            ]
        );
        // The whole string survives the split, whatever it is.
        for text in ["  1.1.01", "  ·  ·  ", "", "..."] {
            let rejoined: String =
                readout_segments(text).into_iter().map(|(run, _)| run).collect();
            assert_eq!(rejoined, text);
        }
    }

    #[test]
    fn the_meter_lights_from_the_left_and_saturates_at_five() {
        assert_eq!(meter_lit(0), 0, "silence lights nothing");
        assert_eq!(meter_lit(1), 1);
        assert_eq!(meter_lit(2), 2);
        assert_eq!(meter_lit(3), 2);
        assert_eq!(meter_lit(4), 3);
        assert_eq!(meter_lit(8), 5);
        assert_eq!(meter_lit(400), 5, "the meter has five bars and no more");
        // Monotonic: one more voice never lights fewer bars.
        let mut previous = 0;
        for notes in 0..64 {
            let lit = meter_lit(notes);
            assert!(lit >= previous, "{notes} voices lit fewer bars than {}", notes - 1);
            previous = lit;
        }
    }

    #[test]
    fn the_clock_pill_names_the_master_rather_than_lighting_up() {
        // The v2 rule this bar is built on: a value says which state it is in, in
        // words, instead of being lit or unlit. So the two answers must differ as
        // *text*, not only as colour.
        assert_eq!(clock_label(true), "INT");
        assert_eq!(clock_label(false), "EXT");
        assert_ne!(clock_label(true), clock_label(false));
    }
}
