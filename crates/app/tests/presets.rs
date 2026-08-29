//! The Presets panel's decisions, taken without a `Ui` and without a box.
//!
//! Everything asserted here is a rule PLAN.md §10.2–§10.4 states and the panel
//! has to hold: browsing never blocks on tagging, a box that cannot be tagged is
//! a state rather than a failure, a tag filter cannot ask a preset nothing has
//! read, and the index is keyed by the box that answered. Each of those is a
//! branch that would otherwise only run with a box on the desk — which is
//! `DEVELOPMENT.md` lesson 4's shape, and the same reason
//! `preset_scan::PresetSource` exists at all.

use std::time::Duration;

use digi_core::device::{Device, DeviceIo, PortRef, A4, DN2, DT2};
use digi_protocol::device::DeviceIdentity;
use digi_protocol::drive::ListEntry;
use digi_protocol::preset_index::{BankIndex, IndexEntry};
use digi_roll_studio::ui::presets::{
    bank_label, bank_paths, blocker, listing_rows, load_blocker, load_target, mismatched_box,
    rate_line, report_line, tag_names, BankData, Library, Row, Tagging, View, DEFAULT_BANKS,
    SOUNDBANKS,
};
use digi_roll_studio::ui::tracks::Selection;

fn port(name: &str) -> PortRef {
    PortRef { id: name.into(), name: name.into() }
}

fn wired(model: &'static digi_core::device::DeviceModel, name: &str) -> Device {
    let mut device = Device::new(name, model, 16);
    device.io =
        DeviceIo { input: Some(port("in")), output: Some(port("out")), ..DeviceIo::default() };
    device
}

fn identity(slug: &str, name: &str) -> DeviceIdentity {
    DeviceIdentity {
        product_id: 0,
        supported_ids: vec![],
        name: name.into(),
        slug: slug.into(),
        family: None,
        build: "0050".into(),
        version: "1.10".into(),
    }
}

fn entry(name: &str, mask: u32, size: u32) -> IndexEntry {
    IndexEntry { name: name.into(), tag_mask: mask, size }
}

fn path(bank: &str) -> String {
    format!("{SOUNDBANKS}/{bank}")
}

/// A library over the named banks, each starting empty — a DN2's.
///
/// The slug is not decoration: `tag_cells` names bits through
/// the table that box uses, and an empty slug names nothing at all — see
/// `Library::slug`.
fn library(banks: &[&str]) -> Library {
    Library {
        banks: banks.iter().map(|b| path(b)).collect(),
        slug: "digitone2".into(),
        ..Library::default()
    }
}

/// Put a bank's listing and/or index into a library.
fn put(lib: &mut Library, bank: &str, listing: Option<Vec<Row>>, index: Option<BankIndex>) {
    lib.data.insert(path(bank), BankData { listing, index });
}

/// The banks of `lib`, as a view over all of them.
fn all(lib: &Library) -> Vec<String> {
    View::All.banks(&lib.banks)
}

/// A long-form listing entry — an occupied preset slot.
fn preset(slot: u32, name: &str, size: u32) -> ListEntry {
    ListEntry {
        name: name.into(),
        is_dir: false,
        index: Some(slot),
        size: Some(size),
        permissions: Some(0),
        occupancy: Some((1, 1)),
        children: None,
    }
}

/// A short-form listing entry — a bank directory under `/soundbanks`.
fn bank_dir(name: &str, children: u32) -> ListEntry {
    ListEntry {
        name: name.into(),
        is_dir: true,
        index: None,
        size: None,
        permissions: None,
        occupancy: None,
        children: Some(children),
    }
}

// --- which boxes can be browsed ---------------------------------------------------

/// **The trap §10 was written around, stated as a test.** The A4 has no `Spec`
/// and no pattern dumps, so `Device::can_sysex` is false for it — and its
/// +Drive was read on 2026-08-28 regardless: it lists, opens and reads. Gating
/// this panel on the dump protocol the way `ui::transfer` correctly does would
/// hide a working feature behind an unrelated capability, which is §9's level
/// bug in a second place.
#[test]
fn an_a4_can_be_browsed_even_though_it_has_no_pattern_dumps() {
    let a4 = wired(&A4, "A4");
    assert!(!a4.can_sysex(), "the premise: the A4 has no dump protocol");
    assert_eq!(blocker(&a4), None, "and its +Drive is still readable");
}

#[test]
fn each_missing_port_is_named_rather_than_lumped_together() {
    let mut device = wired(&DN2, "DN2");
    device.io.input = None;
    assert!(blocker(&device).unwrap().contains("in port"));

    let mut device = wired(&DN2, "DN2");
    device.io.output = None;
    assert!(blocker(&device).unwrap().contains("out port"));

    let mut device = wired(&DN2, "DN2");
    device.io = DeviceIo::default();
    assert!(blocker(&device).unwrap().contains("in and an out"));
}

// --- the index is keyed by the box that answered ----------------------------------

/// Decision 4, and the reason this refusal is stricter than a fetch's: the
/// index is one file per (model key, bank) on disk, so a DT2 answering a DN2's
/// ports would not merely browse wrongly once — it would leave the DT2's
/// presets under `digitone2-…` for every session after this one.
#[test]
fn a_box_that_answers_with_the_wrong_slug_is_refused_before_it_names_a_file() {
    let refusal = mismatched_box(Some("digitone2"), "Digitone II", &identity("digitakt2", "Digitakt II"))
        .expect("a DT2 on a DN2's ports must be refused");
    assert!(refusal.contains("Digitakt II"), "the box that spoke is named: {refusal}");
    assert!(refusal.contains("disk"), "and why a read is refused at all: {refusal}");

    assert_eq!(mismatched_box(Some("digitone2"), "Digitone II", &identity("digitone2", "Digitone II")), None);
}

/// A model with no slug has no filename to key an index by, so it is refused
/// rather than being written somewhere invented.
#[test]
fn a_model_with_no_slug_has_no_index_to_write() {
    assert!(mismatched_box(None, "Some Box", &identity("digitone2", "Digitone II")).is_some());
}

// --- reading a listing -------------------------------------------------------------

#[test]
fn the_bank_picker_comes_from_what_the_box_listed() {
    let entries = vec![bank_dir("A", 256), bank_dir("B", 256), preset(1, "STRAY", 319)];
    assert_eq!(
        bank_paths(&entries),
        vec![path("A"), path("B")],
        "directories only — a file under /soundbanks is not a bank"
    );
    assert_eq!(bank_label("/soundbanks/A"), "A");
}

/// The default banks are a guess for the offline case and are labelled as one
/// in the code. They still have to be the right *shape*, because they are what
/// an index-only open draws its picker from.
#[test]
fn the_offline_bank_guess_is_the_eight_both_digis_have() {
    assert_eq!(DEFAULT_BANKS.len(), 8, "§9 counted eight on both boxes");
    assert_eq!(DEFAULT_BANKS[0], "A");
}

/// The rows on screen and the slots a scan will read have to be the same set.
/// `preset_scan::occupied_slots` applies these three filters, so a browser that
/// applied fewer would show a row that is permanently untagged for no visible
/// reason.
#[test]
fn a_listing_offers_exactly_the_slots_a_scan_would_read() {
    let mut empty = preset(3, "", 0);
    empty.occupancy = Some((0, 1));
    let entries = vec![
        preset(1, "HIDDEN TEARS", 319),
        empty,
        bank_dir("nested", 4),
        preset(6, "7THPAD", 359),
    ];

    let rows = listing_rows(&path("A"), &entries);
    assert_eq!(rows.iter().map(|r| r.slot).collect::<Vec<_>>(), vec![1, 6]);
    assert_eq!(rows[1].size, 359);
    assert_eq!(rows[0].tags, None, "a listing carries no tags — that is the whole of §10.3");
    assert_eq!(rows[0].bank, path("A"), "a row carries its own address");
}

/// A view of one bank the box does not have is empty, not a phantom: the picker
/// can be holding a bank guessed off `DEFAULT_BANKS` before the box was asked.
#[test]
fn a_view_of_a_bank_this_box_does_not_have_is_empty() {
    let lib = library(&["A", "B"]);
    assert_eq!(View::One(path("A")).banks(&lib.banks), vec![path("A")]);
    assert!(View::One(path("Z")).banks(&lib.banks).is_empty());
    assert_eq!(View::All.banks(&lib.banks).len(), 2);
    assert_eq!(View::All.label(), "ALL");
    assert_eq!(View::One(path("C")).label(), "C");
}

// --- the library is the unit ---------------------------------------------------------

/// **Decision 1, and the gap three boxes on a desk found.** The question a user
/// has is "where is there a bass patch", not "what is in bank C", so the search
/// box and the tag chips have to work across every bank at once — and a row has
/// to say which bank it came from, or the answer cannot be acted on.
#[test]
fn a_search_crosses_every_bank_and_each_hit_says_where_it_lives() {
    let mut lib = library(&["A", "B", "C"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"), &[preset(1, "ACID BASS", 319)])), None);
    put(&mut lib, "B", Some(listing_rows(&path("B"), &[preset(4, "PAD SOFT", 319)])), None);
    put(&mut lib, "C", Some(listing_rows(&path("C"), &[preset(9, "SUB BASS", 359)])), None);

    let hits = lib.filtered(&all(&lib), 0, "bass");
    assert_eq!(hits.rows.len(), 2, "two banks answer one search");
    assert_eq!(hits.total, 3);
    assert_eq!(hits.rows[0].bank, path("A"));
    assert_eq!(hits.rows[1].bank, path("C"));

    // And one bank at a time still works, because a targeted rebuild is the
    // reason `PresetIndex` keys by bank at all.
    let just_c = View::One(path("C")).banks(&lib.banks);
    assert_eq!(lib.filtered(&just_c, 0, "bass").rows.len(), 1);
}

/// Tag chips count the whole library, not the bank in front of you — otherwise
/// "Bass 2" would mean something different depending on the picker.
#[test]
fn the_tag_grid_counts_across_every_bank_in_view() {
    let mut a = BankIndex::new("digitone2", &path("A"), "0050", 1);
    a.insert(1, entry("ACID BASS", 1 << 10, 319));
    let mut b = BankIndex::new("digitone2", &path("B"), "0050", 2);
    b.insert(1, entry("SUB BASS", 1 << 10, 319));
    b.insert(2, entry("SOFT PAD", 1 << 12, 319));

    let mut lib = library(&["A", "B"]);
    put(&mut lib, "A", None, Some(a));
    put(&mut lib, "B", None, Some(b));

    assert_eq!(lib.tag_cells(&all(&lib)), vec![(10, "Bass", 2), (12, "Pad", 1)]);
    assert_eq!(lib.filtered(&all(&lib), 1 << 10, "").rows.len(), 2);
    // The same bits, named for a row's tooltip and for the filter caption.
    assert_eq!(tag_names((1 << 10) | (1 << 12), "digitone2"), vec!["Bass", "Pad"]);
}

/// The grid names bits through **the selected box's** table, and the same mask
/// on an A4 is a different set of tags.
///
/// This is the panel-level guard on the correction of 2026-08-29. Before it,
/// one global table named every box's bits, so an A4 library rendered a grid
/// that was complete, well-formed and mostly wrong. The assertion to keep is the
/// pair: the same two bits, two boxes, two answers, neither of them empty.
#[test]
fn the_tag_grid_names_bits_through_the_box_that_is_selected() {
    let mut idx = BankIndex::new("analogfour", &path("A"), "0195", 1);
    idx.insert(1, entry("101 BASS", 1 << 10, 366));
    idx.insert(2, entry("SINGLE CHORD", 1 << 12, 366));

    let mut a4 = Library { slug: "analogfour".into(), ..library(&["A"]) };
    put(&mut a4, "A", None, Some(idx.clone()));

    // Bits 10 and 12 are Kick and Pad on a digi; on an A4 they are Kick and
    // Hi-Hat — one coincidence and one difference, from one pair of bits.
    assert_eq!(a4.tag_cells(&all(&a4)), vec![(10, "Kick", 1), (12, "Hi-Hat", 1)]);
    assert_eq!(tag_names(1 << 12, "analogfour"), vec!["Hi-Hat"]);
    assert_eq!(tag_names(1 << 12, "digitone2"), vec!["Pad"]);

    // And a box with no calibrated grid names nothing rather than borrowing a
    // digi's names — an empty grid, not a confident one.
    let mut unknown = Library { slug: "digitakt".into(), ..library(&["A"]) };
    put(&mut unknown, "A", None, Some(idx));
    assert!(unknown.tag_cells(&all(&unknown)).is_empty());
    assert!(tag_names(1 << 12, "digitakt").is_empty());
}

/// **The trap the library view introduces, and the reason `Tagging::Partial`
/// carries `unread_banks`.** One scanned bank beside seven untouched ones would
/// otherwise report `Complete` — every preset it knows about is tagged — and
/// take the READ TAGS button away with it, which is precisely the state that most
/// needs it.
#[test]
fn one_scanned_bank_does_not_make_an_unread_library_complete() {
    let mut a = BankIndex::new("digitone2", &path("A"), "0050", 2);
    a.insert(1, entry("ONE", 1, 319));
    a.insert(2, entry("TWO", 1, 319));

    let mut lib = library(&["A", "B", "C"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"),
        &[preset(1, "ONE", 319), preset(2, "TWO", 319)])), Some(a));

    let tagging = lib.tagging(&all(&lib));
    assert_eq!(tagging, Tagging::Partial { have: 2, want: 2, unread_banks: 2 });
    assert!(tagging.offers_scan(), "seven unread banks must not hide the scan button");
    assert!(tagging.caption().contains("unread"), "{}", tagging.caption());

    // Looking at bank A alone, it *is* complete — the same data, a different
    // question.
    let just_a = View::One(path("A")).banks(&lib.banks);
    assert_eq!(lib.tagging(&just_a), Tagging::Complete { count: 2 });
    assert!(!lib.tagging(&just_a).offers_scan());
}

// --- browsing never blocks on tagging ----------------------------------------------

/// §10.3's rule, as a property: names and slots work immediately and tags fill
/// in behind.
#[test]
fn a_bank_with_no_index_still_lists_every_preset() {
    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"),
        &[preset(1, "HIDDEN TEARS", 319), preset(2, "MONOLOW", 319)])), None);

    assert_eq!(lib.filtered(&all(&lib), 0, "").rows.len(), 2);
    assert_eq!(lib.tagging(&all(&lib)), Tagging::NotScanned);
    assert!(lib.tagging(&all(&lib)).offers_scan());
    assert!(lib.tagging(&all(&lib)).shows_grid(), "a digi's grid is empty, not absent");
}

/// The other direction, and the thing that makes a second open instant: with no
/// listing at all — the box switched off — last session's index is the rows.
#[test]
fn an_index_off_disk_browses_with_no_box_attached() {
    let mut index = BankIndex::new("digitone2", &path("A"), "0050", 2);
    index.insert(1, entry("HIDDEN TEARS", 0x0488_0804, 319));
    index.insert(2, entry("MONOLOW", 0x05a0_0400, 319));

    let mut lib = library(&["A"]);
    put(&mut lib, "A", None, Some(index));

    let filtered = lib.filtered(&all(&lib), 0, "");
    assert_eq!(filtered.rows.len(), 2, "the browser works with the box unplugged");
    assert_eq!(filtered.rows[0].name, "HIDDEN TEARS");
    assert_eq!(lib.tagging(&all(&lib)), Tagging::Complete { count: 2 });
    assert!(!lib.tagging(&all(&lib)).offers_scan(), "nothing left to read");
}

/// A row's name and its tags have to come out of the same read. `IndexEntry`'s
/// own doc is explicit about why the *struct's* name is the one stored, and a
/// row wearing the listing's name beside the file's tags would be the
/// two-reads-one-row mismatch that doc exists to prevent.
#[test]
fn a_scanned_row_takes_its_name_from_the_file_the_tags_came_out_of() {
    let mut index = BankIndex::new("digitone2", &path("A"), "0050", 1);
    index.insert(1, entry("BLÅ VIND", 0b0100, 359));

    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"), &[preset(1, "BL? VIND", 319)])), Some(index));

    let rows = lib.filtered(&all(&lib), 0, "").rows;
    assert_eq!(rows[0].name, "BLÅ VIND");
    assert_eq!(rows[0].size, 359, "and its measured size, not the listing's allocation");
}

/// A cancelled scan leaves a partial index, and the panel has to say so rather
/// than either claiming completeness or forgetting the work.
#[test]
fn a_half_scanned_bank_reads_as_partial_and_still_offers_the_rest() {
    let mut index = BankIndex::new("digitone2", &path("A"), "0050", 3);
    index.insert(1, entry("HIDDEN TEARS", 0b0001, 319));

    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"), &[
        preset(1, "HIDDEN TEARS", 319),
        preset(2, "MONOLOW", 319),
        preset(6, "7THPAD", 359),
    ])), Some(index));

    let tagging = lib.tagging(&all(&lib));
    assert_eq!(tagging, Tagging::Partial { have: 1, want: 3, unread_banks: 0 });
    assert!(tagging.offers_scan());
    // The header caption states the count and nothing more — the explaining
    // belongs to the TAGS section, which is what the first screenshot of this
    // panel showed by saying nearly the same sentence twice, an inch apart.
    assert_eq!(tagging.caption(), "1 of 3 tagged");
}

// --- the tag filter -----------------------------------------------------------------

/// §10.3: the filter is a bit-mask test and nothing more, and ticking two tags
/// is an OR — `BankIndex::matching` reads that way and so does the box's own
/// browser.
#[test]
fn ticking_two_tags_shows_presets_carrying_either() {
    let mut index = BankIndex::new("digitone2", &path("A"), "0050", 3);
    index.insert(1, entry("KICKY", 0b0001, 319));
    index.insert(2, entry("PADDY", 0b0010, 319));
    index.insert(3, entry("NEITHER", 0b1000, 319));

    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"), &[
        preset(1, "KICKY", 319),
        preset(2, "PADDY", 319),
        preset(3, "NEITHER", 319),
    ])), Some(index));

    let both = lib.filtered(&all(&lib), 0b0011, "");
    assert_eq!(both.rows.iter().map(|r| r.slot).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(both.total, 3, "the caption says 2 of 3, not 2 of 2");
    assert_eq!(lib.filtered(&all(&lib), 0, "").rows.len(), 3, "no filter shows everything");
}

/// **An unscanned preset cannot be tested against a mask** — it has not been
/// read. Hiding it is right; hiding it *silently* is not, because a
/// half-scanned library would then look like a fully-filtered one.
#[test]
fn a_tag_filter_counts_what_it_had_to_hide_for_want_of_a_scan() {
    let mut index = BankIndex::new("digitone2", &path("A"), "0050", 3);
    index.insert(1, entry("KICKY", 0b0001, 319));

    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"), &[
        preset(1, "KICKY", 319),
        preset(2, "UNREAD", 319),
        preset(6, "ALSO UNREAD", 359),
    ])), Some(index));

    let filtered = lib.filtered(&all(&lib), 0b0001, "");
    assert_eq!(filtered.rows.len(), 1);
    assert_eq!(filtered.hidden_untagged, 2, "the panel has to be able to say so");

    // And with no filter up, nothing is hidden — browsing does not depend on
    // tagging.
    assert_eq!(lib.filtered(&all(&lib), 0, "").hidden_untagged, 0);
}

/// A scanned preset with no tags and an unscanned preset are different answers
/// — "looked at, carries none" against "never read" — and a browser that drew
/// them the same would make an unscanned library look like a library of
/// untagged presets.
#[test]
fn no_tags_and_not_yet_scanned_are_not_the_same_row() {
    let mut index = BankIndex::new("digitone2", &path("A"), "0050", 2);
    index.insert(1, entry("PLAIN", 0, 319));

    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"),
        &[preset(1, "PLAIN", 319), preset(2, "UNREAD", 319)])), Some(index));

    let rows = lib.filtered(&all(&lib), 0, "").rows;
    assert_eq!(rows[0].tags, Some(0), "read, and carries nothing");
    assert_eq!(rows[1].tags, None, "never read");
}

// --- the box that cannot be tagged ---------------------------------------------------

/// Decision 4, and the reason `ScanError::BoxNotIndexable` is its own variant:
/// **this is a state, not a failure.** The presets must still list, the grid
/// must go, and nothing may offer a retry — a retry cannot supply what is
/// missing, which is a hardware session calibrating the A4's tag bits against
/// its own display.
#[test]
fn a_box_that_cannot_be_tagged_still_browses_and_is_never_offered_a_retry() {
    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"),
        &[preset(1, "THE SAW", 366), preset(2, "SQUARE WAVE", 366)])), None);
    lib.refused = Some("this box's presets cannot be tagged".into());

    assert_eq!(lib.filtered(&all(&lib), 0, "").rows.len(), 2, "the bank still lists — §10.2");
    let tagging = lib.tagging(&all(&lib));
    assert!(matches!(tagging, Tagging::Unavailable { .. }));
    assert!(!tagging.shows_grid(), "there is nothing to filter by");
    assert!(!tagging.offers_scan(), "a retry here is a button that cannot succeed");

    // And searching by name still works, which is the whole of what an A4 gets.
    assert_eq!(lib.filtered(&all(&lib), 0, "saw").rows.len(), 1);
}

/// The refusal is about the *box*, so picking a different bank must not make it
/// look indexable again — which is why it lives on the library rather than on
/// one bank's data.
#[test]
fn picking_another_bank_does_not_make_an_unindexable_box_indexable() {
    let mut lib = library(&["A", "B"]);
    lib.refused = Some("no calibration".into());
    for view in [View::All, View::One(path("A")), View::One(path("B"))] {
        assert!(!lib.tagging(&view.banks(&lib.banks)).shows_grid(), "{view:?}");
    }
}

// --- what a run says about itself ------------------------------------------------------

/// §10.3's timings are arithmetic and no library has been scanned whole against
/// hardware, so the panel measures rather than predicts. It stays silent about
/// the rate until there is enough of a run to divide by: a projection from one
/// round trip carries no information, and "9 hours left" flashing up on the
/// first tick is worse than nothing.
#[test]
fn the_progress_line_measures_the_run_rather_than_predicting_it() {
    assert_eq!(rate_line(1, 1189, Duration::from_secs(1)), "1 / 1189");
    assert_eq!(rate_line(0, 1189, Duration::ZERO), "0 / 1189");

    // Ten presets in two seconds is five a second; 1,179 left is 236s.
    let line = rate_line(10, 1189, Duration::from_secs(2));
    assert!(line.starts_with("10 / 1189 · 5.0/s · "), "{line}");
    assert!(line.ends_with("3m 56s left"), "minutes, not 236 seconds: {line}");
}

/// A cancelled scan has to read as "stopped, and kept", never as a failure —
/// each finished bank is saved and the next scan resumes from it, which is
/// `scan_bank`'s contract carried across a library.
#[test]
fn a_stopped_scan_reports_kept_work_rather_than_a_loss() {
    let line = report_line(400, 0, true, Duration::from_secs(90));
    assert!(line.contains("400"), "{line}");
    assert!(line.contains("saved"), "{line}");
    assert!(line.contains("1m 30s"), "{line}");

    let line = report_line(1189, 2, false, Duration::from_secs(540));
    assert!(line.starts_with("Tagged 1189 preset(s), 2 skipped"), "{line}");
    assert!(line.contains("9m 00s"), "{line}");
}

/// A bank that has gained presets since its scan is not complete — the same
/// property `BankIndex::is_complete` holds, reached through the listing the
/// panel actually has on screen.
#[test]
fn a_bank_that_grew_since_its_scan_offers_the_new_slot() {
    let mut index = BankIndex::new("digitakt2", &path("A"), "0071", 2);
    index.insert(1, entry("ACIDD", 0x200, 1109));
    index.insert(2, entry("BAM BASS", 0x400, 1109));

    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"), &[
        preset(1, "ACIDD", 1109),
        preset(2, "BAM BASS", 1109),
        preset(3, "BRAND NEW", 1109),
    ])), Some(index));

    assert_eq!(lib.tagging(&all(&lib)), Tagging::Partial { have: 2, want: 3, unread_banks: 0 });
}

// --- the search box -----------------------------------------------------------------

#[test]
fn the_name_search_is_case_insensitive_and_does_not_need_the_index() {
    let mut lib = library(&["A"]);
    put(&mut lib, "A", Some(listing_rows(&path("A"),
        &[preset(1, "HIDDEN TEARS", 319), preset(2, "MONOLOW", 319)])), None);

    assert_eq!(lib.filtered(&all(&lib), 0, "mono").rows.len(), 1);
    assert_eq!(lib.filtered(&all(&lib), 0, "  TEARS ").rows.len(), 1);
    assert_eq!(lib.filtered(&all(&lib), 0, "").rows.len(), 2);
    assert_eq!(lib.filtered(&all(&lib), 0, "nothing").rows.len(), 0);
}

/// Both digis are browsable for the ordinary reason, so the A4 test above is
/// about the A4 rather than about `blocker` being permissive.
#[test]
fn both_digis_are_browsable_when_wired() {
    assert_eq!(blocker(&wired(&DT2, "DT2")), None);
    assert_eq!(blocker(&wired(&DN2, "DN2")), None);
}

// --- the offline open, through the real store ------------------------------------

/// **Decision 2, end to end and with no box anywhere near it.** Everything else
/// in this file builds a `Library` by hand; this one writes indexes to a real
/// directory with `PresetIndex` and then opens a real `PresetsPanel` on them.
/// The claim being checked is the one §10.3 makes and that a header comment
/// cannot keep on its own: a library scanned in a previous session is
/// searchable, with its tags and **across its banks**, before a single MIDI
/// port has been touched.
#[test]
fn a_panel_opens_a_previously_scanned_library_with_the_box_switched_off() {
    use digi_protocol::preset_index::PresetIndex;
    use digi_roll_studio::ui::presets::PresetsPanel;

    let dir = std::env::temp_dir().join(format!(
        "digi-presets-panel-test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let store = PresetIndex::at(&dir);

    let mut a = BankIndex::new("digitone2", &path("A"), "0050", 2);
    a.insert(1, entry("HIDDEN TEARS", 0x0488_0804, 319));
    a.insert(6, entry("7THPAD", 1 << 12, 359));
    store.save(&a).expect("save A");

    let mut c = BankIndex::new("digitone2", &path("C"), "0050", 1);
    c.insert(3, entry("DEEP PAD", 1 << 12, 319));
    store.save(&c).expect("save C");

    let mut panel = PresetsPanel::with_store(store);
    let lib = panel.load_library("digitone2");
    let banks = View::All.banks(&lib.banks);

    // Two banks off disk, in one list, without a port.
    let rows = lib.filtered(&banks, 0, "").rows;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows.iter().map(|r| r.slot).collect::<Vec<_>>(), vec![1, 6, 3]);
    assert_eq!(rows[2].bank, path("C"), "and each says which bank it is in");

    // The tag grid spans them, and filtering crosses the bank boundary — the
    // whole of what 2026-08-29's hardware session asked for.
    assert_eq!(lib.tag_cells(&banks).iter().find(|(_, n, _)| *n == "Pad"), Some(&(12, "Pad", 2)));
    let pads = lib.filtered(&banks, 1 << 12, "");
    assert_eq!(pads.rows.len(), 2);
    assert_eq!(pads.rows[0].bank, path("A"));
    assert_eq!(pads.rows[1].bank, path("C"));

    // Six banks were never scanned, so the library is not complete and READ TAGS
    // stays on offer.
    let tagging = lib.tagging(&banks);
    assert!(matches!(tagging, Tagging::Partial { unread_banks: 6, .. }), "{tagging:?}");
    assert!(tagging.offers_scan());

    let _ = std::fs::remove_dir_all(&dir);
}

// --- loading onto a track, PLAN.md §10.6 step 6 -----------------------------
//
// The two decisions a load makes before it opens a port: whether this box has a
// load path at all, and which track the gesture means. Both are refusals a
// person meets by clicking, so both are tested where clicking is not needed.

/// The A4 browses and does not load, and the two facts are independent.
///
/// This is the distinction the panel exists to state clearly: `blocker` is
/// about *ports* and is a setup step; `load_blocker` is about the box and is
/// permanent. An A4 with both ports set passes the first and fails the second.
#[test]
fn an_a4_browses_and_has_no_load_path() {
    let a4 = wired(&A4, "A4");

    assert_eq!(blocker(&a4), None, "browsing needs ports and it has them");

    let why = load_blocker(&a4).expect("an A4 cannot be loaded onto");
    assert!(why.contains("no dump request"), "it must say which half is missing: {why}");
    assert!(
        why.contains("browse") || why.contains("browses"),
        "and that the rest of the panel still works: {why}"
    );
}

/// The digis have one. Checked so this cannot become a blanket refusal by
/// accident — a `load_blocker` that returned `Some` for everything would make
/// every test above still pass.
#[test]
fn both_digis_have_a_load_path() {
    assert_eq!(load_blocker(&wired(&DT2, "DT2")), None);
    assert_eq!(load_blocker(&wired(&DN2, "DN2")), None);
}

/// A box with no ports still has a load path — it has no *connection*. Two
/// different refusals, and rolling them together would tell somebody their DN2
/// cannot load presets because they had not picked a MIDI port yet.
#[test]
fn a_load_path_is_a_property_of_the_box_not_of_the_cable() {
    let unwired = Device::new("DN2", &DN2, 16);

    assert!(blocker(&unwired).is_some(), "no ports");
    assert_eq!(load_blocker(&unwired), None, "and a load path all the same");
}

#[test]
fn the_selected_track_is_the_one_a_load_lands_on() {
    assert_eq!(load_target(Selection { device: 0, track: 0 }, 0), Ok(0));
    assert_eq!(load_target(Selection { device: 1, track: 11 }, 1), Ok(11));
    assert_eq!(load_target(Selection { device: 0, track: 15 }, 0), Ok(15));
}

/// A kit holds sixteen tracks whatever the roll is showing, and the refusal
/// names the track a person can see rather than the index.
///
/// The failure this prevents is not hypothetical arithmetic: `store_kit_track_sound`
/// takes a `u8`, and a selection of track 16 truncating or wrapping to 0 would
/// store onto the first track of somebody's kit without a word.
#[test]
fn a_track_outside_a_kit_is_refused_by_the_number_on_screen() {
    let err = load_target(Selection { device: 0, track: 16 }, 0).unwrap_err();
    assert!(err.contains("track 17"), "the human number, not the index: {err}");

    assert!(load_target(Selection { device: 0, track: 300 }, 0).is_err());
}

/// The panel follows the roll's selected box and has no picker of its own
/// (decision 6), so this should be unreachable — and it decides where a *write*
/// goes, which is why it is checked rather than assumed.
#[test]
fn a_selection_pointing_at_another_box_is_not_a_load_target() {
    assert!(load_target(Selection { device: 1, track: 3 }, 0).is_err());
}
