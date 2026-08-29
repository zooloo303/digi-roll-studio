// Derive the argument layouts of `0x54` Open, `0x55` Read and `0x56` Close by
// sending them to a real box and reading what comes back.
//
// **Read-only, and guarded rather than merely intended.** Every request here
// goes out through `ElektronDevice::drive_file_request`, which puts its opcode
// through `assert_read_only_file_op` — the positive allowlist admitting List,
// Open, Read and Close and nothing else. In this namespace `0x57`/`0x58`/`0x59`
// write and **`0x5C` deletes**, so "this file contains no write opcode" is not
// the safety property; the allowlist is, and it holds whatever this example
// passes it.
//
// # Why a probe and not a parser
//
// The source document — Ángel Linares García's `plus-drive-file-api.md`, see
// `CREDITS.md` — names all three opcodes and specifies the argument layout of
// none of them. Only `0x53` List's body is written down. So there is nothing to
// port here, and inventing a layout and calling a timeout "unsupported" is the
// mistake this project already made once when it read the DN2's advertised
// `50-5E` as dump types and concluded the box had no +Drive at all.
//
// # The hypothesis being tested, and where it comes from
//
// elk-herd implements the *gen-1* numbering for the same feature, and its Elm
// codecs are explicit about widths (`src/SysEx/Message.elm`):
//
//   0x30 FileReadOpen   req: path (NUL)                  rsp: ok, fd, total-len
//   0x32 FileRead       req: fd, chunk-len, chunk-start   rsp: ok, fd, len, start, end, data
//   0x31 FileReadClose  req: fd                           rsp: fd, total-len
//
// every integer a `u32be`. If `0x54`/`0x55`/`0x56` are that same API renumbered
// — which is what the DN1 answering both generations suggests — these layouts
// carry over unchanged. **One detail says they might not.** The document calls
// `0x55` "read a chunk by sequence number", and elk-herd's `0x32` addresses a
// chunk by byte offset. Those are different calls. So Read is tried under three
// layouts and the box gets to decide, rather than one being assumed and its
// failure blamed on the box.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_drive_read
//   cargo run -p digi_roll_studio --example probe_drive_read -- --path /kits/A

use std::time::Duration;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::{
    parse_list_entries, API_FILE_CLOSE, API_FILE_LIST, API_FILE_OPEN, API_FILE_READ,
};
use digi_protocol::sound::decode_sound;

/// How much of any reply to print. A whole preset is ~1 KB and the interesting
/// part is the header, but the tail matters for Read (the data region), so this
/// is generous.
const HEX_BUDGET: usize = 256;

/// Read this many bytes in the first chunk. Every known preset size fits well
/// inside it, so a single Read should return the whole file and the multi-chunk
/// question stays out of this session.
const CHUNK_LEN: u32 = 4096;

/// How long to keep listening after a reply before deciding the box has
/// finished. Generous: the cost of guessing short is concluding a read returns
/// one message when it returns nine.
const QUIET: Duration = Duration::from_millis(400);

/// How many presets to read per bank. Enough to show the layout holds across
/// files of different sizes; not so many that this becomes the scan itself.
const HOW_MANY: usize = 5;

/// The container's offset inside a +Drive file, and the framing a file carries
/// beyond its container — both measured, on a DT2 and a DN2, as the constant
/// difference between the bytes read and the size the listing declared.
const HEADER: u32 = 36;
const FRAMING: u32 = 43;

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

        // Advertised support is worth printing but not worth gating on: the A4
        // advertises 0x53-0x56 and has never had anything listed off it, so the
        // list is a claim about vocabulary and this example is the check.
        let advertised: Vec<String> = [API_FILE_LIST, API_FILE_OPEN, API_FILE_READ, API_FILE_CLOSE]
            .iter()
            .map(|id| {
                format!(
                    "{id:#04x}{}",
                    if identity.supported_ids.contains(id) { "" } else { "!" }
                )
            })
            .collect();
        println!("  advertises: {} (! = not in supported_ids)", advertised.join(" "));

        // 1. Find a real file. Opening a path the box does not have proves
        //    nothing about the layout — a refusal and a bad-layout refusal look
        //    the same — so the target comes from the box's own listing.
        for (path, declared_size) in files(&mut device, &dir, HOW_MANY) {
            // One Open, one Read, one Close — the box runs a single transfer
            // job, so these cannot overlap and a second Open voids the first.
            let Some((fd, _chunk)) = open_file(&mut device, &path, Some(CHUNK_LEN)) else {
                continue;
            };
            let mut assembled: Vec<u8> = Vec::new();
            let mut seq: u32 = 1;
            while (assembled.len() as u32) < declared_size + FRAMING && seq < 4096 {
                let Ok(replies) = device.drive_file_request_all(API_FILE_READ, &args2(fd, seq), QUIET)
                else {
                    break;
                };
                let reply = &replies[0];
                if reply.first() != Some(&0x01) {
                    println!("  {path}: read seq={seq} refused: {}", message_of(reply));
                    break;
                }
                let len = be32(reply, 18).unwrap_or(0) as usize;
                if len == 0 {
                    break;
                }
                assembled.extend_from_slice(&reply[22..22 + len.min(reply.len() - 22)]);
                seq += 1;
            }
            close_file(&mut device, fd, &path);

            // The container sits at HEADER, and the listing's size is the
            // container's own — so a file is HEADER + size + TRAILER, which is
            // what the read length being 43 over the declared size on both
            // boxes says. `decode_sound` checks the foot magic at size-4, so a
            // wrong offset or a wrong size does not validate: this landing is
            // the evidence, not the byte count.
            // The container sits at HEADER — 36 on all three boxes, which is
            // also what the listing's declared size plus FRAMING comes to. What
            // the listing does *not* give is the struct's own size: the payload
            // is padded, and `decode_sound` checks the foot magic at `size - 4`,
            // so the size has to come from the bytes. Finding the foot and
            // decoding to it is the check — a wrong offset does not validate.
            //
            // The A4 is the exception and the interesting one: its container is
            // magic `BEEFBABA`, not `BEEFBACE`, so `decode_sound` refuses it
            // outright. The name still sits at magic+12 and reads correctly,
            // which is why the fallback prints one rather than nothing.
            let body = assembled.get(HEADER as usize..).unwrap_or(&[]);
            match foot_size(body) {
                Some(size) => match decode_sound(body, size) {
                    Ok(sound) => println!(
                        "  {path:<18} {:>4}b  struct {size:>4}  {:<18} {:?}",
                        assembled.len(),
                        sound.name,
                        sound.tags()
                    ),
                    Err(e) => println!("  {path:<18} decode refused at {size}: {e}"),
                },
                None => {
                    let alt = offsets(&assembled, [0xbe, 0xef, 0xba, 0xba]);
                    println!(
                        "  {path:<18} {:>4}b  no BEEFBACE; BEEFBABA at {alt:?}{}",
                        assembled.len(),
                        alt.first().map_or(String::new(), |&m| format!(
                            ", name {:?}",
                            String::from_utf8_lossy(
                                &assembled[m + 12..(m + 28).min(assembled.len())]
                            )
                            .trim_end_matches('\0')
                            .to_string()
                        ))
                    );
                }
            }
        }
    }
}

/// List `dir` and return up to `n` occupied files as `(path, size)`.
fn files(device: &mut ElektronDevice, dir: &str, n: usize) -> Vec<(String, u32)> {
    let reply = match device.drive_list(dir, 0, 0) {
        Ok(r) if r.ok => r,
        Ok(r) => {
            println!("  {dir}: refused: {}", r.message.as_deref().unwrap_or("(no message)"));
            return Vec::new();
        }
        Err(e) => {
            println!("  {dir}: {e}");
            return Vec::new();
        }
    };
    let entries = match parse_list_entries(&reply.entry_bytes, reply.count) {
        Ok(e) => e,
        Err(e) => {
            println!("  {dir}: entry layout disagrees: {e}");
            return Vec::new();
        }
    };
    entries
        .iter()
        .filter(|e| e.is_occupied() && e.children.is_none() && e.size.is_some_and(|s| s > 0))
        .take(n)
        .map(|e| (format!("{dir}/{}", e.index.unwrap_or(0)), e.size.unwrap_or(0)))
        .collect()
}

/// Open `path`, optionally with a trailing u32, and report `(fd, chunk-size)`.
fn open_file(device: &mut ElektronDevice, path: &str, hint: Option<u32>) -> Option<(u32, u32)> {
    let mut args = path_arg(path);
    if let Some(h) = hint {
        args.extend_from_slice(&h.to_be_bytes());
    }
    match device.drive_file_request(API_FILE_OPEN, &args) {
        Ok(reply) => {
            if reply.first() != Some(&0x01) {
                println!("  0x54 Open refused: {}", message_of(&reply));
                return None;
            }
            let fd = be32(&reply, 1)?;
            let chunk = be32(&reply, 5)?;
            println!("  0x54 Open -> fd={fd}, chunk={chunk} bytes (asked for {})",
                hint.map_or_else(|| "nothing".to_string(), |h| h.to_string()));
            dump_hex(&reply);
            Some((fd, chunk))
        }
        Err(e) => {
            println!("  0x54 Open FAILED: {e}");
            None
        }
    }
}

/// Close `fd` and say what the box made of it. A refusal here is informative:
/// "Reader did not complete" means the reader is a state machine.
fn close_file(device: &mut ElektronDevice, fd: u32, what: &str) {
    match device.drive_file_request(API_FILE_CLOSE, &fd.to_be_bytes()) {
        Ok(reply) if reply.first() == Some(&0x01) => {
            println!("  0x56 Close [{what}] OK — {} bytes: {}", reply.len(),
                reply.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
        }
        Ok(reply) => println!("  0x56 Close [{what}] refused: {}", message_of(&reply)),
        Err(e) => println!("  0x56 Close [{what}] FAILED: {e}"),
    }
}

/// The box's own NUL-terminated complaint, which it supplies on every refusal
/// seen so far — "Invalid sequence number", "Reader did not complete".
fn message_of(reply: &[u8]) -> String {
    let text: Vec<u8> = reply[1..].iter().copied().take_while(|&b| b != 0).collect();
    String::from_utf8_lossy(&text).to_string()
}

fn offsets(bytes: &[u8], magic: [u8; 4]) -> Vec<usize> {
    bytes.windows(4).enumerate().filter(|(_, w)| *w == magic).map(|(i, _)| i).collect()
}
/// A NUL-terminated path, the one argument shape this API is known to use.
fn path_arg(path: &str) -> Vec<u8> {
    let mut v = path.as_bytes().to_vec();
    v.push(0);
    v
}

fn args2(a: u32, b: u32) -> Vec<u8> {
    let mut v = a.to_be_bytes().to_vec();
    v.extend_from_slice(&b.to_be_bytes());
    v
}
fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}
/// Hex + ASCII. The ASCII column is what identifies a container: a preset's
/// name is legible in it, and `BEEFBACE` is not.
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

/// The struct size implied by the foot magic, which is the only thing that
/// makes `decode_sound`'s check meaningful: the listing's size is the padded
/// payload, and the struct inside it is shorter and not a constant.
fn foot_size(body: &[u8]) -> Option<usize> {
    if body.get(..4)? != [0xbe, 0xef, 0xba, 0xce] {
        return None;
    }
    body.windows(4)
        .position(|w| w == [0xba, 0xce, 0xf0, 0x0c])
        .map(|at| at + 4)
}
