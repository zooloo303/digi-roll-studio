// Measure the Analog Four's p-lock **scalings** — the last thing standing
// between a named lane and a draggable one.
//
// **Read-only.** The only messages sent are `0x6a`, the working-pattern request,
// through `assert_request_opcode`, and `0x01` Device for liveness. No slot is
// written and the box never has to save — the same bargain `a4_param_probe` and
// `a4_lane_probe` strike, for the same reason: a save is the one step in the
// loop that can lose somebody's work.
//
// # What is missing, and why nothing here can be derived
//
// 2026-09-01 measured **92 parameter ids** and stopped there deliberately.
// `Param::plock` asserts an id *and a scaling*, so naming a lane was allowed and
// curating one was not — `writable_params_for("A4")` was empty, every A4 lane
// was read-only, and `PLAN.md`'s "Named is not editable" is that split written
// down.
//
// **The first run of this example closed five of them** the same day: cutoff,
// resonance, env depth, overdrive and osc1 level. Everything below still applies
// to the other 87, and the run is repeatable — `--only <name>` takes one
// parameter at a time.
//
// The temptation this example exists to replace: `a4_plocks` measured, twice,
// that **the coarse byte is FLTR1 FREQ's displayed value** — so `Scaled(256)`
// looks correct for every unipolar parameter and one could simply write it in.
// That is one parameter's measurement generalised across a page, which is
// DEVELOPMENT.md lesson 16's shape exactly, and the same file already records
// the correction that makes it dangerous: **it is false for bipolar
// parameters**, where coarse 63 / fine 64 reads on the box as TUN 0.
//
// So the offset is per parameter, `A4_PARAMS`' `bipolar` flags come from
// Elektron's published appendix rather than from this box, and the appendix is
// exactly the kind of source lesson 16 says to check against a screen. This run
// checks it against the screen.
//
// # The protocol, per parameter
//
//   1. Put a trig on SYN1 step 1 (or wherever `--track`/`--step` say) and hold
//      it. Everything below happens with that trig held.
//   2. This names one knob and prints the id it *expects*.
//   3. Turn that knob fully **LEFT**. Type the number the box shows, Enter.
//   4. Turn it fully **RIGHT**. Type that number, Enter.
//   5. It reports which lane actually moved, and what the two points fit.
//
// Nothing is saved on the box, so `FUNC`+`NO` — or just clearing the trig —
// undoes the whole run.
//
//   cargo run -p digi_roll_studio --example a4_scale_probe
//   cargo run -p digi_roll_studio --example a4_scale_probe -- --only filter.cutoff
//   cargo run -p digi_roll_studio --example a4_scale_probe -- --only amp.pan,fx.delaySend
//   cargo run -p digi_roll_studio --example a4_scale_probe -- --save local/a4-check/scales
//
// # Why the endpoints, and why two of them
//
// **Two points fix a line, and the two end-stops are the only two values a hand
// can hit exactly.** A knob turned to "about 64" is a reading with an unstated
// error bar; a knob turned fully left is at its minimum whatever the minimum
// turns out to be. It also means the run reads the parameter's *range* off the
// box rather than off `A4_PARAMS`, so a `min`/`max` this app has wrong is a
// finding here instead of a silent clamp later.
//
// # What it can disprove, which is the point
//
// A probe that can only confirm is not worth running (lesson 18). Three
// distinct ways this run can come back negative:
//
// * **The id is wrong.** The expectation below is a join between two tables
//   made by matching *labels* — `A4_PARAMS`' "FLTR1 FREQ" against
//   `A4_SYNTH_PLOCKS`' "FLTR1 FRQ" — and a label match is not a measurement.
//   Every step here reports the lane that actually moved, so a mis-joined pair
//   is named on the line rather than baked into a table.
// * **The slope is not 1.** If the displayed range and the coarse range differ
//   in width, the box is not storing one display unit per coarse count, and
//   *nothing* in `PLockScaling` expresses that today. It is reported and not
//   fitted.
// * **The `bipolar` flag is wrong.** `fx.overdrive` is flagged bipolar in
//   `A4_PARAMS` from the appendix, and if the box shows it 0..127 this run says
//   so — a wrong flag would put a 64-unit offset on a parameter that has none,
//   which is a wrong byte on hardware and precisely what this is guarding.
//
// # What it deliberately does not do
//
// It does not write `A4_PARAMS`. It prints the line each parameter earns and
// stops there: a probe that edited the curated table would make "the box agreed"
// and "somebody checked the run" the same event, and the second is the one that
// matters.

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
const LIVENESS_WINDOW: Duration = Duration::from_secs(2);
const LIVENESS_TRIES: usize = 3;

/// One parameter this run can measure a scaling for.
///
/// **`id` and `bipolar` are the hypothesis, not the finding.** `id` is a join
/// between `A4_PARAMS` and `A4_SYNTH_PLOCKS` made by matching labels by eye, and
/// `bipolar` is Elektron's appendix. Both are printed as *expectations* and both
/// are checked against what the box does.
struct Target {
    /// The canonical key in `A4_PARAMS`, which is what a fitted line names.
    name: &'static str,
    /// How the box labels it, so a person can find the knob.
    page: &'static str,
    knob: &'static str,
    /// The id `A4_SYNTH_PLOCKS` says this knob is.
    expect_id: u8,
    /// What `A4_PARAMS` says, from the published appendix.
    expect_bipolar: bool,
}

/// **All thirteen parameters `A4_PARAMS` carries** — the set that curating makes
/// editable *and* auditionable, since each already has a measured CC or NRPN.
/// The other 81 measured ids stay named-and-read-only until they have a `Param`
/// entry of their own to hang a scaling on.
///
/// This held only eleven until 2026-09-01: `osc2.level` and `amp.volume` were
/// dropped building the list, so three runs reported themselves complete while
/// two parameters had never been prompted for. A list that is a *subset* of the
/// table it is measuring cannot say so on its own, which is why the count is
/// asserted against `A4_PARAMS` below rather than trusted.
const TARGETS: &[Target] = &[
    Target { name: "filter.cutoff", page: "FLTR1", knob: "FRQ", expect_id: 0x22, expect_bipolar: false },
    Target { name: "filter.resonance", page: "FLTR1", knob: "RES", expect_id: 0x23, expect_bipolar: false },
    // Flagged bipolar in `A4_PARAMS` and the one most likely to be wrong: the
    // appendix's OVERDRIVE and the box's FLTR1 OVR need not agree about zero.
    Target { name: "fx.overdrive", page: "FLTR1", knob: "OVR", expect_id: 0x24, expect_bipolar: true },
    Target { name: "filter.envDepth", page: "FLTR1", knob: "DEP", expect_id: 0x26, expect_bipolar: true },
    Target { name: "fx.chorusSend", page: "AMP", knob: "CHO", expect_id: 0x2d, expect_bipolar: false },
    Target { name: "fx.delaySend", page: "AMP", knob: "DEL", expect_id: 0x2e, expect_bipolar: false },
    Target { name: "fx.reverbSend", page: "AMP", knob: "REV", expect_id: 0x2f, expect_bipolar: false },
    Target { name: "amp.pan", page: "AMP", knob: "PAN", expect_id: 0x30, expect_bipolar: true },
    Target { name: "osc1.level", page: "OSC1", knob: "LEV", expect_id: 0x06, expect_bipolar: false },
    Target { name: "osc2.level", page: "OSC2", knob: "LEV", expect_id: 0x07, expect_bipolar: false },
    Target { name: "amp.volume", page: "AMP", knob: "VOL", expect_id: 0x31, expect_bipolar: false },
    Target { name: "lfo1.depth", page: "LFO1", knob: "DEP1", expect_id: 0x5c, expect_bipolar: true },
    Target { name: "lfo2.depth", page: "LFO2", knob: "DEP1", expect_id: 0x5e, expect_bipolar: true },
];

/// One end-stop: what the box showed, and what it stored.
#[derive(Clone, Copy)]
struct Point {
    display: f64,
    id: u8,
    coarse: u8,
    fine: u8,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port_fragment = flag(&args, "--port").unwrap_or_else(|| DEFAULT_PORT.to_string());
    let save_dir = flag(&args, "--save");
    let only = flag(&args, "--only");
    let track: usize = flag(&args, "--track").and_then(|v| v.parse().ok()).unwrap_or(0);
    let step: usize = flag(&args, "--step").and_then(|v| v.parse().ok()).unwrap_or(1);
    let step = step.saturating_sub(1);

    let request = DUMP_A4_PATTERN_WORKING_REQUEST;
    if let Err(e) = assert_request_opcode(request) {
        eprintln!("{request:#04x} refused before the wire: {e}");
        std::process::exit(2);
    }
    if track >= NUM_TRACKS {
        eprintln!("--track is zero-based and this box has {NUM_TRACKS}");
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

    let track_name = TRACK_NAMES.get(track).copied().unwrap_or("?");
    println!("=== A4 scale probe — {} — watching the working pattern ===", port.name);
    println!();
    println!("Nothing is saved on the box and no slot is written. Ctrl-C to stop.");
    println!();
    println!("Set up once, then leave it alone:");
    println!("  1. Select {track_name}.");
    println!("  2. Put a trig on step {}.", step + 1);
    println!("  3. HOLD that trig for the whole run — every turn below is a p-lock.");
    println!();

    // The baseline is not decoration: a run against a track with no trig held
    // measures nothing and looks identical to a run where the knob did nothing.
    let Some(frame) = fetch(&mut conn, &mut inbox, request) else {
        eprintln!("no reply to {request:#04x} — is the box awake?");
        std::process::exit(1);
    };
    let Ok(pattern) = parse_working_pattern(&frame) else {
        eprintln!("the reply did not parse as a working pattern");
        std::process::exit(1);
    };
    match read_track_trigs(&pattern.payload, track) {
        Ok(trigs) if trigs.iter().any(|t| t.step == step + 1) => {
            println!("{track_name} has a trig on step {} — good.", step + 1);
        }
        Ok(trigs) if trigs.is_empty() => {
            println!("NOTE: {track_name} has no trigs at all yet. Put one on step {} first.", step + 1);
        }
        Ok(trigs) => {
            let steps: Vec<String> = trigs.iter().map(|t| t.step.to_string()).collect();
            println!(
                "NOTE: {track_name} has trig(s) on {} but not on step {}. \
                 Use --step, or move the trig.",
                steps.join(" "),
                step + 1,
            );
        }
        Err(e) => println!("could not read {track_name}'s trigs: {e}"),
    }
    save(&save_dir, &frame, "baseline");
    println!();

    // `--only a,b,c` — a list, because the parameters that need re-running are
    // rarely adjacent in this table and six invocations is six chances to set
    // the trig up differently.
    let wanted: Option<Vec<&str>> =
        only.as_deref().map(|o| o.split(',').map(str::trim).filter(|s| !s.is_empty()).collect());
    // A target list shorter than the table it measures reports itself complete
    // while leaving parameters unprompted, which is exactly what happened on
    // 2026-09-01. Checked here rather than trusted.
    let table = digi_protocol::params::param_table_for("A4");
    for p in table {
        assert!(
            TARGETS.iter().any(|t| t.name == p.name),
            "A4_PARAMS carries {} and TARGETS does not — this run would silently skip it",
            p.name,
        );
    }

    let chosen: Vec<&Target> = TARGETS
        .iter()
        .filter(|t| wanted.as_ref().is_none_or(|w| w.contains(&t.name)))
        .collect();
    if chosen.is_empty() {
        eprintln!("--only matched no parameter. Known: ");
        for t in TARGETS {
            eprintln!("    {}", t.name);
        }
        std::process::exit(2);
    }

    let mut lines: Vec<String> = Vec::new();
    for (i, target) in chosen.iter().enumerate() {
        println!("--- {}/{}  {} {}  ({})", i + 1, chosen.len(), target.page, target.knob, target.name);
        println!(
            "    expecting param {:#04x}, {}",
            target.expect_id,
            if target.expect_bipolar { "bipolar (appendix)" } else { "unipolar (appendix)" },
        );

        let before = snapshot(&mut conn, &mut inbox, request, track, step);
        let Some(min) = endpoint(
            &mut conn, &mut inbox, request, track, step, &before, &save_dir,
            target.expect_id,
            &format!("{}-min", target.name),
            &format!("    Turn {} {} fully LEFT, then type what the box shows: ", target.page, target.knob),
        ) else {
            println!("    skipped\n");
            continue;
        };

        let after_min = snapshot(&mut conn, &mut inbox, request, track, step);
        let Some(max) = endpoint(
            &mut conn, &mut inbox, request, track, step, &after_min, &save_dir,
            target.expect_id,
            &format!("{}-max", target.name),
            &format!("    Turn {} {} fully RIGHT, then type what the box shows: ", target.page, target.knob),
        ) else {
            println!("    skipped\n");
            continue;
        };

        if let Some(line) = report(target, min, max) {
            lines.push(line);
        }
        println!();
        let _ = std::io::stdout().flush();
    }

    println!("=== lines earned, for params.rs — check them before pasting ===");
    if lines.is_empty() {
        println!("(none: nothing fitted, which is a finding and not a failed run)");
    }
    for line in &lines {
        println!("{line}");
    }
    if let Some(dir) = &save_dir {
        let path = format!("{dir}/a4-scales.txt");
        if let Err(e) = std::fs::write(&path, lines.join("\n")) {
            eprintln!("summary NOT saved ({path}: {e})");
        } else {
            println!("\nsaved to {path}");
        }
    }
}

/// Fit the two end-stops, or say which of the three ways it came back negative.
///
/// Returns the `params.rs` line this parameter earned, or `None` when nothing
/// was fitted — which is a result, not a failure.
fn report(target: &Target, min: Point, max: Point) -> Option<String> {
    if min.id != max.id {
        println!(
            "    MIXED: the two turns moved different lanes ({:#04x} then {:#04x}). \
             A knob was nudged between them, or the trig was let go. Re-run this one.",
            min.id, max.id,
        );
        return None;
    }
    let id = min.id;
    if id != target.expect_id {
        println!(
            "    ID MISMATCH: expected {:#04x} from the label join, the box moved {:#04x}. \
             The join is by eye between two tables and this is what disproves one.",
            target.expect_id, id,
        );
    }

    let d_display = max.display - min.display;
    let d_coarse = f64::from(max.coarse) - f64::from(min.coarse);
    println!(
        "    param {id:#04x}   min: shown {} = coarse {} fine {}   max: shown {} = coarse {} fine {}",
        min.display, min.coarse, min.fine, max.display, max.coarse, max.fine,
    );

    if d_coarse == 0.0 {
        println!("    NO FIT: the coarse byte did not move between the end-stops.");
        return None;
    }

    // **What has to hold is that the coarse byte spans its whole range, not that
    // the screen moves one unit per count.** This rejected any slope other than
    // 1 until 2026-09-01, and threw out both LFO depths for it — they show
    // -128..127 across a 128-value byte, so the screen carries two units per
    // count.
    //
    // That is the same category error as the offset below, caught one step
    // later: a DT2's LFO DEPTH shows -128..127 too, and it ships as plain
    // `scaled_plock(29, 256)`. `Param::describe` pins every curated parameter's
    // axis to MIDI_MIN..MIDI_MAX, so **the app's display value is the coarse
    // byte** and the screen's own resolution is the box's business. What would
    // genuinely break `scaled_plock` is a parameter whose coarse byte does *not*
    // reach 0..127, because then the app's axis would address words the box
    // cannot hold — and that is what is checked instead.
    if min.coarse != 0 || max.coarse != 127 {
        println!(
            "    NO FIT: coarse spans {}..={} rather than 0..=127, so this app's \
             0..127 axis would address words the box has no value for.",
            min.coarse, max.coarse,
        );
        return None;
    }
    let slope = d_display / d_coarse;
    if (slope - 1.0).abs() > 1e-9 {
        println!(
            "    note: the screen moves {slope} units per coarse count ({} across {}), so it \
             has finer resolution than one byte and this app addresses every other value — \
             exactly as it already does for the digis' LFO depths.",
            d_display, d_coarse,
        );
    }

    // **The offset is a screen convention, not a scaling**, and reading it as
    // one is the trap this branch was rewritten to avoid on 2026-09-01.
    //
    // The first version of this example concluded that a parameter showing
    // -64..63 needed a new `PLockScaling` variant. It does not. `Param::describe`
    // gives **every** curated parameter `min: MIDI_MIN, max: MIDI_MAX` — the
    // app's display axis is the raw 0..127 on both boxes — and every bipolar
    // parameter the digis carry (`filter.envDepth`, `amp.pan`, three LFO depths,
    // on both DT2 and DN2) is `scaled_plock(id, 256)` with no offset anywhere.
    // A DT2's ENV DEPTH shows -64..+63 on its own screen too. So the offset
    // measured here is the gap between the *box's label* and the byte, which
    // this app already declines to mirror on two boxes and must not start
    // mirroring on a third.
    //
    // What the offset is still good for is checking the `bipolar` flag, which
    // comes from Elektron's appendix rather than from this box.
    let offset = min.display - f64::from(min.coarse);
    let bipolar = offset != 0.0;
    if bipolar != target.expect_bipolar {
        println!(
            "    FLAG MISMATCH: A4_PARAMS says {}, the box says {} (offset {offset}). \
             The appendix is not this box — take the box.",
            if target.expect_bipolar { "bipolar" } else { "unipolar" },
            if bipolar { "bipolar" } else { "unipolar" },
        );
    } else if bipolar {
        println!("    bipolar confirmed: the screen reads {offset:+} at coarse 0, as the appendix says");
    }

    println!(
        "    FIT: coarse {}..={} — the app's axis is the coarse byte, which is the \
         same shape every curated parameter on every box already takes",
        min.coarse, max.coarse,
    );
    Some(format!(
        "        // {} {} — measured on the box: screen {}..={}, coarse {}..={}\n        \
         plock: Some(scaled_plock({id:#04x}, 256)),   // {}",
        target.page, target.knob, min.display, max.display, min.coarse, max.coarse, target.name,
    ))
}

/// Prompt, wait for the turn, and read back which lane moved.
///
/// The display value is asked for *before* the pool is re-read, so the number
/// typed is the one that was on the screen when the knob stopped — not one read
/// after a round trip during which anything could have been nudged.
#[allow(clippy::too_many_arguments)]
fn endpoint(
    conn: &mut impl A4Sink,
    inbox: &mut SysExInbox,
    request: u8,
    track: usize,
    step: usize,
    before: &[(u8, u8, u8)],
    save_dir: &Option<String>,
    expect_id: u8,
    tag: &str,
    prompt: &str,
) -> Option<Point> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return None;
    }
    let line = line.trim();
    if line.eq_ignore_ascii_case("skip") || line.eq_ignore_ascii_case("s") {
        return None;
    }
    let Some(display) = parse_display(line) else {
        println!(
            "    could not read {line:?} — skipping this one.\n\
             \x20   Accepted: a number (64, -64, +63), the box's pan form (L64, CEN, R63), \
             or OFF.\n\x20   Type \"skip\" to skip on purpose."
        );
        return None;
    };

    let frame = fetch(conn, inbox, request)?;
    save(save_dir, &frame, tag);
    let after = pool(&frame, track, step);
    let moved: Vec<(u8, u8, u8)> = after
        .iter()
        .copied()
        .filter(|(id, c, f)| !before.iter().any(|(bid, bc, bf)| bid == id && bc == c && bf == f))
        .collect();

    // **The lock is read where it is, not where it moved to**, and getting that
    // backwards cost `fx.chorusSend` a whole run on 2026-09-01.
    //
    // The first version took "which lane changed since the snapshot" as the
    // measurement. It fails at exactly the values this probe asks for: a knob
    // already sitting at the end-stop you are asking it to be turned to does not
    // move, so the pool does not change, so the probe concluded the trig was not
    // held and skipped a parameter that was correctly set the whole time. CHO
    // was still at OFF from the previous session.
    //
    // So the expected id is read directly, and the diff is kept for what it is
    // actually good for: naming a *disagreement*.
    if let Some(&(id, coarse, fine)) = after.iter().find(|(id, _, _)| *id == expect_id) {
        if moved.len() > 1 {
            let ids: Vec<String> = moved.iter().map(|(id, _, _)| format!("{id:#04x}")).collect();
            println!(
                "    note: {} lanes moved ({}) — reading {expect_id:#04x} as asked, but a \
                 second knob was touched and its own reading will be off.",
                moved.len(),
                ids.join(" "),
            );
        }
        return Some(Point { display, id, coarse, fine });
    }

    match moved.len() {
        0 => {
            println!(
                "    {expect_id:#04x} holds no lock on this step and nothing moved — \
                 was the trig held? Skipping this one."
            );
            None
        }
        // The expected id is absent and something else moved: the label join
        // between the two tables is wrong for this knob. That is a finding, and
        // the reading is still usable because the lane is named on the line.
        1 => {
            println!(
                "    {expect_id:#04x} holds no lock — {:#04x} is what moved. Reading that instead.",
                moved[0].0,
            );
            Some(Point { display, id: moved[0].0, coarse: moved[0].1, fine: moved[0].2 })
        }
        _ => {
            let ids: Vec<String> = moved.iter().map(|(id, _, _)| format!("{id:#04x}")).collect();
            println!(
                "    {expect_id:#04x} holds no lock and {} lanes moved ({}) — cannot attribute \
                 this. Skipping.",
                moved.len(),
                ids.join(" "),
            );
            None
        }
    }
}

/// What the box's screen says, as a number.
///
/// **Not every A4 parameter displays a decimal, and assuming they all did is
/// what silently dropped four parameters from the first run on 2026-09-01.**
/// `amp.pan` reads `L64` … `CEN` … `R63`; a probe that only called
/// `str::parse::<f64>` refused all of it and skipped the parameter, which looked
/// from the transcript like a decision somebody made.
///
/// The forms below are the box's, not this app's: `L`/`R` are the pan scale's
/// own letters and `CEN` is what it prints at zero. `OFF` appears at the bottom
/// of several send and depth ranges and is that range's minimum, which is what
/// an end-stop reading needs it to be.
fn parse_display(line: &str) -> Option<f64> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    // `+63` parses natively; `-64` too. This is only for the symbolic forms.
    if let Ok(v) = t.parse::<f64>() {
        return Some(v);
    }
    let upper = t.to_ascii_uppercase();
    match upper.as_str() {
        "CEN" | "CENTRE" | "CENTER" | "C" => return Some(0.0),
        // The minimum of whatever range it sits at the bottom of. The *value* is
        // read off the coarse byte either way; what this fixes is the parse.
        "OFF" => return Some(0.0),
        _ => {}
    }
    let (sign, rest) = match upper.split_at_checked(1)? {
        ("L", rest) => (-1.0, rest),
        ("R", rest) => (1.0, rest),
        _ => return None,
    };
    rest.parse::<f64>().ok().map(|v| sign * v)
}

fn snapshot(
    conn: &mut impl A4Sink,
    inbox: &mut SysExInbox,
    request: u8,
    track: usize,
    step: usize,
) -> Vec<(u8, u8, u8)> {
    fetch(conn, inbox, request).map(|f| pool(&f, track, step)).unwrap_or_default()
}

/// Every lock on one track's one step, as `(param_id, coarse, fine)`.
fn pool(frame: &[u8], track: usize, step: usize) -> Vec<(u8, u8, u8)> {
    let Ok(pattern) = parse_working_pattern(frame) else { return Vec::new() };
    read_all_plocks(&pattern.payload)
        .unwrap_or_default()
        .into_iter()
        .filter(|l| usize::from(l.track) == track)
        .filter_map(|l| {
            l.values.get(step).copied().flatten().map(|c| (l.param_id, c, fine_at(&l, step)))
        })
        .collect()
}

fn save(dir: &Option<String>, frame: &[u8], tag: &str) {
    let Some(dir) = dir else { return };
    let path = format!("{dir}/a4-scale-{tag}.syx");
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
/// same reading [`A4Lane::word`] takes.
fn fine_at(l: &A4Lane, step: usize) -> u8 {
    l.fine.as_ref().and_then(|f| f.get(step).copied().flatten()).unwrap_or(0)
}
