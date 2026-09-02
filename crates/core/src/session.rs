// A session: several boxes at once, each with its own pattern slots, held
// together by scenes.
//
// The target case is a DT2 and a DN2 sequenced side by side — 16 tracks each,
// 32 in a session. Tempo lives here because there is one clock the studio
// masters; swing stays on the pattern because on the box it genuinely is one
// per-pattern byte.

use std::collections::BTreeMap;

use digi_protocol::device::DeviceIdentity;
use serde::{Deserialize, Serialize};

use crate::chords::Harmony;
use crate::device::{model_for_slug, Device, DeviceId, DeviceModel, PortEnd, PortRef};
use crate::model::{ModelError, Pattern, Track};
use crate::song::{Song, SongRow};

/// A pattern slot, addressed the way the box addresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternRef {
    pub bank: u8,
    /// 0-based within the bank, so A01 is `{ bank: 0, index: 0 }`.
    pub index: u8,
}

impl PatternRef {
    pub fn new(bank: u8, index: u8) -> Self {
        Self { bank, index }
    }

    /// Position in a device's flat `patterns` vec. Sixteen slots to a bank, as
    /// on the hardware.
    pub fn slot(&self) -> usize {
        self.bank as usize * 16 + self.index as usize
    }

    pub fn from_slot(slot: usize) -> Self {
        Self {
            bank: (slot / 16) as u8,
            index: (slot % 16) as u8,
        }
    }

    /// "A01", as the box labels it.
    pub fn label(&self) -> String {
        crate::device::slot_label(self.bank as usize, self.index)
    }

    /// The inverse of [`label`](Self::label): "A01" → bank 0, index 0.
    ///
    /// Returns `None` rather than a guess for anything that is not a bank letter
    /// followed by 1–16 — a mistyped slot must not silently become A01, for the
    /// same reason `model_for_key` refuses to default an unknown project.
    ///
    /// `bank_letter` wraps at 26, so this round-trips for banks 0–25. That is
    /// well past the 16 banks (A–P) either box has, and a slot past P would not
    /// fit the `u8` the wire uses for a pattern index anyway.
    pub fn from_label(label: &str) -> Option<Self> {
        let mut chars = label.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        if !letter.is_ascii_uppercase() {
            return None;
        }
        let n: u16 = chars.as_str().parse().ok()?;
        if !(1..=16).contains(&n) {
            return None;
        }
        Some(Self::new(letter as u8 - b'A', (n - 1) as u8))
    }

    /// The flat slot number a dump request carries, or `None` for a slot past
    /// what one byte can address.
    ///
    /// A pattern index on the wire is a single byte, so this refuses anything
    /// past bank P rather than wrapping into a different pattern — the same
    /// refusal [`from_label`](Self::from_label) makes, at the other end of the
    /// path. It lives here beside [`slot`](Self::slot) for the reason
    /// `from_label` does: a fetch is asked for from the UI and from the hardware
    /// example, and a copy in each is a copy that can drift, with only one of
    /// them anywhere a test runs.
    pub fn wire_index(&self) -> Option<u8> {
        u8::try_from(self.slot()).ok()
    }
}

/// One pattern per device, chosen together. Switching scene switches every box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub name: String,
    /// PLAN.md §2 writes this as a `HashMap`. It is a `BTreeMap` so that saving
    /// an unchanged project twice produces identical bytes — the same
    /// determinism argument that made `encode_track_notes` a `BTreeMap` in
    /// Phase 1, where a `HashMap` was a real bug.
    pub slots: BTreeMap<DeviceId, PatternRef>,
}

impl Scene {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            slots: BTreeMap::new(),
        }
    }

    pub fn with_slot(mut self, device: DeviceId, slot: PatternRef) -> Self {
        self.slots.insert(device, slot);
        self
    }
}

/// Why an identity reply could not be written into a device. Every arm is a
/// case where guessing would put a box's ports on the wrong device, so none of
/// them falls back to a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindError {
    /// The box named itself something the device table does not have. It stays
    /// unbound rather than becoming the nearest model we do know.
    UnknownModel { slug: String, name: String },
    /// The session has no box of that model to bind to — add one first.
    NoDeviceOfModel(&'static DeviceModel),
    /// Several devices of that model, none on these ports and none free.
    Ambiguous {
        model: &'static DeviceModel,
        candidates: Vec<DeviceId>,
    },
    ModelMismatch {
        device: DeviceId,
        expected: &'static DeviceModel,
        found: &'static DeviceModel,
    },
    NoSuchDevice(DeviceId),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindError::UnknownModel { slug, name } => {
                write!(f, "{name} (slug {slug}) is not a model this build knows")
            }
            BindError::NoDeviceOfModel(m) => {
                write!(f, "no {} in this session — add one to bind it", m.display)
            }
            BindError::Ambiguous { model, candidates } => write!(
                f,
                "{} {}s in this session, all bound elsewhere — say which one",
                candidates.len(),
                model.display
            ),
            BindError::ModelMismatch { expected, found, .. } => {
                write!(f, "that box is a {}, but this device is a {}", expected.display, found.display)
            }
            BindError::NoSuchDevice(id) => write!(f, "no device {} in this session", id.0),
        }
    }
}

impl std::error::Error for BindError {}

fn model_for(identity: &DeviceIdentity) -> Result<&'static DeviceModel, BindError> {
    model_for_slug(&identity.slug).ok_or_else(|| BindError::UnknownModel {
        slug: identity.slug.clone(),
        name: identity.name.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub name: String,
    /// One clock for the whole session, sent to every device that takes it.
    /// Never written to a box: the DT2/DN2 pattern struct has a tempo field and
    /// the model deliberately does not mirror it, so minimal diff leaves those
    /// bytes alone (PLAN.md §7 rule 8).
    pub tempo_bpm: f64,
    pub devices: Vec<Device>,
    pub scenes: Vec<Scene>,
    pub current_scene: usize,
    /// The key the roll tints its rows by, and the chord-draw settings — one per
    /// session rather than one per pattern, because a session is in a key and
    /// Phase 7's generator reads this one rather than inventing its own.
    ///
    /// **Not undoable**, and it falls out of `history::Content` holding patterns
    /// only rather than out of any rule here: a key change edits no note. Same
    /// line `js/main.js` draws around its own snapshots.
    #[serde(default)]
    pub harmony: Harmony,
    /// The arrangement: rows of scenes, played in order. `None` until somebody
    /// builds one, which is what lets a project written before song mode load
    /// unchanged and what makes "is there a song at all?" a single question the
    /// transport can ask.
    ///
    /// Song mode is a *transport* setting rather than a field here, for the same
    /// reason the sounding scene is: which arrangement exists is a property of
    /// the session, and whether the engine is walking it is a property of the
    /// engine. See `EngineLink::set_song_mode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub song: Option<Song>,
    /// The Generate panel's settings — genre, progression, seed, feel, and the
    /// part rows with their destinations — carried so a session recalls the
    /// arrangement it was written by, not only the notes that came out.
    ///
    /// **Opaque on purpose.** These are `generator::context::GenContext`, and
    /// `core` cannot name that type: `generator` already depends on `core`, so
    /// a field of it would close a dependency cycle. Core therefore carries the
    /// value and never reads it, the same bargain `DeviceModel::sysex` strikes
    /// with `protocol` — the layer that owns the meaning does the encoding, and
    /// here that is `app::ui::generate`, which depends on both crates.
    ///
    /// `None` for a project written before this field, and for a session whose
    /// Generate panel was never opened. A file whose value no longer
    /// deserializes — a genre removed, say — loses these settings and keeps
    /// every note: the panel falls back to its defaults rather than refusing to
    /// open, which is why this is a `Value` here and not a string the loader
    /// would have to prove.
    ///
    /// **Not undoable**, for `harmony`'s reason above: changing a slider edits
    /// no note, and `history::Content` snapshots patterns only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<serde_json::Value>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            name: String::from("Session"),
            tempo_bpm: 120.0,
            devices: Vec::new(),
            scenes: vec![Scene::new("Scene 1")],
            current_scene: 0,
            harmony: Harmony::default(),
            song: None,
            generator: None,
        }
    }
}

impl Session {
    pub fn device(&self, id: DeviceId) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn device_mut(&mut self, id: DeviceId) -> Option<&mut Device> {
        self.devices.iter_mut().find(|d| d.id == id)
    }

    /// A name for the next box of this model: the model key while it is free,
    /// then "DT2 2", "DT2 3" and so on.
    ///
    /// Names are the user's — this only picks the starting point, for the two
    /// callers that add a device without a person typing one: auto-connect
    /// discovering a box, and Setup's "Add a box". Two boxes *may* share a name
    /// (nothing keys on it; identity is [`DeviceId`]), but a desk with two rows
    /// both called "DT2" is a desk where a scene picker stops meaning anything.
    pub fn suggested_name(&self, model: &DeviceModel) -> String {
        let taken = |name: &str| self.devices.iter().any(|d| d.name == name);
        if !taken(model.key) {
            return model.key.to_string();
        }
        (2..)
            .map(|n| format!("{} {n}", model.key))
            .find(|name| !taken(name))
            .expect("some counter is always free")
    }

    /// Adds the device and points every existing scene at its first slot, so a
    /// scene is never silently missing a box that is in the session.
    pub fn add_device(&mut self, device: Device) -> DeviceId {
        let id = device.id;
        self.devices.push(device);
        for scene in &mut self.scenes {
            scene.slots.entry(id).or_insert(PatternRef::new(0, 0));
        }
        id
    }

    pub fn remove_device(&mut self, id: DeviceId) {
        self.devices.retain(|d| d.id != id);
        for scene in &mut self.scenes {
            scene.slots.remove(&id);
        }
        // A song row's mute mask is per box, so it goes with the box. A re-added
        // device gets a fresh id and would otherwise inherit the mutes of the one
        // it replaced.
        if let Some(song) = &mut self.song {
            song.device_removed(id);
        }
    }

    pub fn scene(&self, index: usize) -> Option<&Scene> {
        self.scenes.get(index)
    }

    /// Append a scene, pointing every device where `like` points, or at each
    /// box's first slot when there is no scene to copy.
    ///
    /// Copying rather than starting blank because a scene is nearly always a
    /// variation on the one being played: a blank one would silently put every
    /// box back on A01, which is a scene nobody asked for. Returns its index.
    pub fn add_scene(&mut self, name: impl Into<String>, like: Option<usize>) -> usize {
        let mut scene = Scene::new(name);
        scene.slots = match like.and_then(|i| self.scenes.get(i)) {
            Some(source) => source.slots.clone(),
            None => self
                .devices
                .iter()
                .map(|d| (d.id, PatternRef::new(0, 0)))
                .collect(),
        };
        // A scene copied from one that predates a device would be missing it.
        for device in &self.devices {
            scene.slots.entry(device.id).or_insert(PatternRef::new(0, 0));
        }
        self.scenes.push(scene);
        self.scenes.len() - 1
    }

    /// Remove a scene, keeping [`Session::current_scene`] pointing at one that
    /// exists.
    ///
    /// Refuses to remove the last scene: `current_scene` is an index, not an
    /// option, and every path that plays anything resolves a pattern through a
    /// scene. A session with no scene would be a session that cannot sound.
    pub fn remove_scene(&mut self, index: usize) -> bool {
        if index >= self.scenes.len() || self.scenes.len() == 1 {
            return false;
        }
        self.scenes.remove(index);
        // Removing a scene before the current one shifts it down; removing the
        // current one lands on whatever took its place, or on the last.
        if self.current_scene > index {
            self.current_scene -= 1;
        }
        self.current_scene = self.current_scene.min(self.scenes.len() - 1);
        // Every song row above the hole shifts down with the list, and a row that
        // named the scene that just went lands on the one that took its place.
        // Left alone, a row would name a scene that no longer exists, and the
        // song would stall at it.
        if let Some(song) = &mut self.song {
            song.scene_removed(index, self.current_scene);
        }
        true
    }

    /// Point a device at a different slot in a scene. `false` if either the
    /// scene or the device is not in this session — a slot must never be written
    /// for a box that is not there, or the scene would name a device nothing can
    /// play.
    pub fn set_slot_in_scene(&mut self, scene: usize, device: DeviceId, slot: PatternRef) -> bool {
        if self.device(device).is_none() {
            return false;
        }
        match self.scenes.get_mut(scene) {
            Some(s) => {
                s.slots.insert(device, slot);
                true
            }
            None => false,
        }
    }

    /// Which slot `device` is on in `scene`.
    pub fn slot_in_scene(&self, scene: usize, device: DeviceId) -> Option<PatternRef> {
        self.scenes.get(scene)?.slots.get(&device).copied()
    }

    /// The pattern `device` plays in `scene` — the whole point of a scene.
    pub fn pattern_in_scene(&self, scene: usize, device: DeviceId) -> Option<&Pattern> {
        let slot = self.slot_in_scene(scene, device)?;
        self.device(device)?.pattern(slot.slot())
    }

    /// The pattern `device` is playing now.
    pub fn current_pattern(&self, device: DeviceId) -> Option<&Pattern> {
        self.pattern_in_scene(self.current_scene, device)
    }

    /// Where a scene change may take effect: the boundary of the *longest* track
    /// across every device in the outgoing scene, so a polymetric track is not
    /// cut mid-cycle (PLAN.md §4). `None` if the scene names no playable track.
    pub fn scene_boundary_steps(&self, scene: usize) -> Option<u16> {
        self.devices
            .iter()
            .filter_map(|d| self.pattern_in_scene(scene, d.id))
            .flat_map(|p| p.tracks().iter().map(|t| t.length_steps))
            .max()
    }

    /// The arrangement, if this session has one.
    pub fn song(&self) -> Option<&Song> {
        self.song.as_ref()
    }

    /// The arrangement, creating an empty one if there is none.
    ///
    /// The only thing that ever makes a song exist. `None` has to keep meaning
    /// *nobody has built an arrangement*, so a read must never conjure one: that
    /// is the difference between a project that saves a `song` key and one that
    /// stays byte-identical to what a pre-song-mode build wrote.
    pub fn song_mut(&mut self) -> &mut Song {
        self.song.get_or_insert_with(Song::default)
    }

    /// A row of the arrangement.
    pub fn song_row(&self, index: usize) -> Option<&SongRow> {
        self.song.as_ref()?.row(index)
    }

    /// Which scene a song row plays, or `None` for a row naming a scene this
    /// session does not have.
    pub fn scene_of_row(&self, index: usize) -> Option<usize> {
        let row = self.song_row(index)?;
        (row.scene < self.scenes.len()).then_some(row.scene)
    }

    /// Add a row playing the scene being edited, taking each box's mute state
    /// from the pattern it plays there — the box's "the row's mute state
    /// initially reflects the pattern's mute state".
    ///
    /// Returns the new row's index, or `None` when the song is full.
    pub fn add_song_row(&mut self, scene: usize) -> Option<usize> {
        let mut row = SongRow::new(scene.min(self.scenes.len().saturating_sub(1)));
        // Read the mutes before taking the song mutably: both borrow `self`.
        let mutes: Vec<(DeviceId, Vec<bool>)> = self
            .devices
            .iter()
            .filter_map(|d| {
                let pattern = self.pattern_in_scene(row.scene, d.id)?;
                Some((d.id, pattern.tracks().iter().map(|t| t.mute).collect()))
            })
            .collect();
        for (device, muted) in mutes {
            // Only when the pattern actually has something muted. A row that
            // adopts an all-unmuted mask has stopped inheriting for no reason,
            // and the panel would then show it as an override.
            if muted.iter().any(|m| *m) {
                row.adopt_mutes(device, muted);
            }
        }
        self.song_mut().push(row)
    }

    /// Solo is session-wide, not per device: soloing a DT2 track silences DN2
    /// tracks too, which is the only reading that makes sense at a mixing desk.
    pub fn any_solo(&self) -> bool {
        self.devices.iter().any(|d| {
            d.patterns
                .iter()
                .any(|p| p.tracks().iter().any(|t| t.solo))
        })
    }

    /// Whether a track sounds, given session-wide solo. Callers pass the track
    /// they are about to schedule.
    pub fn track_audible(&self, track: &Track) -> bool {
        crate::song::audible(None, track, self.any_solo())
    }

    /// Every device's patterns match its model's track count, and every scene
    /// names a slot that exists.
    pub fn validate(&self) -> Result<(), ModelError> {
        for device in &self.devices {
            device.validate()?;
        }
        Ok(())
    }

    /// Re-point every device at the ports actually connected now. Returns the
    /// devices whose I/O could not be restored — the UI says so rather than
    /// failing later at write time.
    pub fn rebind_ports(
        &mut self,
        available_in: &[PortRef],
        available_out: &[PortRef],
    ) -> Vec<DeviceId> {
        let mut unbound = Vec::new();
        for device in &mut self.devices {
            let had_ports = device.io.input.is_some() || device.io.output.is_some();
            if !device.rebind_ports(available_in, available_out) && had_ports {
                unbound.push(device.id);
            }
        }
        unbound
    }

    /// Point one end of a device's I/O at a port by hand, or at nothing.
    ///
    /// This is the path that does not need an Elektron on the desk: an IAC bus, a
    /// soft synth, or a box whose handshake this build does not know. Identify is
    /// otherwise the only thing that ever gives a device a port, which made the
    /// whole app untestable without hardware.
    ///
    /// Two rules it carries, both borrowed from [`Session::bind_identity_to`]
    /// rather than reinvented:
    ///
    /// 1. **A port belongs to one box.** Whoever else held this end of it loses
    ///    it, or two devices would send to the same socket. Only the *same* end is
    ///    released: an input and an output may share a name and are still two
    ///    different ports.
    /// 2. **Moving a device off its ports drops the OS report.** `build` and
    ///    `version` are what answered on the ports the handshake went out on, so
    ///    once either end moves they describe a box that is no longer there.
    ///    Showing "OS 0070" beside a hand-picked IAC bus would be exactly the
    ///    plausible-looking lie that leaving a device visibly unbound exists to
    ///    avoid. Re-identify to get them back.
    ///
    /// Returns whether the session actually changed — an unknown device or a
    /// pick that names the port already there both leave it alone.
    pub fn set_device_port(
        &mut self,
        device: DeviceId,
        end: PortEnd,
        port: Option<PortRef>,
    ) -> bool {
        let Some(current) = self.device(device).map(|d| d.port(end).cloned()) else {
            return false;
        };
        if current.as_ref() == port.as_ref() {
            return false;
        }

        if let Some(port) = &port {
            for d in self.devices.iter_mut().filter(|d| d.id != device) {
                d.release_port(end, port);
            }
        }

        let d = self.device_mut(device).expect("checked just above");
        d.set_port(end, port);
        d.io.build = None;
        d.io.version = None;
        true
    }

    /// Bind an identity reply to the device in this session it belongs to.
    ///
    /// The slug the handshake reported picks the model; the model plus the ports
    /// pick the instance. Which matters because identity is the *instance*: two
    /// DT2s on one host are two devices, and only the ports tell them apart.
    ///
    /// Order of preference, and each step is a real case:
    ///
    /// 1. a device already on these ports — re-identifying after an OS update
    ///    must update that box, not claim another one;
    /// 2. the first device of that model with no ports at all — a fresh session
    ///    being told what is plugged in;
    /// 3. the only device of that model, even if it is on other ports — the box
    ///    moved to a different socket;
    /// 4. otherwise `Ambiguous`: several of that model, all already elsewhere.
    ///    Guessing here would silently re-point the wrong box, so the caller has
    ///    to name one via [`Session::bind_identity_to`].
    pub fn bind_identity(
        &mut self,
        identity: &DeviceIdentity,
        input: PortRef,
        output: PortRef,
    ) -> Result<DeviceId, BindError> {
        let model = model_for(identity)?;
        let candidates: Vec<DeviceId> = self
            .devices
            .iter()
            .filter(|d| d.model == model)
            .map(|d| d.id)
            .collect();

        let on_these_ports = self
            .devices
            .iter()
            .find(|d| d.model == model && d.is_on_ports(&input, &output))
            .map(|d| d.id);
        let unbound = self
            .devices
            .iter()
            .find(|d| d.model == model && !d.has_ports())
            .map(|d| d.id);

        let chosen = match (on_these_ports, unbound, candidates.len()) {
            (Some(id), _, _) => id,
            (None, Some(id), _) => id,
            (None, None, 0) => return Err(BindError::NoDeviceOfModel(model)),
            (None, None, 1) => candidates[0],
            (None, None, _) => return Err(BindError::Ambiguous { model, candidates }),
        };
        self.bind_identity_to(chosen, identity, input, output)?;
        Ok(chosen)
    }

    /// Bind a reply to a device the caller names, for when [`Session::bind_identity`]
    /// could not choose. The model still has to agree: a Digitone II reply is
    /// refused for a DT2 device rather than overwriting what it is.
    pub fn bind_identity_to(
        &mut self,
        device: DeviceId,
        identity: &DeviceIdentity,
        input: PortRef,
        output: PortRef,
    ) -> Result<(), BindError> {
        let model = model_for(identity)?;
        let found = self
            .device(device)
            .ok_or(BindError::NoSuchDevice(device))?
            .model;
        if found != model {
            return Err(BindError::ModelMismatch { device, expected: model, found });
        }
        // A port belongs to one box: whoever else held either end loses it,
        // or two devices would send to the same socket.
        for d in self.devices.iter_mut().filter(|d| d.id != device) {
            d.release_ports(&input, &output);
        }
        self.device_mut(device)
            .expect("checked just above")
            .apply_identity(identity, input, output);
        Ok(())
    }

    /// The highest device id in this session, so a loader can push the id
    /// counter past it.
    pub fn highest_device_id(&self) -> u64 {
        self.devices.iter().map(|d| d.id.0).max().unwrap_or(0)
    }
}
