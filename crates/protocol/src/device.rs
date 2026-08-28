// Device layer: the product table, the identity handshake's payload format,
// and the read-only guard on dump opcodes. Ported from
// `js/elektron/device.js`; the transport that drives it lives in `digi_midi`,
// so everything here is pure and testable without hardware.
//
// Identity does NOT use the universal MIDI identity request — Elektron boxes
// answer their own API instead: opcode 0x01 (Device) returns a product id and
// the device name, 0x02 (Version) returns OS build + version strings.
// Protocol behaviour ported from elk-herd (BSD-2-Clause, © mzero):
// src/SysEx.elm, src/Elektron/Instrument.elm, src/Project/Update.elm.

use crate::protocol::{FAMILY_DIGITAKT, FAMILY_DIGITAKT_2, FAMILY_DIGITONE_2};

/// A product we recognise from the Device response. Only boxes with a known
/// dump family byte can be backed up; anything else stays read-only until we
/// learn its protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Product {
    pub product_id: u8,
    pub name: &'static str,
    pub slug: &'static str,
    /// `None` for a box that answers the identity API but has no dump family —
    /// the Analog Four, whose supported-opcode list contains no `0x6x` request
    /// at all (captured 2026-08-28). Being *identifiable* and being
    /// *dumpable* were the same thing until that box was plugged in, and this
    /// is where they come apart: a row may name a box without claiming its
    /// dumps can be read.
    pub family: Option<u8>,
}

// Both DT2 and DN2 values were captured from real hardware 2026-08-01 (the DN2
// family byte via a 0x60 probe sweep). The A4 row was captured 2026-08-28, off
// the box itself, on the day it arrived.
pub const PRODUCTS: &[Product] = &[
    Product { product_id: 12, name: "Digitakt", slug: "digitakt", family: Some(FAMILY_DIGITAKT) },
    Product { product_id: 42, name: "Digitakt II", slug: "digitakt2", family: Some(FAMILY_DIGITAKT_2) },
    Product { product_id: 43, name: "Digitone II", slug: "digitone2", family: Some(FAMILY_DIGITONE_2) },
    // Answers 0x01 with product id 4 and the name "Analog Four", on OS 1.55B
    // (build 0195) — so the mk1 does *not* predate this API, which is what the
    // 2026-08-24 guess had assumed. `family: None` because the same reply's
    // supported-opcode list is 01,02,03,04,06,07,09 then 50-5e: every file and
    // store opcode, and not one `0x6x` dump request. There is no dump family to
    // capture, rather than one nobody has looked for yet.
    Product { product_id: 4, name: "Analog Four", slug: "analogfour", family: None },
];

pub fn product_for_id(product_id: u8) -> Option<&'static Product> {
    PRODUCTS.iter().find(|p| p.product_id == product_id)
}

/// Which box a dump *file* came off, from its family byte.
///
/// The handshake is the real answer, but a pattern read off a `.syx` on disk
/// never had one — a backup taken months ago on another machine still has to
/// name its box. This is the port of `safe-write.js`'s `PRODUCT_BY_FAMILY`,
/// which was a second hard-coded table beside the one in `device.js`; a lookup
/// over [`PRODUCTS`] answers the same question without the two being able to
/// disagree. It answers for the gen-1 Digitakt too, where the JS map did not —
/// harmless, because whether a box can be *written* is
/// [`crate::safe_write::write_gate`]'s decision and not this one's.
pub fn product_for_family(family: u8) -> Option<&'static Product> {
    PRODUCTS.iter().find(|p| p.family == Some(family))
}

/// Which box a MIDI port name looks like, without asking it.
///
/// The real answer comes from the identity handshake, but that needs a round
/// trip. Until then the port name is all we have — and it is enough to tell a
/// Digitone II from a Digitakt II in the port menu, which is what the UI needs
/// to stop offering you the wrong box's parameters.
///
/// Longest name first, so "Elektron Digitakt II" isn't claimed by the gen-1
/// "Digitakt" entry it starts with. An unrecognised name returns `None`, and
/// every caller must treat that as "don't know" rather than as a default.
pub fn product_from_port_name(port_name: &str) -> Option<&'static Product> {
    let lower = port_name.to_lowercase();
    let mut candidates: Vec<&Product> = PRODUCTS.iter().collect();
    candidates.sort_by_key(|p| std::cmp::Reverse(p.name.len()));
    candidates
        .into_iter()
        .find(|p| lower.contains(&p.name.to_lowercase()))
}

pub fn slug_from_port_name(port_name: &str) -> Option<&'static str> {
    product_from_port_name(port_name).map(|p| p.slug)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub product_id: u8,
    /// Dump request opcodes this box says it supports.
    pub supported_ids: Vec<u8>,
    pub name: String,
    pub slug: String,
    /// `None` for a box whose dump protocol we do not know.
    pub family: Option<u8>,
    /// e.g. "0070" — what struct-version gating keys off later.
    pub build: String,
    /// e.g. "1.15B" — the human-facing OS version.
    pub version: String,
}

impl DeviceIdentity {
    /// "we can fetch its dumps"
    pub fn supported(&self) -> bool {
        self.family.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    /// A response was too short to hold the fields it must carry.
    Truncated(&'static str),
    /// Refused before it reached the wire. See [`assert_request_opcode`].
    NotARequestOpcode(u8),
    /// This box has no known dump protocol, so we cannot address its dumps.
    UnknownFamily(String),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::Truncated(what) => write!(f, "truncated {what} response"),
            DeviceError::NotARequestOpcode(t) => {
                write!(f, "0x{t:02x} is not a dump request opcode — refusing to send it")
            }
            DeviceError::UnknownFamily(name) => {
                write!(f, "no known dump protocol for {name}")
            }
        }
    }
}

impl std::error::Error for DeviceError {}

/// The read-only guard shared by every fetch path: dump *requests* are
/// 0x60–0x6e. An 0x5n message is what stores a payload on the box, so refusing
/// everything outside the request range makes these paths incapable of
/// writing — which matters most when the box belongs to a contributor mapping
/// it for us.
///
/// `DUMP_WHOLE_PROJECT_REQUEST` (0x6f) sits deliberately outside this range,
/// exactly as in the JS: the whole-project path calls it directly rather than
/// widening the guard for every caller.
pub fn assert_request_opcode(dump_type: u8) -> Result<(), DeviceError> {
    if (0x60..=0x6e).contains(&dump_type) {
        Ok(())
    } else {
        Err(DeviceError::NotARequestOpcode(dump_type))
    }
}

/// Windows-1252 → `char`. Bytes 0x00–0x7F and 0xA0–0xFF map to the same code
/// point; 0x80–0x9F carry printable characters that Latin-1 leaves as control
/// codes. Unassigned bytes become U+FFFD, matching `TextDecoder`.
pub fn cp1252_char(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}',
        '\u{017D}', '\u{FFFD}', '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Null-terminated Windows-1252 string at `start`; returns `(value, next_offset)`.
/// An unterminated string runs to the end of the buffer, as in the JS.
pub fn cstring(bytes: &[u8], start: usize) -> (String, usize) {
    if start >= bytes.len() {
        return (String::new(), bytes.len());
    }
    let end = bytes[start..]
        .iter()
        .position(|&b| b == 0)
        .map(|i| start + i)
        .unwrap_or(bytes.len());
    let value = bytes[start..end].iter().map(|&b| cp1252_char(b)).collect();
    (value, end + 1)
}

/// The args of an API_DEVICE response: product id, the dump request opcodes the
/// box supports, and the name it reports for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceResponse {
    pub product_id: u8,
    pub supported_ids: Vec<u8>,
    pub reported_name: String,
}

pub fn parse_device_response(args: &[u8]) -> Result<DeviceResponse, DeviceError> {
    if args.len() < 2 {
        return Err(DeviceError::Truncated("device"));
    }
    let count = args[1] as usize;
    let ids_end = 2 + count;
    if args.len() < ids_end {
        return Err(DeviceError::Truncated("device"));
    }
    let (reported_name, _) = cstring(args, ids_end);
    Ok(DeviceResponse {
        product_id: args[0],
        supported_ids: args[2..ids_end].to_vec(),
        reported_name,
    })
}

/// The args of an API_VERSION response: build string then version string.
pub fn parse_version_response(args: &[u8]) -> Result<(String, String), DeviceError> {
    if args.is_empty() {
        return Err(DeviceError::Truncated("version"));
    }
    let (build, after_build) = cstring(args, 0);
    let (version, _) = cstring(args, after_build);
    Ok((build, version))
}

/// Combine the two handshake responses into the identity the rest of the app
/// keys off. An unrecognised product keeps the name the box reported and is
/// marked unsupported rather than being guessed into a default.
pub fn identity_from_responses(dev: &DeviceResponse, build: String, version: String) -> DeviceIdentity {
    let product = product_for_id(dev.product_id);
    let name = product
        .map(|p| p.name.to_string())
        .or_else(|| (!dev.reported_name.is_empty()).then(|| dev.reported_name.clone()))
        .unwrap_or_else(|| format!("Elektron device #{}", dev.product_id));
    DeviceIdentity {
        product_id: dev.product_id,
        supported_ids: dev.supported_ids.clone(),
        name,
        slug: product.map(|p| p.slug.to_string()).unwrap_or_else(|| "elektron".into()),
        family: product.and_then(|p| p.family),
        build,
        version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bytes as a real DT2 lays them out: product id, one supported opcode, then
    // the null-terminated name.
    fn dt2_device_args() -> Vec<u8> {
        let mut v = vec![42, 1, 0x60];
        v.extend_from_slice(b"Digitakt II\0");
        v
    }

    #[test]
    fn parses_device_response() {
        let r = parse_device_response(&dt2_device_args()).unwrap();
        assert_eq!(r.product_id, 42);
        assert_eq!(r.supported_ids, vec![0x60]);
        assert_eq!(r.reported_name, "Digitakt II");
    }

    #[test]
    fn parses_version_response() {
        let (build, version) = parse_version_response(b"0070\x001.15B\x00").unwrap();
        assert_eq!(build, "0070");
        assert_eq!(version, "1.15B");
    }

    #[test]
    fn truncated_responses_are_errors_not_panics() {
        assert!(parse_device_response(&[]).is_err());
        assert!(parse_device_response(&[42]).is_err());
        // Claims four supported ids but carries only one.
        assert!(parse_device_response(&[42, 4, 0x60]).is_err());
        assert!(parse_version_response(&[]).is_err());
    }

    #[test]
    fn identity_maps_known_product() {
        let dev = parse_device_response(&dt2_device_args()).unwrap();
        let id = identity_from_responses(&dev, "0070".into(), "1.15B".into());
        assert_eq!(id.slug, "digitakt2");
        assert_eq!(id.name, "Digitakt II");
        assert_eq!(id.family, Some(FAMILY_DIGITAKT_2));
        assert!(id.supported());
    }

    #[test]
    fn unknown_product_keeps_its_own_name_and_stays_unsupported() {
        let mut args = vec![99, 0];
        args.extend_from_slice(b"Syntakt\0");
        let dev = parse_device_response(&args).unwrap();
        let id = identity_from_responses(&dev, "0001".into(), "1.0".into());
        assert_eq!(id.name, "Syntakt");
        assert_eq!(id.slug, "elektron");
        assert_eq!(id.family, None);
        assert!(!id.supported());
    }

    #[test]
    fn unknown_product_without_a_name_falls_back_to_its_id() {
        let dev = parse_device_response(&[99, 0]).unwrap();
        let id = identity_from_responses(&dev, "0001".into(), "1.0".into());
        assert_eq!(id.name, "Elektron device #99");
    }

    // The bug the JS comment warns about: "Elektron Digitakt II" must not be
    // claimed by the gen-1 "Digitakt" entry whose name it starts with.
    #[test]
    fn port_name_matching_prefers_the_longest_product_name() {
        assert_eq!(slug_from_port_name("Elektron Digitakt II"), Some("digitakt2"));
        assert_eq!(slug_from_port_name("Elektron Digitakt"), Some("digitakt"));
        assert_eq!(slug_from_port_name("Elektron Digitone II"), Some("digitone2"));
    }

    #[test]
    fn port_name_matching_is_case_insensitive_and_admits_ignorance() {
        assert_eq!(slug_from_port_name("digitakt ii"), Some("digitakt2"));
        assert_eq!(slug_from_port_name("Scarlett 2i2"), None);
        assert_eq!(slug_from_port_name(""), None);
    }

    #[test]
    fn request_opcode_guard_refuses_everything_that_could_write() {
        for t in 0x60u8..=0x6e {
            assert!(assert_request_opcode(t).is_ok());
        }
        // 0x5n is what *stores* a payload on the box.
        for t in [0x50u8, 0x51, 0x52, 0x53, 0x54] {
            assert!(assert_request_opcode(t).is_err());
        }
        // 0x6f is a request, but deliberately outside the shared guard.
        assert!(assert_request_opcode(0x6f).is_err());
    }

    #[test]
    fn cp1252_decodes_high_bytes_the_way_textdecoder_does() {
        let (s, _) = cstring(&[0x93, b'A', 0x94, 0x00], 0);
        assert_eq!(s, "\u{201C}A\u{201D}");
    }

    #[test]
    fn unterminated_cstring_runs_to_the_end() {
        let (s, next) = cstring(b"0070", 0);
        assert_eq!(s, "0070");
        assert_eq!(next, 5);
    }
}
