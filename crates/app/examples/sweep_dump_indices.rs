// Sweep the unanswered dump request opcodes across a spread of indices, on
// both boxes.
//
// **Read-only, structurally.** Every request goes through
// `assert_request_opcode`, which admits only 0x60-0x6e — the *request* half of
// the dump protocol; there is no 0x5n store anywhere in this file, and 0x6f
// (whole project) is never sent.
//
// # Why
//
// `probe_dump_types` swept 0x60-0x6e at index 0 and found five that answer
// (0x62, 0x63, 0x65, 0x66, 0x6b) and seven that time out on the DN2: 0x67,
// 0x68, 0x69, 0x6a, 0x6c, 0x6d, 0x6e. But the DN2's identity reply advertises
// response types 0x55-0x5e, which implies requests 0x65-0x6e all exist. Index
// 0 not answering does not mean the opcode is unsupported — 0x6b itself turned
// out to be a collection (`probe_sound_library`), so a reply that only shows up
// at a nonzero index is exactly the kind of thing a single index-0 sweep would
// miss entirely.
//
// This sweep tries a spread of indices per opcode, on both the DT2 and the
// DN2. The DT2 side is not idle curiosity: a response *type* is shared across
// families (0x67 means the same 0x10-less-than-0x77 thing on both boxes), so a
// DT2 reply to an opcode that only times out on the DN2 still tells us what the
// opcode *means* — the DT2 might just have nothing in that slot, or might not
// implement it either, and either answer narrows things down.
//
// What a reply means, in order of how interesting it is:
//
//   * a payload containing `be ef ba ce` — a sound/kit struct, decoded and
//     printed via `decode_sound_dump`;
//   * a reply at some index but not others for the same opcode — the shape of
//     whatever collection it addresses;
//   * a reply that never showed up at index 0 in `probe_dump_types` but shows
//     up here — proof the opcode exists and needs a nonzero index, which is
//     itself the finding even before anything is decoded.
//
// Run with:
//   cargo run -p digi_roll_studio --example sweep_dump_indices

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::sound::{decode_sound_dump, SOUND_MAGIC_HEAD};

/// Every request opcode worth re-checking: the timed-out seven, plus 0x65,
/// 0x66 and 0x6b for completeness even though those three already answer at
/// index 0 — a different index might still answer with something else.
const OPCODES: &[u8] = &[0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e];

/// A spread wide enough to catch "answers past 128" (a second byte's worth of
/// slots) without spending all day on 5 s timeouts per miss.
const INDICES: &[u8] = &[0, 1, 2, 15, 16, 63, 64, 127, 128, 200, 255];

fn main() {
    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    for input in inputs.iter().filter(|p| p.slug.is_some()) {
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
            println!("\n  --- 0x{request:02x} ---");
            let mut hits = 0usize;
            for &index in INDICES {
                match device.fetch_dump(family, request, index) {
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
                            "    idx {index:>3}  HIT  type {:#04x}, {} bytes{}",
                            reply.dump_type,
                            reply.payload.len(),
                            if magic { "  ** BEEFBACE **" } else { "" }
                        );
                        println!("             head: {}", head.join(" "));
                        if magic {
                            match decode_sound_dump(&reply.payload) {
                                Ok(s) => println!(
                                    "             decodes as sound: v{}, {} bytes, tags {:#010x} {:?}, name {:?}",
                                    s.version,
                                    s.bytes.len(),
                                    s.tag_mask,
                                    s.tags(),
                                    s.name
                                ),
                                Err(e) => println!(
                                    "             not one bare sound struct ({e}) — scanning for repeats:"
                                ),
                            }
                            scan_for_magic_offsets(&reply.payload);
                        }
                    }
                    Err(e) => println!("    idx {index:>3}  miss — {e}"),
                }
            }
            println!("  0x{request:02x} summary: {hits}/{} indices answered", INDICES.len());
        }
    }
}

/// Scan a payload for every occurrence of the sound head magic, to spot a bank
/// of back-to-back structs even when the whole payload does not decode as
/// exactly one.
fn scan_for_magic_offsets(payload: &[u8]) {
    let hits: Vec<usize> = (0..payload.len().saturating_sub(3))
        .filter(|&i| {
            u32::from_be_bytes([payload[i], payload[i + 1], payload[i + 2], payload[i + 3]])
                == SOUND_MAGIC_HEAD
        })
        .collect();
    if hits.len() > 1 {
        let strides: Vec<usize> = hits.windows(2).map(|w| w[1] - w[0]).collect();
        println!(
            "             {} head-magic occurrences at offsets {:?}, strides {:?}",
            hits.len(),
            hits,
            strides
        );
    }
}
