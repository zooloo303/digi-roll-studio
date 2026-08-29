// Find out whether `0x5b` stores one sound onto one track of a box's active
// kit — PLAN.md §10.6 step 3, and the question §10.4 says to settle before the
// kit builder's load path is designed rather than after.
//
// **Answered, positively, on both digis: DT2 0071 on 2026-08-28 and DN2 0050 on
// 2026-08-29.** Both took the *wrapped* payload on the first try, both were
// confirmed by two agreeing reads and by the box's own screen, and both
// restored. The hypothesis below is left standing as written, because it is the
// reasoning that earned the probe and it reads as a record rather than a claim —
// but where it says no box has ever been sent an `0x5b`, two now have.
//
// The file is kept because the question it settles is per-box: a fourth
// Elektron on this desk gets this pointed at it before anything is designed
// around its load path.
//
// # The hypothesis, and where it comes from
//
// `0x6b` returns the *active* kit's per-track sound for index 0-15: a 5-byte
// wrapper then one whole sound struct, hardware-verified on a DT2 and a DN2 on
// 2026-08-26 against Overbridge's own KIT TRACK PRESETS pane, all sixteen in
// order. Every store in the dump namespace is its request minus 0x10 — that is
// exactly how `store_pattern_kit` sends an `0x50` against request `0x60` — so
// **`0x5b` with a track index is the shape a per-track sound store would take.**
//
// That is arithmetic on one working opcode, not a document and not a capture.
// No box has ever been sent an `0x5b`. It may not be a store; it may store
// something other than a track's sound. This example is the check.
//
// # What rides on the answer
//
// §10.4: the only store this codebase has is `store_pattern_kit`, an `0x50`
// carrying the whole pattern *and* kit through the full `safe_write_tracks`
// ceremony. Wired to a double-click in a preset browser, that is one backup per
// audition against a fifty-entry ring — twenty auditions evict twenty real
// backups. If `0x5b` works, a double-click becomes one ~1 KB message instead of
// a 127 KB pattern write and that problem largely dissolves. A negative costs an
// afternoon; a positive changes the design. Hence: probe first.
//
// # Why a rename, and not a swap
//
// The obvious probe — send track N's own bytes straight back — is safe and
// proves nothing: "the box accepted it" and "the box ignored it entirely" both
// read as *nothing changed*. The store returns no reply, so silence is the only
// thing on the wire either way.
//
// So the probe changes exactly one field: the sound's **name**, 16 bytes at
// struct +12. That makes the result observable in two independent places — the
// bytes a re-read returns, and the name on the box's own screen — while every
// machine and parameter byte travels back verbatim. It is `sound.rs`'s
// minimal-diff discipline pointed at a probe: the bytes we cannot explain are
// the bytes we must not rewrite, and that argument does not weaken because this
// is an experiment.
//
// # What makes this recoverable
//
// The active kit is a **working buffer**, so rule 1's ceremony does not fit it:
// there is no `0x50` that puts a working buffer back, and a backup this program
// takes is not what saves you. Three things are what save you, in order:
//
// 1. **The box discards an unsaved kit when the pattern is reloaded.** Press a
//    pattern button — or switch away and back — and the stored kit returns.
//    That is a property of the hardware and it is the real undo here.
// 2. **The original bytes are written to a file before anything is sent**, so
//    an interrupted run leaves a restore on disk rather than in a dead process.
// 3. **The probe restores the original name itself** and re-reads to confirm.
//
// **Do not save the project while this is running.** That is the one action
// that turns a working-buffer edit into a stored one, and no code here can stop
// it.
//
// # The guards, which are not this file
//
// `ElektronDevice::store_kit_track_sound` refuses an OS build outside the write
// allowlist, a track outside 0-15, and a payload that is not a decodable sound
// struct — see `plan_track_sound_store`, which holds all three and the reasons
// for them. This example passes it a track and bytes; it cannot pass it an
// opcode, because the opcode is a constant there rather than a parameter.
//
// The **A4 cannot be probed at all** and that is not a limitation of this file:
// it answers no `0x6x` dump request whatsoever (product 4, OS 1.55B, confirmed
// 2026-08-28), so it has no `0x6b` to derive an `0x5b` from and `Product.family`
// is `None`. It is skipped with a reason rather than silently.
//
// # `--write` names one box, and will not run without it
//
// The read pass sweeps every box that answers, which is what a survey should
// do. Write mode must not: this walks a list of *all* connected boxes, so the
// same flag that probes the one box somebody prepared a throwaway project on
// would go on to probe the one next to it that they did not. Rule 1's
// throwaway-projects-only is a fact about a particular box at a particular
// moment, and a flag that cannot express which box cannot honour it.
//
// So `--write` requires `--box`, and a fragment matching two boxes is refused
// rather than resolved to the first.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_sound_store            # read-only
//   cargo run -p digi_roll_studio --example probe_sound_store -- --write --box Digitakt
//   cargo run -p digi_roll_studio --example probe_sound_store -- --write --box Digitakt --hold
//   cargo run -p digi_roll_studio --example probe_sound_store -- --write --box Digitakt --track 15
//
// `--hold [seconds]` keeps the probe name on the box before restoring, which is
// the only way the second witness this file argues for can actually be
// consulted: without it the restore lands in under a second and nobody ever sees
// the screen it named as the evidence. The first run of this probe had exactly
// that gap.
//
// **A timed wait rather than a keypress**, deliberately. This example is
// normally launched through a pipe with no terminal on stdin, where a
// `read_line` returns end-of-file immediately and the hold silently does not
// happen — a pause that skips itself is worse than no pause, because the run
// still prints as though somebody looked. A clock does the same job and cannot
// be fooled by the absence of a keyboard. Same lesson as `DEVELOPMENT.md`
// lesson 8's cliclick note: a wait that nothing drives is not a wait.

use std::time::Duration;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding, KIT_TRACKS};
use digi_protocol::safe_write::write_gate;
use digi_protocol::sound::{decode_sound_dump, Sound, SoundError, SOUND_NAME_OFFSET, SOUND_WRAPPER};

/// The name written into the probed track. Sixteen bytes is the field, and this
/// is deliberately unmistakable: if it ever turns up on a box that nobody was
/// probing, this file is where it came from.
const PROBE_NAME: &[u8] = b"PROBE5B";

/// Which track to probe by default. The last one, because a kit builder's own
/// worst case is the track someone is listening to, and T16 is the likeliest to
/// be idle on a box sitting on a desk.
const DEFAULT_TRACK: u8 = KIT_TRACKS - 1;

/// How long to let the box settle between a store and the read that checks it.
/// `paced_send` already waits its own `SEND_SETTLE`; this is on top, because a
/// read that races the store would report a false negative — the one error this
/// probe must not make, since a false negative is what parks the design.
const AFTER_STORE: Duration = Duration::from_millis(400);

/// How long `--hold` keeps the probe name on the box when given no number.
/// Long enough to walk to a box and page to a track's sound.
const DEFAULT_HOLD: u64 = 45;

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let writing = argv.iter().any(|a| a == "--write");
    // `--hold` alone is the default window; `--hold 60` is a longer one. The
    // default is generous because the cost of it being too short is walking back
    // to the keyboard, and the cost of it being too long is waiting.
    let hold = argv.iter().position(|a| a == "--hold").map(|at| {
        argv.get(at + 1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(DEFAULT_HOLD)
    });
    let target = argv.windows(2).find(|w| w[0] == "--box").map(|w| w[1].clone());
    let track: u8 = argv
        .windows(2)
        .find(|w| w[0] == "--track")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(DEFAULT_TRACK);

    let target = match (writing, target) {
        (true, None) => {
            eprintln!(
                "--write needs --box <name fragment>, so that exactly one box is probed.\n\
                 Run without --write first to see which boxes are connected."
            );
            std::process::exit(2);
        }
        (_, t) => t,
    };

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    // Resolved before any port is opened and before the banner, so that a
    // refusal reads as a refusal. An ambiguous fragment is an exit rather than a
    // choice: "matched the first of two" is precisely the mistake that would
    // write to the box nobody prepared.
    let selected: Vec<&str> = inputs
        .iter()
        .filter(|p| p.slug.is_some())
        .map(|p| p.name.as_str())
        .filter(|name| target.as_deref().is_none_or(|t| name.contains(t)))
        .collect();
    if writing && selected.len() != 1 {
        eprintln!(
            "--box {:?} matches {} connected boxes {selected:?}; it must match exactly one.",
            target.unwrap_or_default(),
            selected.len()
        );
        std::process::exit(2);
    }

    if writing {
        println!(
            "WRITE MODE — {}, track {} of the ACTIVE kit, renamed to {:?} and then renamed \
             back.\n\
             Recovery if this run dies: reload the pattern on the box (an unsaved kit is \
             discarded).\n\
             Do NOT save the project while this runs.\n",
            selected[0],
            track + 1,
            String::from_utf8_lossy(PROBE_NAME),
        );
    } else {
        println!("READ-ONLY — no 0x5b is sent. Pass --write --box <name> to probe the store.\n");
    }

    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
            continue;
        };
        let mut device =
            match ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output)) {
                Ok(d) => d,
                Err(e) => {
                    println!("\n{}: could not open: {e}", input.name);
                    continue;
                }
            };
        let identity = match device.identify() {
            Ok(id) => id,
            Err(e) => {
                println!("\n{}: no identity: {e}", input.name);
                continue;
            }
        };
        println!("\n=== {} — build {} ===", identity.name, identity.build);

        // Skipped with its reason rather than silently: "no dump family" is the
        // A4's actual testimony about itself and is worth printing every run.
        if identity.family.is_none() {
            println!("  skipped: answers no 0x6x dump request, so there is no 0x6b to mirror");
            continue;
        }

        // 1. Read all sixteen. This half is proven (§9, 2026-08-26) and is the
        //    before-picture: a probe that cannot read the kit cannot check a
        //    write to it either, so a failure here stops the box.
        let mut sounds: Vec<Option<Sound>> = Vec::new();
        for t in 0..KIT_TRACKS {
            match device.fetch_kit_track_sound(t) {
                Ok(payload) => {
                    let decoded = decode_at_wrapper(&payload);
                    println!(
                        "  T{:<2} {:>5}b  {}",
                        t + 1,
                        payload.len(),
                        match &decoded {
                            Ok(s) => format!("{:<18} {:?}", s.name, s.tags(&identity.slug)),
                            Err(e) => format!("undecodable: {e}"),
                        }
                    );
                    sounds.push(decoded.ok());
                }
                Err(e) => {
                    println!("  T{:<2} fetch failed: {e}", t + 1);
                    sounds.push(None);
                }
            }
        }

        // Write mode probes the named box and no other. The read pass above
        // still runs for every box, because a survey costs nothing and the
        // before-picture of the box next door is worth having in the log.
        if !writing || !selected.contains(&input.name.as_str()) {
            continue;
        }

        // 2. The allowlist, before anything is prepared rather than after. It is
        //    checked again inside the store — this is the same gate, asked early
        //    so the refusal reads as a refusal and not as a failed probe.
        let gate = write_gate(Some(&identity));
        if !gate.ok {
            println!("  no 0x5b sent: {}", gate.reason);
            continue;
        }

            probe_store(&mut device, track, hold);
    }
}

/// Send one `0x5b`, check it by re-reading, and put the name back.
///
/// Both payload shapes get a turn. The wrapped one is tried first because it is
/// the byte-for-byte mirror of what `0x6b` returned, which is the relationship
/// `store_pattern_kit` has with `0x60` — but the wrapper is five bytes this
/// project has never interpreted, so "the store wants the struct alone" is a
/// live alternative rather than a fallback nobody expects.
fn probe_store(device: &mut ElektronDevice, track: u8, hold: Option<u64>) {
    let Ok(original) = device.fetch_kit_track_sound(track) else {
        println!("  cannot read T{} back, so nothing will be written to it", track + 1);
        return;
    };
    let Ok(before) = decode_at_wrapper(&original) else {
        println!("  T{} does not decode as a sound; refusing to write to it", track + 1);
        return;
    };

    // The restore, on disk before the first byte goes out — see this file's
    // header. A process that dies between the store and the restore should
    // leave the original somewhere a person can reach.
    let path = std::env::temp_dir().join(format!("probe5b-t{}-original.bin", track + 1));
    match std::fs::write(&path, &original) {
        Ok(()) => println!("\n  original bytes saved to {}", path.display()),
        Err(e) => {
            println!("  could not save the original ({e}) — refusing to write without it");
            return;
        }
    }
    println!("  T{} is {:?}; watch the box's screen", track + 1, before.name);

    for (label, offset) in [("wrapped (as 0x6b returned it)", 0), ("struct only", SOUND_WRAPPER)] {
        let payload = &original[offset..];
        let renamed = with_name(payload, PROBE_NAME);
        if let Err(e) = device.store_kit_track_sound(track, &renamed) {
            println!("  {label}: refused before sending: {e}");
            continue;
        }
        std::thread::sleep(AFTER_STORE);

        match read_name_twice(device, track) {
            Ok(after) if after.as_bytes() == PROBE_NAME => {
                println!("  {label}: POSITIVE — T{} now reads {after:?}", track + 1);
                println!("           confirm on the box's screen before this is believed");
                wait_for_the_screen(hold, track);
                restore(device, track, &original, &before.name);
                return;
            }
            Ok(after) if after != before.name => {
                // The interesting failure: something moved, but not what was
                // aimed at. Worth more than a negative, and worth stopping on.
                println!(
                    "  {label}: CHANGED BUT NOT AS SENT — T{} reads {after:?}, expected {:?}",
                    track + 1,
                    String::from_utf8_lossy(PROBE_NAME)
                );
                restore(device, track, &original, &before.name);
                return;
            }
            Ok(_) => println!("  {label}: no change"),
            Err(why) => {
                println!("  {label}: {why} — STOPPING");
                restore(device, track, &original, &before.name);
                return;
            }
        }
    }

    println!(
        "  negative: neither shape changed T{}. 0x5b is not a per-track sound store, \
         or not this one.",
        track + 1
    );
    // A negative means nothing was stored — but that is the hypothesis being
    // tested, so the restore runs anyway rather than on the strength of it.
    restore(device, track, &original, &before.name);
}

/// Put the original bytes back and say whether they landed.
///
/// Runs after every outcome including the negative ones, because "nothing was
/// stored" is the claim under test and a probe may not lean on its own
/// hypothesis to decide whether it needs to clean up.
fn restore(device: &mut ElektronDevice, track: u8, original: &[u8], was: &str) {
    match device.store_kit_track_sound(track, original) {
        Ok(()) => std::thread::sleep(AFTER_STORE),
        Err(e) => {
            println!("  RESTORE FAILED TO SEND: {e} — reload the pattern on the box");
            return;
        }
    }
    match read_name_twice(device, track) {
        Ok(now) if now == was => println!("  restored: T{} reads {was:?}", track + 1),
        Ok(now) => println!(
            "  RESTORE DID NOT TAKE: T{} reads {now:?}, was {was:?} — reload the pattern on the \
             box (do not save)",
            track + 1
        ),
        Err(why) => println!("  could not verify the restore ({why}) — reload the pattern"),
    }
}

/// Pause so a person can read the box's own screen, if asked to.
///
/// The byte evidence and the screen are two independent witnesses, and this
/// probe's whole design rests on having both — but the restore is a fifth of a
/// second behind the store, so the screen witness is unreachable unless
/// something waits. Reading a line rather than sleeping a fixed count, because
/// how long it takes to walk to a box is not a number this program can guess.
fn wait_for_the_screen(hold: Option<u64>, track: u8) {
    let Some(seconds) = hold else {
        println!("           (pass --hold to keep it on screen and read T{} off the box)", track + 1);
        return;
    };
    println!(
        "\n  HOLDING {seconds}s — open the kit's track {} on the box and read its name.",
        track + 1
    );
    for left in (1..=seconds).rev() {
        std::thread::sleep(Duration::from_secs(1));
        if left % 10 == 0 || left <= 3 {
            println!("    {}s", left - 1);
        }
    }
    println!("  restoring");
}

/// Read the track's name twice, and only believe it if both agree.
///
/// **This is the echo guard, and it is not paranoia about loopback.** A store
/// gets no reply, so the only evidence is a re-read — and if the box has MIDI
/// thru or port echo on, our own `0x5b` comes back at our input carrying the
/// name we just sent, which `fetch_kit_track_sound` cannot tell from the box
/// answering a fetch. `device.rs`'s header records that hazard for loopback
/// ports; a box with thru enabled is the same hazard on a real cable, and it
/// fails in the one direction that matters: it manufactures a **positive**.
///
/// An echo is a one-shot. `fetch_dump` drains before it sends, so a second read
/// cannot see the same stray message twice: if read 1 says `PROBE5B` and read 2
/// says the old name, that was our own message coming home. Two reads agreeing
/// is the box.
///
/// The box's own screen is still the deciding witness — this makes the byte
/// evidence worth reporting alongside it rather than replacing it.
fn read_name_twice(device: &mut ElektronDevice, track: u8) -> Result<String, String> {
    let mut names = Vec::new();
    for _ in 0..2 {
        let payload = device
            .fetch_kit_track_sound(track)
            .map_err(|e| format!("T{} no longer answers ({e})", track + 1))?;
        let sound = decode_at_wrapper(&payload)
            .map_err(|e| format!("T{} no longer decodes ({e})", track + 1))?;
        names.push(sound.name);
    }
    if names[0] != names[1] {
        return Err(format!(
            "two reads disagree ({:?} then {:?}) — that is our own message echoing back, \
             not the box; check MIDI thru on this box before believing anything here",
            names[0], names[1]
        ));
    }
    Ok(names.pop().unwrap())
}

/// Decode the sound in a `0x6b` payload, which sits behind the 5-byte wrapper.
fn decode_at_wrapper(payload: &[u8]) -> Result<Sound, SoundError> {
    decode_sound_dump(payload.get(SOUND_WRAPPER..).unwrap_or(&[]))
}

/// A copy of `payload` with the sound's 16-byte name replaced and everything
/// else byte-identical.
///
/// `payload` may or may not carry the wrapper, so the struct is found by its
/// head magic rather than by an assumed offset — `decode_sound_dump` is what
/// says where it starts, and it checks both magics before agreeing.
fn with_name(payload: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = payload.to_vec();
    let base = if decode_sound_dump(payload).is_ok() { 0 } else { SOUND_WRAPPER };
    let field = base + SOUND_NAME_OFFSET;
    if out.len() >= field + 16 {
        out[field..field + 16].fill(0);
        out[field..field + name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
    }
    out
}
