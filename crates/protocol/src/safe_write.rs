//! The safe write flow: the one path every write to a box goes through.
//!
//! Port of `js/elektron/safe-write.js`. PLAN.md §7 rule 2 is the reason this file
//! is shaped the way it is — **no write path exists until all five safety rules
//! do, as one function** — so a new caller physically cannot skip a step. The
//! rules map onto [`safe_write_track`] like this:
//!
//! 1. **auto-backup** — the untouched target payload goes to
//!    [`WriteHooks::on_backup`] before a single byte is sent, and the write
//!    aborts if that hook fails. A copy is put in the [`crate::backup_stash`]
//!    first, so the bytes survive a hook that could not write its file.
//! 2. **minimal diff** — only `encode_track_notes`, `apply_track_trig_settings`,
//!    `apply_track_prob`, `apply_track_plocks` and `apply_swing` touch the
//!    payload, so every byte outside the track's step words, the trig-record
//!    pool, that track's three trig-condition lanes, its one track-PROB byte,
//!    the p-lock lanes belonging to that track and the pattern's one swing byte
//!    round-trips identically.
//! 3. **allowlist** — [`write_gate`] refuses any OS build the format has not
//!    been verified against.
//! 4. **verify** — the pattern is read back and byte-compared, and the caller
//!    gets the mismatching offsets to report loudly.
//! 5. **throwaway** — a human rule. [`WriteHooks::confirm`] is where the UI
//!    spells out exactly what is about to be overwritten, and
//!    [`write_impact_lines`] is the wording it must not leave out.
//!
//! The target payload is always re-fetched here, immediately before encoding.
//! Callers never pass in bytes captured earlier: writing back a stale payload
//! would silently revert everything changed on the box since.
//!
//! ## Four places the JS's answer does not port directly
//!
//! 1. **The backup hook is a trait method, not an optional callback.** The JS
//!    checks `typeof onBackup !== 'function'` at run time and throws, because
//!    that is the only enforcement JS has. In Rust the hook is the one required
//!    method of [`WriteHooks`] — a caller that has not written a backup path
//!    does not compile. The JS suite's "refuses to write at all without a backup
//!    hook" test therefore has no Rust body; what it was protecting is now a
//!    property of the type, and the tests keep the half that is still reachable:
//!    a hook that *fails* aborts before anything is sent.
//! 2. **The device is a trait, not a duck-typed object.** [`PatternIo`] is the
//!    two round trips this function needs and nothing else, so `digi_midi` can
//!    implement it over a real box and a test can implement it over a `Vec`.
//!    This crate stays free of I/O, per PLAN.md §3.
//! 3. **Nothing here is async.** The JS awaits browser MIDI promises; the Rust
//!    transport is a worker thread the app already polls, so a blocking call on
//!    the caller's thread is the honest shape. `safe_write_track` is therefore
//!    something the app runs off the UI thread, not something it can await.
//! 4. **A `Timestamp` is passed in, never read from the clock here.** The JS
//!    defaults `now` to `new Date()`; this crate has no business reading a clock
//!    inside a function whose output is compared byte-for-byte in tests, so
//!    [`Timestamp::now`] exists and the caller calls it.

use crate::backup_stash::{BackupContext, Stash, StashError};
use crate::device::DeviceIdentity;
use crate::a4_plocks::A4LaneWrite;
use crate::pattern::{
    bank_name, decode_pattern_kit, diff_payloads, dn2_spec, dt2_spec, encode_track_notes,
    track_trig_count, ByteDiff, Note, PatternKit, Spec,
};
use crate::pattern_settings::{apply_swing, read_swing};
use crate::plocks::{apply_track_plocks, free_lane_count, read_track_plocks, LaneWrite, PoolLane};
use crate::protocol::{build_dump_message, DUMP_PATTERN_KIT};
use crate::trig_cond::{apply_track_prob, apply_track_trig_settings, trig_settings_from_notes, TrigSetting};

/// OS builds the pattern write path has been verified against on real hardware,
/// per device — a full encode → send → re-read → byte-compare cycle plus a
/// controlled-experiment pass over the trig fields. **Extend a list only after
/// re-verifying on the new build.**
///
/// 0071 and 0050 joined the list on 2026-08-21. Both OSes landed on 2026-06-23
/// carrying one sentence of release notes — "adds support for updated production
/// processes" — which is the same sentence 1.15B and 1.10D themselves shipped
/// under: an Elektron production-sourcing change, not a firmware feature. That
/// is the reason to expect nothing moved, not the evidence, so the round trip
/// was run on both boxes on the new OSes the same day: a pattern fetched off the
/// box, edited in the app, written back, and verified byte-identical. The
/// trig-field controlled experiment was not repeated — the format is unchanged,
/// and that pass is what these two inherit from the builds above them.
pub const WRITE_ALLOWED_BUILDS: &[(&str, &[&str])] = &[
    ("digitakt2", &["0070", "0071"]), // 1.15B, verified 2026-08-01; 1.15C, 2026-08-21
    ("digitone2", &["0049", "0050"]), // 1.10D, verified 2026-08-01; 1.10E, 2026-08-21
    // 1.55B. The gen-1 write cycle on this build: a pattern sent and displayed
    // 2026-08-30, four authored trig states shown as predicted 2026-08-31, and
    // the same day a `0x64` fetch of A16 carried exactly the trigs the probe
    // wrote — send, re-read and compare, at trig rather than byte granularity.
    // The full byte-for-byte verify is what `a4_safe_write_tracks` runs on
    // every write, so the first send through it completes this row's evidence.
    ("analogfour", &["0195"]),
];

/// How many mismatching offsets the verify step reports. The JS default is 64;
/// this is larger because the report is the only evidence of a bad write and a
/// desktop app is not paying to render it into a browser console.
pub const VERIFY_DIFF_CAP: usize = 1024;

/// Which pattern struct decodes this box's dumps, or `None` for a box whose
/// format we do not know. The name is the JS's `mod`: not a Rust module, but the
/// same idea — the per-device half of the format.
pub fn decoder_for(slug: &str) -> Option<&'static str> {
    match slug {
        "digitakt2" => Some("dt2"),
        "digitone2" => Some("dn2"),
        // Gen-1: `crate::a4_pattern` is the struct, and there is no `Spec` —
        // callers that need one ask `spec_for` and get `None`, which is how the
        // gen-2 flow refuses this box (see `safe_write_tracks`).
        "analogfour" => Some("a4"),
        _ => None,
    }
}

/// The pattern spec for a slug the gate has already accepted, or `None` for a
/// box whose decoder is not a gen-2 `Spec` — the Analog Four, whose format is
/// `crate::a4_pattern` and whose write flow is [`a4_safe_write_tracks`].
pub fn spec_for(slug: &str) -> Option<Spec> {
    match decoder_for(slug)? {
        "dt2" => Some(dt2_spec()),
        "dn2" => Some(dn2_spec()),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteGateResult {
    pub ok: bool,
    /// Written to be shown to the user verbatim.
    pub reason: String,
    pub spec_kind: Option<&'static str>,
}

/// May we write to this device?
pub fn write_gate(identity: Option<&DeviceIdentity>) -> WriteGateResult {
    let id = match identity {
        Some(i) => i,
        None => {
            return WriteGateResult {
                ok: false,
                reason: "no device connected".into(),
                spec_kind: None,
            };
        }
    };
    let spec_kind = match decoder_for(&id.slug) {
        Some(k) => k,
        None => {
            return WriteGateResult {
                ok: false,
                reason: format!("digi-roll can't decode {} patterns — read-only", id.name),
                spec_kind: None,
            };
        }
    };
    let allowed = WRITE_ALLOWED_BUILDS
        .iter()
        .find(|(slug, _)| *slug == id.slug)
        .map(|(_, builds)| *builds)
        .unwrap_or(&[]);
    if !allowed.contains(&id.build.as_str()) {
        return WriteGateResult {
            ok: false,
            reason: format!("OS build {} isn't write-verified yet — read-only", id.build),
            spec_kind: Some(spec_kind),
        };
    }
    WriteGateResult { ok: true, reason: String::new(), spec_kind: Some(spec_kind) }
}

// --- timestamps ---------------------------------------------------------------

/// A UTC wall-clock instant, to the second.
///
/// Only here to name a backup file. `chrono` would be a dependency this crate
/// does not otherwise need, and the one thing wanted from it — the JS's
/// `toISOString().slice(0, 19)` — is fifteen lines of civil-calendar arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Timestamp {
    /// Now, in UTC. The one clock read in this crate, and it is a caller's to
    /// make — see the module doc's fourth deviation.
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self::from_unix_seconds(secs)
    }

    /// Civil date from a Unix second, by Howard Hinnant's `civil_from_days`. The
    /// era arithmetic is the standard published algorithm; it is here rather than
    /// behind a dependency for the reason on the struct.
    pub fn from_unix_seconds(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Self {
            year: (y + i64::from(m <= 2)) as i32,
            month: m as u32,
            day: d as u32,
            hour: (rem / 3600) as u32,
            minute: (rem % 3600 / 60) as u32,
            second: (rem % 60) as u32,
        }
    }

    /// `2026-08-01T12-34-56` — the JS's ISO string, sliced to the second, with
    /// its colons turned into hyphens so the result is a legal filename.
    pub fn file_stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `2026-08-01T12:34:56Z` — the same instant for reading rather than for a
    /// filename. Sorts correctly as a plain string, which is how the backup
    /// index orders its ring without trusting file modification times.
    pub fn iso(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

// --- backup files -------------------------------------------------------------

/// One pattern as a replayable `.syx` file: exactly the bytes the box sent,
/// wrapped back up as a dump message the box would accept again.
///
/// The fields below `name` are the same facts the name encodes, kept separately
/// on purpose: [`crate::backup_stash`] indexes every backup it stores, and an
/// index built by parsing filenames back apart is one rename away from wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternKitFile {
    pub index: u8,
    pub payload: Vec<u8>,
    pub name: String,
    /// The framed SysEx message — what gets written to disk or sent back.
    pub bytes: Vec<u8>,
    pub slug: String,
    /// The word in the filename: `backup` for pre-write, `pre-restore` for the
    /// snapshot a restore takes, or whatever a caller saving a pattern chose.
    pub kind: String,
    pub at: Timestamp,
}

/// Name and frame one pattern for disk.
///
/// Takes a slug and a family rather than a [`DeviceIdentity`], because a pattern
/// decoded from a `.syx` file never had a handshake and those two fields are all
/// it has. `kind` is the word in the filename, so a pre-write backup and a plain
/// "save this pattern" sit apart on disk.
pub fn pattern_kit_file(
    slug: &str,
    family: u8,
    index: u8,
    payload: &[u8],
    kind: &str,
    now: Timestamp,
) -> PatternKitFile {
    PatternKitFile {
        index,
        payload: payload.to_vec(),
        name: format!("{slug}-{}-{kind}-{}.syx", bank_name(index as usize), now.file_stamp()),
        bytes: build_dump_message(family, pattern_dump_type(family), index, payload),
        slug: slug.to_string(),
        kind: kind.to_string(),
        at: now,
    }
}

/// The dump opcode a pattern travels under, per family.
///
/// One byte, and it has to be looked up rather than assumed: a gen-2 pattern is
/// an `0x50` pattern-kit, and the A4's is an `0x54` — numerically the digis'
/// *project settings*, which is why `(family, dump_type)` is the key everywhere
/// (`a4_pattern::is_a4_pattern` has the collision's whole story). A backup
/// framed with the wrong one would replay as a different message.
pub fn pattern_dump_type(family: u8) -> u8 {
    if family == crate::protocol::FAMILY_ANALOG_FOUR {
        crate::a4_pattern::DUMP_A4_PATTERN
    } else {
        DUMP_PATTERN_KIT
    }
}

/// Wrap an untouched pattern payload back up as a replayable `.syx` message.
pub fn pattern_kit_backup(
    slug: &str,
    family: u8,
    index: u8,
    payload: &[u8],
    now: Timestamp,
) -> PatternKitFile {
    pattern_kit_file(slug, family, index, payload, "backup", now)
}

// --- the device and the hooks -------------------------------------------------

/// The two round trips a safe write needs, and nothing else.
///
/// `digi_midi` implements this over a real box; a test implements it over a map
/// of payloads. Keeping it this narrow is what lets the whole flow — including
/// the ordering the safety rules are *about* — be tested without hardware.
pub trait PatternIo {
    fn identity(&self) -> Option<&DeviceIdentity>;
    fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String>;
    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String>;
}

/// Progress, consent, and an optional second copy of the backup.
///
/// **Every method has a default, and rule 1 does not rest on any of them.** It
/// rests on [`crate::backup_stash::Stash::stash`], which runs before any of these
/// and whose failure aborts the write. That is a change from both the JS and this
/// port's first draft, where `on_backup` was mandatory because the browser's
/// backup *was* a download and the stash was only a second copy — see the
/// [`crate::backup_stash`] module doc for why the desktop inverts it.
///
/// So `on_backup` is now what it should have been: somewhere for a caller that
/// wants its own extra copy of every backup to put one. A caller that does not
/// want one is not less safe.
pub trait WriteHooks {
    /// Receives the untouched destination pattern after it is safely in the
    /// stash and before anything is sent. Returning `Err` still aborts the
    /// write — a caller that asked for a second copy and did not get one is
    /// entitled to stop.
    fn on_backup(&mut self, _backup: &PatternKitFile) -> Result<(), String> {
        Ok(())
    }

    /// Return `false` to cancel. The default consents, which is only correct for
    /// a caller that has already asked — the UI's confirm dialog lives here.
    fn confirm(&mut self, _args: &ConfirmArgs) -> bool {
        true
    }

    /// Consent for a whole-slot restore, which gets `(label, index)` and not a
    /// [`ConfirmArgs`].
    ///
    /// **Deliberately not the same hook.** `ConfirmArgs` describes one track of a
    /// pattern that decoded, and building one would mean decoding the slot's
    /// current bytes — which is exactly what a restore cannot depend on, because
    /// the state being reverted may be the botched write that will not decode.
    /// The caller's own text names the capture instead.
    fn confirm_restore(&mut self, _label: &str, _index: u8) -> bool {
        true
    }

    fn on_status(&mut self, _status: &str) {}
    fn on_log(&mut self, _line: &str) {}
}

/// Everything about the destination that is only knowable after the re-fetch —
/// which is to say, everything a confirm dialog needs and cannot look up itself.
///
/// **Per slot, not per track**, since Phase 10. A write is one re-fetch, one
/// backup, one send and one verify covering however many of that slot's tracks
/// the caller named — so consent is asked once, about all of them, with
/// [`ConfirmArgs::tracks`] holding the per-track half. The pattern-wide facts
/// above it (swing, the lane pool) are stated once because there is only one of
/// each: a dialog that repeated the pool budget per track would be describing a
/// budget sixteen times over that all sixteen tracks share.
#[derive(Debug)]
pub struct ConfirmArgs<'a> {
    /// The destination decoded, for the dialog's kit name and track kinds.
    /// `None` on a gen-1 write ([`a4_safe_write_tracks`]), where there is no
    /// `PatternKit` to decode into — the per-track facts below are still real.
    pub pattern_kit: Option<&'a PatternKit>,
    pub label: String,
    pub index: u8,
    /// The swing the box is holding, so a UI can say what the write changes it
    /// *to*. This one reaches every track in the slot, unlike anything in
    /// [`TrackConfirm`]. `None` where swing is not in the mapped format — the
    /// gen-1 write neither reads nor moves it.
    pub swing: Option<u8>,
    /// Spare lanes in the pattern's pool of 80 — the budget *every* named track's
    /// lanes must fit between them, which is why it is here and not per track.
    /// `None` where the write cannot spend from the pool at all.
    pub free_lanes: Option<usize>,
    /// One entry per track being written, in the order the caller named them.
    /// Never empty: [`safe_write_tracks`] refuses a write with no tracks in it
    /// before anything is fetched.
    pub tracks: Vec<TrackConfirm>,
}

impl ConfirmArgs<'_> {
    /// The single track, for the callers that only ever write one.
    ///
    /// [`safe_write_track`] guarantees exactly one entry, so a panic here is a
    /// caller reaching a single-track dialog from a multi-track write — a
    /// mis-wiring rather than a runtime condition, and one that must not be
    /// papered over with the first element.
    #[track_caller]
    pub fn one(&self) -> &TrackConfirm {
        match self.tracks.as_slice() {
            [only] => only,
            other => panic!(
                "this confirm hook words one track and the write names {} — use `tracks`",
                other.len()
            ),
        }
    }
}

/// What one track of the destination holds right now, and what is aimed at it.
#[derive(Debug, Clone)]
pub struct TrackConfirm {
    pub track_index: usize,
    /// Trigs the destination track holds right now — what is being replaced.
    pub existing_trigs: usize,
    pub note_count: usize,
    /// What this track has locked on the box right now.
    pub box_plocks: Vec<PoolLane>,
}

/// One track's worth of write, as the caller describes it.
#[derive(Debug, Clone, Default)]
pub struct TrackWrite {
    pub index: u8,
    pub track_index: usize,
    /// Encoder-shaped notes, each with the trig settings that travel with it.
    /// Paired rather than merged so `Note` stays the hardware-verified encode
    /// shape — the same decision `trig_settings_from_notes` is built on.
    pub notes: Vec<(Note, TrigSetting)>,
    /// `None` leaves the byte alone, which is what a caller with nothing to say
    /// about it should do.
    pub track_prob: Option<u8>,
    /// `None` leaves the lane pool completely alone. `Some` — **including an
    /// empty vec** — means "these are the track's lanes", so lanes the track has
    /// on the box and the caller does not are freed. That is deliberate and
    /// matches the conditions scrub: the notes are being replaced, and automation
    /// left behind would belong to trigs that no longer exist.
    pub plocks: Option<Vec<LaneWrite>>,
    /// `None` leaves the byte alone. Per *pattern*, so it changes every track in
    /// the slot — the confirm hook is where a caller says so.
    pub swing: Option<f64>,
}

// No `Eq`: `ByteDiff` does not have one, and a diff list is the point of this
// struct.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteResult {
    /// The verify passed. An `ok: false` that is not `cancelled` always carries
    /// the offsets that mismatched, for a loud report.
    pub ok: bool,
    pub cancelled: bool,
    pub diffs: Vec<ByteDiff>,
    pub dropped: usize,
    pub written: usize,
    /// What landed differently from what was asked for while still being a
    /// successful write — a full lane pool, say. Callers show these alongside
    /// the result.
    pub warnings: Vec<String>,
    pub label: String,
    pub index: u8,
    /// Which of the slot's tracks this write covered, in the order they were
    /// named. **Empty for a restore**, which replaces the whole slot and names
    /// no track — the shape that made `write_result_message` describe a revert
    /// as "Wrote 0 notes to A01 T1" back when this was a bare `usize`.
    pub tracks: Vec<usize>,
    pub backup: Option<PatternKitFile>,
    pub payload: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// The allowlist, the decoder, or no device at all.
    Gate(String),
    /// The pattern about to be overwritten could not be stored. **This is rule
    /// 1 refusing**, and it is why the stash is not best-effort: it holds the
    /// only automatic copy, so nothing has been sent.
    Stash(StashError),
    /// The caller's own backup hook failed. Nothing has been sent, and the
    /// stash copy is already safe.
    Backup(String),
    /// A fetch or a send failed.
    Io(String),
    /// The payload could not be encoded, or the track/pattern does not exist.
    Encode(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gate(m) => write!(f, "{m}"),
            Self::Stash(e) => write!(f, "nothing was written, because the backup couldn't be saved: {e}"),
            Self::Backup(m) => write!(f, "backup failed, nothing was written: {m}"),
            Self::Io(m) => write!(f, "{m}"),
            Self::Encode(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for WriteError {}

// --- the write ----------------------------------------------------------------

/// Replace one track's notes in one pattern on the box, safely.
///
/// [`safe_write_tracks`] with one track in it, kept as its own name because it is
/// what almost every caller wants and because the single-track wording — one
/// dialog about one track — is the shape `ui::write` and the example are built
/// on. Nothing is special-cased below: this really is the plural flow with a
/// one-element slice, so the two cannot drift apart on any of the five rules.
pub fn safe_write_track(
    device: &mut impl PatternIo,
    stash: &Stash,
    write: &TrackWrite,
    hooks: &mut impl WriteHooks,
    now: Timestamp,
) -> Result<WriteResult, WriteError> {
    safe_write_tracks(device, stash, std::slice::from_ref(write), hooks, now)
}

/// Replace several tracks of **one** pattern on the box, safely, in one pass.
///
/// The only way to write tracks. Every step of the sequence is here — re-fetch,
/// confirm, stash, backup, encode, send, read back, compare — so a caller cannot
/// hold half of it.
///
/// `stash` is where the second copy of the backup goes before the caller's hook
/// is called; see [`crate::backup_stash`] for why there are two copies.
///
/// ## Why this is plural, and why it is not a loop
///
/// Phase 10's mass send syncs every track of a session to its boxes, and the
/// obvious implementation — call [`safe_write_track`] sixteen times per box —
/// breaks rule 3 by obeying it. Every call takes its own backup, so one press
/// puts **32 entries into a fifty-entry ring**: the feature quietly destroys the
/// recovery path it depends on, and it does it on the run where you most want
/// the recovery path. It is also sixteen re-fetches, sixteen 127 KB sends and
/// sixteen verify reads of the *same slot*.
///
/// So a write is per slot. One re-fetch is the backup and the base every named
/// track is encoded into; one send; one verify. Two boxes cost two backups
/// rather than thirty-two, and the ring still holds the last fifty things this
/// app overwrote.
///
/// ## What the caller must get right
///
/// Every [`TrackWrite`] must name the same slot ([`TrackWrite::index`]) and a
/// distinct track, and they must agree about swing. All three are refused rather
/// than resolved: two writes aimed at one track would silently let the last one
/// win, and two swings for one pattern is a caller that has not decided what the
/// pattern's feel is. An empty slice is refused too — there is no such thing as
/// a write of nothing, and letting one through would take a backup and send a
/// payload identical to what was already there.
///
/// ## The order inside
///
/// The tracks are encoded in the order given, each onto the payload the previous
/// one produced, because the p-lock pool of 80 is **shared across the slot**:
/// applying them one at a time against the evolving payload is what makes the
/// pool's remaining room real rather than counted sixteen times over. That is
/// also why [`ConfirmArgs::free_lanes`] is a slot-level number: it is the budget
/// they are all spending out of.
pub fn safe_write_tracks(
    device: &mut impl PatternIo,
    stash: &Stash,
    writes: &[TrackWrite],
    hooks: &mut impl WriteHooks,
    now: Timestamp,
) -> Result<WriteResult, WriteError> {
    let gate = write_gate(device.identity());
    if !gate.ok {
        return Err(WriteError::Gate(gate.reason));
    }
    // The gate passed, so the identity is present, decodable, and has a family.
    let identity = device.identity().expect("the gate refuses a missing identity").clone();
    // Decodable is not the same as gen-2 decodable: the gate also passes the
    // Analog Four, whose decoder is `a4_pattern` and whose write flow is
    // [`a4_safe_write_tracks`]. A caller that routed it here is refused rather
    // than encoded with a `Spec` this box does not have.
    let Some(spec) = spec_for(&identity.slug) else {
        return Err(WriteError::Gate(format!(
            "{} patterns are gen-1 — this write goes through the Analog Four flow, not the \
             gen-2 one",
            identity.name
        )));
    };
    let family = identity
        .family
        .expect("a decodable box has a dump family");

    // Before the fetch, because none of these need a box to be wrong.
    let index = match writes {
        [] => {
            return Err(WriteError::Encode(
                "nothing to write: a write with no tracks in it would back the slot up and send \
                 it back unchanged"
                    .into(),
            ))
        }
        [first, ..] => first.index,
    };
    if let Some(stray) = writes.iter().find(|w| w.index != index) {
        return Err(WriteError::Encode(format!(
            "one write, one slot: these tracks are aimed at {} and {}",
            bank_name(index as usize),
            bank_name(stray.index as usize)
        )));
    }
    for (position, write) in writes.iter().enumerate() {
        if writes[..position].iter().any(|w| w.track_index == write.track_index) {
            return Err(WriteError::Encode(format!(
                "track {} is named twice in one write — the second would silently replace the \
                 first",
                write.track_index + 1
            )));
        }
    }
    // Swing is one byte belonging to the whole pattern, so two answers for one
    // send is a caller that has not decided. `None` is not an answer and does not
    // conflict with one: it means "leave the byte alone".
    let mut swing: Option<f64> = None;
    for write in writes {
        match (swing, write.swing) {
            (Some(a), Some(b)) if a != b => {
                return Err(WriteError::Encode(format!(
                    "two swings for one pattern: {a} and {b} — swing belongs to the slot, not to \
                     a track"
                )))
            }
            (None, Some(b)) => swing = Some(b),
            _ => {}
        }
    }

    let label = bank_name(index as usize);

    // Rule: re-fetch. This payload is both the backup and the base we edit, so
    // the write can only ever differ from what is on the box right now by the
    // tracks we were asked to change.
    hooks.on_status(&format!("Fetching {label} for backup…"));
    let original = device.fetch_pattern_kit(index).map_err(WriteError::Io)?;
    let target = decode_pattern_kit(&spec, &original).map_err(WriteError::Encode)?;

    let mut tracks = Vec::with_capacity(writes.len());
    for write in writes {
        tracks.push(TrackConfirm {
            track_index: write.track_index,
            existing_trigs: track_trig_count(&target, write.track_index),
            note_count: write.notes.len(),
            box_plocks: read_track_plocks(&spec, &original, write.track_index)
                .map_err(WriteError::Encode)?,
        });
    }

    let consented = hooks.confirm(&ConfirmArgs {
        pattern_kit: Some(&target),
        label: label.clone(),
        index,
        swing: Some(read_swing(&spec, &original)),
        free_lanes: Some(free_lane_count(&spec, &original)),
        tracks,
    });
    if !consented {
        return Ok(WriteResult {
            ok: false,
            cancelled: true,
            diffs: Vec::new(),
            dropped: 0,
            written: 0,
            warnings: Vec::new(),
            label,
            index,
            tracks: writes.iter().map(|w| w.track_index).collect(),
            backup: None,
            payload: None,
        });
    }

    // Rule 1, and the point past which the destination is recoverable. This is
    // the only automatic copy, so its failure is the write's failure — nothing
    // below this line runs if the backup did not land.
    //
    // **One backup, however many tracks.** The context names a track only when
    // there is one to name; a slot-wide write reads as "before a write" in the
    // restore list, which is what it is — the row restores all sixteen tracks
    // either way.
    let backup = pattern_kit_backup(&identity.slug, family, index, &original, now);
    let stashed = stash
        .stash(
            &backup,
            &BackupContext {
                device_name: identity.name.clone(),
                kit_name: target.kit.name.clone(),
                track_index: match writes {
                    [only] => Some(only.track_index),
                    _ => None,
                },
            },
        )
        .map_err(WriteError::Stash)?;
    hooks.on_backup(&backup).map_err(WriteError::Backup)?;
    hooks.on_log(&format!("Backed up {} — restorable from “{}”", stashed.summary(), backup.name));

    // Each track is encoded onto what the last one produced, so the shared lane
    // pool is spent once rather than counted once per track.
    let mut payload = original.clone();
    let mut dropped = 0;
    let mut written = 0;
    let mut warnings = Vec::new();
    for write in writes {
        let notes: Vec<Note> = write.notes.iter().map(|(n, _)| n.clone()).collect();
        let (next, lost) = encode_track_notes(&spec, &payload, write.track_index, &notes)
            .map_err(WriteError::Encode)?;
        payload = next;
        dropped += lost;
        written += write.notes.len() - lost;

        // Per-trig conditions live in three per-step lanes the encoder does not
        // know about, so they go on afterwards, into the fresh copy it just
        // returned. `apply_track_trig_settings` scrubs all 128 steps of this
        // track's lanes first — the box does that when it creates a trig, and a
        // write that skips it would leave a new trig inheriting a deleted one's
        // probability.
        apply_track_trig_settings(
            &spec,
            &mut payload,
            write.track_index,
            &trig_settings_from_notes(&write.notes),
        )
        .map_err(WriteError::Encode)?;

        // The track's own PROB default is one byte in the defaults tail. Only
        // touched when the caller has a value; a caller that does not model it
        // leaves whatever the box was already holding.
        if let Some(prob) = write.track_prob {
            // `Some` here, always: `apply_track_prob`'s own `None` means "write
            // the box default of 100", which is a different thing from this
            // caller's `None`, which means "leave the byte where the box left
            // it".
            apply_track_prob(&spec, &mut payload, write.track_index, Some(prob))
                .map_err(WriteError::Encode)?;
        }

        // p-lock lanes live in the pattern-wide pool of 80, shared with the other
        // fifteen tracks — so unlike the condition lanes this scrubs per lane
        // rather than wholesale, and it can run out of room. When it does, the
        // notes still land and the shortfall comes back as a warning.
        if let Some(lanes) = &write.plocks {
            warnings.extend(
                apply_track_plocks(&spec, &mut payload, write.track_index, lanes)
                    .map_err(WriteError::Encode)?,
            );
        }
    }

    // Swing is one byte in the pattern's settings tail, and it belongs to the
    // whole slot rather than any track — so it only moves when a caller has a
    // value, it is applied once, and callers are expected to have warned about
    // the reach.
    if swing.is_some() && !apply_swing(&spec, &mut payload, swing) {
        return Err(WriteError::Encode(
            "payload is too short to hold the pattern's swing byte".into(),
        ));
    }

    hooks.on_status(&match writes {
        [only] => format!("Writing {label} T{}…", only.track_index + 1),
        many => format!("Writing {} to {label}…", plural(many.len(), "track")),
    });
    device.send_pattern_kit(index, &payload).map_err(WriteError::Io)?;

    hooks.on_status("Verifying — reading the pattern back…");
    let reread = device.fetch_pattern_kit(index).map_err(WriteError::Io)?;
    let diffs = diff_payloads(&payload, &reread, VERIFY_DIFF_CAP);

    Ok(WriteResult {
        ok: diffs.is_empty(),
        cancelled: false,
        diffs,
        dropped,
        written,
        warnings,
        label,
        index,
        tracks: writes.iter().map(|w| w.track_index).collect(),
        backup: Some(backup),
        payload: Some(payload),
    })
}

// --- the Analog Four's write ----------------------------------------------------

/// One authored trig: everything about a step this format can carry.
///
/// Four fields where there was one until 2026-09-01, when the per-step lanes
/// were measured on the box — see [`crate::a4_pattern::LANES`]. Every one is
/// written explicitly rather than left at `FF`, for the reason the note lane
/// already was: `FF` means "follow the track default", so a trig left unset
/// would drift the day somebody turns that default on the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct A4Step {
    pub note: u8,
    /// Clamped into the box's 1–127 by the writer; the A4's floor is one, not
    /// zero.
    pub velocity: u8,
    /// The Elektron length byte — the *digis'* length byte, which is the same
    /// scale: `0x00` is .125 of a step, `0x0e` is one, `0x7f` is INF.
    pub length: u8,
    /// Ticks of 1/24 of a step, -23..=23.
    pub micro_timing: i8,
    /// The TRC menu index, or `None` for a trig with no condition.
    ///
    /// **Written either way**, unlike the note lane's `FF`: `None` clears the
    /// lane, because since the menu was mapped this app can carry an A4
    /// condition through a round trip, so "no condition here" is now something
    /// a caller can actually mean rather than something it cannot express.
    pub condition: Option<u8>,
}

/// One track's worth of gen-1 write, as `core` describes it.
///
/// The A4 twin of [`TrackWrite`]. It carries note, velocity, length, micro
/// timing and trig condition per step — the five lanes hardware named on
/// 2026-09-01 — and, since the pool writer landed the same day, the track's
/// p-lock lanes. Not the ARP note lanes, which are named and have no
/// representation in this app's model; those keep the destination's own bytes.
///
/// Chords and notes past step 64 were resolved by the caller, whose warnings
/// ride beside the write — the same split `core::export` makes for a gen-2
/// track.
#[derive(Debug, Clone)]
pub struct A4TrackWrite {
    /// Destination slot, 0–127 — the box's eight banks of sixteen.
    pub index: u8,
    /// 0–5: SYN1–SYN4, FX, CV.
    pub track_index: usize,
    /// One entry per step, always all 64. `Some` authors a note trig; `None`
    /// clears the step — so this write is the track's whole trig lane, exactly
    /// as a gen-2 [`TrackWrite`]'s notes are the track's whole contents, and an
    /// empty track is a deliberate clear rather than a no-op.
    pub steps: Vec<Option<A4Step>>,
    /// This track's p-lock lanes, or `None` to leave the pool alone.
    ///
    /// The same two meanings [`TrackWrite::plocks`] carries, and the same
    /// reason for the `Option`: `Some(vec)` says *the track's lanes are the
    /// truth*, so a parameter not in it is one the caller means to remove,
    /// while `None` says the caller has nothing to say about the pool and the
    /// destination keeps its own. Every A4 write took the second meaning until
    /// 2026-09-01, because there was no encoder.
    ///
    /// **`Some` on this box moves more bytes than `Some` on a digi.** The pool
    /// is rebuilt in `(param_id, track)` order, so lanes belonging to the
    /// tracks this write never names change *index* — see
    /// [`crate::a4_plocks::apply_track_plocks`] for why that is forced and what
    /// is preserved instead.
    pub plocks: Option<Vec<A4LaneWrite>>,
}

/// Replace several tracks of **one** pattern on an Analog Four, safely, in one
/// pass — [`safe_write_tracks`] for the gen-1 format.
///
/// A twin rather than a branch, for the reason `core` keeps `import` and
/// `a4_transfer` as two files: the ceremony is the same five rules in the same
/// order — gate, re-fetch, confirm, stash, send, read back, byte-compare — and
/// almost nothing *inside* the steps is shared. The decode is
/// [`crate::a4_pattern`]'s rather than a `Spec`'s, the encode is five per-step
/// lanes plus a pool rebuild rather than an encoder with a trig-record pool, and
/// there is no swing and no per-track PROB to carry, because neither is in the
/// mapped format.
///
/// **The re-fetch is what retired the baseline rule.** Until 2026-08-31 an A4
/// write was composed on a dump kept from receive time, because "the A4 cannot
/// be re-fetched" — and it can (`0x64`, PLAN.md §10 "The A4 answers dump
/// requests"). Composing on the destination read moments before the send means
/// the unmapped 10 KB — sounds, p-locks, every unnamed lane — is the
/// *destination's own*, so a write can only ever differ from what is on the box
/// right now by the tracks it was asked to change. That is `safe_write_tracks`'
/// exact bargain, kept with the same bytes.
///
/// The one deliberate asymmetry: the send is DIN-paced by the [`PatternIo`]
/// implementation, because an unpaced 14 KB frame is the shape this box has
/// never once accepted (`digi_midi::a4_transfer`). Nothing here knows that —
/// pacing is the wire's problem, and this function's `send` is one call either
/// way.
pub fn a4_safe_write_tracks(
    device: &mut impl PatternIo,
    stash: &Stash,
    writes: &[A4TrackWrite],
    hooks: &mut impl WriteHooks,
    now: Timestamp,
) -> Result<WriteResult, WriteError> {
use crate::a4_pattern::{
        clear_trig, read_track_trigs, set_note_trig, set_trig_condition, set_trig_length,
    set_trig_micro_timing, set_trig_velocity, slot_name, trig_offset, trig_state, TrigState,
    NUM_STEPS, NUM_TRACKS,
        PAYLOAD_LEN, SLOT_MARKER,
    };

    let gate = write_gate(device.identity());
    if !gate.ok {
        return Err(WriteError::Gate(gate.reason));
    }
    let identity = device.identity().expect("the gate refuses a missing identity").clone();
    // The mirror of `safe_write_tracks`' own refusal: the gate passes both
    // formats, and a digi routed here would have its pattern-kit read at
    // `a4_pattern`'s offsets — plausible nonsense, refused by name instead.
    if decoder_for(&identity.slug) != Some("a4") {
        return Err(WriteError::Gate(format!(
            "{} patterns are gen-2 — this write goes through `safe_write_tracks`, not the \
             Analog Four flow",
            identity.name
        )));
    }
    let family = identity.family.expect("a decodable box has a dump family");

    // Before the fetch, because none of these need a box to be wrong.
    let index = match writes {
        [] => {
            return Err(WriteError::Encode(
                "nothing to write: a write with no tracks in it would back the slot up and send \
                 it back unchanged"
                    .into(),
            ))
        }
        [first, ..] => first.index,
    };
    if let Some(stray) = writes.iter().find(|w| w.index != index) {
        return Err(WriteError::Encode(format!(
            "one write, one slot: these tracks are aimed at {} and {}",
            slot_name(index),
            slot_name(stray.index)
        )));
    }
    for (position, write) in writes.iter().enumerate() {
        if write.track_index >= NUM_TRACKS {
            return Err(WriteError::Encode(format!(
                "no track {}; an A4 pattern has {NUM_TRACKS}",
                write.track_index + 1
            )));
        }
        if write.steps.len() != NUM_STEPS {
            return Err(WriteError::Encode(format!(
                "track {} names {} steps and an A4 trig lane has {NUM_STEPS} — a partial lane \
                 would leave steps nobody decided about",
                write.track_index + 1,
                write.steps.len()
            )));
        }
        if writes[..position].iter().any(|w| w.track_index == write.track_index) {
            return Err(WriteError::Encode(format!(
                "track {} is named twice in one write — the second would silently replace the \
                 first",
                write.track_index + 1
            )));
        }
    }

    let label = slot_name(index);

    // Rule: re-fetch. This payload is both the backup and the base we edit —
    // see the header for what that buys on this box in particular.
    hooks.on_status(&format!("Fetching {label} for backup…"));
    let original = device.fetch_pattern_kit(index).map_err(WriteError::Io)?;
    if original.len() != PAYLOAD_LEN {
        return Err(WriteError::Encode(format!(
            "the box answered {} bytes for {label}, an A4 pattern is {PAYLOAD_LEN}",
            original.len()
        )));
    }

    let mut tracks = Vec::with_capacity(writes.len());
    for write in writes {
        tracks.push(TrackConfirm {
            track_index: write.track_index,
            // `read_track_trigs` counts what the box shows — residue excluded —
            // and the survey a UI ran before its dialog counts with the same
            // call, so the two answers can be compared rather than reconciled.
            existing_trigs: read_track_trigs(&original, write.track_index)
                .map_err(WriteError::Encode)?
                .len(),
            note_count: write.steps.iter().filter(|s| s.is_some()).count(),
            // The pool is real on this box and unmapped; a write never touches
            // it, so there is nothing being cleared to warn about.
            box_plocks: Vec::new(),
        });
    }

    let consented = hooks.confirm(&ConfirmArgs {
        pattern_kit: None,
        label: label.clone(),
        index,
        swing: None,
        free_lanes: None,
        tracks,
    });
    if !consented {
        return Ok(WriteResult {
            ok: false,
            cancelled: true,
            diffs: Vec::new(),
            dropped: 0,
            written: 0,
            warnings: Vec::new(),
            label,
            index,
            tracks: writes.iter().map(|w| w.track_index).collect(),
            backup: None,
            payload: None,
        });
    }

    // Rule 1, and the point past which the destination is recoverable — the
    // bytes exactly as the box sent them, framed back up as the `0x54` message
    // it would take again ([`pattern_dump_type`]).
    let backup = pattern_kit_backup(&identity.slug, family, index, &original, now);
    let stashed = stash
        .stash(
            &backup,
            &BackupContext {
                device_name: identity.name.clone(),
                // Nothing in the mapped format is a kit name, and inventing one
                // would label the restore list with a guess.
                kit_name: String::new(),
                track_index: match writes {
                    [only] => Some(only.track_index),
                    _ => None,
                },
            },
        )
        .map_err(WriteError::Stash)?;
    hooks.on_backup(&backup).map_err(WriteError::Backup)?;
    hooks.on_log(&format!("Backed up {} — restorable from “{}”", stashed.summary(), backup.name));

    let mut payload = original.clone();
    let mut written = 0usize;
    let mut plock_warnings: Vec<String> = Vec::new();
    for write in writes {
        for (step, authored) in write.steps.iter().enumerate() {
            match authored {
                Some(trig) => {
                    let track = write.track_index;
                    // Written explicitly even when it equals the track default:
                    // leaving a lane at FF would make the trig follow a default
                    // somebody may change on the box later.
                    set_note_trig(&mut payload, track, step, Some(trig.note))
                        .map_err(WriteError::Encode)?;
                    set_trig_velocity(&mut payload, track, step, Some(trig.velocity))
                        .map_err(WriteError::Encode)?;
                    set_trig_length(&mut payload, track, step, Some(trig.length))
                        .map_err(WriteError::Encode)?;
                    set_trig_micro_timing(&mut payload, track, step, trig.micro_timing)
                        .map_err(WriteError::Encode)?;
                    // The condition is written even when it is `None`, which
                    // clears the lane. That is a deliberate reversal of what
                    // this loop did for the few hours between the lane being
                    // named and its menu being read: while the byte could not
                    // be *shown*, clearing it would have destroyed on every
                    // press a field the user could neither see beforehand nor
                    // restore after, so the lane was left alone. Now that a
                    // condition survives the round trip, leaving it alone would
                    // be the lossy choice — a condition removed in the roll
                    // would come straight back off the box.
                    set_trig_condition(&mut payload, track, step, trig.condition)
                        .map_err(WriteError::Encode)?;
                    written += 1;
                }
                // **A step this app calls empty may hold a trigless trig, and
                // clearing it would be destroying something nobody could see.**
                // This model holds notes; a trigless trig is a trig with no
                // note, so it has no representation here and an import counts
                // it rather than carrying it. A user therefore cannot have
                // *intended* to remove one — there was nothing on screen to
                // remove — and until 2026-09-01 a write-back deleted every one
                // of them silently.
                //
                // A note trig on the same step is a different matter: that one
                // was on screen, and clearing it is exactly what deleting the
                // note meant.
                None => {
                    let o = trig_offset(write.track_index, step);
                    if trig_state(payload[o], payload[o + 1]) != TrigState::Trigless {
                        clear_trig(&mut payload, write.track_index, step)
                            .map_err(WriteError::Encode)?;
                    }
                }
            }
        }
    }
    // The pool, after the trig lanes and once per track — the same order
    // `safe_write_tracks` composes a gen-2 write in, and for the same reason:
    // `apply_track_plocks` scrubs the lanes it is replacing, so it must not run
    // before something that then edits them.
    //
    // A `None` here leaves the destination's pool exactly as fetched, which is
    // what every A4 write did before there was an encoder.
    for write in writes {
        if let Some(lanes) = &write.plocks {
            plock_warnings.extend(
                crate::a4_plocks::apply_track_plocks(&mut payload, write.track_index, lanes)
                    .map_err(WriteError::Encode)?,
            );
        }
    }

    // The payload's own slot marker. Fetched from this very slot it already
    // agrees — except on a slot the box has never saved, where it reads FF, and
    // the box itself writes the slot here on every save. Doing the same is the
    // one byte this write touches outside the named tracks' lanes.
    payload[SLOT_MARKER] = index;

    hooks.on_status(&match writes {
        [only] => format!("Writing {label} T{}…", only.track_index + 1),
        many => format!("Writing {} to {label}…", plural(many.len(), "track")),
    });
    device.send_pattern_kit(index, &payload).map_err(WriteError::Io)?;

    hooks.on_status("Verifying — reading the pattern back…");
    let reread = device.fetch_pattern_kit(index).map_err(WriteError::Io)?;
    let diffs = diff_payloads(&payload, &reread, VERIFY_DIFF_CAP);

    Ok(WriteResult {
        ok: diffs.is_empty(),
        cancelled: false,
        diffs,
        dropped: 0,
        written,
        warnings: plock_warnings,
        label,
        index,
        tracks: writes.iter().map(|w| w.track_index).collect(),
        backup: Some(backup),
        payload: Some(payload),
    })
}

/// Send a previously taken backup back to its slot, safely.
///
/// The counterpart to [`safe_write_track`] for the one write whose payload must
/// **not** come from a re-fetch: the whole point of a restore is reverting the
/// slot to bytes captured earlier, and the caller's confirm text says which
/// capture. The safety rules still apply everywhere they can — the allowlist gate
/// runs at send time rather than when a button was enabled, what the slot holds
/// *now* is backed up first (the state being reverted may be the evidence of what
/// went wrong), and the result is read back and byte-compared.
pub fn safe_restore_pattern_kit(
    device: &mut impl PatternIo,
    stash: &Stash,
    index: u8,
    payload: &[u8],
    hooks: &mut impl WriteHooks,
    now: Timestamp,
) -> Result<WriteResult, WriteError> {
    let gate = write_gate(device.identity());
    if !gate.ok {
        return Err(WriteError::Gate(gate.reason));
    }
    let identity = device.identity().expect("the gate refuses a missing identity").clone();
    let family = identity.family.expect("a decodable box has a dump family");
    let label = bank_name(index as usize);

    hooks.on_status(&format!("Fetching {label} — backing up what it holds now…"));
    let current = device.fetch_pattern_kit(index).map_err(WriteError::Io)?;

    // Nothing here decodes `current`. A restore is the one write whose whole
    // purpose is replacing bytes that may not decode at all, so making consent
    // depend on decoding them would lock the door from the inside.
    if !hooks.confirm_restore(&label, index) {
        return Ok(WriteResult {
            ok: false,
            cancelled: true,
            diffs: Vec::new(),
            dropped: 0,
            written: 0,
            warnings: Vec::new(),
            label,
            index,
            tracks: Vec::new(),
            backup: None,
            payload: None,
        });
    }

    // The state being reverted away from may be the evidence of what went wrong,
    // so it is stored under the same rule and with the same teeth. No track
    // index: a restore replaces the whole slot.
    let backup = pattern_kit_file(&identity.slug, family, index, &current, "pre-restore", now);
    let stashed = stash
        .stash(
            &backup,
            &BackupContext {
                device_name: identity.name.clone(),
                // Deliberately not decoded — see `confirm_restore`. A slot being
                // restored may hold bytes that will not decode, and asking for
                // its kit name is asking for exactly that.
                kit_name: String::new(),
                track_index: None,
            },
        )
        .map_err(WriteError::Stash)?;
    hooks.on_backup(&backup).map_err(WriteError::Backup)?;
    hooks.on_log(&format!("Saved the current state first: {}", stashed.summary()));

    hooks.on_status(&format!("Restoring {label}…"));
    device.send_pattern_kit(index, payload).map_err(WriteError::Io)?;

    hooks.on_status("Verifying — reading the pattern back…");
    let reread = device.fetch_pattern_kit(index).map_err(WriteError::Io)?;
    let diffs = diff_payloads(payload, &reread, VERIFY_DIFF_CAP);

    Ok(WriteResult {
        ok: diffs.is_empty(),
        cancelled: false,
        diffs,
        dropped: 0,
        written: 0,
        warnings: Vec::new(),
        label,
        index,
        tracks: Vec::new(),
        backup: Some(backup),
        payload: Some(payload.to_vec()),
    })
}

// --- the wording --------------------------------------------------------------

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Every write path ends with this line, so no confirm dialog can imply the
/// backup is optional.
///
/// It says *where* since 2026-08-18, because that is now a thing a user can act
/// on: the backups are a list in this app rather than files appearing in a
/// downloads folder.
pub const BACKUP_LINE: &str =
    "The whole destination pattern is backed up first, and can be restored from Backups.";

/// [`BACKUP_LINE`]'s counterpart for a restore, and it exists for the same reason:
/// no dialog that sends bytes may leave out where the previous state went.
///
/// Worded separately rather than shared because the two promises differ in a way
/// that matters. A write backs up *the destination pattern* and the backup is a
/// row in the restore list; a restore snapshots what the slot holds **now** into a
/// ring of [`crate::backup_stash::SNAPSHOT_MAX`] that the restore list deliberately
/// does not show, so pointing someone at "Backups" for it would send them to a list
/// their snapshot is not in.
pub const SNAPSHOT_LINE: &str =
    "What the slot holds right now is saved first — it shows up in Backups under the pre-restore \
     snapshots, so this can be undone.";

/// What a caller is telling the user about, beyond the track's trigs.
#[derive(Debug, Clone, Default)]
pub struct ImpactArgs<'a> {
    pub label: &'a str,
    /// The track these lines are about, or `None` when the caller is describing
    /// the **whole slot** — a mass send, where the swing line's "not just track
    /// N" tail would be naming one of sixteen tracks that are all going.
    pub track: Option<usize>,
    /// The p-lock lanes about to be written.
    pub lanes: &'a [LaneWrite],
    /// What that track holds on the box right now, from [`ConfirmArgs`].
    pub box_plocks: &'a [PoolLane],
    /// Spare lanes in the pattern's pool of 80, or `None` to skip the check.
    pub free_lanes: Option<usize>,
    /// The PROB default travelling with the notes; `None` = not touched.
    pub track_prob: Option<u8>,
    /// The swing about to be written; `None` = not touched.
    pub swing: Option<u8>,
    /// What the box holds now, so a write that changes nothing stays quiet.
    pub box_swing: Option<u8>,
}

/// The sentences a confirm dialog must not leave out: what a write does *beyond*
/// replacing the named track's trigs.
///
/// Shared by every write path, because each one of these is a surface a user can
/// be surprised by — automation vanishing, unlocked trigs suddenly playing at 40%
/// odds, or the whole slot's feel moving because swing belongs to the pattern
/// rather than the track. A path that drops one of them silently is the bug this
/// function exists to prevent.
///
/// Callers write their own header, trig-count line and path-specific caveats
/// around these, and append [`BACKUP_LINE`] last.
pub fn write_impact_lines(args: &ImpactArgs) -> Vec<String> {
    let mut lines = Vec::new();

    // p-lock lanes are replaced the way the trigs are — the caller's lane set is
    // the truth for this track — so automation the box holds and the caller does
    // not goes away. Never left to be discovered on playback.
    if !args.box_plocks.is_empty() || !args.lanes.is_empty() {
        let going = args
            .box_plocks
            .iter()
            .filter(|b| !args.lanes.iter().any(|l| l.param_id == b.param_id))
            .count();
        let mut parts = Vec::new();
        if !args.lanes.is_empty() {
            parts.push(format!("writes {}", plural(args.lanes.len(), "p-lock lane")));
        }
        if going > 0 {
            parts.push(format!(
                "clears {} that track has on the box",
                plural(going, "p-lock lane")
            ));
        }
        if !parts.is_empty() {
            lines.push(format!("This also {}.", parts.join(" and ")));
        }
        if let Some(free) = args.free_lanes {
            if args.lanes.len() > free + args.box_plocks.len() {
                lines.push(format!(
                    "Careful: the pattern only has {}, so some of them won't fit — you'll be told which.",
                    plural(free, "spare p-lock lane")
                ));
            }
        }
    }

    // A second write surface, so it gets named rather than slipped in.
    if let Some(prob) = args.track_prob {
        if prob != 100 {
            lines.push(format!(
                "That track's PROB default is also set to {prob}% — trigs without their own PROB \
                 lock will play at those odds."
            ));
        }
    }

    // Swing reaches further than the track being written — it is the whole
    // pattern's feel on the box — so it is spelled out whenever it would change
    // what the destination is currently doing.
    if let (Some(swing), Some(box_swing)) = (args.swing, args.box_swing) {
        if swing != box_swing {
            lines.push(match args.track {
                Some(track) => format!(
                    "Swing goes from {box_swing} to {swing} — that's the whole pattern, so it \
                     changes the feel of all 16 tracks in {}, not just track {}.",
                    args.label,
                    track + 1
                ),
                None => format!(
                    "Swing goes from {box_swing} to {swing} — that's the whole pattern, so it \
                     changes the feel of all 16 tracks in {}.",
                    args.label
                ),
            });
        }
    }

    lines
}

/// The one-line report for a finished write, identical wording everywhere it is
/// shown. `is_error` tells the UI whether to shout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultMessage {
    pub text: String,
    pub is_error: bool,
}

/// **The lines below carry no leading mark, and that is a fix rather than a
/// style.** They opened with `✓` and `⚠` — `js/main.js`'s marks, which a browser
/// draws — for as long as this function only ever printed to a terminal. The
/// first hardware write put the success line on screen for the first time, on
/// 2026-08-18, and `✓` (U+2713) came out as a **missing-glyph box**: egui's
/// bundled fonts do not have it, so the app's proudest sentence began with tofu.
/// `⚠` (U+26A0) was never seen and is the same class, so it went too.
///
/// The emphasis belongs to each surface instead, which each of them already has:
/// the UI colours the row and puts a failure in a modal, and a terminal has the
/// words. Note what was *not* done — stripping the marks in the UI — because that
/// would make the window and the terminal say different things, which is the one
/// thing this function exists to prevent. See `app::ui`'s glyph section for the
/// rule this is the fourth instance of.
pub fn write_result_message(result: &WriteResult) -> ResultMessage {
    // One track is named; several are counted. "A01 T1, T2, T5, T9, T11, T14"
    // is a line nobody reads to the end, and the row it lands in is 300px wide.
    let where_ = match result.tracks.as_slice() {
        [only] => format!("{} T{}", result.label, only + 1),
        many => format!("{} of {}", plural(many.len(), "track"), result.label),
    };
    if result.cancelled {
        return ResultMessage { text: "Write cancelled".into(), is_error: false };
    }
    if result.ok {
        let mut text = format!(
            "Wrote {} to {where_} — verified byte-identical",
            plural(result.written, "note")
        );
        if result.dropped > 0 {
            text.push_str(&format!(
                " ({} didn't fit and {} dropped)",
                plural(result.dropped, "note"),
                if result.dropped == 1 { "was" } else { "were" }
            ));
        }
        if !result.warnings.is_empty() {
            text.push_str(&format!(" — but {}", result.warnings.join("; ")));
        }
        // A warning means the write succeeded but not entirely as asked — a lane
        // that didn't fit, say. Flagged as an error so the UI shouts, because
        // "verified byte-identical" on its own would read as "all of it went".
        return ResultMessage { text, is_error: !result.warnings.is_empty() };
    }
    ResultMessage {
        text: format!(
            "Write verify FAILED for {where_}: {}+ bytes differ — the box did not store what we \
             sent. The pre-write backup is in Backups as “{}” — restore it to put the slot back.",
            result.diffs.len(),
            result.backup.as_ref().map(|b| b.name.as_str()).unwrap_or_default()
        ),
        is_error: true,
    }
}

/// The same, for a finished restore — because [`write_result_message`] cannot
/// describe one.
///
/// **This is a bug that was found by looking for the caller.** A
/// [`safe_restore_pattern_kit`] result carries `written: 0` and `track_index: 0`,
/// both correct — a restore replaces a whole slot, so it counts no notes and names
/// no track — and both of them are *read* by `write_result_message`, which would
/// therefore report the app's whole-pattern revert as “Wrote 0 notes to A01 T1 —
/// verified byte-identical”. Three claims, all false, on the line a person checks
/// a recovery by.
///
/// So the restore's wording lives here beside the write's, for the reason
/// `write_result_message` gives: the window, a log line and any future terminal
/// caller cannot come to say different things about the same result.
pub fn restore_result_message(result: &WriteResult) -> ResultMessage {
    if result.cancelled {
        return ResultMessage { text: "Restore cancelled".into(), is_error: false };
    }
    if result.ok {
        return ResultMessage {
            text: format!("Restored {} — verified byte-identical", result.label),
            is_error: false,
        };
    }
    // No "restore it to put the slot back" here, deliberately: the snapshot named
    // is what the slot held *before this attempt*, and a failed restore whose
    // remedy is another restore is a loop to walk into knowingly rather than to be
    // sent round by a result line. So this says where the previous state is and
    // stops.
    ResultMessage {
        text: format!(
            "Restore verify FAILED for {}: {}+ bytes differ — the box did not store what we sent. \
             What the slot held before this attempt is in Backups as “{}”.",
            result.label,
            result.diffs.len(),
            result.backup.as_ref().map(|b| b.name.as_str()).unwrap_or_default()
        ),
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::device::{identity_from_responses, DeviceResponse};

    #[test]
    #[should_panic(expected = "words one track and the write names 2")]
    fn a_single_track_confirm_hook_handed_a_slot_write_says_so_rather_than_taking_the_first() {
        // Found by a deliberate bug: `[only]` written `[only, ..]` failed
        // nothing, because every caller of `one()` in this repo really does pass
        // one track. The plant is a mis-wiring — a single-track dialog reached
        // from a mass send — and it would word the write as being about T1 while
        // sixteen tracks went. Loud is the only safe direction, so this
        // constructs the case by hand.
        let kit = PatternKit {
            version: 3,
            name: String::new(),
            tempo_bpm: 120.0,
            kit_index: 0,
            tracks: Vec::new(),
            kit: crate::pattern::KitInfo {
                version: 3,
                name: String::new(),
                sound_names: Vec::new(),
                midi_mask: 0,
            },
        };
        let track = TrackConfirm {
            track_index: 0,
            existing_trigs: 0,
            note_count: 0,
            box_plocks: Vec::new(),
        };
        let args = ConfirmArgs {
            pattern_kit: Some(&kit),
            label: "A01".into(),
            index: 0,
            swing: Some(50),
            free_lanes: Some(80),
            tracks: vec![track.clone(), TrackConfirm { track_index: 1, ..track }],
        };
        let _ = args.one();
    }

    fn identity(product_id: u8, build: &str) -> DeviceIdentity {
        let dev = DeviceResponse {
            product_id,
            supported_ids: vec![0x60],
            reported_name: String::new(),
        };
        identity_from_responses(&dev, build.into(), "1.0".into())
    }

    #[test]
    fn gate_allowed() {
        let r = write_gate(Some(&identity(42, "0070")));
        assert!(r.ok);
        assert_eq!(r.reason, "");
        assert_eq!(r.spec_kind, Some("dt2"));
        let r = write_gate(Some(&identity(43, "0049")));
        assert!(r.ok);
        assert_eq!(r.spec_kind, Some("dn2"));
    }

    #[test]
    fn gate_disallowed_build() {
        let r = write_gate(Some(&identity(43, "0001")));
        assert!(!r.ok);
        assert!(r.reason.contains("isn't write-verified"));
        assert!(r.reason.contains("0001"), "the build has to be in the message");
    }

    // A box we can talk to but cannot decode stays read-only regardless of build.
    #[test]
    fn gate_unknown_product() {
        let r = write_gate(Some(&identity(99, "0070")));
        assert!(!r.ok);
        assert!(r.reason.contains("read-only"));
        assert_eq!(r.spec_kind, None);
    }

    // The gen-1 Digitakt is a box this app can identify and cannot decode, so it
    // is the real case the unknown-product branch exists for.
    #[test]
    fn gate_closes_for_a_box_whose_patterns_we_cannot_decode() {
        let r = write_gate(Some(&identity(12, "0070")));
        assert!(!r.ok);
        assert_eq!(r.spec_kind, None);
        assert!(r.reason.contains("Digitakt"));
        assert_eq!(decoder_for("digitakt"), None);
        assert!(spec_for("digitakt").is_none());
    }

    #[test]
    fn gate_no_identity() {
        let r = write_gate(None);
        assert!(!r.ok);
        assert_eq!(r.reason, "no device connected");
    }

    // The A4 passes the same gate the digis do — its decoder is `a4_pattern`
    // rather than a `Spec`, which is `spec_for`'s answer and not the gate's.
    #[test]
    fn gate_passes_the_analog_four_on_its_verified_build_and_no_other() {
        let r = write_gate(Some(&identity(4, "0195")));
        assert!(r.ok, "{}", r.reason);
        assert_eq!(r.spec_kind, Some("a4"));
        assert!(spec_for("analogfour").is_none(), "gen-1 has no Spec to hand out");

        let r = write_gate(Some(&identity(4, "0196")));
        assert!(!r.ok, "an unverified A4 build must stay read-only");
        assert!(r.reason.contains("0196"));
    }

    #[test]
    fn the_allowlist_holds_only_the_builds_the_writes_were_verified_on() {
        // The one copy of this list in the crate. It had a duplicate at the crate
        // root until 2026-08-18; two copies of a safety allowlist can drift, and
        // the one that drifts is the one nothing tests.
        assert_eq!(
            WRITE_ALLOWED_BUILDS,
            &[
                ("digitakt2", &["0070", "0071"][..]),
                ("digitone2", &["0049", "0050"][..]),
                ("analogfour", &["0195"][..]),
            ]
        );
    }

    #[test]
    fn a_timestamp_formats_the_way_the_js_iso_string_sliced() {
        // 2026-08-01T12:34:56Z, the instant the JS suite's backup-name test uses.
        let t = Timestamp::from_unix_seconds(1_785_587_696);
        assert_eq!(t.file_stamp(), "2026-08-01T12-34-56");
        assert_eq!((t.year, t.month, t.day), (2026, 8, 1));
        assert_eq!((t.hour, t.minute, t.second), (12, 34, 56));
    }

    #[test]
    fn the_civil_calendar_holds_at_the_edges() {
        // Leap day, the epoch itself, and a pre-epoch second — the three places
        // era arithmetic transcribed by hand goes wrong. Every expected string
        // here came out of node first, from the JS's own
        // `toISOString().slice(0, 19).replaceAll(':', '-')`.
        assert_eq!(Timestamp::from_unix_seconds(0).file_stamp(), "1970-01-01T00-00-00");
        assert_eq!(Timestamp::from_unix_seconds(1_709_164_800).file_stamp(), "2024-02-29T00-00-00");
        assert_eq!(Timestamp::from_unix_seconds(-1).file_stamp(), "1969-12-31T23-59-59");
        assert_eq!(Timestamp::from_unix_seconds(951_782_400).file_stamp(), "2000-02-29T00-00-00");
    }
}
