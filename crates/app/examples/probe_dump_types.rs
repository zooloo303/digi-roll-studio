// Sweep the unmapped dump request opcodes and report what each box answers.
//
// **Read-only, structurally.** Every request goes through
// `assert_request_opcode`, which admits only 0x60–0x6e — the *request* half of
// the dump protocol. Storing is 0x5n, and there is no 0x5n in this file. 0x6f
// (whole project) is excluded too: it is not a store either, but it streams
// megabytes and this sweep wants one small answer per opcode. This is the same
// technique that found the DN2's family byte in August (`js/labs/probe.js`),
// which swept 0x60 across candidate family bytes.
//
// # Why
//
// `probe_drive` established the split: the DT2 exposes a +Drive file system over
// the API path (0x10 DirList and friends, and its tree is full of samples), and
// **the DN2 does not implement 0x10 at all**. But the DN2's identity reply lists
// dump types 0x50 through 0x5e — ten more than the five `protocol.rs` names.
// Since a response opcode is always its request minus 0x10, those ten imply
// requests 0x65–0x6e, all of which the read-only guard already admits.
//
// A DN2 preset library has to be reachable somehow, and it is not on the API
// path. This is where to look.
//
// For each opcode the sweep prints the response's dump type, payload length, and
// leading bytes. What to look for:
//
//   * `be ef ba ce` at the head — a sound or kit struct, i.e. exactly the thing
//     a preset browser needs. The version word follows at +4 and the tag mask at
//     +8 (`digi_protocol::sound`).
//   * a payload of ~359 bytes on the DN2 — one sound struct.
//   * a much larger payload — a bank or a whole library in one dump.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_dump_types
//   cargo run -p digi_roll_studio --example probe_dump_types -- --index 1

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::sound::{decode_sound_dump, SOUND_MAGIC_HEAD};

/// The request opcodes to try. 0x60–0x64 are the five we already map, included
/// so the sweep has known-good rows to calibrate the unknown ones against.
const SWEEP: &[(u8, &str)] = &[
    (0x60, "pattern+kit (known)"),
    (0x61, "pattern (known)"),
    (0x62, "kit (known)"),
    (0x63, "sound (known)"),
    (0x64, "project settings (known)"),
    (0x65, "?"),
    (0x66, "?"),
    (0x67, "?"),
    (0x68, "?"),
    (0x69, "?"),
    (0x6a, "?"),
    (0x6b, "?"),
    (0x6c, "?"),
    (0x6d, "?"),
    (0x6e, "?"),
];

fn main() {
    let index: u8 = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--index")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(0);

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
        println!(
            "  supported ids ({}): {}",
            identity.supported_ids.len(),
            identity.supported_ids.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
        );
        // Which of the 0x5n response types the box claims. A request whose
        // response type is not in this list is expected to time out, so a reply
        // from one that is *absent* is the more interesting result.
        let claimed: Vec<String> = (0x50u8..=0x5e)
            .filter(|t| identity.supported_ids.contains(t))
            .map(|t| format!("{t:02x}"))
            .collect();
        println!("  claims dump responses: {}", claimed.join(" "));

        println!("\n  REQ   CLAIMED  RESULT");
        for (request, label) in SWEEP {
            let response_type = request - 0x10;
            let claimed = identity.supported_ids.contains(&response_type);
            match device.fetch_dump(family, *request, index) {
                Ok(reply) => {
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
                        "  {request:#04x}  {:<7}  type {:#04x}, {} bytes{}",
                        if claimed { "yes" } else { "no" },
                        reply.dump_type,
                        reply.payload.len(),
                        if magic { "  ** BEEFBACE — a sound/kit struct **" } else { "" }
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
                            Err(e) => println!(
                                "        not one bare sound struct ({e}) — likely a bank of them"
                            ),
                        }
                    }
                    if *label != "?" {
                        println!("        ({label})");
                    }
                }
                Err(e) => println!("  {request:#04x}  {:<7}  {e}", if claimed { "yes" } else { "no" }),
            }
        }
    }
}
