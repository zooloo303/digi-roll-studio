// The device table, and one physical box in a session.
//
// The table is *data*. Track count is a field, never a constant and never an
// enum arm with a hard-coded 16 in it — which is what let the A4 become a row
// here (2026-08-24) instead of a model rewrite, exactly as planned. v1 shipped
// DT2 and DN2 only; the A4 is the first `sysex: None` row to ship, and the
// tests still prove the shape by constructing a live-only model this crate
// does not ship.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use digi_protocol::device::DeviceIdentity;
use serde::{Deserialize, Serialize};

use crate::model::{ModelError, Pattern, TrackKind};

/// `dt2_spec()`/`dn2_spec()` build a `Spec` by value, so each is built once here
/// and handed out by reference.
static DT2_SPEC: LazyLock<digi_protocol::pattern::Spec> =
    LazyLock::new(digi_protocol::pattern::dt2_spec);
static DN2_SPEC: LazyLock<digi_protocol::pattern::Spec> =
    LazyLock::new(digi_protocol::pattern::dn2_spec);

fn dt2_spec() -> &'static digi_protocol::pattern::Spec {
    &DT2_SPEC
}
fn dn2_spec() -> &'static digi_protocol::pattern::Spec {
    &DN2_SPEC
}

/// How a model reaches its `Spec`.
///
/// PLAN.md §2 writes this field as `Option<&'static Spec>`. It is a function
/// pointer instead, for one reason: a `Spec` is built at runtime, so a
/// `&'static Spec` can only come from a `LazyLock`, and a `LazyLock` cannot be
/// dereferenced inside a `static` initialiser. The alternative was to leave the
/// field `None` in the table and patch it in a lookup — which would make
/// `DT2.sysex.is_some()` read as "the DT2 cannot do SysEx", the exact opposite
/// of the truth. The semantic PLAN.md actually specifies is untouched: `None`
/// still means sequence-live-only.
pub type SpecFn = fn() -> &'static digi_protocol::pattern::Spec;

/// How a whole pattern moves between the app and a box.
///
/// **Added 2026-08-31, because `sysex.is_some()` had stopped answering the
/// question people were asking it.** That field means "has a gen-2 `Spec`", and
/// for two years that was the same set as "can transfer a pattern at all". The
/// Analog Four separates them: it has no `Spec` and never will, and it moves
/// whole patterns perfectly well. `ui::presets` had already had to write a
/// paragraph explaining why it does *not* ask `can_sysex`; that is one place
/// working around a name, and a second would have been a pattern.
///
/// A route is data on the row, per PLAN.md §6's second carried-forward decision
/// — a new Elektron box should be a table entry — rather than a `key == "A4"`
/// test wearing a capability's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternRoute {
    /// None. The box plays live, takes clock and answers CC, and its patterns
    /// stay on it.
    LiveOnly,
    /// **Request and reply, gen-2.** The app sends a `0x60` dump request and
    /// the box answers; `sysex` carries the layout to read the answer with.
    /// Both digis.
    Request,
    /// **Request and reply, gen-1.** The app sends a `0x64` and the box answers
    /// a pattern in `protocol::a4_pattern`'s layout; there is no gen-2 `Spec`
    /// and never will be. The write back is the dump message itself, DIN-paced
    /// (`digi_midi::a4_transfer`), through `safe_write`'s A4 flow — re-fetch,
    /// backup, confirm, verify, all of it. The Analog Four.
    ///
    /// This variant was `FrontPanelDump` until 2026-08-31 — "there is no
    /// request to send" — and that was a misreading of the box's advertised
    /// opcode list, which describes a different namespace (PLAN.md §10, "The A4
    /// answers dump requests"). The box still *can* push a dump from its own
    /// front panel; what died is the claim that this was the only way in.
    RequestGen1,
}

impl PatternRoute {
    /// Whether a pattern can cross between box and app by any route at all.
    ///
    /// This is the question `can_sysex` gets asked and does not answer.
    pub fn transfers(self) -> bool {
        !matches!(self, Self::LiveOnly)
    }

    /// What a box list says about this box in three words.
    ///
    /// Both request routes read "fetch + write" on purpose: which generation of
    /// dump protocol a box speaks is this app's plumbing, not a fact a person
    /// routing a desk acts on. The two panels behave identically, so the label
    /// must too.
    pub fn label(self) -> &'static str {
        match self {
            Self::LiveOnly => "live only",
            Self::Request | Self::RequestGen1 => "fetch + write",
        }
    }
}

/// How a **preset** gets from a box's +Drive onto one of its tracks — the
/// browser's double-click, and a second axis from [`PatternRoute`] rather than a
/// consequence of it.
///
/// **Added 2026-09-01, when the A4 turned out to have a path after all.** Until
/// then `ui::presets` keyed this off `pattern_route()`, on the reasoning that
/// the gen-2 dump namespace is where the kit-track read/store pair lives and
/// only the digis speak it. The first half of that is still true and the
/// conclusion was still wrong: the A4 reaches a kit track through its *kit*
/// (`0x68` → splice → `0x58`), which is a different message and the same
/// feature. Two boxes now load presets by two routes, so the route is a field
/// on the row — PLAN.md §6's carried-forward decision, for the third time.
///
/// Each variant carries how many of the box's tracks have a sound at all, which
/// is not `num_tracks`: an A4 sequences six and its kit holds four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetLoad {
    /// No path mapped. A live-only box, or one whose +Drive this build can
    /// browse and whose kit it cannot write.
    None,
    /// **One kit track's sound, addressed directly** — `0x6b` reads it and
    /// `0x5b` puts one back, and a load is `midi::preset_load`'s five round
    /// trips. Both digis.
    KitTrackSound { slots: usize },
    /// **A whole working kit, read-modify-written** — `0x68` fetches the box's
    /// edit buffer, 350 of its 2,410 bytes are replaced, and `0x58` sends it
    /// back (`midi::a4_preset_load`). The Analog Four.
    ///
    /// The same recovery story as the other route, because both write the kit
    /// the box is playing rather than a stored slot: reloading the pattern on
    /// the box discards an unsaved kit, and that is the undo.
    WorkingKitSplice { slots: usize },
}

impl PresetLoad {
    /// Whether a preset can be put on one of this box's tracks at all.
    pub fn loads(self) -> bool {
        !matches!(self, Self::None)
    }

    /// How many of this box's tracks a kit has a sound for. Zero when nothing
    /// loads.
    pub fn slots(self) -> usize {
        match self {
            Self::None => 0,
            Self::KitTrackSound { slots } | Self::WorkingKitSplice { slots } => slots,
        }
    }

    /// What the tracks past [`Self::slots`] are, for a refusal that has to say
    /// why a selected track has no slot — or `None` where every track has one.
    ///
    /// A sentence rather than a count because the answer is about the box: the
    /// A4's fifth and sixth tracks are not missing sounds, they are the FX and
    /// CV tracks and have never had one.
    pub fn extra_tracks(self) -> Option<&'static str> {
        match self {
            Self::WorkingKitSplice { .. } => {
                Some("the FX and CV tracks sequence, and have no sound to put a preset on")
            }
            _ => None,
        }
    }
}

/// Everything the model needs to know about a box.
///
/// `sysex: None` means the box has no **gen-2** pattern `Spec`. That is *not*
/// the same as "cannot transfer a pattern", and has not been since the A4's
/// gen-1 dump format was mapped on 2026-08-30 — see [`PatternRoute`], which is
/// the field to ask.
#[derive(Debug, Clone, Copy)]
pub struct DeviceModel {
    /// Stable id in the project file. Never rename one of these.
    pub key: &'static str,
    pub display: &'static str,
    /// Links an identity handshake to this model. `protocol::device::Product`
    /// answers "who is this box on the wire"; this answers "how many tracks does
    /// it have". Same box, two different questions.
    pub slug: Option<&'static str>,
    pub num_tracks: usize,
    pub max_steps: u16,
    pub default_track_kind: TrackKind,
    pub sysex: Option<SpecFn>,
    /// How a whole pattern gets on and off this box.
    pub pattern_route: PatternRoute,
    /// How a +Drive preset gets onto one of this box's tracks, and how many of
    /// its tracks have a sound.
    pub preset_load: PresetLoad,
    /// How many slots a dump request can name on this box — what the `from`
    /// picker of a fetch and the `to` picker of a send offer. The digis' wire
    /// index spans sixteen banks of sixteen; the A4's spans eight
    /// (`0x64`'s index is linear 0–127, hardware-verified 2026-08-31), and a
    /// picker offering I01 to a box whose banks stop at H would be offering a
    /// request nobody has ever seen answered. Zero for a live-only box.
    pub wire_slots: usize,
}

impl DeviceModel {
    /// Whether this box has a gen-2 `Spec` — so it can be fetched with a `0x60`
    /// request and written through `safe_write`.
    ///
    /// **Not the same as "can transfer a pattern".** Ask
    /// [`DeviceModel::pattern_route`] for that: an A4 returns false here and
    /// moves patterns both ways. The name is kept because it is what the
    /// gen-2-only paths — `ui::write`, `ui::sync`, `ui::transfer` — actually
    /// mean when they ask.
    pub fn can_sysex(&self) -> bool {
        self.sysex.is_some()
    }

    /// How a whole pattern moves between this box and the app.
    pub fn pattern_route(&self) -> PatternRoute {
        self.pattern_route
    }

    /// How a preset gets from this box's +Drive onto one of its tracks.
    pub fn preset_load(&self) -> PresetLoad {
        self.preset_load
    }

    /// The byte-level spec for this box, or `None` for a live-only model.
    /// `core` treats this as an opaque handle and never parses with it.
    pub fn spec(&self) -> Option<&'static digi_protocol::pattern::Spec> {
        self.sysex.map(|f| f())
    }

    /// How many p-lock lanes one of this box's patterns holds — the pool every
    /// track in a slot shares, and the number a caller has to arbitrate against
    /// before it promises a write will fit.
    ///
    /// **Two pools, because there are two pattern formats.** A digi's count
    /// comes off its `Spec` (80); the A4's gen-1 pool is
    /// [`digi_protocol::a4_plocks::NUM_LANES`] and no `Spec` describes it, which
    /// is why asking `spec()` for this silently answered "no pool" on a box that
    /// has 128 lanes. Zero for a live-only box, which holds no pattern here.
    pub fn plock_pool(&self) -> usize {
        match self.spec() {
            Some(spec) => spec.pattern.num_p_locks,
            None if matches!(self.pattern_route, PatternRoute::RequestGen1) => {
                digi_protocol::a4_plocks::NUM_LANES
            }
            None => 0,
        }
    }
}

impl PartialEq for DeviceModel {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for DeviceModel {}

pub static DT2: DeviceModel = DeviceModel {
    key: "DT2",
    display: "Digitakt II",
    slug: Some("digitakt2"),
    num_tracks: 16,
    max_steps: 128,
    default_track_kind: TrackKind::Audio,
    sysex: Some(dt2_spec),
    pattern_route: PatternRoute::Request,
    preset_load: PresetLoad::KitTrackSound { slots: 16 },
    wire_slots: 256,
};

pub static DN2: DeviceModel = DeviceModel {
    key: "DN2",
    display: "Digitone II",
    slug: Some("digitone2"),
    num_tracks: 16,
    max_steps: 128,
    default_track_kind: TrackKind::Audio,
    sysex: Some(dn2_spec),
    pattern_route: PatternRoute::Request,
    preset_load: PresetLoad::KitTrackSound { slots: 16 },
    wire_slots: 256,
};

/// The Analog Four mk1 — shipped live-only on 2026-08-24, corrected against
/// the box on 2026-08-28, and a full transfer peer of the digis since
/// 2026-08-31.
///
/// Six tracks as the sequencer counts them: four synth voices, the FX track
/// and the CV track. 64 steps — four pages of sixteen, half a digi pattern.
/// Eight banks of sixteen patterns, which is what `wire_slots: 128` records.
///
/// `sysex: None` because there is no gen-2 `Spec` here and there will not be —
/// but that stopped meaning "cannot transfer" twice over. This comment claimed
/// "nothing can ask it for a pattern" on the strength of the box's advertised
/// opcode list; that list describes the API namespace only, and on 2026-08-31
/// the box answered `0x60`–`0x6d` dump requests (PLAN.md §10, "The A4 answers
/// dump requests"). So the route is [`PatternRoute::RequestGen1`]: fetched and
/// written from the same panels as the digis, in `protocol::a4_pattern`'s
/// layout, with the write DIN-paced.
///
/// It also sequences live, takes clock, and answers CC/NRPN (see
/// `protocol::params::A4_PARAMS`).
///
/// It answers the identity handshake like every other shipped box — product id
/// 4, name "Analog Four", OS 1.55B — which the 2026-08-24 guess had assumed it
/// could not. `protocol::device::PRODUCTS` has its row, dump family 0x06.
pub static A4: DeviceModel = DeviceModel {
    key: "A4",
    display: "Analog Four",
    slug: Some("analogfour"),
    num_tracks: 6,
    max_steps: 64,
    default_track_kind: TrackKind::Audio,
    sysex: None,
    pattern_route: PatternRoute::RequestGen1,
    // Four sounds for six tracks: SYN1-SYN4 have one and the FX and CV tracks
    // do not. `protocol::a4_kit::NUM_SOUNDS` is the same number where the
    // offsets are.
    preset_load: PresetLoad::WorkingKitSplice { slots: 4 },
    wire_slots: 128,
};

/// The shipped roster. DT2 and DN2 per PLAN.md §2; A4 since 2026-08-24,
/// hardware-verified 2026-08-28.
pub static MODELS: &[&DeviceModel] = &[&DT2, &DN2, &A4];

pub fn model_for_key(key: &str) -> Option<&'static DeviceModel> {
    MODELS.iter().copied().find(|m| m.key == key)
}

/// Which model an identity handshake just described.
pub fn model_for_slug(slug: &str) -> Option<&'static DeviceModel> {
    MODELS
        .iter()
        .copied()
        .find(|m| m.slug.is_some_and(|s| s == slug))
}

/// Identity is the *instance*, not the model: two DT2s in one session are two
/// devices, and a scene has to be able to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(pub u64);

static NEXT_DEVICE_ID: AtomicU64 = AtomicU64::new(1);

impl DeviceId {
    pub fn next() -> Self {
        Self(NEXT_DEVICE_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// After loading a project, push the counter past every id the file used, so
    /// a device added afterwards cannot collide with one that came off disk.
    pub fn reserve_past(highest: u64) {
        NEXT_DEVICE_ID.fetch_max(highest + 1, Ordering::Relaxed);
    }
}

/// A MIDI port as the project file remembers it.
///
/// Both fields are kept because they fail differently: the OS id is exact but
/// not stable across replug on every platform, and the name is stable but not
/// unique when two identical boxes are connected. Rebinding tries id first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortRef {
    pub id: String,
    pub name: String,
}

impl PortRef {
    /// Whether two references name the same physical port. Id first, then name,
    /// for the same reason [`Device::rebind_ports`] does: the id is exact, the
    /// name survives a replug that renumbered things.
    pub fn same_port(&self, other: &PortRef) -> bool {
        self.id == other.id || self.name == other.name
    }
}

/// Which end of a device's MIDI I/O a caller means.
///
/// The two ends are separate namespaces at the OS level — an input port and an
/// output port may share a name and never share an id — so every operation that
/// touches one has to say which, or releasing an output would silently unbind an
/// input of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortEnd {
    Input,
    Output,
}

/// Ports and the last thing the box said about itself. Session state, not
/// pattern bytes: a missing port disables this device's I/O and touches nothing
/// else it owns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIo {
    #[serde(default)]
    pub input: Option<PortRef>,
    #[serde(default)]
    pub output: Option<PortRef>,
    /// Set from the identity handshake. Purely informational here; `core` does
    /// no I/O and never asks for it itself.
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Whether this box receives the session's MIDI clock. A box slaved to
    /// something else must not be fought over (PLAN.md §4).
    #[serde(default = "default_true")]
    pub takes_clock: bool,
}

fn default_true() -> bool {
    true
}

/// Written out rather than derived, because `#[derive(Default)]` gives
/// `takes_clock: false` while the serde default gives `true` — so a box added in
/// the app would have sat silently off the clock, and the same box saved and
/// reloaded would have come back on it. A new box follows the session's clock;
/// turning that off is a decision the user makes.
impl Default for DeviceIo {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            build: None,
            version: None,
            takes_clock: default_true(),
        }
    }
}

/// One physical box, with its own bank of pattern slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: DeviceId,
    /// The user's label: "DT2", "DN2", "Adeel's Syntakt".
    pub name: String,
    /// Stored as the model *key*; the table is resolved on load. A project file
    /// must not carry a copy of the device table, or a table fix would need
    /// every old file edited.
    #[serde(rename = "model", with = "model_key")]
    pub model: &'static DeviceModel,
    /// Slots, addressed the way the box addresses them.
    pub patterns: Vec<Arc<Pattern>>,
    #[serde(default)]
    pub io: DeviceIo,
}

impl Device {
    /// `slots` patterns, each already the right shape for the model.
    pub fn new(name: impl Into<String>, model: &'static DeviceModel, slots: usize) -> Self {
        Self {
            id: DeviceId::next(),
            name: name.into(),
            model,
            patterns: (0..slots)
                .map(|i| {
                    let mut p = Pattern::for_model(model);
                    p.name = slot_label(i / 16, (i % 16) as u8);
                    Arc::new(p)
                })
                .collect(),
            io: DeviceIo::default(),
        }
    }

    pub fn pattern(&self, index: usize) -> Option<&Pattern> {
        self.patterns.get(index).map(|p| p.as_ref())
    }

    /// Copy-on-write, as `Pattern::track_mut`.
    pub fn pattern_mut(&mut self, index: usize) -> Option<&mut Pattern> {
        self.patterns.get_mut(index).map(Arc::make_mut)
    }

    pub fn can_sysex(&self) -> bool {
        self.model.can_sysex()
    }

    /// How a whole pattern moves between this box and the app.
    pub fn preset_load(&self) -> PresetLoad {
        self.model.preset_load
    }

    pub fn pattern_route(&self) -> PatternRoute {
        self.model.pattern_route
    }

    /// Every pattern must have exactly the model's track count.
    pub fn validate(&self) -> Result<(), ModelError> {
        for p in &self.patterns {
            if !p.matches(self.model) {
                return Err(ModelError::TrackCountMismatch {
                    device: self.name.clone(),
                    model_key: self.model.key,
                    expected: self.model.num_tracks,
                    found: p.num_tracks(),
                });
            }
        }
        Ok(())
    }

    /// Whether this device is already bound to that pair of ports — either end
    /// counts, because a half-bound device is still *this* box.
    pub fn is_on_ports(&self, input: &PortRef, output: &PortRef) -> bool {
        self.io.input.as_ref().is_some_and(|p| p.same_port(input))
            || self.io.output.as_ref().is_some_and(|p| p.same_port(output))
    }

    pub fn has_ports(&self) -> bool {
        self.io.input.is_some() || self.io.output.is_some()
    }

    /// Forget either port that names one of these — a port belongs to one box.
    pub(crate) fn release_ports(&mut self, input: &PortRef, output: &PortRef) {
        if self.io.input.as_ref().is_some_and(|p| p.same_port(input)) {
            self.io.input = None;
        }
        if self.io.output.as_ref().is_some_and(|p| p.same_port(output)) {
            self.io.output = None;
        }
    }

    /// One end of this device's I/O.
    pub fn port(&self, end: PortEnd) -> Option<&PortRef> {
        match end {
            PortEnd::Input => self.io.input.as_ref(),
            PortEnd::Output => self.io.output.as_ref(),
        }
    }

    /// Point one end at a port, or at nothing.
    ///
    /// Unlike [`Device::apply_identity`] this says nothing about *who* is on the
    /// other side, so it leaves `build`/`version` alone. Callers that are moving
    /// a device away from where a handshake happened are the ones who have to
    /// decide what becomes of that report; [`crate::Session::set_device_port`] is
    /// where that decision is made.
    pub fn set_port(&mut self, end: PortEnd, port: Option<PortRef>) {
        match end {
            PortEnd::Input => self.io.input = port,
            PortEnd::Output => self.io.output = port,
        }
    }

    /// Forget this end if it names that port. The narrow counterpart to
    /// [`Device::release_ports`], for when only one end is being reassigned.
    pub(crate) fn release_port(&mut self, end: PortEnd, port: &PortRef) {
        if self.port(end).is_some_and(|p| p.same_port(port)) {
            self.set_port(end, None);
        }
    }

    /// Write what the box just said about itself into this device: its ports and
    /// its OS. Nothing else — an identity reply is session state and must never
    /// reach a pattern.
    ///
    /// The model is *not* set from the reply. `Session::bind_identity` checks
    /// the two agree before calling this, so a Digitone II handshake can never
    /// quietly turn a DT2 device into something else.
    pub fn apply_identity(&mut self, identity: &DeviceIdentity, input: PortRef, output: PortRef) {
        self.io.input = Some(input);
        self.io.output = Some(output);
        self.io.build = Some(identity.build.clone());
        self.io.version = Some(identity.version.clone());
    }

    /// Re-point this device's ports at what is actually connected now.
    ///
    /// Id first, then name: the id is exact, the name survives a replug that
    /// renumbered things. No match disables I/O for this device and leaves its
    /// patterns alone. Returns whether both ends are bound.
    pub fn rebind_ports(&mut self, available_in: &[PortRef], available_out: &[PortRef]) -> bool {
        self.io.input = self.io.input.take().and_then(|p| rematch(&p, available_in));
        self.io.output = self.io.output.take().and_then(|p| rematch(&p, available_out));
        self.io.input.is_some() && self.io.output.is_some()
    }
}

fn rematch(remembered: &PortRef, available: &[PortRef]) -> Option<PortRef> {
    available
        .iter()
        .find(|p| p.id == remembered.id)
        .or_else(|| available.iter().find(|p| p.name == remembered.name))
        .cloned()
}

/// Banks are lettered as the boxes letter them.
pub fn bank_letter(bank: usize) -> char {
    (b'A' + (bank % 26) as u8) as char
}

/// "A01", as the box labels a slot — bank letter then a 1-based index.
///
/// **One place decides this**, because three grew independently:
/// `PatternRef::label`, `Source::label` and the pattern names built below all
/// spelled the same `format!` out by hand (DEVELOPMENT.md's lesson 5). They are all
/// this function now, so a change to how a slot is written cannot land in two
/// of the three.
pub fn slot_label(bank: usize, index: u8) -> String {
    format!("{}{:02}", bank_letter(bank), index + 1)
}

/// Serialize a `&'static DeviceModel` as its key, and resolve it back through
/// the table on load. An unknown key is an error, never a silent default — a
/// project claiming a box we do not have must not quietly become a DT2.
mod model_key {
    use super::{model_for_key, DeviceModel};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(m: &&'static DeviceModel, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(m.key)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<&'static DeviceModel, D::Error> {
        let key = String::deserialize(d)?;
        model_for_key(&key).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown device model {key:?} — this project was made with a build that had it"
            ))
        })
    }
}
