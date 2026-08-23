// The piano roll.
//
// PLAN.md §5 has this rewritten against egui's `interact` model rather than
// extended, and Phase 5 has now taken the first half of that: the widget
// allocates its rect *first* and reads its gestures off the `Response`, where the
// prototype allocated last and hit-tested against raw pointer input. That was
// survivable while it edited a copy of a track. It is not survivable now that it
// edits the session: with a global pointer test, clicking a button in the ports
// panel also created a note in the roll, off the right-hand edge of the pattern.
//
// What is still to come, and is deliberately not here: rubber-band selection,
// the p-lock strip, and ghosted neighbouring tracks. The pitch rows are drawn
// the way `js/pianoroll.js` labels them now — C2–C8, with a key column in the
// gutter, rather than all 128 rows above an empty strip.
//
// The trig lane is here too — the roll carves [`triglane::LANE_H`] off the bottom of its rect
// and hands the lane a [`triglane::Cols`] built from its own `Grid`, so the two
// surfaces cannot disagree about where a step is.
//
// The roll owns no data. It is handed the track the app has selected and reports
// whether it changed anything, so the app can push a snapshot to the engine —
// which is what makes a note you draw sound on the next pass of the loop.
//
// **Every gesture at the top of `js/pianoroll.js` is now here.** Phase 9 took the
// last four; the list below is the whole of it.
//
//   click an empty cell     -> create a note
//   drag on an empty cell   -> create it and set its length in one gesture
//   click a note            -> select it alone
//   shift+click a note      -> toggle it in or out of the selection
//   alt+click a note        -> delete it
//   cmd/ctrl+drag on empty  -> marquee select
//   cmd/ctrl+click on empty -> clear the selection
//   drag a note body        -> move it, and the rest of the selection with it
//   shift+drag a note body  -> velocity, one delta over the whole selection
//   cmd/ctrl+drag a body    -> micro-timing, on that note alone
//   alt+drag a note         -> duplicate the selection and drag the copies
//   drag a note's right edge-> resize it, and the selection by the same delta
//   shift + drag that edge  -> fine resize, snapped to what the box can store
//   right-click a note      -> delete it
//   Delete / Backspace      -> delete every selected note
//
// ## The wheel, which is none of the above
//
// The JS roll has no wheel gestures at all — it lives in a scrolling `div` and
// lets the browser do it — so these three are this app's own, and they share one
// input:
//
//   wheel                   -> scroll the rows; shift for the columns
//   alt+wheel (chord draw)   -> cycle the chord inversion, aiming under the ghost
//   cmd/ctrl+wheel or pinch  -> zoom, holding the cell under the pointer still
//
// They are tried in that order of specificity in `ui`, and each one that spends
// the frame's delta stops the next from spending it again. `zoom` had been a
// field the grid multiplied by since the roll shipped, with nothing in the app
// able to move it off 1.0.
//
// ## Two places this deliberately parts from the JS's ordering
//
// **The right edge always resizes.** `js/pianoroll.js` tests its modifiers before
// `nearEdge`, so in the browser shift on the edge is velocity and cmd on the edge
// is micro-timing — the seven-pixel resize zone quietly means three different
// things depending on which key is down. PLAN.md §9 settled the shift half of that
// the other way ("shift is already the fine resize modifier on a note's right
// edge, and would now be velocity on its body — the zones are already
// separated"), and cmd follows shift for the same reason: a zone this small has to
// mean one thing. Fine resize is still reached by holding shift *during* a
// resize, which is how the JS actually delivers it too — its `resize` handler
// reads `e.shiftKey` per mousemove, not at press.
//
// **Alt owns the whole note**, edge included, because alt has no edge meaning to
// collide with.
//
// The undecided-until-it-moves bargain that `js/pianoroll.js` builds by hand — a
// `DRAG_PX` threshold, a pending `'shift'` or `'alt'` mode, and a `_up` that
// works out which half happened — is not ported, because egui already draws that
// line: a press that crosses egui's own drag threshold arrives as
// `drag_started()` and one that does not arrives as `clicked()`, never both. So
// shift-click-to-toggle and shift-drag-for-velocity read off two different
// handlers rather than one mode that has to guess.

use std::collections::BTreeSet;

use digi_core::chords::{chord_for_cell, ChordNote, Harmony, Row};
use digi_core::edit_ops::{
    adopt_step_trig, clamp_micro, clamp_velocity, nudge_velocities, resize_selection_by, LenEntry,
    PLockShift,
    ResizeOpts,
};
use digi_core::lengths::snap_len_fine;
use digi_core::{Note, Track};
use egui::{Color32, Pos2, Rect, Ui, Vec2};

use crate::ui::plocklane::{self, PLockStrip};
use crate::ui::triglane::{self, TrigLane};

const COLS: usize = 256; // 2 bars @ 128 steps

/// The rows the roll draws by default, as `js/pianoroll.js` draws them: C2 up to
/// C8, seventy-three of them, rather than the prototype's 128. Five of the
/// octaves it drew hold nothing either box makes a sound at, and they were five
/// octaves of scrolling between a kick and a hi-hat.
const PITCH_MIN: u8 = 24; // C2, as the box labels it
const PITCH_MAX: u8 = 96; // C8, as the box labels it

const NAMES: [&str; 12] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// `js/pianoroll.js`'s `noteName`, and its octave numbering is the point of it:
/// **MIDI 60 is C5 here, not C4.** An Elektron shows 60 as C5, and the roll's job
/// is to tell you which note you will see on the box after a write — a key
/// column that disagrees with the box's own screen by a whole octave defeats it.
pub fn note_name(pitch: u8) -> String {
    format!("{}{}", NAMES[pitch as usize % 12], pitch / 12)
}

/// The five black keys of every octave, by pitch class — the JS's `BLACK` set.
fn is_black(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

/// The rows this roll is currently drawing: C2–C8, widened to hold anything the
/// track carries outside it.
///
/// **The JS needs no such widening and this is not a deviation from it.** Its
/// roll can only create notes inside the band and clamps every drag to the same
/// bounds, so a pitch outside C2–C8 cannot exist in it. One can exist here:
/// `protocol::track_notes` reads a trig's pitch as `sl.note & 0x7f`, so a pattern
/// fetched off a box arrives holding any of the 128 MIDI pitches — and since
/// `core::import` landed there is a path for one to. A fixed band would draw that
/// trig nowhere while the engine went on playing it, which is the roll lying
/// about what the pattern contains. So the band is the union of the two.
///
/// It only ever grows by import or load: both gestures that make a pitch are
/// bounded by the band they are made in.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Band {
    lo: u8,
    hi: u8,
}

impl Band {
    fn for_track(track: &Track) -> Self {
        track.notes.iter().fold(Self { lo: PITCH_MIN, hi: PITCH_MAX }, |b, n| Self {
            lo: b.lo.min(n.pitch),
            hi: b.hi.max(n.pitch),
        })
    }

    fn rows(&self) -> usize {
        (self.hi - self.lo) as usize + 1
    }

    /// The band as a pitch range, which is what the chord math clamps a voicing
    /// to: a chord tone the roll cannot draw is a note nobody could then edit.
    fn range(&self) -> (u8, u8) {
        (self.lo, self.hi)
    }

    fn contains(&self, pitch: u8) -> bool {
        (self.lo..=self.hi).contains(&pitch)
    }
}

/// The gutter left of step 0, shared with the trig lane — `js/pianoroll.js`
/// exports the same constant for the same reason. The roll draws its key column
/// in it and the lane labels its rows in the same strip below. A press in it is
/// on a key, never on step 0.
pub const KEY_W: f32 = 52.0;

/// The pitch drawn at the top of the roll before anyone scrolls. High enough to
/// keep a bass line on screen, low enough that the useful range is not off the
/// top — the prototype opened on pitch 127 and every note drawn by hand landed
/// somewhere no box has a sound. It is C7 in the numbering the key column uses.
const TOP_PITCH: u8 = 84;

const CELL_W: f32 = 20.0;
const CELL_H: f32 = 12.0;

/// How far the roll zooms, as a multiple of [`CELL_W`] and [`CELL_H`]: half-size
/// 10x6 cells at the bottom, quadruple 80x48 ones at the top.
///
/// **Both ends are a look rather than an argument, and `PLAN.md` §9 has them.**
/// The floor is where a note's rect (0.8 of a row) and the one-pixel floor under
/// its velocity bar have to share five pixels — whether the brightness ramp is
/// still readable there is `DEVELOPMENT.md` lesson 8's kind of question and no
/// test here can answer it. The ceiling is a bar and a half across a laptop,
/// which is as far in as a micro-timing nudge needs to be visible and no
/// further.
///
/// Public because the Edit panel's VIEW slider is the control that shows the
/// number, and a slider whose range disagreed with [`PianoRoll::set_zoom`]'s
/// clamp would be two statements of one rule — `DEVELOPMENT.md` lesson 5.
pub const ZOOM_MIN: f32 = 0.5;
/// The other end of [`ZOOM_MIN`]'s range.
pub const ZOOM_MAX: f32 = 4.0;

/// How wide the right-edge grab zone is, from `js/pianoroll.js`. It is invisible
/// target, so the roll switches the cursor over it — a seven-pixel gesture
/// nothing announces is a gesture nobody finds.
///
/// A note shorter than this is *all* edge, so it can be resized and not moved.
/// That is the JS's behaviour too (its `nearEdge` window is measured back from
/// the note's true end, which for a 1/8-step note is inside the note), and it is
/// the right way round: a note that short is one you are trying to lengthen.
const EDGE_PX: f32 = 7.0;

/// What the frame's wheel did, which is three answers rather than two: whether the
/// roll may scroll, *and* whether anything changed.
///
/// Folding the last two together is a small bug with a long tail — a trackpad flick
/// part-way to its next notch takes the wheel without moving a field, and reporting
/// that as a change would mark the session unsaved and re-snapshot the engine for
/// holding alt and twitching a finger.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wheel {
    /// Not chord draw, not alt, or not over the grid: the roll scrolls as usual.
    Ignored,
    /// Spent on aiming, but not enough of it to move the inversion yet.
    Aimed,
    Cycled,
}

/// How many points of trackpad scrolling spend one step of the chord inversion.
/// A mouse wheel does not come through here at all — it arrives as discrete
/// notches; see [`PianoRoll::wheel_inversion`]. Roughly a comfortable flick,
/// chosen so a cycle of four takes a deliberate sweep rather than a twitch.
const TRACKPAD_NOTCH: f32 = 40.0;

pub struct PianoRoll {
    /// The cell size, as a multiple of [`CELL_W`]/[`CELL_H`], between
    /// [`ZOOM_MIN`] and [`ZOOM_MAX`].
    ///
    /// **It was `pub`, and nothing in the app ever wrote it** — the grid has
    /// multiplied by it since the roll shipped, always by 1.0. That is
    /// `DEVELOPMENT.md` lesson 7's second half, the one `Note::velocity` was in
    /// until Phase 9: a field with no control is as invisible from above as a
    /// function with no caller, and harder to notice because every test passes a
    /// value in.
    ///
    /// **Private now, so the clamp cannot be somebody else's job.** Phase 9's
    /// velocity slider is the reason — it clamped what it *drew* and not what it
    /// stored, and a zoom is the worse version of that bug: [`Grid::step_at`]
    /// divides by `cell.x`, so a zero would turn every hit test in this file
    /// into an infinity. Written through [`PianoRoll::set_zoom`], which clamps,
    /// or by [`PianoRoll::zoom_by`], which clamps.
    zoom: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    /// The selected trigs, by note id — the JS's `this.selected`, which is a
    /// `Set` for the same reason this is a set. It was one `Option<u32>` until
    /// the marquee landed, and that single id was what made the trig lane's
    /// selection-wide edit unreachable: the lane has taken a `&[u32]` since it
    /// shipped, and the roll never had more than one to give it.
    ///
    /// Ordered rather than hashed so a test can assert on it without sorting.
    selected: BTreeSet<u32>,
    dragging: Option<Drag>,
    lane: TrigLane,
    strip: PLockStrip,
    /// The velocity a note drawn by hand gets, and the number the Edit panel's
    /// slider shows.
    ///
    /// **It lives here rather than in the panel** because both ends move it: the
    /// slider sets it, and touching a note in the roll adopts that note's
    /// velocity — which is `js/main.js`'s `onSelect` writing `state.defaultVelocity
    /// = note.velocity` so the slider mirrors what your hand is on. One field, so
    /// the two cannot disagree; the panel reads and writes it through
    /// [`PianoRoll::default_velocity`].
    ///
    /// Every note this app has ever written to a box went out at 100 because
    /// nothing could set this. That is what Phase 9 was for.
    default_velocity: u8,
    /// The last thing a velocity or micro-timing drag did, for the readout drawn
    /// over the note. Cleared when the drag ends: it is a live annotation, not a
    /// label.
    readout: Option<(u32, String)>,
    /// Where the pointer last settled, and when — the dwell clock for the
    /// hover box (A2). `None` whenever the pointer is not over the roll at
    /// all or a drag is in progress, so leaving and coming back — or a drag
    /// starting and ending — restarts the dwell rather than carrying over
    /// stale settle time.
    hover_still: Option<(Pos2, f64)>,
    /// What the hover box is showing, once the dwell in [`Self::update_hover`]
    /// is satisfied. Held as a field, like `readout`, so `interact` — where the
    /// dwell clock, the drag-suppression rule and the hit test all already
    /// live — is the only place that decides it; `paint_hover_box` only reads
    /// it back.
    hover_box: Option<(u32, Vec<String>)>,
    /// Scroll points spent by alt+wheel since the last inversion step. Only a
    /// trackpad puts anything here; see [`PianoRoll::wheel_inversion`].
    wheel: f32,
}

/// What a press turned into. The JS keeps the same decision in `drag.mode`, and
/// since Phase 9 every one of its modes has a variant here — with the exception
/// of the two *pending* ones (`'shift'` and `'alt'`), which egui's own drag
/// threshold makes unnecessary; see the header.
#[derive(Clone, PartialEq, Debug)]
enum Drag {
    /// Moving the selection as one body. `anchor` is where the button went down.
    Move {
        anchor: Pos2,
        group: Vec<Grabbed>,
        /// The p-lock lanes as the press found them, so the locks ride along
        /// with the trigs. Captured here for the same reason `group` is: a lock
        /// has to be placed from where the gesture *began*, or a drag held
        /// still would shift it once per frame. See [`PLockShift`].
        locks: PLockShift,
    },
    /// Stretching the selection by one note's right edge — the JS's
    /// `_resizeStart`.
    Resize {
        /// The note whose edge is held. The pointer is measured against this one
        /// and every other member follows by the same delta.
        grabbed: u32,
        /// Its length when the drag began. The delta is `wanted - start_len`, so
        /// a drag that reverses lands back where it started rather than
        /// accumulating.
        start_len: f64,
        /// The whole selection with the lengths it began with, parallel to what
        /// [`resize_selection_by`] hands back.
        items: Vec<(u32, LenEntry)>,
    },
    /// A rubber band, held in **content** space rather than screen space, so a
    /// scroll part-way through a drag keeps the band over the notes it was drawn
    /// around instead of sliding off them. The JS stores its corners the same
    /// way, relative to the scrolling container.
    Marquee { anchor: Vec2, current: Vec2 },
    /// Shift on a note's body: velocity, up for harder. One pixel is one unit,
    /// as `js/pianoroll.js`'s `Math.round(startY - clientY)` makes it.
    Velocity {
        /// The note under the pointer, which is the one the readout names.
        grabbed: u32,
        anchor_y: f32,
        /// The selection with the velocities it began with, so the delta applies
        /// to those rather than compounding per frame — the same shape
        /// [`Drag::Resize`] keeps its lengths in, and for the same reason.
        items: Vec<(u32, u8)>,
    },
    /// cmd/ctrl on a note's body: micro-timing.
    ///
    /// **One note, not the selection** — and that is the JS's call, not an
    /// omission. Its `micro` mode holds a single `note` where its `vel` mode holds
    /// `items`. The reason is `js/chords.js`: a strum is built out of per-note
    /// micro offsets *within* one chord, so a gesture that moved every selected
    /// note by the same amount could not make one.
    Micro { grabbed: u32, anchor_x: f32, start: f64 },
}

/// What a press at a given place, with given modifiers, would begin.
///
/// Named separately from [`Drag`] and consulted by both the press handler and the
/// cursor, so **the cursor cannot promise a gesture the press will not perform**.
/// Deciding it twice is how you end up with a crosshair over a note that is about
/// to be moved instead of banded.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Intent {
    /// Alt: the selection is copied and the copies are dragged. Tested before
    /// everything else, edge included — see the header.
    Duplicate(u32),
    Resize(u32),
    Velocity(u32),
    Micro(u32),
    Move(u32),
    Marquee,
    /// A press on empty space with no modifier: a note, and a drag from here sets
    /// its length. The plain drag on empty space was held in reserve for exactly
    /// this from Phase 5 until Phase 9.
    Create,
    /// The gutter, or nothing under the pointer at all.
    Nothing,
}

/// One member of a group move, and where it started — the JS's `_groupStart`
/// items. Captured once at press so the drag applies a single delta to the
/// original positions rather than compounding one per frame.
#[derive(Clone, PartialEq, Debug)]
struct Grabbed {
    id: u32,
    step: f64,
    pitch: u8,
}

impl Default for PianoRoll {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            scroll_x: 0.0,
            scroll_y: -((PITCH_MAX - TOP_PITCH) as f32 * CELL_H),
            selected: BTreeSet::new(),
            dragging: None,
            lane: TrigLane::default(),
            strip: PLockStrip::default(),
            // What `Note::new` was hard-coded with before anything could set it,
            // so the app draws the same note it always did until someone moves the
            // slider. `js/state.js` opens on 100 too.
            default_velocity: 100,
            readout: None,
            hover_still: None,
            hover_box: None,
            wheel: 0.0,
        }
    }
}

/// Where the grid is on screen, so drawing and hit-testing cannot disagree —
/// which they did, in three copies, in the prototype.
struct Grid {
    origin: Pos2,
    cell: Vec2,
    /// Row 0 is `band.hi`, counting down. Held here rather than read from the
    /// track at each call site for the same reason `origin` is: the prototype's
    /// bug was three copies of the geometry that agreed only at the origin.
    band: Band,
}

impl Grid {
    fn x_of_step(&self, step: f64) -> f32 {
        self.origin.x + step as f32 * self.cell.x
    }

    /// Where a note **starts on screen**: its step plus its micro-timing.
    ///
    /// Micro-timing became settable in Phase 9, and a gesture nothing on screen
    /// answers is a gesture that did nothing as far as anyone can tell — so a
    /// nudged note is drawn nudged, which is what `js/pianoroll.js` does too
    /// (`(n.step + (n.micro ?? 0)) * CELL_W`).
    ///
    /// **Everything that hit-tests or resizes goes through this as well**, and
    /// there the JS does *not*: its `_pos` finds a note by whole step and
    /// measures `nearEdge` from `n.step + n.len`, so in the browser a note nudged
    /// half a step right is grabbed half a cell left of where it is drawn. That is
    /// the exact bug this file's `Grid` exists to prevent — one geometry, used by
    /// drawing and by hit-testing, because the prototype's three copies agreed
    /// only at the origin. With micro at 0 the two are identical, which is why
    /// every resize test written before Phase 9 still holds.
    fn start_of(&self, note: &Note) -> f64 {
        note.step + note.micro
    }

    fn y_of_pitch(&self, pitch: u8) -> f32 {
        // Signed: a pitch above the band would wrap a `u8` subtraction into a row
        // near 255, and drawing a note a thousand rows down is a worse answer
        // than drawing it off the top.
        self.origin.y + (self.band.hi as i32 - pitch as i32) as f32 * self.cell.y
    }

    fn step_at(&self, x: f32) -> f64 {
        ((x - self.origin.x) / self.cell.x).floor() as f64
    }

    /// `None` above or below the drawable rows, rather than a clamp: a note
    /// clamped to the top of the band lands somewhere nobody asked for.
    fn pitch_at(&self, y: f32) -> Option<u8> {
        let row = ((y - self.origin.y) / self.cell.y).floor();
        let pitch = self.band.hi as f32 - row;
        // Range-checked before the cast, then checked against the band: a float
        // cast that saturates would turn a click far above the roll into pitch 0.
        (0.0..=127.0)
            .contains(&pitch)
            .then_some(pitch as u8)
            .filter(|p| self.band.contains(*p))
    }

    /// The **cell band** a note occupies: its full length and the full height of
    /// its row, rather than the inset rect it is drawn and clicked in.
    ///
    /// This is what `_inMarquee` tests against, and the JS does it deliberately —
    /// it draws a note inset by 1.5px top and bottom and still marquees against
    /// the whole `CELL_H`. A band that had to enclose the drawn pixels would miss
    /// notes whose row it plainly crosses, which reads as the marquee being
    /// broken rather than as being precise.
    fn note_band(&self, note: &Note) -> Rect {
        let y = self.y_of_pitch(note.pitch);
        let start = self.start_of(note);
        Rect::from_min_max(
            Pos2 { x: self.x_of_step(start), y },
            Pos2 { x: self.x_of_step(start + note.len), y: y + self.cell.y },
        )
    }

    fn note_rect(&self, note: &Note) -> Rect {
        let w = (note.len.max(0.125) as f32 * self.cell.x).max(self.cell.x * 0.3);
        Rect::from_min_size(
            Pos2 { x: self.x_of_step(self.start_of(note)), y: self.y_of_pitch(note.pitch) },
            Vec2 { x: w, y: self.cell.y * 0.8 },
        )
    }
}

/// The wash over a row that is in the key, or `None` for one that is not.
///
/// `js/pianoroll.js`'s own two alphas — 15% on the root and 6% on the rest of the
/// scale — in the JS's orange rather than [`super::ACCENT`], because accent means
/// *the engine is about to do this* and a key is something you chose to look at.
/// The roll's marquee is the same orange for the same reason.
///
/// **A function rather than three literals inside the paint loop**, so the one
/// thing about it a test can hold is held: which row is stronger. Swap the two and
/// the key reads inside out, with every other test still green — the shape
/// `DEVELOPMENT.md` lesson 2 is about. Whether either wash is *visible* at all is a
/// screen check, and PLAN.md §9 has it.
fn scale_wash(row: Row) -> Option<Color32> {
    match row {
        Row::Root => Some(Color32::from_rgba_unmultiplied(240, 145, 58, 38)),
        Row::InScale => Some(Color32::from_rgba_unmultiplied(240, 145, 58, 15)),
        Row::Outside => None,
    }
}

/// The band on screen, from the two content-space corners the drag is holding.
fn marquee_rect(grid: &Grid, anchor: Vec2, current: Vec2) -> Rect {
    Rect::from_two_pos(grid.origin + anchor, grid.origin + current)
}

/// `js/pianoroll.js`'s `_inMarquee`, and the strictness is the part that matters.
///
/// Every edge is tested with `<` and `>`, never `<=` — so a band of no area
/// selects nothing, and a note the band merely *touches* is left out. egui's own
/// [`Rect::intersects`] is closed on both ends and would take that grazed note,
/// which is why this is written out rather than delegated to it.
fn marquee_hits(grid: &Grid, track: &Track, band: Rect) -> BTreeSet<u32> {
    track
        .notes
        .iter()
        .filter(|n| {
            let r = grid.note_band(n);
            r.min.x < band.max.x
                && r.max.x > band.min.x
                && r.min.y < band.max.y
                && r.max.y > band.min.y
        })
        .map(|n| n.id)
        .collect()
}

// --- the pencil cursor (A3) --------------------------------------------------
//
// A real OS cursor, via egui 0.36's `Context::set_cursor_image`, rather than a
// built-in `CursorIcon` or a shape painted over the pointer: an OS cursor is
// not clipped by the window, and nothing has to hide the real pointer under a
// painted one. `CustomCursorImage` wants straight (non-premultiplied) RGBA, a
// buffer of exactly `size[0] * size[1] * 4` bytes, and a hotspot inside the
// bitmap — `egui-winit` uploads it as a genuine `winit::window::CustomCursor`
// and falls back to `cursor_icon` on backends that cannot.

/// The pencil, one character per pixel, authored by hand rather than loaded
/// from an asset file or drawn from a font glyph — `ui::mod`'s rule that a
/// mark shipped unread is a liability, applied to a cursor: this is readable
/// in review and diffable, and it cannot come out as tofu because there is no
/// font behind it.
///
/// Read as a diagonal shaft, tip to eraser: graphite (`G`), a wood taper
/// (`T`), the yellow body (`Y`), a metal ferrule (`M`) and a pink eraser
/// (`E`), outlined in near-black (`K`) so it reads against a light background
/// and a dark one alike. `.` is transparent. The tip is the `G` pixel at row
/// 19, column 0 — bottom-left — which is where [`PENCIL_HOTSPOT`] points, so
/// the mark is exactly where a click will land.
#[rustfmt::skip]
const PENCIL_MASK: [&str; 20] = [
    "....................",
    "...............KKKKK",
    "..............KEEEEK",
    ".............KEEEEK.",
    "............KEEEEK..",
    "...........KMMMMK...",
    "..........KMMMMK....",
    ".........KYYYYK.....",
    "........KYYYYK......",
    ".......KYYYYK.......",
    "......KYYYYK........",
    ".....KYYYYK.........",
    "....KYYYYK..........",
    "...KYYYYK...........",
    "..KTTTTK............",
    ".KTTTTK.............",
    "KTTTTK..............",
    "TTTTK...............",
    "GGGKK...............",
    "GKK.................",
];

/// 20x20. The first cut was 32x32 and Neil's note on it was "too big" — a
/// `CustomCursorImage` takes a pixel buffer with no scale factor, so on a
/// retina display the OS hands those pixels straight through and the pencil
/// came out roughly twice the size of the system arrow it replaces. 20 is
/// close to a macOS cursor's own drawn extent, and the mask was re-authored at
/// that size rather than downsampled, so no edge is half a pixel.
const PENCIL_SIZE: [u16; 2] = [20, 20];
/// The graphite point, bottom-left, matching [`PENCIL_MASK`]'s row 19.
const PENCIL_HOTSPOT: [u16; 2] = [0, 19];

fn pencil_pixel(ch: char) -> [u8; 4] {
    match ch {
        'G' => [0x33, 0x2e, 0x2a, 0xff], // graphite tip
        'T' => [0xca, 0xa4, 0x72, 0xff], // wood taper
        'Y' => [0xf2, 0xc1, 0x4e, 0xff], // pencil body
        'M' => [0xc9, 0xc9, 0xc9, 0xff], // metal ferrule
        'E' => [0xe8, 0x82, 0x9a, 0xff], // eraser
        'K' => [0x18, 0x18, 0x18, 0xff], // outline, for contrast on any background
        _ => [0x00, 0x00, 0x00, 0x00],   // transparent
    }
}

/// Build the pencil's RGBA once and hold it in an `Arc` for the app's life.
/// `egui-winit` dedupes `set_cursor_image` by `Arc` pointer identity, so
/// handing it the same allocation every frame costs nothing and rebuilding it
/// every frame would cost a re-upload to the OS every frame.
fn pencil_cursor() -> egui::CustomCursorImage {
    static RGBA: std::sync::OnceLock<std::sync::Arc<[u8]>> = std::sync::OnceLock::new();
    let rgba = RGBA
        .get_or_init(|| {
            PENCIL_MASK
                .iter()
                .flat_map(|row| row.chars())
                .flat_map(pencil_pixel)
                .collect::<Vec<u8>>()
                .into()
        })
        .clone();
    egui::CustomCursorImage { rgba, size: PENCIL_SIZE, hotspot: PENCIL_HOTSPOT }
}

impl PianoRoll {
    /// Draw and edit `track`. `playhead` is where the engine is *within this
    /// track*, in its own steps, or `None` when stopped.
    ///
    /// `harmony` is the session's key and chord settings, and it is `&mut` for one
    /// gesture: **alt+wheel over the grid cycles the chord inversion**, which the
    /// Harmony panel also edits. Everything else here only reads it — the tinted
    /// rows and the chord a click would stamp.
    ///
    /// Returns whether the caller has something to save: the track changed, or the
    /// inversion did. The two are folded together because both end up in the
    /// session file, and a shell that told them apart would need a second flag to
    /// say the same thing twice — `core::history` already refuses to record a step
    /// for a change that touched no note.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        track: &mut Track,
        playhead: Option<f64>,
        harmony: &mut Harmony,
    ) -> bool {
        let full = ui.available_rect_before_wrap();
        // The trig lane and the p-lock strip take the bottom of what the roll was
        // given, in that order: all three share their step columns, so nothing
        // but the roll may own the space between them. The strip is as tall as
        // the track has lanes and vanishes entirely at zero, rather than leaving
        // an empty band under the roll.
        // The roll keeps a floor whatever the strip wants: an eleven-lane track
        // is a real thing — the Phase 0 captures are one — and eleven rows plus
        // the trig lane would leave the notes a sliver. The strip takes what is
        // left and says how many lanes it could not show.
        const MIN_ROLL_H: f32 = 120.0;
        let room = (full.height() - triglane::LANE_H - MIN_ROLL_H).max(0.0);
        let strip_h = plocklane::strip_height(track).min(room);
        let below = (triglane::LANE_H + strip_h).min(full.height());
        let lane_h = triglane::LANE_H.min(below);
        let rect = Rect::from_min_max(full.min, Pos2 { x: full.max.x, y: full.max.y - below });
        let lane_rect = Rect::from_min_max(
            Pos2 { x: full.min.x, y: rect.max.y },
            Pos2 { x: full.max.x, y: rect.max.y + lane_h },
        );
        let strip_rect = Rect::from_min_max(Pos2 { x: full.min.x, y: lane_rect.max.y }, full.max);

        // Allocated before anything is drawn or hit-tested. egui decides what the
        // pointer is over from the allocation, so a widget that allocates last
        // cannot tell a click on itself from a click on the panel beside it.
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        // The lane scrolls the shared columns too — it sits in the same strip,
        // exactly as the JS lane shares the roll's scroll container.
        // Computed before the scroll, which clamps against how tall the band is.
        let band = Band::for_track(track);
        // **Alt+wheel over the grid aims the voicing instead of scrolling it.**
        // `js/pianoroll.js` gives the wheel to `onChordWheel` first and calls
        // `preventDefault` when it is taken, which is exactly this `else`: a
        // gesture that both cycled the inversion and scrolled the roll away from
        // the cell being aimed at would be unusable.
        let wheel = self.wheel_inversion(ui, &response, harmony);
        // **Only a `Cycled` is a change.** `Aimed` is a trackpad flick part-way to
        // its next notch: the wheel is spent, so the roll must not scroll, but no
        // field moved — and reporting one would mark the session unsaved for holding
        // alt and twitching a finger.
        let mut changed = wheel == Wheel::Cycled;
        // **Three gestures want this one wheel, and a held modifier is what tells
        // them apart**: alt aims the chord (above), cmd/ctrl zooms, a bare wheel
        // scrolls. Tried in that order, and **whichever takes the frame's delta
        // stops the next from spending it again** — the same rule `Wheel::Aimed`
        // states for the chord aim, one layer out. Without it a flick at
        // [`ZOOM_MAX`] would scroll the roll away from the cell it was trying to
        // magnify.
        if wheel == Wheel::Ignored
            && !self.wheel_zoom(ui, &response, rect)
            && (response.hovered()
                || ui.rect_contains_pointer(lane_rect)
                || ui.rect_contains_pointer(strip_rect))
        {
            self.scroll(ui);
        }
        // **The scroll bound is applied here, after the gesture and before the
        // grid, and nowhere else.** It is a function of the cell size, so a zoom
        // moves it — and the Edit panel's slider can move the zoom without ever
        // holding a [`Band`] to measure it against. One owner rather than one
        // rule restated in every writer of these three fields, which is
        // `DEVELOPMENT.md` lesson 5.
        self.clamp_scroll(band);

        let grid = Grid {
            origin: Pos2 {
                x: rect.min.x + KEY_W + self.scroll_x,
                y: rect.min.y + self.scroll_y,
            },
            cell: self.cell(),
            band,
        };

        self.paint_grid(&painter, rect, &grid, track, harmony);
        self.paint_notes(&painter, &grid, track);
        // Over the notes and under the playhead: it is a preview of notes, so it
        // belongs in their layer, and nothing may sit on top of where the engine is.
        self.paint_chord_ghost(&painter, rect, &grid, track, harmony, response.hover_pos());
        if let Some(step) = playhead {
            let x = grid.x_of_step(step);
            painter.line_segment(
                [Pos2 { x, y: rect.min.y }, Pos2 { x, y: rect.max.y }],
                egui::Stroke::new(2.0, Color32::from_rgb(255, 210, 80)),
            );
        }

        self.paint_keyboard(&painter, rect, &grid);

        changed |= self.interact(&response, &grid, track, harmony);
        self.paint_marquee(&painter, &grid);
        // After `interact`, so the number is this frame's rather than last
        // frame's — the same reason the marquee is drawn here.
        self.paint_readout(&painter, rect, &grid, track);
        // Same reason, and the two never fight over one note: `update_hover`
        // (called from `interact`) clears `hover_box` whenever a drag —
        // hence a `readout` — is running.
        self.paint_hover_box(&painter, rect, &grid, track);
        let cols = triglane::Cols { origin_x: grid.origin.x, cell_w: grid.cell.x };
        // **This is the line the marquee was built for.** The lane's
        // selection-wide edit has been ported since the lane shipped, and until
        // the roll could hold more than one id there was never more than one
        // here to hand it.
        let selected: Vec<u32> = self.selected.iter().copied().collect();
        changed |= self.lane.ui(ui, lane_rect, cols, track, &selected);
        changed |= self.strip.ui(ui, strip_rect, cols, track, &selected);
        changed
    }

    /// **Alt+wheel over the grid cycles the chord inversion**, and returns whether
    /// it took the wheel — `js/main.js`'s `onChordWheel`, which is the one control
    /// in this app that is reached from the roll rather than from a panel. It is
    /// there because inversion is the setting you want to change *while aiming*:
    /// the ghost redraws under the cursor as it cycles, so the voicing is chosen by
    /// looking at where it will land rather than by looking at a combo box.
    ///
    /// **Only in chord mode**, so alt+wheel is an ordinary scroll the rest of the
    /// time rather than a silent no-op that also refuses to scroll.
    ///
    /// ## Why the events are counted rather than the delta read
    ///
    /// `smooth_scroll_delta` is smoothed *across frames*, so one notch of a mouse
    /// wheel arrives as several frames of small deltas — and a four-way cycle
    /// stepped once per frame spins two or three positions per notch, which is a
    /// control nobody can land on a chosen value. A wheel notch is one
    /// [`egui::Event::MouseWheel`] in `Line` units, so those are counted: one notch,
    /// one step, which is what the browser's discrete `wheel` event gave the JS for
    /// free.
    ///
    /// A trackpad sends `Point` units continuously instead, so those accumulate and
    /// spend one step per [`TRACKPAD_NOTCH`] points. Without that, one two-finger
    /// flick would go round the cycle several times.
    ///
    /// **Wheel up raises the voicing**, which is the direction egui's positive
    /// `delta.y` already means for the rows: the roll scrolls toward higher pitches
    /// and the inversion moves the chord's bottom note up an octave.
    fn wheel_inversion(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        harmony: &mut Harmony,
    ) -> Wheel {
        if !harmony.chord.on || !response.hovered() || !ui.input(|i| i.modifiers.alt) {
            // The accumulator is a gesture's worth of state, so it goes with the
            // gesture: half a flick left behind would spend itself on the next one.
            self.wheel = 0.0;
            return Wheel::Ignored;
        }
        let mut steps = 0i32;
        let mut points = 0.0f32;
        ui.input(|i| {
            for event in &i.events {
                if let egui::Event::MouseWheel { unit, delta, .. } = event {
                    match unit {
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
                            steps += delta.y.signum() as i32;
                        }
                        egui::MouseWheelUnit::Point => points += delta.y,
                    }
                }
            }
        });
        self.wheel += points;
        // **Divided rather than looped**, and the deliberate-bug pass is why. The loop
        // this replaced tested `abs() >= TRACKPAD_NOTCH` and subtracted the same
        // constant — correct, but only while the two agreed: planting a smaller
        // threshold while leaving the subtraction alone made it oscillate across zero
        // and **hang**, which in this function means the window stops drawing. A
        // division cannot spin, and it spends a big flick in one step rather than one
        // iteration per notch.
        let notches = (self.wheel / TRACKPAD_NOTCH).trunc();
        if notches != 0.0 {
            steps += notches as i32;
            self.wheel -= notches * TRACKPAD_NOTCH;
        }
        if steps == 0 {
            // Still taken when the wheel moved at all: the frame's delta has been
            // spent into the accumulator, and letting `scroll` read it again would
            // scroll the roll by the same flick that is aiming the chord.
            return if points == 0.0 { Wheel::Ignored } else { Wheel::Aimed };
        }
        harmony.cycle_inversion(steps);
        Wheel::Cycled
    }

    /// Wheel scrolls the roll. Shift-wheel scrolls it sideways, as every DAW
    /// does; without this the roll opens on a fixed window of pitches and there
    /// is no way to reach the rest.
    ///
    /// **The bound belongs to [`Self::clamp_scroll`]**, which `ui` applies once a
    /// frame for every writer of these fields rather than here for one of them.
    fn scroll(&mut self, ui: &Ui) {
        let delta = ui.input(|i| i.smooth_scroll_delta);
        if delta == Vec2::ZERO {
            return;
        }
        let (dx, dy) = if ui.input(|i| i.modifiers.shift) {
            (delta.y + delta.x, 0.0)
        } else {
            (delta.x, delta.y)
        };
        self.scroll_x += dx;
        self.scroll_y += dy;
    }

    /// The cell size the grid is currently drawn at. **One expression rather than
    /// the three copies of `CELL_W * self.zoom` this replaced** — the scroll
    /// bound, the grid and the hit test all measure in cells, and a zoom that
    /// reached two of the three would put drawing and hit-testing back into the
    /// disagreement [`Grid`] exists to prevent.
    fn cell(&self) -> Vec2 {
        Vec2 { x: CELL_W * self.zoom, y: CELL_H * self.zoom }
    }

    /// How far the view may be scrolled: from the whole of the content pushed off
    /// the top-left, to step 0 and the band's top row against the corner. In
    /// pixels, so it moves with the zoom.
    fn clamp_scroll(&mut self, band: Band) {
        let cell = self.cell();
        self.scroll_x = self.scroll_x.clamp(-(COLS as f32 * cell.x), 0.0);
        self.scroll_y = self.scroll_y.clamp(-(band.rows() as f32 * cell.y), 0.0);
    }

    /// **Cmd/ctrl+wheel over the grid zooms it, and a trackpad pinch does the
    /// same** — what every DAW binds, and the reason the wheel had to be shared
    /// three ways. Returns whether the gesture was a zoom.
    ///
    /// ## Why `zoom_delta` rather than the wheel events
    ///
    /// [`Self::wheel_inversion`] counts raw [`egui::Event::MouseWheel`]s because
    /// it drives a four-way cycle and needs a notch to be one step. This wants
    /// the opposite — a continuous factor — and egui 0.36 has already computed
    /// it: `InputOptions::zoom_modifier` is `COMMAND` by default, and a wheel
    /// event carrying it is turned into `zoom_factor_delta` as
    /// `(scroll_zoom_speed * delta).exp()` while **`smooth_scroll_delta` is set
    /// to zero for that frame** (`egui-0.36.1/src/input_state/mod.rs` ~461).
    /// Three things follow, and the last is the one worth knowing:
    ///
    /// - The scroll below is already inert under cmd, so the two cannot both run
    ///   on one flick. The `&&` in `ui` says so out loud anyway, because a pinch
    ///   arrives as [`egui::Event::Zoom`] and does *not* zero the scroll delta.
    /// - A pinch on a Mac trackpad and a ctrl+wheel from a mouse land on the same
    ///   field, so this gesture ships on both for free.
    /// - **It is exponential, so the frame smoothing cannot change the answer.**
    ///   A mouse notch arrives spread over several frames; the factors multiply
    ///   where the deltas add, so `exp(a)*exp(b) == exp(a+b)` and the flick is
    ///   worth the same whether egui delivers it in one frame or six. An additive
    ///   step per frame would have zoomed by however many frames the machine
    ///   managed.
    ///
    /// Gated on `hovered`, because `zoom_delta` is the *window's* input and not
    /// this widget's: without it a cmd+wheel anywhere in the app — over the
    /// tracks pane, over a panel's slider — would zoom the roll underneath it.
    fn wheel_zoom(&mut self, ui: &Ui, response: &egui::Response, rect: Rect) -> bool {
        if !response.hovered() {
            return false;
        }
        let delta = ui.input(|i| i.zoom_delta());
        if delta == 1.0 {
            return false;
        }
        // The pointer, in the grid's own coordinates. `hover_pos` is `Some`
        // whenever `hovered` is, so the centre is a fallback for a pinch that
        // arrives on the frame the pointer left; the cell it holds still is then
        // the middle of the view rather than one under nothing.
        let anchor = response
            .hover_pos()
            .map(|pos| Vec2 { x: pos.x - (rect.min.x + KEY_W), y: pos.y - rect.min.y })
            .unwrap_or_else(|| Vec2 { x: rect.width() / 2.0, y: rect.height() / 2.0 });
        self.zoom_by(delta, anchor);
        // **Taken even when the zoom did not move.** That is the clamp's case,
        // and it is `Wheel::Aimed`'s distinction again: the frame's delta has
        // been spent, so the caller must not hand it to `scroll` as well.
        true
    }

    /// Multiply the cell size by `delta`, keeping whatever is under `anchor`
    /// under it. Returns whether the zoom moved.
    ///
    /// `anchor` is measured from **the grid's own top-left corner** — where step
    /// 0 of the band's top row sits at rest — rather than from the panel or the
    /// screen, which is what lets this be arithmetic: no `Rect`, no `Ui`, and a
    /// test that can state the invariant directly.
    ///
    /// **A view change is not an edit**, so nothing here reports one. The roll's
    /// `ui` returns whether the caller has something to save, and a zoom would
    /// mark the session unsaved and re-snapshot the engine for looking closer at
    /// it — the same reason `Wheel::Aimed` exists.
    fn zoom_by(&mut self, delta: f32, anchor: Vec2) -> bool {
        let wanted = (self.zoom * delta).clamp(ZOOM_MIN, ZOOM_MAX);
        // **The factor that was achieved, not the one that was asked for.** The
        // two differ at either end of the range, and anchoring on the asked-for
        // one would slide the view sideways while the cell size stayed exactly
        // where it was: a gesture that only moves the roll is worse than one that
        // does nothing at all.
        let factor = wanted / self.zoom;
        if factor == 1.0 {
            return false;
        }
        self.zoom = wanted;
        // A content point sits `anchor - scroll` pixels into the grid, and that
        // distance scales with the cell size. The scroll takes up the difference,
        // so the point lands back under `anchor`.
        self.scroll_x = anchor.x - (anchor.x - self.scroll_x) * factor;
        self.scroll_y = anchor.y - (anchor.y - self.scroll_y) * factor;
        true
    }

    fn paint_grid(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        grid: &Grid,
        track: &Track,
        harmony: &Harmony,
    ) {
        let faint = egui::Stroke::new(1.0, Color32::from_gray(40));
        // The five black-key rows are tinted darker, as `js/pianoroll.js` tints
        // them. This is what makes the key column read as a keyboard rather than
        // as a list of names: the pattern of twos and threes runs all the way
        // across the grid, so you can find a pitch out at step 40 without
        // tracking back to the gutter.
        for pitch in grid.band.lo..=grid.band.hi {
            let y = grid.y_of_pitch(pitch);
            if y + grid.cell.y < rect.min.y || y > rect.max.y {
                continue;
            }
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2 { x: rect.min.x, y },
                    Vec2 { x: rect.width(), y: grid.cell.y },
                ),
                0.0,
                if is_black(pitch) {
                    Color32::from_rgb(0x16, 0x18, 0x1c)
                } else {
                    Color32::from_rgb(0x1d, 0x20, 0x26)
                },
            );
            // **The key, washed over the row and nothing more.** PLAN.md §5: the
            // tint is visual only and never restricts what can be drawn, which is
            // `js/chords.js`'s rule and `js/pianoroll.js` draws it exactly this
            // way — the root a little stronger than the rest of the scale, and the
            // notes outside it left plain.
            //
            // In the JS's own orange rather than [`super::ACCENT`], deliberately:
            // accent means *the engine is about to do this*, and a key is a thing
            // you chose to look at. The roll's marquee is the same orange for the
            // same reason.
            if let Some(wash) = harmony.row(pitch).and_then(scale_wash) {
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2 { x: rect.min.x, y },
                        Vec2 { x: rect.width(), y: grid.cell.y },
                    ),
                    0.0,
                    wash,
                );
            }
            painter.line_segment(
                [Pos2 { x: rect.min.x, y }, Pos2 { x: rect.max.x, y }],
                faint,
            );
        }
        for col in 0..COLS {
            let x = grid.x_of_step(col as f64);
            if x < rect.min.x || x > rect.max.x {
                continue;
            }
            // Every fourth step is a beat and every sixteenth a bar, which is the
            // only way to count steps by eye at this width.
            let stroke = match col % 16 {
                0 => egui::Stroke::new(1.0, Color32::from_gray(90)),
                _ if col % 4 == 0 => egui::Stroke::new(1.0, Color32::from_gray(60)),
                _ => faint,
            };
            painter.line_segment(
                [Pos2 { x, y: rect.min.y }, Pos2 { x, y: rect.max.y }],
                stroke,
            );
        }

        // Where this track wraps. Per track, not per pattern — that is what
        // polymeter looks like, and without the line a 12-step track next to a
        // 16-step one is invisible until you hear it.
        if track.length_steps > 0 {
            let x = grid.x_of_step(track.length_steps as f64);
            painter.line_segment(
                [Pos2 { x, y: rect.min.y }, Pos2 { x, y: rect.max.y }],
                egui::Stroke::new(2.0, Color32::from_rgb(120, 90, 160)),
            );
        }
    }

    /// The selection, with where each member started — the JS's `_groupStart`.
    /// Read off the track rather than off the selection set so a stale id cannot
    /// join a drag, and captured once at press so the move applies one delta to
    /// the original positions instead of compounding one per frame.
    fn grab(&self, track: &Track) -> Vec<Grabbed> {
        track
            .notes
            .iter()
            .filter(|n| self.selected.contains(&n.id))
            .map(|n| Grabbed { id: n.id, step: n.step, pitch: n.pitch })
            .collect()
    }

    /// The p-lock lanes as this move found them, so the locks travel with the
    /// trigs rather than staying on the steps those trigs came off.
    ///
    /// The rules — and the reason a lock is the trig's rather than the step's —
    /// live in [`PLockShift`], in `core`, where they can be argued about without
    /// a pointer. This is only the group's ids in the shape it wants.
    fn hold_locks(&self, track: &Track, group: &[Grabbed]) -> PLockShift {
        let moving: Vec<u32> = group.iter().map(|g| g.id).collect();
        PLockShift::capture(track, &moving)
    }

    /// The rubber band, in the JS's own colours — a 16% wash and a solid edge.
    /// Drawn after `interact` so the band is the one the current frame's pointer
    /// made, not the one the last frame left behind.
    fn paint_marquee(&self, painter: &egui::Painter, grid: &Grid) {
        let Some(Drag::Marquee { anchor, current }) = &self.dragging else {
            return;
        };
        let rect = marquee_rect(grid, *anchor, *current);
        painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(240, 145, 58, 41));
        painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(1.0, Color32::from_rgb(240, 145, 58)),
            egui::StrokeKind::Inside,
        );
    }

    /// The key column, drawn last so it stays clean over whatever scrolled under
    /// it — `js/pianoroll.js` says exactly that above its own copy, and the order
    /// is the whole reason the gutter can be opaque while the grid scrolls.
    ///
    /// **Only the Cs are labelled.** Twelve names an octave at this row height is
    /// a smear, and the seven that are left are the ones you count octaves by.
    /// Every glyph here is ASCII — the names are letters, `#` and a digit — so
    /// this ships no new characters and cannot repeat the scene bar's tofu.
    fn paint_keyboard(&self, painter: &egui::Painter, rect: Rect, grid: &Grid) {
        // Behind the keys, for the rows the band does not reach.
        let gutter = Rect::from_min_size(rect.min, Vec2 { x: KEY_W, y: rect.height() });
        painter.rect_filled(gutter, 0.0, Color32::from_rgb(0x17, 0x1a, 0x20));

        for pitch in grid.band.lo..=grid.band.hi {
            let y = grid.y_of_pitch(pitch);
            if y + grid.cell.y < rect.min.y || y > rect.max.y {
                continue;
            }
            let key =
                Rect::from_min_size(Pos2 { x: rect.min.x, y }, Vec2 { x: KEY_W, y: grid.cell.y });
            painter.rect_filled(
                key,
                0.0,
                if is_black(pitch) {
                    Color32::from_rgb(0x0c, 0x0d, 0x10)
                } else {
                    Color32::from_rgb(0xe8, 0xe6, 0xe0)
                },
            );
            painter.rect_stroke(
                key,
                0.0,
                egui::Stroke::new(1.0, Color32::from_black_alpha(0x40)),
                egui::StrokeKind::Inside,
            );
            if pitch % 12 == 0 {
                painter.text(
                    Pos2 { x: rect.min.x + 6.0, y: y + grid.cell.y / 2.0 },
                    egui::Align2::LEFT_CENTER,
                    note_name(pitch),
                    egui::FontId::proportional(10.0),
                    Color32::from_gray(0x55),
                );
            }
        }

        painter.line_segment(
            [
                Pos2 { x: rect.min.x + KEY_W, y: rect.min.y },
                Pos2 { x: rect.min.x + KEY_W, y: rect.max.y },
            ],
            egui::Stroke::new(1.0, Color32::from_rgb(0x2a, 0x2e, 0x38)),
        );
    }

    /// A note's fill, brightened by its velocity.
    ///
    /// **This is what makes the velocity drag visible**, and without it the
    /// headline gesture of Phase 9 would move a number nothing on screen answers —
    /// which is the same class of bug as a tofu glyph: it passes every test and
    /// tells the user nothing happened. `js/pianoroll.js` scales lightness from
    /// 45% to 90% across the velocity range and this does the same, mixing each
    /// hue toward white rather than doing HSL arithmetic egui has no need of.
    ///
    /// The **hue** still carries selection, so the two readings do not compete:
    /// blue means selected, green means not, and brightness means loud in both.
    fn note_fill(velocity: u8, selected: bool) -> Color32 {
        let base = if selected {
            Color32::from_rgb(52, 104, 166)
        } else {
            Color32::from_rgb(65, 117, 78)
        };
        // 1 is dim and 127 is full. The floor keeps the quietest note visible
        // rather than fading it into the grid.
        let t = (f32::from(velocity.max(1)) / 127.0).clamp(0.0, 1.0);
        let lift = |c: u8, to: u8| (f32::from(c) + (f32::from(to) - f32::from(c)) * t) as u8;
        let peak = if selected {
            Color32::from_rgb(120, 190, 255)
        } else {
            Color32::from_rgb(135, 215, 150)
        };
        Color32::from_rgb(
            lift(base.r(), peak.r()),
            lift(base.g(), peak.g()),
            lift(base.b(), peak.b()),
        )
    }

    /// A darker shade of a note's own fill, for the velocity bar. Scaled
    /// rather than a fixed offset, so it stays a *shade* of whatever hue
    /// `note_fill` picked — selection is a hue and stays one; the bar only
    /// darkens it.
    fn darker(c: Color32) -> Color32 {
        let scale = |v: u8| (f32::from(v) * 0.55) as u8;
        Color32::from_rgb(scale(c.r()), scale(c.g()), scale(c.b()))
    }

    /// The inner rect a velocity fill bar occupies within `rect` — a note's
    /// own body — factored out of the paint call so a test can assert on the
    /// geometry rather than on pixels.
    ///
    /// Anchored to the bottom edge, its height is `velocity / 127` of the
    /// note's own height, with a floor of one pixel so the quietest note
    /// still shows *something* — without the floor, velocity 1 and an empty
    /// note read identically. At 127 the bar's height equals the note's, so
    /// the loud/quiet boundary the bar draws simply has nothing above it to
    /// contrast against: full is full, and that is intended rather than a
    /// bug to chase.
    ///
    /// Inset by up to a pixel on each side, and `None` rather than a
    /// negative-width rect when even that would not fit — the smallest note
    /// the roll draws is a few pixels wide, and a bar that has to clip to fit
    /// is not a cue.
    fn velocity_bar_rect(rect: Rect, velocity: u8) -> Option<Rect> {
        const INSET: f32 = 1.0;
        const FLOOR_H: f32 = 1.0;
        if rect.width() <= INSET * 2.0 {
            return None;
        }
        // **The bottom edge is inset too, and that is not symmetry for its own
        // sake.** `paint_notes` strokes the note with `StrokeKind::Middle`,
        // which centres a 1px white line *on* `rect.max.y` — so a bar anchored
        // flush to `rect.max.y` has its bottom pixel painted over by the very
        // next call. At high velocities that costs nothing visible. At the
        // floor it costs everything: the 1px bar is exactly the pixel the
        // stroke takes, so the quietest note renders identically to an empty
        // one, which is the failure this floor exists to prevent.
        //
        // Measured on a real screen, 2026-08-20: before this inset, a note at
        // velocity 3 sampled as a uniform `#006EAE` across its whole interior —
        // no bar at any row. The unit tests all passed throughout, because they
        // assert on the `Rect` this returns and never on a pixel. That is
        // DEVELOPMENT.md's lesson 8 in one function.
        let floor_y = rect.max.y - INSET;
        let avail = (floor_y - rect.min.y).max(0.0);
        if avail < FLOOR_H {
            return None;
        }
        let t = (f32::from(velocity.max(1)) / 127.0).clamp(0.0, 1.0);
        let h = (avail * t).clamp(FLOOR_H, avail);
        Some(Rect::from_min_max(
            Pos2 { x: rect.min.x + INSET, y: floor_y - h },
            Pos2 { x: rect.max.x - INSET, y: floor_y },
        ))
    }

    fn paint_notes(&self, painter: &egui::Painter, grid: &Grid, track: &Track) {
        for note in &track.notes {
            let rect = grid.note_rect(note);
            let selected = self.selected.contains(&note.id);
            let fill = Self::note_fill(note.velocity, selected);
            painter.rect_filled(rect, 2.0, fill);
            // The second, independent cue PLAN.md's item 1 asks for: the hue
            // still says selected-or-not and the ramp is untouched, but a
            // note that clusters at the loud end of it now also has a bar
            // that says so without a drag. Before the stroke, so the stroke
            // still frames the whole note rather than just the fill above
            // the bar.
            if let Some(bar) = Self::velocity_bar_rect(rect, note.velocity) {
                painter.rect_filled(bar, 0.0, Self::darker(fill));
            }
            // StrokeKind is new in egui 0.32; Middle is what 0.27 drew — the
            // stroke centred on the rect edge rather than inset or outset.
            painter.rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0, Color32::WHITE),
                egui::StrokeKind::Middle,
            );
        }
    }

    /// The chord a press at `pos` would stamp, empty when there is not one.
    ///
    /// **The ghost and the press both read this**, so the preview cannot promise a
    /// chord the click does not deliver — the same argument [`Self::press_intent`]
    /// makes for the cursor, and it matters more here because
    /// [`chord_for_cell`] leaves notes out: a pitch the step already holds, and
    /// anything past the four notes a trig can carry. Those omissions are only
    /// reported by the ghost, since the roll has no status line.
    ///
    /// Empty over an existing note, which is PLAN.md §5's rule that notes you
    /// click on still move, resize and delete as usual — chord draw only acts on an
    /// empty cell.
    fn chord_at(
        &self,
        grid: &Grid,
        track: &Track,
        pos: Pos2,
        harmony: &Harmony,
    ) -> Vec<ChordNote> {
        if !harmony.chord.on {
            return Vec::new();
        }
        let Some(pitch) = grid.pitch_at(pos.y) else {
            return Vec::new();
        };
        if self.note_at(grid, track, pos).is_some() {
            return Vec::new();
        }
        let step = grid.step_at(pos.x).max(0.0);
        chord_for_cell(
            &track.notes,
            step,
            pitch,
            harmony,
            self.default_velocity,
            grid.band.range(),
        )
    }

    /// The chord under the cursor, drawn where its notes would land.
    ///
    /// **This is chord draw's whole affordance**, and the reason PLAN.md §5 asks
    /// for it: quality, inversion, drop-2 and strum are four settings whose effect
    /// on a voicing nobody can hold in their head, and the alternative to a preview
    /// is stamping four notes to find out. The strum is in it — each box sits at
    /// `step + micro`, so a staggered chord leans on screen before it is committed.
    ///
    /// Re-queried every frame rather than cached, so alt+wheel and every control in
    /// the panel update it in place under a still cursor. `js/pianoroll.js` says the
    /// same thing about its own ghost.
    fn paint_chord_ghost(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        grid: &Grid,
        track: &Track,
        harmony: &Harmony,
        hover: Option<Pos2>,
    ) {
        // Not during a gesture: a preview of the *next* press, drawn while one is
        // in hand, is a chord nobody is about to get.
        if self.dragging.is_some() {
            return;
        }
        let Some(pos) = hover.filter(|p| p.x >= rect.min.x + KEY_W) else {
            return;
        };
        for note in self.chord_at(grid, track, pos, harmony) {
            let step = grid.step_at(pos.x).max(0.0) + note.micro;
            let at = Rect::from_min_size(
                Pos2 { x: grid.x_of_step(step), y: grid.y_of_pitch(note.pitch) },
                grid.cell,
            )
            .shrink(1.5);
            painter.rect_filled(at, 2.0, Color32::from_rgba_unmultiplied(240, 145, 58, 71));
            painter.rect_stroke(
                at,
                2.0,
                egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 115)),
                egui::StrokeKind::Middle,
            );
        }
    }

    /// `vel 100` or `micro +0.12` over the note being dragged.
    ///
    /// Neither gesture is legible without a number: brightness says *louder*, not
    /// *how loud*, and a nine-pixel horizontal nudge says nothing at all about how
    /// many ticks the box will store. `js/pianoroll.js` draws both readouts for
    /// the same reason, and it is why its fine resize has one too.
    ///
    /// Every character is ASCII — letters, digits, `+`, `-`, `.` and a space —
    /// per `ui::mod`'s rule that a glyph nobody has looked at is a liability.
    fn paint_readout(&self, painter: &egui::Painter, rect: Rect, grid: &Grid, track: &Track) {
        let Some((id, text)) = &self.readout else { return };
        let Some(note) = track.notes.iter().find(|n| n.id == *id) else { return };
        let at = Pos2 {
            x: grid.x_of_step(grid.start_of(note)) + 2.0,
            // Above the note, but never off the top of the roll — the JS's
            // `Math.max(11, ...)`, which is why a note on the top row still says
            // what it is doing.
            y: (grid.y_of_pitch(note.pitch) - 3.0).max(rect.min.y + 9.0),
        };
        painter.text(
            at,
            egui::Align2::LEFT_BOTTOM,
            text,
            egui::FontId::proportional(11.0),
            Color32::WHITE,
        );
    }

    /// How long the pointer has to sit still over a note before the hover box
    /// shows it. No dwell means the box strobes across a track as the
    /// pointer merely crosses it on its way somewhere else.
    const HOVER_DWELL: f64 = 0.35;

    /// A note's length, in steps, the way the hover box prints it — a whole
    /// number when it is one, two decimal places when it is not (a fine
    /// resize can leave it fractional). ASCII only, per `ui::mod`'s glyph
    /// rule.
    fn fmt_len(len: f64) -> String {
        if (len.fract()).abs() < 1e-6 {
            format!("{len:.0}")
        } else {
            format!("{len:.2}")
        }
    }

    /// What the hover box says about `note`, one line per field — a pure
    /// function of the note, so a test can assert on it without a pointer or
    /// a painter. `vel` and `len` always show; `micro`, `PROB`, `FILL` and
    /// `COND` are omitted rather than printed as a placeholder when the note
    /// does not carry them — an absent trig condition is not a value.
    fn hover_lines(note: &Note) -> Vec<String> {
        let mut lines = vec![format!("vel {}", note.velocity), format!("len {}", Self::fmt_len(note.len))];
        if note.micro != 0.0 {
            let sign = if note.micro >= 0.0 { "+" } else { "" };
            lines.push(format!("micro {sign}{:.2}", note.micro));
        }
        if let Some(prob) = note.prob {
            lines.push(format!("PROB {prob}"));
        }
        if let Some(fill) = note.fill {
            // `ON`/`OFF`, matching how the trig lane already shows it
            // (`triglane`'s own `Field::Fill` formatting) rather than
            // inventing a second vocabulary for the same tri-state.
            lines.push(format!("FILL {}", if fill { "ON" } else { "OFF" }));
        }
        if let Some(cond) = &note.cond {
            lines.push(format!("COND {cond}"));
        }
        lines
    }

    /// Find the hovered note and, once the pointer has been still over it for
    /// [`Self::HOVER_DWELL`], fill in [`Self::hover_box`]. Called from
    /// `interact` — never repeated in the painter — so the dwell clock and
    /// the hit test have exactly one home each.
    ///
    /// **Reads the hit test off [`Self::note_at`]**, which is the function
    /// [`Self::press_intent`] itself is built on, rather than testing
    /// distance or step membership again here: a tooltip naming a different
    /// note from the one a click would act on is worse than no tooltip.
    ///
    /// Suppressed — `hover_box` cleared and the dwell clock reset — whenever
    /// a drag is in progress or the pointer is off the roll (`pos` is
    /// `None`, already filtered past the gutter by the caller): the drag
    /// readout owns the annotation job while a gesture is running, and two
    /// boxes naming one note is worse than either alone.
    fn update_hover(
        &mut self,
        ctx: &egui::Context,
        now: f64,
        pos: Option<Pos2>,
        grid: &Grid,
        track: &Track,
    ) {
        let Some(pos) = pos.filter(|_| self.dragging.is_none()) else {
            self.hover_still = None;
            self.hover_box = None;
            return;
        };
        // Restart the clock whenever the pointer lands somewhere new: a
        // pointer mid-crossing has no business showing the cell it is
        // passing through.
        let moved = self.hover_still.is_none_or(|(p, _)| p != pos);
        if moved {
            self.hover_still = Some((pos, now));
        }
        let settled_since = self.hover_still.map_or(now, |(_, t)| t);
        let waited = now - settled_since;
        if waited < Self::HOVER_DWELL {
            self.hover_box = None;
            // **Ask for the frame this dwell is waiting for.** egui repaints on
            // demand, so once the pointer stops moving nothing else schedules a
            // frame — the deadline passes with no frame to notice it, and the
            // box never appears at all.
            //
            // **Measured on a real screen, 2026-08-20, and no test in this file
            // can see it.** Hovering a note on an otherwise idle window showed
            // nothing after 2.5s, with the app confirmed frontmost
            // (`NSWorkspace.frontmostApplication`) and the pointer confirmed on
            // the note — so it was neither a focus problem nor a hit-test one.
            // The same hover *did* produce the box moments after a click,
            // because the click had left the app repainting.
            //
            // The plant for this line fails nothing, which is the finding rather
            // than the nuisance (DEVELOPMENT.md's lesson 6, third answer). Under
            // `Context::run_ui` the reported `repaint_delay` is `0ns` on every
            // frame whether this call is here or not — measured at t=0.05 and
            // t=0.30 with and without it, four runs, all `0ns` — because the
            // harness drives frames itself and egui's own pointer smoothing
            // keeps asking for more. Only a live, idle window can starve this
            // timer, so the evidence for this line is a screenshot and this
            // comment, not a test.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                Self::HOVER_DWELL - waited,
            ));
            return;
        }
        self.hover_box = self
            .note_at(grid, track, pos)
            .and_then(|id| track.notes.iter().find(|n| n.id == id))
            .map(|note| (note.id, Self::hover_lines(note)));
    }

    /// The hover box itself: a backed rect behind the text, unlike
    /// `paint_readout`'s bare white letters. A drag readout is brief and the
    /// eye is on the pointer already; a dwell box sits over a busy grid and
    /// is not readable without something behind it.
    fn paint_hover_box(&self, painter: &egui::Painter, rect: Rect, grid: &Grid, track: &Track) {
        let Some((id, lines)) = &self.hover_box else { return };
        let Some(note) = track.notes.iter().find(|n| n.id == *id) else { return };
        let font = egui::FontId::proportional(11.0);
        const LINE_H: f32 = 13.0;
        const PAD: f32 = 4.0;
        let width = lines
            .iter()
            .map(|line| {
                painter
                    .layout_no_wrap(line.clone(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0f32, f32::max)
            + PAD * 2.0;
        let height = LINE_H * lines.len() as f32 + PAD * 2.0;
        let at = Pos2 {
            x: grid.x_of_step(grid.start_of(note)) + 2.0,
            // Same clamp as `paint_readout`: never off the top of the roll,
            // so a note on the top row still gets a legible box.
            y: (grid.y_of_pitch(note.pitch) - 3.0 - height).max(rect.min.y),
        };
        let bg = Rect::from_min_size(at, Vec2 { x: width, y: height });
        painter.rect_filled(bg, 3.0, Color32::from_rgba_unmultiplied(20, 22, 26, 235));
        painter.rect_stroke(
            bg,
            3.0,
            egui::Stroke::new(1.0, Color32::from_gray(70)),
            egui::StrokeKind::Inside,
        );
        for (i, line) in lines.iter().enumerate() {
            painter.text(
                Pos2 { x: at.x + PAD, y: at.y + PAD + LINE_H * i as f32 },
                egui::Align2::LEFT_TOP,
                line,
                font.clone(),
                Color32::WHITE,
            );
        }
    }

    /// Click to select or create, drag to move. Returns whether the track
    /// changed.
    fn interact(
        &mut self,
        response: &egui::Response,
        grid: &Grid,
        track: &mut Track,
        harmony: &Harmony,
    ) -> bool {
        let mut changed = false;

        // A press in the gutter is on the (future) keyboard, not on step 0 —
        // without this, `step_at` comes back negative there and the clamp below
        // would turn a gutter click into a note on the downbeat.
        let in_gutter =
            |pos: Pos2| pos.x < response.rect.min.x + KEY_W;

        // **Right-click a note to delete it, immediately** — `js/pianoroll.js`'s
        // gesture, and the reason the roll could create trigs but never remove
        // one. The JS's other two deletes are Delete/Backspace (below) and
        // alt+click, which is deliberately *not* ported: alt+click deletes only
        // as the click half of a bargain whose drag half duplicates the note, and
        // alt-drag-to-copy does not exist here yet. Half a gesture that silently
        // does something else when you move the mouse is worse than no gesture.
        if response.secondary_clicked() {
            if let Some(pos) = response.interact_pointer_pos().filter(|p| !in_gutter(*p)) {
                if let Some(id) = self.note_at(grid, track, pos) {
                    changed |= self.delete(track, id);
                }
            }
        }

        // **Delete/Backspace removes every selected note** — plural since the
        // marquee, and the JS has always been plural here
        // (`notes.filter(n => !this.selected.has(n.id))`). The JS guards it on
        // not typing; egui's equivalent is that no widget holds keyboard focus,
        // which keeps Backspace inside the tempo and length fields where it
        // belongs rather than eating a trig.
        let typing = response.ctx.memory(|m| m.focused().is_some());
        let delete_pressed = response.ctx.input(|i| {
            i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
        });
        if !typing && delete_pressed {
            for id in self.selected.iter().copied().collect::<Vec<_>>() {
                changed |= self.delete(track, id);
            }
        }

        // `command` is cmd on macOS and ctrl everywhere else. The JS tests
        // `metaKey || ctrlKey` and accepts either on either platform; that is not
        // safe to copy here, because macOS delivers ctrl+click as a secondary
        // click and this roll spends the secondary button on delete.
        let mods = response.ctx.input(|i| i.modifiers);

        if response.drag_started() {
            // **What is being dragged is decided by where the press landed, not
            // by where the pointer is now.** egui reports `drag_started` on the
            // frame the movement threshold is crossed, by which point
            // `interact_pointer_pos` has already left the note — so picking the
            // note up from the current position worked only for a slow drag that
            // crossed the threshold while still inside the note's own rect, and a
            // quick flick grabbed nothing at all. `press_origin` is where the
            // button went down. It also makes the drag exact: the delta below is
            // measured from the true press point rather than from wherever the
            // threshold happened to be crossed.
            self.dragging = None;
            let origin = response
                .ctx
                .input(|i| i.pointer.press_origin())
                .filter(|pos| !in_gutter(*pos));
            if let Some(pos) = origin {
                match self.press_intent(grid, track, Some(pos), &mods) {
                    // **Alt-drag duplicates, and the drag continues on the
                    // copies** — Neil's "drag-copy". The originals stay exactly
                    // where they are, which is what makes it a copy rather than a
                    // move: the clones become the selection and the `Move` below
                    // is handed *their* positions, which are the originals'. So
                    // the delta arithmetic is the same arithmetic, and there is no
                    // second code path for a copy-move.
                    Intent::Duplicate(id) => {
                        if !self.selected.contains(&id) {
                            self.selected.clear();
                            self.selected.insert(id);
                        }
                        let clones: Vec<Note> = track
                            .notes
                            .iter()
                            .filter(|n| self.selected.contains(&n.id))
                            .map(|n| {
                                let mut copy = n.clone();
                                // Two notes carrying one id makes the selection
                                // ambiguous, so `Note::reissue_id` exists for
                                // exactly this and for paste.
                                copy.reissue_id();
                                copy
                            })
                            .collect();
                        if !clones.is_empty() {
                            self.selected = clones.iter().map(|n| n.id).collect();
                            track.notes.extend(clones);
                            changed = true;
                            let group = self.grab(track);
                            // **The copies carry copies of the locks**, and that
                            // falls out of `PLockShift`'s own rule rather than
                            // needing a case here: the clones begin on their
                            // originals' steps, the originals are staying put, so
                            // those steps keep their locks and the copies take a
                            // copy. Same arithmetic, same as the positions.
                            let locks = self.hold_locks(track, &group);
                            self.dragging = Some(Drag::Move { anchor: pos, group, locks });
                        }
                    }
                    Intent::Resize(id) => {
                        // Same group rule as a move, and for the same reason.
                        if !self.selected.contains(&id) {
                            self.selected.clear();
                            self.selected.insert(id);
                        }
                        let items = self.stretch(track);
                        let start_len = items
                            .iter()
                            .find(|(other, _)| *other == id)
                            .map(|(_, e)| e.len);
                        if let Some(start_len) = start_len {
                            self.dragging =
                                Some(Drag::Resize { grabbed: id, start_len, items });
                        }
                    }
                    Intent::Move(id) => {
                        // **A note already in the selection keeps the group**, so
                        // the group can be dragged; a note outside it *becomes*
                        // the selection. Straight from the JS's `_down`, and it
                        // is what makes "select five, drag one" work at all.
                        if !self.selected.contains(&id) {
                            self.selected.clear();
                            self.selected.insert(id);
                        }
                        let group = self.grab(track);
                        if !group.is_empty() {
                            let locks = self.hold_locks(track, &group);
                            self.dragging = Some(Drag::Move { anchor: pos, group, locks });
                        }
                    }
                    Intent::Velocity(id) => {
                        // Same group rule as a move and a resize: a note already
                        // in the selection brings the selection, one outside it
                        // becomes the selection.
                        if !self.selected.contains(&id) {
                            self.selected.clear();
                            self.selected.insert(id);
                        }
                        let items = self.velocities(track);
                        if !items.is_empty() {
                            // The slider mirrors the note under your hand, which
                            // is `js/main.js`'s `onSelect` — and it happens at
                            // press, so the number is right before the drag has
                            // moved anything.
                            self.adopt_velocity(track, id);
                            self.dragging =
                                Some(Drag::Velocity { grabbed: id, anchor_y: pos.y, items });
                        }
                    }
                    Intent::Micro(id) => {
                        if let Some(note) = track.notes.iter().find(|n| n.id == id) {
                            let start = note.micro;
                            // Selected as well, so the note being nudged is the
                            // one drawn selected. Not the group: see `Drag::Micro`.
                            if !self.selected.contains(&id) {
                                self.selected.clear();
                                self.selected.insert(id);
                            }
                            self.dragging =
                                Some(Drag::Micro { grabbed: id, anchor_x: pos.x, start });
                        }
                    }
                    Intent::Marquee => {
                        let corner = pos - grid.origin;
                        self.dragging =
                            Some(Drag::Marquee { anchor: corner, current: corner });
                    }
                    // **Create-drag-to-length.** The note is stamped on the press
                    // and the gesture carries straight on into a resize of it, so
                    // one movement draws a four-step note. `js/pianoroll.js` does
                    // the same thing — `this.drag = this._resizeStart(n, { created:
                    // true })` — and it is why the plain drag on empty space was
                    // left doing nothing from Phase 5 until now rather than being
                    // spent on the marquee.
                    Intent::Create => {
                        if let Some(id) = self.create(grid, track, pos, harmony) {
                            changed = true;
                            let items = self.stretch(track);
                            let start_len =
                                items.iter().find(|(other, _)| *other == id).map(|(_, e)| e.len);
                            if let Some(start_len) = start_len {
                                self.dragging =
                                    Some(Drag::Resize { grabbed: id, start_len, items });
                            }
                        }
                    }
                    Intent::Nothing => {}
                }
            }
        }

        if let (Some(drag), Some(pos)) = (self.dragging.clone(), response.interact_pointer_pos())
        {
            match drag {
                Drag::Move { anchor, group, locks } => {
                    let step_delta = ((pos.x - anchor.x) / grid.cell.x).round() as f64;
                    let pitch_delta = -((pos.y - anchor.y) / grid.cell.y).round() as i32;

                    // **One delta for the whole selection, clamped so that no
                    // member leaves the grid** — the JS's rule, with the band's
                    // bounds substituted for its fixed `PITCH_MIN`/`PITCH_MAX`.
                    // Clamping each note on its own instead would squash the
                    // selection flat against the edge rather than stopping it,
                    // silently collapsing a chord into a unison.
                    let min_step =
                        group.iter().map(|g| g.step).fold(f64::INFINITY, f64::min);
                    let lowest = group.iter().map(|g| g.pitch).min().unwrap_or(0);
                    let highest = group.iter().map(|g| g.pitch).max().unwrap_or(0);
                    let step_delta = step_delta.max(-min_step);
                    // No upper step bound, unlike the JS's `lengthSteps - maxEnd`:
                    // a trig past the wrap line is representable here, the same
                    // call `core::import` made about not clamping note lengths.
                    let pitch_delta = pitch_delta.clamp(
                        grid.band.lo as i32 - lowest as i32,
                        grid.band.hi as i32 - highest as i32,
                    );

                    for g in &group {
                        if let Some(note) = track.notes.iter_mut().find(|n| n.id == g.id) {
                            let step = g.step + step_delta;
                            let pitch = (g.pitch as i32 + pitch_delta) as u8;
                            // Only report a change when something moved: a drag
                            // that holds still still fires every frame, and each
                            // report costs a snapshot.
                            if note.step != step || note.pitch != pitch {
                                note.step = step;
                                note.pitch = pitch;
                                changed = true;
                            }
                        }
                    }
                    // **The locks go where the trigs went**, off the *clamped*
                    // delta above rather than the raw one, so a group stopped by
                    // the edge of the grid does not leave its automation a step
                    // ahead of the trigs it belongs to.
                    changed |= locks.apply(track, step_delta);
                }
                Drag::Resize { grabbed, start_len, items } => {
                    // Everything is measured against the grabbed note, because
                    // that is the one under the pointer; the rest follow by the
                    // delta it travelled.
                    if let Some(note) = track.notes.iter().find(|n| n.id == grabbed) {
                        let from = note.step;
                        // Measured from where the note is *drawn*, so grabbing the
                        // visible right edge of a micro-nudged note does not
                        // immediately change its length by a step. `start_of` is
                        // `step + micro`, so at micro 0 this is exactly the
                        // arithmetic it replaced — which is why every resize test
                        // written before Phase 9 still passes unchanged.
                        let drawn = grid.x_of_step(grid.start_of(note));
                        let pattern_len = track.length_steps as f64;
                        // Room is measured from the *stored* step, not the drawn
                        // one: the wrap line is where the box stops playing, and it
                        // counts trigs rather than pixels.
                        let room = pattern_len - from;
                        // **Shift is fine mode**, and its floor is whatever the
                        // box can store rather than a whole step —
                        // `snap_len_fine` goes through `steps_to_length_byte`, so
                        // a fine drag shows exactly the length that will land on
                        // hardware rather than one that quietly rounds on write.
                        // Cells from the note's drawn start to the pointer.
                        let under = ((pos.x - drawn) / grid.cell.x) as f64;
                        let (wanted, opts) = if mods.shift {
                            (
                                snap_len_fine(under, room),
                                ResizeOpts::fine(
                                    pattern_len,
                                    snap_len_fine,
                                    snap_len_fine(0.0, room),
                                ),
                            )
                        } else {
                            // Whole steps, and `+ 1` because the pointer sitting
                            // on the note's own step means a length of one.
                            // `min` then `max`, in that order, so a note with
                            // less than a step of room left still comes out at 1
                            // rather than at whatever is left — the JS's
                            // `Math.max(1, Math.min(..., room))`.
                            let coarse = (under.floor() + 1.0).min(room).max(1.0);
                            (coarse, ResizeOpts::coarse(pattern_len))
                        };

                        // The group is clamped **once**, inside
                        // `resize_selection_by`, so a mix of long and short notes
                        // stays a mix. That rule and its argument live in `core`
                        // where they can be tested without a pointer.
                        let entries: Vec<LenEntry> = items.iter().map(|(_, e)| *e).collect();
                        let lens = resize_selection_by(&entries, wanted - start_len, &opts);
                        for ((id, _), len) in items.iter().zip(lens) {
                            if let Some(note) = track.notes.iter_mut().find(|n| n.id == *id) {
                                if note.len != len {
                                    note.len = len;
                                    changed = true;
                                }
                            }
                        }
                    }
                }
                Drag::Marquee { anchor, .. } => {
                    let current = pos - grid.origin;
                    self.dragging = Some(Drag::Marquee { anchor, current });
                    // Replaced wholesale every frame, as the JS's `setSelection`
                    // replaces it: shrinking the band has to let notes go again.
                    self.selected = marquee_hits(grid, track, marquee_rect(grid, anchor, current));
                }
                Drag::Velocity { grabbed, anchor_y, items } => {
                    // Up is harder, so the delta is *anchor minus current*. One
                    // pixel is one velocity unit, from the JS's
                    // `Math.round(this.drag.startY - e.clientY)` — not scaled by
                    // zoom, because velocity is not a distance on the grid.
                    let delta = (anchor_y - pos.y).round() as i32;
                    let starts: Vec<u8> = items.iter().map(|(_, v)| *v).collect();
                    // The group-delta rule and its per-note clamp live in `core`,
                    // where they can be argued about without a pointer — see
                    // `nudge_velocities` on why this clamp is not the resize's.
                    for ((id, _), velocity) in items.iter().zip(nudge_velocities(&starts, delta)) {
                        if let Some(note) = track.notes.iter_mut().find(|n| n.id == *id) {
                            if note.velocity != velocity {
                                note.velocity = velocity;
                                changed = true;
                            }
                        }
                    }
                    // The slider follows the drag, which is what makes the panel a
                    // readout as well as a control.
                    self.adopt_velocity(track, grabbed);
                    if let Some(note) = track.notes.iter().find(|n| n.id == grabbed) {
                        self.readout = Some((grabbed, format!("vel {}", note.velocity)));
                    }
                }
                Drag::Micro { grabbed, anchor_x, start } => {
                    // 0.01 of a step per pixel, from the JS. A full sweep of the
                    // window is about a hundred pixels, which is the resolution
                    // this wants: the whole range is a fifth of a cell either way.
                    let micro = clamp_micro(start + f64::from(pos.x - anchor_x) * 0.01);
                    if let Some(note) = track.notes.iter_mut().find(|n| n.id == grabbed) {
                        if note.micro != micro {
                            note.micro = micro;
                            changed = true;
                        }
                        // `+` written in, so `micro +0.12` and `micro -0.12` are
                        // the same width and read as a signed offset rather than
                        // as a position. ASCII only, per `ui::mod`'s glyph rule.
                        let sign = if note.micro >= 0.0 { "+" } else { "" };
                        self.readout =
                            Some((grabbed, format!("micro {sign}{:.2}", note.micro)));
                    }
                }
            }
        }

        if response.drag_stopped() {
            // **Adoption happens on release, never mid-drag.** A note that lands
            // on an occupied step joins that trig, because PROB/FILL/COND are per
            // trig on the box and every note sharing a step has to agree
            // (`edit_ops::adopt_step_trig`). This used to run on every frame the
            // note moved, so dragging a trig *past* an occupied step stamped that
            // step's conditions onto it permanently — a `2:4` acquired from a step
            // the note merely travelled through, which the encoder would then
            // write to hardware. `js/main.js` calls `adoptStepTrig` from
            // `onChange`, which the JS roll fires from `_up()`: on release, once
            // per gesture. Found by hand-testing; see the regression test below.
            match self.dragging.take() {
                Some(Drag::Move { group, .. }) => {
                    let moved: Vec<u32> = group.iter().map(|g| g.id).collect();
                    if adopt_step_trig(&mut track.notes, &moved) > 0 {
                        changed = true;
                    }
                }
                // Nothing to adopt after a resize: PROB/FILL/COND are per
                // *step*, and a resize moves no note onto a new one. That covers
                // the create-drag too, which is a resize of a note that already
                // adopted when it was stamped.
                Some(Drag::Resize { .. }) => {}
                // Nor after velocity or micro-timing: neither moves a note onto a
                // step it was not already on. Micro-timing looks like it might —
                // it is a horizontal offset — but the trig stays on its own step,
                // which is the whole reason the box stores it as a separate field.
                Some(Drag::Velocity { .. }) | Some(Drag::Micro { .. }) => {}
                // A band with no area is a press on empty space, and the JS's
                // `_up` makes that clear the selection.
                Some(Drag::Marquee { anchor, current }) if anchor == current => {
                    self.selected.clear();
                }
                Some(Drag::Marquee { .. }) | None => {}
            }
            // The velocity and micro readouts are live annotations on a gesture in
            // progress, so they go when it does.
            self.readout = None;
        }

        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos().filter(|pos| !in_gutter(*pos)) {
                match self.note_at(grid, track, pos) {
                    // **Alt+click deletes**, which is the click half of the
                    // alt-drag bargain. It was declined on its own from Phase 5
                    // until now on the grounds that half a gesture which silently
                    // does something else when the mouse moves is worse than no
                    // gesture; the drag half arrived in the same change, so it is
                    // whole. Tested before shift, matching the JS's `_down`.
                    Some(id) if mods.alt => changed |= self.delete(track, id),
                    // **Shift toggles membership.** The JS defers this to release
                    // because shift+drag is its velocity gesture and the two share
                    // a press. Here they are two handlers rather than one mode —
                    // egui only ever reports one of `clicked` and `drag_started` —
                    // so the click can act the moment it lands.
                    Some(id) if mods.shift => {
                        if !self.selected.remove(&id) {
                            self.selected.insert(id);
                            self.adopt_velocity(track, id);
                        }
                    }
                    Some(id) => {
                        self.selected.clear();
                        self.selected.insert(id);
                        self.adopt_velocity(track, id);
                    }
                    // A marquee that never moved: the JS's
                    // `x0 === x1 && y0 === y1` case, which clears rather than
                    // creating a note. Without this, cmd+click on empty space
                    // would stamp a trig you did not ask for.
                    None if mods.command => self.selected.clear(),
                    None => changed |= self.create(grid, track, pos, harmony).is_some(),
                }
            }
        }

        // **Say what the pointer is about to do.** A painted rect has no widget
        // to look pressable, so the cursor is the only affordance the roll gets —
        // and two of its gestures need one badly: the resize zone is seven pixels
        // of invisible target, and the band is invisible *state*, since holding
        // cmd is the only thing that distinguishes it from a drag that does
        // nothing at all.
        //
        // Read off [`Self::press_intent`] rather than worked out again here, so
        // the cursor cannot promise a band over a note that is about to be moved.
        // A drag in progress wins over whatever is under the pointer, so neither
        // icon flickers when the pointer outruns the gesture holding it — and
        // this runs **last**, after the frame's drag has been decided, so the icon
        // names the gesture that is actually running rather than trailing it by a
        // frame.
        // Phase 9 added three modifier gestures on a note's body, and **all three
        // are invisible without this**: nothing on screen distinguishes a note
        // that is about to be moved from one that is about to have its velocity
        // dragged. Each gets an icon that says which axis it works on, so the two
        // vertical/horizontal pairs cannot be confused with each other:
        //
        // | gesture | icon | why that one |
        // |---|---|---|
        // | resize | `ResizeHorizontal` | as before |
        // | velocity | `ResizeVertical` | it is a vertical drag |
        // | micro-timing | `ResizeColumn` | horizontal, and deliberately *not* the resize's arrow |
        // | duplicate | `Copy` | the one icon here that names the act rather than the axis |
        // | marquee | `Crosshair` | as before |
        //
        // **All five have been read off a real screen** — the first three by use
        // before Phase 9, and `ResizeVertical`, `ResizeColumn` and `Copy` confirmed
        // by Neil on macOS, 2026-08-18. That list is the point of this note, and it
        // is `ui::mod`'s glyph rule applied one layer over: `pass_cursor` proves the
        // roll *asked* egui for an icon and nothing whatever about what the platform
        // drew, so an icon nobody has looked at is exactly as much of a liability as
        // a glyph nobody has looked at. **Swap one of these for an untested variant
        // and the gesture it announces goes silent** — with every test still green,
        // because the test asserts the request.
        let hovering = response.hover_pos().filter(|p| !in_gutter(*p));
        let intent = self.press_intent(grid, track, hovering, &mods);
        let icon = match (&self.dragging, intent) {
            (Some(Drag::Resize { .. }), _) | (None, Intent::Resize(_)) => {
                Some(egui::CursorIcon::ResizeHorizontal)
            }
            (Some(Drag::Marquee { .. }), _) | (None, Intent::Marquee) => {
                Some(egui::CursorIcon::Crosshair)
            }
            (Some(Drag::Velocity { .. }), _) | (None, Intent::Velocity(_)) => {
                Some(egui::CursorIcon::ResizeVertical)
            }
            (Some(Drag::Micro { .. }), _) | (None, Intent::Micro(_)) => {
                Some(egui::CursorIcon::ResizeColumn)
            }
            // Only while the pointer is *waiting* on it: once alt-drag has cloned,
            // the drag in hand is a `Move`, and a copy cursor over a move would be
            // claiming a second copy is coming.
            (None, Intent::Duplicate(_)) => Some(egui::CursorIcon::Copy),
            // A move gets none: the arrow is right for dragging a thing about,
            // and `Grab` on a rect that is already moving reads as a scroll.
            // Nor does `Create`, which is the roll's ordinary click.
            _ => None,
        };
        if let Some(icon) = icon {
            response.ctx.set_cursor_icon(icon);
        }

        // **The pencil (A3).** Only `Create` gets one — `Intent::Move`
        // already returns the plain arrow above, which is the second half of
        // Neil's ask answered for free. No drag in progress (a pencil
        // trailing a move would be a second, wrong affordance on top of the
        // one already showing), and not in chord mode — the ghost is chord
        // draw's affordance, and a pencil over it would be two answers to one
        // question.
        //
        // **The image is sticky between frames**, so this has to *actively*
        // clear it on every frame it is not wanted — pointer in the gutter or
        // off the roll entirely (`intent` is `Nothing` there, since `hovering`
        // is already `None`), any intent other than `Create`, any drag, chord
        // mode. A pencil that only ever gets set and never cleared would
        // follow the pointer out over the Edit panel.
        let want_pencil = self.dragging.is_none() && !harmony.chord.on && matches!(intent, Intent::Create);
        if want_pencil {
            response.ctx.set_cursor_image(Some(pencil_cursor()));
        } else {
            // **Measured 2026-08-20: this `else` currently fails no test.**
            // `egui::Context::end_pass` (egui 0.36.1, `context.rs`) takes
            // `viewport.output` with `std::mem::take`, which resets
            // `cursor_icon`/`cursor_image` to their `Default` for the *next*
            // pass to accumulate into — so a pass that never calls
            // `set_cursor_image` already reports `None`, not last pass's
            // value. The "sticky between frames" doc comment this packet
            // cites belongs to `PlatformOutput::take` (`data/output.rs:242`),
            // a *different* method that `end_pass` does not call in this
            // version. Kept anyway: it matches what was asked for, it is
            // free, and it is the only line standing between a correct
            // cursor and a stale one if a future egui version — or a second
            // internal pass this file does not currently trigger — makes the
            // reset conditional instead of unconditional.
            response.ctx.set_cursor_image(None);
        }

        // **The hover box (A2).** Last, so it reads this frame's drag state —
        // `update_hover` itself suppresses while `self.dragging` is `Some`.
        self.update_hover(&response.ctx, response.ctx.input(|i| i.time), hovering, grid, track);

        changed
    }

    /// Stamp a note — or a whole chord — where the pointer is, join whatever trig
    /// is already on that step, and select what was made.
    ///
    /// One function for both halves of the create gesture: the click and the
    /// press-that-becomes-a-drag stamp the same note, so
    /// create-drag-to-length cannot end up drawing a different note from a plain
    /// click. `None` above or below the drawable rows, which is why a press off
    /// the top of the roll makes nothing rather than a note at pitch 0.
    ///
    /// **With chord draw on, one press stamps the voicing under the ghost.** The
    /// id it returns is the *top* note's, because the caller continues the gesture
    /// into a resize of it — and since the whole chord is the selection, dragging
    /// right lengthens every note of it. `js/pianoroll.js` returns `made.at(-1)`
    /// into `_resizeStart` for exactly that.
    ///
    /// A chord mode with nothing to stamp — every pitch of the voicing already on
    /// the step, or the trig full — falls back to the ordinary one-note click
    /// rather than doing nothing, which is what the JS does when `getChord` comes
    /// back empty. The ghost has already said so by drawing nothing.
    fn create(
        &mut self,
        grid: &Grid,
        track: &mut Track,
        pos: Pos2,
        harmony: &Harmony,
    ) -> Option<u32> {
        let pitch = grid.pitch_at(pos.y)?;
        let step = grid.step_at(pos.x).max(0.0);
        let chord = self.chord_at(grid, track, pos, harmony);
        let made: Vec<u32> = if chord.is_empty() {
            // The default the panel's slider holds, which is what makes velocity
            // settable for *new* notes as well as for the selection — the other half
            // of PLAN.md §9's headline.
            let note = Note::new(step, pitch, 1.0, self.default_velocity, 0.0);
            let id = note.id;
            track.notes.push(note);
            vec![id]
        } else {
            chord
                .iter()
                .map(|c| {
                    let note = Note::new(step, c.pitch, 1.0, c.velocity, c.micro);
                    let id = note.id;
                    track.notes.push(note);
                    id
                })
                .collect()
        };
        adopt_step_trig(&mut track.notes, &made);
        self.selected.clear();
        self.selected.extend(made.iter().copied());
        made.last().copied()
    }

    /// The selection with the velocities it currently holds, in the order
    /// [`nudge_velocities`] answers in — [`Self::stretch`]'s shape, for lengths'
    /// sibling.
    fn velocities(&self, track: &Track) -> Vec<(u32, u8)> {
        track
            .notes
            .iter()
            .filter(|n| self.selected.contains(&n.id))
            .map(|n| (n.id, n.velocity))
            .collect()
    }

    /// Take `id`'s velocity as the default for new notes.
    ///
    /// `js/main.js`'s `onSelect`: `state.defaultVelocity = note.velocity`. It is
    /// what makes the panel's slider a readout of the note under your hand as well
    /// as a control, and it means drawing a note after touching a soft one draws a
    /// soft note — which is the behaviour, not a side effect.
    fn adopt_velocity(&mut self, track: &Track, id: u32) {
        if let Some(note) = track.notes.iter().find(|n| n.id == id) {
            // Through the setter, for the reason it documents: a trig off a box can
            // be at velocity 0, and adopting that would make every note drawn
            // afterwards a note-off.
            self.set_default_velocity(note.velocity);
        }
    }

    /// Remove one note, and forget it everywhere the roll was holding onto it.
    ///
    /// One note, not the whole step: the JS deletes the note under the pointer,
    /// and a step holding a single note loses its trig by losing that note.
    /// Returns whether anything went, so the caller snapshots — a deleted trig
    /// that never reaches the engine keeps sounding.
    fn delete(&mut self, track: &mut Track, id: u32) -> bool {
        let before = track.notes.len();
        track.notes.retain(|n| n.id != id);
        self.selected.remove(&id);
        // A note deleted mid-drag would otherwise leave the group naming a note
        // that no longer exists — and, worse, still counting its pitch toward the
        // bounds the group is clamped by.
        if let Some(Drag::Move { group, .. }) = &mut self.dragging {
            group.retain(|g| g.id != id);
        }
        before != track.notes.len()
    }

    fn note_at(&self, grid: &Grid, track: &Track, pos: Pos2) -> Option<u32> {
        self.note_at_with_edge(grid, track, pos).map(|(id, _)| id)
    }

    /// The note under `pos`, and whether the pointer is in its right-edge grab
    /// zone — the JS's `nearEdge`.
    ///
    /// The zone is measured back from where the note **really ends on screen**
    /// (`x_of_step(start_of(n) + len)`), not from the right edge of the rect it is
    /// drawn in: [`Grid::note_rect`] floors a short note's width so it stays
    /// visible, and measuring from that floor would put the grab zone somewhere the
    /// note does not actually end.
    ///
    /// `start_of` rather than `step`, since Phase 9 — a micro-nudged note is drawn
    /// nudged, so its edge is nudged with it. Getting this wrong is what the
    /// regression test below found: the grab zone stayed at the stored step while
    /// the note moved off it, which is the JS's behaviour and is a wart there.
    fn note_at_with_edge(&self, grid: &Grid, track: &Track, pos: Pos2) -> Option<(u32, bool)> {
        track
            .notes
            .iter()
            .rev()
            .find(|n| grid.note_rect(n).contains(pos))
            .map(|n| (n.id, pos.x > grid.x_of_step(grid.start_of(n) + n.len) - EDGE_PX))
    }

    /// What a press at `pos` would start. `pos` is `None` for a press in the
    /// gutter, which begins nothing.
    ///
    /// The **order of these arms is load-bearing**, and Phase 9 filled it out to
    /// five gestures on one note. Reading down:
    ///
    /// 1. **alt** first, and over the whole note including the edge, because it is
    ///    the one modifier with no edge meaning to collide with;
    /// 2. **the right edge**, which always resizes — this is where the file parts
    ///    from `js/pianoroll.js`, whose modifiers are tested first; the header
    ///    argues it, and PLAN.md §9 settled the shift half of it;
    /// 3. **shift** on the body: velocity;
    /// 4. **cmd/ctrl** on the body: micro-timing;
    /// 5. the body: a move.
    ///
    /// Off a note, cmd bands and everything else creates. Note there is no arm
    /// left returning `Nothing` for a press inside the roll — every pixel of it
    /// now does something, which is what "all four gestures land in this pass"
    /// came to.
    fn press_intent(
        &self,
        grid: &Grid,
        track: &Track,
        pos: Option<Pos2>,
        mods: &egui::Modifiers,
    ) -> Intent {
        let Some(pos) = pos else {
            return Intent::Nothing;
        };
        match self.note_at_with_edge(grid, track, pos) {
            Some((id, _)) if mods.alt => Intent::Duplicate(id),
            Some((id, true)) => Intent::Resize(id),
            Some((id, false)) if mods.shift => Intent::Velocity(id),
            Some((id, false)) if mods.command => Intent::Micro(id),
            Some((id, false)) => Intent::Move(id),
            None if mods.command => Intent::Marquee,
            None => Intent::Create,
        }
    }

    // --- what the Edit panel reads and writes --------------------------------
    //
    // The panel edits the same track the roll does, so it needs the roll's
    // selection to aim at and the roll's default velocity to show. Both are
    // deliberately narrow: the panel never sees `dragging`, and it cannot replace
    // the selection — only read it — because a selection is made in the roll.

    /// The selected trigs, by id, in ascending id order.
    pub fn selection(&self) -> Vec<u32> {
        self.selected.iter().copied().collect()
    }

    /// The velocity a note drawn by hand will get. See the field.
    pub fn default_velocity(&self) -> u8 {
        self.default_velocity
    }

    /// Clamped onto the wire's range, which is not decoration.
    ///
    /// **A fetched pattern can carry velocity 0.** `protocol::track_notes` reads a
    /// trig's velocity as `sl.velocity & 0x7f`, so a byte of zero arrives as a note
    /// at zero — and until this clamp, clicking that note made
    /// [`Self::adopt_velocity`] take 0 as the default, after which **every note
    /// drawn by hand was a note-off**. Silent, and with the slider showing 1,
    /// because `SliderClamping::Always` clamps what it *displays* and not what it
    /// was handed. Found by a deliberate bug that failed nothing.
    pub fn set_default_velocity(&mut self, velocity: u8) {
        self.default_velocity = clamp_velocity(i32::from(velocity));
    }

    /// The multiple of [`CELL_W`]/[`CELL_H`] the grid is drawn at. What the Edit
    /// panel's VIEW slider reads, and the only place the number is ever shown.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Clamped to [`ZOOM_MIN`]..=[`ZOOM_MAX`] **here rather than by the slider**,
    /// for the reason on the field and on [`Self::set_default_velocity`]: a
    /// control that clamps its display is not a control that clamps its value.
    ///
    /// **The scroll is left alone**, so a slider zoom grows the grid from step 0
    /// of the band's top row while the wheel's holds the cell under the pointer
    /// still. That is not an omission — the wheel has a pointer over the grid to
    /// anchor on and a panel three hundred pixels to the left does not, and
    /// picking a cell for it to hold would be inventing an intent. Either way the
    /// view stays in bounds: `ui` clamps the scroll every frame, which is why
    /// this needs no `Band` and the panel needs to know nothing about one.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    }

    /// Forget the selection. Called when the track under the roll is replaced
    /// wholesale — a MIDI file imported over it, a clear, an undo — because ids
    /// from the music that was there name nothing in the music that is.
    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.readout = None;
    }

    /// Replace the selection — `js/pianoroll.js`'s `setSelection`.
    ///
    /// The JS calls it from `main.js` for the three things that make notes the user
    /// did not draw one at a time: paste, harmonise, and stamping a chord. **Two of
    /// the three are ported**: harmonising calls this from `ui::harmony` so the
    /// added notes join the melody in the selection, and a stamped chord sets the
    /// selection from inside [`Self::create`] because the press continues into a
    /// resize of it. Paste is still waiting on a caret.
    ///
    /// It existed before any of them, because [`Self::clear_selection`]'s claim
    /// could not otherwise be *tested* — a panel test asserting the selection is
    /// empty after an import passes trivially on a roll that never had one. That
    /// plant failed nothing, which is the finding `DEVELOPMENT.md` lesson 6 is about.
    pub fn select(&mut self, ids: impl IntoIterator<Item = u32>) {
        self.selected = ids.into_iter().collect();
    }

    /// The pitch range the roll can draw for `track`, which is what a chord is
    /// clamped to.
    ///
    /// Exposed because the Harmony panel harmonises a selection without a pointer
    /// ever touching the roll, and a chord tone the roll cannot draw is a note
    /// nobody could then edit. It is an associated function rather than a method:
    /// the band is the *track's*, and the roll only recomputes it per frame.
    pub fn band(track: &Track) -> (u8, u8) {
        Band::for_track(track).range()
    }

    /// The selection with the lengths it currently holds — `_resizeStart`'s
    /// items, in the order [`resize_selection_by`] will answer in.
    fn stretch(&self, track: &Track) -> Vec<(u32, LenEntry)> {
        track
            .notes
            .iter()
            .filter(|n| self.selected.contains(&n.id))
            .map(|n| (n.id, LenEntry::from(n)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::edit_ops::{MICRO_MAX, MICRO_MIN};
    use digi_core::TrackKind;

    /// A grid scrolled somewhere awkward, because the bug this file had was that
    /// drawing and hit-testing agreed only at the origin.
    fn grid() -> Grid {
        Grid {
            origin: Pos2 { x: 100.0 - 37.0, y: 50.0 - 512.0 },
            cell: Vec2 { x: CELL_W, y: CELL_H },
            band: Band { lo: PITCH_MIN, hi: PITCH_MAX },
        }
    }

    #[test]
    fn a_position_maps_to_the_cell_it_is_drawn_in() {
        let g = grid();
        for step in [0.0, 1.0, 7.0, 63.0] {
            let x = g.x_of_step(step);
            assert_eq!(g.step_at(x), step, "the left edge belongs to its own step");
            assert_eq!(g.step_at(x + CELL_W * 0.99), step);
            assert_eq!(g.step_at(x + CELL_W), step + 1.0);
        }
        for pitch in [PITCH_MIN, 36, 60, PITCH_MAX] {
            let y = g.y_of_pitch(pitch);
            assert_eq!(g.pitch_at(y), Some(pitch));
            assert_eq!(g.pitch_at(y + CELL_H * 0.99), Some(pitch));
        }
    }

    #[test]
    fn a_click_off_the_top_or_bottom_row_makes_no_note() {
        // `None` rather than a clamp: a note clamped to the edge of the band
        // lands somewhere nobody asked for.
        let g = grid();
        assert_eq!(g.pitch_at(g.y_of_pitch(PITCH_MAX) - 1.0), None, "above C8");
        assert_eq!(g.pitch_at(g.y_of_pitch(PITCH_MIN) + CELL_H), None, "below C2");
        // Far enough out that the float cast would saturate rather than wrap.
        assert_eq!(g.pitch_at(g.y_of_pitch(PITCH_MAX) - 10_000.0), None);
    }

    #[test]
    fn a_note_is_found_where_it_was_drawn() {
        let g = grid();
        let roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 60, 1.0, 100, 0.0), Note::new(9.0, 72, 2.0, 100, 0.0)];
        let wanted = track.notes[1].id;

        assert_eq!(roll.note_at(&g, &track, g.note_rect(&track.notes[1]).center()), Some(wanted));
        // Between the two, on neither.
        let gap = Pos2 { x: g.x_of_step(7.0), y: g.y_of_pitch(66) };
        assert_eq!(roll.note_at(&g, &track, gap), None);
    }

    /// A grid whose cells sit inside a test rect at 0,0, so a pointer event can
    /// land on a note: `y_of_pitch` counts down from the top of the band, so the
    /// pitches near C8 are the ones with small `y`. The gesture tests below drive
    /// [`TOP_PITCH`] (C7) — they used to drive 120, which is above C8 and has no
    /// row at all now.
    fn live_grid() -> Grid {
        Grid {
            origin: Pos2 { x: KEY_W, y: 0.0 },
            cell: Vec2 { x: CELL_W, y: CELL_H },
            band: Band { lo: PITCH_MIN, hi: PITCH_MAX },
        }
    }

    const TEST_RECT: Rect =
        Rect { min: Pos2 { x: 0.0, y: 0.0 }, max: Pos2 { x: 900.0, y: 600.0 } };

    /// Where to click for a given step and pitch, a little inside the cell.
    fn at(grid: &Grid, step: f64, pitch: u8) -> Pos2 {
        Pos2 { x: grid.x_of_step(step) + 4.0, y: grid.y_of_pitch(pitch) + 4.0 }
    }

    /// One headless egui pass driving the real `interact` path. The roll is
    /// allocated at [`TEST_RECT`] and handed `grid` directly, so the test controls
    /// the geometry instead of depending on how a `Ui` lays itself out.
    ///
    /// Same two gotchas as the trig lane's picker test: egui hit-tests against the
    /// *previous* pass's layout, so a press needs a layout pass before it can
    /// land; and the font-atlas delta has to be cleared or epaint's debug assert
    /// fires when it drops.
    fn pass(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        events: Vec<egui::Event>,
    ) -> bool {
        pass_with(ctx, roll, track, grid, egui::Modifiers::NONE, events)
    }

    /// The same, with a modifier held for the whole frame.
    ///
    /// `interact` reads the modifiers off the context, and the only thing that
    /// moves `InputState::modifiers` is an [`egui::Event::ModifiersChanged`] —
    /// `RawInput` has no modifiers field, and the ones hanging off a
    /// `PointerButton` event are not read for this. So the state is announced
    /// ahead of the frame's own events, which is what a real backend does.
    fn pass_with(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        modifiers: egui::Modifiers,
        events: Vec<egui::Event>,
    ) -> bool {
        run(ctx, roll, track, grid, modifiers, events).0
    }

    /// The cursor the roll asked for on this frame. egui reports it in
    /// `platform_output`, which makes an affordance that exists only to be
    /// *looked* at testable without anyone looking.
    fn pass_cursor(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        modifiers: egui::Modifiers,
        events: Vec<egui::Event>,
    ) -> egui::CursorIcon {
        run(ctx, roll, track, grid, modifiers, events).1
    }

    /// The bitmap cursor the roll asked for, if any — `pass_cursor`'s sibling
    /// for A3, since a `CustomCursorImage` request is a second, independent
    /// output alongside the `CursorIcon` and neither implies the other.
    fn pass_cursor_image(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        modifiers: egui::Modifiers,
        events: Vec<egui::Event>,
    ) -> Option<egui::CustomCursorImage> {
        run(ctx, roll, track, grid, modifiers, events).2
    }

    fn run(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        modifiers: egui::Modifiers,
        events: Vec<egui::Event>,
    ) -> (bool, egui::CursorIcon, Option<egui::CustomCursorImage>) {
        // Chord draw off, which is what every gesture test below assumes: with it
        // on, a click on an empty cell would stamp three notes instead of one.
        run_with(ctx, roll, track, grid, modifiers, &Harmony::default(), events)
    }

    /// The same, with a key and chord settings in hand.
    fn run_with(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        modifiers: egui::Modifiers,
        harmony: &Harmony,
        events: Vec<egui::Event>,
    ) -> (bool, egui::CursorIcon, Option<egui::CustomCursorImage>) {
        run_at(ctx, None, roll, track, grid, modifiers, harmony, events)
    }

    /// The same, with the frame's clock pinned to `time` when `Some` —
    /// otherwise egui stamps it with the wall clock, which is what every
    /// test not about the hover dwell wants. A2's dwell is measured in
    /// `egui::InputState::time`, and `RawInput::time` is the one place a
    /// test can set that without sleeping.
    fn run_at(
        ctx: &egui::Context,
        time: Option<f64>,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        modifiers: egui::Modifiers,
        harmony: &Harmony,
        events: Vec<egui::Event>,
    ) -> (bool, egui::CursorIcon, Option<egui::CustomCursorImage>) {
        let mut changed = false;
        let mut all = vec![egui::Event::ModifiersChanged(modifiers)];
        all.extend(events);
        let input = egui::RawInput {
            events: all,
            screen_rect: Some(TEST_RECT),
            time,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let response = ui.allocate_rect(TEST_RECT, egui::Sense::click_and_drag());
            changed = roll.interact(&response, grid, track, harmony);
        });
        let icon = output.platform_output.cursor_icon;
        let image = output.platform_output.cursor_image.clone();
        output.textures_delta.clear();
        (changed, icon, image)
    }

    /// A frame at an explicit clock time, with no modifier and chord draw
    /// off — `pass`'s sibling for the hover dwell tests, which need to
    /// control `egui::InputState::time` directly rather than sleep for real
    /// milliseconds.
    fn pass_at(
        ctx: &egui::Context,
        time: f64,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        events: Vec<egui::Event>,
    ) -> bool {
        run_at(ctx, Some(time), roll, track, grid, egui::Modifiers::NONE, &Harmony::default(), events).0
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        button_with(pos, pressed, egui::Modifiers::NONE)
    }

    fn button_with(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        }
    }

    /// A grid pushed down the test rect, so there is room above the band's top
    /// row to drag into.
    fn offset_grid() -> Grid {
        Grid {
            origin: Pos2 { x: KEY_W, y: 200.0 },
            cell: Vec2 { x: CELL_W, y: CELL_H },
            band: Band { lo: PITCH_MIN, hi: PITCH_MAX },
        }
    }

    /// Press at `from`, drag to `to`, release — with `modifiers` held throughout.
    ///
    /// **`to` has to be further from `from` than egui's drag threshold**, or no
    /// drag ever starts and the gesture arrives as a click instead. It fails by
    /// doing nothing at all, which is the most confusing way for it to fail.
    fn drag_between(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        modifiers: egui::Modifiers,
        from: Pos2,
        to: Pos2,
    ) {
        let go = |roll: &mut PianoRoll, track: &mut Track, events| {
            pass_with(ctx, roll, track, grid, modifiers, events)
        };
        go(roll, track, vec![]);
        go(
            roll,
            track,
            vec![egui::Event::PointerMoved(from), button_with(from, true, modifiers)],
        );
        go(roll, track, vec![egui::Event::PointerMoved(to)]);
        go(roll, track, vec![button_with(to, false, modifiers)]);
    }

    /// A note carrying conditions, so a test can watch whether they survive.
    fn conditioned(step: f64, pitch: u8, prob: u8, cond: &str) -> Note {
        let mut n = Note::new(step, pitch, 1.0, 100, 0.0);
        n.prob = Some(prob);
        n.cond = Some(cond.to_owned());
        n
    }

    /// Drag the note at `from` to `to`, passing through `via`, and return the
    /// dragged note's state afterwards.
    fn drag_note(track: &mut Track, grid: &Grid, pitch: u8, from: f64, via: f64, to: f64) {
        let ctx = egui::Context::default();
        let mut roll = PianoRoll::default();
        let start = at(grid, from, pitch);

        pass(&ctx, &mut roll, track, grid, vec![]);
        pass(&ctx, &mut roll, track, grid, vec![egui::Event::PointerMoved(start), button(start, true)]);
        // The frame the note is over `via` — where the bug used to strike.
        pass(&ctx, &mut roll, track, grid, vec![egui::Event::PointerMoved(at(grid, via, pitch))]);
        pass(&ctx, &mut roll, track, grid, vec![egui::Event::PointerMoved(at(grid, to, pitch))]);
        pass(&ctx, &mut roll, track, grid, vec![button(at(grid, to, pitch), false)]);
    }

    #[test]
    fn a_note_dragged_past_an_occupied_step_keeps_its_own_conditions() {
        // The bug Neil found by hand: adoption ran on every frame the note moved,
        // so travelling *over* an occupied step stamped that step's PROB/COND onto
        // the note for good. `js/main.js` adopts from `onChange`, which the JS
        // roll fires on release.
        let grid = live_grid();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![conditioned(0.0, TOP_PITCH, 25, "1:2"), conditioned(4.0, TOP_PITCH, 90, "PRE")];
        let travelling = track.notes[0].id;

        drag_note(&mut track, &grid, TOP_PITCH, 0.0, 4.0, 8.0);

        let moved = track.notes.iter().find(|n| n.id == travelling).expect("still there");
        assert_eq!(moved.step, 8.0, "the drag has to have actually moved it");
        assert_eq!(moved.prob, Some(25), "it passed over step 4; it did not join it");
        assert_eq!(moved.cond.as_deref(), Some("1:2"));
    }

    #[test]
    fn a_note_dropped_onto_an_occupied_step_still_joins_that_trig() {
        // The other half of the rule, which the fix must not lose: landing on an
        // occupied step *does* adopt, because PROB/FILL/COND are per trig and
        // every note sharing a step has to agree.
        let grid = live_grid();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![conditioned(0.0, TOP_PITCH, 25, "1:2"), conditioned(8.0, TOP_PITCH, 90, "PRE")];
        let arriving = track.notes[0].id;

        drag_note(&mut track, &grid, TOP_PITCH, 0.0, 4.0, 8.0);

        let dropped = track.notes.iter().find(|n| n.id == arriving).expect("still there");
        assert_eq!(dropped.step, 8.0);
        assert_eq!(dropped.prob, Some(90), "the incumbent wins on the step it lands on");
        assert_eq!(dropped.cond.as_deref(), Some("PRE"));
    }

    #[test]
    fn right_clicking_a_note_deletes_it() {
        // The roll could create trigs and never remove one until this landed.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(0.0, TOP_PITCH, 1.0, 100, 0.0), Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0)];
        let doomed = track.notes[1].id;
        let pos = at(&grid, 4.0, TOP_PITCH);
        let secondary = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(pos), secondary(true)]);
        let changed = pass(&ctx, &mut roll, &mut track, &grid, vec![secondary(false)]);

        assert!(changed, "a delete has to be reported, or the trig keeps sounding");
        assert_eq!(track.notes.len(), 1, "one note, not the step and not everything");
        assert!(track.notes.iter().all(|n| n.id != doomed));
    }

    #[test]
    fn the_delete_key_removes_the_selected_note_and_nothing_when_none_is_selected() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(0.0, TOP_PITCH, 1.0, 100, 0.0)];
        let key = |k| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };

        // Nothing selected: the key is not a "delete whatever is under the mouse".
        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        assert!(!pass(&ctx, &mut roll, &mut track, &grid, vec![key(egui::Key::Delete)]));
        assert_eq!(track.notes.len(), 1);

        roll.selected.insert(track.notes[0].id);
        assert!(pass(&ctx, &mut roll, &mut track, &grid, vec![key(egui::Key::Backspace)]));
        assert!(track.notes.is_empty());
        assert!(roll.selected.is_empty(), "the selection cannot outlive its notes");
    }

    #[test]
    fn the_key_column_is_c2_to_c8_and_names_them_the_way_the_box_does() {
        // Derived by running `js/pianoroll.js`'s own `noteName` under node
        // against its own PITCH_MIN/PITCH_MAX, not re-reasoned here.
        assert_eq!((PITCH_MIN, PITCH_MAX), (24, 96));
        assert_eq!(Band { lo: PITCH_MIN, hi: PITCH_MAX }.rows(), 73, "not 128");
        assert_eq!(note_name(PITCH_MIN), "C2");
        assert_eq!(note_name(PITCH_MAX), "C8");

        // **MIDI 60 is C5, not C4.** The boxes number octaves this way, and a key
        // column that disagreed with the DT2's own screen by an octave would make
        // the roll wrong about the one thing it exists to tell you.
        assert_eq!(note_name(60), "C5");

        // The seven labelled rows, top down — every other row is left blank.
        let labelled: Vec<String> = (PITCH_MIN..=PITCH_MAX)
            .rev()
            .filter(|p| p % 12 == 0)
            .map(note_name)
            .collect();
        assert_eq!(labelled, ["C8", "C7", "C6", "C5", "C4", "C3", "C2"]);

        assert_eq!(note_name(61), "C#5", "the sharps are ASCII, so no new glyphs");
    }

    #[test]
    fn the_black_keys_are_the_five_of_every_octave() {
        // `js/pianoroll.js`'s `BLACK` set, and the tint that makes the grid rows
        // readable as a keyboard rather than as 73 identical stripes.
        let octave: Vec<(String, bool)> =
            (60u8..72).map(|p| (note_name(p), is_black(p))).collect();
        let black: Vec<&str> =
            octave.iter().filter(|(_, b)| *b).map(|(n, _)| n.as_str()).collect();
        assert_eq!(black, ["C#5", "D#5", "F#5", "G#5", "A#5"]);
        assert_eq!(octave.iter().filter(|(_, b)| !*b).count(), 7);
    }

    #[test]
    fn an_imported_note_outside_c2_c8_gets_a_row_instead_of_vanishing() {
        // The one decision here the JS cannot answer, because its roll can never
        // hold such a note: `protocol::track_notes` reads a trig's pitch as
        // `sl.note & 0x7f`, so a fetched pattern can arrive with any of the 128.
        // A fixed band would draw it nowhere while the engine kept playing it.
        let mut track = Track::new(0, TrackKind::Audio);
        assert_eq!(
            Band::for_track(&track),
            Band { lo: PITCH_MIN, hi: PITCH_MAX },
            "an empty track is C2-C8 and nothing more"
        );

        track.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
        assert_eq!(
            Band::for_track(&track),
            Band { lo: PITCH_MIN, hi: PITCH_MAX },
            "a note inside the band does not widen it"
        );

        track.notes.push(Note::new(1.0, 12, 1.0, 100, 0.0));
        track.notes.push(Note::new(2.0, 110, 1.0, 100, 0.0));
        let band = Band::for_track(&track);
        assert_eq!(band, Band { lo: 12, hi: 110 });

        // Having a row is the whole point: it has to be hit-testable, not merely
        // counted.
        let g = Grid {
            origin: Pos2 { x: KEY_W, y: 0.0 },
            cell: Vec2 { x: CELL_W, y: CELL_H },
            band,
        };
        assert_eq!(g.pitch_at(g.y_of_pitch(12)), Some(12));
        assert_eq!(g.pitch_at(g.y_of_pitch(110)), Some(110));
        assert_eq!(g.pitch_at(g.y_of_pitch(110) - 1.0), None, "and still bounded");
    }

    #[test]
    fn a_note_dragged_off_the_top_of_the_band_stops_at_c8() {
        // The JS clamps a move to `PITCH_MIN - g.minPitch` .. `PITCH_MAX -
        // g.maxPitch`. Without the clamp the note lands above the highest row
        // drawn, which is a trig you can hear and cannot see.
        let ctx = egui::Context::default();
        // Origin pushed down the rect so there is room above C8 to drag into.
        let grid = Grid {
            origin: Pos2 { x: KEY_W, y: 200.0 },
            cell: Vec2 { x: CELL_W, y: CELL_H },
            band: Band { lo: PITCH_MIN, hi: PITCH_MAX },
        };
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0)];

        let start = at(&grid, 4.0, TOP_PITCH);
        // Twenty rows up from C7 is well past C8.
        let end = Pos2 { x: start.x, y: start.y - 20.0 * CELL_H };
        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(start), button(start, true)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(end)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![button(end, false)]);

        assert_eq!(track.notes[0].pitch, PITCH_MAX, "stopped at C8, not above it");
    }

    /// Three notes spread over three rows, for the selection tests.
    fn spread(track: &mut Track) -> (u32, u32, u32) {
        track.notes = vec![
            Note::new(2.0, 84, 1.0, 100, 0.0),
            Note::new(4.0, 83, 1.0, 100, 0.0),
            Note::new(10.0, 80, 1.0, 100, 0.0),
        ];
        (track.notes[0].id, track.notes[1].id, track.notes[2].id)
    }

    fn ids(roll: &PianoRoll) -> Vec<u32> {
        roll.selected.iter().copied().collect()
    }

    #[test]
    fn a_marquee_takes_every_trig_its_band_crosses_and_leaves_the_rest() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let (a, b, c) = spread(&mut track);

        // A band drawn from empty space over the first two rows only.
        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::COMMAND,
            Pos2 { x: grid.x_of_step(1.5), y: grid.y_of_pitch(84) - 4.0 },
            Pos2 { x: grid.x_of_step(5.5), y: grid.y_of_pitch(83) + 4.0 },
            );

        let mut want = vec![a, b];
        want.sort();
        assert_eq!(ids(&roll), want, "both rows the band crossed");
        assert!(!roll.selected.contains(&c), "and not the row it never reached");
    }

    #[test]
    fn a_marquee_is_strict_on_every_edge_where_egui_would_not_be() {
        // `_inMarquee` tests with `<` and `>`, never `<=`. egui's own
        // `Rect::intersects` is closed on both ends, which is why `marquee_hits`
        // is written out rather than delegating to it — the last assertion here
        // is the proof that the difference is real and not imagined.
        let g = live_grid();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 1.0, 100, 0.0)];
        let id = track.notes[0].id;
        let b = g.note_band(&track.notes[0]);

        let touching =
            Rect::from_min_max(Pos2 { x: b.min.x - 20.0, y: b.min.y }, Pos2 { x: b.min.x, y: b.max.y });
        assert!(marquee_hits(&g, &track, touching).is_empty(), "touching is not crossing");

        let crossing = Rect::from_min_max(
            Pos2 { x: b.min.x - 20.0, y: b.min.y },
            Pos2 { x: b.min.x + 1.0, y: b.max.y },
        );
        assert_eq!(marquee_hits(&g, &track, crossing), BTreeSet::from([id]));

        // A band of no area away from every note takes nothing — but note what
        // is *not* claimed here. A degenerate band inside a note's own row does
        // match it, because the strict test is `nx0 < x1 && nx1 > x0` and a point
        // strictly inside satisfies both. That is the JS's behaviour too, and it
        // is why the JS clears a zero-movement marquee in `_up` with an explicit
        // `x0 === x1 && y0 === y1` check rather than leaning on the geometry.
        let elsewhere = Pos2 { x: b.min.x - 40.0, y: b.min.y };
        assert!(marquee_hits(&g, &track, Rect::from_min_max(elsewhere, elsewhere)).is_empty());
        assert_eq!(
            marquee_hits(&g, &track, Rect::from_min_max(b.center(), b.center())),
            BTreeSet::from([id]),
            "and a point inside the row does match, which is why the clear is explicit"
        );

        assert!(touching.intersects(b), "egui would have taken the grazed note");
    }

    #[test]
    fn a_marquee_tests_the_whole_row_not_the_drawn_note() {
        // The JS draws a note inset inside its row and still marquees against the
        // full `CELL_H`. A band that had to enclose the drawn pixels would miss
        // notes whose row it plainly crosses.
        let g = live_grid();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 1.0, 100, 0.0)];
        let note = &track.notes[0];
        let id = note.id;

        // A sliver along the bottom of the row, below the drawn rect entirely.
        let below = g.note_rect(note).max.y;
        let sliver = Rect::from_min_max(
            Pos2 { x: g.x_of_step(4.0) + 1.0, y: below + 0.5 },
            Pos2 { x: g.x_of_step(5.0) - 1.0, y: g.note_band(note).max.y },
        );
        assert!(sliver.min.y > g.note_rect(note).max.y, "genuinely below the drawn note");
        assert_eq!(marquee_hits(&g, &track, sliver), BTreeSet::from([id]));
    }

    #[test]
    fn dragging_one_trig_of_a_selection_moves_the_whole_selection() {
        // The payoff of the set, and the JS's rule: a press on a note that is
        // already selected keeps the group instead of narrowing to that note.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let (a, b, _) = spread(&mut track);
        roll.selected.extend([a, b]);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            at(&grid, 2.0, 84),
            at(&grid, 5.0, 84),
        );

        let note = |id: u32| track.notes.iter().find(|n| n.id == id).expect("still there");
        assert_eq!(note(a).step, 5.0, "the one under the pointer");
        assert_eq!(note(b).step, 7.0, "and the other, by the same delta");
        assert_eq!(note(b).pitch, 83, "the shape is kept, not flattened");
        assert!(roll.selected.contains(&b), "and the group survives the drag");
    }

    #[test]
    fn a_group_move_stops_at_the_band_edge_without_flattening_the_chord() {
        // Clamping each note on its own would squash the selection against C8 —
        // two notes four semitones apart arriving as a unison, which is an edit
        // nobody asked for and which the engine would happily play.
        let ctx = egui::Context::default();
        let grid = offset_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes =
            vec![Note::new(4.0, 84, 1.0, 100, 0.0), Note::new(4.0, 80, 1.0, 100, 0.0)];
        let (high, low) = (track.notes[0].id, track.notes[1].id);
        roll.selected.extend([high, low]);

        let from = at(&grid, 4.0, 84);
        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            from,
            Pos2 { x: from.x, y: from.y - 20.0 * CELL_H },
        );

        let note = |id: u32| track.notes.iter().find(|n| n.id == id).expect("still there");
        assert_eq!(note(high).pitch, PITCH_MAX, "the top of the group reaches C8");
        assert_eq!(note(low).pitch, PITCH_MAX - 4, "and the interval is intact");
    }

    #[test]
    fn shift_clicking_toggles_a_trig_in_and_out_of_the_selection() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let (a, b, _) = spread(&mut track);
        let shift = egui::Modifiers::SHIFT;
        let click = |roll: &mut PianoRoll, track: &mut Track, pos: Pos2| {
            pass_with(&ctx, roll, track, &grid, shift, vec![]);
            pass_with(
                &ctx,
                roll,
                track,
                &grid,
                shift,
                vec![egui::Event::PointerMoved(pos), button_with(pos, true, shift)],
            );
            pass_with(&ctx, roll, track, &grid, shift, vec![button_with(pos, false, shift)]);
        };

        click(&mut roll, &mut track, at(&grid, 2.0, 84));
        assert_eq!(ids(&roll), vec![a]);

        click(&mut roll, &mut track, at(&grid, 4.0, 83));
        let mut both = vec![a, b];
        both.sort();
        assert_eq!(ids(&roll), both, "shift adds rather than replacing");

        click(&mut roll, &mut track, at(&grid, 2.0, 84));
        assert_eq!(ids(&roll), vec![b], "and clicking it again takes it back out");
        assert_eq!(track.notes.len(), 3, "no note was created or deleted by any of it");
    }

    #[test]
    fn the_delete_key_removes_every_selected_trig() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let (a, b, c) = spread(&mut track);
        roll.selected.extend([a, c]);

        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        let changed = pass(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            vec![egui::Event::Key {
                key: egui::Key::Delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );

        assert!(changed);
        assert_eq!(track.notes.len(), 1, "both went, not just one");
        assert_eq!(track.notes[0].id, b, "and the unselected one stayed");
        assert!(roll.selected.is_empty());
    }

    #[test]
    fn a_plain_drag_on_empty_space_creates_a_note_and_sets_its_length() {
        // **Create-drag-to-length**, and the gesture this reserved drag was being
        // held for since Phase 5. One movement stamps a note and stretches it: the
        // press creates, the drag resizes the thing it just created.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        spread(&mut track);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            at(&grid, 1.0, 84),
            at(&grid, 5.0, 84),
        );

        assert_eq!(track.notes.len(), 4, "one note stamped, not one per frame");
        let made = track.notes.last().expect("the new note");
        assert_eq!(made.step, 1.0);
        assert_eq!(made.pitch, 84);
        assert_eq!(made.len, 5.0, "the pointer landed on step 5, so five steps long");
        assert_eq!(ids(&roll), vec![made.id], "and it is what is selected");
    }

    #[test]
    fn a_plain_click_on_empty_space_still_makes_the_one_step_note_it_always_did() {
        // The other half of the same gesture. A click and a drag have to stamp the
        // *same* note — they go through one `create` for that reason — and a click
        // must not have quietly become a four-step note.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let pos = at(&grid, 3.0, 84);

        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(pos), button(pos, true)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![button(pos, false)]);

        assert_eq!(track.notes.len(), 1);
        assert_eq!((track.notes[0].step, track.notes[0].len), (3.0, 1.0));
    }

    #[test]
    fn a_created_note_takes_the_default_velocity_rather_than_a_hard_coded_hundred() {
        // The half of PLAN.md §9's headline that is about *new* notes. Before this,
        // every note this app had ever written to a box went out at 100 because
        // `Note::new` was handed a literal.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        assert_eq!(roll.default_velocity(), 100, "what it was hard-coded to");

        roll.set_default_velocity(40);
        let pos = at(&grid, 3.0, 84);
        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(pos), button(pos, true)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![button(pos, false)]);

        assert_eq!(track.notes[0].velocity, 40);
    }

    #[test]
    fn a_press_off_the_top_of_the_band_creates_nothing_rather_than_a_note_at_pitch_zero() {
        // `create` inherits `pitch_at`'s `None`, which is what stops a float cast
        // saturating a press far above the roll into pitch 0.
        let ctx = egui::Context::default();
        let grid = offset_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let above = Pos2 { x: grid.x_of_step(4.0) + 4.0, y: grid.origin.y - 40.0 };

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            above,
            Pos2 { x: above.x + 80.0, y: above.y },
        );

        assert!(track.notes.is_empty());
    }

    #[test]
    fn a_modified_click_on_empty_space_clears_rather_than_creating_a_trig() {
        // The JS's zero-movement marquee, which ends in `clearSelection`. Without
        // this branch the same click would stamp a trig nobody asked for.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let (a, _, _) = spread(&mut track);
        roll.selected.insert(a);
        let cmd = egui::Modifiers::COMMAND;
        let pos = Pos2 { x: grid.x_of_step(1.5), y: grid.y_of_pitch(84) - 4.0 };

        pass_with(&ctx, &mut roll, &mut track, &grid, cmd, vec![]);
        pass_with(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            cmd,
            vec![egui::Event::PointerMoved(pos), button_with(pos, true, cmd)],
        );
        let changed =
            pass_with(&ctx, &mut roll, &mut track, &grid, cmd, vec![button_with(pos, false, cmd)]);

        assert!(roll.selected.is_empty());
        assert_eq!(track.notes.len(), 3, "no trig stamped");
        assert!(!changed, "clearing a selection is not an edit and costs no snapshot");
    }

    /// A position inside a note's right-edge grab zone.
    fn edge_of(grid: &Grid, step: f64, len: f64, pitch: u8) -> Pos2 {
        Pos2 { x: grid.x_of_step(step + len) - 2.0, y: grid.y_of_pitch(pitch) + 4.0 }
    }

    #[test]
    fn the_grab_zone_is_measured_from_where_the_note_really_ends() {
        // Not from the right edge of the rect it is *drawn* in: `note_rect` floors
        // a short note's width so it stays visible, and measuring from that floor
        // would put the zone past where the note ends.
        let g = live_grid();
        let roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 4.0, 100, 0.0)];
        let y = g.y_of_pitch(TOP_PITCH) + 4.0;
        let end = g.x_of_step(8.0);

        let body = Pos2 { x: end - EDGE_PX - 1.0, y };
        let edge = Pos2 { x: end - 1.0, y };
        assert_eq!(roll.note_at_with_edge(&g, &track, body).map(|(_, e)| e), Some(false));
        assert_eq!(roll.note_at_with_edge(&g, &track, edge).map(|(_, e)| e), Some(true));

        // A note shorter than the zone is all edge, as it is in the JS.
        track.notes = vec![Note::new(4.0, TOP_PITCH, 0.125, 100, 0.0)];
        let inside = Pos2 { x: g.x_of_step(4.0) + 1.0, y };
        assert_eq!(roll.note_at_with_edge(&g, &track, inside).map(|(_, e)| e), Some(true));
    }

    #[test]
    fn dragging_a_notes_right_edge_lengthens_it_in_whole_steps() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0)];

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            edge_of(&grid, 4.0, 1.0, TOP_PITCH),
            Pos2 { x: grid.x_of_step(8.0) + 4.0, y: grid.y_of_pitch(TOP_PITCH) + 4.0 },
        );

        // The pointer sitting on step 8 for a note starting at step 4 means five
        // steps, not four — the JS's `pos.step - n.step + 1`.
        assert_eq!(track.notes[0].len, 5.0);
        assert_eq!(track.notes[0].step, 4.0, "a resize moves nothing");
        assert_eq!(track.notes[0].pitch, TOP_PITCH);
    }

    #[test]
    fn a_resize_carries_the_selection_and_keeps_a_mix_of_lengths_mixed() {
        // The point of `resize_selection_by`'s single group clamp: long and short
        // notes stay long and short instead of being levelled.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes =
            vec![Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0), Note::new(4.0, 80, 3.0, 100, 0.0)];
        let (short, long) = (track.notes[0].id, track.notes[1].id);
        roll.selected.extend([short, long]);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            edge_of(&grid, 4.0, 1.0, TOP_PITCH),
            Pos2 { x: grid.x_of_step(8.0) + 4.0, y: grid.y_of_pitch(TOP_PITCH) + 4.0 },
        );

        let len = |id: u32| track.notes.iter().find(|n| n.id == id).expect("there").len;
        assert_eq!(len(short), 5.0, "the one under the pointer");
        assert_eq!(len(long), 7.0, "and the other by the same delta, not to the same length");
    }

    #[test]
    fn a_shift_resize_snaps_to_a_length_the_box_can_actually_store() {
        // Fine mode goes through `lengths::snap_len_fine`, which snaps on the
        // hardware LEN scale via `steps_to_length_byte` — so what the roll shows
        // is what a write would put on the box, rather than a number that rounds
        // silently later. 2.1 steps is not storable; 2.125 is.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0)];

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::SHIFT,
            edge_of(&grid, 4.0, 1.0, TOP_PITCH),
            // 6.1 steps under the pointer, so 2.1 steps of length asked for. Far
            // enough from the edge to clear egui's drag threshold, which is the
            // trap here: a shorter reach registers as a click and the resize
            // never starts at all.
            Pos2 { x: grid.origin.x + 6.1 * CELL_W, y: grid.y_of_pitch(TOP_PITCH) + 4.0 },
        );

        assert_eq!(track.notes[0].len, 2.125, "snapped up the LEN scale, not left at 2.1");
        // Coarse mode at that same position would have said three whole steps, so
        // this cannot pass by accident with shift ignored.
        assert_ne!(track.notes[0].len, 3.0);
    }

    #[test]
    fn a_resize_stops_at_the_end_of_the_pattern() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        assert_eq!(track.length_steps, 16, "the room this test is about");
        track.notes = vec![Note::new(14.0, TOP_PITCH, 1.0, 100, 0.0)];

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            edge_of(&grid, 14.0, 1.0, TOP_PITCH),
            Pos2 { x: grid.x_of_step(30.0), y: grid.y_of_pitch(TOP_PITCH) + 4.0 },
        );

        assert_eq!(track.notes[0].len, 2.0, "up to the wrap and no further");
    }

    #[test]
    fn a_resize_stops_when_any_member_of_the_group_runs_out_of_room() {
        // `resize_selection_by`'s group ceiling, and the bargain `core` documents
        // with it: the grabbed note stops following the pointer once *another*
        // member hits the end, because the alternative is levelling the very
        // differences this mode exists to keep.
        //
        // This is also the only thing here that reads `ResizeOpts::length_steps`.
        // The single-note case is already bounded by the coarse formula's own
        // `room`, so `a_resize_stops_at_the_end_of_the_pattern` would still pass
        // with the pattern length replaced by nonsense — found by trying exactly
        // that, which is why this test exists.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes =
            vec![Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0), Note::new(14.0, 80, 1.0, 100, 0.0)];
        let (grabbed, penned_in) = (track.notes[0].id, track.notes[1].id);
        roll.selected.extend([grabbed, penned_in]);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            edge_of(&grid, 4.0, 1.0, TOP_PITCH),
            Pos2 { x: grid.x_of_step(12.0) + 4.0, y: grid.y_of_pitch(TOP_PITCH) + 4.0 },
        );

        let len = |id: u32| track.notes.iter().find(|n| n.id == id).expect("there").len;
        assert_eq!(len(penned_in), 2.0, "it had one step of room and took it");
        assert_eq!(
            len(grabbed),
            2.0,
            "and the grabbed note stopped with it, though the pointer went to step 12"
        );
    }

    #[test]
    fn a_fine_resize_is_bounded_by_the_group_too_and_gives_up_its_fraction() {
        // Fine mode has its own `ResizeOpts`, and nothing bound them until this
        // test — the coarse pair was covered by the group ceiling above while
        // `ResizeOpts::fine`'s `length_steps` could be replaced by nonsense and
        // every test still passed. Found by trying it.
        //
        // The behaviour is worth seeing as well as pinning: a member with one
        // whole step of room left drags the fine request back to a whole step, so
        // the fraction asked for is simply not available to the group.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes =
            vec![Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0), Note::new(14.0, 80, 1.0, 100, 0.0)];
        let (grabbed, penned_in) = (track.notes[0].id, track.notes[1].id);
        roll.selected.extend([grabbed, penned_in]);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::SHIFT,
            edge_of(&grid, 4.0, 1.0, TOP_PITCH),
            // 2.125 steps asked for, as in the single-note fine test.
            Pos2 { x: grid.origin.x + 6.1 * CELL_W, y: grid.y_of_pitch(TOP_PITCH) + 4.0 },
        );

        let len = |id: u32| track.notes.iter().find(|n| n.id == id).expect("there").len;
        assert_eq!(len(penned_in), 2.0, "all the room it had");
        assert_eq!(len(grabbed), 2.0, "and the fraction went with it, not just the length");
    }

    #[test]
    fn a_resize_that_reverses_lands_where_the_pointer_is_not_where_it_has_been() {
        // The delta is measured from the length the drag *began* with, so pulling
        // out and coming back lands on the length under the pointer rather than
        // accumulating every frame's movement.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0)];
        let y = grid.y_of_pitch(TOP_PITCH) + 4.0;
        let start = edge_of(&grid, 4.0, 1.0, TOP_PITCH);
        let far = Pos2 { x: grid.x_of_step(12.0) + 4.0, y };
        let back = Pos2 { x: grid.x_of_step(5.0) + 4.0, y };

        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(start), button(start, true)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(far)]);
        assert_eq!(track.notes[0].len, 9.0, "out to step 12 first");
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(back)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![button(back, false)]);

        assert_eq!(track.notes[0].len, 2.0, "and back to what step 5 means");
    }

    #[test]
    fn a_press_on_a_notes_body_still_moves_it_rather_than_resizing_it() {
        // The guard on the whole gesture: the edge zone must not swallow an
        // ordinary drag, or moving a note becomes impossible for short ones.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 4.0, 100, 0.0)];

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            at(&grid, 4.0, TOP_PITCH),
            at(&grid, 7.0, TOP_PITCH),
        );

        assert_eq!(track.notes[0].step, 7.0, "it moved");
        assert_eq!(track.notes[0].len, 4.0, "and kept its length");
    }

    #[test]
    fn the_cursor_names_the_gesture_the_pointer_is_actually_on() {
        // Two of this roll's gestures are invisible. The resize zone is seven
        // pixels of unmarked target, and the band is unmarked *state* — holding
        // cmd is the only thing separating it from a drag that does nothing at
        // all. A painted rect has no widget to look pressable, so the cursor is
        // the only affordance either one gets.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 4.0, 100, 0.0)];
        let y = grid.y_of_pitch(TOP_PITCH) + 4.0;
        let (cmd, none) = (egui::Modifiers::COMMAND, egui::Modifiers::NONE);
        let empty = Pos2 { x: grid.x_of_step(20.0), y };
        let body = at(&grid, 4.0, TOP_PITCH);
        let edge = edge_of(&grid, 4.0, 4.0, TOP_PITCH);

        // Two passes per probe: egui hit-tests against the previous layout, so a
        // hover needs a frame to land in.
        let cursor = |roll: &mut PianoRoll, track: &mut Track, mods, pos| {
            pass_with(&ctx, roll, track, &grid, mods, vec![]);
            pass_cursor(&ctx, roll, track, &grid, mods, vec![egui::Event::PointerMoved(pos)])
        };

        assert_eq!(
            cursor(&mut roll, &mut track, cmd, empty),
            egui::CursorIcon::Crosshair,
            "cmd over empty space is the one place a press starts a band"
        );
        assert_ne!(
            cursor(&mut roll, &mut track, none, empty),
            egui::CursorIcon::Crosshair,
            "without cmd that same drag does nothing, so it must not offer a band"
        );
        assert_eq!(cursor(&mut roll, &mut track, none, edge), egui::CursorIcon::ResizeHorizontal);

        // **The pair that a second copy of the decision would get wrong.** cmd is
        // held both times, but a press here resizes or moves — it does not band.
        assert_eq!(
            cursor(&mut roll, &mut track, cmd, edge),
            egui::CursorIcon::ResizeHorizontal,
            "cmd does not turn a note's edge into a band"
        );
        assert_ne!(
            cursor(&mut roll, &mut track, cmd, body),
            egui::CursorIcon::Crosshair,
            "nor a note's body"
        );
    }

    #[test]
    fn a_band_in_progress_keeps_its_cursor_over_the_trigs_it_sweeps() {
        // Otherwise the icon flickers between crosshair and arrow as the band
        // crosses notes, which is worse than having no icon: it reads as the
        // gesture being dropped and picked up again.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 4.0, 100, 0.0)];
        let cmd = egui::Modifiers::COMMAND;
        let from = Pos2 { x: grid.x_of_step(20.0), y: grid.y_of_pitch(TOP_PITCH) + 4.0 };
        let over_a_note = at(&grid, 4.0, TOP_PITCH);

        pass_with(&ctx, &mut roll, &mut track, &grid, cmd, vec![]);
        pass_with(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            cmd,
            vec![egui::Event::PointerMoved(from), button_with(from, true, cmd)],
        );
        // The frame the band starts, and it starts on top of a note.
        let icon = pass_cursor(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            cmd,
            vec![egui::Event::PointerMoved(over_a_note)],
        );

        assert!(
            matches!(roll.dragging, Some(Drag::Marquee { .. })),
            "the band really is running, or this asserts nothing"
        );
        assert_eq!(icon, egui::CursorIcon::Crosshair);
    }

    // --- Phase 9: velocity, duplicate, micro-timing --------------------------
    //
    // **No oracle covered any of these.** `test/pianoroll.test.js` tests exactly
    // one function — `noteName` — so the JS's `vel`, `micro` and `alt` modes were
    // untested on the far side of the port too. Checked by grep before a line was
    // written, per `DEVELOPMENT.md`. Each test below therefore pins a claim read out
    // of `js/pianoroll.js` and says which.

    /// Drag `note` vertically by `dy` pixels with shift held. Negative is up,
    /// which is harder.
    fn velocity_drag(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        from: Pos2,
        dy: f32,
    ) {
        drag_between(
            ctx,
            roll,
            track,
            grid,
            egui::Modifiers::SHIFT,
            from,
            Pos2 { x: from.x, y: from.y + dy },
        );
    }

    #[test]
    fn shift_dragging_a_notes_body_upward_makes_it_harder() {
        // `js/pianoroll.js`: `Math.round(this.drag.startY - e.clientY)`, so one
        // pixel up is one unit louder.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 60, 0.0)];

        velocity_drag(&ctx, &mut roll, &mut track, &grid, at(&grid, 4.0, 84), -20.0);
        assert_eq!(track.notes[0].velocity, 80);
    }

    #[test]
    fn shift_dragging_down_makes_it_softer_and_never_reaches_zero() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 60, 0.0)];

        velocity_drag(&ctx, &mut roll, &mut track, &grid, at(&grid, 4.0, 84), 20.0);
        assert_eq!(track.notes[0].velocity, 40);

        // All the way down: 1, because 0 is a note-off on the wire.
        velocity_drag(&ctx, &mut roll, &mut track, &grid, at(&grid, 4.0, 84), 400.0);
        assert_eq!(track.notes[0].velocity, 1);
    }

    #[test]
    fn a_velocity_drag_carries_the_selection_by_one_delta_and_keeps_a_mix_mixed() {
        // **PLAN.md §9's exit criterion.** "A group selection behaving like the
        // resize does — the group delta rule, not levelling."
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![
            Note::new(0.0, 84, 1.0, 100, 0.0),
            Note::new(4.0, 83, 1.0, 60, 0.0),
            Note::new(8.0, 82, 1.0, 30, 0.0),
        ];
        roll.selected = track.notes.iter().map(|n| n.id).collect();

        velocity_drag(&ctx, &mut roll, &mut track, &grid, at(&grid, 4.0, 83), -10.0);

        let velocities: Vec<u8> = track.notes.iter().map(|n| n.velocity).collect();
        assert_eq!(velocities, [110, 70, 40], "one delta, not one value");
    }

    #[test]
    fn a_velocity_drag_that_reverses_lands_where_the_pointer_is() {
        // The same claim the resize's own reversal test makes, and for the same
        // reason: the delta is measured off the velocities the drag *began* with,
        // so it cannot accumulate a frame at a time.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 60, 0.0)];
        let from = at(&grid, 4.0, 84);
        let mods = egui::Modifiers::SHIFT;
        let go = |roll: &mut PianoRoll, track: &mut Track, events| {
            pass_with(&ctx, roll, track, &grid, mods, events);
        };

        go(&mut roll, &mut track, vec![]);
        go(&mut roll, &mut track, vec![egui::Event::PointerMoved(from), button_with(from, true, mods)]);
        go(&mut roll, &mut track, vec![egui::Event::PointerMoved(Pos2 { x: from.x, y: from.y - 30.0 })]);
        assert_eq!(track.notes[0].velocity, 90, "part-way through");
        go(&mut roll, &mut track, vec![egui::Event::PointerMoved(Pos2 { x: from.x, y: from.y - 5.0 })]);
        assert_eq!(track.notes[0].velocity, 65, "back to where the pointer is, not 90 + 25");
    }

    #[test]
    fn shift_on_a_notes_edge_is_still_a_fine_resize_and_not_a_velocity_drag() {
        // **The collision PLAN.md §9 names as the one to check first.** Shift means
        // fine-resize on the right edge and velocity on the body, and this is where
        // the roll parts from `js/pianoroll.js`, which tests its modifiers before
        // `nearEdge` and so gives velocity in both places.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 60, 0.0)];
        let edge = edge_of(&grid, 4.0, 4.0, 84);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::SHIFT,
            edge,
            Pos2 { x: grid.x_of_step(6.5), y: edge.y },
        );

        assert_eq!(track.notes[0].velocity, 60, "the edge does not set velocity");
        assert_eq!(track.notes[0].len, snap_len_fine(2.5, 12.0), "it fine-resizes");
    }

    #[test]
    fn a_velocity_drag_leaves_the_notes_step_pitch_and_length_alone() {
        // A vertical drag on a note body is a move in every other roll, so the one
        // thing this must not do is move it.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 60, 0.0)];

        velocity_drag(&ctx, &mut roll, &mut track, &grid, at(&grid, 4.0, 84), -30.0);
        let n = &track.notes[0];
        assert_eq!((n.step, n.pitch, n.len), (4.0, 84, 4.0));
    }

    #[test]
    fn the_default_velocity_follows_the_note_your_hand_is_on() {
        // `js/main.js`'s `onSelect`: `state.defaultVelocity = note.velocity`, which
        // is what makes the panel's slider a readout as well as a control — and
        // what makes the *next* note drawn match the one just touched.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 33, 0.0)];
        let pos = at(&grid, 4.0, 84);

        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(pos), button(pos, true)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![button(pos, false)]);

        assert_eq!(roll.default_velocity(), 33, "clicking a note adopts its velocity");

        velocity_drag(&ctx, &mut roll, &mut track, &grid, pos, -10.0);
        assert_eq!(roll.default_velocity(), 43, "and the drag keeps it in step");
    }

    #[test]
    fn a_trig_off_a_box_at_velocity_zero_does_not_silence_every_note_drawn_after_it() {
        // **Found by a deliberate bug that failed nothing.** `protocol::track_notes`
        // reads velocity as `sl.velocity & 0x7f`, so a fetched pattern can hold a
        // note at 0. Clicking it made that the default for new notes, and 0 is a
        // note-off on the wire — so every note drawn afterwards was silent, with the
        // panel's slider showing 1 because it clamps what it draws rather than what
        // it is handed.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 1.0, 0, 0.0)];
        let pos = at(&grid, 4.0, 84);

        pass(&ctx, &mut roll, &mut track, &grid, vec![]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(pos), button(pos, true)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![button(pos, false)]);

        assert_eq!(roll.default_velocity(), 1, "clamped onto the wire's range, not adopted raw");

        // And the note drawn next actually sounds.
        let empty = at(&grid, 9.0, 84);
        pass(&ctx, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(empty), button(empty, true)]);
        pass(&ctx, &mut roll, &mut track, &grid, vec![button(empty, false)]);
        let made = track.notes.iter().find(|n| n.step == 9.0).expect("the new note");
        assert!(made.velocity >= 1, "a note at velocity 0 is a note-off");
    }

    #[test]
    fn the_default_velocity_cannot_be_set_outside_the_wires_range() {
        let mut roll = PianoRoll::default();
        roll.set_default_velocity(0);
        assert_eq!(roll.default_velocity(), 1);
        roll.set_default_velocity(200);
        assert_eq!(roll.default_velocity(), 127);
    }

    #[test]
    fn alt_dragging_a_note_leaves_the_original_and_moves_a_copy() {
        // Neil's "drag-copy". The originals stay put and the *copies* travel, which
        // is the difference between this and a move.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 2.0, 70, 0.0)];
        let original = track.notes[0].id;

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::ALT,
            at(&grid, 4.0, 84),
            at(&grid, 8.0, 84),
        );

        assert_eq!(track.notes.len(), 2);
        let stayed = track.notes.iter().find(|n| n.id == original).expect("the original");
        assert_eq!((stayed.step, stayed.pitch), (4.0, 84), "the original did not move");
        let copy = track.notes.iter().find(|n| n.id != original).expect("the copy");
        assert_eq!((copy.step, copy.pitch, copy.len, copy.velocity), (8.0, 84, 2.0, 70));
        assert_eq!(ids(&roll), vec![copy.id], "and the copy is what is selected");
    }

    /// A track with one editable lane holding a value on each named slot.
    fn with_lane(track: &mut Track, at: &[(usize, u16)]) {
        let mut values = vec![None; 128];
        for &(slot, v) in at {
            values[slot] = Some(v);
        }
        track.plocks = vec![digi_core::PLockLane::new(
            Some(String::from("filter.cutoff")),
            None,
            Some(String::from("DT2")),
            false,
            values,
        )
        .unwrap()];
    }

    /// The slots a lane holds a value on.
    fn lane_locks(track: &Track) -> Vec<(usize, u16)> {
        track.plocks[0]
            .values
            .iter()
            .enumerate()
            .filter_map(|(slot, v)| v.map(|v| (slot, v)))
            .collect()
    }

    #[test]
    fn dragging_a_trig_takes_its_p_locks_with_it() {
        // The roll moved the trig and left the lock behind on the step it came
        // off, so the sweep belonged to whatever trig turned up there next and
        // the moved trig had none. Reported by Neil, 2026-08-20. The rules are
        // `edit_ops::PLockShift`'s; this is the gesture actually carrying them.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(2.0, 84, 1.0, 100, 0.0)];
        with_lane(&mut track, &[(2, 100)]);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            at(&grid, 2.0, 84),
            at(&grid, 6.0, 84),
        );

        assert_eq!(track.notes[0].step, 6.0, "the trig moved");
        assert_eq!(lane_locks(&track), [(6, 100)], "and the lock is on it, not on step 2");
    }

    #[test]
    fn a_group_move_carries_every_moved_trigs_locks_by_the_clamped_delta() {
        // Two trigs, two locks, and a delta the group's own clamp decides — the
        // locks have to travel exactly as far as the notes did or the automation
        // ends up a step away from the trig it belongs to.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes =
            vec![Note::new(2.0, 84, 1.0, 100, 0.0), Note::new(5.0, 83, 1.0, 100, 0.0)];
        with_lane(&mut track, &[(2, 30), (5, 90)]);
        roll.selected.extend(track.notes.iter().map(|n| n.id));

        // Dragged left, further than the earliest trig can go: the clamp stops
        // the group at step 0 and both locks stop with it.
        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            at(&grid, 2.0, 84),
            at(&grid, 0.0, 84),
        );

        let steps: Vec<f64> = track.notes.iter().map(|n| n.step).collect();
        assert_eq!(steps, [0.0, 3.0]);
        assert_eq!(lane_locks(&track), [(0, 30), (3, 90)]);
    }

    #[test]
    fn an_alt_drag_copies_the_locks_and_leaves_the_originals() {
        // The copy lands on a trig of its own, so it needs the lock the original
        // was playing; the original has not moved, so it keeps its own.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 1.0, 100, 0.0)];
        with_lane(&mut track, &[(4, 77)]);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::ALT,
            at(&grid, 4.0, 84),
            at(&grid, 8.0, 84),
        );

        assert_eq!(lane_locks(&track), [(4, 77), (8, 77)]);
    }

    #[test]
    fn neither_a_resize_nor_a_velocity_drag_moves_a_lock() {
        // Locks are per step, and neither gesture moves a note onto a new one.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 1.0, 100, 0.0)];
        with_lane(&mut track, &[(4, 55)]);

        let edge = Pos2 { x: grid.x_of_step(5.0) - 2.0, y: at(&grid, 4.0, 84).y };
        drag_between(&ctx, &mut roll, &mut track, &grid, egui::Modifiers::NONE, edge, at(&grid, 9.0, 84));
        assert_eq!(lane_locks(&track), [(4, 55)], "a resize leaves it alone");

        let body = at(&grid, 4.0, 84);
        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::SHIFT,
            body,
            Pos2 { x: body.x, y: body.y - 20.0 },
        );
        assert_eq!(lane_locks(&track), [(4, 55)], "and so does a velocity drag");
    }

    #[test]
    fn a_copy_gets_its_own_id_so_the_selection_is_never_ambiguous() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 2.0, 70, 0.0)];
        let original = track.notes[0].id;

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::ALT,
            at(&grid, 4.0, 84),
            at(&grid, 8.0, 84),
        );

        let ids: Vec<u32> = track.notes.iter().map(|n| n.id).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        assert!(ids.contains(&original));
    }

    #[test]
    fn alt_dragging_one_trig_of_a_selection_copies_the_whole_selection() {
        // The same group rule the move and the velocity drag follow: a note already
        // in the selection brings the selection with it.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![
            Note::new(0.0, 84, 1.0, 100, 0.0),
            Note::new(0.0, 83, 1.0, 100, 0.0),
            Note::new(8.0, 82, 1.0, 100, 0.0),
        ];
        roll.selected = track.notes[..2].iter().map(|n| n.id).collect();

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::ALT,
            at(&grid, 0.0, 84),
            at(&grid, 4.0, 84),
        );

        assert_eq!(track.notes.len(), 5, "two copied, three left as they were");
        let at_four: Vec<u8> =
            track.notes.iter().filter(|n| n.step == 4.0).map(|n| n.pitch).collect();
        assert_eq!(at_four, [84, 83], "the chord kept its shape");
        assert!(track.notes.iter().filter(|n| n.step == 0.0).count() == 2, "and stayed put");
    }

    #[test]
    fn a_copy_carries_the_trig_conditions_of_the_note_it_came_from() {
        // A copy that shed its `2:4` would be a different trig, and the encoder
        // would write the difference to hardware.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![conditioned(4.0, 84, 40, "2:4")];

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::ALT,
            at(&grid, 4.0, 84),
            at(&grid, 9.0, 84),
        );

        let copy = track.notes.iter().find(|n| n.step == 9.0).expect("the copy");
        assert_eq!((copy.prob, copy.cond.as_deref()), (Some(40), Some("2:4")));
    }

    #[test]
    fn alt_clicking_a_note_deletes_it() {
        // The click half of the alt bargain, which was declined on its own from
        // Phase 5 until the drag half arrived in the same change.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 2.0, 70, 0.0), Note::new(9.0, 83, 1.0, 70, 0.0)];
        let pos = at(&grid, 4.0, 84);
        let alt = egui::Modifiers::ALT;
        let go = |roll: &mut PianoRoll, track: &mut Track, events| {
            pass_with(&ctx, roll, track, &grid, alt, events)
        };

        go(&mut roll, &mut track, vec![]);
        go(&mut roll, &mut track, vec![egui::Event::PointerMoved(pos), button_with(pos, true, alt)]);
        let changed = go(&mut roll, &mut track, vec![button_with(pos, false, alt)]);

        assert!(changed, "a deleted trig has to reach the engine or it keeps sounding");
        assert_eq!(track.notes.len(), 1);
        assert_eq!(track.notes[0].step, 9.0);
    }

    #[test]
    fn alt_clicking_empty_space_makes_a_note_the_way_a_plain_click_does() {
        // `js/pianoroll.js` checks `altKey` only when a note is under the pointer,
        // so alt on empty space falls through to the create. Kept, because the
        // alternative is a modifier that silently does nothing.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let pos = at(&grid, 3.0, 84);
        let alt = egui::Modifiers::ALT;

        pass_with(&ctx, &mut roll, &mut track, &grid, alt, vec![]);
        pass_with(&ctx, &mut roll, &mut track, &grid, alt, vec![egui::Event::PointerMoved(pos), button_with(pos, true, alt)]);
        pass_with(&ctx, &mut roll, &mut track, &grid, alt, vec![button_with(pos, false, alt)]);

        assert_eq!(track.notes.len(), 1);
    }

    #[test]
    fn alt_wins_over_the_right_edge_where_shift_and_cmd_do_not() {
        // Alt is the one modifier with no edge meaning of its own, so it takes the
        // whole note — the JS's ordering, kept here because there is nothing to
        // collide with.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 70, 0.0)];
        let edge = edge_of(&grid, 4.0, 4.0, 84);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::ALT,
            edge,
            Pos2 { x: edge.x + CELL_W * 4.0, y: edge.y },
        );

        assert_eq!(track.notes.len(), 2, "alt on the edge copies rather than resizing");
        assert!(track.notes.iter().all(|n| n.len == 4.0), "and nothing was stretched");
    }

    #[test]
    fn cmd_dragging_a_notes_body_sideways_sets_micro_timing() {
        // `js/pianoroll.js`: 0.01 of a step per pixel.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 70, 0.0)];
        let from = at(&grid, 4.0, 84);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::COMMAND,
            from,
            Pos2 { x: from.x + 20.0, y: from.y },
        );

        assert_eq!(track.notes[0].micro, 0.2);
        assert_eq!(track.notes[0].step, 4.0, "the trig stays on its own step");
    }

    #[test]
    fn micro_timing_stops_short_of_the_neighbouring_step_in_both_directions() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 70, 0.0)];
        let from = at(&grid, 4.0, 84);
        let sweep = |roll: &mut PianoRoll, track: &mut Track, dx: f32| {
            drag_between(
                &ctx,
                roll,
                track,
                &grid,
                egui::Modifiers::COMMAND,
                from,
                Pos2 { x: from.x + dx, y: from.y },
            );
        };

        sweep(&mut roll, &mut track, 400.0);
        assert_eq!(track.notes[0].micro, MICRO_MAX);
        track.notes[0].micro = 0.0;
        sweep(&mut roll, &mut track, -400.0);
        assert_eq!(track.notes[0].micro, MICRO_MIN);
    }

    #[test]
    fn a_micro_drag_moves_one_note_even_with_a_selection_behind_it() {
        // **Deliberately not the group rule.** `js/pianoroll.js`'s `micro` mode
        // holds a single `note` where its `vel` mode holds `items`, and the reason
        // is `js/chords.js`: a strum is per-note micro offsets *within* one chord,
        // which a group gesture could not make.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(0.0, 84, 1.0, 100, 0.0), Note::new(0.0, 83, 1.0, 100, 0.0)];
        roll.selected = track.notes.iter().map(|n| n.id).collect();
        let from = at(&grid, 0.0, 84);

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::COMMAND,
            from,
            Pos2 { x: from.x + 20.0, y: from.y },
        );

        assert_eq!(track.notes[0].micro, 0.2);
        assert_eq!(track.notes[1].micro, 0.0, "the chord-mate is untouched");
    }

    #[test]
    fn a_nudged_note_is_grabbed_where_it_is_drawn() {
        // The JS's one real wart here, fixed rather than ported: its `_pos` finds a
        // note by whole step and measures `nearEdge` from `n.step + n.len`, so in
        // the browser a note nudged half a step right is grabbed half a cell left
        // of where it appears. One `Grid`, used by drawing and by hit-testing, is
        // this file's whole reason for existing.
        let grid = live_grid();
        let roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 70, 0.4)];
        let note = &track.notes[0];

        // Where it is drawn: step 4.4 through 8.4.
        let drawn_start = Pos2 { x: grid.x_of_step(4.4) + 2.0, y: grid.y_of_pitch(84) + 4.0 };
        assert_eq!(roll.note_at(&grid, &track, drawn_start), Some(note.id));

        // And the grab zone is at the drawn end, not at step 8.
        let drawn_edge = Pos2 { x: grid.x_of_step(8.4) - 2.0, y: drawn_start.y };
        assert_eq!(roll.note_at_with_edge(&grid, &track, drawn_edge), Some((note.id, true)));
        let unnudged_edge = Pos2 { x: grid.x_of_step(8.0) - 2.0, y: drawn_start.y };
        assert_eq!(
            roll.note_at_with_edge(&grid, &track, unnudged_edge),
            Some((note.id, false)),
            "step 8 is the middle of this note now, not its edge"
        );
    }

    #[test]
    fn resizing_a_nudged_note_does_not_jump_its_length_by_a_step() {
        // The consequence of the above that would have been a bug: grabbing the
        // drawn edge has to mean "leave the length alone", not "add one".
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 84, 4.0, 70, 0.4)];
        let edge = Pos2 { x: grid.x_of_step(8.4) - 2.0, y: grid.y_of_pitch(84) + 4.0 };

        drag_between(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            edge,
            Pos2 { x: grid.x_of_step(10.4) - 2.0, y: edge.y },
        );

        assert_eq!(track.notes[0].len, 6.0, "two steps further along is two steps longer");
    }

    #[test]
    fn a_louder_note_is_drawn_brighter_than_a_quieter_one() {
        // The velocity drag's only visible answer. Without this, PLAN.md §9's
        // headline gesture would move a number nothing on screen reports — the same
        // class of failure as a tofu glyph, and just as invisible to a test suite
        // that does not look.
        let quiet = PianoRoll::note_fill(1, false);
        let loud = PianoRoll::note_fill(127, false);
        assert!(
            loud.r() > quiet.r() && loud.g() > quiet.g() && loud.b() > quiet.b(),
            "{loud:?} is not brighter than {quiet:?}"
        );
        // And selection stays a *hue*, so the two readings do not compete.
        assert!(PianoRoll::note_fill(100, true).b() > PianoRoll::note_fill(100, false).b());
    }

    #[test]
    fn the_cursor_learns_the_three_new_zones() {
        // PLAN.md §9: "the cursor must learn the two new zones or both gestures are
        // invisible". Three, in the end — duplicate came with its own. Each icon
        // names the axis the gesture works on, so the two horizontal ones are not
        // the same arrow.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 4.0, 100, 0.0)];
        let body = at(&grid, 4.0, TOP_PITCH);
        let edge = edge_of(&grid, 4.0, 4.0, TOP_PITCH);
        let cursor = |roll: &mut PianoRoll, track: &mut Track, mods, pos| {
            pass_with(&ctx, roll, track, &grid, mods, vec![]);
            pass_cursor(&ctx, roll, track, &grid, mods, vec![egui::Event::PointerMoved(pos)])
        };

        assert_eq!(
            cursor(&mut roll, &mut track, egui::Modifiers::SHIFT, body),
            egui::CursorIcon::ResizeVertical,
            "shift on a body is velocity, which is a vertical drag"
        );
        assert_eq!(
            cursor(&mut roll, &mut track, egui::Modifiers::COMMAND, body),
            egui::CursorIcon::ResizeColumn,
            "cmd on a body is micro-timing — horizontal, and not the resize's arrow"
        );
        assert_eq!(
            cursor(&mut roll, &mut track, egui::Modifiers::ALT, body),
            egui::CursorIcon::Copy,
        );
        // And the collision, both ways round: the edge keeps its own gesture under
        // shift and cmd, and gives it up to alt.
        assert_eq!(
            cursor(&mut roll, &mut track, egui::Modifiers::SHIFT, edge),
            egui::CursorIcon::ResizeHorizontal,
        );
        assert_eq!(
            cursor(&mut roll, &mut track, egui::Modifiers::COMMAND, edge),
            egui::CursorIcon::ResizeHorizontal,
        );
        assert_eq!(cursor(&mut roll, &mut track, egui::Modifiers::ALT, edge), egui::CursorIcon::Copy);
    }

    // ----------------------------------------------------- A1: velocity bar

    #[test]
    fn the_velocity_bar_grows_with_velocity() {
        // Factored geometry, not pixels: the note's own rect, and the four
        // velocities the packet asks for, including both ends of the range.
        let rect = Rect::from_min_size(Pos2 { x: 0.0, y: 0.0 }, Vec2 { x: 20.0, y: 100.0 });
        let heights: Vec<f32> = [1u8, 40, 80, 127]
            .into_iter()
            .map(|v| {
                let bar = PianoRoll::velocity_bar_rect(rect, v).expect("wide enough to draw");
                bar.height()
            })
            .collect();
        assert!(
            heights.windows(2).all(|w| w[0] < w[1]),
            "not monotonic: {heights:?}"
        );
        // 127 fills everything the stroke does not cover — full is full, for
        // the part of the note that is actually the note.
        assert_eq!(*heights.last().unwrap(), rect.height() - 1.0);
    }

    /// The bar stops short of the note's bottom edge, and that one pixel is the
    /// whole feature at the quiet end.
    ///
    /// `paint_notes` strokes the note with `StrokeKind::Middle`, centring a 1px
    /// white line *on* `rect.max.y`. A bar flush to `rect.max.y` therefore has
    /// its bottom pixel repainted white immediately afterwards — invisible at
    /// velocity 127, and the entire bar at velocity 1. Photographed on a real
    /// screen on 2026-08-20: a note at velocity 3 sampled as a single flat
    /// `#006EAE` down its whole interior, with no bar on any row, while every
    /// test in this file passed. Geometry tests cannot see a later draw call
    /// paint over an earlier one; this asserts the clearance that makes the
    /// overpaint impossible.
    #[test]
    fn the_bar_clears_the_stroke_that_frames_the_note() {
        let rect = Rect::from_min_size(Pos2 { x: 0.0, y: 0.0 }, Vec2 { x: 20.0, y: 100.0 });
        for v in [1u8, 3, 40, 127] {
            let bar = PianoRoll::velocity_bar_rect(rect, v).expect("wide enough to draw");
            assert!(
                bar.max.y <= rect.max.y - 1.0,
                "velocity {v}: the bar's bottom ({}) must clear the 1px stroke centred on {}",
                bar.max.y,
                rect.max.y
            );
        }
    }

    #[test]
    fn the_quietest_note_still_shows_a_sliver() {
        // Without the floor, velocity 1 on a short note rounds to zero height
        // and reads identically to an empty one.
        let rect = Rect::from_min_size(Pos2 { x: 0.0, y: 0.0 }, Vec2 { x: 20.0, y: 5.0 });
        let bar = PianoRoll::velocity_bar_rect(rect, 1).expect("wide enough to draw");
        assert_eq!(bar.height(), 1.0, "the floor, not zero");
    }

    #[test]
    fn a_note_too_narrow_for_an_inset_bar_disappears_rather_than_clips() {
        // The smallest note the roll draws — a couple of pixels wide once the
        // 1px-a-side inset is taken out. `None`, not a negative-width rect.
        let rect = Rect::from_min_size(Pos2 { x: 0.0, y: 0.0 }, Vec2 { x: 1.5, y: 10.0 });
        assert_eq!(PianoRoll::velocity_bar_rect(rect, 100), None);
    }

    // -------------------------------------------------------- A2: hover box

    /// A note carrying every optional field, so a test can see all of them
    /// land in the box at once.
    fn richly_conditioned(step: f64, pitch: u8) -> Note {
        let mut n = Note::new(step, pitch, 2.0, 90, 0.12);
        n.prob = Some(50);
        n.fill = Some(true);
        n.cond = Some("1:2".to_owned());
        n
    }

    #[test]
    fn hover_lines_report_every_field_the_note_carries_and_omit_the_rest() {
        let full = richly_conditioned(0.0, 60);
        assert_eq!(
            PianoRoll::hover_lines(&full),
            vec![
                "vel 90".to_string(),
                "len 2".to_string(),
                "micro +0.12".to_string(),
                "PROB 50".to_string(),
                "FILL ON".to_string(),
                "COND 1:2".to_string(),
            ]
        );

        // A bare note: no micro, no PROB, no FILL, no COND — omitted, not
        // printed as a placeholder. An absent trig condition is not a value.
        let bare = Note::new(0.0, 60, 1.0, 100, 0.0);
        assert_eq!(PianoRoll::hover_lines(&bare), vec!["vel 100".to_string(), "len 1".to_string()]);

        // FILL OFF is still a value the trig carries — `cycle_fill`'s middle
        // state — and gets its own line rather than being folded into
        // "absent".
        let mut off = Note::new(0.0, 60, 1.0, 100, 0.0);
        off.fill = Some(false);
        assert!(PianoRoll::hover_lines(&off).contains(&"FILL OFF".to_string()));
    }

    #[test]
    fn the_hover_box_waits_for_the_pointer_to_settle() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![richly_conditioned(4.0, TOP_PITCH)];
        let wanted = track.notes[0].id;
        let pos = at(&grid, 4.0, TOP_PITCH);

        // A layout pass before the pointer lands — egui hit-tests against the
        // *previous* pass's layout, the same gotcha every gesture test above
        // works around.
        pass_at(&ctx, 0.0, &mut roll, &mut track, &grid, vec![]);
        pass_at(&ctx, 0.0, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(pos)]);
        assert!(roll.hover_box.is_none(), "not yet — the dwell has not elapsed");

        // Same position, no new event needed: egui's pointer state is sticky
        // between frames, exactly the property that lets the dwell clock work.
        pass_at(&ctx, PianoRoll::HOVER_DWELL, &mut roll, &mut track, &grid, vec![]);
        let (id, lines) = roll.hover_box.clone().expect("dwell elapsed, pointer held still");
        assert_eq!(id, wanted);
        assert_eq!(lines, PianoRoll::hover_lines(&track.notes[0]));
    }

    #[test]
    fn nothing_is_shown_while_a_drag_is_in_progress() {
        // The drag readout owns the annotation job while a gesture is
        // running — two boxes naming one note is worse than either alone.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 4.0, 100, 0.0)];
        let start = at(&grid, 4.0, TOP_PITCH);
        let mid = Pos2 { x: start.x + 10.0, y: start.y };

        pass_at(&ctx, 0.0, &mut roll, &mut track, &grid, vec![]);
        pass_at(
            &ctx,
            0.0,
            &mut roll,
            &mut track,
            &grid,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        pass_at(&ctx, 0.05, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(mid)]);
        assert!(roll.dragging.is_some(), "the drag threshold has to have been crossed");
        // Well past the dwell, and still nothing: the drag suppresses it.
        pass_at(&ctx, 1.0, &mut roll, &mut track, &grid, vec![]);
        assert!(roll.hover_box.is_none());
    }

    #[test]
    fn the_hover_box_names_the_note_press_intent_would_act_on() {
        // The failure this design exists to prevent: a pointer near a
        // boundary, with a second note on the same step at a different
        // pitch, so a lookup that goes by step rather than by hit test would
        // name the wrong one.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![
            Note::new(4.0, TOP_PITCH, 4.0, 100, 0.0),
            Note::new(4.0, TOP_PITCH - 3, 4.0, 100, 0.0),
        ];
        let wanted = track.notes[1].id;
        let pos = edge_of(&grid, 4.0, 4.0, TOP_PITCH - 3);

        assert_eq!(
            roll.press_intent(&grid, &track, Some(pos), &egui::Modifiers::NONE),
            Intent::Resize(wanted),
            "a sanity check on the fixture: the press has to land in the resize zone"
        );

        pass_at(&ctx, 0.0, &mut roll, &mut track, &grid, vec![]);
        pass_at(&ctx, 0.0, &mut roll, &mut track, &grid, vec![egui::Event::PointerMoved(pos)]);
        pass_at(&ctx, PianoRoll::HOVER_DWELL, &mut roll, &mut track, &grid, vec![]);
        let (id, _) = roll.hover_box.clone().expect("dwell elapsed, pointer held still");
        assert_eq!(id, wanted, "the box has to name the note the press would act on");
    }

    // ----------------------------------------------------- A3: the pencil

    #[test]
    fn create_requests_a_pencil_and_move_does_not() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 4.0, 100, 0.0)];
        let empty = at(&grid, 20.0, TOP_PITCH);
        let body = at(&grid, 4.0, TOP_PITCH);

        pass_with(&ctx, &mut roll, &mut track, &grid, egui::Modifiers::NONE, vec![]);
        let over_empty = pass_cursor_image(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            vec![egui::Event::PointerMoved(empty)],
        );
        assert!(over_empty.is_some(), "empty canvas is Create, which gets the pencil");

        let over_note = pass_cursor_image(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            vec![egui::Event::PointerMoved(body)],
        );
        assert!(over_note.is_none(), "a note body is Move, which is the plain arrow");
    }

    #[test]
    fn the_pencil_clears_when_the_pointer_leaves_the_roll() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let empty = at(&grid, 20.0, TOP_PITCH);
        // Outside `TEST_RECT`, which is where the roll was allocated — the
        // stand-in for the pointer having wandered onto, say, the Edit panel.
        let elsewhere = Pos2 { x: TEST_RECT.max.x + 50.0, y: TEST_RECT.max.y - 50.0 };

        pass_with(&ctx, &mut roll, &mut track, &grid, egui::Modifiers::NONE, vec![]);
        let while_over = pass_cursor_image(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            vec![egui::Event::PointerMoved(empty)],
        );
        assert!(while_over.is_some(), "set up: the pencil has to be showing first");

        let after_leaving = pass_cursor_image(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            vec![egui::Event::PointerMoved(elsewhere)],
        );
        assert!(after_leaving.is_none(), "sticky between frames — it has to be actively cleared");
    }

    #[test]
    fn chord_mode_suppresses_the_pencil() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let empty = at(&grid, 20.0, TOP_PITCH);
        let mut harmony = Harmony::default();
        harmony.chord.on = true;

        run_with(&ctx, &mut roll, &mut track, &grid, egui::Modifiers::NONE, &harmony, vec![]);
        let (_, _, image) = run_with(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            egui::Modifiers::NONE,
            &harmony,
            vec![egui::Event::PointerMoved(empty)],
        );
        assert!(image.is_none(), "the chord ghost is the affordance here, not a pencil on top of it");
    }

    #[test]
    fn the_pencil_bitmap_is_well_formed() {
        let cursor = pencil_cursor();
        assert_eq!(
            cursor.rgba.len(),
            usize::from(cursor.size[0]) * usize::from(cursor.size[1]) * 4,
            "straight RGBA, exactly w * h * 4 bytes or the OS upload is malformed"
        );
        // Per row as well as in total: two rows off by one in opposite
        // directions sum to the right byte count and shear the whole bitmap.
        // Cheap to assert, and the mask is hand-edited every time the cursor
        // is resized — as it was on 2026-08-20, from 32 down to 20.
        assert_eq!(PENCIL_MASK.len(), usize::from(PENCIL_SIZE[1]));
        for (y, row) in PENCIL_MASK.iter().enumerate() {
            assert_eq!(row.chars().count(), usize::from(PENCIL_SIZE[0]), "row {y}: {row}");
        }
        assert!(cursor.hotspot[0] < cursor.size[0] && cursor.hotspot[1] < cursor.size[1]);
    }

    #[test]
    fn a_short_note_is_still_wide_enough_to_grab() {
        // `js/pianoroll.js` floors the drawn width at a third of a cell for the
        // same reason: a 1/8-step note would otherwise be two pixels wide.
        let g = grid();
        let short = Note::new(0.0, 60, 0.125, 100, 0.0);
        assert!(g.note_rect(&short).width() >= CELL_W * 0.3);
    }

    // ---------------------------------------------------------- chord draw

    /// Chord draw on, a fixed major triad, no key — so the voicing under the
    /// cursor is the same three notes wherever the tests click.
    fn chord_mode() -> Harmony {
        Harmony {
            chord: digi_core::chords::ChordSettings {
                on: true,
                quality: digi_core::chords::QualityChoice::Fixed(digi_core::chords::Quality::Major),
                ..Default::default()
            },
            ..Harmony::default()
        }
    }

    /// One pass with a key and chord settings in hand.
    fn chord_pass(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        grid: &Grid,
        harmony: &Harmony,
        events: Vec<egui::Event>,
    ) -> bool {
        run_with(ctx, roll, track, grid, egui::Modifiers::NONE, harmony, events).0
    }

    /// Notes on `step`, as (pitch, velocity, micro), ascending.
    fn on_step(track: &Track, step: f64) -> Vec<(u8, u8, f64)> {
        let mut got: Vec<(u8, u8, f64)> = track
            .notes
            .iter()
            .filter(|n| n.step == step)
            .map(|n| (n.pitch, n.velocity, n.micro))
            .collect();
        got.sort_by_key(|(pitch, _, _)| *pitch);
        got
    }

    #[test]
    fn a_click_with_chord_draw_on_stamps_the_whole_voicing() {
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let harmony = chord_mode();
        let pos = at(&grid, 4.0, 60);

        chord_pass(&ctx, &mut roll, &mut track, &grid, &harmony, vec![]);
        let changed = chord_pass(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            &harmony,
            vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)],
        );

        assert!(changed);
        // The taper is in it: the top of the chord is at the default velocity and
        // the notes under it come in softer, which is what makes a stamped chord
        // sound like a chord rather than a block.
        assert_eq!(on_step(&track, 4.0), [(60, 86, 0.0), (64, 93, 0.0), (67, 100, 0.0)]);
        // **All three are selected**, so the drag half of the gesture can stretch
        // the chord and an immediate move can transpose it.
        assert_eq!(roll.selection().len(), 3);
    }

    #[test]
    fn a_stamped_chord_lengthens_as_one() {
        // The press continues into a resize of the *top* note, and the rest follow
        // by the same delta because they are the selection — `js/pianoroll.js`
        // hands `made.at(-1)` to `_resizeStart` for exactly this.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.length_steps = 16;
        let harmony = chord_mode();

        let from = at(&grid, 2.0, 60);
        let to = at(&grid, 5.0, 60);
        let go = |roll: &mut PianoRoll, track: &mut Track, events| {
            chord_pass(&ctx, roll, track, &grid, &harmony, events);
        };
        go(&mut roll, &mut track, vec![]);
        go(&mut roll, &mut track, vec![egui::Event::PointerMoved(from), button(from, true)]);
        go(&mut roll, &mut track, vec![egui::Event::PointerMoved(to)]);
        go(&mut roll, &mut track, vec![button(to, false)]);

        assert_eq!(track.notes.len(), 3);
        for note in &track.notes {
            assert_eq!(note.len, 4.0, "pitch {} did not follow", note.pitch);
        }
    }

    #[test]
    fn chord_draw_leaves_the_notes_you_click_on_alone() {
        // PLAN.md §5: a note you click on still moves, resizes and deletes as
        // usual. So the mode changes what an *empty* cell does and nothing else —
        // which is why `chord_at` is empty over a note.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 60, 1.0, 100, 0.0)];
        let id = track.notes[0].id;
        let harmony = chord_mode();
        let pos = grid.note_rect(&track.notes[0]).center();

        assert!(roll.chord_at(&grid, &track, pos, &harmony).is_empty());

        chord_pass(&ctx, &mut roll, &mut track, &grid, &harmony, vec![]);
        chord_pass(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            &harmony,
            vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)],
        );

        assert_eq!(track.notes.len(), 1, "no chord was stamped on top of it");
        assert_eq!(roll.selection(), vec![id], "it was selected, as an ordinary click");
    }

    #[test]
    fn the_ghost_and_the_stamp_are_the_same_chord() {
        // The ghost is chord draw's only report — the roll has no status line — so
        // this is the claim that matters: what is drawn under the cursor is what the
        // press puts on the step, including the notes left out. Here the step
        // already holds the third, so both are the remaining two.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, 64, 1.0, 100, 0.0)];
        let harmony = chord_mode();
        let pos = at(&grid, 4.0, 60);

        let ghost: Vec<u8> =
            roll.chord_at(&grid, &track, pos, &harmony).iter().map(|c| c.pitch).collect();
        assert_eq!(ghost, [60, 67]);

        chord_pass(&ctx, &mut roll, &mut track, &grid, &harmony, vec![]);
        chord_pass(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            &harmony,
            vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)],
        );

        let stamped: Vec<u8> = on_step(&track, 4.0).iter().map(|(p, _, _)| *p).collect();
        assert_eq!(stamped, [60, 64, 67], "the 64 was already there; the other two landed");
    }

    #[test]
    fn a_chord_mode_click_with_nothing_to_stamp_still_draws_one_note() {
        // A full trig offers no chord, and the ghost says so by drawing nothing.
        // The click then behaves as it always did — `js/pianoroll.js`'s own
        // fallback when `getChord` comes back empty — rather than being a press
        // that silently does nothing.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = (40..44).map(|pitch| Note::new(4.0, pitch, 1.0, 100, 0.0)).collect();
        let harmony = chord_mode();
        let pos = at(&grid, 4.0, 60);

        assert!(roll.chord_at(&grid, &track, pos, &harmony).is_empty());

        chord_pass(&ctx, &mut roll, &mut track, &grid, &harmony, vec![]);
        chord_pass(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            &harmony,
            vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)],
        );

        assert_eq!(track.notes.len(), 5);
        assert_eq!(roll.selection().len(), 1);
    }

    #[test]
    fn a_stamped_chord_joins_the_trig_that_is_already_on_the_step() {
        // PROB/FILL/COND are per trig on the box, so every note sharing a step has
        // to agree. The incumbent wins and the arriving chord takes its conditions.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![conditioned(4.0, 40, 60, "2:4")];
        let harmony = chord_mode();
        let pos = at(&grid, 4.0, 60);

        chord_pass(&ctx, &mut roll, &mut track, &grid, &harmony, vec![]);
        chord_pass(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            &harmony,
            vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)],
        );

        // Three notes' room on the trig: one is taken, so the voicing loses its top.
        assert_eq!(track.notes.len(), 4);
        for note in &track.notes {
            assert_eq!(note.cond.as_deref(), Some("2:4"), "pitch {}", note.pitch);
            assert_eq!(note.prob, Some(60));
        }
    }

    #[test]
    fn the_strum_leans_the_chord_and_the_offsets_are_whole_ticks() {
        // Strum is real micro-timing — the field the box stores a signed tick in —
        // so a stamped chord survives a write. Counting in ticks is what makes the
        // offsets here the ones a re-fetch gives back.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let harmony = Harmony {
            chord: digi_core::chords::ChordSettings { strum: 3, ..chord_mode().chord },
            ..chord_mode()
        };
        let pos = at(&grid, 4.0, 60);

        chord_pass(&ctx, &mut roll, &mut track, &grid, &harmony, vec![]);
        chord_pass(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            &harmony,
            vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)],
        );

        let micros: Vec<f64> = on_step(&track, 4.0).iter().map(|(_, _, m)| *m).collect();
        assert_eq!(micros, [0.0, 0.125, 0.25]);
        for micro in micros {
            let ticks = micro * 24.0;
            assert_eq!(ticks, ticks.round(), "{micro} of a step is not a whole tick");
        }
    }

    /// One pass of the wheel handler alone, with alt held and the pointer over the
    /// grid. Returns what the wheel was taken for.
    fn wheel_pass(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        harmony: &mut Harmony,
        modifiers: egui::Modifiers,
        events: Vec<egui::Event>,
    ) -> Wheel {
        let mut took = Wheel::Ignored;
        let mut all = vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(Pos2 { x: 200.0, y: 200.0 }),
        ];
        all.extend(events);
        let input = egui::RawInput {
            events: all,
            screen_rect: Some(TEST_RECT),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let response = ui.allocate_rect(TEST_RECT, egui::Sense::click_and_drag());
            took = roll.wheel_inversion(ui, &response, harmony);
        });
        output.textures_delta.clear();
        took
    }

    fn notch(lines: f32) -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2 { x: 0.0, y: lines },
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::ALT,
        }
    }

    #[test]
    fn alt_wheel_cycles_the_inversion_one_step_per_notch() {
        // **One notch, one step.** The delta cannot be read for this: egui smooths
        // scrolling across frames, so a single notch arrives as several frames of
        // small deltas and a four-way cycle stepped per frame lands wherever it
        // likes. So the wheel *events* are counted.
        let ctx = egui::Context::default();
        let mut roll = PianoRoll::default();
        let mut harmony = chord_mode();
        // A layout pass first: egui hit-tests against the previous pass, so nothing
        // is hovered until the rect has been allocated once.
        wheel_pass(&ctx, &mut roll, &mut harmony, egui::Modifiers::ALT, vec![]);

        for expected in [1, 2, 3, 0] {
            let took = wheel_pass(
                &ctx,
                &mut roll,
                &mut harmony,
                egui::Modifiers::ALT,
                vec![notch(1.0)],
            );
            assert_eq!(took, Wheel::Cycled, "a notch cycles, and the roll must not scroll");
            assert_eq!(harmony.chord.inversion, expected);
        }
        // And down the other way.
        wheel_pass(&ctx, &mut roll, &mut harmony, egui::Modifiers::ALT, vec![notch(-1.0)]);
        assert_eq!(harmony.chord.inversion, 3);
    }

    #[test]
    fn the_wheel_is_only_taken_with_alt_and_chord_draw_on() {
        let ctx = egui::Context::default();
        let mut roll = PianoRoll::default();
        wheel_pass(&ctx, &mut roll, &mut chord_mode(), egui::Modifiers::ALT, vec![]);

        // Chord draw off: alt+wheel is an ordinary scroll, not a silent no-op that
        // also refuses to scroll.
        let mut off = Harmony::default();
        assert_eq!(
            wheel_pass(&ctx, &mut roll, &mut off, egui::Modifiers::ALT, vec![notch(1.0)]),
            Wheel::Ignored
        );
        assert_eq!(off.chord.inversion, 0);

        // Chord draw on, no alt: the roll scrolls, as it always did.
        let mut on = chord_mode();
        assert_eq!(
            wheel_pass(&ctx, &mut roll, &mut on, egui::Modifiers::NONE, vec![notch(1.0)]),
            Wheel::Ignored
        );
        assert_eq!(on.chord.inversion, 0);
    }

    #[test]
    fn a_trackpad_flick_does_not_spin_the_cycle() {
        // A trackpad sends a stream of small point deltas rather than notches, so
        // they accumulate and spend one step per `TRACKPAD_NOTCH`. Without that,
        // one two-finger flick would go round the four positions several times.
        let ctx = egui::Context::default();
        let mut roll = PianoRoll::default();
        let mut harmony = chord_mode();
        wheel_pass(&ctx, &mut roll, &mut harmony, egui::Modifiers::ALT, vec![]);

        let points = |y: f32| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2 { x: 0.0, y },
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::ALT,
        };
        // A third of a notch, three times: one step, not three. **And the two
        // part-way frames report `Aimed` rather than `Cycled`** — the wheel is spent,
        // so the roll must not scroll, but nothing changed and the session must not
        // be marked unsaved for it.
        //
        // The arithmetic under this is a division rather than a loop, because a loop
        // whose test and whose decrement can disagree is a loop that can hang — see
        // `wheel_inversion`, and the plant that found it by freezing this test.
        let mut answers = Vec::new();
        for _ in 0..3 {
            answers.push(wheel_pass(
                &ctx,
                &mut roll,
                &mut harmony,
                egui::Modifiers::ALT,
                vec![points(TRACKPAD_NOTCH / 3.0 + 0.1)],
            ));
        }
        assert_eq!(answers, [Wheel::Aimed, Wheel::Aimed, Wheel::Cycled]);
        assert_eq!(harmony.chord.inversion, 1);

        // A flick worth several notches spends them all at once rather than one per
        // frame, which is the other half of what the division buys.
        wheel_pass(
            &ctx,
            &mut roll,
            &mut harmony,
            egui::Modifiers::ALT,
            vec![points(TRACKPAD_NOTCH * 2.0)],
        );
        assert_eq!(harmony.chord.inversion, 3, "two more notches, in one frame");

        // Letting go of alt drops what is left over: half a flick must not spend
        // itself on the next gesture.
        wheel_pass(&ctx, &mut roll, &mut harmony, egui::Modifiers::NONE, vec![]);
        assert_eq!(roll.wheel, 0.0);
    }

    #[test]
    fn the_root_row_is_tinted_more_strongly_than_the_rest_of_the_scale() {
        // The whole of what a test can say about the tint: the root is stronger, and
        // a row outside the key is not washed at all. Whether 6% is visible on a
        // screen is the check PLAN.md §9 lists.
        let root = scale_wash(Row::Root).expect("the root is washed");
        let in_scale = scale_wash(Row::InScale).expect("so is the rest of the scale");
        assert!(root.a() > in_scale.a(), "{root:?} must read stronger than {in_scale:?}");
        assert_eq!(scale_wash(Row::Outside), None);
        // And the two are one hue, so the strength is the only thing that differs.
        // Read back unmultiplied and compared loosely: `Color32` premultiplies on the
        // way in, which is lossy at these alphas — 240 comes back as 242 at 15% and
        // as 238 at 6%. The claim is "the same amber", not "the same bytes".
        let [rr, rg, rb, _] = root.to_srgba_unmultiplied();
        let [sr, sg, sb, _] = in_scale.to_srgba_unmultiplied();
        for (a, b) in [(rr, sr), (rg, sg), (rb, sb)] {
            assert!(a.abs_diff(b) <= 10, "{a} and {b} are not the same colour");
        }
    }

    #[test]
    fn a_key_tints_rows_and_changes_nothing_else() {
        // PLAN.md §5's one rule: the tint is visual only. The claim a test can
        // make is that drawing is untouched by it — a note goes exactly where it
        // was put, on an out-of-scale row, with a key set.
        let ctx = egui::Context::default();
        let grid = live_grid();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        let harmony = Harmony {
            root: 0,
            scale: Some(digi_core::chords::Scale::Major),
            ..Harmony::default()
        };
        // C# — outside C major, and the row the tint leaves plain.
        let pos = at(&grid, 4.0, 61);

        chord_pass(&ctx, &mut roll, &mut track, &grid, &harmony, vec![]);
        chord_pass(
            &ctx,
            &mut roll,
            &mut track,
            &grid,
            &harmony,
            vec![egui::Event::PointerMoved(pos), button(pos, true), button(pos, false)],
        );

        assert_eq!(track.notes.len(), 1);
        assert_eq!(track.notes[0].pitch, 61, "no snapping, ever");
    }

    // --- zoom -----------------------------------------------------------------

    /// The `Grid` `ui` builds, for a roll sitting in [`TEST_RECT`]. **A second
    /// copy of that origin arithmetic, on purpose**: a zoom test has to observe
    /// where a cell is *drawn*, and asserting on `scroll_x` alone would pass on a
    /// roll whose scroll and cell size had both moved the wrong way. It reads
    /// through the real [`Grid::x_of_step`] and [`Grid::y_of_pitch`] so the
    /// arithmetic under test is the frame's own.
    fn grid_of(roll: &PianoRoll, band: Band) -> Grid {
        Grid {
            origin: Pos2 {
                x: TEST_RECT.min.x + KEY_W + roll.scroll_x,
                y: TEST_RECT.min.y + roll.scroll_y,
            },
            cell: roll.cell(),
            band,
        }
    }

    const FULL_BAND: Band = Band { lo: PITCH_MIN, hi: PITCH_MAX };

    /// **The invariant the whole gesture exists for**: whatever is under the
    /// pointer is still under it afterwards. Without it, zooming in on a bar
    /// throws that bar off the side of the window and you chase the music with
    /// the scroll wheel.
    ///
    /// The anchor is deliberately **not** the grid's origin. At the origin
    /// `anchor - scroll` is zero and the anchored scroll collapses to `0 * factor`
    /// — every way of getting this arithmetic wrong gives the right answer there,
    /// which is `DEVELOPMENT.md` lesson 4's shape: a fixture that makes two
    /// different rules agree.
    #[test]
    fn a_zoom_holds_what_is_under_the_pointer_where_it_was_drawn() {
        let mut roll = PianoRoll::default();
        // Twelve columns along, and — with the opening `scroll_y` — seventeen rows
        // down, so both anchors land on a cell edge and the assertion can name a
        // whole step and a real pitch rather than a fraction of one.
        let anchor = Vec2 { x: 12.0 * CELL_W, y: 60.0 };
        let pitch = FULL_BAND.hi - 17;
        let before = grid_of(&roll, FULL_BAND);
        let (x, y) = (before.x_of_step(12.0), before.y_of_pitch(pitch));

        assert!(roll.zoom_by(2.0, anchor), "a zoom inside the range moves");
        assert_eq!(roll.zoom, 2.0);

        let after = grid_of(&roll, FULL_BAND);
        assert!((after.x_of_step(12.0) - x).abs() < 1e-3, "step 12 stayed at {x}");
        assert!((after.y_of_pitch(pitch) - y).abs() < 1e-3, "the row stayed at {y}");
        // And the cells really did grow, or the test above would pass on a
        // function that did nothing at all.
        assert_eq!(after.cell, Vec2 { x: CELL_W * 2.0, y: CELL_H * 2.0 });
    }

    /// The range holds, **and the anchor still holds with it** — which is the
    /// half that is easy to get wrong. The scroll has to be moved by the factor
    /// the cell size actually took, not by the one the gesture asked for: anchor
    /// on a delta of 100 while the zoom stops at 4 and the view slides off while
    /// the grid stays exactly as it was, a gesture that does nothing except lose
    /// your place.
    #[test]
    fn a_zoom_stops_at_the_range_and_holds_its_anchor_there() {
        let anchor = Vec2 { x: 12.0 * CELL_W, y: 60.0 };
        let pitch = FULL_BAND.hi - 17;

        for delta in [100.0, 0.001] {
            let mut roll = PianoRoll::default();
            let before = grid_of(&roll, FULL_BAND);
            let (x, y) = (before.x_of_step(12.0), before.y_of_pitch(pitch));

            assert!(roll.zoom_by(delta, anchor));
            let wanted = if delta > 1.0 { ZOOM_MAX } else { ZOOM_MIN };
            assert_eq!(roll.zoom, wanted, "a delta of {delta} stopped at the range");

            let after = grid_of(&roll, FULL_BAND);
            assert!(
                (after.x_of_step(12.0) - x).abs() < 1e-3,
                "step 12 stayed at {x} at the clamp, not at {}",
                after.x_of_step(12.0)
            );
            assert!((after.y_of_pitch(pitch) - y).abs() < 1e-3, "the row stayed at {y} too");
        }
    }

    /// A second flick at the end of the range does **nothing**, rather than
    /// re-anchoring on a factor of 1 that floating point has rounded to 0.9999.
    /// The gesture is held down for whole seconds at a time; a per-frame drift of
    /// a pixel is a roll that walks away while you lean on the wheel.
    #[test]
    fn a_zoom_past_the_end_of_the_range_moves_nothing_at_all() {
        let mut roll = PianoRoll::default();
        roll.set_zoom(ZOOM_MAX);
        roll.scroll_x = -400.0;
        roll.scroll_y = -200.0;

        assert!(!roll.zoom_by(2.0, Vec2 { x: 240.0, y: 60.0 }), "nothing left to give");
        assert_eq!(roll.zoom, ZOOM_MAX);
        assert_eq!((roll.scroll_x, roll.scroll_y), (-400.0, -200.0), "and the view did not drift");
    }

    /// **Clamped at the setter, not at the control.** The Edit panel's slider is
    /// ranged to the same two constants, so this is the belt to that braces — and
    /// the one that matters, per `set_default_velocity`'s own history: Phase 9's
    /// velocity slider clamped what it drew and not what it stored, and a zoom of
    /// zero is the worse version of that bug because `Grid::step_at` divides by
    /// the cell width.
    #[test]
    fn set_zoom_clamps_rather_than_trusting_whatever_wrote_it() {
        let mut roll = PianoRoll::default();

        roll.set_zoom(0.0);
        assert_eq!(roll.zoom(), ZOOM_MIN);
        assert!(roll.cell().x > 0.0, "a cell with width, so the hit test is not an infinity");
        assert!(grid_of(&roll, FULL_BAND).step_at(300.0).is_finite());

        roll.set_zoom(99.0);
        assert_eq!(roll.zoom(), ZOOM_MAX);
    }

    /// The scroll bound is in pixels, so it is a function of the cell size — and
    /// one function, in one place, because `ui` applies it for every writer of
    /// these fields. A zoom out that left the old bound in place would strand the
    /// view below the last row, on empty background.
    #[test]
    fn the_scroll_bound_moves_with_the_zoom() {
        let mut roll = PianoRoll { scroll_y: -100_000.0, ..PianoRoll::default() };
        roll.clamp_scroll(FULL_BAND);
        let full = -(FULL_BAND.rows() as f32 * CELL_H);
        assert_eq!(roll.scroll_y, full, "the whole band, at 1x");

        roll.set_zoom(0.5);
        roll.clamp_scroll(FULL_BAND);
        assert_eq!(roll.scroll_y, full / 2.0, "half the pixels at half the zoom");
    }

    /// One headless frame through the whole of [`PianoRoll::ui`] — geometry,
    /// wheel chain and all, which `pass` deliberately bypasses by handing
    /// `interact` a `Grid` of the test's own.
    ///
    /// `modifiers` is announced ahead of the frame's events for the same reason
    /// `pass_with` announces it: `RawInput` has no modifiers field.
    fn frame(
        ctx: &egui::Context,
        roll: &mut PianoRoll,
        track: &mut Track,
        modifiers: egui::Modifiers,
        events: Vec<egui::Event>,
    ) -> bool {
        let mut changed = false;
        let mut all = vec![egui::Event::ModifiersChanged(modifiers)];
        all.extend(events);
        let input = egui::RawInput {
            events: all,
            screen_rect: Some(TEST_RECT),
            ..Default::default()
        };
        let mut harmony = Harmony::default();
        let mut output = ctx.run_ui(input, |ui| {
            changed = roll.ui(ui, track, None, &mut harmony);
        });
        output.textures_delta.clear();
        changed
    }

    /// A mouse wheel, in the shape `egui-winit` actually pushes for one:
    /// `Event::MouseWheel` in `Line` units carrying the modifiers winit was
    /// tracking (`egui-winit-0.36.1/src/lib.rs` ~961).
    ///
    /// **The modifiers on the event are the ones that matter, not the frame's.**
    /// egui reads `zoom_modifier` off the wheel event itself
    /// (`InputState::update`, ~461), so a `ModifiersChanged` without them here
    /// would be a test proving nothing about the real gesture — which is
    /// `ui::tracks`' Cmd+C lesson in a new costume.
    fn wheel(lines: f32, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2 { x: 0.0, y: lines },
            phase: egui::TouchPhase::Move,
            modifiers,
        }
    }

    /// Somewhere inside the roll's own rect — above the trig lane, right of the
    /// key column.
    const OVER_GRID: Pos2 = Pos2 { x: KEY_W + 300.0, y: 200.0 };

    /// **The gesture, end to end, through the input the platform sends.**
    ///
    /// Two frames because egui hit-tests against the previous pass's layout, so
    /// the wheel has to arrive on a frame where the roll's rect is already known
    /// — otherwise `hovered` is false and the roll declines the wheel, which
    /// fails by doing nothing at all.
    #[test]
    fn cmd_wheel_over_the_grid_zooms_it_and_a_bare_wheel_scrolls_instead() {
        let ctx = egui::Context::default();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);

        frame(&ctx, &mut roll, &mut track, egui::Modifiers::COMMAND, vec![egui::Event::PointerMoved(OVER_GRID)]);
        let opened_at = roll.zoom();
        frame(&ctx, &mut roll, &mut track, egui::Modifiers::COMMAND, vec![wheel(1.0, egui::Modifiers::COMMAND)]);
        assert!(
            roll.zoom() > opened_at,
            "cmd+wheel up zoomed in: {opened_at} -> {}",
            roll.zoom()
        );

        // The same wheel with nothing held scrolls and leaves the zoom alone.
        //
        // **On a second `Context`, and that is not tidiness.** egui smooths a
        // mouse notch across frames, so a flick's leftover sits in the
        // *context's* `WheelState` — with the modifiers it arrived under — and is
        // spent a fraction at a time on the frames after it. Sharing the context
        // here zoomed this roll by the previous gesture's residue and failed the
        // assertion below, which is the honest behaviour of a wheel and a
        // dishonest fixture: one gesture, one cold start.
        let cold = egui::Context::default();
        let mut plain = PianoRoll::default();
        let opened_at = plain.scroll_y;
        frame(&cold, &mut plain, &mut track, egui::Modifiers::NONE, vec![egui::Event::PointerMoved(OVER_GRID)]);
        frame(&cold, &mut plain, &mut track, egui::Modifiers::NONE, vec![wheel(1.0, egui::Modifiers::NONE)]);
        assert_eq!(plain.zoom(), 1.0, "a bare wheel is not a zoom");
        assert!(plain.scroll_y > opened_at, "it scrolled instead");
    }

    /// A trackpad pinch, which arrives as `Event::Zoom(delta.exp())` rather than
    /// as a wheel (`egui-winit-0.36.1/src/lib.rs` ~537) and lands on the same
    /// field. So the gesture ships on a mouse and a trackpad from one
    /// implementation — and this is the deterministic half of the pair above,
    /// since a pinch is not smoothed across frames.
    ///
    /// **And the frame reports no change**, which is the claim that keeps a zoom
    /// out of the undo stack and off the unsaved-changes flag.
    #[test]
    fn a_pinch_over_the_grid_zooms_it_and_is_not_an_edit() {
        let ctx = egui::Context::default();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        track.notes = vec![Note::new(4.0, TOP_PITCH, 1.0, 100, 0.0)];

        frame(&ctx, &mut roll, &mut track, egui::Modifiers::NONE, vec![egui::Event::PointerMoved(OVER_GRID)]);
        let changed = frame(
            &ctx,
            &mut roll,
            &mut track,
            egui::Modifiers::NONE,
            vec![egui::Event::Zoom(2.0)],
        );

        assert_eq!(roll.zoom(), 2.0, "a pinch of 2x is a zoom of 2x");
        assert!(!changed, "looking closer at a bar is not an edit of it");
    }

    /// **The zoom reaches the grid the frame actually hit-tests with**, which is
    /// the claim every other test here can't see: they build their own [`Grid`]
    /// from `roll.cell()`, so a `ui` that zoomed the field and went on handing
    /// `interact` a 20x12 grid would pass all of them — the roll would zoom in
    /// its own head and draw and click at one size for ever. That is
    /// `DEVELOPMENT.md` lesson 5's shape, one geometry read in two places, and
    /// the only witness is what a click *lands on*.
    ///
    /// So the numbers here are the point: at 2x the cell is 40x24, and the same
    /// pointer position that means step 6 and pitch 76 at rest means step 3 and
    /// pitch 86. `set_zoom` rather than a pinch, because it leaves the scroll
    /// alone and the expected cell can then be arithmetic anyone can check.
    #[test]
    fn a_click_lands_on_the_cell_the_zoom_puts_under_it() {
        let ctx = egui::Context::default();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        // Three zoomed columns along the grid, and ten zoomed rows below the
        // opening `scroll_y` of -144.
        let pos = Pos2 { x: KEY_W + 3.0 * CELL_W * 2.0 + 5.0, y: 100.0 };

        roll.set_zoom(2.0);
        frame(&ctx, &mut roll, &mut track, egui::Modifiers::NONE, vec![]);
        frame(
            &ctx,
            &mut roll,
            &mut track,
            egui::Modifiers::NONE,
            vec![egui::Event::PointerMoved(pos), button(pos, true)],
        );
        frame(&ctx, &mut roll, &mut track, egui::Modifiers::NONE, vec![button(pos, false)]);

        assert_eq!(track.notes.len(), 1, "a click on an empty cell still draws one note");
        assert_eq!(track.notes[0].step, 3.0, "step 3 at 2x, where the same pixel is step 6 at 1x");
        assert_eq!(track.notes[0].pitch, 86, "and pitch 86, where that pixel is pitch 76 at 1x");
    }

    /// **A frame that carries both a zoom and a scroll spends it once.**
    ///
    /// This is the case the `&&` in `ui` is for, and it had to be built by hand
    /// because the obvious gesture cannot reach it: egui zeroes
    /// `smooth_scroll_delta` for a wheel event carrying the zoom modifier, so
    /// cmd+wheel alone leaves nothing for `scroll` to find and a plant on that
    /// guard would fail no test at all — `DEVELOPMENT.md` lesson 6's first
    /// answer, construct the case, rather than its third. A **pinch** does not
    /// zero it: `Event::Zoom` arrives on its own path, so two fingers that
    /// pinch *and* slide put a real delta on both fields in one frame. The roll
    /// must then zoom and not also scroll, or the music slides out from under
    /// the fingers magnifying it.
    ///
    /// Two contexts, because egui's wheel smoothing outlives a frame and the
    /// residue would reach the other roll.
    #[test]
    fn a_pinch_that_arrives_with_a_scroll_zooms_and_does_not_also_scroll() {
        let mut track = Track::new(0, TrackKind::Audio);
        let mut with_wheel = PianoRoll::default();
        let mut alone = PianoRoll::default();

        for (ctx, roll, events) in [
            (egui::Context::default(), &mut alone, vec![egui::Event::Zoom(2.0)]),
            (
                egui::Context::default(),
                &mut with_wheel,
                vec![egui::Event::Zoom(2.0), wheel(1.0, egui::Modifiers::NONE)],
            ),
        ] {
            frame(&ctx, roll, &mut track, egui::Modifiers::NONE, vec![egui::Event::PointerMoved(OVER_GRID)]);
            frame(&ctx, roll, &mut track, egui::Modifiers::NONE, events);
        }

        assert_eq!(alone.zoom(), 2.0);
        assert_eq!(with_wheel.zoom(), 2.0, "the pinch is the same pinch");
        // The zoom moves the scroll itself, to hold its anchor — so the claim is
        // not "the scroll did not move" but "it moved by the zoom and by nothing
        // else", which is what the pinch on its own measures.
        assert_eq!(
            with_wheel.scroll_y, alone.scroll_y,
            "the wheel in the same frame was not spent a second time as a scroll"
        );
    }

    /// The zoom is the *window's* input in egui, not this widget's: without the
    /// hover gate, cmd+wheel over a panel's slider — or over the trig lane — would
    /// zoom the roll underneath it.
    #[test]
    fn a_cmd_wheel_that_is_not_over_the_grid_leaves_the_roll_alone() {
        let ctx = egui::Context::default();
        let mut roll = PianoRoll::default();
        let mut track = Track::new(0, TrackKind::Audio);
        // In the trig lane's strip, which the roll's own rect stops above.
        let in_the_lane = Pos2 { x: KEY_W + 300.0, y: TEST_RECT.max.y - 4.0 };

        frame(&ctx, &mut roll, &mut track, egui::Modifiers::COMMAND, vec![egui::Event::PointerMoved(in_the_lane)]);
        frame(&ctx, &mut roll, &mut track, egui::Modifiers::COMMAND, vec![wheel(1.0, egui::Modifiers::COMMAND)]);

        assert_eq!(roll.zoom(), 1.0, "the pointer was not on the grid");
    }

}

