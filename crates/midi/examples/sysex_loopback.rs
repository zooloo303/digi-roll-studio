// Does this machine's MIDI stack carry a SysEx message of a given size intact?
//
// **Touches no hardware.** It creates a virtual MIDI destination, connects an
// output to it, sends, and reassembles what arrives. Both ends are this process,
// so a failure here is CoreMIDI, `midir` or our own framing — never the box.
//
// This exists because the A4's 14,843-byte pattern dump went out and nothing
// happened, and "the bytes never left intact" and "the box declined them" are
// different problems with no overlap in what you do next. A loopback separates
// them without a capture, a click or a power cycle.
//
// Unix only. The virtual destination comes from `midir`'s ALSA/CoreMIDI-only
// `VirtualInput`; Windows MME has no equivalent, so there is nothing to gate on
// but the target. It is compiled away there rather than left to break the
// workspace build, because `cargo test --workspace` builds examples too.
//
//   cargo run -p digi_midi --example sysex_loopback
//       Sweeps sizes from 16 bytes to 32 KiB and reports the largest that
//       arrives byte-exact.
//
//   cargo run -p digi_midi --example sysex_loopback -- <file.syx>
//       Sends that exact file — the real message, not a synthetic one of the
//       same length, because padding and content are not the same test.
//
//   cargo run -p digi_midi --example sysex_loopback -- <file.syx> --chunk 256
//       Delivers it in pieces, the way `a4_pattern_send` paces a message at DIN
//       rate. Worth its own test: every chunk after the first begins with a
//       *data* byte, and a packet with no status byte is exactly the kind of
//       thing a MIDI stack is entitled to drop.

#[cfg(unix)]
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use digi_midi::sysex_stream::SysExReassembler;
#[cfg(unix)]
use midir::os::unix::VirtualInput;
#[cfg(unix)]
use midir::{Ignore, MidiInput, MidiOutput};

#[cfg(unix)]
const PORT: &str = "digi-roll-loopback";
/// How long to wait for a message to come back. A long SysEx is delivered over
/// several driver callbacks, so this is generous on purpose.
#[cfg(unix)]
const SETTLE: Duration = Duration::from_millis(600);

#[cfg(unix)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = args.iter().find(|a| !a.starts_with("--")).cloned();
    let chunk: Option<usize> = args
        .iter()
        .position(|a| a == "--chunk")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok());

    let mut input = MidiInput::new("digi-roll-loopback-in").expect("MidiInput");
    // Without this, the backend filters SysEx out and every test reports zero
    // bytes received — which looks exactly like the failure being investigated.
    input.ignore(Ignore::None);

    let got: Arc<Mutex<SysExReassembler>> = Arc::new(Mutex::new(SysExReassembler::new()));
    let frames: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let (g, f) = (Arc::clone(&got), Arc::clone(&frames));
    let _in_conn = input
        .create_virtual(
            PORT,
            move |_ts, bytes, _| {
                let done = g.lock().unwrap().push(bytes);
                f.lock().unwrap().extend(done);
            },
            (),
        )
        .expect("create_virtual — this is the step that needs CoreMIDI");

    let out = MidiOutput::new("digi-roll-loopback-out").expect("MidiOutput");
    let port = out
        .ports()
        .into_iter()
        .find(|p| out.port_name(p).as_deref() == Ok(PORT))
        .expect("the virtual destination we just created is not in the port list");
    let mut conn = out.connect(&port, "loopback").expect("connect");

    let mut send_and_check = |msg: &[u8], label: &str| -> bool {
        frames.lock().unwrap().clear();
        let sent = match chunk {
            None => conn.send(msg),
            Some(n) => {
                let mut r = Ok(());
                for piece in msg.chunks(n) {
                    r = conn.send(piece);
                    if r.is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                r
            }
        };
        std::thread::sleep(SETTLE);
        let back = frames.lock().unwrap();
        match (sent, back.first()) {
            (Err(e), _) => {
                println!("  {label:>12}: send refused — {e}");
                false
            }
            (Ok(()), None) => {
                println!("  {label:>12}: sent, nothing arrived");
                false
            }
            (Ok(()), Some(f)) if f.as_slice() == msg => {
                println!("  {label:>12}: arrived byte-exact ({} bytes)", f.len());
                true
            }
            (Ok(()), Some(f)) => {
                println!("  {label:>12}: arrived MANGLED — sent {} bytes, got {}", msg.len(), f.len());
                false
            }
        }
    };

    match arg {
        Some(path) => {
            let raw = std::fs::read(&path).expect("read file");
            let msg = if raw.first() == Some(&0xf0) {
                raw
            } else {
                let mut w = vec![0xf0];
                w.extend_from_slice(&raw);
                w.push(0xf7);
                w
            };
            println!("{path}: {} bytes on the wire", msg.len());
            let ok = send_and_check(&msg, "file");
            std::process::exit(if ok { 0 } else { 1 });
        }
        None => {
            println!("sweeping SysEx sizes through a virtual port:");
            let mut largest = 0usize;
            for total in [16usize, 64, 256, 512, 1024, 2048, 4096, 8192, 14843, 16384, 32768] {
                // A real-looking body: F0, Elektron id, 7-bit filler, F7.
                let mut msg = vec![0xf0, 0x00, 0x20, 0x3c];
                msg.extend((0..total.saturating_sub(5)).map(|i| (i % 128) as u8));
                msg.push(0xf7);
                if send_and_check(&msg, &format!("{total}")) {
                    largest = total;
                }
            }
            println!("\nlargest size that survived: {largest} bytes");
        }
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("sysex_loopback needs a virtual MIDI destination, which exists only on ALSA and CoreMIDI.");
}
