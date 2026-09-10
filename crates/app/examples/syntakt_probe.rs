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
    let fragment = arg("--port").unwrap_or_else(|| "Syntakt".to_string());
    let out = arg("--out");

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
    println!("{:>5}  {:>5}  {:>8}  {}", "req", "resp", "bytes", "leading bytes");

    if let Some(dir) = &out {
        std::fs::create_dir_all(dir).expect("could not make the output directory");
    }

    for request in REQUESTS {
        match device.fetch_dump(family, request, index) {
            Ok(reply) => {
                let head: Vec<String> =
                    reply.payload.iter().take(16).map(|b| format!("{b:02x}")).collect();
                println!(
                    "{request:>5x}  {:>5x}  {:>8}  {}",
                    reply.dump_type,
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
            Err(e) => println!("{request:>5x}  {:>5}  {:>8}  {e}", "-", "-"),
        }
    }

    if let Some(dir) = &out {
        println!("\nraw dumps written to {dir}/");
    }
}
