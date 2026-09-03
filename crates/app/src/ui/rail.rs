// The left rail, and the two bits of state that say which side panels are open.
//
// digi-roll's rail was six icon-and-word buttons down the left edge, one panel
// open at a time, clicking the active one closing it. This is that, minus the
// icons: see [`super`] for why a glyph nobody has looked at is a liability, and
// `✎ ♬ ⚄ ▤` are four of them.
//
// The rail cannot be hidden. It is what reopens the tool panel, and the
// transport's `SETUP` toggle is what reopens the other one — a collapsible panel
// with no visible way back is a feature that has quietly gone.

use digi_core::Session;
use eframe::egui::{self, Color32, Ui};

/// The left panel's tools, in rail order. Each one is a slot in the workflow;
/// what is actually built behind them is in [`super::tools`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Edit,
    Harmony,
    Generate,
    Song,
    Presets,
    Session,
}

impl Tool {
    // **Presets sits fifth, above Session, and not at the bottom.** The rail is
    // in workflow order and Session is the file panel — save and open — which
    // has been the last row since banks were cut. A browser you pick sounds
    // from belongs with the things you compose with, not below the thing you
    // close the session with.
    pub const ALL: [Self; 6] =
        [Self::Edit, Self::Harmony, Self::Generate, Self::Song, Self::Presets, Self::Session];

    pub fn title(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Harmony => "Harmony",
            Self::Generate => "Generate",
            Self::Song => "Song",
            Self::Presets => "Presets",
            Self::Session => "Session",
        }
    }

    /// The rail's tooltip: what the panel is *for*, not what it currently has.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Edit => "Velocity, length, micro-timing, the trig lane and the p-lock lanes",
            Self::Harmony => "Key and scale, scale-tinted rows, chord draw, and a chord under everything selected",
            Self::Generate => "Bass, chords, lead and a kick/snare/hat kit, generated to agree with each other",
            Self::Song => "The arrangement: rows of scenes, played in order",
            Self::Presets => "The selected box's +Drive soundbanks, by bank, filtered by tag",
            Self::Session => "Save and open the session file — Cmd+S saves it from anywhere",
        }
    }

    /// The bare letter that opens this tool from anywhere in the window, or
    /// `None` for a tool that has no key.
    ///
    /// **Initials, and the one collision is resolved in Song's favour.** `S` is
    /// Song's because Song is a panel you switch *to* while writing. The Session
    /// panel's own verb is Save, and that is on `Cmd+S`
    /// ([`super::session::SessionPanel::save_shortcut`]) — a key that saves
    /// without opening any panel at all, which is the thing actually worth
    /// having. Giving Session some second-choice letter would be a shortcut
    /// nobody could guess and nobody would need.
    ///
    /// No modifier on any of them: the five that are bound are bound to the
    /// letter alone, guarded on nothing being typed into
    /// ([`shortcuts`]), and the chords stay free — `Shift+C`/`Shift+V` are the
    /// TRACKS clipboard (`super::tracks`) and `Cmd+Z` is the history.
    pub fn key(self) -> Option<egui::Key> {
        match self {
            Self::Edit => Some(egui::Key::E),
            Self::Harmony => Some(egui::Key::H),
            Self::Generate => Some(egui::Key::G),
            Self::Song => Some(egui::Key::S),
            Self::Presets => Some(egui::Key::P),
            Self::Session => None,
        }
    }

    /// Which tool a bare letter opens. Derived from [`Tool::key`] rather than
    /// written out a second time, so a key can never open one tool while the
    /// rail announces it as another's.
    pub fn from_key(key: egui::Key) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.key() == Some(key))
    }
}

/// The rail row's tooltip: what the panel is for, and the key that opens it.
///
/// The key is announced *here* rather than drawn in the row, for the same
/// reason the transport says "or press Space" in the PLAY button's hover text
/// and not on its face: the rail is 86px wide and its labels are the one thing
/// on it that must stay legible. A shortcut nobody is told about is a shortcut
/// nobody finds, and a hover is where this app has already decided to tell
/// people about its keys.
fn row_hint(tool: Tool) -> String {
    match tool.key() {
        Some(key) => format!("{} — press {}", tool.hint(), key.name()),
        None => tool.hint().to_string(),
    }
}

/// Which side panels are open, and which tool the left one is showing.
///
/// `tool` is remembered while `tool_open` is false, so reopening the panel comes
/// back to the tool that was last looked at rather than to the first one.
pub struct Sidebars {
    pub tool: Tool,
    pub tool_open: bool,
    pub setup_open: bool,
}

impl Default for Sidebars {
    fn default() -> Self {
        // Setup open, tools closed: nothing can be heard until a box has a port,
        // so that is the panel the app should open on.
        Self { tool: Tool::Edit, tool_open: false, setup_open: true }
    }
}

/// The active tool's label colour, per the v2 rail spec: `TEXT_BRIGHT` and
/// weight 500 when active, `TEXT_MUTED` otherwise. Pulled out of [`rail_row`]
/// so it can be checked without a `Ui` — text colour depends on `active` alone,
/// never on hover, unlike the fill.
fn rail_row_text_colour(active: bool) -> Color32 {
    if active { super::TEXT_BRIGHT } else { super::TEXT_MUTED }
}

/// The active tool's fill and left-border colour. `(background, border)`.
///
/// This is the whole point of `design_handoff_digi_roll_ui_v2/README.md`'s
/// colour-system rule: **filled cyan means a thing you can press.** The active
/// row is marked with a cyan *border*, never a cyan *fill* — a filled chip
/// would read as a button rather than as "this is the panel already open" —
/// so `border` is the only place `CYAN` appears here, and `background` never
/// carries it. `Color32::TRANSPARENT` for an inactive, unhovered row lets
/// whatever the rail's own background is show through, rather than painting a
/// second, redundant fill on top of it.
fn rail_row_fill(active: bool, hovered: bool) -> (Color32, Color32) {
    let background = if active {
        super::PANEL_BG_RAISED
    } else if hovered {
        super::INSET_BG_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let border = if active { super::CYAN } else { Color32::TRANSPARENT };
    (background, border)
}

/// One rail row, painted rather than left to `selectable_label`'s default
/// highlight — that highlight is a filled chip, which is exactly the treatment
/// the v2 colour system reserves for "a thing you can press" and therefore the
/// wrong mark for "the panel already open".
///
/// Follows the painter-plus-`interact` convention this codebase already uses
/// for its other custom-styled clickable rows (`setup.rs`'s `status_strip`,
/// [`super::disclosure_row`]): a placeholder shape reserves this row's draw
/// slot before its content is laid out, so the background can be filled with
/// *this* frame's hover state once `ui.interact`'s response is known, rather
/// than last frame's.
///
/// Returns whether the row was clicked, leaving what a click *means* — switch
/// tool, or close the open one — to the caller, the same division [`ui`] keeps
/// today.
fn rail_row(ui: &mut Ui, active: bool, label: &str, hint: &str) -> bool {
    let placeholder = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 8, right: 0, top: 7, bottom: 7 })
                .show(ui, |ui| {
                    let mut text =
                        egui::RichText::new(label).size(12.0).color(rail_row_text_colour(active));
                    if active {
                        text = text.strong();
                    }
                    ui.label(text);
                });
        })
        .response
        .on_hover_text(hint);

    let (background, border) = rail_row_fill(active, response.hovered());
    ui.painter().set(placeholder, egui::Shape::rect_filled(response.rect, 0.0, background));
    if border != Color32::TRANSPARENT {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(response.rect.left_top(), egui::vec2(2.0, response.rect.height())),
            0.0,
            border,
        );
    }

    response.clicked()
}

/// Where the support link goes. Held as a constant so the row and the test that
/// guards it cannot drift apart.
pub const KOFI_URL: &str = "https://ko-fi.com/zooloo303";

/// **ASCII, and that is the whole point.** [`super`]'s glyph table is the record
/// of four separate marks that shipped as tofu boxes, and a coffee cup (`☕`
/// U+2615) would be the fifth candidate. Its codepoint *is* in the cmap of two
/// of the four fonts egui bundles — and so is `●` U+25CF, which was still drawn
/// as a box on screen. Font coverage does not predict what egui paints, so the
/// only mark this row is allowed is one that cannot fail.
const KOFI_LABEL: &str = "Buy me a Ko-fi";

/// The support row at the foot of the rail.
///
/// Deliberately *not* a [`Tool`]: it opens a browser rather than a panel, and
/// putting it in `Tool::ALL` would make it a fifth tool slot — the one thing
/// this rail's four labels must keep meaning. It is separated from them by the
/// full height of the rail and a hairline, and it is the only row here whose
/// text stays [`super::TEXT_DIM`] until hovered.
///
/// No colour of its own. Ko-fi's brand red would read as [`super::CAUTION`] in
/// this window, and [`super`]'s colour rule is that a tint means the engine is
/// doing something — a donation link is the last thing in the app that should
/// look like a warning.
fn kofi_row(ui: &mut Ui) -> bool {
    let placeholder = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 8, right: 6, top: 7, bottom: 7 })
                .show(ui, |ui| {
                    // 10px and wrapping: the rail is 86px wide, so this sits as
                    // two short lines rather than being clipped at "Buy me a".
                    ui.label(
                        egui::RichText::new(KOFI_LABEL)
                            .size(10.0)
                            .color(if ui.ui_contains_pointer() {
                                super::TEXT_SECONDARY
                            } else {
                                super::TEXT_DIM
                            }),
                    );
                });
        })
        .response
        .on_hover_text(format!("Support Digi Roll Studio — {KOFI_URL}"));

    if response.hovered() {
        ui.painter().set(
            placeholder,
            egui::Shape::rect_filled(response.rect, 0.0, super::INSET_BG_HOVER),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    response.clicked()
}

/// What a rail row's click, and the row's letter, both mean.
///
/// Clicking (or pressing the letter of) the open tool closes the panel, as
/// digi-roll's rail did — it is the fastest way to get the roll back. Shared by
/// [`ui`] and [`shortcuts`] so the key can never drift from the click: whatever
/// `E` does is by construction what pressing `Edit` does.
fn open_or_close(bars: &mut Sidebars, tool: Tool) {
    if bars.tool == tool {
        bars.tool_open = !bars.tool_open;
    } else {
        bars.tool = tool;
        bars.tool_open = true;
    }
}

/// The rail's letters — `E` `H` `G` `S` `P` — read from the shell before any
/// panel is drawn. Returns whether the key was taken.
///
/// Read from the window rather than from the rail's own `ui` for the reason
/// `edit::shortcuts` and `transport::shortcuts` are: this runs before the
/// panels, so the frame that presses `G` is also the frame that draws Generate.
/// Nothing here touches the session, so there is nothing to report to the
/// engine and nothing for the history to record — which side panel is open is
/// desk state, not music.
///
/// ## Why this is not `consume_key(Modifiers::NONE, Key::E)`
///
/// The same two reasons `transport::space_tap` is not, and they bite harder for
/// a letter. `Modifiers::NONE` as a `consume_key` pattern does not mean "no
/// modifiers" — `matches_logically` only rejects a modifier the *pattern* asks
/// for and the input lacks (`modifiers.rs` ~211) — so `Shift+E` and `Alt+E`
/// would both open Edit, and `Shift+S` would open Song from under the TRACKS
/// clipboard's own chord family. `matches_exact` binds the letter alone and
/// leaves every chord on it free. And `count_and_consume_key` never looks at
/// `repeat`, so a leant-on `E` would flap the panel open and shut at the
/// key-repeat rate; only the first press of a hold counts here.
///
/// The `Event::Text("e")` that `egui-winit` pushes beside the key event goes
/// with it (`egui-winit` 0.36.1 `lib.rs` ~1046 — text is suppressed for a
/// command chord but not for a bare letter). Taking the key and leaving the
/// character is how pressing `S` for Song ends up typing an `s` into whatever
/// takes focus next.
///
/// Guarded on focus and on modals, exactly as the spacebar is. With a
/// `TextEdit` or a `DragValue` focused a letter is a character being typed and
/// nothing else; with a write, sync or restore dialog up, the window is a
/// question waiting for an answer and moving the panel behind it is not one. A
/// clicked TRACKS cell is the standing exemption — it holds focus, that is what
/// arms its Delete, and picking a track must not cost you the rail.
pub fn shortcuts(ui: &Ui, bars: &mut Sidebars, session: &Session) -> bool {
    let Some(tool) = tool_tapped(ui.ctx(), session) else {
        return false;
    };
    open_or_close(bars, tool);
    true
}

/// Which tool's letter arrived this frame, taking it — and the character beside
/// it — out of the queue. See [`shortcuts`] for why it is written out rather
/// than left to `consume_key`.
fn tool_tapped(ctx: &egui::Context, session: &Session) -> Option<Tool> {
    if crate::ui::tracks::typing_elsewhere(ctx, session)
        || ctx.memory(|m| m.top_modal_layer().is_some())
    {
        return None;
    }
    ctx.input_mut(|i| {
        let mut tapped: Option<Tool> = None;
        // The letter whose `Event::Text` twin is still to come, if a key was
        // taken this frame. `Key::name` is "E" for `Key::E`, and the twin is the
        // lower-case "e" — compared without case so a layout that reports it
        // otherwise cannot leave half a keypress behind.
        let mut took: Option<&'static str> = None;
        i.events.retain(|event| match event {
            egui::Event::Key { key, pressed: true, repeat, modifiers, .. }
                if modifiers.matches_exact(egui::Modifiers::NONE) =>
            {
                let Some(tool) = Tool::from_key(*key) else {
                    return true;
                };
                took = Some(key.name());
                // First press only, and the first letter only: two rail letters
                // in one frame is not a gesture, and the second would land on a
                // panel the first had just opened.
                if !*repeat && tapped.is_none() {
                    tapped = Some(tool);
                }
                false
            }
            egui::Event::Text(text) => !took.is_some_and(|letter| text.eq_ignore_ascii_case(letter)),
            _ => true,
        });
        tapped
    })
}

/// Draw the rail. Nothing here touches the session, so there is nothing to
/// report to the engine.
///
/// The whole column is filled with `INSET_BG` from here rather than left to
/// whatever default `egui::Panel` would otherwise paint behind it — the rail's
/// background was not one of this pass's named tokens before now. `main.rs`'s
/// `Panel::left("rail")` carries its own default inner margin outside this
/// `Ui`, so a sliver of that default fill can still show at the rail's outer
/// edge; that margin is `main.rs`'s call, not this file's.
pub fn ui(ui: &mut Ui, bars: &mut Sidebars) {
    egui::Frame::new()
        .fill(super::INSET_BG)
        .inner_margin(egui::Margin::symmetric(0, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(ui.available_height());
            // No gap between rows: the spec gives each row its own padding and
            // lets adjoining backgrounds (or the lack of one) mark the seam,
            // rather than a visible rule between them.
            ui.spacing_mut().item_spacing.y = 0.0;
            for tool in Tool::ALL {
                let active = bars.tool_open && bars.tool == tool;
                if rail_row(ui, active, tool.title(), &row_hint(tool)) {
                    open_or_close(bars, tool);
                }
            }

            // **The bottom-left corner of the window, and the rail is the only
            // panel that can offer one.** Both side panels collapse and the
            // workspace is the roll, which is never covered; the rail is the one
            // column that is always on screen, so it is the only corner a link
            // can sit in without vanishing when a panel is closed.
            //
            // `bottom_up` claims the height the four rows left over and stacks
            // from the floor, so the link is pinned to the bottom edge however
            // tall the window is — rather than floating under the last row.
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                if kofi_row(ui) {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(KOFI_URL));
                }
                // A hairline above it, drawn after the row in a bottom-up
                // layout so it lands *on top of* it on screen. Marks the link
                // as not one of the four tools without spending a colour on it.
                let line = ui.available_rect_before_wrap();
                ui.painter().hline(
                    line.x_range(),
                    line.max.y,
                    egui::Stroke::new(1.0, super::PANEL_BORDER),
                );
            });
        });
}

#[cfg(test)]
mod tests {
    /// How many rail rows open a panel — Edit, Harmony, Generate, Song,
    /// Presets, Session.
    /// One number rather than a literal in each test, so the next tool added
    /// moves it once and both assertions below keep meaning what they say.
    const PANEL_TOOLS: usize = 6;

    use super::*;

    #[test]
    fn the_kofi_row_is_ascii_only() {
        // The rail dropped its four icons because an unverified glyph is a
        // liability, and `super`'s table lists four marks that reached a window
        // as tofu boxes. This row must not become the fifth: `☕` renders in some
        // fonts egui bundles and `●` was in one too and still drew as a box, so
        // the guard is the codepoint range, not the font.
        assert!(
            KOFI_LABEL.is_ascii(),
            "the rail's support label must stay ASCII: {KOFI_LABEL:?}"
        );
    }

    #[test]
    fn the_kofi_link_is_neils_page_over_https() {
        assert_eq!(KOFI_URL, "https://ko-fi.com/zooloo303");
        // Plain http would be a donation link a network could rewrite.
        assert!(KOFI_URL.starts_with("https://"), "a payment link must be https");
        assert!(KOFI_URL.is_ascii(), "no confusable characters in a payment host");
    }

    #[test]
    fn the_support_link_is_not_a_tool() {
        // The rail is built from `ALL`, so anything added there becomes a panel
        // slot. The Ko-fi row opens a browser instead, and every label above it
        // has to keep meaning "a panel opens here".
        //
        // **This was `the_support_link_is_not_a_fifth_tool` until 2026-08-22**, and
        // then Song became a fifth tool that is a panel — so the name had gone
        // from stating the rule to contradicting it. The rule was never about
        // five; it is that the support row is not in `ALL`. `DEVELOPMENT.md`
        // lesson 4 is this, in the one direction it is easiest to miss: the count
        // in the assertion moved and the name did not.
        assert_eq!(Tool::ALL.len(), PANEL_TOOLS);
        for tool in Tool::ALL {
            assert!(
                !tool.title().to_ascii_lowercase().contains("ko-fi"),
                "the support link must not be a tool"
            );
        }
    }

    #[test]
    fn every_tool_has_a_title_and_a_hint() {
        // The rail is built from `ALL`, so a tool added to the enum and left out
        // of it simply never appears — which is the failure this catches.
        assert_eq!(Tool::ALL.len(), PANEL_TOOLS);
        for tool in Tool::ALL {
            assert!(!tool.title().is_empty());
            assert!(!tool.hint().is_empty());
        }
    }

    // --- the rail's letters ---------------------------------------------------

    /// A bare letter as the platform really sends it: the key, and the printable
    /// character `egui-winit` pushes beside it.
    ///
    /// Both, for the reason `ui::tracks`' clipboard comment spells out at
    /// length — a test that feeds the input the code expects rather than the
    /// input the platform produces cannot fail. The character is the half that
    /// would otherwise be left in the queue to be typed into whatever takes
    /// focus next, and the only way to catch that is to send it.
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

    /// Letting go, in a frame of its own. Not optional: `InputState::begin_pass`
    /// rewrites `repeat` from its own `keys_down` set, so a second press with no
    /// release between is a *held* key — which this shortcut ignores by design.
    fn release(key: egui::Key) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    /// One shell pass of the rail's shortcut read. Returns whether the key was
    /// taken, and what was left in the event queue after it.
    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        bars: &mut Sidebars,
        session: &digi_core::Session,
    ) -> (bool, Vec<egui::Event>) {
        let mut took = false;
        let mut left = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            took = shortcuts(ui, bars, session);
            left = ui.input(|i| i.events.clone());
        });
        output.textures_delta.clear();
        (took, left)
    }

    /// A press and the release after it, which is what one tap of a key is.
    fn tap(
        ctx: &egui::Context,
        key: egui::Key,
        bars: &mut Sidebars,
        session: &digi_core::Session,
    ) -> (bool, Vec<egui::Event>) {
        let pressed = frame(ctx, letter(key), bars, session);
        frame(ctx, release(key), bars, session);
        pressed
    }

    #[test]
    fn every_letter_is_the_tools_own_initial_and_no_two_share_one() {
        // The whole case for these five keys is that they are guessable. A
        // letter that is not the tool's initial is one more thing to memorise,
        // and two tools on one letter is a rail row that can never be reached.
        let mut seen = Vec::new();
        for tool in Tool::ALL {
            let Some(key) = tool.key() else { continue };
            assert_eq!(
                key.name(),
                tool.title()[..1].to_uppercase(),
                "{}'s key must be its initial",
                tool.title()
            );
            assert!(!seen.contains(&key), "{key:?} is bound twice");
            seen.push(key);
            assert_eq!(Tool::from_key(key), Some(tool), "the lookup must agree with the key");
        }
        assert_eq!(seen.len(), PANEL_TOOLS - 1, "every tool but one carries a letter");
    }

    #[test]
    fn s_is_songs_and_session_has_no_letter_of_its_own() {
        // The one collision, settled: Session's verb is Save, and that is on
        // Cmd+S — a key that needs no panel open at all.
        assert_eq!(Tool::from_key(egui::Key::S), Some(Tool::Song));
        assert_eq!(Tool::Session.key(), None);
    }

    #[test]
    fn the_rows_tooltip_names_the_key_that_opens_it() {
        // A shortcut nobody is told about is a shortcut nobody finds, and the
        // hover is where this app has already decided to say so.
        for tool in Tool::ALL {
            let hint = row_hint(tool);
            assert!(hint.starts_with(tool.hint()), "the hint must still say what the panel is for");
            match tool.key() {
                Some(key) => assert!(
                    hint.ends_with(&format!("press {}", key.name())),
                    "{} must announce its key: {hint}",
                    tool.title()
                ),
                None => assert!(hint.contains("Cmd+S"), "Session must point at the key it does have"),
            }
        }
    }

    #[test]
    fn a_letter_opens_its_panel_and_the_same_letter_closes_it() {
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();
        let mut bars = Sidebars::default();

        for tool in Tool::ALL {
            let Some(key) = tool.key() else { continue };
            let (took, left) = tap(&ctx, key, &mut bars, &session);
            assert!(took, "{key:?} is a rail key");
            assert!(left.is_empty(), "{key:?} and the character beside it both go: {left:?}");
            assert!(bars.tool_open, "{key:?} opens the panel");
            assert_eq!(bars.tool, tool, "{key:?} opens {}", tool.title());

            // Pressing it again closes it, exactly as clicking the open row does.
            tap(&ctx, key, &mut bars, &session);
            assert!(!bars.tool_open, "{key:?} on the open panel closes it");
            assert_eq!(bars.tool, tool, "and it is still the tool that would reopen");

            // And back open, so the next tool in the loop is a *switch* rather
            // than an open — which is the other half of `open_or_close`.
            tap(&ctx, key, &mut bars, &session);
            assert!(bars.tool_open);
        }
    }

    #[test]
    fn another_letter_switches_the_open_panel_rather_than_closing_it() {
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();
        let mut bars = Sidebars::default();

        tap(&ctx, egui::Key::G, &mut bars, &session);
        assert_eq!((bars.tool, bars.tool_open), (Tool::Generate, true));
        tap(&ctx, egui::Key::H, &mut bars, &session);
        assert_eq!((bars.tool, bars.tool_open), (Tool::Harmony, true), "a switch, not a toggle");
    }

    #[test]
    fn a_held_letter_is_one_tap_rather_than_a_panel_flapping_at_the_repeat_rate() {
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();
        let mut bars = Sidebars::default();

        // No release between the two, so egui marks the second a repeat.
        frame(&ctx, letter(egui::Key::E), &mut bars, &session);
        assert!(bars.tool_open);
        let (took, left) = frame(&ctx, letter(egui::Key::E), &mut bars, &session);
        assert!(bars.tool_open, "a leant-on E must not close what the first press opened");
        assert!(!took, "a repeat is not a tap, so nothing downstream is told the key moved a panel");
        assert!(left.is_empty(), "but it is still swallowed rather than typed into the window");
    }

    #[test]
    fn a_letter_no_tool_claims_is_left_where_it_was() {
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();
        let mut bars = Sidebars::default();

        let (took, left) = frame(&ctx, letter(egui::Key::Q), &mut bars, &session);
        assert!(!took);
        assert_eq!(left.len(), 2, "the key and its character both pass through: {left:?}");
        assert!(!bars.tool_open);
    }

    #[test]
    fn a_chord_on_a_rail_letter_is_not_a_rail_letter() {
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();

        // `Modifiers::NONE` as a `consume_key` pattern would match every one of
        // these — see `shortcuts` — which is why this read matches exactly.
        // Shift+C/Shift+V are the TRACKS clipboard and Cmd+S is the save.
        for modifiers in [
            egui::Modifiers::SHIFT,
            egui::Modifiers::ALT,
            egui::Modifiers::COMMAND,
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        ] {
            let mut bars = Sidebars::default();
            for key in [egui::Key::E, egui::Key::S, egui::Key::P] {
                let event = egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                };
                let (took, left) = frame(&ctx, vec![event], &mut bars, &session);
                assert!(!took, "{modifiers:?}+{key:?} is not the rail");
                assert_eq!(left.len(), 1, "and it is left for whoever does want it");
            }
            assert!(!bars.tool_open);
        }
    }

    #[test]
    fn a_focused_field_keeps_its_letters() {
        // With a TextEdit or a DragValue focused, a letter is a character being
        // typed. The same guard Cmd+Z and the spacebar carry.
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();
        let mut bars = Sidebars::default();
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("some-text-field")));

        let (took, left) = frame(&ctx, letter(egui::Key::E), &mut bars, &session);
        assert!(!took);
        assert_eq!(left.len(), 2, "the character has to reach the field: {left:?}");
        assert!(!bars.tool_open);
    }

    #[test]
    fn nothing_opens_while_a_dialog_is_waiting_for_an_answer() {
        // A write, sync or restore dialog is a question, and moving the panel
        // behind it is not an answer to it. The same guard the spacebar carries.
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();
        let mut bars = Sidebars::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::Modal::new(egui::Id::new("a-question")).show(ui.ctx(), |ui| {
                ui.label("Send this pattern to the box?");
            });
        });
        output.textures_delta.clear();

        let (took, left) = frame(&ctx, letter(egui::Key::E), &mut bars, &session);
        assert!(!took);
        assert_eq!(left.len(), 2, "the modal has the keyboard until it is answered");
        assert!(!bars.tool_open);
    }

    #[test]
    fn a_clicked_track_cell_does_not_disarm_the_rail() {
        // The standing exemption `typing_elsewhere` exists for: a TRACKS cell
        // holds focus — that is what arms its Delete — and picking a track must
        // not cost you the rail.
        let ctx = egui::Context::default();
        let session = digi_core::two_box_session();
        let mut bars = Sidebars::default();
        let cell = crate::ui::tracks::cell_id(session.devices[0].id, 0);
        ctx.memory_mut(|m| m.request_focus(cell));

        let (took, _) = frame(&ctx, letter(egui::Key::P), &mut bars, &session);
        assert!(took);
        assert_eq!((bars.tool, bars.tool_open), (Tool::Presets, true));
    }

    #[test]
    fn the_app_opens_on_setup_with_the_tools_closed() {
        let bars = Sidebars::default();
        assert!(bars.setup_open, "a box needs a port before anything can be heard");
        assert!(!bars.tool_open, "the roll gets the width until a tool is asked for");
    }

    #[test]
    fn the_active_row_never_takes_a_cyan_fill() {
        // The one rule this whole file exists to satisfy: filled cyan means "a
        // thing you can press", so the active tool — which is already pressed —
        // must mark itself with a border, never a fill.
        let (background, border) = rail_row_fill(true, false);
        assert_eq!(background, super::super::PANEL_BG_RAISED);
        assert_eq!(border, super::super::CYAN);
        assert_ne!(background, super::super::CYAN, "the active row's fill must not be cyan");
    }

    #[test]
    fn hover_only_paints_an_inactive_row() {
        // An active row's fill does not change on hover — it is already the
        // raised panel colour — and an inactive, unhovered row is left
        // transparent so the rail's own background shows through it.
        assert_eq!(rail_row_fill(false, false).0, Color32::TRANSPARENT);
        assert_eq!(rail_row_fill(false, true).0, super::super::INSET_BG_HOVER);
        assert_eq!(rail_row_fill(true, true).0, super::super::PANEL_BG_RAISED);
        assert_eq!(rail_row_fill(false, false).1, Color32::TRANSPARENT, "no border when inactive");
    }

    #[test]
    fn only_the_active_label_brightens() {
        assert_eq!(rail_row_text_colour(true), super::super::TEXT_BRIGHT);
        assert_eq!(rail_row_text_colour(false), super::super::TEXT_MUTED);
        assert_ne!(rail_row_text_colour(true), rail_row_text_colour(false));
    }
}
