// Fetch the project's sound pool from each connected box, decode it, and print
// what the tag bits appear to mean.
//
// **Read-only.** Two API requests to identify, then one 0x63 sound request per
// slot — every one through `assert_request_opcode`, which admits no opcode that
// stores. Same safety class as `fetch_pattern_kit` and `identify_into_session`.
//
// This exists for two jobs that turn out to be the same hardware session.
//
// **One: 0x63 has never been sent to a box.** The opcode has been a constant in
// `protocol.rs` since the port, and `assert_request_opcode` has always admitted
// it, but nothing ever called it. So the response format is an assumption:
// `decode_sound_dump` guesses the struct size from the payload length and the
// three sizes we have mapped, and the foot magic is what stops a wrong guess
// passing. Slot 0 is therefore printed raw — payload length and leading bytes —
// before anything tries to decode it. If the box wraps the struct in a header we
// do not expect, that print is where it shows up.
//
// **Two: this is the recipe for calibrating a tag mask, and it has now been
// run.** `sound::tag_names_for` maps the 32 bits of the mask to the 32 cells of
// that box's +Drive browser filter grid, read left-to-right and top-to-bottom.
// As of 2026-08-29 the digis and the A4 are both calibrated and checked against
// 24 captures in `protocol/tests/drive_preset.rs`; what is left here is the
// procedure, for the next box.
//
// The table at the end prints each tagged sound's mask alongside what the box's
// table claims it says, to be read against an independent display of the same
// data. **Use Overbridge's Sound Browser rather than the box's own screen if
// you have it** — that is what made the A4 exact. The A4 truncates its tag row
// at four entries, so three positions were read wrong off photographs of the
// hardware and only the desktop grid, which lays all 32 cells out at once,
// settled them.
//
// The bit histogram is the cheaper signal: across a whole pool, a grid cell like
// "Kick" that is common in reality but never set here means the mapping is
// shifted, without needing any single preset's tags to be known. For a box with
// no table at all it prints bit numbers, which is still enough to spot that.
//
// Note what this cannot reach. `index` is one byte, so it addresses the
// project's 128-slot sound pool — Overbridge's "PROJECT PRESET POOL" — and not
// the +Drive's banks A-H. The +Drive needs the API path (0x10 DirList), which
// this crate does not implement.
//
// Run with:
//   cargo run -p digi_roll_studio --example fetch_preset_pool
//   cargo run -p digi_roll_studio --example fetch_preset_pool -- --slots 16

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::sound::{decode_sound_dump, tag_names_for, Sound};

/// The project sound pool is 128 slots on both boxes (elk-herd's
/// `soundPool = Bank.initializeEmpty 128`).
const POOL_SLOTS: u8 = 128;

/// Give up on a box after this many slots in a row time out. Each timeout costs
/// a full 5s `DUMP_STALL`, so a box that answers nothing would otherwise take
/// ten minutes to say so.
const MAX_CONSECUTIVE_TIMEOUTS: usize = 4;

fn main() {
    let slots: u8 = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--slots")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(POOL_SLOTS);

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    let mut boxes = 0usize;
    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
            println!("\n{}: no matching output port", input.name);
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
        // The fetch needs the family byte off the identity, and identifying
        // confirms which box actually answered on this port.
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
        boxes += 1;

        // Slot 0, raw, before anything decodes it. This is the only look we get
        // at what 0x63 actually returns if the decode is about to be wrong.
        match device.fetch_sound(0) {
            Ok(payload) => {
                println!("  slot 0 raw: {} payload bytes", payload.len());
                let head: Vec<String> =
                    payload.iter().take(32).map(|b| format!("{b:02x}")).collect();
                println!("  slot 0 head: {}", head.join(" "));
                match decode_sound_dump(&payload) {
                    Ok(s) => println!(
                        "  slot 0 decodes: struct v{}, {} bytes, name {:?}",
                        s.version,
                        s.bytes.len(),
                        s.name
                    ),
                    Err(e) => {
                        println!("  slot 0 DOES NOT DECODE: {e}");
                        println!("  the response format is not what sound.rs assumes — stopping");
                        println!("  here rather than printing 127 more failures.");
                        continue;
                    }
                }
            }
            Err(e) => {
                println!("  slot 0 fetch failed: {e}");
                println!("  0x63 may not be supported on this box — skipping it.");
                continue;
            }
        }

        let mut pool: Vec<(u8, Sound)> = Vec::new();
        let mut timeouts = 0usize;
        let mut consecutive = 0usize;
        let mut undecodable = 0usize;
        for index in 0..slots {
            match device.fetch_sound(index) {
                Ok(payload) => {
                    consecutive = 0;
                    match decode_sound_dump(&payload) {
                        Ok(s) => pool.push((index, s)),
                        Err(e) => {
                            undecodable += 1;
                            println!("  slot {index}: {} bytes but {e}", payload.len());
                        }
                    }
                }
                Err(e) => {
                    timeouts += 1;
                    consecutive += 1;
                    if consecutive >= MAX_CONSECUTIVE_TIMEOUTS {
                        println!(
                            "  slot {index}: {e} — {consecutive} in a row, giving up on this box"
                        );
                        break;
                    }
                }
            }
        }

        report(&pool, &identity.slug, timeouts, undecodable);
    }

    if boxes == 0 {
        println!("\nNo Elektron box answered. Nothing was sent to anything else.");
    }
}

fn report(pool: &[(u8, Sound)], slug: &str, timeouts: usize, undecodable: usize) {
    let filled: Vec<&(u8, Sound)> = pool.iter().filter(|(_, s)| !s.is_empty()).collect();
    println!(
        "\n  {} slot(s) answered, {} named, {} empty, {timeouts} timed out, {undecodable} undecodable",
        pool.len(),
        filled.len(),
        pool.len() - filled.len(),
    );

    if filled.is_empty() {
        println!("  Nothing named in the pool — load some presets into the project and re-run.");
        return;
    }

    println!("\n  SLOT  NAME               TAG MASK    TAGS (per this box's table)");
    for (index, s) in &filled {
        println!(
            "  {:>4}  {:<18} {:#010x}  {}",
            index + 1,
            s.name,
            s.tag_mask,
            s.tags(slug).join(", ")
        );
    }

    // Which bits the whole pool ever uses. A grid cell that is common in real
    // libraries but never set across a whole pool is the cheapest evidence that
    // this box's table is shifted — it needs no preset's true tags to be known.
    let mut used = [0usize; 32];
    for (_, s) in &filled {
        for (bit, count) in used.iter_mut().enumerate() {
            if s.tag_mask & (1 << bit) != 0 {
                *count += 1;
            }
        }
    }
    // An uncalibrated box has no names to print, and printing a digi's would be
    // the whole mistake this tool exists to catch. Bit numbers still carry the
    // histogram, which is the part that works without a table.
    let name = |bit: usize| -> String {
        tag_names_for(slug).map(|t| t[bit].to_string()).unwrap_or_else(|| "?".into())
    };
    println!("\n  tag bits used across the pool:");
    for (bit, count) in used.iter().enumerate().filter(|(_, &c)| c > 0) {
        println!("    bit {bit:>2} ({:<11}) set on {count} preset(s)", name(bit));
    }
    let never: Vec<String> =
        (0..32).filter(|&b| used[b] == 0).map(|b| format!("{b} {}", name(b))).collect();
    if !never.is_empty() {
        println!("    never set: {}", never.join(", "));
    }

    println!(
        "\n  To calibrate a new box: compare the TAGS column against the same presets\n  \
         in Overbridge's Sound Browser, whose filter grid shows all 32 cells at once.\n  \
         A row that disagrees means that box's table in protocol::sound is wrong —\n  \
         fix it there, add a table to tag_names_for, and pin captures in\n  \
         crates/protocol/tests/drive_preset.rs::every_capture_decodes_the_tags_its_box_displays."
    );
}
