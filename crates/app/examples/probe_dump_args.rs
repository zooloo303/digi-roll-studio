// Try dump requests carrying a non-empty request payload, on the opcodes that
// never answer with an empty one.
//
// **Read-only, structurally.** `fetch_dump_with_args` goes through
// `assert_request_opcode` exactly like `fetch_dump` — only 0x60-0x6e is
// reachable — before it ever calls `build_dump_message`. There is no 0x5n
// store anywhere in this file.
//
// # Why
//
// `fetch_dump` — and every probe before this one — always sends an *empty*
// dump-request payload. That is enough to address the 128 slots a request's
// one `index` byte reaches, but the +Drive holds banks A-H of 128+ presets
// each: far more than one byte can name. `build_dump_message` already accepts
// a `payload` argument; nothing before this file ever put anything in it. If a
// bank/slot address travels anywhere in a dump *request*, this is the only
// place left for it to be.
//
// This tries three shapes of argument on each opcode that timed out empty
// (`probe_dump_types`, `sweep_dump_indices`): a one-byte bank (0-7, matching
// the DN2's known bank range A-H), a two-byte (bank, slot) pair, and a
// big-endian u16 slot number — the three most obvious ways a firmware author
// would encode "which one of several hundred".
//
// A hit here — any reply at all to an opcode that has never once answered
// empty — is the finding, independent of whether the payload decodes as a
// sound.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_dump_args

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::sound::{decode_sound_dump, SOUND_MAGIC_HEAD};

/// The opcodes that have never answered an empty-payload request, at any index
/// tried so far.
const OPCODES: &[u8] = &[0x67, 0x68, 0x69, 0x6a, 0x6c, 0x6d, 0x6e];

/// Indices to pair each argument shape with.
///
/// Index 0 was the obvious guess and the wrong one: `sweep_dump_indices`
/// (2026-08-26, DN2 1.10E) found 0x68, 0x69 and 0x6a answer at **index 1 and
/// nowhere else** across 0, 1, 2, 15, 16, 63, 64, 127, 128, 200, 255. An
/// argument probe that only tried index 0 would have reported a clean negative
/// for three opcodes that do in fact answer.
const INDICES: &[u8] = &[0, 1];

/// One argument shape to try: a label and the bytes themselves.
///
/// Deliberately not a full cross product — with 5 s per miss and seven
/// opcodes to try each shape against, on two boxes, in strict sequence (the
/// ports are exclusive, so this file and every other probe in this session run
/// one at a time), every extra shape here costs minutes rather than seconds.
/// This is a spread wide enough to catch an obviously-right encoding, not an
/// exhaustive search of the argument space.
fn arg_shapes() -> Vec<(String, Vec<u8>)> {
    let mut shapes = Vec::new();
    // One byte: a bank, A-H as 0-7, plus one past it.
    for bank in [0u8, 1, 2, 7, 8] {
        shapes.push((format!("bank byte {bank:#04x}"), vec![bank]));
    }
    // Two bytes: (bank, slot).
    for (bank, slot) in [(0u8, 0u8), (0, 1), (7, 0), (3, 64)] {
        shapes.push((format!("(bank {bank:#04x}, slot {slot:#04x})"), vec![bank, slot]));
    }
    // A big-endian u16 slot number — a flat index across the whole library
    // rather than a bank/slot pair.
    for slot in [0u16, 1, 127, 128, 255, 1024] {
        let b = slot.to_be_bytes();
        shapes.push((format!("u16be slot {slot}"), vec![b[0], b[1]]));
    }
    shapes
}

fn main() {
    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let shapes = arg_shapes();
    // Optional port-name fragment. The ports are exclusive and every miss costs
    // 5 s, so one box at a time is the difference between 18 and 35 minutes.
    let only: Option<String> = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--only")
        .map(|w| w[1].clone());

    for input in inputs
        .iter()
        .filter(|p| p.slug.is_some())
        .filter(|p| only.as_ref().is_none_or(|f| p.name.contains(f.as_str())))
    {
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
            continue;
        };
        let mut device =
            match ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output)) {
                Ok(d) => d,
                Err(e) => {
                    println!("\n{}: could not open: {e}", input.name);
                    continue;
                }
            };
        let identity = match device.identify() {
            Ok(id) => id,
            Err(e) => {
                println!("\n{}: no identity: {e}", input.name);
                continue;
            }
        };
        let Some(family) = identity.family else {
            println!("\n{}: no known family byte — cannot address its dumps", identity.name);
            continue;
        };
        println!(
            "\n=== {} — build {}, version {}, family {family:#04x} ===",
            identity.name, identity.build, identity.version
        );

        for &request in OPCODES {
          for index in INDICES {
            println!("\n  --- 0x{request:02x}, index {index}, with args ---");
            let mut hits = 0usize;
            for (label, args) in &shapes {
                match device.fetch_dump_with_args(family, request, *index, args) {
                    Ok(reply) => {
                        hits += 1;
                        let head: Vec<String> =
                            reply.payload.iter().take(16).map(|b| format!("{b:02x}")).collect();
                        let magic = reply.payload.len() >= 4
                            && u32::from_be_bytes([
                                reply.payload[0],
                                reply.payload[1],
                                reply.payload[2],
                                reply.payload[3],
                            ]) == SOUND_MAGIC_HEAD;
                        println!(
                            "    args {label:<24} HIT  type {:#04x}, {} bytes{}",
                            reply.dump_type,
                            reply.payload.len(),
                            if magic { "  ** BEEFBACE **" } else { "" }
                        );
                        println!("        head: {}", head.join(" "));
                        if magic {
                            match decode_sound_dump(&reply.payload) {
                                Ok(s) => println!(
                                    "        decodes as sound: v{}, {} bytes, tags {:#010x} {:?}, name {:?}",
                                    s.version,
                                    s.bytes.len(),
                                    s.tag_mask,
                                    s.tags(&identity.slug),
                                    s.name
                                ),
                                Err(e) => println!("        not one bare sound struct ({e})"),
                            }
                        }
                    }
                    Err(e) => println!("    args {label:<24} miss — {e}"),
                }
            }
            println!(
                "  0x{request:02x} idx {index} summary: {hits}/{} argument shapes answered",
                shapes.len()
            );
          }
        }
    }
}
