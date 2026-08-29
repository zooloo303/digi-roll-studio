// Pull whole preset files off a box's +Drive and write them to disk, so the
// container layer can be built at a desk with no box on it.
//
// # Why this exists as its own example
//
// `read_drive_preset.rs` proves `drive_read_file` works — five files per box,
// three independently-sourced sizes agreeing on each. It prints and discards.
// That was the right shape for "does the transport work", and it is the wrong
// shape for what comes next: **the container layer needs the bytes, not a
// report about them.**
//
// PLAN.md §10.2 is the work — the struct inside a preset file is shorter than
// the file (299 on a DT2, 319 or 359 on a DN2), only one of those lengths is in
// `KNOWN_SOUND_SIZES`, a DT2 file carries a second `BEEFBACE` at 1060, and the
// A4's container magic is `BEEFBABA` where `decode_sound_dump` hard-requires
// `BEEFBACE`. None of that is guessable and all of it is derivable from files.
// Deriving it wants the files committed, the way the plock and tagged-sound
// `.syx` captures under `crates/protocol/tests/fixtures/` already are.
//
// # Read-only, and structurally so
//
// Every opcode here goes through `assert_read_only_file_op`, which admits List,
// Open, Read and Close and refuses Move, Copy and Delete. This example cannot
// write to a +Drive because the layer beneath it will not carry the opcodes
// that would, and that guard is the safety property in this namespace rather
// than anything in this file.
//
// # What lands on disk, and why the manifest matters more than it looks
//
// One `.bin` per preset, plus a `manifest.tsv` recording where each came from:
// box, OS build, +Drive path, the name and size the *listing* declared, the
// size the file's *own* header declares at +27, and where the container magic
// was found. **A capture with no provenance is not evidence** — §9's standard
// is that a claim names the box and the build it came from, and a directory of
// anonymous `.bin` files could not meet it six months from now.
//
// The listing's size and the header's size are recorded separately on purpose.
// They come from different places in the box, and `read_drive_preset.rs`
// checking them against each other is what made the transport believable. A
// fixture that quietly disagrees with its own listing is one this project would
// rather find in the manifest than in a parser.
//
// # Preset names are captured and committed, decided 2026-08-29
//
// Earlier captures in this project deliberately carried no user-authored preset
// names — `drive.rs`'s file-read tests say so and give the reason. That
// restraint was about a *listing* withheld by the source document's author, and
// it is not a rule about this desk's own boxes. Owner's decision, taken
// knowingly: these files are captured whole, names and tag masks included,
// because a tag index cannot be derived from files with the tags removed.
//
// Run with:
//   cargo run -p digi_roll_studio --example capture_drive_presets
//   cargo run -p digi_roll_studio --example capture_drive_presets -- --path /soundbanks/B
//   cargo run -p digi_roll_studio --example capture_drive_presets -- --count 16
//   cargo run -p digi_roll_studio --example capture_drive_presets -- --out /tmp/presets

use std::fmt::Write as _;
use std::path::PathBuf;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::{container_offset, file_declared_size, parse_list_entries};

/// How many presets to take from each directory on each box. Eight is enough
/// spread to tell a per-box constant from a per-file one — which is the whole
/// question the container layer turns on — and small enough that a run is a
/// minute rather than an afternoon.
const DEFAULT_COUNT: usize = 8;

/// Where captures land by default: alongside the `.syx` fixtures that the plock
/// and tag work already committed, in a subdirectory because there will be more
/// of these than there ever were of those.
const DEFAULT_OUT: &str = "crates/protocol/tests/fixtures/drive";

/// Stamped into every filename so a capture carries its date without anyone
/// having to read git. Overridable, because a re-run on another day should say
/// so rather than overwrite.
const DEFAULT_DATE: &str = "2026-08-29";

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let arg = |name: &str| argv.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());

    let dir = arg("--path").unwrap_or_else(|| "/soundbanks/A".to_string());
    let count = arg("--count").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_COUNT);
    let date = arg("--date").unwrap_or_else(|| DEFAULT_DATE.to_string());
    let out = PathBuf::from(arg("--out").unwrap_or_else(|| DEFAULT_OUT.to_string()));

    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("cannot create {}: {e}", out.display());
        std::process::exit(1);
    }
    println!("capturing up to {count} presets from {dir} into {}\n", out.display());

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    // One manifest for the whole run rather than one per box: the interesting
    // comparisons in this data are *across* boxes — 36-byte header against 31,
    // BEEFBACE against BEEFBABA — and a reader should not have to join three
    // files to see them.
    let mut manifest = String::from("box\tbuild\tpath\tlisted_name\tlisted_size\theader_size\tfile_len\tcontainer_at\tmagic\tfile\n");
    let mut captured = 0usize;

    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else { continue };
        let mut device =
            match ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output)) {
                Ok(d) => d,
                Err(e) => {
                    println!("{}: could not open: {e}", input.name);
                    continue;
                }
            };
        let identity = match device.identify() {
            Ok(id) => id,
            Err(e) => {
                println!("{}: no identity: {e}", input.name);
                continue;
            }
        };
        println!("=== {} — build {} ===", identity.name, identity.build);

        // The A4 is *not* skipped here, unlike the `0x5b` probe. It has no
        // `0x6x` dump family, but the file API is a separate question and it
        // answers that one — and its container is the odd one out, so it is the
        // box this capture most needs.
        let slug = input.slug.unwrap_or("unknown").to_string();

        let listing = match device.drive_list(&dir, 0, 0) {
            Ok(l) => l,
            Err(e) => {
                println!("  cannot list {dir}: {e}\n");
                continue;
            }
        };
        let entries = match parse_list_entries(&listing.entry_bytes, listing.count) {
            Ok(e) => e,
            Err(e) => {
                println!("  cannot parse the listing: {e}\n");
                continue;
            }
        };

        let targets: Vec<_> = entries
            .iter()
            .filter(|e| e.is_occupied() && e.children.is_none() && e.size.is_some_and(|s| s > 0))
            .take(count)
            .collect();
        if targets.is_empty() {
            println!("  nothing occupied in {dir}\n");
            continue;
        }

        for entry in targets {
            let index = entry.index.unwrap_or(0);
            let path = format!("{dir}/{index}");
            let listed = entry.size.unwrap_or(0);
            let name = entry.name.clone();

            let bytes = match device.drive_read_file(&path) {
                Ok(b) => b,
                Err(e) => {
                    println!("  {path}: read failed: {e}");
                    continue;
                }
            };

            // Recorded, not enforced. A disagreement here is a finding about the
            // format rather than a reason to drop the capture — and dropping the
            // one file that disagrees is how a parser comes to be built against
            // only the files that already fit it.
            let header = file_declared_size(&bytes);
            let at = container_offset(&bytes);
            let magic = at
                .and_then(|a| bytes.get(a..a + 4))
                .map(|m| format!("{:02X}{:02X}{:02X}{:02X}", m[0], m[1], m[2], m[3]))
                .unwrap_or_else(|| "none".to_string());

            let safe: String = name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            let file = format!("{slug}-{}-{index}-{}-{date}.bin", dir.trim_matches('/').replace('/', "-"), safe.trim_matches('-'));
            let full = out.join(&file);
            if let Err(e) = std::fs::write(&full, &bytes) {
                println!("  {path}: could not write {}: {e}", full.display());
                continue;
            }

            println!(
                "  {path:<20} {name:<18} listed {listed:>5}  header {:>5}  file {:>5}  container @{} {magic}",
                header.map(|h| h.to_string()).unwrap_or_else(|| "?".into()),
                bytes.len(),
                at.map(|a| a.to_string()).unwrap_or_else(|| "?".into()),
            );
            let _ = writeln!(
                manifest,
                "{}\t{}\t{path}\t{name}\t{listed}\t{}\t{}\t{}\t{magic}\t{file}",
                identity.name,
                identity.build,
                header.map(|h| h.to_string()).unwrap_or_else(|| "?".into()),
                bytes.len(),
                at.map(|a| a.to_string()).unwrap_or_else(|| "?".into()),
            );
            captured += 1;
        }
        println!();
    }

    if captured == 0 {
        println!("nothing captured — no box answered, or no directory had occupied entries");
        return;
    }
    let manifest_path = out.join("manifest.tsv");
    match std::fs::write(&manifest_path, &manifest) {
        Ok(()) => println!("{captured} files captured; provenance in {}", manifest_path.display()),
        // The bytes are already on disk, so this is a real loss of provenance
        // rather than a lost run — say so plainly instead of exiting 0 quietly.
        Err(e) => println!(
            "{captured} files captured BUT the manifest could not be written ({e}) — \
             these captures have no provenance until that is fixed"
        ),
    }
}
