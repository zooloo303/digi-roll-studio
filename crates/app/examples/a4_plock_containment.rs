// Write one A4 track's p-locks to the box through the real path, and prove the
// pool a write is *not* asked about comes back untouched.
//
// The containment test PLAN.md §10 asks for. Everything this app does not carry
// stays the destination's own — verified on hardware 2026-09-01, when writing
// A07 back moved 56 of 12,974 bytes and left the whole 8.4 KB pool alone. **The
// pool writer is the first thing that changes that**, so the claim has to be
// re-earned rather than inherited.
//
// # What is at stake, and why a unit test is not enough
//
// A gen-1 pool write rebuilds all 128 lanes, because the box normalises what it
// is sent and a non-canonical pool makes a correct write read back as a failed
// one (three writes to A16, 2026-09-01 — see `a4_plocks::apply_track_plocks`).
// Rebuilding means lanes belonging to the tracks this write never names change
// *index*. Their contents must not change at all, and "must not" is a property
// of code that `tests/all/a4.rs` already pins against fixtures.
//
// What a fixture cannot check is the other half: that the box agrees. A rebuilt
// pool is a shape the A4 has never been handed by this app before, and the only
// witness for "the destination kept its own" is the destination.
//
// # The path
//
// Nothing here is a second copy of the write. It drives exactly what the UI
// drives:
//
//   fetch  ->  a4_pattern_to_model  ->  a4_track_write  ->  a4_safe_write_tracks
//
// including the five-rule ceremony inside that last call — gate, re-fetch,
// confirm, backup, send, read back, byte-compare — over a real `ElektronDevice`.
// The gate is the real allowlist, so a box whose OS build is not write-verified
// refuses here exactly as it would in the app.
//
//   cargo run -p digi_roll_studio --example a4_plock_containment -- --slot 15 --track 0
//       Rehearsal. Runs the fetch and the whole composition, prints what would
//       move, and swallows the send.
//
//   cargo run -p digi_roll_studio --example a4_plock_containment -- --slot 15 --track 0 --write
//       The real thing, through the real stash.
//
// `--track` is the one being written; every *other* track's lanes are the ones
// under test. A16 is the scratch slot.

use std::collections::HashMap;
use std::io::Write as _;

use digi_core::a4_transfer::{a4_pattern_to_model, a4_track_write};
use digi_core::device::A4;
use digi_core::session::PatternRef;
use digi_midi::device::ElektronDevice;
use digi_midi::{list_inputs, list_outputs};
use digi_protocol::a4_pattern::{slot_name, A4Pattern, NUM_TRACKS, PAYLOAD_LEN};
use digi_protocol::a4_plocks::{pool_order, read_all_plocks, read_track_plocks, A4Lane};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::DeviceIdentity;
use digi_protocol::safe_write::{
    a4_safe_write_tracks, ConfirmArgs, PatternIo, Timestamp, WriteHooks,
};

const DEFAULT_PORT: &str = "Analog Four";
const CONSENT: &str = "OVERWRITE";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port_fragment = flag(&args, "--port").unwrap_or_else(|| DEFAULT_PORT.to_string());
    let slot: u8 = flag(&args, "--slot").and_then(|v| v.parse().ok()).unwrap_or(15);
    let track: usize = flag(&args, "--track").and_then(|v| v.parse().ok()).unwrap_or(0);
    let for_real = args.iter().any(|a| a == "--write");

    if track >= NUM_TRACKS {
        eprintln!("no track {}; an A4 pattern has {NUM_TRACKS}", track + 1);
        std::process::exit(2);
    }

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let needle = port_fragment.to_lowercase();
    let (Some(inp), Some(outp)) = (
        inputs.iter().find(|p| p.name.to_lowercase().contains(&needle)),
        outputs.iter().find(|p| p.name.to_lowercase().contains(&needle)),
    ) else {
        eprintln!("no port pair matching {port_fragment:?}");
        std::process::exit(1);
    };

    let mut device = match ElektronDevice::open(&inp.into(), &outp.into()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("could not open {}: {e}", inp.name);
            std::process::exit(1);
        }
    };
    match device.identify() {
        Ok(id) => println!("{} — OS {} (build {})\n", id.name, id.version, id.build),
        Err(e) => {
            eprintln!("identity handshake failed: {e}");
            std::process::exit(1);
        }
    }

    // The destination as it stands, read once for the report. The write does its
    // own re-fetch — this is not that fetch, and it must not be mistaken for it.
    let label = slot_name(slot);
    println!("Reading {label} to describe the write…");
    let before = match PatternIo::fetch_pattern_kit(&mut device, slot) {
        Ok(p) if p.len() == PAYLOAD_LEN => p,
        Ok(p) => {
            eprintln!("the box answered {} bytes; an A4 pattern is {PAYLOAD_LEN}", p.len());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("fetch failed: {e}");
            std::process::exit(1);
        }
    };
    // `fetch_pattern_kit` hands back the decoded payload, not the `F0 … F7`
    // frame, so the dump the model import wants is built rather than re-parsed.
    let dump = A4Pattern { slot, payload: before.clone() };

    print_pool("before", &before);

    // The real composition: the box's bytes into the model, one track back out.
    let (pattern, report) = match a4_pattern_to_model(&A4, &dump) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("import failed: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "\nimported: {} note(s), {} p-lock lane(s) ({} trigless)",
        report.notes, report.plock_lanes, report.trigless_plock_lanes
    );

    let into = PatternRef::from_slot(usize::from(slot));
    let export = match a4_track_write(&pattern, track, into) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("cannot describe the write: {e}");
            std::process::exit(1);
        }
    };
    let lanes = export.write.plocks.as_ref().map_or(0, Vec::len);
    println!(
        "writing track {} — {} trig(s), {} p-lock lane(s)",
        track + 1,
        export.write.steps.iter().filter(|s| s.is_some()).count(),
        lanes,
    );
    for w in &export.warnings {
        println!("  ! {w}");
    }

    // Which lanes are the ones under test: every lane the write did not name.
    let others: Vec<A4Lane> = read_all_plocks(&before)
        .unwrap_or_default()
        .into_iter()
        .filter(|l| usize::from(l.track) != track)
        .collect();
    println!(
        "\n{} lane(s) belong to tracks this write does not name — those are the ones under test.",
        others.len()
    );
    if others.is_empty() {
        println!("  With none, this run proves nothing about containment. Put a p-lock on");
        println!("  another track of {label} first.");
    }

    let stash = if for_real {
        match Stash::default_stash() {
            Ok(s) => s,
            Err(e) => {
                println!("Refusing to write: the backup store is unusable ({e}).");
                return;
            }
        }
    } else {
        let dir = std::env::temp_dir().join("digi-roll-studio-a4-containment");
        println!("\nRehearsal backups go to {} (not the real store).", dir.display());
        Stash::at(dir)
    };

    let mut hooks = Hooks { consented: false };
    let result = if for_real {
        a4_safe_write_tracks(
            &mut device,
            &stash,
            std::slice::from_ref(&export.write),
            &mut hooks,
            Timestamp::now(),
        )
    } else {
        let mut rehearsal = Rehearsal { inner: device, stored: HashMap::new() };
        a4_safe_write_tracks(
            &mut rehearsal,
            &stash,
            std::slice::from_ref(&export.write),
            &mut hooks,
            Timestamp::now(),
        )
    };

    let result = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("\nwrite refused: {e}");
            std::process::exit(1);
        }
    };
    if result.cancelled {
        println!("\nnot confirmed — nothing sent.");
        return;
    }

    println!("\n--- the write ---");
    println!("  {} trig(s) written", result.written);
    for w in &result.warnings {
        println!("  ! {w}");
    }
    if result.ok {
        println!("  read back BYTE-IDENTICAL to what was sent");
    } else {
        println!("  {} byte(s) differ from what was sent — the write is NOT verified", result.diffs.len());
        for d in result.diffs.iter().take(20) {
            println!("      {d:?}");
        }
    }

    let Some(after) = result.payload.as_ref() else { return };

    println!("\n--- containment ---");
    let moved: Vec<usize> = (0..PAYLOAD_LEN).filter(|&i| before[i] != after[i]).collect();
    println!("  {} of {PAYLOAD_LEN} bytes moved in total", moved.len());
    print_pool("after ", after);

    let after_others: Vec<A4Lane> = read_all_plocks(after)
        .unwrap_or_default()
        .into_iter()
        .filter(|l| usize::from(l.track) != track)
        .collect();

    let mut held = true;
    if after_others.len() != others.len() {
        println!("  LANE COUNT CHANGED: {} -> {}", others.len(), after_others.len());
        held = false;
    }
    for (b, a) in others.iter().zip(&after_others) {
        let moved_index = if a.lane == b.lane { String::new() } else { format!("  (lane {} -> {})", b.lane, a.lane) };
        let same = (a.param_id, a.track) == (b.param_id, b.track)
            && a.values == b.values
            && a.fine == b.fine;
        println!(
            "  {} param {:#04x} track {}{moved_index}",
            if same { "kept " } else { "CHANGED" },
            b.param_id,
            b.track,
        );
        held &= same;
    }
    println!();
    if others.is_empty() {
        println!("  inconclusive — no other track had lanes to preserve.");
    } else if held {
        println!("  CONTAINED: every lane belonging to a track this write did not name came");
        println!("  back with the same parameter, the same values and the same fine bytes.");
    } else {
        println!("  NOT CONTAINED — a lane this write was never asked about changed.");
        std::process::exit(1);
    }
}

fn print_pool(tag: &str, payload: &[u8]) {
    let Ok(order) = pool_order(payload) else { return };
    let lanes = read_all_plocks(payload).unwrap_or_default();
    println!(
        "{tag} pool: {} lane group(s)  packed={} ext_adjacent={} keys_sorted={}",
        lanes.len(),
        order.packed,
        order.extensions_adjacent,
        order.keys_sorted
    );
    for t in 0..NUM_TRACKS {
        for l in read_track_plocks(payload, t).unwrap_or_default() {
            let held: Vec<String> = l
                .values
                .iter()
                .enumerate()
                .filter_map(|(s, v)| v.map(|v| format!("s{}={v}", s + 1)))
                .collect();
            println!(
                "    lane {:3}  param {:#04x}  track {}  {}{}",
                l.lane,
                l.param_id,
                l.track,
                if l.ext_lane.is_some() { "ext  " } else { "" },
                held.join(" ")
            );
        }
    }
}

/// The write flow with the send swallowed — everything up to the wire runs, and
/// the read-back is served from what would have been stored, so a rehearsal
/// still exercises the byte-compare.
struct Rehearsal {
    inner: ElektronDevice,
    stored: HashMap<u8, Vec<u8>>,
}

impl PatternIo for Rehearsal {
    fn identity(&self) -> Option<&DeviceIdentity> {
        PatternIo::identity(&self.inner)
    }
    fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
        match self.stored.get(&index) {
            Some(p) => Ok(p.clone()),
            None => PatternIo::fetch_pattern_kit(&mut self.inner, index),
        }
    }
    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String> {
        println!("  [rehearsal] would send {} bytes to slot {index} — swallowed", payload.len());
        self.stored.insert(index, payload.to_vec());
        Ok(())
    }
}

struct Hooks {
    consented: bool,
}

impl WriteHooks for Hooks {
    fn confirm(&mut self, args: &ConfirmArgs) -> bool {
        let track = args.one();
        println!(
            "\n  About to replace track {} of {} — {} trig(s) there now, {} note(s) going in.",
            track.track_index + 1,
            args.label,
            track.existing_trigs,
            track.note_count,
        );
        println!("  The p-lock pool will be REBUILT: lanes belonging to other tracks keep");
        println!("  their contents and change index. This is the run that checks that.");
        print!("  Type {CONSENT} to proceed: ");
        let _ = std::io::stdout().flush();
        let mut typed = String::new();
        self.consented = std::io::stdin().read_line(&mut typed).is_ok() && typed.trim() == CONSENT;
        self.consented
    }
    fn on_status(&mut self, msg: &str) {
        println!("  {msg}");
    }
    fn on_log(&mut self, msg: &str) {
        println!("  {msg}");
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}
