// Read the p-lock pool out of one or more A4 pattern dumps and answer PLAN.md §10
// **open item 3**: does the box omit an extension lane whose fine bytes are all
// zero, or is FLTR1 RESO simply integer-valued?
//
//   cargo run -p digi_roll_studio --example a4_plock_extension_check -- <dump.syx>...
//
// Read-only, and no port is opened: the argument is a file. Accepts anything
// `a4_pattern::parse_pattern` accepts, so a `.syx` from `local/decode_mmon.py`
// or a committed fixture both work.
//
// # Why this question blocks the pool writer
//
// A gen-1 value is a `u8` in a lane plus an optional fine byte in an `80 80`
// extension lane after it. FLTR1 FREQ has allocated an extension in every
// capture; RESO never has. Two readings fit:
//
//   A. RESO is integer-valued, so it has no fine byte to store.
//   B. The box omits an extension whose fine bytes are all zero, and RESO's one
//      captured lock happened to be exactly integral.
//
// **The two are not equally supported, and the asymmetry is the point.** Across
// every fixture, FREQ has five distinct fine bytes and none is zero — which is
// what a fractional parameter looks like — while RESO has been locked exactly
// **once**, SYN1 step 1 coarse 100, captured four times because it was the
// *control* in those diffs. Under B, that one sample had to land on a fine byte
// of exactly zero: a 1-in-256 accident. Under A it needs no accident at all.
// `tests/a4.rs::the_reso_observation_rests_on_a_single_lock` pins that count.
//
// So one further RESO lock at a different value settles it, and this tool is what
// reads the answer off it.
//
// # The capture that settles it
//
//   1. Start from a cleared A16 so the diff has one variable. Dump it first if
//      you want the baseline — `a4_pattern.py diff` wants one.
//   2. Put **trigs on steps 1, 5, 9 and 13 of SYN1**. Every p-lock capture so far
//      carries a trig on the locked step, so a lock lives on a trig — a
//      prerequisite rather than a detail.
//   3. P-lock **RESO on those four steps** to four clearly different values.
//      Ordinary encoder turns, not fine-adjust: an ordinary turn is exactly what
//      produced FREQ's fractional fine bytes, so it is the gesture under test.
//   4. P-lock **FREQ on the same four steps** as a control. FREQ is known to
//      allocate an extension, so its lane says the capture is good — and because
//      every lock ever captured sits on step 1, a four-step FREQ lane also gives
//      the first measurement of an extension carrying a fine byte **per step**
//      rather than the geometry implying it.
//   5. Save the pattern, dump it from the front panel, decode it, run this.
//
// # Reading the result
//
//   * **RESO's lane has an extension** -> hypothesis B. The box omits an
//     all-zero extension, and an encoder emits one only when some fine byte is
//     non-zero.
//   * **RESO's lane has none, across four distinct values** -> hypothesis A.
//     RESO is integer-valued, and B would now need four 1-in-256 accidents.
//
// **Both outcomes give the same encoder rule** — emit an extension iff some fine
// byte is non-zero — which is worth knowing before the capture rather than after,
// because it means the *writer* is unblocked either way. What neither outcome
// settles is a third possibility: that some parameter *requires* an extension
// even when its fine bytes are all zero. Nothing in a dump can show a device
// requiring something, so that one is a write test, and it is the same shape as
// the compaction question one bullet down in PLAN.md §10.

use digi_protocol::a4_pattern::{parse_pattern, slot_name, TRACK_NAMES};
use digi_protocol::a4_plocks::{is_compacted, orphan_extension_count, read_all_plocks};

/// The two parameter ids this box's captures have mapped. Everything else prints
/// as its raw id rather than being guessed at.
const FLTR1_FREQ: u8 = 0x22;
const FLTR1_RESO: u8 = 0x23;

/// Wrap a bare SysEx body in `F0 … F7`, and leave an already-framed one alone.
///
/// `local/decode_mmon.py --write-bodies` writes the body *without* the framing
/// bytes, and that is the pipeline a fresh capture actually arrives through — so
/// requiring `F0 … F7` here meant wrapping every file by hand before reading it.
/// [`parse_pattern`] then rejects anything that is not a well-formed dump, so
/// being lenient about two bytes at the ends costs no strictness where it counts.
fn framed(raw: &[u8]) -> Vec<u8> {
    if raw.first() == Some(&0xf0) && raw.last() == Some(&0xf7) {
        return raw.to_vec();
    }
    let mut out = Vec::with_capacity(raw.len() + 2);
    out.push(0xf0);
    out.extend_from_slice(raw);
    out.push(0xf7);
    out
}

fn param_name(id: u8) -> String {
    match id {
        FLTR1_FREQ => "FLTR1 FREQ".to_string(),
        FLTR1_RESO => "FLTR1 RESO".to_string(),
        other => format!("param {other:#04x}"),
    }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
    if paths.is_empty() {
        eprintln!("usage: a4_plock_extension_check <dump.syx>...");
        eprintln!();
        eprintln!("the current evidence, for comparison:");
        eprintln!("  crates/protocol/tests/fixtures/analogfour-A16-plock-*.syx");
        std::process::exit(2);
    }

    let mut reso_samples: Vec<(String, u8, Vec<u8>, bool)> = Vec::new();

    for path in &paths {
        let raw = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: {e}");
                std::process::exit(2);
            }
        };
        let pattern = match parse_pattern(&framed(&raw)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{path}: {e}");
                std::process::exit(1);
            }
        };
        let lanes = read_all_plocks(&pattern.payload).unwrap();

        println!("\n{path}");
        println!("  slot {} ({})", pattern.slot, slot_name(pattern.slot));
        if lanes.is_empty() {
            println!("  no p-locks — nothing to say about extensions");
            continue;
        }
        // Both of these are observations rather than rules, and both are pinned
        // as such in tests/a4.rs. Printed because a capture that breaks either
        // one is more interesting than the question this tool was run to answer.
        println!(
            "  pool: {} allocated lane(s), compacted {}, {} orphan extension(s)",
            lanes.len(),
            is_compacted(&pattern.payload).unwrap(),
            orphan_extension_count(&pattern.payload).unwrap()
        );

        for lane in &lanes {
            let steps: Vec<usize> = (0..64).filter(|&s| lane.values[s].is_some()).collect();
            println!(
                "\n  lane {:>3}  {:<12} {:<5}  {} locked step(s), extension: {}",
                lane.lane,
                param_name(lane.param_id),
                TRACK_NAMES.get(lane.track as usize).copied().unwrap_or("?"),
                steps.len(),
                match lane.ext_lane {
                    Some(i) => format!("lane {i}"),
                    None => "NONE".to_string(),
                }
            );
            println!("           step  coarse  fine  word");
            for s in &steps {
                let coarse = lane.values[*s].unwrap();
                let fine = lane.fine.as_ref().and_then(|f| f[*s]);
                println!(
                    "           {:>4}  {:>6}  {:>4}  {}",
                    s + 1,
                    coarse,
                    fine.map_or_else(|| "--".to_string(), |v| v.to_string()),
                    // `word` reads an absent extension as fine = 0, which is the
                    // half of item 3 that is inference; the raw columns above are
                    // the measurement.
                    lane.word(*s).map_or_else(|| "--".to_string(), |w| format!("{w:#06x}"))
                );
            }

            if lane.param_id == FLTR1_RESO {
                let coarse: Vec<u8> = steps.iter().map(|s| lane.values[*s].unwrap()).collect();
                reso_samples.push((
                    path.clone(),
                    lane.track,
                    coarse,
                    lane.ext_lane.is_some(),
                ));
            }
        }
    }

    // --- The verdict ---------------------------------------------------------

    println!("\n{}", "-".repeat(72));
    if reso_samples.is_empty() {
        println!("No RESO lane in any of those dumps, so item 3 is untouched.");
        println!("The capture it needs is in this file's header comment.");
        return;
    }

    let distinct: std::collections::BTreeSet<(u8, Vec<u8>)> =
        reso_samples.iter().map(|(_, t, c, _)| (*t, c.clone())).collect();
    let with_ext = reso_samples.iter().filter(|(_, _, _, e)| *e).count();
    let total_values: usize = distinct.iter().map(|(_, c)| c.len()).sum();

    println!(
        "RESO: {} lane-instance(s), {} distinct lock(s), {} coarse value(s) in all,",
        reso_samples.len(),
        distinct.len(),
        total_values
    );
    println!("      {with_ext} of them carrying an extension lane.");

    if with_ext > 0 {
        println!("\nVERDICT: hypothesis B. A RESO lane HAS an extension, so RESO is not");
        println!("integer-valued and the box omits an extension whose fine bytes are all");
        println!("zero. An encoder emits one iff some fine byte is non-zero.");
        println!("PLAN.md §10 item 3 is answered; go and close it.");
    } else if total_values >= 4 {
        println!("\nVERDICT: hypothesis A. {total_values} RESO values and no extension anywhere,");
        println!("so RESO is integer-valued — B would need {total_values} separate 1-in-256");
        println!("accidents. An encoder emits an extension iff some fine byte is non-zero,");
        println!("which is the same rule either way. PLAN.md §10 item 3 is answered.");
    } else {
        println!("\nNOT YET DECISIVE. {total_values} distinct RESO coarse value(s); one of them");
        println!("being integral is a 1-in-256 accident rather than an implausible one.");
        println!("Four distinct values on one lane is one dump's work — see the header.");
    }
    println!("\nEither way, whether the box *requires* an extension it did not need is a");
    println!("write test, not a capture. No dump can show a device requiring something.");
}
