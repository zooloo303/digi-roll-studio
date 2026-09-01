// Send a gen-1 SysEx **pattern dump** to an Analog Four. The first thing in this
// repo that writes to an A4 at all.
//
// **Read this before running it.** Two modes, and the default is the harmless one:
//
//   cargo run -p digi_roll_studio --example a4_pattern_send -- <file.syx>
//       A *rehearsal*. Loads the file, re-derives its checksum and length,
//       unpacks the payload and reports the slot and every trig it carries.
//       **No port is opened.** This is pure arithmetic on a `Vec<u8>`.
//
//   cargo run -p digi_roll_studio --example a4_pattern_send -- <file.syx> --send
//       **The real thing.** Overwrites one pattern slot on the box. Asks for
//       typed consent, and refuses without it.
//
// **The format lives in `digi_protocol::a4_pattern` as of 2026-08-31**, and this
// example is now just the twelve inches of cable. It used to carry its own copy
// of the layout, on the grounds that "this workspace speaks gen-2 and
// `sevenbit.rs` packs the other way" — which was wrong twice over. `sevenbit.rs`
// packs the same way, and `protocol::parse_sysex` reads one of these dumps with
// nothing added; only the *payload* layout is gen-1's own. DEVELOPMENT.md lesson
// 17.
//
// That copy had gone stale in exactly the way a second copy does. It counted a
// trig as `byte 0 bit 0`, which is the model the box's own screen refuted on
// 2026-08-31 — so it reported A01 SYN4 as 19 trigs where the box shows 4 — and
// it printed every note an octave low. Both are fixed by not having a second
// copy. Nothing else in the workspace calls this, and it constructs no `0x5x`
// file opcode and no `0x7x` store: it sends one dump message and reads no reply.
//
// **The validation and the pacing both moved to `digi_midi::a4_transfer` on
// 2026-08-31**, when the app grew a panel that needed them. They were this
// file's own for one day, which was the right length of time: a thing with one
// caller belongs at its caller, and a thing with two belongs underneath both.
//
// The validation is still deliberately duplicated *work* —
// `a4_transfer::verify_before_send` re-checks framing `protocol::a4_pattern`
// already checked — and its doc carries the argument for that: the thing which
// must be true is not "the builder was correct" but "these bytes are
// well-formed", and a file can be edited or half-written in between. What is
// gone is the second *copy*, which is a different thing and is the one this
// project keeps getting caught by.
//
// ~~**Verifying afterwards is a manual step, and there is no way around it.**~~
// **There is, since 2026-08-31: the A4 answers dump requests** (PLAN.md §10,
// "The A4 answers dump requests"; the supported-opcode reply describes the API
// namespace, not this one). The wire verify is
// `a4_dump_probe --opcode 0x64 --index <slot>` until the app grows the real
// one. The front-panel route still works and stays written down:
// send, then re-dump the slot from the box, then
// `python3 local/a4_pattern.py verify <sent.syx> <redump.mmon>`.

use std::io::Write as _;
use std::time::Duration;

use digi_midi::a4_transfer::{
    send_pattern, verify_before_send, Consent, Pacing, CAN_PACE, DIN_BYTES_PER_SEC,
};
use digi_midi::{capture_sysex, list_outputs, open_output_by_name};
use digi_protocol::a4_pattern::{
    effective_note, note_name, read_track_trigs, slot_name, TRACK_NAMES,
};

/// Which box to send to when nothing is named. Matched as a case-insensitive
/// substring of the port name.
const DEFAULT_PORT: &str = "Analog Four";

/// The word `--send` makes you type. Not a y/n, because a y/n is something you
/// press by reflex.
const CONSENT: &str = "overwrite";

/// **Why the send is paced by default**, and where that now lives: DIN MIDI is
/// 31,250 baud, so a 14,843-byte dump takes 4.75 s over a cable and the box was
/// designed against that rate. One unpaced call did nothing at all on a box that
/// was demonstrably listening. `a4_transfer::Pacing` is the whole argument and
/// the Windows caveat with it; `--single` restores the shape that failed, kept so
/// the difference stays measurable rather than assumed.
const DEFAULT_CHUNK: usize = 256;
/// How long to listen for a reply after the last byte. The box may say nothing;
/// silence is recorded as silence rather than as success.
const REPLY_WINDOW: Duration = Duration::from_millis(1500);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let for_real = args.iter().any(|a| a == "--send");
    let single = args.iter().any(|a| a == "--single");
    let chunk: usize = flag(&args, "--chunk").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_CHUNK);
    let pace_ms: f64 = flag(&args, "--pace-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(chunk as f64 / DIN_BYTES_PER_SEC * 1000.0);
    let port_fragment = flag(&args, "--port").unwrap_or_else(|| DEFAULT_PORT.to_string());
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!(
            "usage: a4_pattern_send <file.syx> [--send] [--port NAME] \\\n                    [--chunk BYTES] [--pace-ms MS] [--single]"
        );
        std::process::exit(2);
    };

    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(2);
        }
    };

    let msg = match validate(&raw) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{path}: {e}");
            eprintln!("refusing to send a message this process cannot itself verify");
            std::process::exit(1);
        }
    };

    println!("{path}");
    println!("  {} bytes on the wire, F0/F7 included", msg.wire.len());
    println!("  slot {} ({})  checksum {:#06x} ok  length {} ok", msg.slot, slot_name(msg.slot), msg.checksum, msg.length);
    println!("  payload {} bytes, decodes clean", msg.payload.len());
    for (track, name) in TRACK_NAMES.iter().enumerate() {
        // `read_track_trigs` skips the steps that carry only residue, which is
        // what the box's own screen does. The count printed here is the count
        // the front panel shows — that equality is the regression that caught
        // two wrong trig models, so it is worth it being the same function.
        let trigs = read_track_trigs(&msg.payload, track).expect("validated payload");
        if trigs.is_empty() {
            continue;
        }
        let listed: Vec<String> = trigs
            .iter()
            .map(|t| {
                // `effective_note`, not `t.note`: an unset note lane means "take
                // the track default", so the raw byte would print `--` on a step
                // the box sounds. No capture has one, which is the reason to be
                // careful rather than the reason not to be.
                let note = effective_note(&msg.payload, track, t).expect("validated payload");
                let shown = note.map_or_else(|| "--".to_string(), note_name);
                let inherited = if note.is_some() && t.note.is_none() { "*" } else { "" };
                format!("{}:{shown}{inherited}", t.step)
            })
            .collect();
        println!("  {name}  {} trigs  {}", trigs.len(), listed.join(" "));
    }

    let pacing = if single {
        Pacing::single()
    } else {
        Pacing { chunk, gap: Duration::from_secs_f64(pace_ms / 1000.0) }
    }
    .resolve(CAN_PACE);

    if pacing.packets(msg.wire.len()) == 1 {
        println!("\n  delivery: ONE unpaced send — the shape that did nothing on 2026-08-30");
        if !CAN_PACE && !single {
            println!("  (this platform's MIDI backend refuses a split SysEx, so pacing collapsed)");
        }
    } else {
        println!(
            "\n  delivery: {} chunks of {} bytes, {:.1} ms apart (~{:.1} s, DIN is {:.1} s)",
            pacing.packets(msg.wire.len()),
            pacing.chunk,
            pacing.gap.as_secs_f64() * 1000.0,
            pacing.estimate(msg.wire.len()).as_secs_f64(),
            msg.wire.len() as f64 / DIN_BYTES_PER_SEC
        );
    }

    if !for_real {
        println!("rehearsal — nothing was sent, and no port was opened.");
        println!("re-run with --send to write this to the box.");
        return;
    }

    let outputs = match list_outputs() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("could not enumerate MIDI outputs: {e:?}");
            std::process::exit(1);
        }
    };
    let needle = port_fragment.to_lowercase();
    let Some(port) = outputs.iter().find(|p| p.name.to_lowercase().contains(&needle)) else {
        eprintln!("no output port matching {port_fragment:?}. Ports present:");
        for p in &outputs {
            eprintln!("  {}", p.name);
        }
        std::process::exit(1);
    };

    println!("\nThis will OVERWRITE pattern {} on {}.", slot_name(msg.slot), port.name);
    // **Corrected 2026-08-31.** This line said the box must be in SETTINGS >
    // SYSEX DUMP > SYSEX RECEIVE. It does not: PLAN.md §9 measured on 2026-08-30
    // that it takes a dump sitting at its ordinary menu, and a full round trip on
    // 2026-08-31 confirmed it. The stale instruction was copied out of here into
    // `ui::a4`'s tooltip before anyone re-read it, which is this file's own
    // header warning happening to this file.
    println!("The box needs no receive mode and will not prompt — there is no arming step.");
    println!("Whatever is in that slot now will be gone, and this tool keeps no backup:");
    println!("dump the slot from the box first if you want one.");
    print!("Type {CONSENT} to proceed: ");
    let _ = std::io::stdout().flush();
    let mut typed = String::new();
    if std::io::stdin().read_line(&mut typed).is_err() || typed.trim() != CONSENT {
        println!("not confirmed — nothing sent.");
        return;
    }

    let mut conn = match open_output_by_name(&port.name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not open {}: {e:?}", port.name);
            std::process::exit(1);
        }
    };
    // Listen on the box's *output* while we talk to its input, so a complaint is
    // caught. The A4 answers no dump request, but nothing rules out an
    // acknowledgement, and an unheard reply is indistinguishable from silence.
    let listening = port.name.clone();
    let reply = std::thread::spawn(move || capture_sysex(&listening, REPLY_WINDOW));

    // The send itself is `a4_transfer`'s, including the consent check and the
    // re-verify. Nothing about delivery is decided here any more.
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = send_pattern(&mut conn, &msg.wire, pacing, Consent::given_for(msg.slot), &cancel, |p| {
        if p.packets_sent == 1 || p.packets_sent % 8 == 0 || p.packets_sent == p.packets_total {
            print!("\r  {} / {} packets", p.packets_sent, p.packets_total);
            let _ = std::io::stdout().flush();
        }
    });
    println!();

    match result {
        Ok(report) => {
            println!("sent {} bytes to {} in {:.2} s.", report.bytes, port.name, report.elapsed.as_secs_f64());
            if !report.paced {
                println!("  NOT paced — this is the shape that did nothing on 2026-08-30.");
            }
        }
        Err(e) => {
            eprintln!("send failed: {e}");
            std::process::exit(1);
        }
    }

    match reply.join() {
        Ok(Ok(frames)) if frames.is_empty() => {
            println!("the box said nothing back (which it may well not).");
        }
        Ok(Ok(frames)) => {
            println!("the box replied with {} SysEx frame(s):", frames.len());
            for f in &frames {
                let head: Vec<String> = f.iter().take(16).map(|b| format!("{b:02x}")).collect();
                println!("  {} bytes: {}", f.len(), head.join(" "));
            }
        }
        Ok(Err(e)) => println!("could not listen for a reply: {e:?}"),
        Err(_) => println!("the reply listener panicked"),
    }

    println!("\nNow verify, because nothing here can:");
    println!("  1. dump {} from the box's front panel, capturing it", slot_name(msg.slot));
    println!("  2. python3 local/a4_pattern.py verify {path} <redump.mmon>");
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

struct Message {
    wire: Vec<u8>,
    slot: u8,
    checksum: u16,
    length: u16,
    payload: Vec<u8>,
}

/// Frame the bytes on disk and hand them to `a4_transfer::verify_before_send`.
///
/// The only thing left here is tolerating a file saved without its `F0`/`F7`,
/// which is a property of captures on this desk rather than of the format.
fn validate(raw: &[u8]) -> Result<Message, String> {
    let wire: Vec<u8> = match (raw.first(), raw.last()) {
        (Some(0xf0), Some(0xf7)) => raw.to_vec(),
        (Some(0xf0), _) => return Err("starts F0 but does not end F7 — truncated".into()),
        _ => {
            let mut w = Vec::with_capacity(raw.len() + 2);
            w.push(0xf0);
            w.extend_from_slice(raw);
            w.push(0xf7);
            w
        }
    };
    let parsed = verify_before_send(&wire)?;
    let body = &wire[1..wire.len() - 1];
    // Both trailer fields verified by `verify_before_send`; read back only to print.
    let checksum = u16::from(body[body.len() - 4]) << 7 | u16::from(body[body.len() - 3]);
    let length = u16::from(body[body.len() - 2]) << 7 | u16::from(body[body.len() - 1]);
    Ok(Message { wire, slot: parsed.slot, checksum, length, payload: parsed.payload })
}
