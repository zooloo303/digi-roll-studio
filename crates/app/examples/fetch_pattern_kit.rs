// Fetch one pattern-kit dump from each connected box and decode it.
//
// **Read-only.** Two API requests to identify, then one 0x60 pattern-kit request
// per box — every one of them through `assert_request_opcode`, which refuses any
// opcode that stores. Since 2026-08-18 a store path does exist in `digi_midi`;
// this example never reaches it, because reaching it means calling
// `safe_write_track`. Same safety class as `identify_into_session`.
//
// This exists for one reason: `midi/sysex_stream.rs` has never met real
// hardware. PLAN.md §9 calls it "what has not [touched a box], and matters
// most". A pattern-kit dump is ~111 KB of 7-bit SysEx — far more than one driver
// callback delivers on CoreMIDI — so the reassembler has to stitch it back
// together, and any real-time byte the box interleaves has to be stripped rather
// than spliced into the payload. Both cases are unit-tested; neither had been
// seen for real.
//
// The checksum is what makes this a test rather than a demo. `fetch_dump`
// rejects a dump whose checksum or count is wrong, and *both* failure modes
// above corrupt the payload in a way the checksum catches: a spliced clock byte
// shifts every following byte, and a dropped callback truncates. A decode that
// comes back clean is the reassembler working end to end.
//
// It also joins the whole fetch path into one run: `fetch_pattern_kit` →
// `decode_pattern_kit` → `read_swing` → `read_track_trig_settings` →
// `Session::import_pattern`, which ends with the box's pattern sitting in a slot
// of a real session. That last step is what PLAN.md §5 had listed as missing;
// everything the import model does is unit-tested against the committed
// fixtures (`core/tests/import.rs`), and this is where it meets a box.
//
// Run with:
//   cargo run -p digi_roll_studio --example fetch_pattern_kit
//   cargo run -p digi_roll_studio --example fetch_pattern_kit -- "Digitakt II=A01" "Digitone II=A03"

use digi_core::device::{model_for_slug, PortRef};
use digi_core::import::Fetched;
use digi_core::session::PatternRef;
use digi_core::{two_box_session, Session};
use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::pattern::decode_pattern_kit;
use digi_protocol::pattern_settings::read_swing;

/// What to pull when nothing is named on the command line.
const DEFAULT_TARGETS: &[(&str, &str)] = &[("digitakt2", "A01"), ("digitone2", "A03")];

fn main() {
    let overrides: Vec<(String, String)> = std::env::args()
        .skip(1)
        .filter_map(|a| a.split_once('=').map(|(p, s)| (p.to_string(), s.to_string())))
        .collect();

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    // The same session the app opens with: a DT2 and a DN2, one bank of slots
    // each. A fetched pattern has to land somewhere, and "somewhere" is a real
    // session or the fetch has proved nothing about the import.
    let mut session = two_box_session();

    let mut attempted = 0usize;
    let mut ok = 0usize;

    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let slug = input.slug.expect("filtered");
        // A command-line override wins; otherwise fall back to this slug's
        // default. A box with neither is skipped rather than guessed at.
        let label = overrides
            .iter()
            .find(|(frag, _)| input.name.contains(frag.as_str()))
            .map(|(_, s)| s.clone())
            .or_else(|| {
                DEFAULT_TARGETS
                    .iter()
                    .find(|(s, _)| *s == slug)
                    .map(|(_, p)| p.to_string())
            });
        let Some(label) = label else {
            println!("\n{}: no pattern named for this box — skipped", input.name);
            continue;
        };
        let Some((slot, index)) = PatternRef::from_label(&label)
            .and_then(|slot| slot.wire_index().map(|index| (slot, index)))
        else {
            println!("\n{}: {label:?} is not a pattern label like A01 — skipped", input.name);
            continue;
        };
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
            println!("\n{}: no matching output port", input.name);
            continue;
        };

        attempted += 1;
        println!("\n{} — fetching pattern {label} (index {index}) …", input.name);

        let mut device =
            match ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output)) {
                Ok(d) => d,
                Err(e) => {
                    println!("  could not open: {e}");
                    continue;
                }
            };
        // The fetch needs the family byte off the identity, so identify first —
        // and it confirms which box actually answered on this port.
        let identity = match device.identify() {
            Ok(id) => id,
            Err(e) => {
                println!("  no identity: {e}");
                continue;
            }
        };
        println!("  {} — build {}, version {}", identity.name, identity.build, identity.version);

        // Which device in the session just spoke. The import needs one: a slot
        // belongs to a box, never to "the session" (PLAN.md §7 rule 4).
        let in_ref = PortRef { id: input.id.clone(), name: input.name.clone() };
        let out_ref = PortRef { id: output.id.clone(), name: output.name.clone() };
        let bound = match session.bind_identity(&identity, in_ref, out_ref) {
            Ok(id) => Some(id),
            Err(e) => {
                println!("  not bound to a device: {e} — decoding anyway, importing nothing");
                None
            }
        };

        // The spec comes from the identity's slug, not from the port name. A
        // model with `sysex: None` is sequence-live-only and says so here rather
        // than failing somewhere inside the decoder.
        let Some(model) = model_for_slug(&identity.slug) else {
            println!("  unrecognised slug {:?} — not decoding", identity.slug);
            continue;
        };
        let Some(spec_fn) = model.sysex else {
            println!("  {} is live-play only — no SysEx dumps", model.display);
            continue;
        };
        let spec = spec_fn();

        let payload = match device.fetch_pattern_kit(index) {
            Ok(p) => p,
            Err(e) => {
                // A CorruptDump here is the interesting failure: it means the
                // reassembler handed the parser something the box did not send.
                println!("  fetch failed: {e}");
                continue;
            }
        };
        println!("  {} payload bytes, checksum and count OK", payload.len());

        let kit = match decode_pattern_kit(spec, &payload) {
            Ok(k) => k,
            Err(e) => {
                println!("  decode failed: {e}");
                continue;
            }
        };

        let trigs: usize = kit.tracks.iter().map(|t| t.trigs.len()).sum();
        let notes: usize =
            kit.tracks.iter().flat_map(|t| t.trigs.values()).map(|v| v.len()).sum();
        println!(
            "  pattern {:?}  v{}  {:.1} bpm  kit {} {:?}",
            kit.name, kit.version, kit.tempo_bpm, kit.kit_index, kit.kit.name
        );
        println!(
            "  swing {}%  ({} tracks, {trigs} trigs, {notes} notes)",
            read_swing(spec, &payload),
            kit.tracks.len(),
        );
        // The end of the path: the decoded pattern into the slot it came from.
        // Read-only still — this writes to a `Session` in memory and sends the
        // box nothing.
        if let Some(device_id) = bound {
            let fetched = Fetched { spec, kit: &kit, payload: &payload, from: slot };
            match session.import_pattern(device_id, slot, &fetched) {
                Ok(report) => println!(
                    "  imported into {} {} — {} note(s) across {} track(s), swing {}%{}",
                    session.device(device_id).map(|d| d.name.as_str()).unwrap_or("?"),
                    slot.label(),
                    report.notes,
                    report.tracks_with_notes,
                    report.swing,
                    if report.trimmed_past_len > 0 {
                        format!(", {} trig(s) past LEN dropped", report.trimmed_past_len)
                    } else {
                        String::new()
                    },
                ),
                Err(e) => println!("  not imported: {e}"),
            }
        }

        for (t, track) in kit.tracks.iter().enumerate() {
            if track.trigs.is_empty() {
                continue;
            }
            let steps: Vec<String> = track.trigs.keys().map(|s| (s + 1).to_string()).collect();
            println!(
                "    track {:>2}: len {:>3}  {:>2} trigs on steps {}",
                t + 1,
                track.length_steps,
                track.trigs.len(),
                steps.join(",")
            );
        }
        ok += 1;
    }

    print_session(&session);

    println!("\n{ok}/{attempted} pattern(s) fetched and decoded.");
    if attempted > 0 && ok == attempted {
        println!(
            "sysex_stream.rs has now reassembled a real multi-callback dump — the\n\
             checksum passing is what says so. PLAN.md §9 can lose that entry."
        );
    }
}

/// Every slot in the session that has notes in it — which, before this example
/// existed, was never any of them.
fn print_session(session: &Session) {
    println!("\nsession after importing:");
    let mut any = false;
    for device in &session.devices {
        for (index, pattern) in device.patterns.iter().enumerate() {
            let notes: usize = pattern.tracks().iter().map(|t| t.notes.len()).sum();
            if notes == 0 {
                continue;
            }
            any = true;
            let source = pattern
                .source
                .as_ref()
                .map(|s| format!("{} {}", s.device_slug, PatternRef::new(s.bank, s.index).label()))
                .unwrap_or_else(|| "—".to_string());
            println!(
                "  {:<4} {:<4} {:<20} {notes:>4} note(s), swing {}%, from {source}",
                device.name,
                PatternRef::from_slot(index).label(),
                pattern.name,
                pattern.swing,
            );
        }
    }
    if !any {
        println!("  (nothing imported)");
    }
}
