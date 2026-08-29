// Exercise `ElektronDevice::drive_read_file` against real boxes.
//
// This is the check that matters for it. The parsers have unit tests pinned on
// captured replies, and those tests would pass just as happily if the read loop
// asked for the wrong chunk, stopped a chunk early, or closed a reader the box
// thought was still open — none of which is visible from a fixture. What is
// visible here is whether three boxes hand back whole files.
//
// **Read-only.** Every opcode goes through `assert_read_only_file_op`.
//
// Run with:
//   cargo run -p digi_roll_studio --example read_drive_preset
//   cargo run -p digi_roll_studio --example read_drive_preset -- --path /kits/A

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::{container_offset, file_declared_size, parse_list_entries};

const HOW_MANY: usize = 5;

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let dir = argv
        .windows(2)
        .find(|w| w[0] == "--path")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "/soundbanks/A".to_string());

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else { continue };
        let Ok(mut device) =
            ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
        else {
            continue;
        };
        let Ok(identity) = device.identify() else { continue };
        println!("\n=== {} — {} ===", identity.name, identity.version);

        let Ok(listing) = device.drive_list(&dir, 0, 0) else { continue };
        let Ok(entries) = parse_list_entries(&listing.entry_bytes, listing.count) else { continue };
        let targets: Vec<(String, u32)> = entries
            .iter()
            .filter(|e| e.is_occupied() && e.children.is_none() && e.size.is_some_and(|s| s > 0))
            .take(HOW_MANY)
            .map(|e| (format!("{dir}/{}", e.index.unwrap_or(0)), e.size.unwrap_or(0)))
            .collect();

        for (path, listed) in targets {
            match device.drive_read_file(&path) {
                Ok(file) => {
                    // Three independent numbers for one file: what the listing
                    // said, what the file's own header says, and what Close
                    // counted (checked inside `drive_read_file`). Agreement
                    // across all three is the claim being made here.
                    let declared = file_declared_size(&file);
                    let agree = declared == Some(listed as u16);
                    println!(
                        "  {path:<18} {:>5}b read  header says {:?}  listing said {listed}  {}",
                        file.len(),
                        declared,
                        if agree { "AGREE" } else { "*** DISAGREE ***" }
                    );
                    match container_offset(&file) {
                        Some(at) => println!(
                            "       container at {at}, magic {:02x?}",
                            &file[at..at + 4]
                        ),
                        None => println!("       no container magic found"),
                    }
                }
                Err(e) => println!("  {path:<18} FAILED: {e}"),
            }
        }
    }
}
