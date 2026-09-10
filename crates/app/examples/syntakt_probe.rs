// Sweep the Syntakt's dump requests and keep what comes back.
//
// **Read-only, structurally.** Every request goes through `fetch_dump`, whose
// `assert_request_opcode` admits only 0x60–0x6e — the *request* half of the
// dump protocol. Storing is 0x5n and there is no 0x5n in this file.
//
// # Why this exists when `probe_dump_types` already does the same sweep
//
// That one asks the device table for the box's family byte and skips any port
// whose slug it does not recognise. Both are reasonable for a box that has been
// mapped, and both are exactly wrong here: **a probe must not require the answer
// it is looking for.** The Syntakt has no entry in Studio's model table, so
// `probe_dump_types` finds nothing to talk to and prints an empty sweep, which
// is what it did on 2026-09-10.
//
// So the family byte is an argument. `0x16` is the default because that is what
// the donated captures in the browser repo recorded
// (`docs/syntakt-pattern-format.md`, product 30, request 0x60, response 0x50),
// and confirming it against a second box is itself worth a run.
//
// # What it is for
//
// `docs/digitakt-syntakt-mapping-handoff.md` asks first for "a same-state 0x60,
// 0x61, 0x62 dump trio to compare actual contents and resolve the
// combined/standalone boundary". The probe reported a standalone pattern of
// 27136 and a kit of 5632, summing to 1024 more than the 31744-byte combined
// dump, so the standalone sizes cannot simply be concatenated and nobody should
// slice a kit using them. Three dumps taken without touching the box in between
// are what settles it.
//
// Run with:
//   cargo run -p digi_roll_studio --example syntakt_probe
//   cargo run -p digi_roll_studio --example syntakt_probe -- --out captures/base
//   cargo run -p digi_roll_studio --example syntakt_probe -- --family 16 --index 0
//   cargo run -p digi_roll_studio --example syntakt_probe -- --only 60,61,65 --out captures/before
//
// `--only` is what a *pair* wants. A capture pair is one edit and one variable,
// so the two halves have to be taken the same way; sweeping the whole range
// between them spends about a second per timing-out opcode and asks the box
// fifteen questions when three would do.

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};

/// The whole request range the read-only guard admits, minus 0x6f, which is not
/// a store either but streams the entire project and would bury the answer.
const REQUESTS: std::ops::RangeInclusive<u8> = 0x60..=0x6e;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn main() {
    let family = arg("--family")
        .and_then(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x16);
    let index = arg("--index").and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
    // `--indices 0-15` sweeps slots instead of one. Which pattern a box is
    // actually sitting on is not something a dump request can ask, and reading
    // the wrong slot looks exactly like a box that ignores every edit — so
    // finding the live one by taking a slot sweep either side of a single edit
    // is cheaper than asking someone to read a display correctly.
    let indices: Vec<u8> = match arg("--indices") {
        Some(range) => {
            let (lo, hi) = range.split_once('-').unwrap_or((range.as_str(), range.as_str()));
            let lo: u8 = lo.trim().parse().unwrap_or(0);
            let hi: u8 = hi.trim().parse().unwrap_or(lo);
            (lo..=hi).collect()
        }
        None => vec![index],
    };
    let fragment = arg("--port").unwrap_or_else(|| "Syntakt".to_string());
    let out = arg("--out");
    let only: Option<Vec<u8>> = arg("--only").map(|list| {
        list.split(',')
            .filter_map(|t| u8::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok())
            .collect()
    });

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    // By name, not by slug: an unmapped box has no slug, which is the whole
    // situation this tool is for.
    let Some(input) = inputs.iter().find(|p| p.name.contains(&fragment)) else {
        println!("no input port matching {fragment:?}");
        return;
    };
    let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
        println!("{} has no matching output port", input.name);
        return;
    };

    let mut device =
        ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
            .expect("could not open the port");
    let identity = device.identify().expect("no identity");
    println!(
        "{} — product {}, build {}, version {}, sweeping family {family:#04x} index {index}",
        identity.name, identity.product_id, identity.build, identity.version
    );
    // The reply's index byte is printed because it is the box answering "which
    // slot is this?", and a stored-slot request echoes what was asked while a
    // working-state request names whatever is loaded. Reading a pattern the box
    // is not editing looks exactly like a box that ignores edits.
    println!("{:>5}  {:>5}  {:>4}  {:>8}  {}", "req", "resp", "idx", "bytes", "leading bytes");

    if let Some(dir) = &out {
        std::fs::create_dir_all(dir).expect("could not make the output directory");
    }

    let wanted: Vec<u8> = match &only {
        Some(list) => list.clone(),
        None => REQUESTS.collect(),
    };
    for (request, index) in
        wanted.iter().copied().flat_map(|r| indices.iter().copied().map(move |i| (r, i)))
    {
        match device.fetch_dump(family, request, index) {
            Ok(reply) => {
                let head: Vec<String> =
                    reply.payload.iter().take(16).map(|b| format!("{b:02x}")).collect();
                println!(
                    "{request:>5x}  {:>5x}  {:>4}  {:>8}  {}",
                    reply.dump_type,
                    reply.index,
                    reply.payload.len(),
                    head.join(" ")
                );
                if let Some(dir) = &out {
                    // Both halves. The raw frame because the framing is evidence
                    // too and a payload alone cannot be re-parsed into one; the
                    // unpacked payload because that is what offsets are quoted
                    // against, and re-deriving it by hand is how a diff ends up
                    // comparing two different unpackings.
                    let stem = format!("{dir}/req-{request:02x}-idx-{index:02}");
                    std::fs::write(format!("{stem}.syx"), &reply.raw)
                        .expect("could not write the frame");
                    std::fs::write(format!("{stem}.bin"), &reply.payload)
                        .expect("could not write the payload");
                }
            }
            Err(e) => println!("{request:>5x}  {:>5}  {:>4}  {:>8}  {e}", "-", "-", "-"),
        }
    }

    if let Some(dir) = &out {
        println!("\nraw dumps written to {dir}/");
    }
}
