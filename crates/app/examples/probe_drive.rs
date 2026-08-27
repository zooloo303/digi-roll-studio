// Ask each connected box what its +Drive tree actually contains.
//
// **Read-only.** Two API requests to identify, then `0x10` DirList only. The
// mutating +Drive opcodes (0x11 DirCreate, 0x12 DirDelete, 0x20 FileDelete,
// 0x21 ItemRename, and the whole 0x4n write family) are not implemented in
// `digi_protocol::drive` at all, so this example could not damage a +Drive even
// if it tried to.
//
// # The question
//
// `fetch_preset_pool` established that the dump path reaches the project's
// 128-slot sound pool and that the pool is nearly empty in normal use — the
// presets a project is actually using live in its *kit*, and the library of
// hundreds lives on the +Drive. The +Drive is reached by the API path instead.
//
// elk-herd implements that API and points it exclusively at **samples**:
// `Elektron/Drive.elm` is "a model of the +Drive sample tree", with the roots
// `/`, `/factory` and `/trash`. It never asks whether presets are in the same
// tree, because a Digitakt sample-library manager never needed to know.
//
// So the format is ported and the tree's shape is not documented, and this is
// the cheapest experiment that settles it: walk from the root and print what
// comes back. Three outcomes, all useful:
//
//   * the tree contains presets, under some path — the browser is buildable on
//     DirList plus the 0x3n file-read family, and we now know where to look;
//   * the tree is samples only — presets are reached some other way, and the
//     `0x09` Query opcode is the next thing to ask;
//   * the box does not list 0x10 in its supported ids — DN2/DT2 do not expose a
//     +Drive file system at all, and `TAG_NAMES` calibration off a real library
//     is off the table until Overbridge is the source.
//
// Whichever it is, it is worth more than another round of reading Elm.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_drive
//   cargo run -p digi_roll_studio --example probe_drive -- --depth 3

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::DirEntry;

/// The 0x1n and 0x3n opcodes worth reporting on, since between them they decide
/// whether a preset browser is possible at all.
const INTERESTING: &[(u8, &str)] = &[
    (0x09, "Query"),
    (0x10, "DirList"),
    (0x23, "SampleFileInfo"),
    (0x30, "FileReadOpen"),
    (0x32, "FileRead"),
];

/// Directories to try even if the root does not name them — elk-herd knows
/// `/factory` exists on a Digitakt without it necessarily being listed, and a
/// preset tree may be similarly unlisted.
const BLIND_GUESSES: &[&str] = &[
    "/factory",
    "/presets",
    "/sounds",
    "/Presets",
    "/Sounds",
    "/preset",
    "/patterns",
    "/projects",
];

fn main() {
    let depth: usize = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--depth")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(2);

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
        println!(
            "\n=== {} — build {}, version {} ===",
            identity.name, identity.build, identity.version
        );

        // What the box says it supports. This is free — it came back with the
        // identity — and it is the difference between "no answer" meaning
        // "unsupported" and meaning "we got the format wrong".
        let ids = &identity.supported_ids;
        println!("  supported message ids ({}): {}", ids.len(), hex_list(ids));
        for (id, name) in INTERESTING {
            println!(
                "    0x{id:02x} {name:<15} {}",
                if ids.contains(id) { "yes" } else { "NOT LISTED" }
            );
        }

        if !ids.contains(&0x10) {
            println!(
                "\n  This box does not list 0x10 DirList. Sending it anyway, once, because\n  \
                 the id list has been wrong before and one 5s timeout is cheap:"
            );
        }

        match device.dir_list("/") {
            Ok(entries) => {
                println!("\n  / — {} entr{}", entries.len(), plural(entries.len()));
                print_entries(&entries, 2);
                walk(&mut device, &entries, "", depth, 1);
            }
            Err(e) => {
                println!("\n  / — no listing: {e}");
                println!("  So either 0x10 is unsupported here, or the request format is wrong.");
            }
        }

        // Blind guesses, only for paths the root did not already name.
        println!("\n  blind guesses:");
        for path in BLIND_GUESSES {
            match device.dir_list(path) {
                // Measured on a DT2 (1.15C) 2026-08-26: *every* nonexistent path
                // answers with an empty listing rather than an error, including
                // nonsense ones. So empty means "no such directory" and not "an
                // empty directory exists here" — do not read it as a hit.
                Ok(entries) if entries.is_empty() => {
                    println!("    {path:<12} empty — i.e. no such directory")
                }
                Ok(entries) => {
                    println!("    {path:<12} {} entr{}", entries.len(), plural(entries.len()));
                    print_entries(&entries, 6);
                }
                Err(_) => println!("    {path:<12} no"),
            }
        }
    }
}

/// Recurse into subdirectories, breadth-first per level, to `depth`.
fn walk(device: &mut ElektronDevice, entries: &[DirEntry], prefix: &str, depth: usize, at: usize) {
    if at >= depth {
        return;
    }
    for entry in entries.iter().filter(|e| e.is_dir()) {
        let path = format!("{prefix}/{}", entry.name);
        match device.dir_list(&path) {
            Ok(kids) => {
                println!(
                    "{:indent$}{path} — {} entr{}",
                    "",
                    kids.len(),
                    plural(kids.len()),
                    indent = 2 + at * 2
                );
                print_entries(&kids, 4 + at * 2);
                walk(device, &kids, &path, depth, at + 1);
            }
            Err(e) => println!("{:indent$}{path} — {e}", "", indent = 2 + at * 2),
        }
    }
}

/// At most a dozen entries per directory: a sample library has hundreds, and the
/// question here is what *kind* of thing is in the tree, not the full inventory.
fn print_entries(entries: &[DirEntry], indent: usize) {
    for entry in entries.iter().take(12) {
        println!(
            "{:indent$}{} {:<24} {:>9} bytes  hash {:08x}{}",
            "",
            entry.kind,
            entry.name,
            entry.size,
            entry.hash,
            if entry.locked { "  [locked]" } else { "" },
            indent = indent
        );
    }
    if entries.len() > 12 {
        println!("{:indent$}… {} more", "", entries.len() - 12, indent = indent);
    }
}

fn hex_list(ids: &[u8]) -> String {
    ids.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        "y"
    } else {
        "ies"
    }
}
