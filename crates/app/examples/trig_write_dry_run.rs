// Take a real pattern off a box, run the trig-condition **write** half over it
// in memory, and report exactly which bytes moved. **Nothing is sent.**
//
// Read-only, and the same safety class as `fetch_pattern_kit`: two API requests
// to identify, one 0x60 pattern-kit request, and then pure arithmetic on a
// `Vec<u8>` this process owns. No 0x70-family opcode is constructed anywhere,
// and nothing here calls `safe_write_track`, which is the only way to reach the
// store path `digi_midi` gained later the same day. The modified payload is
// dropped when the example exits.
//
// This exists because the write half (`protocol::trig_cond`, Phase 6,
// 2026-08-18) is complete, tested and reachable by nothing: no UI authors a
// write, and the safe-write function that will compose these appliers is
// several rungs away. That is the exact shape of thing DEVELOPMENT.md keeps having
// to confess three sessions later — so this is the dry run that lets a person
// see it work before any of it can touch a box.
//
// **What it proves that the fixtures cannot.** The committed condition captures
// are 8–16 trigs on one track, hand-set on a bench. A pattern off the box in
// front of you is whatever you have been working on: a full trig pool, p-lock
// lanes, several tracks, polymetric lengths. Minimal diff is one of the five
// write-safety rules, and this is the first time it is checked against a pattern
// nobody prepared. Every changed byte is printed with the field name
// `describe_offset` gives it, so an unexpected one is legible rather than a
// number — and anything landing outside the four legal regions is called out as
// a FAILURE, because that is the rule this example exists to test.
//
// The edit it makes is deliberately the awkward one: it puts a condition on the
// **first** trig of the busiest track and leaves every other trig alone. So the
// run exercises the scrub (every other step's lanes must come back to `FF`),
// the pool rewrite (every trig is re-encoded), and the "nothing else moves"
// rule at once.
//
// Run with:
//   cargo run -p digi_roll_studio --example trig_write_dry_run
//   cargo run -p digi_roll_studio --example trig_write_dry_run -- "Digitakt II=A01"

use std::collections::BTreeMap;

use digi_core::device::model_for_slug;
use digi_core::session::PatternRef;
use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::pattern::{
    decode_pattern_kit, describe_offset, diff_annotated_ranges, encode_track_notes, track_notes,
    Spec,
};
use digi_protocol::plocks::free_lane_count;
use digi_protocol::trig_cond::{
    apply_track_prob, apply_track_trig_settings, read_track_prob, read_track_trig_settings,
    trig_settings_from_notes, TrigSetting,
};

/// What to pull when nothing is named on the command line.
const DEFAULT_TARGETS: &[(&str, &str)] = &[("digitakt2", "A01"), ("digitone2", "A01")];

/// The lock this dry run puts on one trig, and the track PROB default it sets.
/// Both are arbitrary; what matters is that they are *distinguishable* from
/// anything a box would already be holding, so a wrong byte cannot look right.
const LOCK: TrigSetting = TrigSetting { prob: Some(35), fill: Some(true), cond: Some("2:4") };
const TRACK_PROB: u8 = 70;

fn main() {
    let overrides: Vec<(String, String)> = std::env::args()
        .skip(1)
        .filter_map(|a| a.split_once('=').map(|(p, s)| (p.to_string(), s.to_string())))
        .collect();

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    let mut attempted = 0usize;
    let mut clean = 0usize;

    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let slug = input.slug.expect("filtered");
        let label = overrides
            .iter()
            .find(|(frag, _)| input.name.contains(frag.as_str()))
            .map(|(_, s)| s.clone())
            .or_else(|| {
                DEFAULT_TARGETS.iter().find(|(s, _)| *s == slug).map(|(_, p)| p.to_string())
            });
        let Some(label) = label else {
            println!("\n{}: no pattern named for this box — skipped", input.name);
            continue;
        };
        let Some(index) = PatternRef::from_label(&label).and_then(|slot| slot.wire_index()) else {
            println!("\n{}: {label:?} is not a pattern label like A01 — skipped", input.name);
            continue;
        };
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
            println!("\n{}: no matching output port", input.name);
            continue;
        };

        attempted += 1;
        println!("\n=== {} — pattern {label} (index {index}) ===", input.name);

        let mut device =
            match ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output)) {
                Ok(d) => d,
                Err(e) => {
                    println!("  could not open: {e}");
                    continue;
                }
            };
        // Identify first: the fetch needs the family byte, and — the reason that
        // matters here — the pattern must be decoded with the spec of whatever
        // actually *answered*, not of whatever the port name suggests. Reading a
        // DN2 payload at DT2 offsets yields plausible nonsense rather than an
        // error, and a dry run that reported a plausible diff would be worse
        // than useless.
        let identity = match device.identify() {
            Ok(id) => id,
            Err(e) => {
                println!("  no identity: {e}");
                continue;
            }
        };
        println!("  {} — build {}", identity.name, identity.build);

        let Some(model) = model_for_slug(&identity.slug) else {
            println!("  unrecognised slug {:?} — not decoding", identity.slug);
            continue;
        };
        let Some(spec_fn) = model.sysex else {
            println!("  {} is live-play only — no SysEx dumps", model.display);
            continue;
        };
        let spec = spec_fn();

        let original = match device.fetch_pattern_kit(index) {
            Ok(p) => p,
            Err(e) => {
                println!("  fetch failed: {e}");
                continue;
            }
        };
        println!("  {} payload bytes, checksum and count OK", original.len());

        if dry_run(spec, &original, &identity.name) {
            clean += 1;
        }
    }

    println!("\n{clean}/{attempted} pattern(s) came back with a clean minimal diff.");
    if attempted == 0 {
        println!(
            "No box answered. This example needs hardware; the same rules are\n\
             pinned against committed fixtures by `cargo test -p digi_protocol`."
        );
    }
    println!(
        "\nNothing was sent to any box. This run constructed no write opcode and\n\
         the modified payload existed only in this process's memory."
    );
}

/// Everything after the fetch: encode, apply, diff, read back. Returns whether
/// the minimal-diff contract held.
fn dry_run(spec: &Spec, original: &[u8], box_name: &str) -> bool {
    let kit = match decode_pattern_kit(spec, original) {
        Ok(k) => k,
        Err(e) => {
            println!("  decode failed: {e}");
            return false;
        }
    };
    println!(
        "  pattern {:?}  swing-independent view: {} tracks, {} free p-lock lane(s)",
        kit.name,
        kit.tracks.len(),
        free_lane_count(spec, original),
    );

    // The busiest track, because it exercises the pool rewrite hardest. A
    // pattern with no trigs at all has nothing to say here.
    let Some(track) = (0..kit.tracks.len()).max_by_key(|&t| kit.tracks[t].trigs.len()) else {
        println!("  no tracks — nothing to do");
        return false;
    };
    let notes = track_notes(&kit, track);
    if notes.is_empty() {
        println!("  every track is empty — draw some trigs on the box and run this again");
        return false;
    }
    let before = read_track_trig_settings(spec, original, track).unwrap_or_default();
    println!(
        "  editing track {} — {} note(s), {} step(s) already carrying conditions on the box",
        track + 1,
        notes.len(),
        before.len(),
    );

    // The edit: lock the first trig, leave every other one bare. Pair each note
    // with its setting, which is the shape `trig_settings_from_notes` takes —
    // `pattern::Note` deliberately does not carry the trio (PLAN.md §7 rule 3).
    let first_step = notes.iter().map(|n| n.step).min().expect("non-empty");
    let paired: Vec<_> = notes
        .iter()
        .map(|n| (n.clone(), if n.step == first_step { LOCK } else { TrigSetting::default() }))
        .collect();

    let (mut payload, dropped) = match encode_track_notes(spec, original, track, &notes) {
        Ok(v) => v,
        Err(e) => {
            println!("  encode refused: {e}");
            return false;
        }
    };
    if dropped > 0 {
        println!("  note: {dropped} note(s) would not fit and were dropped by the encoder");
    }
    if let Err(e) =
        apply_track_trig_settings(spec, &mut payload, track, &trig_settings_from_notes(&paired))
    {
        println!("  apply refused: {e}");
        return false;
    }
    if let Err(e) = apply_track_prob(spec, &mut payload, track, Some(TRACK_PROB)) {
        println!("  PROB apply refused: {e}");
        return false;
    }

    // --- did the edit land? -------------------------------------------------
    let after = read_track_trig_settings(spec, &payload, track).unwrap_or_default();
    let landed = after.get(&first_step) == Some(&LOCK);
    println!(
        "  step {} reads back {}  ({})",
        first_step + 1,
        describe_setting(after.get(&first_step)),
        if landed { "as asked" } else { "WRONG — expected the lock we set" },
    );
    let stray: Vec<u8> = after.keys().copied().filter(|s| *s != first_step).collect();
    println!(
        "  every other step scrubbed to nothing: {}",
        if stray.is_empty() {
            "yes".to_string()
        } else {
            format!("NO — {} step(s) still carry settings: {stray:?}", stray.len())
        }
    );
    let prob = read_track_prob(spec, &payload, track).unwrap_or(0);
    println!("  track PROB default now {prob}% (asked for {TRACK_PROB}%)");

    // --- and did anything else move? ---------------------------------------
    // The four regions a track write is allowed to touch, per PLAN.md's
    // extended minimal-diff contract. Anything outside them is the failure this
    // example is for.
    let base = spec.pattern.tracks_offset + track * spec.track.size;
    let t = &spec.track;
    let legal = [
        ("step words", base, base + t.num_steps * 2),
        ("trig pool", spec.pattern.trig_pool, spec.pattern.p_locks_index),
        ("COND lane", base + t.trig_cond, base + t.trig_cond + t.num_steps),
        ("FILL lane", base + t.trig_fill, base + t.trig_fill + t.num_steps),
        ("PROB lane", base + t.trig_prob, base + t.trig_prob + t.num_steps),
        ("track PROB", base + t.track_prob, base + t.track_prob + 1),
    ];

    let ranges = diff_annotated_ranges(original, &payload, |o| describe_offset(spec, o));
    let mut per_region: BTreeMap<&str, usize> = BTreeMap::new();
    let mut outside: Vec<(usize, usize, String)> = Vec::new();
    for r in &ranges {
        let width = r.end - r.start + 1;
        match legal.iter().find(|&&(_, lo, hi)| r.start >= lo && r.end < hi) {
            Some((name, _, _)) => *per_region.entry(name).or_insert(0) += width,
            None => outside.push((r.start, width, r.label.clone())),
        }
    }
    println!("  bytes changed, by region:");
    for (name, bytes) in &per_region {
        println!("    {name:<12} {bytes:>6}");
    }

    let ok = outside.is_empty() && landed && stray.is_empty();
    if outside.is_empty() {
        println!("  minimal diff HELD — nothing outside those regions moved.");
    } else {
        let shown = outside.len().min(20);
        println!(
            "  MINIMAL DIFF FAILED — {} range(s) outside the legal regions:",
            outside.len()
        );
        for (start, width, label) in outside.iter().take(shown) {
            println!("    offset {start:>7}  {width:>4} byte(s)  {label}");
        }
        if outside.len() > shown {
            println!("    … and {} more", outside.len() - shown);
        }
        println!(
            "  That is a real finding: on {box_name}'s own bytes, this write reaches\n\
             further than the contract allows. Do not build the send path until it\n\
             is understood."
        );
    }

    // Writing twice must change nothing — the property that says the write is a
    // function of the notes rather than of what was already there.
    let twice = {
        let (mut p2, _) = encode_track_notes(spec, &payload, track, &notes).expect("re-encode");
        apply_track_trig_settings(spec, &mut p2, track, &trig_settings_from_notes(&paired))
            .expect("re-apply");
        apply_track_prob(spec, &mut p2, track, Some(TRACK_PROB)).expect("re-apply PROB");
        p2 == payload
    };
    println!(
        "  writing the same thing twice is byte-identical: {}",
        if twice { "yes" } else { "NO — the write is not idempotent" }
    );

    ok && twice
}

fn describe_setting(s: Option<&TrigSetting>) -> String {
    match s {
        None => "nothing".to_string(),
        Some(t) => format!(
            "PROB {} / FILL {} / COND {}",
            t.prob.map(|p| format!("{p}%")).unwrap_or_else(|| "—".into()),
            match t.fill {
                Some(true) => "ON",
                Some(false) => "OFF",
                None => "—",
            },
            t.cond.unwrap_or("—"),
        ),
    }
}
