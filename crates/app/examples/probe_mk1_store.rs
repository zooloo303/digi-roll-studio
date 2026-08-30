// Does `0x5b` accept a **Digitone mk1** sound where a native one is expected?
//
// PLAN.md §10.6 step 6 left this refused, and the refusal is what a third of a
// DN2's library meets: **388 of 1,189 presets are `DN1S` files**, spread across
// banks B, C and D rather than confined to one, so browsing for a bass patch
// hits one about as often as not. `drive::preset_load_payload` turns them away
// on container magic, and the panel says so — see the screenshot in §9.
//
// # Why the refusal is not obviously right, which is why this exists
//
// The easy assumption is that an mk1 preset is simply *foreign* and the box
// would reject it. Two facts say otherwise, and they pull in opposite
// directions.
//
// **For:** the DN2 uses these presets. It lists them in its own browser, loads
// them onto its own tracks, and — §9, 2026-08-29 — re-maps their mk1 tag bits
// into its own 32-cell vocabulary before displaying them. So a conversion path
// exists inside the box. The only question is whether `0x5b` is on the near
// side of it.
//
// **Against:** that path may live entirely in the box's own load code, with
// `0x5b` taking the kit-track format and nothing else.
//
// # The detail that makes this worth probing rather than assuming
//
// **An mk1 payload is 364 bytes and a native DN2 payload is 364 bytes.** The
// length check in `midi::preset_load` — the one that asks the box's own `0x6b`
// reply how long a payload should be — therefore *passes* for an mk1 preset. It
// is not the thing refusing them. The layouts differ inside an identical
// envelope:
//
// ```text
//   native  [5-byte wrapper][BEEFBACE … 359 bytes]   = 364
//   mk1     [DN1S … flush, no wrapper …          ]   = 364
// ```
//
// That is exactly the shape where a size check gives false comfort, and it is
// the reason the format gate is separate rather than folded into it.
//
// # What this sends, and why it is a fair test
//
// The **whole payload of a real mk1 preset file, verbatim** — the same bytes
// `preset_load_payload` would hand to `store_kit_track_sound` if the magic
// check were removed, and nothing constructed. So a positive result here is
// directly a statement about the shipping load path rather than about a probe's
// own byte-fiddling.
//
// It reads a *native* preset onto the track first, so the before-picture is
// known and decodable, and so a "nothing changed" result cannot be confused
// with "it was already that".
//
// # Recovery, and why this is riskier than `probe_sound_store`
//
// That probe sent the box's own bytes back with one field changed, so a null
// result was harmless by construction. This sends a layout the box may not
// recognise, into the kit it is playing. Three things stand behind it, in the
// order that matters:
//
// 1. **The active kit is a working buffer.** Reloading the pattern on the box
//    discards an unsaved kit and brings the stored one back. That is hardware
//    behaviour and it is the real undo. **Do not save the project while this
//    runs.**
// 2. **The original bytes are on disk before anything is sent**, so a process
//    that dies between the store and the restore leaves the restore reachable.
// 3. **The restore runs after every outcome**, including the negative ones — a
//    probe may not lean on its own hypothesis to decide whether it needs to
//    clean up. `probe_sound_store`'s rule, and it is not weaker here.
//
// # Reading the result
//
// Four outcomes, and three of them are findings:
//
// * **POSITIVE** — the track reads back as the mk1 preset's name. `0x5b` takes
//   the format, the refusal in `drive::preset_load_payload` should come out, and
//   388 presets become loadable. Confirm on the box's own screen before this is
//   believed: pass `--hold`.
// * **CHANGED, NOT AS SENT** — something landed and it is not what went out.
//   The most interesting answer and the worst one to ship on: it means the box
//   accepted bytes it did not understand. The refusal stays and gets stronger.
// * **NO CHANGE** — the box ignored it. The refusal is correct and costs
//   nothing; mark the rows in the browser instead.
// * **UNDECODABLE AFTER** — the read-back no longer parses as a sound. Report
//   the head bytes and restore. This is the outcome that most wants the screen.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_mk1_store
//   cargo run -p digi_roll_studio --example probe_mk1_store -- --write --box Digitone
//   cargo run -p digi_roll_studio --example probe_mk1_store -- --write --box Digitone --hold 45

use std::time::Duration;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding, KIT_TRACKS};
use digi_protocol::drive::{
    container_offset, parse_list_entries, preset_load_payload, FILE_HEADER_LEN,
};
use digi_protocol::safe_write::write_gate;
use digi_protocol::sound::{decode_sound_dump, Sound, SoundError, SOUND_WRAPPER};

/// The track probed unless `--track` says otherwise. The last one, on the same
/// reasoning `probe_sound_store` picked it: a throwaway project's T16 is the
/// least likely to be carrying something somebody wants.
const DEFAULT_TRACK: u8 = 15;

/// On top of the send pacing's own settle — a read that races the store reports
/// a false negative, which is the one error this probe must not make.
const AFTER_STORE: Duration = Duration::from_millis(400);

/// Where the presets are.
const SOUNDBANKS: &str = "/soundbanks";

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let writing = argv.iter().any(|a| a == "--write");
    let hold = argv.iter().position(|a| a == "--hold").map(|at| {
        argv.get(at + 1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(45)
    });
    let target = argv.windows(2).find(|w| w[0] == "--box").map(|w| w[1].clone());
    let track: u8 = argv
        .windows(2)
        .find(|w| w[0] == "--track")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(DEFAULT_TRACK);

    if writing && target.is_none() {
        eprintln!(
            "--write needs --box <name fragment>, so that exactly one box is probed.\n\
             Run without --write first to see what is connected and what it holds."
        );
        std::process::exit(2);
    }

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

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
            "WRITE MODE — {}, track {} of the ACTIVE kit.\n\
             It is sent one mk1 preset's payload and then the original bytes.\n\
             Recovery if this run dies: reload the pattern on the box (an unsaved kit is \
             discarded).\n\
             Do NOT save the project while this runs.\n",
            selected[0],
            track + 1,
        );
    } else {
        println!(
            "READ-ONLY — nothing is stored. This finds the mk1 presets and reports what \
             would be sent.\nPass --write --box <name> to probe the store.\n"
        );
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

        if identity.family.is_none() {
            println!("  skipped: answers no 0x6x dump request, so there is no 0x5b at all");
            continue;
        }

        // Find one mk1 preset and one native one. Both are needed: the native
        // one sets a known before-picture so that "nothing changed" is a real
        // observation rather than a coincidence.
        let (mk1, native) = match survey(&mut device) {
            Ok(pair) => pair,
            Err(why) => {
                println!("  {why}");
                continue;
            }
        };
        println!("  mk1 preset:    {} — {} bytes of payload", mk1.0, mk1.1.len());
        println!("  native preset: {} — {} bytes of payload", native.0, native.1.len());
        if mk1.1.len() == native.1.len() {
            println!(
                "  the two payloads are the same length, which is why the length check \
                 cannot be what refuses one"
            );
        }

        if !writing || !selected.contains(&input.name.as_str()) {
            continue;
        }

        let gate = write_gate(Some(&identity));
        if !gate.ok {
            println!("  no 0x5b sent: {}", gate.reason);
            continue;
        }

        probe(&mut device, track, &mk1, &native, hold);
    }
}

/// One preset path and its load payload.
type Preset = (String, Vec<u8>);

/// Find the first mk1 preset and the first native one on this box.
///
/// Walks the banks in order and stops as soon as it has one of each, so it
/// costs a handful of reads rather than a library scan. `preset_load_payload`
/// is what decides which is which — deliberately the *shipping* rule rather
/// than a copy of it, so this probe cannot be testing a different question from
/// the one the app asks.
fn survey(device: &mut ElektronDevice) -> Result<(Preset, Preset), String> {
    let banks = list_dir(device, SOUNDBANKS)?;
    let mut mk1: Option<Preset> = None;
    let mut native: Option<Preset> = None;

    for bank in banks.iter().filter(|e| e.is_dir && !e.name.is_empty()) {
        let path = format!("{SOUNDBANKS}/{}", bank.name);
        let entries = list_dir(device, &path)?;
        for entry in entries.iter().filter(|e| {
            e.is_occupied() && e.children.is_none() && e.size.is_some_and(|s| s > 0)
        }) {
            if mk1.is_some() && native.is_some() {
                break;
            }
            let Some(index) = entry.index else { continue };
            let at = format!("{path}/{index}");
            let Ok(file) = device.drive_read_file(&at) else { continue };
            match preset_load_payload(&file) {
                Ok(payload) if native.is_none() => {
                    native = Some((at, payload.to_vec()));
                }
                Ok(_) => {}
                // Refused: take its payload the way the load path *would* if the
                // magic gate were not there — header to declared length, which
                // is the same cut for every container.
                Err(_) if mk1.is_none() => {
                    if let Some(payload) = raw_payload(&file) {
                        let magic = container_offset(&file)
                            .map(|o| {
                                u32::from_be_bytes([
                                    file[o],
                                    file[o + 1],
                                    file[o + 2],
                                    file[o + 3],
                                ])
                            })
                            .unwrap_or(0);
                        // `DN1S`, and only that: an A4 container would be a
                        // different question and this box cannot hold one.
                        if magic == 0x444e_3153 {
                            mk1 = Some((at, payload.to_vec()));
                        }
                    }
                }
                Err(_) => {}
            }
        }
        if mk1.is_some() && native.is_some() {
            break;
        }
    }

    match (mk1, native) {
        (Some(m), Some(n)) => Ok((m, n)),
        (None, _) => Err("no mk1 preset found on this box — nothing to probe".into()),
        (_, None) => Err("no native preset found, so there is no known before-picture".into()),
    }
}

fn list_dir(
    device: &mut ElektronDevice,
    path: &str,
) -> Result<Vec<digi_protocol::drive::ListEntry>, String> {
    let reply = device.drive_list(path, 0, 0).map_err(|e| format!("could not list {path}: {e}"))?;
    parse_list_entries(&reply.entry_bytes, reply.count)
        .map_err(|e| format!("{path} did not parse: {e}"))
}

/// The declared payload of a preset file, whatever its container.
///
/// The same cut `preset_load_payload` makes, without the magic check — which is
/// the whole point: the bytes under test are the ones the app would send if the
/// only thing standing in the way were removed.
fn raw_payload(file: &[u8]) -> Option<&[u8]> {
    let declared = digi_protocol::drive::file_declared_size(file)? as usize;
    let end = FILE_HEADER_LEN + declared;
    (end <= file.len()).then(|| &file[FILE_HEADER_LEN..end])
}

/// Send the mk1 payload, see what the track becomes, and put it back.
fn probe(
    device: &mut ElektronDevice,
    track: u8,
    mk1: &Preset,
    native: &Preset,
    hold: Option<u64>,
) {
    if track >= KIT_TRACKS {
        println!("  track {} is outside a kit", track + 1);
        return;
    }

    // The original, before anything at all — this is what gets restored, not
    // the native preset the probe is about to load as scaffolding.
    let Ok(original) = device.fetch_kit_track_sound(track) else {
        println!("  cannot read T{} back, so nothing will be written to it", track + 1);
        return;
    };
    let path = std::env::temp_dir().join(format!("probe-mk1-t{}-original.bin", track + 1));
    match std::fs::write(&path, &original) {
        Ok(()) => println!("\n  original bytes saved to {}", path.display()),
        Err(e) => {
            println!("  could not save the original ({e}) — refusing to write without it");
            return;
        }
    }

    // Scaffolding: put a *native* preset on the track first, so the
    // before-picture is a known name that the box definitely accepted. Without
    // this, "no change" could mean the track already held something similar.
    println!("  loading {} as the before-picture…", native.0);
    if let Err(e) = device.store_kit_track_sound(track, &native.1) {
        println!("  could not set up the before-picture: {e}");
        return;
    }
    std::thread::sleep(AFTER_STORE);
    let before = match read_name_twice(device, track) {
        Ok(name) => {
            println!("  T{} now reads {name:?}", track + 1);
            name
        }
        Err(why) => {
            println!("  {why} — STOPPING before the mk1 payload goes out");
            restore(device, track, &original);
            return;
        }
    };

    // The question.
    println!("\n  sending {} ({} bytes) under 0x5b…", mk1.0, mk1.1.len());
    if let Err(e) = device.store_kit_track_sound(track, &mk1.1) {
        // `plan_track_sound_store`'s own guard refusing is a result too: it
        // means the payload does not decode as a sound at either offset, which
        // is a statement about the format rather than about the box.
        println!("  REFUSED BEFORE SENDING: {e}");
        println!("  (that is this end's guard, not the box's answer)");
        restore(device, track, &original);
        return;
    }
    std::thread::sleep(AFTER_STORE);

    let expected = mk1_name(&mk1.1);
    match read_name_twice(device, track) {
        Ok(after) if Some(&after) == expected.as_ref() => {
            println!("  POSITIVE — T{} now reads {after:?}", track + 1);
            println!("           0x5b takes an mk1 payload. Confirm on the box's screen.");
            wait_for_the_screen(hold, track);
        }
        Ok(after) if after == before => {
            println!("  NO CHANGE — T{} still reads {after:?}", track + 1);
            println!("           the box ignored it. The refusal in the app is correct.");
        }
        Ok(after) => {
            println!(
                "  CHANGED BUT NOT AS SENT — T{} reads {after:?}, and the mk1 preset is {}",
                track + 1,
                expected.as_deref().unwrap_or("<undecodable as mk1>")
            );
            println!(
                "           the box accepted bytes it did not understand. This is the \
                 answer that makes the refusal stronger, not weaker."
            );
            wait_for_the_screen(hold, track);
        }
        Err(why) => {
            println!("  UNDECODABLE AFTER: {why}");
            println!("           the track no longer reads as a sound — the most important");
            println!("           outcome to see on the box's own screen.");
            wait_for_the_screen(hold, track);
        }
    }

    restore(device, track, &original);
}

/// The name inside an mk1 payload, which is flush rather than behind a wrapper.
///
/// Read straight off the container at +12, the offset every format on every box
/// puts it at (§9, 24 captures) — `decode_sound_dump` cannot be used because it
/// requires a digi head magic, which is the entire distinction under test.
fn mk1_name(payload: &[u8]) -> Option<String> {
    let name = payload.get(12..28)?;
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    Some(
        name[..end]
            .iter()
            .map(|b| digi_protocol::device::cp1252_char(*b))
            .collect::<String>()
            .trim_end()
            .to_string(),
    )
}

/// Read a track's name twice and refuse an answer that is our own echo.
///
/// `probe_sound_store`'s rule: with MIDI thru on, one read can return this end's
/// own message coming home, and two agreeing reads is the box rather than the
/// cable.
fn read_name_twice(device: &mut ElektronDevice, track: u8) -> Result<String, String> {
    let mut names = Vec::new();
    for _ in 0..2 {
        let payload = device
            .fetch_kit_track_sound(track)
            .map_err(|e| format!("T{} no longer answers ({e})", track + 1))?;
        let sound = decode_at_wrapper(&payload).map_err(|e| {
            format!(
                "T{} no longer decodes ({e}); payload starts {}",
                track + 1,
                payload.iter().take(16).map(|b| format!("{b:02x}")).collect::<String>()
            )
        })?;
        names.push(sound.name);
    }
    if names[0] != names[1] {
        return Err(format!(
            "two reads disagree ({:?} then {:?}) — that is our own message echoing back; \
             check MIDI thru on this box",
            names[0], names[1]
        ));
    }
    Ok(names.pop().unwrap())
}

fn decode_at_wrapper(payload: &[u8]) -> Result<Sound, SoundError> {
    decode_sound_dump(payload.get(SOUND_WRAPPER..).unwrap_or(&[]))
}

/// Put the track's own bytes back and say whether they landed.
fn restore(device: &mut ElektronDevice, track: u8, original: &[u8]) {
    println!("\n  restoring T{}…", track + 1);
    match device.store_kit_track_sound(track, original) {
        Ok(()) => std::thread::sleep(AFTER_STORE),
        Err(e) => {
            println!("  RESTORE FAILED TO SEND: {e} — reload the pattern on the box");
            return;
        }
    }
    match read_name_twice(device, track) {
        Ok(now) => println!("  restored: T{} reads {now:?}", track + 1),
        Err(why) => println!(
            "  could not verify the restore ({why}) — reload the pattern on the box, and \
             do not save it"
        ),
    }
}

/// Pause so a person can read the box's own screen.
fn wait_for_the_screen(hold: Option<u64>, track: u8) {
    let Some(seconds) = hold else {
        println!("           (pass --hold to keep it there and read T{} off the box)", track + 1);
        return;
    };
    println!("\n  HOLDING {seconds}s — open the kit's track {} on the box.", track + 1);
    for left in (1..=seconds).rev() {
        std::thread::sleep(Duration::from_secs(1));
        if left % 10 == 0 || left <= 3 {
            println!("    {}s", left - 1);
        }
    }
}
