// What does an Analog Four do with a **version-5** sound put into a kit slot?
//
// `a4_kit_store_probe` established that the box stores a `0x58` working kit:
// SYN1–3 came back untouched and the spliced slot changed. What it also found is
// that the slot does not come back as what was sent. A +Drive preset file's
// container is struct **version 5** — every one of the eight captured, and the
// factory banks predate the OS these kits were written by — and after the store
// the box's own slot reads:
//
//   * version **6**, not 5;
//   * tag mask **zeroed**, where the file carried `0x05840003`;
//   * name **"SOUND 4"**, not "THE SAW";
//   * 42 of the ~320 parameter bytes different from the ones sent.
//
// Two readings fit that, and they are opposites:
//
//   A. **The box converted the sound.** It read a version-5 struct, up-converted
//      it to version 6 — which is what those 42 bytes are — and gave the result
//      the default name a kit-local sound gets. The audition works; only the
//      name on screen is the box's rather than the preset's.
//   B. **The box refused the sound and re-initialised the slot.** "SOUND 4" is
//      exactly what an init sound is called, and the 42 bytes are init values
//      that happen to coincide with THE SAW everywhere else.
//
// # The experiment that separates them
//
// Load **two different presets** onto the same slot and compare what the box
// ends up holding. Under A the two results differ from each other by however
// much the two sounds differ. Under B they are byte-identical, because an init
// sound does not depend on what was sent.
//
// It finishes by putting the slot back to the bytes it found there.
//
// Run with:
//   cargo run -p digi_roll_studio --example a4_sound_convert_probe
//   cargo run -p digi_roll_studio --example a4_sound_convert_probe -- --track 4

use digi_midi::a4_preset_load::A4KitIo;
use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::a4_kit::{read_kit, sound_slot, splice_sound, NUM_SOUNDS, SOUND_SIZE};
use digi_protocol::drive::a4_preset_sound;

/// Two presets from the same bank, as different from each other as the factory
/// bank offers: a raw saw and a bass.
const FIRST: &str = "/soundbanks/A/1";
const SECOND: &str = "/soundbanks/A/8";

fn arg(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn main() {
    let track: u8 = arg("--track").and_then(|s| s.parse().ok()).unwrap_or(4);
    assert!(track >= 1 && usize::from(track) <= NUM_SOUNDS, "--track is 1-{NUM_SOUNDS}");
    let slot = usize::from(track - 1);

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let input = inputs.iter().find(|p| p.slug == Some("analogfour")).expect("no A4 on this desk");
    let output = outputs.iter().find(|p| p.name == input.name).expect("no A4 output port");
    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
        .expect("could not open the A4's ports");
    let identity = device.identify().expect("the A4 did not answer");
    println!("=== {} — {} (build {}) ===", identity.name, identity.version, identity.build);

    let opening = device.fetch_a4_working_kit().expect("a kit read");
    let opening_slot = sound_slot(&opening, slot).expect("a slot").to_vec();
    println!(
        "SYN{track} holds {:?} (version {})",
        read_kit(0, &opening).unwrap().sound_name(slot).unwrap_or("(none)"),
        u32::from_be_bytes([
            opening_slot[4],
            opening_slot[5],
            opening_slot[6],
            opening_slot[7]
        ])
    );

    // --- 0. a sound the box wrote itself ------------------------------------
    //
    // **Asked first, because it is what makes the file result interpretable.**
    // SYN1's own sound is struct version 6 and came out of this box's kit, so
    // splicing it into another slot changes nothing about the format and
    // everything about which slot holds it. If *this* re-initialises too, the
    // box re-inits any slot whose bytes changed and no splice can ever work; if
    // it lands, the store path is sound and the version is the whole question.
    let source = if slot == 0 { 1 } else { 0 };
    let own = sound_slot(&opening, source).expect("a slot").to_vec();
    let own_name =
        read_kit(0, &opening).unwrap().sound_name(source).unwrap_or("(none)").to_string();
    println!("\n0. splice SYN{}'s own {own_name:?} (version 6) onto SYN{track}", source + 1);
    let spliced = splice_sound(&opening, slot, &own).expect("a splice");
    device.store_a4_working_kit(&spliced).expect("the store failed");
    device.settle();
    let back = device.fetch_a4_working_kit().expect("a kit read");
    let got = sound_slot(&back, slot).expect("a slot");
    println!(
        "   the box now calls SYN{track} {:?}, and {} of {SOUND_SIZE} bytes differ from what \
         was sent",
        read_kit(0, &back).unwrap().sound_name(slot).unwrap_or("(none)"),
        (0..SOUND_SIZE).filter(|&i| got[i] != own[i]).count()
    );

    let mut results = Vec::new();
    for path in [FIRST, SECOND] {
        let file = device.drive_read_file(path).expect("the +Drive read failed");
        let sound = a4_preset_sound(&file).expect("an A4 preset").to_vec();
        let kit = device.fetch_a4_working_kit().expect("a kit read");
        let spliced = splice_sound(&kit, slot, &sound).expect("a splice");
        device.store_a4_working_kit(&spliced).expect("the store failed");
        device.settle();
        let back = device.fetch_a4_working_kit().expect("a kit read");
        let got = sound_slot(&back, slot).expect("a slot").to_vec();
        let name = read_kit(0, &back).unwrap().sound_name(slot).unwrap_or("(none)").to_string();
        let differ = (0..SOUND_SIZE).filter(|&i| got[i] != sound[i]).count();
        println!("\nsent {path}  — the box now calls SYN{track} {name:?}");
        println!("  {differ} of {SOUND_SIZE} bytes differ from what was sent");
        results.push((path, sound, got));
    }

    // --- the answer ----------------------------------------------------------

    let (_, sent_a, got_a) = &results[0];
    let (_, sent_b, got_b) = &results[1];
    let sent_differ = (0..SOUND_SIZE).filter(|&i| sent_a[i] != sent_b[i]).count();
    let got_differ = (0..SOUND_SIZE).filter(|&i| got_a[i] != got_b[i]).count();
    println!("\n--- the two presets against each other ---");
    println!("  as files:          {sent_differ} of {SOUND_SIZE} bytes differ");
    println!("  as the box holds them: {got_differ} of {SOUND_SIZE} bytes differ");
    if got_differ == 0 {
        println!("\n  => B. The slot does not depend on what was sent: the box refused both");
        println!("        sounds and re-initialised. There is no load path this way.");
    } else {
        println!("\n  => A. The box kept each sound: two different presets leave two");
        println!("        different slots, so the parameters landed. The name and the tag");
        println!("        mask are the box's own, and a verify must not compare names.");
    }

    // --- put it back ---------------------------------------------------------

    let kit = device.fetch_a4_working_kit().expect("a kit read");
    let restored = splice_sound(&kit, slot, &opening_slot).expect("a splice");
    device.store_a4_working_kit(&restored).expect("the restore failed");
    device.settle();
    let end = device.fetch_a4_working_kit().expect("a kit read");
    let end_slot = sound_slot(&end, slot).expect("a slot");
    println!(
        "\nrestored: SYN{track} is {}",
        if end_slot == opening_slot.as_slice() {
            "byte-for-byte what this probe found".to_string()
        } else {
            format!(
                "{:?}, and {} bytes differ from what it found — reload the pattern on the box",
                read_kit(0, &end).unwrap().sound_name(slot).unwrap_or("(none)"),
                (0..SOUND_SIZE).filter(|&i| end_slot[i] != opening_slot[i]).count()
            )
        }
    );
}
