// Read named +Drive files off a box and write them to disk, byte for byte.
//
// **Read-only, and guarded rather than merely intended.** Everything here goes
// through `ElektronDevice::drive_read_file`, whose opcodes pass
// `assert_read_only_file_op` — the positive allowlist admitting List, Open, Read
// and Close and nothing else. `0x5C` in the same namespace deletes, so "this
// file contains no write opcode" is not the safety property; the allowlist is.
//
// # Why this exists when `probe_drive_read` already reads files
//
// That probe reads the *first few* files in a directory, which is the right
// shape for deriving an argument layout and the wrong one for answering "what is
// in slot 205". This takes exact paths.
//
// # Why it writes the bytes out instead of only printing them
//
// It was written to identify 388 DN2 presets that carry no sound container, and
// the first attempt at that read the head bytes **off a screenshot of the panel**
// and found `444e3153` — ASCII `DN1S`, which would have meant Digitone mk1
// presets on a DN2's +Drive. It sits at a half-byte offset. The transcription
// had an odd number of hex characters and the match was a nibble-shift artifact
// of the misreading, not a field in the file.
//
// A 96-character hex string read by eye is not evidence, and the fix is not to
// read it more carefully. So this saves the file, and the analysis runs on
// bytes.
//
// Run with:
//   cargo run -p digi_roll_studio --example capture_drive_file -- --path /soundbanks/B/205
//   cargo run -p digi_roll_studio --example capture_drive_file -- \
//       --path /soundbanks/B/205 --path /soundbanks/B/206 --out local/dn2-odd

use std::path::PathBuf;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::{decode_drive_preset, FILE_MAGIC};
use digi_protocol::sound::{SOUND_MAGIC_FOOT, SOUND_MAGIC_HEAD};

/// The A4's container magic. Not imported from `drive` for the reason it is
/// printed at all: this example reports what it *found*, and a file carrying
/// neither known magic is the interesting case.
const A4_MAGIC: u32 = 0xBEEF_BABA;

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let paths: Vec<String> =
        argv.windows(2).filter(|w| w[0] == "--path").map(|w| w[1].clone()).collect();
    if paths.is_empty() {
        eprintln!("give at least one --path, e.g. --path /soundbanks/B/205");
        std::process::exit(2);
    }
    let out: PathBuf = argv
        .windows(2)
        .find(|w| w[0] == "--out")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("local/drive-capture"));
    std::fs::create_dir_all(&out).expect("could not create the output directory");

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
        // The slug names the tag table, and it is not defaultable — see
        // `sound::tag_names_for`. A port with none decodes to a mask with no
        // labels rather than to a digi's guess.
        let slug = input.slug.unwrap_or("");
        println!("\n=== {} — build {}, slug {slug:?} ===", identity.name, identity.build);

        for path in &paths {
            let bytes = match device.drive_read_file(path) {
                Ok(b) => b,
                Err(e) => {
                    println!("  {path}: read refused: {e}");
                    continue;
                }
            };

            let path_slug = path.trim_start_matches('/').replace('/', "-");
            let file = out.join(format!("{}-{path_slug}.bin", identity.name.replace(' ', "")));
            std::fs::write(&file, &bytes).expect("could not write the capture");
            println!("\n  {path}: {} bytes -> {}", bytes.len(), file.display());

            // The three questions the panel could not answer, asked of the bytes.
            let u32_at = |i: usize| {
                bytes
                    .get(i..i + 4)
                    .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
            };
            println!(
                "    file magic at 0: {:?}{}",
                u32_at(0).map(|m| format!("{m:#010x}")),
                if u32_at(0) == Some(FILE_MAGIC) { " (FILE_MAGIC — a real file)" } else { "" }
            );
            println!("    word at 36:      {:?}", u32_at(36).map(|m| format!("{m:#010x}")));

            for (name, magic) in
                [("BEEFBACE", SOUND_MAGIC_HEAD), ("BEEFBABA", A4_MAGIC), ("BACEF00C", SOUND_MAGIC_FOOT)]
            {
                let needle = magic.to_be_bytes();
                let at: Vec<usize> =
                    bytes.windows(4).enumerate().filter(|(_, w)| *w == needle).map(|(i, _)| i).collect();
                println!("    {name}: {}", if at.is_empty() { "nowhere".into() } else { format!("{at:?}") });
            }

            // Printable runs of 4+, which is what a format tag or a name looks
            // like and what the nibble-shifted `DN1S` only pretended to be.
            let mut runs: Vec<(usize, String)> = Vec::new();
            let (mut start, mut cur) = (0usize, String::new());
            for (i, b) in bytes.iter().enumerate() {
                if b.is_ascii_graphic() || *b == b' ' {
                    if cur.is_empty() {
                        start = i;
                    }
                    cur.push(*b as char);
                } else {
                    if cur.len() >= 4 {
                        runs.push((start, std::mem::take(&mut cur)));
                    }
                    cur.clear();
                }
            }
            if cur.len() >= 4 {
                runs.push((start, cur));
            }
            println!("    ascii runs (4+): {runs:?}");

            match decode_drive_preset(&bytes) {
                Ok(s) => println!("    decodes: {:?} tags {:?}", s.name, s.tags(slug)),
                Err(e) => println!("    decode refused: {e}"),
            }

            println!("    first 64 bytes:");
            for (row, chunk) in bytes.iter().take(64).collect::<Vec<_>>().chunks(16).enumerate() {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                let ascii: String = chunk
                    .iter()
                    .map(|b| if b.is_ascii_graphic() { **b as char } else { '.' })
                    .collect();
                println!("      +{:04x}  {:<47}  |{ascii}|", row * 16, hex.join(" "));
            }
        }
    }
}
