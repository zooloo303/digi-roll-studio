// The +Drive preset browser: a box's whole soundbank library, searched and
// filtered by tag.
//
// PLAN.md §10.6 step 5, and the first caller `preset_scan::scan_bank` has ever
// had. Everything below this file already shipped and is tested — `drive_list`
// → `parse_list_entries` for the names, `scan_bank` → `PresetIndex` for the
// tags — so this file is the thread, the channel and the screen, and it holds
// no byte offsets, no opcodes and no decode rules of its own.
//
// **It browses, and since PLAN.md §10.6 step 6 it also loads.** Everything that
// touches the +Drive still goes through `drive::assert_read_only_file_op`,
// which admits List, Open, Read and Close and refuses the write trio and `0x5C`
// Delete — so nothing in here can alter a +Drive. The one thing that *does*
// write is the load, and it writes somewhere else entirely: `0x5b` onto a track
// of the kit the box is playing. Those are two different namespaces and the
// distinction is worth keeping in mind while reading this file — the browser
// cannot change the library, and the loader cannot touch it.
//
// ## Nine decisions
//
//  1. **The library is the unit, not the bank, and it opens on ALL.** The panel
//     browsed one bank at a time until Neil put it on three boxes on
//     2026-08-29, and the gap was immediate: the question a person actually has
//     is *"where is there a bass patch"*, not *"what is in bank C"*. Eight
//     banks behind a picker makes the user the search index. So the bank
//     selector's first entry is ALL, that is the default, and a row carries the
//     bank it came from rather than being defined by it. Per-bank stays,
//     because a targeted rebuild of one bank is the difference between a
//     five-second refresh and a nine-minute one — which is the reason
//     `PresetIndex` keys by bank in the first place, and that storage decision
//     is untouched by this. **The store is per bank; the browser is not.**
//
//  2. **It opens from the index, not from the box.** §10.3 promises that a
//     second open of the panel is instant, and the only way that is true is if
//     the first thing this panel does is read JSON files rather than a MIDI
//     port. So the rows on screen come off disk when there is an index, and the
//     box is asked nothing until somebody presses something. A consequence
//     worth having on purpose: **the browser works with the box switched off**,
//     which is when a good deal of arranging actually gets done.
//
//  3. **The listing and the tags are two different reads, and the panel never
//     makes one wait for the other.** LIST is one round trip per bank and gives
//     names and slots; READ TAGS opens and reads every preset and gives tags.
//     §10.3's rule is that browsing must never block on tagging, so they are two
//     buttons and the row list is drawn from whichever of the two the panel has.
//     Tags are an overlay on rows, not a precondition for them.
//
//  4. **A box that cannot be tagged is a state — but the button that was
//     pressed still has to answer.** `scan_bank` stops at the first preset it
//     cannot decode with `ScanError::BoxNotIndexable`, rather than skipping 128
//     slots to find that out. This renders as [`Tagging::Unavailable`]: the bank
//     still lists, the grid is gone, and there is **no retry**, because a retry
//     cannot supply the one thing missing, which is a hardware session.
//
//     **What the A4 on the desk changed:** the first build expressed all of that
//     by quietly *removing* the READ TAGS button, on the reasoning that a refusal
//     belongs in the tag section as an explanation rather than on a red line
//     under the buttons. Neil pressed it on an A4 and reported that it "flashes
//     and then the button disappears" — which is the panel answering a press by
//     deleting the thing pressed, and reads as a bug rather than as an answer.
//     So the state is still a state, and a [`Note::Warn`] now says so at the
//     point of action as well. A control that vanishes is not a reply.
//
//     **The A4 is no longer the box this describes.** Since 2026-08-29 it lists,
//     scans and tags like a digi; see `midi::preset_scan` and
//     `protocol::drive::decode_drive_preset` for the two mis-framings that had
//     it refused, neither of which was the one first recorded here. The state
//     stays for the next unknown box, and it was worth building for its own
//     sake: it is the difference between a panel that says *this box cannot do
//     that* and one that appears broken.
//
//     What replaced it as the live concern is quieter and is [`Library::slug`]:
//     the tag *names* now follow the box, and a library with no slug names no
//     tags. Reading an A4's mask through a digi's table produces a full,
//     plausible, wholly wrong list rather than an empty one — five of six wrong
//     on `THE SAW` — so the failure this panel now has to avoid is not a missing
//     grid but a confident one.
//
//  5. **The index is keyed by the box that answered, never by the row.** The
//     store is one file per (model key, bank) and it outlives the session, so a
//     mis-cabled desk does not merely import wrong bytes the way
//     `ui::transfer`'s does — it writes a DT2's 148 presets into
//     `digitone2-soundbanks-A.json` and every later session believes them. So
//     the worker identifies first and [`mismatched_box`] refuses when the slug
//     that answers is not the slug the row names. This is `ui::write`'s
//     `wrong_box` rule, kept for a read, because of what the read persists.
//
//  6. **It follows the roll's selected box, and has no picker of its own.** §10
//     asks for "the selected box and track", and the desk already learned on
//     2026-08-28 what two surfaces answering one question costs: a SEND row
//     said A02, provenance said A01, and the write landed on A01. A picker here
//     would be a second answer to "which box" with the track lanes' selection
//     three inches away. A job in flight still names the box it was started
//     for, and [`PresetsPanel::poll`] enforces that rather than intending it.
//
//  7. **Closing the panel does not cancel the scan, and the worker is what
//     saves — per bank, as it finishes each one.** `scan_bank`'s contract is
//     that a cancelled scan still returns its work and a later one resumes from
//     `BankIndex::missing`; the honest way to hold that across eight banks is to
//     write each bank's index the moment that bank is done, so a library scan
//     stopped at bank D keeps A, B and C whole. Nine minutes of reading must not
//     be lost because a panel was collapsed. **Quitting the app mid-scan does
//     lose the unsaved tail** — the thread is detached and the process does not
//     wait for it — which is the one gap in this and is called out on screen.
//
//  8. **The backup is the read a load had to do anyway, and the first one per
//     track wins.** §10.4 asks for "one backup when the kit builder opens", and
//     that cannot be what this panel does: decision 2 is that opening it touches
//     no port at all, and a sixteen-track pre-read on open would spend nine
//     round trips to protect an audition nobody has asked for yet.
//
//     What it does instead gives the same guarantee for nothing. A load reads
//     the target track *before* it writes — it has to, because the box's own
//     reply is the only witness to what length payload it wants — and those
//     bytes are the backup. Keeping the first one per track, and never
//     overwriting it, means REVERT goes back to what the track held before the
//     auditioning started rather than one step back through nineteen of them,
//     which is §10.4's own definition of the honest unit.
//
//     **The backups outlive a change of selected box**, alone among this
//     panel's state: a library view can be rebuilt from disk and these bytes
//     exist nowhere else. They do not outlive the app, and the panel says so.
//
//     None of this replaces the real undo, which is the box discarding an
//     unsaved kit when the pattern is reloaded. `midi::preset_load`'s module
//     doc has the ordering, and the panel says it on screen every time rather
//     than in the `?` reveal.
//
//  9. **A preset the box will not take says so on its own row, and the mark is
//     a recorded fact rather than an inference.** A DN2's library is two
//     formats: 388 of 1,189 presets are Digitone mk1 files, across banks B, C
//     and D. The box **ignores** one sent under `0x5b` — probed 2026-08-29,
//     and it accepted the very next store on the same track, so that is a
//     refusal and not a deaf box.
//
//     Shipped without this, a third of the library refused *after* five round
//     trips with nothing on screen to warn anyone, and the refusal appeared at
//     the top of the panel rather than beside the row that was double-clicked.
//     Both are fixed: [`Row::format`] carries the container magic out of the
//     index, [`foreign_format`] turns it into a dim mark, and a load with a
//     known-foreign format is refused with no port opened at all.
//
//     **The magic is stored, not a verdict.** `IndexEntry::format` keeps
//     `BEEFBACE`/`DN1S`/`BEEFBABA` rather than a `loadable: bool`, for the same
//     reason the tag mask is a mask and not a list of words: a verdict is
//     policy, policy lives in `drive::preset_load_payload`, and every index
//     written before a policy change would otherwise be wrong. And **unknown is
//     not native** — an index from before the field reads as `None`, draws no
//     mark, and is backfilled by the next READ TAGS.
//
// ## What has been verified, and what has not
//
// **On hardware, 2026-08-29:** bank select, LIST and READ TAGS on a DT2 and a DN2
// return names and tags, and the tag filter narrows the list. The A4 lists and
// refuses to be tagged, as designed. **Not yet:** no whole 1,189-preset library
// has been scanned, so §10.3's timings remain arithmetic — which is why
// [`rate_line`] reports presets-per-second and a projection from the run in
// progress rather than from a constant.
//
// **The load runs, on both digis, from this panel** — 2026-08-29, the day it was
// built: a double-click put the selected preset onto the selected track of a DT2
// (0071) and a DN2 (0050). That is the whole path rather than the `0x5b` under
// it, which two boxes had already answered. **The A4's refusal is legible** on
// the same day's testing: double-clicking one of its presets shows the LOAD
// section explaining that the box has no such message, which is decision 4
// holding on the box that taught it.
//
// **The mk1 refusal has met a user** — Neil found it before decision 9 existed,
// which is how decision 9 came to. **The marks have not met a screen**, let
// alone a library: no index on this desk carries `IndexEntry::format` yet, so a
// DN2 shows no marks until the next READ TAGS backfills them, and the attempt
// to photograph them ran into the windowless-relaunch limit. Still desk-only
// besides: REVERT, and the OS-build gate speaking through this path. §9 has the
// list.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use digi_core::device::{Device, PortRef};
use digi_core::{DeviceId, Session};
use digi_midi::preset_load::{load_preset_onto_track, revert_track};
use digi_midi::preset_scan::{scan_bank, ScanError};
use digi_midi::{ElektronDevice, KIT_TRACKS};
use digi_protocol::device::{product_for_slug, DeviceIdentity};
use digi_protocol::drive::{parse_list_entries, ListEntry, A4_CONTAINER_MAGIC};
use digi_protocol::preset_index::{BankIndex, PresetIndex};
use digi_protocol::sound::{tag_names_for, DN1_SOUND_MAGIC_HEAD, SOUND_MAGIC_HEAD};
pub use digi_protocol::sound::tag_names;
use eframe::egui::{self, Ui};

use crate::ui::Note;
use crate::ui::tracks::Selection;
use crate::ui::transfer::binding;

/// The +Drive directory the preset banks live under. One constant rather than
/// three literals, because the bank paths, the index keys and the worker's
/// listing call all have to name the same place.
pub const SOUNDBANKS: &str = "/soundbanks";

/// The banks to offer before a box has been asked — **a guess, and marked as
/// one.** §9 counted eight on both digis, so this is what the panel opens on
/// when it is working from an index with the box switched off. The moment a
/// listing comes back it is replaced by what the box actually said, so a box
/// with a different shape corrects this rather than being mis-drawn by it.
pub const DEFAULT_BANKS: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];

/// One preset as a row on screen, whichever read it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The bank it lives in. Carried on the row rather than implied by the view,
    /// because the browser's default view is the whole library and a name with
    /// no bank beside it is not an address — decision 1.
    pub bank: String,
    pub slot: u32,
    pub name: String,
    /// The struct's size in bytes. Per-preset rather than per-box: one DN2 bank
    /// holds both 319 and 359, which is why `IndexEntry` carries it at all.
    pub size: u32,
    /// The tag mask, or `None` when this slot has not been scanned. `None` and
    /// `Some(0)` are different answers — "not looked at" and "looked at, no
    /// tags" — and a browser that showed them the same would make an unscanned
    /// library look like a library of untagged presets.
    pub tags: Option<u32>,
    /// The container magic, when the index has read this preset. `None` is
    /// *unknown* — a listing knows names and slots and nothing about what is
    /// inside a file — and unknown is drawn as no mark at all rather than as an
    /// assumption either way.
    pub format: Option<u32>,
}

/// What a filter pass produced, and what it had to leave out to produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filtered {
    pub rows: Vec<Row>,
    /// How many rows a tag filter hid **because they have no tags yet**, as
    /// opposed to because their tags did not match. An unscanned preset cannot
    /// be tested against a mask, and silently dropping it would make a
    /// half-scanned library look like a fully-filtered one.
    pub hidden_untagged: usize,
    /// Rows before any filtering, so the panel can say "12 of 148".
    pub total: usize,
}

/// How much of what is in view has tags, and whether it can ever have them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tagging {
    /// Nothing scanned yet. The names are real and the tags are a press away.
    NotScanned,
    /// Some of it is tagged. `unread_banks` is the part of the count that is
    /// **not knowable yet** — a bank nothing has listed or indexed contributes
    /// no presets to `want`, so without this a library with one scanned bank
    /// and seven untouched ones would read as complete.
    Partial { have: u32, want: u32, unread_banks: usize },
    /// Everything in view is tagged. `unread_formats` is how many of those
    /// entries predate `IndexEntry::format` and so cannot say whether their
    /// preset can be loaded.
    ///
    /// **Not folded into `Partial`, because the tags really are complete.** An
    /// index written before 2026-08-29 holds every tag it should; what it lacks
    /// is a second fact about the same files. Reporting that as "801 of 1,189
    /// tagged" would be a lie about the thing this index is named for — so it
    /// says so in its own words, and keeps READ TAGS on the screen so the gap
    /// can be closed.
    Complete { count: u32, unread_formats: usize },
    /// **This box's presets cannot be decoded at all** — the A4. Not a failure
    /// of this bank or this cable, so there is no retry offered for it. See
    /// decision 4.
    Unavailable { why: String },
}

impl Tagging {
    /// Whether the tag grid should be drawn. False for the one box that can
    /// never fill it in.
    pub fn shows_grid(&self) -> bool {
        !matches!(self, Self::Unavailable { .. })
    }

    /// Whether offering a scan makes sense. False once everything in view is
    /// tagged, and false for a box that cannot be indexed — the two reasons not
    /// to press it are different and neither one is a failure.
    pub fn offers_scan(&self) -> bool {
        match self {
            Self::NotScanned | Self::Partial { .. } => true,
            // A complete tag index that cannot say which presets are loadable
            // still has work READ TAGS can do — `BankIndex::missing` counts
            // those entries as missing, so the scan backfills exactly them and
            // is a no-op afterwards.
            Self::Complete { unread_formats, .. } => *unread_formats > 0,
            Self::Unavailable { .. } => false,
        }
    }

    /// The BANK header's right-hand caption.
    ///
    /// **Short on purpose, and it was not on the first draft.** A section
    /// caption sits on the header row and competes with the rule beside it, so
    /// a whole sentence there both squeezes the rule out and — as the first
    /// screenshot of this panel showed plainly — says almost exactly what the
    /// TAGS section an inch below already says at length. The header states the
    /// count; the explaining is done once, where the tags are.
    pub fn caption(&self) -> String {
        match self {
            Self::NotScanned => "not scanned".into(),
            Self::Partial { have, want, unread_banks: 0 } => format!("{have} of {want} tagged"),
            Self::Partial { have, unread_banks, .. } => {
                format!("{have} tagged · {unread_banks} bank(s) unread")
            }
            Self::Complete { count, unread_formats: 0 } => format!("{count} tagged"),
            Self::Complete { count, unread_formats } => {
                format!("{count} tagged · {unread_formats} unread")
            }
            Self::Unavailable { .. } => "cannot be tagged".into(),
        }
    }
}

/// One bank's two reads.
#[derive(Debug, Default, Clone)]
pub struct BankData {
    /// What the box's listing said, when it has been asked. `None` means the
    /// rows below came off disk.
    pub listing: Option<Vec<Row>>,
    /// Last session's tags, off disk.
    pub index: Option<BankIndex>,
}

impl BankData {
    /// Whether this bank has been read at all, by either route.
    fn known(&self) -> bool {
        self.listing.is_some() || self.index.is_some()
    }

    /// How many presets this bank is believed to hold. The listing when there
    /// is one — it is this session's truth — and the index's own recorded count
    /// otherwise, which is the number the last scan wrote down rather than one
    /// this session invented.
    fn want(&self) -> u32 {
        match (&self.listing, &self.index) {
            (Some(rows), _) => rows.len() as u32,
            (None, Some(index)) => index.occupied,
            (None, None) => 0,
        }
    }
}

/// A box's whole soundbank library, as this panel knows it.
///
/// Split out of the panel so the render decision is a function of data a test
/// can build — the panel is then a drawing of this and holds no state a test
/// would have to reach through a `Ui` to see. Every method takes the banks in
/// view rather than a mode, so ALL and one-bank are the same code path with a
/// different list.
#[derive(Debug, Default)]
pub struct Library {
    /// Every bank there is to pick from, guessed off [`DEFAULT_BANKS`] until the
    /// box says otherwise.
    pub banks: Vec<String>,
    pub data: BTreeMap<String, BankData>,
    /// Set when this box answered `BoxNotIndexable`. **A property of the box,
    /// not of a bank**, which is why it lives here and not in [`BankData`]: a
    /// box does not become indexable because a different bank was picked.
    pub refused: Option<String>,
    /// Which box's library this is, as an identity slug — and therefore which
    /// tag table names its bits.
    ///
    /// **Empty until a box is selected, and empty means no tag is named at
    /// all.** That is the safe default rather than an oversight: a mask read
    /// through the wrong box's table produces a full, plausible, wrong list —
    /// see `sound::TAG_NAMES_A4`, where five of `THE SAW`'s six tags come out
    /// wrong under a digi's names. Empty renders as a mask with no labels,
    /// which is visibly incomplete instead of quietly false.
    pub slug: String,
}

impl Library {
    /// How much of `banks` carries tags.
    pub fn tagging(&self, banks: &[String]) -> Tagging {
        if let Some(why) = &self.refused {
            return Tagging::Unavailable { why: why.clone() };
        }
        let mut have = 0u32;
        let mut want = 0u32;
        let mut unread_banks = 0usize;
        let mut unread_formats = 0usize;
        for bank in banks {
            match self.data.get(bank) {
                Some(data) if data.known() => {
                    have += data.index.as_ref().map(|i| i.entries.len() as u32).unwrap_or(0);
                    unread_formats +=
                        data.index.as_ref().map(|i| i.unread_formats()).unwrap_or(0);
                    want += data.want();
                }
                // Neither listed nor indexed: this bank's size is not merely
                // zero, it is unknown, and the difference is what stops seven
                // untouched banks reading as done.
                _ => unread_banks += 1,
            }
        }
        if have == 0 && unread_banks == banks.len() {
            return Tagging::NotScanned;
        }
        if have == 0 {
            return Tagging::NotScanned;
        }
        if unread_banks == 0 && have >= want {
            return Tagging::Complete { count: have, unread_formats };
        }
        Tagging::Partial { have, want, unread_banks }
    }

    /// The rows to draw across `banks`, filtered by `mask` and a name substring.
    ///
    /// A bank's listing is its spine when there is one and its index is the
    /// fallback, which is what makes an offline open show anything at all. A
    /// row's *name* comes from the index when the index has one: that name and
    /// its tag mask came out of the same read of the same file, and a row
    /// wearing a name from the listing beside tags from the file would be the
    /// two-reads-one-row mismatch `IndexEntry`'s own doc was written to avoid.
    /// They have agreed in every capture taken so far.
    pub fn filtered(&self, banks: &[String], mask: u32, search: &str) -> Filtered {
        let needle = search.trim().to_ascii_uppercase();
        let mut base: Vec<Row> = Vec::new();
        for bank in banks {
            let Some(data) = self.data.get(bank) else { continue };
            match &data.listing {
                Some(rows) => base.extend(rows.iter().map(|row| {
                    let found = data.index.as_ref().and_then(|i| i.entries.get(&row.slot));
                    Row {
                        bank: bank.clone(),
                        slot: row.slot,
                        name: found.map(|e| e.name.clone()).unwrap_or_else(|| row.name.clone()),
                        size: found.map(|e| e.size).unwrap_or(row.size),
                        tags: found.map(|e| e.tag_mask),
                        format: found.and_then(|e| e.format),
                    }
                })),
                None => base.extend(data.index.iter().flat_map(|i| i.entries.iter()).map(
                    |(slot, e)| Row {
                        bank: bank.clone(),
                        slot: *slot,
                        name: e.name.clone(),
                        size: e.size,
                        tags: Some(e.tag_mask),
                        format: e.format,
                    },
                )),
            }
        }

        let total = base.len();
        let mut hidden_untagged = 0;
        let rows = base
            .into_iter()
            .filter(|row| needle.is_empty() || row.name.to_ascii_uppercase().contains(&needle))
            .filter(|row| {
                if mask == 0 {
                    return true;
                }
                match row.tags {
                    Some(tags) => tags & mask != 0,
                    // Not "does not match" — *cannot be asked*. Counted so the
                    // panel can say so instead of quietly showing a short list.
                    None => {
                        hidden_untagged += 1;
                        false
                    }
                }
            })
            .collect();
        Filtered { rows, hidden_untagged, total }
    }

    /// Every tag that at least one indexed preset in `banks` carries, as
    /// `(bit, name, count)`.
    ///
    /// Only the tags actually present, rather than all 32: a grid of 32 cells in
    /// a 330px panel is four rows of noise, and most of them would be dead. The
    /// counts come from the index, so a library half-scanned shows the tags
    /// found so far and grows as the scan lands.
    pub fn tag_cells(&self, banks: &[String]) -> Vec<(usize, &'static str, usize)> {
        let Some(table) = tag_names_for(&self.slug) else {
            return Vec::new();
        };
        (0..32)
            .filter_map(|bit| {
                let count: usize = banks
                    .iter()
                    .filter_map(|b| self.data.get(b))
                    .filter_map(|d| d.index.as_ref())
                    .map(|i| i.entries.values().filter(|e| e.tag_mask & (1u32 << bit) != 0).count())
                    .sum();
                (count > 0).then_some((bit, table[bit], count))
            })
            .collect()
    }
}

// The names of the tags in a mask are `sound::tag_names`, re-exported above.
//
// This panel used to carry its own copy, reading a global `TAG_NAMES` directly
// rather than through `Sound::tags`, because the index stores a mask and never
// keeps a `Sound`. That reason still holds and is now served by the free
// function in `protocol::sound` — which takes a slug, so the copy here would
// have had to grow one too and there is no case for two of them.

/// Why this box's +Drive cannot be read, or `None` if it can.
///
/// **This deliberately does not ask `Device::can_sysex`,** and that is the whole
/// trap §10 was written around. `can_sysex` is false for the A4, because it has
/// no `Spec` and no pattern dumps — and its +Drive was read on 2026-08-28
/// anyway: it lists, opens and reads like the digis do. Gating a browse on the
/// dump protocol would hide a working feature behind an unrelated capability,
/// which is the exact shape of §9's level bug. The +Drive needs ports and
/// nothing else.
pub fn blocker(device: &Device) -> Option<String> {
    match (&device.io.input, &device.io.output) {
        (Some(_), Some(_)) => None,
        (None, None) => Some("No ports set — pick an in and an out for this box above".into()),
        (None, Some(_)) => Some("No in port — the +Drive's answers come back on the input".into()),
        (Some(_), None) => Some("No out port — the request goes out on it".into()),
    }
}

/// The short mark a row wears when its container is one the box's own kit will
/// not take, or `None` for a native preset and for one nothing has read.
///
/// **Probed rather than assumed, 2026-08-29.** A DN2 was sent a real mk1
/// preset's payload under `0x5b` — 364 bytes, exactly the length its own `0x6b`
/// reply carries, so nothing about the size could have refused it — and it
/// **ignored the store**, then accepted the very next one on the same track in
/// the same session. So the box is not deaf and it is not converting: it reads
/// the head magic and declines. `examples/probe_mk1_store` is that probe and is
/// kept, because the question is per-box.
///
/// That is why this is a permanent mark rather than a temporary gap. 388 of a
/// DN2's 1,189 presets are mk1, across banks B, C and D, and until this existed
/// a third of the library refused *after* a round trip with nothing on screen to
/// warn anyone — which is exactly how Neil met it.
///
/// `None` for an unrecorded format is deliberate: an index written before this
/// was stored knows nothing, and drawing nothing is honest where drawing
/// "native" would be a guess. `BankIndex::missing` backfills those on the next
/// READ TAGS.
pub fn foreign_format(format: Option<u32>) -> Option<&'static str> {
    match format? {
        SOUND_MAGIC_HEAD => None,
        DN1_SOUND_MAGIC_HEAD => Some("mk1"),
        A4_CONTAINER_MAGIC => Some("A4"),
        // A magic nobody has mapped. Marked, because "unrecognised" is a
        // stronger reason not to send something than "recognised and foreign".
        _ => Some("other"),
    }
}

/// The sentence a row's format earns when a load would refuse it.
pub fn foreign_format_reason(format: Option<u32>) -> Option<String> {
    Some(match format? {
        SOUND_MAGIC_HEAD => return None,
        DN1_SOUND_MAGIC_HEAD => "This is a Digitone mk1 preset. It browses, searches and tags \
             like any other, and the box will not take one onto a kit track — asked \
             directly on 2026-08-29, it ignores the store. Load it from the box's own \
             browser instead."
            .to_string(),
        A4_CONTAINER_MAGIC => "This is an Analog Four preset, and no message is known that \
             puts one on a track — the A4's 0x6b is not the kit-track read a load is built \
             on, and no store path for a sound has been found."
            .to_string(),
        magic => format!(
            "This preset's container is {magic:#010x}, which this build does not recognise. \
             It is not sent under a store opcode for that reason."
        ),
    })
}

/// Why this box cannot be loaded onto at all, or `None`.
///
/// **Stricter than [`blocker`], and the extra refusal is permanent rather than a
/// setup step.** Browsing needs two ports and nothing else. Loading needs the
/// box to answer `0x6b` with a **kit-track sound** — a load reads the track
/// before it writes it, and there is no other way to know what a track held or
/// what length it wants — and only the gen-2 boxes do.
///
/// The box refused is the Analog Four, and the reason had to be *re*-stated on
/// 2026-08-31: this function keyed on "no dump family" until the A4 turned out
/// to have one and answer requests on it. Its `0x6b` is not this message —
/// it returns the current *pattern* with the index ignored (`0x65`'s twin),
/// not a kit track's sound — and no `0x5b` store path is known. So the
/// discriminator is now the pattern route: the gen-2 dump namespace is where
/// the kit-track read/store pair lives, and `PatternRoute::Request` is the row
/// that speaks it.
///
/// The distinction matters because everything *else* about the A4 here works:
/// it lists, it reads, it decodes, it tags, and its presets sit in this browser
/// next to a DN2's — since 2026-08-31 its *patterns* fetch and write from the
/// Setup panel too. So the refusal has to say which half is missing, or it
/// reads as the browser being broken on that box.
///
/// **There is nothing to enable here.** No cable, port, OS build or setting
/// changes it: the box does not have the message. That is why this is a
/// sentence and not a disabled control with a tooltip.
pub fn load_blocker(device: &Device) -> Option<String> {
    if device.model.slug.is_none() || product_for_slug(device.model.slug.expect("checked")).is_none()
    {
        return Some(format!(
            "This build has no protocol for the {} beyond the names in this list, so it \
             cannot put a preset on one of its tracks.",
            device.model.display
        ));
    }
    match device.model.pattern_route() {
        digi_core::device::PatternRoute::Request => None,
        _ => Some(format!(
            "The {} answers no dump request for a kit track's sound — its 0x6b is a \
             different message — so there is no way to read a track back or store a sound \
             onto one. Its presets browse, search and tag here like any other box's, and \
             its patterns fetch and write from Setup; loading a preset onto a track is the \
             one thing this protocol does not give it, and no cable or OS build changes \
             that.",
            device.model.display
        )),
    }
}

/// The track a load would land on, given the roll's selection and the box in
/// view — or why there is not one.
///
/// **A function so the rule is testable, because it is the one place two
/// different numbering schemes meet.** The roll counts a device's tracks from
/// zero and a box's kit holds [`KIT_TRACKS`] of them; a session may hold a model
/// with fewer, and a selection may still be pointing at a track index the roll
/// allows and a kit does not. The panel must not silently store onto track 1
/// because track 17 was selected.
pub fn load_target(selection: Selection, device_index: usize) -> Result<u8, String> {
    if selection.device != device_index {
        // Not reachable while the panel follows the selection's own box, and
        // checked anyway: this decides where a *write* goes.
        return Err("the selected box is not the one this browser is showing".into());
    }
    match u8::try_from(selection.track) {
        Ok(track) if track < KIT_TRACKS => Ok(track),
        _ => Err(format!(
            "track {} is outside the {KIT_TRACKS} a kit holds, so there is no kit slot to \
             load onto",
            selection.track + 1
        )),
    }
}

/// Why the box that answered must not be indexed as the box this row names, or
/// `None`.
///
/// See decision 4: the refusal is about what the index *persists*. A slug is the
/// index's filename, so a DT2 answering a DN2's ports would not merely be
/// browsed wrongly once — it would leave 148 presets on disk under
/// `digitone2-…` for every session after this one.
pub fn mismatched_box(
    expected: Option<&str>,
    display: &str,
    identity: &DeviceIdentity,
) -> Option<String> {
    match expected {
        Some(slug) if slug == identity.slug => None,
        Some(_) => Some(format!(
            "this row is the {display} and the box on those ports says it's a {} — not reading \
             its +Drive, because the tag index is kept on disk under the box's own name and \
             would outlive the mistake",
            identity.name
        )),
        None => Some(format!("{display} is not a box this build knows how to name an index for")),
    }
}

/// The bank paths a `/soundbanks` listing offered, in the order the box gave
/// them.
///
/// Directories only: `/soundbanks` answers in the short form, and a short-form
/// entry is a bank while anything with a slot index would be a stray file.
pub fn bank_paths(entries: &[ListEntry]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.is_dir && !e.name.is_empty())
        .map(|e| format!("{SOUNDBANKS}/{}", e.name))
        .collect()
}

/// The occupied presets in a bank listing, as rows.
///
/// The same three filters `preset_scan`'s `occupied_slots` applies, because the
/// rows on screen and the slots a scan will read have to be the same set — a
/// browser listing a preset the scan then never reaches would show a row that
/// is permanently untagged for no visible reason.
///
/// Takes the bank so the row carries its own address: in the ALL view a name
/// without a bank beside it cannot be found again.
pub fn listing_rows(bank: &str, entries: &[ListEntry]) -> Vec<Row> {
    entries
        .iter()
        .filter(|e| e.is_occupied() && e.children.is_none() && e.size.is_some_and(|s| s > 0))
        .filter_map(|e| {
            Some(Row {
                bank: bank.to_string(),
                slot: e.index?,
                name: e.name.clone(),
                size: e.size.unwrap_or(0),
                tags: None,
                // A listing has not opened the file, so it cannot know.
                format: None,
            })
        })
        .collect()
}

/// The last segment of a bank path, for a picker that has 330px to work in.
pub fn bank_label(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The progress line under a running scan: how far in, how fast, how much left.
///
/// **The rate is measured, not assumed, and that is the point of it.** §10.3's
/// nine-minute figure is one round trip's arithmetic multiplied by 1,189, and
/// no bank has ever been scanned against hardware — so the first real run of
/// this panel is the measurement, and it should be readable off the screen while
/// it happens rather than reconstructed from a stopwatch afterwards.
///
/// Silent about the rate until five presets are in: a projection from one round
/// trip is a number with no information in it, and "9 hours remaining" flashing
/// up on the first tick is worse than nothing.
pub fn rate_line(done: u32, total: u32, elapsed: Duration) -> String {
    let head = format!("{done} / {total}");
    let seconds = elapsed.as_secs_f32();
    if done < 5 || seconds <= 0.0 {
        return head;
    }
    let rate = done as f32 / seconds;
    if rate <= 0.0 {
        return head;
    }
    let left = ((total.saturating_sub(done)) as f32 / rate).round() as u64;
    format!("{head} · {rate:.1}/s · {} left", duration_words(left))
}

/// A whole number of seconds as something readable at a glance. Minutes once it
/// is past a minute, because "413s left" is a number a person has to divide.
fn duration_words(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        _ => {
            let (m, s) = (seconds / 60, seconds % 60);
            match m {
                0..=59 => format!("{m}m {s:02}s"),
                _ => format!("{}h {:02}m", m / 60, m % 60),
            }
        }
    }
}

/// The one-line verdict a finished scan leaves on screen.
///
/// Takes the run's own totals rather than a `ScanReport`, because a library scan
/// is up to eight of those and the number a person wants is what the *run* did.
pub fn report_line(indexed: u32, skipped: u32, cancelled: bool, elapsed: Duration) -> String {
    let took = duration_words(elapsed.as_secs());
    let skipped = match skipped {
        0 => String::new(),
        n => format!(", {n} skipped"),
    };
    if cancelled {
        format!(
            "Stopped at {indexed} preset(s){skipped} after {took} — every bank that \
             finished is saved, and READ TAGS picks up from here"
        )
    } else {
        format!("Tagged {indexed} preset(s){skipped} in {took}")
    }
}

// --- the worker -------------------------------------------------------------------

/// What the worker says. Every error is a `String` by the time it crosses, for
/// `ui::transfer`'s reason: five error types from four crates arrive at one
/// label, and what matters is that each carries the box's or the protocol's own
/// words.
enum Event {
    /// The listing landed, for every bank that was asked for. Carries the
    /// identity that answered, because the index is keyed off it — decision 5 —
    /// and the full bank list, because a bank set guessed off [`DEFAULT_BANKS`]
    /// before the box was consulted is exactly that, and this reply is where the
    /// box corrects it.
    Listed {
        model_key: String,
        build: String,
        banks: Vec<String>,
        listings: Vec<(String, Vec<Row>)>,
    },
    /// One preset indexed. `done`/`total` are **within the current bank**, with
    /// `bank_n`/`banks` saying where that bank sits in the run — rather than one
    /// library-wide count, which cost eight List round trips to compute and is
    /// what the 2026-08-29 DN2 failure came out of. See [`scan_worker`].
    Progress {
        done: u32,
        total: u32,
        bank: String,
        bank_n: usize,
        banks: usize,
        name: Option<String>,
    },
    /// One bank finished and was written to disk. Sent per bank rather than once
    /// at the end, so a library scan stopped at bank D leaves A, B and C on
    /// screen as well as on disk — decision 7.
    BankDone { bank: String, index: Box<BankIndex>, saved: Result<(), String> },
    /// The whole run ended, completely or by cancel. `first_skip` is why the
    /// first passed-over preset was passed over — the thing a bare count of
    /// skips could not say.
    Finished {
        indexed: u32,
        skipped: u32,
        cancelled: bool,
        save_error: Option<String>,
        first_skip: Option<String>,
    },
    /// This box cannot be tagged at all. Its own event, not a `Failed`, because
    /// the panel renders it as a state rather than as a fault — decision 4.
    NotIndexable(String),
    /// One preset is on one track, verified by reading it back.
    ///
    /// `backup` is what that track held **before** this load, and the panel
    /// keeps only the first one it is given per track — audition mode's whole
    /// mechanism, and decision 8 is why it arrives here rather than being
    /// fetched separately.
    Loaded { track: u8, loaded: String, replaced: String, backup: Vec<u8> },
    /// Every track this panel had touched is back to what it found there.
    /// `failed` names the ones that did not take, which are the ones a person
    /// has to reload the pattern for.
    Reverted { restored: Vec<u8>, failed: Vec<String> },
    Failed(String),
}

/// What the worker in flight is doing.
///
/// **A `bool` until step 6, and it stopped being one for a reason worth naming:**
/// a scan is the only job that can be stopped, and a load is the only job that
/// writes. Those are different questions, and a second bool beside the first is
/// how a panel ends up offering STOP on a store — which is the one operation in
/// here that must run to its read-back.
#[derive(Debug, Clone, PartialEq, Eq)]
enum JobKind {
    Listing,
    Scanning,
    /// A `0x5b` onto this track, with the preset's address for the progress
    /// line. Named rather than numbered: at four seconds a person wants to see
    /// *which* preset is going where.
    Loading { track: u8, what: String },
    /// Putting every touched track back.
    Reverting { tracks: usize },
}

impl JobKind {
    /// Whether STOP applies. Only the scan: a load is five round trips that end
    /// in a verify, and a half-run load is exactly the state nobody can act on.
    fn stoppable(&self) -> bool {
        matches!(self, Self::Scanning)
    }

    /// Whether this job writes to the box. The Setup panel is held off for
    /// either kind — one desk, one person — but only this one needs the
    /// consequence line that says so.
    fn writes(&self) -> bool {
        matches!(self, Self::Loading { .. } | Self::Reverting { .. })
    }

    /// The line to show while it runs and no progress has arrived.
    fn waiting_line(&self) -> String {
        match self {
            Self::Listing => "listing…".into(),
            Self::Scanning => "reading the library…".into(),
            Self::Loading { track, what } => format!("loading {what} onto T{}…", track + 1),
            Self::Reverting { tracks } => format!("putting {tracks} track(s) back…"),
        }
    }
}

/// One job in flight.
struct Job {
    /// The box it was started for. A scan belongs to the box that was selected
    /// when it was pressed, not to whatever the roll is pointing at when it
    /// lands — decision 6 — and [`PresetsPanel::poll`] enforces that rather
    /// than leaving it as an intention.
    device: DeviceId,
    /// That box's name in the session, so a result arriving after the selection
    /// has moved can say whose it was instead of appearing under the wrong one.
    name: String,
    kind: JobKind,
    rx: Receiver<Event>,
    /// Read by `scan_bank` before every preset, so a cancel costs at most one
    /// round trip.
    cancel: Arc<AtomicBool>,
    started: Instant,
    /// The last progress line's parts, for the readout: done, total, bank,
    /// which bank of how many, and the preset just read.
    progress: Option<(u32, u32, String, usize, usize, Option<String>)>,
}

/// Open the ports and identify, refusing a box that is not the one asked for.
///
/// The refusal is here, on the worker, rather than after the reply reaches the
/// panel, so that nothing a mis-cabled desk said ever gets as far as a filename.
fn open(
    input: &PortRef,
    output: &PortRef,
    expected: Option<&str>,
    display: &str,
) -> Result<(ElektronDevice, DeviceIdentity), String> {
    let mut device =
        ElektronDevice::open(&binding(input), &binding(output)).map_err(|e| e.to_string())?;
    let identity = device.identify().map_err(|e| e.to_string())?;
    if let Some(why) = mismatched_box(expected, display, &identity) {
        return Err(why);
    }
    Ok((device, identity))
}

/// List one directory and parse it, naming the path in any failure. Both
/// workers need this and a listing that fails without saying *which* path
/// failed is unhelpful in a run that touches nine of them.
fn list(device: &mut ElektronDevice, path: &str) -> Result<Vec<ListEntry>, String> {
    let reply = device.drive_list(path, 0, 0).map_err(|e| format!("could not list {path}: {e}"))?;
    parse_list_entries(&reply.entry_bytes, reply.count)
        .map_err(|e| format!("{path} did not parse: {e}"))
}

/// The listing half: the bank set, then every bank asked for.
///
/// `wanted` is `None` for the whole library — decision 1's default — and
/// `Some(bank)` for a targeted refresh of one. Nine round trips against one,
/// which is why both are offered rather than only the first.
fn list_worker(
    input: PortRef,
    output: PortRef,
    expected: Option<&'static str>,
    display: String,
    wanted: Option<String>,
    events: Sender<Event>,
) {
    let (mut device, identity) = match open(&input, &output, expected, &display) {
        Ok(pair) => pair,
        Err(why) => {
            let _ = events.send(Event::Failed(why));
            return;
        }
    };

    let banks = match list(&mut device, SOUNDBANKS).map(|e| bank_paths(&e)) {
        Ok(banks) if !banks.is_empty() => banks,
        Ok(_) => {
            let _ = events.send(Event::Failed(format!("{SOUNDBANKS} has no banks in it")));
            return;
        }
        Err(why) => {
            let _ = events.send(Event::Failed(why));
            return;
        }
    };

    // A bank asked for that the box does not have is dropped rather than
    // demanded: the request came from a guess, and the box has just corrected it.
    let todo: Vec<String> = match wanted {
        Some(one) if banks.contains(&one) => vec![one],
        Some(_) | None => banks.clone(),
    };

    let mut listings = Vec::new();
    for bank in todo {
        match list(&mut device, &bank) {
            Ok(entries) => listings.push((bank.clone(), listing_rows(&bank, &entries))),
            Err(why) => {
                let _ = events.send(Event::Failed(why));
                return;
            }
        }
    }

    let _ = events.send(Event::Listed {
        model_key: identity.slug.clone(),
        build: identity.build.clone(),
        banks,
        listings,
    });
}

/// The long half. `scan_bank` **is** the body of this — the loop, the cancel
/// check, the skip-and-continue and the A4 stop all live there and are tested
/// there, so what this adds is a thread, a channel, a save per bank, and the
/// arithmetic that turns eight per-bank counts into one library-wide bar.
#[allow(clippy::too_many_arguments)]
fn scan_worker(
    input: PortRef,
    output: PortRef,
    expected: Option<&'static str>,
    display: String,
    banks: Vec<String>,
    existing: BTreeMap<String, BankIndex>,
    // **Passed in rather than resolved here**, so the panel writes to the same
    // place it reads from. A worker that called `default_index` itself would
    // leave a test's injected directory readable and unwritable — which reads as
    // "the scan found nothing" and is a fault nothing would fail on.
    store: PresetIndex,
    cancel: Arc<AtomicBool>,
    events: Sender<Event>,
) {
    let (mut device, identity) = match open(&input, &output, expected, &display) {
        Ok(pair) => pair,
        Err(why) => {
            let _ = events.send(Event::Failed(why));
            return;
        }
    };

    // **No pre-pass — and the reason recorded here first was wrong.**
    //
    // This loop used to open by asking every bank for its occupied slots, purely
    // so the bar could show one library-wide "412 / 1189": eight extra List
    // round trips before any read. When a DN2 run reported **0 tagged, 388
    // skipped, in 2 seconds**, those eight Lists were the only thing that had
    // changed in the read sequence, and this comment confidently blamed them —
    // a stuck Open/Read/Close session cascading through every later read.
    //
    // **`ScanReport::first_skip` then said what actually happened**, which is
    // why that field exists: `/soundbanks/B/205: no sound container magic in 407
    // bytes`. The read *succeeded* — 407 bytes is exactly the length of a good
    // DN2 preset file — and the **decode** failed. Nothing was stuck and nothing
    // cascaded; those slots hold something this parser does not recognise, and
    // the earlier per-bank scans skipped the very same ones (bank D indexed
    // 1–100, skipped 101–228, then indexed 229–256 — a session that had died
    // would never have come back).
    //
    // The pre-pass stays gone anyway, on its own merits rather than on that
    // story: it bought one progress number for eight round trips, and the bank
    // plus `(3/8)` below reads nearly as well for nothing. **A progress readout
    // is not worth a round trip on the wire it is measuring.** But the diagnosis
    // it was removed under was mistaken, and a comment that leaves a wrong cause
    // standing is worse than no comment.
    let (mut indexed, mut skipped) = (0u32, 0u32);
    let mut save_error = None;
    let mut cancelled = false;
    let mut first_skip = None;
    let banks_total = banks.len();

    for (n, bank) in banks.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }

        let progress = events.clone();
        let label = bank.clone();
        // An index loaded under a different build is still resumed from, not
        // thrown away: `preset_index`'s doc records the build as a fact a caller
        // may act on rather than as a guard, and discarding a nine-minute scan
        // because a box was updated is the caller acting on it badly.
        let scanned = scan_bank(
            &mut device,
            &identity.slug,
            &identity.build,
            &bank,
            existing.get(&bank).cloned(),
            &cancel,
            |p| {
                let _ = progress.send(Event::Progress {
                    done: p.done,
                    total: p.total,
                    bank: label.clone(),
                    bank_n: n + 1,
                    banks: banks_total,
                    name: p.name,
                });
            },
        );

        match scanned {
            Ok((index, report)) => {
                indexed += report.indexed;
                skipped += report.skipped;
                if first_skip.is_none() {
                    first_skip = report.first_skip;
                }
                // A bank with nothing left to do returns an empty report and is
                // not worth a disk write — `missing` was empty, so the file on
                // disk is already what this would save.
                if report.indexed > 0 || report.skipped > 0 {
                    let saved = store.save(&index).map(|_| ()).map_err(|e| e.to_string());
                    if let Err(e) = &saved {
                        save_error = Some(e.clone());
                    }
                    // Written the moment this bank is done, so a library scan
                    // stopped at D keeps A, B and C — decision 7.
                    let _ = events.send(Event::BankDone {
                        bank: bank.clone(),
                        index: Box::new(index),
                        saved,
                    });
                }
                if report.cancelled {
                    cancelled = true;
                    break;
                }
            }
            Err(ScanError::BoxNotIndexable { why }) => {
                let _ = events.send(Event::NotIndexable(why.to_string()));
                return;
            }
            Err(e) => {
                let _ = events.send(Event::Failed(e.to_string()));
                return;
            }
        }
    }
    let _ = events.send(Event::Finished {
        indexed,
        skipped,
        cancelled,
        save_error,
        first_skip,
    });
}

/// The one worker in this file that writes to a box.
///
/// `midi::preset_load` is the body of it, exactly as `scan_bank` is the body of
/// [`scan_worker`] — the pre-read, the length check against the box's own reply,
/// the store, the settle and the two-read verify all live there and are tested
/// there against a fake box. What this adds is a thread, a channel and the
/// identity check.
///
/// **The identity check is not inherited from the read path's reasoning, it is
/// stronger here.** [`mismatched_box`] refuses a read because of what the read
/// *persists* — a tag index on disk under the wrong box's name. This refuses a
/// write because of what the write *does*: a store aimed at a DN2 that reaches
/// a DT2 puts a Digitone sound in a Digitakt's kit, and there is no `0x50` to
/// put a working buffer back.
fn load_worker(
    input: PortRef,
    output: PortRef,
    expected: Option<&'static str>,
    display: String,
    path: String,
    track: u8,
    events: Sender<Event>,
) {
    let (mut device, _identity) = match open(&input, &output, expected, &display) {
        Ok(pair) => pair,
        Err(why) => {
            let _ = events.send(Event::Failed(why));
            return;
        }
    };

    let _ = match load_preset_onto_track(&mut device, &path, track) {
        Ok(report) => events.send(Event::Loaded {
            track,
            loaded: report.loaded,
            replaced: report.replaced,
            backup: report.backup,
        }),
        Err(why) => events.send(Event::Failed(why.to_string())),
    };
}

/// Put every track this panel has loaded onto back to what it found there.
///
/// **Every track is attempted even after one fails**, which is the opposite of
/// how the read workers handle an error and is deliberate: this runs when
/// something has already gone differently than a person wanted, and stopping at
/// the first failure would leave the tracks after it changed *and* unmentioned.
/// The failures are collected and named instead.
fn revert_worker(
    input: PortRef,
    output: PortRef,
    expected: Option<&'static str>,
    display: String,
    backups: Vec<(u8, Vec<u8>)>,
    events: Sender<Event>,
) {
    let (mut device, _identity) = match open(&input, &output, expected, &display) {
        Ok(pair) => pair,
        Err(why) => {
            let _ = events.send(Event::Failed(why));
            return;
        }
    };

    let (mut restored, mut failed) = (Vec::new(), Vec::new());
    for (track, bytes) in backups {
        match revert_track(&mut device, track, &bytes) {
            Ok(_) => restored.push(track),
            Err(why) => failed.push(format!("T{}: {why}", track + 1)),
        }
    }
    let _ = events.send(Event::Reverted { restored, failed });
}

// --- the panel --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    pub close: bool,
}

/// Which banks the browser is showing.
///
/// `All` is the default, and decision 1 is why: the question is "where is there
/// a bass patch", not "what is in bank C". `One` survives because a targeted
/// rebuild of a single bank is the difference between a five-second refresh and
/// a nine-minute one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    All,
    One(String),
}

impl View {
    /// The banks this view covers, given everything the box has.
    pub fn banks(&self, all: &[String]) -> Vec<String> {
        match self {
            Self::All => all.to_vec(),
            // Filtered against `all` rather than returned bare: the picker can
            // hold a bank guessed off `DEFAULT_BANKS` that this box does not
            // have, and a view of a bank that does not exist should be empty
            // rather than a phantom row source.
            Self::One(bank) => all.iter().filter(|b| *b == bank).cloned().collect(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::All => "ALL".into(),
            Self::One(bank) => bank_label(bank).to_string(),
        }
    }
}

pub struct PresetsPanel {
    reference_visible: bool,
    /// The box whose library is on screen, so a change of selection is something
    /// this panel can notice rather than silently redraw under.
    showing: Option<DeviceId>,
    view: View,
    library: Library,
    /// The tag filter, as a mask — §10.3's "a bit-mask test and nothing more".
    mask: u32,
    search: String,
    job: Option<Job>,
    /// What the last *read* said — LIST, READ TAGS — drawn under BANK beside
    /// the buttons that start them.
    note: Option<Note>,
    /// What the last *write* said — a load, a revert — drawn in the LOAD
    /// section beside the gesture that started it.
    ///
    /// **Two notes rather than one, and the split was earned.** A single note
    /// rendered under BANK put a load's refusal at the top of the panel, an
    /// entire preset list away from the row that was double-clicked and often
    /// off-screen from it. A reply belongs where the action was — the same
    /// lesson decision 4 records for the READ TAGS button on an A4, which is
    /// this panel learning it for the second time.
    load_note: Option<Note>,
    /// The store, held so tests can point it somewhere of their own.
    store: Option<PresetIndex>,
    /// **Audition mode's backup**: what each track held the *first* time this
    /// panel loaded onto it, keyed by box and track — decision 8.
    ///
    /// Kept across a change of selected box, unlike everything else in this
    /// panel, and that is the point of the key. The library on screen is a view
    /// and can be rebuilt from disk; these bytes exist nowhere else, and a
    /// person who auditions on a DN2, clicks a DT2 track and comes back must
    /// still be able to put the DN2 back.
    ///
    /// Never overwritten by a later load onto the same track: recovery is to
    /// the state before the auditioning started, not one step back through
    /// nineteen of them.
    backups: BTreeMap<(DeviceId, u8), Vec<u8>>,
    /// The row LOAD would act on, as `(bank, slot)`. `None` until something is
    /// clicked — a LOAD button with no row picked is a button that cannot say
    /// what it would do.
    picked: Option<(String, u32)>,
}

impl Default for PresetsPanel {
    fn default() -> Self {
        Self {
            reference_visible: false,
            showing: None,
            view: View::All,
            library: Library {
                banks: DEFAULT_BANKS.iter().map(|b| format!("{SOUNDBANKS}/{b}")).collect(),
                ..Library::default()
            },
            mask: 0,
            search: String::new(),
            job: None,
            note: None,
            load_note: None,
            store: None,
            backups: BTreeMap::new(),
            picked: None,
        }
    }
}

impl PresetsPanel {
    /// Point the index at a directory of the caller's choosing. Tests use it;
    /// the app leaves it alone and gets `PresetIndex::default_index`.
    pub fn with_store(store: PresetIndex) -> Self {
        Self { store: Some(store), ..Self::default() }
    }

    /// Whether a read is in flight. The Setup panel's transfer and send buttons
    /// ask, for the reason they ask each other: one desk, one person, and two
    /// connections to one box is a state nothing here is good at.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn store(&self) -> Option<PresetIndex> {
        self.store.clone().or_else(|| PresetIndex::default_index().ok())
    }

    /// Load every bank's tags off disk. **The whole of an offline open** — no
    /// port is touched, and a library scanned in a previous session is browsable
    /// before the box has been asked anything (decision 2), which is the only
    /// reason §10.3's "a second open is instant" is true.
    ///
    /// Public because it is the one path in this panel worth asserting without a
    /// `Ui`: it is a whole feature — searching the library with the box switched
    /// off — reachable in one call, and the alternative is a claim in a header
    /// comment that nothing checks.
    pub fn load_library(&mut self, model_key: &str) -> &Library {
        self.library.slug = model_key.to_string();
        let store = self.store();
        for bank in self.library.banks.clone() {
            let index = store.as_ref().and_then(|s| s.load(model_key, &bank));
            let entry = self.library.data.entry(bank).or_default();
            entry.index = index;
        }
        &self.library
    }

    /// The banks the current view covers.
    fn in_view(&self) -> Vec<String> {
        self.view.banks(&self.library.banks)
    }

    /// Draw the panel.
    ///
    /// `blocked` holds the two read buttons off while the Setup panel is working
    /// — a fetch, a write, a send or a restore. **One desk, one person, and the
    /// hazard here is not merely two spinners:** `safe_write_tracks` is a
    /// re-fetch, a confirm, a backup, a send and a read-back, and a scan holding
    /// a second connection to the same box while that ceremony runs is a way to
    /// make a write fail somewhere in the middle. Every group in Setup already
    /// holds every other one off for this reason; this is the sixth surface
    /// joining that rule rather than a seventh sitting outside it.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: &Session,
        selection: Selection,
        blocked: bool,
    ) -> Outcome {
        let mut out = Outcome::default();
        let selected = selection.device;
        let device = session.devices.get(selected);

        // **The selection is settled before the channel is drained, and the
        // order is load-bearing.** [`Self::poll`] decides whether an answer is
        // still for what is on screen by comparing against `showing`, so
        // draining first would judge this frame's arrivals against last frame's
        // box — and on the one frame the selection moves, that is exactly the
        // frame a stale answer would be applied and then wiped by the reset
        // below.
        if self.showing != device.map(|d| d.id) {
            self.showing = device.map(|d| d.id);
            self.library = Library {
                banks: DEFAULT_BANKS.iter().map(|b| format!("{SOUNDBANKS}/{b}")).collect(),
                ..Library::default()
            };
            self.view = View::All;
            self.mask = 0;
            self.search.clear();
            self.note = None;
            // The picked row and the load's reply both name a preset on the box
            // that is leaving the screen. The *backups* deliberately survive —
            // see decision 8; they are the one thing here that exists nowhere
            // else.
            self.load_note = None;
            self.picked = None;
            if let Some(slug) = device.and_then(|d| d.model.slug) {
                self.load_library(slug);
            }
        }
        self.poll();
        if self.job.is_some() {
            // Nothing wakes the UI thread when a worker speaks, so keep asking
            // while one is out — `ui::transfer`'s bargain, and at scan length it
            // is also what keeps the progress line moving.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        let context = match device {
            Some(d) => format!("{} · {}", d.name, self.view.label()),
            None => String::from("no box"),
        };
        out.close = super::panel_title_bar(ui, "Presets", &context, &mut self.reference_visible);
        if self.reference_visible {
            reference_prose(ui);
        }

        let Some(device) = device else {
            ui.weak("No boxes in this session.");
            return out;
        };

        self.bank_section(ui, device, blocked);
        ui.add_space(6.0);
        self.tag_section(ui);
        ui.add_space(6.0);
        self.rows_section(ui, device, selection, blocked);
        out
    }

    /// The bank picker, the two reads, and whatever the last one said.
    fn bank_section(&mut self, ui: &mut Ui, device: &Device, blocked: bool) {
        let banks = self.in_view();
        let tagging = self.library.tagging(&banks);
        super::section_header(ui, "BANK", Some(&tagging.caption()));

        if let Some(reason) = blocker(device) {
            ui.weak(reason);
            // Not a return: an index off disk is still worth browsing with the
            // box unplugged, which is decision 2's whole point.
        } else if blocked {
            // Said, not left as a dead button. A greyed control with no reason
            // beside it is the thing this codebase keeps deciding not to ship.
            ui.weak("The Setup panel is talking to a box — reading the +Drive waits for it.");
        }

        let in_flight = self.job.is_some();
        let mut picked = self.view.clone();
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("preset-bank")
                .selected_text(
                    egui::RichText::new(self.view.label()).color(super::TEXT_DIMMER),
                )
                .width(56.0)
                .show_ui(ui, |ui| {
                    // ALL first, because it is the default and the common case —
                    // decision 1.
                    ui.selectable_value(&mut picked, View::All, "ALL");
                    for bank in &self.library.banks {
                        ui.selectable_value(
                            &mut picked,
                            View::One(bank.clone()),
                            bank_label(bank),
                        );
                    }
                });

            let ready = !blocked && blocker(device).is_none() && !in_flight;
            ui.add_enabled_ui(ready, |ui| {
                if super::colored_button(
                    ui,
                    "LIST",
                    super::CYAN_FILL,
                    super::CYAN_TEXT,
                    super::CYAN,
                    super::CYAN,
                    super::CYAN_INK,
                )
                .on_hover_text(
                    "Read-only: asks the box which banks it has and what is in the ones \
                     in view. Names and slots only — one round trip per bank. Tags are \
                     inside the files themselves; READ TAGS is what opens them.",
                )
                .clicked()
                {
                    self.start_list(device);
                }
            });

            if tagging.offers_scan() {
                ui.add_enabled_ui(ready, |ui| {
                    if super::colored_button(
                        ui,
                        "READ TAGS",
                        super::CYAN_FILL,
                        super::CYAN_TEXT,
                        super::CYAN,
                        super::CYAN,
                        super::CYAN_INK,
                    )
                    .on_hover_text(
                        "Read-only: opens and reads every preset in view to find its tags. \
                         The whole library is minutes, not seconds — it can be stopped, and \
                         each bank is saved as it finishes.",
                    )
                    .clicked()
                    {
                        self.start_scan(device);
                    }
                });
            }
        });

        if picked != self.view {
            self.view = picked;
            self.note = None;
        }

        self.job_ui(ui);
        if let Some(note) = &self.note {
            ui.colored_label(note.colour(), note.text());
        }
    }

    /// The running job's spinner, progress and STOP.
    fn job_ui(&mut self, ui: &mut Ui) {
        let Some(job) = &self.job else { return };
        let elapsed = job.started.elapsed();
        let kind = job.kind.clone();
        let (line, last) = match &job.progress {
            // "C (3/8) · 142 / 236 · 4.1/s · 23s left" — the bank, where it sits
            // in the run, and the measured rate. The elapsed clock is the whole
            // run's, so the rate is a run rate rather than a per-bank one that
            // restarts eight times.
            Some((done, total, bank, bank_n, banks, name)) => (
                format!(
                    "{} ({bank_n}/{banks}) · {}",
                    bank_label(bank),
                    rate_line(*done, *total, elapsed)
                ),
                name.clone(),
            ),
            None => (kind.waiting_line(), None),
        };
        // Whose job this is, said out loud only when it is not the box on
        // screen — decision 6's other half. A spinner with no owner beside a
        // box that was never asked anything is the confusing case.
        let elsewhere = (Some(job.device) != self.showing).then(|| job.name.clone());

        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(line);
        });
        if let Some(whose) = elsewhere {
            ui.weak(format!("on {whose}, which is not the box selected"));
        }
        if let Some(name) = last {
            ui.weak(name);
        }
        if kind.writes() {
            // No STOP, and said rather than merely absent. A load is a store
            // followed by the read that proves it landed; stopping between the
            // two is the one state this panel could produce that nobody could
            // act on.
            super::consequence_line(
                ui,
                "This one writes to the box, so it runs to its read-back rather than \
                 offering a stop.",
            );
        }
        if kind.stoppable() {
            if ui
                .small_button("STOP")
                .on_hover_text(
                    "Stop after the preset in flight. Every bank already finished is \
                     saved, and READ TAGS resumes from there.",
                )
                .clicked()
            {
                // The flag `scan_bank` reads before each preset. Not a thread
                // kill: the scan has to reach its own exit to return the index
                // it has built, which is what makes a resume possible.
                if let Some(job) = &self.job {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            super::consequence_line(
                ui,
                "Closing this panel does not stop the scan — it keeps reading and saves \
                 each bank as it finishes. Quitting the app does lose the bank in progress.",
            );
        }
    }

    /// The tag grid: one chip per tag any preset in view carries.
    fn tag_section(&mut self, ui: &mut Ui) {
        let banks = self.in_view();
        let tagging = self.library.tagging(&banks);
        if !tagging.shows_grid() {
            // Decision 4: the A4. Said plainly, with no retry, and the library
            // below stays exactly as browsable as it was.
            super::section_header(ui, "TAGS", None);
            super::consequence_line(
                ui,
                "This box's tag names have never been checked against its own display, so \
                 filtering by them would be showing you a guess. The presets still list, \
                 and every one of them can still be browsed and searched by name.",
            );
            return;
        }

        let cells = self.library.tag_cells(&banks);
        let caption = match self.mask {
            0 => String::from("no filter"),
            mask => tag_names(mask, &self.library.slug).join(" · "),
        };
        super::section_header(ui, "TAGS", Some(&caption));

        if cells.is_empty() {
            super::consequence_line(
                ui,
                "No tags yet. They live inside each preset file, not in the bank's listing, \
                 so they only appear once a scan has read them.",
            );
            return;
        }

        ui.horizontal_wrapped(|ui| {
            for (bit, name, count) in cells {
                let bit_mask = 1u32 << bit;
                let mut on = self.mask & bit_mask != 0;
                if ui
                    .toggle_value(&mut on, format!("{name} {count}"))
                    .on_hover_text(format!("{count} preset(s) in view tagged {name}"))
                    .changed()
                {
                    // Any of the ticked tags, not all of them — `BankIndex::matching`
                    // is an OR, and the box's own browser reads the same way.
                    self.mask ^= bit_mask;
                }
            }
            if self.mask != 0 && ui.small_button("clear").clicked() {
                self.mask = 0;
            }
        });

        if let Tagging::Partial { have, want, unread_banks } = tagging {
            let unread = match unread_banks {
                0 => String::new(),
                n => format!(", and {n} bank(s) have not been read at all"),
            };
            super::consequence_line(
                ui,
                &format!(
                    "{have} of {want} read so far{unread}. This grid is what has been \
                     found rather than what is there; READ TAGS picks up where it left off.",
                ),
            );
        }
    }

    /// The presets themselves, and the one gesture in this panel that writes.
    fn rows_section(
        &mut self,
        ui: &mut Ui,
        device: &Device,
        selection: Selection,
        blocked: bool,
    ) {
        let banks = self.in_view();
        let filtered = self.library.filtered(&banks, self.mask, &self.search);
        let caption = format!("{} of {}", filtered.rows.len(), filtered.total);
        super::section_header(ui, "PRESETS", Some(&caption));

        ui.horizontal(|ui| {
            ui.weak("find");
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("name, across every bank in view")
                    .desired_width(ui.available_width()),
            );
        });

        if filtered.hidden_untagged > 0 {
            super::consequence_line(
                ui,
                &format!(
                    "{} preset(s) are hidden because they have not been scanned — a tag \
                     filter cannot ask a preset nothing has read.",
                    filtered.hidden_untagged
                ),
            );
        }

        if filtered.total == 0 {
            ui.weak(match self.view {
                View::All => "Nothing read yet — LIST asks this box for its banks.",
                View::One(_) => "Nothing read yet — LIST asks the box for this bank.",
            });
            return;
        }

        // The bank column earns its place only when more than one is in view.
        let show_bank = matches!(self.view, View::All);
        let slug = self.library.slug.clone();
        let picked = self.picked.clone();
        let target = load_target(selection, selection.device).ok();
        let mut clicked: Option<(String, u32)> = None;
        let mut double_clicked: Option<(String, u32)> = None;

        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for row in &filtered.rows {
                let address = (row.bank.clone(), row.slot);
                let is_picked = picked.as_ref() == Some(&address);
                // **The whole row senses the click, and the labels inside it do
                // not.** `super::install_style` makes labels non-selectable for
                // exactly this reason — `ui::generate`'s destination chip lost
                // half its clicks to a selectable label eating them — so the
                // sense goes on the scope and the parts stay decoration.
                let response = ui
                    .scope_builder(
                        egui::UiBuilder::new()
                            .id_salt(("preset-row", &row.bank, row.slot))
                            .sense(egui::Sense::click()),
                        |ui| {
                            ui.horizontal(|ui| {
                                if show_bank {
                                    ui.label(
                                        egui::RichText::new(bank_label(&row.bank))
                                            .monospace()
                                            .size(10.0)
                                            .color(super::TEXT_DIMMEST),
                                    );
                                }
                                ui.label(
                                    egui::RichText::new(format!("{:>3}", row.slot))
                                        .monospace()
                                        .size(10.0)
                                        .color(super::TEXT_DIMMER),
                                );
                                let foreign = foreign_format(row.format);
                                let name = egui::RichText::new(&row.name).size(11.0).color(
                                    match (is_picked, foreign.is_some()) {
                                        (true, _) => super::CYAN,
                                        // Dimmed rather than struck through or
                                        // hidden: it is a real preset, it can be
                                        // searched for and it can be loaded from
                                        // the box's own browser. What it cannot
                                        // do is come from here.
                                        (false, true) => super::TEXT_DIMMER,
                                        (false, false) => super::TEXT_PRIMARY,
                                    },
                                );
                                ui.label(name);
                                if let Some(mark) = foreign {
                                    ui.label(
                                        egui::RichText::new(mark)
                                            .monospace()
                                            .size(9.0)
                                            .color(super::TEXT_DIMMEST),
                                    );
                                }
                            });
                        },
                    )
                    .response;

                let where_it_is = format!("{}/{}", bank_label(&row.bank), row.slot);
                let tags = match row.tags {
                    Some(0) => String::from("no tags"),
                    Some(mask) => tag_names(mask, &slug).join(", "),
                    None => String::from("not scanned, so its tags are unknown"),
                };
                // The gesture is spelled out on every row rather than once
                // above the list: a double-click that writes to hardware is not
                // a thing to leave anybody guessing at, and the tooltip is
                // where a person checks before trying it.
                // **The gesture line tells the truth per row.** Saying
                // "double-click to load" over a preset that will refuse is the
                // tooltip actively misleading, and on a DN2 that is a third of
                // them.
                let gesture = match (foreign_format(row.format), target) {
                    (Some(mark), _) => format!("{mark} format — cannot be loaded onto a track"),
                    (None, Some(track)) => format!("double-click to load onto T{}", track + 1),
                    (None, None) => String::from("no track selected to load onto"),
                };
                let response = response.on_hover_text(format!(
                    "{where_it_is} · {} bytes · {tags}\n{gesture}",
                    row.size
                ));

                if response.clicked() {
                    clicked = Some(address.clone());
                }
                if response.double_clicked() {
                    double_clicked = Some(address);
                }
            }
        });

        if let Some(address) = clicked {
            // A single click picks, so LOAD has something to name — and so a
            // person can see what a double-click would have hit before making
            // one. The last load's reply goes with it: it named a different
            // preset, and a stale green line under a new pick reads as this
            // one having loaded.
            if self.picked.as_ref() != Some(&address) {
                self.load_note = None;
            }
            self.picked = Some(address);
        }

        self.load_section(ui, device, selection, blocked, &filtered.rows);

        if let Some((bank, slot)) = double_clicked {
            self.start_load(device, selection, &bank, slot);
        }
    }

    /// Where a load would land, what it would displace, and the two buttons.
    ///
    /// Drawn under the list rather than over it because it is about the *track*,
    /// not about the library: everything above this line answers "which preset",
    /// and this answers "onto what, and what happens to what is there".
    fn load_section(
        &mut self,
        ui: &mut Ui,
        device: &Device,
        selection: Selection,
        blocked: bool,
        rows: &[Row],
    ) {
        ui.add_space(6.0);
        let target = load_target(selection, selection.device);
        let caption = match &target {
            Ok(track) => format!("T{}", track + 1),
            Err(_) => String::from("no track"),
        };
        super::section_header(ui, "LOAD", Some(&caption));

        // The permanent refusal first and on its own: it is not a step somebody
        // can complete, so putting it above the buttons stops them reading as
        // "nearly ready".
        if let Some(why) = load_blocker(device) {
            super::consequence_line(ui, &why);
            return;
        }

        let picked_row = self
            .picked
            .as_ref()
            .and_then(|(bank, slot)| {
                rows.iter().find(|r| r.bank == *bank && r.slot == *slot)
            })
            .cloned();

        // Known before a port is opened, because the index read it — so a
        // preset the box will not take is refused here rather than five round
        // trips later. That round trip is what made the refusal feel like a
        // fault the first time anyone met it.
        let foreign = picked_row.as_ref().and_then(|r| foreign_format_reason(r.format));

        let in_flight = self.job.is_some();
        let ready = !blocked
            && blocker(device).is_none()
            && !in_flight
            && target.is_ok()
            && picked_row.is_some()
            && foreign.is_none();

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(ready, |ui| {
                if super::colored_button(
                    ui,
                    "LOAD",
                    super::CYAN_FILL,
                    super::CYAN_TEXT,
                    super::CYAN,
                    super::CYAN,
                    super::CYAN_INK,
                )
                .on_hover_text(
                    "Writes: puts the picked preset onto the selected track of the kit \
                     the box is playing right now. Double-clicking a row does the same \
                     thing. About a kilobyte, and it is read back to prove it landed.",
                )
                .clicked()
                {
                    if let (Some(row), Ok(_)) = (&picked_row, &target) {
                        let (bank, slot) = (row.bank.clone(), row.slot);
                        self.start_load(device, selection, &bank, slot);
                    }
                }
            });

            let touched = self.touched(device.id);
            if !touched.is_empty() {
                ui.add_enabled_ui(!blocked && !in_flight && blocker(device).is_none(), |ui| {
                    if ui
                        .small_button(format!("REVERT {}", touched.len()))
                        .on_hover_text(
                            "Writes: puts every track this panel has loaded onto back to \
                             the sound it held when the first load happened — not one step \
                             back, all the way back.",
                        )
                        .clicked()
                    {
                        self.start_revert(device);
                    }
                });
            }
        });

        // The load surface's own note, drawn where the gesture happened. The
        // read jobs' note lives up under BANK beside the buttons that start
        // them; a load's belongs here, and putting both in one place is what
        // left a refusal at the top of a panel scrolled past its own list.
        if let Some(note) = &self.load_note {
            ui.colored_label(note.colour(), note.text());
        }

        match (&target, &picked_row) {
            _ if foreign.is_some() => {
                super::consequence_line(ui, &foreign.unwrap_or_default());
            }
            (Err(why), _) => super::consequence_line(ui, why),
            (Ok(track), Some(row)) => {
                // **Words, not an arrow.** `→` U+2192 is on `ui::mod`'s
                // known-missing list and it shipped here anyway — drawn once,
                // read off the screen as `ACIDD □ T1`, and fixed. Fourth
                // instance of the same lesson and the first where the table
                // that names the character already existed.
                super::consequence_line(
                    ui,
                    &format!(
                        "{} onto T{} of the kit {} is playing now.",
                        row.name,
                        track + 1,
                        device.name
                    ),
                );
            }
            (Ok(_), None) => {
                super::consequence_line(ui, "Click a preset to pick it, or double-click to load it.")
            }
        }

        // **Said every time, not once in the `?` reveal.** This is the only
        // control in the app that changes a box without a backup this app can
        // restore from, and PLAN.md §10.4's whole argument is that a quiet
        // backup policy is worse than a slow one.
        super::consequence_line(
            ui,
            "A load changes the kit the box is playing, not a stored one. The box's own \
             undo is reloading the pattern, which discards an unsaved kit — so if you like \
             what you hear, save the kit on the box before changing pattern.",
        );
    }

    /// The tracks on `device` this panel has a backup for, in track order.
    fn touched(&self, device: DeviceId) -> Vec<u8> {
        self.backups.keys().filter(|(id, _)| *id == device).map(|(_, t)| *t).collect()
    }

    /// Put the listing job out.
    fn start_list(&mut self, device: &Device) {
        if self.job.is_some() {
            return;
        }
        let (Some(input), Some(output)) = (device.io.input.clone(), device.io.output.clone())
        else {
            return;
        };
        self.note = None;
        let (tx, rx) = channel();
        let (expected, display) = (device.model.slug, device.model.display.to_string());
        let wanted = match &self.view {
            View::All => None,
            View::One(bank) => Some(bank.clone()),
        };
        std::thread::spawn(move || {
            list_worker(input, output, expected, display, wanted, tx);
        });
        self.job = Some(Job {
            device: device.id,
            name: device.name.clone(),
            kind: JobKind::Listing,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            progress: None,
        });
    }

    /// Put the scan out, resuming from whatever each bank's index already holds.
    fn start_scan(&mut self, device: &Device) {
        if self.job.is_some() {
            return;
        }
        let (Some(input), Some(output)) = (device.io.input.clone(), device.io.output.clone())
        else {
            return;
        };
        let Some(store) = self.store() else {
            self.note = Some(Note::Bad(
                "there is nowhere to keep the tag index, so a scan would read for nothing"
                    .into(),
            ));
            return;
        };
        self.note = None;
        let banks = self.in_view();
        let existing: BTreeMap<String, BankIndex> = banks
            .iter()
            .filter_map(|b| {
                self.library.data.get(b).and_then(|d| d.index.clone()).map(|i| (b.clone(), i))
            })
            .collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let (tx, rx) = channel();
        let (expected, display) = (device.model.slug, device.model.display.to_string());
        std::thread::spawn(move || {
            scan_worker(input, output, expected, display, banks, existing, store, flag, tx);
        });
        self.job = Some(Job {
            device: device.id,
            name: device.name.clone(),
            kind: JobKind::Scanning,
            rx,
            cancel,
            started: Instant::now(),
            progress: None,
        });
    }

    /// Put one preset onto the selected track.
    ///
    /// **Everything this decides is decided at the press**, the way
    /// `ui::transfer` captures its destination and [`Self::start_scan`] captures
    /// its box: the track comes off the selection *now*, and the job carries the
    /// box it was started for. A load is seconds rather than minutes, but the
    /// gesture that makes a stale target possible is one click on the roll, and
    /// a stale target here is a store onto the wrong track.
    fn start_load(&mut self, device: &Device, selection: Selection, bank: &str, slot: u32) {
        if self.job.is_some() {
            return;
        }
        if let Some(why) = load_blocker(device) {
            self.load_note = Some(Note::Warn(why));
            return;
        }
        let track = match load_target(selection, selection.device) {
            Ok(track) => track,
            Err(why) => {
                self.load_note = Some(Note::Bad(why));
                return;
            }
        };
        // Costs no round trip: the index already read this file once, and a
        // format the box will not take is a fact rather than an outcome.
        let format = self
            .library
            .filtered(&self.in_view(), 0, "")
            .rows
            .iter()
            .find(|r| r.bank == bank && r.slot == slot)
            .and_then(|r| r.format);
        if foreign_format_reason(format).is_some() {
            // **Picked, and not also noted.** The LOAD section draws this
            // preset's reason as its standing line for as long as the row is
            // picked, so a note here would print the same sentence twice — once
            // in amber and once dim, two inches apart. Picking the row *is* the
            // reply: the section changes to say why this one cannot go.
            self.picked = Some((bank.to_string(), slot));
            self.load_note = None;
            return;
        }
        let (Some(input), Some(output)) = (device.io.input.clone(), device.io.output.clone())
        else {
            self.load_note = Some(Note::Bad(
                blocker(device).unwrap_or_else(|| "this box has no ports set".into()),
            ));
            return;
        };
        self.picked = Some((bank.to_string(), slot));
        self.load_note = None;
        let path = format!("{}/{slot}", bank.trim_end_matches('/'));
        let what = format!("{}/{slot}", bank_label(bank));
        let (tx, rx) = channel();
        let (expected, display) = (device.model.slug, device.model.display.to_string());
        let sent = path.clone();
        std::thread::spawn(move || {
            load_worker(input, output, expected, display, sent, track, tx);
        });
        self.job = Some(Job {
            device: device.id,
            name: device.name.clone(),
            kind: JobKind::Loading { track, what },
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            progress: None,
        });
    }

    /// Put every track this panel has touched on this box back.
    fn start_revert(&mut self, device: &Device) {
        if self.job.is_some() {
            return;
        }
        let backups: Vec<(u8, Vec<u8>)> = self
            .backups
            .iter()
            .filter(|((id, _), _)| *id == device.id)
            .map(|((_, track), bytes)| (*track, bytes.clone()))
            .collect();
        if backups.is_empty() {
            return;
        }
        let (Some(input), Some(output)) = (device.io.input.clone(), device.io.output.clone())
        else {
            return;
        };
        self.note = None;
        let tracks = backups.len();
        let (tx, rx) = channel();
        let (expected, display) = (device.model.slug, device.model.display.to_string());
        std::thread::spawn(move || {
            revert_worker(input, output, expected, display, backups, tx);
        });
        self.job = Some(Job {
            device: device.id,
            name: device.name.clone(),
            kind: JobKind::Reverting { tracks },
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            progress: None,
        });
    }

    /// Take whatever the worker has said.
    ///
    /// **Every arm asks whether the answer is still for what is on screen**, and
    /// that is not a nicety. A scan is minutes long and the roll's selection is
    /// one click away, so "the reply lands on whatever box is selected when it
    /// arrives" is a defect with a very ordinary gesture behind it: pick a DN2,
    /// press READ TAGS, click a DT2 track while you wait, and 1,189 Digitone presets
    /// appear under the Digitakt. `ui::transfer` captures its destination at the
    /// press for the same reason; this captures its box.
    ///
    /// A result for a box that has since been left is **not** thrown away — the
    /// worker has already written each finished bank to disk, so switching back
    /// and letting [`Self::load_library`] read it is what recovers it. What is
    /// dropped is only this panel's copy of a view that is no longer on screen.
    fn poll(&mut self) {
        loop {
            let Some(job) = &mut self.job else { return };
            let Ok(event) = job.rx.try_recv() else { return };
            let mine = Some(job.device) == self.showing;
            let job_device = job.device;
            // Which surface this job's answer belongs to — see `load_note`.
            let writes = job.kind.writes();
            let whose = job.name.clone();
            let elapsed = job.started.elapsed();
            // Prefixed when it belongs to a box that is no longer on screen: a
            // nine-minute scan that ends in silence is worse than one that ends
            // in a line naming whose it was.
            let attribute =
                |text: String| if mine { text } else { format!("{whose}: {text}") };

            match event {
                // The one event that arrives many times, so the job stays open
                // and nothing is cloned beyond the line itself.
                Event::Progress { done, total, bank, bank_n, banks, name } => {
                    job.progress = Some((done, total, bank, bank_n, banks, name));
                }
                Event::BankDone { bank, index, saved } => {
                    if mine {
                        self.library.data.entry(bank).or_default().index = Some(*index);
                    }
                    if let Err(e) = saved {
                        self.note = Some(Note::Bad(attribute(format!(
                            "a bank's index could not be saved ({e}), so it will have to be \
                             read again next time"
                        ))));
                    }
                }
                Event::Listed { model_key, build, banks, listings } => {
                    self.job = None;
                    if !mine {
                        self.note = Some(Note::Good(attribute("listed".into())));
                        return;
                    }
                    self.library.banks = banks;
                    // A view pointing at a bank the box does not have is put
                    // back to ALL rather than left showing nothing: the picker
                    // was a guess and the box has just corrected it.
                    if let View::One(bank) = &self.view {
                        if !self.library.banks.contains(bank) {
                            self.view = View::All;
                        }
                    }
                    let count: usize = listings.iter().map(|(_, r)| r.len()).sum();
                    for (bank, rows) in listings {
                        self.library.data.entry(bank).or_default().listing = Some(rows);
                    }
                    self.load_library(&model_key);
                    self.note = Some(Note::Good(format!(
                        "{model_key} · OS build {build} · {count} preset(s)"
                    )));
                    return;
                }
                Event::Finished { indexed, skipped, cancelled, save_error, first_skip } => {
                    self.job = None;
                    let line = report_line(indexed, skipped, cancelled, elapsed);
                    // **The reason rides with the count.** A bare "388 skipped"
                    // is what made a real DN2 failure unreadable; the box's own
                    // words about the first one are the diagnosis.
                    let line = match &first_skip {
                        Some(why) => format!("{line}\nfirst skip — {why}"),
                        None => line,
                    };
                    self.note = Some(match (save_error, skipped > 0) {
                        (Some(e), _) => Note::Bad(attribute(format!("{line} — but {e}"))),
                        // Everything skipped and nothing tagged is not a
                        // partial success, it is a failed run wearing one.
                        (None, true) if indexed == 0 => Note::Bad(attribute(line)),
                        (None, true) => Note::Warn(attribute(line)),
                        (None, false) => Note::Good(attribute(line)),
                    });
                    return;
                }
                Event::NotIndexable(why) => {
                    self.job = None;
                    // A property of the *box*, not of a bank, so it sticks for
                    // the session and hides the grid everywhere.
                    //
                    // **And it also answers out loud**, which the first build did
                    // not do: it expressed this purely by removing the READ TAGS
                    // button, and on an A4 that reads as a press deleting its own
                    // control. Decision 4 carries the session that found it.
                    if mine {
                        self.library.refused = Some(why);
                    }
                    self.note = Some(Note::Warn(attribute(
                        "this box's presets cannot be tagged — its tag names have never \
                         been checked against its own display, so the browser lists them \
                         by name instead"
                            .into(),
                    )));
                    return;
                }
                // **The backup is kept whether or not the answer is still on
                // screen**, which is the one place this panel does not follow
                // decision 6's "drop what is not mine". A dropped view can be
                // read back off disk; these bytes exist nowhere else, and the
                // box they belong to has already been written to.
                Event::Loaded { track, loaded, replaced, backup } => {
                    self.job = None;
                    self.backups.entry((job_device, track)).or_insert(backup);
                    self.load_note = Some(Note::Good(attribute(format!(
                        "T{} is {loaded} — it was {replaced}",
                        track + 1
                    ))));
                    return;
                }
                Event::Reverted { restored, failed } => {
                    self.job = None;
                    for track in &restored {
                        self.backups.remove(&(job_device, *track));
                    }
                    let put_back = restored.len();
                    self.load_note = Some(if failed.is_empty() {
                        Note::Good(attribute(format!("{put_back} track(s) put back")))
                    } else {
                        // The failures name themselves, because the recovery
                        // for one is walking to the box and reloading the
                        // pattern, and that cannot be asked for vaguely.
                        Note::Bad(attribute(format!(
                            "{put_back} track(s) put back, and {} did not — {}. Reload the \
                             pattern on the box, and do not save it",
                            failed.len(),
                            failed.join("; ")
                        )))
                    });
                    return;
                }
                Event::Failed(e) => {
                    self.job = None;
                    let note = Note::Bad(attribute(e));
                    if writes {
                        self.load_note = Some(note);
                    } else {
                        self.note = Some(note);
                    }
                    return;
                }
            }
        }
    }
}

/// The `?` reveal.
fn reference_prose(ui: &mut Ui) {
    super::consequence_line(
        ui,
        "The bank picker opens on ALL, so the search box and the tag chips work across \
         this box's whole library rather than one bank at a time. Pick a single bank when \
         you want to re-read just that one.",
    );
    super::consequence_line(
        ui,
        "LIST asks the box for names and slots — one round trip per bank. READ TAGS opens \
         and reads every preset to find its tags, because tags live inside each file and \
         not in a bank's listing; on a Digitone II that is 1,189 files and it takes \
         minutes. It can be stopped at any point, each bank is saved as it finishes, and \
         it picks up where it left off — so opening this panel again is instant and works \
         with the box switched off.",
    );
    super::consequence_line(
        ui,
        "Double-click a preset to load it onto the selected track, or click one and press \
         LOAD. It goes onto the kit the box is playing right now — about a kilobyte, and \
         it is read back to prove it landed.",
    );
    super::consequence_line(
        ui,
        "That kit is a working buffer, so the box's own undo is reloading the pattern, \
         which throws an unsaved kit away. REVERT puts every track this panel has loaded \
         onto back to what it held before the first load — while the app is open. Quitting \
         loses that, and the box's undo is what is left.",
    );
    super::consequence_line(
        ui,
        "Nothing here can change the +Drive itself. The library is read with List, Open, \
         Read and Close and nothing else — a load writes to the active kit, which is a \
         different place, and no button in this panel can write to or delete a preset.",
    );
}
