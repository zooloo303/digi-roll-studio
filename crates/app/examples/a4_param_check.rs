// Check the Analog Four's curated parameter table against the box, one entry
// at a time, with a human watching the box's screen.
//
// `A4_PARAMS` and `track_level_midi("A4")` were written 2026-08-24 from two of
// Elektron's manuals, four days before the box existed on this desk. Nothing in
// them has ever moved a knob. This walks them.
//
// **Read-only in the sense that matters.** It sends channel-voice messages
// only — CC and NRPN, through `MidiMsg::write_bytes`, which is the same encoder
// the engine's audition path uses. It constructs no SysEx of any kind, so it
// cannot reach the store path at all.
//
// It sweeps rather than sets: a parameter that moves 0 -> 127 over a second and
// a half is visible on the box's screen from across a desk, where a single
// value lands as a number nobody saw change. Every sweep ends at 64 so the box
// is left mid-range rather than at a floor.
//
// NRPN and CC are sent as *separate* sweeps, because they are two claims. The
// audition path prefers NRPN (`app::plocks`) and every A4 entry has one, so a
// wrong CC number would never show up there — but the CC is in the table, and a
// table entry that has never been sent is exactly what this file exists to stop.
//
// Run with:  cargo run -p digi_roll_studio --example a4_param_check [channel]
//
// `channel` is 1-based and defaults to 1 (TRACK 1 on a factory A4).

use std::io::{stdin, stdout, BufRead, Write};
use std::thread::sleep;
use std::time::Duration;

use digi_engine::event::MidiMsg;
use digi_midi::{open_output_by_name, MidiOutputConnection};
use digi_protocol::params::{track_level_midi, MidiMap, A4_PARAMS, TRACK_LEVEL_LABEL};

const PORT: &str = "Elektron Analog Four";

/// Every step of a sweep, and the gap between them. 32 steps at 45 ms is about
/// a second and a half — slow enough to watch, fast enough that a person will
/// sit through thirteen of them twice.
const SWEEP_STEPS: u16 = 32;
const SWEEP_GAP: Duration = Duration::from_millis(45);

fn main() {
    let channel = std::env::args()
        .nth(1)
        .map(|a| a.parse::<u8>().expect("channel must be a number 1-16"))
        .unwrap_or(1);
    // `<channel> nrpn|cc <entry|L>` runs one half of one entry **on a loop**
    // until killed, instead of opening the prompt. A single 1.5-second sweep is
    // over before someone standing at the box has looked up; a loop lets the
    // person watching take their time, and lets the two halves be asked about
    // separately, which is the whole reason they are sent separately.
    let mode = std::env::args().nth(2);
    let which = std::env::args().nth(3);
    assert!((1..=16).contains(&channel), "channel must be 1-16");
    let wire_channel = channel - 1;

    let mut out = match open_output_by_name(PORT) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("could not open \"{PORT}\": {e}");
            eprintln!("is the box plugged in and its USB port named that?");
            return;
        }
    };

    println!("Analog Four parameter check — sending on MIDI channel {channel}");
    println!();
    println!("On the box, before starting:");
    println!("  GLOBAL > MIDI CONFIG > MIDI PORT CONFIG");
    println!("    INPUT FROM        = MIDI+USB  (or USB)");
    println!("    PARAM OUTPUT      = (irrelevant here, this is receive)");
    println!("    RECEIVE NOTES     = ON");
    println!("    RECEIVE CC/NRPN   = ON");
    println!("  Then select TRACK {channel} on the box and open the page the parameter lives on,");
    println!("  so the value is on screen while it moves.");
    if let (Some(mode), Some(which)) = (mode.as_deref(), which.as_deref()) {
        looping(&mut out, wire_channel, mode, which);
        return;
    }

    println!();
    list();
    println!();
    println!("Commands:  <n> sweep entry n   L track level   A all in order   ? list   Q quit");

    let stdin = stdin();
    let mut lines = stdin.lock().lines();
    loop {
        print!("a4> ");
        stdout().flush().ok();
        let Some(Ok(line)) = lines.next() else { break };
        match line.trim().to_ascii_lowercase().as_str() {
            "" => continue,
            "q" | "quit" => break,
            "?" | "l?" | "list" => list(),
            "l" | "level" => {
                let map = track_level_midi("A4").expect("the A4 has a level mapping");
                probe(&mut out, wire_channel, TRACK_LEVEL_LABEL, "track level (the mixer fader, not AMP VOL)", map);
            }
            "a" | "all" => {
                for (i, p) in A4_PARAMS.iter().enumerate() {
                    println!();
                    println!("--- {i}. {} ---", p.label);
                    probe(&mut out, wire_channel, p.label, p.name, p.midi);
                }
            }
            other => match other.parse::<usize>() {
                Ok(i) if i < A4_PARAMS.len() => {
                    let p = &A4_PARAMS[i];
                    probe(&mut out, wire_channel, p.label, p.name, p.midi);
                }
                _ => println!("no entry {other} — ? to list"),
            },
        }
    }
    println!("done — nothing was stored on the box.");
}

/// Sweep one half of one entry over and over until the process is killed.
///
/// Announces itself once and then goes quiet apart from a lap counter: the
/// person this is for is looking at the box, not at this terminal.
fn looping(out: &mut MidiOutputConnection, channel: u8, mode: &str, which: &str) {
    // A comma list is a **page pass**: sweep each entry once, in order, with a
    // gap between them. The box shows a whole parameter page at a time, so
    // somebody watching one page can report which bars moved and in what order
    // — which is four or five answers for one look, where a loop is one answer
    // for one look. The A4 pops no parameter name up on screen when a CC or
    // NRPN arrives (checked on the box, 2026-08-28), so being on the right page
    // is the whole of how anyone sees any of this.
    if which.contains(',') {
        let items: Vec<&str> = which.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        println!("page pass — {} entries, {mode} only, in this order:", items.len());
        for (n, item) in items.iter().enumerate() {
            match resolve(item) {
                Some((label, _)) => println!("  {}. {label}", n + 1),
                None => println!("  {}. ?? ({item})", n + 1),
            }
        }
        for (n, item) in items.iter().enumerate() {
            println!();
            println!("--- {} of {} ---", n + 1, items.len());
            one(out, channel, mode, item);
            sleep(Duration::from_millis(1200));
        }
        println!();
        println!("page pass done.");
        return;
    }
    one_looping(out, channel, mode, which);
}

/// One entry, one sweep, no loop — the page-pass unit.
fn one(out: &mut MidiOutputConnection, channel: u8, mode: &str, which: &str) {
    let Some((label, map)) = resolve(which) else {
        eprintln!("no entry {which}");
        return;
    };
    match (mode, map.nrpn, map.cc) {
        ("nrpn", Some((msb, lsb)), _) => {
            println!("{label} — NRPN {msb}/{lsb}");
            sweep(out, |v| MidiMsg::Nrpn { channel, msb, lsb, value14: (v as u16) << 7 });
        }
        ("nrpn", None, _) => println!("{label} — no NRPN in the table, skipped"),
        ("cc", _, Some(cc)) => {
            println!("{label} — CC {cc}");
            sweep(out, |v| MidiMsg::ControlChange { channel, controller: cc, value: v });
            if let Some(lsb) = map.cc_lsb {
                send(out, MidiMsg::ControlChange { channel, controller: lsb, value: 0 });
            }
        }
        ("cc", _, None) => println!("{label} — no CC in the table, skipped"),
        (other, _, _) => eprintln!("mode must be `nrpn` or `cc`, not `{other}`"),
    }
}

fn one_looping(out: &mut MidiOutputConnection, channel: u8, mode: &str, which: &str) {
    let (label, map) = match resolve(which) {
        Some(pair) => pair,
        None => {
            eprintln!("no entry {which} — pass an index 0-{}, or L", A4_PARAMS.len() - 1);
            return;
        }
    };
    match mode {
        "nrpn" => match map.nrpn {
            Some((msb, lsb)) => {
                println!("{label} — NRPN {msb}/{lsb}, sweeping 0 -> 127 on a loop. Ctrl-C to stop.");
                for lap in 1.. {
                    println!("  lap {lap}");
                    sweep(out, |v| MidiMsg::Nrpn { channel, msb, lsb, value14: (v as u16) << 7 });
                }
            }
            None => println!("{label} has no NRPN in the table — nothing to send"),
        },
        "cc" => match map.cc {
            Some(cc) => {
                println!("{label} — CC {cc}, sweeping 0 -> 127 on a loop. Ctrl-C to stop.");
                if let Some(lsb) = map.cc_lsb {
                    println!("  (14-bit pair; LSB {lsb} is held at 0, so this lands where the MSB alone would)");
                }
                for lap in 1.. {
                    println!("  lap {lap}");
                    sweep(out, |v| MidiMsg::ControlChange { channel, controller: cc, value: v });
                }
            }
            None => println!("{label} has no CC in the table — NRPN is the only way to hear it"),
        },
        other => eprintln!("mode must be `nrpn` or `cc`, not `{other}`"),
    }
}

/// An entry by index, or `L` for the track level that lives outside the table.
fn resolve(which: &str) -> Option<(&'static str, MidiMap)> {
    if which.eq_ignore_ascii_case("l") {
        return Some((TRACK_LEVEL_LABEL, track_level_midi("A4")?));
    }
    let p = A4_PARAMS.get(which.parse::<usize>().ok()?)?;
    Some((p.label, p.midi))
}

fn list() {
    println!("The thirteen curated entries:");
    for (i, p) in A4_PARAMS.iter().enumerate() {
        println!(
            "  {i:>2}. {:<18} {:<20} CC {:<12} NRPN {}",
            p.label,
            p.name,
            match (p.midi.cc, p.midi.cc_lsb) {
                (Some(cc), Some(lsb)) => format!("{cc}+{lsb}"),
                (Some(cc), None) => cc.to_string(),
                (None, _) => "-".into(),
            },
            match p.midi.nrpn {
                Some((msb, lsb)) => format!("{msb}/{lsb}"),
                None => "-".into(),
            },
        );
    }
    let map = track_level_midi("A4").expect("the A4 has a level mapping");
    println!(
        "   L. {:<18} {:<20} CC {:<12} NRPN {}",
        TRACK_LEVEL_LABEL,
        "(not a curated entry)",
        map.cc.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
        map.nrpn.map(|(m, l)| format!("{m}/{l}")).unwrap_or_else(|| "-".into()),
    );
}

/// Sweep one parameter twice — once as NRPN, once as CC — announcing each.
fn probe(out: &mut MidiOutputConnection, channel: u8, label: &str, name: &str, map: MidiMap) {
    println!("{label}  ({name})");
    match map.nrpn {
        Some((msb, lsb)) => {
            println!("  NRPN {msb}/{lsb} sweeping 0 -> 127 …");
            sweep(out, |v| MidiMsg::Nrpn { channel, msb, lsb, value14: (v as u16) << 7 });
            println!("  did {label} move on the box?");
        }
        None => println!("  NRPN: none in the table"),
    }
    sleep(Duration::from_millis(400));
    match map.cc {
        Some(cc) => {
            // The 14-bit pairs get their LSB sent as zero, which puts the
            // parameter exactly where the MSB alone would. Sending the MSB
            // without the LSB is the thing the table warns about, so it is also
            // the thing worth watching once.
            match map.cc_lsb {
                Some(lsb) => println!("  CC {cc} (MSB) with LSB {lsb} at 0, sweeping 0 -> 127 …"),
                None => println!("  CC {cc} sweeping 0 -> 127 …"),
            }
            sweep(out, |v| MidiMsg::ControlChange { channel, controller: cc, value: v });
            if let Some(lsb) = map.cc_lsb {
                send(out, MidiMsg::ControlChange { channel, controller: lsb, value: 0 });
            }
            println!("  did it move the same way?");
        }
        None => println!("  CC: none in the table (NRPN is the only way to hear this one)"),
    }
}

fn sweep(out: &mut MidiOutputConnection, msg: impl Fn(u8) -> MidiMsg) {
    for i in 0..=SWEEP_STEPS {
        let v = (i * 127 / SWEEP_STEPS) as u8;
        send(out, msg(v));
        sleep(SWEEP_GAP);
    }
    // Leave it mid-range rather than at the ceiling: 64 is the centre for the
    // bipolar entries and a sane place for the rest.
    send(out, msg(64));
}

fn send(out: &mut MidiOutputConnection, msg: MidiMsg) {
    let mut bytes = Vec::with_capacity(16);
    msg.write_bytes(&mut bytes);
    if let Err(e) = out.send(&bytes) {
        eprintln!("  send failed: {e}");
    }
}
