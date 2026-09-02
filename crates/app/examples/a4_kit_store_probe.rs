// Does an Analog Four accept a kit sent back to it? — the one question the
// preset-load path could not answer without a box.
//
// Everything else about that path was measured off captures already on disk:
// the kit's four 350-byte sound containers, the same container inside a +Drive
// preset file, and — the check that let this probe be written at all —
// `a4_kit::build_working_kit` reproducing the box's own `0x58` message byte for
// byte (`protocol/tests/all/a4_kit.rs`). What no capture can say is whether the
// box *stores* one when it arrives. `0x58` is the reply to a `0x68` request; on
// the digis the reply opcode doubles as the store opcode, and on this box that
// is a prediction from arithmetic until it is asked.
//
// # Three stages, in increasing consequence, and it stops at the first failure
//
//  1. **Read.** Fetch the working kit and decode it. Read-only, and the same
//     call `ui::sync`'s patch names already make.
//  2. **Store what was already there.** Send the fetched payload straight back,
//     unchanged, and re-fetch. This is the whole question and it risks nothing:
//     if the box stores it, the kit it now holds is the kit it held before, byte
//     for byte. A box that ignores it is where this stops.
//  3. **Load a preset, then put it back.** Only if 2 says yes. Reads a real
//     file off the box's own +Drive, splices it onto one synth track, verifies
//     it landed, and reverts from the backup the load handed back.
//
// The kit written is the **working** kit — the box's edit buffer. Its undo is
// reloading the pattern on the box, which discards an unsaved kit; nothing here
// touches a stored kit slot and no `0x52` is built anywhere in this workspace.
//
// Run with:
//   cargo run -p digi_roll_studio --example a4_kit_store_probe
//   cargo run -p digi_roll_studio --example a4_kit_store_probe -- --track 1
//   cargo run -p digi_roll_studio --example a4_kit_store_probe -- --preset /soundbanks/A/7
//   cargo run -p digi_roll_studio --example a4_kit_store_probe -- --read-only
//   cargo run -p digi_roll_studio --example a4_kit_store_probe -- --track 4 --restore
//   cargo run -p digi_roll_studio --example a4_kit_store_probe -- --diff /soundbanks/A/1

use digi_midi::a4_preset_load::{load_a4_preset_onto_track, revert_a4_track, A4KitIo};
use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::a4_kit::{read_kit, sound_slot, splice_sound, NUM_SOUNDS};
use digi_protocol::drive::a4_preset_sound;

fn arg(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn main() {
    // SYN4 by default: the last synth track, and the one least likely to be
    // carrying the sound somebody is in the middle of using.
    let track: u8 = arg("--track").and_then(|s| s.parse().ok()).unwrap_or(4);
    let preset = arg("--preset").unwrap_or_else(|| "/soundbanks/A/1".to_string());
    let read_only = flag("--read-only");
    if track == 0 || usize::from(track) > NUM_SOUNDS {
        eprintln!("--track must be 1-{NUM_SOUNDS} (SYN1 to SYN4)");
        std::process::exit(2);
    }
    let slot = track - 1;

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let Some(input) = inputs.iter().find(|p| p.slug == Some("analogfour")) else {
        eprintln!("no Analog Four on this desk");
        std::process::exit(1);
    };
    let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
        eprintln!("{}: no output port of the same name", input.name);
        std::process::exit(1);
    };
    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
        .expect("could not open the A4's ports");
    let identity = device.identify().expect("the A4 did not answer the handshake");
    println!("=== {} — {} (build {}) ===", identity.name, identity.version, identity.build);

    // --- 1. read -------------------------------------------------------------

    let before = match device.fetch_a4_working_kit() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not read the working kit: {e}");
            std::process::exit(1);
        }
    };
    let kit = read_kit(0, &before).expect("the working kit should decode");
    println!("\n1. READ  {} bytes — kit {:?}", before.len(), kit.name);
    for n in 0..NUM_SOUNDS {
        println!("     SYN{}  {}", n + 1, kit.sound_name(n).unwrap_or("(none)"));
    }
    if flag("--restore") {
        // **Undoing this probe, the box's own way, over MIDI.** Reloading the
        // pattern on the box discards an unsaved kit and brings the stored one
        // back; that is the undo every load in this app rests on. This does the
        // same thing for one slot without anyone walking to the box: read the
        // *stored* kit, take its sound for this track, splice it into the
        // working kit and send that back. Only the slot named moves.
        let stored = device.fetch_a4_kit(0).expect("stored kit 0");
        let want = sound_slot(&stored, usize::from(slot)).expect("a slot").to_vec();
        let spliced =
            splice_sound(&before, usize::from(slot), &want).expect("a splice");
        println!(
            "\nRESTORE SYN{track} from stored kit 0 — {:?} over {:?}",
            read_kit(0, &stored).unwrap().sound_name(usize::from(slot)).unwrap_or("(none)"),
            kit.sound_name(usize::from(slot)).unwrap_or("(none)")
        );
        device.store_a4_working_kit(&spliced).expect("the store failed");
        device.settle();
        let end = device.fetch_a4_working_kit().expect("a kit read");
        println!(
            "   SYN{track} now reads {:?}, and the slot is {}",
            read_kit(0, &end).unwrap().sound_name(usize::from(slot)).unwrap_or("(none)"),
            if sound_slot(&end, usize::from(slot)).unwrap() == want.as_slice() {
                "byte-for-byte the stored kit's"
            } else {
                "*** not the stored kit's ***"
            }
        );
        return;
    }
    if let Some(path) = arg("--diff") {
        // What did the box make of the sound it was sent? Compare the slot it
        // now holds against the file's own 350 bytes, field by field and then
        // byte by byte. This is the mode that answered the one surprise this
        // probe found — see the file header.
        let file = device.drive_read_file(&path).expect("the +Drive read failed");
        let sound = a4_preset_sound(&file).expect("an A4 preset");
        let slot_bytes = sound_slot(&before, usize::from(slot)).expect("a slot");
        println!("\nDIFF SYN{track} against {path}");
        let field = |name: &str, at: usize, len: usize| {
            println!("  {name:<8} box  {:02x?}", &slot_bytes[at..at + len]);
            println!("           file {:02x?}", &sound[at..at + len]);
        };
        field("head", 0, 4);
        field("version", 4, 4);
        field("tagmask", 8, 4);
        println!(
            "  name     box {:<40} file {:?}",
            format!("{:?}", String::from_utf8_lossy(&slot_bytes[12..28])),
            String::from_utf8_lossy(&sound[12..28])
        );
        let differing: Vec<usize> =
            (0..350).filter(|&i| slot_bytes[i] != sound[i]).collect();
        println!("  {} of 350 bytes differ", differing.len());
        if !differing.is_empty() {
            println!("  at: {:?}", differing);
        }
        println!(
            "  past the name (byte 28 on): {} differ",
            differing.iter().filter(|&&i| i >= 28).count()
        );
        return;
    }
    if read_only {
        // The stored kit beside the working one, because that is what the box's
        // own undo restores: reloading the pattern discards the edit buffer and
        // brings this back.
        if let Ok(stored) = device.fetch_a4_kit(0) {
            if let Ok(kit) = read_kit(0, &stored) {
                println!("\n   stored kit 0 — what reloading the pattern would bring back");
                println!("     kit {:?}", kit.name);
                for n in 0..NUM_SOUNDS {
                    println!("     SYN{}  {}", n + 1, kit.sound_name(n).unwrap_or("(none)"));
                }
            }
        }
        println!("\n--read-only: stopping before anything is sent");
        return;
    }

    // --- 2. store what was already there -------------------------------------

    println!("\n2. STORE the same {} bytes back (0x58, DIN-paced)", before.len());
    if let Err(e) = device.store_a4_working_kit(&before) {
        eprintln!("   the store failed on the wire: {e}");
        std::process::exit(1);
    }
    device.settle();
    let after = match device.fetch_a4_working_kit() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("   the box stopped answering after the store: {e}");
            eprintln!("   power-cycle it before trying again");
            std::process::exit(1);
        }
    };
    if after == before {
        println!("   the box still holds the same kit, byte for byte");
        println!("   — which is what a store of these bytes and an ignored store both look");
        println!("     like. Stage 3 is what tells them apart.");
    } else {
        println!("   *** the kit CHANGED after storing its own bytes back ***");
        let now = read_kit(0, &after);
        println!("   now: {now:?}");
        eprintln!("   stopping: the box did something with that message that nobody predicted");
        std::process::exit(1);
    }

    // --- 3. load a preset, then put it back ----------------------------------

    println!("\n3. LOAD {preset} onto SYN{track}");
    let report = match load_a4_preset_onto_track(&mut device, &preset, slot) {
        Ok(r) => {
            println!("   loaded {:?}, displacing {:?}", r.loaded, r.replaced);
            println!("   the box's own kit read back with it on SYN{track} — twice");
            r
        }
        Err(e) => {
            eprintln!("   FAILED: {e}");
            eprintln!("\n   If that says the kit did not take, the answer to this probe is");
            eprintln!("   no: this box does not store a 0x58. Reload the pattern on the box.");
            std::process::exit(1);
        }
    };

    println!("\n4. REVERT SYN{track} from the backup the load handed back");
    match revert_a4_track(&mut device, slot, &report.backup) {
        Ok(name) => println!("   SYN{track} reads {name:?} again"),
        Err(e) => {
            eprintln!("   FAILED: {e}");
            eprintln!("   reload the pattern on the box to discard this kit");
            std::process::exit(1);
        }
    }

    let end = device.fetch_a4_working_kit().expect("a final read");
    println!(
        "\nfinal: the kit is {}",
        if end == before {
            "byte-for-byte what this probe found".to_string()
        } else {
            format!("NOT what this probe found — {} bytes differ",
                end.iter().zip(&before).filter(|(a, b)| a != b).count())
        }
    );
}
