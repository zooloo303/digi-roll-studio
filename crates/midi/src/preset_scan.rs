//! Reading a whole +Drive bank into a tag index — the scan half of PLAN.md
//! §10.6 step 4.
//!
//! [`crate::preset_scan::scan_bank`] is the only long-running read this codebase
//! has. A DN2 has **1,189 occupied presets**; at one Open/Read/Close round trip
//! each, a full library is minutes rather than seconds. Three properties follow
//! from that, and they are the reason this is its own module rather than a loop
//! inside the panel:
//!
//! * **It is cancellable at every slot.** The caller owns an [`AtomicBool`] and
//!   this checks it before each preset, not before each bank.
//! * **A cancelled scan still returns its work.** The partial [`BankIndex`] comes
//!   back so the caller can save it, and a later scan resumes from
//!   [`BankIndex::missing`]. Nine minutes of reading must not be thrown away
//!   because somebody closed a panel.
//! * **It reports progress per preset**, because a bar that only moves per bank
//!   moves eight times in nine minutes, which is indistinguishable from hung.
//!
//! # One unreadable preset does not sink a bank
//!
//! A slot that fails to read or decode is counted in [`ScanReport::skipped`] and
//! the scan carries on. This is `decode_kit_sounds`' rule — one unreadable sound
//! should not cost the browser the other fifteen — applied to a bank.
//!
//! # The A4 stops the scan immediately, and that is not the same thing
//!
//! An A4's presets carry no foot magic, so **none** of them can be decoded (see
//! `drive::decode_drive_preset`). Grinding through 128 slots to skip all 128 is
//! not resilience, it is a hang with a progress bar. So the first
//! [`DriveError::UndecodableContainer`] ends the scan with
//! [`ScanError::BoxNotIndexable`], which is a distinct answer meaning *this box
//! cannot be tagged at all* — the browser should list it and hide the tag grid,
//! per §10.2. Distinguishing "this preset is odd" from "this box is not
//! supported" is the whole reason these are two variants and not one.

use std::sync::atomic::{AtomicBool, Ordering};

use digi_protocol::drive::{decode_drive_preset, parse_list_entries, DriveError};
use digi_protocol::preset_index::{BankIndex, IndexEntry};

use crate::{ElektronDevice, MidiError};

/// The two things a scan needs from a box: which slots are occupied, and the
/// bytes of one preset.
///
/// **This trait exists so the scan's decisions can be tested without a box.**
/// Resume, cancel, skip-and-continue and the A4 stop are all branches that only
/// run on hardware otherwise, and `DEVELOPMENT.md` lesson 4 is what happens when
/// the only fixture available makes two different rules agree. A scan is minutes
/// long and is the first thing a user meets; its branches are worth pinning.
///
/// [`ElektronDevice`] is the real implementation and does the listing parse, so
/// the loop below is the same code in tests and on a desk.
pub trait PresetSource {
    /// Occupied preset slots in `bank`, in the order the box listed them.
    fn occupied_slots(&mut self, bank: &str) -> Result<Vec<u32>, ScanError>;
    /// One preset file's bytes.
    fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError>;
}

impl PresetSource for ElektronDevice {
    fn occupied_slots(&mut self, bank: &str) -> Result<Vec<u32>, ScanError> {
        let listing = self.drive_list(bank, 0, 0).map_err(ScanError::Listing)?;
        let entries =
            parse_list_entries(&listing.entry_bytes, listing.count).map_err(ScanError::Layout)?;
        Ok(entries
            .iter()
            .filter(|e| e.is_occupied() && e.children.is_none() && e.size.is_some_and(|s| s > 0))
            .filter_map(|e| e.index)
            .collect())
    }

    fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError> {
        self.drive_read_file(path)
    }
}

/// What a scan reports as it goes, once per preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanProgress {
    /// Slots finished, including skipped ones — what a bar is drawn from.
    pub done: u32,
    /// Slots this scan intends to read. **Not the bank's size**: a resumed scan
    /// counts only what it lacks, so a progress bar reflects the work actually
    /// left rather than restarting at zero.
    pub total: u32,
    /// The slot just finished.
    pub slot: u32,
    /// Its name, or `None` if it could not be read.
    pub name: Option<String>,
}

/// How a scan ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReport {
    /// Slots read and indexed by this scan.
    pub indexed: u32,
    /// Slots that failed to read or decode and were passed over. A non-zero
    /// count is worth showing: it means the index is missing presets the box
    /// says it has, and a re-scan is free to retry them.
    pub skipped: u32,
    /// Whether the caller's cancel flag ended it. The index is still valid and
    /// still worth saving — see the module doc.
    pub cancelled: bool,
}

#[derive(Debug)]
pub enum ScanError {
    /// The bank could not be listed, so there is nothing to scan.
    Listing(MidiError),
    /// The listing came back in a shape the parser does not recognise.
    Layout(DriveError),
    /// **This box's presets cannot be decoded at all** — in practice the A4,
    /// whose containers carry no foot magic. Not a fault in the bank or the
    /// cable: a statement about the box, and the browser should list it without
    /// a tag grid rather than retry.
    BoxNotIndexable { why: DriveError },
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listing(e) => write!(f, "could not list the bank: {e}"),
            Self::Layout(e) => write!(f, "the listing did not parse: {e}"),
            Self::BoxNotIndexable { why } => {
                write!(f, "this box's presets cannot be tagged: {why}")
            }
        }
    }
}

impl std::error::Error for ScanError {}

/// Read every occupied preset in `bank` into an index.
///
/// `existing` is what a resume starts from — pass the loaded [`BankIndex`] and
/// only the slots it lacks are read. Pass `None` for a fresh scan, or after the
/// user has asked for a rebuild.
///
/// The returned index is always worth saving, including when
/// [`ScanReport::cancelled`] is set.
pub fn scan_bank(
    device: &mut impl PresetSource,
    model_key: &str,
    build: &str,
    bank: &str,
    existing: Option<BankIndex>,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<(BankIndex, ScanReport), ScanError> {
    // Occupancy is sparse and a resume should cost the slots it lacks, not the
    // size of the bank — so the slot list is collected and carried rather than
    // recomputed as a range.
    let slots = device.occupied_slots(bank)?;

    let mut index = existing.unwrap_or_else(|| BankIndex::new(model_key, bank, build, 0));
    // Refreshed from this listing rather than trusted from the loaded index: the
    // bank may have gained presets since, and that is exactly what
    // `is_complete` is meant to notice.
    index.occupied = slots.len() as u32;

    let todo = index.missing(&slots);
    let total = todo.len() as u32;
    let mut report = ScanReport { indexed: 0, skipped: 0, cancelled: false };

    for (n, slot) in todo.into_iter().enumerate() {
        // Checked before the read rather than after, so a cancel costs at most
        // one round trip rather than one preset.
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }

        let path = format!("{}/{slot}", bank.trim_end_matches('/'));
        let name = match device.read_preset(&path) {
            Ok(bytes) => match decode_drive_preset(&bytes) {
                Ok(sound) => {
                    let name = sound.name.clone();
                    index.insert(
                        slot,
                        IndexEntry {
                            name: name.clone(),
                            tag_mask: sound.tag_mask,
                            size: sound.bytes.len() as u32,
                        },
                    );
                    report.indexed += 1;
                    Some(name)
                }
                // The A4, and the one failure that is about the box rather than
                // the slot. Stopping here rather than skipping 128 times.
                Err(why @ DriveError::UndecodableContainer { .. }) => {
                    return Err(ScanError::BoxNotIndexable { why });
                }
                Err(_) => {
                    report.skipped += 1;
                    None
                }
            },
            Err(_) => {
                report.skipped += 1;
                None
            }
        };

        on_progress(ScanProgress { done: n as u32 + 1, total, slot, name });
    }

    Ok((index, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A box made of the real captures. The preset bytes are the ones committed
    /// under `digi_protocol`'s `tests/fixtures/drive/`, so what this scan
    /// decodes is what three boxes actually sent on 2026-08-29 — a fake source
    /// wrapped around real files, rather than bytes invented to suit the parser.
    struct FakeBox {
        slots: Vec<u32>,
        files: Vec<(u32, Vec<u8>)>,
        /// Slots that error on read, to exercise skip-and-continue.
        dead: Vec<u32>,
        reads: Vec<String>,
    }

    fn capture(name: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../protocol/tests/fixtures/drive")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    impl FakeBox {
        fn dn2() -> Self {
            let files = vec![
                (1, capture("digitone2-soundbanks-A-1-HIDDEN-TEARS-2026-08-29.bin")),
                (2, capture("digitone2-soundbanks-A-2-MONOLOW-2026-08-29.bin")),
                (6, capture("digitone2-soundbanks-A-6-7THPAD-2026-08-29.bin")),
            ];
            Self { slots: vec![1, 2, 6], files, dead: vec![], reads: vec![] }
        }

        fn a4() -> Self {
            let files = vec![
                (1, capture("analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin")),
                (2, capture("analogfour-soundbanks-A-2-SQUARE-WAVE-2026-08-29.bin")),
            ];
            Self { slots: vec![1, 2], files, dead: vec![], reads: vec![] }
        }
    }

    impl PresetSource for FakeBox {
        fn occupied_slots(&mut self, _bank: &str) -> Result<Vec<u32>, ScanError> {
            Ok(self.slots.clone())
        }
        fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError> {
            self.reads.push(path.to_string());
            let slot: u32 = path.rsplit('/').next().unwrap().parse().unwrap();
            if self.dead.contains(&slot) {
                return Err(MidiError::Timeout);
            }
            Ok(self.files.iter().find(|(s, _)| *s == slot).map(|(_, b)| b.clone()).unwrap())
        }
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn a_scan_indexes_every_occupied_slot_with_its_tags() {
        let mut boxx = FakeBox::dn2();
        let (index, report) =
            scan_bank(&mut boxx, "digitone2", "0050", "/soundbanks/A", None, &never(), |_| {})
                .expect("scan");

        assert_eq!(report.indexed, 3);
        assert_eq!(report.skipped, 0);
        assert!(!report.cancelled);
        assert!(index.is_complete());
        assert_eq!(index.entries[&1].name, "HIDDEN TEARS");
        assert_eq!(index.entries[&1].tag_mask, 0x0488_0804);
        // Two struct sizes in one bank, carried through the scan rather than
        // flattened to a per-box constant.
        assert_eq!(index.entries[&2].size, 319);
        assert_eq!(index.entries[&6].size, 359);
    }

    /// The resume, and the property that makes a nine-minute scan survivable:
    /// a second run reads only what the first lacked.
    #[test]
    fn a_resumed_scan_reads_only_the_missing_slots() {
        let mut first = FakeBox::dn2();
        let cancel = AtomicBool::new(false);
        let (partial, report) = scan_bank(
            &mut first,
            "digitone2",
            "0050",
            "/soundbanks/A",
            None,
            &cancel,
            |p| {
                // Cancel after the first preset lands.
                if p.done == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .expect("first pass");

        assert!(report.cancelled);
        assert_eq!(report.indexed, 1);
        assert!(!partial.is_complete(), "a cancelled scan is not a complete index");

        let mut second = FakeBox::dn2();
        let (full, report) = scan_bank(
            &mut second,
            "digitone2",
            "0050",
            "/soundbanks/A",
            Some(partial),
            &never(),
            |_| {},
        )
        .expect("resume");

        assert!(full.is_complete());
        assert_eq!(report.indexed, 2, "the resume read two, not three");
        assert_eq!(second.reads, vec!["/soundbanks/A/2", "/soundbanks/A/6"]);
    }

    /// Progress counts the work this scan is doing, not the bank's size — so a
    /// resumed scan's bar does not restart at zero of everything.
    #[test]
    fn progress_totals_the_work_left_not_the_whole_bank() {
        let mut boxx = FakeBox::dn2();
        let mut partial = BankIndex::new("digitone2", "/soundbanks/A", "0050", 3);
        partial.insert(
            1,
            IndexEntry { name: "HIDDEN TEARS".into(), tag_mask: 0, size: 319 },
        );

        let mut seen = Vec::new();
        scan_bank(
            &mut boxx,
            "digitone2",
            "0050",
            "/soundbanks/A",
            Some(partial),
            &never(),
            |p| seen.push((p.done, p.total, p.slot)),
        )
        .expect("scan");

        assert_eq!(seen, vec![(1, 2, 2), (2, 2, 6)]);
    }

    /// One unreadable preset does not cost the bank the others — `decode_kit_sounds`'
    /// rule, applied to a scan.
    #[test]
    fn one_dead_slot_is_skipped_and_the_rest_still_index() {
        let mut boxx = FakeBox::dn2();
        boxx.dead = vec![2];

        let (index, report) =
            scan_bank(&mut boxx, "digitone2", "0050", "/soundbanks/A", None, &never(), |_| {})
                .expect("scan");

        assert_eq!(report.indexed, 2);
        assert_eq!(report.skipped, 1);
        assert!(!index.entries.contains_key(&2));
        assert!(index.entries.contains_key(&6), "the scan carried on past the dead slot");
    }

    /// The A4 stops at the first preset rather than skipping all of them. A
    /// scan that reads 128 slots to skip 128 is a hang with a progress bar, and
    /// "this box cannot be tagged" is a different answer from "this preset is
    /// odd" — which is why it is a distinct error and asserted as one.
    #[test]
    fn an_a4_stops_the_scan_at_the_first_preset() {
        let mut boxx = FakeBox::a4();
        let err = scan_bank(&mut boxx, "analogfour", "0195", "/soundbanks/A", None, &never(), |_| {})
            .expect_err("an A4 cannot be indexed");

        assert!(matches!(err, ScanError::BoxNotIndexable { .. }), "got {err:?}");
        assert_eq!(boxx.reads.len(), 1, "it must not grind through the whole bank");
    }

    /// A bank that gained a preset since its last scan is picked up, because the
    /// declared count is refreshed from the listing rather than trusted from the
    /// loaded index.
    #[test]
    fn a_bank_that_grew_since_the_last_scan_is_rescanned_for_the_new_slot() {
        let mut boxx = FakeBox::dn2();
        let mut stale = BankIndex::new("digitone2", "/soundbanks/A", "0050", 2);
        stale.insert(1, IndexEntry { name: "HIDDEN TEARS".into(), tag_mask: 0, size: 319 });
        stale.insert(2, IndexEntry { name: "MONOLOW".into(), tag_mask: 0, size: 319 });
        assert!(stale.is_complete(), "complete as far as it knew");

        let (fresh, report) = scan_bank(
            &mut boxx,
            "digitone2",
            "0050",
            "/soundbanks/A",
            Some(stale),
            &never(),
            |_| {},
        )
        .expect("scan");

        assert_eq!(report.indexed, 1);
        assert_eq!(fresh.occupied, 3);
        assert!(fresh.is_complete());
    }
}
