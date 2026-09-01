// Ask an Analog Four for a dump, one request opcode at a time, and report
// everything it says back — including the wedge, if one of them causes it.
//
// # Why this sweep exists at all
//
// "The A4 answers no dump request" has rested on one piece of evidence since
// 2026-08-28: the supported-opcode reply lists `01,02,03,04,06,07,09` and
// `50-5e`, and no `0x6x`. No request has ever actually been *sent* to this box.
// What 2026-08-30 established is that the advertised list describes the **API
// namespace** — the `0x53` file API whose `0x54` is FileOpen — and says nothing
// about the **dump namespace**, whose `0x54` is a pattern. The box demonstrably
// speaks the dump namespace in both directions (its panel emits `0x53`/`0x54`
// dumps; it accepts our `0x54` sends), and *those* opcodes are not in the
// advertised list either, because the list never described that namespace.
// So the absence of `0x6x` from it is not testimony about dump requests.
// That is DEVELOPMENT.md lesson 11's shape — absence of the other generation's
// opcode read as absence of the capability — which has cost this project four
// findings already, the pattern path itself among them.
//
// The prior is favourable: the A4's dump header turned out to be the gen-2
// header field for field (PLAN.md §9, 2026-08-31), and on gen-2 a request is
// its dump type plus 0x10. If the convention travels with the header, a
// pattern request is 0x64 (the A4's pattern dump is 0x54) and a sound request
// is 0x63. The panel also offers kit and pattern+kit dumps, whose types nobody
// has captured, so the sweep runs the whole 0x60-0x6e range rather than the
// two educated guesses. 0x6f (whole project on gen-2) is excluded the same way
// probe_dump_types excluded it: it streams megabytes on a box that answers,
// and this sweep wants one small answer per opcode.
//
// # Why this is not `ElektronDevice::fetch_dump` in a loop
//
// `fetch_dump` discards any reply whose family, type or index does not match
// the request — correct for a transfer, wrong for discovery, where a reply
// with a surprising index convention is the finding rather than the noise.
// This probe listens raw and reports every frame that lands, classified but
// never filtered.
//
// # The hazard, and the discipline that answers it
//
// This is the box that takes down its whole SysEx API on a body it cannot
// parse — it stops answering `0x01` while the digis on the same bus answer
// normally, and it needs a power cycle (PLAN.md §10, four cycles bought three
// facts). That was earned on malformed *file-API* bodies, and a well-formed
// dump-header request is a different message; but the discipline transfers:
// after every probe message the box is asked `0x01` Device, and the moment it
// stops answering the sweep STOPS and names the opcode that killed it, so
// "the box went offline" arrives as a finding with a culprit rather than a
// mystery three opcodes later. Resume after the power cycle with `--from`.
//
// Every reply frame is also written to local/a4-check/dump-probe/ as .syx,
// so whatever answers never needs re-capturing.
//
// Run with the box online:
//   cargo run -p digi_roll_studio --example a4_dump_probe
//   cargo run -p digi_roll_studio --example a4_dump_probe -- --index 15
//   cargo run -p digi_roll_studio --example a4_dump_probe -- --from 0x65
//   cargo run -p digi_roll_studio --example a4_dump_probe -- --opcode 0x64
//
// # What the first run found — 2026-08-31, A4 0195
//
// The box answered eleven of fifteen and never wedged. PLAN.md §10 "The A4
// answers dump requests" is the full map; the shape of it: `0x60` streams the
// whole project (417 frames), `0x62`-`0x67` fetch kit / sound / pattern /
// 16×unknown / settings / global by slot index (pattern index verified linear
// 0-127 across banks), `0x68`-`0x6d` return the same six objects' *current
// state* with the index ignored, `0x61`/`0x6e` are silent, and a requested
// pattern is byte-identical in format to a front-panel dump — `parse_pattern`
// read it unmodified, and A16 came back carrying the trigs `build_trig_probe`
// had written. The lesson-11 paragraph above stands as the reason this sweep
// was worth one evening five days after the claim it demolished was written.

use std::io::Write as _;
use std::time::{Duration, Instant};

use digi_midi::a4_transfer::A4Sink;
use digi_midi::{list_outputs, open_output_by_name, SysExInbox};
use digi_protocol::a4_pattern::{is_a4_pattern, parse_pattern, read_track_trigs, slot_name, TRACK_NAMES};
use digi_protocol::device::assert_request_opcode;
use digi_protocol::protocol::{
    build_api_message, build_dump_message, parse_sysex, SysExKind, API_DEVICE, API_RESPONSE,
    FAMILY_ANALOG_FOUR,
};

/// Matched as a case-insensitive substring of the port name, like
/// `a4_pattern_send`'s.
const DEFAULT_PORT: &str = "Analog Four";

/// How long to wait for the *first* frame after a request. The digis answer a
/// known request in well under a second; three is generous without making
/// eleven silences cost a minute.
const FIRST_REPLY: Duration = Duration::from_secs(3);

/// Silence after the last complete frame that means the reply is over. A
/// pattern is ~15 KB and the box may pace its own send at cable rate, so the
/// window has to survive gaps *between* packets of one frame — `mid_frame`
/// covers those — and gaps between two frames of a multi-frame answer.
const QUIET_AFTER: Duration = Duration::from_millis(1500);

/// How long to wait for the box to answer `0x01` Device, per try. The liveness
/// check retries, because one lost reply is a USB fact and not a wedge.
const LIVENESS_WINDOW: Duration = Duration::from_secs(2);
const LIVENESS_TRIES: usize = 3;

/// Where reply frames land, relative to the workspace root.
const SAVE_DIR: &str = "local/a4-check/dump-probe";

/// The gen-2 name of each response type, for orientation only — the whole
/// point of the sweep is that the A4 assigns these bytes its own meanings
/// (`0x54` is already known to be a pattern here and project settings there).
fn gen2_name(response_type: u8) -> &'static str {
    match response_type {
        0x50 => "pattern+kit on gen-2",
        0x51 => "pattern on gen-2",
        0x52 => "kit on gen-2",
        0x53 => "sound on gen-2; sound HERE (captured off the panel)",
        0x54 => "project settings on gen-2; PATTERN here (captured off the panel)",
        0x5b => "kit-track sound on gen-2",
        _ => "unmapped on gen-2",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let index: u8 = flag(&args, "--index").and_then(|v| v.parse().ok()).unwrap_or(0);
    let port_fragment = flag(&args, "--port").unwrap_or_else(|| DEFAULT_PORT.to_string());
    let from: u8 = flag(&args, "--from").and_then(|v| parse_hex(&v)).unwrap_or(0x60);
    let only: Option<u8> = flag(&args, "--opcode").and_then(|v| parse_hex(&v));

    let sweep: Vec<u8> = match only {
        Some(op) => vec![op],
        None => (from..=0x6e).collect(),
    };
    for &op in &sweep {
        // The read-only guard, kept even though every opcode here is inside its
        // range: if this list is ever edited, the edit meets the guard rather
        // than the box.
        if let Err(e) = assert_request_opcode(op) {
            eprintln!("{op:#04x} refused before the wire: {e}");
            std::process::exit(2);
        }
    }

    let outputs = list_outputs().expect("MIDI would not start");
    let needle = port_fragment.to_lowercase();
    let Some(port) = outputs.iter().find(|p| p.name.to_lowercase().contains(&needle)) else {
        eprintln!("no output port matching {port_fragment:?}. Ports present:");
        for p in &outputs {
            eprintln!("  {}", p.name);
        }
        std::process::exit(1);
    };

    let mut inbox = SysExInbox::open(&port.name).expect("could not open the box's output");
    let mut conn = open_output_by_name(&port.name).expect("could not open the box's input");
    // Not `FIRST_MSG_ID`: that is `device.rs`'s private counter for its own
    // connection. Any id works — the reply is matched on it, not allocated by it.
    let mut msg_id: u16 = 0x2000;

    println!("=== A4 dump-request probe — {} — index {index} ===", port.name);

    // Baseline before the first probe: a box that is not answering 0x01 *now*
    // would make every silence below meaningless.
    if !alive(&mut conn, &mut inbox, &mut msg_id) {
        eprintln!("the box is not answering 0x01 Device before any probe was sent.");
        eprintln!("check the cable, or power-cycle it if a previous run wedged it.");
        std::process::exit(1);
    }
    println!("baseline: the box answers 0x01 Device.\n");
    println!("  REQ   EXPECTS  RESULT");

    let _ = std::fs::create_dir_all(SAVE_DIR);

    for &request in &sweep {
        let response_type = request - 0x10;
        print!("  {request:#04x}  {response_type:#04x}     ");
        let _ = std::io::stdout().flush();

        // Anything already queued belongs to the previous opcode, not this one.
        let _ = inbox.drain();
        if let Err(e) =
            conn.send_chunk(&build_dump_message(FAMILY_ANALOG_FOUR, request, index, &[]))
        {
            println!("send failed: {e} — stopping");
            std::process::exit(1);
        }

        let frames = listen(&mut inbox, FIRST_REPLY, QUIET_AFTER);
        if frames.is_empty() {
            println!("silent   ({})", gen2_name(response_type));
        } else {
            println!("** {} frame(s) ** ({})", frames.len(), gen2_name(response_type));
            for (n, frame) in frames.iter().enumerate() {
                describe(frame);
                let path = format!("{SAVE_DIR}/req{request:02x}_idx{index}_frame{n}.syx");
                match std::fs::write(&path, frame) {
                    Ok(()) => println!("        saved: {path}"),
                    Err(e) => println!("        NOT saved ({path}: {e}) — copy the hex above"),
                }
            }
        }

        // The check that lets a wedge name its culprit. Silence above plus
        // death here means *this* opcode did it, not the next one.
        if !alive(&mut conn, &mut inbox, &mut msg_id) {
            println!("\n*** the box stopped answering 0x01 Device after {request:#04x}. ***");
            println!("*** That opcode wedged its SysEx API — power-cycle the box, ***");
            if request < 0x6e && only.is_none() {
                println!("*** then resume with: --from {:#04x} ***", request + 1);
            }
            std::process::exit(1);
        }
    }

    println!("\nsweep complete, and the box still answers 0x01 Device.");
    println!("A silent row is a real answer; a frame is a discovery. Anything saved");
    println!("under {SAVE_DIR}/ is ready for decoding without re-capture.");
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn parse_hex(v: &str) -> Option<u8> {
    u8::from_str_radix(v.trim_start_matches("0x"), 16).ok()
}

/// Wait for a reply: up to `first` for anything at all, then until `quiet` of
/// silence after the last activity. `mid_frame` counts as activity, so a 15 KB
/// dump the box paces at cable rate is not cut off between its packets.
fn listen(inbox: &mut SysExInbox, first: Duration, quiet: Duration) -> Vec<Vec<u8>> {
    let poll = Duration::from_millis(20);
    let started = Instant::now();
    let mut last_activity = Instant::now();
    let mut frames = Vec::new();
    loop {
        let got = inbox.drain();
        if !got.is_empty() || inbox.mid_frame() {
            last_activity = Instant::now();
        }
        frames.extend(got);
        if frames.is_empty() && !inbox.mid_frame() {
            if started.elapsed() >= first {
                return frames;
            }
        } else if last_activity.elapsed() >= quiet {
            return frames;
        }
        std::thread::sleep(poll);
    }
}

/// One frame, classified but never filtered: the reply that fails every
/// expectation is the one this probe exists to see.
fn describe(frame: &[u8]) {
    let head: Vec<String> = frame.iter().take(16).map(|b| format!("{b:02x}")).collect();
    let parsed = parse_sysex(frame);
    match parsed.kind {
        SysExKind::Dump => {
            let dump = parsed.dump.as_ref().expect("kind says dump");
            println!(
                "        dump: family {:#04x}, type {:#04x}, index {}, {} payload bytes, checksum {}, count {}",
                dump.family,
                dump.dump_type,
                dump.index,
                dump.payload.len(),
                if dump.checksum_ok { "ok" } else { "BAD" },
                if dump.count_ok { "ok" } else { "BAD" },
            );
            if is_a4_pattern(&parsed) {
                match parse_pattern(frame) {
                    Ok(p) => {
                        let trigs: Vec<String> = TRACK_NAMES
                            .iter()
                            .enumerate()
                            .filter_map(|(t, name)| {
                                let n = read_track_trigs(&p.payload, t).ok()?.len();
                                (n > 0).then(|| format!("{name}:{n}"))
                            })
                            .collect();
                        println!(
                            "        ** AN A4 PATTERN — slot {} ({}), trigs {} **",
                            p.slot,
                            slot_name(p.slot),
                            if trigs.is_empty() { "none".into() } else { trigs.join(" ") }
                        );
                    }
                    Err(e) => println!("        shaped like an A4 pattern but: {e}"),
                }
            }
        }
        SysExKind::Api => {
            let api = parsed.api.as_ref().expect("kind says api");
            println!(
                "        API frame (unexpected here): api_id {:#04x}, {} arg bytes",
                api.api_id,
                api.args.len()
            );
        }
        SysExKind::Foreign | SysExKind::Unknown => {
            println!("        unclassified, {} bytes", frame.len());
        }
    }
    println!("        head: {}", head.join(" "));
}

/// Ask `0x01` Device and wait for its `0x81` reply, with retries. One lost
/// reply is USB weather; [`LIVENESS_TRIES`] losses in a row on a box that
/// answered before this probe's message is the wedge.
fn alive(conn: &mut impl A4Sink, inbox: &mut SysExInbox, msg_id: &mut u16) -> bool {
    for _ in 0..LIVENESS_TRIES {
        *msg_id = msg_id.wrapping_add(1);
        let id = *msg_id;
        let _ = inbox.drain();
        if conn.send_chunk(&build_api_message(id, API_DEVICE, &[], 0)).is_err() {
            return false;
        }
        let deadline = Instant::now() + LIVENESS_WINDOW;
        while Instant::now() < deadline {
            for frame in inbox.drain() {
                let parsed = parse_sysex(&frame);
                if let Some(api) = parsed.api {
                    if api.resp_id == id && api.api_id == API_DEVICE.wrapping_add(API_RESPONSE) {
                        return true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    false
}
