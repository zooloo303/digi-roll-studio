// One capture of a Syntakt pattern for p-lock mapping — Neil's contributor
// plan, item 1 (SYNTAKT-CONTRIBUTOR-PLAN.md): "change one setting at a time;
// capture before, after and return-to-baseline".
//
// **Read-only, structurally.** It asks for the stored pattern with `0x61`
// through `fetch_dump`, whose guard admits only the 0x60–0x6e request range.
// Nothing is ever stored to the box. The live pattern's slot reflects edits
// made on the box without saving (measured 2026-09-10, stride-H01).
//
// Each run reads the slot twice (two reads that disagree are refused — an
// echo, or the box still settling), writes `<label>.bin` / `.syx` into the
// capture folder, prints every p-lock record with its raw values, and prints
// what changed since the previous capture in that folder.
//
//   cargo run -p digi_roll_studio --example syntakt_plock_capture -- --slot H01 --label 00-baseline
//   cargo run -p digi_roll_studio --example syntakt_plock_capture -- --slot H01 --label 01-reso-t1-s1-64
//
// `--dir` defaults to dumps/syntakt-<today>/plocks.

use std::path::PathBuf;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::syntakt_pattern::{
    looks_like_pattern, PLOCK_FREE, PLOCK_NO_VALUE, PLOCK_POOL, PLOCK_RECORDS, PLOCK_RECORD_BYTES,
};

const FAMILY: u8 = 0x16;
const PATTERN: u8 = 0x61;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

/// `A01`..`H16` → 0..127.
fn slot_index(slot: &str) -> Option<u8> {
    let mut chars = slot.trim().chars();
    let bank = chars.next()?.to_ascii_uppercase();
    let n: u8 = chars.as_str().parse().ok()?;
    if !('A'..='H').contains(&bank) || !(1..=16).contains(&n) {
        return None;
    }
    Some((bank as u8 - b'A') * 16 + n - 1)
}

/// `(record, param, track, [(step, raw)])` for every record in use.
fn records(p: &[u8]) -> Vec<(usize, u8, u8, Vec<(usize, u16)>)> {
    (0..PLOCK_RECORDS)
        .filter_map(|r| {
            let at = PLOCK_POOL + r * PLOCK_RECORD_BYTES;
            let b = p.get(at..at + PLOCK_RECORD_BYTES)?;
            if b[..2] == PLOCK_FREE {
                return None;
            }
            let values = (0..64)
                .filter_map(|s| {
                    let v = u16::from_be_bytes([b[2 + 2 * s], b[3 + 2 * s]]);
                    (v != PLOCK_NO_VALUE).then_some((s, v))
                })
                .collect();
            Some((r, b[0], b[1], values))
        })
        .collect()
}

fn show(values: &[(usize, u16)]) -> String {
    values
        .iter()
        .map(|(s, v)| format!("S{}={v:#06x} ({}{})", s + 1, v >> 8, if v & 0xff != 0 { format!("+{}/256", v & 0xff) } else { String::new() }))
        .collect::<Vec<_>>()
        .join("  ")
}

fn main() {
    let Some(index) = arg("--slot").and_then(|s| slot_index(&s)).or_else(|| arg("--index").and_then(|s| s.parse().ok())) else {
        println!("say which pattern the box is on: --slot H01 (or --index 112)");
        return;
    };
    let Some(label) = arg("--label") else {
        println!("give the capture a name: --label 01-reso-t1-s1-64");
        return;
    };
    let dir = PathBuf::from(arg("--dir").unwrap_or_else(|| {
        let today = std::process::Command::new("date").arg("+%Y-%m-%d").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        format!("dumps/syntakt-{today}/plocks")
    }));
    let fragment = arg("--port").unwrap_or_else(|| "Syntakt".to_string());

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let Some(input) = inputs.iter().find(|p| p.name.contains(&fragment)) else {
        println!("no input port matching {fragment:?}");
        return;
    };
    let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
        println!("{} has no matching output port", input.name);
        return;
    };
    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output)).expect("could not open the port");
    let id = device.identify().expect("no identity");
    if !id.name.to_ascii_lowercase().contains("syntakt") {
        println!("{} is not a Syntakt — stopping", id.name);
        return;
    }
    println!("{} · product {} · build {} · version {} · slot {index}", id.name, id.product_id, id.build, id.version);

    let first = device.fetch_dump(FAMILY, PATTERN, index).expect("first read failed");
    let second = device.fetch_dump(FAMILY, PATTERN, index).expect("second read failed");
    if first.payload != second.payload {
        println!("two reads disagree — nothing saved. Let the box settle (or turn MIDI thru off) and run again");
        return;
    }
    if !looks_like_pattern(&first.payload) {
        println!("the reply is not a pattern ({} bytes) — nothing saved", first.payload.len());
        return;
    }

    std::fs::create_dir_all(&dir).expect("could not make the capture folder");
    let previous = std::fs::read_to_string(dir.join("last.txt")).ok().map(|s| s.trim().to_string());
    let stem = dir.join(&label);
    std::fs::write(stem.with_extension("bin"), &first.payload).expect("could not write the payload");
    std::fs::write(stem.with_extension("syx"), &first.raw).expect("could not write the frame");
    std::fs::write(dir.join("last.txt"), &label).expect("could not write last.txt");
    let meta = format!("{label}\tslot {index}\tbuild {}\tversion {}\n", id.build, id.version);
    let log = dir.join("captures.tsv");
    let mut all = std::fs::read_to_string(&log).unwrap_or_default();
    all.push_str(&meta);
    std::fs::write(&log, all).expect("could not write captures.tsv");
    println!("saved {}", stem.with_extension("bin").display());

    let now = records(&first.payload);
    println!("\np-lock records in use: {}", now.len());
    for (r, param, track, values) in &now {
        println!("  rec {r:>2}  param {param:>3}  track {:>2}  {}", track + 1, show(values));
    }

    let Some(prev) = previous.filter(|p| *p != label) else {
        println!("\n(first capture in this folder — nothing to compare)");
        return;
    };
    let Ok(before) = std::fs::read(dir.join(&prev).with_extension("bin")) else { return };
    let changed: Vec<usize> = (0..first.payload.len().min(before.len())).filter(|&i| first.payload[i] != before[i]).collect();
    println!("\nsince {prev}: {} bytes changed", changed.len());
    let outside: Vec<usize> = changed.iter().copied().filter(|&i| !(PLOCK_POOL..PLOCK_POOL + PLOCK_RECORDS * PLOCK_RECORD_BYTES).contains(&i)).collect();
    if !outside.is_empty() {
        let shown: Vec<String> = outside.iter().take(24).map(|i| format!("{i}:{:02x}→{:02x}", before[*i], first.payload[*i])).collect();
        println!("  outside the pool ({}): {}", outside.len(), shown.join(" "));
    }
    let was = records(&before);
    for rec in &now {
        if !was.iter().any(|w| w.1 == rec.1 && w.2 == rec.2 && w.3 == rec.3) {
            println!("  NEW/CHANGED  param {:>3}  track {:>2}  {}", rec.1, rec.2 + 1, show(&rec.3));
        }
    }
    for rec in &was {
        if !now.iter().any(|n| n.1 == rec.1 && n.2 == rec.2) {
            println!("  GONE         param {:>3}  track {:>2}", rec.1, rec.2 + 1);
        }
    }
}
