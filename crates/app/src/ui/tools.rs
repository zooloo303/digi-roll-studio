// The left panel: one body per rail tool.
//
// **All five are real panels now.** Song took a fifth position on 2026-08-22
// with PLAN.md §6 phase 12 and is drawn by `ui::song` — the arrangement, and the
// one panel here that reads the engine as well as the session, because a row is
// only meaningful beside the pointer walking it.
//
// The four before it: Session took the rail's fourth position when
// banks were cut on 2026-08-18 and is drawn by `ui::session`; Edit stopped being a
// slot in Phase 9 (`ui::edit`); Harmony in Phase 11 (`ui::harmony`); and Generate
// — the last one, `crates/generator` having shipped its five build stages with
// nothing in `app` calling any of it — is drawn by `ui::generate`. Every arm of
// [`ui`] below delegates and returns.
//
// **The rule this file followed while any of them was still a labelled empty
// slot, kept here because the next one will be too:** no widget in a
// not-yet-built body may look operable, and each slot says what lands in it and
// what it waits on. A labelled empty slot is a decision; a missing one is a
// question, asked again every session. And when a slot becomes a panel, **the
// entry changes in the same change that ships it** — this file once told a user
// the trig lane and the p-lock lanes were "not built" for a day after both had
// shipped, spotted by Neil in a screenshot rather than by any test, and every
// panel since has landed its "this is real now" together with the panel itself.

use eframe::egui::Ui;

use crate::engine::EngineLink;

use digi_core::device::PortRef;
use digi_core::history::History;
use digi_core::Session;

use crate::ui::edit::EditPanel;
use crate::ui::generate::GeneratePanel;
use crate::ui::harmony::HarmonyPanel;
use crate::ui::pianoroll::PianoRoll;
use crate::ui::rail::Tool;
use crate::ui::session::SessionPanel;
use crate::ui::song::SongPanel;
use crate::ui::tracks::Selection;

/// What one frame of the left panel did, whichever tool it was showing.
///
/// The union of the five panels' outcomes, because the shell has one call site
/// and every answer reaches it through this. `reloaded` is Session's, `stepped`
/// is Edit's, and `edited` is Edit's, Harmony's, Generate's and Song's.
#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    pub close: bool,
    /// The session was replaced by one off disk.
    pub reloaded: bool,
    /// An ordinary edit — the shell opens a history step around it.
    pub edited: bool,
    /// The history moved. Sync the engine; do **not** record a step.
    pub stepped: bool,
}

/// Draw the panel for `tool`. Every arm draws its own panel and returns —
/// [`EditPanel::ui`], [`HarmonyPanel::ui`], [`GeneratePanel::ui`] and
/// [`SessionPanel::ui`] — so this function hands each the session rather than a
/// description of what it would do if it existed.
#[allow(clippy::too_many_arguments)]
pub fn ui(
    ui: &mut Ui,
    tool: Tool,
    session_panel: &mut SessionPanel,
    edit_panel: &mut EditPanel,
    harmony_panel: &mut HarmonyPanel,
    generate_panel: &mut GeneratePanel,
    song_panel: &mut SongPanel,
    session: &mut Session,
    engine: &mut EngineLink,
    selection: Selection,
    roll: &mut PianoRoll,
    history: &mut History,
    available_in: &[PortRef],
    available_out: &[PortRef],
) -> Outcome {
    match tool {
        Tool::Session => {
            let out = session_panel.ui(ui, session, available_in, available_out);
            Outcome {
                close: out.close,
                reloaded: out.reloaded,
                edited: false,
                stepped: false,
            }
        }
        Tool::Edit => {
            let out = edit_panel.ui(ui, session, selection, roll, history);
            Outcome {
                close: out.close,
                reloaded: false,
                edited: out.edited,
                stepped: out.stepped,
            }
        }
        Tool::Harmony => {
            let out = harmony_panel.ui(ui, session, selection, roll);
            Outcome {
                close: out.close,
                reloaded: false,
                edited: out.edited,
                stepped: false,
            }
        }
        Tool::Generate => {
            let out = generate_panel.ui(ui, session);
            Outcome {
                close: out.close,
                reloaded: false,
                edited: out.edited,
                stepped: false,
            }
        }
        Tool::Song => {
            let out = song_panel.ui(ui, session, engine);
            Outcome {
                close: out.close,
                reloaded: false,
                edited: out.edited,
                stepped: false,
            }
        }
    }
}
