// The three musical roles: bass, chords, lead. Each is a pure function from
// a resolved song context and one part's own settings to a plain note list
// — no ids, no pattern, no byte encoded anywhere. `arrange.rs` is what turns
// this into pattern state a slot can hold.

pub mod bass;
pub mod chord_lead;
pub mod chords;
pub mod drums;
pub mod lead;

use crate::genres::LenProfile;
use crate::rhythm::Trig;

/// One note, before it becomes a roll note: `arrange::build_part` is what
/// assigns an id. Fields mirror the JS's plain spec object exactly, so a
/// generated note is provably something the hardware can hold — see each
/// part's tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteSpec {
    pub step: u32,
    pub pitch: u8,
    pub len: f64,
    pub velocity: u8,
    pub micro: f64,
    pub prob: Option<i64>,
    pub fill: Option<bool>,
    pub cond: Option<&'static str>,
}

/// What one part generator returns: the notes, and the trig list the next
/// part in the arrangement order reads to answer this one.
#[derive(Debug, Clone, Default)]
pub struct GeneratedPart {
    pub notes: Vec<NoteSpec>,
    pub trigs: Vec<Trig>,
}

/// A role's length profile, reduced to the three numbers every generator
/// wants regardless of which JS shape it came from. `ghost` on a
/// [`LenProfile::Plain`] falls back to `normal` when the role never plays a
/// ghost note (every lead), and a [`LenProfile::Mode`] role has no ghost
/// note at all, so `normal` stands in for it there too.
pub(crate) fn len_bounds(len: &LenProfile) -> (f64, f64, f64) {
    match *len {
        LenProfile::Plain { normal, ghost, max } => (normal, ghost.unwrap_or(normal), max),
        LenProfile::Mode { normal, max, .. } => (normal, normal, max),
    }
}
