// Write the two project files the Analog Four's live-play check needs, so the
// notes under test are exact rather than drawn with a mouse.
//
// Steps 2 and 3 of the 2026-08-28 hardware session — live notes on channels
// 1-6, and 64-step patterns. Both are questions about *which* note lands
// *where*, and a pattern clicked into the roll answers them approximately. This
// authors them.
//
// **It touches no MIDI at all.** It builds a `Session`, runs it through the same
// `Project::to_json_pretty` the Save menu uses, and writes two files. Open them
// with Session > Open; auto-connect binds the A4 onto the row by handshake, the
// way it would any saved project.
//
// Run with:  cargo run -p digi_roll_studio --example a4_test_sessions [dir]

use std::path::PathBuf;

use digi_core::device::{Device, A4};
use digi_core::model::Note;
use digi_core::session::PatternRef;
use digi_core::{Project, Session};

fn main() {
    // Default into `local/`, which is gitignored: these are session scratch,
    // not fixtures, and a file dialog can reach them in two clicks from the
    // repo. `.json` because that is the extension `ui::session`'s open dialog
    // filters on — a `.dgroll` would simply not appear in the picker.
    let dir: PathBuf =
        std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("local/a4-check"));
    std::fs::create_dir_all(&dir).expect("could not make the output directory");

    write(&dir.join("a4-channels.json"), channels());
    write(&dir.join("a4-64-steps.json"), sixty_four());

    println!();
    println!("Open these with Session > Open in the app.");
}

/// Step 2: one note per row, each on its own step, so six tracks land as six
/// separate events in one bar rather than as a chord nobody can take apart.
///
/// Rows are two steps apart and the pitches climb, so the order is audible as
/// well as visible: if row 3 sounds where row 2 should, the channel map is
/// wrong and it is wrong in a way you can hear without looking at the box.
///
/// Every row keeps `Track::new`'s channel — `index % 16`, so rows 1-6 go out on
/// MIDI channels 1-6. That is the assumption under test; nothing here sets a
/// channel, because setting one would test this file instead of the default.
fn channels() -> Session {
    let mut session = Session {
        name: "A4 channels 1-6".into(),
        // Slow, because six events two steps apart at 120 is a blur and the
        // point is to watch which track on the box lights up for each one.
        tempo_bpm: 90.0,
        ..Session::default()
    };

    let mut device = Device::new("A4", &A4, 16);
    let pattern = device.pattern_mut(0).expect("slot A01");
    for row in 0..6 {
        let track = pattern.track_mut(row).expect("the A4 has six rows");
        track.length_steps = 16;
        // C3 up in fourths: far enough apart that two rows cannot be confused
        // by ear, and inside every A4 voice's range.
        let pitch = 48 + (row as u8) * 5;
        track.notes.push(Note::new((row * 2) as f64, pitch, 1.0, 100, 0.0));
    }
    let id = session.add_device(device);
    session.set_slot_in_scene(0, id, PatternRef::from_slot(0));
    session
}

/// Step 3: does a 64-step pattern actually run to 64?
///
/// Row 1 is 64 steps long and carries a note on the **first** and the **last**
/// step. Row 2 is 16 steps long and carries one note on its first step, so it
/// restates the downbeat four times per lap of row 1.
///
/// That shape is chosen because the two likely failures sound different:
///
/// * a pattern truncated to 16 or 32 puts row 1's last note in the wrong place
///   relative to row 2's ticking downbeat, and the lap comes round early;
/// * a pattern that runs to 64 correctly puts row 1's last note immediately
///   before row 2's fourth downbeat, once every four bars.
///
/// One note at step 63 and nothing else would prove neither: it would sound the
/// same whether the lap were 64 steps or the pattern were simply short. The
/// second row is the ruler.
fn sixty_four() -> Session {
    let mut session =
        Session { name: "A4 64 steps".into(), tempo_bpm: 120.0, ..Session::default() };

    let mut device = Device::new("A4", &A4, 16);
    let pattern = device.pattern_mut(0).expect("slot A01");

    let row1 = pattern.track_mut(0).expect("row 1");
    row1.length_steps = 64;
    row1.notes.push(Note::new(0.0, 48, 1.0, 110, 0.0));
    row1.notes.push(Note::new(63.0, 60, 1.0, 110, 0.0));

    let row2 = pattern.track_mut(1).expect("row 2");
    row2.length_steps = 16;
    row2.notes.push(Note::new(0.0, 53, 1.0, 80, 0.0));

    let id = session.add_device(device);
    session.set_slot_in_scene(0, id, PatternRef::from_slot(0));
    session
}

fn write(path: &std::path::Path, session: Session) {
    let json = Project::new(session).to_json_pretty().expect("a session this file just built");
    std::fs::write(path, json).unwrap_or_else(|e| panic!("could not write {}: {e}", path.display()));
    println!("wrote {}", path.display());
}
