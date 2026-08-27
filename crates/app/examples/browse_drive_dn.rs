// List a Digitone's +Drive through the 0x53 file API, and dump the entry bytes
// raw so the per-entry layout can be derived from a real reply.
//
// **Read-only, and guarded.** Every call goes through
// `digi_protocol::drive::assert_read_only_file_op`, which admits only 0x53 List,
// 0x54 Open, 0x55 Read and 0x56 Close. This matters more here than anywhere
// else in the project: in *this* namespace 0x57/0x58/0x59 write and **0x5C
// deletes**, so the dump path's "0x5n never appears in this file, therefore it
// cannot write" reasoning does not apply. Only List is ever sent below.
//
// # Why this exists
//
// The DN2's preset library is not reachable by any dump opcode — swept and
// ruled out — and the DN2 does not answer elk-herd's 0x10 DirList either. It
// answers a *second* file API, 0x53-0x5C, documented by Ángel Linares García
// (DNX) in digi-roll's `docs/plus-drive-file-api.md` and credited in
// `CREDITS.md`. Under a dump header 0x53 is a Sound dump; under the API header
// it is List. Reading the DN2's advertised `50-5E` as dump types instead of as
// that API's opcode list is what made this project conclude, wrongly, that the
// DN2 had no +Drive at all.
//
// # What is and is not known
//
// The reply *header* is fully specified and `parse_list_reply` handles it:
// status byte, echoed start, next cursor, entry count. The **per-entry layout
// is not** — the source document withheld a populated capture on purpose,
// because entries carry real project and preset names. All it records is the
// tail of a long-form entry (u32be index, u32be size, u16be permissions, then a
// two-byte occupancy pair) and that two layouts exist, told apart by a
// per-entry byte.
//
// So this example prints entries as raw hex rather than pretending to parse
// them. With `count` known from the header and the byte region in hand, the
// stride falls out by division, and the names will be visible in the ASCII
// column. That is the evidence needed to write the parser — and writing it
// before having it would be inventing a layout.
//
// Paths from the document's own captures: `/projects`, `/soundbanks/H` (236
// entries there), `/kits/A` (96). The bank letters are the thing to confirm.
//
// Run with:
//   cargo run -p digi_roll_studio --example browse_drive_dn
//   cargo run -p digi_roll_studio --example browse_drive_dn -- --path /soundbanks/A

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::parse_list_entries;

/// Paths to try when none is named. Roots first, then the collections the
/// source document names, then every bank letter.
fn default_paths() -> Vec<String> {
    let mut paths: Vec<String> = ["/", "/projects", "/soundbanks", "/kits", "/presets", "/sounds"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for bank in 'A'..='H' {
        paths.push(format!("/soundbanks/{bank}"));
    }
    paths.push("/kits/A".to_string());
    paths
}

/// How much of the entry region to print. Enough to see several entries and
/// find the stride; not so much that a 236-entry bank floods the terminal.
const HEX_BUDGET: usize = 512;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let paths: Vec<String> = args
        .windows(2)
        .find(|w| w[0] == "--path")
        .map(|w| vec![w[1].clone()])
        .unwrap_or_else(default_paths);

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
            "\n=== {} — build {}, version {}, product id {} ===",
            identity.name, identity.build, identity.version, identity.product_id
        );

        for path in &paths {
            match device.drive_list(path, 0, 0) {
                Ok(reply) if !reply.ok => println!(
                    "  {path:<18} refused: {}",
                    reply.message.as_deref().unwrap_or("(no message)")
                ),
                Ok(reply) => {
                    println!(
                        "  {path:<18} OK  {} entr{}, start {}, next cursor {}, {} entry bytes",
                        reply.count,
                        if reply.count == 1 { "y" } else { "ies" },
                        reply.start,
                        reply.next_cursor,
                        reply.entry_bytes.len()
                    );
                    // The parser derived from these captures, checked against
                    // the count the device declared. A mismatch means the
                    // layout is wrong, and the raw dump below is then the
                    // useful output rather than a list of guesses.
                    match parse_list_entries(&reply.entry_bytes, reply.count) {
                        Ok(entries) => {
                            let occupied: Vec<&digi_protocol::drive::ListEntry> =
                                entries.iter().filter(|e| e.is_occupied() || e.children.is_some()).collect();
                            println!(
                                "       parsed {} entries, {} occupied",
                                entries.len(),
                                occupied.len()
                            );
                            for e in occupied.iter().take(12) {
                                if let Some(children) = e.children {
                                    println!("         {:<20} dir, {children} children", e.name);
                                } else {
                                    println!(
                                        "         {:>3}  {:<20} {} bytes",
                                        e.index.unwrap_or(0),
                                        e.name,
                                        e.size.unwrap_or(0)
                                    );
                                }
                            }
                            if occupied.len() > 12 {
                                println!("         … {} more occupied", occupied.len() - 12);
                            }
                            continue;
                        }
                        Err(e) => println!("       LAYOUT DISAGREES: {e}"),
                    }
                    if reply.count > 0 && !reply.entry_bytes.is_empty() {
                        // The stride, if the entries are fixed-width. A clean
                        // division is itself evidence; a remainder says the
                        // entries are variable-length (a name field would do
                        // that) and the layout needs reading, not dividing.
                        let len = reply.entry_bytes.len();
                        let n = reply.count as usize;
                        if len % n == 0 {
                            println!("       stride: {} bytes/entry (exact)", len / n);
                        } else {
                            println!(
                                "       {len} bytes / {n} entries = {} rem {} — variable-length entries",
                                len / n,
                                len % n
                            );
                        }
                        dump_hex(&reply.entry_bytes);
                    }
                }
                Err(e) => println!("  {path:<18} {e}"),
            }
        }
    }
}

/// Classic hex + ASCII dump. The ASCII column is the point: preset names will
/// be legible in it, which is what identifies the name field's offset.
fn dump_hex(bytes: &[u8]) {
    for (row, chunk) in bytes.iter().take(HEX_BUDGET).collect::<Vec<_>>().chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        println!("       +{:04x}  {:<47}  |{ascii}|", row * 16, hex.join(" "));
    }
    if bytes.len() > HEX_BUDGET {
        println!("       … {} more bytes", bytes.len() - HEX_BUDGET);
    }
}
