// Build the A4 **trig-model write experiment** — PLAN.md §10 item 2, the last
// question about the gen-1 pattern format that a capture could not answer.
//
// **Run on 2026-08-31 against A4 0195, and every prediction held**: steps 3, 5
// and 12 lit, the other 61 dark, and 3 and 12 shown as trigless trigs rather than
// merely lit. Item 2 is closed. This tool is kept because it is the experiment
// the finding rests on — the same reason `probe_drive_read.rs` is kept — and
// because it is what a future OS, or a gen-1 box that is not an A4, would be
// re-checked with.
//
//   cargo run -p digi_roll_studio --example a4_trig_probe -- \
//       crates/protocol/tests/fixtures/analogfour-A16-trigless-trk1-step1-2026-08-31.syx \
//       -o /tmp/a4-trig-probe.syx
//
// **This opens no port and sends nothing, ever.** It is arithmetic on a
// `Vec<u8>` and one file write. Sending is `a4_pattern_send`'s job, which
// already owns the consent ceremony, the DIN pacing and the reply listener —
// there is no reason for a second thing in this repo that can write to a box.
//
// # What the experiment is for
//
// The trig model says the box reads `byte1 & 0x03` alone, and that byte 0 bit 0
// is residue left by a note trig that has been cleared, which the box ignores.
// That model is the third one, the two before it were wrong in the same
// direction, and neither was refuted by any byte we had — what settled it was
// Neil looking at an unlit LED (DEVELOPMENT.md lesson 16).
//
// Every byte behind the model came from dumps the box **sent**. Nothing in it
// established that the box *reads* its own bytes the way it writes them, and the
// sharp half of the claim — a bit that is set and must be ignored — could not be
// checked from this side of the cable at all. So the experiment authors all four
// states onto one track and asks the front panel, which answered yes on all
// seven steps.
//
// # Running it
//
//   1. Dump A16 from the box's front panel and keep the capture. `a4_pattern_send`
//      keeps no backup — the app's own send path does, but this bare-wire tool
//      does not, so the capture is the only backup there is. (~~"the A4 answers
//      no dump request"~~ fell 2026-08-31, PLAN.md §10 — `0x64` could fetch the
//      backup now, and the app's Setup panel does exactly that.)
//   2. Build the probe with this tool. Read the table it prints.
//   3. Send it: `a4_pattern_send <probe.syx> --send`.
//   4. Put the box on A16, SYN1, and read the step LEDs. Every authored step is
//      on the first page, steps 1-16, but the whole-track check below needs all
//      four pages.
//
// The prediction is one line: **steps 3, 5 and 12 lit, everything else dark.**
// The table says what a disagreement on each step would mean, which is the part
// worth having in front of you before you look rather than after.
//
// **Read step 5 before the others.** It carries the note-trig shape hardware has
// already accepted, so a dark step 5 means the message never arrived. That check
// is load-bearing because a send that never arrives leaves A16 holding whatever
// it held before, and **what that is, is not known** — the slot has been written
// twice by `a4_pattern_send` and edited by hand on the box since. So "nothing
// happened" does not have a predictable appearance, and cannot be told from "the
// model is wrong" by looking at the predicted-dark steps.
//
// The other half of that guard is the whole-track count this tool prints: if the
// send landed, SYN1 holds exactly three live trigs and 61 dark steps, because the
// message overwrites all 12,974 bytes. A track showing 3, 5 and 12 lit **and
// nothing else anywhere in the 64** is a result. A track showing those three plus
// something on step 40 is a send that did not fully replace the pattern, and the
// run should be discarded rather than interpreted.

use digi_protocol::a4_pattern::{
    build_trig_probe, note_name, parse_pattern, slot_name, trig_offset, trig_state, NO_NOTE,
    NUM_STEPS, TRACK_NAMES,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Index-based rather than by value, so that naming the same path for both
    // -- which would overwrite the baseline -- is refused as a repeated
    // positional rather than silently swallowing the baseline argument.
    let mut out = None;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                out = args.get(i + 1).cloned();
                i += 2;
            }
            a if a.starts_with('-') => i += 1,
            _ => {
                positional.push(args[i].clone());
                i += 1;
            }
        }
    }
    let Some(path) = positional.first() else {
        eprintln!("usage: a4_trig_probe <baseline.syx> [-o probe.syx]");
        eprintln!();
        eprintln!("the intended baseline is the committed A16-trigless capture:");
        eprintln!(
            "  crates/protocol/tests/fixtures/analogfour-A16-trigless-trk1-step1-2026-08-31.syx"
        );
        std::process::exit(2);
    };

    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(2);
        }
    };
    // `parse_pattern` verifies family, opcode, checksum, count and payload
    // length. A baseline that does not verify is not evidence of anything, and a
    // probe built on one would predict the wrong bytes.
    let baseline = match parse_pattern(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    // The preconditions live in `build_trig_probe`, not here: which steps must
    // start bare is part of the experiment's design, and the design has one copy.
    let probe = match build_trig_probe(&baseline) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };

    println!("baseline {path}");
    println!("  slot {} ({}), payload verified", baseline.slot, slot_name(baseline.slot));
    println!(
        "\nprobe on {} of {} — {} bytes changed\n",
        TRACK_NAMES[probe.track],
        slot_name(probe.slot),
        (0..probe.payload.len()).filter(|&i| baseline.payload[i] != probe.payload[i]).count()
    );

    println!("  step  bytes    note  our reading  the box should");
    println!("  ----  -------  ----  -----------  --------------");
    for s in &probe.steps {
        let note = if s.note == NO_NOTE { "--".to_string() } else { note_name(s.note) };
        println!(
            "  {:>4}  {:02x} {:02x}    {:<4}  {:<11}  {}",
            s.step,
            s.bytes.0,
            s.bytes.1,
            note,
            format!("{:?}", s.state),
            if s.expect_lit { "LIGHT a trig" } else { "show nothing" }
        );
    }

    // Reading order matters, and it is not left-to-right. A dark step 1 is the
    // predicted result AND what a send that never arrived looks like, so the
    // control has to be read before anything is concluded from the rest.
    println!("\n  READ STEP 5 FIRST. It is the shape hardware has already accepted;");
    println!("  if it is dark the send did not land, and A16's previous contents are");
    println!("  not known well enough for any other step to mean anything.");
    println!("  Then check the whole 64: three lit and 61 dark, or discard the run.");

    println!("\n  if a step disagrees:");
    for s in &probe.steps {
        println!("    {:>2}  {}", s.step, s.falsifies);
    }

    // The steps the probe does not author are part of the claim too: a trig that
    // appears anywhere else on the track means the write landed at offsets this
    // tool did not intend, and that is worth being able to see at a glance.
    let lit: Vec<String> = (1..=NUM_STEPS)
        .filter(|step| {
            let o = trig_offset(probe.track, step - 1);
            trig_state(probe.payload[o], probe.payload[o + 1]).is_live()
        })
        .map(|s| s.to_string())
        .collect();
    println!(
        "\n  the whole track reads {} live trig(s): {}",
        lit.len(),
        if lit.is_empty() { "none".to_string() } else { lit.join(" ") }
    );
    // Counted from the payload, not from the table: step 7 is *in* the table
    // precisely because nothing was authored onto it, so counting rows would
    // overstate this by one.
    let authored: Vec<usize> = (1..=NUM_STEPS)
        .filter(|step| {
            let o = trig_offset(probe.track, step - 1);
            probe.payload[o] != baseline.payload[o] || probe.payload[o + 1] != baseline.payload[o + 1]
        })
        .collect();
    println!(
        "  {} steps authored ({}), {} in the table left exactly as the baseline had them",
        authored.len(),
        authored.iter().map(usize::to_string).collect::<Vec<_>>().join(" "),
        probe.steps.iter().filter(|s| !authored.contains(&s.step)).count()
    );

    let Some(out) = out else {
        println!("\nno -o given, so nothing was written. Add -o <file.syx> to emit the message.");
        return;
    };
    let msg = match probe.build() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("framing the probe: {e}");
            std::process::exit(1);
        }
    };
    // `build_pattern` emits the whole `F0 … F7` message, so there is nothing to
    // wrap. Every A4 pattern message is the same size — the payload length is
    // fixed at 12,974 — so the baseline's own length is a free check on that,
    // and it is here because the first version of this file wrapped the frame a
    // second time and produced a 14,845-byte file that no box would have read.
    if msg.len() != raw.len() {
        eprintln!(
            "built {} bytes where the baseline is {}: an A4 pattern message is a fixed size, \
             so this is a framing bug rather than a difference in content",
            msg.len(),
            raw.len()
        );
        std::process::exit(1);
    }
    // `a4_pattern_send` re-validates from the bytes on disk, which is the check
    // that matters — a file can be edited or half-written between here and there.
    if let Err(e) = std::fs::write(&out, &msg) {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    }
    println!("\nwrote {out} ({} bytes, F0 … F7)", msg.len());
    println!("\nBefore sending: dump {} from the front panel and keep it.", slot_name(probe.slot));
    println!("This overwrites that slot and nothing here keeps a backup.");
    println!("  cargo run -p digi_roll_studio --example a4_pattern_send -- {out} --send");
    println!("\nThen: box on {}, {}, and read the step LEDs.", slot_name(probe.slot), TRACK_NAMES[probe.track]);
    println!(
        "Expected lit: {}. Anything else lit or dark refutes the row above.",
        probe.expected_lit_steps().iter().map(usize::to_string).collect::<Vec<_>>().join(", ")
    );
}
