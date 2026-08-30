// Read a whole *project* off one box's +Drive and write it to disk.
//
// **Read-only, and guarded rather than merely intended.** Everything here goes
// through `ElektronDevice::drive_read_file`, whose opcodes pass
// `assert_read_only_file_op` — the positive allowlist admitting List, Open,
// Read and Close and nothing else. `0x5C` in the same namespace deletes, so
// "this file contains no write opcode" is not the safety property; the
// allowlist is (PLAN.md §10, the safety inversion).
//
// # Why this exists when `capture_drive_file` already reads named paths
//
// Two reasons, both about scale.
//
//   1. **It reads every slugged box in turn.** A project slot is 2 MiB on an A4
//      and 16 MiB on a DT2/DN2 — the latter is exactly `MAX_CHUNKS × READ_CHUNK`,
//      the ceiling of the read path — so "read /projects/1" aimed at the whole
//      desk is three long transfers to answer a question about one box. This
//      takes `--port`.
//   2. **Its reporting is sized for a preset.** It prints every offset of each
//      container magic and every printable run of 4+ across the file. On a
//      359-byte sound that is a readable summary; on 2 MiB it is a flood that
//      buries the one line you opened it for. This counts and samples instead.
//
// # What is genuinely unknown here
//
// Nothing has ever read a project file over this API. The fifteen files the
// read path is verified on are all presets, and **every one of them fit in a
// single 4 KB chunk** — so the multi-chunk loop in `drive_read_file`, its
// sequence check and its short-chunk terminator have never run against
// hardware at length. A 2 MiB project is 512 chunks. That is the thing this
// example is really testing; the container layout is a bonus.
//
// Run with:
//   cargo run -p digi_roll_studio --example capture_drive_project -- --port "Analog Four"
//   cargo run -p digi_roll_studio --example capture_drive_project -- \
//       --port "Analog Four" --path /projects/1 --out local/a4-project

use std::path::PathBuf;
use std::time::Instant;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::FILE_MAGIC;
use digi_protocol::sound::{SOUND_MAGIC_FOOT, SOUND_MAGIC_HEAD};

/// The A4's container magic, as `capture_drive_file` names it.
const A4_MAGIC: u32 = 0xBEEF_BABA;

/// How many offsets of a repeated magic to print before saying "and N more".
/// A project holds a sound pool, so these repeat by the hundred and the
/// interesting facts are the *count* and the *stride*, not the list.
const OFFSETS_SHOWN: usize = 8;

/// How many printable runs to print. Same reasoning.
const RUNS_SHOWN: usize = 40;

/// The shortest printable run worth reporting. 4 is what a format tag or a
/// slot name looks like.
const RUN_MIN: usize = 4;

/// FNV-1a 64. Not a checksum the protocol knows about — it exists so two reads
/// of the same slot, or a read and a read-back after a write, can be compared
/// in one line instead of by diffing two megabyte files by eye.
fn digest(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn arg<'a>(argv: &'a [String], flag: &str) -> Option<&'a String> {
    argv.windows(2).find(|w| w[0] == flag).map(|w| &w[1])
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let Some(port) = arg(&argv, "--port") else {
        eprintln!("give --port, e.g. --port \"Analog Four\" — this reads megabytes, so it");
        eprintln!("does not fan out across the desk the way capture_drive_file does");
        std::process::exit(2);
    };
    let path = arg(&argv, "--path").cloned().unwrap_or_else(|| "/projects/1".to_string());
    let out: PathBuf = arg(&argv, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("local/drive-capture"));
    std::fs::create_dir_all(&out).expect("could not create the output directory");

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    let Some(input) = inputs.iter().find(|p| p.name.contains(port.as_str())) else {
        eprintln!("no input port matching {port:?}");
        std::process::exit(1);
    };
    let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
        eprintln!("{}: no matching output port", input.name);
        std::process::exit(1);
    };

    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
        .unwrap_or_else(|e| panic!("{}: could not open: {e}", input.name));

    // Identify first, and print it, for the reason `ui::presets` identifies
    // before it scans: the capture is named after whatever *answered*, so a
    // mis-cabled desk yields a file with the wrong box's name on it otherwise.
    let identity = device.identify().unwrap_or_else(|e| panic!("{}: no identity: {e}", input.name));
    println!(
        "=== {} — build {}, version {}, product id {} ===",
        identity.name, identity.build, identity.version, identity.product_id
    );

    // The listing's declared size, so the read has something to be checked
    // against that did not come from the read itself. A project path is
    // `/projects/<n>`, so its parent is the directory to list.
    //
    // **The last component is a slot number, not the entry's name.** `/projects/1`
    // is the A4's slot 1, whose entry is named `PRESETS` — matching on the name
    // finds nothing and reports "no declared size" for a file the listing
    // describes perfectly well. So match the 1-based `index` when the component
    // is numeric, and fall back to the name for a path that really is named.
    let declared: Option<u32> = path.rsplit_once('/').and_then(|(dir, leaf)| {
        let dir = if dir.is_empty() { "/" } else { dir };
        let reply = device.drive_list(dir, 0, 0).ok()?;
        let entries = digi_protocol::drive::parse_list_entries(&reply.entry_bytes, reply.count).ok()?;
        let found = match leaf.parse::<u32>() {
            Ok(slot) => entries.iter().find(|e| e.index == Some(slot)),
            Err(_) => entries.iter().find(|e| e.name == leaf),
        }?;
        println!("  {path}: the listing calls it {:?}", found.name);
        found.size
    });
    match declared {
        Some(n) => println!("  {path}: listing declares {n} bytes"),
        None => println!("  {path}: the listing does not declare a size for it"),
    }

    let started = Instant::now();
    let bytes = match device.drive_read_file(&path) {
        Ok(b) => b,
        Err(e) => {
            println!("  {path}: read refused: {e}");
            std::process::exit(1);
        }
    };
    let took = started.elapsed();

    let path_slug = path.trim_start_matches('/').replace('/', "-");
    let file = out.join(format!("{}-{path_slug}.bin", identity.name.replace(' ', "")));
    std::fs::write(&file, &bytes).expect("could not write the capture");

    println!("\n  read {} bytes in {:.1}s -> {}", bytes.len(), took.as_secs_f32(), file.display());
    println!("  fnv1a64: {:#018x}", digest(&bytes));
    match declared {
        Some(n) if n as usize == bytes.len() => println!("  length agrees with the listing"),
        Some(n) => println!("  LENGTH DISAGREES: listing said {n}, read got {}", bytes.len()),
        None => {}
    }

    let u32_at = |i: usize| {
        bytes.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    };
    println!(
        "  file magic at 0: {:?}{}",
        u32_at(0).map(|m| format!("{m:#010x}")),
        if u32_at(0) == Some(FILE_MAGIC) { " (FILE_MAGIC — a real file)" } else { "" }
    );

    for (name, magic) in [
        ("BEEFBACE", SOUND_MAGIC_HEAD),
        ("BEEFBABA", A4_MAGIC),
        ("BACEF00C", SOUND_MAGIC_FOOT),
    ] {
        let needle = magic.to_be_bytes();
        let at: Vec<usize> =
            bytes.windows(4).enumerate().filter(|(_, w)| *w == needle).map(|(i, _)| i).collect();
        if at.is_empty() {
            println!("  {name}: nowhere");
            continue;
        }
        // The stride is the fact worth having: a constant gap between
        // occurrences is a table of fixed-size records, which is what a sound
        // pool is and what the next session would want to walk.
        let strides: Vec<usize> = at.windows(2).map(|w| w[1] - w[0]).collect();
        let mut uniq: Vec<usize> = strides.clone();
        uniq.sort_unstable();
        uniq.dedup();
        let shown: Vec<usize> = at.iter().copied().take(OFFSETS_SHOWN).collect();
        println!(
            "  {name}: {} occurrence{}, first {shown:?}{}",
            at.len(),
            if at.len() == 1 { "" } else { "s" },
            if at.len() > OFFSETS_SHOWN { format!(" and {} more", at.len() - OFFSETS_SHOWN) } else { String::new() }
        );
        if !uniq.is_empty() {
            let head: Vec<usize> = uniq.iter().copied().take(OFFSETS_SHOWN).collect();
            println!(
                "    strides: {} distinct, {head:?}{}",
                uniq.len(),
                if uniq.len() > OFFSETS_SHOWN { " …" } else { "" }
            );
        }
    }

    // Printable runs, counted rather than listed. A name in a project is what
    // this is looking for, and on 2 MiB the list is thousands long.
    let mut runs: Vec<(usize, String)> = Vec::new();
    let (mut start, mut cur) = (0usize, String::new());
    for (i, b) in bytes.iter().enumerate() {
        if b.is_ascii_graphic() || *b == b' ' {
            if cur.is_empty() {
                start = i;
            }
            cur.push(*b as char);
        } else {
            if cur.len() >= RUN_MIN {
                runs.push((start, std::mem::take(&mut cur)));
            }
            cur.clear();
        }
    }
    if cur.len() >= RUN_MIN {
        runs.push((start, cur));
    }
    println!("\n  ascii runs ({RUN_MIN}+): {} total, first {RUNS_SHOWN}:", runs.len());
    for (at, run) in runs.iter().take(RUNS_SHOWN) {
        println!("    +{at:#08x}  {run:?}");
    }

    let hexdump = |label: &str, base: usize, slice: &[u8]| {
        println!("\n  {label}:");
        for (row, chunk) in slice.iter().collect::<Vec<_>>().chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            let ascii: String =
                chunk.iter().map(|b| if b.is_ascii_graphic() { **b as char } else { '.' }).collect();
            println!("    +{:08x}  {:<47}  |{ascii}|", base + row * 16, hex.join(" "));
        }
    };
    hexdump("first 128 bytes", 0, &bytes[..bytes.len().min(128)]);

    // The tail matters as much as the head here, and for a reason specific to
    // this read: `drive_read_file` ends on a *short chunk*, so a transfer that
    // died early and one that reached the end look identical from the outside.
    // A tail of padding says the file ended; a tail cut mid-record says it did
    // not, and the digest above is then a digest of a truncation.
    let tail = 128.min(bytes.len());
    hexdump("last 128 bytes", bytes.len() - tail, &bytes[bytes.len() - tail..]);
}
