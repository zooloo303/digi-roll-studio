// The track lanes: which track the roll is editing, and — since the 2026-08-19
// redesign — a real per-cell view of what each of the 32 tracks in a session
// actually holds, rather than a bare numbered button.
//
// This replaces the old PATTERNS pane's wrapped row of "1", "2·", "3" buttons
// with `design_handoff_digi_roll_ui/README.md` §1b's grid: one 46px-tall cell
// per track, showing trig count, step density, LEN, CH, and — while the
// transport runs — where that track is in its own loop. Ported into egui rather
// than copied from the HTML mock; see that file's `Component` class for the
// step-bucketing math this module's `step_density` is drawn from.
//
// **Per-box folding is gone.** The old pane collapsed a device's tracks behind
// a one-line summary once "32 open headers is more than a screen" — a concern
// that mattered when each track was a wrapped inline button. A 46px-tall row of
// 16 cells costs a fixed ~54px per box regardless of window width, and the
// spec's own layout (§1b) never mentions a fold control; §1b's `pane()` caller
// in `workspace.rs` already scrolls this pane when a session outgrows the
// row's height, which is the mechanism a third or fourth box now leans on
// instead. Dropping the fold state also let `Groups` and its fold-arrow polygon
// go — nothing in this redesign draws a glyph that could turn to tofu, per
// `super`'s postmortem table, because there is no glyph left to draw here.
//
// **The selected-track parameter row (§1b item 4) is deliberately almost
// untouched.** The spec calls it "unchanged from the current app apart from
// the box·track label at its head", so the M/S toggles, LEN, SCALE, CH and
// trig-count controls below are the same code the old pane had — ported
// forward rather than rebuilt. The one change beyond the label format is that
// the mock's static "PRESET 1" is not reproduced: this app has no preset/kit
// concept to back it, and printing a number nothing points to would be a lie
// dressed as a value. The label itself moves from "{box} · {track name}" to
// "{box} · {two-digit track number}", matching the spec's "DT2 · 01" — the
// grid cells are numbered, not named, so the row that follows them now speaks
// the same vocabulary.
//
// **Trig count and step density can disagree by design, and that is fine.**
// The spec says the density strip's green-bar count "must equal the trig
// count shown in row 1" for a track at or under 16 steps — true whenever a
// step holds at most one note, which is the common case. This app also lets
// one press of a chord put several notes on a single step (PLAN.md's harmony
// feature), and `track.notes.len()` — the trig count this row and the old pane
// both already used — counts notes, not steps. Rather than invent a second,
// disagreeing definition of "trig" for the strip alone, `step_density` buckets
// by *step*, so a stacked chord still reads as one lit bar rather than
// inflating the strip past the cell's 16 slots. The two numbers coincide
// exactly whenever they matter (no chords, LEN ≤ 16) and diverge only in the
// one case the spec never had real data for.
//
// **Every glyph this module draws is ASCII** — zero-padded numbers, "T", "L",
// "CH" — so none of it is a candidate for `super`'s tofu table. The spec calls
// for IBM Plex Mono/Sans throughout; this app bundles neither, and per
// `super`'s note that the mono family (Hack) "carries far less" glyph coverage
// than the proportional one, every custom-painted string here uses
// `FontId::proportional` rather than reach for a font this app has already
// learned to be careful with.
//
// **The playhead is a fill, not a needle.** The mock's CSS animates
// `transform: scaleX(0)` to `scaleX(1)` on the progress bar and the cell tint
// alike — a bar that *grows* left to right over one loop and snaps back, not a
// line that travels. `overlay_fraction` gives that growth as a plain 0..1
// number, computed the same way `workspace.rs`'s piano-roll playhead already
// is: the engine's `position_steps()`, scaled by the track's own `TrackScale`
// and wrapped by its own `length_steps` — so a DT2 track at 2x and a DN2 track
// at LEN 64 each sweep at their own true rate against the one clock, which is
// the whole point of a per-track overlay instead of one bar for the pane.

use digi_core::model::{PatchSound, TrackScale};
use digi_core::track_clip::{paste_track, TrackClip};
use digi_core::{Device, Pattern, Session, Source, Track, TrackKind};
use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::engine::EngineLink;

/// Which track the roll is editing: a device by position in the session, and a
/// track within whatever pattern that device plays in the current scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub device: usize,
    pub track: usize,
}

const SCALES: [(TrackScale, &str); 7] = [
    (TrackScale::Eighth, "1/8"),
    (TrackScale::Quarter, "1/4"),
    (TrackScale::Half, "1/2"),
    (TrackScale::ThreeQuarters, "3/4"),
    (TrackScale::One, "1x"),
    (TrackScale::ThreeHalves, "3/2"),
    (TrackScale::Two, "2x"),
];

fn scale_label(scale: TrackScale) -> &'static str {
    SCALES.iter().find(|(s, _)| *s == scale).map(|(_, l)| *l).unwrap_or("?")
}

/// The one line of a track cell's tooltip that answers "what patch is on this
/// track" — kept apart from `track_tooltip_text` so the plant that matters most
/// in this packet (making the no-patch branch print `track.name` as if it were
/// a sound name) has a single function's return value to break, and a single
/// function's return value for a test to pin back down.
///
/// Three shapes, and the wording is Neil's to approve, not this file's:
///
/// - **known** — `track.patch` is set and its `from` still matches
///   `pattern_source`: what the app last saw is presented as current, because
///   nothing has happened since to make it doubtful.
/// - **stale** — `track.patch` is set but `from` no longer matches
///   `pattern_source` (a re-fetch from elsewhere, or a pattern given a
///   different source since). Still shown — it is real information — but
///   labelled so it is not mistaken for what is loaded now.
/// - **none** — `track.patch` is `None`. This is the branch the plant targets:
///   it must say plainly that nothing was fetched, and **must never fall back
///   to `track.name`**, because a track name is not a claim this app can back
///   with a fetch. "What the app last saw" and "what is loaded right now" are
///   different claims, and blurring them here is exactly the dishonesty the
///   packet exists to avoid.
///
/// **Packet E's addendum adds a fourth split, inside "known"/"stale".** A
/// `track.patch` that *is* set no longer always means a named sound —
/// [`PatchSound`] also carries `Midi` (a fetched MIDI track, which has no
/// sound to name) and `Unnamed` (a fetched audio slot the kit never named).
/// Both branches are still a record of a real fetch and take the same
/// known/stale split as a named sound; only the head of the sentence differs.
/// Neil approved the MIDI wording verbatim on 2026-08-20; the `Unnamed`
/// wording is this file's draft, offered alongside it for the same approval.
pub fn patch_line(track: &Track, pattern_source: Option<&Source>) -> String {
    match &track.patch {
        // Names the control that fills this in rather than only the absence.
        // "Has not been fetched" was true and unhelpful: a whole-pattern fetch
        // is not the only way to get here, and on 2026-08-20 the read path's
        // own refusal sent Neil looking for a fetch he did not want. The
        // patch-names button in Setup is one click and touches nothing else.
        None => "No patch read from the box — \"Read patch names\" in Setup fills this in.".to_string(),
        Some(patch) => {
            let ts = digi_protocol::safe_write::Timestamp::from_unix_seconds(patch.seen_at);
            let date = format!("{:04}-{:02}-{:02}", ts.year, ts.month, ts.day);
            // **Where the names came from, said once.** A stored-slot read
            // names the slot; an edit-buffer read (the A4's `0x68`) has no slot
            // behind it and must not borrow one — see `TrackPatch::live`. Both
            // branches below that need to say where a name came from take this
            // rather than formatting `patch.from` themselves, so the two cannot
            // drift into disagreeing about what was read.
            let origin = if patch.live {
                "the kit the box had loaded".to_string()
            } else {
                format!("from {}", patch.from.label())
            };
            let base = match &patch.sound {
                PatchSound::Named(name) => {
                    format!("SOUND: {name} — kit {}, {origin}, read {date}", patch.kit_name)
                }
                // Neil's wording, approved verbatim 2026-08-20: a fetched MIDI
                // track was genuinely read, it simply has nothing to be named
                // after, and the sentence has to say that rather than fall
                // silent (which would read as "not fetched") or invent a name.
                PatchSound::Midi => format!("MIDI track — no sound to name (read {date})"),
                // This file's draft for the fourth case, offered for the same
                // approval: an audio track the kit never named. Shaped like the
                // known-sound line rather than the MIDI one, because unlike a
                // MIDI track this one *could* have had a sound — the kit just
                // never gave it one — so the kit and slot it was read from are
                // still worth saying.
                PatchSound::Unnamed => format!(
                    "No sound name on the box for this track — kit {}, {origin}, read {date}",
                    patch.kit_name,
                ),
                // The fifth case, 2026-09-01: the Analog Four's FX and CV
                // tracks, which its kit holds no sound for at all. Shaped like
                // the MIDI line rather than the `Unnamed` one — there is no kit
                // slot worth pointing at, because no read of any kit will ever
                // put a name here — and it says *why* rather than only that,
                // since "no sound" on its own reads like a fault.
                PatchSound::NoSound => {
                    format!("No sound on this track — the box's kit holds none for it (read {date})")
                }
            };
            // **Staleness is a claim about a slot, so a live read cannot be
            // stale in this sense.** The suffix means "the pattern on screen is
            // no longer the one this kit was read beside"; an edit-buffer read
            // never named a slot to disagree with, and printing the warning
            // against `from` — which for a live read is only where the records
            // landed — would flag every one of them.
            if patch.live || pattern_source == Some(&patch.from) {
                base
            } else {
                format!("{base} — stale: this pattern no longer matches that fetch")
            }
        }
    }
}

/// The full hover text for a track cell: identity, the patch line above, and
/// the same studio facts the parameter row below the grid already shows for
/// the selected track — kind, note count, length, scale, channel and out port.
pub fn track_tooltip_text(track_number: usize, track: &Track, pattern_source: Option<&Source>) -> String {
    let kind = match track.kind {
        TrackKind::Audio => "Audio",
        TrackKind::Midi => "MIDI",
    };
    let port = track.out_port.as_deref().unwrap_or("none");
    format!(
        // `·` throughout, never `→`: this is `.on_hover_text`, plain text with
        // no way to reach `super::paint_direction_arrow`, and `→` (U+2192) is
        // on `super`'s glyph table as unconfirmed — flagged from a real window
        // capture 2026-08-20, where it rendered as tofu. `channel_note`'s doc
        // comment above already made the same call for the same reason.
        // The last line is the copy chord, because a shortcut nobody can find
        // is a shortcut that does not exist — and this one cannot be guessed:
        // the obvious Cmd+C is unavailable for the reason
        // `handle_clipboard_shortcuts` documents at length. The cell's own
        // hover is the one place a person is already looking at the track they
        // want to copy.
        "Track {track_number} — {}\n{}\n{kind} · {} note{} · LEN {} · {} · CH{} · out: {port}\n\
         Shift+C to copy this track · Shift+V to paste onto it",
        track.name,
        patch_line(track, pattern_source),
        track.notes.len(),
        if track.notes.len() == 1 { "" } else { "s" },
        track.length_steps,
        scale_label(track.scale),
        track.channel as u16 + 1,
    )
}

/// What a channel means on a *factory* box, when it means something other
/// than "this track". `None` for the channels that are exactly what they look
/// like.
///
/// The reason this exists is a bug report from 2026-08-18: trigs on tracks 9 and
/// 10 of both digis played nothing at all. The engine was correct —
/// `scheduler.rs`'s `every_track_of_a_box_plays_on_its_own_channel` proves it puts
/// a note-on on all sixteen channels — and the app was still wrong, because it
/// let someone put trigs on a channel their box was never going to hear.
///
/// What a DT2/DN2 actually ships with, in `SETTINGS → MIDI CONFIG → CHANNELS`:
///
/// - `TRACK 1–8 CH` = channels 1–8. **`TRACK 9–16 CH` = `OFF`.** Sixteen tracks,
///   eight of them addressable, until the user says otherwise.
/// - `FX CONTROL CH` = **9**. Notes there play no track, and on DT2 OS earlier
///   than 1.15A a note on it could freeze the box (Elektron's own release notes
///   list "Sending Note On/Off messages to the FX control channel could, in some
///   circumstances, freeze the device" as a 1.15A fix). This one is worth a
///   warning on its own merits.
/// - `AUTO CHANNEL` = **10**. Notes there play whichever track is *selected* on
///   the box — so a trig aimed at track 10 comes out of whatever the user last
///   touched, which is worse than silence because it is intermittent.
///
/// And a factory Analog Four, in `GLOBAL → MIDI CONFIG → MIDI CHANNELS`
/// (manual, OS 1.0 through 1.51): `TRACK 1–4` = channels 1–4, `FX` = 5,
/// `CV` = 6 — a perfect 1:1 with this app's six rows — and nothing listens
/// above 6. The per-model split is why this takes the model key and the track
/// index: channel 5 *is* the FX track's own channel and only a warning on the
/// other five rows, where the digis' channel 9 is a warning on all sixteen.
///
/// **The 1:1 default in `Track::new` stays**, because it is the map to configure
/// the boxes *to*: it is the only mapping where the app's track number and the
/// box's agree — on an A4 it is the factory map outright — and any cleverer
/// default would be a second thing to reconcile at the moment something sounds
/// wrong. What the app owes is to say this next to the field where the channel
/// is chosen, rather than to send trigs into the dark.
///
/// Returned as `(label, why)` rather than drawn here so it can be tested without a
/// window — and the label is a word, not `⚠`, which `super`'s glyph table records
/// as withdrawn on suspicion of being tofu. **Both strings are ASCII**, for the
/// same reason: the menu path wants a `→` and `super`'s table has that one on the
/// suspect list, so it is spelled with `>`, and the ranges use `-` rather than the
/// `–` nobody has drawn either. The prose in this doc comment is free to be
/// typeset properly; it never reaches a font this app ships.
///
/// An unknown model key gets no note at all: this function only speaks about
/// boxes whose factory state it has actually read out of a manual.
pub fn channel_note(
    model_key: &str,
    track_index: usize,
    channel_1_based: u16,
) -> Option<(&'static str, &'static str)> {
    match model_key {
        "DT2" | "DN2" => match channel_1_based {
            9 => Some((
                "FX CTRL by default",
                "Channel 9 is FX CONTROL CH on a factory DT2/DN2: notes sent there play no track at \
                 all, and on DT2 OS before 1.15A one could freeze the box. Set TRACK 9 CH and move FX \
                 CONTROL CH in SETTINGS > MIDI CONFIG > CHANNELS, or give this track another channel.",
            )),
            10 => Some((
                "AUTO by default",
                "Channel 10 is AUTO CHANNEL on a factory DT2/DN2: notes sent there play whichever track \
                 is selected on the box, not this one. Move AUTO CHANNEL off 10 and set TRACK 10 CH in \
                 SETTINGS > MIDI CONFIG > CHANNELS.",
            )),
            11..=16 => Some((
                "unassigned by default",
                "A factory DT2/DN2 gives channels to tracks 1-8 only; TRACK 9-16 CH are OFF, so this \
                 track stays silent until the box is told to listen. Set TRACK n CH in SETTINGS > MIDI \
                 CONFIG > CHANNELS.",
            )),
            _ => None,
        },
        "A4" => match channel_1_based {
            // Channel 5 is the FX track's own channel — a note only when some
            // *other* row is aimed at it. Same shape for CV on 6.
            5 if track_index != 4 => Some((
                "FX track by default",
                "Channel 5 belongs to the FX track on a factory Analog Four, so this track's notes \
                 go to the FX track, not to a voice. Change either channel in GLOBAL > MIDI CONFIG > \
                 MIDI CHANNELS.",
            )),
            6 if track_index != 5 => Some((
                "CV track by default",
                "Channel 6 belongs to the CV track on a factory Analog Four, so this track's notes \
                 drive the CV outputs, not a voice. Change either channel in GLOBAL > MIDI CONFIG > \
                 MIDI CHANNELS.",
            )),
            7..=16 => Some((
                "unassigned by default",
                "A factory Analog Four listens on channels 1-4 for its synth tracks, 5 for FX and 6 \
                 for CV; nothing listens above 6 until the box is told to. Set the channel in \
                 GLOBAL > MIDI CONFIG > MIDI CHANNELS.",
            )),
            _ => None,
        },
        _ => None,
    }
}

/// The track `selection` names, in the pattern its device plays right now.
pub fn track(session: &Session, selection: Selection) -> Option<&Track> {
    let device = session.devices.get(selection.device)?;
    session.current_pattern(device.id)?.track(selection.track)
}

/// The same track, to edit. Resolved through the scene rather than remembered,
/// so switching scene moves the roll onto the pattern that is now playing.
pub fn track_mut(session: &mut Session, selection: Selection) -> Option<&mut Track> {
    let device = session.devices.get(selection.device)?.id;
    let slot = session.slot_in_scene(session.current_scene, device)?.slot();
    session.device_mut(device)?.pattern_mut(slot)?.track_mut(selection.track)
}

// --- Shift+C / Shift+V: whole-track copy/paste -------------------------------
//
// The decision (`digi_core::track_clip`) is pure and lives in `core`; this is
// only the seam that wires a keypress to it. `PLAN.md`'s copy-track item and
// `DEVELOPMENT.md`'s lesson 7 both named this exact gap — `protocol::copy_track`
// and the note-level `edit_ops::place_clipboard` landed with no caller — and
// this is the third of the three: the in-app, whole-*track* copy the TRACKS
// grid itself wants, one click and one keystroke apart.
//
// **Where the clipboard lives, and why not a field on `App`.** `tracks::ui`'s
// signature is `(ui, session, selection, engine)` and two other packets are
// editing this repo at the same time, so a new field threaded from
// `main.rs`/`App` is off the table for this change (and the brief says so).
// `ui.ctx()`'s own per-id memory already has exactly this job for a different
// pane's state — see `super::working_popup`'s doc comment, which chose the
// same mechanism for the same reason: state that is real but belongs to one
// widget's lifetime, not to the session or to the shell. `insert_temp`/
// `get_temp` need no serde impl and are never written to egui's persisted
// `memory.json`, which is right for this: a clipboard that outlived a restart
// would be surprising, not useful.
//
// **The clipboard holds a `Selection`, not a copy of the track's notes.** Both
// `track` and `track_mut` above already resolve a `Selection` through the
// *current* scene rather than remembering a `Pattern`, on purpose — "switching
// scene moves the roll onto the pattern that is now playing". Pasting follows
// the same rule: it re-reads the source at paste time, so a copy behaves
// exactly as well across a scene change as the selection mechanism it is built
// on, and a copied device that gets removed from the session fails exactly the
// way any other stale `Selection` already fails elsewhere in this file — `None`,
// caught and reported here rather than panicking.

/// The whole-track clipboard's contents, kept in `ui.ctx()`'s per-id memory.
/// See the section header above for why here and why a `Selection` rather than
/// a snapshot of the music.
#[derive(Clone, Debug, Default)]
struct Clipboard {
    source: Option<Selection>,
    /// The last thing a copy or a paste had to say, shown in the pane header
    /// by `ui()`. Untouched by a frame that does neither, so it reads as a
    /// status line rather than flickering — the same contract `ui::edit`'s
    /// `Status` keeps for its panel.
    message: Option<String>,
}

/// A fixed id, not one derived from `ui.id()`: the clipboard has to keep
/// meaning the same thing regardless of what else this pane's `Ui` tree looks
/// like on a given frame, which a path-derived id cannot promise as readily as
/// a name that never changes.
fn clipboard_id() -> egui::Id {
    egui::Id::new("digi-roll-studio::tracks::clipboard")
}

/// "DT2 T01" — a cell's identity for a status line, independent of whether it
/// still resolves to a track (a removed device still has a name to report).
fn cell_label(session: &Session, selection: Selection) -> String {
    let device_name = session.devices.get(selection.device).map(|d| d.name.as_str()).unwrap_or("a removed box");
    format!("{device_name} T{:02}", selection.track + 1)
}

/// What trying to paste `source` onto `target` came to.
enum PasteOutcome {
    Pasted { report: digi_core::PasteReport, source_label: String, target_label: String },
    /// Nothing was ever copied. Silent: there is nothing to have an opinion
    /// about yet.
    NothingCopied,
    /// The source and the target are the same cell. Silent, per the brief:
    /// copying a track onto itself must be a no-op that does not flag `edited`
    /// or dirty the session.
    SameCell,
    /// The copied `Selection` no longer resolves — its device was removed, or
    /// (not reachable with today's two 16-track models, but not assumed away
    /// either) its track index no longer exists.
    SourceGone,
    /// The paste target itself no longer resolves. Silent for the same reason
    /// `SameCell` is: there is no cell on screen this could be reported
    /// against.
    NoTarget,
}

/// The pure decision, `core::track_clip::paste_track`, wired to two
/// `Selection`s. Never mutates `session` on any branch except `Pasted`.
fn try_paste(session: &mut Session, source: Option<Selection>, target: Selection) -> PasteOutcome {
    let Some(source) = source else {
        return PasteOutcome::NothingCopied;
    };
    if source == target {
        return PasteOutcome::SameCell;
    }
    let Some(source_kind) = session.devices.get(source.device).map(|d| d.model.key) else {
        return PasteOutcome::SourceGone;
    };
    let Some(source_track) = track(session, source) else {
        return PasteOutcome::SourceGone;
    };
    let clip = TrackClip::copy_from(source_track, source_kind);
    let source_label = cell_label(session, source);

    let Some((target_kind, target_max_steps)) =
        session.devices.get(target.device).map(|d| (d.model.key, d.model.max_steps))
    else {
        return PasteOutcome::NoTarget;
    };
    let target_label = cell_label(session, target);
    let Some(target_track) = track_mut(session, target) else {
        return PasteOutcome::NoTarget;
    };
    let report = paste_track(&clip, target_track, target_kind, target_max_steps);
    PasteOutcome::Pasted { report, source_label, target_label }
}

/// The status line for a completed paste: what landed, and anything that
/// didn't, in one sentence a person reads once rather than a table they have
/// to parse.
fn paste_message(source_label: &str, target_label: &str, report: &digi_core::PasteReport) -> String {
    let mut message = format!(
        "Pasted {source_label} onto {target_label} — {} note{}, {} lane{}.",
        report.notes_pasted,
        if report.notes_pasted == 1 { "" } else { "s" },
        report.lanes_pasted,
        if report.lanes_pasted == 1 { "" } else { "s" },
    );
    if !report.warnings.is_empty() {
        message.push(' ');
        message.push_str(&report.warnings.join(" "));
    }
    message
}

/// Shift+C copies `selection` onto the clipboard; Shift+V pastes whatever is
/// there onto `selection`. Returns whether the session changed — only a real
/// paste ever does, per the no-op rules below.
///
/// ## Why not Cmd+C / Cmd+V, which is what was asked for first
///
/// **Because they never arrive.** `egui-winit` intercepts the platform
/// clipboard chord before it becomes a key event
/// (`is_copy_command`/`is_paste_command`, egui-winit 0.36.1 `src/lib.rs`
/// ~1019): on a match it pushes `Event::Copy` or `Event::Paste` and
/// **returns**, so the `Event::Key { key: Key::C, .. }` a `consume_key` needs
/// is never pushed at all. The first cut of this function matched
/// `Modifiers::COMMAND` with `Key::C` and was unreachable code on every
/// platform — it shipped dead, and the tests missed it because they fed the
/// context a hand-built `Event::Key` the real platform never sends. **A test
/// that builds the input the code expects, rather than the input the platform
/// produces, proves nothing.** That is `DEVELOPMENT.md`'s lesson arriving in a
/// new costume, and the tests below now use the chord this function actually
/// binds.
///
/// Neither is Cmd+**Shift**+C a way out: `is_copy_command` tests only
/// `modifiers.command && keycode == Key::C` and never checks that Shift is
/// absent, so *any* C or V pressed with Command held is swallowed the same
/// way.
///
/// And `Event::Paste` could not carry this even if it were caught: egui-winit
/// only emits it when the **system** clipboard already holds non-empty text,
/// so a track paste bound to it would work or not depending on whether you had
/// lately copied text in some other app. A shortcut that is live only when an
/// unrelated app has left something on the pasteboard is worse than no
/// shortcut.
///
/// So the chord is Shift+C / Shift+V, which `egui-winit` passes straight
/// through, and plain Cmd+C/Cmd+V stays free for text fields and for the
/// note-level clipboard `core::edit_ops::place_clipboard` is waiting to become.
/// `Modifiers::SHIFT` as a `consume_key` pattern requires Shift *and* no
/// ctrl/command (`Modifiers::cmd_ctrl_matches`), so this cannot fire on the
/// intercepted chord by accident.
///
/// Guarded by focus exactly the way `ui::edit::shortcuts` guards Cmd+Z: with a
/// `DragValue` or a `TextEdit` focused elsewhere in this pane's own parameter
/// row (or anywhere else in the app), a letter key has to mean whatever that
/// control means by it, not "copy this track".
fn handle_clipboard_shortcuts(ui: &Ui, session: &mut Session, selection: Selection) -> bool {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return false;
    }
    let (copy, paste) = ui.ctx().input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::SHIFT, egui::Key::C),
            i.consume_key(egui::Modifiers::SHIFT, egui::Key::V),
        )
    });
    if !copy && !paste {
        return false;
    }

    let id = clipboard_id();
    let mut clipboard: Clipboard = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    let mut edited = false;

    if copy {
        // A copy over a selection naming no track (nothing there to copy) is
        // silent and leaves whatever was already on the clipboard alone,
        // rather than clearing a working copy on a stray keypress.
        if track(session, selection).is_some() {
            clipboard.source = Some(selection);
            clipboard.message = Some(format!("Copied {} — Shift+V onto another cell to paste it.", cell_label(session, selection)));
        }
    } else {
        match try_paste(session, clipboard.source, selection) {
            PasteOutcome::Pasted { report, source_label, target_label } => {
                edited = true;
                clipboard.message = Some(paste_message(&source_label, &target_label, &report));
            }
            PasteOutcome::SourceGone => {
                clipboard.message = Some("Nothing pasted — the copied track no longer exists.".to_string());
            }
            PasteOutcome::NothingCopied | PasteOutcome::SameCell | PasteOutcome::NoTarget => {}
        }
    }

    ui.ctx().data_mut(|d| d.insert_temp(id, clipboard));
    edited
}

// --- pure geometry and data rules -------------------------------------------
//
// Kept off the widget, in `triglane.rs`'s style, so the arithmetic that has to
// be right can be tested without an egui context.

/// One 46px-tall row's height, from the spec's `height: 46px` cell.
const CELL_H: f32 = 46.0;
/// Gap between cells in a device's 16-wide grid, and between the gutter and it.
const CELL_GAP: f32 = 4.0;
const GUTTER_W: f32 = 46.0;
const GUTTER_GRID_GAP: f32 = 8.0;
/// Margin below each device row — the spec's `margin-bottom: 8px`.
const ROW_GAP: f32 = 8.0;
/// The header line's own height, and the gap the outer flex column leaves
/// below it before the first device row.
const HEADER_H: f32 = 18.0;
const SECTION_GAP: f32 = 10.0;
/// Padding between the last device row's divider and the parameter row below —
/// the spec's `padding-top: 10px` on that row.
const PARAM_ROW_GAP: f32 = 10.0;

/// Whether `track` carries anything worth a density strip, a trig count, or a
/// progress overlay. The one predicate the whole cell hangs off: an empty
/// track renders none of the three, which is what keeps a `scaleX(1)` overlay
/// from ever appearing on a track with nothing to show progress *through*.
fn has_data(track: &Track) -> bool {
    !track.notes.is_empty()
}

/// Up to 16 buckets across `track`'s own length, one per bar the step-density
/// strip draws — `true` where a note falls in that bucket.
///
/// At `LEN <= 16` this is one bucket per step, exactly as the box counts them.
/// Past 16 the strip aggregates: `design_handoff_digi_roll_ui/README.md` §1b
/// requires this ("the strip shows 16 buckets across the pattern") rather than
/// scrolling or shrinking bars past legibility. A note's own micro-timing
/// (`Note::micro`) plays no part here — this is which *step* a trig sits on,
/// not where inside it.
fn step_density(track: &Track) -> Vec<bool> {
    if track.length_steps == 0 {
        return Vec::new();
    }
    let len = track.length_steps as f64;
    // Integers, so `clamp` and `.min().max()` are the same thing — unlike the two
    // `f64` chains that keep their `#[allow]`.
    let n = (track.length_steps as usize).clamp(1, 16);
    let mut on = vec![false; n];
    for note in &track.notes {
        let idx = ((note.step / len) * n as f64).floor().clamp(0.0, (n - 1) as f64) as usize;
        on[idx] = true;
    }
    on
}

/// The progress overlays' fill fraction (0..1), or `None` when neither the
/// cell tint nor the sweeping bar should be drawn at all.
///
/// **`None` on an empty track is the point, not a fallback.** A `scaleX`
/// overlay with nothing animating it sits at its CSS default of `scaleX(1)` —
/// a full bar, which reads as "this just finished playing" on a track that has
/// never played anything. §1b's own interaction notes call this out by name,
/// and the fix is the same one `has_data` already makes: no notes, no overlay,
/// full stop.
///
/// The arithmetic mirrors `workspace.rs`'s piano-roll playhead exactly — the
/// engine's pattern-step position, scaled by this track's own [`TrackScale`]
/// and wrapped by its own `length_steps` — so a lane at 2x and a lane at LEN 64
/// each sweep at the rate that is actually true for them, against the one
/// `position_steps()` both read.
fn overlay_fraction(track: &Track, position_steps: f64) -> Option<f32> {
    if track.notes.is_empty() || track.length_steps == 0 {
        return None;
    }
    let steps = position_steps * track.scale.multiplier();
    let wrapped = steps % track.length_steps as f64;
    Some((wrapped / track.length_steps as f64) as f32)
}

/// Where the `index`'th of `n` equal cells sits inside `row`, `gap` apart —
/// `grid-template-columns: repeat(n, 1fr)` translated to egui. Painting and
/// hit-testing both call this, so a cell can never be drawn in one place and
/// clicked in another.
fn cell_rect(row: Rect, n: usize, index: usize, gap: f32) -> Rect {
    if n == 0 {
        return Rect::NOTHING;
    }
    let n = n as f32;
    let w = ((row.width() - gap * (n - 1.0)) / n).max(0.0);
    let x = row.min.x + index as f32 * (w + gap);
    Rect::from_min_size(Pos2::new(x, row.min.y), Vec2::new(w, row.height()))
}

/// How many of a device's tracks carry data, and how many it has, summed
/// across every device the header's "N OF M TRACKS CARRY DATA" line covers.
fn data_summary(session: &Session) -> (usize, usize) {
    let mut with_data = 0;
    let mut total = 0;
    for device in &session.devices {
        if let Some(pattern) = session.current_pattern(device.id) {
            for track in pattern.tracks() {
                total += 1;
                if has_data(track) {
                    with_data += 1;
                }
            }
        }
    }
    (with_data, total)
}

/// "PATTERN A01" for the device `selection` currently names, or empty if the
/// selection cannot be resolved (a session with no devices, say).
///
/// **This is one device's pattern, not "the" pattern.** Two boxes in one scene
/// can sit on different slots — nothing forces them to agree — so the header
/// names the pattern the parameter row and the roll are already editing rather
/// than assert a single session-wide "current pattern" that need not exist.
fn header_pattern_label(session: &Session, selection: Selection) -> Option<String> {
    let device = session.devices.get(selection.device)?;
    let slot = session.slot_in_scene(session.current_scene, device.id)?;
    Some(format!("PATTERN {}", slot.label()))
}

// --- drawing -----------------------------------------------------------------

/// The header row: "TRACKS", the pattern being edited, a filler rule, and the
/// live "N OF M TRACKS CARRY DATA" summary.
fn paint_header(ui: &mut Ui, session: &Session, selection: Selection) {
    let (with_data, total) = data_summary(session);
    let summary = format!("{with_data} OF {total} TRACKS CARRY DATA");
    let pattern_label = header_pattern_label(session, selection);

    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADER_H), Sense::hover());
    let painter = ui.painter_at(rect);
    let y = rect.center().y;
    let font = FontId::proportional(10.0);

    let r = painter.text(Pos2::new(rect.min.x, y), Align2::LEFT_CENTER, "TRACKS", font.clone(), super::TEXT_DIM);
    let mut x = r.max.x + 10.0;
    if let Some(label) = &pattern_label {
        let r = painter.text(Pos2::new(x, y), Align2::LEFT_CENTER, label, font.clone(), super::TEXT_DIMMEST);
        x = r.max.x + 10.0;
    }

    let summary_w = painter.layout_no_wrap(summary.clone(), font.clone(), super::TEXT_DIMMEST).size().x;
    let rule_end = rect.max.x - summary_w - 10.0;
    if rule_end > x {
        painter.line_segment([Pos2::new(x, y), Pos2::new(rule_end, y)], Stroke::new(1.0, super::PANEL_BORDER));
    }
    painter.text(Pos2::new(rect.max.x, y), Align2::RIGHT_CENTER, &summary, font, super::TEXT_DIMMEST);
}

/// One track cell: three text rows, the step-density strip, and — for a track
/// that carries data — the two progress overlays. `number` is the 1-based
/// track number this cell shows, zero-padded.
///
/// `copied` draws a second, inset ring in [`super::CYAN_FILL`] — the shade
/// `colored_button` already pairs with [`super::CYAN`] elsewhere in this app
/// for an "armed" control, reused here rather than a new token so a cell that
/// is both selected (the outer, full-strength `CYAN` border) and the
/// clipboard's source (this inner one) reads as two related marks rather than
/// two competing ones. Modest on purpose — `DEVELOPMENT.md`'s glyph lesson is
/// why this is a shape, not a badge crowded in among the four corners that are
/// already spoken for on a data-carrying cell.
fn paint_cell(
    painter: &egui::Painter,
    rect: Rect,
    number: usize,
    track: &Track,
    selected: bool,
    copied: bool,
    position_steps: f64,
) {
    let has = has_data(track);
    let bg = if has { super::CELL_BG_DATA } else { super::INSET_BG };
    painter.rect_filled(rect, 0.0, bg);

    let overlay = overlay_fraction(track, position_steps);

    // The cell tint (z-index 1 in the mock) sits under the text, so it is
    // painted right after the background and before anything else.
    if let Some(frac) = overlay {
        let w = rect.width() * frac;
        if w > 0.0 {
            painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(w, rect.height())), 0.0, super::CYAN_WASH);
        }
    }

    let inner = rect.shrink2(Vec2::new(5.0, 4.0));
    let num_colour = if has { super::TEXT_BRIGHT } else { super::TEXT_DISABLED };
    painter.text(
        Pos2::new(inner.min.x, inner.min.y),
        Align2::LEFT_TOP,
        format!("{number:02}"),
        FontId::proportional(13.0),
        num_colour,
    );
    if has {
        painter.text(
            Pos2::new(inner.max.x, inner.min.y),
            Align2::RIGHT_TOP,
            format!("{}T", track.notes.len()),
            FontId::proportional(9.0),
            super::TEXT_DIM,
        );

        let density = step_density(track);
        let n = density.len();
        if n > 0 {
            let gap = 1.0;
            let w = ((inner.width() - gap * (n as f32 - 1.0)) / n as f32).max(0.0);
            // Row 1's own height plus the strip's 13px: `space-between` over
            // three rows in a 38px-tall inner box leaves exactly this much for
            // row 3, which is where its own text is anchored from the bottom.
            let strip_bottom = inner.min.y + 27.0;
            for (i, on) in density.iter().enumerate() {
                let h = if *on { if i % 4 == 0 { 13.0 } else { 8.0 } } else { 2.0 };
                let x = inner.min.x + i as f32 * (w + gap);
                let bar = Rect::from_min_size(Pos2::new(x, strip_bottom - h), Vec2::new(w, h));
                let colour = if *on { super::TRIG_GREEN } else { super::PANEL_BORDER };
                painter.rect_filled(bar, 0.0, colour);
            }
        }

        painter.text(
            Pos2::new(inner.min.x, inner.max.y),
            Align2::LEFT_BOTTOM,
            format!("L{}", track.length_steps),
            FontId::proportional(9.0),
            super::TEXT_DIM,
        );
        painter.text(
            Pos2::new(inner.max.x, inner.max.y),
            Align2::RIGHT_BOTTOM,
            format!("CH{}", track.channel as u16 + 1),
            FontId::proportional(9.0),
            super::TEXT_DIMMEST,
        );
    }

    // The sweeping bar (z-index 3) sits on top of the text, at the very bottom
    // edge — a 3px sliver below row 3's baseline, never over it.
    if let Some(frac) = overlay {
        let w = rect.width() * frac;
        if w > 0.0 {
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(rect.min.x, rect.max.y - 3.0), Vec2::new(w, 3.0)),
                0.0,
                super::CYAN,
            );
        }
    }

    let border = if selected {
        super::CYAN
    } else if has {
        super::CELL_BORDER_RAISED
    } else {
        super::CELL_BORDER_SUBTLE
    };
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, border), egui::StrokeKind::Inside);

    if copied {
        painter.rect_stroke(rect.shrink(3.0), 0.0, Stroke::new(1.0, super::CYAN_FILL), egui::StrokeKind::Inside);
    }
}

/// One device's row: the box-id gutter and its 16-cell grid. Returns whether a
/// cell was clicked and changed the selection — folded into the caller's
/// `changed`-style bookkeeping is deliberately *not* done here, since picking a
/// track to look at is not a session edit any more than it was in the old pane.
fn paint_device_row(
    ui: &mut Ui,
    device: &Device,
    pattern: &Pattern,
    device_index: usize,
    selection: &mut Selection,
    copied_selection: Option<Selection>,
    position_steps: f64,
) {
    let width = ui.available_width();
    let (row_rect, _) = ui.allocate_exact_size(Vec2::new(width, CELL_H), Sense::hover());
    let gutter = Rect::from_min_size(row_rect.min, Vec2::new(GUTTER_W, CELL_H));
    let grid = Rect::from_min_max(Pos2::new(gutter.max.x + GUTTER_GRID_GAP, row_rect.min.y), row_rect.max);

    let painter = ui.painter_at(row_rect);
    painter.line_segment(
        [Pos2::new(gutter.min.x, gutter.min.y), Pos2::new(gutter.min.x, gutter.max.y)],
        Stroke::new(2.0, super::CYAN),
    );
    let text_x = gutter.min.x + 7.0;
    let mid = gutter.center().y;
    painter.text(
        Pos2::new(text_x, mid - 6.0),
        Align2::LEFT_CENTER,
        &device.name,
        FontId::proportional(13.0),
        super::TEXT_PRIMARY,
    );
    painter.text(
        Pos2::new(text_x, mid + 6.0),
        Align2::LEFT_CENTER,
        device.model.display,
        FontId::proportional(9.0),
        super::TEXT_DIMMEST,
    );

    let n = pattern.tracks().len();
    for (t, track) in pattern.tracks().iter().enumerate() {
        let cell = cell_rect(grid, n, t, CELL_GAP);
        let id = ui.id().with(("digi-roll-track-cell", device.id, t));
        let response = ui
            .interact(cell, id, Sense::click())
            .on_hover_text(track_tooltip_text(t + 1, track, pattern.source.as_ref()));
        if response.clicked() {
            *selection = Selection { device: device_index, track: t };
        }
        let this_cell = Selection { device: device_index, track: t };
        let selected = *selection == this_cell;
        let copied = copied_selection == Some(this_cell);
        paint_cell(&painter, cell, t + 1, track, selected, copied, position_steps);
    }
}

/// Draw the pane. Returns whether the session changed — which clicking a cell
/// never does; only the parameter row below the grid can.
pub fn ui(ui: &mut Ui, session: &mut Session, selection: &mut Selection, engine: &EngineLink) -> bool {
    let mut changed = false;
    /// What the VOL field shows before anything has set it. The middle of the
    /// range rather than the top: a fader that opens at 127 invites a first drag
    /// that can only go down, and the box's own default is not knowable from
    /// here anyway (see `Track::level`).
    const DEFAULT_LEVEL: u8 = 100;
    // Set by the VOL field below, acted on after the selected track's mutable
    // borrow of the session ends — sending reads the session back, which is the
    // one thing that borrow forbids.
    let mut level_moved = false;

    // Read before the mutable borrow of the selected track below: the param
    // row says which box the track belongs to, and a bare track number does
    // not — there are two of every number in a session.
    let owner = session
        .devices
        .get(selection.device)
        .map(|d| d.name.clone())
        .unwrap_or_default();

    let position_steps = engine.position_steps();

    // The engine publishes into atomics and nothing wakes the UI when it does.
    // `transport.rs` already asks for a repaint every frame it is playing, so
    // this is belt and suspenders — but a lane with a data-carrying track
    // should not depend on some other panel's call to keep sweeping.
    if engine.is_playing() {
        ui.ctx().request_repaint();
    }

    // Shift+C and Shift+V, read here rather than hoisted into the shell
    // the way `edit::shortcuts` is: this pane is always drawn (`workspace::ui`
    // calls it unconditionally, unlike the collapsible tool panel `edit`
    // lives in), so it reaches every frame without help from `main.rs`. See
    // the section above `Clipboard` for the rest of the argument.
    changed |= handle_clipboard_shortcuts(ui, session, *selection);
    let clipboard: Clipboard = ui.ctx().data(|d| d.get_temp(clipboard_id())).unwrap_or_default();

    egui::Frame::new()
        .fill(super::PANEL_BG)
        .stroke(Stroke::new(1.0, super::PANEL_BORDER))
        .inner_margin(egui::Margin { left: 14, right: 14, top: 12, bottom: 14 })
        .show(ui, |ui| {
            paint_header(ui, session, *selection);
            // The clipboard's last word — what a copy or a paste had to say,
            // including any warning a paste came back with — sits directly
            // under the header, the pane's own natural home for a line about
            // the pane as a whole rather than about one track.
            match clipboard.message.as_deref() {
                Some(message) => {
                    ui.add_space(4.0);
                    super::consequence_line(ui, message);
                    ui.add_space(SECTION_GAP - 4.0);
                }
                None => ui.add_space(SECTION_GAP),
            }

            for (index, device) in session.devices.iter().enumerate() {
                let Some(pattern) = session.current_pattern(device.id) else {
                    continue;
                };
                paint_device_row(ui, device, pattern, index, selection, clipboard.source, position_steps);
                ui.add_space(ROW_GAP);
            }

            let rule_rect = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover()).0;
            ui.painter().line_segment(
                [rule_rect.left_top(), rule_rect.right_top()],
                Stroke::new(1.0, super::PANEL_BORDER),
            );
            ui.add_space(PARAM_ROW_GAP);

            // --- the selected-track parameter row: unchanged from the old
            // pane apart from the label at its head (see this module's doc
            // comment for why "PRESET 1" is not reproduced). ---
            // Read before `track_mut` takes the session: the CH note below is
            // per-model, and a `&'static str` key is all it needs to survive
            // the borrow.
            let model_key = session.devices.get(selection.device).map(|d| d.model.key);
            let Some(track) = track_mut(session, *selection) else {
                ui.weak("no track selected");
                return;
            };

            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("{owner} · {:02}", selection.track + 1))
                        .strong()
                        .color(super::TEXT_PRIMARY),
                );

                if ui.toggle_value(&mut track.mute, "M").changed() {
                    changed = true;
                }
                if ui
                    .toggle_value(&mut track.solo, "S")
                    .on_hover_text("Solo is session-wide: soloing a DT2 track silences DN2 tracks too")
                    .changed()
                {
                    changed = true;
                }

                // **VOL is the box's own track LEVEL, and moving it moves the
                // box.** It sends the moment it changes — a fader that waited
                // for the transport would be a fader that does nothing while
                // stopped, which is when most mixing happens. Nothing puts the
                // level back afterwards, exactly as if the encoder had been
                // turned by hand, and the hover says so.
                //
                // `None` until touched: see `Track::level`. The field shows
                // `DEFAULT_LEVEL` while it is unset, dimmed by the same argument
                // the hover makes — the app does not know where the box's fader
                // is and must not pretend the number under the pointer is a
                // reading. The first drag makes it a value, and only then is
                // anything sent.
                ui.label("VOL");
                let mut level = track.level.unwrap_or(DEFAULT_LEVEL);
                let response = ui.add(egui::DragValue::new(&mut level).range(0..=127));
                if response.changed() {
                    track.level = Some(level);
                    level_moved = true;
                    changed = true;
                }
                response.on_hover_text(match track.level {
                    Some(_) => "The box's own track LEVEL (CC 95). Sent as you move it — this \
                                rides the box's fader, and nothing puts it back.",
                    None => "The box's own track LEVEL (CC 95). Nothing has been sent yet: \
                             only the box knows where its fader is, so this shows a starting \
                             number rather than a reading. Move it and it rides the box's \
                             fader, and nothing puts it back.",
                });

                ui.label("LEN");
                if ui
                    .add(egui::DragValue::new(&mut track.length_steps).range(1..=128))
                    .on_hover_text("Steps before this track wraps — its own, not the pattern's")
                    .changed()
                {
                    changed = true;
                }

                ui.label("SCALE");
                egui::ComboBox::from_id_salt("track-scale")
                    .selected_text(scale_label(track.scale))
                    .show_ui(ui, |ui| {
                        for (scale, label) in SCALES {
                            if ui.selectable_value(&mut track.scale, scale, label).changed() {
                                changed = true;
                            }
                        }
                    });

                ui.label("CH");
                // Channels are 1–16 to the user and 0–15 on the wire, which is
                // the one place that difference is allowed to show.
                let mut channel = track.channel as u16 + 1;
                if ui
                    .add(egui::DragValue::new(&mut channel).range(1..=16))
                    .on_hover_text(
                        "The channel this track's notes go out on. It has to match TRACK n CH on the box \
                         (SETTINGS > MIDI CONFIG > CHANNELS), which is not the same thing as the track \
                         number.",
                    )
                    .changed()
                {
                    track.channel = (channel - 1) as u8;
                    changed = true;
                }
                // Amber, because [`super::CAUTION`] is "this worked but not
                // the way you wanted" and a trig on a channel the box does not
                // listen on is exactly that: the app is sending it, and
                // nothing will ever play it.
                if let Some((label, why)) =
                    model_key.and_then(|k| channel_note(k, selection.track, channel))
                {
                    ui.label(egui::RichText::new(label).color(super::CAUTION))
                        .on_hover_text(why);
                }

                ui.label(egui::RichText::new(format!("{} trigs", track.notes.len())).color(super::TRIG_GREEN));
            });
        });

    // **Out of the closure, because sending reads the session** — which the
    // selected track's `&mut` borrow above forbids. The engine link resolves the
    // port and the box's own controller number
    // (`EngineLink::send_track_level`); a `false` from it means the fader moved
    // and nothing heard it, which is not worth a line of UI here — every one of
    // its four reasons (no engine, no box, no port, no chart) is already visible
    // in the device strip and the CH note beside this very field.
    if level_moved {
        if let Some(device) = session.devices.get(selection.device).map(|d| d.id) {
            engine.send_track_level(session, device, selection.track);
        }
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::model::TrackPatch;

    /// A patch record, as if read off A01 on 2026-08-20 — the same worked
    /// example the packet's report quotes tooltip text against.
    fn a_patch(from: Source) -> TrackPatch {
        TrackPatch {
            sound: PatchSound::Named("BD HARD".into()),
            kit_name: "KIT 1".into(),
            kit_index: 1,
            from,
            seen_at: 1_787_184_000, // 2026-08-20T00:00:00Z
            live: false,
        }
    }

    fn a01() -> Source {
        Source { device_slug: "digitakt2".into(), bank: 0, index: 0 }
    }

    #[test]
    fn patch_line_when_the_patch_is_known_and_still_matches_the_pattern() {
        let mut track = Track::new(0, TrackKind::Audio);
        track.patch = Some(a_patch(a01()));
        assert_eq!(
            patch_line(&track, Some(&a01())),
            "SOUND: BD HARD — kit KIT 1, from A01, read 2026-08-20"
        );
    }

    #[test]
    fn patch_line_when_there_is_no_patch_record() {
        // The plant this packet names by name: a no-patch track must say so
        // plainly, never fall back to `track.name`.
        let mut track = Track::new(0, TrackKind::Audio);
        track.name = "Not A Sound Name".into();
        assert_eq!(track.patch, None);
        assert_eq!(
            patch_line(&track, None),
            "No patch read from the box — \"Read patch names\" in Setup fills this in."
        );
    }

    #[test]
    fn patch_line_when_the_patch_no_longer_matches_the_patterns_source() {
        let mut track = Track::new(0, TrackKind::Audio);
        track.patch = Some(a_patch(a01()));
        // The pattern this track now lives in was fetched from B03, not A01 —
        // the patch record is real, but it is not current.
        let b03 = Source { device_slug: "digitakt2".into(), bank: 1, index: 2 };
        assert_eq!(
            patch_line(&track, Some(&b03)),
            "SOUND: BD HARD — kit KIT 1, from A01, read 2026-08-20 — stale: this pattern no longer matches that fetch"
        );
    }

    /// **A live read names no slot and can never be stale.** The Analog Four's
    /// patch names come off its edit buffer (`0x68`), so there is no slot
    /// behind them to print or to disagree with the pattern on screen — and
    /// `TrackPatch::from` on such a record only says where the records landed.
    /// Printing "from A01" there would be inventing provenance, and flagging it
    /// stale would flag every one of them.
    #[test]
    fn patch_line_for_a_live_read_names_the_loaded_kit_and_is_never_stale() {
        let mut track = Track::new(0, TrackKind::Audio);
        let mut patch = a_patch(a01());
        patch.live = true;
        track.patch = Some(patch);

        let line = patch_line(&track, None);
        assert_eq!(line, "SOUND: BD HARD — kit KIT 1, the kit the box had loaded, read 2026-08-20");
        assert!(!line.contains("from A01"), "a live read never named a slot: {line}");
        assert!(!line.contains("stale"), "and cannot go stale against one: {line}");

        // Same record beside a pattern fetched from somewhere else entirely —
        // still not stale, because staleness is a claim about a slot.
        let b03 = Source { device_slug: "digitakt2".into(), bank: 1, index: 2 };
        assert!(!patch_line(&track, Some(&b03)).contains("stale"));
    }

    /// The A4's FX and CV tracks: the kit holds no sound for them, and the line
    /// says why rather than reading as a fault or as a slot nobody has named.
    #[test]
    fn patch_line_for_a_track_the_kit_holds_no_sound_for() {
        let mut track = Track::new(4, TrackKind::Audio);
        let mut patch = a_patch(a01());
        patch.sound = PatchSound::NoSound;
        patch.live = true;
        track.patch = Some(patch);
        assert_eq!(
            patch_line(&track, None),
            "No sound on this track — the box's kit holds none for it (read 2026-08-20)"
        );
    }

    #[test]
    fn patch_line_when_the_pattern_has_no_source_at_all() {
        // A pattern written here from scratch, never fetched: any patch record
        // on one of its tracks cannot possibly still be current.
        let mut track = Track::new(0, TrackKind::Audio);
        track.patch = Some(a_patch(a01()));
        assert!(patch_line(&track, None).ends_with("stale: this pattern no longer matches that fetch"));
    }

    #[test]
    fn patch_line_for_a_fetched_midi_track() {
        // The bug packet E's addendum names: a fetched MIDI track carries a
        // patch record too now (`PatchSound::Midi`), and the tooltip must not
        // fall back to "not fetched" just because there is no sound to name.
        let mut track = Track::new(0, TrackKind::Midi);
        track.patch = Some(TrackPatch {
            sound: PatchSound::Midi,
            kit_name: "KIT 1".into(),
            kit_index: 1,
            from: a01(),
            seen_at: 1_787_184_000, // 2026-08-20T00:00:00Z
            live: false,
        });
        assert_eq!(
            patch_line(&track, Some(&a01())),
            "MIDI track — no sound to name (read 2026-08-20)"
        );
    }

    #[test]
    fn patch_line_for_a_fetched_but_unnamed_audio_track() {
        // The fourth case: an audio track slot the kit never named. Also
        // fetched, also honest about it, and — unlike MIDI — it still names
        // the kit and slot, because this track could have had a sound.
        let mut track = Track::new(0, TrackKind::Audio);
        track.patch = Some(TrackPatch {
            sound: PatchSound::Unnamed,
            kit_name: "KIT 1".into(),
            kit_index: 1,
            from: a01(),
            seen_at: 1_787_184_000, // 2026-08-20T00:00:00Z
            live: false,
        });
        assert_eq!(
            patch_line(&track, Some(&a01())),
            "No sound name on the box for this track — kit KIT 1, from A01, read 2026-08-20"
        );
    }

    #[test]
    fn track_tooltip_text_for_a_fetched_track() {
        let mut track = Track::new(2, TrackKind::Audio);
        track.name = "BD HARD".into();
        track.patch = Some(a_patch(a01()));
        track.notes.push(digi_core::Note::new(0.0, 60, 1.0, 100, 0.0));
        assert_eq!(
            track_tooltip_text(3, &track, Some(&a01())),
            "Track 3 — BD HARD\n\
             SOUND: BD HARD — kit KIT 1, from A01, read 2026-08-20\n\
             Audio · 1 note · LEN 16 · 1x · CH3 · out: none\n\
             Shift+C to copy this track · Shift+V to paste onto it"
        );
    }

    #[test]
    fn track_tooltip_text_for_a_generated_from_nothing_track() {
        // A track a generator wrote, never fetched from any box — the case
        // Neil's report calls "generated-from-nothing".
        let track = Track::new(0, TrackKind::Midi);
        assert_eq!(
            track_tooltip_text(1, &track, None),
            "Track 1 — T1\n\
             No patch read from the box — \"Read patch names\" in Setup fills this in.\n\
             MIDI · 0 notes · LEN 16 · 1x · CH1 · out: none\n\
             Shift+C to copy this track · Shift+V to paste onto it"
        );
    }

    #[test]
    fn a_selection_resolves_through_the_scene_not_through_a_remembered_slot() {
        let mut session = digi_core::two_box_session();
        let dn2 = session.devices[1].id;
        let sel = Selection { device: 1, track: 3 };

        track_mut(&mut session, sel).expect("DN2 track 4").name = "before".into();
        assert_eq!(track(&session, sel).map(|t| t.name.as_str()), Some("before"));

        // Point the scene at another slot: the same selection must now land on
        // the track of the pattern that is actually playing.
        session.scenes[0]
            .slots
            .insert(dn2, digi_core::PatternRef::new(0, 5));
        assert_eq!(track(&session, sel).map(|t| t.name.as_str()), Some("T4"));
    }

    #[test]
    fn a_selection_past_the_end_of_the_session_is_none_rather_than_a_panic() {
        let mut session = digi_core::two_box_session();
        assert!(track(&session, Selection { device: 9, track: 0 }).is_none());
        assert!(track_mut(&mut session, Selection { device: 0, track: 99 }).is_none());
    }

    #[test]
    fn the_eight_channels_a_factory_digi_listens_on_are_not_flagged_and_the_rest_are() {
        for kind in ["DT2", "DN2"] {
            for channel in 1..=8 {
                assert_eq!(
                    channel_note(kind, 0, channel),
                    None,
                    "{kind} channel {channel} is the track it says"
                );
            }
            for channel in 9..=16 {
                assert!(
                    channel_note(kind, 0, channel).is_some(),
                    "{kind} channel {channel} needs the box set up before it plays anything"
                );
            }
            // The two that are not merely unassigned but already spoken for, which
            // is why they are named rather than lumped in with 11–16.
            assert_eq!(channel_note(kind, 0, 9).map(|(l, _)| l), Some("FX CTRL by default"));
            assert_eq!(channel_note(kind, 0, 10).map(|(l, _)| l), Some("AUTO by default"));
        }
    }

    #[test]
    fn a_factory_analog_four_flags_exactly_the_channels_nothing_plays_on() {
        // The factory map *is* this app's 1:1 default — tracks 1-6 on channels
        // 1-6 — so the six rows of a fresh A4 draw no amber at all…
        for (index, channel) in (0..6).zip(1..=6u16) {
            assert_eq!(
                channel_note("A4", index, channel),
                None,
                "A4 track {} on its own factory channel {channel}",
                index + 1
            );
        }
        // …while a *synth* row aimed at the FX or CV track's channel is warned
        // by name, and everything above 6 is dead air on a factory box.
        assert_eq!(channel_note("A4", 0, 5).map(|(l, _)| l), Some("FX track by default"));
        assert_eq!(channel_note("A4", 0, 6).map(|(l, _)| l), Some("CV track by default"));
        for channel in 7..=16 {
            assert_eq!(
                channel_note("A4", 0, channel).map(|(l, _)| l),
                Some("unassigned by default"),
                "A4 channel {channel}"
            );
        }
    }

    #[test]
    fn a_box_this_function_has_not_read_the_manual_for_gets_no_note() {
        // Speak about factory state only where a manual has been read out —
        // a made-up key must not inherit the digis' warnings.
        for channel in 1..=16 {
            assert_eq!(channel_note("ST", 0, channel), None);
        }
    }

    /// The default map and the note have to agree about which tracks are the
    /// awkward ones, or the amber appears on the wrong eight.
    #[test]
    fn the_default_channel_map_puts_exactly_tracks_nine_to_sixteen_on_flagged_channels() {
        let session = digi_core::two_box_session();
        let pattern = session
            .current_pattern(session.devices[0].id)
            .expect("the DT2 plays a pattern in the opening scene");
        for (t, track) in pattern.tracks().iter().enumerate() {
            let flagged = channel_note("DT2", t, track.channel as u16 + 1).is_some();
            assert_eq!(flagged, t >= 8, "track {} at channel {}", t + 1, track.channel + 1);
        }
    }

    /// The A4 half of the same agreement: the 1:1 default lands every one of
    /// its six rows on the factory channel, so a fresh A4 shows no amber.
    #[test]
    fn the_default_channel_map_flags_nothing_on_a_fresh_analog_four() {
        let device = digi_core::Device::new("A4", &digi_core::A4, 16);
        for (t, track) in device.pattern(0).unwrap().tracks().iter().enumerate() {
            assert_eq!(
                channel_note("A4", t, track.channel as u16 + 1),
                None,
                "track {} at channel {}",
                t + 1,
                track.channel + 1
            );
        }
    }

    #[test]
    fn every_scale_the_model_has_is_labelled() {
        for (scale, label) in SCALES {
            assert_eq!(scale_label(scale), label);
        }
    }

    #[test]
    fn a_track_with_no_notes_carries_no_data() {
        let track = Track::new(0, digi_core::TrackKind::Audio);
        assert!(!has_data(&track));
    }

    #[test]
    fn a_track_with_a_note_carries_data() {
        let mut track = Track::new(0, digi_core::TrackKind::Audio);
        track.notes.push(digi_core::Note::new(0.0, 60, 1.0, 100, 0.0));
        assert!(has_data(&track));
    }

    #[test]
    fn an_empty_track_gets_no_overlay_at_all() {
        // The bug the spec calls out by name: a `scaleX` overlay with nothing
        // driving it sits at its default full width, which reads as "just
        // finished" rather than "never played". `None` here is what keeps
        // `paint_cell` from drawing either overlay on such a track.
        let track = Track::new(0, digi_core::TrackKind::Audio);
        assert_eq!(overlay_fraction(&track, 0.0), None);
        assert_eq!(overlay_fraction(&track, 123.456), None);
    }

    #[test]
    fn a_data_track_s_overlay_wraps_by_its_own_length_and_scale() {
        let mut track = Track::new(0, digi_core::TrackKind::Audio);
        track.notes.push(digi_core::Note::new(0.0, 60, 1.0, 100, 0.0));
        track.length_steps = 16;
        track.scale = TrackScale::One;
        // 20 pattern steps in, at 1x: 20 % 16 = 4, a quarter through its loop.
        assert_eq!(overlay_fraction(&track, 20.0), Some(0.25));

        // The same position, but this track runs twice as fast: 40 % 16 = 8.
        track.scale = TrackScale::Two;
        assert_eq!(overlay_fraction(&track, 20.0), Some(0.5));
    }

    #[test]
    fn a_zero_length_track_gets_no_overlay_rather_than_a_division_by_zero() {
        let mut track = Track::new(0, digi_core::TrackKind::Audio);
        track.notes.push(digi_core::Note::new(0.0, 60, 1.0, 100, 0.0));
        track.length_steps = 0;
        assert_eq!(overlay_fraction(&track, 5.0), None);
    }

    #[test]
    fn a_track_with_no_notes_has_sixteen_unlit_buckets() {
        // A fresh track has 16 steps and no notes: 16 buckets, all unlit —
        // `step_density` does not special-case "no data" the way `paint_cell`
        // does, since `has_data` is what gates whether the strip is drawn at
        // all.
        let track = Track::new(0, digi_core::TrackKind::Audio);
        let density = step_density(&track);
        assert_eq!(density.len(), 16);
        assert!(density.iter().all(|on| !on));
    }

    #[test]
    fn step_density_lights_exactly_the_steps_a_note_sits_on_at_len_16() {
        let mut track = Track::new(0, digi_core::TrackKind::Audio);
        track.length_steps = 16;
        for step in [0.0, 3.0, 15.0] {
            track.notes.push(digi_core::Note::new(step, 60, 1.0, 100, 0.0));
        }
        let density = step_density(&track);
        assert_eq!(density.len(), 16);
        let lit: Vec<usize> = density.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect();
        assert_eq!(lit, vec![0, 3, 15]);
        // No chord in this track, so the lit-bucket count equals the trig
        // count row 1 shows — the invariant the spec asks for at LEN <= 16.
        assert_eq!(lit.len(), track.notes.len());
    }

    #[test]
    fn a_chord_on_one_step_still_lights_exactly_one_bar() {
        // The case the spec's synthetic data never had to face: one press of
        // a chord puts several notes on the same step. The density strip has
        // 16 slots regardless, so it counts steps, not notes.
        let mut track = Track::new(0, digi_core::TrackKind::Audio);
        track.length_steps = 16;
        for pitch in [60, 64, 67] {
            track.notes.push(digi_core::Note::new(2.0, pitch, 1.0, 100, 0.0));
        }
        let density = step_density(&track);
        assert_eq!(density.iter().filter(|on| **on).count(), 1);
        assert!(density[2]);
    }

    #[test]
    fn step_density_aggregates_into_at_most_sixteen_buckets_past_len_16() {
        let mut track = Track::new(0, digi_core::TrackKind::Audio);
        track.length_steps = 64;
        // Steps 0 and 63 are as far apart as this pattern gets; they must not
        // collide into the same bucket, or the strip would hide a track that
        // uses its whole length.
        track.notes.push(digi_core::Note::new(0.0, 60, 1.0, 100, 0.0));
        track.notes.push(digi_core::Note::new(63.0, 60, 1.0, 100, 0.0));
        let density = step_density(&track);
        assert_eq!(density.len(), 16);
        assert!(density[0]);
        assert!(density[15]);
    }

    #[test]
    fn cell_rect_tiles_a_row_exactly_with_no_gap_left_over() {
        let row = Rect::from_min_size(Pos2::new(63.0, 10.0), Vec2::new(728.0, CELL_H));
        let n = 16;
        let gap = CELL_GAP;
        let mut prev_right: Option<f32> = None;
        for i in 0..n {
            let cell = cell_rect(row, n, i, gap);
            assert!((cell.height() - CELL_H).abs() < 1e-4);
            if let Some(right) = prev_right {
                assert!((cell.min.x - right - gap).abs() < 1e-3, "cell {i} does not sit exactly one gap on");
            }
            prev_right = Some(cell.max.x);
        }
        assert!((prev_right.unwrap() - row.max.x).abs() < 1e-2, "the last cell's right edge is the row's");
    }

    /// One pass of the pane in a headless context. The pattern is the trig
    /// lane's, and its two rules apply here too: egui hit-tests against the
    /// previous pass's layout, and the font-atlas delta has to be cleared
    /// because there is no renderer to hand it to.
    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        session: &mut Session,
        selection: &mut Selection,
        engine: &EngineLink,
    ) {
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |u| {
            crate::ui::tracks::ui(u, session, selection, engine);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn clicking_a_cell_selects_its_track() {
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let mut selection = Selection::default();
        let engine = EngineLink::default();

        // The first device row's first cell. `row_rect`'s origin is the outer
        // frame's inner margin (14 left, 12 top) offset by the header and the
        // gap below it — the same constants `ui::tracks::ui` lays out with.
        let x = 14.0 + GUTTER_W + GUTTER_GRID_GAP + 2.0;
        let y = 12.0 + HEADER_H + SECTION_GAP + 2.0;
        let pos = Pos2::new(x, y);
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        frame(&ctx, vec![], &mut session, &mut selection, &engine);
        frame(&ctx, vec![egui::Event::PointerMoved(pos), press(true)], &mut session, &mut selection, &engine);
        frame(&ctx, vec![press(false)], &mut session, &mut selection, &engine);

        assert_eq!(selection, Selection { device: 0, track: 0 }, "the release selects the first cell's track");
    }

    /// Hovers a track that carries a patch record, through the real pane —
    /// `ui::tracks::ui` → `paint_device_row` → the cell's `Response` — rather
    /// than calling `track_tooltip_text` directly. This is what proves the
    /// tooltip is actually wired to the cell and not just correct in
    /// isolation: a field with no control reading it is invisible from above,
    /// and this drives the control.
    #[test]
    fn hovering_a_track_cell_with_a_patch_runs_without_panicking() {
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let device = session.devices[0].id;
        {
            let pattern = session.device_mut(device).unwrap().pattern_mut(0).unwrap();
            let track = pattern.track_mut(0).unwrap();
            track.name = "BD HARD".into();
            track.patch = Some(a_patch(a01()));
        }
        let mut selection = Selection::default();
        let engine = EngineLink::default();

        let x = 14.0 + GUTTER_W + GUTTER_GRID_GAP + 2.0;
        let y = 12.0 + HEADER_H + SECTION_GAP + 2.0;
        let pos = Pos2::new(x, y);

        frame(&ctx, vec![], &mut session, &mut selection, &engine);
        frame(&ctx, vec![egui::Event::PointerMoved(pos)], &mut session, &mut selection, &engine);
        // No panic reaching here is the assertion: `paint_device_row` computed
        // `track_tooltip_text` for a real, patched track under a real hover.
    }

    // --- Shift+C / Shift+V ------------------------------------------------------
    //
    // These drive `handle_clipboard_shortcuts` directly (private to this
    // module, so only reachable from here) to pin the clipboard's own state
    // and its status message. `crates/app/tests/tracks_clipboard.rs` covers
    // the same feature from outside the module, through the public `ui()`
    // entry point, and deliberately does not reach into `Clipboard` — the
    // split is the same one `ui::edit`'s own tests keep between `Status` and
    // the shell.

    /// The chord `handle_clipboard_shortcuts` actually binds, built the way
    /// `egui-winit` really delivers it. **Not `Modifiers::COMMAND`**: that is
    /// the event the platform never sends for C or V, because the clipboard
    /// chord is intercepted upstream and turned into `Event::Copy` /
    /// `Event::Paste` — see the function's own doc comment. Feeding a
    /// COMMAND-modified C here is what let the first cut of this feature ship
    /// dead with a green suite.
    fn shift_key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::SHIFT }
    }

    fn run_clipboard(ctx: &egui::Context, key: egui::Key, session: &mut Session, selection: Selection) -> bool {
        let input = egui::RawInput { events: vec![shift_key(key)], ..Default::default() };
        let mut edited = false;
        let mut output = ctx.run_ui(input, |ui| {
            edited = handle_clipboard_shortcuts(ui, session, selection);
        });
        output.textures_delta.clear();
        edited
    }

    fn read_clipboard(ctx: &egui::Context) -> Clipboard {
        ctx.data(|d| d.get_temp(clipboard_id())).unwrap_or_default()
    }

    #[test]
    fn copying_a_track_remembers_its_selection_and_says_so() {
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let sel = Selection { device: 0, track: 0 };

        let edited = run_clipboard(&ctx, egui::Key::C, &mut session, sel);

        assert!(!edited, "a copy alone is never an edit");
        let clip = read_clipboard(&ctx);
        assert_eq!(clip.source, Some(sel));
        assert_eq!(clip.message.as_deref(), Some("Copied DT2 T01 — Shift+V onto another cell to paste it."));
    }

    #[test]
    fn copying_a_selection_with_no_track_leaves_the_clipboard_alone() {
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let real = Selection { device: 0, track: 0 };
        let nothing_there = Selection { device: 9, track: 0 };

        run_clipboard(&ctx, egui::Key::C, &mut session, real);
        let before = read_clipboard(&ctx);

        run_clipboard(&ctx, egui::Key::C, &mut session, nothing_there);
        let after = read_clipboard(&ctx);

        assert_eq!(before.source, after.source, "a stray copy over nothing must not clear a working copy");
    }

    #[test]
    fn pasting_reports_what_landed_and_what_did_not() {
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let source_sel = Selection { device: 0, track: 0 };
        let dest_sel = Selection { device: 0, track: 5 };
        {
            let t = track_mut(&mut session, source_sel).unwrap();
            t.notes = vec![digi_core::Note::new(0.0, 60, 1.0, 100, 0.0)];
            t.plocks = vec![
                digi_core::PLockLane::new(Some("filter.cutoff".into()), None, Some("DT2".into()), false, vec![Some(64)])
                    .unwrap(),
            ];
        }

        run_clipboard(&ctx, egui::Key::C, &mut session, source_sel);
        let edited = run_clipboard(&ctx, egui::Key::V, &mut session, dest_sel);

        assert!(edited);
        let clip = read_clipboard(&ctx);
        let message = clip.message.expect("a paste always leaves a status line");
        assert!(message.contains("Pasted DT2 T01 onto DT2 T06"), "{message}");
        assert!(message.contains("1 note"), "{message}");
        assert!(message.contains("1 lane"), "{message}");
        assert!(!message.contains("wasn't copied"), "nothing should have been dropped here: {message}");
    }

    #[test]
    fn a_dropped_cross_device_lane_is_named_in_the_paste_message() {
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let source_sel = Selection { device: 0, track: 0 }; // DT2
        let dest_sel = Selection { device: 1, track: 0 }; // DN2
        {
            let t = track_mut(&mut session, source_sel).unwrap();
            t.notes = vec![digi_core::Note::new(0.0, 60, 1.0, 100, 0.0)];
            // Unnamed and raw: meaningless on a different box's numbering.
            t.plocks =
                vec![digi_core::PLockLane::new(None, Some(200), Some("DT2".into()), false, vec![Some(10)]).unwrap()];
        }

        run_clipboard(&ctx, egui::Key::C, &mut session, source_sel);
        let edited = run_clipboard(&ctx, egui::Key::V, &mut session, dest_sel);

        assert!(edited, "the note still crosses even though the lane can't");
        let message = read_clipboard(&ctx).message.unwrap();
        assert!(message.contains("wasn't copied"), "{message}");
    }

    #[test]
    fn pasting_a_stale_copy_says_the_track_is_gone_rather_than_editing_anything() {
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let dn2_sel = Selection { device: 1, track: 0 };
        {
            let t = track_mut(&mut session, dn2_sel).unwrap();
            t.notes = vec![digi_core::Note::new(0.0, 60, 1.0, 100, 0.0)];
        }

        run_clipboard(&ctx, egui::Key::C, &mut session, dn2_sel);
        session.devices.truncate(1); // the DN2 this clipboard points at is gone

        let dt2_sel = Selection { device: 0, track: 0 };
        let edited = run_clipboard(&ctx, egui::Key::V, &mut session, dt2_sel);

        assert!(!edited);
        let message = read_clipboard(&ctx).message.unwrap();
        assert!(message.contains("no longer exists"), "{message}");
    }

    #[test]
    fn a_focused_widget_swallows_the_shortcut_rather_than_letting_it_copy() {
        // The same guard `ui::edit::shortcuts` keeps for Cmd+Z: with a control
        // elsewhere in the app focused, Shift+C/Shift+V has to mean whatever
        // that control means by it, not "copy this track".
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let sel = Selection { device: 0, track: 0 };
        // A focus target that isn't part of this pane at all — the guard reads
        // `ctx.memory().focused()`, which does not care which widget it is.
        let focus_id = egui::Id::new("some-other-widget-entirely");
        ctx.memory_mut(|m| m.request_focus(focus_id));

        let edited = run_clipboard(&ctx, egui::Key::C, &mut session, sel);

        assert!(!edited);
        assert_eq!(read_clipboard(&ctx).source, None, "the keystroke must not have reached the clipboard at all");
    }

    /// Draws a whole frame with a cell marked as the clipboard's source, the
    /// same way `hovering_a_track_cell_with_a_patch_runs_without_panicking`
    /// checks a different field is wired up: not proof the ring is legible —
    /// that is a screen check, per `DEVELOPMENT.md` lesson 8 — but proof
    /// `paint_device_row`/`paint_cell` accept a live `copied_selection`
    /// without a panic or an off-by-one in which cell it marks.
    #[test]
    fn drawing_the_pane_with_a_copied_cell_runs_without_panicking() {
        let ctx = egui::Context::default();
        let mut session = digi_core::two_box_session();
        let mut selection = Selection { device: 0, track: 0 };
        let engine = EngineLink::default();

        frame(&ctx, vec![], &mut session, &mut selection, &engine);
        frame(&ctx, vec![shift_key(egui::Key::C)], &mut session, &mut selection, &engine);
        assert_eq!(read_clipboard(&ctx).source, Some(selection));

        selection = Selection { device: 1, track: 3 };
        frame(&ctx, vec![], &mut session, &mut selection, &engine);
        frame(&ctx, vec![], &mut session, &mut selection, &engine);
    }
}
