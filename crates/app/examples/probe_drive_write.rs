// Write a file to the +Drive and prove it by reading it back.
//
// # This one writes. Everything else in this family does not.
//
// It is guarded by `assert_write_file_op` — a **separate** allowlist from
// `assert_read_only_file_op`, admitting exactly WriteOpen, Write and
// WriteClose. `0x5A` Move, `0x5B` Copy and `0x5C` **Delete** are not
// implemented anywhere in this workspace and this cannot reach them.
//
// Three things make it safe to run against a real +Drive:
//
//   1. **It writes into a slot the listing reports unoccupied**, and refuses to
//      start if the slot is occupied. Nothing is overwritten.
//   2. **`0x59` is the commit**, and every failure path returns before it, so a
//      refused chunk leaves the slot as it was.
//   3. **It verifies by reading back and byte-comparing** — the only way a
//      write that silently truncates announces itself.
//
// # What this was, and what changed
//
// This began as a sweep over candidate `0x58` layouts, because Ángel Linares
// García's document names the three write opcodes and specifies none of their
// bodies. That sweep put six candidates to an Analog Four on 2026-08-29 and got
// three clean refusals and three hangs — and on this box **a body it cannot
// parse takes down the whole SysEx API**, not just the file layer: it stops
// answering `0x01` Device and needs a power cycle, while a DT2 and DN2 on the
// same bus keep answering throughout. Four power cycles bought three facts and
// no working write.
//
// What settled it was a **CoreMIDI spy capture of Elektron's own Transfer
// 1.10.4** uploading one sound to this same box, on 2026-08-30. The layouts now
// live in `digi_protocol::drive` with the captured bytes pinned as tests, and
// this file is the hardware check on them rather than a search.
//
// Two things the sweep could not have reached, for the record:
// the `0x58` body carries **four** u32 fields in an order no gen-1 opcode uses,
// and its checksum is a zero-seeded CRC32 that is then **inverted**. Neither is
// reachable by guessing, and the second answers the source document's own open
// question about why its multi-chunk writes were refused on checksum.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_drive_write -- \
//       --port "Analog Four" --into /soundbanks/P/2 --from /soundbanks/A/1

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::{differs_only_by_location, file_location, parse_list_entries};

fn arg<'a>(argv: &'a [String], flag: &str) -> Option<&'a String> {
    argv.windows(2).find(|w| w[0] == flag).map(|w| &w[1])
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let Some(port) = arg(&argv, "--port") else {
        eprintln!("give --port, --into and optionally --from");
        std::process::exit(2);
    };
    let into = arg(&argv, "--into").cloned().unwrap_or_else(|| "/soundbanks/P/2".to_string());
    let from = arg(&argv, "--from").cloned().unwrap_or_else(|| "/soundbanks/A/1".to_string());

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let input = inputs.iter().find(|p| p.name.contains(port.as_str())).expect("no such input port");
    let output = outputs.iter().find(|p| p.name == input.name).expect("no matching output port");
    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
        .expect("could not open the port");
    let identity = device.identify().expect("no identity");
    println!("=== {} — build {} ===", identity.name, identity.build);

    // The source is read off the same box, so what gets written is known to be
    // a valid file for it rather than something this end invented.
    let source = device.drive_read_file(&from).expect("could not read the source file");
    println!("  source {from}: {} bytes", source.len());

    // Refuse an occupied target. This is the guard that makes the run additive.
    let (dir, leaf) = into.rsplit_once('/').expect("a path with a directory");
    let slot: u32 = leaf.parse().expect("the target's last component should be a slot number");
    let reply = device.drive_list(dir, 0, 0).expect("could not list the target directory");
    let entries =
        parse_list_entries(&reply.entry_bytes, reply.count).expect("could not parse the listing");
    let target = entries.iter().find(|e| e.index == Some(slot)).expect("no such slot");
    if target.is_occupied() {
        println!("  {into} is OCCUPIED ({:?}) — refusing to write", target.name);
        std::process::exit(1);
    }
    println!("  target {into}: free");

    println!("\n  writing {} bytes to {into}", source.len());
    match device.drive_write_file(&into, &source) {
        Ok(n) => println!("    the box committed {n} bytes"),
        Err(e) => {
            println!("    write failed: {e}");
            std::process::exit(1);
        }
    }

    println!("\n  verify: reading {into} back");
    match device.drive_read_file(&into) {
        Ok(back) if back == source => {
            println!("    {} bytes, BYTE-IDENTICAL — the write round-trips", back.len())
        }
        // The expected outcome for a copy between slots: the box rewrites the
        // file's own bank and slot at commit time, so two bytes differ and the
        // write is still perfect. See `differs_only_by_location`.
        Ok(back) if differs_only_by_location(&source, &back) => {
            let (from_bank, from_slot) = file_location(&source).expect("a long enough file");
            let (to_bank, to_slot) = file_location(&back).expect("a long enough file");
            println!(
                "    {} bytes, identical but for the box's own location stamp: \
                 ({from_bank:#04x}, {from_slot:#04x}) -> ({to_bank:#04x}, {to_slot:#04x})",
                back.len()
            );
            println!("    the write round-trips");
        }
        Ok(back) => {
            println!("    {} bytes back, {} written", back.len(), source.len());
            let diffs: Vec<usize> = source
                .iter()
                .zip(back.iter())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i)
                .collect();
            println!("    differs at {} of {} bytes", diffs.len(), source.len().min(back.len()));
            for i in diffs.iter().take(8) {
                println!("      +{i}: wrote {:#04x}, read {:#04x}", source[*i], back[*i]);
            }
        }
        Err(e) => println!("    read-back failed: {e}"),
    }
}
