//! The backup store: the last [`STASH_MAX`] patterns this app overwrote.
//!
//! Descended from `js/elektron/backup-stash.js`, but the browser's shape is gone
//! and the reason is worth stating, because it inverts one of that file's
//! decisions.
//!
//! In the browser, rule 1 (auto-backup) was served by a `.syx` **download**, and
//! the stash was a *second* copy in `localStorage` — needed because a download is
//! fire-and-forget: a cancelled save dialog or a blocked multi-download produces
//! no file and no error, and JS has no way to tell. So the stash was
//! best-effort; if it failed, the download had still run.
//!
//! On the desktop that is backwards. A filesystem write returns a result, so this
//! store cannot fail silently — which makes it good enough to be the *only*
//! automatic copy, and that in turn makes it **load-bearing**: [`Stash::stash`]
//! returns a `Result`, and [`crate::safe_write::safe_write_track`] aborts the
//! write when it fails. There is no automatic export to a downloads folder, and
//! [`crate::safe_write::WriteHooks::on_backup`] is now optional — exporting a
//! backup somewhere a user chose is a thing they *ask* for, from the restore list,
//! not a file that appears on every transfer. Decided with Neil 2026-08-18.
//!
//! ## Shape on disk
//!
//! A directory of `.syx` files plus `index.json`:
//!
//! ```text
//!   backups/
//!     index.json                                       newest first
//!     digitakt2-A02-backup-2026-08-01T12-34-56.syx     one framed dump each
//!     ...
//! ```
//!
//! Each file is a **replayable dump message**, not a bare payload — so any MIDI
//! utility can send one at a box even if this app will not start, which is the
//! last-resort recovery path and the reason this is files rather than rows in a
//! database. Fifty of them is roughly 6 MB.
//!
//! `index.json` holds only what a list view needs and the bytes do not cheaply
//! give: which box by name, what the box called the pattern, which track the
//! write was about. **The files are the truth and the index is a convenience** —
//! [`Stash::backups`] falls back to scanning the directory when the index is
//! missing or unreadable, so a corrupt index costs you the labels and not the
//! backups.
//!
//! ## Two rings, because a snapshot is not a backup
//!
//! A restore stores what the slot held *before* it ran — the state being reverted
//! away from, which may be the evidence of what went wrong. Useful, and **not
//! what someone opening a restore list is looking for**: they want the patterns
//! this app overwrote, and rows saying "here is the thing you just decided was
//! wrong" are noise between them. Neil's call, 2026-08-18.
//!
//! So the two kinds are counted separately — [`STASH_MAX`] pre-write backups and
//! [`SNAPSHOT_MAX`] pre-restore snapshots — and [`Stash::backups`] returns only
//! the first. That second cap is the half a display filter would not have fixed:
//! sharing one ring of fifty means ten restores evict ten real backups, silently,
//! and hiding the snapshots from the list would have hidden the eviction too.
//! [`Stash::snapshots`] and [`Stash::all`] are there for the paths that do want
//! them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::protocol::{split_sysex_stream, SysExKind};
use crate::safe_write::{pattern_dump_type, PatternKitFile};

/// How many backups the ring keeps.
///
/// Fifty, at roughly 125 KB a framed pattern, is about 6 MB — small enough to
/// keep without asking and deep enough to walk back through a whole session's
/// mistakes. The JS kept four because it was spending a browser's
/// `localStorage` quota.
pub const STASH_MAX: usize = 50;

/// How many pre-restore snapshots the store keeps, counted separately from
/// [`STASH_MAX`].
///
/// Ten rather than fifty because a snapshot answers a much shorter question — "I
/// restored the wrong thing, put it back" — and because whatever number this is,
/// it must not come out of the backups' fifty. See the module doc.
pub const SNAPSHOT_MAX: usize = 10;

/// The filename word, and the index's, for the snapshot a restore takes first.
pub const SNAPSHOT_KIND: &str = "pre-restore";

/// The index file's name inside the stash directory.
pub const INDEX_FILE: &str = "index.json";

/// What went wrong storing a backup.
///
/// Every variant aborts a write, which is the whole point of this type existing
/// where the JS returned a bare `false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StashError {
    /// The stash directory could not be created.
    Dir { path: PathBuf, why: String },
    /// The backup's own file could not be written.
    Write { path: PathBuf, why: String },
    /// No home/app-data directory could be determined, so there is nowhere to
    /// put a default stash. Only [`Stash::default_dir`] produces this.
    NoDefaultDir,
}

impl std::fmt::Display for StashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dir { path, why } => {
                write!(f, "couldn't create the backup folder {}: {why}", path.display())
            }
            Self::Write { path, why } => {
                write!(f, "couldn't write the backup {}: {why}", path.display())
            }
            Self::NoDefaultDir => write!(f, "couldn't work out where to keep backups"),
        }
    }
}

impl std::error::Error for StashError {}

/// The context a backup's own bytes do not carry, and a list view wants.
///
/// Supplied by the caller because the write flow already has all of it — it has
/// just decoded the destination pattern and knows which track it was asked
/// about. Nothing here is re-derived by decoding, and nothing is parsed back out
/// of a filename.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupContext {
    /// The box's human name, e.g. "Digitakt II".
    pub device_name: String,
    /// What the box called the kit in that slot, e.g. "JO_KIT". Recognising a
    /// pattern by name is the difference between a usable restore list and
    /// fifty rows of timestamps.
    pub kit_name: String,
    /// The track the write was about, for a pre-write backup. `None` for a
    /// whole-slot operation such as a pre-restore snapshot.
    pub track_index: Option<usize>,
}

/// One row of the restore list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashEntry {
    /// The file inside the stash directory. The key for [`Stash::export`] and
    /// [`Stash::payload`].
    pub file: String,
    pub slug: String,
    #[serde(default)]
    pub device_name: String,
    /// The bank label, e.g. "A02".
    pub bank: String,
    /// The slot, 0–127.
    pub index: u8,
    /// "backup" for pre-write, [`SNAPSHOT_KIND`] for the snapshot a restore takes.
    /// Read it through [`StashEntry::is_snapshot`] rather than comparing it: which
    /// ring an entry belongs to depends on the answer, and a string compared in
    /// several places is a misclassification waiting to happen.
    pub kind: String,
    #[serde(default)]
    pub kit_name: String,
    #[serde(default)]
    pub track_index: Option<usize>,
    /// UTC, `2026-08-01T12:34:56Z`. Sortable as a string, which is how the ring
    /// stays ordered without trusting file modification times.
    pub at: String,
}

impl StashEntry {
    /// Is this the snapshot a restore took, rather than a pattern this app
    /// overwrote?
    ///
    /// The one place the `kind` string is interpreted. Anything unrecognised
    /// counts as a backup, which is the safe direction: a row wrongly shown in
    /// the restore list is a puzzle, and one wrongly hidden from it is a lost
    /// pattern.
    pub fn is_snapshot(&self) -> bool {
        self.kind == SNAPSHOT_KIND
    }

    /// One line for a list view: what a person needs to pick a row.
    ///
    /// Here rather than in `app/` so every surface that shows a backup — the
    /// restore dialog, a log line, an error — words it the same way, for the
    /// reason [`crate::safe_write::write_result_message`] exists.
    pub fn summary(&self) -> String {
        let what = if self.is_snapshot() {
            "before a restore".to_string()
        } else {
            match self.track_index {
                Some(t) => format!("before writing T{}", t + 1),
                None => "before a write".to_string(),
            }
        };
        let name = if self.kit_name.is_empty() {
            String::new()
        } else {
            format!(" “{}”", self.kit_name)
        };
        let device = if self.device_name.is_empty() { &self.slug } else { &self.device_name };
        format!("{} {}{name} — {what}, {}", device, self.bank, self.at)
    }
}

/// Cap each kind independently, in place, preserving newest-first order.
///
/// The two rings of the module doc. Written as a free function because it is pure
/// list surgery and the property worth testing — that filling one ring cannot
/// evict the other — is easiest to state about a `Vec`.
fn prune_rings(entries: &mut Vec<StashEntry>) {
    let (mut backups, mut snapshots) = (0usize, 0usize);
    entries.retain(|e| {
        let (seen, cap) = if e.is_snapshot() {
            (&mut snapshots, SNAPSHOT_MAX)
        } else {
            (&mut backups, STASH_MAX)
        };
        *seen += 1;
        *seen <= cap
    });
}

/// Pull `(slug, bank, kind, stamp)` back out of a `pattern_kit_file` name.
///
/// Only used by the scan fallback, which is the one path with no index to read —
/// everywhere else these travel as fields, for exactly the reason this function is
/// fiddlier than it looks. `{slug}-{bank}-{kind}-{stamp}.syx`, where **the kind
/// can contain hyphens** (`pre-restore`) and so can the stamp
/// (`2026-08-01T12-34-56`), and `free_name` may have appended `-2`. So the split
/// is anchored on the stamp: the first four-digit run followed by a hyphen starts
/// it, and everything between the bank and that is the kind.
///
/// A naive `splitn(4, '-')` read `pre-restore` as `pre`, which put restore
/// snapshots back into the restore list the moment an index went missing. That was
/// a real bug, found by the test below, and it is why this is a named function with
/// its own cases rather than four lines inside `scan`.
fn parse_name(file: &str) -> (String, String, String, String) {
    let stem = file.strip_suffix(".syx").unwrap_or(file);
    let mut fields = stem.splitn(3, '-');
    let slug = fields.next().unwrap_or_default();
    let bank = fields.next().unwrap_or_default();
    let rest = fields.next().unwrap_or_default();

    let bytes = rest.as_bytes();
    let stamp_at = (0..bytes.len()).find(|&i| {
        // `NNNN-`, the year and its separator.
        bytes.len() >= i + 5
            && bytes[i..i + 4].iter().all(u8::is_ascii_digit)
            && bytes[i + 4] == b'-'
    });
    match stamp_at {
        // Trim the hyphen that joined the kind to the stamp.
        Some(i) if i > 0 => (
            slug.into(),
            bank.into(),
            rest[..i - 1].into(),
            rest[i..].into(),
        ),
        // No recognisable stamp: the whole remainder is the kind, and the row
        // still lists and still restores. A name we cannot read is not a backup we
        // should hide.
        _ => (slug.into(), bank.into(), rest.into(), String::new()),
    }
}

/// The platform's per-user application-data directory for this app.
///
/// Hand-rolled from environment variables rather than through the `dirs` crate:
/// it is three `cfg` arms, and this crate otherwise depends only on serde.
/// `HOME` being unset is the one case that has no answer, and it returns an
/// error rather than writing into the working directory.
///
/// **The one place the root is decided**, and a free function rather than a
/// `Stash` associated one because the stash is no longer the only thing under
/// it: [`Stash::default_dir`] is this plus `backups`, and the app's crash-copy
/// store (`ui::recovery`) is this plus `recovery`. Two hand-copies of the three
/// arms already existed when this was extracted — see
/// [`crate::preset_index`] — and a third was the thing to avoid.
pub fn app_data_dir() -> Result<PathBuf, StashError> {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
        })
    };
    base.map(|b| b.join("digi-roll-studio")).ok_or(StashError::NoDefaultDir)
}

/// A directory holding the backup ring.
///
/// Injectable rather than a global so tests get their own, and so the app can put
/// it beside its project files instead of somewhere this crate decided.
///
/// [`Stash::default_dir`] is what the app uses when the user has not chosen.
#[derive(Debug, Clone)]
pub struct Stash {
    dir: PathBuf,
}

impl Stash {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The stash in the platform's per-user application-data directory.
    pub fn default_dir() -> Result<PathBuf, StashError> {
        app_data_dir().map(|d| d.join("backups"))
    }

    /// The default stash, or an error saying why there isn't one.
    pub fn default_stash() -> Result<Self, StashError> {
        Self::default_dir().map(Self::at)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Store a backup and prune the ring to [`STASH_MAX`].
    ///
    /// **An `Err` here must abort the write that was about to happen.** This is
    /// the only automatic copy of the pattern being overwritten, so a failure to
    /// store it is a failure of rule 1, not a warning — see the module doc for
    /// why that differs from the JS.
    ///
    /// Returns the entry as it was recorded, which is what a caller shows and
    /// what a later restore is picked from.
    pub fn stash(
        &self,
        backup: &PatternKitFile,
        context: &BackupContext,
    ) -> Result<StashEntry, StashError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| StashError::Dir { path: self.dir.clone(), why: e.to_string() })?;

        // The filename carries a stamp to the second, so two backups of one slot
        // inside one second would collide. With fifty entries that is worth
        // avoiding rather than reasoning about: the second one gets a suffix.
        let file = self.free_name(&backup.name);
        let path = self.dir.join(&file);
        std::fs::write(&path, &backup.bytes)
            .map_err(|e| StashError::Write { path: path.clone(), why: e.to_string() })?;

        let entry = StashEntry {
            file,
            slug: backup.slug.clone(),
            device_name: context.device_name.clone(),
            bank: crate::pattern::bank_name(backup.index as usize),
            index: backup.index,
            kind: backup.kind.clone(),
            kit_name: context.kit_name.clone(),
            track_index: context.track_index,
            at: backup.at.iso(),
        };

        // The index and the pruning are conveniences over files that are already
        // safely on disk, so neither failing un-stashes the backup — and neither
        // is worth failing the write over. `backups()` rebuilds a lost index by
        // scanning.
        let mut entries = self.read_index();
        entries.insert(0, entry.clone());
        prune_rings(&mut entries);
        for stale in self.files().into_iter().filter(|f| !entries.iter().any(|e| &e.file == f)) {
            let _ = std::fs::remove_file(self.dir.join(stale));
        }
        let _ = self.write_index(&entries);

        Ok(entry)
    }

    /// A cheap marker that changes whenever the store does, for a view that wants
    /// to know when to read it again.
    ///
    /// The index file's modification time. [`Stash::stash`] rewrites the index on
    /// every backup and every snapshot, so any change to the store moves this, and
    /// a caller can compare it once a frame for the price of one `stat` rather than
    /// re-reading a directory or being told by whoever did the writing.
    ///
    /// **Being told was the first design, and it was worse.** A UI that re-listed
    /// when a write *it knew about* finished is a UI that goes stale when anything
    /// else touches the folder — a restore's own snapshot, a second instance, or a
    /// user tidying up — and it puts the freshness rule in three places that each
    /// have to remember to say so. Asking the store cannot be forgotten.
    ///
    /// `None` when there is no index yet, which is also the answer for an empty
    /// store. The one case it cannot see is a stash whose files landed but whose
    /// index write failed; that costs the labels anyway (see [`Stash::all`]), and a
    /// view is expected to offer a manual refresh regardless.
    pub fn generation(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(self.dir.join(INDEX_FILE)).ok()?.modified().ok()
    }

    /// The patterns this app overwrote, newest first — optionally only one box's.
    ///
    /// **This is the restore list**, so it excludes pre-restore snapshots: see the
    /// module doc for why they are noise here. [`Stash::snapshots`] has those and
    /// [`Stash::all`] has both.
    pub fn backups(&self, slug: Option<&str>) -> Vec<StashEntry> {
        let mut v = self.all(slug);
        v.retain(|e| !e.is_snapshot());
        v
    }

    /// The snapshots restores took of what they were about to overwrite, newest
    /// first — the "I restored the wrong thing" path.
    pub fn snapshots(&self, slug: Option<&str>) -> Vec<StashEntry> {
        let mut v = self.all(slug);
        v.retain(StashEntry::is_snapshot);
        v
    }

    /// Everything stored, both kinds, newest first — optionally only one box's.
    ///
    /// Read from the index, falling back to a directory scan when there is no
    /// usable index: the files are the truth. A scan cannot recover
    /// `device_name`, `kit_name` or `track_index`, which live only in the index,
    /// so those come back empty — the row is still restorable, which is the part
    /// that matters.
    pub fn all(&self, slug: Option<&str>) -> Vec<StashEntry> {
        let mut entries = self.read_index();
        if entries.is_empty() {
            entries = self.scan();
        } else {
            // An index that names a file somebody deleted underneath us must not
            // offer it.
            let present = self.files();
            entries.retain(|e| present.contains(&e.file));
        }
        entries.retain(|e| slug.is_none_or(|s| e.slug == s));
        entries
    }

    /// The pattern payload of one stored backup, ready for
    /// [`crate::safe_write::safe_restore_pattern_kit`].
    ///
    /// `None` if the file is gone or is not a pattern-kit dump — a stash
    /// directory is a place a user can drop things.
    pub fn payload(&self, file: &str) -> Option<Vec<u8>> {
        let bytes = std::fs::read(self.dir.join(file)).ok()?;
        Self::payload_of(&bytes)
    }

    /// Copy one stored backup to a path the user chose.
    ///
    /// The user-initiated half of what the browser did automatically on every
    /// write. A plain file copy, so the exported file is the same replayable
    /// dump message the stash holds.
    pub fn export(&self, file: &str, dest: &Path) -> std::io::Result<u64> {
        std::fs::copy(self.dir.join(file), dest)
    }

    // --- internals ------------------------------------------------------------

    fn payload_of(bytes: &[u8]) -> Option<Vec<u8>> {
        split_sysex_stream(bytes)
            .into_iter()
            .find(|m| {
                m.kind == SysExKind::Dump
                    // Per family, not the bare `0x50`: an A4 backup is framed
                    // `0x54` (`safe_write::pattern_dump_type`), and a filter
                    // that only knew the gen-2 opcode made every A4 backup a
                    // file the restore path could list and never read — found
                    // by the first test that tried, 2026-08-31.
                    && m.dump
                        .as_ref()
                        .is_some_and(|d| d.dump_type == pattern_dump_type(d.family))
            })?
            .dump
            .map(|d| d.payload)
    }

    /// `name`, or `name-2`, `name-3`… if that file is already taken.
    fn free_name(&self, name: &str) -> String {
        if !self.dir.join(name).exists() {
            return name.to_string();
        }
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, "syx"));
        (2u32..)
            .map(|n| format!("{stem}-{n}.{ext}"))
            .find(|candidate| !self.dir.join(candidate).exists())
            .expect("the integers do not run out")
    }

    /// The `.syx` filenames present in the directory.
    fn files(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "syx"))
            .filter_map(|p| p.file_name()?.to_str().map(str::to_string))
            .collect()
    }

    fn read_index(&self) -> Vec<StashEntry> {
        std::fs::read_to_string(self.dir.join(INDEX_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_index(&self, entries: &[StashEntry]) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(entries)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(self.dir.join(INDEX_FILE), json)
    }

    /// Rebuild a list from the files alone, newest-name first.
    ///
    /// The filename's stamp is only to the second and sorts lexically, which is
    /// good enough for a fallback: this runs when the index is gone, and getting
    /// the order slightly wrong beats showing nothing.
    fn scan(&self) -> Vec<StashEntry> {
        let mut files = self.files();
        files.sort_by(|a, b| b.cmp(a));
        files
            .into_iter()
            .filter_map(|file| {
                let bytes = std::fs::read(self.dir.join(&file)).ok()?;
                // Per family, as `payload_of`: a rebuilt index that only knew
                // the gen-2 opcode would silently drop every A4 backup file.
                let index = split_sysex_stream(&bytes)
                    .into_iter()
                    .find_map(|m| m.dump.filter(|d| d.dump_type == pattern_dump_type(d.family)))?
                    .index;
                let (slug, bank, kind, at) = parse_name(&file);
                Some(StashEntry {
                    slug,
                    bank,
                    kind,
                    at,
                    index,
                    file,
                    device_name: String::new(),
                    kit_name: String::new(),
                    track_index: None,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::FAMILY_DIGITAKT_2;
    use crate::safe_write::{pattern_kit_file, Timestamp};

    /// A payload the dump framing will accept, distinguishable by its first byte.
    fn payload(marker: u8) -> Vec<u8> {
        let mut p = vec![0u8; 64];
        p[0] = marker;
        p
    }

    fn file(index: u8, marker: u8, second: i64) -> PatternKitFile {
        pattern_kit_file(
            "digitakt2",
            FAMILY_DIGITAKT_2,
            index,
            &payload(marker),
            "backup",
            Timestamp::from_unix_seconds(1_785_587_696 + second),
        )
    }

    fn snapshot(index: u8, marker: u8, second: i64) -> PatternKitFile {
        pattern_kit_file(
            "digitakt2",
            FAMILY_DIGITAKT_2,
            index,
            &payload(marker),
            SNAPSHOT_KIND,
            Timestamp::from_unix_seconds(1_785_587_696 + second),
        )
    }

    fn context() -> BackupContext {
        BackupContext {
            device_name: "Digitakt II".into(),
            kit_name: "JO_KIT".into(),
            track_index: Some(2),
        }
    }

    struct Tmp(Stash);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("digi-roll-stash-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(Stash::at(dir))
        }
    }
    impl std::ops::Deref for Tmp {
        type Target = Stash;
        fn deref(&self) -> &Stash {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.dir());
        }
    }

    #[test]
    fn the_generation_marker_moves_when_the_store_does() {
        let stash = Tmp::new("generation");
        assert_eq!(stash.generation(), None, "an empty store has no index to stamp");

        stash.stash(&file(0, 0x11, 0), &context()).unwrap();
        let first = stash.generation().expect("a stored backup wrote an index");

        // A filesystem timestamp is only useful here if it actually moves between
        // two writes a moment apart, so this is the property being pinned rather
        // than the mechanism: the same store, twice, must not look unchanged.
        std::thread::sleep(std::time::Duration::from_millis(20));
        stash.stash(&file(1, 0x22, 0), &context()).unwrap();
        let second = stash.generation().expect("still an index");
        assert!(second > first, "{second:?} should be later than {first:?}");

        // And it does not move on its own, which is the half that makes it worth
        // comparing every frame.
        let _ = stash.backups(None);
        assert_eq!(stash.generation(), Some(second), "reading the store is not a change");
    }

    #[test]
    fn a_stashed_backup_comes_back_with_its_payload_and_its_labels() {
        let stash = Tmp::new("roundtrip");
        let f = file(3, 0x5a, 0);
        let recorded = stash.stash(&f, &context()).expect("a writable directory");

        let got = stash.backups(None);
        assert_eq!(got, vec![recorded.clone()]);
        assert_eq!(got[0].index, 3);
        assert_eq!(got[0].bank, "A04");
        assert_eq!(got[0].slug, "digitakt2");
        assert_eq!(got[0].device_name, "Digitakt II");
        assert_eq!(got[0].kit_name, "JO_KIT");
        assert_eq!(got[0].track_index, Some(2));
        assert_eq!(got[0].kind, "backup");
        assert_eq!(got[0].at, "2026-08-01T12:34:56Z");
        // And the bytes a restore would send.
        assert_eq!(stash.payload(&got[0].file), Some(payload(0x5a)));
    }

    #[test]
    fn the_ring_keeps_the_newest_fifty_and_deletes_the_files_it_drops() {
        let stash = Tmp::new("ring");
        for i in 0..(STASH_MAX as i64 + 5) {
            stash.stash(&file((i % 128) as u8, i as u8, i), &context()).unwrap();
        }
        let got = stash.backups(None);
        assert_eq!(got.len(), STASH_MAX, "the ring is capped");
        // Newest first: the last one stashed is at the top.
        assert_eq!(got[0].index, (STASH_MAX + 4) as u8);
        assert_eq!(got.last().unwrap().index, 5);
        // The dropped ones are gone from disk too, not just from the index —
        // otherwise the directory grows without limit behind a capped list.
        let on_disk = std::fs::read_dir(stash.dir())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "syx"))
            .count();
        assert_eq!(on_disk, STASH_MAX);
    }

    #[test]
    fn a_restore_snapshot_is_kept_but_stays_out_of_the_restore_list() {
        // Neil's call, 2026-08-18: a restore list is for the patterns this app
        // overwrote. "Here is the state you just decided was wrong" is a row
        // between you and the one you want.
        let stash = Tmp::new("noise");
        let overwritten = stash.stash(&file(0, 0x11, 0), &context()).unwrap();
        let reverted = stash
            .stash(&snapshot(0, 0x22, 1), &BackupContext { track_index: None, ..context() })
            .unwrap();

        assert_eq!(
            stash.backups(None).iter().map(|e| &e.file).collect::<Vec<_>>(),
            vec![&overwritten.file],
            "the restore list holds only what was overwritten"
        );
        // Kept, not discarded — the state being reverted away from may be the
        // evidence of what went wrong.
        assert_eq!(
            stash.snapshots(None).iter().map(|e| &e.file).collect::<Vec<_>>(),
            vec![&reverted.file]
        );
        assert_eq!(stash.all(None).len(), 2, "and both are there for a path that wants both");
        assert_eq!(stash.payload(&reverted.file), Some(payload(0x22)), "still restorable");
        assert!(reverted.is_snapshot());
        assert!(!overwritten.is_snapshot());
    }

    #[test]
    fn snapshots_cannot_evict_the_backups_they_sit_beside() {
        // **The half a display filter would not have fixed.** Sharing one ring of
        // fifty means ten restores silently push out ten real backups — and if the
        // snapshots were merely hidden from the list, the eviction would be hidden
        // with them. Two counts, so filling one cannot reach the other.
        let stash = Tmp::new("no-evict");
        for i in 0..STASH_MAX as i64 {
            stash.stash(&file((i % 128) as u8, i as u8, i), &context()).unwrap();
        }
        let before: Vec<String> = stash.backups(None).iter().map(|e| e.file.clone()).collect();
        assert_eq!(before.len(), STASH_MAX);

        for i in 0..(SNAPSHOT_MAX as i64 * 3) {
            stash
                .stash(
                    &snapshot(9, i as u8, 1_000 + i),
                    &BackupContext { track_index: None, ..context() },
                )
                .unwrap();
        }

        let after: Vec<String> = stash.backups(None).iter().map(|e| e.file.clone()).collect();
        assert_eq!(after, before, "every backup survived thirty restores");
        // And the snapshots are capped on their own terms rather than growing.
        assert_eq!(stash.snapshots(None).len(), SNAPSHOT_MAX);
        assert_eq!(stash.all(None).len(), STASH_MAX + SNAPSHOT_MAX);
    }

    #[test]
    fn an_unrecognised_kind_counts_as_a_backup_rather_than_being_hidden() {
        // The safe direction, and the one the scan fallback needs: a row wrongly
        // shown in the restore list is a puzzle, and one wrongly hidden is a lost
        // pattern.
        let stash = Tmp::new("odd-kind");
        let odd = stash
            .stash(
                &pattern_kit_file(
                    "digitakt2",
                    FAMILY_DIGITAKT_2,
                    0,
                    &payload(0x77),
                    "something-else",
                    Timestamp::from_unix_seconds(1_785_587_696),
                ),
                &context(),
            )
            .unwrap();
        assert!(!odd.is_snapshot());
        assert_eq!(stash.backups(None).len(), 1);
        assert_eq!(stash.snapshots(None).len(), 0);
    }

    #[test]
    fn a_failure_to_store_is_an_error_rather_than_a_shrug() {
        // The inversion this module exists to record: this is the only automatic
        // copy, so a write that cannot be stored has to say so loudly enough to
        // stop the transfer. A file where the directory should be is the cheapest
        // way to make `create_dir_all` fail.
        let blocked = std::env::temp_dir().join(format!("dr-stash-blocked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&blocked);
        std::fs::write(&blocked, b"not a directory").unwrap();
        let stash = Stash::at(&blocked);

        let err = stash.stash(&file(0, 1, 0), &context()).expect_err("nowhere to write");
        assert!(matches!(err, StashError::Dir { .. }), "{err:?}");
        assert!(err.to_string().contains("couldn't create the backup folder"));
        let _ = std::fs::remove_file(&blocked);
    }

    #[test]
    fn two_backups_of_one_slot_in_one_second_do_not_overwrite_each_other() {
        // The stamp is only to the second, and at fifty entries this stops being
        // something to reason about and starts being something to handle.
        let stash = Tmp::new("collide");
        let first = stash.stash(&file(0, 0x11, 0), &context()).unwrap();
        let second = stash.stash(&file(0, 0x22, 0), &context()).unwrap();
        assert_ne!(first.file, second.file);
        assert_eq!(second.file, first.file.replace(".syx", "-2.syx"));
        assert_eq!(stash.payload(&first.file), Some(payload(0x11)));
        assert_eq!(stash.payload(&second.file), Some(payload(0x22)));
        assert_eq!(stash.backups(None).len(), 2);
    }

    #[test]
    fn one_boxs_backups_can_be_asked_for_alone() {
        let stash = Tmp::new("slug");
        stash.stash(&file(0, 1, 0), &context()).unwrap();
        stash
            .stash(
                &pattern_kit_file(
                    "digitone2",
                    crate::protocol::FAMILY_DIGITONE_2,
                    1,
                    &payload(2),
                    "backup",
                    Timestamp::from_unix_seconds(1_785_587_697),
                ),
                &BackupContext { device_name: "Digitone II".into(), ..Default::default() },
            )
            .unwrap();
        assert_eq!(stash.backups(Some("digitakt2")).len(), 1);
        assert_eq!(stash.backups(Some("digitone2")).len(), 1);
        assert_eq!(stash.backups(Some("octatrack")).len(), 0);
        assert_eq!(stash.backups(None).len(), 2);
    }

    #[test]
    fn a_lost_index_costs_the_labels_and_not_the_backups() {
        // The files are the truth. A corrupt or deleted index must leave every
        // backup restorable, because the index is the part that exists for the
        // list view's convenience.
        let stash = Tmp::new("no-index");
        stash.stash(&file(1, 0x33, 0), &context()).unwrap();
        std::fs::write(stash.dir().join(INDEX_FILE), b"{ not json at all").unwrap();

        let got = stash.backups(None);
        assert_eq!(got.len(), 1, "the backup is still listed");
        assert_eq!(got[0].index, 1, "and the slot came out of the bytes");
        assert_eq!(got[0].slug, "digitakt2");
        assert_eq!(got[0].bank, "A02");
        assert_eq!(got[0].kind, "backup");
        assert_eq!(stash.payload(&got[0].file), Some(payload(0x33)), "and it can be restored");
        // What only the index knew is gone, and says so by being empty rather
        // than by being guessed.
        assert_eq!(got[0].device_name, "");
        assert_eq!(got[0].kit_name, "");
        assert_eq!(got[0].track_index, None);
    }

    #[test]
    fn the_scan_fallback_still_tells_a_snapshot_from_a_backup() {
        // The filename carries the kind, so losing the index must not merge the two
        // rings back together — that would put the noise straight back in the list.
        let stash = Tmp::new("scan-kind");
        stash.stash(&file(0, 0x11, 0), &context()).unwrap();
        stash
            .stash(&snapshot(1, 0x22, 1), &BackupContext { track_index: None, ..context() })
            .unwrap();
        std::fs::remove_file(stash.dir().join(INDEX_FILE)).unwrap();

        assert_eq!(stash.all(None).len(), 2);
        let listed = stash.backups(None);
        assert_eq!(listed.len(), 1, "the snapshot stayed out of the list");
        assert_eq!(listed[0].index, 0);
        assert_eq!(stash.snapshots(None).len(), 1);
        assert_eq!(stash.snapshots(None)[0].index, 1);
    }

    #[test]
    fn a_filename_parses_back_apart_even_when_the_kind_has_a_hyphen_in_it() {
        // A `splitn(4, '-')` read `pre-restore` as `pre` and put snapshots back in
        // the restore list whenever an index went missing.
        let p = |f: &str| parse_name(f);
        assert_eq!(
            p("digitakt2-A02-backup-2026-08-01T12-34-56.syx"),
            ("digitakt2".into(), "A02".into(), "backup".into(), "2026-08-01T12-34-56".into())
        );
        assert_eq!(
            p("digitone2-A01-pre-restore-2026-08-01T12-34-56.syx"),
            ("digitone2".into(), "A01".into(), "pre-restore".into(), "2026-08-01T12-34-56".into())
        );
        // `free_name`'s collision suffix lands after the stamp, so anchoring on the
        // stamp rather than counting from the end is what makes this hold.
        assert_eq!(
            p("digitakt2-A02-pre-restore-2026-08-01T12-34-56-2.syx").2,
            "pre-restore"
        );
        // And a name nobody here wrote still yields something listable.
        assert_eq!(p("whatever.syx"), ("whatever".into(), "".into(), "".into(), "".into()));
        assert_eq!(p("a-b-c.syx"), ("a".into(), "b".into(), "c".into(), "".into()));
    }

    #[test]
    fn an_index_naming_a_file_that_is_gone_does_not_offer_it() {
        let stash = Tmp::new("stale-index");
        let a = stash.stash(&file(0, 0x44, 0), &context()).unwrap();
        let b = stash.stash(&file(1, 0x55, 1), &context()).unwrap();
        std::fs::remove_file(stash.dir().join(&b.file)).unwrap();

        let got = stash.backups(None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file, a.file);
    }

    #[test]
    fn an_unreadable_stash_is_empty_rather_than_an_error() {
        // Listing is not the load-bearing half; only storing is. A stash
        // directory that does not exist yet is the normal first-run state.
        let stash = Stash::at(std::env::temp_dir().join("digi-roll-stash-nonexistent-xyz"));
        assert_eq!(stash.backups(None), vec![]);
        assert_eq!(stash.payload("anything.syx"), None);
    }

    #[test]
    fn a_file_that_is_not_a_pattern_dump_is_skipped_not_fatal() {
        let stash = Tmp::new("junk");
        let real = stash.stash(&file(0, 7, 0), &context()).unwrap();
        std::fs::write(stash.dir().join("notes.syx"), b"not sysex at all").unwrap();

        // The index path ignores it because it is not indexed…
        assert_eq!(stash.backups(None).len(), 1);
        assert_eq!(stash.payload("notes.syx"), None);
        // …and so does the scan path, which is where a dropped file would show up.
        std::fs::remove_file(stash.dir().join(INDEX_FILE)).unwrap();
        let scanned = stash.backups(None);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].file, real.file);
    }

    #[test]
    fn a_backup_can_be_exported_to_somewhere_the_user_chose() {
        // The user-initiated half of what the browser did on every write.
        let stash = Tmp::new("export");
        let entry = stash.stash(&file(0, 0x66, 0), &context()).unwrap();
        let dest = std::env::temp_dir().join(format!("dr-export-{}.syx", std::process::id()));
        let _ = std::fs::remove_file(&dest);

        stash.export(&entry.file, &dest).expect("the copy succeeds");
        let exported = std::fs::read(&dest).unwrap();
        // Still a replayable dump message, which is the point of exporting it.
        assert_eq!(Stash::payload_of(&exported), Some(payload(0x66)));
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn a_row_says_enough_to_be_picked_out_of_fifty() {
        let stash = Tmp::new("summary");
        let entry = stash.stash(&file(1, 1, 0), &context()).unwrap();
        assert_eq!(
            entry.summary(),
            "Digitakt II A02 “JO_KIT” — before writing T3, 2026-08-01T12:34:56Z"
        );
        // A pre-restore snapshot is a different sentence, because it is a
        // different thing: the state someone was reverting away from.
        let mut restore = entry.clone();
        restore.kind = "pre-restore".into();
        restore.track_index = None;
        assert!(restore.summary().contains("before a restore"));
        // And a row rebuilt by the scan fallback still reads.
        let bare = StashEntry {
            device_name: String::new(),
            kit_name: String::new(),
            track_index: None,
            ..entry
        };
        assert_eq!(bare.summary(), "digitakt2 A02 — before a write, 2026-08-01T12:34:56Z");
    }

    #[test]
    fn the_default_directory_is_the_platforms_own_app_data_folder() {
        let dir = Stash::default_dir().expect("this machine has a home directory");
        assert!(dir.ends_with("digi-roll-studio/backups"), "{}", dir.display());
        assert!(dir.is_absolute(), "{}", dir.display());
        if cfg!(target_os = "macos") {
            assert!(dir.starts_with(std::env::var("HOME").unwrap()));
            assert!(dir.to_string_lossy().contains("Library/Application Support"));
        }
    }
}
