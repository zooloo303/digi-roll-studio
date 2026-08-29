//! +Drive directory listing: the API path, rather than the dump path.
//!
//! Elektron boxes speak two unrelated SysEx mechanisms (see [`crate::protocol`]).
//! Everything else in this crate uses the *dump* mechanism — checksummed
//! payloads addressed by a one-byte index. That index is the reason this module
//! exists: one byte reaches 128 slots, and the +Drive holds far more than that.
//!
//! The API mechanism is an RPC with no checksum and no length field, and it
//! carries a general file system: list a directory, read a file, write a file.
//! This module implements the read-only listing half.
//!
//! Wire format ported from elk-herd (BSD-2-Clause, © 2017-2025 Mark Lentczner):
//! `src/SysEx/Message.elm` (`apiDirList`) and `src/SysEx/ApiUtil.elm`
//! (`argDirEntry`, `argString0win1252`, `argListAll`). See `CREDITS.md`.
//!
//! # What elk-herd used this for, and what it does not tell us
//!
//! elk-herd points this API at **samples** — `Elektron/Drive.elm` is "a model of
//! the +Drive *sample* tree", with roots `/`, `/factory` and `/trash`. Whether a
//! box's *preset* library lives in the same tree under some other path is a
//! question elk-herd never asks, because it never needed to. So this module can
//! list directories correctly and still not find presets: the format is ported,
//! the tree's shape is not documented. `examples/probe_drive.rs` is what answers
//! it, against a box.
//!
//! # Two file APIs, not one
//!
//! There are **two** renumberings of this file API, and a box answers one of
//! them. The first half of this module implements elk-herd's gen-1 numbering
//! (`0x10` DirList), which a DT2 answers and which reaches its *sample* tree.
//! The second half — below the banner comment — implements the `0x53`–`0x5C`
//! numbering a Digitone answers, which is where a DN2's preset banks live.
//! Read that section's header before touching it: under a dump header `0x53` is
//! a Sound dump, and under the API header it is List.
//!
//! # Read-only by construction
//!
//! Only the reading opcodes are implemented, in either numbering: `0x10`
//! DirList, and `0x53` List / `0x54` Open / `0x55` Read / `0x56` Close.
//! Everything that mutates — `0x11` DirCreate, `0x12` DirDelete, `0x20`
//! FileDelete, `0x21` ItemRename, the `0x4n` family, and `0x57`/`0x58`/`0x59`
//! WriteOpen/Write/WriteClose, `0x5A` Move, `0x5B` Copy, `0x5C` Delete — is
//! deliberately absent. There is no kit-builder reason to mutate a +Drive, and
//! the safest way not to delete somebody's library is to own no code that can.
//! [`assert_read_only_file_op`] enforces it for the second numbering.

use crate::device::cstring;
use crate::pattern::{u16_be, u32_be};
use crate::sound::{decode_a4_sound, decode_sound, Sound, SoundError, SOUND_MAGIC_FOOT, SOUND_MAGIC_HEAD, SOUND_NAME_OFFSET};

/// DirList request. Response comes back as `0x90`, per the API's
/// request-plus-0x80 convention.
pub const API_DIR_LIST: u8 = 0x10;

/// One entry in a +Drive directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Content hash. A project's sample pool refers to samples by hash and
    /// size rather than by path, so a sample survives being moved.
    pub hash: u32,
    /// Byte size for a file; elk-herd computes a directory's size itself, so
    /// expect this to be meaningless for a directory.
    pub size: u32,
    /// The write-protect flag the browser shows as a padlock.
    pub locked: bool,
    /// `'D'` for a directory, `'F'` for a file. Anything else is a kind elk-herd
    /// did not expect either, and is preserved here rather than rejected — an
    /// unknown kind is exactly the interesting result when probing for presets.
    pub kind: char,
    pub name: String,
}

impl DirEntry {
    pub fn is_dir(&self) -> bool {
        self.kind == 'D'
    }
    pub fn is_file(&self) -> bool {
        self.kind == 'F'
    }
}

#[derive(Debug, PartialEq)]
pub enum DriveError {
    /// A path with a byte this API cannot carry. Paths we send are our own, so
    /// this is a programming error rather than a device condition.
    UnsendablePath(String),
    /// The response ended mid-entry.
    TruncatedEntry { at: usize, need: usize, got: usize },
    /// A `locked` byte that was neither 0 nor 1 — the entry layout is not what
    /// this parser thinks, so the whole listing is suspect.
    NotABooleanByte { at: usize, found: u8 },
    /// An API file opcode outside the read-only allowlist. See
    /// [`assert_read_only_file_op`] — in this namespace 0x57/0x58/0x59 write
    /// and 0x5C deletes.
    NotAReadOnlyFileOp(u8),
    /// An entry's form byte was neither 1 (short) nor 2 (long). The layout is
    /// not what [`parse_list_entries`] derived, so the listing is not readable.
    UnknownEntryForm { at: usize, found: u8 },
    /// The entry walk did not yield the count the reply header declared.
    EntryCountMismatch { expected: u32, got: u32 },
    /// The box refused a file operation and said why. The message is the
    /// device's own — "Invalid sequence number", "Reader did not complete",
    /// "File transfer is not active" — and is carried verbatim because it has
    /// been more useful than anything this end could infer.
    DeviceRefused { what: &'static str, message: String },
    /// A read assembled a different number of bytes than the box said it sent.
    ShortRead { expected: u32, got: usize },
    /// A chunk arrived under a sequence number the reader did not ask for.
    /// Assembling it would produce a plausible, wrong file.
    SequenceOutOfOrder { expected: u32, got: u32 },
    /// The box says the file has already been delivered in full.
    ///
    /// **This is an end-of-file, not a fault**, and it is a distinct variant so
    /// that a reader can match on the type rather than on English. Reading past
    /// the last chunk is refused rather than answered with an empty one, which
    /// is the opposite of what this crate first assumed — see
    /// [`END_OF_TRANSFER`].
    TransferComplete,
    /// No sound container magic anywhere in the file — so this is not a preset
    /// file, or not one this parser recognises at all.
    ///
    /// **Carries the head bytes, and that was learned the hard way.** On
    /// 2026-08-29 a DN2 scan reported `no sound container magic in 407 bytes`
    /// for 388 presets — and 407 is *exactly* the length of a good DN2 preset
    /// file, so the length alone said nothing at all about what had arrived.
    /// Every working capture starts `ac11d303 02000500 …` with the magic at 36;
    /// printing the first bytes is the difference between "a file this parser
    /// does not know" and "not a file at all", and it costs nothing to carry.
    NoContainer { len: usize, head: String },
    /// A container whose head magic is neither a digi's [`SOUND_MAGIC_HEAD`]
    /// nor an A4's [`A4_CONTAINER_MAGIC`] — a fourth box, or a corrupt file.
    ///
    /// **This used to mean "the A4", and stopped meaning that on 2026-08-29.**
    /// The A4 now decodes; see [`decode_drive_preset`]. Anything reached here is
    /// genuinely unrecognised, which is why it carries the magic it found: the
    /// next box to land will be diagnosed from this number.
    UndecodableContainer { magic: u32, at: usize },
    /// An A4 container that could not be sized, because the file around it is
    /// not the shape every A4 capture has: a [`FILE_HEADER_LEN`]-byte header
    /// declaring a payload size, with the container flush against it.
    ///
    /// The A4 has no foot magic, so the declared payload size is the *only*
    /// witness to the struct's extent. When the layout does not hold, that
    /// witness is gone and there is nothing left to fall back on — so this
    /// refuses rather than computing an offset from one box's worth of
    /// evidence.
    UnsizedContainer { at: usize, declared: Option<u16> },
    /// A digi container with no foot magic after its head. The struct's length
    /// is measured by finding the foot, so without one there is no size to
    /// decode at — and guessing is what the foot check exists to prevent.
    NoFootMagic { at: usize },
    /// The container was found and sized and still did not decode.
    NotASound(SoundError),
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriveError::UnsendablePath(p) => {
                write!(f, "path {p:?} has bytes the API cannot carry")
            }
            DriveError::UnsizedContainer { at, declared } => match declared {
                Some(n) => write!(
                    f,
                    "A4 container at {at} is not flush with a {FILE_HEADER_LEN}-byte header \
                     declaring {n} bytes — cannot size it"
                ),
                None => write!(f, "A4 container at {at} has no file header to size it by"),
            },
            DriveError::TruncatedEntry { at, need, got } => write!(
                f,
                "dir entry at byte {at} needs {need} bytes, {got} left — truncated listing"
            ),
            DriveError::NotAReadOnlyFileOp(id) => write!(
                f,
                "API file opcode {id:#04x} is not in the read-only set \
                 (0x53 List, 0x54 Open, 0x55 Read, 0x56 Close) — refusing to send it"
            ),
            DriveError::UnknownEntryForm { at, found } => write!(
                f,
                "list entry form byte at {at} is {found}, not 1 (short) or 2 (long) \
                 — entry layout is not as derived"
            ),
            DriveError::EntryCountMismatch { expected, got } => write!(
                f,
                "listing declared {expected} entries but the walk found {got} \
                 — entry layout is wrong, refusing a partial list"
            ),
            DriveError::NotABooleanByte { at, found } => write!(
                f,
                "locked byte at {at} is {found}, not 0 or 1 — entry layout is not as expected"
            ),
            DriveError::DeviceRefused { what, message } => {
                write!(f, "the box refused {what}: {message}")
            }
            DriveError::TransferComplete => {
                write!(f, "the box says the transfer is already complete")
            }
            DriveError::NoContainer { len, head } => write!(
                f,
                "no sound container magic in {len} bytes — this is not a preset file \
                 (starts {head})"
            ),
            DriveError::UndecodableContainer { magic, at } => write!(
                f,
                "container magic {magic:#010x} at {at} carries no foot magic, so its length \
                 cannot be measured — this is the A4, and decoding it is not supported yet"
            ),
            DriveError::NoFootMagic { at } => write!(
                f,
                "container at {at} has no foot magic after its head — refusing to guess \
                 the struct's length"
            ),
            DriveError::NotASound(e) => write!(f, "container did not decode as a sound: {e}"),
            DriveError::ShortRead { expected, got } => write!(
                f,
                "the box reported sending {expected} bytes and {got} were assembled \
                 — the file is incomplete, refusing it"
            ),
            DriveError::SequenceOutOfOrder { expected, got } => write!(
                f,
                "asked for chunk {expected} and got {got} — refusing to assemble out of order"
            ),
        }
    }
}

impl std::error::Error for DriveError {}

/// The fixed part of a `DirEntry` on the wire: hash, size, locked, kind. The
/// name follows, NUL-terminated and variable-length.
const ENTRY_FIXED: usize = 4 + 4 + 1 + 1;

/// Build the argument bytes for a DirList request: the path as a
/// NUL-terminated Windows-1252 string.
///
/// Only ASCII paths are accepted. The high half of Windows-1252 is a different
/// mapping from UTF-8's, so encoding it properly needs the inverse of
/// [`crate::device::cp1252_char`] — and since every path this crate sends is a
/// literal it wrote itself, refusing is better than encoding it wrongly and
/// listing the wrong directory.
pub fn dir_list_args(path: &str) -> Result<Vec<u8>, DriveError> {
    if !path.is_ascii() || path.contains('\0') {
        return Err(DriveError::UnsendablePath(path.to_string()));
    }
    let mut args = path.as_bytes().to_vec();
    args.push(0);
    Ok(args)
}

/// Parse a DirList response's arguments: entries repeated to the end.
pub fn parse_dir_list(args: &[u8]) -> Result<Vec<DirEntry>, DriveError> {
    let mut entries = Vec::new();
    let mut at = 0usize;
    while at < args.len() {
        // A trailing NUL or two is padding, not the start of an entry.
        if args[at..].iter().all(|&b| b == 0) {
            break;
        }
        if args.len() - at < ENTRY_FIXED + 1 {
            return Err(DriveError::TruncatedEntry {
                at,
                need: ENTRY_FIXED + 1,
                got: args.len() - at,
            });
        }
        let hash = u32_be(args, at);
        let size = u32_be(args, at + 4);
        let locked = match args[at + 8] {
            0 => false,
            1 => true,
            found => return Err(DriveError::NotABooleanByte { at: at + 8, found }),
        };
        let kind = crate::device::cp1252_char(args[at + 9]);
        let (name, next) = cstring(args, at + ENTRY_FIXED);
        entries.push(DirEntry { hash, size, locked, kind, name });
        // `cstring` returns the offset past the terminator; if the string ran
        // to the end unterminated it returns len+1, which would loop forever.
        if next <= at {
            break;
        }
        at = next.min(args.len());
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_bytes(hash: u32, size: u32, locked: bool, kind: u8, name: &str) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&hash.to_be_bytes());
        b.extend_from_slice(&size.to_be_bytes());
        b.push(locked as u8);
        b.push(kind);
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        b
    }

    #[test]
    fn a_path_becomes_a_nul_terminated_string() {
        assert_eq!(dir_list_args("/").expect("ascii"), b"/\0");
        assert_eq!(dir_list_args("/factory").expect("ascii"), b"/factory\0");
    }

    #[test]
    fn a_non_ascii_path_is_refused_rather_than_mis_encoded() {
        assert!(matches!(dir_list_args("/kick\u{e9}"), Err(DriveError::UnsendablePath(_))));
        assert!(matches!(dir_list_args("/a\0b"), Err(DriveError::UnsendablePath(_))));
    }

    #[test]
    fn parses_a_directory_and_a_file() {
        let mut args = entry_bytes(0, 0, false, b'D', "factory");
        args.extend(entry_bytes(0xdead_beef, 4096, true, b'F', "BD BRASSY KICK"));
        let entries = parse_dir_list(&args).expect("parse");
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir());
        assert_eq!(entries[0].name, "factory");
        assert!(entries[1].is_file());
        assert!(entries[1].locked);
        assert_eq!(entries[1].hash, 0xdead_beef);
        assert_eq!(entries[1].size, 4096);
        assert_eq!(entries[1].name, "BD BRASSY KICK");
    }

    #[test]
    fn an_empty_listing_is_no_entries_not_an_error() {
        assert_eq!(parse_dir_list(&[]).expect("parse"), vec![]);
        assert_eq!(parse_dir_list(&[0, 0]).expect("parse"), vec![]);
    }

    /// An unexpected kind byte is preserved. When probing for presets, a kind
    /// that is neither D nor F is a finding, not a parse failure.
    #[test]
    fn an_unknown_kind_is_kept_rather_than_rejected() {
        let args = entry_bytes(1, 2, false, b'P', "SOMETHING");
        let entries = parse_dir_list(&args).expect("parse");
        assert_eq!(entries[0].kind, 'P');
        assert!(!entries[0].is_dir() && !entries[0].is_file());
    }

    #[test]
    fn a_truncated_entry_is_an_error_not_a_panic() {
        let args = vec![0, 0, 0, 1, 0, 0];
        assert!(matches!(parse_dir_list(&args), Err(DriveError::TruncatedEntry { .. })));
    }

    #[test]
    fn a_bad_locked_byte_is_refused() {
        let mut args = entry_bytes(0, 0, false, b'F', "X");
        args[8] = 7;
        assert!(matches!(parse_dir_list(&args), Err(DriveError::NotABooleanByte { found: 7, .. })));
    }

    /// A name that runs to the end of the buffer with no terminator must not
    /// spin the loop.
    #[test]
    fn an_unterminated_name_terminates_the_loop() {
        let mut args = Vec::new();
        args.extend_from_slice(&0u32.to_be_bytes());
        args.extend_from_slice(&0u32.to_be_bytes());
        args.push(0);
        args.push(b'F');
        args.extend_from_slice(b"NO TERMINATOR");
        let entries = parse_dir_list(&args).expect("parse");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "NO TERMINATOR");
    }
}

// ---------------------------------------------------------------------------
// The *other* +Drive file API: 0x53-0x5C on the API mechanism
// ---------------------------------------------------------------------------
//
// Everything above this line implements elk-herd's gen-1 numbering (0x10
// DirList and friends), which a DT2 answers. This section implements the
// second, renumbered file API that a Digitone answers instead.
//
// Documented by Ángel Linares García (DNX) in digi-roll's
// `docs/plus-drive-file-api.md` — measured on real hardware and on a USB
// capture of Elektron's Transfer app. See `CREDITS.md`.
//
// # The trap this section exists to survive
//
// **`0x53` means two different things depending on the header carrying it.**
//
// | header | `0x53` |
// |---|---|
// | dump — `F0 00 20 3C <family> 00 …` | Sound dump, a payload |
// | API — `F0 00 20 3C 10 00 …` | **List**, a directory listing |
//
// A DN2's identity reply advertises `50–5E`. Read as dump response types that
// looks like a rich dump vocabulary and no file API; it is in fact the file
// API's own opcode list. This project read it the wrong way and concluded the
// DN2 had no +Drive before finding the document above — the same error
// digi-roll itself had been carrying.

/// The read-only half of the file API. **This is a positive allowlist, and it
/// is the safety boundary for this module.**
///
/// The old habit — "0x5n opcodes store, so code that never sends 0x5n cannot
/// write" — is a fact about the *dump* namespace and does not transfer here. In
/// this namespace `0x57` WriteOpen, `0x58` Write and `0x59` WriteClose write,
/// and **`0x5C` Delete deletes**. Those four, plus `0x5A` Move and `0x5B` Copy,
/// are deliberately not implemented: reading a preset library needs none of
/// them, and the cheapest way not to delete somebody's +Drive is to own no code
/// that can.
pub const API_FILE_LIST: u8 = 0x53;
pub const API_FILE_OPEN: u8 = 0x54;
pub const API_FILE_READ: u8 = 0x55;
pub const API_FILE_CLOSE: u8 = 0x56;

/// Refuse any file-API opcode outside the read-only set, before it reaches the
/// wire. The counterpart to [`crate::device::assert_request_opcode`] for the API
/// mechanism, which has never had one because nothing before this sent an API
/// opcode that could write.
pub fn assert_read_only_file_op(api_id: u8) -> Result<(), DriveError> {
    match api_id {
        API_FILE_LIST | API_FILE_OPEN | API_FILE_READ | API_FILE_CLOSE => Ok(()),
        _ => Err(DriveError::NotAReadOnlyFileOp(api_id)),
    }
}

/// Build the body of a `0x53` List request: a NUL-terminated path, then a
/// `u32be` start index and a `u32be` count.
///
/// `start = 0, count = 0` asks for the whole listing. **Do not invent a
/// non-zero `start`**: per the source document, a start the device did not
/// itself hand back returns zero entries — on `/soundbanks/H` (236 entries) and
/// `/kits/A` (96) alike. Page with the cursor from [`ListReply::next_cursor`].
pub fn list_request_args(path: &str, start: u32, count: u32) -> Result<Vec<u8>, DriveError> {
    if !path.is_ascii() || path.contains('\0') {
        return Err(DriveError::UnsendablePath(path.to_string()));
    }
    let mut args = path.as_bytes().to_vec();
    args.push(0);
    args.extend_from_slice(&start.to_be_bytes());
    args.extend_from_slice(&count.to_be_bytes());
    Ok(args)
}

/// A decoded `0x53` List reply.
///
/// The header is fully specified by the source document; the per-entry layout is
/// **not** — that document withheld a populated capture on purpose, because the
/// entries carry real project and preset names. So [`ListReply::entry_bytes`] is
/// handed back raw rather than parsed into a guess. What the document does
/// state about an entry's *trailing* 12 bytes is recorded on
/// [`ENTRY_TRAIL_NOTE`].
#[derive(Debug, Clone, PartialEq)]
pub struct ListReply {
    /// Leading status byte: `0x01` ok, `0x00` failure.
    pub ok: bool,
    /// On failure, the device's own NUL-terminated message — e.g. "Invalid path".
    pub message: Option<String>,
    /// The start index echoed back.
    pub start: u32,
    /// The cursor to pass as the next request's `start`. Paging wants this
    /// value and not an arbitrary one.
    pub next_cursor: u32,
    /// How many entries follow.
    pub count: u32,
    /// The entry region, undecoded. See the struct docs.
    pub entry_bytes: Vec<u8>,
}

/// What the source document says about the tail of a long-form entry (files and
/// bank directories), kept as prose because it is not enough to parse an entry
/// from and guessing the rest would be worse than not trying:
///
/// ```text
///   u32be index
///   u32be size          constant across a collection
///   u16be permissions
///   u8 u8               occupancy pair — 01 01 occupied, 00 01 empty on kits
/// ```
///
/// Read occupancy from **the pair**, never one byte: a single-byte read was
/// once mistaken for a tag mask, and empty is `00 01` rather than `00 00` on at
/// least one collection type. Two entry layouts exist, distinguished by a
/// per-entry byte.
pub const ENTRY_TRAIL_NOTE: &str = "see ENTRY_TRAIL_NOTE docs";

/// Parse a `0x53` List reply body.
pub fn parse_list_reply(args: &[u8]) -> Result<ListReply, DriveError> {
    if args.is_empty() {
        return Err(DriveError::TruncatedEntry { at: 0, need: 1, got: 0 });
    }
    match args[0] {
        0x00 => {
            let (message, _) = cstring(args, 1);
            Ok(ListReply {
                ok: false,
                message: Some(message),
                start: 0,
                next_cursor: 0,
                count: 0,
                entry_bytes: Vec::new(),
            })
        }
        0x01 => {
            if args.len() < 13 {
                return Err(DriveError::TruncatedEntry { at: 1, need: 13, got: args.len() });
            }
            Ok(ListReply {
                ok: true,
                message: None,
                start: u32_be(args, 1),
                next_cursor: u32_be(args, 5),
                count: u32_be(args, 9),
                entry_bytes: args[13..].to_vec(),
            })
        }
        found => Err(DriveError::NotABooleanByte { at: 0, found }),
    }
}

#[cfg(test)]
mod file_api_tests {
    use super::*;

    #[test]
    fn a_list_request_carries_path_then_two_u32s() {
        assert_eq!(
            list_request_args("/projects", 7, 8).expect("ascii"),
            b"/projects\0\x00\x00\x00\x07\x00\x00\x00\x08".to_vec()
        );
        // The whole-listing form from the source document's capture.
        assert_eq!(
            list_request_args("/projects", 0, 0).expect("ascii"),
            b"/projects\0\x00\x00\x00\x00\x00\x00\x00\x00".to_vec()
        );
    }

    /// The empty-listing reply quoted in the source document, byte for byte.
    #[test]
    fn parses_the_documented_empty_listing() {
        let body = [0x01, 0, 0, 0, 0x1c, 0, 0, 0, 0x1c, 0, 0, 0, 0x00];
        let reply = parse_list_reply(&body).expect("parse");
        assert!(reply.ok);
        assert_eq!(reply.start, 28);
        assert_eq!(reply.next_cursor, 28);
        assert_eq!(reply.count, 0);
        assert!(reply.entry_bytes.is_empty());
    }

    /// The error reply from the same document: status 0x00 then "Invalid path".
    #[test]
    fn parses_the_documented_error_reply() {
        let mut body = vec![0x00];
        body.extend_from_slice(b"Invalid path\0");
        let reply = parse_list_reply(&body).expect("parse");
        assert!(!reply.ok);
        assert_eq!(reply.message.as_deref(), Some("Invalid path"));
    }

    #[test]
    fn entries_come_back_raw_rather_than_guessed_at() {
        let mut body = vec![0x01, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 2];
        body.extend_from_slice(&[0xaa, 0xbb, 0xcc]);
        let reply = parse_list_reply(&body).expect("parse");
        assert_eq!(reply.count, 2);
        assert_eq!(reply.entry_bytes, vec![0xaa, 0xbb, 0xcc]);
    }

    /// The safety boundary: the writing and deleting opcodes are refused here,
    /// not merely unimplemented.
    #[test]
    fn the_write_and_delete_ops_are_refused() {
        for read_op in [API_FILE_LIST, API_FILE_OPEN, API_FILE_READ, API_FILE_CLOSE] {
            assert!(assert_read_only_file_op(read_op).is_ok(), "{read_op:#04x} should be allowed");
        }
        // 0x57 WriteOpen, 0x58 Write, 0x59 WriteClose, 0x5A Move, 0x5B Copy, 0x5C Delete
        for bad in [0x57u8, 0x58, 0x59, 0x5a, 0x5b, 0x5c, 0x5d, 0x5e, 0x10, 0x00] {
            assert!(
                matches!(assert_read_only_file_op(bad), Err(DriveError::NotAReadOnlyFileOp(_))),
                "{bad:#04x} must be refused"
            );
        }
    }
}

/// One entry in a `0x53` List reply.
///
/// Layout derived from real listings on a DT2 (1.15C) and DN2 (1.10E),
/// 2026-08-26, on top of the trailing-field description in
/// `docs/plus-drive-file-api.md`. An entry is a NUL-terminated name followed by
/// a fixed tail, and **the second tail byte says how long the tail is**:
///
/// ```text
///   name\0
///   +0  u8      0x00 file, 0x01 directory
///   +1  u8      0x01 short form (6-byte tail), 0x02 long form (14-byte tail)
///   short form (top-level directories):
///   +2  u32be   number of children
///   long form (files and bank directories):
///   +2  u32be   index, one-based
///   +6  u32be   size — a fixed allocation, constant across a collection
///   +10 u16be   permissions
///   +12 u8 u8   occupancy pair
/// ```
///
/// What pinned the long tail at 14: `/soundbanks/F` and `/soundbanks/G` on the
/// DN2 are entirely empty, so every name is the empty string and the reply
/// divides exactly — 3840 bytes / 256 entries = 15 = one NUL plus 14.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub name: String,
    pub is_dir: bool,
    /// One-based slot number for a long-form entry; `None` on the short form.
    pub index: Option<u32>,
    /// Allocation size for a long-form entry; `None` on the short form.
    pub size: Option<u32>,
    pub permissions: Option<u16>,
    /// The occupancy pair, verbatim. **Compare the pair, never one byte.**
    /// `(1, 1)` is occupied. Empty is `(0, 0)` under `/projects` and `(0, 1)`
    /// under `/kits` — which is why [`ListEntry::is_occupied`] tests for the
    /// occupied value rather than for either flavour of empty.
    pub occupancy: Option<(u8, u8)>,
    /// Child count, short form only.
    pub children: Option<u32>,
}

impl ListEntry {
    /// Whether the slot holds anything. Defined as "the pair is (1, 1)".
    pub fn is_occupied(&self) -> bool {
        self.occupancy == Some((1, 1))
    }
}

/// Parse the entry region of a [`ListReply`].
///
/// `expected` is [`ListReply::count`]. It is checked rather than trusted: if the
/// walk does not yield exactly that many entries and land exactly on the end of
/// the region, the layout assumption is wrong and this returns an error instead
/// of a plausible-looking partial list. That check is the whole reason this is
/// safe to rely on — the layout was derived from captures, not from a spec.
pub fn parse_list_entries(bytes: &[u8], expected: u32) -> Result<Vec<ListEntry>, DriveError> {
    let mut entries = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let (name, after_name) = cstring(bytes, at);
        if after_name + 2 > bytes.len() {
            return Err(DriveError::TruncatedEntry {
                at,
                need: after_name + 2 - at,
                got: bytes.len() - at,
            });
        }
        let is_dir = match bytes[after_name] {
            0 => false,
            1 => true,
            found => return Err(DriveError::NotABooleanByte { at: after_name, found }),
        };
        let form = bytes[after_name + 1];
        let tail = match form {
            1 => 6,
            2 => 14,
            found => return Err(DriveError::UnknownEntryForm { at: after_name + 1, found }),
        };
        if after_name + tail > bytes.len() {
            return Err(DriveError::TruncatedEntry {
                at,
                need: after_name + tail - at,
                got: bytes.len() - at,
            });
        }
        let body = after_name + 2;
        entries.push(if form == 1 {
            ListEntry {
                name,
                is_dir,
                index: None,
                size: None,
                permissions: None,
                occupancy: None,
                children: Some(u32_be(bytes, body)),
            }
        } else {
            ListEntry {
                name,
                is_dir,
                index: Some(u32_be(bytes, body)),
                size: Some(u32_be(bytes, body + 4)),
                permissions: Some(u16_be(bytes, body + 8)),
                occupancy: Some((bytes[body + 10], bytes[body + 11])),
                children: None,
            }
        });
        at = after_name + tail;
    }
    if entries.len() as u32 != expected {
        return Err(DriveError::EntryCountMismatch { expected, got: entries.len() as u32 });
    }
    Ok(entries)
}

#[cfg(test)]
mod list_entry_tests {
    use super::*;

    /// The DT2's root listing, byte for byte off the wire 2026-08-26. Three
    /// short-form directories, and the child counts are real: /projects has 128,
    /// /soundbanks and /kits have 8 banks each.
    #[test]
    fn parses_the_captured_root_listing() {
        let bytes = [
            0x70, 0x72, 0x6f, 0x6a, 0x65, 0x63, 0x74, 0x73, 0x00, 0x01, 0x01, 0, 0, 0, 0x80,
            0x73, 0x6f, 0x75, 0x6e, 0x64, 0x62, 0x61, 0x6e, 0x6b, 0x73, 0x00, 0x01, 0x01, 0, 0,
            0, 0x08, 0x6b, 0x69, 0x74, 0x73, 0x00, 0x01, 0x01, 0, 0, 0, 0x08,
        ];
        assert_eq!(bytes.len(), 43, "the reply was 43 entry bytes");
        let entries = parse_list_entries(&bytes, 3).expect("parse");
        let named: Vec<(&str, bool, Option<u32>)> =
            entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.children)).collect();
        assert_eq!(
            named,
            vec![
                ("projects", true, Some(128)),
                ("soundbanks", true, Some(8)),
                ("kits", true, Some(8)),
            ]
        );
    }

    /// The head of the DT2's `/soundbanks/A`, off the wire. Long-form file
    /// entries: a real preset name, a one-based index, and the 1114-byte
    /// allocation that matches a DT2 sound container.
    #[test]
    fn parses_captured_soundbank_entries() {
        let bytes = [
            0x41, 0x43, 0x49, 0x44, 0x44, 0x00, 0x00, 0x02, 0, 0, 0, 0x01, 0, 0, 0x04, 0x5a,
            0x00, 0x12, 0x01, 0x01, 0x42, 0x41, 0x4d, 0x20, 0x42, 0x41, 0x53, 0x53, 0x00, 0x00,
            0x02, 0, 0, 0, 0x02, 0, 0, 0x04, 0x5a, 0x00, 0x12, 0x01, 0x01,
        ];
        let entries = parse_list_entries(&bytes, 2).expect("parse");
        assert_eq!(entries[0].name, "ACIDD");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].index, Some(1));
        assert_eq!(entries[0].size, Some(1114));
        assert_eq!(entries[0].permissions, Some(0x0012));
        assert!(entries[0].is_occupied());
        assert_eq!(entries[1].name, "BAM BASS");
        assert_eq!(entries[1].index, Some(2));
        assert!(entries[1].is_occupied());
    }

    /// An empty bank: every name is the empty string, so the region divides
    /// exactly by 15. This is the case that fixed the long tail at 14 bytes.
    #[test]
    fn an_empty_bank_divides_exactly_by_fifteen() {
        let one = [0x00u8, 0x00, 0x02, 0, 0, 0, 0x01, 0, 0, 0x04, 0x5a, 0x00, 0x12, 0x00, 0x00];
        assert_eq!(one.len(), 15);
        let mut bytes = Vec::new();
        for _ in 0..256 {
            bytes.extend_from_slice(&one);
        }
        assert_eq!(bytes.len(), 3840, "the DN2's empty banks F and G were 3840 bytes");
        let entries = parse_list_entries(&bytes, 256).expect("parse");
        assert_eq!(entries.len(), 256);
        assert!(entries.iter().all(|e| e.name.is_empty() && !e.is_occupied()));
    }

    /// Both flavours of empty, and why `is_occupied` tests for the occupied
    /// value rather than against one of them.
    #[test]
    fn occupancy_is_read_as_a_pair() {
        let mk = |a: u8, b: u8| {
            let mut v = vec![0x58u8, 0x00, 0x00, 0x02, 0, 0, 0, 0x01, 0, 0, 0, 0x10, 0x00, 0x12];
            v.push(a);
            v.push(b);
            v
        };
        assert!(parse_list_entries(&mk(1, 1), 1).unwrap()[0].is_occupied());
        // (0,0) is how /projects reports empty; (0,1) is how /kits does.
        assert!(!parse_list_entries(&mk(0, 0), 1).unwrap()[0].is_occupied());
        assert!(!parse_list_entries(&mk(0, 1), 1).unwrap()[0].is_occupied());
    }

    /// The self-check: a count that disagrees with the walk means the layout is
    /// wrong, and a wrong layout must not return a plausible partial list.
    #[test]
    fn a_count_mismatch_is_an_error_not_a_short_list() {
        let bytes = [0x00u8, 0x00, 0x02, 0, 0, 0, 0x01, 0, 0, 0, 0x10, 0x00, 0x12, 0x01, 0x01];
        assert!(parse_list_entries(&bytes, 1).is_ok());
        assert!(matches!(
            parse_list_entries(&bytes, 9),
            Err(DriveError::EntryCountMismatch { expected: 9, got: 1 })
        ));
    }

    #[test]
    fn an_unknown_form_byte_is_refused() {
        let bytes = [0x58u8, 0x00, 0x00, 0x07, 0, 0, 0, 0x01];
        assert!(matches!(
            parse_list_entries(&bytes, 1),
            Err(DriveError::UnknownEntryForm { found: 7, .. })
        ));
    }
}

// ---------------------------------------------------------------------------
// Reading a file: 0x54 Open, 0x55 Read, 0x56 Close
// ---------------------------------------------------------------------------
//
// The source document names these three and specifies the argument layout of
// none of them — only List's body is written down. Everything below was derived
// by `crates/app/examples/probe_drive_read.rs` against a DT2 (0071/1.15C), a DN2
// (0050/1.10E) and an A4 (0195/1.55B) on 2026-08-28, and each claim is the same
// answer from all three boxes unless it says otherwise.
//
// # What the probe settled, and what it cost to learn
//
// **Read addresses a chunk by sequence number, not by byte offset.** elk-herd's
// gen-1 `0x32` FileRead takes `(fd, len, start)`; sending that shape here is
// refused with the box's own words, `Invalid sequence number`. That is the one
// place the renumbered API genuinely differs from the gen-1 one rather than
// being it with new opcodes, and the document's wording was right.
//
// **`seq = 0` is not the first chunk.** It answers with a zero-length body and a
// differently-shaped header, so it is metadata. A file starts at `seq = 1`.
//
// **The chunk size is negotiable, and the default is 16 bytes.** A path-only
// Open answers `chunk = 16`; a path followed by a `u32be` answers with the size
// asked for. Sixteen bytes is 27,000 round trips for one DN2 bank, so
// [`READ_CHUNK`] is not an optimisation.
//
// **A box runs one transfer job at a time.** A second Open voids the first, and
// the first `fd` then answers `File transfer is not active`. Nothing here may
// interleave two reads on one device.
//
// **The reader is a state machine.** Close refuses with `Reader did not
// complete` until the read has reached the end of the file — so an abandoned
// read has to be finished or the job is stuck, and a successful Close is
// evidence the read was whole.

/// The chunk size to ask Open for. Large enough that every preset seen so far
/// arrives in one Read, which keeps a bank scan to one round trip per file.
pub const READ_CHUNK: u32 = 4096;

/// The chunk size a path-only Open selects. Recorded because it is the cost of
/// forgetting the argument, not because anything should use it.
pub const DEFAULT_CHUNK: u32 = 16;

/// Where a Read reply's data begins. `ok(1) fd(4) seq(4) …(4) pad(1) …(4) len(4)`.
const READ_DATA_OFFSET: usize = 22;

/// A file's own header, which every box writes identically ahead of the
/// container: magic, the OS build that wrote it, and the payload size.
pub const FILE_MAGIC: u32 = 0xAC11_D303;

/// Where the payload size sits in that header, as a `u16be`. Confirmed on three
/// boxes against three different sizes — 1114, 364 and 366 — each of which is
/// also what the directory listing declared for the same file.
pub const FILE_SIZE_OFFSET: usize = 27;

/// How long that header is, before the payload starts. Measured across all 24
/// committed captures on 2026-08-29 and identical on all three boxes — see
/// [`container_offset`] for the whole layout and for why the digis' container
/// still lands five bytes later than the A4's.
pub const FILE_HEADER_LEN: usize = 31;

/// The trailer after the payload: four checksum-shaped bytes, the payload
/// length again, then magic `AAA1DAAA`. The header declares this length too, as
/// a `u16be` at [`FILE_SIZE_OFFSET`]` + 2`, and it reads 12 on every capture.
///
/// The four leading bytes are **not** a zlib crc32 of the payload under a zero
/// seed, which was the obvious guess given the read path's crc32 is
/// zero-seeded. Unidentified, and nothing consumes it.
pub const FILE_TRAILER_LEN: usize = 12;

/// Build the body of a `0x54` Open: a NUL-terminated path, then an optional
/// `u32be` chunk size.
///
/// Pass a chunk. The trailing word is optional to the box and not to anything
/// that cares how long a bank takes — see [`DEFAULT_CHUNK`].
pub fn open_request_args(path: &str, chunk: Option<u32>) -> Result<Vec<u8>, DriveError> {
    if !path.is_ascii() || path.contains('\0') {
        return Err(DriveError::UnsendablePath(path.to_string()));
    }
    let mut args = path.as_bytes().to_vec();
    args.push(0);
    if let Some(chunk) = chunk {
        args.extend_from_slice(&chunk.to_be_bytes());
    }
    Ok(args)
}

/// Build the body of a `0x55` Read: the `fd` Open handed back, then the chunk's
/// sequence number. **Sequence numbers start at 1** — see the section header.
pub fn read_request_args(fd: u32, seq: u32) -> Vec<u8> {
    let mut args = fd.to_be_bytes().to_vec();
    args.extend_from_slice(&seq.to_be_bytes());
    args
}

/// Build the body of a `0x56` Close: the `fd`, alone.
pub fn close_request_args(fd: u32) -> Vec<u8> {
    fd.to_be_bytes().to_vec()
}

/// A decoded `0x54` Open reply.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenReply {
    /// The handle every later Read and the Close must carry.
    pub fd: u32,
    /// The chunk size the box actually chose, which is not necessarily the one
    /// asked for and is the number a reader should page by.
    pub chunk: u32,
}

/// A decoded `0x55` Read reply.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadReply {
    pub fd: u32,
    /// The sequence number echoed back. Checked by the reader: a chunk arriving
    /// under the wrong number would assemble a plausible, wrong file.
    pub seq: u32,
    pub data: Vec<u8>,
    /// Two `u32be` fields that are in every Read reply and are **not
    /// identified**, kept rather than skipped so a later session has them.
    ///
    /// The document says a read reports one checksum per chunk, `crc32` seeded
    /// with zero rather than all-ones. Neither field is that: measured against
    /// the chunk's own bytes, under both seeds, at every offset in the header,
    /// nothing matched. So the checksum is either over something other than the
    /// plain chunk bytes or it is not here at all, and calling either field
    /// `checksum` would be a guess wearing a name.
    pub unidentified: [u32; 2],
}

/// A decoded `0x56` Close reply. The total length is the box's own count of
/// what it sent, and the reader compares it against what it assembled.
#[derive(Debug, Clone, PartialEq)]
pub struct CloseReply {
    pub fd: u32,
    pub total_len: u32,
}

/// The leading `ok` byte and, on a refusal, the NUL-terminated message the box
/// supplies with it — "Invalid sequence number", "Reader did not complete",
/// "File transfer is not active", all seen.
///
/// Every reply in this family starts this way, and the message is worth
/// surfacing verbatim: it has been more informative than anything this end
/// could infer.
/// The box's word for "you have already had the whole file".
///
/// **Matching on this string is not a shortcut, it is the only signal there
/// is.** A read past the end is refused rather than answered with a zero-length
/// chunk, so there is no length, flag or empty body to terminate on — the first
/// version of this reader waited for an empty chunk and every read failed on
/// hardware while its unit tests passed. The string is confined to this one
/// place and turned into [`DriveError::TransferComplete`] immediately, so no
/// caller ever compares it.
pub const END_OF_TRANSFER: &str = "File transfer is complete";

fn check_ok(args: &[u8], what: &'static str) -> Result<(), DriveError> {
    match args.first() {
        Some(0x01) => Ok(()),
        Some(_) => {
            let text: Vec<u8> = args[1..].iter().copied().take_while(|&b| b != 0).collect();
            let message = String::from_utf8_lossy(&text).into_owned();
            if message == END_OF_TRANSFER {
                return Err(DriveError::TransferComplete);
            }
            Err(DriveError::DeviceRefused { what, message })
        }
        None => Err(DriveError::TruncatedEntry { at: 0, need: 1, got: 0 }),
    }
}

fn need_u32(args: &[u8], at: usize) -> Result<u32, DriveError> {
    args.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(DriveError::TruncatedEntry { at, need: at + 4, got: args.len() })
}

/// Parse a `0x54` Open reply.
pub fn parse_open_reply(args: &[u8]) -> Result<OpenReply, DriveError> {
    check_ok(args, "Open")?;
    Ok(OpenReply { fd: need_u32(args, 1)?, chunk: need_u32(args, 5)? })
}

/// Parse a `0x55` Read reply, data included.
pub fn parse_read_reply(args: &[u8]) -> Result<ReadReply, DriveError> {
    check_ok(args, "Read")?;
    let fd = need_u32(args, 1)?;
    let seq = need_u32(args, 5)?;
    let a = need_u32(args, 9)?;
    let b = need_u32(args, 14)?;
    let len = need_u32(args, 18)? as usize;
    let data = args
        .get(READ_DATA_OFFSET..READ_DATA_OFFSET + len)
        .ok_or(DriveError::TruncatedEntry {
            at: READ_DATA_OFFSET,
            need: READ_DATA_OFFSET + len,
            got: args.len(),
        })?
        .to_vec();
    Ok(ReadReply { fd, seq, data, unidentified: [a, b] })
}

/// Parse a `0x56` Close reply.
pub fn parse_close_reply(args: &[u8]) -> Result<CloseReply, DriveError> {
    check_ok(args, "Close")?;
    Ok(CloseReply { fd: need_u32(args, 1)?, total_len: need_u32(args, 5)? })
}

/// The payload size a file declares in its own header, which should agree with
/// the size the directory listing gave for it.
///
/// Worth checking rather than trusting one of the two: they come from different
/// places in the box, and a disagreement means this parser is reading a file it
/// does not understand.
pub fn file_declared_size(file: &[u8]) -> Option<u16> {
    if need_u32(file, 0).ok()? != FILE_MAGIC {
        return None;
    }
    file.get(FILE_SIZE_OFFSET..FILE_SIZE_OFFSET + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}

/// Where the sound container starts inside a file, found by its magic rather
/// than by a fixed header length.
///
/// **A constant would be wrong**, though not for the reason first recorded. The
/// container sits at 36 on a DT2 and a DN2 and at 31 on an A4, and the original
/// note here read that as two different header lengths. Measuring all 24
/// captures on 2026-08-29 says otherwise:
///
/// ```text
///   31-byte header | payload (= file_declared_size) | 12-byte trailer
///                                                     crc?  len  AAA1DAAA
/// ```
///
/// **The header is 31 bytes on all three boxes.** The digis' container is five
/// bytes further in because their payload opens with a five-byte wrapper — the
/// same [`crate::sound::SOUND_WRAPPER`] that a `0x6b` kit-track-sound payload
/// carries in front of its struct. The A4 has no wrapper, so its container is
/// flush with the start of its payload.
///
/// So the offset still must not be a constant, but what varies is the presence
/// of a wrapper rather than the size of a header — and the magic still varies
/// too, `BEEFBACE` against `BEEFBABA`. Searching for the magic covers both
/// without needing to know which box answered.
pub fn container_offset(file: &[u8]) -> Option<usize> {
    file.windows(4).position(|w| {
        w == [0xbe, 0xef, 0xba, 0xce] || w == [0xbe, 0xef, 0xba, 0xba]
    })
}

/// The A4's container magic, where the digis use [`SOUND_MAGIC_HEAD`].
///
/// Named so that [`decode_drive_preset`] can *route* on it. It was introduced
/// so the A4 could be refused as itself rather than as a corrupt digi file;
/// since 2026-08-29 it selects the sizing rule instead, which is the same
/// distinction put to better use.
pub const A4_CONTAINER_MAGIC: u32 = 0xBEEF_BABA;

/// How long the sound struct at `body`'s front is, found by locating its foot
/// magic.
///
/// **A size table cannot do this job.** `KNOWN_SOUND_SIZES` was written when the
/// size looked like a per-box constant; the 2026-08-29 capture shows one DN2
/// bank holding both 319 and 359, tracking the word at `+4`. The foot needs no
/// table and no per-box knowledge: it is the end of the struct, so finding it
/// *is* measuring it.
///
/// The search starts past the name field, so a foot cannot be "found" inside
/// the header it would have to precede.
fn struct_size(body: &[u8]) -> Option<usize> {
    let smallest = SOUND_NAME_OFFSET + 16 + 4;
    let foot = SOUND_MAGIC_FOOT.to_be_bytes();
    body.windows(4)
        .enumerate()
        .skip(smallest.saturating_sub(4))
        .find(|(_, w)| *w == foot)
        .map(|(at, _)| at + 4)
}

/// The first bytes of a file, as hex, for an error that has to describe
/// something it could not parse.
///
/// Sixteen bytes: enough to show the 36-byte head's opening — every good capture
/// begins `ac11d303 02000500 0f303035 30…`, the box's own build string included —
/// and short enough to sit in a one-line message on a 330px panel.
fn head_hex(file: &[u8]) -> String {
    if file.is_empty() {
        return String::from("nothing");
    }
    file.iter().take(16).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join("")
}

/// Decode a whole +Drive preset file into the [`Sound`] it contains.
///
/// This is the container layer PLAN.md §10.2 is about, and the thing standing
/// between a preset listing and a tag index: `0x54`/`0x55`/`0x56` return a
/// file, and until this existed nothing turned one into a sound.
///
/// The file is a header, then a sound container, then a 43-byte tail this does
/// not interpret. Neither the header's length nor the struct's is a constant —
/// 36 bytes of header on the digis and 31 on an A4, 299/319/359 of struct — so
/// both are *found*: the container by its magic, the struct by its foot.
///
/// # The A4 takes the other branch, and is sized by its header
///
/// Its container announces itself with [`A4_CONTAINER_MAGIC`] and **carries no
/// foot magic at all** — not once in any of the eight files captured on
/// 2026-08-29. [`decode_sound`] leans on the foot, so for three days this
/// returned [`DriveError::UndecodableContainer`] rather than guess an extent.
///
/// **That refusal is retired, and the reasoning behind it was wrong twice over.**
///
/// The first mis-framing was the extent. The foot's actual job is to validate a
/// *guessed* size: [`struct_size`] finds the end of the struct by searching for
/// it, and the magic landing is what proves the search was right. The A4 needs
/// no search. Its file header declares the payload length, its container is
/// flush with the start of that payload, and so the struct's extent is stated
/// rather than inferred. A declared length is a **better** witness than a
/// found one, not a weaker substitute for it — so skipping the foot check here
/// gives up nothing. What this branch checks instead is that the layout the
/// declaration depends on actually holds; when it does not, it refuses with
/// [`DriveError::UnsizedContainer`] rather than reaching for a fallback.
///
/// The second, and the one that actually blocked it, was the tag mask.
/// `sound::TAG_NAMES` had only ever been calibrated on a DN2, and an A4's masks
/// differ in character from every digi capture — low bits set, which no digi
/// file shows. Reading them through a digi's table would have been guessing at
/// a field, which is what PLAN.md §9 exists to stop. It is no longer a guess:
/// [`crate::sound::TAG_NAMES_A4`] is calibrated, and
/// [`crate::sound::tag_names_for`] makes the table follow the box.
///
/// So an A4 preset now **browses and tags** like a digi's. It still never
/// **loads onto a track**, because the A4 answers no `0x6x` dump request and so
/// has no `0x6b`, no `0x5b`, and no load path in this codebase — a complete and
/// honest v1 for that box rather than a gap.
pub fn decode_drive_preset(file: &[u8]) -> Result<Sound, DriveError> {
    let at = container_offset(file)
        .ok_or_else(|| DriveError::NoContainer { len: file.len(), head: head_hex(file) })?;
    let body = &file[at..];
    let magic = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    match magic {
        SOUND_MAGIC_HEAD => {
            let size = struct_size(body).ok_or(DriveError::NoFootMagic { at })?;
            decode_sound(body, size).map_err(DriveError::NotASound)
        }
        A4_CONTAINER_MAGIC => {
            // The declared payload length is the struct length only while the
            // container is flush with the payload — so both are checked, and
            // neither is worked around.
            let declared = file_declared_size(file);
            match declared {
                Some(size) if at == FILE_HEADER_LEN => {
                    decode_a4_sound(body, size as usize).map_err(DriveError::NotASound)
                }
                _ => Err(DriveError::UnsizedContainer { at, declared }),
            }
        }
        // **Unreachable today, and deliberately kept.** `container_offset`
        // searches for exactly these two magics, so the magic at `at` is
        // always one of them. This arm is the guard on the *next* box: adding
        // a third pattern to that search without adding a branch here lands
        // an honest error instead of a silent misparse, and the error carries
        // the magic so whoever hits it knows what to write.
        _ => Err(DriveError::UndecodableContainer { magic, at }),
    }
}

#[cfg(test)]
mod file_read_tests {
    use super::*;

    // Every reply below is a verbatim capture from `probe_drive_read` on
    // 2026-08-28 — a DT2 (0071/1.15C) and a DN2 (0050/1.10E). None of them
    // carries a preset name: the Read capture's payload is the file's own
    // header, which holds the OS build and nothing a user wrote.
    //
    // **That is a property of these captures, not a rule, and it was narrowed
    // on 2026-08-29.** The restraint it came from was about a *listing* the
    // source document's author withheld — someone else's data, withheld by
    // them. It was never a rule about this desk's own boxes. Whole preset
    // files, names and tag masks included, are captured and committed under
    // `tests/fixtures/drive/` by `capture_drive_presets.rs`, because a tag
    // index cannot be derived from files with the tags taken out. Owner's
    // decision, taken knowingly.

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn open_reports_the_chunk_size_it_chose() {
        // Asked for 4096 and agreed to it.
        let reply = parse_open_reply(&hex("01 00 00 00 05 00 00 10 00 00")).unwrap();
        assert_eq!(reply, OpenReply { fd: 5, chunk: 4096 });
    }

    #[test]
    fn a_path_only_open_selects_sixteen_bytes() {
        // The reason `open_request_args` takes a chunk and callers pass one: a
        // DN2 bank is 1,189 files, and at 16 bytes a file that is 27,000 round
        // trips rather than 1,189.
        let reply = parse_open_reply(&hex("01 00 00 00 01 00 00 00 10 00")).unwrap();
        assert_eq!(reply, OpenReply { fd: 1, chunk: DEFAULT_CHUNK });
    }

    #[test]
    fn read_carries_its_sequence_number_and_its_chunk() {
        let reply = parse_read_reply(&hex(
            "01 00 00 00 02 00 00 00 01 00 00 00 69 00 6d 94 b9 ac 00 00 00 10 \
             ac 11 d3 03 02 00 05 00 0f 30 30 35 30 00 00 00",
        ))
        .unwrap();
        assert_eq!(reply.fd, 2);
        assert_eq!(reply.seq, 1);
        assert_eq!(reply.data.len(), 16);
        // The chunk is the front of the file, so it opens with the file magic
        // and carries the DN2's OS build as ASCII.
        assert_eq!(u32::from_be_bytes([reply.data[0], reply.data[1], reply.data[2], reply.data[3]]), FILE_MAGIC);
        assert_eq!(&reply.data[9..13], b"0050");
        // Kept, not understood — see the field's docs.
        assert_eq!(reply.unidentified, [0x69, 0x6d94_b9ac]);
    }

    #[test]
    fn close_reports_the_length_it_sent() {
        let reply = parse_close_reply(&hex("01 00 00 00 1a 00 00 04 85")).unwrap();
        assert_eq!(reply, CloseReply { fd: 26, total_len: 1157 });
    }

    #[test]
    fn a_refusal_carries_the_boxs_own_words() {
        // The two refusals this API answers with, both earned: the first by
        // sending elk-herd's (fd, len, start) instead of (fd, seq), the second
        // by closing a reader that had not reached the end of the file.
        let mut invalid_seq = vec![0x00];
        invalid_seq.extend_from_slice(b"Invalid sequence number\0");
        assert!(matches!(
            parse_read_reply(&invalid_seq),
            Err(DriveError::DeviceRefused { what: "Read", ref message })
                if message == "Invalid sequence number"
        ));

        let mut not_complete = vec![0x00];
        not_complete.extend_from_slice(b"Reader did not complete\0");
        assert!(matches!(
            parse_close_reply(&not_complete),
            Err(DriveError::DeviceRefused { what: "Close", ref message })
                if message == "Reader did not complete"
        ));
    }

    #[test]
    fn reading_past_the_end_is_an_end_of_file_and_not_a_fault() {
        // The bug this test exists for: the reader waited for a zero-length
        // chunk that a box never sends. Its unit tests passed and every read
        // failed on hardware — `DEVELOPMENT.md` lesson 1, and the cheapest
        // possible version of it, since the box says exactly what is wrong.
        let mut complete = vec![0x00];
        complete.extend_from_slice(END_OF_TRANSFER.as_bytes());
        complete.push(0);
        assert!(matches!(parse_read_reply(&complete), Err(DriveError::TransferComplete)));

        // And it stays distinguishable from the other refusal with "complete"
        // in it, which is a real failure and must not read as an end of file.
        let mut not_complete = vec![0x00];
        not_complete.extend_from_slice(b"Reader did not complete\0");
        assert!(matches!(
            parse_close_reply(&not_complete),
            Err(DriveError::DeviceRefused { .. })
        ));
    }

    #[test]
    fn the_request_bodies_are_what_the_box_accepted() {
        assert_eq!(open_request_args("/soundbanks/A/1", None).unwrap(), b"/soundbanks/A/1\0");
        assert_eq!(
            open_request_args("/kits/A/1", Some(4096)).unwrap(),
            [b"/kits/A/1\0".as_slice(), &[0, 0, 0x10, 0]].concat()
        );
        assert_eq!(read_request_args(2, 1), hex("00 00 00 02 00 00 00 01"));
        assert_eq!(close_request_args(26), hex("00 00 00 1a"));
    }

    #[test]
    fn a_file_declares_its_own_payload_size() {
        // A real DT2 file header, container magic onwards trimmed off. The
        // listing said 1114 bytes for the same file, and the two agreeing is
        // the check — they come from different places in the box.
        let header = hex(
            "ac 11 d3 03 02 00 04 00 10 30 30 37 31 00 00 00 03 00 00 00 \
             00 00 00 00 00 00 00 04 5a 00 0c 00 00 00 00 00",
        );
        assert_eq!(file_declared_size(&header), Some(1114));
        // Not a +Drive file at all: no magic, no answer.
        assert_eq!(file_declared_size(&[0; 36]), None);
    }

    #[test]
    fn the_container_is_found_by_its_magic_and_both_magics_count() {
        // 36 on the digis, 31 on the A4 — and the A4's magic is BEEFBABA. A
        // constant header length would be right twice and wrong once.
        let mut digi = vec![0u8; 36];
        digi.extend_from_slice(&[0xbe, 0xef, 0xba, 0xce, 0x00]);
        assert_eq!(container_offset(&digi), Some(36));

        let mut a4 = vec![0u8; 31];
        a4.extend_from_slice(&[0xbe, 0xef, 0xba, 0xba, 0x00]);
        assert_eq!(container_offset(&a4), Some(31));

        assert_eq!(container_offset(&[0u8; 64]), None);
    }
}
