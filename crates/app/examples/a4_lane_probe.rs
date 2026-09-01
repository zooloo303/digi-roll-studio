// Watch the Analog Four's *working* pattern and name every byte a knob turn
// changes.
//
// **Read-only, structurally.** The only message this sends is `0x6a` — the
// working-pattern request — through `assert_request_opcode`, plus `0x01` Device
// for liveness. There is no store opcode anywhere in the file, nothing is
// written to a slot, and the box never has to save.
//
// # Why the working pattern and not a slot
//
// `0x64` reads a *stored* pattern, so mapping a field that way costs a save on
// the box per measurement, and a save is the one step in the loop that can lose
// somebody's work. `0x6a` returns the edit buffer instead: it answered on
// 2026-08-31 carrying A01's 32 saved trigs plus two the box had not saved, so a
// knob turn shows up here immediately and nothing on the +Drive moves.
//
// # What it is for
//
// Six of the nine per-step lanes in `a4_pattern::LANES` are named from
// correlation and three are not named at all — and the A4's per-trig
// *condition*, which puts probability and trig condition on one knob where the
// digis use three lanes, is in one of the two unnamed ones. A correlation
// cannot say which, and it cannot say what a length byte means musically. The
// box can, in one turn of one knob: change one field on one step, and the diff
// below prints the lane, the step and the two bytes.
//
// So the protocol is: hold a trig, turn one knob, read the line this prints.
//
//   cargo run -p digi_roll_studio --example a4_lane_probe
//   cargo run -p digi_roll_studio --example a4_lane_probe -- --save local/a4-check/lanes
//
// `--save` writes every payload that differs from the one before it, so a
// session leaves behind the captures a fixture would need. `--slot N` reads a
// stored slot once instead of watching, for a baseline.

use std::io::Write as _;
use std::time::{Duration, Instant};

use digi_midi::a4_transfer::A4Sink;
use digi_midi::{list_outputs, open_output_by_name, SysExInbox};
use digi_protocol::a4_conditions;
use digi_protocol::a4_pattern::{
    describe_offset, effective_length, effective_note, effective_velocity, note_name,
    parse_pattern, parse_working_pattern, read_track_trigs, slot_name, A4Pattern,
    DUMP_A4_PATTERN_REQUEST, DUMP_A4_PATTERN_WORKING_REQUEST, NUM_TRACKS, TRACK_NAMES,
};
use digi_protocol::pattern::length_byte_to_steps;
use digi_protocol::device::assert_request_opcode;
use digi_protocol::protocol::{
    build_api_message, build_dump_message, parse_sysex, API_DEVICE, API_RESPONSE,
    FAMILY_ANALOG_FOUR,
};

/// Matched as a case-insensitive substring of the port name, like the other A4
/// examples.
const DEFAULT_PORT: &str = "Analog Four";

/// How long to wait for the first frame of a reply. The box answers a working
/// pattern in well under a second on USB.
const FIRST_REPLY: Duration = Duration::from_secs(3);

/// Silence after the last packet that means the 15 KB reply is complete.
const QUIET_AFTER: Duration = Duration::from_millis(400);

/// Between polls. Fast enough that a knob turn and the line describing it feel
/// like the same event; slow enough that the box is not answering a 15 KB
/// request back to back forever.
const POLL_EVERY: Duration = Duration::from_millis(600);

/// How long to wait for the box to answer `0x01` Device, per try, and how many
/// tries: one lost reply is USB weather.
const LIVENESS_WINDOW: Duration = Duration::from_secs(2);
const LIVENESS_TRIES: usize = 3;

/// A diff this large is a pattern *change* — a different slot loaded, or a
/// clear — rather than an edit, and printing 12,000 lines would bury the run.
const DIFF_IS_A_RELOAD: usize = 400;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port_fragment = flag(&args, "--port").unwrap_or_else(|| DEFAULT_PORT.to_string());
    let save_dir = flag(&args, "--save");
    let slot: Option<u8> = flag(&args, "--slot").and_then(|v| v.parse().ok());

    let request = match slot {
        Some(_) => DUMP_A4_PATTERN_REQUEST,
        None => DUMP_A4_PATTERN_WORKING_REQUEST,
    };
    // The read-only guard, met before the wire rather than after it.
    if let Err(e) = assert_request_opcode(request) {
        eprintln!("{request:#04x} refused before the wire: {e}");
        std::process::exit(2);
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
    let mut msg_id: u16 = 0x3000;

    if !alive(&mut conn, &mut inbox, &mut msg_id) {
        eprintln!("the box is not answering 0x01 Device. Check the cable.");
        std::process::exit(1);
    }

    if let Some(dir) = &save_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("could not make {dir}: {e}");
            std::process::exit(1);
        }
    }

    // The one-shot arm: a stored slot, printed as the box's own values.
    //
    // This is the arm that checks the *mapping* rather than the plumbing. A
    // write verifying byte-identical proves our bytes landed and says nothing
    // about what they mean — it is the same bytes compared with themselves. The
    // table below is this app's reading of a slot in the box's own units, so it
    // can be held against the box's screen, which is the only witness that ever
    // settles a unit.
    if let Some(slot) = slot {
        println!("=== A4 lane probe — {} — stored slot {} ===", port.name, slot_name(slot));
        match fetch(&mut conn, &mut inbox, request, slot) {
            Some(frame) => match parse_pattern(&frame) {
                Ok(p) => {
                    summarise(&p);
                    println!();
                    tabulate(&p);
                    save(&save_dir, &frame, &format!("slot{slot:02}"));
                }
                Err(e) => eprintln!("the reply is not a pattern: {e}"),
            },
            None => eprintln!("no reply — the box did not answer {request:#04x}"),
        }
        return;
    }

    println!("=== A4 lane probe — {} — watching the working pattern ===", port.name);
    println!();
    println!("Turn ONE knob on ONE held trig at a time. Each change prints the lane");
    println!("it wrote, so what the box calls VEL names its own bytes. Ctrl-C to stop.");
    println!();

    let mut previous: Option<A4Pattern> = None;
    let mut change = 0usize;
    loop {
        let Some(frame) = fetch(&mut conn, &mut inbox, request, 0) else {
            eprintln!("no reply to {request:#04x} — is the box awake?");
            std::thread::sleep(POLL_EVERY);
            continue;
        };
        let current = match parse_working_pattern(&frame) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("reply is not a working pattern: {e}");
                std::thread::sleep(POLL_EVERY);
                continue;
            }
        };
        match &previous {
            None => {
                println!("baseline:");
                summarise(&current);
                println!("\nwatching…");
                save(&save_dir, &frame, "baseline");
            }
            Some(before) => {
                let diff: Vec<usize> = (0..current.payload.len())
                    .filter(|&i| before.payload[i] != current.payload[i])
                    .collect();
                if !diff.is_empty() {
                    change += 1;
                    println!("\n--- change {change}: {} byte(s) ---", diff.len());
                    if diff.len() > DIFF_IS_A_RELOAD {
                        println!("  {} bytes moved — that is a pattern reload or a clear,", diff.len());
                        println!("  not an edit. Re-baselining on it.");
                    } else {
                        for i in diff {
                            println!(
                                "  {:6}  {:02x} -> {:02x}   {}",
                                i,
                                before.payload[i],
                                current.payload[i],
                                describe_offset(i)
                            );
                        }
                    }
                    save(&save_dir, &frame, &format!("change{change:03}"));
                }
            }
        }
        previous = Some(current);
        // Flushed every poll because a mapping session wants a log: Rust's
        // stdout is block-buffered into a pipe, so `| tee session.log` would
        // otherwise show nothing for the first 8 KB — which reads as a probe
        // that is not seeing the knob.
        let _ = std::io::stdout().flush();
        std::thread::sleep(POLL_EVERY);
    }
}

/// Trigs per track, so the run starts by agreeing with the front panel about
/// what is on the screen. A probe whose baseline already disagrees is measuring
/// the wrong pattern.
fn summarise(pattern: &A4Pattern) {
    for (track, name) in TRACK_NAMES.iter().enumerate().take(NUM_TRACKS) {
        let Ok(trigs) = read_track_trigs(&pattern.payload, track) else { continue };
        if trigs.is_empty() {
            continue;
        }
        let steps: Vec<String> = trigs.iter().map(|t| t.step.to_string()).collect();
        println!("  {name:5} {:2} trig(s): {}", trigs.len(), steps.join(" "));
    }
}

/// Every trig of every track, in the units the box shows — so a line here can
/// be read straight against the front panel.
///
/// The raw byte is printed beside each value, because the two together are what
/// makes a disagreement diagnosable: a wrong *unit* shows up as a byte that
/// matches the box and a value that does not, and a wrong *lane* shows up as
/// neither matching.
fn tabulate(pattern: &A4Pattern) {
    for (track, name) in TRACK_NAMES.iter().enumerate().take(NUM_TRACKS) {
        let Ok(trigs) = read_track_trigs(&pattern.payload, track) else { continue };
        if trigs.is_empty() {
            continue;
        }
        println!("{name}");
        println!("  step  note      VEL        LEN            micro     TRC");
        for trig in &trigs {
            let velocity = effective_velocity(&pattern.payload, track, trig).unwrap_or(0);
            let length = effective_length(&pattern.payload, track, trig).unwrap_or(0);
            // An unset lane is worth showing as unset: it means the trig
            // follows the track default, and after a write from this app it
            // will not, because a write states every lane.
            let mark = |lane: Option<u8>| if lane.is_none() { "*" } else { " " };
            let condition = match trig.condition {
                None => "-".to_owned(),
                Some(byte) => match a4_conditions::from_byte(byte) {
                    Some(c) => format!("{} ({byte:#04x})", a4_conditions::label(c)),
                    None => format!("PAST THE MENU ({byte:#04x})"),
                },
            };
            let note = match effective_note(&pattern.payload, track, trig) {
                Ok(Some(n)) => format!("{:4} ({n:#04x})", note_name(n)),
                _ => "trigless   ".to_owned(),
            };
            println!(
                "  {:4}  {note}  {velocity:3}{} ({velocity:#04x})  {:>8}{} ({length:#04x})  \
                 {:+3} tick  {condition}",
                trig.step,
                mark(trig.velocity),
                format_length(length),
                mark(trig.length),
                trig.micro_timing,
            );
        }
    }
    println!();
    println!("  * = the lane is unset, so the trig follows the track default. A write from");
    println!("    this app states every lane, so these turn into explicit values.");
}

/// The box's own length wording: `.125` to `128`, and `INF` at the top.
fn format_length(byte: u8) -> String {
    let steps = length_byte_to_steps(byte);
    if steps.is_infinite() {
        return "INF".to_owned();
    }
    // Trailing zeros off, because the box writes `2` rather than `2.000`.
    format!("{steps}")
}

fn save(dir: &Option<String>, frame: &[u8], tag: &str) {
    let Some(dir) = dir else { return };
    let path = format!("{dir}/a4-working-{tag}.syx");
    match std::fs::write(&path, frame) {
        Ok(()) => println!("  saved {path}"),
        Err(e) => eprintln!("  NOT saved ({path}: {e})"),
    }
}

/// Send one request and return the first frame that parses as an A4 dump.
fn fetch(
    conn: &mut impl A4Sink,
    inbox: &mut SysExInbox,
    request: u8,
    index: u8,
) -> Option<Vec<u8>> {
    let _ = inbox.drain();
    conn.send_chunk(&build_dump_message(FAMILY_ANALOG_FOUR, request, index, &[])).ok()?;
    listen(inbox, FIRST_REPLY, QUIET_AFTER)
        .into_iter()
        .find(|f| parse_sysex(f).dump.is_some())
}

/// Does the box still answer `0x01` Device? The same liveness check
/// `a4_dump_probe` opens with, and deliberately the same *matching* as well:
/// the reply's api id is `API_DEVICE + API_RESPONSE`, and it carries the
/// request's id in `resp_id`. Matching the bare `API_RESPONSE` instead is a
/// check that can never pass, and it reads exactly like a box that is not
/// plugged in — which cost one run here before it was noticed.
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
                if let Some(api) = parse_sysex(&frame).api {
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

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}
