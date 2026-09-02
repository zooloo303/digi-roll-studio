// Name the Analog Four's p-lock parameter ids by watching the pool while a hand
// turns knobs on the box.
//
// **Read-only.** The only message sent is `0x6a`, the working-pattern request,
// through `assert_request_opcode`, plus `0x01` Device for liveness. No slot is
// written and the box never has to save — the same bargain `a4_lane_probe`
// strikes, and for the same reason: a save is the one step in the loop that can
// lose somebody's work.
//
// # Why this has to be measured rather than derived
//
// Three ids are known — `0x22` FLTR1 FREQ, `0x23` RESO, `0x24` OVERDRIVE, all
// three named by a hand on the box. Their published NRPN LSBs are 40, 41 and 42,
// so `param_id = nrpn - 6` fits all three. **It is wrong**: `osc1.level` is NRPN
// 4, which the same rule sends to −2. The two numberings plausibly follow one
// underlying parameter order, but not with a constant offset, and a table fitted
// from three adjacent points in one page would be confidently wrong in every
// other. DEVELOPMENT.md lessons 14 and 16.
//
// So: one knob, one lane, one line printed here.
//
// # The protocol
//
//   1. Put a trig on SYN1 step 1 (or wherever `--track`/`--step` say).
//   2. Start this. It prints a baseline of whatever is already locked.
//   3. **Hold the trig and turn one knob of one page, left to right.** Each turn
//      allocates a pool lane, and each new lane prints as a numbered line within
//      a second.
//   4. Say what the page's knobs are, in that order. The numbering is the join.
//
// Nothing is saved on the box, so `FUNC`+`NO` — or just clearing the trig —
// undoes a whole page.
//
//   cargo run -p digi_roll_studio --example a4_param_probe
//   cargo run -p digi_roll_studio --example a4_param_probe -- --track 1 --save local/a4-check/params
//
// # Reading the output
//
// A pool lane is keyed `(param_id, track)`, but what this prints is keyed
// `(param_id, track, step)` — **the locked step, not the lane**. One knob turn on
// one held trig is one locked step, so that is the unit a sweep actually
// produces. Keying on the lane instead would print nothing when a parameter
// already locked on some *other* step gets locked on this one, which is exactly
// the state a pattern that has been experimented on is already in.
//
// Re-turning a knob you have already turned on this step changes its value and
// adds no line, so a mis-turn is recoverable rather than a corrupted run.
//
// # The one thing this cannot see, and now says so
//
// **The join between these lines and a human's list of knob names is the order
// they arrive in, and that order is only trustworthy one knob at a time.** A poll
// reads the whole pool at once and `read_all_plocks` returns lanes in *lane*
// order — which is `(param_id, track)` order, not the order the knobs were
// turned. So two knobs turned inside one poll are reported sorted by id, and if
// the sort disagrees with the turn order the names silently attach to the wrong
// ids.
//
// That cost one OSC1 sweep on 2026-09-01 before it was noticed. A probe that
// cannot see a bad reading cannot report one, so this now prints a warning
// whenever a single poll turns up more than one parameter — the run stays usable
// and the ambiguous batches are named rather than left to be discovered by their
// results not making sense.
//
// The value is the box's own displayed number (the coarse byte), so a page swept
// with visibly different values can be checked against the screen afterwards
// without trusting the order the lanes came in.

use std::io::Write as _;
use std::time::{Duration, Instant};

use digi_midi::a4_transfer::A4Sink;
use digi_midi::{list_outputs, open_output_by_name, SysExInbox};
use digi_protocol::a4_pattern::{
    parse_working_pattern, read_track_trigs, DUMP_A4_PATTERN_WORKING_REQUEST, NUM_TRACKS,
    TRACK_NAMES,
};
use digi_protocol::a4_plocks::{read_all_plocks, A4Lane};
use digi_protocol::device::assert_request_opcode;
use digi_protocol::protocol::{
    build_api_message, build_dump_message, parse_sysex, API_DEVICE, API_RESPONSE,
    FAMILY_ANALOG_FOUR,
};

const DEFAULT_PORT: &str = "Analog Four";
const FIRST_REPLY: Duration = Duration::from_secs(3);
const QUIET_AFTER: Duration = Duration::from_millis(400);
const POLL_EVERY: Duration = Duration::from_millis(600);
const LIVENESS_WINDOW: Duration = Duration::from_secs(2);
const LIVENESS_TRIES: usize = 3;

/// The three ids a hand on the box has already named, so the run can say when it
/// is reproducing a known answer — the calibration that says the rig is right
/// before it is trusted for an unknown one.
const KNOWN: &[(u8, &str)] =
    &[(0x22, "FLTR1 FREQ"), (0x23, "FLTR1 RESO"), (0x24, "FLTR OVERDRIVE")];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port_fragment = flag(&args, "--port").unwrap_or_else(|| DEFAULT_PORT.to_string());
    let save_dir = flag(&args, "--save");
    let only_track: Option<usize> = flag(&args, "--track").and_then(|v| v.parse().ok());

    let request = DUMP_A4_PATTERN_WORKING_REQUEST;
    if let Err(e) = assert_request_opcode(request) {
        eprintln!("{request:#04x} refused before the wire: {e}");
        std::process::exit(2);
    }

    let outputs = list_outputs().expect("MIDI would not start");
    let needle = port_fragment.to_lowercase();
    let Some(port) = outputs.iter().find(|p| p.name.to_lowercase().contains(&needle)) else {
        eprintln!("no output port matching {port_fragment:?}");
        std::process::exit(1);
    };
    let mut inbox = SysExInbox::open(&port.name).expect("could not open the box's output");
    let mut conn = open_output_by_name(&port.name).expect("could not open the box's input");
    let mut msg_id: u16 = 0x5000;
    if !alive(&mut conn, &mut inbox, &mut msg_id) {
        eprintln!("the box is not answering 0x01 Device. Check the cable.");
        std::process::exit(1);
    }
    if let Some(dir) = &save_dir {
        let _ = std::fs::create_dir_all(dir);
    }

    println!("=== A4 param probe — {} — watching the working pattern ===", port.name);
    println!();
    println!("Hold a trig and turn ONE knob of ONE page at a time, left to right.");
    println!("Each new parameter prints a numbered line. Re-turning a knob you have");
    println!("already turned changes its value and adds no line, so a mis-turn is safe.");
    println!("Nothing is saved on the box. Ctrl-C to stop.");
    println!();

    // (param_id, track, zero-based step) -> (coarse, fine). The value is kept so
    // a *change* to an already-known lock can be reported too, which a probe that
    // only announced new lanes could not do — and which is what a knob sharing a
    // lane with another knob looks like from here. OSC TUN and FIN are the
    // coarse and fine bytes of one parameter (2026-09-01), so turning FIN
    // allocates nothing and moves one byte.
    let mut seen: Vec<((u8, u8, usize), (u8, u8))> = Vec::new();
    let mut n = 0usize;
    let mut first = true;
    let mut changes = 0usize;

    loop {
        let Some(frame) = fetch(&mut conn, &mut inbox, request) else {
            eprintln!("no reply to {request:#04x} — is the box awake?");
            std::thread::sleep(POLL_EVERY);
            continue;
        };
        let Ok(pattern) = parse_working_pattern(&frame) else {
            std::thread::sleep(POLL_EVERY);
            continue;
        };
        let lanes: Vec<A4Lane> = read_all_plocks(&pattern.payload)
            .unwrap_or_default()
            .into_iter()
            .filter(|l| only_track.is_none_or(|t| usize::from(l.track) == t))
            .collect();

        if first {
            first = false;
            println!("baseline — {} lane(s) already locked:", lanes.len());
            for l in &lanes {
                println!("    {}", describe(l));
                for (step, v) in l.values.iter().enumerate() {
                    if let Some(coarse) = v {
                        seen.push(((l.param_id, l.track, step), (*coarse, fine_at(l, step))));
                    }
                }
            }
            for (t, name) in TRACK_NAMES.iter().enumerate().take(NUM_TRACKS) {
                if let Ok(trigs) = read_track_trigs(&pattern.payload, t) {
                    if !trigs.is_empty() {
                        let steps: Vec<String> =
                            trigs.iter().map(|x| x.step.to_string()).collect();
                        println!("    {name}: trig(s) on {}", steps.join(" "));
                    }
                }
            }
            println!("\nwatching…\n");
            save(&save_dir, &frame, "baseline");
            let _ = std::io::stdout().flush();
            std::thread::sleep(POLL_EVERY);
            continue;
        }

        let mut fresh = 0usize;
        let mut fresh_change = false;
        let batch_start = n + 1;
        for l in &lanes {
            for (step, value) in l.values.iter().enumerate() {
                let Some(value) = value else { continue };
                let key = (l.param_id, l.track, step);
                let now = (*value, fine_at(l, step));
                if let Some(entry) = seen.iter_mut().find(|(k, _)| *k == key) {
                    if entry.1 != now {
                        let (was_c, was_f) = entry.1;
                        // The track is named because the id space is **per
                        // track kind** — measured 2026-09-01, when an FX-track
                        // lock landed on 0x1a and 0x29, both already mapped to
                        // synth parameters. A change line without a track cannot
                        // be attributed once more than one track is in play.
                        println!(
                            "       changed  param {:#04x} ({:>3})  {}  step {}  \
                             coarse {was_c} -> {}   fine {was_f} -> {}",
                            l.param_id,
                            l.param_id,
                            TRACK_NAMES.get(usize::from(l.track)).copied().unwrap_or("?"),
                            step + 1,
                            now.0,
                            now.1,
                        );
                        entry.1 = now;
                        fresh_change = true;
                    }
                    continue;
                }
                seen.push((key, now));
                n += 1;
                fresh += 1;
                let known = KNOWN
                    .iter()
                    .find(|(id, _)| *id == l.param_id)
                    .map(|(_, name)| format!("   <- already named: {name}"))
                    .unwrap_or_default();
                let fine = l.fine.as_ref().and_then(|f| f.get(step).copied().flatten());
                let fine = match fine {
                    Some(f) if f != 0 => format!(" (+{f}/128)"),
                    _ => String::new(),
                };
                println!(
                    "  #{n:<3} param {:#04x} ({:>3})  {}  step {} = {value}{fine}{known}",
                    l.param_id,
                    l.param_id,
                    TRACK_NAMES.get(usize::from(l.track)).copied().unwrap_or("?"),
                    step + 1,
                );
            }
        }
        if fresh > 0 || fresh_change {
            if fresh > 1 {
                // Sorted by id inside this batch, not by the order the knobs
                // moved. Slow down and the next batch will be one line again.
                println!(
                    "       ^^ #{batch_start}-#{n} arrived in ONE poll, so they are listed by \
                     param id, NOT by the order you turned them. Turn one knob per second to \
                     keep the order meaningful, or treat these {fresh} as unordered."
                );
            }
            changes += 1;
            save(&save_dir, &frame, &format!("param{changes:03}"));
            let _ = std::io::stdout().flush();
        }
        std::thread::sleep(POLL_EVERY);
    }
}

fn describe(l: &A4Lane) -> String {
    let held: Vec<String> = l
        .values
        .iter()
        .enumerate()
        .filter_map(|(step, v)| {
            v.map(|v| {
                let fine = l.fine.as_ref().and_then(|f| f.get(step).copied().flatten());
                match fine {
                    Some(f) if f != 0 => format!("step {} = {v} (+{f}/128)", step + 1),
                    _ => format!("step {} = {v}", step + 1),
                }
            })
        })
        .collect();
    format!(
        "param {:#04x} ({:>3})  {}  {}",
        l.param_id,
        l.param_id,
        TRACK_NAMES.get(usize::from(l.track)).copied().unwrap_or("?"),
        held.join(", "),
    )
}

fn save(dir: &Option<String>, frame: &[u8], tag: &str) {
    let Some(dir) = dir else { return };
    let path = format!("{dir}/a4-param-{tag}.syx");
    if let Err(e) = std::fs::write(&path, frame) {
        eprintln!("  NOT saved ({path}: {e})");
    }
}

fn fetch(conn: &mut impl A4Sink, inbox: &mut SysExInbox, request: u8) -> Option<Vec<u8>> {
    let _ = inbox.drain();
    conn.send_chunk(&build_dump_message(FAMILY_ANALOG_FOUR, request, 0, &[])).ok()?;
    listen(inbox, FIRST_REPLY, QUIET_AFTER)
        .into_iter()
        .find(|f| parse_sysex(f).dump.is_some())
}

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

/// The fine byte at one step, or zero where the lane has no extension — the
/// same reading [`A4Lane::word`] takes, so a line printed here and a word read
/// out of the pool agree.
fn fine_at(l: &A4Lane, step: usize) -> u8 {
    l.fine.as_ref().and_then(|f| f.get(step).copied().flatten()).unwrap_or(0)
}
