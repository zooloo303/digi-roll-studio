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

use eframe::egui::{self, Color32, Ui};

/// The left panel's tools, in rail order. Each one is a slot in the workflow;
/// what is actually built behind them is in [`super::tools`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Edit,
    Harmony,
    Generate,
    Song,
    Session,
}

impl Tool {
    pub const ALL: [Self; 5] =
        [Self::Edit, Self::Harmony, Self::Generate, Self::Song, Self::Session];

    pub fn title(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Harmony => "Harmony",
            Self::Generate => "Generate",
            Self::Song => "Song",
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
            Self::Session => "Save and open the session file",
        }
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
                if rail_row(ui, active, tool.title(), tool.hint()) {
                    if bars.tool == tool {
                        // Clicking the open tool closes the panel, as digi-roll's
                        // rail did — it is the fastest way to get the roll back.
                        bars.tool_open = !bars.tool_open;
                    } else {
                        bars.tool = tool;
                        bars.tool_open = true;
                    }
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
    /// How many rail rows open a panel — Edit, Harmony, Generate, Song, Session.
    /// One number rather than a literal in each test, so the next tool added
    /// moves it once and both assertions below keep meaning what they say.
    const PANEL_TOOLS: usize = 5;

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
