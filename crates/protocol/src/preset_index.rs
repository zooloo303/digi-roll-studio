//! The +Drive preset tag index: what a bank's presets are called, and which
//! tags each one carries.
//!
//! # Why an index exists at all
//!
//! A `0x53` List entry carries a name, an index, a size, permissions and
//! occupancy. It does **not** carry a tag mask — tags live at sound-struct `+8`,
//! *inside* the file. So the browser PLAN.md §10.3 describes, with
//! `Bass-Glitchy` visible before you click anything, cannot be drawn from a
//! directory listing at all. It needs every preset opened and read: **1,189 on
//! a DN2**, 148 on a DT2.
//!
//! That is a scan, not a browse, and it is the longest-running read this app
//! has ever done. Everything in this module follows from that one fact.
//!
//! # Keyed by device *and* bank, which is the whole design
//!
//! One file per (device, bank). Not one per device, and not one for everything:
//!
//!   * a second open of the panel is instant, because the bank is already on
//!     disk;
//!   * a box that gains presets can have **one bank rebuilt rather than all
//!     eight**, which is the difference between a five-second refresh and a
//!     nine-minute one;
//!   * a scan interrupted halfway leaves seven whole banks and one partial one,
//!     rather than one corrupt everything.
//!
//! # Partial is a normal state, not a failure
//!
//! [`BankIndex`] is a map from slot to entry and does not pretend to be
//! complete. A scan that is cancelled, or that dies with the cable pulled out,
//! saves what it has; [`BankIndex::missing`] is what the next scan asks for.
//! **Browsing must never block on tagging** — a bank with no index still lists,
//! names and slots come from the listing, and tags fill in behind. So a caller
//! that finds no index, or half of one, has a normal situation to render rather
//! than an error to report.
//!
//! # What is not stored, and why
//!
//! Not the preset bytes. The index is the *answer* to a scan — name, tag mask,
//! size — and a copy of 1,189 sound structs is a cache of the +Drive, which is
//! a different and much larger promise. Loading a preset onto a track re-reads
//! the file, so nothing here can go stale in a way that puts wrong bytes on a
//! box.
//!
//! The OS build **is** stored, because the index is derived data and the thing
//! that most plausibly invalidates it is the box's firmware changing under it.
//! Recorded rather than enforced: a build mismatch is a fact a caller may use to
//! offer a rebuild, not grounds for this module to throw away work.
//!
//! # Layout
//!
//! ```text
//!   <dir>/
//!     digitone2-soundbanks-A.json
//!     digitone2-soundbanks-B.json
//!     digitakt2-soundbanks-A.json
//! ```
//!
//! Directory injectable rather than global, the same way [`crate::backup_stash::Stash`]
//! takes one: tests get their own, and the app can put it where it keeps
//! everything else instead of somewhere this crate decided.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One preset, as the index remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// The name from the sound struct, not from the listing. They agree in
    /// every capture taken so far, but the struct's is the one the tag mask
    /// came out of, so it is the one recorded — a name and a mask from
    /// different reads could disagree and nothing would say so.
    pub name: String,
    /// The raw tag bitmask from sound-struct `+8`. Stored raw, and named by
    /// `sound::tag_names` at the point of display: a stored *label* would rot
    /// the moment the calibration changed, and a mask cannot.
    ///
    /// **That has now paid for itself twice.** The tables were corrected once
    /// and then split per box on 2026-08-29 — the same bit means Kick on a digi
    /// and Bass on an A4 — and every index written before either change still
    /// reads correctly, because none of them stored a word.
    pub tag_mask: u32,
    /// The struct's measured length. Kept because it varies **within** a bank —
    /// one DN2 bank holds both 319 and 359 — so it is per-preset data rather
    /// than a per-box constant somebody could recompute later.
    pub size: u32,
    /// The container magic this preset carries — `BEEFBACE`, `DN1S`,
    /// `BEEFBABA`. `None` for an entry written before 2026-08-29, which is
    /// *unknown* rather than native.
    ///
    /// **Why a browser needs this on disk, and why it is the magic rather than
    /// a verdict.** A DN2's library is two formats: 388 of 1,189 presets are
    /// Digitone mk1 files, spread across banks B, C and D, and the box will not
    /// take one onto a kit track — probed 2026-08-29, it ignores the store
    /// outright. They browse, they search, they tag, and they cannot load. A
    /// browser that cannot tell them apart makes a third of the library refuse
    /// after a round trip with no warning, which is what shipping without this
    /// field did for a day.
    ///
    /// It is recorded as the **magic** and not as a `loadable: bool` for the
    /// same reason [`IndexEntry::tag_mask`] is a mask and not a list of words:
    /// a verdict is policy, policy is `drive::preset_load_payload`'s and it may
    /// change, and every index written before the change would then be wrong.
    /// A magic is a fact about the file.
    ///
    /// `#[serde(default)]` so an index from before this field reads as `None`
    /// and is backfilled by [`BankIndex::missing`] on the next READ TAGS.
    #[serde(default)]
    pub format: Option<u32>,
}

/// One bank's worth of index, complete or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BankIndex {
    /// The model key — `digitone2`, `digitakt2`. Keyed off the model rather
    /// than a `Spec`, because the A4 has no `Spec::device` at all and §10's
    /// code is required to key on the model for exactly that reason.
    pub device: String,
    /// The +Drive path this indexes, e.g. `/soundbanks/A`.
    pub bank: String,
    /// The OS build the scan ran against. See the module doc: recorded as a
    /// fact, not enforced as a guard.
    pub build: String,
    /// How many occupied slots the listing declared when the scan started.
    /// [`BankIndex::is_complete`] compares against this rather than against a
    /// count of entries, so a bank that grew since the scan reads as
    /// incomplete instead of as done.
    pub occupied: u32,
    /// Slot index to entry. A [`BTreeMap`] so the on-disk JSON is ordered and
    /// two scans of the same bank produce the same file — a diff that is noise
    /// is a diff nobody reads.
    pub entries: BTreeMap<u32, IndexEntry>,
}

impl BankIndex {
    /// An empty index for a bank about to be scanned.
    pub fn new(device: &str, bank: &str, build: &str, occupied: u32) -> Self {
        Self {
            device: device.to_string(),
            bank: bank.to_string(),
            build: build.to_string(),
            occupied,
            entries: BTreeMap::new(),
        }
    }

    /// Whether every occupied slot the listing declared has an entry.
    ///
    /// Compares against the declared count rather than against `entries.len()`
    /// alone, so a bank that has gained presets since the scan is incomplete
    /// rather than quietly done.
    pub fn is_complete(&self) -> bool {
        self.entries.len() as u32 >= self.occupied
    }

    /// Which of `slots` this index does not yet hold — what a resumed scan asks
    /// the box for.
    ///
    /// Takes the slots rather than computing a range, because occupancy is
    /// sparse: a bank of 256 with 12 presets in it should cost 12 reads on a
    /// resume, not 256.
    /// **An entry with no recorded format counts as missing**, so an index
    /// written before that field existed is backfilled by the next scan rather
    /// than needing its file deleted by hand. It costs exactly the entries that
    /// lack it, resumes and cancels like any other scan, and is a no-op once
    /// done — which is the whole reason a resume is keyed on slots rather than
    /// on a version number.
    pub fn missing(&self, slots: &[u32]) -> Vec<u32> {
        slots
            .iter()
            .copied()
            .filter(|s| self.entries.get(s).is_none_or(|e| e.format.is_none()))
            .collect()
    }

    /// How many entries predate [`IndexEntry::format`] and so cannot say
    /// whether their preset is loadable.
    ///
    /// The panel asks so it can keep offering READ TAGS on a library that is
    /// fully *tagged* — the tags are real and complete, and a second fact about
    /// the same files is not. Reporting it as untagged would be a lie about the
    /// thing this index is named for.
    pub fn unread_formats(&self) -> usize {
        self.entries.values().filter(|e| e.format.is_none()).count()
    }

    /// Record one preset. Re-scanning a slot overwrites it, which is what makes
    /// a rebuild of a single bank meaningful.
    pub fn insert(&mut self, slot: u32, entry: IndexEntry) {
        self.entries.insert(slot, entry);
    }

    /// Every slot carrying any of the tags in `mask`. The filter the browser's
    /// tag grid runs, and the reason the mask is stored raw.
    pub fn matching(&self, mask: u32) -> Vec<u32> {
        if mask == 0 {
            return self.entries.keys().copied().collect();
        }
        self.entries
            .iter()
            .filter(|(_, e)| e.tag_mask & mask != 0)
            .map(|(slot, _)| *slot)
            .collect()
    }
}

#[derive(Debug)]
pub enum IndexError {
    /// The index directory could not be created.
    Dir { path: PathBuf, why: String },
    /// A bank's file could not be written. **A scan whose save fails has done
    /// nine minutes of reading for nothing**, so this is worth surfacing rather
    /// than swallowing.
    Write { path: PathBuf, why: String },
    /// No home/app-data directory could be determined. Only
    /// [`PresetIndex::default_dir`] produces this.
    NoDefaultDir,
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dir { path, why } => {
                write!(f, "couldn't create the preset index folder {}: {why}", path.display())
            }
            Self::Write { path, why } => {
                write!(f, "couldn't write the preset index {}: {why}", path.display())
            }
            Self::NoDefaultDir => write!(f, "couldn't work out where to keep the preset index"),
        }
    }
}

impl std::error::Error for IndexError {}

/// A directory of per-bank index files.
#[derive(Debug, Clone)]
pub struct PresetIndex {
    dir: PathBuf,
}

impl PresetIndex {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The index in the platform's per-user application-data directory.
    ///
    /// Hand-rolled from environment variables for the same reason
    /// [`crate::backup_stash::Stash::default_dir`] is: it is three `cfg` arms
    /// against a crate that otherwise depends only on serde. `HOME` unset
    /// returns an error rather than writing a cache into the working directory.
    pub fn default_dir() -> Result<PathBuf, IndexError> {
        let base = if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
        } else if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
            })
        };
        base.map(|b| b.join("digi-roll-studio").join("preset-index"))
            .ok_or(IndexError::NoDefaultDir)
    }

    pub fn default_index() -> Result<Self, IndexError> {
        Self::default_dir().map(Self::at)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where one bank's index lives.
    ///
    /// The bank path becomes part of a filename, so its separators are flattened
    /// — `/soundbanks/A` is `soundbanks-A`. Every other byte is passed through a
    /// filter rather than trusted: a +Drive path is data from a box, and a box
    /// that answered with a `..` in a directory name should not be able to
    /// choose where this crate writes.
    pub fn path_for(&self, device: &str, bank: &str) -> PathBuf {
        let safe: String = bank
            .trim_matches('/')
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        self.dir.join(format!("{device}-{safe}.json"))
    }

    /// Load a bank's index, or `None` if there is not one.
    ///
    /// **`None` is the normal first-run answer**, and so is a file that no
    /// longer parses: an index is derived data, and the cost of a corrupt one
    /// is a re-scan rather than a lost anything. Returning `None` for both lets
    /// a caller treat "no tags yet" as one case instead of two.
    pub fn load(&self, device: &str, bank: &str) -> Option<BankIndex> {
        let path = self.path_for(device, bank);
        let text = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write a bank's index, creating the directory if needed.
    ///
    /// Saving a **partial** index is expected rather than exceptional — see the
    /// module doc. A cancelled scan calls this with what it got.
    pub fn save(&self, index: &BankIndex) -> Result<PathBuf, IndexError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| IndexError::Dir { path: self.dir.clone(), why: e.to_string() })?;
        let path = self.path_for(&index.device, &index.bank);
        let json = serde_json::to_string_pretty(index)
            .map_err(|e| IndexError::Write { path: path.clone(), why: e.to_string() })?;
        std::fs::write(&path, json)
            .map_err(|e| IndexError::Write { path: path.clone(), why: e.to_string() })?;
        Ok(path)
    }

    /// Forget one bank, so the next open re-scans it. Missing is success: the
    /// caller asked for it to be gone, and it is.
    pub fn forget(&self, device: &str, bank: &str) -> Result<(), IndexError> {
        let path = self.path_for(device, bank);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(IndexError::Write { path, why: e.to_string() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("digi-preset-index-test-{n}"))
    }

    fn entry(name: &str, mask: u32) -> IndexEntry {
        IndexEntry {
            name: name.to_string(),
            tag_mask: mask,
            size: 319,
            format: Some(crate::sound::SOUND_MAGIC_HEAD),
        }
    }

    #[test]
    fn a_saved_bank_comes_back_the_same() {
        let dir = temp();
        let index = PresetIndex::at(&dir);
        let mut bank = BankIndex::new("digitone2", "/soundbanks/A", "0050", 2);
        bank.insert(1, entry("HIDDEN TEARS", 0x0488_0804));
        bank.insert(2, entry("MONOLOW", 0x05a0_0400));

        index.save(&bank).expect("save");
        assert_eq!(index.load("digitone2", "/soundbanks/A").as_ref(), Some(&bank));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_index_yet_is_none_rather_than_an_error() {
        let index = PresetIndex::at(temp());
        assert!(index.load("digitone2", "/soundbanks/A").is_none());
    }

    /// The resume case, and the reason `missing` takes the slots rather than a
    /// count: occupancy is sparse, so a resumed scan should cost the slots it
    /// lacks and not the size of the bank.
    #[test]
    fn a_partial_index_asks_only_for_what_it_lacks() {
        let mut bank = BankIndex::new("digitone2", "/soundbanks/A", "0050", 4);
        bank.insert(1, entry("ONE", 0));
        bank.insert(7, entry("SEVEN", 0));

        assert_eq!(bank.missing(&[1, 4, 7, 9]), vec![4, 9]);
        assert!(!bank.is_complete());
    }

    /// A bank that has gained presets since its scan is incomplete, not done —
    /// which is why the declared count is stored rather than inferred from the
    /// entries.
    #[test]
    fn a_bank_that_grew_reads_as_incomplete() {
        let mut bank = BankIndex::new("digitakt2", "/soundbanks/A", "0071", 2);
        bank.insert(1, entry("ACIDD", 0x200));
        bank.insert(2, entry("BAM BASS", 0x400));
        assert!(bank.is_complete());

        bank.occupied = 3;
        assert!(!bank.is_complete(), "the box has one this index has never seen");
    }

    #[test]
    fn the_tag_filter_is_a_mask_test() {
        let mut bank = BankIndex::new("digitone2", "/soundbanks/A", "0050", 3);
        bank.insert(1, entry("BASSY", 0b0001));
        bank.insert(2, entry("PADDY", 0b0010));
        bank.insert(3, entry("BOTH", 0b0011));

        assert_eq!(bank.matching(0b0001), vec![1, 3]);
        assert_eq!(bank.matching(0b0010), vec![2, 3]);
        // No filter selected shows everything, rather than nothing — the state
        // a freshly opened panel is in.
        assert_eq!(bank.matching(0), vec![1, 2, 3]);
    }

    /// A +Drive path is data from a box. A box that answered with a `..` in a
    /// name must not be able to pick where this crate writes.
    #[test]
    fn a_bank_path_cannot_escape_the_index_directory() {
        let index = PresetIndex::at("/tmp/idx");
        let path = index.path_for("digitone2", "/../../etc/passwd");
        assert_eq!(path.parent().unwrap(), Path::new("/tmp/idx"));
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn forgetting_a_bank_that_was_never_there_is_success() {
        let index = PresetIndex::at(temp());
        assert!(index.forget("digitone2", "/soundbanks/A").is_ok());
    }

    #[test]
    fn a_corrupt_index_reads_as_no_index() {
        let dir = temp();
        std::fs::create_dir_all(&dir).unwrap();
        let index = PresetIndex::at(&dir);
        std::fs::write(index.path_for("digitone2", "/soundbanks/A"), b"not json").unwrap();

        assert!(index.load("digitone2", "/soundbanks/A").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
