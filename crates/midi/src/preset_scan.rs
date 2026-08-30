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
//! # A box that cannot be decoded stops the scan immediately
//!
//! When **none** of a box's presets can be indexed, grinding through 128 slots
//! to skip all 128 is not resilience, it is a hang with a progress bar. So the
//! first box-level parse failure ends the scan with
//! [`ScanError::BoxNotIndexable`], a distinct answer meaning *this box cannot be
//! tagged at all* — the browser should list it and hide the tag grid, per §10.2.
//! Distinguishing "this preset is odd" from "this box is not supported" is the
//! whole reason these are two variants and not one.
//!
//! **This used to be the A4, and as of 2026-08-29 it is no longer any box we
//! own.** The A4 indexes. Two separate mis-framings had it refused, and both are
//! written up on `drive::decode_drive_preset`: the missing foot magic, which
//! turned out not to matter because the file header declares the extent; and the
//! uncalibrated tag mask at `+8`, which was the real blocker and is now
//! calibrated against the A4's own filter grid — `sound::TAG_NAMES_A4`, checked
//! by `protocol/tests/drive_preset.rs` on eight captures.
//!
//! The variant stays, because the *situation* is real and will recur: the next
//! box to land will announce a container magic nobody has mapped, and it should
//! say so on the first slot rather than the hundred-and-twenty-eighth.

use std::sync::atomic::{AtomicBool, Ordering};

use digi_protocol::drive::{
    container_magic, decode_drive_preset, parse_list_entries, DriveError,
};
use digi_protocol::preset_index::{BankIndex, IndexEntry};

use crate::{ElektronDevice, MidiError};

/// The two things a scan needs from a box: which slots are occupied, and the
/// bytes of one preset.
///
/// **This trait exists so the scan's decisions can be tested without a box.**
/// Resume, cancel, skip-and-continue and the box-level stop are branches that only
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
    /// Why the **first** skipped slot was skipped, in the box's or the parser's
    /// own words.
    ///
    /// **Added 2026-08-29 after a scan reported "0 tagged, 388 skipped" and
    /// nothing could be learned from it.** A count of skips answers *how many*
    /// and never *what happened*, and at 388 of 388 the difference between "a
    /// few odd presets" and "every read is failing" is the entire diagnosis —
    /// which had to be reconstructed from index files on disk instead. The first
    /// one rather than all of them because a cascade produces one cause and 388
    /// copies of it; [`ScanReport::skipped`] already carries the multiplicity.
    pub first_skip: Option<String>,
}

#[derive(Debug)]
pub enum ScanError {
    /// The bank could not be listed, so there is nothing to scan.
    Listing(MidiError),
    /// The listing came back in a shape the parser does not recognise.
    Layout(DriveError),
    /// **This box's presets cannot be decoded at all.** Not a fault in the bank
    /// or the cable: a statement about the box, and the browser should list it
    /// without a tag grid rather than retry.
    ///
    /// No box on this desk reaches this any more — the A4, which used to, has
    /// been decoding since 2026-08-29. It is reachable by a box whose container
    /// magic is neither a digi's nor an A4's, which is the next unknown box.
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
    let mut report = ScanReport { indexed: 0, skipped: 0, cancelled: false, first_skip: None };

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
                            // Recorded from the same read the tags came out of,
                            // so a row can say whether the box's kit will take
                            // this preset before anyone double-clicks it. See
                            // `IndexEntry::format`.
                            format: container_magic(&bytes),
                        },
                    );
                    report.indexed += 1;
                    Some(name)
                }
                // The failures that are about the box rather than the slot:
                // a container magic nobody has mapped, or an A4-shaped file
                // whose layout does not hold. Both are systematic, so this
                // stops here rather than skipping 128 times.
                Err(
                    why @ (DriveError::UndecodableContainer { .. }
                    | DriveError::UnsizedContainer { .. }),
                ) => {
                    return Err(ScanError::BoxNotIndexable { why });
                }
                Err(why) => {
                    report.skipped += 1;
                    report.first_skip.get_or_insert_with(|| format!("{path}: {why}"));
                    None
                }
            },
            Err(why) => {
                report.skipped += 1;
                report.first_skip.get_or_insert_with(|| format!("{path}: {why}"));
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

    /// The container magic a native digi preset carries, for entries a test is
    /// standing up as though a modern scan had written them.
    fn native() -> Option<u32> {
        Some(digi_protocol::sound::SOUND_MAGIC_HEAD)
    }

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

        /// The other half of a DN2's library: bank C, where the Digitone mk1
        /// files live. 388 of that box's 1,189 presets are these.
        fn dn2_bank_c() -> Self {
            let files = vec![
                (1, capture("digitone2-soundbanks-C-1-ORGANIC-2026-08-29.bin")),
                (2, capture("digitone2-soundbanks-C-2-PHASEY-DUB-2026-08-29.bin")),
            ];
            Self { slots: vec![1, 2], files, dead: vec![], reads: vec![] }
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
    /// An index written before `IndexEntry::format` existed is re-read rather
    /// than skipped, so the next READ TAGS backfills it. Without this a library
    /// scanned before 2026-08-29 could only be brought up to date by deleting
    /// its files by hand — and a browser with no formats cannot say which of a
    /// DN2's 388 mk1 presets will refuse to load.
    #[test]
    fn an_entry_with_no_recorded_format_is_read_again() {
        let mut boxx = FakeBox::dn2();
        let mut old = BankIndex::new("digitone2", "/soundbanks/A", "0050", 3);
        old.insert(
            1,
            IndexEntry { name: "HIDDEN TEARS".into(), tag_mask: 0, size: 319, format: None },
        );

        let (fresh, report) = scan_bank(
            &mut boxx,
            "digitone2",
            "0050",
            "/soundbanks/A",
            Some(old),
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("the scan should run");

        assert_eq!(report.indexed, 3, "all three, including the one with no format");
        assert_eq!(
            fresh.entries[&1].format,
            Some(digi_protocol::sound::SOUND_MAGIC_HEAD),
            "and it now knows what container that preset carries"
        );
        assert_eq!(fresh.unread_formats(), 0);
    }

    /// And once every entry has one, the same scan is a no-op — the backfill
    /// costs exactly the entries that need it and nothing afterwards.
    #[test]
    fn a_backfilled_index_is_not_read_a_third_time() {
        let mut boxx = FakeBox::dn2();
        let (once, _) = scan_bank(
            &mut boxx,
            "digitone2",
            "0050",
            "/soundbanks/A",
            None,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();

        let (_, again) = scan_bank(
            &mut boxx,
            "digitone2",
            "0050",
            "/soundbanks/A",
            Some(once),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();

        assert_eq!(again.indexed, 0);
        assert_eq!(again.skipped, 0);
    }

    /// The mk1 presets a DN2 carries are recorded as mk1, which is the whole
    /// point of storing the magic: the browser marks them and the load path
    /// refuses them without a round trip.
    #[test]
    fn an_mk1_preset_is_indexed_with_its_own_container_magic() {
        let mut boxx = FakeBox::dn2_bank_c();
        let (index, report) = scan_bank(
            &mut boxx,
            "digitone2",
            "0050",
            "/soundbanks/C",
            None,
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("mk1 presets index like any other");

        assert!(report.indexed > 0, "they browse and tag");
        for (slot, entry) in &index.entries {
            assert_eq!(
                entry.format,
                Some(digi_protocol::sound::DN1_SOUND_MAGIC_HEAD),
                "slot {slot} is a mk1 file"
            );
        }
    }

    #[test]
    fn progress_totals_the_work_left_not_the_whole_bank() {
        let mut boxx = FakeBox::dn2();
        let mut partial = BankIndex::new("digitone2", "/soundbanks/A", "0050", 3);
        partial.insert(
            1,
            IndexEntry { name: "HIDDEN TEARS".into(), tag_mask: 0, size: 319, format: native() },
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

        // **And it says why.** A count without a cause is what made a real
        // "0 tagged, 388 skipped" on a DN2 impossible to read — see the field's
        // own doc. The slot is named too, because "which one first" is the other
        // half of the question.
        let why = report.first_skip.expect("a skip must carry its reason");
        assert!(why.contains("/soundbanks/A/2"), "{why}");
    }

    /// The A4 indexes like a digi, with the tags its own filter grid shows.
    ///
    /// **This test asserted the exact opposite until 2026-08-29**, and the
    /// reasons it gave for doing so were both wrong — see this module's header
    /// and `drive::decode_drive_preset`. It is kept pointing the other way
    /// rather than deleted, because "the A4 cannot be tagged" was believed
    /// firmly enough to be written into four files, and the cheapest guard
    /// against believing it again is a test that fails if it comes back.
    #[test]
    fn an_a4_indexes_like_a_digi() {
        let mut boxx = FakeBox::a4();
        let (index, report) =
            scan_bank(&mut boxx, "analogfour", "0195", "/soundbanks/A", None, &never(), |_| {})
                .expect("an A4 indexes");

        assert_eq!(report.indexed, 2);
        assert_eq!(report.skipped, 0);
        assert_eq!(boxx.reads.len(), 2, "both slots read, neither refused");

        let saw = index.entries.get(&1).expect("slot 1");
        assert_eq!(saw.name, "THE SAW");
        assert_eq!(saw.tag_mask, 0x0584_0003);
        assert_eq!(saw.size, 366, "sized by the header, since the A4 has no foot");
    }

    /// A box whose files cannot be sized stops at the **first** preset rather
    /// than skipping all of them. A scan that reads 128 slots to skip 128 is a
    /// hang with a progress bar, and "this box cannot be tagged" is a different
    /// answer from "this preset is odd" — which is why it is a distinct error
    /// and asserted as one.
    ///
    /// The A4 used to be this case and no longer is, so the box here is a made
    /// one: A4 files with their container shifted off the payload boundary, so
    /// the declared length no longer describes the struct. That is the shape of
    /// the failure a genuinely unknown box would produce, and the mechanism
    /// under test is the stop, not the box.
    #[test]
    fn a_box_whose_files_cannot_be_sized_stops_at_the_first_preset() {
        let mut boxx = FakeBox::a4();
        for (_, file) in boxx.files.iter_mut() {
            file.insert(31, 0x00);
        }
        let err =
            scan_bank(&mut boxx, "analogfour", "0195", "/soundbanks/A", None, &never(), |_| {})
                .expect_err("a box that cannot be sized cannot be indexed");

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
        stale.insert(
            1,
            IndexEntry { name: "HIDDEN TEARS".into(), tag_mask: 0, size: 319, format: native() },
        );
        stale.insert(
            2,
            IndexEntry { name: "MONOLOW".into(), tag_mask: 0, size: 319, format: native() },
        );
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
