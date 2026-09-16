// Listen to what a Syntakt sends off its own front panel, and keep it.
//
// **Read-only. This file opens an input and nothing else** — there is no output
// connection in it, so it cannot put a byte on the wire even by mistake.
//
// # Why this exists
//
// Every Syntakt dump this repo holds was *requested*: `fetch_dump` asks `0x61`
// and the box answers `0x51`. That proves what the box will hand over when
// asked. It does not prove what the box's own SETTINGS > SYSEX DUMP > SYSEX
// SEND > PATTERN produces, and those are different questions with possibly
// different answers.
//
// It matters because of the other half. SYSEX RECEIVE is the store path, and
// the manual (OS 1.40, §14.5.2) says "PATTERN will store a received pattern to
// the selected pattern slot" — the slot picked in that screen's right-hand
// column, not the pattern being played. A box that stores what its own SEND
// produces is the likeliest reading of a pair of menu items that sit next to
// each other, so **what SEND emits is the best available guess at what RECEIVE
// accepts.**
//
// On 2026-09-10 and again on 2026-09-11 this repo sent `0x51`, the pattern
// without its kit, and the box stored none of it — unpaced and paced, into an
// armed slot and otherwise, with all 128 slots swept afterwards and nothing
// found. `0x51` was chosen for safety: it cannot reach a sound, so it cannot
// break one. If SEND emits `0x50` instead, that safety is also the reason
// nothing lands, and this run is what says so.
//
// # What to do with it
//
// Start this, then on the box: SETTINGS > SYSEX DUMP > SYSEX SEND, left column
// PATTERN, right column the pattern to send, [YES]. Every frame that arrives is
// reported and saved.
//
// Run with:
//   cargo run -p digi_roll_studio --example syntakt_listen
//   cargo run -p digi_roll_studio --example syntakt_listen -- --out captures/panel-send
//   cargo run -p digi_roll_studio --example syntakt_listen -- --wait 600

use std::time::{Duration, Instant};

use digi_midi::{list_inputs, SysExInbox};
use digi_protocol::protocol::parse_sysex;
use digi_protocol::syntakt_pattern as st;

/// The default wait for the first frame, unless `--wait` says otherwise.
///
/// This is somebody walking to the box and finding a menu three levels down, so
/// it is generous on purpose — and 90 seconds still ran out once, on somebody
/// reading the instructions while the clock was already going.
const FIRST: Duration = Duration::from_secs(90);

/// Silence after the last activity before calling it finished. `mid_frame`
/// counts as activity, so a 31 KB frame arriving in packets is not cut in half.
const QUIET: Duration = Duration::from_secs(3);

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn main() {
    let fragment = arg("--port").unwrap_or_else(|| "Syntakt".to_string());
    let out = arg("--out");
    let first = arg("--wait")
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(FIRST);

    let inputs = list_inputs().expect("MIDI would not start");
    let Some(port) = inputs.iter().find(|p| p.name.contains(&fragment)) else {
        println!("no input port matching {fragment:?}");
        return;
    };
    let mut inbox = SysExInbox::open(&port.name).expect("could not open the box's output");
    let _ = inbox.drain();

    println!("listening on {}", port.name);
    println!("On the box: SETTINGS > SYSEX DUMP > SYSEX SEND, pick PATTERN, press [YES].");
    println!("Waiting up to {}s for the first frame…\n", first.as_secs());

    let frames = listen(&mut inbox, first, QUIET);
    if frames.is_empty() {
        println!("nothing arrived.");
        return;
    }

    if let Some(dir) = &out {
        std::fs::create_dir_all(dir).expect("could not make the output directory");
    }
    println!(
        "{:>3}  {:>8}  {:>6}  {:>4}  {:>4}  what",
        "n", "bytes", "family", "type", "idx"
    );
    for (n, frame) in frames.iter().enumerate() {
        let parsed = parse_sysex(frame);
        match parsed.dump {
            None => println!(
                "{n:>3}  {:>8}  {:>6}  {:>4}  {:>4}  not a dump",
                frame.len(),
                '-',
                '-',
                '-'
            ),
            Some(d) => {
                let what = if st::looks_like_pattern(&d.payload) {
                    format!("a pattern, {} unpacked", d.payload.len())
                } else {
                    format!("{} unpacked, not a pattern", d.payload.len())
                };
                println!(
                    "{n:>3}  {:>8}  {:>#6x}  {:>#4x}  {:>4}  {what}",
                    frame.len(),
                    d.family,
                    d.dump_type,
                    d.index
                );
                if let Some(dir) = &out {
                    let stem = format!(
                        "{dir}/panel-{n:02}-type-{:02x}-idx-{:03}",
                        d.dump_type, d.index
                    );
                    std::fs::write(format!("{stem}.syx"), frame)
                        .expect("could not write the frame");
                    std::fs::write(format!("{stem}.bin"), &d.payload)
                        .expect("could not write the payload");
                }
            }
        }
    }
    if let Some(dir) = &out {
        println!("\nwritten to {dir}/");
    }
}

/// Collect frames until `quiet` passes with nothing arriving and no frame open.
fn listen(inbox: &mut SysExInbox, first: Duration, quiet: Duration) -> Vec<Vec<u8>> {
    let start = Instant::now();
    let mut last = Instant::now();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    loop {
        let got = inbox.drain();
        if !got.is_empty() || inbox.mid_frame() {
            last = Instant::now();
        }
        for frame in got {
            println!("   got {} bytes", frame.len());
            frames.push(frame);
        }
        if frames.is_empty() && !inbox.mid_frame() {
            if start.elapsed() > first {
                return frames;
            }
        } else if last.elapsed() > quiet {
            return frames;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
