// Ask the Analog Four whether it **requires** the compacted, `(param_id, track)`
// -sorted p-lock pool it produces — PLAN.md §10, "What the p-lock writer still
// needs", and the last thing standing between `a4_plocks`'s reader and its
// writer.
//
// That the box produces that order is measured. That it *needs* it is not, and
// a pool written in another order is a guess delivered to hardware.
//
// # The question is three questions
//
// "Compacted and sorted" bundles three independent properties, and they have
// different consequences for the encoder — `a4_plocks::PoolOrder` is the split:
//
//   packed              no holes between allocated lanes
//   extensions_adjacent every `80 80` immediately after the lane it extends
//   keys_sorted         allocated lanes non-decreasing by (param_id, track)
//
// If holes are tolerated, a gen-1 writer can be gen-2-shaped: edit lanes in
// place, move the fewest bytes, stay scoped to the track the caller named. If
// they are not, the writer has to repack — which means moving lanes belonging
// to five tracks nobody asked it to touch. The middle property is the reader's
// own assumption (`read_all_plocks` binds an extension to the lane physically
// before it) and has never been tested, because the box has never produced a
// pool where the two are apart.
//
// So this sends three variants, each **one** deviation from a ground truth the
// box itself authored, and each byte-identical everywhere else.
//
// # The two theories it separates
//
//   1. The sorted pool is a *serialisation artefact*. The box holds p-locks in
//      RAM keyed by (param, track) and writes them out in iteration order; load
//      is a linear scan that accepts anything. Compaction at edit time would be
//      a multi-kilobyte memmove for a knob turn, which no sequencer does — what
//      looks like "compacts on edit" from outside is "re-serialises on save".
//   2. The file layout *is* the structure, indexed or binary-searched on load.
//
// The discriminating observation needs no eyes: write a scrambled pool, load it
// on the box, dump the **working** buffer. Under 1 it comes back
// re-canonicalised with both locks present, which proves the box parsed every
// lane and rebuilt them from its own structure. Under 2 a lane is missing or
// misread — and "lane silently missing" is the failure mode that matters, so
// that is where the instrument points.
//
// Bytes surviving a round trip only prove the box *stored* them. The front
// panel is the only witness that a lock is *live*, which is why each variant
// ends with a line telling the operator what to hold and what to read off.
//
// # Arms
//
//   --calibrate --slot N
//       Read-only. Fetch stored slot N (0x64) and the working buffer (0x6a) and
//       byte-compare. Establishes whether a working dump is a faithful copy of
//       the stored bytes *before* any scrambled pool exists to confuse the
//       reading. Without this, "it came back sorted" proves nothing.
//
//   --baseline --slot N [--save DIR]
//       Read-only. Fetch slot N, print its pool and its three order properties,
//       save it as the ground truth the variants are built from.
//
//   --variant A|B|C --slot N [--save DIR] [--send]
//       Default is a rehearsal: builds the variant from a freshly fetched slot
//       and prints what would change, opening no port for writing. `--send`
//       runs the ceremony — re-fetch, confirm, backup, send, read back,
//       byte-compare.
//
//   --working [--against FILE] [--save DIR]
//       Read-only. Fetch the working buffer, print its pool, and diff it
//       against a saved payload — the post-load observation.
//
// A16 (`--slot 15`) is the scratch slot. Free experimentation on the A4 is
// authorised and a factory reset is acceptable; that consent is A4-only.

use std::io::Write as _;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use digi_midi::a4_transfer::{send_pattern, A4Sink, Consent, Pacing, CAN_PACE};
use digi_midi::{list_outputs, open_output_by_name, SysExInbox};
use digi_protocol::a4_pattern::{
    build_pattern, parse_pattern, parse_working_pattern, read_track_trigs, slot_name,
    DUMP_A4_PATTERN_REQUEST, DUMP_A4_PATTERN_WORKING_REQUEST, NUM_TRACKS, PAYLOAD_LEN,
    TRACK_NAMES,
};
use digi_protocol::a4_plocks::{
    pool_order, read_all_plocks, A4Lane, FREE, LANE_SIZE, NUM_LANES, POOL_BASE,
};
use digi_protocol::device::assert_request_opcode;
use digi_protocol::protocol::{
    build_api_message, build_dump_message, parse_sysex, API_DEVICE, API_RESPONSE,
    FAMILY_ANALOG_FOUR,
};

const DEFAULT_PORT: &str = "Analog Four";
const CONSENT: &str = "OVERWRITE";

const FIRST_REPLY: Duration = Duration::from_secs(3);
const QUIET_AFTER: Duration = Duration::from_millis(400);
const LIVENESS_WINDOW: Duration = Duration::from_secs(2);
const LIVENESS_TRIES: usize = 3;

/// How long to let the box settle after a send before reading it back. The
/// round trip on 2026-08-31 needed none, and this is cheap insurance against
/// reading a slot mid-commit.
const SETTLE_AFTER_SEND: Duration = Duration::from_millis(500);

/// Diffs printed before the list is truncated. A variant should move a few
/// hundred bytes at most — anything larger is a different pattern, not an edit.
const DIFF_CAP: usize = 40;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port_fragment = flag(&args, "--port").unwrap_or_else(|| DEFAULT_PORT.to_string());
    let save_dir = flag(&args, "--save");
    let slot: u8 = flag(&args, "--slot").and_then(|v| v.parse().ok()).unwrap_or(15);
    let for_real = args.iter().any(|a| a == "--send");

    // Both request opcodes through the read-only guard, before the wire.
    for request in [DUMP_A4_PATTERN_REQUEST, DUMP_A4_PATTERN_WORKING_REQUEST] {
        if let Err(e) = assert_request_opcode(request) {
            eprintln!("{request:#04x} refused before the wire: {e}");
            std::process::exit(2);
        }
    }

    let Some(port_name) = find_port(&port_fragment) else { std::process::exit(1) };
    let mut inbox = SysExInbox::open(&port_name).expect("could not open the box's output");
    let mut conn = open_output_by_name(&port_name).expect("could not open the box's input");
    let mut msg_id: u16 = 0x4000;
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

    if args.iter().any(|a| a == "--calibrate") {
        calibrate(&mut conn, &mut inbox, slot, &save_dir);
    } else if args.iter().any(|a| a == "--baseline") {
        baseline(&mut conn, &mut inbox, slot, &save_dir);
    } else if args.iter().any(|a| a == "--working") {
        working(&mut conn, &mut inbox, flag(&args, "--against"), &save_dir);
    } else if let Some(which) = flag(&args, "--variant") {
        variant(&mut conn, &mut inbox, slot, &which, for_real, &save_dir);
    } else {
        eprintln!("pick an arm: --calibrate | --baseline | --variant A|B|C | --working");
        eprintln!("see the header. --slot defaults to 15 (A16, the scratch slot).");
        std::process::exit(2);
    }
}

// --- stage 0: is a working dump a faithful copy of the stored bytes? ---------

fn calibrate(conn: &mut impl A4Sink, inbox: &mut SysExInbox, slot: u8, save_dir: &Option<String>) {
    println!("=== stage 0 — calibrating the instrument, read-only ===");
    println!();
    println!("Comparing stored {} (0x64) against the working buffer (0x6a).", slot_name(slot));
    println!("This only means anything if {} is the pattern SELECTED on the box.", slot_name(slot));
    println!();

    let Some(stored_frame) = fetch(conn, inbox, DUMP_A4_PATTERN_REQUEST, slot) else {
        eprintln!("no reply to the stored-pattern request");
        return;
    };
    let stored = match parse_pattern(&stored_frame) {
        Ok(p) => p,
        Err(e) => return eprintln!("the stored reply is not a pattern: {e}"),
    };
    let Some(working_frame) = fetch(conn, inbox, DUMP_A4_PATTERN_WORKING_REQUEST, 0) else {
        eprintln!("no reply to the working-pattern request");
        return;
    };
    let live = match parse_working_pattern(&working_frame) {
        Ok(p) => p,
        Err(e) => return eprintln!("the working reply is not a pattern: {e}"),
    };

    save(save_dir, &stored_frame, &format!("calibrate-stored-slot{slot:02}"));
    save(save_dir, &working_frame, "calibrate-working");

    println!("stored  {}:", slot_name(slot));
    summarise(&stored.payload);
    println!("working buffer:");
    summarise(&live.payload);
    println!();

    let diff = diff(&stored.payload, &live.payload);
    if diff.is_empty() {
        println!("IDENTICAL in all {PAYLOAD_LEN} bytes.");
        println!();
        println!("  So a working dump is a faithful copy of the stored bytes, and any");
        println!("  difference after loading a scrambled pool is the box normalising.");
        println!("  That is the reading the variants need.");
    } else {
        println!("{} byte(s) differ between the stored slot and the working buffer.", diff.len());
        report_diff(&stored.payload, &live.payload, &diff);
        println!();
        println!("  A working dump is NOT a verbatim copy. Note which regions move: if the");
        println!("  pool ({POOL_BASE}..) is among them the variants cannot be read this way,");
        println!("  and if it is not, the pool comparison is still sound.");
        let pool_moved = diff.iter().any(|&i| i >= POOL_BASE);
        println!();
        println!("  pool bytes among the differences: {}", if pool_moved { "YES" } else { "no" });
    }
    println!();
    print_pool("stored", &stored.payload);
    print_pool("working", &live.payload);
}

// --- stage 1: the ground truth -----------------------------------------------

fn baseline(conn: &mut impl A4Sink, inbox: &mut SysExInbox, slot: u8, save_dir: &Option<String>) {
    println!("=== stage 1 — ground truth from {} , read-only ===", slot_name(slot));
    println!();
    let Some(frame) = fetch(conn, inbox, DUMP_A4_PATTERN_REQUEST, slot) else {
        eprintln!("no reply to the stored-pattern request");
        return;
    };
    let p = match parse_pattern(&frame) {
        Ok(p) => p,
        Err(e) => return eprintln!("the reply is not a pattern: {e}"),
    };
    save(save_dir, &frame, &format!("baseline-slot{slot:02}"));
    summarise(&p.payload);
    println!();
    print_pool("baseline", &p.payload);

    let groups = groups_of(&p.payload);
    println!();
    match check_baseline(&p.payload, &groups) {
        Ok(()) => {
            println!("The baseline is the shape the variants need:");
            println!("  {} allocated lane group(s), pool in the box's canonical order.", groups.len());
            println!();
            println!("Next: --variant A (keys out of order), then B (a hole), then C (an");
            println!("extension detached from its parent). Each is a rehearsal until --send.");
        }
        Err(why) => {
            println!("NOT usable as a baseline: {why}");
            println!();
            println!("Re-do stage 1 on the box: clear {}, trigs on SYN1 steps 1 and 5,", slot_name(slot));
            println!("p-lock FLTR1 FREQ on step 1 and FLTR1 RESO on step 5, then SAVE.");
        }
    }
}

/// One allocated lane and its extension, as a block of lane indices.
///
/// The unit the variants move around: a lane and its `80 80` travel together
/// everywhere except in variant C, which is the one that pulls them apart on
/// purpose.
#[derive(Debug, Clone)]
struct Group {
    lane: usize,
    param_id: u8,
    track: u8,
    ext: Option<usize>,
}

fn groups_of(payload: &[u8]) -> Vec<Group> {
    read_all_plocks(payload)
        .unwrap_or_default()
        .iter()
        .map(|l: &A4Lane| Group { lane: l.lane, param_id: l.param_id, track: l.track, ext: l.ext_lane })
        .collect()
}

fn check_baseline(payload: &[u8], groups: &[Group]) -> Result<(), String> {
    let order = pool_order(payload).map_err(|e| e.to_string())?;
    if !order.is_canonical() {
        return Err(format!(
            "the pool is not in the box's own order ({order:?}) — a variant built from it \
             would carry two deviations, not one"
        ));
    }
    if groups.len() < 2 {
        return Err(format!(
            "{} allocated lane group(s); the variants need at least two to reorder",
            groups.len()
        ));
    }
    if !groups.iter().any(|g| g.ext.is_some()) {
        return Err("no lane has an extension, so variant C has nothing to detach — \
                    p-lock a fractional parameter (FLTR1 FREQ) as well"
            .into());
    }
    Ok(())
}

// --- stages 2-4: the variants ------------------------------------------------

fn lane_bytes(payload: &[u8], lane: usize) -> Vec<u8> {
    let o = POOL_BASE + lane * LANE_SIZE;
    payload[o..o + LANE_SIZE].to_vec()
}

fn free_lane_bytes() -> Vec<u8> {
    // The box's own free lane, measured 2026-09-01 while A16 was cleared:
    // `FF FF` and 64 **zero** bytes, not 64 `NO_VALUE`.
    let mut v = vec![0u8; LANE_SIZE];
    v[0] = FREE;
    v[1] = FREE;
    v
}

/// Rewrite the pool as this exact sequence of lane blocks, free lanes after.
fn lay_out(payload: &mut [u8], lanes: &[Vec<u8>]) {
    for lane in 0..NUM_LANES {
        let o = POOL_BASE + lane * LANE_SIZE;
        let block = lanes.get(lane).cloned().unwrap_or_else(free_lane_bytes);
        payload[o..o + LANE_SIZE].copy_from_slice(&block);
    }
}

/// Build one variant's pool from the baseline payload. Returns the new payload
/// and the sentence describing what single property it breaks.
fn build_variant(payload: &[u8], which: &str) -> Result<(Vec<u8>, String), String> {
    let groups = groups_of(payload);
    check_baseline(payload, &groups)?;
    let mut out = payload.to_vec();

    // Each group as its block(s), in the box's own order.
    let block = |g: &Group| -> Vec<Vec<u8>> {
        let mut v = vec![lane_bytes(payload, g.lane)];
        if let Some(e) = g.ext {
            v.push(lane_bytes(payload, e));
        }
        v
    };

    let (lanes, what): (Vec<Vec<u8>>, String) = match which.to_uppercase().as_str() {
        // A — keys out of order. Packed, extensions still adjacent to their
        // parents, only the (param_id, track) sequence broken.
        "A" => {
            let mut lanes = Vec::new();
            for g in groups.iter().rev() {
                lanes.extend(block(g));
            }
            let keys: Vec<String> =
                groups.iter().rev().map(|g| format!("{:#04x}/{}", g.param_id, g.track)).collect();
            (lanes, format!("keys out of order — the pool now reads {}", keys.join(" then ")))
        }
        // B — one hole. Sorted, extensions adjacent, a single free lane wedged
        // between the first group and the rest.
        "B" => {
            let mut lanes = block(&groups[0]);
            lanes.push(free_lane_bytes());
            for g in &groups[1..] {
                lanes.extend(block(g));
            }
            (lanes, "one free lane between two used ones — the pool has a hole".to_string())
        }
        // C — an extension separated from the lane it extends. The reader's own
        // adjacency assumption, which the box has never had occasion to test.
        "C" => {
            let with_ext = groups
                .iter()
                .position(|g| g.ext.is_some())
                .ok_or("no lane has an extension to detach")?;
            let mut lanes = Vec::new();
            let mut detached = None;
            for (i, g) in groups.iter().enumerate() {
                let mut b = block(g);
                if i == with_ext {
                    detached = Some(b.remove(1));
                }
                lanes.extend(b);
            }
            lanes.push(detached.expect("the group was chosen for having one"));
            (
                lanes,
                format!(
                    "the 80 80 extension of lane {:#04x}/{} moved to the end of the pool, \
                     away from the lane it extends",
                    groups[with_ext].param_id, groups[with_ext].track
                ),
            )
        }
        other => return Err(format!("no variant {other} — pick A, B or C")),
    };

    lay_out(&mut out, &lanes);
    Ok((out, what))
}

fn variant(
    conn: &mut impl A4Sink,
    inbox: &mut SysExInbox,
    slot: u8,
    which: &str,
    for_real: bool,
    save_dir: &Option<String>,
) {
    let label = slot_name(slot);
    println!("=== variant {} into {label} ===", which.to_uppercase());
    println!();

    // Rule: re-fetch. The base we edit and the backup are the same bytes, read
    // moments before the send — `safe_write::a4_safe_write_tracks`'s bargain.
    println!("Fetching {label} — this is both the baseline and the backup…");
    let Some(frame) = fetch(conn, inbox, DUMP_A4_PATTERN_REQUEST, slot) else {
        eprintln!("no reply — nothing sent.");
        return;
    };
    let original = match parse_pattern(&frame) {
        Ok(p) => p,
        Err(e) => return eprintln!("the reply is not a pattern: {e} — nothing sent."),
    };
    save(save_dir, &frame, &format!("backup-slot{slot:02}-before-{}", which.to_lowercase()));

    let (payload, what) = match build_variant(&original.payload, which) {
        Ok(v) => v,
        Err(e) => return eprintln!("cannot build variant {which}: {e}\nnothing sent."),
    };

    print_pool("before", &original.payload);
    print_pool("after ", &payload);
    let moved = diff(&original.payload, &payload);
    println!();
    println!("what this variant breaks: {what}");
    println!("{} byte(s) move, all inside the pool: {}", moved.len(), moved.iter().all(|&i| i >= POOL_BASE));

    let wire = match build_pattern(slot, &payload) {
        Ok(w) => w,
        Err(e) => return eprintln!("cannot encode: {e}\nnothing sent."),
    };

    if !for_real {
        println!();
        println!("rehearsal — nothing sent, and the port was only read from.");
        println!("re-run with --send to run the ceremony.");
        return;
    }

    println!();
    println!("This will OVERWRITE pattern {label}. The backup above is the recovery path.");
    print!("Type {CONSENT} to proceed: ");
    let _ = std::io::stdout().flush();
    let mut typed = String::new();
    if std::io::stdin().read_line(&mut typed).is_err() || typed.trim() != CONSENT {
        println!("not confirmed — nothing sent.");
        return;
    }

    let cancel = AtomicBool::new(false);
    let sent = send_pattern(
        conn,
        &wire,
        Pacing::din().resolve(CAN_PACE),
        Consent::given_for(slot),
        &cancel,
        |p| {
            if p.packets_sent == 1 || p.packets_sent % 16 == 0 || p.packets_sent == p.packets_total {
                print!("\r  {} / {} packets", p.packets_sent, p.packets_total);
                let _ = std::io::stdout().flush();
            }
        },
    );
    println!();
    if let Err(e) = sent {
        return eprintln!("send failed: {e:?}");
    }

    std::thread::sleep(SETTLE_AFTER_SEND);
    println!("Reading {label} back…");
    let Some(back) = fetch(conn, inbox, DUMP_A4_PATTERN_REQUEST, slot) else {
        eprintln!("no reply to the read-back — the write is UNVERIFIED.");
        return;
    };
    let reread = match parse_pattern(&back) {
        Ok(p) => p,
        Err(e) => return eprintln!("the read-back is not a pattern: {e}"),
    };
    save(save_dir, &back, &format!("readback-slot{slot:02}-{}", which.to_lowercase()));

    let d = diff(&payload, &reread.payload);
    println!();
    if d.is_empty() {
        println!("READ BACK BYTE-IDENTICAL — the box stored the scrambled pool verbatim.");
        println!("That is transfer-level acceptance and says nothing yet about parsing.");
    } else {
        println!("{} byte(s) differ from what was sent:", d.len());
        report_diff(&payload, &reread.payload, &d);
        println!();
        println!("  The box REWROTE the pool on the way in — which is itself the answer to");
        println!("  whether it cares about order. Compare the pools below.");
    }
    println!();
    print_pool("sent    ", &payload);
    print_pool("readback", &reread.payload);

    println!();
    println!("--- now the two observations that matter ---");
    println!();
    println!("1. Select {label} on the box (STOPPED, not playing), then run:");
    println!("     cargo run -p digi_roll_studio --example a4_plock_order_probe -- --working");
    println!("   A pool that comes back re-canonicalised means the box parsed every lane");
    println!("   and rebuilt it. A pool with a lane MISSING is the failure that matters.");
    println!();
    println!("2. On the box, hold SYN1 step 1 and step 5 in turn and read the screen.");
    println!("   Bytes surviving a round trip only prove the box stored them; the front");
    println!("   panel is the only witness that the locks are live.");
}

// --- the post-load observation -----------------------------------------------

fn working(
    conn: &mut impl A4Sink,
    inbox: &mut SysExInbox,
    against: Option<String>,
    save_dir: &Option<String>,
) {
    println!("=== the working buffer, read-only ===");
    println!();
    let Some(frame) = fetch(conn, inbox, DUMP_A4_PATTERN_WORKING_REQUEST, 0) else {
        eprintln!("no reply to the working-pattern request");
        return;
    };
    let p = match parse_working_pattern(&frame) {
        Ok(p) => p,
        Err(e) => return eprintln!("the reply is not a working pattern: {e}"),
    };
    save(save_dir, &frame, "working");
    summarise(&p.payload);
    println!();
    print_pool("working", &p.payload);

    let Some(path) = against else { return };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return eprintln!("could not read {path}: {e}"),
    };
    let other = match parse_pattern(&bytes).or_else(|_| parse_working_pattern(&bytes)) {
        Ok(o) => o,
        Err(e) => return eprintln!("{path} is not a pattern: {e}"),
    };
    println!();
    print_pool("saved  ", &other.payload);
    let d = diff(&other.payload, &p.payload);
    println!();
    if d.is_empty() {
        println!("The working buffer is byte-identical to {path}.");
    } else {
        println!("{} byte(s) differ from {path}:", d.len());
        report_diff(&other.payload, &p.payload, &d);
        let pool: Vec<usize> = d.iter().copied().filter(|&i| i >= POOL_BASE).collect();
        println!();
        println!("  {} of them are in the pool.", pool.len());
    }
}

// --- reporting ----------------------------------------------------------------

fn print_pool(tag: &str, payload: &[u8]) {
    let order = match pool_order(payload) {
        Ok(o) => o,
        Err(e) => return println!("{tag}: cannot read the pool: {e}"),
    };
    let lanes = read_all_plocks(payload).unwrap_or_default();
    println!(
        "{tag} pool: {} lane group(s)   packed={}  ext_adjacent={}  keys_sorted={}",
        lanes.len(),
        order.packed,
        order.extensions_adjacent,
        order.keys_sorted,
    );
    for l in &lanes {
        let held: Vec<String> = l
            .values
            .iter()
            .enumerate()
            .filter_map(|(step, v)| v.map(|v| {
                let fine = l.fine.as_ref().and_then(|f| f.get(step).copied().flatten());
                match fine {
                    Some(f) => format!("s{}={v}+{f}", step + 1),
                    None => format!("s{}={v}", step + 1),
                }
            }))
            .collect();
        println!(
            "    lane {:3}  param {:#04x}  track {}  {}{}",
            l.lane,
            l.param_id,
            l.track,
            l.ext_lane.map_or_else(String::new, |e| format!("ext@{e}  ")),
            held.join(" "),
        );
    }
}

fn summarise(payload: &[u8]) {
    for (track, name) in TRACK_NAMES.iter().enumerate().take(NUM_TRACKS) {
        let Ok(trigs) = read_track_trigs(payload, track) else { continue };
        if trigs.is_empty() {
            continue;
        }
        let steps: Vec<String> = trigs.iter().map(|t| t.step.to_string()).collect();
        println!("  {name:5} {:2} trig(s): {}", trigs.len(), steps.join(" "));
    }
}

fn diff(a: &[u8], b: &[u8]) -> Vec<usize> {
    (0..a.len().min(b.len())).filter(|&i| a[i] != b[i]).collect()
}

fn report_diff(a: &[u8], b: &[u8], d: &[usize]) {
    for &i in d.iter().take(DIFF_CAP) {
        let where_ = if i >= POOL_BASE {
            let lane = (i - POOL_BASE) / LANE_SIZE;
            let within = (i - POOL_BASE) % LANE_SIZE;
            match within {
                0 => format!("pool lane {lane} param_id"),
                1 => format!("pool lane {lane} track"),
                n => format!("pool lane {lane} step {}", n - 1),
            }
        } else {
            format!("offset {i}")
        };
        println!("  {:6}  {:02x} -> {:02x}   {where_}", i, a[i], b[i]);
    }
    if d.len() > DIFF_CAP {
        println!("  … and {} more", d.len() - DIFF_CAP);
    }
}

// --- wire ---------------------------------------------------------------------

fn find_port(fragment: &str) -> Option<String> {
    let outputs = list_outputs().expect("MIDI would not start");
    let needle = fragment.to_lowercase();
    match outputs.iter().find(|p| p.name.to_lowercase().contains(&needle)) {
        Some(p) => Some(p.name.clone()),
        None => {
            eprintln!("no output port matching {fragment:?}. Ports present:");
            for p in &outputs {
                eprintln!("  {}", p.name);
            }
            None
        }
    }
}

fn fetch(conn: &mut impl A4Sink, inbox: &mut SysExInbox, request: u8, index: u8) -> Option<Vec<u8>> {
    let _ = inbox.drain();
    conn.send_chunk(&build_dump_message(FAMILY_ANALOG_FOUR, request, index, &[])).ok()?;
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

fn save(dir: &Option<String>, frame: &[u8], tag: &str) {
    let Some(dir) = dir else { return };
    let path = format!("{dir}/a4-plock-order-{tag}.syx");
    match std::fs::write(&path, frame) {
        Ok(()) => println!("  saved {path}"),
        Err(e) => eprintln!("  NOT saved ({path}: {e})"),
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}
