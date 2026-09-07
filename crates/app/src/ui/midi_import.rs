// The "Import MIDI file — as a song" dialog: Stage 3 of PLAN.md §11
// §5, the second of §5.1's two gestures over the one engine.
//
// The Edit panel's IMPORT answers "into this track"; this dialog answers "as a
// song", entered from the SONG panel and the SCENES panel ("IMPORT MIDI
// FILE…"). It is a thin shell over the pure halves: `midifile::score` answers
// what the file holds (Stage 1), `midi_import::fit` answers where it lands
// under a mapping (Stage 2), and `apply_import` writes the plan in one
// history step (§5.6). Everything on screen — the summary line, the refusal
// reason, the amber list of slots being replaced — is recomputed from `fit`
// on every change, because `fit` is pure and fast and a summary that can lag
// the controls is a summary that lies (§5.2).
//
// ## The auto-mapping (§5.3), and where it lives
//
// The rules — drums to the sample box, name matches against track names, poly
// to a poly box, low mono to a synth box, anything else to the first free
// track — run once, when the file is scored, in [`auto_map`]. "Free" means
// not already taken by an earlier rule in the same pass; notes already on a
// destination track do not block it, because the plan writes into **fresh**
// patterns in fresh slots, never into the current one (§5.3's own note).
// Everything the pass decides is an ordinary field of the dialog, so every
// default is editable before a byte is planned.
//
// ## What the dialog never does (§5.4)
//
// Merge two parts onto one track without the summary saying so. Clamp pitch.
// Invent `prob`/`fill`/`cond`. Change the tempo without the checkbox. Send a
// byte to a box — the plan's patterns reach hardware only through the usual
// SEND in Setup, which is not this dialog's business.
//
// ## Undo honesty
//
// `history::Content` holds patterns only, so one step here restores the
// imported *patterns*; the imported scenes and rows stay, exactly as a removed
// row is not undoable in the SONG panel (`ui::song`'s reference prose says
// so). The apply still goes down as one step — `history.begin` /
// `apply_import` / `history.commit` — because the patterns are the half of an
// import that overwriting a slot can lose.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use digi_core::history::{Content, History};
use digi_core::midi_import::{
    apply_import, default_pattern_bars, fit, gm_drum_name, AppliedImport, ImportPlan, Mapping,
    Options as MidiImportOptions, Overflow, PartMapping, PlanError,
};
use digi_core::midifile::score::{bar_starts, score_file, Part, Score};
use digi_core::model::TrackScale;
use digi_core::{DeviceId, PatternRef, Session};
use eframe::egui::{self, Ui};

use crate::engine::EngineLink;
use crate::ui::session::Chooser;

/// The name synonyms §5.3 rule 2 matches a part name against, checked against
/// a destination track's name in both directions, case-insensitive. Track
/// names on the desk come from fetched kits — "BD 909", "SD 707" — so the
/// DAW-side word a musician types ("Kick", "Snare") has to meet the kit-side
/// abbreviation halfway.
const SYNONYMS: &[&[&str]] = &[
    &["kick", "bd", "bass drum"],
    &["snare", "sd"],
    &["hat", "hh"],
    &["clap", "cp"],
    &["bass"],
    &["lead"],
    &["pad"],
    &["chord"],
    &["keys", "piano"],
];

/// A drum part's one row of the fan-out sub-table (§5.2): the pitch, which
/// destination track plays it, and the GM name — so a row reads "36 · Kick →
/// DT2 T3" rather than asking the user to hold the GM map in their head.
#[derive(Debug, Clone)]
pub struct DrumFanOut {
    pub pitch: u8,
    pub device: DeviceId,
    pub track: usize,
    /// Hit count, for §5.3's "ranked by hit count" cap and for the row label.
    pub hits: usize,
}

/// The state of one part's row (§5.2). `Drums` is a part-level choice with
/// per-pitch detail, so the destination enum and the fan-out table are
/// separate fields rather than one enum that has to carry a vector.
#[derive(Debug, Clone)]
pub struct PartState {
    pub mapping: PartMapping,
    /// Some while this part's destination is `Drums`: the editable fan-out.
    pub fan_out: Option<Vec<DrumFanOut>>,
}

/// A MIDI file waiting on the song dialog's answers: the file read and
/// scored, the per-part mapping (auto-mapped at open), and the options row.
pub struct SongImport {
    /// The file's base name, for the title bar and the pattern/scene names.
    pub name: String,
    /// The file stem — `name` minus its extension — which `fit` names the
    /// patterns and scenes after.
    stem: String,
    pub score: Score,
    /// One per `score.parts`, same order, as `Mapping` wants.
    pub parts: Vec<PartState>,
    pub options: MidiImportOptions,
}

impl SongImport {
    /// Score the file and pre-fill everything §5.2/§5.3 decide at open: the
    /// auto-mapping, the default pattern length, the tempo checkbox (on only
    /// when no track in the session has a note, §5.5).
    pub fn from_score(name: String, score: Score, session: &Session) -> Self {
        let stem = Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        let parts = auto_map(&score, session);
        // The allowed set is {1,2,4,8} and the default is the largest that
        // fits every destination box (§4.2, §5.2's options row).
        let destinations: Vec<&'static digi_core::DeviceModel> = session
            .devices
            .iter()
            .filter(|d| {
                parts.iter().any(|p| match &p.mapping {
                    PartMapping::Track { device, .. } | PartMapping::Drums { device, .. } => {
                        *device == d.id
                    }
                    PartMapping::Skip => false,
                })
            })
            .map(|d| d.model)
            .collect();
        let has_notes = session
            .devices
            .iter()
            .flat_map(|d| d.patterns.iter())
            .flat_map(|p| p.tracks().iter())
            .any(|t| !t.notes.is_empty());
        let pattern_bars = default_pattern_bars(&score, &destinations);
        Self {
            name,
            stem,
            score,
            parts,
            options: MidiImportOptions {
                pattern_bars,
                trim_leading_silence: true,
                start_slot: BTreeMap::new(),
                overwrite_occupied: false,
                apply_tempo: !has_notes,
                label_rows_from_markers: true,
            },
        }
    }

    /// The current answers as a `Mapping`, ready for `fit`.
    pub fn mapping(&self) -> Mapping {
        Mapping {
            parts: self
                .parts
                .iter()
                .map(|p| match (&p.mapping, &p.fan_out) {
                    (PartMapping::Drums { device, .. }, Some(fan_out)) => PartMapping::Drums {
                        device: *device,
                        tracks: fan_out.iter().map(|f| (f.pitch, f.track)).collect(),
                    },
                    _ => p.mapping.clone(),
                })
                .collect(),
        }
    }

    /// The live plan (§5.2's bottom line). Pure, and cheap enough to run on
    /// every change — that is the whole reason the summary can be live.
    pub fn plan(&self, session: &Session) -> Result<ImportPlan, PlanError> {
        fit(
            &self.score,
            &self.mapping(),
            &self.options,
            &self.stem,
            session,
        )
    }

    /// The summary the bottom line shows for a good plan (§5.2): unique
    /// patterns per box, scenes, rows, notes placed, and the honest counts of
    /// what fitting dropped.
    pub fn summary(plan: &ImportPlan, session: &Session) -> String {
        let report = &plan.report;
        let mut s = String::new();
        for (i, (device, count)) in report.unique_patterns.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let name = session
                .device(*device)
                .map(|d| d.name.as_str())
                .unwrap_or("?");
            s.push_str(&format!("{count} unique patterns on {name}"));
        }
        if report.unique_patterns.is_empty() {
            s.push_str("no patterns");
        }
        s.push_str(&format!(
            " · {} scenes · {} rows · {} notes placed",
            plan.scenes.len(),
            plan.rows.len(),
            report.notes_placed
        ));
        if report.clamped_at_boundary > 0 {
            s.push_str(&format!(
                " · {} clamped at boundaries",
                report.clamped_at_boundary
            ));
        }
        if report.notes_over_polyphony > 0 {
            s.push_str(&format!(
                " · {} dropped for polyphony",
                report.notes_over_polyphony
            ));
        }
        if report.drum_pitches_dropped > 0 {
            s.push_str(&format!(
                " · {} drum pitches dropped",
                report.drum_pitches_dropped
            ));
        }
        if report.cc_dropped > 0 {
            s.push_str(&format!(" · {} CC events not imported", report.cc_dropped));
        }
        s
    }
}

/// Which (device, track) a §5.3 rule can still take. Tracks are the unit of
/// "free": a box with sixteen tracks is sixteen destinations, and the drum
/// fan-out can take several of them in one rule.
struct FreeMap {
    taken: BTreeSet<(DeviceId, usize)>,
}

impl FreeMap {
    fn new() -> Self {
        Self {
            taken: BTreeSet::new(),
        }
    }

    fn take(&mut self, device: DeviceId, track: usize) {
        self.taken.insert((device, track));
    }

    /// The first free track on `device`, in track order.
    fn first_on(&self, session: &Session, device: DeviceId) -> Option<usize> {
        let model = session.device(device)?.model;
        (0..model.num_tracks).find(|t| !self.taken.contains(&(device, *t)))
    }

    /// How many tracks are still free on `device` — §5.3 rule 1's cap.
    fn free_on(&self, session: &Session, device: DeviceId) -> usize {
        session
            .device(device)
            .map(|d| {
                (0..d.model.num_tracks)
                    .filter(|t| !self.taken.contains(&(device, *t)))
                    .count()
            })
            .unwrap_or(0)
    }
}

/// Whether `part_name` and `track_name` meet through §5.3's synonym list, in
/// either direction ("Kick" ↔ "BD 909"). Plain `contains` would never join
/// the two, which is the whole reason the list exists.
fn names_meet(part_name: &str, track_name: &str) -> bool {
    let part = part_name.to_lowercase();
    let track = track_name.to_lowercase();
    SYNONYMS.iter().any(|group| {
        group.iter().any(|w| part.contains(w)) && group.iter().any(|w| track.contains(w))
    })
}

/// §5.3, run once at open. The rules in order, each taking only destinations
/// no earlier rule took. A part no rule claims is `Skip`, which the row's
/// destination dropdown then *says* — refused-by-default is the design's
/// answer to "where does this go" having no honest default.
pub fn auto_map(score: &Score, session: &Session) -> Vec<PartState> {
    let mut free = FreeMap::new();
    // The first sample-based box (§5.3 rule 1): the model whose default track
    // kind is audio *and* whose key names a sample box. `default_track_kind`
    // is `Audio` on every entry in the table today, so the kind alone cannot
    // tell a DT2 from a DN2 — the key can, and "DT2" is the only sampler the
    // roster holds. When a second sample box ships this grows a model field
    // rather than another arm, per the model's own rule.
    let sampler = session
        .devices
        .iter()
        .find(|d| d.model.key == "DT2")
        .map(|d| d.id);
    let states: Vec<PartState> = score
        .parts
        .iter()
        .map(|part| {
            // Rule 1: drum-like → the sampler, fanned out per pitch, ranked by
            // hit count and capped to the tracks still free.
            if part.stats.looks_like_drums {
                if let Some(device) = sampler {
                    let mut pitches: BTreeMap<u8, usize> = BTreeMap::new();
                    for note in &part.notes {
                        *pitches.entry(note.pitch).or_default() += 1;
                    }
                    let mut ranked: Vec<(u8, usize)> = pitches.into_iter().collect();
                    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    let cap = free.free_on(session, device);
                    let mut fan_out = Vec::new();
                    for (pitch, hits) in ranked.into_iter().take(cap) {
                        let track = free
                            .first_on(session, device)
                            .expect("cap is the free count");
                        free.take(device, track);
                        fan_out.push(DrumFanOut {
                            pitch,
                            device,
                            track,
                            hits,
                        });
                    }
                    return PartState {
                        mapping: PartMapping::Drums {
                            device,
                            tracks: Vec::new(),
                        },
                        fan_out: Some(fan_out),
                    };
                }
            }

            // Rule 2: a name match takes that track if it is free.
            for device in &session.devices {
                for (t, track) in device
                    .patterns
                    .first()
                    .map(|p| p.tracks())
                    .unwrap_or(&[])
                    .iter()
                    .enumerate()
                {
                    if !track.name.is_empty()
                        && names_meet(&part.name, &track.name)
                        && !free.taken.contains(&(device.id, t))
                    {
                        free.take(device.id, t);
                        return PartState {
                            mapping: PartMapping::Track {
                                device: device.id,
                                track: t,
                                overflow: Overflow::KeepLowest,
                                scale: TrackScale::One,
                            },
                            fan_out: None,
                        };
                    }
                }
            }

            // Rule 3: polyphonic → a box that can hold the chord, first free
            // track. The cap is `notes_per_trig`, read from the model, never a
            // literal — the same rule `fit` itself follows (§6).
            if part.stats.max_simultaneous > 1 {
                for device in &session.devices {
                    if device.model.notes_per_trig > 1 {
                        if let Some(t) = free.first_on(session, device.id) {
                            free.take(device.id, t);
                            return PartState {
                                mapping: PartMapping::Track {
                                    device: device.id,
                                    track: t,
                                    overflow: Overflow::KeepLowest,
                                    scale: TrackScale::One,
                                },
                                fan_out: None,
                            };
                        }
                    }
                }
            }

            // Rule 4: low mono (pitch_hi < 55, below G3 — bass territory) → a
            // synth box's first free track. "Synth" is the DN2 or the A4 —
            // the boxes whose engine plays pitched notes rather than samples;
            // the same key read rule 1 makes, inverted.
            if part.stats.max_simultaneous <= 1 && part.stats.pitch_hi < 55 {
                for device in &session.devices {
                    if device.model.key != "DT2" {
                        if let Some(t) = free.first_on(session, device.id) {
                            free.take(device.id, t);
                            return PartState {
                                mapping: PartMapping::Track {
                                    device: device.id,
                                    track: t,
                                    overflow: Overflow::KeepLowest,
                                    scale: TrackScale::One,
                                },
                                fan_out: None,
                            };
                        }
                    }
                }
            }

            // Rule 5: first free track anywhere, else Skip.
            for device in &session.devices {
                if let Some(t) = free.first_on(session, device.id) {
                    free.take(device.id, t);
                    return PartState {
                        mapping: PartMapping::Track {
                            device: device.id,
                            track: t,
                            overflow: Overflow::KeepLowest,
                            scale: TrackScale::One,
                        },
                        fan_out: None,
                    };
                }
            }
            PartState {
                mapping: PartMapping::Skip,
                fan_out: None,
            }
        })
        .collect();
    states
}

/// The panel-facing half: owns the file chooser and the open dialog, one
/// instance per entry point (SONG panel, SCENES panel) so a dialog opened
/// from one stays answerable while the rail moves.
pub struct MidiImportPanel {
    chooser: Box<dyn Chooser>,
    /// Some while a scored file is waiting on the dialog's answers.
    import: Option<SongImport>,
    /// The last thing that went wrong before the dialog could open, shown by
    /// the owning panel — a file that would not parse never reaches the
    /// dialog, so it cannot be reported inside it.
    failure: Option<String>,
}

impl Default for MidiImportPanel {
    fn default() -> Self {
        Self::with_chooser(Box::new(crate::ui::session::NativeChooser))
    }
}

// Manual rather than derived: the chooser is a `Box<dyn Chooser>`, which has
// no `Debug`, and the panel's interesting state is whether an import is
// pending. `SongPanel` derives `Debug` over this field.
impl std::fmt::Debug for MidiImportPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiImportPanel")
            .field("import", &self.import.as_ref().map(|i| &i.name))
            .field("failure", &self.failure)
            .finish()
    }
}

impl MidiImportPanel {
    pub fn with_chooser(chooser: Box<dyn Chooser>) -> Self {
        Self {
            chooser,
            import: None,
            failure: None,
        }
    }

    /// The import waiting on an answer, if one is — public so the owning
    /// panel (and the integration tests) can see the dialog is up.
    pub fn import(&self) -> Option<&SongImport> {
        self.import.as_ref()
    }

    /// Mutable, so a test can answer the dialog without clicking through it.
    pub fn import_mut(&mut self) -> Option<&mut SongImport> {
        self.import.as_mut()
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// IMPORT MIDI FILE…'s first half: ask for the file. Returns whether the
    /// dialog opened.
    pub fn begin_import(&mut self, session: &Session) -> bool {
        let Some(path) = self.chooser.open_midi() else {
            return false;
        };
        self.begin_import_from(&path, session)
    }

    /// Read and score `path`, opening the dialog on it. **Nothing about the
    /// session is touched until the file has parsed**, the same rule
    /// `ui::edit::import_midi_from` follows on the one-track gesture.
    pub fn begin_import_from(&mut self, path: &Path, session: &Session) -> bool {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.failure = Some(format!("could not read {}: {e}", path.display()));
                return false;
            }
        };
        let score = match score_file(&bytes) {
            Ok(score) => score,
            Err(e) => {
                self.failure = Some(format!("could not read {name}: {e}"));
                return false;
            }
        };
        if score.parts.is_empty() {
            self.failure = Some(format!("no notes found in {name}"));
            return false;
        }
        self.failure = None;
        self.import = Some(SongImport::from_score(name, score, session));
        true
    }

    /// IMPORT's answer: one history step, the report in the console, and the
    /// first imported scene selected — §5.6 step 5's "so the panel can".
    /// `history` comes from the caller rather than the frame's own begin/
    /// commit because this gesture must be exactly one step even mid-frame.
    pub fn apply(
        &mut self,
        session: &mut Session,
        history: &mut History,
        engine: &mut EngineLink,
        ctx: &egui::Context,
    ) -> Option<AppliedImport> {
        let import = self.import.as_ref()?;
        let plan = import.plan(session).ok()?;
        let apply_tempo = import.options.apply_tempo;
        let report = plan.report.clone();
        let name = import.name.clone();

        history.begin(Content::of(session));
        let applied = apply_import(session, plan, apply_tempo);
        history.commit(session);

        // The engine half of the tempo path (§5.5): the same call the
        // transport bar makes, outside the history step because the tempo is
        // not in `history::Content` at all.
        if apply_tempo {
            engine.set_tempo(session.tempo_bpm);
        }
        engine.select_scene(session, applied.first_scene);
        engine.select_row(applied.first_row);
        if applied.rows > 0 && !engine.song_mode() {
            engine.set_song_mode(session, true);
        }
        crate::ui::console::post(ctx, report_line(&name, &report, &applied));
        self.import = None;
        Some(applied)
    }

    /// The dialog's modal (§5.2). Returns true when an import landed, so the
    /// owning panel can mark the frame edited.
    pub fn dialog_ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        history: &mut History,
        engine: &mut EngineLink,
    ) -> bool {
        let Some(import) = &mut self.import else {
            return false;
        };
        let mut cancel = false;
        let mut apply_now = false;

        let response = egui::Modal::new(egui::Id::new("midi-import-song")).show(ui.ctx(), |ui| {
            ui.set_max_width(640.0);
            ui.label(egui::RichText::new(format!("Import {} as a song", import.name)).strong());
            ui.separator();

            // ---- top: what the file holds (§5.2) ----
            let score = &import.score;
            let bars = bar_starts(score);
            let meters: Vec<String> = score
                .meters
                .iter()
                .map(|&(_, m)| format!("{}/{}", m.num, m.den))
                .collect();
            let tempo = score.tempo.first().map(|&(_, bpm)| format!("{bpm:.1} BPM"));
            ui.horizontal_wrapped(|ui| {
                if let Some(bpm) = &tempo {
                    ui.label(format!("tempo {bpm}"));
                    ui.checkbox(&mut import.options.apply_tempo, "set session tempo");
                    ui.separator();
                }
                ui.label(format!("meter {}", meters.join(" then ")));
                ui.separator();
                ui.label(format!("{} bars", bars.len()));
                ui.separator();
                ui.label(format!("{} markers", score.markers.len()));
            });
            ui.add_space(6.0);

            // ---- middle: one row per part (§5.2) ----
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for index in 0..import.parts.len() {
                        part_row(ui, session, import, index);
                    }
                });
            ui.add_space(6.0);

            // ---- options row (§5.2) ----
            options_row(ui, session, import);
            ui.add_space(6.0);

            // ---- bottom: the live plan (§5.2) ----
            let plan = import.plan(session);
            match &plan {
                Ok(plan) => {
                    if !plan.report.replacing.is_empty() {
                        let mut body = String::from(
                            "This import writes into slots that already hold patterns:",
                        );
                        for (device, slot, old_name, old_notes) in &plan.report.replacing {
                            let name = session
                                .device(*device)
                                .map(|d| d.name.as_str())
                                .unwrap_or("?");
                            body.push_str(&format!(
                                "\n{name} {} — \"{old_name}\" ({old_notes} notes)",
                                slot.label()
                            ));
                        }
                        super::destructive_note(ui, "OVERWRITES OCCUPIED SLOTS", &body);
                        ui.add_space(4.0);
                    }
                    ui.label(SongImport::summary(plan, session));
                }
                Err(e) => {
                    ui.colored_label(super::CAUTION, e.to_string());
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                // Cancel leftmost, as every dialog in this app has it.
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui
                    .add_enabled(plan.is_ok(), egui::Button::new("IMPORT"))
                    .clicked()
                {
                    apply_now = true;
                }
            });
        });
        if response.should_close() {
            cancel = true;
        }

        if cancel {
            self.import = None;
            return false;
        }
        if apply_now {
            return self.apply(session, history, engine, ui.ctx()).is_some();
        }
        false
    }
}

/// One part's row (§5.2): the honest facts on the left, the destination and
/// policy on the right.
fn part_row(ui: &mut Ui, session: &Session, import: &mut SongImport, index: usize) {
    let part: &Part = &import.score.parts[index];
    let state = &mut import.parts[index];

    let range = if part.stats.notes > 0 {
        format!("{}–{}", part.stats.pitch_lo, part.stats.pitch_hi)
    } else {
        String::from("—")
    };
    let grid = format!(
        "{:.0}% off-grid",
        f64::from(part.stats.off_grid_ratio) * 100.0
    );
    let cc: usize = part.cc.values().sum();

    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(&part.name).strong());
        ui.weak(format!(
            "ch {} · {} notes · {} · poly {}",
            part.channel + 1,
            part.stats.notes,
            range,
            part.stats.max_simultaneous
        ));
        ui.weak(grid);
        if part.stats.looks_like_triplets {
            ui.colored_label(super::CAUTION, "3/2?");
        }
        // Greyed until the CC→p-lock work (§7): the count is honest, the
        // destination for it does not exist yet.
        ui.weak(format!("{cc} CC"));
    });

    // The destination dropdown: Skip, every (box, track) as the TRACKS grid
    // labels it, and — for drum-like parts — `Drums → {box}` per sampler.
    let selected = destination_label(session, state);
    egui::ComboBox::from_id_salt(("midi-import-dest", index))
        .selected_text(selected)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(matches!(state.mapping, PartMapping::Skip), "Skip")
                .clicked()
            {
                state.mapping = PartMapping::Skip;
                state.fan_out = None;
            }
            for device in &session.devices {
                for t in 0..device.model.num_tracks {
                    let label = track_label(session, device.id, t);
                    let is = matches!(
                        &state.mapping,
                        PartMapping::Track { device: d, track, .. } if *d == device.id && *track == t
                    );
                    if ui.selectable_label(is, &label).clicked() {
                        let overflow = match &state.mapping {
                            PartMapping::Track { overflow, .. } => *overflow,
                            _ => Overflow::KeepLowest,
                        };
                        state.mapping = PartMapping::Track {
                            device: device.id,
                            track: t,
                            overflow,
                            scale: TrackScale::One,
                        };
                        state.fan_out = None;
                    }
                }
            }
            if part.stats.looks_like_drums {
                for device in &session.devices {
                    if device.model.key != "DT2" {
                        continue;
                    }
                    let is = matches!(
                        &state.mapping,
                        PartMapping::Drums { device: d, .. } if *d == device.id
                    );
                    if ui
                        .selectable_label(is, format!("Drums → {}", device.name))
                        .clicked()
                    {
                        let device = device.id;
                        let fan_out = default_fan_out(part, device, session);
                        state.mapping = PartMapping::Drums { device, tracks: Vec::new() };
                        state.fan_out = Some(fan_out);
                    }
                }
            }
        });

    // The policy dropdown, shown only when the part's peak polyphony exceeds
    // the destination's per-trig cap (§5.2) — the only situation it changes
    // anything in.
    if let PartMapping::Track {
        device,
        track,
        overflow,
        ..
    } = &mut state.mapping
    {
        let cap = session
            .device(*device)
            .map(|d| d.model.notes_per_trig)
            .unwrap_or(1);
        if part.stats.max_simultaneous > cap {
            let this_device = *device;
            let this_track = *track;
            egui::ComboBox::from_id_salt(("midi-import-policy", index))
                .selected_text(match overflow {
                    Overflow::KeepLowest => "keep lowest",
                    Overflow::KeepHighest => "keep highest",
                    Overflow::SplitTo { .. } => "split",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(overflow, Overflow::KeepLowest, "keep lowest");
                    ui.selectable_value(overflow, Overflow::KeepHighest, "keep highest");
                    for device in &session.devices {
                        for t in 0..device.model.num_tracks {
                            if device.id == this_device && t == this_track {
                                continue;
                            }
                            ui.selectable_value(
                                overflow,
                                Overflow::SplitTo {
                                    device: device.id,
                                    track: t,
                                },
                                format!("split → {}", track_label(session, device.id, t)),
                            );
                        }
                    }
                });
        }
    }

    // The drum fan-out sub-table: one row per pitch, GM name, hit count, and
    // the destination track as a picker.
    if let Some(fan_out) = &mut state.fan_out {
        ui.indent(("midi-import-fanout", index), |ui| {
            for row in fan_out.iter_mut() {
                ui.horizontal(|ui| {
                    ui.weak(format!(
                        "{} · {} · {} hits",
                        row.pitch,
                        gm_drum_name(row.pitch),
                        row.hits
                    ));
                    let mut track = row.track;
                    egui::ComboBox::from_id_salt(("midi-import-drum", index, row.pitch))
                        .selected_text(format!("T{}", track + 1))
                        .show_ui(ui, |ui| {
                            let tracks = session
                                .device(row.device)
                                .map(|d| d.model.num_tracks)
                                .unwrap_or(0);
                            for t in 0..tracks {
                                ui.selectable_value(&mut track, t, format!("T{}", t + 1));
                            }
                        });
                    row.track = track;
                });
            }
        });
    }
    ui.add_space(4.0);
}

/// The default fan-out for a drum part aimed at `device`: every pitch, ranked
/// by hit count, capped to the box's track count — §4.5's shape. Unlike the
/// auto-mapper's pass this ignores what other parts took: the user just aimed
/// this part here by hand, and "free" was the auto-mapper's bookkeeping, not
/// theirs.
fn default_fan_out(part: &Part, device: DeviceId, session: &Session) -> Vec<DrumFanOut> {
    let mut pitches: BTreeMap<u8, usize> = BTreeMap::new();
    for note in &part.notes {
        *pitches.entry(note.pitch).or_default() += 1;
    }
    let mut ranked: Vec<(u8, usize)> = pitches.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let cap = session
        .device(device)
        .map(|d| d.model.num_tracks)
        .unwrap_or(0);
    ranked
        .into_iter()
        .take(cap)
        .enumerate()
        .map(|(t, (pitch, hits))| DrumFanOut {
            pitch,
            device,
            track: t,
            hits,
        })
        .collect()
}

/// The options row (§5.2): pattern length, a start slot per destination box,
/// trim, overwrite.
fn options_row(ui: &mut Ui, session: &Session, import: &mut SongImport) {
    ui.horizontal_wrapped(|ui| {
        ui.label("pattern length");
        for bars in [1u8, 2, 4, 8] {
            ui.selectable_value(
                &mut import.options.pattern_bars,
                bars,
                format!("{bars} bars"),
            );
        }
        ui.separator();
        ui.checkbox(
            &mut import.options.trim_leading_silence,
            "trim leading silence",
        );
        ui.checkbox(
            &mut import.options.overwrite_occupied,
            "overwrite occupied slots",
        );
    });

    // One slot picker per box any part lands on, in device order — the
    // scenes.rs picker idiom: `from_slot` over the device's slot count.
    let mut destinations: Vec<DeviceId> = import
        .parts
        .iter()
        .filter_map(|p| match &p.mapping {
            PartMapping::Track { device, .. } | PartMapping::Drums { device, .. } => Some(*device),
            PartMapping::Skip => None,
        })
        .collect();
    destinations.sort();
    destinations.dedup();
    if destinations.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            for device in destinations {
                let Some(dev) = session.device(device) else {
                    continue;
                };
                ui.weak(format!("{} from", dev.name));
                let mut chosen = import
                    .options
                    .start_slot
                    .get(&device)
                    .copied()
                    .unwrap_or(PatternRef::new(0, 0));
                egui::ComboBox::from_id_salt(("midi-import-slot", device.0))
                    .selected_text(chosen.label())
                    .width(52.0)
                    .show_ui(ui, |ui| {
                        for slot in 0..dev.patterns.len() {
                            let candidate = PatternRef::from_slot(slot);
                            ui.selectable_value(&mut chosen, candidate, candidate.label());
                        }
                    });
                import.options.start_slot.insert(device, chosen);
            }
        });
    }
}

/// How the destination dropdown names the current answer.
fn destination_label(session: &Session, state: &PartState) -> String {
    match &state.mapping {
        PartMapping::Skip => String::from("Skip"),
        PartMapping::Track { device, track, .. } => track_label(session, *device, *track),
        PartMapping::Drums { device, .. } => {
            let name = session
                .device(*device)
                .map(|d| d.name.as_str())
                .unwrap_or("?");
            format!("Drums → {name}")
        }
    }
}

/// A destination as the TRACKS grid labels it: box name, `T{n}`, the track's
/// name when the kit gave it one (§5.2).
fn track_label(session: &Session, device: DeviceId, track: usize) -> String {
    let Some(dev) = session.device(device) else {
        return String::from("?");
    };
    let name = dev
        .patterns
        .first()
        .and_then(|p| p.track(track))
        .map(|t| t.name.as_str())
        .unwrap_or("");
    if name.is_empty() {
        format!("{} T{}", dev.name, track + 1)
    } else {
        format!("{} T{} {name}", dev.name, track + 1)
    }
}

/// The console line after a landing import (§5.6 step 5): what came in, and
/// everything fitting cost, in one line.
fn report_line(
    name: &str,
    report: &digi_core::midi_import::ImportReport,
    applied: &AppliedImport,
) -> String {
    let mut s = format!(
        "Imported {name} — {} patterns, {} scenes, {} rows, {} notes",
        applied.patterns, applied.scenes, applied.rows, report.notes_placed
    );
    let mut drops: Vec<String> = Vec::new();
    if report.clamped_at_boundary > 0 {
        drops.push(format!(
            "{} clamped at boundaries",
            report.clamped_at_boundary
        ));
    }
    if report.notes_over_polyphony > 0 {
        drops.push(format!(
            "{} dropped for polyphony",
            report.notes_over_polyphony
        ));
    }
    if report.drum_pitches_dropped > 0 {
        drops.push(format!(
            "{} drum pitches dropped",
            report.drum_pitches_dropped
        ));
    }
    if report.cc_dropped > 0 {
        drops.push(format!("{} CC events not imported", report.cc_dropped));
    }
    if report.tempo_changes_dropped > 0 {
        drops.push(format!(
            "{} tempo changes dropped",
            report.tempo_changes_dropped
        ));
    }
    if report.pitch_bend_dropped > 0 {
        drops.push(format!(
            "{} pitch-bend events dropped",
            report.pitch_bend_dropped
        ));
    }
    if !drops.is_empty() {
        s.push_str(&format!(" ({})", drops.join(", ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::midifile::score::{Meter, Part, PartStats, RawNote};
    use digi_core::{two_box_session, Note};

    const BAR: u64 = 96 * 4;

    /// A hand-built score with the parts given — same fixture style as
    /// `core::midi_import`'s tests, per part because the auto-mapper's rules
    /// are all per-part facts.
    fn score_of(parts: Vec<Part>, end_tick: u64) -> Score {
        Score {
            division: 96,
            tempo: vec![(0, 118.0)],
            meters: vec![(0, Meter { num: 4, den: 4 })],
            markers: Vec::new(),
            parts,
            end_tick,
        }
    }

    fn part(name: &str, channel: u8, notes: &[(u64, u64, u8)], drums: bool) -> Part {
        let stats = PartStats {
            notes: notes.len(),
            pitch_lo: notes.iter().map(|n| n.2).min().unwrap_or(0),
            pitch_hi: notes.iter().map(|n| n.2).max().unwrap_or(0),
            first_bar: 0,
            last_bar: 0,
            max_simultaneous: 1,
            off_grid_ratio: 0.0,
            looks_like_drums: drums,
            looks_like_triplets: false,
        };
        Part {
            mtrk: 0,
            channel,
            name: name.into(),
            program: None,
            notes: notes
                .iter()
                .map(|&(on, off, pitch)| RawNote {
                    on,
                    off,
                    pitch,
                    velocity: 100,
                })
                .collect(),
            stats,
            cc: BTreeMap::new(),
            sustain_events: 0,
            pitch_bend_events: 0,
        }
    }

    fn four_notes(pitch: u8) -> Vec<(u64, u64, u8)> {
        (0..4).map(|b| (b * BAR, b * BAR + 24, pitch)).collect()
    }

    // ---- §5.3 rule 1: drums → the sampler, fanned out ----

    #[test]
    fn a_drum_part_maps_to_the_dt2_fanned_out_by_hit_count() {
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        // Kick ×4, snare ×2, hat ×1 — the ranking must follow the hits.
        let mut notes = four_notes(36);
        notes.extend_from_slice(&[(0, 24, 38), (BAR, BAR + 24, 38), (0, 24, 42)]);
        let score = score_of(vec![part("Drums", 9, &notes, true)], 4 * BAR);

        let states = auto_map(&score, &session);
        let fan_out = states[0].fan_out.as_ref().expect("a fan-out");
        assert_eq!(fan_out.len(), 3);
        assert_eq!(fan_out[0].pitch, 36, "most hits first");
        assert_eq!(fan_out[0].device, dt2);
        // Distinct free tracks, in order.
        let tracks: BTreeSet<usize> = fan_out.iter().map(|f| f.track).collect();
        assert_eq!(tracks.len(), 3);
        assert!(matches!(states[0].mapping, PartMapping::Drums { device, .. } if device == dt2));
    }

    #[test]
    fn a_drum_part_on_a_synth_only_desk_falls_through_to_the_later_rules() {
        // No DT2 on the desk: rule 1 cannot fire, and a mono drum part is not
        // low (pitch_hi ≥ 55 only when the part is high) — so rule 5's first
        // free track anywhere takes it rather than it being skipped.
        let mut session = Session::default();
        session.add_device(digi_core::Device::new("DN2", &digi_core::DN2, 16));
        let dn2 = session.devices[0].id;
        let score = score_of(vec![part("Drums", 9, &four_notes(60), true)], 4 * BAR);

        let states = auto_map(&score, &session);
        assert!(
            matches!(states[0].mapping, PartMapping::Track { device, .. } if device == dn2),
            "fell through to a plain track: {:?}",
            states[0].mapping
        );
    }

    // ---- §5.3 rule 2: name match, through the synonym list ----

    #[test]
    fn a_part_named_kick_takes_the_track_named_bd_909() {
        let mut session = two_box_session();
        let dt2 = session.devices[0].id;
        session
            .device_mut(dt2)
            .unwrap()
            .pattern_mut(0)
            .unwrap()
            .track_mut(4)
            .unwrap()
            .name = "BD 909".into();
        let score = score_of(vec![part("Kick", 0, &four_notes(48), false)], 4 * BAR);

        let states = auto_map(&score, &session);
        assert!(
            matches!(states[0].mapping, PartMapping::Track { device, track: 4, .. } if device == dt2),
            "the synonym list joined Kick to BD 909: {:?}",
            states[0].mapping
        );
    }

    #[test]
    fn a_name_match_does_not_steal_a_track_an_earlier_part_took() {
        let mut session = two_box_session();
        let dt2 = session.devices[0].id;
        session
            .device_mut(dt2)
            .unwrap()
            .pattern_mut(0)
            .unwrap()
            .track_mut(4)
            .unwrap()
            .name = "BD 909".into();
        let score = score_of(
            vec![
                part("Kick", 0, &four_notes(48), false),
                part("Kick 2", 0, &four_notes(50), false),
            ],
            4 * BAR,
        );

        let states = auto_map(&score, &session);
        let first = match &states[0].mapping {
            PartMapping::Track { track, .. } => *track,
            m => panic!("rule 2 took the first Kick: {m:?}"),
        };
        assert_eq!(first, 4);
        // The second Kick finds T4 taken and falls to a later rule — anywhere
        // but T4.
        match &states[1].mapping {
            PartMapping::Track { track, .. } => assert_ne!(*track, 4),
            m => panic!("the second Kick still landed somewhere: {m:?}"),
        }
    }

    // ---- §5.3 rule 3: polyphonic → a poly box ----

    #[test]
    fn a_polyphonic_part_takes_a_track_on_a_poly_box() {
        let session = two_box_session();
        let dn2 = session.devices[1].id;
        let mut chords = part("Chords", 0, &four_notes(60), false);
        chords.stats.max_simultaneous = 4;
        let score = score_of(vec![chords], 4 * BAR);

        let states = auto_map(&score, &session);
        // Every box in the roster has notes_per_trig 4 today, so the claim is
        // narrower than the rule: it lands *somewhere*, on the first box with
        // a free track — the DT2, which is also a poly box. The rule's real
        // teeth (skipping a mono box) have no fixture until the roster grows
        // one; what this pins is that polyphony routes by the rule at all.
        match &states[0].mapping {
            PartMapping::Track { device, .. } => {
                let model = session.device(*device).unwrap().model;
                assert!(model.notes_per_trig > 1);
                let _ = dn2;
            }
            m => panic!("a poly part mapped to a track: {m:?}"),
        }
    }

    // ---- §5.3 rule 4: low mono → a synth box ----

    #[test]
    fn a_low_mono_part_prefers_the_synth_box_over_the_sampler() {
        let session = two_box_session();
        let dn2 = session.devices[1].id;
        let score = score_of(vec![part("Sub", 0, &four_notes(36), false)], 4 * BAR);

        let states = auto_map(&score, &session);
        assert!(
            matches!(states[0].mapping, PartMapping::Track { device, .. } if device == dn2),
            "pitch_hi 36 < 55 routes to the synth: {:?}",
            states[0].mapping
        );
    }

    #[test]
    fn a_high_mono_part_does_not_claim_to_be_bass() {
        let session = two_box_session();
        let dt2 = session.devices[0].id;
        let score = score_of(vec![part("Lead", 0, &four_notes(72), false)], 4 * BAR);

        let states = auto_map(&score, &session);
        // Rule 5: first free track anywhere, which is the DT2's T1.
        assert!(
            matches!(states[0].mapping, PartMapping::Track { device, track: 0, .. } if device == dt2),
            "rule 5's first free track: {:?}",
            states[0].mapping
        );
    }

    // ---- §5.3 rule 5: nothing free → Skip ----

    #[test]
    fn more_parts_than_tracks_skip_the_overflow_rather_than_guess() {
        let mut session = Session::default();
        session.add_device(digi_core::Device::new("DT2", &digi_core::DT2, 16));
        // Sixteen high mono parts fill the box; the seventeenth has nowhere
        // honest to go, and Skip *says* so in the row's dropdown (§5.3).
        let parts: Vec<Part> = (0..17)
            .map(|i| part(&format!("Part {i}"), 0, &four_notes(72), false))
            .collect();
        let score = score_of(parts, 4 * BAR);

        let states = auto_map(&score, &session);
        assert_eq!(
            states
                .iter()
                .filter(|s| matches!(s.mapping, PartMapping::Skip))
                .count(),
            1,
            "exactly the seventeenth"
        );
    }

    // ---- the dialog's live plan: summary and error states ----

    fn song_import(session: &Session) -> SongImport {
        let score = score_of(vec![part("Lead", 0, &four_notes(72), false)], 4 * BAR);
        SongImport::from_score("lead.mid".into(), score, session)
    }

    #[test]
    fn the_live_plan_names_the_counts_the_bottom_line_shows() {
        let session = two_box_session();
        let import = song_import(&session);
        let plan = import.plan(&session).expect("a plan");
        let summary = SongImport::summary(&plan, &session);
        assert!(summary.contains("1 unique patterns on DT2"), "{summary}");
        assert!(summary.contains("1 scenes"), "{summary}");
        assert!(summary.contains("1 rows"), "{summary}");
        assert!(summary.contains("4 notes placed"), "{summary}");
    }

    #[test]
    fn a_plan_with_nowhere_to_go_is_an_error_not_a_summary() {
        let mut session = Session::default();
        // One slot, already holding music, no overwrite: NoFreeSlots.
        session.add_device(digi_core::Device::new("DT2", &digi_core::DT2, 1));
        let dt2 = session.devices[0].id;
        session
            .device_mut(dt2)
            .unwrap()
            .pattern_mut(0)
            .unwrap()
            .track_mut(0)
            .unwrap()
            .notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];

        let mut import = song_import(&session);
        // Aim at the DT2 by hand: the auto-map took T1 fine, the slot is the
        // problem.
        import.options.overwrite_occupied = false;
        let err = import.plan(&session).expect_err("no free slots");
        let text = err.to_string();
        assert!(text.contains("needs 1 pattern slots"), "{text}");
    }

    #[test]
    fn overwrite_occupied_turns_the_error_into_a_replacing_list() {
        let mut session = Session::default();
        session.add_device(digi_core::Device::new("DT2", &digi_core::DT2, 1));
        let dt2 = session.devices[0].id;
        session
            .device_mut(dt2)
            .unwrap()
            .pattern_mut(0)
            .unwrap()
            .track_mut(0)
            .unwrap()
            .notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];

        let mut import = song_import(&session);
        import.options.overwrite_occupied = true;
        let plan = import.plan(&session).expect("now it fits");
        assert_eq!(plan.report.replacing.len(), 1);
        assert_eq!(plan.report.replacing[0].3, 1, "the old note count");
    }

    // ---- §5.5: the tempo checkbox's default ----

    #[test]
    fn the_tempo_checkbox_defaults_on_only_when_the_session_is_noteless() {
        let session = two_box_session();
        let import = song_import(&session);
        assert!(
            import.options.apply_tempo,
            "a blank session takes the file's tempo"
        );

        let mut seeded = two_box_session();
        let dt2 = seeded.devices[0].id;
        seeded
            .device_mut(dt2)
            .unwrap()
            .pattern_mut(0)
            .unwrap()
            .track_mut(0)
            .unwrap()
            .notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
        let import = song_import(&seeded);
        assert!(!import.options.apply_tempo, "music on the desk wins (§5.5)");
    }

    // ---- the whole gesture, headless ----

    struct NullSink;
    impl digi_engine::transport::PortSink for NullSink {
        fn send(&mut self, _: digi_engine::event::PortId, _: &[u8]) {}
    }

    fn engine() -> EngineLink {
        EngineLink::with_sinks(Box::new(|_| {
            (
                Box::new(NullSink) as Box<dyn digi_engine::transport::PortSink>,
                Vec::new(),
            )
        }))
    }

    #[test]
    fn apply_lands_the_plan_selects_the_first_scene_and_posts_the_report() {
        let ctx = egui::Context::default();
        let mut session = two_box_session();
        let mut engine = engine();
        let mut history = History::default();
        let mut panel = MidiImportPanel::with_chooser(Box::new(crate::ui::session::NativeChooser));

        // Open the dialog straight from a score rather than a file on disk:
        // `begin_import_from`'s parsing half is `score_file`'s own tests'
        // business; the dialog's half starts at the Score.
        let score = score_of(vec![part("Lead", 0, &four_notes(72), false)], 4 * BAR);
        panel.import = Some(SongImport::from_score("lead.mid".into(), score, &session));

        let applied = panel
            .apply(&mut session, &mut history, &mut engine, &ctx)
            .expect("the plan landed");

        assert_eq!(applied.first_scene, 1);
        assert_eq!(
            engine.selected_scene(),
            1,
            "the first imported scene is selected"
        );
        assert_eq!(engine.selected_row(), 0);
        assert!(engine.song_mode(), "a song that exists plays");
        assert_eq!(session.scenes.len(), 2);
        assert_eq!(session.song().map(|s| s.rows.len()), Some(1));
        assert_eq!(
            session.tempo_bpm, 118.0,
            "blank session: tempo came with it"
        );
        assert!(panel.import.is_none(), "the dialog closed");
        // One undo step, and the patterns come back with it.
        assert_eq!(history.depth().0, 1);
    }

    #[test]
    fn a_seeded_session_keeps_its_own_tempo_unless_asked() {
        let ctx = egui::Context::default();
        let mut session = two_box_session();
        session.tempo_bpm = 90.0;
        let dt2 = session.devices[0].id;
        session
            .device_mut(dt2)
            .unwrap()
            .pattern_mut(0)
            .unwrap()
            .track_mut(0)
            .unwrap()
            .notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
        let mut engine = engine();
        let mut history = History::default();
        let mut panel = MidiImportPanel::with_chooser(Box::new(crate::ui::session::NativeChooser));

        let score = score_of(vec![part("Lead", 0, &four_notes(72), false)], 4 * BAR);
        let import = SongImport::from_score("lead.mid".into(), score, &session);
        assert!(!import.options.apply_tempo);
        panel.import = Some(import);

        panel.apply(&mut session, &mut history, &mut engine, &ctx);
        assert_eq!(session.tempo_bpm, 90.0, "the checkbox was off (§5.5)");
    }

    #[test]
    fn the_modal_draws_a_plan_and_a_refusal_without_panicking() {
        // The headless-pass argument from ui::song's own test: egui Id
        // collisions do not panic on their own, and the only way to see one is
        // to draw the thing — twice, once per branch the bottom line has.
        let ctx = egui::Context::default();
        let mut session = two_box_session();
        let mut engine = engine();
        let mut history = History::default();
        let mut panel = MidiImportPanel::with_chooser(Box::new(crate::ui::session::NativeChooser));
        let score = score_of(
            vec![
                part("Drums", 9, &four_notes(36), true),
                part("Lead", 0, &four_notes(72), false),
            ],
            4 * BAR,
        );
        panel.import = Some(SongImport::from_score("song.mid".into(), score, &session));

        fn frame(
            ctx: &egui::Context,
            panel: &mut MidiImportPanel,
            session: &mut Session,
            history: &mut History,
            engine: &mut EngineLink,
        ) {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel.dialog_ui(ui, session, history, engine);
            });
            output.textures_delta.clear();
        }

        // The plan branch, with the fan-out sub-table open.
        frame(&ctx, &mut panel, &mut session, &mut history, &mut engine);
        // The refusal branch: every slot occupied, no overwrite.
        for d in 0..session.devices.len() {
            let id = session.devices[d].id;
            for slot in 0..session.device(id).unwrap().patterns.len() {
                session
                    .device_mut(id)
                    .unwrap()
                    .pattern_mut(slot)
                    .unwrap()
                    .track_mut(0)
                    .unwrap()
                    .notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
            }
        }
        frame(&ctx, &mut panel, &mut session, &mut history, &mut engine);
        // And the overwrite branch, which draws the amber note.
        panel.import.as_mut().unwrap().options.overwrite_occupied = true;
        frame(&ctx, &mut panel, &mut session, &mut history, &mut engine);
    }
}
