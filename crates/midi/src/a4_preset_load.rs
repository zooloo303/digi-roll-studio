//! Putting one +Drive preset onto one **Analog Four** track — the gen-1 twin of
//! [`crate::preset_load`], and the last thing the digis could do that this box
//! could not.
//!
//! [`load_a4_preset_onto_track`] is four round trips: read the kit, read the
//! file, send the kit back, read it twice. The panel above it is the same panel,
//! the gesture is the same double-click, and the recovery story is word for word
//! the same one. What differs is the object that moves, and that difference is
//! the whole module.
//!
//! # A digi addresses a track; this box only addresses a kit
//!
//! `preset_load` rests on a **pair** of messages: `0x6b` returns one kit track's
//! sound and `0x5b` puts one back. Nothing on the A4 does that. Its `0x6b` is
//! the working *pattern* — the index ignored, `0x65`'s twin — and for a day that
//! read as "this box has no load path", which is what the panel used to say in a
//! paragraph.
//!
//! The path it does have was already on disk. A kit dump (`0x62` stored, `0x68`
//! working) carries its four synth sounds as 350-byte `0xBEEFBABA` containers at
//! a fixed stride, and a +Drive preset file holds **the same container** —
//! `protocol::a4_kit`'s header has the offsets and the 512 containers they were
//! checked against. So a load is a read-modify-write: fetch the working kit,
//! replace 350 of its 2,410 bytes, send it back.
//!
//! Three consequences follow, and every one of them is a behaviour a user can
//! see:
//!
//! 1. **The pre-read is not optional and not merely a size check.** On a digi
//!    the pre-read is a courtesy that earns a backup and a length witness; skip
//!    it and you have a load with no undo. Here the fetched bytes are 2,060 of
//!    the 2,410 that get sent back, so a load without a pre-read would
//!    overwrite the other three sounds, the kit's name and the FX and CV tracks'
//!    settings with whatever this app last saw. There is no version of this
//!    that sends less than a whole kit.
//! 2. **Four tracks, not sixteen.** A kit holds SYN1–SYN4. The A4 sequences six
//!    tracks and the FX and CV tracks have no sound to put one on, so a
//!    selection pointing at T5 or T6 is refused by name — see
//!    [`A4LoadError::NoSuchTrack`], and `ui::presets::load_target` refuses it a
//!    layer earlier so nobody double-clicks into it.
//! 3. **The backup is free, again, and for a better reason.** `sound_slot`
//!    reads 350 bytes out of a kit this path had to fetch anyway. Keeping the
//!    first one per track gives [`revert_a4_track`] the same undo the digi path
//!    documents — back to what the track held when the auditioning started, not
//!    one step back through nineteen of them.
//!
//! # There is no length check here, and there is nothing missing
//!
//! `preset_load`'s central argument is that only the box can say what length
//! payload it wants, so the track is read before the file is sent and the box's
//! own reply is the witness. That check is **structural** on this box rather
//! than measured: the destination is a 350-byte stride inside a 2,410-byte
//! object, and `a4_kit::splice_sound` refuses anything that is not exactly 350
//! bytes with the head magic at one end and the foot magic at the other. A
//! payload the wrong length cannot be sent, because a kit containing it would
//! not be a kit.
//!
//! What replaces the digis' measurement is the **foot magic**, which is the one
//! witness that the 350 bytes were cut from the right place: a +Drive file
//! declares 366 bytes of payload and a kit slot takes 350, so the declaration
//! cannot be believed and `drive::a4_preset_sound` checks the foot landed where
//! a kit's does. `sound::A4_SOUND_MAGIC_FOOT` carries that argument in full.
//!
//! # Verification reads twice, and it is the same trap
//!
//! A `0x68` request is answered with a `0x58` dump, and a `0x58` dump is what a
//! store *is* — the identical collision `preset_load` documents for `0x6b`/`0x5b`
//! and defends the same way. A box with MIDI thru enabled echoes this end's own
//! store, and a single read can therefore return our own message coming home,
//! which reads as a success and is the one false answer a load must never give.
//! Two reads that agree are the box rather than the cable.
//!
//! # What is not written here
//!
//! A `0x52`. The working kit is the box's edit buffer, and the box's own undo —
//! reloading the pattern, which discards an unsaved kit — is what makes an
//! audition recoverable at all. Writing a *stored* kit slot would have no undo
//! whatsoever, so nothing in this workspace builds one; the panel says the
//! rest, every time, in the same words it says them to a DN2.

use std::time::Duration;

use digi_protocol::a4_kit::{
    read_kit, sound_for_kit, sound_slot, splice_sound, A4Kit, NUM_SOUNDS,
};
use digi_protocol::drive::{a4_preset_sound, decode_drive_preset, DriveError};

use crate::preset_load::LoadReport;
use crate::{ElektronDevice, MidiError};

/// How long to leave the box alone after a store before reading it back.
///
/// [`crate::preset_load`]'s own settle value and its argument, kept as its own
/// constant because it is a different box: a false negative here tells a user
/// their preset did not load when it did, and whichever way that is wrong it is
/// wrong about hardware nobody has measured this on.
const AFTER_STORE: Duration = Duration::from_millis(400);

/// The three things an A4 load needs from a box.
///
/// A trait for the reason [`crate::preset_load::KitTrackIo`] is one, and the
/// argument is stronger here: this write sends a whole kit, so the branch that
/// most needs pinning is the one where the splice lands in the wrong 350 bytes,
/// and nobody wants to reach that by experiment on a real kit.
pub trait A4KitIo {
    /// One preset file's bytes off the +Drive.
    fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError>;
    /// The box's **working** kit — a `0x58` reply's payload, 2,410 bytes.
    fn read_working_kit(&mut self) -> Result<Vec<u8>, MidiError>;
    /// Put a whole kit payload back into the box's edit buffer.
    fn write_working_kit(&mut self, payload: &[u8]) -> Result<(), MidiError>;
    /// Wait for the box to digest a store. Separated so a test runs instantly
    /// and a desk waits [`AFTER_STORE`].
    fn settle(&mut self) {
        std::thread::sleep(AFTER_STORE);
    }
}

impl A4KitIo for ElektronDevice {
    fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError> {
        self.drive_read_file(path)
    }

    fn read_working_kit(&mut self) -> Result<Vec<u8>, MidiError> {
        self.fetch_a4_working_kit()
    }

    fn write_working_kit(&mut self, payload: &[u8]) -> Result<(), MidiError> {
        self.store_a4_working_kit(payload)
    }
}

/// How an A4 load failed, in terms of the thing that refused it.
///
/// Deliberately its own enum rather than [`crate::preset_load::LoadError`]'s
/// variants reused: the two paths refuse different things — there is no length
/// mismatch here and no unreadable *track*, and there is a kit that does not
/// decode, which a digi has no equivalent of.
#[derive(Debug)]
pub enum A4LoadError {
    /// The +Drive read failed, the kit read did, or the store did.
    Wire(MidiError),
    /// The preset file is not an A4 sound container — a digi's or an mk1's file,
    /// or one whose layout does not hold. Carries `drive`'s own words.
    Preset(DriveError),
    /// The kit the box answered with does not decode, so there is nothing to
    /// splice into and no backup to take.
    ///
    /// **Refused rather than worked around**, exactly as an unreadable track is
    /// on a digi: a kit this end cannot read is a kit it cannot put back, and a
    /// whole-kit store built on one would be the single irreversible act in this
    /// workspace.
    UnreadableKit { why: String },
    /// The splice itself refused — a sound of the wrong length, a slot outside
    /// the four, a destination that is not a kit of the measured version.
    ///
    /// Every one of these is a bug rather than a user error, and it is carried
    /// through in the box's own terms anyway: this is the guard that stands
    /// between a wrong slice and a kit whose slot boundaries have moved.
    Unspliceable { why: String },
    /// `track` is not one of the four the kit has a sound for.
    NoSuchTrack { track: u8 },
    /// The store went out and the kit does not read back with the preset on
    /// that track.
    ///
    /// Not treated as a maybe. The box was written to and did not end up where
    /// it was asked to be, so the caller is told to reload the pattern rather
    /// than left to assume it worked.
    NotVerified { track: u8, expected: String, found: String },
    /// Two reads of the kit disagreed, which is this end's own message echoing
    /// back rather than the box answering.
    Echo { first: String, second: String },
}

impl std::fmt::Display for A4LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wire(e) => write!(f, "{e}"),
            Self::Preset(e) => write!(f, "{e}"),
            Self::UnreadableKit { why } => write!(
                f,
                "the kit this box is playing does not read back as a kit ({why}), so it could \
                 not be backed up — refusing to write a kit that cannot be put back"
            ),
            Self::Unspliceable { why } => write!(f, "refusing to build this kit: {why}"),
            Self::NoSuchTrack { track } => write!(
                f,
                "a kit holds {NUM_SOUNDS} sounds — SYN1 to SYN4 — and this is track {}. The FX \
                 and CV tracks sequence, and have no sound to load onto",
                track + 1
            ),
            Self::NotVerified { track, expected, found } => write!(
                f,
                "SYN{} reads {found:?} and not {expected:?} — the kit went out and did not \
                 take. Reload the pattern on the box, and do not save the kit",
                track + 1
            ),
            Self::Echo { first, second } => write!(
                f,
                "two reads of this box's kit disagree ({first:?} then {second:?}) — that is \
                 this app's own message coming home rather than the box, so nothing here can \
                 be believed. Turn MIDI thru off on this box"
            ),
        }
    }
}

impl std::error::Error for A4LoadError {}

impl From<MidiError> for A4LoadError {
    fn from(e: MidiError) -> Self {
        Self::Wire(e)
    }
}

impl From<DriveError> for A4LoadError {
    fn from(e: DriveError) -> Self {
        Self::Preset(e)
    }
}

/// Read the working kit and decode it, refusing an answer that is our own echo.
///
/// Two reads, because one cannot tell the box from MIDI thru — see the module
/// doc. Returns the decoded kit and the payload, since every caller here wants
/// both: the names to report and the bytes to splice.
///
/// **The two reads are compared by their whole payload**, not by a name. On a
/// digi the comparable object is one track's sound and its name is what a user
/// sees; here the object is a kit, the bytes are what the next step writes back,
/// and a difference anywhere in them means this end does not know what the box
/// is holding. The message names the kit and the track that differ so the report
/// is still readable.
fn read_kit_twice(io: &mut impl A4KitIo) -> Result<(A4Kit, Vec<u8>), A4LoadError> {
    let mut seen: Vec<Vec<u8>> = Vec::new();
    for _ in 0..2 {
        let payload = io.read_working_kit()?;
        // Decoded on the way in, so an undecodable answer is refused before it
        // can be compared, spliced into or sent.
        read_kit(0, &payload).map_err(|why| A4LoadError::UnreadableKit { why })?;
        seen.push(payload);
    }
    let second = seen.pop().expect("two reads");
    let first = seen.pop().expect("two reads");
    if first != second {
        return Err(A4LoadError::Echo {
            first: describe(&first),
            second: describe(&second),
        });
    }
    let kit = read_kit(0, &second).map_err(|why| A4LoadError::UnreadableKit { why })?;
    Ok((kit, second))
}

/// A kit payload in the few words an error message has room for.
fn describe(payload: &[u8]) -> String {
    match read_kit(0, payload) {
        Ok(kit) => {
            let sounds: Vec<&str> =
                (0..NUM_SOUNDS).filter_map(|n| kit.sound_name(n)).collect();
            format!("{} [{}]", kit.name, sounds.join(", "))
        }
        Err(why) => format!("{} bytes that do not decode ({why})", payload.len()),
    }
}

/// Put the preset at `path` onto `track` — SYN1 to SYN4 — of the kit the box is
/// playing.
///
/// The order is the design, and each step does work the next one needs:
///
/// 1. **Read the working kit, twice.** Its bytes are 2,060 of the bytes that go
///    back out, and slot `track`'s 350 are the backup.
/// 2. **Read the preset file** and cut its container out —
///    [`a4_preset_sound`], which refuses anything that is not this box's own
///    format and checks the foot magic that says where a sound ends.
/// 3. **Splice**, which is the only place a length can be wrong and is where it
///    is refused.
/// 4. **Send the whole kit**, then settle.
/// 5. **Read back twice** and check SYN`track` is the preset.
///
/// The recovery story is the digis' and the panel must say it: the box discards
/// an unsaved kit when the pattern is reloaded, plus [`LoadReport::backup`],
/// and it is not a stored slot that can be restored.
pub fn load_a4_preset_onto_track(
    io: &mut impl A4KitIo,
    path: &str,
    track: u8,
) -> Result<LoadReport, A4LoadError> {
    // Ahead of every read, so a bad track costs no round trip. `splice_sound`
    // checks it again where it is a guard on the bytes rather than a courtesy.
    if usize::from(track) >= NUM_SOUNDS {
        return Err(A4LoadError::NoSuchTrack { track });
    }

    let (kit, payload) = read_kit_twice(io)?;
    let replaced = kit.sound_name(usize::from(track)).unwrap_or_default().to_string();
    let backup = sound_slot(&payload, usize::from(track))
        .map_err(|why| A4LoadError::Unspliceable { why })?
        .to_vec();

    let file = io.read_preset(path)?;
    let sound = a4_preset_sound(&file)?;
    // Decoded for its name, not for validation — `a4_preset_sound` has already
    // decoded it and refused if it did not.
    let loaded = decode_drive_preset(&file)?.name;

    // **The version conversion, and it is not optional.** A +Drive file is
    // struct version 5 and a kit slot takes version 6; handed a version 5 the
    // box stores the kit and puts an *init sound* in the slot, which is a load
    // that looks like it worked and has thrown the user's track away.
    // `a4_kit::sound_for_kit` is the box's own two-byte conversion, measured
    // against 28 pairs of the same sound in both versions.
    let for_kit =
        sound_for_kit(sound).map_err(|why| A4LoadError::Unspliceable { why })?;
    let spliced = splice_sound(&payload, usize::from(track), &for_kit)
        .map_err(|why| A4LoadError::Unspliceable { why })?;

    io.write_working_kit(&spliced)?;
    io.settle();

    let (after, _) = read_kit_twice(io)?;
    let found = after.sound_name(usize::from(track)).unwrap_or_default().to_string();
    if found != loaded.trim() {
        return Err(A4LoadError::NotVerified { track, expected: loaded, found });
    }
    Ok(LoadReport { loaded, replaced, backup })
}

/// Put a track's own sound back — audition mode's undo, for a caller holding a
/// [`LoadReport::backup`].
///
/// Returns the name the track reads afterwards, verified the way a load is.
/// **The 350 bytes are sent exactly as the box gave them**, spliced into a
/// *freshly fetched* kit rather than into the one the load remembered: anything
/// else would undo the audition and also revert whatever else has changed on the
/// box since, which nobody asked for.
///
/// This is not a substitute for reloading the pattern on the box. It restores
/// what this app saw on this track before its own first load; anything else that
/// has touched the kit since — a knob, another app, the box's own browser — it
/// neither knows about nor claims to undo.
pub fn revert_a4_track(
    io: &mut impl A4KitIo,
    track: u8,
    backup: &[u8],
) -> Result<String, A4LoadError> {
    if usize::from(track) >= NUM_SOUNDS {
        return Err(A4LoadError::NoSuchTrack { track });
    }
    let (_, payload) = read_kit_twice(io)?;
    let spliced = splice_sound(&payload, usize::from(track), backup)
        .map_err(|why| A4LoadError::Unspliceable { why })?;
    // The name to expect comes from the backup bytes themselves, read through
    // the same reader the verify below uses — so a revert cannot report success
    // against a name it invented.
    let expected = expected_name(&spliced, track)?;

    io.write_working_kit(&spliced)?;
    io.settle();

    let (after, _) = read_kit_twice(io)?;
    let found = after.sound_name(usize::from(track)).unwrap_or_default().to_string();
    if found != expected {
        return Err(A4LoadError::NotVerified { track, expected, found });
    }
    Ok(found)
}

/// What SYN`track` of this payload calls itself — the name a verify compares
/// against, read out of the bytes about to be sent rather than remembered.
fn expected_name(payload: &[u8], track: u8) -> Result<String, A4LoadError> {
    let kit = read_kit(0, payload).map_err(|why| A4LoadError::UnreadableKit { why })?;
    Ok(kit.sound_name(usize::from(track)).unwrap_or_default().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_protocol::protocol::parse_sysex;
    use std::path::PathBuf;

    /// An A4 made of the committed captures: a real kit off the box as its edit
    /// buffer, and real +Drive preset files as its library.
    struct FakeA4 {
        kit: Vec<u8>,
        files: Vec<(String, Vec<u8>)>,
        /// Every kit payload this box was asked to store, in order.
        stored: Vec<Vec<u8>>,
        /// Set to make the store land nowhere, as a box with the pattern
        /// reloaded under it would.
        deaf: bool,
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../protocol/tests/fixtures")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    fn kit_payload(name: &str) -> Vec<u8> {
        parse_sysex(&fixture(name)).dump.expect("a dump").payload
    }

    const KIT00: &str = "analogfour-kit-00-2026-08-31.syx";
    const THE_SAW: &str = "drive/analogfour-soundbanks-A-1-THE-SAW-2026-08-29.bin";
    const EDGAR: &str = "drive/analogfour-soundbanks-A-7-EDGAR-2026-08-29.bin";
    const DN2_PRESET: &str = "drive/digitone2-soundbanks-A-1-HIDDEN-TEARS-2026-08-29.bin";

    impl FakeA4 {
        /// The kit named POLYTRON in the box's edit buffer, with three files on
        /// its +Drive — two of its own and one a DN2 could only have put there.
        fn new() -> Self {
            Self {
                kit: kit_payload(KIT00),
                files: vec![
                    ("/soundbanks/A/1".into(), fixture(THE_SAW)),
                    ("/soundbanks/A/7".into(), fixture(EDGAR)),
                    ("/soundbanks/A/9".into(), fixture(DN2_PRESET)),
                ],
                stored: Vec::new(),
                deaf: false,
            }
        }

        fn names(&self) -> Vec<String> {
            let kit = read_kit(0, &self.kit).expect("a kit");
            (0..NUM_SOUNDS)
                .map(|n| kit.sound_name(n).unwrap_or_default().to_string())
                .collect()
        }
    }

    impl A4KitIo for FakeA4 {
        fn read_preset(&mut self, path: &str) -> Result<Vec<u8>, MidiError> {
            self.files
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| MidiError::Send(format!("{path}: no such file")))
        }

        fn read_working_kit(&mut self) -> Result<Vec<u8>, MidiError> {
            Ok(self.kit.clone())
        }

        fn write_working_kit(&mut self, payload: &[u8]) -> Result<(), MidiError> {
            self.stored.push(payload.to_vec());
            if !self.deaf {
                self.kit = payload.to_vec();
            }
            Ok(())
        }

        fn settle(&mut self) {}
    }

    #[test]
    fn a_load_puts_the_preset_on_the_synth_track_and_says_what_it_displaced() {
        let mut a4 = FakeA4::new();

        let report = load_a4_preset_onto_track(&mut a4, "/soundbanks/A/1", 2).unwrap();

        assert_eq!(report.loaded, "THE SAW");
        assert_eq!(report.replaced, "ALONE");
        assert_eq!(a4.names(), ["ARPME", "WAVE MOD LEAD", "THE SAW", "BRE"]);
    }

    /// **One store per load, and it is a whole kit.** The count is the thing to
    /// pin: a read-modify-write that sent twice would be sending a kit built on
    /// a stale read the second time.
    #[test]
    fn a_load_sends_one_whole_kit() {
        let mut a4 = FakeA4::new();
        let before = a4.kit.clone();

        load_a4_preset_onto_track(&mut a4, "/soundbanks/A/7", 0).unwrap();

        assert_eq!(a4.stored.len(), 1);
        assert_eq!(a4.stored[0].len(), before.len(), "a whole kit, not a fragment");
    }

    /// The other three sounds, the kit's name and every byte this crate cannot
    /// name come through untouched. The assertion that matters most on this
    /// path, because a whole-kit write is what makes it possible to lose them.
    #[test]
    fn no_other_sound_in_the_kit_moves() {
        let mut a4 = FakeA4::new();
        let before = a4.kit.clone();

        load_a4_preset_onto_track(&mut a4, "/soundbanks/A/1", 3).unwrap();

        assert_eq!(a4.names(), ["ARPME", "WAVE MOD LEAD", "ALONE", "THE SAW"]);
        // Byte level, so the FX and CV tracks' settings are covered too.
        let slot = 32 + 3 * 350;
        assert_eq!(before[..slot], a4.kit[..slot]);
        assert_eq!(before[slot + 350..], a4.kit[slot + 350..]);
    }

    /// The backup a load hands back restores the track byte for byte, and the
    /// revert is spliced into a *fresh* read — so a second track auditioned in
    /// between survives the first one's undo.
    #[test]
    fn a_revert_puts_one_track_back_without_undoing_another() {
        let mut a4 = FakeA4::new();

        let first = load_a4_preset_onto_track(&mut a4, "/soundbanks/A/1", 0).unwrap();
        load_a4_preset_onto_track(&mut a4, "/soundbanks/A/7", 1).unwrap();
        assert_eq!(a4.names(), ["THE SAW", "EDGAR", "ALONE", "BRE"]);

        assert_eq!(revert_a4_track(&mut a4, 0, &first.backup).unwrap(), "ARPME");
        assert_eq!(
            a4.names(),
            ["ARPME", "EDGAR", "ALONE", "BRE"],
            "SYN2's audition must survive SYN1's revert"
        );
    }

    /// Audition mode's actual shape: several loads onto one track, then one
    /// revert to where the panel found it.
    #[test]
    fn reverting_after_several_auditions_goes_back_to_the_first_state() {
        let mut a4 = FakeA4::new();
        let opened_holding = a4.kit.clone();

        let first = load_a4_preset_onto_track(&mut a4, "/soundbanks/A/1", 1).unwrap();
        load_a4_preset_onto_track(&mut a4, "/soundbanks/A/7", 1).unwrap();
        load_a4_preset_onto_track(&mut a4, "/soundbanks/A/1", 1).unwrap();

        revert_a4_track(&mut a4, 1, &first.backup).unwrap();
        assert_eq!(a4.kit, opened_holding, "byte for byte, not merely by name");
    }

    /// The FX and CV tracks cost no round trip at all: refused before the kit
    /// is read.
    #[test]
    fn the_fx_and_cv_tracks_are_refused_without_reading_anything() {
        let mut a4 = FakeA4::new();

        for track in 4..6u8 {
            let err = load_a4_preset_onto_track(&mut a4, "/soundbanks/A/1", track).unwrap_err();
            assert!(matches!(err, A4LoadError::NoSuchTrack { .. }), "{err}");
            assert!(err.to_string().contains("FX"), "{err}");
        }
        assert!(a4.stored.is_empty(), "a refusal must not reach the wire");
    }

    /// A DN2's preset on an A4's drive — impossible in life, and the shortest
    /// way to the format refusal. Nothing is sent, and the kit is untouched.
    #[test]
    fn a_digi_preset_is_refused_before_anything_is_sent() {
        let mut a4 = FakeA4::new();

        let err = load_a4_preset_onto_track(&mut a4, "/soundbanks/A/9", 0).unwrap_err();

        assert!(
            matches!(err, A4LoadError::Preset(DriveError::NotTheBoxsOwnFormat { .. })),
            "{err}"
        );
        assert!(a4.stored.is_empty());
        assert_eq!(a4.names()[0], "ARPME");
    }

    /// A store that did not take is reported as a failure, not as a load.
    #[test]
    fn a_store_that_does_not_land_is_not_reported_as_a_load() {
        let mut a4 = FakeA4::new();
        a4.deaf = true;

        let err = load_a4_preset_onto_track(&mut a4, "/soundbanks/A/1", 2).unwrap_err();

        match err {
            A4LoadError::NotVerified { track: 2, expected, found } => {
                assert_eq!(expected, "THE SAW");
                assert_eq!(found, "ALONE");
            }
            other => panic!("expected a verify failure, got {other}"),
        }
    }

    /// MIDI thru, as the probe found it on a digi and as this box's `0x68`/`0x58`
    /// pair is just as exposed to: the second read disagrees with the first, and
    /// the load says so instead of believing either.
    #[test]
    fn two_reads_disagreeing_is_reported_as_an_echo() {
        /// A box that answers a different kit each time it is read.
        struct Echoing {
            kits: Vec<Vec<u8>>,
            at: usize,
        }
        impl A4KitIo for Echoing {
            fn read_preset(&mut self, _: &str) -> Result<Vec<u8>, MidiError> {
                Ok(fixture(THE_SAW))
            }
            fn read_working_kit(&mut self) -> Result<Vec<u8>, MidiError> {
                let out = self.kits[self.at % self.kits.len()].clone();
                self.at += 1;
                Ok(out)
            }
            fn write_working_kit(&mut self, _: &[u8]) -> Result<(), MidiError> {
                Ok(())
            }
            fn settle(&mut self) {}
        }

        let mut box_ = Echoing {
            kits: vec![kit_payload(KIT00), kit_payload("analogfour-kit-01-2026-08-31.syx")],
            at: 0,
        };

        let err = load_a4_preset_onto_track(&mut box_, "/soundbanks/A/1", 0).unwrap_err();

        match err {
            A4LoadError::Echo { first, second } => {
                assert!(first.contains("POLYTRON"), "{first}");
                assert!(second.contains("STEPPA"), "{second}");
            }
            other => panic!("expected an echo refusal, got {other}"),
        }
    }

    /// A kit the box answered that does not decode stops the load before the
    /// splice — there is no backup to be had, so there is no load to be made.
    #[test]
    fn a_kit_that_does_not_decode_is_refused_rather_than_spliced_into() {
        struct Broken;
        impl A4KitIo for Broken {
            fn read_preset(&mut self, _: &str) -> Result<Vec<u8>, MidiError> {
                Ok(fixture(THE_SAW))
            }
            fn read_working_kit(&mut self) -> Result<Vec<u8>, MidiError> {
                // The right length, the wrong struct version: the one refusal
                // that says "these offsets are not this box's".
                Ok(vec![0u8; 2410])
            }
            fn write_working_kit(&mut self, _: &[u8]) -> Result<(), MidiError> {
                panic!("nothing may be sent");
            }
        }

        let err = load_a4_preset_onto_track(&mut Broken, "/soundbanks/A/1", 0).unwrap_err();
        assert!(matches!(err, A4LoadError::UnreadableKit { .. }), "{err}");
    }
}
