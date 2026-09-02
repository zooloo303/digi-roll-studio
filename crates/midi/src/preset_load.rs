//! Putting one +Drive preset onto one kit track — PLAN.md §10.6 step 6, and
//! the only write in this codebase that is not [`digi_protocol::safe_write`].
//!
//! [`load_preset_onto_track`] is five round trips: read the track, read the
//! file, send the payload, read the track twice. **The gen-2 half of the
//! feature**: the Analog Four reaches a kit track through its whole kit and has
//! its own module, [`crate::a4_preset_load`], which documents what differs and
//! why the panel above both did not have to change. `preset_scan` is the sibling
//! module and the shape is deliberately the same — a trait so the decisions can
//! be tested without a box, and the loop that matters kept out of the panel.
//!
//! # Why this is not `safe_write_tracks`, and what replaces each of its steps
//!
//! Rule 1's ceremony is a re-fetch, a confirm, a stash, a send and a read-back,
//! and it is built on a **slot**: something that can be fetched now and written
//! back later. The active kit is a **working buffer**. There is no `0x50` that
//! puts one back, so three of those five steps have nothing to act on and the
//! remaining two are not enough on their own.
//!
//! What actually makes an audition recoverable, in the order it matters:
//!
//! 1. **The box discards an unsaved kit when the pattern is reloaded.** Press a
//!    pattern button and the stored kit returns. That is hardware behaviour,
//!    it is the real undo here, and no code in this crate can improve on it.
//!    A panel built on this has to say so plainly rather than imply a backup it
//!    does not have — PLAN.md §10.4.
//! 2. **The track's own bytes come back in [`LoadReport::backup`]**, because
//!    they had to be read anyway (see below). A caller that keeps the first one
//!    it is handed per track can put that track back with [`revert_track`]
//!    without a single extra round trip having been spent on the backup.
//! 3. **Nothing is sent that was not read back and decoded first.**
//!
//! # The pre-read is the size check, and the size check is the point
//!
//! `drive::preset_load_payload` can say that a file's payload is well-formed.
//! It cannot say that *this box* wants a payload of that length, because it
//! never sees the box. Every measurement so far says a preset payload and a
//! `0x6b` reply are the same length on a given box — DT2 1,114, DN2 364 — but
//! two boxes is not a proof, and the failure mode of being wrong is a store
//! opcode carrying a length the box did not expect.
//!
//! So the track is read **before** the file is sent, and the box's own reply is
//! the witness the lengths are compared against. That read is free: its bytes
//! are the backup, and its length is the check.
//!
//! # Verification reads twice, and that is not belt-and-braces
//!
//! A box with MIDI thru enabled echoes what this end sends. A single read after
//! a store can therefore return our own message coming home, which reads as a
//! success and is the one false answer a load must never give — it would report
//! a preset loaded onto a box that never heard it. `examples/probe_sound_store`
//! established the defence and this module keeps it: `fetch_kit_track_sound`
//! drains before it sends, so a stray cannot be seen twice, and two reads
//! agreeing is the box rather than the cable.

use std::time::Duration;

use digi_protocol::drive::{decode_drive_preset, preset_load_payload, DriveError};
use digi_protocol::sound::{decode_sound_dump, SoundError, SOUND_WRAPPER};

use crate::{ElektronDevice, MidiError, KIT_TRACKS};

/// How long to leave the box alone after a store before reading it back.
///
/// On top of the [`crate::device`] pacing's own settle, and for the same reason
/// the probe carries it: a read that races the store reports a false negative,
/// and a false negative here tells a user their preset did not load when it
/// did. Whichever way this is wrong it is wrong about the box.
const AFTER_STORE: Duration = Duration::from_millis(400);

/// The three things a load needs from a box.
///
/// **A trait so the decisions can be tested without hardware**, which is the
/// same argument [`crate::preset_scan::PresetSource`] makes and it applies
/// harder here: this is a *write* path, so the branches that most need pinning
/// are precisely the ones nobody wants to reach by experiment on a real kit.
pub trait KitTrackIo {
    /// One preset file's bytes off the +Drive.
    fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError>;
    /// One track's sound out of the active kit — a `0x6b` reply, wrapper first.
    fn read_track_sound(&mut self, track: u8) -> Result<Vec<u8>, MidiError>;
    /// Put a payload onto a track of the active kit — the `0x5b` store.
    fn write_track_sound(&mut self, track: u8, payload: &[u8]) -> Result<(), MidiError>;
    /// Wait for the box to digest a store. Separated so a test runs instantly
    /// and a desk waits [`AFTER_STORE`].
    fn settle(&mut self) {
        std::thread::sleep(AFTER_STORE);
    }
}

impl KitTrackIo for ElektronDevice {
    fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError> {
        self.drive_read_file(path)
    }

    fn read_track_sound(&mut self, track: u8) -> Result<Vec<u8>, MidiError> {
        self.fetch_kit_track_sound(track)
    }

    fn write_track_sound(&mut self, track: u8, payload: &[u8]) -> Result<(), MidiError> {
        self.store_kit_track_sound(track, payload)
    }
}

/// What a finished load leaves the caller holding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadReport {
    /// The preset's name, as the file spells it.
    pub loaded: String,
    /// What the track held before — for a note that says what was displaced,
    /// which is the one thing a user cannot get back by looking.
    pub replaced: String,
    /// The track's bytes as they were before the store: a `0x6b` payload, ready
    /// to hand straight back to [`revert_track`].
    ///
    /// **Read for the size check and kept because it was already in hand.** A
    /// caller that keeps only the *first* of these per track has audition
    /// mode's backup at no round-trip cost at all — see the module doc.
    pub backup: Vec<u8>,
}

/// How a load failed, in terms of the thing that refused it.
#[derive(Debug)]
pub enum LoadError {
    /// The +Drive read failed, or the store did.
    Wire(MidiError),
    /// The preset file is not one this box's kit can be handed — the mk1
    /// container, an A4's, and the malformed. Carries `drive`'s own words.
    ///
    /// An A4 file reaching here is the wrong *box*, not a dead end: that
    /// container loads onto an A4 through [`crate::a4_preset_load`].
    Preset(DriveError),
    /// The track's current sound did not decode, so there is no backup and no
    /// length to check against.
    ///
    /// **Refused rather than worked around.** A track this end cannot read is a
    /// track it cannot put back, and a store onto one would be the single
    /// irreversible act in this codebase.
    UnreadableTrack { track: u8, why: SoundError },
    /// The payload and the box's own reply are different lengths.
    ///
    /// The check the module doc is about. Every box measured says these agree;
    /// a box that says otherwise has found something nobody has mapped, and the
    /// honest response is to stop and report the two numbers.
    LengthMismatch { track: u8, preset: usize, box_says: usize },
    /// `track` is outside a kit.
    NoSuchTrack { track: u8 },
    /// The store went out and the track does not read back as the preset.
    ///
    /// Not treated as a maybe: the box was written to and did not end up where
    /// it was asked to be, so the caller is told to reload the pattern rather
    /// than left to assume it worked.
    NotVerified { track: u8, expected: String, found: String },
    /// Two reads of the same track disagreed, which is this end's own message
    /// echoing back rather than the box answering.
    Echo { track: u8, first: String, second: String },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wire(e) => write!(f, "{e}"),
            Self::Preset(e) => write!(f, "{e}"),
            Self::UnreadableTrack { track, why } => write!(
                f,
                "track {} does not read back as a sound ({why}), so it could not be backed \
                 up — refusing to load onto a track that cannot be put back",
                track + 1
            ),
            Self::LengthMismatch { track, preset, box_says } => write!(
                f,
                "this preset is {preset} bytes and track {} answers with {box_says} — \
                 refusing to store a length this box has never been measured wanting",
                track + 1
            ),
            Self::NoSuchTrack { track } => {
                write!(f, "a kit has tracks 1-{KIT_TRACKS}, and this is track {}", track + 1)
            }
            Self::NotVerified { track, expected, found } => write!(
                f,
                "track {} reads {found:?} and not {expected:?} — the store went out and did \
                 not take. Reload the pattern on the box, and do not save it",
                track + 1
            ),
            Self::Echo { track, first, second } => write!(
                f,
                "two reads of track {} disagree ({first:?} then {second:?}) — that is this \
                 app's own message coming home rather than the box, so nothing here can be \
                 believed. Turn MIDI thru off on this box",
                track + 1
            ),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<MidiError> for LoadError {
    fn from(e: MidiError) -> Self {
        Self::Wire(e)
    }
}

impl From<DriveError> for LoadError {
    fn from(e: DriveError) -> Self {
        Self::Preset(e)
    }
}

/// Read one track's sound and name it, refusing an answer that is our own echo.
///
/// Two reads, because one cannot tell the box from MIDI thru — see the module
/// doc. Returns the name and the bytes, since every caller here wants both.
fn read_track_name(io: &mut impl KitTrackIo, track: u8) -> Result<(String, Vec<u8>), LoadError> {
    let mut seen: Vec<(String, Vec<u8>)> = Vec::new();
    for _ in 0..2 {
        let payload = io.read_track_sound(track)?;
        let sound = decode_sound_dump(payload.get(SOUND_WRAPPER..).unwrap_or(&[]))
            .map_err(|why| LoadError::UnreadableTrack { track, why })?;
        seen.push((sound.name, payload));
    }
    let (second, payload) = seen.pop().expect("two reads");
    let (first, _) = seen.pop().expect("two reads");
    if first != second {
        return Err(LoadError::Echo { track, first, second });
    }
    Ok((second, payload))
}

/// Put the preset at `path` onto `track` of the box's active kit.
///
/// The order is the whole design and every step of it is doing work the next
/// one depends on:
///
/// 1. **Read the track.** Its bytes are the backup and its length is the check.
/// 2. **Read the preset file** and cut its payload out —
///    [`preset_load_payload`], which refuses anything that is not this box's own
///    container format.
/// 3. **Compare the two lengths**, which is the only place the box gets a say in
///    whether the payload is the shape it wants.
/// 4. **Store**, then settle.
/// 5. **Read back twice** and check the name is the preset's.
///
/// See the module doc for what recovery means here, and say it to the user: it
/// is the box discarding an unsaved kit on pattern reload, plus
/// [`LoadReport::backup`], and it is not a stored slot that can be restored.
pub fn load_preset_onto_track(
    io: &mut impl KitTrackIo,
    path: &str,
    track: u8,
) -> Result<LoadReport, LoadError> {
    // Ahead of every read, so a bad index costs no round trip. The store checks
    // it again at the port, where it is a guard rather than a courtesy.
    if track >= KIT_TRACKS {
        return Err(LoadError::NoSuchTrack { track });
    }

    let (replaced, backup) = read_track_name(io, track)?;

    let file = io.read_preset(path)?;
    let payload = preset_load_payload(&file)?;
    // Decoded for its name, not for validation — `preset_load_payload` has
    // already decoded it and refused if it did not.
    let loaded = decode_drive_preset(&file)?.name;

    if payload.len() != backup.len() {
        return Err(LoadError::LengthMismatch {
            track,
            preset: payload.len(),
            box_says: backup.len(),
        });
    }

    io.write_track_sound(track, payload)?;
    io.settle();

    let (found, _) = read_track_name(io, track)?;
    if found != loaded {
        return Err(LoadError::NotVerified { track, expected: loaded, found });
    }
    Ok(LoadReport { loaded, replaced, backup })
}

/// Put a track's own bytes back — audition mode's undo, for a caller holding a
/// [`LoadReport::backup`].
///
/// Returns the name the track reads afterwards, verified the same way a load
/// is. **The bytes are sent exactly as the box gave them**: a revert composes
/// nothing and reinterprets nothing, which is what makes it the one operation
/// here that cannot introduce a new mistake.
///
/// This is *not* a substitute for reloading the pattern on the box. It restores
/// what this app saw before its own first load; anything else that has touched
/// the kit since — a knob, another app, the box's own browser — it neither knows
/// about nor claims to undo.
pub fn revert_track(
    io: &mut impl KitTrackIo,
    track: u8,
    backup: &[u8],
) -> Result<String, LoadError> {
    if track >= KIT_TRACKS {
        return Err(LoadError::NoSuchTrack { track });
    }
    let expected = decode_sound_dump(backup.get(SOUND_WRAPPER..).unwrap_or(&[]))
        .map_err(|why| LoadError::UnreadableTrack { track, why })?
        .name;

    io.write_track_sound(track, backup)?;
    io.settle();

    let (found, _) = read_track_name(io, track)?;
    if found != expected {
        return Err(LoadError::NotVerified { track, expected, found });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A box made of the committed captures, with a kit whose tracks hold real
    /// `0x6b`-shaped payloads.
    ///
    /// The same trick `preset_scan`'s tests use: a preset file's payload *is* a
    /// `0x6b` payload, so one fixture serves as both the +Drive file and the
    /// track's current sound. That is not a convenience — it is the finding
    /// `drive::preset_load_payload` rests on, and building the fake box out of
    /// it means a test would notice if it stopped being true.
    struct FakeBox {
        tracks: Vec<Vec<u8>>,
        /// What the +Drive answers, by path.
        files: Vec<(String, Vec<u8>)>,
        /// Every payload this box was asked to store, in order.
        stored: Vec<(u8, Vec<u8>)>,
        /// Set to make the store land nowhere, as a box with the pattern
        /// reloaded under it would.
        deaf: bool,
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../protocol/tests/fixtures/drive")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    /// A DN2 preset's payload — the bytes a track holds and the bytes a load
    /// sends, which on this format are the same shape.
    fn payload_of(name: &str) -> Vec<u8> {
        let file = fixture(name);
        preset_load_payload(&file).expect("a native preset").to_vec()
    }

    const HIDDEN: &str = "digitone2-soundbanks-A-1-HIDDEN-TEARS-2026-08-29.bin";
    const MONOLOW: &str = "digitone2-soundbanks-A-2-MONOLOW-2026-08-29.bin";
    const SEVENTH: &str = "digitone2-soundbanks-A-6-7THPAD-2026-08-29.bin";
    const ORGANIC: &str = "digitone2-soundbanks-C-1-ORGANIC-2026-08-29.bin";
    const ACIDD: &str = "digitakt2-soundbanks-A-1-ACIDD-2026-08-29.bin";

    impl FakeBox {
        /// A DN2 whose sixteen tracks all hold MONOLOW, with two presets on its
        /// +Drive.
        fn dn2() -> Self {
            Self {
                tracks: vec![payload_of(MONOLOW); KIT_TRACKS as usize],
                files: vec![
                    ("/soundbanks/A/1".into(), fixture(HIDDEN)),
                    ("/soundbanks/A/6".into(), fixture(SEVENTH)),
                    ("/soundbanks/C/1".into(), fixture(ORGANIC)),
                    // A DT2 preset on a DN2's drive: impossible in life, and
                    // the shortest way to a length mismatch in a test.
                    ("/soundbanks/A/9".into(), fixture(ACIDD)),
                ],
                stored: Vec::new(),
                deaf: false,
            }
        }
    }

    impl KitTrackIo for FakeBox {
        fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError> {
            self.files
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| MidiError::Send(format!("{path}: no such file")))
        }

        fn read_track_sound(&mut self, track: u8) -> Result<Vec<u8>, MidiError> {
            self.tracks.get(track as usize).cloned().ok_or(MidiError::Timeout)
        }

        fn write_track_sound(&mut self, track: u8, payload: &[u8]) -> Result<(), MidiError> {
            self.stored.push((track, payload.to_vec()));
            if !self.deaf {
                self.tracks[track as usize] = payload.to_vec();
            }
            Ok(())
        }

        fn settle(&mut self) {}
    }

    fn name_on(box_: &mut FakeBox, track: u8) -> String {
        read_track_name(box_, track).expect("a decodable track").0
    }

    #[test]
    fn a_load_puts_the_preset_on_the_track_and_says_what_it_displaced() {
        let mut dn2 = FakeBox::dn2();

        let report = load_preset_onto_track(&mut dn2, "/soundbanks/A/1", 3).unwrap();

        assert_eq!(report.loaded, "HIDDEN TEARS");
        assert_eq!(report.replaced, "MONOLOW");
        assert_eq!(name_on(&mut dn2, 3), "HIDDEN TEARS");
    }

    /// The bytes that go out are the file's payload, unaltered — the claim
    /// `preset_load_payload` makes, checked at the point it reaches a wire.
    #[test]
    fn what_is_stored_is_the_file_s_payload_verbatim() {
        let mut dn2 = FakeBox::dn2();

        load_preset_onto_track(&mut dn2, "/soundbanks/A/6", 0).unwrap();

        assert_eq!(dn2.stored.len(), 1, "one store per load");
        assert_eq!(dn2.stored[0], (0, payload_of(SEVENTH)));
    }

    /// Only the track asked for is touched. Worth pinning rather than assuming:
    /// this is a write, and the fifteen tracks nobody mentioned are somebody's
    /// work.
    #[test]
    fn no_other_track_moves() {
        let mut dn2 = FakeBox::dn2();

        load_preset_onto_track(&mut dn2, "/soundbanks/A/1", 7).unwrap();

        for track in 0..KIT_TRACKS {
            let expect = if track == 7 { "HIDDEN TEARS" } else { "MONOLOW" };
            assert_eq!(name_on(&mut dn2, track), expect, "track {}", track + 1);
        }
    }

    /// The backup comes back from the load itself, and putting it back is
    /// exact. Audition mode's undo, at no round-trip cost of its own.
    #[test]
    fn the_backup_a_load_hands_back_restores_the_track() {
        let mut dn2 = FakeBox::dn2();
        let before = dn2.tracks[2].clone();

        let report = load_preset_onto_track(&mut dn2, "/soundbanks/A/1", 2).unwrap();
        assert_eq!(name_on(&mut dn2, 2), "HIDDEN TEARS");

        assert_eq!(revert_track(&mut dn2, 2, &report.backup).unwrap(), "MONOLOW");
        assert_eq!(dn2.tracks[2], before, "byte for byte, not merely by name");
    }

    /// Audition mode's actual shape: several loads onto one track, then one
    /// revert to where the panel found it. Recovery is to the state it opened
    /// in — not one step back through nineteen auditions.
    #[test]
    fn reverting_after_several_auditions_goes_back_to_the_first_state() {
        let mut dn2 = FakeBox::dn2();
        let opened_holding = dn2.tracks[5].clone();

        let first = load_preset_onto_track(&mut dn2, "/soundbanks/A/1", 5).unwrap();
        load_preset_onto_track(&mut dn2, "/soundbanks/A/6", 5).unwrap();
        load_preset_onto_track(&mut dn2, "/soundbanks/A/1", 5).unwrap();
        assert_eq!(name_on(&mut dn2, 5), "HIDDEN TEARS");

        revert_track(&mut dn2, 5, &first.backup).unwrap();
        assert_eq!(dn2.tracks[5], opened_holding);
    }

    /// A Digitone mk1 preset off the same box's +Drive is refused, and nothing
    /// is sent. It browses; it does not load.
    #[test]
    fn an_mk1_preset_is_refused_before_anything_is_sent() {
        let mut dn2 = FakeBox::dn2();

        let err = load_preset_onto_track(&mut dn2, "/soundbanks/C/1", 0).unwrap_err();

        assert!(
            matches!(err, LoadError::Preset(DriveError::NotTheBoxsOwnFormat { .. })),
            "{err}"
        );
        assert!(dn2.stored.is_empty(), "a refusal must not reach the wire");
        assert_eq!(name_on(&mut dn2, 0), "MONOLOW", "the track is untouched");
    }

    /// A payload the box's own reply says is the wrong length stops the load.
    /// The check that only the box can make, and the reason the track is read
    /// before the file is sent.
    #[test]
    fn a_payload_the_box_did_not_ask_for_is_not_stored() {
        let mut dn2 = FakeBox::dn2();

        let err = load_preset_onto_track(&mut dn2, "/soundbanks/A/9", 0).unwrap_err();

        match err {
            LoadError::LengthMismatch { track: 0, preset: 1114, box_says: 364 } => {}
            other => panic!("expected a length refusal, got {other}"),
        }
        assert!(dn2.stored.is_empty());
    }

    /// A store that did not take is reported as a failure, not as a load.
    #[test]
    fn a_store_that_does_not_land_is_not_reported_as_a_load() {
        let mut dn2 = FakeBox::dn2();
        dn2.deaf = true;

        let err = load_preset_onto_track(&mut dn2, "/soundbanks/A/1", 1).unwrap_err();

        match err {
            LoadError::NotVerified { track: 1, expected, found } => {
                assert_eq!(expected, "HIDDEN TEARS");
                assert_eq!(found, "MONOLOW");
            }
            other => panic!("expected a verify failure, got {other}"),
        }
    }

    /// A track outside the kit costs no round trip at all.
    #[test]
    fn a_track_outside_the_kit_is_refused_without_reading_anything() {
        let mut dn2 = FakeBox::dn2();

        let err = load_preset_onto_track(&mut dn2, "/soundbanks/A/1", KIT_TRACKS).unwrap_err();

        assert!(matches!(err, LoadError::NoSuchTrack { track: 16 }), "{err}");
        assert!(dn2.stored.is_empty());
    }

    /// MIDI thru, as the probe found it: the second read disagrees with the
    /// first, and the load says so instead of believing either.
    #[test]
    fn two_reads_disagreeing_is_reported_as_an_echo() {
        /// A box that answers a different track each time it is read.
        struct Echoing {
            names: Vec<Vec<u8>>,
            at: usize,
        }
        impl KitTrackIo for Echoing {
            fn read_preset(&mut self, _: &str) -> Result<Vec<u8>, MidiError> {
                Ok(fixture(HIDDEN))
            }
            fn read_track_sound(&mut self, _: u8) -> Result<Vec<u8>, MidiError> {
                let out = self.names[self.at % self.names.len()].clone();
                self.at += 1;
                Ok(out)
            }
            fn write_track_sound(&mut self, _: u8, _: &[u8]) -> Result<(), MidiError> {
                Ok(())
            }
            fn settle(&mut self) {}
        }

        let mut box_ =
            Echoing { names: vec![payload_of(MONOLOW), payload_of(SEVENTH)], at: 0 };

        let err = load_preset_onto_track(&mut box_, "/soundbanks/A/1", 0).unwrap_err();

        match err {
            LoadError::Echo { first, second, .. } => {
                assert_eq!(first, "MONOLOW");
                assert_eq!(second, "7THPAD");
            }
            other => panic!("expected an echo refusal, got {other}"),
        }
    }
}
