// Drive `safe_write_track` against a real box — the first thing in this repo that
// can store a pattern on one.
//
// **Read this before running it.** There are two modes and the default is the
// harmless one:
//
//   cargo run -p digi_roll_studio --example safe_write_track
//       A *rehearsal*. Identifies, fetches, runs the whole safe-write flow, and
//       swallows the send: `Rehearsal` below is a `PatternIo` that records the
//       payload instead of transmitting it and then answers the verify read with
//       what it recorded, so the flow runs green end to end with **no write opcode
//       reaching a port**. Same safety class as `fetch_pattern_kit` — one identity
//       handshake and one 0x60 request per box.
//
//   cargo run -p digi_roll_studio --example safe_write_track -- --write
//       **The real thing.** Overwrites one track of one pattern slot on the box.
//       Asks for typed consent first, and refuses to proceed without it.
//
// PLAN.md §7 rule 5 keeps hardware out of the dev loop, and §9 says a first write
// is a separate manual step. This is that step, built so the rehearsal can be run
// as often as you like and the write is something you have to ask for twice.
//
// **What the rehearsal is worth.** Everything except the twelve inches of cable:
// the allowlist gate on the live identity, the re-fetch, the stash actually
// writing a backup file to disk, the confirm wording, the minimal-diff encode over
// the box's own bytes, and the verify comparison. The one thing it cannot tell you
// is whether the box *accepts* the message, which is the only reason `--write`
// exists.
//
// **The edit** is deliberately the same one `trig_write_dry_run` has already been
// run against both boxes with a clean minimal diff (PLAN.md §9 entry 8): a
// condition on the first trig of the busiest track, and that track's PROB default.
// So the bytes are not new — the send is. It is confined to one track, it is
// obvious on the box's own TRIG page, and the pre-write backup puts it back.

use std::collections::BTreeMap;
use std::io::Write as _;

use digi_core::device::model_for_slug;
use digi_core::session::PatternRef;
use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::backup_stash::Stash;
use digi_protocol::device::DeviceIdentity;
use digi_protocol::pattern::{decode_pattern_kit, track_notes, Note};
use digi_protocol::safe_write::{
    safe_write_track, write_gate, write_impact_lines, write_result_message, ConfirmArgs, ImpactArgs,
    PatternIo, PatternKitFile, Timestamp, TrackWrite, WriteError, WriteHooks, WriteResult,
    BACKUP_LINE,
};
use digi_protocol::trig_cond::TrigSetting;

/// What to write to when nothing is named on the command line. A01 on each box,
/// matching every other example here.
const DEFAULT_TARGETS: &[(&str, &str)] = &[("digitakt2", "A01"), ("digitone2", "A01")];

/// The lock this puts on one trig, and the track PROB default it sets. Identical
/// to `trig_write_dry_run`'s, on purpose: those exact bytes have already been
/// encoded against both boxes' real payloads and diffed clean.
const LOCK: TrigSetting = TrigSetting { prob: Some(35), fill: Some(true), cond: Some("2:4") };
const TRACK_PROB: u8 = 70;

/// The word `--write` makes you type. Not a y/n, because a y/n is something you
/// press by reflex.
const CONSENT: &str = "overwrite";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let for_real = args.iter().any(|a| a == "--write");
    let overrides: Vec<(String, String)> = args
        .iter()
        .filter_map(|a| a.split_once('=').map(|(p, s)| (p.to_string(), s.to_string())))
        .collect();

    if for_real {
        println!(
            "*** --write: this will OVERWRITE one track of one pattern slot on the box. ***\n\
             The whole destination pattern is backed up first and can be restored from\n\
             the backup store. Consent is asked for per box, below.\n"
        );
    } else {
        println!(
            "Rehearsal. The safe-write flow runs end to end and the send is swallowed:\n\
             nothing is stored on any box. Pass --write to do it for real.\n"
        );
    }

    // A rehearsal must not leave rows in the real restore list — that list means
    // "patterns this app overwrote", and a rehearsal overwrote nothing. A real
    // write uses the real store, because being in it is the point.
    let stash = if for_real {
        match Stash::default_stash() {
            Ok(s) => s,
            Err(e) => {
                println!("Refusing to write: the backup store is unusable ({e}).");
                println!("Rule 1 — a backup that cannot be stored is a write that does not happen.");
                return;
            }
        }
    } else {
        let dir = std::env::temp_dir().join("digi-roll-studio-rehearsal");
        println!("Rehearsal backups go to {} (not the real store).\n", dir.display());
        Stash::at(dir)
    };

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    let mut attempted = 0usize;
    let mut written = 0usize;

    for (slug, slot) in DEFAULT_TARGETS {
        let wanted = overrides
            .iter()
            .find(|(p, _)| p.to_lowercase().contains(&slug.replace('2', " ii")))
            .map(|(_, s)| s.as_str())
            .unwrap_or(slot);
        let Some(index) = PatternRef::from_label(wanted).and_then(|r| r.wire_index()) else {
            println!("{slug}: {wanted:?} is not a slot this box has — skipping");
            continue;
        };

        // Match ports by name the way every other example does.
        let Some(input) = inputs.iter().find(|p| slug_matches(&p.name, slug)) else {
            continue;
        };
        let Some(output) = outputs.iter().find(|p| slug_matches(&p.name, slug)) else {
            println!("{}: no output port — a write goes out on it", input.name);
            continue;
        };
        attempted += 1;
        println!("--- {} → slot {wanted} ---", input.name);

        let mut device = match ElektronDevice::open(
            &PortBinding::from(input),
            &PortBinding::from(output),
        ) {
            Ok(d) => d,
            Err(e) => {
                println!("  could not open: {e}");
                continue;
            }
        };
        let identity = match device.identify() {
            Ok(id) => id,
            Err(e) => {
                println!("  no identity: {e}");
                continue;
            }
        };
        println!("  {} — build {}", identity.name, identity.build);

        // The allowlist, shown before anything else happens. `safe_write_track`
        // checks it again and `plan_store` a third time; this is only so the
        // refusal is legible rather than arriving as an error four steps later.
        let gate = write_gate(Some(&identity));
        if !gate.ok {
            println!("  write refused: {}", gate.reason);
            continue;
        }

        let Some(write) = plan_edit(&mut device, &identity, index) else { continue };

        let mut hooks = Hooks { for_real, consented: false };
        let result = if for_real {
            safe_write_track(&mut device, &stash, &write, &mut hooks, Timestamp::now())
        } else {
            let mut rehearsal = Rehearsal { inner: device, stored: BTreeMap::new() };
            safe_write_track(&mut rehearsal, &stash, &write, &mut hooks, Timestamp::now())
        };

        match result {
            Ok(r) => {
                report(&r);
                if r.ok && !r.cancelled {
                    written += 1;
                }
            }
            Err(e) => {
                println!("  FAILED: {e}");
                if matches!(e, WriteError::Stash(_) | WriteError::Backup(_) | WriteError::Gate(_)) {
                    println!("  Nothing was sent — that is the refusal working.");
                }
            }
        }
    }

    println!();
    if attempted == 0 {
        println!(
            "No box answered. This example needs hardware; the flow itself is pinned\n\
             against fixtures by `cargo test -p digi_protocol --test safe_write`."
        );
    } else if for_real {
        println!("{written}/{attempted} slot(s) written and verified byte-identical.");
    } else {
        println!(
            "{written}/{attempted} rehearsal(s) completed clean. **Nothing was sent to any\n\
             box** — the send was swallowed by the `Rehearsal` wrapper and no 0x50 opcode\n\
             reached a port."
        );
    }
}

fn slug_matches(port: &str, slug: &str) -> bool {
    let p = port.to_lowercase();
    match slug {
        "digitakt2" => p.contains("digitakt ii") || p.contains("digitakt2"),
        "digitone2" => p.contains("digitone ii") || p.contains("digitone2"),
        _ => false,
    }
}

/// Fetch the slot, pick the busiest track, and describe the write. Returns `None`
/// with a printed reason if there is nothing sensible to write.
///
/// Note this fetch is *not* the one the write uses. `safe_write_track` re-fetches
/// immediately before encoding (PLAN.md §7 rule 2) and this one only chooses the
/// track and the notes, so a pattern edited on the box between the two is still
/// encoded against its current bytes.
fn plan_edit(
    device: &mut ElektronDevice,
    identity: &DeviceIdentity,
    index: u8,
) -> Option<TrackWrite> {
    let spec = model_for_slug(&identity.slug).and_then(|m| m.sysex).map(|f| f())?;
    let payload = match device.fetch_pattern_kit(index) {
        Ok(p) => p,
        Err(e) => {
            println!("  fetch failed: {e}");
            return None;
        }
    };
    let kit = match decode_pattern_kit(spec, &payload) {
        Ok(k) => k,
        Err(e) => {
            println!("  decode failed: {e}");
            return None;
        }
    };
    let track = (0..kit.tracks.len()).max_by_key(|&t| kit.tracks[t].trigs.len())?;
    let notes: Vec<Note> = track_notes(&kit, track);
    if notes.is_empty() {
        println!("  every track of kit {:?} is empty — draw some trigs and try again", kit.kit.name);
        return None;
    }
    let first = notes.iter().map(|n| n.step).min()?;
    // The *kit* name, not `kit.name` — these boxes do not name patterns, so
    // `PatternKit::name` comes back empty and the kit is what a person recognises
    // the slot by. It is also what the backup store puts in its rows.
    println!(
        "  kit {:?}: writing track {} back with {} note(s), a lock on step {}",
        kit.kit.name,
        track + 1,
        notes.len(),
        first + 1,
    );
    Some(TrackWrite {
        index,
        track_index: track,
        notes: notes
            .into_iter()
            .map(|n| {
                let s = if n.step == first { LOCK } else { TrigSetting::default() };
                (n, s)
            })
            .collect(),
        track_prob: Some(TRACK_PROB),
        // `None`, not `Some(vec![])`: this example does not model automation, and
        // an empty vec would mean "this track has no lanes" and free the ones the
        // box is holding. Leaving the pool alone is the honest thing for a caller
        // that has nothing to say about it.
        plocks: None,
        // Swing reaches all sixteen tracks in the slot, so a caller that is not
        // asking for it must leave the byte alone.
        swing: None,
    })
}

/// A `PatternIo` that goes through the motions without transmitting anything.
///
/// Forwards the identity and the re-fetch to the real box — so the gate, the
/// backup and the encode all run against live bytes — and then keeps the store in
/// a map, answering the verify read from it. The verify therefore passes for the
/// same reason it would against a box that stored the message perfectly, which is
/// exactly the rehearsal being asked for: everything but the wire.
struct Rehearsal {
    inner: ElektronDevice,
    stored: BTreeMap<u8, Vec<u8>>,
}

impl PatternIo for Rehearsal {
    fn identity(&self) -> Option<&DeviceIdentity> {
        PatternIo::identity(&self.inner)
    }

    fn fetch_pattern_kit(&mut self, index: u8) -> Result<Vec<u8>, String> {
        if let Some(p) = self.stored.get(&index) {
            return Ok(p.clone());
        }
        PatternIo::fetch_pattern_kit(&mut self.inner, index)
    }

    fn send_pattern_kit(&mut self, index: u8, payload: &[u8]) -> Result<(), String> {
        println!(
            "  [rehearsal] would send {} payload bytes to slot {index} — swallowed",
            payload.len()
        );
        self.stored.insert(index, payload.to_vec());
        Ok(())
    }
}

/// Progress and consent on a terminal.
struct Hooks {
    for_real: bool,
    consented: bool,
}

impl WriteHooks for Hooks {
    fn confirm(&mut self, args: &ConfirmArgs) -> bool {
        // One track: this example drives `safe_write_track`, the one-element
        // case of the plural flow.
        let track = args.one();
        println!(
            "\n  About to replace track {} of {} — {} trig(s) there now, {} note(s) going in.",
            track.track_index + 1,
            args.label,
            track.existing_trigs,
            track.note_count,
        );
        // The shared wording, so this terminal says exactly what the UI will.
        // Anything omitted here is a surface a user gets surprised by later.
        for line in write_impact_lines(&ImpactArgs {
            label: &args.label,
            track: Some(track.track_index),
            lanes: &[],
            box_plocks: &track.box_plocks,
            free_lanes: args.free_lanes,
            track_prob: Some(TRACK_PROB),
            swing: None,
            box_swing: args.swing,
        }) {
            println!("  {line}");
        }
        println!("  {BACKUP_LINE}");

        if !self.for_real {
            println!("  [rehearsal] consenting automatically — nothing will be sent.");
            self.consented = true;
            return true;
        }
        print!("\n  Type {CONSENT:?} to write, anything else to skip: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return false;
        }
        self.consented = line.trim() == CONSENT;
        if !self.consented {
            println!("  skipped.");
        }
        self.consented
    }

    fn on_backup(&mut self, backup: &PatternKitFile) -> Result<(), String> {
        println!("  backup taken: {} ({} bytes framed)", backup.name, backup.bytes.len());
        Ok(())
    }

    fn on_status(&mut self, status: &str) {
        println!("  {status}");
    }

    fn on_log(&mut self, line: &str) {
        println!("  {line}");
    }
}

/// The result, plus where the bytes moved — because a verify failure is only
/// actionable if you can see which region disagreed.
fn report(r: &WriteResult) {
    let msg = write_result_message(r);
    println!("  {}", msg.text);
    if r.cancelled {
        return;
    }
    for w in &r.warnings {
        println!("  warning: {w}");
    }
    if r.diffs.is_empty() {
        return;
    }
    println!("  the first {} mismatching offset(s):", r.diffs.len().min(10));
    // Both sides are optional because one payload can be shorter than the other,
    // and "the box sent back fewer bytes than we wrote" is a different failure from
    // "byte 900 came back wrong". Printing `--` keeps them distinguishable.
    let byte = |b: Option<u8>| b.map_or("--".to_string(), |v| format!("{v:02x}"));
    for d in r.diffs.iter().take(10) {
        println!(
            "    offset {:>7}  sent 0x{}  read 0x{}",
            d.offset,
            byte(d.sent),
            byte(d.read)
        );
    }
}
