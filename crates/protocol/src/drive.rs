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
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriveError::UnsendablePath(p) => {
                write!(f, "path {p:?} has bytes the API cannot carry")
            }
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
