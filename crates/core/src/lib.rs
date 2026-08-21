// The session model: a session holds boxes, a box holds pattern slots, a slot
// holds tracks, a track holds notes. Four levels, because the hardware has four.
//
// This replaced the old eight-slot browser state, which was a workaround for
// the browser app having no multi-track concept. See PLAN.md §2.
//
// No I/O lives here, and no byte parsing. `protocol::Spec` is a handle, not
// something this crate reads with: `device` holds one per model meaning "this
// model can do SysEx", and `import` hands it straight back to `protocol` to do
// the reading. Every byte of a dump is still parsed on the other side of that
// call.

pub mod audition;
pub mod chords;
pub mod device;
pub mod edit_ops;
pub mod export;
pub mod history;
pub mod import;
pub mod lengths;
pub mod midifile;
pub mod model;
pub mod project;
pub mod session;
pub mod track_clip;

pub use chords::{Harmony, Scale};
pub use device::{Device, DeviceId, DeviceIo, DeviceModel, PortEnd, PortRef, DN2, DT2, MODELS};
pub use export::{track_write, ExportError, TrackExport};
pub use history::{Content, History};
pub use import::{pattern_from_kit, Fetched, ImportError, ImportReport};
pub use lengths::{snap_len_fine, LEN_MIN};
pub use midifile::{midi_file_to_notes, track_to_midi_file, Imported, MidiFileError};
pub use model::{ModelError, Note, PLockLane, Pattern, Source, Track, TrackKind, TrackScale};
pub use project::{Project, ProjectError, FORMAT_VERSION};
pub use session::{BindError, PatternRef, Scene, Session};
pub use track_clip::{paste_track, ChordDrop, PasteReport, TrackClip};

/// A session with a DT2 and a DN2 in it, one bank of slots each, and one scene
/// pointing both at A01. The target case from PLAN.md §2, and what the app opens
/// with until there is a project to load.
pub fn default_session() -> Session {
    let mut session = Session::default();
    let dt2 = session.add_device(Device::new("DT2", &DT2, 16));
    let dn2 = session.add_device(Device::new("DN2", &DN2, 16));
    debug_assert!(session.device(dt2).is_some() && session.device(dn2).is_some());
    session
}
