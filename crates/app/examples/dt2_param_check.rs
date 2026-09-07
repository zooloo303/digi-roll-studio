// Which of the DT2's published CC/NRPN numbers does the box actually answer?
//
// Every entry in `DT2_PARAMS` and `track_level_midi("DT2")` was **read out of
// Appendix B on 2026-08-24, and never played**. On 2026-09-07 the first one to
// be sent to hardware — track level, NRPN 1/100 — turned out to be ignored by
// the box, while its CC 95 worked (see `PLAN.md`, that date). The other eleven
// entries came from the same appendix by the same method, and the audition path
// prefers NRPN wherever a chart has one, so any of them could be silent in
// exactly the same way. This walks them.
//
// **Started life as `dt2_level_check`**, which asked about one parameter; the
// walk is the same ask, eleven more times, and two files holding one interaction
// is `DEVELOPMENT.md` lesson 5's shape.
//
// **What it does to the box.** Channel-voice messages only, through the same
// encoder the engine uses — no SysEx of any kind, so it cannot reach the store
// path. It does genuinely move the selected track's parameters and leaves each
// one at 64, the centre of the axis, exactly as turning the encoder by hand
// would. Reloading the pattern on the box discards the lot.
//
// **One half at a time, and each half sweeps until it is answered.** The person
// this is for is looking at the box, not at this terminal: a sweep on a timer is
// over before they look down, and two halves alternating on a timer cannot be
// told apart. So each half is announced, starts on a keypress, and keeps
// sweeping until a y/n comes back.
//
// **The CC is only asked about when the NRPN failed.** A working NRPN is the end
// of the question — it is what the app already sends. A dead one turns the CC
// into the decision: answered, the entry loses its NRPN the way track level did;
// dead too, and the entry is wrong in a way this probe cannot fix.
//
// Run with:
//   cargo run -p digi_roll_studio --example dt2_param_check -- <channel>
//   cargo run -p digi_roll_studio --example dt2_param_check -- <channel> <from>
//
// `channel` is 1-based and must match TRACK n CH on the box. `from` is an entry
// number to resume at, because twelve parameters is more than one standing-up.

use std::io::{stdin, stdout, BufRead, StdinLock, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{scope, sleep};
use std::time::Duration;

use digi_engine::event::MidiMsg;
use digi_midi::{list_outputs, open_output_by_name, MidiOutputConnection};
use digi_protocol::params::{track_level_midi, MidiMap, DT2_PARAMS, TRACK_LEVEL_LABEL};

const PORT: &str = "Elektron Digitakt II";

/// Every step of a sweep, and the gap between them — 32 at 45 ms is about a
/// second and a half, slow enough to watch and fast enough to repeat.
const SWEEP_STEPS: u16 = 32;
const SWEEP_GAP: Duration = Duration::from_millis(45);

/// Where each sweep leaves the parameter: the centre of the axis, which is 0 on
/// a bipolar one.
const CENTRE: u8 = 64;

/// One thing to ask about: what the box calls it, what the table calls it, and
/// the two numbers the appendix gives.
struct Entry {
    label: &'static str,
    name: &'static str,
    bipolar: bool,
    map: MidiMap,
}

/// What the box said about one entry.
enum Verdict {
    /// The NRPN moved it — the entry is right and the app is already sending it.
    Nrpn,
    /// The NRPN moved nothing and the CC moved it: track level's shape, and the
    /// entry should lose its NRPN.
    CcOnly,
    /// The appendix gives no NRPN, and the CC moved it. Nothing to decide.
    CcAlone,
    /// Neither half reached the parameter.
    Neither,
    /// Not asked.
    Skipped,
}

fn main() {
    let channel: u8 = std::env::args()
        .nth(1)
        .map(|a| a.parse().expect("channel must be a number 1-16"))
        .unwrap_or(1);
    let from: usize = std::env::args()
        .nth(2)
        .map(|a| a.parse().expect("from must be an entry number"))
        .unwrap_or(1);
    assert!((1..=16).contains(&channel), "channel must be 1-16");
    let wire_channel = channel - 1;

    match list_outputs() {
        Ok(ports) => {
            println!("outputs:");
            for p in &ports {
                println!("  {}", p.name);
            }
        }
        Err(e) => eprintln!("could not list outputs: {e}"),
    }

    let mut out = match open_output_by_name(PORT) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("could not open \"{PORT}\": {e}");
            eprintln!("is the box plugged in and its USB port named that?");
            return;
        }
    };

    // Track level last: it is the one entry whose answer is already known, and
    // it is here as the probe's own control — a run where CC 95 moves nothing
    // is a run that proves nothing about the eleven above it.
    let mut entries: Vec<Entry> = DT2_PARAMS
        .iter()
        .map(|p| Entry { label: p.label, name: p.name, bipolar: p.bipolar, map: p.midi })
        .collect();
    entries.push(Entry {
        label: TRACK_LEVEL_LABEL,
        name: "track.level (the control — CC 95 is known to work, NRPN 1/100 known dead)",
        bipolar: false,
        map: track_level_midi("DT2").expect("the DT2 has a level chart"),
    });

    println!();
    println!("DT2 parameter check — channel {channel} (must match TRACK {channel} CH on the box)");
    println!();
    println!("On the box: GLOBAL > MIDI CONFIG > PORT CONFIG > RECEIVE CC/NRPN = ON,");
    println!("then select TRACK {channel}. Each entry names the parameter to watch — put");
    println!("the page carrying it on screen before answering, because the DT2 pops no");
    println!("parameter name up when a CC or NRPN arrives.");
    println!();
    println!("Answers:  y it moved   n nothing   s or q leave this entry — it then asks");
    println!("          whether to carry on, and `n` there ends the run and reports.");
    println!("Each entry is left at {CENTRE}, the centre of its axis.");

    let stdin = stdin();
    let mut lines = stdin.lock().lines();
    let mut verdicts: Vec<(&Entry, Verdict)> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let n = i + 1;
        if n < from {
            continue;
        }
        println!();
        println!("=== {n}/{} — {} ({}) ===", entries.len(), entry.label, entry.name);
        println!(
            "    NRPN {:?}   CC {:?}{}{}",
            entry.map.nrpn,
            entry.map.cc,
            entry.map.cc_lsb.map(|l| format!(" (+LSB {l})")).unwrap_or_default(),
            if entry.bipolar { "   bipolar — the sweep runs the whole axis" } else { "" },
        );
        println!("    put {} on screen", entry.label);

        let verdict = match entry.map.nrpn {
            Some(_) => match ask(&mut out, &mut lines, wire_channel, entry, "nrpn") {
                Some(true) => Verdict::Nrpn,
                Some(false) => match entry.map.cc {
                    Some(_) => match ask(&mut out, &mut lines, wire_channel, entry, "cc") {
                        Some(true) => Verdict::CcOnly,
                        Some(false) => Verdict::Neither,
                        None => Verdict::Skipped,
                    },
                    None => Verdict::Neither,
                },
                None => Verdict::Skipped,
            },
            None => match ask(&mut out, &mut lines, wire_channel, entry, "cc") {
                Some(true) => Verdict::CcAlone,
                Some(false) => Verdict::Neither,
                None => Verdict::Skipped,
            },
        };
        centre(&mut out, wire_channel, entry);
        let stopped = matches!(verdict, Verdict::Skipped);
        verdicts.push((entry, verdict));
        if stopped && !ask_more(&mut lines) {
            break;
        }
    }

    report(&verdicts);
}

/// Ask about to keep going after a skip — `s` skips one entry, `q` stops.
fn ask_more(lines: &mut std::io::Lines<StdinLock<'static>>) -> bool {
    print!("  skipped — carry on to the next entry? [y/n] ");
    stdout().flush().ok();
    matches!(lines.next(), Some(Ok(l)) if !l.trim().eq_ignore_ascii_case("n"))
}

/// Sweep one half of one entry until somebody says whether it moved anything.
///
/// The sweep runs on a scoped thread so the answer can arrive mid-sweep, which
/// is the whole point: `Some(bool)` is an answer, `None` is a skip or a closed
/// stdin.
fn ask(
    out: &mut MidiOutputConnection,
    lines: &mut std::io::Lines<StdinLock<'static>>,
    channel: u8,
    entry: &Entry,
    half: &str,
) -> Option<bool> {
    let what = match half {
        "nrpn" => {
            let (msb, lsb) = entry.map.nrpn?;
            format!("NRPN {msb}/{lsb}")
        }
        _ => format!("CC {}", entry.map.cc?),
    };
    print!("  press Enter to sweep {what} (repeats until you answer)... ");
    stdout().flush().ok();
    match lines.next() {
        Some(Ok(line)) if line.trim().eq_ignore_ascii_case("s") => return None,
        Some(Ok(line)) if line.trim().eq_ignore_ascii_case("q") => return None,
        Some(Ok(_)) => {}
        _ => return None,
    }

    let stop = AtomicBool::new(false);
    scope(|s| {
        s.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                for i in 0..=SWEEP_STEPS {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    send(out, message(channel, entry, half, (i * 127 / SWEEP_STEPS) as u8));
                    sleep(SWEEP_GAP);
                }
            }
        });

        let answer = loop {
            print!("  sweeping {what} — is {} moving on the box? [y/n/s] ", entry.label);
            stdout().flush().ok();
            match lines.next() {
                Some(Ok(line)) => match line.trim().to_ascii_lowercase().as_str() {
                    "y" | "yes" => break Some(true),
                    "n" | "no" => break Some(false),
                    "s" | "skip" | "q" | "quit" => break None,
                    _ => continue,
                },
                _ => break None,
            }
        };
        stop.store(true, Ordering::Relaxed);
        answer
    })
}

/// One value of one half, as the engine's own encoder would build it.
fn message(channel: u8, entry: &Entry, half: &str, value: u8) -> MidiMsg {
    match half {
        "nrpn" => {
            let (msb, lsb) = entry.map.nrpn.expect("an NRPN");
            MidiMsg::Nrpn { channel, msb, lsb, value14: (value as u16) << 7 }
        }
        _ => MidiMsg::ControlChange {
            channel,
            controller: entry.map.cc.expect("a CC"),
            value,
        },
    }
}

/// Leave the parameter mid-axis, by whichever half the box has. On a
/// high-resolution CC the low half goes out as 0, or the box keeps the bottom
/// seven bits of wherever the sweep left it.
fn centre(out: &mut MidiOutputConnection, channel: u8, entry: &Entry) {
    let half = if entry.map.nrpn.is_some() { "nrpn" } else { "cc" };
    send(out, message(channel, entry, half, CENTRE));
    if let (Some(lsb), true) = (entry.map.cc_lsb, half == "cc") {
        send(out, MidiMsg::ControlChange { channel, controller: lsb, value: 0 });
    }
}

fn send(out: &mut MidiOutputConnection, msg: MidiMsg) {
    let mut bytes = Vec::new();
    msg.write_bytes(&mut bytes);
    out.send(&bytes).expect("send");
}

/// The run, and the `params.rs` edits it earns — printed rather than applied,
/// so "the box said so" and "somebody read the run" stay two events.
fn report(verdicts: &[(&Entry, Verdict)]) {
    println!();
    println!("=== what the box said ===");
    for (entry, verdict) in verdicts {
        let line = match verdict {
            Verdict::Nrpn => "NRPN works — entry is right, nothing to change".to_string(),
            Verdict::CcOnly => format!(
                "NRPN {:?} DEAD, CC {:?} works — drop the NRPN",
                entry.map.nrpn, entry.map.cc
            ),
            Verdict::CcAlone => "CC works (no NRPN in the appendix)".to_string(),
            Verdict::Neither => "NEITHER half moved it".to_string(),
            Verdict::Skipped => "skipped".to_string(),
        };
        println!("  {:<16} {line}", entry.label);
    }

    let dead: Vec<_> = verdicts
        .iter()
        .filter(|(_, v)| matches!(v, Verdict::CcOnly))
        .map(|(e, _)| e)
        .collect();
    if !dead.is_empty() {
        println!();
        println!("Edits this earns in `protocol/src/params.rs` — each one needs the comment");
        println!("that says the box was asked, and the date:");
        for entry in &dead {
            println!("  {}: nrpn: None  (was {:?})", entry.name, entry.map.nrpn);
        }
    }

    let missing: Vec<_> = verdicts
        .iter()
        .filter(|(_, v)| matches!(v, Verdict::Neither))
        .map(|(e, _)| e.label)
        .collect();
    if !missing.is_empty() {
        println!();
        println!("Neither half reached these: {}", missing.join(", "));
        println!("Before believing it of the box, check the page was the right one and the");
        println!("track was the one on this channel — a parameter nobody was looking at");
        println!("moves exactly like one the box ignored. TRACK LEVEL's CC is the control:");
        println!("if that moved, the channel and the receive setting are not the problem.");
    }
}
