//! `test/chords.test.js`, ported case for case, plus the four things the Rust
//! side has that the JS did not.
//!
//! **Every expected value in the ported half was derived by running
//! `js/chords.js` under node**, not by reasoning about intervals — the same rule
//! every phase in this repo has followed, and the reason the diatonic cases are
//! worth having at all: a chord table would have been easier to write and would
//! not have told anyone whether the *walk* is right.
//!
//! The four that are ours:
//!
//!   * the four-note cap is checked against **both boxes' specs** rather than
//!     against a literal, so a spec that ever disagreed with `MAX_CHORD_NOTES`
//!     fails here rather than at a write;
//!   * the strum grid is checked to be whole micro-timing **ticks**, which is
//!     where this deliberately parts from the JS's 0–100 slider;
//!   * `chord_for_cell` and `harmonise` keep every step inside the trig's
//!     four-note cap and say what they left out — the JS does neither;
//!   * and the duplicate-pitch case the JS's `harmonize` gets wrong.

use digi_core::chords::{
    chord_for_cell, chord_pitches, harmonise, strum_steps, voice_chord, ChordOpts, ChordSettings,
    Harmony, KeyScale, Quality, QualityChoice, Row, Scale, VoiceOpts, MAX_CHORD_NOTES,
    STRUM_TICKS_MAX,
};
use digi_core::edit_ops::MICRO_MAX;
use digi_core::Note;

/// C major as the roll passes it.
const C_MAJOR: KeyScale = KeyScale { root: 0, scale: Scale::Major };
const PENT: KeyScale = KeyScale { root: 0, scale: Scale::PentatonicMinor };
/// Middle C, which the boxes call C5.
const C: u8 = 60;

/// The whole roll, so nothing is dropped for being out of range.
fn opts() -> ChordOpts {
    ChordOpts::default()
}

fn quality(q: Quality) -> ChordOpts {
    ChordOpts { quality: q, ..opts() }
}

// --------------------------------------------------------------- fixed qualities

#[test]
fn it_builds_the_basic_triads() {
    assert_eq!(chord_pitches(C, &quality(Quality::Major)), [60, 64, 67]);
    assert_eq!(chord_pitches(C, &quality(Quality::Minor)), [60, 63, 67]);
    assert_eq!(chord_pitches(C, &quality(Quality::Sus2)), [60, 62, 67]);
    assert_eq!(chord_pitches(C, &quality(Quality::Sus4)), [60, 65, 67]);
    assert_eq!(chord_pitches(C, &quality(Quality::Dim)), [60, 63, 66]);
    assert_eq!(chord_pitches(C, &quality(Quality::Aug)), [60, 64, 68]);
}

#[test]
fn each_quality_adds_its_own_seventh() {
    let seventh = |q| ChordOpts { seventh: true, ..quality(q) };
    // maj7 — the lush voicing, not the dominant. See `Quality`.
    assert_eq!(chord_pitches(C, &seventh(Quality::Major)), [60, 64, 67, 71]);
    assert_eq!(chord_pitches(C, &seventh(Quality::Minor)), [60, 63, 67, 70]);
    assert_eq!(chord_pitches(C, &seventh(Quality::Dim)), [60, 63, 66, 69]);
}

#[test]
fn in_scale_with_no_scale_falls_back_to_major() {
    // The JS's "falls back to Major for an unknown quality" has no port — an enum
    // has no unknown — but the fallback that *is* reachable is this one, and it
    // has to land in the same place: the menu can say `In scale` while the scale
    // menu says `Off`.
    let harmony = Harmony {
        chord: ChordSettings { quality: QualityChoice::InScale, ..ChordSettings::default() },
        scale: None,
        ..Harmony::default()
    };
    let pitches: Vec<u8> = harmony.specs(C, 100, false, (0, 127)).iter().map(|c| c.pitch).collect();
    assert_eq!(pitches, [60, 64, 67]);
}

// ------------------------------------------------------------------ diatonic mode

#[test]
fn every_degree_of_c_major_gets_its_natural_quality() {
    let in_scale = ChordOpts { scale: Some(C_MAJOR), ..opts() };
    assert_eq!(chord_pitches(60, &in_scale), [60, 64, 67], "I major");
    assert_eq!(chord_pitches(62, &in_scale), [62, 65, 69], "ii minor");
    assert_eq!(chord_pitches(64, &in_scale), [64, 67, 71], "iii minor");
    assert_eq!(chord_pitches(71, &in_scale), [71, 74, 77], "vii diminished");
}

#[test]
fn the_v7_comes_out_dominant_not_major_seventh() {
    let opts = ChordOpts { scale: Some(C_MAJOR), seventh: true, ..opts() };
    assert_eq!(chord_pitches(67, &opts), [67, 71, 74, 77], "G7");
}

#[test]
fn an_out_of_scale_root_snaps_to_the_nearest_tone_preferring_below() {
    let in_scale = ChordOpts { scale: Some(C_MAJOR), ..opts() };
    assert_eq!(chord_pitches(61, &in_scale), [60, 64, 67], "C# to C, the tie going down");
    assert_eq!(chord_pitches(66, &in_scale), [65, 69, 72], "F# to F");
}

#[test]
fn thirds_past_the_octave_wrap_in_a_short_scale() {
    let pent = ChordOpts { scale: Some(PENT), ..opts() };
    assert_eq!(chord_pitches(60, &pent), [60, 65, 70], "degrees 0, 5, 10");
    assert_eq!(chord_pitches(70, &pent), [70, 75, 79], "from the top degree, up and over");
}

#[test]
fn a_snap_can_cross_the_octave_wrap() {
    // B in a minor pentatonic is one semitone above its Bb and four above its G:
    // the search has to look into the octave below to find that, which is the
    // whole reason `snap_to_scale` tries three octaves.
    let pent = ChordOpts { scale: Some(PENT), ..opts() };
    assert_eq!(chord_pitches(71, &pent)[0], 70);
}

// ------------------------------------------------------------------------ voicing

#[test]
fn inversions_move_the_bottom_note_up_an_octave() {
    let inv = |n| ChordOpts { inversion: n, ..opts() };
    assert_eq!(chord_pitches(C, &inv(1)), [64, 67, 72]);
    assert_eq!(chord_pitches(C, &inv(2)), [67, 72, 76]);
    assert_eq!(chord_pitches(C, &inv(3)), [60, 64, 67], "wraps on a triad");
    assert_eq!(
        chord_pitches(C, &ChordOpts { seventh: true, ..inv(3) }),
        [71, 72, 76, 79],
        "and does not wrap on a seventh"
    );
}

#[test]
fn spread_drops_the_second_note_from_the_top_an_octave() {
    assert_eq!(chord_pitches(C, &ChordOpts { spread: true, ..opts() }), [52, 60, 67]);
    assert_eq!(
        chord_pitches(C, &ChordOpts { spread: true, seventh: true, ..opts() }),
        [55, 60, 64, 71]
    );
}

#[test]
fn everything_stays_inside_the_pitch_range() {
    // The roll's own band, which is what the app passes.
    let band = ChordOpts { min: 24, max: 96, ..opts() };
    assert_eq!(chord_pitches(95, &band), [95], "top of the roll: the extensions go");
    assert_eq!(
        chord_pitches(25, &ChordOpts { spread: true, ..band }),
        [25, 32],
        "a drop-2 below the floor goes"
    );
}

#[test]
fn the_bands_own_edges_are_inside_it() {
    // C2 and C8 are rows the roll draws, so a chord tone on either is kept. Found by
    // the deliberate-bug pass: the two ported range cases both sit inside the band, so
    // `>=` could quietly become `>` with everything green — and the note that would
    // vanish is the lowest one anybody could draw.
    let band = ChordOpts { min: 24, max: 96, ..opts() };
    assert_eq!(chord_pitches(24, &band), [24, 28, 31], "the root on the floor stays");
    // And the ceiling: a fifth whose top note is exactly C8.
    assert_eq!(chord_pitches(89, &band), [89, 93, 96], "the fifth on the ceiling stays");
}

#[test]
fn a_chord_never_holds_more_than_a_trig_can() {
    for q in Quality::ALL {
        let pitches = chord_pitches(C, &ChordOpts { seventh: true, ..quality(q) });
        assert!(pitches.len() <= MAX_CHORD_NOTES, "{} is {} notes", q.label(), pitches.len());
    }
}

#[test]
fn the_cap_is_a_parameter_a_caller_can_lower() {
    // At four the cap cannot fire — a voicing is three notes or four by construction,
    // which is why the deliberate-bug pass could delete the `take` and break nothing.
    // What it is really for is a caller that wants fewer, so that is what is pinned:
    // the notes kept are the lowest, as they are everywhere else here.
    let two = ChordOpts { seventh: true, max_notes: 2, ..opts() };
    assert_eq!(chord_pitches(C, &two), [60, 64]);
}

#[test]
fn the_four_note_cap_is_both_boxes_own_limit() {
    // **The cap is not a taste decision, and this is what says so.** Both specs
    // carry `trig.max_notes`, and if either ever disagreed with the constant this
    // module caps voicings at, a chord would be built that the encoder then drops
    // a note of — silently, at a write, on hardware.
    for spec in [digi_protocol::pattern::dt2_spec(), digi_protocol::pattern::dn2_spec()] {
        assert_eq!(spec.trig.max_notes, MAX_CHORD_NOTES, "{}", spec.device);
    }
}

#[test]
fn pitches_are_deduped() {
    // An augmented 7th spread puts two voices on one pitch.
    let got = chord_pitches(
        C,
        &ChordOpts { seventh: true, spread: true, ..quality(Quality::Aug) },
    );
    let mut unique = got.clone();
    unique.dedup();
    assert_eq!(got, unique);
    assert_eq!(got, [56, 60, 64, 70]);
}

// -------------------------------------------------------------------- voice_chord

#[test]
fn strum_staggers_bottom_up_within_the_micro_clamp() {
    let specs = voice_chord(&[60, 64, 67, 71], &VoiceOpts { strum: 0.12, ..VoiceOpts::default() });
    let micros: Vec<f64> = specs.iter().map(|s| s.micro).collect();
    assert_eq!(micros, [0.0, 0.12, 0.24, 0.36]);

    let wild = voice_chord(&[60, 64, 67, 71], &VoiceOpts { strum: 0.3, ..VoiceOpts::default() });
    assert!(wild.iter().all(|s| s.micro <= MICRO_MAX));
}

#[test]
fn the_strum_control_counts_whole_micro_timing_ticks() {
    // **Where this parts from the JS on purpose.** Its slider is 0–100 mapped onto
    // 0–0.12 of a step, which is 2.88 of the box's 1/24-step ticks — so the
    // stagger it shows is not the stagger the card keeps. Every value this control
    // can reach is a whole tick, so the ghost's strum is the strum a re-fetch
    // gives back.
    for ticks in 0..=STRUM_TICKS_MAX {
        let steps = strum_steps(ticks);
        assert_eq!(steps * 24.0, f64::from(ticks), "{ticks} ticks is {steps} of a step");
    }
    // Three ticks on a four-note chord is the widest spread that does not clamp —
    // which is what `STRUM_TICKS_MAX` is chosen for.
    let widest = voice_chord(
        &[60, 64, 67, 71],
        &VoiceOpts { strum: strum_steps(STRUM_TICKS_MAX), ..VoiceOpts::default() },
    );
    assert_eq!(
        widest.iter().map(|s| s.micro).collect::<Vec<_>>(),
        [0.0, 0.125, 0.25, 0.375]
    );
    assert!(widest.iter().all(|s| s.micro < MICRO_MAX), "nothing reaches the clamp");
    // And the control refuses to ask for more, rather than clamping four notes on
    // top of each other.
    assert_eq!(strum_steps(STRUM_TICKS_MAX + 5), strum_steps(STRUM_TICKS_MAX));
}

#[test]
fn the_taper_eases_the_lower_notes_and_leaves_the_top_alone() {
    let specs = voice_chord(&[60, 64, 67], &VoiceOpts::default());
    let vels: Vec<u8> = specs.iter().map(|s| s.velocity).collect();
    assert_eq!(vels, [86, 93, 100]);
    let four = voice_chord(&[60, 64, 67, 71], &VoiceOpts::default());
    assert_eq!(four.iter().map(|s| s.velocity).collect::<Vec<_>>(), [79, 86, 93, 100]);
}

#[test]
fn the_taper_can_be_skipped_and_never_reaches_zero() {
    let flat = voice_chord(
        &[60, 64, 67],
        &VoiceOpts { velocity: 90, taper: false, ..VoiceOpts::default() },
    );
    assert!(flat.iter().all(|s| s.velocity == 90), "harmonising keeps the melody on top");
    // Velocity 0 is a note-off on the wire, so the taper floors at 1.
    let quiet = voice_chord(&[60, 64, 67, 71], &VoiceOpts { velocity: 1, ..VoiceOpts::default() });
    assert!(quiet.iter().all(|s| s.velocity >= 1));
}

// ------------------------------------------------------------------------ the key

#[test]
fn the_key_says_which_rows_to_tint_and_nothing_else() {
    let mut harmony = Harmony::default();
    // No scale, no tint: the roll draws its plain black-and-white rows.
    assert_eq!(harmony.row(60), None);

    harmony.scale = Some(Scale::Major);
    harmony.root = 2; // D major
    assert_eq!(harmony.row(62), Some(Row::Root), "D is the root");
    assert_eq!(harmony.row(50), Some(Row::Root), "and so is every D");
    assert_eq!(harmony.row(66), Some(Row::InScale), "F# is in it");
    assert_eq!(harmony.row(65), Some(Row::Outside), "F is not");
}

#[test]
fn the_inversion_cycles_both_ways_and_wraps() {
    let mut harmony = Harmony::default();
    for expected in [1, 2, 3, 0, 1] {
        harmony.cycle_inversion(1);
        assert_eq!(harmony.chord.inversion, expected);
    }
    for expected in [0, 3, 2] {
        harmony.cycle_inversion(-1);
        assert_eq!(harmony.chord.inversion, expected);
    }
}

// ------------------------------------------------------------------- chord draw

fn chord_mode() -> Harmony {
    Harmony {
        chord: ChordSettings {
            on: true,
            quality: QualityChoice::Fixed(Quality::Major),
            ..ChordSettings::default()
        },
        ..Harmony::default()
    }
}

const BAND: (u8, u8) = (24, 96);

#[test]
fn a_click_on_an_empty_step_stamps_the_whole_voicing() {
    let chord = chord_for_cell(&[], 4.0, C, &chord_mode(), 100, BAND);
    assert_eq!(chord.iter().map(|c| c.pitch).collect::<Vec<_>>(), [60, 64, 67]);
    assert_eq!(chord.iter().map(|c| c.velocity).collect::<Vec<_>>(), [86, 93, 100]);
}

#[test]
fn a_chord_never_stamps_a_pitch_the_step_already_holds() {
    // Two notes at one step and pitch are one wasted slot of the four a trig has,
    // and both would be written.
    let notes = vec![Note::new(4.0, 64, 1.0, 100, 0.0)];
    let chord = chord_for_cell(&notes, 4.0, C, &chord_mode(), 100, BAND);
    assert_eq!(chord.iter().map(|c| c.pitch).collect::<Vec<_>>(), [60, 67]);
}

#[test]
fn a_chord_is_truncated_to_the_room_the_trig_has_left() {
    // A step already holding two notes has room for two more, and the two that
    // land are the lowest of the voicing — the end the encoder keeps, so the ghost
    // and the card agree.
    let notes = vec![Note::new(4.0, 40, 1.0, 100, 0.0), Note::new(4.0, 41, 1.0, 100, 0.0)];
    let harmony = Harmony {
        chord: ChordSettings { seventh: true, ..chord_mode().chord },
        ..chord_mode()
    };
    let chord = chord_for_cell(&notes, 4.0, C, &harmony, 100, BAND);
    assert_eq!(chord.iter().map(|c| c.pitch).collect::<Vec<_>>(), [60, 64]);
}

#[test]
fn a_full_step_offers_no_chord_at_all() {
    // Which is what makes the ghost the report: nothing is drawn, so nothing is
    // promised. The roll falls back to its ordinary one-note click, as
    // `js/pianoroll.js` does when `getChord` comes back empty.
    let notes: Vec<Note> =
        (40..44).map(|pitch| Note::new(4.0, pitch, 1.0, 100, 0.0)).collect();
    assert!(chord_for_cell(&notes, 4.0, C, &chord_mode(), 100, BAND).is_empty());
    // The step next door is untouched by any of it.
    assert_eq!(chord_for_cell(&notes, 5.0, C, &chord_mode(), 100, BAND).len(), 3);
}

// --------------------------------------------------------------------- harmonise

#[test]
fn harmonising_builds_a_chord_under_each_selected_note() {
    let mut notes = vec![Note::new(0.0, 60, 2.0, 100, 0.0), Note::new(4.0, 62, 1.0, 90, 0.0)];
    let selected: Vec<u32> = notes.iter().map(|n| n.id).collect();
    let harmony = Harmony { scale: Some(Scale::Major), ..chord_mode() };
    let harmony = Harmony {
        chord: ChordSettings { quality: QualityChoice::InScale, ..harmony.chord },
        ..harmony
    };

    let out = harmonise(&mut notes, &selected, &harmony, BAND);

    assert_eq!(out.sources, 2);
    assert_eq!(out.added.len(), 4, "two notes, two chord tones each");
    assert_eq!(out.already_there, 0);
    assert_eq!(out.over_cap, 0);

    // The melody is exactly as it was drawn.
    let melody = notes.iter().find(|n| n.id == selected[0]).unwrap();
    assert_eq!((melody.pitch, melody.velocity, melody.len), (60, 100, 2.0));

    // Under it, the rest of the I chord: this note's own length, and softer.
    let under: Vec<(u8, u8, f64)> = notes
        .iter()
        .filter(|n| n.step == 0.0 && n.id != selected[0])
        .map(|n| (n.pitch, n.velocity, n.len))
        .collect();
    assert_eq!(under, [(64, 85, 2.0), (67, 85, 2.0)]);

    // And the ii chord under the D, at that note's own velocity.
    let under: Vec<(u8, u8)> = notes
        .iter()
        .filter(|n| n.step == 4.0 && n.id != selected[1])
        .map(|n| (n.pitch, n.velocity))
        .collect();
    assert_eq!(under, [(65, 77), (69, 77)]);
}

#[test]
fn a_harmonised_note_carries_the_melody_notes_trig_conditions() {
    // PROB/FILL/COND are per trig on the box, so every note sharing a step has to
    // agree. The melody note is the incumbent and the chord-mates join it.
    let mut melody = Note::new(0.0, 60, 1.0, 100, 0.0);
    melody.cond = Some(String::from("1:2"));
    melody.prob = Some(60);
    let id = melody.id;
    let mut notes = vec![melody];

    harmonise(&mut notes, &[id], &chord_mode(), BAND);

    assert_eq!(notes.len(), 3);
    for note in &notes {
        assert_eq!(note.cond.as_deref(), Some("1:2"));
        assert_eq!(note.prob, Some(60));
    }
}

#[test]
fn the_strum_rides_on_the_melody_notes_own_micro_timing() {
    let melody = Note::new(0.0, 60, 1.0, 100, 0.2);
    let id = melody.id;
    let mut notes = vec![melody];
    let harmony = Harmony {
        chord: ChordSettings { strum: 3, ..chord_mode().chord },
        ..chord_mode()
    };

    harmonise(&mut notes, &[id], &harmony, BAND);

    let micros: Vec<f64> = notes.iter().map(|n| n.micro).collect();
    // 0.2 as drawn, then the stagger added to it — three ticks apart, as the
    // control counts them.
    assert_eq!(micros, [0.2, 0.325, 0.45]);
}

#[test]
fn a_harmonised_micro_offset_stays_inside_the_clamp() {
    let melody = Note::new(0.0, 60, 1.0, 100, MICRO_MAX);
    let id = melody.id;
    let mut notes = vec![melody];
    let harmony = Harmony {
        chord: ChordSettings { strum: 3, ..chord_mode().chord },
        ..chord_mode()
    };

    harmonise(&mut notes, &[id], &harmony, BAND);

    assert!(notes.iter().all(|n| n.micro <= MICRO_MAX));
}

#[test]
fn harmonising_never_puts_one_pitch_on_a_step_twice() {
    // **The JS bug this closes.** `js/main.js` tests for a collision against the
    // pattern while its own additions are still in a separate array, so a chord tone
    // one selected note added is invisible to the next.
    //
    // **In-scale mode, and that is the whole point of the setup** — found by the
    // deliberate-bug pass, which planted the JS's own version of the check and was
    // not caught. With a *fixed* quality the two chords under C and E are 60-64-67
    // and 64-68-71, which share nothing, so the bug cannot show. Diatonic thirds
    // overlap by design: the iii chord is 64-67-71 and the 67 is exactly the note the
    // I chord just added. The test that names this bug has to be in the mode that
    // reaches it.
    let harmony = Harmony {
        scale: Some(Scale::Major),
        chord: ChordSettings {
            on: true,
            quality: QualityChoice::InScale,
            ..ChordSettings::default()
        },
        ..Harmony::default()
    };
    let mut notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0), Note::new(0.0, 64, 1.0, 100, 0.0)];
    let selected: Vec<u32> = notes.iter().map(|n| n.id).collect();

    let out = harmonise(&mut notes, &selected, &harmony, BAND);

    let mut pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
    pitches.sort_unstable();
    let unique = {
        let mut p = pitches.clone();
        p.dedup();
        p
    };
    assert_eq!(pitches, unique, "no pitch appears twice on the step");
    // 60 and 64 were drawn. Under the C: 64 is already there, 67 is added. Under the
    // E: 67 is already there *because this pass added it*, and 71 is added. In the JS
    // the 67 would be added a second time.
    assert_eq!(pitches, [60, 64, 67, 71]);
    assert_eq!(out.added.len(), 2);
    assert_eq!(out.already_there, 2, "the collisions are reported, not swallowed");
    assert_eq!(out.over_cap, 0);
}

#[test]
fn a_key_root_from_a_hand_edited_file_is_still_a_pitch_class() {
    // The panel offers twelve roots and nothing else, so this is unreachable from the
    // app — but a project file is JSON and a person can type into it, which is the
    // same argument `core::export` makes about a step no byte can name. Found by the
    // deliberate-bug pass: dropping the mask broke nothing, because nothing here had
    // ever handed it a root above eleven.
    let wild = Harmony { root: 14, scale: Some(Scale::Major), ..Harmony::default() };
    let sane = Harmony { root: 2, scale: Some(Scale::Major), ..Harmony::default() };
    assert_eq!(wild.key().unwrap().root, 2);
    for pitch in [50, 62, 65, 66] {
        assert_eq!(wild.row(pitch), sane.row(pitch), "row {pitch}");
    }
    // And the voicing it builds is the one that key would build.
    let of = |h: &Harmony| -> Vec<u8> {
        h.specs(62, 100, false, BAND).iter().map(|c| c.pitch).collect()
    };
    assert_eq!(of(&wild), of(&sane));
}

#[test]
fn harmonising_keeps_the_step_inside_the_trigs_four_notes_and_says_what_it_dropped() {
    // Three notes drawn on one step, one of them selected: the chord under it
    // would be two more, and only one fits.
    let mut notes = vec![
        Note::new(0.0, 60, 1.0, 100, 0.0),
        Note::new(0.0, 40, 1.0, 100, 0.0),
        Note::new(0.0, 41, 1.0, 100, 0.0),
    ];
    let id = notes[0].id;

    let out = harmonise(&mut notes, &[id], &chord_mode(), BAND);

    assert_eq!(notes.len(), MAX_CHORD_NOTES);
    assert_eq!(out.added.len(), 1);
    assert_eq!(out.over_cap, 1, "the note the encoder would have dropped is reported here");
}

#[test]
fn harmonising_nothing_adds_nothing() {
    let mut notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];
    let out = harmonise(&mut notes, &[], &chord_mode(), BAND);
    assert!(out.is_empty());
    assert_eq!(out.sources, 0);
    assert_eq!(notes.len(), 1);
    // A selection can outlive the note it named — a stale id is skipped, not a
    // panic and not a chord under nothing.
    let out = harmonise(&mut notes, &[9_999_999], &chord_mode(), BAND);
    assert!(out.is_empty());
    assert_eq!(notes.len(), 1);
}

#[test]
fn a_chord_tone_outside_the_roll_is_dropped_rather_than_folded_back_in() {
    // The band the app passes is the roll's own, and a note it cannot draw is a
    // note nobody could then edit.
    let melody = Note::new(0.0, 95, 1.0, 100, 0.0);
    let id = melody.id;
    let mut notes = vec![melody];

    let out = harmonise(&mut notes, &[id], &chord_mode(), BAND);

    assert!(out.is_empty(), "a major triad on 95 has nothing under 96 but its own root");
    assert_eq!(notes.len(), 1);
}
