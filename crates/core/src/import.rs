// The import seam: a pattern fetched off a box becomes a `Pattern` in a slot.
//
// Port of the note half of `js/roll-bridge.js` plus its one caller, the import
// path in `js/main.js`. `lengths.rs` is the length half of the same file and
// this sits beside it deliberately: both are the *bridge* between the note
// model here and the Elektron pattern format in `protocol`, and both belong on
// the model side of that bridge because both are pure and testable without a
// port in sight.
//
// **`core` still parses no bytes** (PLAN.md §3). Every value below is read by a
// `protocol` function — `track_notes`, `read_swing`, `read_track_trig_settings`,
// `read_track_prob` — and this file only assembles the answers into the model.
// What changes is that a `Spec` stops being purely opaque here: it is the handle
// core hands *back* to protocol to read with, never one core indexes into.
//
// Where the JS and this differ, and why — three of them, each because the
// browser app's roll slot is not this model's track:
//
//   1. **Lengths are not clamped to the room left in the pattern.** The JS
//      clamps a note's length to `slotLength - step`, because its roll draws a
//      fixed window and a note longer than the window cannot be drawn. Here the
//      track keeps its real length and the engine schedules every note-off in
//      absolute seconds (`engine::notes`), so a trig that rings past the loop
//      point is representable — and it is what the box does with the INF length
//      byte that `track_notes` already maps to the whole track. Clamping would
//      quietly shorten notes the box is holding. Only `LEN_MIN` is enforced,
//      because a zero-length note is a note nothing can play.
//   2. **Pitch is not clamped.** The JS clamps to the rows its roll draws;
//      ours draws all 128 and `track_notes` has already masked to 0-127.
//   3. **A whole pattern arrives, not one track.** The browser app imported one
//      track into one of its eight slots; a slot here *is* a pattern, with the
//      box's own 16 tracks in it.
//
// Two things a fetched pattern carries that this deliberately drops:
//
//   * **Tempo.** The box's pattern struct has one and this model does not
//      mirror it (PLAN.md §7 rule 8 — one clock, the studio's). It is reported
//      rather than silently discarded, so a caller can offer it.
//   * **P-lock lanes.** `js/roll-bridge.js` imports them; the per-box parameter
//      tables they need are unported (PLAN.md §6), so an imported pattern comes
//      in with no lanes rather than with lanes nothing can name.
//      **Closed 2026-08-18**: the tables are ported and so is the pool reader, so
//      lanes come across — named where the tables know the paramId, and carried
//      raw where they do not, which is what `describe_param`'s fallback is for.
//
// And one the *format* does not carry into this model yet: **SCALE**. The
// per-track clock multiplier is not in the mapped pattern struct, so every
// imported track arrives at 1x however the box was playing it. Track *length*
// does come across, so polymeter by length survives an import and polymeter by
// scale does not.

use digi_protocol::params::{describe_param, param_table_for};
use digi_protocol::pattern::{track_notes, PatternKit, Spec};
use digi_protocol::pattern_settings::read_swing;
use digi_protocol::plocks::{lane_has_trigless_values, read_track_plocks, PoolLane};
use digi_protocol::trig_cond::{read_track_prob, read_track_trig_settings};

use crate::device::{DeviceId, DeviceModel};
use crate::model::{Note, PLockLane, Pattern, PatchSound, Source, TrackKind, TrackPatch};
use crate::session::{PatternRef, Session};
use crate::LEN_MIN;

/// What a kit calls one of its tracks — the one place this app decides that,
/// so an import and a write dialog's label (`write.rs::track_kind_label`) never
/// drift apart. `None`-shaped answers are still meaningful: a MIDI track has no
/// sound to be named after, and an unnamed slot is not the same thing as an
/// empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KitTrackName<'a> {
    /// Masked as a MIDI track in this kit — it has no sound to be named after.
    Midi,
    /// An audio track the kit has actually named.
    Sound(&'a str),
    /// An audio track slot the kit has never named.
    Unnamed,
}

/// What `kit` calls `track_index`, resolved once so nothing downstream
/// re-derives the MIDI mask or the "trim, then check empty" rule on its own.
pub fn kit_track_name(kit: &PatternKit, track_index: usize) -> KitTrackName<'_> {
    if track_index < 16 && kit.kit.midi_mask & (1u16 << track_index) != 0 {
        return KitTrackName::Midi;
    }
    match kit.kit.sound_names.get(track_index).map(|s| s.trim()) {
        Some(name) if !name.is_empty() => KitTrackName::Sound(name),
        _ => KitTrackName::Unnamed,
    }
}

/// Now, as Unix epoch seconds. The one clock read in this module, and a
/// caller's to make (`TrackPatch::seen_at`) — kept a plain `i64` rather than a
/// dependency, matching `protocol::safe_write::Timestamp`'s reasoning about
/// `chrono` for the same problem.
pub fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One track's patch record, as `kit` names it — always constructible, unlike
/// the sound name alone, because [`kit_track_name`]'s three answers all mean
/// something (packet E's addendum, 2026-08-20). The only caller-supplied
/// pieces are the ones the kit cannot know: where this came from and when.
///
/// **The one place `TrackPatch` is assembled.** `pattern_from_kit` (a full
/// import) and [`Session::apply_patch_read`] (a patch-names-only read) both
/// call this rather than building the struct by hand, so a MIDI track and an
/// unnamed track get the same treatment whichever path fetched them.
pub fn patch_for_track(kit: &PatternKit, track_index: usize, from: &Source, seen_at: i64) -> TrackPatch {
    let sound = match kit_track_name(kit, track_index) {
        KitTrackName::Midi => PatchSound::Midi,
        KitTrackName::Sound(name) => PatchSound::Named(name.to_owned()),
        KitTrackName::Unnamed => PatchSound::Unnamed,
    };
    TrackPatch {
        sound,
        kit_name: kit.kit.name.clone(),
        kit_index: kit.kit_index,
        from: from.clone(),
        seen_at,
    }
}

/// One fetched pattern, exactly as the fetch path leaves it.
///
/// The raw `payload` is kept alongside the decoded `kit` because
/// `decode_pattern_kit` does not carry the per-step trig lanes or the swing
/// byte — those are read straight off the payload, which is the same bargain
/// `js/main.js` makes when it holds on to `fetched.payload`.
#[derive(Debug, Clone, Copy)]
pub struct Fetched<'a> {
    /// The spec the payload was decoded with. Checked against the destination
    /// device's own spec, because the lane offsets differ between boxes.
    pub spec: &'a Spec,
    pub kit: &'a PatternKit,
    pub payload: &'a [u8],
    /// The slot on the box this came out of — provenance, not a destination.
    pub from: PatternRef,
}

/// What an import brought across, and what it did not.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    /// The box's own name for the pattern. Often empty: a pattern that has
    /// never been named on the box carries no name at all.
    pub pattern_name: String,
    pub notes: usize,
    pub tracks_with_notes: usize,
    pub swing: u8,
    /// The tempo stored in the fetched pattern. **Read, never applied** — the
    /// session masters one clock (PLAN.md §7 rule 8). Reported so a caller can
    /// offer it rather than have it vanish.
    pub box_tempo_bpm: f64,
    /// Trigs that sat past their own track's LEN and were dropped. The box
    /// stores them and does not play them; so does this.
    pub trimmed_past_len: usize,
    /// P-lock lanes that came across, over every track.
    pub plock_lanes: usize,
    /// Of those, the ones holding a parameter no curated table names. They are
    /// carried byte-exact and shown read-only — the strip cannot honestly draw
    /// or edit a knob it cannot identify. Worth reporting because it is the
    /// difference between "your automation is here" and "your automation is here
    /// and some of it is untouchable".
    pub unnamed_plock_lanes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    NoSuchDevice(DeviceId),
    NoSuchSlot { device: DeviceId, slot: PatternRef },
    /// The destination is a sequence-live-only model, which has no pattern
    /// format to import into.
    LiveOnly(&'static DeviceModel),
    /// The payload was decoded with a different box's spec. Its lane offsets
    /// would be read at the wrong addresses, so this refuses rather than
    /// importing plausible nonsense — moving a track between boxes is
    /// cross-device copy (`js/elektron/copy-track.js`), which is not this.
    NotThisBox { expected: &'static str, found: &'static str },
    /// A decoded pattern with a different track count than the model has. Only
    /// reachable through a hand-built kit, and it would leave a `Pattern` that
    /// fails `Device::validate`.
    TrackCountMismatch { expected: usize, found: usize },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDevice(id) => write!(f, "no device {} in this session", id.0),
            Self::NoSuchSlot { slot, .. } => {
                write!(f, "that box has no slot {}", slot.label())
            }
            Self::LiveOnly(m) => write!(
                f,
                "{} plays over MIDI but has no pattern dumps to import",
                m.display
            ),
            Self::NotThisBox { expected, found } => write!(
                f,
                "that pattern was fetched from a {found} — this device is a {expected}"
            ),
            Self::TrackCountMismatch { expected, found } => write!(
                f,
                "this box has {expected} tracks and that pattern carries {found}"
            ),
        }
    }
}

impl std::error::Error for ImportError {}

/// A fetched pattern as a [`Pattern`] for `model`, with nothing of the session
/// in it.
///
/// This is the whole conversion and it is pure: the same kit and payload always
/// give the same pattern, note ids aside. Studio state — ports, channels, mute,
/// solo — is *not* set here, because a fetched pattern says nothing about it;
/// [`Session::import_pattern`] is where the destination slot's own routing is
/// preserved.
pub fn pattern_from_kit(
    model: &'static DeviceModel,
    fetched: &Fetched,
) -> Result<(Pattern, ImportReport), ImportError> {
    let spec = model.spec().ok_or(ImportError::LiveOnly(model))?;
    if spec.device != fetched.spec.device {
        return Err(ImportError::NotThisBox {
            expected: spec.device,
            found: fetched.spec.device,
        });
    }
    let kit = fetched.kit;
    if kit.tracks.len() != model.num_tracks {
        return Err(ImportError::TrackCountMismatch {
            expected: model.num_tracks,
            found: kit.tracks.len(),
        });
    }

    let mut pattern = Pattern::for_model(model);
    // A pattern that has never been named on the box carries no name, and a
    // blank pattern name in the UI would be worse than a slot label.
    pattern.name = match kit.name.trim() {
        "" => fetched.from.label(),
        named => named.to_owned(),
    };
    pattern.swing = read_swing(fetched.spec, fetched.payload);
    let fetched_from = Source {
        device_slug: model.slug.unwrap_or(model.key).to_owned(),
        bank: fetched.from.bank,
        index: fetched.from.index,
    };
    pattern.source = Some(fetched_from.clone());

    let mut report = ImportReport {
        pattern_name: kit.name.clone(),
        notes: 0,
        tracks_with_notes: 0,
        swing: pattern.swing,
        box_tempo_bpm: kit.tempo_bpm,
        trimmed_past_len: 0,
        plock_lanes: 0,
        unnamed_plock_lanes: 0,
    };

    // One instant for the whole fetch, not one call per track: sixteen tracks
    // read off the same dump were seen at the same moment, and sixteen calls a
    // millisecond apart would print sixteen slightly different "read" dates for
    // one press.
    let seen_at = now_unix_seconds();

    for t in 0..model.num_tracks {
        // Every read below is `protocol`'s; a track index inside `num_tracks`
        // is one both of them accept, so these cannot fail.
        let settings = read_track_trig_settings(fetched.spec, fetched.payload, t)
            .expect("track index is inside the spec's own track count");
        let track_prob = read_track_prob(fetched.spec, fetched.payload, t)
            .expect("track index is inside the spec's own track count");
        let source = &kit.tracks[t];
        // On a DT2 the MIDI tracks are a mask over the same 16 tracks, held in
        // the kit; a DN2 has no mask, so its 0 makes every track audio.
        // Asked of `kit_track_name` rather than re-testing `midi_mask` here:
        // that rule now lives in exactly one place, and this was the third
        // copy of it (DEVELOPMENT.md's lesson 5).
        let kind = if kit_track_name(kit, t) == KitTrackName::Midi {
            TrackKind::Midi
        } else {
            model.default_track_kind
        };
        let length_steps = source.length_steps.clamp(1, model.max_steps);

        let mut notes = Vec::new();
        for n in track_notes(kit, t) {
            // A trig past the track's own LEN is stored by the box and never
            // played by it. The JS filters these out at import for the same
            // reason: they are not part of what the pattern sounds like.
            if u16::from(n.step) >= length_steps {
                report.trimmed_past_len += 1;
                continue;
            }
            let mut note = Note::new(
                f64::from(n.step),
                n.pitch,
                n.len_steps.max(LEN_MIN),
                n.velocity,
                n.micro,
            );
            // PROB/FILL/COND are per trig, so every note on a step takes the
            // step's values — the uniformity rule `adopt_step_trig` upholds.
            if let Some(s) = settings.get(&n.step) {
                note.prob = s.prob;
                note.fill = s.fill;
                note.cond = s.cond.map(str::to_owned);
            }
            notes.push(note);
        }

        report.notes += notes.len();
        report.tracks_with_notes += usize::from(!notes.is_empty());

        // The steps that actually have trigs, which is what makes a lock
        // "trigless". Taken from the trimmed notes rather than the raw ones: a
        // lock on a step past LEN is on a step this import has decided does not
        // exist, and calling it trigless is the honest answer.
        let live_steps: Vec<usize> = notes.iter().map(|n| n.step.round() as usize).collect();
        let lanes: Vec<PLockLane> = read_track_plocks(fetched.spec, fetched.payload, t)
            .expect("track index is inside the spec's own track count")
            .iter()
            .map(|pool| lane_to_model(pool, fetched.spec.device, &live_steps))
            .collect();
        report.plock_lanes += lanes.len();
        report.unnamed_plock_lanes += lanes.iter().filter(|l| l.name.is_none()).count();

        let track = pattern.track_mut(t).expect("built for this model a moment ago");
        track.length_steps = length_steps;
        track.track_prob = track_prob;
        track.kind = kind;
        track.notes = notes;
        track.plocks = lanes;
        // The kit's sound name is what the box shows for the track, and it is
        // the most useful label an import can bring: "BD HARD" beats "T1". A
        // MIDI track has no sound to be named after, and an unnamed slot keeps
        // the default rather than going blank. `kit_track_name` is the one place
        // that rule lives — `write.rs::track_kind_label` asks it the same
        // question for a write dialog's label.
        if let KitTrackName::Sound(name) = kit_track_name(kit, t) {
            track.name = name.to_owned();
        }
        // **Every track gets a patch record, not only the named ones.** A
        // fetched MIDI track and a fetched-but-unnamed audio track were both
        // fetched — leaving them with no record at all is the same dishonesty
        // this whole item exists to remove, one case over (packet E's
        // addendum, 2026-08-20): the tooltip would say "not fetched" about a
        // track that plainly was. `fetched_from` is the same value just written
        // to `pattern.source`, a few lines up, from this same fetch — exactly
        // what `from` means here.
        track.patch = Some(patch_for_track(kit, t, &fetched_from, seen_at));
    }

    Ok((pattern, report))
}

/// One pool lane as the model holds it — the port of `devicePLocksToRoll`.
///
/// Two conversions happen here and both are worth naming.
///
/// **Stored word → display value.** The pool holds the box's uint16; the model
/// holds the display axis the roll draws and the audition path sends. The
/// scaling lives in the parameter descriptor, so a lane whose paramId is in no
/// table converts by the identity — which is exactly what keeps it byte-exact on
/// the way back out.
///
/// **paramId → canonical name, where one exists.** A named lane is one the strip
/// can draw and edit and cross-device copy can translate. An unnamed one is
/// carried and shown, and nothing more; `param_id` and `device_kind` travel with
/// it either way, because they are what a write-back will need.
fn lane_to_model(pool: &PoolLane, device_kind: &'static str, live_steps: &[usize]) -> PLockLane {
    let desc = describe_param(
        param_table_for(device_kind),
        None,
        Some(u16::from(pool.param_id)),
        Some(device_kind),
    );
    let values = pool
        .values
        .iter()
        .map(|v| v.and_then(|w| desc.display_from_stored(w)).map(|d| d.max(0) as u16))
        .collect();
    PLockLane::new(
        desc.name.map(str::to_owned),
        Some(u16::from(pool.param_id)),
        Some(device_kind.to_owned()),
        lane_has_trigless_values(pool, live_steps),
        values,
    )
    .expect("a pool lane always has a paramId, so the lane is never nameless")
}

// --- reading patch names off a box, without touching the rest of the pattern ---
//
// Packet E, stage 2 (2026-08-20). Different from [`pattern_from_kit`] on
// purpose: that is a whole-pattern import and overwrites notes, lengths, trig
// locks and swing along with the name. A patch-names read touches none of
// that — it is a smaller, safer claim: "here is what the box currently calls
// each track's sound", landed onto whatever pattern is already sitting in the
// session, renamed by hand or not. So [`Session::apply_patch_read`] writes
// only `Track.patch`, never `Track.name` and never anything else.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchReadError {
    NoSuchDevice(DeviceId),
    NoSuchSlot { device: DeviceId, slot: PatternRef },
    /// The pattern at that slot carries no [`Source`] at all — this app does
    /// not know which slot on the box it came from, and a read must not guess
    /// one (packet E: "do not silently read A01"). The pattern has to be
    /// fetched from a named slot first, or the caller has to ask which slot to
    /// read rather than assume.
    UnknownSlot,
    /// The pattern's own `source` names a different box than the one being
    /// asked. Reading anyway would attach this box's kit names to a pattern
    /// that says it came from another — the read-path twin of
    /// `ui::write::wrong_box`.
    NotThisBox { pattern_slug: String },
    /// A decoded kit with a different track count than the model has. Only
    /// reachable through a hand-built kit in a test.
    TrackCountMismatch { expected: usize, found: usize },
}

impl std::fmt::Display for PatchReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchDevice(id) => write!(f, "no device {} in this session", id.0),
            Self::NoSuchSlot { slot, .. } => write!(f, "that box has no slot {}", slot.label()),
            Self::UnknownSlot => write!(
                f,
                "this pattern has no record of which slot on the box it came from — fetch it \
                 from a named slot first, rather than have this guess one"
            ),
            Self::NotThisBox { pattern_slug } => write!(
                f,
                "this pattern was fetched from a {pattern_slug} — reading patch names from a \
                 different box would attach the wrong box's kit to it"
            ),
            Self::TrackCountMismatch { expected, found } => write!(
                f,
                "this box has {expected} tracks and that kit carries {found}"
            ),
        }
    }
}

impl std::error::Error for PatchReadError {}

/// Which of the box's slots a patch-names read should ask for, resolved from
/// the pattern's own provenance rather than guessed.
///
/// Pure and box-free on purpose, so the "ask rather than assume" rule
/// (packet E's addendum) is testable without a fetch in sight: a pattern with
/// no [`Source`] refuses here, before anything opens a port.
pub fn patch_read_source(pattern: Option<&Pattern>, slug: Option<&str>) -> Result<Source, PatchReadError> {
    match pattern.and_then(|p| p.source.clone()) {
        Some(source) if Some(source.device_slug.as_str()) == slug => Ok(source),
        Some(source) => Err(PatchReadError::NotThisBox { pattern_slug: source.device_slug }),
        None => Err(PatchReadError::UnknownSlot),
    }
}

/// The same resolve, for a slot the **user has named** rather than one this
/// app worked out.
///
/// [`patch_read_source`] can only answer for a pattern that came off a box,
/// which left the commonest case in the cold: you start in this app, build a
/// pattern from scratch, and want to know what the box currently calls each of
/// its sixteen tracks. There is nothing to resolve from, so that function
/// refuses — correctly, since a resolver that quietly picked A01 is the bug
/// packet E's addendum names. What was missing was the third option between
/// guessing and refusing: **let the slot be said out loud.** `asked` comes
/// from a picker sitting next to the button with its value on screen, so the
/// read is aimed by the person pressing it. Nothing here is inferred.
///
/// `NotThisBox` still applies. A slot number is the user's to choose; which
/// box a pattern came off is not, and attaching a DT2's kit names to a pattern
/// whose own record says DN2 would be wrong however clearly it was asked for.
///
/// Note what this cannot promise. A dump request names a *stored* slot, and
/// the Elektron dump protocol as implemented here has no way to ask "what are
/// you playing right now" — so this reads what is saved at `asked`, which is
/// what the box has live exactly when it is sitting on that pattern with no
/// unsaved kit edits. The caller says so on screen rather than claiming more.
pub fn patch_read_source_named(
    pattern: Option<&Pattern>,
    slug: &str,
    asked: PatternRef,
) -> Result<Source, PatchReadError> {
    match pattern.and_then(|p| p.source.as_ref()) {
        Some(source) if source.device_slug != slug => {
            Err(PatchReadError::NotThisBox { pattern_slug: source.device_slug.clone() })
        }
        _ => Ok(Source {
            device_slug: slug.to_owned(),
            bank: asked.bank,
            index: asked.index,
        }),
    }
}

impl Session {
    /// Write a patch record onto every track of the pattern at `at`, from a
    /// kit already fetched and decoded. Touches `Track.patch` and nothing
    /// else — see this section's header for why.
    ///
    /// `from` is the box's own slot the kit was fetched from — normally
    /// [`patch_read_source`]'s answer, resolved by the caller before opening a
    /// port so a failed handshake never gets this far. `at` is the session
    /// slot whose tracks are being updated, which need not be the same
    /// position: a pattern imported into B03 still names A01 as its `source`.
    ///
    /// Returns the number of tracks patched. Refuses wholesale — a
    /// `TrackCountMismatch` touches nothing rather than patching the tracks
    /// that do line up, so a caller never has to reason about a half-applied
    /// read.
    pub fn apply_patch_read(
        &mut self,
        device: DeviceId,
        at: PatternRef,
        kit: &PatternKit,
        from: &Source,
        seen_at: i64,
    ) -> Result<usize, PatchReadError> {
        let model = self.device(device).ok_or(PatchReadError::NoSuchDevice(device))?.model;
        if kit.tracks.len() != model.num_tracks {
            return Err(PatchReadError::TrackCountMismatch {
                expected: model.num_tracks,
                found: kit.tracks.len(),
            });
        }
        let d = self.device_mut(device).expect("checked just above");
        let pattern =
            d.pattern_mut(at.slot()).ok_or(PatchReadError::NoSuchSlot { device, slot: at })?;
        for t in 0..model.num_tracks {
            let track = pattern.track_mut(t).expect("checked against this model's track count");
            track.patch = Some(patch_for_track(kit, t, from, seen_at));
        }
        Ok(model.num_tracks)
    }
}

impl Session {
    /// Load a fetched pattern into one of a device's slots.
    ///
    /// This is the end of the fetch path: `fetch_pattern_kit` →
    /// `decode_pattern_kit` → here. `into` is where it lands, which need not be
    /// where it came from — [`Fetched::from`] records the latter as the
    /// pattern's [`Source`].
    ///
    /// **The slot's studio state survives.** Ports, channels, mute and solo are
    /// per track and belong to the session, not to the box's pattern bytes
    /// (`Track`'s own note says so); an import that reset them would silently
    /// re-route a desk that was already working. Everything a dump actually
    /// carries — notes, lengths, trig locks, track PROB, kind, swing — is
    /// replaced wholesale.
    ///
    /// Nothing else in the session moves. In particular the tempo does not: the
    /// box's is in the returned [`ImportReport`], for a caller to offer rather
    /// than to have taken.
    pub fn import_pattern(
        &mut self,
        device: DeviceId,
        into: PatternRef,
        fetched: &Fetched,
    ) -> Result<ImportReport, ImportError> {
        let model = self
            .device(device)
            .ok_or(ImportError::NoSuchDevice(device))?
            .model;
        let (mut pattern, report) = pattern_from_kit(model, fetched)?;

        let d = self.device_mut(device).expect("checked just above");
        let old = d
            .pattern(into.slot())
            .ok_or(ImportError::NoSuchSlot { device, slot: into })?;
        // Carried before the slot is overwritten, and by index because both
        // patterns are this model's shape.
        let studio: Vec<_> = old
            .tracks()
            .iter()
            .map(|t| (t.out_port.clone(), t.channel, t.mute, t.solo))
            .collect();
        for (t, (out_port, channel, mute, solo)) in studio.into_iter().enumerate() {
            let track = pattern.track_mut(t).expect("same model, same track count");
            track.out_port = out_port;
            track.channel = channel;
            track.mute = mute;
            track.solo = solo;
        }

        *d.pattern_mut(into.slot()).expect("checked just above") = pattern;
        Ok(report)
    }
}
