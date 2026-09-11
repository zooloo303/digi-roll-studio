//! Hardware acceptance on an explicitly named slot in a throwaway project.
//! Writes notes, chord, timing, condition and filter p-lock to track 1, verifies
//! the complete payload, then restores and verifies the pre-write backup.
//! Usage: verify_firmware digitone2 A16 OUTPUT_DIR --throwaway
//! No OS gate is bypassed: candidate builds must first pass capture analysis.
use std::path::PathBuf;

use digi_core::session::PatternRef;
use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::a4_plocks::A4LaneWrite;
use digi_protocol::backup_stash::Stash;
use digi_protocol::pattern::Note;
use digi_protocol::plocks::LaneWrite;
use digi_protocol::safe_write::{
    a4_safe_write_tracks, safe_restore_pattern_kit, safe_write_track, syntakt_safe_write_tracks,
    A4Step, A4TrackWrite, PatternKitFile, SyntaktStep, SyntaktTrackWrite, Timestamp, TrackWrite,
    WriteHooks,
};
use digi_protocol::syntakt_pattern::{self as st, SyntaktCond};
use digi_protocol::trig_cond::TrigSetting;

struct Evidence {
    out: PathBuf,
    original: Option<PatternKitFile>,
}
impl WriteHooks for Evidence {
    fn on_backup(&mut self, backup: &PatternKitFile) -> Result<(), String> {
        std::fs::write(self.out.join(&backup.name), &backup.bytes).map_err(|e| e.to_string())?;
        if self.original.is_none() { self.original = Some(backup.clone()); }
        Ok(())
    }
    fn on_status(&mut self, status: &str) { println!("{status}"); }
    fn on_log(&mut self, line: &str) { println!("{line}"); }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 || args[3] != "--throwaway" {
        return Err("usage: verify_firmware DEVICE_SLUG SLOT OUTPUT_DIR --throwaway; overwrites track 1, verifies, then restores the slot".into());
    }
    let slug = &args[0];
    let expected = match slug.as_str() {
        "digitakt2" => ("0079", "1.16"),
        "digitone2" => ("0059", "1.11"),
        "analogfour" => ("0201", "1.55D"),
        "syntakt" => ("0082", "1.40"),
        _ => return Err("not a September 2026 firmware target".into()),
    };
    let index = PatternRef::from_label(&args[1]).and_then(|p| p.wire_index()).ok_or("invalid pattern slot")?;
    let out = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out)?;
    let inputs = list_inputs()?;
    let outputs = list_outputs()?;
    let matches: Vec<_> = inputs.iter().filter(|p| p.slug == Some(slug.as_str())).collect();
    if matches.len() != 1 { return Err("expected exactly one matching device".into()); }
    let input = matches[0];
    let output = outputs.iter().find(|p| p.name == input.name).ok_or("missing output port")?;
    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))?;
    let id = device.identify()?;
    if id.slug != *slug || (id.build.as_str(), id.version.as_str()) != expected {
        return Err(format!("unexpected device identity: {id:?}").into());
    }
    println!("{} OS {} build {}: {} track 1; restore afterwards", id.name, id.version, id.build, args[1]);
    let stash = Stash::at(out.join("stash"));
    let mut hooks = Evidence { out: out.clone(), original: None };
    let result = if slug == "analogfour" {
        let mut steps = vec![None; 64];
        for (step, note, micro) in [(0, 60, 0), (4, 67, 1)] {
            steps[step] = Some(A4Step {
                note, velocity: 100, length: 14, micro_timing: micro,
                condition: Some(0), arp_notes: if step == 0 { [Some(4), Some(7), None] } else { [None; 3] },
            });
        }
        a4_safe_write_tracks(&mut device, &stash, &[A4TrackWrite {
            index, track_index: 0, steps,
            plocks: Some(vec![A4LaneWrite::new(0x22, vec![Some(64 << 8)])]),
        }], &mut hooks, Timestamp::now())
    } else if slug == "syntakt" {
        // Five lanes and the pattern's swing, which is every field this box's
        // format carries. The values differ from each other on purpose: a verify
        // that sent the same number down every lane could not tell a lane that
        // arrived from a lane that was already right.
        let ratio = st::condition_byte(&SyntaktCond::Ratio { a: 2, b: 4 }).ok_or("2:4")?;
        let first = st::condition_byte(&SyntaktCond::Logic { name: "1ST", negated: false })
            .ok_or("1ST")?;
        let mut steps = vec![None; st::NUM_STEPS];
        for (step, note, velocity, length_byte, micro_ticks, condition_byte) in [
            (0usize, 60u8, 100u8, 14u8, 0i8, st::NO_LOCK),
            (4, 67, 80, 7, 8, ratio),
            (9, 55, 120, 14, -8, first),
        ] {
            steps[step] =
                Some(SyntaktStep { note, velocity, length_byte, micro_ticks, condition_byte });
        }
        syntakt_safe_write_tracks(
            &mut device,
            &stash,
            &[SyntaktTrackWrite { index, track_index: 0, steps, swing: Some(58.0) }],
            &mut hooks,
            Timestamp::now(),
        )
    } else {
        let condition = TrigSetting { prob: Some(75), fill: Some(false), cond: Some("2:4") };
        let notes = [(0, 60, 100, 1.0, 0.0), (0, 64, 90, 2.0, 0.0), (4, 67, 80, 1.0, 1.0 / 24.0)]
            .into_iter().map(|(step, pitch, velocity, len_steps, micro)|
                (Note { step, pitch, velocity, len_steps, micro }, condition)).collect();
        safe_write_track(&mut device, &stash, &TrackWrite {
            index, track_index: 0, notes, track_prob: Some(80), swing: None,
            plocks: Some(vec![LaneWrite::new(if slug == "digitakt2" { 44 } else { 74 }, vec![Some(64 << 8)])]),
        }, &mut hooks, Timestamp::now())
    };
    match &result {
        Ok(r) => println!("WRITE: ok={}, written={}, dropped={}, diffs={:?}, warnings={:?}", r.ok, r.written, r.dropped, r.diffs, r.warnings),
        Err(e) => println!("WRITE: {e}"),
    }
    // Restore even after a failed send/verification; the exact fresh backup
    // supplied by the write flow is the authority, never an earlier capture.
    if let Some(original) = hooks.original.clone() {
        let restore = safe_restore_pattern_kit(&mut device, &stash, index, &original.payload, &mut hooks, Timestamp::now())?;
        if !restore.ok { return Err(format!("RESTORE FAILED: {:?}; backup at {}", restore.diffs, out.display()).into()); }
        println!("RESTORE: byte-identical");
    }
    let result = result?;
    if !result.ok { return Err("write verification failed; see mismatch report".into()); }
    if result.dropped != 0 || !result.warnings.is_empty() { return Err("write altered the requested content; inspect the report".into()); }
    std::fs::write(out.join("verified.txt"), format!("{id:?}\nSlot {} track 1: write/readback and restore byte-identical\n", args[1]))?;
    Ok(())
}
