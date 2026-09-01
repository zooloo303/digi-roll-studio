// Pattern and track: the two levels below a box.
//
// A pattern here mirrors the DT2/DN2 pattern struct closely enough to map 1:1
// onto one of that box's pattern slots, and deliberately not more closely than
// that — see `Pattern`'s note on tempo.
//
// Positions are `f64` fractional steps, not a tick grid. PLAN.md §4 is explicit
// about why: micro-timing is 1/24 of a step and swing is a percentage offset on
// odd steps, and 30% of a step is not a whole number of 1/24ths. Quantising to
// 96 PPQN would be exact for one and wrong for the other. (96 PPQN survives as
// the resolution for MIDI *file* export, which is a different problem.) The JS
// these are ported from uses doubles throughout, so f64 also keeps the ported
// arithmetic bit-comparable with its oracle.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::device::DeviceModel;

pub const PLOCK_STEPS: usize = 128;

/// Note ids are unique within a process, not within a project file. They exist
/// so the UI can name a note across a drag; nothing persists a reference to one.
static NEXT_NOTE_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

fn next_note_id() -> u32 {
    NEXT_NOTE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: u32,
    pub step: f64,
    pub pitch: u8,
    pub len: f64,
    pub velocity: u8,
    pub micro: f64,
    /// Per-trig on the box, not per note: every note sharing a step has to
    /// agree. `edit_ops::adopt_step_trig` is what keeps that true when notes
    /// arrive on an occupied step.
    #[serde(default)]
    pub prob: Option<u8>,
    #[serde(default)]
    pub fill: Option<bool>,
    #[serde(default)]
    pub cond: Option<String>,
}

impl Note {
    pub fn new(step: f64, pitch: u8, len: f64, velocity: u8, micro: f64) -> Self {
        Self {
            id: next_note_id(),
            step,
            pitch,
            len,
            velocity,
            micro,
            prob: None,
            fill: None,
            cond: None,
        }
    }

    /// A fresh identity for a note that was copied. Paste, alt-drag and undo all
    /// need this: two notes carrying one id makes the selection ambiguous.
    pub fn reissue_id(&mut self) {
        self.id = next_note_id();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PLockLane {
    /// Canonical parameter key (`filter.cutoff`) when this app authored the
    /// lane, so it knows exactly which knob this is. `None` on a lane that came
    /// off a box. One of `name`/`param_id` is always set.
    #[serde(default)]
    pub name: Option<String>,
    /// The box's own parameter byte, when the lane came off a box. Resolved from
    /// the curated tables on the way out (`digi_protocol::params`).
    #[serde(default)]
    pub param_id: Option<u16>,
    /// Which box this lane is for. The two boxes number their parameters
    /// differently *and* use different CCs for the same knob, so a lane without
    /// this is meaningless — and the audition path refuses to send one.
    #[serde(default)]
    pub device_kind: Option<String>,
    /// The box's own lane held a value on a step with no trig. v1 does not model
    /// trigless locks, so such a lane is shown read-only and passed through
    /// untouched rather than edited into something that is not what the box has.
    #[serde(default)]
    pub trigless: bool,
    /// One value per step, `None` where the step has no lock, **on the
    /// parameter's display axis** — MIDI 0–127 for a curated parameter, the raw
    /// uint16 for a lane off a box whose knob we cannot name.
    ///
    /// Display rather than stored, and `js/state.js` gives the reason: a lane can
    /// be authored and auditioned before anyone knows what its uint16 would be.
    /// The stored word (display × 256, per the Phase 0 measurements) appears only
    /// at the pattern's lane pool, which is unported.
    #[serde(default)]
    pub values: Vec<Option<u16>>,
}

impl PLockLane {
    /// `Err` rather than the JS's throw: a lane with neither a name nor a
    /// paramId cannot be written to any box, so it must not be constructible.
    pub fn new(
        name: Option<String>,
        param_id: Option<u16>,
        device_kind: Option<String>,
        trigless: bool,
        values: Vec<Option<u16>>,
    ) -> Result<Self, ModelError> {
        if name.is_none() && param_id.is_none() {
            return Err(ModelError::NamelessPLockLane);
        }
        let mut v = vec![None; PLOCK_STEPS];
        for (i, val) in values.into_iter().enumerate().take(PLOCK_STEPS) {
            v[i] = val;
        }
        Ok(Self {
            name,
            param_id,
            device_kind,
            trigless,
            values: v,
        })
    }

    /// Which parameter this lane is, resolved in **its own box's** table.
    ///
    /// The lane's two identities have a precedence and it lives once, in
    /// `digi_protocol::params::describe_param`: a name wins, because a lane this
    /// app authored knows exactly which knob it is, and a paramId is the weaker
    /// evidence. What this adds is the other half of the same rule — the table
    /// and the label both come from `device_kind`, because 74 is filter
    /// frequency on a DN2 and overdrive on a DT2, so a lane read against the
    /// wrong table is not merely unlabelled but wrong.
    ///
    /// **This is not the descriptor a write uses.** A write resolves the lane
    /// against the box it is aimed at (`export::lanes_for_device`), which is how
    /// a lane belonging to the other box's numbering gets refused instead of
    /// translated. This one answers "what is this lane", which is what the strip
    /// draws and what a refusal has to name.
    pub fn param(&self) -> digi_protocol::params::ParamDesc {
        let kind = self.device_kind.as_deref().unwrap_or_default();
        digi_protocol::params::describe_param(
            digi_protocol::params::param_table_for(kind),
            self.name.as_deref(),
            self.param_id,
            digi_protocol::params::device_kind_key(kind),
        )
    }
}

/// Audio or MIDI. On a DT2 this is a mask over the same 16 tracks, held in the
/// *kit* (`dt2_spec().kits[..].midi_mask_offset`) — MIDI tracks are emphatically
/// not tracks 17–32. A DN2 has no mask, so every track is audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    #[default]
    Audio,
    Midi,
}

/// The per-track clock multiplier the boxes call SCALE. Stored as an enum rather
/// than a float so a project file can never carry a ratio no box can play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TrackScale {
    Eighth,
    Quarter,
    Half,
    ThreeQuarters,
    #[default]
    One,
    ThreeHalves,
    Two,
}

impl TrackScale {
    /// How fast this track's clock runs relative to the pattern's.
    pub fn multiplier(self) -> f64 {
        match self {
            Self::Eighth => 0.125,
            Self::Quarter => 0.25,
            Self::Half => 0.5,
            Self::ThreeQuarters => 0.75,
            Self::One => 1.0,
            Self::ThreeHalves => 1.5,
            Self::Two => 2.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub name: String,
    /// Per track, which is what makes polymeter real: tracks wrap independently,
    /// exactly as they do on the box.
    pub length_steps: u16,
    pub scale: TrackScale,
    /// The default an unlocked trig runs at. 100 = always.
    pub track_prob: u8,
    pub kind: TrackKind,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub plocks: Vec<PLockLane>,

    // Studio state, not pattern bytes. These persist with the project file and
    // are never encoded into a dump, which is what keeps the minimal-diff
    // contract clean (PLAN.md §2).
    #[serde(default)]
    pub out_port: Option<String>,
    pub channel: u8,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    /// The box's own track LEVEL, once someone has moved it — and `None` until
    /// then, which is the whole of why this is an `Option`.
    ///
    /// **`None` means "this app has never touched that fader", not "the fader is
    /// at zero" and not "it is at some default".** Only the box knows where its
    /// level actually sits; nothing in a pattern dump tells us
    /// (`protocol::pattern` does not map the kit's level bytes), so a `0`
    /// default would have the app open a project and immediately ride sixteen
    /// faders to silence to make the box agree with a number it invented. A
    /// value here is a value the user set, and the only thing that ever sends
    /// one is that act.
    ///
    /// On the display axis every parameter in this app uses: MIDI 0–127. The
    /// controller it goes out on is per box and is
    /// `protocol::params::track_level_midi` — **not CC 7**, which is in neither
    /// box's chart.
    #[serde(default)]
    pub level: Option<u8>,
    /// What the app last saw fetched into this track, if anything. Survives a
    /// rename — the track's own `name` is a copy of `patch.sound` taken at
    /// import time, and nothing keeps the two in sync afterwards, so this is
    /// what a renamed track has left to point back to its sound.
    ///
    /// **This is not "what is loaded on the box right now."** It is a record of
    /// the last fetch, nothing newer. Only a fetch (packet E) can answer the
    /// live question; a UI that reads this and implies otherwise is lesson 3
    /// happening again in miniature.
    #[serde(default)]
    pub patch: Option<TrackPatch>,
}

/// What a fetch found this track named as. Three shapes, not one string with an
/// empty-string sentinel — packet E (2026-08-20) found the first draft used
/// `sound_name: String::new()` to mean "no sound", which is exactly how "no
/// sound" quietly becomes a sound called `""` the moment anything prints it
/// unguarded. A MIDI track has no sound to be named after at all, and an
/// unnamed audio slot is a third, different fact — neither is a string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PatchSound {
    /// The kit's name for this track's sound — the same string `Track::name`
    /// was set to at the moment this was recorded, before anyone renamed it.
    Named(String),
    /// Masked as a MIDI track in this kit — it has no sound to be named after.
    /// A fetched MIDI track still gets a record; it just names no sound.
    Midi,
    /// An audio track slot the kit has never named.
    Unnamed,
}

/// Provenance for a track's sound: what a fetch last saw, and where it came
/// from. Set once, at import (`import::pattern_from_kit`) or by a patch-names
/// read (`import::apply_patch_read`), and left alone by everything else that
/// touches the track — including a rename, which is the whole reason this
/// exists separately from `Track::name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPatch {
    /// What the kit called this track: a named sound, MIDI, or unnamed. See
    /// [`PatchSound`] for why this is not a plain string.
    pub sound: PatchSound,
    /// The kit this sound came from.
    pub kit_name: String,
    /// The kit's own index, as the box numbers it.
    pub kit_index: u8,
    /// The slot this was fetched from.
    pub from: Source,
    /// Unix epoch seconds, UTC, when this was recorded. A plain timestamp
    /// rather than a dependency: nothing in this crate needs calendar
    /// arithmetic on it, only a caller formatting one for a tooltip.
    pub seen_at: i64,
}

impl Track {
    pub fn new(index: usize, kind: TrackKind) -> Self {
        Self {
            name: format!("T{}", index + 1),
            length_steps: 16,
            scale: TrackScale::One,
            track_prob: 100,
            kind,
            notes: Vec::new(),
            plocks: Vec::new(),
            out_port: None,
            // Track n on channel n: 16 tracks fill a port's 16 channels exactly,
            // and it is the only map where the number in this app and the number
            // on the box agree.
            //
            // **It is not what a factory DT2 or DN2 listens on.** Both ship with
            // `TRACK 1–8 CH` = 1–8 and `TRACK 9–16 CH` = `OFF`, and they reserve
            // channel 9 for `FX CONTROL CH` and 10 for `AUTO CHANNEL` — so half of
            // this map plays nothing until someone opens SETTINGS → MIDI CONFIG →
            // CHANNELS on the box. The default stays 1:1 because that is the map to
            // configure a box *to*; `ui::tracks::channel_note` is where the app
            // says so, and it carries the whole story.
            channel: (index % 16) as u8,
            mute: false,
            solo: false,
            // Never a number: see the field. Until someone moves the fader
            // there is nothing honest to send.
            level: None,
            patch: None,
        }
    }
}

/// Where a pattern was fetched from, if it was fetched rather than written here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub device_slug: String,
    pub bank: u8,
    pub index: u8,
}

impl Source {
    /// "A01", as the box labels it. Shares
    /// [`crate::device::slot_label`] with [`crate::session::PatternRef::label`]
    /// rather than spelling the format out again — a `Source` outlives the slot
    /// it was fetched from, so the two types stay separate, but the one rule
    /// they both need does not.
    pub fn label(&self) -> String {
        crate::device::slot_label(self.bank as usize, self.index)
    }
}

/// One box's pattern: up to 16 tracks, sized by the device model.
///
/// Note what is *absent*. The DT2/DN2 pattern struct has a tempo field
/// (`pattern.tempo_offset`) and this does not mirror it, deliberately — tempo is
/// the session's, one clock the studio masters, and minimal diff must leave
/// those bytes alone. PLAN.md §7 rule 8. Swing is here because on the box it
/// genuinely is one per-pattern byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pattern {
    pub name: String,
    /// 50..=80, as the box stores it.
    pub swing: u8,
    /// Private so the `len() == model.num_tracks` invariant cannot be broken by
    /// a push or a truncate. Read through `tracks()`, write through
    /// `track_mut()`; neither can change the length.
    tracks: Vec<Arc<Track>>,
    #[serde(default)]
    pub source: Option<Source>,
}

// A `dump_baseline: Option<DumpBaseline>` field lived here for one (uncommitted)
// day — the A4's received dump, 26 KB of hex per pattern in the project file,
// carried because "a box that answers no dump request cannot be re-read for its
// unmapped bytes at write time". It can (2026-08-31, PLAN.md §10 "The A4
// answers dump requests"), so the A4's write now re-fetches exactly as a digi's
// does and the pattern carries nothing a slot operation could go stale on.
// `serde` ignores unknown fields, so a project file written during that day
// still loads; its baseline is simply dropped.

impl Pattern {
    /// The only way to build a pattern, which is how the track-count invariant
    /// is enforced rather than merely documented.
    pub fn for_model(model: &DeviceModel) -> Self {
        let tracks = (0..model.num_tracks)
            .map(|i| Arc::new(Track::new(i, model.default_track_kind)))
            .collect();
        Self {
            name: String::from("Pattern"),
            swing: 50,
            tracks,
            source: None,
        }
    }

    pub fn tracks(&self) -> &[Arc<Track>] {
        &self.tracks
    }

    pub fn num_tracks(&self) -> usize {
        self.tracks.len()
    }

    pub fn track(&self, index: usize) -> Option<&Track> {
        self.tracks.get(index).map(|t| t.as_ref())
    }

    /// Copy-on-write: cloning a `Session` shares every track until one is
    /// actually edited, so a snapshot costs pointer bumps rather than notes.
    pub fn track_mut(&mut self, index: usize) -> Option<&mut Track> {
        self.tracks.get_mut(index).map(Arc::make_mut)
    }

    /// Whether this pattern still matches the model it claims to belong to. A
    /// hand-edited project file is the case this exists for; `Project::load`
    /// checks every pattern.
    pub fn matches(&self, model: &DeviceModel) -> bool {
        self.tracks.len() == model.num_tracks
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    NamelessPLockLane,
    /// A pattern carried a different number of tracks than its device model
    /// says the box has.
    TrackCountMismatch {
        device: String,
        model_key: &'static str,
        expected: usize,
        found: usize,
    },
    UnknownModel(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NamelessPLockLane => {
                write!(f, "a p-lock lane needs either a parameter name or a paramId")
            }
            Self::TrackCountMismatch {
                device,
                model_key,
                expected,
                found,
            } => write!(
                f,
                "device {device:?} is a {model_key}, which has {expected} tracks, \
                 but a pattern carries {found}"
            ),
            Self::UnknownModel(key) => write!(f, "no device model named {key:?}"),
        }
    }
}

impl std::error::Error for ModelError {}
