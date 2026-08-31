//! Elektron SysEx message framing.
//!
//! Elektron boxes speak two unrelated SysEx mechanisms, both starting with the
//! manufacturer header `F0 00 20 3C` and diverging at byte 4 — an API message
//! (RPC request/response: device info, +Drive access) and a dump message
//! (pattern/sound/project transfers, with a checksum and a count in the
//! trailer). Ported from `js/elektron/protocol.js`, which has the full layout
//! of both in its own header.
//!
//! Byte-level format ported from elk-herd (BSD-2-Clause, © mzero):
//! `src/SysEx/SysEx.elm`, `src/SysEx/Dump.elm`, `src/SysEx/ApiUtil.elm`.
//! See `CREDITS.md`.

use crate::sevenbit::{encode7, decode7};

pub const ELEKTRON_ID: [u8; 3] = [0x00, 0x20, 0x3c];
pub const API_TAG: u8 = 0x10;

pub const API_DEVICE: u8 = 0x01;
pub const API_VERSION: u8 = 0x02;
pub const API_RESPONSE: u8 = 0x80;

// Dump message types: 0x5n are responses/payloads, 0x6n are requests. A
// response opcode is always its request minus 0x10.
pub const DUMP_PATTERN_KIT: u8 = 0x50;
pub const DUMP_PATTERN: u8 = 0x51;
pub const DUMP_KIT: u8 = 0x52;
pub const DUMP_SOUND: u8 = 0x53;
/// **On the digis.** The same opcode is a *pattern* dump on the gen-1 Analog
/// Four, so a `dump_type` is only meaningful alongside its `family` — see
/// [`crate::a4_pattern::DUMP_A4_PATTERN`] and
/// [`crate::a4_pattern::is_a4_pattern`].
pub const DUMP_PROJECT_SETTINGS: u8 = 0x54;
/// One track's sound in the box's **active** kit, addressed by track index
/// 0–15 rather than by a stored slot. The store half of
/// [`DUMP_KIT_TRACK_SOUND_REQUEST`], by the minus-0x10 rule above.
///
/// **Named from the rule, not from a reply.** The request half is
/// hardware-verified (PLAN.md §9, 2026-08-26); this one is what the rule
/// predicts a per-track store would be, and `examples/probe_sound_store.rs` is
/// the check. Read [`crate::safe_write::write_gate`]'s callers before sending
/// it: under a *dump* header this is a store, and the +Drive API's unrelated
/// `0x5B` Copy is a different namespace entirely (see [`crate::drive`]).
pub const DUMP_KIT_TRACK_SOUND: u8 = 0x5b;

pub const DUMP_PATTERN_KIT_REQUEST: u8 = 0x60;
pub const DUMP_PATTERN_REQUEST: u8 = 0x61;
pub const DUMP_KIT_REQUEST: u8 = 0x62;
pub const DUMP_SOUND_REQUEST: u8 = 0x63;
pub const DUMP_PROJECT_SETTINGS_REQUEST: u8 = 0x64;
/// Fetch one track's sound from the box's active kit, index 0–15. Payload is a
/// 5-byte wrapper then one whole sound struct — hardware-verified on a DT2 and
/// a DN2 on 2026-08-26 against Overbridge's KIT TRACK PRESETS pane, all
/// sixteen in order. Inside `assert_request_opcode`'s 0x60–0x6e range, so every
/// existing fetch guard already admits it.
pub const DUMP_KIT_TRACK_SOUND_REQUEST: u8 = 0x6b;
pub const DUMP_WHOLE_PROJECT_REQUEST: u8 = 0x6f;

pub const FAMILY_DIGITAKT: u8 = 0x0a;
pub const FAMILY_DIGITAKT_2: u8 = 0x14;
pub const FAMILY_DIGITONE_2: u8 = 0x15;
/// The gen-1 Analog Four mk1. The identity API calls this box 4; the byte in a
/// dump header is 6.
///
/// **This family is on the same dump framing as the digis and a different
/// payload format entirely.** Everything in this module reads and writes an A4
/// pattern dump correctly — header, checksum, count, seven-bit packing — and
/// nothing in [`crate::pattern`] can make sense of the result.
/// [`crate::a4_pattern`] is where gen-1 layout lives.
pub const FAMILY_ANALOG_FOUR: u8 = 0x06;

#[derive(Debug, PartialEq)]
pub enum SysExKind {
    Foreign,
    Unknown,
    Api,
    Dump,
}

#[derive(Debug)]
pub struct ApiMessage {
    pub msg_id: u16,
    pub resp_id: u16,
    pub api_id: u8,
    pub args: Vec<u8>,
}

#[derive(Debug)]
pub struct DumpMessage {
    pub family: u8,
    pub dump_type: u8,
    pub version: [u8; 2],
    pub index: u8,
    pub payload: Vec<u8>,
    pub checksum_ok: bool,
    pub count_ok: bool,
}

#[derive(Debug)]
pub struct ParsedSysEx {
    pub kind: SysExKind,
    pub api: Option<ApiMessage>,
    pub dump: Option<DumpMessage>,
}

fn uint14be(v: u16) -> [u8; 2] {
    [((v >> 7) & 0x7f) as u8, (v & 0x7f) as u8]
}

pub fn checksum14(bytes: &[u8]) -> u16 {
    let sum: u32 = bytes.iter().map(|&b| b as u32).sum();
    (sum & 0x3fff) as u16
}

pub fn build_api_message(msg_id: u16, api_id: u8, args: &[u8], resp_id: u16) -> Vec<u8> {
    let mut body = Vec::with_capacity(5 + args.len());
    body.push((msg_id >> 8) as u8);
    body.push((msg_id & 0xff) as u8);
    body.push((resp_id >> 8) as u8);
    body.push((resp_id & 0xff) as u8);
    body.push(api_id);
    body.extend_from_slice(args);
    let encoded = encode7(&body);
    let mut out = Vec::with_capacity(6 + encoded.len() + 1);
    out.push(0xf0);
    out.extend_from_slice(&ELEKTRON_ID);
    out.push(API_TAG);
    out.push(0x00);
    out.extend(encoded);
    out.push(0xf7);
    out
}

pub fn build_dump_message(family: u8, dump_type: u8, index: u8, payload: &[u8]) -> Vec<u8> {
    let encoded = encode7(payload);
    let csum = checksum14(&encoded);
    let count = ((encoded.len() + 5) & 0x3fff) as u16;
    let mut out = Vec::new();
    out.push(0xf0);
    out.extend_from_slice(&ELEKTRON_ID);
    out.push(family);
    out.push(0x00);
    out.push(dump_type);
    out.push(0x01);
    out.push(0x01);
    out.push(index);
    out.extend(encoded);
    out.push(uint14be(csum)[0]);
    out.push(uint14be(csum)[1]);
    out.push(uint14be(count)[0]);
    out.push(uint14be(count)[1]);
    out.push(0xf7);
    out
}

pub fn parse_sysex(bytes: &[u8]) -> ParsedSysEx {
    if bytes.len() < 6
        || bytes[0] != 0xf0
        || bytes[1] != ELEKTRON_ID[0]
        || bytes[2] != ELEKTRON_ID[1]
        || bytes[3] != ELEKTRON_ID[2]
    {
        return ParsedSysEx { kind: SysExKind::Foreign, api: None, dump: None };
    }

    if bytes[4] == API_TAG && bytes[5] == 0x00 {
        let encoded_body = &bytes[6..bytes.len() - 1];
        let body = match std::panic::catch_unwind(|| decode7(encoded_body)) {
            Ok(v) => v,
            Err(_) => return ParsedSysEx { kind: SysExKind::Unknown, api: None, dump: None },
        };
        if body.len() < 5 {
            return ParsedSysEx { kind: SysExKind::Unknown, api: None, dump: None };
        }
        let msg_id = ((body[0] as u16) << 8) | body[1] as u16;
        let resp_id = ((body[2] as u16) << 8) | body[3] as u16;
        let api_id = body[4];
        let args = body[5..].to_vec();
        return ParsedSysEx {
            kind: SysExKind::Api,
            api: Some(ApiMessage { msg_id, resp_id, api_id, args }),
            dump: None,
        };
    }

    if bytes[5] == 0x00 && bytes.len() >= 15 {
        let encoded = &bytes[10..bytes.len() - 5];
        let payload = match std::panic::catch_unwind(|| decode7(encoded)) {
            Ok(v) => v,
            Err(_) => return ParsedSysEx { kind: SysExKind::Unknown, api: None, dump: None },
        };
        let checksum = ((bytes[bytes.len() - 5] as u16) << 7) | bytes[bytes.len() - 4] as u16;
        let count = ((bytes[bytes.len() - 3] as u16) << 7) | bytes[bytes.len() - 2] as u16;
        let checksum_ok = checksum14(encoded) == (checksum & 0x3fff);
        let count_ok = ((encoded.len() + 5) & 0x3fff) as u16 == (count & 0x3fff);
        return ParsedSysEx {
            kind: SysExKind::Dump,
            api: None,
            dump: Some(DumpMessage {
                family: bytes[4],
                dump_type: bytes[6],
                version: [bytes[7], bytes[8]],
                index: bytes[9],
                payload,
                checksum_ok,
                count_ok,
            }),
        };
    }

    ParsedSysEx { kind: SysExKind::Unknown, api: None, dump: None }
}

pub fn split_sysex_stream(bytes: &[u8]) -> Vec<ParsedSysEx> {
    let mut messages = Vec::new();
    let mut start = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == 0xf0 {
            start = Some(i);
        } else if b == 0xf7 {
            if let Some(s) = start {
                messages.push(parse_sysex(&bytes[s..=i]));
                start = None;
            }
        }
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_roundtrip() {
        let msg = build_api_message(0x1234, 0x01, &[1,2,3], 0);
        let parsed = parse_sysex(&msg);
        assert_eq!(parsed.kind, SysExKind::Api);
        let api = parsed.api.unwrap();
        assert_eq!(api.msg_id, 0x1234);
        assert_eq!(api.api_id, 0x01);
        assert_eq!(api.args, vec![1,2,3]);
    }

    #[test]
    fn dump_roundtrip() {
        let payload = vec![10,20,30,40,50];
        let msg = build_dump_message(FAMILY_DIGITAKT_2, DUMP_PATTERN, 0x01, &payload);
        let parsed = parse_sysex(&msg);
        assert_eq!(parsed.kind, SysExKind::Dump);
        let dump = parsed.dump.unwrap();
        assert_eq!(dump.family, FAMILY_DIGITAKT_2);
        assert_eq!(dump.dump_type, DUMP_PATTERN);
        assert_eq!(dump.index, 0x01);
        assert_eq!(dump.payload, payload);
        assert!(dump.checksum_ok);
        assert!(dump.count_ok);
    }
}
