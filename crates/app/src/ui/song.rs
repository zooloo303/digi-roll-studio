// The SONG panel: the rail's fifth slot, and the arrangement.
//
// PLAN.md §6 phase 12, and the DT2/DN2 SONG page column for column — with the
// one substitution §2 forces: **a row names a scene, not a pattern.** A scene is
// already one pattern per box chosen together, so one row moves the DT2 and the
// DN2 at the same boundary. `core::song`'s header has the full column mapping and
// the two arguments behind it (ROW LENGTH's unit, and why ROW TEMPO is a report
// rather than a control).
//
// ## The shape, and why it is not the box's grid
//
// The box draws eight columns across a 128×64 screen and edits the highlighted
// row in place, because it has encoders sitting under each column. This panel is
// a 320px-wide side panel with a mouse, so it splits that into the two halves the
// box merges:
//
//   ROWS   one line per row — playhead, number, label, scene, ×plays, length, M
//   ROW    every field of the *selected* row, edited with real controls
//
// That is the same division `scenes.rs` already makes ("the slot pickers edit the
// scene the user last asked for"), and it keeps the row list readable at the width
// the panel actually has. Eight editable columns in 320px would be eight controls
// of forty pixels each.
//
// ## Selecting a row is not jumping to it
//
// The box's `[UP]`/`[DOWN]` move a *selection*; the playhead stays where it is.
// So clicking a row here selects it for editing, and the `▶` button on the row is
// what moves the playhead. Both are on [`EngineLink`] and named for the
// difference: `select_row` versus `jump_to_row`.
//
// ## What the panel does not decide
//
// **Whether the song is playing.** SONG/PATTERN is a transport-bar control
// (`transport::mode_zone`) and the toggle here is the same call — one mode, two
// places to reach it, because the mode belongs beside PLAY and the arrangement
// belongs beside the rows. The panel follows what the engine publishes, exactly
// as the scene bar does: the playhead mark moves when the boxes move.
//
// **Where a row boundary falls.** The engine owns that (`Scheduler::advance_song`)
// and this panel cannot see it. What it can do is say what the row is *set* to,
// which is why the length field shows the resolved step count for a row that
// inherits — a row reading "cycle" and nothing else would be a control with no
// value.
//
// ## The song is not undoable, and that is the existing line rather than a new one
//
// `history::Content` holds patterns only, so `commit` drops a step in which no
// note moved — which is what a song edit is. Removing a row is therefore not
// something Cmd+Z takes back, exactly as changing the key is not (`ui::harmony`
// says the same about `Session::harmony`). The edit still marks the file dirty and
// still reaches the engine, because both of those are keyed off the same flag.
// The `?` prose says so, so that nobody discovers it by deleting the wrong row.
//
// ## Glyphs
//
// `▶`, `·`, `×`, `−` and `—` only, all five on [`super`]'s read-off-a-screen list.
// The row-order arrows are painted ([`super::paint_direction_arrow`]) rather than
// typed, for the reason that list exists: `▾` and `▼` were both tofu, and a shape
// has no font behind it. Nothing here introduces a mark that has not been seen.

use digi_core::song::{clamp_row_length, EndAction, SongRow, LABELS, MAX_ROWS};
use digi_core::{DeviceId, Session};
use eframe::egui::{self, Ui};

use crate::engine::EngineLink;

/// What one frame of this panel did.
#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    pub close: bool,
    /// The song changed. An ordinary edit: the shell opens a history step around
    /// it and the engine gets a snapshot at the end of the frame.
    pub edited: bool,
}

#[derive(Debug, Default)]
pub struct SongPanel {
    /// Whether the `?` reveal is open, as in every other v2 panel.
    reference_visible: bool,
    /// Which box's mute row is expanded. One at a time: sixteen toggles per box
    /// is the tallest thing in the panel, and two boxes' worth would push the row
    /// list off the bottom.
    mutes_open: Option<DeviceId>,
}

impl SongPanel {
    pub fn ui(&mut self, ui: &mut Ui, session: &mut Session, engine: &mut EngineLink) -> Outcome {
        let mut out = Outcome::default();

        let rows = session.song().map(|s| s.len()).unwrap_or(0);
        let context = match session.song() {
            Some(song) => format!("{} · {rows} rows", song.name),
            None => String::from("no song yet"),
        };
        out.close = super::panel_title_bar(ui, "Song", &context, &mut self.reference_visible);

        if self.reference_visible {
            reference_prose(ui);
        }

        out.edited |= self.transport_row(ui, session, engine);
        ui.add_space(6.0);
        out.edited |= self.rows_section(ui, session, engine);
        ui.add_space(6.0);
        out.edited |= self.row_editor(ui, session, engine);

        out
    }

    /// SONG/PATTERN, the song's name, and the END row.
    fn transport_row(&mut self, ui: &mut Ui, session: &mut Session, engine: &mut EngineLink) -> bool {
        let mut edited = false;
        let playing = engine.song_position();

        // The box's SONG POINTER, in the panel that owns the rows. `+ 1` because a
        // row is `01` on the box and 0 in the vec, and every number a user reads
        // here is the box's.
        let pointer = match playing {
            Some((row, repeat)) => format!("row {:02} · pass {}", row + 1, repeat + 1),
            None if engine.song_mode() => String::from("song mode, nothing to walk"),
            None => String::from("pattern mode"),
        };
        super::section_header(ui, "ARRANGEMENT", Some(&pointer));

        ui.horizontal(|ui| {
            let mut mode = engine.song_mode();
            if ui
                .toggle_value(&mut mode, "SONG")
                .on_hover_text(
                    "Walk the rows below instead of staying on one scene. Also on \
                     the transport bar, beside PLAY",
                )
                .changed()
            {
                engine.set_song_mode(session, mode);
            }

            ui.separator();

            if session.song().is_some() {
                let mut name = session.song().map(|s| s.name.clone()).unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut name)
                            .desired_width(120.0)
                            .hint_text("song name"),
                    )
                    .changed()
                {
                    session.song_mut().name = name;
                    edited = true;
                }
            }
        });

        if let Some(song) = session.song() {
            let end = song.end;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("END")
                        .size(9.0)
                        .color(super::TEXT_DIMMER),
                );
                for (action, hint) in [
                    (EndAction::Loop, "Back to row 01 and round again"),
                    (EndAction::Stop, "Stop the transport after the last row"),
                ] {
                    if ui
                        .selectable_label(end == action, action.label())
                        .on_hover_text(hint)
                        .clicked()
                        && end != action
                    {
                        session.song_mut().end = action;
                        edited = true;
                    }
                }
            });
        }

        edited
    }

    /// The row list, and the buttons that add to and reorder it.
    fn rows_section(&mut self, ui: &mut Ui, session: &mut Session, engine: &mut EngineLink) -> bool {
        let mut edited = false;
        let rows = session.song().map(|s| s.len()).unwrap_or(0);
        let selected = engine.selected_row().min(rows.saturating_sub(1));
        let playing = engine.song_position().map(|(row, _)| row);
        let broken = session
            .song()
            .map(|s| s.broken_rows(session.scenes.len()))
            .unwrap_or_default();

        let count = match rows {
            0 => String::from("none yet"),
            _ if rows >= MAX_ROWS => format!("full — {MAX_ROWS} rows"),
            _ => format!("{rows} of {MAX_ROWS}"),
        };
        super::section_header(ui, "ROWS", Some(&count));

        if rows == 0 {
            super::consequence_line(
                ui,
                "A row plays a scene. Add one, and the song plays the scenes in \
                 order — every box moving together at each row's boundary.",
            );
        }

        // The list. A `ScrollArea` because 99 rows is a real number and the panel
        // is one column of a window that also has a piano roll in it.
        egui::ScrollArea::vertical()
            .id_salt("song_rows")
            .max_height(190.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for index in 0..rows {
                    let line = row_line(session, index);
                    let is_broken = broken.contains(&index);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;

                        // The playhead column: `▶` on the row sounding, and a
                        // fixed-width blank on every other, so the numbers below
                        // it do not shift when the playhead moves.
                        let mark = if playing == Some(index) { "▶" } else { " " };
                        ui.add_sized(
                            egui::vec2(10.0, 14.0),
                            egui::Label::new(
                                egui::RichText::new(mark).size(9.0).color(super::TRIG_GREEN),
                            ),
                        );

                        let colour = if is_broken {
                            super::WARN_AMBER
                        } else if playing == Some(index) {
                            super::TEXT_BRIGHT
                        } else {
                            super::TEXT_SECONDARY
                        };
                        let label = egui::RichText::new(line).monospace().size(10.5).color(colour);
                        let response = ui.selectable_label(selected == index, label);
                        let response = if is_broken {
                            response.on_hover_text(
                                "This row names a scene the session no longer has. \
                                 It takes its turn and the scene before it keeps \
                                 playing — pick a scene below.",
                            )
                        } else {
                            response
                        };
                        if response.clicked() {
                            // Selecting is not jumping, exactly as `[UP]` and
                            // `[DOWN]` are not on the box.
                            engine.select_row(index);
                        }

                        if ui
                            .small_button("▶")
                            .on_hover_text("Play from this row")
                            .clicked()
                        {
                            engine.jump_to_row(index);
                        }
                    });
                }
            });

        ui.horizontal(|ui| {
            let full = rows >= MAX_ROWS;
            let scene = session.current_scene;
            if ui
                .add_enabled(!full, egui::Button::new("+").small())
                .on_hover_text("Add a row playing the scene that is up now")
                .clicked()
            {
                if let Some(new) = session.add_song_row(scene) {
                    engine.select_row(new);
                    edited = true;
                }
            }
            if ui
                .add_enabled(rows > 0 && !full, egui::Button::new("copy").small())
                .on_hover_text("Duplicate this row directly after it")
                .clicked()
            {
                if let Some(new) = session.song_mut().duplicate(selected) {
                    engine.select_row(new);
                    edited = true;
                }
            }
            if ui
                .add_enabled(rows > 0, egui::Button::new("−").small())
                .on_hover_text("Remove this row")
                .clicked()
                && session.song_mut().remove(selected).is_some()
            {
                engine.select_row(selected.min(session.song().map(|s| s.len()).unwrap_or(1).saturating_sub(1)));
                edited = true;
            }

            ui.separator();

            for (down, hint) in [(false, "Move this row up"), (true, "Move this row down")] {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(18.0, 16.0),
                    egui::Sense::click(),
                );
                let enabled = rows > 1;
                let colour = if enabled && response.hovered() {
                    super::TEXT_BRIGHT
                } else if enabled {
                    super::TEXT_MUTED
                } else {
                    super::TEXT_DISABLED
                };
                // Painted, not typed: `▾` and `▼` were both tofu on this screen,
                // and a rotated arrow is a shape either way.
                super::paint_vertical_arrow(ui.painter(), rect, down, colour);
                if response.on_hover_text(hint).clicked() && enabled {
                    if let Some(to) = session.song_mut().move_row(selected, down) {
                        engine.select_row(to);
                        edited = true;
                    }
                }
            }
        });

        edited
    }

    /// Every field of the selected row — the box's eight columns, with controls.
    fn row_editor(&mut self, ui: &mut Ui, session: &mut Session, engine: &mut EngineLink) -> bool {
        let mut edited = false;
        let rows = session.song().map(|s| s.len()).unwrap_or(0);
        if rows == 0 {
            return edited;
        }
        let index = engine.selected_row().min(rows - 1);
        let Some(row) = session.song().and_then(|s| s.row(index)).cloned() else {
            return edited;
        };

        super::section_header(ui, "ROW", Some(&format!("{:02}", index + 1)));

        // LABEL — free text, because the box lets a row be named after its
        // pattern as well as after a section.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("LABEL").size(9.0).color(super::TEXT_DIMMER));
            let mut label = row.label.clone();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut label)
                        .desired_width(110.0)
                        .hint_text("verse, fill…"),
                )
                .changed()
            {
                if let Some(r) = session.song_mut().row_mut(index) {
                    r.label = label;
                }
                edited = true;
            }
        });
        // The keywords, as one click each. Wrapped: they are short and there are
        // ten of them.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for keyword in LABELS {
                if ui
                    .add(egui::Button::new(egui::RichText::new(keyword).size(9.0)).small())
                    .clicked()
                {
                    if let Some(r) = session.song_mut().row_mut(index) {
                        r.label = keyword.to_string();
                    }
                    edited = true;
                }
            }
        });

        // PTN — a scene, and the panel's one substitution from the box.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("SCENE").size(9.0).color(super::TEXT_DIMMER));
            let names: Vec<String> = session.scenes.iter().map(|s| s.name.clone()).collect();
            let current = names
                .get(row.scene)
                .cloned()
                .unwrap_or_else(|| format!("— scene {} is gone —", row.scene + 1));
            egui::ComboBox::from_id_salt("song_row_scene")
                .selected_text(current)
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(row.scene == i, name).clicked() {
                            if let Some(r) = session.song_mut().row_mut(index) {
                                r.scene = i;
                            }
                            edited = true;
                        }
                    }
                });
        });

        // ROW PLAY COUNT and ROW LENGTH.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("PLAYS").size(9.0).color(super::TEXT_DIMMER));
            let mut plays = row.plays();
            if ui
                .add(egui::DragValue::new(&mut plays).range(1..=99).speed(0.1))
                .on_hover_text("How many times this row plays before the song moves on")
                .changed()
            {
                if let Some(r) = session.song_mut().row_mut(index) {
                    r.repeats = plays.max(1);
                }
                edited = true;
            }

            ui.separator();

            ui.label(egui::RichText::new("LEN").size(9.0).color(super::TEXT_DIMMER));
            let mut own_length = row.length_steps.is_some();
            if ui
                .checkbox(&mut own_length, "")
                .on_hover_text(
                    "Off: the row lasts its scene's own cycle — the longest track \
                     in it. On: a length in steps, cutting every track short.",
                )
                .changed()
            {
                // The scene's resolved length is the honest starting point for a
                // row that has just stopped inheriting: ticking the box must not
                // also change how long the row is. Read before the song is
                // borrowed mutably, since both come out of the session.
                let resolved = resolved_length(session_scene_steps(session, row.scene));
                if let Some(r) = session.song_mut().row_mut(index) {
                    r.length_steps = own_length.then_some(resolved);
                }
                edited = true;
            }
            match row.length_steps {
                Some(steps) => {
                    let mut steps = steps;
                    if ui
                        .add(egui::DragValue::new(&mut steps).range(2..=1024).speed(0.5))
                        .on_hover_text("Steps, counted as 1/16 at 1x — the grid the clock runs on")
                        .changed()
                    {
                        if let Some(r) = session.song_mut().row_mut(index) {
                            r.length_steps = Some(clamp_row_length(steps));
                        }
                        edited = true;
                    }
                }
                None => {
                    // Not "cycle" alone: a control with no value is a control
                    // nobody can check. This is what the engine will resolve it to.
                    let steps = session_scene_steps(session, row.scene);
                    ui.label(
                        egui::RichText::new(match steps {
                            Some(steps) => format!("cycle · {steps}"),
                            None => String::from("cycle · —"),
                        })
                        .size(10.0)
                        .color(super::TEXT_DIM),
                    );
                }
            }
        });

        // ROW TEMPO — the box's column, as a report. `core::song`'s header has the
        // argument: one clock for the session until the engine's timeline is
        // piecewise, and a column that pretended otherwise would be a lie with a
        // number in it.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("BPM").size(9.0).color(super::TEXT_DIMMER));
            ui.label(
                egui::RichText::new(format!("{:.1}", session.tempo_bpm))
                    .monospace()
                    .size(10.5)
                    .color(super::TEXT_DIM),
            );
            super::info_icon(
                ui,
                super::TEXT_DIMMER,
                "The session's tempo, shown per row because the box has the column. \
                 Per-row BPM is not built: the engine dates every event from one \
                 clock, and a row that changed it would rescale the whole timeline. \
                 Set the tempo on the transport bar.",
            );
        });

        // ROW MUTE, per box.
        edited |= self.mutes(ui, session, index, &row);

        edited
    }

    /// The mute masks, one box at a time.
    fn mutes(&mut self, ui: &mut Ui, session: &mut Session, index: usize, row: &SongRow) -> bool {
        let mut edited = false;
        let boxes: Vec<(DeviceId, String, usize)> = session
            .devices
            .iter()
            .map(|d| (d.id, d.name.clone(), d.model.num_tracks))
            .collect();

        super::section_header(
            ui,
            "ROW MUTE",
            Some(if row.has_mutes() { "muting" } else { "" }),
        );

        for (id, name, num_tracks) in boxes {
            let open = self.mutes_open == Some(id);
            let overrides = row.muted_tracks.contains_key(&id);
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(open, egui::RichText::new(&name).size(10.5))
                    .on_hover_text("The tracks this row mutes on this box")
                    .clicked()
                {
                    self.mutes_open = if open { None } else { Some(id) };
                }
                ui.label(
                    egui::RichText::new(if overrides {
                        // A count, not a mark: "how many" is the thing a glance
                        // wants, and it distinguishes an all-unmuted override from
                        // inheriting, which no icon can.
                        format!("{} muted", (0..num_tracks).filter(|t| row.mutes(id, *t) == Some(true)).count())
                    } else {
                        String::from("follows the pattern")
                    })
                    .size(9.5)
                    .color(if overrides { super::TEXT_SECONDARY } else { super::TEXT_DIM }),
                );
                if overrides
                    && ui
                        .small_button("×")
                        .on_hover_text("Give this box its pattern's own mutes back on this row")
                        .clicked()
                {
                    if let Some(r) = session.song_mut().row_mut(index) {
                        r.inherit_mutes(id);
                    }
                    edited = true;
                }
            });

            if !open {
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for track in 0..num_tracks {
                    // The three states the mask has, and the reason `mutes` returns
                    // an `Option`: muted by the row, sounded by the row, or not
                    // spoken for — where the pattern's own flag decides.
                    let by_row = row.mutes(id, track);
                    let inherited = session
                        .pattern_in_scene(row.scene, id)
                        .and_then(|p| p.track(track))
                        .map(|t| t.mute)
                        .unwrap_or(false);
                    let muted = by_row.unwrap_or(inherited);
                    let text = egui::RichText::new(format!("{:02}", track + 1))
                        .monospace()
                        .size(9.5);
                    let response = ui.selectable_label(muted, text).on_hover_text(match by_row {
                        Some(true) => "Muted by this row",
                        Some(false) => "Sounded by this row",
                        None if inherited => "Muted by the pattern — this row says nothing",
                        None => "Sounding — this row says nothing",
                    });
                    if response.clicked() {
                        if let Some(r) = session.song_mut().row_mut(index) {
                            r.set_mute(id, track, !muted);
                        }
                        edited = true;
                    }
                }
            });
        }

        edited
    }
}

/// One line of the row list, monospaced so the columns line up:
/// `01 INTRO   Scene 1  ×4 016 M`.
///
/// A pure function of the session, so what the list says is checkable without a
/// window — `ui::mod`'s standing complaint about the rest of this file's
/// contents.
pub fn row_line(session: &Session, index: usize) -> String {
    let Some(row) = session.song_row(index) else {
        return String::new();
    };
    let scene = session
        .scenes
        .get(row.scene)
        .map(|s| s.name.as_str())
        .unwrap_or("—");
    let length = match row.length_steps {
        Some(steps) => format!("{steps:03}"),
        None => String::from("cyc"),
    };
    format!(
        "{:02} {:<8.8} {:<9.9} ×{:<2} {} {}",
        index + 1,
        row.label,
        scene,
        row.plays(),
        length,
        if row.has_mutes() { "M" } else { " " }
    )
}

/// The scene's own cycle, in reference steps — what a row that inherits its
/// length will actually last.
///
/// `Session::scene_boundary_steps` counts *steps* and the engine converts with
/// SCALE, so these two agree exactly where every track is at 1x and this is the
/// nearest honest number the UI can show otherwise. It is a display value: the
/// engine never reads it.
fn session_scene_steps(session: &Session, scene: usize) -> Option<u16> {
    session.scene_boundary_steps(scene)
}

fn resolved_length(steps: Option<u16>) -> u16 {
    clamp_row_length(steps.unwrap_or(16))
}

fn reference_prose(ui: &mut Ui) {
    super::consequence_line(
        ui,
        "A row plays a scene, so one row moves every box in the session at the \
         same boundary. Clicking a row selects it to edit; the ▶ beside it moves \
         the playhead there.",
    );
    super::consequence_line(
        ui,
        "LEN off means the row lasts its scene's own cycle — the longest track in \
         it, which is the boundary a queued scene change already waits for. LEN on \
         counts steps as 1/16 at 1x and cuts every track short, which is what a \
         fill row is.",
    );
    super::consequence_line(
        ui,
        "PLAYS repeats the row without re-launching its patterns, so 1ST fires \
         once across the whole row and LST fires on the last pass before the \
         scene changes. A row with its own LEN does re-launch them, because it \
         cut them short.",
    );
    super::consequence_line(
        ui,
        "ROW MUTE replaces the pattern's own mute state rather than adding to it, \
         so a row can silence a track the pattern plays and sound one it mutes. \
         Solo is the desk, not the arrangement, and a row never overrides it.",
    );
    super::consequence_line(
        ui,
        "Undo does not reach the song. The history is over the notes — the same \
         line the key is on — so a removed row is gone, though the session is \
         marked unsaved either way.",
    );
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::song::SongRow;

    fn session() -> Session {
        let mut s = digi_core::default_session();
        s.scenes[0].name = "Intro".into();
        s.add_scene("Chorus", Some(0));
        s
    }

    #[test]
    fn a_row_line_puts_the_columns_where_the_box_puts_them() {
        let mut s = session();
        s.add_song_row(0).unwrap();
        let row = s.song_mut().row_mut(0).unwrap();
        row.label = "VERSE".into();
        row.repeats = 4;
        row.length_steps = Some(16);

        assert_eq!(row_line(&s, 0), "01 VERSE    Intro     ×4  016  ");
    }

    #[test]
    fn a_row_that_inherits_its_length_says_so_rather_than_showing_a_number() {
        let mut s = session();
        s.add_song_row(1).unwrap();
        assert!(row_line(&s, 0).contains("cyc"));
        assert!(row_line(&s, 0).contains("Chorus"));
    }

    #[test]
    fn a_row_with_mutes_is_marked_and_one_without_is_not() {
        let mut s = session();
        let dt2 = s.devices[0].id;
        s.add_song_row(0).unwrap();
        assert!(row_line(&s, 0).ends_with(' '));
        s.song_mut().row_mut(0).unwrap().set_mute(dt2, 0, true);
        assert!(row_line(&s, 0).ends_with('M'));
    }

    #[test]
    fn a_long_label_is_truncated_rather_than_pushing_the_columns_along() {
        // The list is monospace and fixed-width on purpose: a label nobody
        // shortened must not move the scene column on one row and not the others.
        let mut s = session();
        s.add_song_row(0).unwrap();
        s.song_mut().row_mut(0).unwrap().label = "A VERY LONG SECTION NAME".into();
        let long = row_line(&s, 0);
        s.song_mut().row_mut(0).unwrap().label = "V".into();
        let short = row_line(&s, 0);
        assert_eq!(long.len(), short.len());
    }

    #[test]
    fn a_row_naming_a_scene_that_has_gone_shows_a_dash_not_a_panic() {
        let mut s = session();
        s.song_mut().push(SongRow::new(9)).unwrap();
        assert!(row_line(&s, 0).contains('—'));
    }

    #[test]
    fn a_row_that_is_not_there_draws_nothing() {
        assert_eq!(row_line(&session(), 3), "");
    }

    #[test]
    fn a_headless_pass_draws_every_row_and_both_mute_grids_without_panicking() {
        // **What this catches is an egui Id collision**, which no pure function in
        // this file can see: the row list draws a `selectable_label` and a `▶`
        // button per row, the scene picker is a `ComboBox` inside a section that
        // redraws per selection, and the mute grid is sixteen more labels per box.
        // Two widgets sharing an Id in egui is one widget that eats the other's
        // clicks, and it does not panic — it just quietly does nothing, which is
        // exactly the failure `ui::mod`'s selectable-labels note is about.
        //
        // It also draws the two branches a screen sweep cannot reach on the way
        // past: a row whose scene has been deleted, and a mute grid whose box
        // inherits rather than overriding.
        let ctx = egui::Context::default();
        let mut engine = EngineLink::with_sinks(Box::new(|_| {
            (Box::new(NullSink) as Box<dyn digi_engine::transport::PortSink>, Vec::new())
        }));
        let mut session = session();
        let dt2 = session.devices[0].id;
        session.add_song_row(0).unwrap();
        session.add_song_row(1).unwrap();
        session.song_mut().row_mut(0).unwrap().label = "INTRO".into();
        session.song_mut().row_mut(0).unwrap().length_steps = Some(8);
        session.song_mut().row_mut(1).unwrap().set_mute(dt2, 3, true);
        // A row naming a scene that is gone, which the list marks in amber.
        session.song_mut().push(SongRow::new(9)).unwrap();

        // Both reveals open, so the prose and the widest thing in the panel are
        // both in the pass.
        let mut panel = SongPanel {
            reference_visible: true,
            mutes_open: Some(dt2),
        };

        // `engine` is passed through rather than captured: a closure holding it
        // mutably would own it for the whole test, and the point is to move the
        // selection between frames.
        fn frame(
            ctx: &egui::Context,
            panel: &mut SongPanel,
            session: &mut Session,
            engine: &mut EngineLink,
        ) {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel.ui(ui, session, engine);
            });
            output.textures_delta.clear();
        }

        // Once per row selected, so the ROW editor is drawn against each of the
        // three — including the broken one, whose scene picker has no name to show.
        for row in 0..3 {
            engine.select_row(row);
            frame(&ctx, &mut panel, &mut session, &mut engine);
        }
        // And once with the mute grid closed, which is the other branch.
        panel.mutes_open = None;
        frame(&ctx, &mut panel, &mut session, &mut engine);
    }

    /// A sink that swallows everything: this test wants a panel drawn, not a
    /// `midir` connection, and `EngineLink::default` would try to open one.
    struct NullSink;

    impl digi_engine::transport::PortSink for NullSink {
        fn send(&mut self, _: digi_engine::event::PortId, _: &[u8]) {}
    }

    #[test]
    fn switching_a_row_onto_its_own_length_starts_from_what_it_was_lasting() {
        // Ticking the LEN box must not also change how long the row is.
        let s = session();
        assert_eq!(resolved_length(session_scene_steps(&s, 0)), 16);
        // And a scene with nothing to measure still gives a legal length.
        assert_eq!(resolved_length(None), 16);
    }
}
