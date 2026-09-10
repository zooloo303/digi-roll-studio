// The first thing in this repo that writes to a Syntakt at all.
//
// **Refuses to send without `--send` and a typed confirmation.** Without them
// this fetches the destination, builds the message it would send, prints the
// diff, and stops.
//
// # Where the store goes out
//
// `digi_midi::syntakt_transfer::send_pattern`, which validates the frame
// immediately before it leaves and takes a `Consent` naming the slot.
//
// **The first attempt at this example reached for `a4_transfer::send_pattern`
// instead**, on a reading that it was a generic paced sender. It is not: it
// verifies the Analog Four's own format, and it refused — `not an A4 pattern
// dump: family 0x16, type 0x51` — with nothing on the wire. That refusal is why
// `syntakt_transfer` exists. The alternative was `A4Sink::send_chunk`, which
// would have gone out unvalidated by stepping around the guard that had just
// done its job, and that is not a trade worth making.
//
// `ElektronDevice::send` stays private, so the read path still has no store
// bolted to its side.
//
// This is the shape `a4_pattern_send` had when the Analog Four was where the
// Syntakt is now: one example, one box, consent typed by hand, and no promotion
// of the box in the device table until a write has actually been seen to work.
//
// # The rules
//
//  1. **Back up first, or do not write.** The destination is fetched and
//     written to a `.syx` before anything is sent; a failure to write that file
//     aborts the run. A file with a checked error, not a browser download that
//     can vanish silently.
//  2. **Minimal diff.** The payload is the fetched one with only the requested
//     edit applied through `syntakt_pattern::set_track_notes`, and every byte
//     that moved is printed *before* the send, so the confirmation is typed
//     with the diff in view.
//  3. **Narrow.** It stores `0x51`, the pattern alone — never the `0x50` that
//     carries the kit. Sounds are not this tool's business, and a write that
//     cannot reach them cannot break them.
//  4. **Verify.** It re-fetches the slot and compares every byte with what was
//     sent, naming the first mismatches.
//  5. **A throwaway project.** Not enforceable in code, which is why it is said
//     here.
//
// # Why the first run should change nothing
//
// `--send` with no `--relock` sends the pattern **exactly as it was fetched**.
// The framing is already known to match the box's own — a test compares this
// builder's output against captured `.syx` for `0x50`, `0x51` and `0x52`, byte
// for byte — but "known" and "seen to work" are different, and DEVELOPMENT.md
// lesson 13 is what the gap cost on an Analog Four: a body it could not parse
// took its whole SysEx API down until a power cycle, six times over two days.
//
// So the first send is the one where being wrong costs nothing. The bytes going
// out are the bytes that came in.
//
// Run with:
//   cargo run -p digi_roll_studio --example syntakt_write -- --index 1
//   cargo run -p digi_roll_studio --example syntakt_write -- --index 1 --send
//   cargo run -p digi_roll_studio --example syntakt_write -- --index 1 --relock 0:0:60 --send

use std::io::Write as _;

use digi_midi::syntakt_transfer::{send_pattern, Consent, PATTERN_DUMP};
use digi_midi::{list_inputs, list_outputs, open_output_by_name, ElektronDevice, PortBinding};
use digi_protocol::protocol::{build_dump_message, FAMILY_SYNTAKT};
use digi_protocol::syntakt_pattern as st;

/// The request that fetches a pattern without its kit.
const PATTERN_REQUEST: u8 = 0x61;
/// Typed in full, so a held return key cannot agree to this.
const CONSENT: &str = "overwrite";

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn main() {
    let index = arg("--index").and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
    let fragment = arg("--port").unwrap_or_else(|| "Syntakt".to_string());
    let sending = flag("--send");
    // Fetch from one slot, store into another. The point is not copying: it is
    // that a store into an *empty* slot has a visible outcome, where a store of
    // a slot's own bytes back into itself verifies whether or not anything
    // happened at all.
    let into = arg("--to").and_then(|s| s.parse::<u8>().ok()).unwrap_or(index);
    // `--relock track:step:note` — track and step zero-based, note raw MIDI.
    let relock: Option<(usize, usize, u8)> = arg("--relock").and_then(|s| {
        let p: Vec<&str> = s.split(':').collect();
        Some((p.first()?.parse().ok()?, p.get(1)?.parse().ok()?, p.get(2)?.parse().ok()?))
    });

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let Some(input) = inputs.iter().find(|p| p.name.contains(&fragment)) else {
        println!("no input port matching {fragment:?}");
        return;
    };
    let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
        println!("{} has no matching output port", input.name);
        return;
    };
    let port_name = output.name.clone();

    // --- 1. fetch, and back the destination up before anything else ---------
    let before = {
        let mut device =
            ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
                .expect("could not open the port");
        let identity = device.identify().expect("no identity");
        println!("{} — build {}, slot {index}", identity.name, identity.build);
        match device.fetch_dump(FAMILY_SYNTAKT, PATTERN_REQUEST, index) {
            Ok(r) => r,
            Err(e) => {
                println!("the destination would not answer: {e}");
                return;
            }
        }
    };
    if !st::looks_like_pattern(&before.payload) {
        println!("that reply does not look like a pattern — stopping");
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = format!("syntakt-slot{index:03}-{stamp}.syx");
    if let Err(e) = std::fs::write(&backup, &before.raw) {
        println!("could not write the backup {backup}: {e}");
        println!("no backup, no write — stopping");
        return;
    }
    let trigs: usize = (0..st::NUM_BLOCKS).map(|t| st::trig_count(&before.payload, t)).sum();
    println!("backed the destination up to {backup} ({} bytes, {trigs} trigs)", before.raw.len());

    // --- 2. build the payload, and show the diff ---------------------------
    let mut payload = before.payload.clone();
    match relock {
        None => println!("\nno edit requested — this sends the slot back unchanged"),
        Some((track, step, note)) => {
            let mut notes = st::track_notes(&payload, track);
            match notes.iter_mut().find(|n| n.step == step) {
                Some(n) => {
                    n.note = note;
                    n.locked.note = true;
                }
                None => {
                    println!("block {} has no trig on step {}", track + 1, step + 1);
                    return;
                }
            }
            st::set_track_notes(&mut payload, track, &notes);
        }
    }
    // A single non-step byte, for telling "the store ignores step data" apart
    // from "no store is happening at all". Those look identical when the source
    // and the destination are both nearly empty.
    if let Some(swing) = arg("--swing").and_then(|s| s.parse::<u8>().ok()) {
        payload[st::SWING] = swing.saturating_sub(st::SWING_STRAIGHT_PERCENT);
    }
    let moved: Vec<usize> =
        (0..payload.len()).filter(|&i| payload[i] != before.payload[i]).collect();
    println!("{} byte(s) would change", moved.len());
    for i in moved.iter().take(8) {
        println!("   {i:6}  {:02x} -> {:02x}", before.payload[*i], payload[*i]);
    }

    if !sending {
        println!("\nDry run. Pass --send to write it.");
        return;
    }

    // --- 3. consent, typed -------------------------------------------------
    println!("\nThis overwrites slot {into} on the box. The backup above is slot {index}.");
    print!("Type {CONSENT} to proceed: ");
    let _ = std::io::stdout().flush();
    let mut typed = String::new();
    if std::io::stdin().read_line(&mut typed).is_err() || typed.trim() != CONSENT {
        println!("not confirmed — nothing sent.");
        return;
    }

    // --- 4. send, narrowly -------------------------------------------------
    let message = build_dump_message(FAMILY_SYNTAKT, PATTERN_DUMP, into, &payload);
    println!("sending {} bytes as dump type {PATTERN_DUMP:#04x}…", message.len());
    {
        let mut conn = match open_output_by_name(&port_name) {
            Ok(c) => c,
            Err(e) => {
                println!("could not open {port_name} to send: {e:?}");
                return;
            }
        };
        // `syntakt_transfer::send_pattern` re-derives the framing from these
        // bytes and refuses on anything that is not this box's pattern dump for
        // this slot. The first attempt at this example reached for the Analog
        // Four's sender instead, and that function refused — correctly, since
        // it validates the A4's format. Nothing went out. This is the path that
        // was missing, rather than a way around the one that said no.
        match send_pattern(&mut conn, &message, Consent::given_for(into)) {
            Ok(frame) => println!("sent {} bytes to slot {}", message.len(), frame.slot),
            Err(e) => {
                println!("the send failed: {e}");
                println!("the destination as it was is in {backup}");
                return;
            }
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(800));

    // --- 5. verify ---------------------------------------------------------
    let mut device = match ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
    {
        Ok(d) => d,
        Err(e) => {
            println!("VERIFY FAILED — could not reopen the port: {e}");
            println!("the destination as it was is in {backup}");
            return;
        }
    };
    let after = match device.fetch_dump(FAMILY_SYNTAKT, PATTERN_REQUEST, into) {
        Ok(r) => r,
        Err(e) => {
            println!("VERIFY FAILED — the box would not answer afterwards: {e}");
            println!("If it has stopped answering at all, power-cycle it — lesson 13.");
            println!("the destination as it was is in {backup}");
            return;
        }
    };
    if after.payload == payload {
        println!("VERIFIED — every byte of the slot matches what was sent");
    } else {
        let bad: Vec<usize> = (0..payload.len().min(after.payload.len()))
            .filter(|&i| after.payload[i] != payload[i])
            .collect();
        println!("VERIFY FAILED — {} byte(s) differ from what was sent", bad.len());
        for i in bad.iter().take(8) {
            println!("   {i:6}  sent {:02x}, box has {:02x}", payload[*i], after.payload[*i]);
        }
        println!("the destination as it was is in {backup}");
    }
}
