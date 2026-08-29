// Walk the 0x6b dump across indices and find out what collection it addresses.
//
// **Read-only.** 0x6b goes through `assert_request_opcode` like every other
// fetch; there is no 0x5n store opcode anywhere in this file.
//
// # What 0x6b is
//
// `probe_dump_types` swept 0x60-0x6e on both boxes. Both answered 0x6b with a
// payload that is exactly **five bytes longer than one sound struct**:
//
//   DT2  1114 bytes = 5 + 1109   head: 00 00 00 03 00 | be ef ba ce 00 00 00 03 …
//   DN2   364 bytes = 5 +  359   head: 00 00 00 02 00 | be ef ba ce 00 00 00 02 …
//
// So the payload is a 5-byte wrapper — a u32be version plus one more byte —
// followed by a whole sound struct, magics and all. The wrapper's version
// matches the sound's own version in both cases.
//
// The interesting part: at index 0 the DN2's 0x6b sound is **not** the same sound
// its 0x63 returns. 0x63 index 0 is the project pool's slot 1 (`BD BRASSY KICK`,
// tags 0x04100021). 0x6b index 0 has a different tag mask. Two different
// collections, addressed the same way.
//
// The candidate this example is testing: **0x6b is the +Drive sound library** —
// the A001-H… banks in Overbridge's "+DRIVE PRESET LIBRARY", which is precisely
// what an easy kit builder needs and what nothing else we have can reach. The DN2
// makes this the only candidate left: it implements no file-system API at all
// (no 0x10 DirList, no 0x30/0x32 FileRead), so if its library is reachable it is
// reachable as a dump.
//
// What the output tells us:
//
//   * distinct, named, tagged sounds across many indices → it is a library, and
//     the trailing wrapper byte is probably a bank selector;
//   * the same sound at every index → 0x6b ignores the index and is something
//     else (a work buffer, say);
//   * 128 sounds and then nothing → same one-byte addressing limit as the pool,
//     so banks need the wrapper byte or another request argument.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_sound_library
//   cargo run -p digi_roll_studio --example probe_sound_library -- --count 128

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::sound::decode_sound_dump;

/// The 0x6b payload's wrapper: u32be version, then one unidentified byte.
const WRAPPER: usize = 5;

const REQUEST: u8 = 0x6b;

fn main() {
    let count: usize = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--count")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(24);

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
        let Some(family) = identity.family else { continue };
        println!("\n=== {} — {} ===", identity.name, identity.version);
        println!("  IDX  WRAP         NAME               TAG MASK    TAGS");

        let mut names: Vec<String> = Vec::new();
        let mut wrappers: Vec<Vec<u8>> = Vec::new();
        let mut misses = 0usize;

        for index in 0..count {
            let Ok(index_byte) = u8::try_from(index) else { break };
            match device.fetch_dump(family, REQUEST, index_byte) {
                Ok(reply) => {
                    misses = 0;
                    if reply.payload.len() <= WRAPPER {
                        println!("  {index:>3}  payload only {} bytes", reply.payload.len());
                        continue;
                    }
                    let wrap = &reply.payload[..WRAPPER];
                    match decode_sound_dump(&reply.payload[WRAPPER..]) {
                        Ok(s) => {
                            println!(
                                "  {index:>3}  {}  {:<18} {:#010x}  {}",
                                wrap.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
                                if s.name.is_empty() { "(empty)" } else { &s.name },
                                s.tag_mask,
                                s.tags(&identity.slug).join(", ")
                            );
                            if !s.name.is_empty() {
                                names.push(s.name.clone());
                            }
                            wrappers.push(wrap.to_vec());
                        }
                        Err(e) => println!(
                            "  {index:>3}  {} bytes after wrapper, but {e}",
                            reply.payload.len() - WRAPPER
                        ),
                    }
                }
                Err(e) => {
                    misses += 1;
                    println!("  {index:>3}  {e}");
                    if misses >= 3 {
                        println!("  three misses in a row — stopping");
                        break;
                    }
                }
            }
        }

        // The question this example exists to answer.
        let distinct: std::collections::BTreeSet<&String> = names.iter().collect();
        println!(
            "\n  {} answered, {} named, {} distinct name(s)",
            wrappers.len(),
            names.len(),
            distinct.len()
        );
        if distinct.len() > 1 {
            println!("  → 0x6b indexes a COLLECTION of distinct sounds. This is the lead.");
        } else if !names.is_empty() {
            println!("  → the same sound at every index; 0x6b is not index-addressed.");
        }
        let distinct_wrappers: std::collections::BTreeSet<&Vec<u8>> = wrappers.iter().collect();
        println!("  distinct wrapper values: {}", distinct_wrappers.len());
        for w in distinct_wrappers.iter().take(6) {
            println!("    {}", w.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
        }
    }
}
