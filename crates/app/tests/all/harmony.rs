//! The Harmony panel's decisions, driven without a window — and the two claims
//! that only exist once a panel, a roll and a session are in the same room.
//!
//! `core`'s `tests/all/chords.rs` owns the pitch math and what `harmonise` does to a
//! note array. What is here is everything that needs the app:
//!
//!   * the harmonise button aiming at the *selected* track of the *selected* box,
//!     and putting the added notes into the roll's selection;
//!   * the key surviving a save and an open, which is why it is on `Session` at
//!     all;
//!   * an undo taking a harmonise back **and leaving the key alone** — the line
//!     `core::history` draws, seen from the outside;
//!   * and the status line's wording, which is the only report either omission
//!     gets.
//!
//! What no test in this file can claim: that the tint is *visible* on a screen, or
//! that the ghost is legible under a cursor. Both are run-the-app-and-look checks,
//! like the glyph table in `ui::mod`.

use digi_core::chords::{ChordSettings, Harmonised, Harmony, Quality, QualityChoice, Scale};
use digi_core::history::{Content, History};
use digi_core::{two_box_session, Note, Project, Session};
use digi_roll_studio::ui::harmony::{harmonise_message, HarmonyPanel, Status};
use digi_roll_studio::ui::pianoroll::PianoRoll;
use digi_roll_studio::ui::tracks::{track, track_mut, Selection};

/// A session with a key set, chord draw on, and one note on the DT2's track 0.
fn session_with_a_melody() -> (Session, Selection, PianoRoll) {
    let mut session = two_box_session();
    session.harmony = Harmony {
        root: 0,
        scale: Some(Scale::Major),
        chord: ChordSettings {
            on: true,
            quality: QualityChoice::InScale,
            ..ChordSettings::default()
        },
    };
    let selection = Selection { device: 0, track: 0 };
    let mut roll = PianoRoll::default();
    let melody = Note::new(0.0, 60, 1.0, 100, 0.0);
    let id = melody.id;
    track_mut(&mut session, selection).unwrap().notes.push(melody);
    roll.select([id]);
    (session, selection, roll)
}

#[test]
fn the_button_harmonises_the_selected_track_of_the_selected_box() {
    let (mut session, selection, mut roll) = session_with_a_melody();
    let mut panel = HarmonyPanel::default();

    assert!(panel.harmonise_selection(&mut session, selection, &mut roll));

    let notes = &track(&session, selection).unwrap().notes;
    let mut pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
    pitches.sort_unstable();
    assert_eq!(pitches, [60, 64, 67], "the I chord of C major, under the melody note");

    // **The added notes join the melody in the selection**, which is what lets an
    // immediate drag move the whole harmonised phrase — `js/main.js`'s
    // `setSelection([...selected, ...added])`.
    assert_eq!(roll.selection().len(), 3);

    // And nothing reached the other box, or any other track of this one.
    let empty = |device: &digi_core::Device| {
        device
            .patterns
            .iter()
            .all(|p| p.tracks().iter().all(|t| t.notes.is_empty()))
    };
    assert!(empty(&session.devices[1]), "the DN2 was not touched");
    let dt2 = &session.devices[0];
    assert!(dt2.pattern(0).unwrap().tracks()[1..].iter().all(|t| t.notes.is_empty()));
}

#[test]
fn harmonising_nothing_is_refused_with_a_reason_rather_than_silently() {
    let (mut session, selection, mut roll) = session_with_a_melody();
    roll.clear_selection();
    let mut panel = HarmonyPanel::default();

    assert!(!panel.harmonise_selection(&mut session, selection, &mut roll));

    assert_eq!(
        panel.status(),
        Some(&Status::Failed(String::from("Select some notes to harmonise first")))
    );
    assert_eq!(track(&session, selection).unwrap().notes.len(), 1);
}

#[test]
fn pressing_harmonise_again_stops_at_the_four_notes_a_trig_holds() {
    // **The button is safe to lean on**, which is worth pinning because the
    // selection grows every time: after one press the whole chord is selected, so
    // the next press builds under three notes rather than one. It stacks up to the
    // trig's ceiling and then stops, rather than piling on notes the encoder would
    // drop at a write.
    let (mut session, selection, mut roll) = session_with_a_melody();
    let mut panel = HarmonyPanel::default();

    panel.harmonise_selection(&mut session, selection, &mut roll);
    assert_eq!(track(&session, selection).unwrap().notes.len(), 3);

    // The second press: the chord's own upper extension fits, and nothing else.
    assert!(panel.harmonise_selection(&mut session, selection, &mut roll));
    assert_eq!(track(&session, selection).unwrap().notes.len(), 4);

    // The third adds nothing at all, and says which of the two reasons it is.
    let added = panel.harmonise_selection(&mut session, selection, &mut roll);
    assert!(!added, "nothing was added, so the shell opens no history step");
    assert_eq!(track(&session, selection).unwrap().notes.len(), 4);
    match panel.status() {
        Some(Status::Harmonised(out)) => {
            assert!(out.is_empty());
            assert!(out.over_cap > 0, "the step is full, which is not the same as nothing to add");
            assert!(harmonise_message(out).contains("four notes a trig can carry"));
        }
        other => panic!("expected a harmonised status, got {other:?}"),
    }
}

#[test]
fn the_key_and_the_chord_settings_survive_a_save_and_an_open() {
    // **Why the settings are on `Session` and not in the panel.** They are saved
    // with the session, and Phase 7's Generate panel edits the same key.
    let (session, _, _) = session_with_a_melody();
    let json = Project::new(session.clone()).to_json_pretty().unwrap();
    let back = Project::from_json(&json).unwrap().session;
    assert_eq!(back.harmony, session.harmony);
}

#[test]
fn an_undo_takes_a_harmonise_back_and_leaves_the_key_alone() {
    // The line `core::history` draws, from the outside: a harmonise is music and
    // undoes; a key change is where you are sitting and does not. Which means an
    // undo of a harmonise must not also revert a key chosen after it.
    let (mut session, selection, mut roll) = session_with_a_melody();
    let mut panel = HarmonyPanel::default();
    let mut history = History::default();

    history.begin(Content::of(&session));
    panel.harmonise_selection(&mut session, selection, &mut roll);
    history.commit(&session);
    assert_eq!(track(&session, selection).unwrap().notes.len(), 3);

    // A key change after the fact, with no step opened around it — which is what
    // the shell does, because `Content` compares patterns and this changes none.
    session.harmony.root = 5;
    session.harmony.scale = Some(Scale::Dorian);

    assert!(history.undo(&mut session));

    assert_eq!(track(&session, selection).unwrap().notes.len(), 1, "the chord went");
    assert_eq!(session.harmony.root, 5, "and the key stayed");
    assert_eq!(session.harmony.scale, Some(Scale::Dorian));
}

#[test]
fn the_status_line_names_both_kinds_of_omission() {
    // The only report either omission gets. `already_there` is a pitch the step
    // had; `over_cap` is a note past the four a trig holds — the one the encoder
    // would otherwise have dropped for us, silently, at a write.
    let plain = Harmonised { added: vec![1, 2], sources: 1, ..Harmonised::default() };
    assert_eq!(harmonise_message(&plain), "Harmonised 1 note — added 2");

    let both = Harmonised {
        added: vec![1],
        sources: 2,
        already_there: 1,
        over_cap: 2,
    };
    assert_eq!(
        harmonise_message(&both),
        "Harmonised 2 notes — added 1 · 1 note already on the step · 2 left out: a trig holds four notes"
    );

    let nothing = Harmonised { sources: 3, already_there: 6, ..Harmonised::default() };
    assert_eq!(
        harmonise_message(&nothing),
        "Nothing to add — those chords are already there · 6 notes already on the step"
    );

    // And the other empty case, which is a hardware limit rather than a no-op — said
    // once, not twice.
    let full = Harmonised { sources: 1, over_cap: 2, ..Harmonised::default() };
    assert_eq!(
        harmonise_message(&full),
        "Nothing to add — those steps already hold the four notes a trig can carry"
    );
}

#[test]
fn a_harmonise_over_a_track_the_roll_cannot_draw_is_still_bounded_by_the_roll() {
    // The band is the *track's* — C2–C8 widened to whatever the track already
    // carries — because a fetched pattern can hold a pitch outside the default
    // rows. A chord under such a note has somewhere to go, and it must be inside
    // the band the roll will draw.
    let mut session = two_box_session();
    session.harmony = Harmony {
        chord: ChordSettings {
            on: true,
            quality: QualityChoice::Fixed(Quality::Major),
            ..ChordSettings::default()
        },
        ..Harmony::default()
    };
    let selection = Selection { device: 0, track: 0 };
    let mut roll = PianoRoll::default();
    // Pitch 20 is below C2, which only an import or a load can put here.
    let low = Note::new(0.0, 20, 1.0, 100, 0.0);
    let id = low.id;
    track_mut(&mut session, selection).unwrap().notes.push(low);
    roll.select([id]);

    let mut panel = HarmonyPanel::default();
    assert!(panel.harmonise_selection(&mut session, selection, &mut roll));

    let track = track(&session, selection).unwrap();
    let (lo, hi) = PianoRoll::band(track);
    for note in &track.notes {
        assert!(
            (lo..=hi).contains(&note.pitch),
            "pitch {} is outside the rows the roll draws ({lo}..={hi})",
            note.pitch
        );
    }
}
