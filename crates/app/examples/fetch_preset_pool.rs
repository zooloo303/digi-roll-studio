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
// **Two: the tag mask needs calibrating.** `sound::TAG_NAMES` maps the 32 bits
// of the mask to the 32 cells of the +Drive browser's filter grid, read
// left-to-right and top-to-bottom. Seven of those bits are corroborated by patch
// names in the committed fixture; the rest are a guess, and bit 8 is actively
// suspect. The calibration table at the end prints each tagged sound's mask
// alongside what `TAG_NAMES` claims it says, so it can be read against the
// device's own display — either the box's PRESET browser or the same project in
// Overbridge. A row that disagrees is `TAG_NAMES` being wrong, and the fixture
// test `the_calibrated_tag_bits_match_the_patch_names` is where the fix belongs.
//
// The bit histogram is the cheaper signal: across a whole pool, a grid cell like
// "Kick" that is common in reality but never set here means the mapping is
// shifted, without needing any single preset's tags to be known.
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
use digi_protocol::sound::{decode_sound_dump, Sound, TAG_NAMES};

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

        report(&pool, timeouts, undecodable);
    }

    if boxes == 0 {
        println!("\nNo Elektron box answered. Nothing was sent to anything else.");
    }
}

fn report(pool: &[(u8, Sound)], timeouts: usize, undecodable: usize) {
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

    println!("\n  SLOT  NAME               TAG MASK    TAGS (per TAG_NAMES — unverified)");
    for (index, s) in &filled {
        println!(
            "  {:>4}  {:<18} {:#010x}  {}",
            index + 1,
            s.name,
            s.tag_mask,
            s.tags().join(", ")
        );
    }

    // Which bits the whole pool ever uses. A grid cell that is common in real
    // libraries but never set across a whole pool is the cheapest evidence that
    // TAG_NAMES is shifted — it needs no preset's true tags to be known.
    let mut used = [0usize; 32];
    for (_, s) in &filled {
        for (bit, count) in used.iter_mut().enumerate() {
            if s.tag_mask & (1 << bit) != 0 {
                *count += 1;
            }
        }
    }
    println!("\n  tag bits used across the pool:");
    for (bit, count) in used.iter().enumerate().filter(|(_, &c)| c > 0) {
        println!("    bit {bit:>2} ({:<11}) set on {count} preset(s)", TAG_NAMES[bit]);
    }
    let never: Vec<String> = (0..32)
        .filter(|&b| used[b] == 0)
        .map(|b| format!("{b} {}", TAG_NAMES[b]))
        .collect();
    if !never.is_empty() {
        println!("    never set: {}", never.join(", "));
    }

    println!(
        "\n  To calibrate: compare the TAGS column against the same presets in the\n  \
         box's own browser (or Overbridge). Any row that disagrees means TAG_NAMES\n  \
         is wrong — fix it there, then update the assertions in\n  \
         crates/protocol/tests/sound.rs::the_calibrated_tag_bits_match_the_patch_names."
    );
}
