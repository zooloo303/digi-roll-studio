// Send a gen-1 SysEx **pattern dump** to an Analog Four. The first thing in this
// repo that writes to an A4 at all.
//
// **Read this before running it.** Two modes, and the default is the harmless one:
//
//   cargo run -p digi_roll_studio --example a4_pattern_send -- <file.syx>
//       A *rehearsal*. Loads the file, re-derives its checksum and length,
//       unpacks the payload and reports the slot and every trig it carries.
//       **No port is opened.** This is pure arithmetic on a `Vec<u8>`.
//
//   cargo run -p digi_roll_studio --example a4_pattern_send -- <file.syx> --send
//       **The real thing.** Overwrites one pattern slot on the box. Asks for
//       typed consent, and refuses without it.
//
// **Why this is a separate example and not a `digi_midi` function.** The A4's
// pattern format is gen-1 and this workspace speaks gen-2: `sevenbit.rs` packs
// the other way (DEVELOPMENT.md lesson 14), and `protocol::pattern` cannot read
// one of these. So the layout lives in `local/a4_pattern.py` while it is still
// being settled by captures, and this example is the twelve inches of cable.
// Nothing else in the workspace calls it, and it constructs no `0x5x` file
// opcode and no `0x7x` store — it sends one dump message and reads no reply.
//
// **The validation is deliberately duplicated.** `local/a4_pattern.py` already
// checks all of this before it writes the file. It is checked again here, from
// the bytes on disk, because the thing that must be true is not "the builder was
// correct" but "these bytes are well-formed" — and a file can be edited, renamed
// or half-written between the two. A body this box cannot parse takes its whole
// SysEx API down until it is power-cycled (DEVELOPMENT.md lesson 13), so the
// check that matters is the one immediately before the send.
//
// **Verifying afterwards is a manual step, and there is no way around it.** The
// A4 answers no dump *request* — the supported-opcode reply lists no `0x6x`, and
// that part of PLAN.md §10 is still true. A dump is initiated from the box's own
// front panel. So: send, then re-dump the slot from the box, then
// `python3 local/a4_pattern.py verify <sent.syx> <redump.mmon>`.

use std::io::Write as _;
use std::time::Duration;

use digi_midi::{capture_sysex, list_outputs, open_output_by_name};

/// Header is mfr(3) product device type 01 01 slot; trailer is checksum(2)
/// length(2). PLAN.md §10.
const HEADER: usize = 9;
const TRAILER: usize = 4;
const MFR: [u8; 3] = [0x00, 0x20, 0x3C];
const PRODUCT_A4: u8 = 0x06;
const TYPE_PATTERN: u8 = 0x54;
const PAYLOAD_LEN: usize = 12974;
const TRACK_BASE: usize = 4;
const TRACK_STRIDE: usize = 751;
const TRACKS: usize = 6;
const STEPS: usize = 64;
const NOTE_LANE: usize = 128;
const NO_NOTE: u8 = 0xFF;
const TRACK_NAMES: [&str; TRACKS] = ["SYN1", "SYN2", "SYN3", "SYN4", "FX", "CV"];

/// Which box to send to when nothing is named. Matched as a case-insensitive
/// substring of the port name.
const DEFAULT_PORT: &str = "Analog Four";

/// The word `--send` makes you type. Not a y/n, because a y/n is something you
/// press by reflex.
const CONSENT: &str = "overwrite";

/// **Why the send is paced by default.** DIN MIDI is 31,250 baud — ten bits a
/// byte, so 3,125 bytes a second. A 14,843-byte dump takes **4.75 seconds** to
/// arrive over a cable, and that is the rate the box was designed against in
/// 2013. Over USB, `send` hands CoreMIDI the whole frame at once and it lands in
/// microseconds. A receive path built for a trickle has no reason to survive a
/// flood, and the first send of this file — one unpaced call — did nothing at
/// all on a box that was demonstrably listening.
///
/// So the frame is delivered in pieces, which is legal: `F0 … F7` is one message
/// however many packets carry it, and `SysExReassembler` on our own side exists
/// because that is normal. `--single` restores the original behaviour, kept so
/// the difference stays measurable rather than assumed.
const DIN_BYTES_PER_SEC: f64 = 3125.0;
const DEFAULT_CHUNK: usize = 256;
/// How long to listen for a reply after the last byte. The box may say nothing;
/// silence is recorded as silence rather than as success.
const REPLY_WINDOW: Duration = Duration::from_millis(1500);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let for_real = args.iter().any(|a| a == "--send");
    let single = args.iter().any(|a| a == "--single");
    let chunk: usize = flag(&args, "--chunk").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_CHUNK);
    let pace_ms: f64 = flag(&args, "--pace-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(chunk as f64 / DIN_BYTES_PER_SEC * 1000.0);
    let port_fragment = flag(&args, "--port").unwrap_or_else(|| DEFAULT_PORT.to_string());
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!(
            "usage: a4_pattern_send <file.syx> [--send] [--port NAME] \\\n                    [--chunk BYTES] [--pace-ms MS] [--single]"
        );
        std::process::exit(2);
    };

    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(2);
        }
    };

    let msg = match validate(&raw) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{path}: {e}");
            eprintln!("refusing to send a message this process cannot itself verify");
            std::process::exit(1);
        }
    };

    println!("{path}");
    println!("  {} bytes on the wire, F0/F7 included", msg.wire.len());
    println!("  slot {} ({})  checksum {:#06x} ok  length {} ok", msg.slot, slot_name(msg.slot), msg.checksum, msg.length);
    println!("  payload {} bytes, decodes clean", msg.payload.len());
    for (track, name) in TRACK_NAMES.iter().enumerate() {
        let trigs = trigs_of(&msg.payload, track);
        if trigs.is_empty() {
            continue;
        }
        let listed: Vec<String> = trigs
            .iter()
            .map(|(step, note)| match note {
                Some(n) => format!("{step}:{}", note_name(*n)),
                None => format!("{step}:--"),
            })
            .collect();
        println!("  {name}  {} trigs  {}", trigs.len(), listed.join(" "));
    }

    if single {
        println!("\n  delivery: ONE unpaced send — the shape that did nothing on 2026-08-30");
    } else {
        let n = msg.wire.len().div_ceil(chunk);
        println!(
            "\n  delivery: {n} chunks of {chunk} bytes, {pace_ms:.1} ms apart (~{:.1} s, DIN is {:.1} s)",
            n as f64 * pace_ms / 1000.0,
            msg.wire.len() as f64 / DIN_BYTES_PER_SEC
        );
    }

    if !for_real {
        println!("rehearsal — nothing was sent, and no port was opened.");
        println!("re-run with --send to write this to the box.");
        return;
    }

    let outputs = match list_outputs() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("could not enumerate MIDI outputs: {e:?}");
            std::process::exit(1);
        }
    };
    let needle = port_fragment.to_lowercase();
    let Some(port) = outputs.iter().find(|p| p.name.to_lowercase().contains(&needle)) else {
        eprintln!("no output port matching {port_fragment:?}. Ports present:");
        for p in &outputs {
            eprintln!("  {}", p.name);
        }
        std::process::exit(1);
    };

    println!("\nThis will OVERWRITE pattern {} on {}.", slot_name(msg.slot), port.name);
    println!("The box must be in SETTINGS > SYSEX DUMP > SYSEX RECEIVE.");
    println!("Whatever is in that slot now will be gone, and this tool keeps no backup:");
    println!("dump the slot from the box first if you want one.");
    print!("Type {CONSENT} to proceed: ");
    let _ = std::io::stdout().flush();
    let mut typed = String::new();
    if std::io::stdin().read_line(&mut typed).is_err() || typed.trim() != CONSENT {
        println!("not confirmed — nothing sent.");
        return;
    }

    let mut conn = match open_output_by_name(&port.name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not open {}: {e:?}", port.name);
            std::process::exit(1);
        }
    };
    // Listen on the box's *output* while we talk to its input, so a complaint is
    // caught. The A4 answers no dump request, but nothing rules out an
    // acknowledgement, and an unheard reply is indistinguishable from silence.
    let listening = port.name.clone();
    let reply = std::thread::spawn(move || capture_sysex(&listening, REPLY_WINDOW));

    let result = if single {
        conn.send(&msg.wire).map_err(|e| e.to_string())
    } else {
        let mut sent = Ok(());
        for (i, piece) in msg.wire.chunks(chunk).enumerate() {
            if let Err(e) = conn.send(piece) {
                sent = Err(format!("chunk {i} of {}: {e}", msg.wire.len().div_ceil(chunk)));
                break;
            }
            std::thread::sleep(Duration::from_secs_f64(pace_ms / 1000.0));
        }
        sent
    };

    match result {
        Ok(()) => println!("sent {} bytes to {}.", msg.wire.len(), port.name),
        Err(e) => {
            eprintln!("send failed: {e}");
            eprintln!("the box may hold a partial message — power-cycle it before retrying");
            std::process::exit(1);
        }
    }

    match reply.join() {
        Ok(Ok(frames)) if frames.is_empty() => {
            println!("the box said nothing back (which it may well not).");
        }
        Ok(Ok(frames)) => {
            println!("the box replied with {} SysEx frame(s):", frames.len());
            for f in &frames {
                let head: Vec<String> = f.iter().take(16).map(|b| format!("{b:02x}")).collect();
                println!("  {} bytes: {}", f.len(), head.join(" "));
            }
        }
        Ok(Err(e)) => println!("could not listen for a reply: {e:?}"),
        Err(_) => println!("the reply listener panicked"),
    }

    println!("\nNow verify, because nothing here can:");
    println!("  1. dump {} from the box's front panel, capturing it", slot_name(msg.slot));
    println!("  2. python3 local/a4_pattern.py verify {path} <redump.mmon>");
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

struct Message {
    wire: Vec<u8>,
    slot: u8,
    checksum: u16,
    length: u16,
    payload: Vec<u8>,
}

/// Everything that must be true of these bytes before one of them leaves.
fn validate(raw: &[u8]) -> Result<Message, String> {
    let body: &[u8] = match (raw.first(), raw.last()) {
        (Some(0xf0), Some(0xf7)) => &raw[1..raw.len() - 1],
        (Some(0xf0), _) => return Err("starts F0 but does not end F7 — truncated".into()),
        _ => raw,
    };
    if body.len() < HEADER + TRAILER {
        return Err(format!("{} bytes is too short to be a dump", body.len()));
    }
    if body[..3] != MFR {
        return Err(format!("manufacturer id is {:02x?}, not Elektron", &body[..3]));
    }
    if body[3] != PRODUCT_A4 {
        return Err(format!("product byte is {:#04x}, not an A4's {PRODUCT_A4:#04x}", body[3]));
    }
    if body[5] != TYPE_PATTERN {
        return Err(format!("message type is {:#04x}, not a pattern dump", body[5]));
    }
    // Every byte between F0 and F7 must be 7-bit, or the OS will not send what
    // we think it will.
    if let Some(i) = body.iter().position(|b| b & 0x80 != 0) {
        return Err(format!("byte {i} is {:#04x}: high bit set inside the frame", body[i]));
    }

    let encoded = &body[HEADER..body.len() - TRAILER];
    let stated_sum = u16::from(body[body.len() - 4]) << 7 | u16::from(body[body.len() - 3]);
    let stated_len = u16::from(body[body.len() - 2]) << 7 | u16::from(body[body.len() - 1]);
    let sum = encoded.iter().map(|&b| u32::from(b)).sum::<u32>() as u16 & 0x3FFF;
    let len = (body.len() - 8) as u16;
    if stated_sum != sum {
        return Err(format!("checksum says {stated_sum:#06x}, bytes say {sum:#06x}"));
    }
    if stated_len != len {
        return Err(format!("length says {stated_len}, bytes say {len}"));
    }

    let payload = decode7_msb_first(encoded);
    if payload.len() != PAYLOAD_LEN {
        return Err(format!("payload is {} bytes, an A4 pattern is {PAYLOAD_LEN}", payload.len()));
    }

    let mut wire = Vec::with_capacity(body.len() + 2);
    wire.push(0xf0);
    wire.extend_from_slice(body);
    wire.push(0xf7);
    Ok(Message { wire, slot: body[8], checksum: stated_sum, length: stated_len, payload })
}

/// The A4's gen-1 order: the MSB byte's bit 6 carries the *first* payload byte.
/// `digi_protocol::sevenbit` runs bit 0 to byte 0 and is right for the digis —
/// the two generations do not share this. DEVELOPMENT.md lesson 14.
fn decode7_msb_first(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 8 * 7 + 7);
    for group in data.chunks(8) {
        let msbs = group[0];
        for (n, &b) in group[1..].iter().enumerate() {
            out.push(b | if msbs >> (6 - n) & 1 == 1 { 0x80 } else { 0 });
        }
    }
    out
}

fn trigs_of(payload: &[u8], track: usize) -> Vec<(usize, Option<u8>)> {
    let base = TRACK_BASE + track * TRACK_STRIDE;
    (0..STEPS)
        .filter(|&s| payload[base + s * 2] & 1 == 1)
        .map(|s| {
            let note = payload[base + NOTE_LANE + s];
            (s + 1, (note != NO_NOTE).then_some(note))
        })
        .collect()
}

fn slot_name(slot: u8) -> String {
    format!("{}{:02}", (b'A' + slot / 16) as char, slot % 16 + 1)
}

fn note_name(v: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[usize::from(v) % 12], i16::from(v) / 12 - 1)
}
