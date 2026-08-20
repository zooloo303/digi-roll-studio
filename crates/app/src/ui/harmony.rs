// The Harmony panel: the rail's second slot, and the last one of the four that
// was a labelled empty box.
//
// This is `js/main.js`'s Harmony aside, and PLAN.md §5's three groups in its
// order:
//
//   KEY         root and scale, tinting the roll's matching rows
//   CHORD DRAW  one click stamps a whole chord, with a ghost under the cursor
//   HARMONISE   build a chord under each selected note
//
// ## The one rule this panel must not break
//
// **The key is visual only. It never restricts what you can draw.** That is
// `js/chords.js`'s rule, PLAN.md §5 asks for it to stay one, and it is why there
// is no "snap to scale" anywhere here: choosing a key tints rows and changes what
// *chord draw* builds, and a note put on an out-of-scale row by hand stays exactly
// where it was put. The panel says so in as many words, because a tint that looked
// like a constraint would have people transposing music to get out of it.
//
// ## Where the controls live, and why two of them are not here
//
// The settings are on the *session* (`Session::harmony`), not in this struct, for
// two reasons: they are saved with the session, and Phase 7's Generate panel edits
// the same Root and Scale rather than inventing a second pair — which is
// `js/main.js`'s own arrangement and the reason PLAN.md §6 runs harmony before the
// generator.
//
// So the only state here is the last thing the harmonise button did.
//
// **And there is deliberately no slot label at the top**, where `ui::edit` opens with
// one. Nothing in this panel is per-track: the key is the session's, chord draw is a
// mode, and harmonise acts on the selection — which is the thing the roll is already
// showing. A slot name here would say the key belongs to one of the 32 tracks, which
// is the opposite of why it is on `Session`.
//
// **Inversion is also reachable from the roll**, by alt+wheel while aiming, and
// that is the gesture the ghost was built for: see `PianoRoll::wheel_inversion`.
// The combo here and the wheel there move one field.
//
// ## The 2b pass: the same six rules, applied by inference
//
// `design_handoff_digi_roll_ui_v2/README.md` does not draw this panel — it is
// prose-only, one bullet under "Harmony and Session panels" — so what changed
// here is the six shared rules read onto a panel the mock never rendered:
// [`super::panel_title_bar`] (context is the key, since this is where the key
// is set — "C chromatic" when the scale is Off, "C Major" once one is chosen);
// [`super::section_header`] for KEY/CHORD DRAW/HARMONISE in place of the old
// bare [`super::caption`]; [`super::slider_row`] for Inversion and Strum; and
// the on-C5 voicing readout moved from its own label into CHORD DRAW's caption
// slot. Key and Quality, and the Chord draw checkbox, are untouched per the
// bullet's own instruction.
//
// **What "the three paragraphs" turned out to mean.** The bullet's "the three
// paragraphs and the green chord/PROB explanation go behind `?`" reads as three
// prose blocks plus one more, singled out because it is colour, not text. Read
// against the actual file that is: the chord-draw ghost paragraph, the key-tint
// paragraph, and the old `in_the_roll` gesture list, plus — separately — the
// green PROB paragraph that closed it. All four are now [`reference_prose`],
// gated by the title bar's `?`. Two casualties of adopting [`slider_row`] joined
// them: it returns a `bool`, not a `Response`, so the Inversion and Strum
// sliders' `on_hover_text` — real reference content, not the panel's ever-visible
// wording — had nowhere left to hang and moved into the same block rather than
// being dropped silently.
//
// **What stayed visible, and why.** The one-line "Alt+wheel over the roll cycles
// this while aiming" survives as a [`super::consequence_line`] under the
// Inversion slider rather than joining the rest behind `?`. The comment above it
// already made the case before this pass existed: alt+wheel is invisible input
// state, and a gesture nothing on screen advertises is a gesture nobody finds —
// the same argument that keeps it out of a tooltip. Hiding it behind a second
// click would undo the reason it was pulled out of one in the first place. And
// HARMONISE's old "Nothing selected — band-select in the roll first" label moved
// into that section's own header caption: it is exactly rule 2's Caption example
// ("nothing selected — sets new notes") restated, so it takes that slot instead
// of sitting on as a leftover plain label the new prose taxonomy has no name for.
//
// ## What this panel cannot claim
//
// Nothing here has been read off a screen yet — see `ui::mod`'s glyph note and
// PLAN.md §9's "not verified on a screen" list. Every mark drawn is ASCII, and the
// three the roll draws for this feature (the tint wash, the chord ghost and its
// stroke) are shapes rather than glyphs, so they cannot come out as tofu — but
// whether the 6% wash is *visible* is a question no test can answer.

use digi_core::chords::{
    harmonise, Harmonised, Quality, QualityChoice, Scale, PITCH_CLASSES, STRUM_TICKS_MAX,
};
use digi_core::Session;
use eframe::egui::{self, Ui};

use crate::ui::pianoroll::{note_name, PianoRoll};
use crate::ui::tracks::Selection;

/// What one frame of the panel did.
#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    pub close: bool,
    /// An edit the shell opens a history step around. **A key or chord setting
    /// counts**: it belongs in the session file, so it has to reach the dirty
    /// flag. It leaves no undo step, and that falls out rather than being
    /// arranged — `core::history` snapshots patterns and a key change touches no
    /// note.
    pub edited: bool,
}

/// The last thing the harmonise button did, shown until the next thing does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Harmonised(Harmonised),
    /// Already worded for a person.
    Failed(String),
}

#[derive(Debug, Default)]
pub struct HarmonyPanel {
    status: Option<Status>,
    /// Whether the `?` reveal is open. Per-panel and not yet wired into the
    /// session's persisted UI state (`referenceVisible` in the README's State
    /// Management list) — that plumbing is the shell's job, across all four
    /// panels at once, not this file's.
    reference_visible: bool,
}

impl HarmonyPanel {
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    // --- the decisions, reachable without a window ----------------------------

    /// Build a chord under every selected note.
    ///
    /// The added notes join the melody in the roll's selection, which is what lets
    /// an immediate drag move the whole harmonised phrase — `js/main.js` calls
    /// `setSelection([...selected, ...added])` for exactly that.
    ///
    /// Returns whether any note was added, so the shell can open a history step
    /// only when there is something to undo. A harmonise that adds nothing is not
    /// an error: the chords are already there, and the status line says so.
    pub fn harmonise_selection(
        &mut self,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
    ) -> bool {
        let selected = roll.selection();
        if selected.is_empty() {
            self.status =
                Some(Status::Failed(String::from("Select some notes to harmonise first")));
            return false;
        }
        // Copied out before the track is borrowed: the settings live on the session
        // and the track is inside it.
        let harmony = session.harmony;
        let Some(track) = crate::ui::tracks::track_mut(session, selection) else {
            self.status = Some(Status::Failed(String::from("no track is selected")));
            return false;
        };
        let band = PianoRoll::band(track);
        let out = harmonise(&mut track.notes, &selected, &harmony, band);
        let added = !out.is_empty();
        roll.select(selected.into_iter().chain(out.added.iter().copied()));
        self.status = Some(Status::Harmonised(out));
        added
    }

    // --- drawing ---------------------------------------------------------------

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
    ) -> Outcome {
        let context = key_context(session.harmony.root, session.harmony.scale);
        let mut out = Outcome {
            close: super::panel_title_bar(ui, "Harmony", &context, &mut self.reference_visible),
            edited: false,
        };
        egui::ScrollArea::vertical()
            .id_salt("harmony-panel")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.reference_visible {
                    reference_prose(ui);
                    ui.add_space(10.0);
                }
                out.edited |= key_group(ui, session);
                ui.add_space(10.0);
                out.edited |= self.chord_group(ui, session);
                ui.add_space(10.0);
                out.edited |= self.harmonise_group(ui, session, selection, roll);
            });
        out
    }

    /// CHORD DRAW: the mode, and the five settings that shape a voicing.
    fn chord_group(&mut self, ui: &mut Ui, session: &mut Session) -> bool {
        let mut harmony = session.harmony;
        let before = harmony;

        // The voicing, spelled out, moved into the section header's own caption
        // slot per 2b rule 4 rather than sitting below as its own label — the
        // README calls this move out by name. The ghost over the grid is the
        // preview for when the pointer is aiming; this is the one for when it is
        // over the panel instead.
        let root = 60 + harmony.root % 12;
        let names: Vec<String> = harmony
            .specs(root, 100, true, (0, 127))
            .iter()
            .map(|c| note_name(c.pitch))
            .collect();
        let voicing = if names.is_empty() {
            String::from("No chord: nothing in this voicing fits")
        } else {
            format!("On {}: {}", note_name(root), names.join(" "))
        };
        super::section_header(ui, "CHORD DRAW", Some(&voicing));

        ui.checkbox(&mut harmony.chord.on, "Chord draw")
            .on_hover_text(
                "One click on an empty cell stamps a whole chord. A note you click \
                 on still moves, resizes and deletes as usual.",
            );

        ui.add_space(4.0);
        let quality = &mut harmony.chord.quality;
        egui::ComboBox::from_label("Quality")
            .selected_text(quality.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(quality, QualityChoice::InScale, QualityChoice::InScale.label())
                    .on_hover_text(
                        "Walk the scale in thirds, so each degree gets its own \
                         natural quality: ii minor, V7 dominant, vii diminished. \
                         With the scale off this is a plain major triad.",
                    );
                for q in Quality::ALL {
                    ui.selectable_value(quality, QualityChoice::Fixed(q), q.label());
                }
            });

        ui.horizontal(|ui| {
            ui.checkbox(&mut harmony.chord.seventh, "7th").on_hover_text(
                "A fourth note. In scale it is the seventh of that degree, so V \
                 comes out dominant; on a fixed quality, Major takes the major 7th.",
            );
            ui.checkbox(&mut harmony.chord.spread, "Spread").on_hover_text(
                "Drop-2: the second note from the top goes down an octave, for an \
                 open voicing.",
            );
        });

        // Inversion and Strum adopt the shared slider row (2b rule 3): label,
        // track, value, in that reading order. The row works in `f32` and hands
        // back only a `bool`, so each value is copied out to a local, handed to
        // the row, and rounded back into the `u8` the model actually stores —
        // rounding rather than truncating so a click at the far end of the track
        // lands on 3, not 2.
        ui.add_space(4.0);
        let mut inversion = f32::from(harmony.chord.inversion);
        if super::slider_row(ui, "Inversion", &mut inversion, 0.0..=3.0, |v| {
            format!("{}", v.round() as i32)
        }) {
            harmony.chord.inversion = inversion.round().clamp(0.0, 3.0) as u8;
        }
        // **Said in the panel, not only in a tooltip.** Alt+wheel is invisible
        // state — nothing on screen distinguishes a roll that will cycle a voicing
        // from one that will scroll — and a gesture nothing advertises is a gesture
        // nobody finds. Same argument as the cursor icons in `ui::pianoroll`. This
        // is the one piece of this panel's prose that stays out of the `?` reveal;
        // see this file's top comment for why.
        super::consequence_line(ui, "Alt+wheel over the roll cycles this while aiming.");

        ui.add_space(4.0);
        let mut strum = f32::from(harmony.chord.strum);
        // Abbreviated to fit `slider_row`'s 38px value box — "2 ticks" does
        // not, and the unit is already given by the row's own label. The word
        // survives in the reference text behind `?`.
        if super::slider_row(ui, "Strum", &mut strum, 0.0..=f32::from(STRUM_TICKS_MAX), |v| {
            match v.round() as u8 {
                0 => String::from("off"),
                n => format!("{n}t"),
            }
        }) {
            harmony.chord.strum = strum.round().clamp(0.0, f32::from(STRUM_TICKS_MAX)) as u8;
        }

        session.harmony = harmony;
        harmony != before
    }

    /// HARMONISE: a chord under everything selected, and what that did.
    fn harmonise_group(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        selection: Selection,
        roll: &mut PianoRoll,
    ) -> bool {
        let selected = roll.selection().len();
        // The old "Nothing selected — band-select in the roll first" label is
        // rule 2's Caption form by another name — it is state qualifying this
        // section, exactly the shape of the README's own "nothing selected —
        // sets new notes" example — so it takes the header's caption slot
        // instead of a leftover standalone label. Once something is selected the
        // button's own "Harmonise N notes" wording already carries the count, so
        // the caption clears rather than repeating it.
        let caption = (selected == 0).then_some("nothing selected — band-select in the roll first");
        super::section_header(ui, "HARMONISE", caption);
        let mut edited = false;
        ui.horizontal(|ui| {
            let button = ui.add_enabled(
                selected > 0,
                egui::Button::new(match selected {
                    0 | 1 => String::from("Harmonise selection"),
                    n => format!("Harmonise {n} notes"),
                }),
            );
            if button
                .on_hover_text(
                    "Builds a chord under each selected note, using the settings \
                     above. The notes you drew are left exactly as they are; the \
                     added ones come in a touch softer so the line stays on top.",
                )
                .clicked()
            {
                edited = self.harmonise_selection(session, selection, roll);
            }
        });
        if let Some(status) = &self.status {
            let (text, colour) = match status {
                Status::Harmonised(out) => (
                    harmonise_message(out),
                    if out.is_empty() { super::CAUTION } else { super::ACCENT },
                ),
                Status::Failed(why) => (why.clone(), super::CAUTION),
            };
            ui.add_space(2.0);
            ui.label(egui::RichText::new(text).small().color(colour));
        }
        edited
    }
}

/// KEY: the root and the scale, and the promise that neither restricts anything.
///
/// Key and Quality stay exactly as they were per the README's own instruction;
/// only the caption above them moved to the shared [`super::section_header`],
/// and the paragraph that used to sit below them moved behind the title bar's
/// `?` — see [`reference_prose`].
fn key_group(ui: &mut Ui, session: &mut Session) -> bool {
    super::section_header(ui, "KEY", None);
    let before = session.harmony;
    let harmony = &mut session.harmony;

    ui.horizontal(|ui| {
        let root = &mut harmony.root;
        egui::ComboBox::from_id_salt("harmony-root")
            .selected_text(PITCH_CLASSES[usize::from(*root % 12)])
            .width(56.0)
            .show_ui(ui, |ui| {
                for (i, name) in PITCH_CLASSES.iter().enumerate() {
                    ui.selectable_value(root, i as u8, *name);
                }
            });
        let scale = &mut harmony.scale;
        egui::ComboBox::from_id_salt("harmony-scale")
            .selected_text(match scale {
                Some(s) => s.label(),
                None => "Off",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(scale, None, "Off");
                for s in Scale::ALL {
                    ui.selectable_value(scale, Some(s), s.label());
                }
            });
    });
    session.harmony != before
}

/// The title bar's context string: the key, since Harmony is where it is set.
/// "C chromatic" when the scale is Off — chromatic being the twelve-note
/// reading of "no scale chosen" — and "C Major" once one is. The README's own
/// example ("C chromatic · from Harmony") is Generate's context, borrowed here
/// on the reasoning that Harmony's title bar should read the same way for the
/// same key, since that string is *from* here.
fn key_context(root: u8, scale: Option<Scale>) -> String {
    let name = PITCH_CLASSES[usize::from(root % 12)];
    match scale {
        Some(s) => format!("{name} {}", s.label()),
        None => format!("{name} chromatic"),
    }
}

/// What a harmonise did, in one line a person can read.
///
/// **Both omissions are named**, because a chord tone that quietly did not appear
/// is the kind of thing someone finds out about at a write: `already_there` is a
/// pitch the step had, and `over_cap` is a note past the four a trig holds — the
/// one the encoder would otherwise have dropped for us.
pub fn harmonise_message(out: &Harmonised) -> String {
    let notes = |n: usize| if n == 1 { "note" } else { "notes" };
    let mut line = if !out.added.is_empty() {
        format!(
            "Harmonised {} {} — added {}",
            out.sources,
            notes(out.sources),
            out.added.len()
        )
    } else if out.over_cap > 0 {
        // **The two empty cases are not the same, and saying so is the point.**
        // "Already there" means the harmony was written; a full trig means it was
        // not, and would not have been by any means, because four notes is what the
        // card holds. Rolling both into one line would report a hardware limit as a
        // no-op.
        String::from("Nothing to add — those steps already hold the four notes a trig can carry")
    } else {
        String::from("Nothing to add — those chords are already there")
    };
    let mut asides: Vec<String> = Vec::new();
    if out.already_there > 0 {
        asides.push(format!(
            "{} {} already on the step",
            out.already_there,
            notes(out.already_there)
        ));
    }
    // Only alongside notes that *did* land: when nothing did, the head line above
    // has already said the trigs were full, and repeating the count would be the
    // same sentence twice.
    if out.over_cap > 0 && !out.added.is_empty() {
        asides.push(format!("{} left out: a trig holds four notes", out.over_cap));
    }
    if !asides.is_empty() {
        line.push_str(" · ");
        line.push_str(&asides.join(" · "));
    }
    line
}

/// Everything 2b rule 1 moves off the control surface, folded behind the title
/// bar's `?`: the chord-draw ghost paragraph, the key-tint paragraph, the two
/// slider tooltips [`slider_row`](super::slider_row) had no `Response` left to
/// carry, the gesture list `in_the_roll` used to draw unconditionally, and —
/// called out by name in the README's Harmony bullet — the green PROB
/// paragraph that closed it. See this file's top comment for how "the three
/// paragraphs and the green chord/PROB explanation" maps onto these blocks.
fn reference_prose(ui: &mut Ui) {
    super::caption(ui, "CHORD DRAW");
    ui.label(
        egui::RichText::new(
            "The chord a click would stamp follows the cursor as a ghost. \
             Anything the ghost leaves out is a note that would not fit: a \
             pitch the step already holds, or a fifth note on a trig that \
             keeps four.",
        )
        .weak()
        .small(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Inversion moves notes from the bottom of the chord to the top, an \
             octave up.",
        )
        .weak()
        .small(),
    );
    ui.label(
        egui::RichText::new(
            "Strum staggers the chord bottom-up, by whole micro-timing ticks — \
             the box's own 1/24 of a step. It is real micro-timing, so it \
             survives a write and comes back off the box unchanged. Three \
             ticks is as wide as a four-note chord goes before the top of it \
             hits the micro clamp.",
        )
        .weak()
        .small(),
    );

    ui.add_space(8.0);
    super::caption(ui, "KEY");
    ui.label(
        egui::RichText::new(
            "Tints the roll's matching rows, the root a shade stronger. Visual \
             only: it never restricts what you can draw, and it is what `In scale` \
             chord draw walks.",
        )
        .weak()
        .small(),
    );

    ui.add_space(8.0);
    super::caption(ui, "IN THE ROLL");
    let green = egui::Color32::from_rgb(0x7a, 0xa8, 0x4a);
    for (what, how) in [
        (
            "Chord",
            "Click an empty cell to stamp the ghost. Drag right and every note of \
             it lengthens together.",
        ),
        ("Inversion", "Alt+wheel over the grid, while the ghost is under the cursor."),
        (
            "Strum",
            "Cmd-drag one note of a stamped chord sideways to place it by hand — \
             the same field the strum sets.",
        ),
    ] {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new(what).strong().small());
            ui.label(egui::RichText::new(how).small().weak());
        });
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "A chord is one trig on the box: four notes at most, and the dice are \
             rolled once for all of them, so a PROB or a condition on that step \
             fires the whole chord or none of it.",
        )
        .small()
        .color(green),
    );
}
