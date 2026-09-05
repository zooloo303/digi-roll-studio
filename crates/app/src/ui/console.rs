// The console: one line along the foot of the window, saying what the app just
// did, with everything it has said this session behind it.
//
// **This exists because the TRACKS pane was the wrong place for it.** Copy,
// paste, clear and transpose used to speak in a line under that pane's own
// header, and the pane is drawn at a *fixed* height (`tracks::pane_height`) —
// so every pixel that line took came out of the grid's `ScrollArea` beneath it.
// With three boxes on the desk the third row went below the fold, and a message
// about a track you had just moved was the thing hiding the track you had just
// moved. It also stuck: the line stayed until something replaced it, so a
// sentence about a slot you had since navigated away from sat there being
// quietly untrue.
//
// A `Panel::bottom` cannot do either. It takes its height from the window once,
// off the roll — which is elastic and has pixels to give — and never from the
// grid, whose rows are the app's navigation centrepiece. Expanding the
// scrollback takes more of the same pixels, and still none of the grid's.
//
// ## Why a post-box rather than a `&mut Console` passed down
//
// The pane that has something to say is drawn *after* this one: the console is
// claimed off the window's floor before the central panel exists, which is the
// only order in which a panel can reserve space. So a message posted while the
// grid draws cannot be drawn until the next pass, and [`post`] leaves it in
// egui's own per-id memory for the console to collect.
//
// The cost is one frame, and [`post`] pays it by asking for a repaint —
// otherwise a message posted by the last keystroke before you sat back would
// wait, unshown, for whatever input happened next. The benefit is that
// `tracks::ui` keeps its signature and its ignorance: it says things, and where
// they are drawn is not its business. It was already keeping its status line in
// this same memory for the same kind of reason.
//
// ## What is not here
//
// The panels keep their own consequence lines — the fetch row's summary, a
// write's verdict, Presets' scan reports — because those belong beside the
// controls that produced them, in space the owning panel is free to reflow.
// What *also* lands here is one line per thing done: the TRACKS grid's copies
// and clears, the Setup panel's adds, port moves, clock toggles, fetches and
// sends, Session's "Saved to…". The inline line is replaced by the next
// attempt; this log is where the previous ones survive. Lines worded for a
// row — which sit under a heading that already names the box — are prefixed
// with the box's name at the `post` call site, because an inch above a console
// line there is no such heading.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui::{self, Ui};

/// How many messages the scrollback keeps. Old enough to have scrolled past
/// twice over, small enough that nothing has to think about the memory.
pub const CAPACITY: usize = 100;

/// The collapsed strip's height. Fixed, and the whole point of it being fixed:
/// an empty console and a console with something to say take the same room, so
/// nothing in the window moves when a message arrives.
pub const STRIP_H: f32 = 24.0;

/// The scrollback's height when it is open, on top of the strip.
const SCROLLBACK_H: f32 = 148.0;

/// The padding either end of the strip, and the room kept at the right for the
/// age and the disclosure. Fixed, so the message's own width is known before it
/// is drawn — see [`Console::strip`].
const PAD: f32 = 12.0;
const RIGHT_W: f32 = 150.0;

/// What the strip says before anything has happened. Names the pane whose
/// messages land here, so an empty console reads as a place rather than as a
/// panel that failed to load.
const EMPTY: &str = "What the app does \u{2014} copies, pastes, clears, transposes, fetches, \
                     sends \u{2014} reports here.";

/// One thing the app has said.
#[derive(Debug, Clone)]
pub struct Entry {
    pub text: String,
    /// When it was said. `Instant`, not a wall clock: see [`age_label`].
    pub at: Instant,
}

/// The window's message log.
#[derive(Debug, Default)]
pub struct Console {
    entries: VecDeque<Entry>,
    /// Whether the scrollback is showing under the strip. Closed on launch, and
    /// a message arriving never opens it — a log that unfolds itself over the
    /// roll every time you transpose a track is the bug this module was written
    /// to fix, in a new place.
    expanded: bool,
}

/// Where [`post`] leaves messages for the next pass to collect.
fn outbox_id() -> egui::Id {
    egui::Id::new("digi-roll-studio::console::outbox")
}

/// Say something in the console.
///
/// Callable from anywhere with a `Context`, and deliberately not a method: the
/// pane that has something to say is drawn after the console and cannot hold a
/// `&mut` to it. See the module doc for the frame this costs and why the
/// repaint request is not optional.
pub fn post(ctx: &egui::Context, message: impl Into<String>) {
    let message = message.into();
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<String>>(outbox_id()).push(message));
    // Without this a message posted by the last keystroke before your hands
    // leave the keyboard is drawn whenever something else happens to ask for a
    // frame, which could be minutes.
    ctx.request_repaint();
}

/// How long ago, in words.
///
/// **Relative, not a clock time, and that is a limitation stated rather than
/// hidden.** `protocol::safe_write::Timestamp` can turn a Unix second into
/// hours and minutes but only in **UTC**, and this workspace carries no
/// timezone database — a strip that said `04:31` for something you did at half
/// nine in the evening would be worse than no time at all. An age needs no
/// zone, and for a log you read while working it is the more useful of the two
/// anyway. A wall clock here is a dependency decision, not an afternoon's work.
pub fn age_label(age: Duration) -> String {
    let secs = age.as_secs();
    match secs {
        0..=4 => String::from("just now"),
        5..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

/// How long until an age label would go stale, or `None` for one that will not
/// change again on any timescale worth a repaint.
///
/// The strip has to ask for its own repaints or "just now" stays "just now"
/// until something else happens to draw a frame. This keeps that to the
/// smallest honest interval: once a second while the newest message is under a
/// minute old, once a minute for the hour after that, and then nothing.
fn refresh_after(age: Duration) -> Option<Duration> {
    match age.as_secs() {
        0..=59 => Some(Duration::from_secs(1)),
        60..=3599 => Some(Duration::from_secs(60)),
        _ => None,
    }
}

impl Console {
    /// Collect anything [`post`]ed since the last pass. Public because it is
    /// how a test drives the same path the window does, rather than reaching
    /// into egui's memory itself.
    pub fn collect(&mut self, ctx: &egui::Context) {
        let posted: Vec<String> =
            ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<String>>(outbox_id())));
        let now = Instant::now();
        for text in posted {
            self.entries.push_front(Entry { text, at: now });
        }
        while self.entries.len() > CAPACITY {
            self.entries.pop_back();
        }
    }

    /// The most recent thing said, if anything has been.
    pub fn latest(&self) -> Option<&Entry> {
        self.entries.front()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The height the console wants off the window's floor this frame.
    ///
    /// Asked before the panel is built, the way `tracks::pane_height` is asked
    /// before the workspace row is allocated.
    pub fn height(&self) -> f32 {
        if self.expanded {
            STRIP_H + SCROLLBACK_H
        } else {
            STRIP_H
        }
    }

    /// Draw it. Collects first, so a message posted last pass is on screen in
    /// this one.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.collect(ui.ctx());

        let frame = egui::Frame::new().fill(super::PANEL_BG_RAISED);
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The 1px rule along the top, drawn rather than left to a `Frame`
            // stroke for the same reason the transport bar draws its own: the
            // panel's frame is `NONE` so no default fill can show through as a
            // seam. `paint_at` on the strip's own rect, so the line sits on the
            // boundary the eye reads as the window's floor beginning.
            let top = ui.max_rect().left_top();
            ui.painter().line_segment(
                [top, egui::pos2(ui.max_rect().right(), top.y)],
                egui::Stroke::new(1.0, super::PANEL_BORDER),
            );
            self.strip(ui);
            if self.expanded {
                self.scrollback(ui);
            }
        });
    }

    /// The one line: the newest message, its age, and the disclosure.
    ///
    /// **The rect is split by hand rather than by nesting two layouts**, and
    /// the first cut of this got it wrong on screen in exactly the way that
    /// costs a build: an inner `with_layout(right_to_left)` takes *all* the
    /// remaining width, so the age and the toggle drew correctly at the right
    /// and the message itself was left with nothing and drew as nothing. The
    /// transport bar gets away with the same nesting because everything to the
    /// left of its right-hand end is a fixed-size zone; a sentence is not.
    ///
    /// So the strip allocates its rect and cuts it in two — `slider_row`'s own
    /// idiom for a row with a fixed end — which also means the message's width
    /// is known, and a `Label` only truncates against a width it knows.
    fn strip(&mut self, ui: &mut Ui) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), STRIP_H), egui::Sense::hover());
        let split = (rect.right() - PAD - RIGHT_W).max(rect.left() + PAD);
        let left = egui::Rect::from_min_max(egui::pos2(rect.left() + PAD, rect.top()), egui::pos2(split, rect.bottom()));
        let right = egui::Rect::from_min_max(egui::pos2(split, rect.top()), egui::pos2(rect.right() - PAD, rect.bottom()));

        // The message. Truncated rather than wrapped: the strip is one line by
        // construction, and a sentence that wrapped would change this panel's
        // height — which is the entire behaviour this module exists to stop.
        let mut message_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(left)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        message_ui.set_clip_rect(left.intersect(ui.clip_rect()));
        match self.entries.front() {
            Some(newest) => {
                message_ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(&newest.text).size(11.0).color(super::TEXT_SECONDARY),
                        )
                        .truncate(),
                    )
                    // The whole sentence on hover, because the strip is where a
                    // long one gets cut and this is the cheapest way to read the
                    // rest without opening the log.
                    .on_hover_text(&newest.text);
            }
            None => {
                message_ui.add(
                    egui::Label::new(egui::RichText::new(EMPTY).size(11.0).color(super::TEXT_DIMMEST))
                        .truncate(),
                );
            }
        }

        // The age and the disclosure, pinned to the corner however long the
        // message is — which is what the split buys.
        let mut right_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right_ui.set_clip_rect(right.intersect(ui.clip_rect()));
        right_ui.spacing_mut().item_spacing.x = 10.0;
        if self.entries.is_empty() {
            return;
        }
        let label = if self.expanded {
            String::from("hide")
        } else {
            match self.entries.len() {
                1 => String::from("1 message"),
                n => format!("{n} messages"),
            }
        };
        let toggle = right_ui.add(
            egui::Label::new(egui::RichText::new(label).size(11.0).color(super::TEXT_DIM))
                .sense(egui::Sense::click()),
        );
        if toggle.clicked() {
            self.expanded = !self.expanded;
        }
        toggle.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if self.expanded {
            "Close the log."
        } else {
            "Everything this session has reported, newest first."
        });

        if let Some(newest) = self.entries.front() {
            let age = newest.at.elapsed();
            right_ui.label(egui::RichText::new(age_label(age)).size(11.0).color(super::TEXT_DIMMEST));
            if let Some(after) = refresh_after(age) {
                ui.ctx().request_repaint_after(after);
            }
        }
    }

    /// The scrollback: everything said this session, newest first.
    fn scrollback(&mut self, ui: &mut Ui) {
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("digi-roll-console-log")
            .max_height(SCROLLBACK_H - 12.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(2.0);
                for entry in &self.entries {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.spacing_mut().item_spacing.x = 8.0;
                        // The age column first and fixed-width, so a run of
                        // messages reads as a log rather than as ragged prose.
                        ui.add_sized(
                            egui::vec2(56.0, 0.0),
                            egui::Label::new(
                                egui::RichText::new(age_label(entry.at.elapsed()))
                                    .size(10.5)
                                    .color(super::TEXT_DIMMEST),
                            ),
                        );
                        ui.label(
                            egui::RichText::new(&entry.text)
                                .size(10.5)
                                .color(super::TEXT_SECONDARY),
                        );
                    });
                }
                ui.add_space(4.0);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posted_message_is_collected_once_and_only_once() {
        let ctx = egui::Context::default();
        let mut console = Console::default();
        post(&ctx, "Cleared DT2 T01.");

        console.collect(&ctx);
        assert_eq!(console.len(), 1);
        assert_eq!(console.latest().unwrap().text, "Cleared DT2 T01.");

        // The outbox is taken, not copied — a second pass must not redraw the
        // same sentence as a second event.
        console.collect(&ctx);
        assert_eq!(console.len(), 1);
    }

    #[test]
    fn the_newest_message_is_the_one_the_strip_shows() {
        let ctx = egui::Context::default();
        let mut console = Console::default();
        post(&ctx, "first");
        post(&ctx, "second");
        console.collect(&ctx);

        assert_eq!(console.latest().unwrap().text, "second", "newest first");
        assert_eq!(console.len(), 2);
    }

    #[test]
    fn the_scrollback_is_bounded() {
        let ctx = egui::Context::default();
        let mut console = Console::default();
        for i in 0..(CAPACITY + 20) {
            post(&ctx, format!("message {i}"));
        }
        console.collect(&ctx);

        assert_eq!(console.len(), CAPACITY);
        assert_eq!(console.latest().unwrap().text, format!("message {}", CAPACITY + 19));
    }

    /// The property the whole module rests on: an empty console and a busy one
    /// claim exactly the same pixels, so nothing in the window moves when a
    /// message arrives. Only a deliberate click on the toggle changes it.
    #[test]
    fn a_message_arriving_never_changes_the_height() {
        let ctx = egui::Context::default();
        let mut console = Console::default();
        let empty = console.height();
        assert_eq!(empty, STRIP_H);

        post(&ctx, "Moved DT2 T01 up an octave.");
        console.collect(&ctx);
        assert_eq!(console.height(), empty, "a message costs the window nothing");

        console.expanded = true;
        assert!(console.height() > empty, "only opening the log does");
    }

    #[test]
    fn an_age_reads_as_words_at_every_scale() {
        assert_eq!(age_label(Duration::from_secs(0)), "just now");
        assert_eq!(age_label(Duration::from_secs(4)), "just now");
        assert_eq!(age_label(Duration::from_secs(5)), "5s ago");
        assert_eq!(age_label(Duration::from_secs(59)), "59s ago");
        assert_eq!(age_label(Duration::from_secs(60)), "1m ago");
        assert_eq!(age_label(Duration::from_secs(3599)), "59m ago");
        assert_eq!(age_label(Duration::from_secs(3600)), "1h ago");
        assert_eq!(age_label(Duration::from_secs(7_200)), "2h ago");
    }

    /// A label that will not change again must not keep the app waking up to
    /// redraw it.
    #[test]
    fn a_settled_age_asks_for_no_more_repaints() {
        assert_eq!(refresh_after(Duration::from_secs(0)), Some(Duration::from_secs(1)));
        assert_eq!(refresh_after(Duration::from_secs(59)), Some(Duration::from_secs(1)));
        assert_eq!(refresh_after(Duration::from_secs(60)), Some(Duration::from_secs(60)));
        assert_eq!(refresh_after(Duration::from_secs(3600)), None);
    }

    #[test]
    fn it_draws_empty_and_full_and_open_without_panicking() {
        let ctx = egui::Context::default();
        let mut console = Console::default();
        let pass = |console: &mut Console| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| console.ui(ui));
            output.textures_delta.clear();
        };

        pass(&mut console);
        post(&ctx, "Moved DN2 T10 down an octave \u{2014} 16 trigs, D#5 to F6 now.");
        pass(&mut console);
        console.expanded = true;
        pass(&mut console);
        assert_eq!(console.len(), 1);
    }
}
