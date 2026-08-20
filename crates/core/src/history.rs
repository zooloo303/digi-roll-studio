// Undo and redo, and what "one step" means here.
//
// PLAN.md §9 asks for this to be scoped honestly before a button is drawn,
// because the JS undoes over one track's note array and this app edits a whole
// session. Two questions had to be answered, and this module is both answers.
//
// ## What an undo step contains: the music, and nothing else
//
// A [`Content`] snapshot holds **every device's pattern slots** and nothing
// outside them. So notes, lengths, scales, swing, PROB, p-lock lanes and the
// per-track studio fields are undoable, and these are not:
//
// * the tempo,
// * which port a box is on, and whether it takes the clock,
// * the scenes, their names, their slots, and which one is playing,
// * the session's name.
//
// The line is the model's own, and `js/main.js` draws it in the same place —
// its snapshot is `state.patterns[slot]`, never `state.bpm` or the MIDI output.
// The argument for it is that undo is a music-editing gesture: identifying a box
// gives it a port, and having Ctrl+Z take that port away again — silently
// stopping the box — would be a worse surprise than any it prevented. The desk is
// not history, it is where you are sitting.
//
// This falls out cheaply rather than needing to be enforced: an edit to the desk
// leaves the content identical, so [`History::commit`] compares and pushes
// nothing. There is no list of "undoable actions" to keep in step with the panels.
//
// ## What one step is: one press to one release
//
// The shell has no notion of a gesture — it has an `edited` flag per frame, and a
// drag sets it on every frame it moves. So a step is bounded by the pointer:
// [`History::begin`] takes the snapshot on the first frame of a change and
// [`History::commit`] pushes it when the button comes up. A keyboard edit (Delete)
// begins and commits in the same frame, which makes it its own step. A slider
// dragged across forty frames is one step, which is what `js/main.js` gets from
// its `velGesture` and `lenGesture` latches — and this gets it once, for every
// control, rather than one latch per widget.
//
// A gesture that changed nothing back to nothing leaves no step: `commit`
// compares, which is `dropUnchangedUndo` in the JS. That matters more than it
// sounds — clicking a note selects it and sets `edited` on the frame it is
// adopted, and an undo stack full of no-ops is an undo button that does nothing
// several times before it does something.
//
// ## Why the snapshots are cheap
//
// `Session` is copy-on-write all the way down: a `Content` is a vector of
// `Arc<Pattern>` clones, so taking one costs pointer bumps rather than notes.
// Holding one is what *makes* the next edit clone the pattern it touches — which
// is exactly the semantics wanted, and the reason `begin` has to run before the
// frame's edits rather than after.

use std::sync::Arc;

use crate::device::DeviceId;
use crate::model::Pattern;
use crate::session::Session;

/// How many steps back the stack goes, from `js/main.js`'s `HISTORY_MAX`.
pub const HISTORY_MAX: usize = 100;

/// The musical content of a session: every box's pattern slots, and nothing else.
/// See the module header for where that line is drawn and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Content(Vec<(DeviceId, Vec<Arc<Pattern>>)>);

impl Content {
    pub fn of(session: &Session) -> Self {
        Self(session.devices.iter().map(|d| (d.id, d.patterns.clone())).collect())
    }

    /// Whether this is the same music as `session` is holding.
    ///
    /// Compares `Arc` identity first, which is what keeps this cheap: everything
    /// the frame did not touch is still the same allocation, so only the one
    /// pattern that was edited is compared note by note.
    pub fn matches(&self, session: &Session) -> bool {
        self.0.len() == session.devices.len()
            && self.0.iter().zip(&session.devices).all(|((id, patterns), device)| {
                *id == device.id
                    && patterns.len() == device.patterns.len()
                    && patterns
                        .iter()
                        .zip(&device.patterns)
                        .all(|(a, b)| Arc::ptr_eq(a, b) || a == b)
            })
    }

    /// Put this content back into `session`.
    ///
    /// **A device that is no longer there is skipped, and so is one whose slot
    /// count has changed.** Both are reachable: Setup can remove a box while
    /// there are steps on the stack behind it, and putting patterns back into a
    /// box that is not the box they came from would be worse than not undoing.
    /// Returns whether anything actually moved.
    pub fn restore(&self, session: &mut Session) -> bool {
        let mut changed = false;
        for (id, patterns) in &self.0 {
            let Some(device) = session.device_mut(*id) else { continue };
            if device.patterns.len() != patterns.len() {
                continue;
            }
            if device
                .patterns
                .iter()
                .zip(patterns)
                .any(|(a, b)| !Arc::ptr_eq(a, b) && a != b)
            {
                device.patterns = patterns.clone();
                changed = true;
            }
        }
        changed
    }
}

/// The undo and redo stacks, and the half-open step between them.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Content>,
    redo: Vec<Content>,
    /// The content as it was before the gesture in progress touched it. `Some`
    /// from the first frame of a change until the gesture ends.
    pending: Option<Content>,
}

impl History {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    /// Whether a gesture is part-way through, which is what tells the shell it
    /// does not need another snapshot this frame.
    pub fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    /// Open a step, if one is not already open. `before` must be the content from
    /// **before** this frame's edits — see the module header on why the order
    /// matters rather than merely being tidy.
    pub fn begin(&mut self, before: Content) {
        if self.pending.is_none() {
            self.pending = Some(before);
        }
    }

    /// Close the open step. Returns whether one was pushed.
    ///
    /// A step whose content matches what the session now holds is dropped rather
    /// than pushed: that is `js/main.js`'s `dropUnchangedUndo`, and it is what
    /// keeps a click that selected a note out of the history.
    pub fn commit(&mut self, session: &Session) -> bool {
        let Some(before) = self.pending.take() else {
            return false;
        };
        if before.matches(session) {
            return false;
        }
        self.undo.push(before);
        if self.undo.len() > HISTORY_MAX {
            self.undo.remove(0);
        }
        // A new step invalidates everything ahead of it, as every undo stack
        // does: there is no longer one future to redo into.
        self.redo.clear();
        true
    }

    /// Abandon the open step without pushing it. For the case where the session
    /// in hand is replaced wholesale — a project loaded off disk — and the step
    /// would be measured against music that is no longer there.
    pub fn abandon(&mut self) {
        self.pending = None;
    }

    /// Throw the whole history away. Called when a session is opened: the steps
    /// behind it belong to a different piece of music.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
    }

    /// Step back. Returns whether anything moved.
    pub fn undo(&mut self, session: &mut Session) -> bool {
        Self::step(&mut self.undo, &mut self.redo, session)
    }

    /// Step forward again.
    pub fn redo(&mut self, session: &mut Session) -> bool {
        Self::step(&mut self.redo, &mut self.undo, session)
    }

    /// `js/main.js`'s `step`, which is one function for both directions because
    /// the two are the same operation with the stacks swapped: pop from one, push
    /// where you were onto the other.
    ///
    /// The push happens **whether or not the restore changes anything**, so a
    /// step over a device that has since been removed still moves the cursor
    /// through the history rather than getting stuck on it.
    fn step(from: &mut Vec<Content>, to: &mut Vec<Content>, session: &mut Session) -> bool {
        let Some(entry) = from.pop() else {
            return false;
        };
        to.push(Content::of(session));
        entry.restore(session)
    }
}
