// The crash copy: the only thing in this app that writes a session nobody asked
// it to.
//
// PLAN.md's open-questions list carried this for months as one line — *"Saving
// is manual; there is no autosave, so a crash takes the session"* — and the
// shape of the answer was the whole of the decision. There are two autosaves a
// program can have and they are not the same feature:
//
// 1. **Keep the user's file live.** Re-save to the chosen path every few
//    seconds. Cheap to build, and it quietly deletes "quit without saving" as a
//    way out: an experiment you had not decided about is on disk before you
//    decide, and this app has no undo across launches to walk it back.
// 2. **Keep a copy somewhere else.** What this module does. The session file is
//    never touched by anything but a deliberate Save; a shadow copy goes to the
//    per-user application-data directory while there is unsaved work, and the
//    next launch offers it back. The dirty flag, `Cmd+S` and the close guard all
//    keep exactly the meaning they had, and `Discard and close` still discards.
//
// The second is what the PLAN.md line was actually asking for — it names a
// *crash*, not a habit — and it is the one built.
//
// ## The copy is a project file, not a format of its own
//
// `session.json` in the recovery directory is byte-for-byte what
// [`SessionPanel::save_to`] would have written to a path you chose: a plain
// [`Project`], pretty-printed. That is deliberate and it is the module's one
// real safety net. If the modal below never appears, or appears and you do not
// trust it, the recovery is `Open…` on that file — no tool, no import, nothing
// in this file needed. A bespoke container would have made this module the only
// way to get the bytes back, which is the wrong thing to be on the day it
// matters.
//
// `session.meta.json` beside it holds the one thing a project file has no room
// for: which path the copy was a copy *of*, so a recovered session can go back
// to `Cmd+S` without a dialog. It is strictly optional — a missing or unreadable
// meta costs you the remembered path and nothing else, which is why it is a
// second file rather than a field that could take the project down with it.
//
// **When** the copy was taken is not stored at all: it is the snapshot file's
// mtime. A recorded timestamp and a file that disagreed with it would need a
// rule about which to believe, and there is no such rule worth writing.
//
// ## Both writes are rename-into-place
//
// A crash is exactly the event this module exists for, so a crash landing
// *during* its own write is not a hypothetical. Each file is written to a `.tmp`
// sibling and renamed over the target, so the reader only ever sees a whole one.
// Without it the failure mode is the cruel one: a recovery file that exists,
// gets offered, and is half a session.
//
// ## What is deliberately not here
//
// **No ring.** One copy, overwritten. `protocol::backup_stash` keeps a ring
// because it holds the only copy of bytes that live on a box; this holds a copy
// of something the user can also save themselves, and a directory of
// near-identical sessions is a thing to search rather than a thing to recover
// from.
//
// **No second instance.** Two copies of the app running at once share the one
// file and the later write wins. Said here rather than worked around: the guard
// would be a lock file, and a stale lock file after a crash would break the
// feature precisely when it is needed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use digi_core::project::Project;
use digi_core::Session;

/// How long the app must go without an edit before a dirty session is copied
/// out.
///
/// A debounce, not an interval: a drag across forty frames is one edit as far as
/// this is concerned, and writing on each of them would be forty copies of a
/// gesture that is not finished.
pub const QUIET: Duration = Duration::from_secs(2);

/// The longest unsaved work goes uncopied while the edits keep coming.
///
/// The ceiling is what stops [`QUIET`] from never firing. Someone drawing
/// steadily for a minute never leaves a two-second gap, and without this their
/// whole minute is the thing at risk.
pub const CEILING: Duration = Duration::from_secs(20);

/// The copy itself: a project file, same bytes a Save writes.
pub const SNAPSHOT_FILE: &str = "session.json";
/// The sidecar. See the module header for why it is separate and optional.
pub const META_FILE: &str = "session.meta.json";

/// The recovery directory under the per-user application-data root.
///
/// `Err` is a machine with no home directory to put one on — the app runs
/// without a crash copy rather than inventing a location in the working
/// directory, the same call [`digi_protocol::backup_stash::app_data_dir`] makes
/// for backups.
pub fn default_dir() -> Result<PathBuf, String> {
    digi_protocol::backup_stash::app_data_dir()
        .map(|d| d.join("recovery"))
        .map_err(|e| e.to_string())
}

/// The sidecar's contents.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    /// The session file this was a copy of, or `None` for work that had never
    /// been saved anywhere.
    origin: Option<PathBuf>,
}

/// A crash copy found on disk, proved readable, and not yet offered.
///
/// Holds the JSON rather than a parsed [`Session`] on purpose. The parse this
/// struct's existence proves is done at launch, so nothing is ever offered that
/// cannot be delivered; but the parse that *counts* — the one that re-points
/// every device at the ports actually plugged in — needs port lists the app has
/// not finished enumerating that early. So the text is kept and parsed again
/// when the offer is accepted, which costs one pass over a small string and
/// buys a recovered session whose boxes are bound to the desk as it is now.
#[derive(Debug, Clone)]
pub struct Found {
    /// The project file's text, already proved to parse and validate.
    pub json: String,
    /// Which file this was a copy of, if the sidecar survived and named one.
    pub origin: Option<PathBuf>,
    /// How long ago it was written, from the snapshot file's mtime. `None` on a
    /// filesystem that will not say.
    pub age: Option<Duration>,
}

/// When the next crash copy is due.
///
/// Split out from [`Recovery`] and holding no clock of its own: every method
/// takes the `Instant` rather than reading one, so the cadence can be driven
/// through an hour of editing in a test that takes no time at all. It is the
/// same move `ui::session::Chooser` makes on the dialog — the interesting half
/// is the decision, not the syscall.
#[derive(Debug, Clone, Default)]
pub struct Cadence {
    /// The first edit since the last copy. The ceiling is measured from here.
    first_edit: Option<Instant>,
    /// The most recent edit. The quiet period is measured from here.
    last_edit: Option<Instant>,
}

impl Cadence {
    /// Fold in a frame that changed the session.
    pub fn note_edit(&mut self, now: Instant) {
        self.first_edit.get_or_insert(now);
        self.last_edit = Some(now);
    }

    /// Whether a copy is due. `false` whenever nothing has been edited since the
    /// last one — an idle app writes nothing at all.
    pub fn due(&self, now: Instant) -> bool {
        let (Some(first), Some(last)) = (self.first_edit, self.last_edit) else {
            return false;
        };
        now.saturating_duration_since(last) >= QUIET
            || now.saturating_duration_since(first) >= CEILING
    }

    /// A copy was taken, so both clocks start again from the next edit.
    pub fn note_write(&mut self) {
        self.first_edit = None;
        self.last_edit = None;
    }

    /// Whether anything is waiting to be copied. Used by the shell only to
    /// decide whether to ask for a repaint, so an idle window stays idle.
    pub fn pending(&self) -> bool {
        self.last_edit.is_some()
    }
}

/// The crash-copy store: one directory, two files.
///
/// Injectable rather than a global for the reason `Stash` is — tests get their
/// own directory and never go near the real one. [`default_dir`] is what the app
/// uses.
#[derive(Debug, Clone)]
pub struct Recovery {
    dir: PathBuf,
}

impl Recovery {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The store under the per-user application-data root, or why there isn't
    /// one.
    pub fn default_store() -> Result<Self, String> {
        default_dir().map(Self::at)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn snapshot_path(&self) -> PathBuf {
        self.dir.join(SNAPSHOT_FILE)
    }

    pub fn meta_path(&self) -> PathBuf {
        self.dir.join(META_FILE)
    }

    /// Take a copy of `session`, recording which file it was a copy of.
    ///
    /// Pretty-printed, exactly as a Save is, and for the same reason: a file a
    /// person can read in a text editor is a file a person can salvage.
    pub fn write(&self, session: &Session, origin: Option<&Path>) -> Result<(), String> {
        let json = Project::new(session.clone())
            .to_json_pretty()
            .map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("could not make {}: {e}", self.dir.display()))?;
        write_atomic(&self.snapshot_path(), &json)?;
        // The snapshot is the copy; the sidecar is a convenience. Written second
        // so that a failure here leaves a recoverable session that has merely
        // forgotten its path, rather than the other way round.
        let meta = Meta { origin: origin.map(Path::to_path_buf) };
        let meta_json = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
        write_atomic(&self.meta_path(), &meta_json)
    }

    /// Look for a copy left behind by a previous run.
    ///
    /// Three answers, and the middle one is the point: `None` is a clean shelf,
    /// `Some(Ok)` is work to offer back, and `Some(Err)` is *there was something
    /// here and it cannot be read*. Collapsing that last case into `None` would
    /// mean the one run where the copy is broken is also the one run that says
    /// nothing about it.
    pub fn find(&self) -> Option<Result<Found, String>> {
        let path = self.snapshot_path();
        if !path.exists() {
            return None;
        }
        Some(self.read(&path))
    }

    fn read(&self, path: &Path) -> Result<Found, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        // Parsed now, and thrown away, purely so that nothing is offered that
        // cannot be delivered. See [`Found`] for why the text is what is kept.
        Project::from_json(&json).map_err(|e| e.to_string())?;
        let age = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok());
        // A missing or unreadable sidecar is not a failure — it costs the
        // remembered path and nothing else.
        let origin = std::fs::read_to_string(self.meta_path())
            .ok()
            .and_then(|m| serde_json::from_str::<Meta>(&m).ok())
            .and_then(|m| m.origin);
        Ok(Found { json, origin, age })
    }

    /// Drop the copy. Called on every path that ends the unsaved work — a save,
    /// a New, and a close the user has agreed to.
    ///
    /// A file that is already gone is a success: this runs on ordinary exits,
    /// where most of the time there was never a copy to remove.
    pub fn clear(&self) -> Result<(), String> {
        for path in [self.snapshot_path(), self.meta_path()] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("could not remove {}: {e}", path.display())),
            }
        }
        Ok(())
    }
}

/// Write `contents` to a `.tmp` sibling and rename it over `path`.
///
/// See the module header: the event this whole file exists for can land in the
/// middle of this function, and a reader must never see half a session.
fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("could not replace {}: {e}", path.display()))
}
