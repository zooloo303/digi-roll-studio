// What is in an Analog Four's project sound pool, and what version are they?
//
// Read-only. The question behind it belongs to the preset-load path: a +Drive
// preset file's container is struct **version 5** and the box replaces a
// version-5 kit sound with an init sound named "SOUND n"
// (`a4_sound_convert_probe`), while a **version-6** sound spliced into the same
// slot lands byte for byte. So a load needs a version-6 container, and the pool
// is the one other place this protocol can fetch a sound from: `0x63` → `0x53`,
// 350 bytes, index = slot, 128 slots.
//
// Three things are worth reading off it:
//
//   * **the struct version of a pool sound**, which `a4_kit`'s header records as
//     6 from the project stream and which this asks the box directly;
//   * **whether any pool sound is named the same as a +Drive preset**, because
//     such a pair is a version-5 file and a version-6 rendering of the same
//     sound — the conversion, measured, with nobody's hands in it;
//   * **which slots are empty**, for any later probe that needs a pool slot it
//     can spend.
//
// Run with:
//   cargo run -p digi_roll_studio --example a4_sound_pool_probe
//   cargo run -p digi_roll_studio --example a4_sound_pool_probe -- --slots 16

use std::collections::BTreeMap;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::a4_kit::{sound_for_kit, V5_ONLY_BYTE};
use digi_protocol::drive::{a4_preset_sound, decode_drive_preset, parse_list_entries};
use digi_protocol::protocol::FAMILY_ANALOG_FOUR;
use digi_protocol::sound::decode_a4_sound;

/// `0x63`, the pool-sound request — the table in PLAN.md §10's dump map.
const DUMP_A4_SOUND_REQUEST: u8 = 0x63;
const SOUND_SIZE: usize = 350;

fn arg(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn main() {
    let slots: u8 = arg("--slots").and_then(|s| s.parse().ok()).unwrap_or(128);

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let input = inputs.iter().find(|p| p.slug == Some("analogfour")).expect("no A4 on this desk");
    let output = outputs.iter().find(|p| p.name == input.name).expect("no A4 output port");
    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
        .expect("could not open the A4's ports");
    let identity = device.identify().expect("the A4 did not answer");
    println!("=== {} — {} (build {}) ===", identity.name, identity.version, identity.build);

    // --- the pool ------------------------------------------------------------

    let mut pool: BTreeMap<String, u8> = BTreeMap::new();
    let mut versions: BTreeMap<u32, usize> = BTreeMap::new();
    let mut named = 0usize;
    for slot in 0..slots {
        let reply = match device.fetch_dump(FAMILY_ANALOG_FOUR, DUMP_A4_SOUND_REQUEST, slot) {
            Ok(r) => r,
            Err(e) => {
                println!("slot {slot}: {e}");
                continue;
            }
        };
        let sound = match decode_a4_sound(&reply.payload, SOUND_SIZE) {
            Ok(s) => s,
            Err(e) => {
                println!("slot {slot}: {} bytes, does not decode ({e})", reply.payload.len());
                continue;
            }
        };
        *versions.entry(sound.version).or_default() += 1;
        let name = sound.name.trim().to_string();
        // "SOUND n" is what an unused pool slot is called, and the pool on a
        // factory project is mostly that.
        if !name.is_empty() && name != format!("SOUND {}", slot + 1) {
            named += 1;
            pool.insert(name.clone(), slot);
            println!("  slot {:>3}  v{}  {name:?}  tags {:#010x}", slot, sound.version, sound.tag_mask);
        }
    }
    println!(
        "\n{named} of {slots} pool slots carry a name of their own; struct versions {versions:?}"
    );

    // --- against the +Drive --------------------------------------------------

    let listing = device.drive_list("/soundbanks/A", 0, 0).expect("a bank listing");
    let entries = parse_list_entries(&listing.entry_bytes, listing.count).expect("a listing");
    let mut pairs = Vec::new();
    for entry in entries.iter().filter(|e| e.is_occupied() && e.index.is_some()) {
        let name = entry.name.trim().to_string();
        if let Some(&slot) = pool.get(&name) {
            pairs.push((name, slot, entry.index.unwrap()));
        }
    }
    if pairs.is_empty() {
        println!(
            "no +Drive preset in /soundbanks/A shares a name with a pool sound — so this box \
             offers no version-5/version-6 pair of the same sound, and the conversion cannot \
             be measured from dumps alone"
        );
        return;
    }
    println!("\n--- the same sound in both versions ---");
    for (name, slot, index) in pairs {
        let file = device
            .drive_read_file(&format!("/soundbanks/A/{index}"))
            .expect("the +Drive read failed");
        // The 350-byte cut, not `decode_drive_preset`'s 366-byte declared
        // payload: a kit slot's stride is what both sides of this comparison
        // have to be measured in.
        let v5_bytes = a4_preset_sound(&file).expect("an A4 preset").to_vec();
        let v5 = decode_drive_preset(&file).expect("a preset");
        let reply = device
            .fetch_dump(FAMILY_ANALOG_FOUR, DUMP_A4_SOUND_REQUEST, slot)
            .expect("a pool sound");
        let v6 = decode_a4_sound(&reply.payload, SOUND_SIZE).expect("a pool sound");
        let differ: Vec<usize> =
            (0..SOUND_SIZE).filter(|&i| v5_bytes[i] != v6.bytes[i]).collect();
        // **The conversion, checked against the box's own copy.** If the two
        // differ in nothing but the version word and `V5_ONLY_BYTE`, then this
        // pair is a sound the pool has not been edited since, and
        // `sound_for_kit` must reproduce the box's version-6 bytes exactly.
        // That is the claim, and this is the only place it can be tested.
        if differ.iter().all(|&i| i == 7 || i == V5_ONLY_BYTE) {
            let converted = sound_for_kit(&v5_bytes).expect("a version 5 preset converts");
            println!(
                "  {name:<16} UNEDITED PAIR — sound_for_kit(file) {} the box's own version 6",
                if converted == v6.bytes { "== reproduces" } else { "*** DIFFERS FROM ***" }
            );
        }
        println!(
            "  {name:<16} file v{} pool v{} — {:>2} differ  126: {:>3} to {:<3}  235: {:>3} to \
             {:<3}  {}",
            v5.version,
            v6.version,
            differ.len(),
            v5_bytes[126],
            v6.bytes[126],
            v5_bytes[235],
            v6.bytes[235],
            if differ.iter().all(|&i| [7, 126, 235].contains(&i)) {
                "<- nothing else".to_string()
            } else {
                format!("also {:?}", differ.iter().filter(|&&i| ![7, 126, 235].contains(&i)).collect::<Vec<_>>())
            }
        );
    }
}
