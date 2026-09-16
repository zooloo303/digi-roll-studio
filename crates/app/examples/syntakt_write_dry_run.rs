// Take a real Syntakt pattern off the box, run the **write** half over it in
// memory, and report exactly which bytes moved. **Nothing is sent.**
//
// Read-only, and the same safety class as `syntakt_probe`: an identity request,
// one `0x61` pattern request, and then arithmetic on a `Vec<u8>` this process
// owns. No `0x5n` opcode is constructed anywhere and nothing here can reach a
// store path — there is not one for this box. The modified payload is dropped
// when the example exits.
//
// # What this proves that the fixtures cannot
//
// `crates/protocol/tests/all/syntakt.rs` already round-trips every committed
// capture. Those captures are a bench pattern and a handful of deliberate
// edits. **A pattern off the box in front of you is whatever you have been
// working on** — a full trig pool, p-lock lanes on several tracks, polymetric
// lengths, machines nobody here has seen. Minimal diff is the rule that makes
// an unmapped byte safe to carry rather than dangerous, and this is the first
// time it is checked against a pattern nobody prepared for it.
//
// Two passes:
//
//   1. **Decode and write back unchanged.** Zero bytes may move. This is the
//      round trip, against live content.
//   2. **One note relocked.** Exactly one byte may move, and it must be the
//      note lane's cell for that step.
//
// Anything landing outside the six lanes of the track being written is a
// FAILURE, because that is the rule this example exists to test.
//
// Run with:
//   cargo run -p digi_roll_studio --example syntakt_write_dry_run -- --index 112

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::syntakt_pattern as st;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

/// Which lane an offset falls in, relative to `track`'s block, or `None` when
/// it is outside that block's editable region entirely.
fn locate(offset: usize, track: usize) -> Option<String> {
    let base = st::BLOCK_BASE + st::BLOCK_STRIDE * track;
    let rel = offset.checked_sub(base)?;
    let lanes = [
        (st::TRIG_LANE, st::NOTE_LANE, "trig words"),
        (st::NOTE_LANE, st::VELOCITY_LANE, "note"),
        (st::VELOCITY_LANE, st::LENGTH_LANE, "velocity"),
        (st::LENGTH_LANE, st::MICRO_LANE, "length"),
        (st::MICRO_LANE, st::CONDITION_LANE, "micro"),
        (st::CONDITION_LANE, st::CONDITION_LANE + 64, "condition"),
    ];
    lanes.iter().find(|(lo, hi, _)| rel >= *lo && rel < *hi).map(|(lo, _, name)| {
        let step = if *name == "trig words" { (rel - lo) / 2 } else { rel - lo };
        format!("{name}, step {}", step + 1)
    })
}

fn report(original: &[u8], modified: &[u8], track: usize, expected: &str) -> bool {
    let moved: Vec<usize> =
        (0..original.len().min(modified.len())).filter(|&i| original[i] != modified[i]).collect();
    println!("  {expected}");
    println!("  {} byte(s) moved", moved.len());
    let mut ok = true;
    for i in &moved {
        match locate(*i, track) {
            Some(where_) => println!(
                "    {i:6}  {:02x} -> {:02x}   {where_}",
                original[*i], modified[*i]
            ),
            None => {
                println!(
                    "    {i:6}  {:02x} -> {:02x}   *** OUTSIDE THE TRACK'S LANES ***",
                    original[*i], modified[*i]
                );
                ok = false;
            }
        }
    }
    ok
}

fn main() {
    let index = arg("--index").and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
    let fragment = arg("--port").unwrap_or_else(|| "Syntakt".to_string());

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
    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
        .expect("could not open the port");
    let identity = device.identify().expect("no identity");
    println!("{} — build {}, pattern slot {index}\n", identity.name, identity.build);

    let reply = device
        .fetch_dump(digi_protocol::protocol::FAMILY_SYNTAKT, 0x61, index)
        .expect("no pattern came back");
    let original = reply.payload;
    if !st::looks_like_pattern(&original) {
        println!("that reply does not look like a pattern");
        return;
    }

    let counts: Vec<usize> = (0..st::NUM_BLOCKS).map(|t| st::trig_count(&original, t)).collect();
    println!("trigs per block: {counts:?}");
    let busiest = counts.iter().enumerate().max_by_key(|(_, n)| **n).map(|(t, _)| t).unwrap_or(0);
    println!("writing block {} — the busiest, with {} trigs\n", busiest + 1, counts[busiest]);

    let mut ok = true;

    println!("PASS 1 — decode and write back unchanged");
    let mut copy = original.clone();
    let notes = st::track_notes(&original, busiest);
    st::set_track_notes(&mut copy, busiest, &notes);
    ok &= report(&original, &copy, busiest, "expected: nothing moves");
    ok &= copy == original;

    println!("\nPASS 2 — relock one note");
    let mut copy = original.clone();
    let mut notes = st::track_notes(&original, busiest);
    if let Some(first) = notes.first_mut() {
        first.note = if first.note == 60 { 62 } else { 60 };
        first.locked.note = true;
        let step = first.step;
        st::set_track_notes(&mut copy, busiest, &notes);
        let want = st::BLOCK_BASE + st::BLOCK_STRIDE * busiest + st::NOTE_LANE + step;
        ok &= report(&original, &copy, busiest, "expected: one byte, the note lane's");
        let moved: Vec<usize> =
            (0..original.len()).filter(|&i| copy[i] != original[i]).collect();
        ok &= moved == vec![want];
    } else {
        println!("  that block has no trigs, so there is nothing to relock");
    }

    println!("\n{}", if ok { "OK — minimal diff held" } else { "FAILURE — see the marked lines" });
    println!("Nothing was sent. The modified payload is discarded now.");
}
