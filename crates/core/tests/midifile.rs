// The MIDI file codec, against values derived by running the JS.
//
// **The oracle has no tests of its own.** `test/` in `digi-roll` contains no
// occurrence of `patternToMidiFile` or `midiFileToNotes` — checked by grep before
// this port began, the way `DEVELOPMENT.md` says to. So the byte strings below were
// produced by *running* the original, and each test records the exact call that
// produced it:
//
// ```text
// cd ../digi-roll && node --input-type=module -e "
//   import { patternToMidiFile } from './js/midi.js';
//   const p = { name: 'A01 T1', channel: 3, lengthSteps: 16, swing: 50, notes: [
//     { step: 0, pitch: 60, len: 1,     velocity: 100, micro: 0 },
//     { step: 1, pitch: 63, len: 2,     velocity: 40,  micro: 0.25 },
//     { step: 4, pitch: 67, len: 0.125, velocity: 127, micro: -0.25 } ] };
//   console.log([...patternToMidiFile(p, 120)].map(b=>b.toString(16).padStart(2,'0')).join(' '));"
// ```
//
// The three notes in that fixture are chosen to exercise one thing each: a plain
// note on the downbeat, an **odd** step carrying positive micro-timing (so swing
// and micro land on the same note), and a note shorter than a tick (so the
// one-tick floor is exercised).

use digi_core::edit_ops::{BAR_STEPS, MAX_STEPS};
use digi_core::midifile::{
    midi_file_to_notes, midi_file_name, track_to_midi_file, MidiFileError, TICKS_PER_STEP, TPQN,
};
use digi_core::{Note, Track, TrackKind};

/// The fixture the byte strings were derived from, as a `Track`.
fn fixture() -> Track {
    let mut track = Track::new(0, TrackKind::Audio);
    track.channel = 3;
    track.length_steps = 16;
    track.notes = vec![
        Note::new(0.0, 60, 1.0, 100, 0.0),
        Note::new(1.0, 63, 2.0, 40, 0.25),
        Note::new(4.0, 67, 0.125, 127, -0.25),
    ];
    track
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

#[test]
fn an_export_is_byte_for_byte_what_the_js_writes() {
    // Derived, not reasoned about: `patternToMidiFile(fixture, 120)`.
    let expected = "4d 54 68 64 00 00 00 06 00 00 00 01 00 60 4d 54 72 6b 00 00 00 2e 00 ff 03 \
                    06 41 30 31 20 54 31 00 ff 51 03 07 a1 20 00 93 3c 64 18 83 3c 00 06 93 3f 28 \
                    30 83 3f 00 0c 93 43 7f 03 83 43 00 82 23 ff 2f 00";
    assert_eq!(hex(&track_to_midi_file(&fixture(), "A01 T1", 50, 120.0)), expected);
}

#[test]
fn swing_and_tempo_move_the_bytes_the_way_the_js_moves_them() {
    // `patternToMidiFile({ ...fixture, swing: 66 }, 174)`. Two things change and
    // nothing else does: the tempo meta's three bytes, and the deltas around the
    // odd step — which is the whole claim that swing is baked into tick
    // positions rather than stored.
    let expected = "4d 54 68 64 00 00 00 06 00 00 00 01 00 60 4d 54 72 6b 00 00 00 2e 00 ff 03 \
                    06 41 30 31 20 54 31 00 ff 51 03 05 42 fc 00 93 3c 64 18 83 3c 00 09 93 3f 28 \
                    30 83 3f 00 09 93 43 7f 03 83 43 00 82 23 ff 2f 00";
    assert_eq!(hex(&track_to_midi_file(&fixture(), "A01 T1", 66, 174.0)), expected);
}

#[test]
fn the_header_says_type_zero_one_track_at_ninety_six_ticks() {
    let bytes = track_to_midi_file(&fixture(), "A01 T1", 50, 120.0);
    assert_eq!(&bytes[0..4], b"MThd");
    assert_eq!(u32::from_be_bytes(bytes[4..8].try_into().unwrap()), 6);
    assert_eq!(u16::from_be_bytes(bytes[8..10].try_into().unwrap()), 0, "type 0");
    assert_eq!(u16::from_be_bytes(bytes[10..12].try_into().unwrap()), 1, "one track");
    assert_eq!(u16::from_be_bytes(bytes[12..14].try_into().unwrap()), TPQN);
    assert_eq!(TICKS_PER_STEP, 24.0, "a 16th step is a whole number of ticks");
}

#[test]
fn a_pattern_with_no_notes_still_runs_out_to_its_full_length() {
    // The bar count is what a loop is; a file that ends at the last note would
    // shorten a pattern whose last bar is empty.
    let mut track = fixture();
    track.notes.clear();
    track.length_steps = 64;
    let bytes = track_to_midi_file(&track, "empty", 50, 120.0);
    // The end-of-track delta is a VLQ of 64 * 24 = 1536, then FF 2F 00.
    let tail = &bytes[bytes.len() - 5..];
    assert_eq!(tail, &[0x8c, 0x00, 0xff, 0x2f, 0x00]);
}

#[test]
fn a_note_off_comes_before_a_note_on_that_shares_its_tick() {
    // Two one-step notes on the same pitch, back to back. Without the ordering
    // the file says "on, on, off, off" and a reader hears one long note.
    let mut track = Track::new(0, TrackKind::Audio);
    track.channel = 0;
    track.notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0), Note::new(1.0, 60, 1.0, 100, 0.0)];
    let bytes = track_to_midi_file(&track, "x", 50, 120.0);
    let statuses: Vec<u8> =
        bytes.iter().copied().filter(|b| *b == 0x90 || *b == 0x80).collect();
    assert_eq!(statuses, [0x90, 0x80, 0x90, 0x80]);
}

#[test]
fn an_exported_file_reads_back_as_the_notes_that_went_into_it() {
    // What survives the round trip and what does not is the point of this one.
    // `midiFileToNotes(patternToMidiFile(fixture, 120))` in the JS answers
    // step/pitch/velocity/micro exactly as below — including the third note's
    // length coming back as 1 rather than 0.125, because a MIDI file holds a
    // duration in ticks and this grid holds a length in steps.
    let bytes = track_to_midi_file(&fixture(), "A01 T1", 50, 120.0);
    let back = midi_file_to_notes(&bytes, MAX_STEPS).expect("our own file parses");
    let got: Vec<(f64, u8, f64, u8, f64)> =
        back.notes.iter().map(|n| (n.step, n.pitch, n.len, n.velocity, n.micro)).collect();
    assert_eq!(
        got,
        [
            (0.0, 60, 1.0, 100, 0.0),
            (1.0, 63, 2.0, 40, 0.25),
            (4.0, 67, 1.0, 127, -0.25),
        ]
    );
    assert_eq!(back.length_steps, 16);
    assert_eq!(back.dropped, 0);
}

#[test]
fn conditions_do_not_survive_the_round_trip_and_that_is_the_format() {
    // The claim the export button has to make on screen. A MIDI file has no PROB,
    // no FILL and no COND, so this asserts the loss rather than pretending to
    // prevent it — if a future format ever carried them, this test is where the
    // claim in `ui::edit` would have to be revisited.
    let mut track = fixture();
    track.notes[0].prob = Some(50);
    track.notes[0].fill = Some(true);
    track.notes[0].cond = Some(String::from("2:4"));
    let bytes = track_to_midi_file(&track, "A01 T1", 50, 120.0);
    let back = midi_file_to_notes(&bytes, MAX_STEPS).unwrap();
    assert!(back.notes.iter().all(|n| n.prob.is_none() && n.fill.is_none() && n.cond.is_none()));
}

#[test]
fn swing_comes_back_as_micro_timing_because_the_file_cannot_say_which_it_was() {
    // The one-way door in the module header, asserted. A straight note on an odd
    // step exported at swing 66 returns carrying micro-timing.
    let mut track = Track::new(0, TrackKind::Audio);
    track.channel = 0;
    track.notes = vec![Note::new(1.0, 60, 1.0, 100, 0.0)];
    let bytes = track_to_midi_file(&track, "x", 66, 120.0);
    let back = midi_file_to_notes(&bytes, MAX_STEPS).unwrap();
    assert_eq!(back.notes[0].step, 1.0);
    assert!(back.notes[0].micro > 0.0, "the swing is now micro-timing: {}", back.notes[0].micro);
}

#[test]
fn a_file_at_a_different_resolution_lands_on_this_grid() {
    // Division 480 — Logic's and Ableton's usual export. 120 ticks to a 16th, so
    // a note 60 ticks in is half a step late and must arrive as micro-timing
    // rather than quantised onto step 0 or step 1.
    let bytes = smf(480, &[(0, 0x90, 60, 100), (60, 0x80, 60, 0), (0, 0x90, 62, 90), (120, 0x80, 62, 0)]);
    let back = midi_file_to_notes(&bytes, MAX_STEPS).unwrap();
    assert_eq!(back.notes.len(), 2);
    assert_eq!((back.notes[0].step, back.notes[0].micro), (0.0, 0.0));
    // 60/120 = 0.5 → `Math.round(0.5)` is 1, so the note is step 1 half a step
    // early. The clamp holds it at -0.49, which is the gesture's window and the
    // reason a re-export is a tick out rather than a step out.
    assert_eq!(back.notes[1].step, 1.0);
    assert_eq!(back.notes[1].micro, -0.49);
}

#[test]
fn running_status_is_understood() {
    // Two note-ons sharing one status byte, which is what most sequencers write.
    // A parser that missed this would read the second note's pitch as a status.
    let mut body = Vec::new();
    body.extend_from_slice(&[0x00, 0x90, 60, 100]);
    body.extend_from_slice(&[0x18, 62, 100]); // running status: no 0x90
    body.extend_from_slice(&[0x18, 0x80, 60, 0]);
    body.extend_from_slice(&[0x00, 0x80, 62, 0]);
    body.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let back = midi_file_to_notes(&wrap(96, &body), MAX_STEPS).unwrap();
    assert_eq!(back.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [60, 62]);
}

#[test]
fn a_note_that_is_never_released_gets_one_step() {
    let mut body = Vec::new();
    body.extend_from_slice(&[0x00, 0x90, 60, 100]);
    body.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let back = midi_file_to_notes(&wrap(96, &body), MAX_STEPS).unwrap();
    assert_eq!(back.notes.len(), 1);
    assert_eq!(back.notes[0].len, 1.0);
}

#[test]
fn a_note_on_at_velocity_zero_is_a_note_off() {
    let mut body = Vec::new();
    body.extend_from_slice(&[0x00, 0x90, 60, 100]);
    body.extend_from_slice(&[0x30, 0x90, 60, 0]); // the running-status note-off
    body.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let back = midi_file_to_notes(&wrap(96, &body), MAX_STEPS).unwrap();
    assert_eq!(back.notes.len(), 1);
    assert_eq!(back.notes[0].len, 2.0);
}

#[test]
fn the_first_track_with_notes_wins_and_the_empty_ones_are_skipped() {
    // A type-1 file's first track is usually tempo-only. Taking it would import
    // nothing and report an empty file.
    let mut meta_only = Vec::new();
    meta_only.extend_from_slice(&[0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20]);
    meta_only.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let mut notes = Vec::new();
    notes.extend_from_slice(&[0x00, 0x90, 64, 90]);
    notes.extend_from_slice(&[0x18, 0x80, 64, 0]);
    notes.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MThd");
    bytes.extend_from_slice(&6u32.to_be_bytes());
    bytes.extend_from_slice(&[0, 1]); // type 1
    bytes.extend_from_slice(&[0, 2]); // two tracks
    bytes.extend_from_slice(&96u16.to_be_bytes());
    for body in [&meta_only, &notes] {
        bytes.extend_from_slice(b"MTrk");
        bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(body);
    }
    let back = midi_file_to_notes(&bytes, MAX_STEPS).unwrap();
    assert_eq!(back.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [64]);
}

#[test]
fn the_length_is_whole_bars_and_the_notes_past_the_limit_are_counted_not_clamped() {
    // A note at step 20 needs two bars; a note at step 200 has nowhere to go on a
    // box and is dropped rather than parked on the last step. Derived from the
    // oracle: `{"notes":[{"step":20,...}],"lengthSteps":32,"dropped":1}`.
    let bytes = smf(
        96,
        &[
            (24 * 20, 0x90, 60, 100),
            (24, 0x80, 60, 0),
            (24 * 179, 0x90, 62, 100),
            (24, 0x80, 62, 0),
        ],
    );
    let back = midi_file_to_notes(&bytes, MAX_STEPS).unwrap();
    assert_eq!(back.notes.iter().map(|n| n.step).collect::<Vec<_>>(), [20.0]);
    assert_eq!(back.length_steps, 32, "step 20 is in bar 2, so two bars");
    assert_eq!(back.dropped, 1);
}

#[test]
fn a_short_file_never_reports_less_than_one_bar() {
    let bytes = smf(96, &[(0, 0x90, 60, 100), (24, 0x80, 60, 0)]);
    assert_eq!(midi_file_to_notes(&bytes, MAX_STEPS).unwrap().length_steps, BAR_STEPS);
}

#[test]
fn a_length_is_clipped_to_the_track_it_lands_in() {
    // One long note in the first bar: the import may not hand back a length that
    // runs past the wrap line it just chose.
    let bytes = smf(96, &[(0, 0x90, 60, 100), (24 * 40, 0x80, 60, 0)]);
    let back = midi_file_to_notes(&bytes, MAX_STEPS).unwrap();
    assert_eq!(back.length_steps, BAR_STEPS);
    assert_eq!(back.notes[0].len, 16.0);
}

#[test]
fn max_steps_is_respected_and_never_exceeds_what_a_box_holds() {
    let bytes = smf(96, &[(24 * 40, 0x90, 60, 100), (24, 0x80, 60, 0)]);
    // Asked for one bar: the note at step 40 is past it, so it is dropped.
    let tight = midi_file_to_notes(&bytes, 16).unwrap();
    assert!(tight.notes.is_empty());
    assert_eq!(tight.dropped, 1);
    // Asked for more than a box can hold: capped, not honoured.
    let wide = midi_file_to_notes(&bytes, 4096).unwrap();
    assert_eq!(wide.notes[0].step, 40.0);
    assert!(wide.length_steps <= MAX_STEPS);
}

#[test]
fn a_file_with_no_notes_at_all_is_an_empty_import_rather_than_an_error() {
    let mut body = Vec::new();
    body.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let back = midi_file_to_notes(&wrap(96, &body), MAX_STEPS).unwrap();
    assert!(back.notes.is_empty());
    assert_eq!(back.length_steps, BAR_STEPS);
}

#[test]
fn the_three_ways_a_file_is_refused_each_say_which() {
    assert_eq!(midi_file_to_notes(b"not a midi file at all", MAX_STEPS), Err(MidiFileError::NotAMidiFile));
    assert_eq!(midi_file_to_notes(b"short", MAX_STEPS), Err(MidiFileError::NotAMidiFile));
    // Division 0xE728: 25 fps, 40 subframes. A tick is a video frame, and there
    // is no honest step grid for that.
    let smpte = wrap(0xe728, &[0x00, 0xff, 0x2f, 0x00]);
    assert_eq!(midi_file_to_notes(&smpte, MAX_STEPS), Err(MidiFileError::SmpteTimecode));
    // A track chunk claiming more bytes than the file holds. The JS reads
    // `undefined` here and carries on; this refuses, which is the one place the
    // port is stricter than its oracle.
    let mut truncated = wrap(96, &[0x00, 0xff, 0x2f, 0x00]);
    truncated.truncate(truncated.len() - 2);
    assert_eq!(midi_file_to_notes(&truncated, MAX_STEPS), Err(MidiFileError::Truncated));
}

#[test]
fn a_header_longer_than_six_bytes_is_stepped_over() {
    // Legal, and rare enough to be exactly the sort of thing a parser gets wrong.
    let body = [0x00u8, 0x90, 60, 100, 0x18, 0x80, 60, 0, 0x00, 0xff, 0x2f, 0x00];
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MThd");
    bytes.extend_from_slice(&8u32.to_be_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 1]);
    bytes.extend_from_slice(&96u16.to_be_bytes());
    bytes.extend_from_slice(&[0xaa, 0xbb]); // two bytes of nothing this reads
    bytes.extend_from_slice(b"MTrk");
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);
    assert_eq!(midi_file_to_notes(&bytes, MAX_STEPS).unwrap().notes.len(), 1);
}

#[test]
fn an_exported_filename_survives_a_pattern_name_off_a_box_and_refuses_a_path() {
    assert_eq!(midi_file_name("A01 T1"), "A01 T1.mid");
    assert_eq!(midi_file_name("  kick  "), "kick.mid");
    assert_eq!(midi_file_name("../../etc/passwd"), "etcpasswd.mid");
    // A file called `.mid` is a hidden file on this platform, so an unusable name
    // falls back rather than producing one.
    assert_eq!(midi_file_name("///"), "pattern.mid");
    assert_eq!(midi_file_name(""), "pattern.mid");
}

// --- fixture helpers ---------------------------------------------------------

/// A one-track SMF around `body`.
fn wrap(division: u16, body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MThd");
    bytes.extend_from_slice(&6u32.to_be_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 1]);
    bytes.extend_from_slice(&division.to_be_bytes());
    bytes.extend_from_slice(b"MTrk");
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes
}

/// A one-track SMF from `(delta, status, d1, d2)` events, with the end-of-track
/// appended.
fn smf(division: u16, events: &[(u32, u8, u8, u8)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (delta, status, d1, d2) in events {
        // A VLQ, written out here rather than reused from the module: a test that
        // shares the code under test cannot fail it.
        let mut v = *delta;
        let mut vb = vec![(v & 0x7f) as u8];
        v /= 128;
        while v > 0 {
            vb.insert(0, ((v & 0x7f) | 0x80) as u8);
            v /= 128;
        }
        body.extend_from_slice(&vb);
        body.extend_from_slice(&[*status, *d1, *d2]);
    }
    body.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    wrap(division, &body)
}
