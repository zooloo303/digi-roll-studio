// The UI layer: how the window is laid out, the workflow that layout is shaped
// around, the handful of things every panel shares — and one trap that cuts
// across all of them.
//
// ## The layout
//
// ```text
// ┌──────────────────────────────────────────────────────────────────────┐
// │ transport — PLAY/STOP, tempo, position, CLOCK/FILL, SCENE, SETUP    │
// ├──────┬───────────┬───────────────────────────────┬──────────────────┤
// │ rail │ tool      │ TRACKS                        │ Setup            │
// │      │ panel     │                               │  BOXES           │
// │ Edit │           │                               │  ports, CLK      │
// │ Harm │ (closed   │          piano roll           │                  │
// │ Gen  │  until a  │                               │  MIDI PORTS      │
// │ Sess │  rail     ├───────────────────────────────┤  in/out, Identify│
// │      │  item is  │ trig lane — PROB/COND/FILL    │                  │
// │      │  clicked) │                               │                  │
// └──────┴───────────┴───────────────────────────────┴──────────────────┘
// ```
//
// Both side panels collapse — `Panel::show_collapsible` — and both leave a way
// back: the rail is always on screen, and the transport carries the `SETUP`
// toggle. The rail is the one piece of chrome that cannot be hidden, because
// hiding the thing that reopens the panels is how a UI loses a feature.
//
// ## The workflow it is shaped around
//
// Left to right is roughly the order of work, and the two panes above the roll
// are side by side because the first two steps are one sitting:
//
// 1. **Setup**, on the right: which boxes are in the session, which ports they
//    are on, which of them take our clock. Done once per desk.
// 2. **Patterns**, top left of the workspace: pick one of the 32 tracks, set its
//    length and scale, draw it in the roll.
// 3. **Scenes**, from the transport bar's SCENE zone: name the pattern each box
//    plays, together. Built out of patterns, so it used to sit in a pane beside
//    them; the 2026-08-19 v2 pass moved it into the transport so the tracks and
//    the roll could share one left and one right edge — see `transport.rs`.
// 4. **Play**, from the transport at the top, which stays put whatever else is
//    open.
//
// The left rail is the editing tools that act on what step 2 has selected. All
// four of its panels are real now — Edit (Phase 9), Harmony (Phase 11),
// Generate (Phase 7) and Session (Phase 8) — the last of a run where each one
// shipped as a labelled empty slot first and a panel afterward, on the
// argument that the shape of the workflow was worth deciding before it was
// built: an empty labelled slot is a decision, where a missing one is a
// question.
//
// ## Colour carries meaning, so panel furniture does not use it
//
// [`ACCENT`] means *the engine is about to do this, or is doing it*: the queued
// scene, the playhead, a soloed track. [`CAUTION`] means *this worked but not
// the way you wanted*. Titles and captions are weight and size only, never
// colour, so that the amber in the window is always worth looking at.
//
// ## A glyph the bundled fonts do not have is drawn as a missing-glyph box
//
// And nothing catches it. The scene bar shipped that way. *Both* of its marks
// were characters egui does not bundle — `●` (U+25CF) for the scene that is
// sounding and `▸` (U+25B8) for the one queued behind it — so the entire state
// the bar existed to show was two tofu boxes, while all 299 tests passed. It was
// found by running the app and looking at the window, which until then nobody
// had done.
//
// The non-ASCII characters the UI draws, all in the proportional family. Every
// row was checked by drawing it and reading it off the screen, not by reasoning
// about which font ought to have what:
//
// | glyph | where |
// |---|---|
// | `·` U+00B7 | separator in the device strip, track strip and transport; a live trig-lane step with nothing set, and the lane picker's title |
// | `×` U+00D7 | the close button on both side panels, and the remove-box control in the device strip |
// | `»` U+00BB | scene bar: this scene is queued |
// | `—` U+2014 | the em dash in "— none —", an unbound port, the lane picker's no-condition button, every write/restore result line, and the Session panel's close guard |
// | `…` U+2026 | ports panel: waiting for the box; the status line of a fetch, a write or a restore; the Session panel's `Save As…` and `Open…`, and the Backups list's `Export…` |
// | `−` U+2212 | scene bar: remove this scene |
// | `■` U+25A0 | transport: stop |
// | `▶` U+25B6 | transport: play; scene bar: this scene is sounding |
// | `↻` U+21BB | reroll: a PARTS card's row, the Generate panel's progression, and its seed |
// | `▲` U+25B2 | the conflict marker on a PARTS card |
// | `“ ”` U+201C/D | the kit name in the write confirm dialog, a backup's filename in the send group's log line, and every row of the Backups list (`StashEntry::summary`, which the restore dialog quotes too) |
//
// **Known missing, all six found by drawing them**: `●` U+25CF, `▸` U+25B8 (the
// scene bar's original pair), `▾` U+25BE and `▼` U+25BC — tried in that order
// for the patterns pane's fold arrows on 2026-08-18 — `✓` U+2713, which shipped
// in the write result line and was read off Neil's screenshot of the first
// hardware write, and `→` U+2192, drawn at last on 2026-08-29 in the Presets
// panel's load line and read back as `ACIDD □ T1`.
//
// **`→` was on this page as a suspicion for eleven days before anything drew
// it**, which is the interesting part: the paragraph below already said it "sits
// near the known-missing marks" and that a button had been reworded to avoid it.
// A new line in a new panel used it anyway, and no test noticed, and the screen
// did — the same way every other row here was settled. A table of characters to
// avoid is only as good as the habit of drawing the thing and looking at it. `▶` renders and its own mirror `▼` does not, which is the whole
// argument against reasoning about this: the pair that looks like a pair is not
// one. `⚠` U+26A0 is not on this list because it was never drawn — it was
// withdrawn on suspicion, which is the cheaper move once a neighbour has failed.
//
// **`✓` U+2713 was tofu, and it was found the way all four of these are found:
// by looking at the screen.** It opened `write_result_message`'s success line,
// which reached a window for the first time on the first hardware write
// (2026-08-18) — so `□ Wrote 3 notes to A01 T1 — verified byte-identical` is what
// the app said at the best moment it has ever had. `⚠` (U+26A0) opened the
// verify-failure line, was never seen, and was removed as the same class rather
// than left to be discovered on the one line nobody wants to be reading closely.
// Both are gone from `protocol` — **not stripped in the panel**, because the
// window and the terminal have to keep saying the same thing — and the emphasis
// is each surface's own: colour and a modal here, words there.
//
// **`“` and `”` (U+201C/U+201D) render**, confirmed on the same screen: they quote
// a kit name in the confirm dialog and a backup's filename in the row's log line.
// They sit in General Punctuation beside `—` and `…`, which is where the guess
// was, and this is the one time the guess held. The Backups list leans on that
// confirmation heavily — `StashEntry::summary` quotes a kit name in every row —
// but it introduces no mark that has not been read off a screen.
//
// The lesson, since this is the fourth instance: **a mark inherited from a port
// is still a mark you are shipping.** `✓` survived review because it came with
// `js/main.js`'s wording and had "always worked" — in a browser, which has fonts
// this app does not. The `→` in the same handler's button label *was* caught
// beforehand, on the reasoning that U+2192 sits near the known-missing marks, and
// the button spells `Send to` because of it. The two were one commit apart, and
// only the one that looked like *our* text got checked.
//
// **So a fold arrow was drawn rather than typed** — a `convex_polygon` in what
// was `tracks::paint_fold_arrow`, which had no font behind it and therefore
// could not come out as a box. That function is gone as of the 2026-08-19
// track-lanes redesign (the per-box fold it drew for no longer exists — see
// `tracks.rs`'s doc comment), but the lesson outlives the code: any mark that
// can be a shape should be one, and this table is for the ones that cannot. The
// Setup panel's own disclosure rows and IN/OUT header arrows, from the same
// redesign, keep the habit alive — see [`paint_fold_arrow`] and
// [`paint_direction_arrow`] below.
//
// Everything drawn with `ui.monospace` — only the transport's bar.beat.step
// readout — is ASCII, and stays that way: the mono family is Hack, which carries
// far less than Ubuntu-Light does. The rail is words rather than digi-roll's
// icons for exactly this reason: `✎ ♬ ⚄ ▤ ⇄` are five more chances to ship a row
// of tofu, and an icon font is a dependency, not a label.
//
// **There is no test for this, and the obvious one does not work.** `epaint`
// exposes `Fonts::has_glyph`, but on a `Fonts` built outside a running eframe app
// it under-reports badly: it answers "no" for `▶` and for *every* character in the
// monospace family including `A`, all of which visibly render. Its negatives are
// therefore worthless, which is the direction a test would need. So this list is
// prose, and the check is to run the app and look — the same way the `●` bug
// surfaced. Anything added here deserves that look.

use eframe::egui::{self, Color32, Ui};

pub mod autoconnect;
pub mod devices;
pub mod edit;
pub mod generate;
pub mod harmony;
pub mod pianoroll;
pub mod plocklane;
pub mod presets;
pub mod ports;
pub mod rail;
pub mod restore;
pub mod scenes;
pub mod session;
pub mod setup;
pub mod song;
pub mod tools;
pub mod tracks;
pub mod transfer;
pub mod sync;
pub mod transport;
pub mod triglane;
pub mod workspace;
pub mod write;

/// The engine is about to do this, or is doing it: the queued scene, the
/// playhead, a soloed track.
pub const ACCENT: Color32 = Color32::from_rgb(255, 210, 80);

/// This worked, but not the way you wanted: an unknown dump protocol, a box that
/// could not be placed.
pub const CAUTION: Color32 = Color32::from_rgb(200, 160, 60);

/// A group heading inside a panel or pane.
const CAPTION: Color32 = Color32::from_rgb(140, 146, 158);

/// The palette from `design_handoff_digi_roll_ui/README.md`'s "Design Tokens"
/// table, shared by the 2026-08-19 Setup panel (§1a) and track-lanes (§1b)
/// redesigns — built in parallel worktrees from the same table, which is why
/// the names and hexes below agree between the two: keeping them as one block
/// is what stops the two panels' cyan, green and text scales drifting apart.
///
/// Named for what each token *is* rather than what it means, unlike
/// [`ACCENT`]/[`CAUTION`]: those two already carry meaning elsewhere in this
/// app — the engine's own colour and "worked, but not as asked" — and
/// repointing them to a panel-chrome hex would quietly change what they say
/// everywhere else they are read. These are simply the fixed furniture of two
/// widgets.
///
/// The two with an alpha component in the mock (`#2f9fd014`, `#3ddc9760`) are
/// built with [`Color32::from_rgba_unmultiplied_const`] rather than
/// [`Color32::from_rgb`]: the README's alpha hexes are straight sRGBA, the same
/// as a CSS hex-with-alpha or a colour picker, and the *premultiplied*
/// constructor would paint them too bright since it stores its arguments as-is
/// rather than scaling by alpha first.
pub const CYAN: Color32 = Color32::from_rgb(0x2f, 0x9f, 0xd0);
pub const CYAN_FILL: Color32 = Color32::from_rgb(0x1f, 0x6d, 0x90);
pub const CYAN_TEXT: Color32 = Color32::from_rgb(0xcf, 0xe9, 0xf5);
pub const CYAN_WASH: Color32 = Color32::from_rgba_unmultiplied_const(0x2f, 0x9f, 0xd0, 0x14);
pub const CYAN_INK: Color32 = Color32::from_rgb(0x0d, 0x14, 0x17);

pub const TRIG_GREEN: Color32 = Color32::from_rgb(0x3d, 0xdc, 0x97);
pub const TRIG_GREEN_GLOW: Color32 = Color32::from_rgba_unmultiplied_const(0x3d, 0xdc, 0x97, 0x60);
/// PLAY's hover fill — the transport bar's own state, since nothing else in
/// the app has a trig-green button to hover.
pub const TRIG_GREEN_HOVER: Color32 = Color32::from_rgb(0x5f, 0xe8, 0xac);

pub const WARN_AMBER: Color32 = Color32::from_rgb(0xd9, 0xa5, 0x3c);
pub const WARN_AMBER_BORDER: Color32 = Color32::from_rgb(0x8a, 0x6d, 0x1f);
pub const WARN_AMBER_FILL: Color32 = Color32::from_rgb(0x4a, 0x3a, 0x15);
pub const WARN_AMBER_FILL_HOVER: Color32 = Color32::from_rgb(0x5c, 0x48, 0x19);
pub const WARN_AMBER_TEXT: Color32 = Color32::from_rgb(0xf0, 0xdc, 0xb4);
pub const WARN_AMBER_BODY: Color32 = Color32::from_rgb(0x9a, 0x86, 0x57);
pub const WARN_AMBER_INK: Color32 = Color32::from_rgb(0x1a, 0x14, 0x08);

pub const PANEL_BG: Color32 = Color32::from_rgb(0x1e, 0x22, 0x26);
pub const PANEL_BG_RAISED: Color32 = Color32::from_rgb(0x23, 0x28, 0x2d);
pub const INSET_BG: Color32 = Color32::from_rgb(0x1a, 0x1e, 0x21);
pub const INSET_BG_HOVER: Color32 = Color32::from_rgb(0x22, 0x27, 0x2b);
pub const PANEL_BORDER: Color32 = Color32::from_rgb(0x2c, 0x32, 0x37);
/// An outline button's border on hover — STOP/CONTINUE and FILL at rest both
/// lighten to this rather than to a filled background.
pub const BORDER_HOVER: Color32 = Color32::from_rgb(0x3c, 0x46, 0x50);

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xc8, 0xcc, 0xd0);
pub const TEXT_BRIGHT: Color32 = Color32::from_rgb(0xdf, 0xe4, 0xe8);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xa8, 0xb0, 0xb8);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x8c, 0x94, 0x9c);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x6d, 0x75, 0x80);
pub const TEXT_DIMMER: Color32 = Color32::from_rgb(0x5c, 0x64, 0x6c);
pub const TEXT_DIMMEST: Color32 = Color32::from_rgb(0x4a, 0x51, 0x58);
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x3c, 0x43, 0x48);

/// A track cell that carries data — `#212a30`. Not in the README's shared
/// token list above; this fill is specific to the track lanes' own cell
/// states.
const CELL_BG_DATA: Color32 = Color32::from_rgb(0x21, 0x2a, 0x30);
/// An empty track cell's border — `#252a2e`.
const CELL_BORDER_SUBTLE: Color32 = Color32::from_rgb(0x25, 0x2a, 0x2e);
/// A data-carrying, unselected track cell's border — `#33414a`.
const CELL_BORDER_RAISED: Color32 = Color32::from_rgb(0x33, 0x41, 0x4a);

/// A group heading: small, dim, and shouted, so it reads as furniture rather
/// than as a value. Pass it already upper-cased.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().strong().color(CAPTION));
}

/// A side panel's title row, with the `×` that closes it.
///
/// Returns whether the `×` was clicked rather than taking the flag: while this
/// is drawn, `Panel::show_collapsible` is already holding a `&mut` to the very
/// bool that would have to be cleared, so the caller applies the answer once the
/// panel has finished.
///
/// Setup, Edit, Harmony, Generate and Session all still call this plain form.
/// [`panel_title_bar`] below is the v2 replacement — it adds the panel's
/// context string and the `?` reference toggle from
/// `design_handoff_digi_roll_ui_v2/README.md`'s "Panel title bar" rule — but
/// swapping five call sites over to it is each panel's own v2 conversion, not
/// this refactor's job, so both live here side by side for now.
pub fn panel_header(ui: &mut Ui, title: &str) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close = ui
                .small_button("×")
                .on_hover_text("Close this panel")
                .clicked();
        });
    });
    ui.separator();
    close
}

/// The v2 side-panel title row: name, then the panel's own context string
/// ("A01 · A_303_INNIT" for Edit), then a flexible spacer, then the `?`
/// reference toggle, then the `×` that closes the panel — per 2b rule 6.
/// Putting the context here is what lets a panel's body drop the line that
/// used to restate it.
///
/// `reference_visible` is toggled in place rather than reported back, the way
/// [`disclosure_row`]'s `open` is: nothing else in the caller needs to react to
/// the click within the same frame, so there is no reason to make every caller
/// re-apply an answer this function can just apply itself.
///
/// Returns whether the `×` was clicked, matching [`panel_header`]'s contract.
pub fn panel_title_bar(ui: &mut Ui, title: &str, context: &str, reference_visible: &mut bool) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(egui::RichText::new(title).strong());
        ui.label(egui::RichText::new(context).monospace().size(10.0).color(TEXT_DIMMER));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close = ui.small_button("×").on_hover_text("Close this panel").clicked();
            let hover = if *reference_visible { "Hide this panel's reference notes" } else { "Show this panel's reference notes" };
            if ui.small_button("?").on_hover_text(hover).clicked() {
                *reference_visible = !*reference_visible;
            }
        });
    });
    ui.separator();
    close
}

/// The fold triangle: right when folded, down when open.
///
/// Drawn rather than typed, for the reason recorded at the top of this file —
/// a polygon has no font behind it, so it cannot come out as a missing-glyph
/// box. `tracks.rs` used to have its own copy for the patterns pane's per-box
/// fold; that fold is gone as of the track-lanes redesign, so this is now the
/// only one, used by the Setup panel's disclosure rows.
pub fn paint_fold_arrow(painter: &egui::Painter, rect: egui::Rect, folded: bool, colour: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.34;
    let points = if folded {
        vec![
            egui::pos2(c.x - r * 0.7, c.y - r),
            egui::pos2(c.x - r * 0.7, c.y + r),
            egui::pos2(c.x + r, c.y),
        ]
    } else {
        vec![
            egui::pos2(c.x - r, c.y - r * 0.7),
            egui::pos2(c.x + r, c.y - r * 0.7),
            egui::pos2(c.x, c.y + r),
        ]
    };
    painter.add(egui::Shape::convex_polygon(points, colour, egui::Stroke::NONE));
}

/// The `←`/`→` direction glyph on the Setup panel's IN/OUT headers, drawn rather
/// than typed.
///
/// This file's glyph table lists U+2190/U+2192 as *unconfirmed* — seen once, in
/// a tooltip, and never risked in a place as prominent as a coloured 13px
/// header mark read on every open of this panel. A simple arrowhead-on-a-shaft
/// has no font behind it, so it is drawn the same way the fold triangle is.
pub fn paint_direction_arrow(
    painter: &egui::Painter,
    rect: egui::Rect,
    pointing_right: bool,
    colour: Color32,
) {
    let c = rect.center();
    let half_w = rect.width() * 0.36;
    let half_h = rect.height() * 0.30;
    let stroke = egui::Stroke::new((rect.height() * 0.16).max(1.0), colour);
    let (tail_x, head_x) = if pointing_right {
        (c.x - half_w, c.x + half_w)
    } else {
        (c.x + half_w, c.x - half_w)
    };
    painter.line_segment(
        [egui::pos2(tail_x, c.y), egui::pos2(head_x, c.y)],
        stroke,
    );
    let back = if pointing_right { head_x - half_w * 0.8 } else { head_x + half_w * 0.8 };
    let points = vec![
        egui::pos2(head_x, c.y),
        egui::pos2(back, c.y - half_h),
        egui::pos2(back, c.y + half_h),
    ];
    painter.add(egui::Shape::convex_polygon(points, colour, egui::Stroke::NONE));
}

/// The same arrow, pointing up or down — the SONG panel's row-order buttons.
///
/// A separate function rather than an axis flag on [`paint_direction_arrow`]: its
/// callers all pass a `bool` already, and a second one meaning "but sideways"
/// reads worse at every call site than two named functions do.
///
/// Painted for the reason the whole family is: `▾` U+25BE and `▼` U+25BC were
/// both tofu on this screen, tried in that order, and a triangle has no font
/// behind it.
pub fn paint_vertical_arrow(
    painter: &egui::Painter,
    rect: egui::Rect,
    pointing_down: bool,
    colour: Color32,
) {
    let c = rect.center();
    let half_h = rect.height() * 0.36;
    let half_w = rect.width() * 0.30;
    let stroke = egui::Stroke::new((rect.width() * 0.16).max(1.0), colour);
    let (tail_y, head_y) = if pointing_down {
        (c.y - half_h, c.y + half_h)
    } else {
        (c.y + half_h, c.y - half_h)
    };
    painter.line_segment([egui::pos2(c.x, tail_y), egui::pos2(c.x, head_y)], stroke);
    let back = if pointing_down { head_y - half_h * 0.8 } else { head_y + half_h * 0.8 };
    let points = vec![
        egui::pos2(c.x, head_y),
        egui::pos2(c.x - half_w, back),
        egui::pos2(c.x + half_w, back),
    ];
    painter.add(egui::Shape::convex_polygon(points, colour, egui::Stroke::NONE));
}

/// A button painted with an explicit fill, text colour and border per
/// interaction state, for the FETCH/SEND/SYNC buttons of the Setup panel's
/// transfer container — the design spec gives each a specific colour and a
/// specific hover colour, neither of which is the theme's default grey.
///
/// `egui::Button::fill` cannot do this alone: once set, it overrides the frame's
/// fill for *every* interaction state alike (see `egui`'s own `button_style`,
/// which only reads `self.fill` after computing the per-state style), so a
/// button built with `.fill()` looks the same hovered as it does at rest. Scoping
/// the style's `inactive`/`hovered`/`active` [`egui::style::WidgetVisuals`]
/// instead lets each state carry its own colour, which is what
/// `Style::button_style` actually reads.
#[allow(clippy::too_many_arguments)]
pub fn colored_button(
    ui: &mut Ui,
    text: impl Into<egui::WidgetText>,
    fill: Color32,
    text_colour: Color32,
    border: Color32,
    hover_fill: Color32,
    hover_text: Color32,
) -> egui::Response {
    ui.scope(|ui| {
        let widgets = &mut ui.style_mut().visuals.widgets;
        for state in [&mut widgets.inactive, &mut widgets.active] {
            state.weak_bg_fill = fill;
            state.bg_fill = fill;
            state.fg_stroke = egui::Stroke::new(1.0, text_colour);
            state.bg_stroke = egui::Stroke::new(1.0, border);
        }
        widgets.hovered.weak_bg_fill = hover_fill;
        widgets.hovered.bg_fill = hover_fill;
        widgets.hovered.fg_stroke = egui::Stroke::new(1.0, hover_text);
        widgets.hovered.bg_stroke = egui::Stroke::new(1.0, hover_fill);
        ui.add(egui::Button::new(text))
    })
    .inner
}

/// A section label with a rule filling the rest of the row, and an optional
/// right-aligned caption after it — "DATA TRANSFER" over the transfer
/// container today, plus every section header the v2 side panels need per 2b
/// rule 4 ("Section headers"), which is the same eyebrow-plus-rule shape with
/// a caption slot added at the end ("3 selected", "nothing selected — sets new
/// notes").
///
/// This supersedes the old `section_rule(ui, text)`; there was exactly one
/// caller (Setup's "DATA TRANSFER" heading), so it was extended in place with
/// a `caption` parameter rather than kept alongside a near-duplicate — pass
/// `None` to get the old behaviour back.
///
/// **The rule is painted into a rect this function measures, not an
/// `egui::Separator`, and that is not a style preference.** A horizontal
/// `Separator` is *greedy*: it asks for every pixel of available width, and in
/// asking it pushes the parent `Ui`'s width out to whatever it was offered.
/// Since a section header is the first thing in a section, every plain label
/// after it then saw an unbounded width, and a label with unbounded width
/// never wraps — so the panels' prose ran off the right edge and was clipped.
/// (`destructive_note`'s copy escaped it only because `ui.indent` happens to
/// re-bound the width.) Measuring the gap and painting a line into it asks for
/// nothing, so the `Ui` keeps the width the panel gave it and the text below
/// wraps into it.
pub fn section_header(ui: &mut Ui, eyebrow: &str, caption: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(eyebrow).size(10.0).color(TEXT_DIM));

        // Reserve the caption first so the rule can be sized against what is
        // actually left, rather than the rule taking it all and the caption
        // wrapping under.
        let caption_w = caption
            .map(|c| {
                ui.painter()
                    .layout_no_wrap(c.to_owned(), egui::FontId::proportional(10.0), TEXT_DIMMEST)
                    .size()
                    .x
            })
            .unwrap_or(0.0);
        // Only pay for the gap when there is a caption to put after it;
        // subtracting it unconditionally left the rule short of the right
        // margin on every header that has none, which is most of them.
        let gap = if caption.is_some() { ui.spacing().item_spacing.x } else { 0.0 };
        let rule_w = (ui.available_width() - caption_w - gap).max(0.0);
        if rule_w > 0.0 {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(rule_w, 1.0), egui::Sense::hover());
            ui.painter().hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, PANEL_BORDER));
        }

        if let Some(caption) = caption {
            ui.label(egui::RichText::new(caption).size(10.0).color(TEXT_DIMMEST));
        }
    });
}

/// A disclosure row: a fold triangle, a title, a flexible spacer, and a
/// right-aligned hint — "BACKUPS · put a slot back" today, and per 2b rule 5
/// the same shape every v2 side panel's KEYS & GESTURES row and Edit's HISTORY
/// row need too. Promoted from Setup's own private `fold_row`, which was
/// already exactly this; nothing about the behaviour changed in the move.
///
/// Its own flat row rather than `egui::CollapsingHeader`: the design spec's
/// rows are a plain bordered strip with a hover fill, not the indented tree
/// look `CollapsingHeader` draws. The background is painted through a
/// placeholder shape index — the same trick `egui`'s own `ComboBox` uses — so
/// the fill can depend on this frame's hover state rather than last frame's.
pub fn disclosure_row(ui: &mut Ui, open: &mut bool, title: &str, hint: &str, body: impl FnOnce(&mut Ui)) {
    let placeholder = ui.painter().add(egui::Shape::Noop);
    let response = ui
        .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    let (icon, _) =
                        ui.allocate_exact_size(egui::Vec2::splat(9.0), egui::Sense::hover());
                    paint_fold_arrow(ui.painter(), icon, !*open, TEXT_DIMMER);
                    ui.label(egui::RichText::new(title).size(10.0).color(TEXT_MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(hint).size(10.0).color(TEXT_DIMMEST));
                    });
                });
            });
        })
        .response;

    let fill = if response.hovered() { INSET_BG_HOVER } else { INSET_BG };
    ui.painter().set(placeholder, egui::Shape::rect_filled(response.rect, 0.0, fill));
    ui.painter().rect_stroke(
        response.rect,
        0.0,
        egui::Stroke::new(1.0, PANEL_BORDER),
        egui::StrokeKind::Middle,
    );

    if response.clicked() {
        *open = !*open;
    }
    ui.add_space(1.0);
    if *open {
        egui::Frame::new()
            .fill(INSET_BG)
            .stroke(egui::Stroke::new(1.0, PANEL_BORDER))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, body);
        ui.add_space(1.0);
    }
}

/// The destructive note: an amber rule on the left, an eyebrow and a body
/// line — verbatim text, not italic (the design spec calls the original
/// italic out by name as a legibility loss at this size). Per 2b rule 2, this
/// is one of exactly three permitted prose forms and the only one reserved for
/// copy that changes or destroys data. Promoted from Setup's own private
/// `amber_warning`, which guards its SEND buttons; Edit's MIDI-file IMPORT and
/// Generate's part-card conflicts need the same shape.
///
/// The indent id is salted with the eyebrow so two notes on the same panel
/// (Edit's IMPORT warning and a future second one, say) don't collide.
pub fn destructive_note(ui: &mut Ui, eyebrow: &str, body: &str) {
    let response = ui
        .scope(|ui| {
            ui.indent(egui::Id::new(("destructive-note", eyebrow)), |ui| {
                ui.label(egui::RichText::new(eyebrow).size(9.5).color(WARN_AMBER));
                ui.add_space(3.0);
                ui.label(egui::RichText::new(body).size(10.5).color(WARN_AMBER_BODY));
            });
        })
        .response;
    ui.painter().line_segment(
        [response.rect.left_top(), response.rect.left_bottom()],
        egui::Stroke::new(2.0, WARN_AMBER_BORDER),
    );
}

/// [`destructive_note`]'s one-line form: the eyebrow stays on the panel and the
/// body moves into an ⓘ's tooltip.
///
/// The Setup panel's DATA TRANSFER container has both halves of a round trip
/// permanently visible in a 300px column, and the OUT warning's three wrapped
/// lines were the largest block in it that says the same thing on every open.
/// Behind the icon the sentence is one hover away and still a sentence *before*
/// the SEND buttons, which is the only property the amber rule was ever
/// protecting. The eyebrow and the rule stay on screen: what is folded away
/// here is the explanation, never the fact that this block writes.
pub fn destructive_note_tip(ui: &mut Ui, eyebrow: &str, body: &str) {
    let response = ui
        .scope(|ui| {
            ui.indent(egui::Id::new(("destructive-note", eyebrow)), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.label(egui::RichText::new(eyebrow).size(9.5).color(WARN_AMBER));
                    info_icon(ui, WARN_AMBER, body);
                });
            });
        })
        .response;
    ui.painter().line_segment(
        [response.rect.left_top(), response.rect.left_bottom()],
        egui::Stroke::new(2.0, WARN_AMBER_BORDER),
    );
}

/// A hoverable ⓘ that carries `tip`, sized to sit beside a 9.5px eyebrow.
///
/// **Drawn rather than typed**, for the reason at the top of this file: `ⓘ`
/// U+24D8 is not on the confirmed-glyph table, it is nowhere near the marks that
/// *are*, and `⚠` was withdrawn on that same suspicion. A circle with a dot over
/// a stem in it has no font behind it, so it cannot come out as tofu — and this
/// one cannot be checked by looking at the window either, because until it is
/// hovered there is nothing on screen but the mark itself.
pub fn info_icon(ui: &mut Ui, colour: Color32, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    paint_info_icon(ui.painter(), rect, colour);
    response.on_hover_text(tip)
}

/// The ⓘ mark itself: a ring, a dot, and a stem. See [`info_icon`] for why it is
/// three shapes rather than one character.
pub fn paint_info_icon(painter: &egui::Painter, rect: egui::Rect, colour: Color32) {
    let c = rect.center();
    // Half a pixel in from the edge so the ring's own width stays inside the
    // rect that was allocated for it, rather than bleeding onto the eyebrow.
    let r = rect.width().min(rect.height()) * 0.5 - 0.5;
    let w = (r * 0.22).max(1.0);
    painter.circle_stroke(c, r, egui::Stroke::new(w, colour));
    painter.circle_filled(egui::pos2(c.x, c.y - r * 0.42), w * 0.7, colour);
    painter.line_segment(
        [egui::pos2(c.x, c.y - r * 0.08), egui::pos2(c.x, c.y + r * 0.48)],
        egui::Stroke::new(w, colour),
    );
}

/// A consequence line: dim, sits directly under the control it qualifies —
/// "Writes into the tracks in this session. Nothing reaches a box until you
/// SEND in Setup." Per 2b rule 2, the second of the three permitted prose
/// forms; unlike [`destructive_note`] it carries no rule and no warning
/// colour, because it is describing an ordinary consequence rather than a
/// destructive one. New in this pass — nothing needed it before the side
/// panels did.
pub fn consequence_line(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(10.5).line_height(Some(15.5)).color(TEXT_DIMMER));
}

/// The one global style change this app makes, and it is here because of a bug
/// that looked like four separate ones.
///
/// egui's `Style::interaction::selectable_labels` defaults to **true**, which
/// gives every plain `ui.label` `Sense::click_and_drag()` so its text can be
/// selected with the mouse — and a label that senses clicks *takes* them. Every
/// custom-painted clickable row in this UI is a `scope_builder(..sense(click))`
/// with labels inside it ([`disclosure_row`] here, `rail::rail_row`,
/// `generate::destination_chip`), so on all of them the only part that
/// responded to a click was the padding *between* the words: clicking the words
/// themselves showed an I-beam and did nothing. The rail is where it was
/// noticed — EDIT / HARMONY / GENERATE / SESSION are almost entirely text, so
/// almost none of the row worked.
///
/// Turning it off globally rather than per-label is deliberate: nothing in this
/// app is text a user wants to select and copy — the readouts are one-glance
/// values, and everything genuinely editable is a `TextEdit`, which keeps its
/// own selection either way. A per-call-site `.selectable(false)` would have to
/// be remembered by every row added after this one, which is the same trap
/// dressed as a fix.
/// `all_styles_mut` rather than the dark style alone: egui 0.36 keeps a `Style`
/// per theme and follows the system's setting, and a click that works only
/// while the desk is in dark mode is the kind of thing nobody finds until it
/// matters.
pub fn install_style(ctx: &egui::Context) {
    // **This palette is dark-only, so the theme is pinned rather than followed.**
    // Every colour above is a fixed hex from the v2 handoff — there is no light
    // set of them and no token that varies — but egui's own default is to follow
    // the *system* theme, and it was doing so unopposed. On a Mac in light mode
    // the app came up with every surface it does not paint itself at egui's
    // `Visuals::light()` panel fill: `#f8f8f8`, measured, over 15% of the window.
    //
    // The worst of it was the Setup panel, which is the one panel open on launch
    // and the one that overwrites devices: its background went white while its
    // labels stayed `TEXT_MUTED` and `TEXT_SECONDARY`, so the panel carrying
    // OVERWRITES THE DEVICE was light grey on white. The transport, rail, tracks
    // and roll all survived because each paints its own background — which is
    // exactly why this went unnoticed until someone ran it in light mode.
    //
    // `ThemePreference::Dark`, not `Visuals::dark()`: the preference is what
    // egui consults on a system theme change, so setting it once here is what
    // makes switching the Mac to light mode mid-session a no-op. Setting the
    // visuals alone would be corrected back the next time the OS said otherwise.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        style.interaction.selectable_labels = false;
        // Pinning the theme stops the white, but it lands on egui's *stock* dark
        // grey. These two are the surfaces that showed through above, so they
        // are pointed at the handoff's own panel colour: the seams between
        // panels and the ground under a `CentralPanel` are furniture, and
        // furniture in this UI is `PANEL_BG`.
        style.visuals.panel_fill = PANEL_BG;
        style.visuals.window_fill = PANEL_BG;
    });
}

/// Whether a click at some position dismisses an open [`working_popup`], given
/// whether it landed inside the popup and which layer order it landed on.
///
/// Pure, and separate from [`working_popup`], because it is the whole judgement
/// call in that function and a `Ui` cannot be conjured in a unit test. A click
/// on a `Foreground` layer we are not inside is a click on *another popup* —
/// which, for a working popup, means one of its own combo-box dropdowns, since
/// those are drawn in their own `Area` above ours — and must not dismiss us.
/// A click on a lower layer is a click on the app behind the popup, and does.
fn click_dismisses_working_popup(inside: bool, landed_on: Option<egui::Order>) -> bool {
    if inside {
        return false;
    }
    match landed_on {
        Some(order) => order < egui::Order::Foreground,
        None => true,
    }
}

/// A popup that is a **working surface**: a compact chip expands into the real
/// controls, and those controls include pickers. The scene pill in the
/// transport bar and the destination chip on a Generate part card are the two.
///
/// **Why this exists rather than `egui::Popup` directly.** egui's memory holds
/// *one* open popup per viewport — `Memory::open_popup` inserts, it does not
/// push — so a popup whose open state lives in that memory is closed the
/// instant anything inside it opens a popup of its own. Every `ComboBox` does
/// exactly that. Both of these surfaces shipped that way on 2026-08-19 and both
/// had the same symptom from the user's side: click the scene pill, reach for a
/// pattern, and the whole scene box vanishes; click a part's destination chip
/// and no box picker ever appears. Neither is a layout or a hit-test problem,
/// which is why staring at the drawing code found nothing.
///
/// So the open flag lives in `ctx.data` under this widget's own id instead,
/// leaving egui's single popup slot free for the dropdowns *inside* — which is
/// the only place it is contended.
///
/// Close behaviour is then ours too, since egui's own would have to be told
/// which foreign `Area` is one of our children:
/// [`PopupCloseBehavior::IgnoreClicks`] so no click inside can dismiss a
/// half-finished edit, plus [`click_dismisses_working_popup`] for clicks
/// outside. Escape still closes it — egui handles that whatever the close
/// behaviour — and clicking the chip again toggles it shut.
pub fn working_popup<R>(
    toggle: &egui::Response,
    min_width: f32,
    content: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let ctx = toggle.ctx.clone();
    let state = toggle.id.with("working-popup-open");
    let mut open = ctx.data(|d| d.get_temp::<bool>(state).unwrap_or(false));
    // Read before `show`, because the same click is about to be offered to the
    // dismissal check below and opening a popup must not close it.
    let toggled = toggle.clicked();
    if toggled {
        open = !open;
    }

    let inner = egui::Popup::from_response(toggle)
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .show(|ui| {
            ui.set_min_width(min_width);
            content(ui)
        });

    if let (false, Some(shown)) = (toggled, &inner) {
        let click = ctx.input(|i| i.pointer.any_click().then_some(i.pointer.interact_pos()).flatten());
        if let Some(pos) = click {
            let inside = shown.response.interact_rect.contains(pos);
            let landed_on = ctx.layer_id_at(pos).map(|layer| layer.order);
            if click_dismisses_working_popup(inside, landed_on) {
                open = false;
            }
        }
    }

    ctx.data_mut(|d| d.insert_temp(state, open));
    inner.map(|shown| shown.inner)
}

/// Where in `range` a value falls, as a 0.0..=1.0 fraction along the slider
/// track. Pulled out of [`slider_row`] so the mapping can be checked without a
/// `Ui`. An empty or inverted range reads as the low end rather than dividing
/// by zero or going negative.
fn slider_fraction(value: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
    let (lo, hi) = (*range.start(), *range.end());
    if hi <= lo {
        return 0.0;
    }
    ((value - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// The inverse of [`slider_fraction`]: a 0.0..=1.0 position along the track
/// back to a value in `range`. What a click or drag at a given pointer
/// position turns into.
fn slider_value_at(fraction: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
    let (lo, hi) = (*range.start(), *range.end());
    lo + fraction.clamp(0.0, 1.0) * (hi - lo)
}

/// A slider row: `[ label 62px ] [ track flex:1 ] [ value 38px ]`, reading left
/// to right in the one order 2b rule 3 asks every panel to share — Edit's
/// Velocity/Length/Prob/Swing, Harmony's Inversion/Strum, Generate's
/// Motion/Looseness/Humanize and its compact Density row all draw from this
/// one function rather than laying the three pieces out separately four times.
///
/// `format` renders the current value for the value box — "62", "38%", "0.75"
/// — independently of what drag/typing store, which is always the raw `f32`.
///
/// The track is a custom-painted `Sense::click_and_drag()` rect, following the
/// painter-plus-`interact` convention `pianoroll.rs` and `tracks.rs` already
/// use for this codebase's other custom widgets, rather than reaching for
/// `egui::Slider` — the design spec's 3px track and 3×11px handle are not that
/// widget's look. The value box reuses `egui::DragValue`, restyled the way
/// [`colored_button`] restyles `egui::Button`: this is the same drag-to-change,
/// click-to-type convention `transport.rs` already gives the tempo field, so a
/// caller does not learn two different ways to type a number into this app.
///
/// Returns whether either half changed the value, matching this codebase's
/// `changed: bool` convention rather than handing back an `egui::Response`.
///
/// **Left out of this pass**: the value box's default parser is `egui`'s own
/// (whitespace-insensitive, no unit-stripping) rather than a parser built from
/// `format`'s inverse, so typing over a formatted "38%" and leaving the `%` in
/// will fail to parse — `DragValue` selects-all on focus, so the common case
/// (type digits, overwrite the lot) works, but a caller whose `format` embeds
/// a non-numeric suffix and wants that suffix to survive editing will need its
/// own `custom_parser`, which this function does not expose yet.
pub fn slider_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    format: impl Fn(f32) -> String,
) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        ui.add_sized(
            egui::vec2(62.0, 0.0),
            egui::Label::new(egui::RichText::new(label).size(11.5).color(TEXT_MUTED)),
        );

        let value_w = 38.0;
        let row_h = 16.0;
        let track_w = (ui.available_width() - value_w - ui.spacing().item_spacing.x).max(20.0);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(track_w, row_h), egui::Sense::click_and_drag());

        if response.dragged() || response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let fraction = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
                let new_value = slider_value_at(fraction, &range);
                if new_value != *value {
                    *value = new_value;
                    changed = true;
                }
            }
        }

        let fraction = slider_fraction(*value, &range);
        let painter = ui.painter_at(rect);
        let mid_y = rect.center().y;
        let track_rect =
            egui::Rect::from_min_size(egui::pos2(rect.left(), mid_y - 1.5), egui::vec2(rect.width(), 3.0));
        painter.rect_filled(track_rect, 0.0, PANEL_BORDER);
        let fill_w = rect.width() * fraction;
        if fill_w > 0.0 {
            painter.rect_filled(
                egui::Rect::from_min_size(track_rect.min, egui::vec2(fill_w, 3.0)),
                0.0,
                CYAN,
            );
        }
        let handle_x = rect.left() + fill_w;
        painter.rect_filled(
            egui::Rect::from_center_size(egui::pos2(handle_x, mid_y), egui::vec2(3.0, 11.0)),
            0.0,
            TEXT_BRIGHT,
        );

        // **The value box is allocated and clipped, not `add_sized`.**
        // `add_sized` is a *minimum*: a `DragValue` still grows to fit its own
        // text, so a long format ("2 ticks") made the box wider than the 38px
        // the row's arithmetic had already subtracted — the row overflowed the
        // panel and, worse, pushed the parent `Ui`'s width out with it, so the
        // *next* widget started further right again. That is what made
        // Generate's part cards walk progressively off the edge. Allocating
        // the rect and clipping a child `Ui` to it means the box is 38px
        // whatever the text says, so every value box down a panel shares one
        // left and one right edge.
        let (value_rect, _) = ui.allocate_exact_size(egui::vec2(value_w, row_h), egui::Sense::hover());
        let mut value_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(value_rect)
                .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        value_ui.set_clip_rect(value_rect.intersect(ui.clip_rect()));
        value_ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
        let widgets = &mut value_ui.style_mut().visuals.widgets;
        for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
            state.weak_bg_fill = PANEL_BORDER;
            state.bg_fill = PANEL_BORDER;
            state.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);
            state.bg_stroke = egui::Stroke::NONE;
            state.corner_radius = egui::CornerRadius::ZERO;
        }
        let dv = egui::DragValue::new(value)
            .range(*range.start() as f64..=*range.end() as f64)
            .custom_formatter(move |v, _| format(v as f32));
        changed |= value_ui.add(dv).changed();
    });

    changed
}

#[cfg(test)]
mod tests {

    /// **The regression this guards was found by running the app in light mode.**
    /// Nothing pinned the theme, so egui followed the OS and painted every
    /// surface the app does not paint itself — the whole Setup panel among them —
    /// at its light `panel_fill`. Tests cannot see a colour on a screen, but they
    /// can see that the preference was set, which is the thing that was missing.
    #[test]
    fn the_theme_is_pinned_dark_and_does_not_follow_the_system() {
        let ctx = egui::Context::default();
        install_style(&ctx);
        assert_eq!(
            ctx.options(|o| o.theme_preference),
            egui::ThemePreference::Dark,
            "this palette has no light variant, so the system theme must not reach it"
        );
    }

    /// The two surfaces that actually leaked land on the handoff's panel colour
    /// rather than on egui's stock dark grey.
    #[test]
    fn the_leaking_surfaces_use_the_handoffs_panel_colour() {
        let ctx = egui::Context::default();
        install_style(&ctx);
        // `style_of`, because egui 0.36 keeps one `Style` per theme and the
        // dark one is the only one this app can reach.
        let style = ctx.style_of(egui::Theme::Dark);
        assert_eq!(style.visuals.panel_fill, PANEL_BG);
        assert_eq!(style.visuals.window_fill, PANEL_BG);
        // The setting this function existed for before, still set.
        assert!(!style.interaction.selectable_labels);

        // **And the light style too, which is the belt to the braces.**
        // `all_styles_mut` writes both, so the surfaces that leaked are the
        // palette's colour even if a future change lets the theme go light.
        // This is not theoretical: it is the assertion that was checked on
        // screen — the app was run with the preference forced to Light and the
        // Setup panel, the strip under the trig lane and the panel seams all
        // measured `PANEL_BG`, zero near-white pixels among 970,400.
        let light = ctx.style_of(egui::Theme::Light);
        assert_eq!(light.visuals.panel_fill, PANEL_BG);
        assert_eq!(light.visuals.window_fill, PANEL_BG);
    }
    use super::*;

    #[test]
    fn slider_fraction_maps_the_range_ends_to_zero_and_one() {
        assert_eq!(slider_fraction(0.0, &(0.0..=127.0)), 0.0);
        assert_eq!(slider_fraction(127.0, &(0.0..=127.0)), 1.0);
        assert_eq!(slider_fraction(63.5, &(0.0..=127.0)), 0.5);
    }

    #[test]
    fn slider_fraction_clamps_values_outside_the_range() {
        assert_eq!(slider_fraction(-10.0, &(0.0..=100.0)), 0.0);
        assert_eq!(slider_fraction(200.0, &(0.0..=100.0)), 1.0);
    }

    #[test]
    fn slider_fraction_does_not_divide_by_zero_on_a_degenerate_range() {
        assert_eq!(slider_fraction(5.0, &(3.0..=3.0)), 0.0);
    }

    #[test]
    fn slider_value_at_is_the_inverse_of_slider_fraction() {
        let range = 20.0..=300.0;
        for value in [20.0, 40.0, 160.0, 299.9, 300.0] {
            let fraction = slider_fraction(value, &range);
            let recovered = slider_value_at(fraction, &range);
            assert!((recovered - value).abs() < 0.01, "{value} round-tripped to {recovered}");
        }
    }

    #[test]
    fn slider_value_at_clamps_out_of_range_fractions() {
        let range = 0.0..=10.0;
        assert_eq!(slider_value_at(-0.5, &range), 0.0);
        assert_eq!(slider_value_at(1.5, &range), 10.0);
    }

    #[test]
    fn a_click_inside_a_working_popup_never_dismisses_it() {
        // IgnoreClicks is the whole point: a click on a combo box, a text field
        // or a button inside the popup is an edit in progress, not a dismissal.
        assert!(!click_dismisses_working_popup(true, Some(egui::Order::Foreground)));
        assert!(!click_dismisses_working_popup(true, Some(egui::Order::Background)));
    }

    #[test]
    fn a_click_on_a_dropdown_above_the_popup_never_dismisses_it() {
        // The bug this function exists for. A combo box inside the popup draws
        // its list in its own `Area` at `Order::Foreground`, so a click on a
        // pattern name is outside the popup's own rect and on a layer above it.
        // Dismissing there would close the scene box the moment the user picked
        // the pattern they opened it for.
        assert!(!click_dismisses_working_popup(false, Some(egui::Order::Foreground)));
        assert!(!click_dismisses_working_popup(false, Some(egui::Order::Tooltip)));
    }

    #[test]
    fn a_click_on_the_app_behind_the_popup_dismisses_it() {
        // Panels and the workspace are below Foreground, so clicking the roll,
        // the transport or a side panel puts the popup away — which is what
        // makes it feel like a popup rather than a window.
        assert!(click_dismisses_working_popup(false, Some(egui::Order::Background)));
        assert!(click_dismisses_working_popup(false, Some(egui::Order::Middle)));
        // No layer at all: the click hit nothing interactive, so treat it as
        // outside rather than leaving the popup open on a click into the void.
        assert!(click_dismisses_working_popup(false, None));
    }

    /// One headless egui frame: feed `events`, run `body`, return. The same
    /// harness `triglane.rs`'s COND-picker test uses, and for the same reason —
    /// both bugs it is aimed at here are *interaction* bugs, invisible to any
    /// test of a pure rule, and both were found by hand on a screen.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, body: impl FnMut(&mut Ui)) {
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, body);
        // No renderer here to apply the font-atlas delta to, and epaint's debug
        // assert refuses to let it drop unhandled.
        output.textures_delta.clear();
    }

    /// Every string a frame actually painted, tooltips included.
    ///
    /// The paint list rather than a claim about it, because the whole point of
    /// [`info_icon`] is that its sentence is *not* on screen — and "absent" and
    /// "present" are indistinguishable to any test that only calls the drawing
    /// code and looks at what it returned.
    fn text_painted(
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
        body: impl FnMut(&mut Ui),
    ) -> String {
        let input = egui::RawInput { time: Some(time), events, ..Default::default() };
        let mut output = ctx.run_ui(input, body);
        output.textures_delta.clear();

        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut painted = String::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    #[test]
    fn the_info_icon_keeps_its_sentence_off_screen_until_it_is_hovered() {
        // The Setup panel's OUT warning, 2026-08-20: the eyebrow and the amber
        // rule stay, the three wrapped lines under them move into a tooltip.
        // Both halves are asserted, because a note that hid its body and had no
        // way to get it back would pass a test of either one alone.
        const EYEBROW: &str = "OVERWRITES THE DEVICE";
        const BODY: &str = "Everything in here overwrites what is on the device.";

        let ctx = egui::Context::default();
        install_style(&ctx);

        let icon = std::cell::Cell::new(egui::Rect::NOTHING);
        let mut draw = |ui: &mut Ui| {
            destructive_note_tip(ui, EYEBROW, BODY);
            // The same call the note makes, drawn again below it so the hover
            // has a rect to aim at: `destructive_note_tip` reports nothing, and
            // guessing at where a 9.5px eyebrow ends is a test that fails on a
            // font change rather than on a regression.
            ui.horizontal(|ui| icon.set(info_icon(ui, WARN_AMBER, BODY).rect));
        };

        let cold = text_painted(&ctx, 0.0, vec![], &mut draw);
        assert!(cold.contains(EYEBROW), "the eyebrow is the part that must not hide");
        assert!(!cold.contains(BODY), "the body is the space this bought back");

        // egui gates a tooltip on three things, and a headless frame satisfies
        // none of them by accident: the pointer has to be *over* the widget as
        // of the previous pass's layout, it has to be **still**, and it has to
        // have been still for `tooltip_delay` — half a second, measured against
        // `RawInput::time`, since there is no clock behind a bare `Context`.
        // So the move is sent once and the frames after it carry no events at
        // all; re-sending `PointerMoved` every frame keeps resetting the very
        // timer being waited on, which is what the first draft of this test did.
        text_painted(&ctx, 0.1, vec![egui::Event::PointerMoved(icon.get().center())], &mut draw);
        let mut hovered = String::new();
        for tick in 0..12 {
            hovered = text_painted(&ctx, 0.2 + tick as f64 * 0.1, vec![], &mut draw);
            if hovered.contains(BODY) {
                break;
            }
        }
        assert!(
            hovered.contains(BODY),
            "hovering the ⓘ must put the warning back on screen; painted:\n{hovered}"
        );
    }

    /// A primary press and release at `pos`, as two frames' worth of events.
    fn click_at(pos: egui::Pos2) -> (Vec<egui::Event>, Vec<egui::Event>) {
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        (vec![egui::Event::PointerMoved(pos), press(true)], vec![press(false)])
    }

    /// Draws a `scope_builder`-sensed row with a label in it — the shape every
    /// custom clickable row in this UI has — and reports whether a click on the
    /// *label's own rect* reached the row.
    fn row_takes_a_click_on_its_text(ctx: &egui::Context) -> bool {
        let label_rect = std::cell::Cell::new(egui::Rect::NOTHING);
        let clicked = std::cell::Cell::new(false);
        let mut draw = |ui: &mut Ui| {
            let response = ui
                .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                    label_rect.set(ui.label("HARMONY").rect);
                })
                .response;
            if response.clicked() {
                clicked.set(true);
            }
        };

        // egui hit-tests against the previous pass's layout, so the row has to
        // have been drawn once before a press can land on it.
        frame(ctx, vec![], &mut draw);
        let (press, release) = click_at(label_rect.get().center());
        frame(ctx, press, &mut draw);
        frame(ctx, release, &mut draw);
        clicked.get()
    }

    #[test]
    fn a_label_does_not_eat_the_click_on_the_row_it_is_in() {
        // Bug 2 of 2026-08-19: the rail's EDIT / HARMONY / GENERATE / SESSION
        // rows only responded to a click on the padding *between* the words.
        // egui's labels are selectable by default, which gives them
        // `Sense::click_and_drag()`, and a label that senses a click takes it
        // from the row underneath — with an I-beam cursor as the only clue.
        let installed = egui::Context::default();
        install_style(&installed);
        assert!(
            row_takes_a_click_on_its_text(&installed),
            "a click on the row's own text must reach the row"
        );

        // The control, and the reason `install_style` exists: on egui's defaults
        // the identical row swallows the identical click.
        let defaults = egui::Context::default();
        assert!(
            !row_takes_a_click_on_its_text(&defaults),
            "if egui's default ever stops eating this click, install_style can go"
        );
    }

    #[test]
    fn a_working_popup_survives_the_combo_boxes_inside_it() {
        // Bugs 1 and 3 of 2026-08-19, which were one bug: the scene pill's
        // popup and a Generate part's destination chip both vanished the moment
        // a picker inside them was clicked, because egui's memory holds one
        // open popup per viewport and a `ComboBox` opening its own list evicted
        // the popup it was drawn in. This walks the exact sequence the user
        // reported: open the popup, click the combo box, pick a value.
        let ctx = egui::Context::default();
        install_style(&ctx);

        let toggle_rect = std::cell::Cell::new(egui::Rect::NOTHING);
        let combo_rect = std::cell::Cell::new(egui::Rect::NOTHING);
        let item_rect = std::cell::Cell::new(egui::Rect::NOTHING);
        let popup_shown = std::cell::Cell::new(false);
        let chosen = std::cell::Cell::new(0usize);

        let mut draw = |ui: &mut Ui| {
            let toggle = ui.button("SCENE 1");
            toggle_rect.set(toggle.rect);
            let shown = working_popup(&toggle, 120.0, |ui| {
                let combo = egui::ComboBox::from_id_salt("slot")
                    .selected_text("A01")
                    .show_ui(ui, |ui| {
                        let mut pick = chosen.get();
                        ui.selectable_value(&mut pick, 0, "A01");
                        item_rect.set(ui.selectable_value(&mut pick, 1, "A02").rect);
                        chosen.set(pick);
                    });
                combo_rect.set(combo.response.rect);
            });
            popup_shown.set(shown.is_some());
        };

        frame(&ctx, vec![], &mut draw);
        assert!(!popup_shown.get(), "the popup starts closed");

        let (press, release) = click_at(toggle_rect.get().center());
        frame(&ctx, press, &mut draw);
        frame(&ctx, release, &mut draw);
        assert!(popup_shown.get(), "clicking the pill opens it");
        frame(&ctx, vec![], &mut draw);
        assert!(popup_shown.get(), "and it survives the frame that opened it");

        // The click that used to kill it: the combo box inside.
        let (press, release) = click_at(combo_rect.get().center());
        frame(&ctx, press, &mut draw);
        frame(&ctx, release, &mut draw);
        assert!(popup_shown.get(), "clicking a picker inside must not dismiss the popup");
        frame(&ctx, vec![], &mut draw);
        assert!(popup_shown.get(), "and the popup is still there to pick from");
        assert_ne!(item_rect.get(), egui::Rect::NOTHING, "the dropdown list opened");

        // Picking a value: the dropdown is its own `Area` above the popup, so
        // this click is outside the popup's rect and must still not dismiss it.
        let (press, release) = click_at(item_rect.get().center());
        frame(&ctx, press, &mut draw);
        frame(&ctx, release, &mut draw);
        assert_eq!(chosen.get(), 1, "the value was picked");
        assert!(popup_shown.get(), "picking a value leaves the popup open for the next box");

        // A click on the app behind it does close it.
        let away = egui::Pos2 { x: 700.0, y: 500.0 };
        let (press, release) = click_at(away);
        frame(&ctx, press, &mut draw);
        frame(&ctx, release, &mut draw);
        frame(&ctx, vec![], &mut draw);
        assert!(!popup_shown.get(), "a click outside puts it away");
    }
}
