// The song context: everything above the individual pattern slots.
//
// Port of `js/gen/context.js`, with one deliberate structural change from the
// JS — see "Parts are per-track" below, which is PLAN.md Phase 7 Decision 1,
// settled with Neil on 2026-08-19.
//
// **Root and scale mirror the Harmony panel's own values**
// (`digi_core::chords::Harmony`). The generator always needs a concrete
// scale to work in, where the Harmony panel's own `scale` can be "off" — so
// the fields here are a fallback kept in step by whichever panel edits them:
// choosing a scale in the Generate panel sets both, and the tinted rows on
// the grid always agree with what was generated.
//
// The progression is stored as **text**, not as a parsed array. One source
// of truth for the field you type in, a malformed entry keeps the last good
// text, and [`resolve_context`] is the only place that parses — which is
// also where the error message a user sees comes from.
//
// ## Parts are per-track, and there are N of them
//
// The JS keys `parts` by role: `{ bass, chords, lead }`, exactly one of
// each, because it only ever knew one box. `safe_write_tracks` already
// writes any number of a slot's tracks in one re-fetch, one backup, one send
// and one verify, and PLAN.md's own rule 3 says reach for the plural — so a
// session with two boxes of sixteen tracks each should not inherit a ceiling
// that came from a constraint this repo does not have. A [`Part`] here is a
// row: a role, a **box + slot + track** [`Destination`], and its own
// density/octave/variation — any number of them, in any role.
//
// **The stream-tag trap.** `arrange.js`'s `streamTag(role, variation)` keys
// a part's RNG stream by its role, which assumed at most one part per role.
// Keying a stream by a part's *position in the row list* would be the
// obvious replacement and it is the wrong one: inserting or deleting a row
// would renumber every part after it and silently re-roll their music, same
// seed and same settings, different output — the panel's 🔒 lying. Every
// [`Part`] therefore carries a [`PartId`] assigned once at row creation and
// never reused, and that id — not the row's index — is what a stream tag
// must be derived from (Stage 3, `arrange.rs`).
//
// **A destination can name something that does not exist.** A plain slot
// index can be clamped into range the way `normalizeGenContext` clamps one;
// a box+slot+track destination cannot, because the box may not be in the
// session at all and the track index may exceed that model's own track
// count. It arrives from the panel and from a saved project file alike, so
// [`Destination::validate`] is the one place that checks it against a live
// session, and every write path must call it before touching a pattern.

use std::sync::atomic::{AtomicU64, Ordering};

use digi_core::chords::Scale;
use digi_core::device::DeviceId;
use digi_core::session::{PatternRef, Session};
use serde::{Deserialize, Serialize};

use crate::genres::{genre_profile, role_profile, GenreId, GenreProfile, Role, RoleProfile};
use crate::progressions::default_progression_for;
use crate::theory::{bar_slots, parse_progression, progression_bars, ChordParseError, ChordSlot, Key};

pub const GEN_BARS: [u32; 4] = [1, 2, 4, 8];

fn clamp_u8(v: u8, lo: u8, hi: u8) -> u8 {
    v.max(lo).min(hi)
}

/// The three feel sliders, 0–100 each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feel {
    pub motion: u8,
    pub looseness: u8,
    pub humanize: u8,
}

impl Default for Feel {
    fn default() -> Self {
        Self { motion: 35, looseness: 35, humanize: 20 }
    }
}

impl Feel {
    fn sanitized(self) -> Self {
        Self {
            motion: self.motion.min(100),
            looseness: self.looseness.min(100),
            humanize: self.humanize.min(100),
        }
    }
}

/// A part's stable identity, assigned once at row creation and never reused
/// — see the module header's "stream-tag trap".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PartId(pub u64);

static NEXT_PART_ID: AtomicU64 = AtomicU64::new(1);

impl PartId {
    pub fn next() -> Self {
        Self(NEXT_PART_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// After loading a project, push the counter past every id the file
    /// used, so a part added afterwards cannot collide with one that came
    /// off disk — the same rule `DeviceId::reserve_past` follows.
    pub fn reserve_past(highest: u64) {
        NEXT_PART_ID.fetch_max(highest + 1, Ordering::Relaxed);
    }
}

/// Where a part's notes go: a box, one of its pattern slots, and a track
/// within that slot. See the module header for why this cannot simply be
/// clamped into validity the way a slot index can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    /// `None` until a device is chosen — the state a freshly-added part
    /// starts in when there is no session to default it from.
    pub device: Option<DeviceId>,
    pub slot: PatternRef,
    pub track: usize,
}

/// Why a [`Destination`] cannot be resolved against a session, so the panel
/// can say which of the three things is missing rather than refusing
/// silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationError {
    /// No device has been chosen for this part yet.
    NoDevice,
    /// The chosen device is not (or no longer) in this session.
    DeviceMissing,
    /// The slot index is past the end of the device's own pattern list.
    SlotOutOfRange,
    /// The track index is past the end of the device model's track count.
    TrackOutOfRange { num_tracks: usize },
}

impl Destination {
    /// Check this destination against a live session — the one place a
    /// box+slot+track is confirmed to name something real, whether it came
    /// from the panel just now or from a project file loaded off disk.
    pub fn validate(&self, session: &Session) -> Result<(), DestinationError> {
        let device_id = self.device.ok_or(DestinationError::NoDevice)?;
        let device = session.devices.iter().find(|d| d.id == device_id).ok_or(DestinationError::DeviceMissing)?;
        if self.slot.slot() >= device.patterns.len() {
            return Err(DestinationError::SlotOutOfRange);
        }
        if self.track >= device.model.num_tracks {
            return Err(DestinationError::TrackOutOfRange { num_tracks: device.model.num_tracks });
        }
        Ok(())
    }
}

/// One row of the Generate panel: a role, where it writes, and how it plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub id: PartId,
    pub role: Role,
    pub on: bool,
    pub destination: Destination,
    /// 0–100.
    pub density: u8,
    /// 1–7, the box's own octave labelling.
    pub octave: u8,
    /// Which re-roll of this part we're on. "Generate this part" bumps it,
    /// so that part gets a new RNG stream while the song seed — and
    /// therefore every other part — stays put.
    pub variation: u32,
}

impl Part {
    fn sanitized(self) -> Self {
        Self { density: self.density.min(100), octave: clamp_u8(self.octave, 1, 7), ..self }
    }
}

fn new_part(role: Role, track: usize, density: u8, octave: u8) -> Part {
    Part {
        id: PartId::next(),
        role,
        on: true,
        destination: Destination { device: None, slot: PatternRef::new(0, 0), track },
        density,
        octave,
        variation: 0,
    }
}

/// The six default rows — bass, chords, lead, then a kick/snare/closed-hat
/// kit, in that order — lining up with tracks 1–6 on a box if they're sent
/// in order. Octaves are the register windows the three melodic rows land
/// in: bass C3–C5, chords C5–C7, lead C6–up.
///
/// **Each is one octave above the design's own figures** (bass C2–C4,
/// chords C4–C6, lead C5–up), which put a generated part low enough that
/// every run wanted transposing up by hand before it was usable. The design
/// numbers are a register in the abstract; these are where the parts sit
/// once a box is actually playing them. The window heights (`span` in
/// `genres.rs`) are untouched — this moves the floor, not the range.
///
/// **Drum voices have no register.** `arrange::generate_for_role` never
/// passes a drum row's octave to [`generate_drums`](crate::parts::drums::generate_drums)
/// at all — a voice's destination track *is* the sound, so there is nothing
/// for an octave to move (see `genres::drum_profile`'s header). `4` is
/// therefore not a register choice, only a value that survives
/// [`Part::sanitized`]'s 1–7 clamp and matches the placeholder the panel's
/// own "+ add part…" button already leaves behind when a row is switched to
/// a drum role (`ui::generate::parts_group`) — the panel hides the Oct
/// control for a drum voice (`Role::is_drum_voice`) rather than pretending
/// the number means something.
///
/// Densities: `60`/`55`/`65` for kick/snare/closed hat are read off
/// `rhythm::trig_count_for` against each drum role's own `trigs_per_bar`
/// range in `genres.rs`, not copied from the melodic rows. The spine tier
/// (kick, snare — `DRUM_SPINE_RECIPE`) only ever carries `ProbWeak`, which
/// never touches an accented trig, so pushing their density toward the
/// busier half of their range is safe: at 60, DnB's kick (`(1,3)` per bar)
/// lands ~2/bar — the table's own "1, and the syncopated and-of-3" — and
/// Breaks' kick (`(2,5)`) lands ~4/bar, matching its "busier than DnB's"
/// comment. Snare at 55 keeps the backbeat itself dominant (weight 1.0 on
/// beats 2/4 always wins the weighted pick) while still drawing in the
/// ghost hits the DnB and Breaks tables reserve for it. Closed hat's
/// `trigs_per_bar` floor is already high everywhere (6–16), because a hat
/// carries the pulse rather than punctuating it, so 65 pushes toward a
/// steady near-continuous 16th feel (DnB: ~14/16 steps) and leaves
/// `DRUM_COLOUR_RECIPE`'s conditions to do the thinning, rather than the
/// raw trig count.
///
/// **Not the JS's ceiling of three**: this is a starting point, not a limit.
/// Rows can be added, removed and reassigned to any box, slot or track.
pub fn default_parts() -> Vec<Part> {
    vec![
        new_part(Role::Bass, 0, 55, 3),
        new_part(Role::Chords, 1, 40, 5),
        new_part(Role::Lead, 2, 40, 6),
        new_part(Role::Kick, 3, 60, 4),
        new_part(Role::Snare, 4, 55, 4),
        new_part(Role::ClosedHat, 5, 65, 4),
    ]
}

/// The song context: everything a generate run needs above the individual
/// pattern slots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenContext {
    pub genre: GenreId,
    /// Mirrors `Harmony::root`.
    pub root: u8,
    /// Mirrors `Harmony::scale`, always concrete — see the module header.
    pub scale: Scale,
    pub bars: u32,
    pub progression: String,
    pub seed: u32,
    pub seed_locked: bool,
    pub feel: Feel,
    pub parts: Vec<Part>,
}

impl Default for GenContext {
    fn default() -> Self {
        Self {
            genre: GenreId::Dnb,
            root: 0,
            scale: crate::theory::DEFAULT_SCALE,
            bars: 2,
            progression: default_progression_for(GenreId::Dnb).to_string(),
            seed: 1_834_721,
            seed_locked: false,
            feel: Feel::default(),
            parts: default_parts(),
        }
    }
}

impl GenContext {
    /// Anything from a project file or a hand edit → a context the
    /// generator can safely use. Never fails: a broken numeric field is
    /// clamped into range rather than refused, because the panel has to
    /// open. Unlike `normalizeGenContext`, most of the JS's "is this even
    /// the right type" work has no equivalent — a malformed enum or string
    /// simply fails to deserialize in the first place — so this only clamps
    /// the ranges that serde's own type-checking cannot: `root`, `bars`,
    /// `feel`, and each part's `density`/`octave`.
    pub fn sanitized(mut self) -> Self {
        self.root = self.root.min(11);
        if !GEN_BARS.contains(&self.bars) {
            self.bars = GenContext::default().bars;
        }
        if self.progression.trim().is_empty() {
            self.progression = default_progression_for(self.genre).to_string();
        }
        self.feel = self.feel.sanitized();
        self.parts = self.parts.into_iter().map(Part::sanitized).collect();
        self
    }

    /// Switching genre re-defaults the things that are *about* the genre —
    /// its bar count and its progression — and leaves the things that are
    /// about you: the seed, the feel sliders, the parts. Changing genre with
    /// a hand-typed progression keeps it: you typed it, it isn't ours to
    /// throw away.
    pub fn for_genre(&self, genre: GenreId, keep_progression: bool) -> Self {
        let profile = genre_profile(genre);
        let mut out = self.clone();
        out.genre = genre;
        if GEN_BARS.contains(&profile.bars) {
            out.bars = profile.bars;
        }
        if !keep_progression {
            out.progression = default_progression_for(genre).to_string();
        }
        out
    }

    /// Which destinations a generate would overwrite: the "on" parts', in
    /// row order.
    pub fn target_destinations(&self) -> Vec<Destination> {
        self.parts.iter().filter(|p| p.on).map(|p| p.destination).collect()
    }

    /// Every part aimed at this exact destination — "Generate this slot"
    /// needs to know which row(s) a track being edited belongs to. Plural,
    /// unlike the JS's `roleForSlot`: nothing stops two parts sharing a
    /// destination, since a session's write path is a re-fetch and a
    /// minimal diff either way.
    pub fn parts_at<'a>(&'a self, destination: &Destination) -> Vec<&'a Part> {
        self.parts.iter().filter(|p| &p.destination == destination).collect()
    }

    /// A fresh arrangement is the canonical one for its seed, so every
    /// part's re-roll counter goes back to zero.
    pub fn reset_variations(&mut self) {
        for p in &mut self.parts {
            p.variation = 0;
        }
    }

    /// One part re-rolled: only that part's stream moves.
    pub fn bump_variation(&mut self, id: PartId) {
        if let Some(p) = self.parts.iter_mut().find(|p| p.id == id) {
            p.variation += 1;
        }
    }

    /// What a generated slot gets called, so the slot dropdown says what's
    /// in it.
    pub fn part_label(&self, part: &Part) -> String {
        format!("{} {}", genre_profile(self.genre).label, part.role.label().to_lowercase())
    }
}

/// The context every generator function actually takes: normalized, with
/// the progression parsed and the derived values worked out once.
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    pub profile: GenreProfile,
    pub key_root: i32,
    pub key_intervals: &'static [i32],
    pub prog: Vec<ChordSlot>,
    pub bar_slots: Vec<ChordSlot>,
    pub bars: u32,
    pub length_steps: u32,
    pub feel: Feel,
    pub seed: u32,
}

impl ResolvedContext {
    pub fn key(&self) -> Key<'static> {
        Key { root: self.key_root, intervals: self.key_intervals }
    }

    pub fn role_profile(&self, role: Role) -> RoleProfile {
        role_profile(self.profile.id, role)
    }
}

/// Resolve a context for the generator. Fails with the parser's own message
/// for a malformed progression — the one error the panel expects and
/// reports on the status line.
pub fn resolve_context(ctx: &GenContext) -> Result<ResolvedContext, ChordParseError> {
    let ctx = ctx.clone().sanitized();
    let profile = genre_profile(ctx.genre);
    let prog = parse_progression(&ctx.progression)?;
    let intervals = ctx.scale.intervals();
    let bar_slots_v = bar_slots(&prog, ctx.bars);
    Ok(ResolvedContext {
        profile,
        key_root: i32::from(ctx.root),
        key_intervals: intervals,
        prog,
        bar_slots: bar_slots_v,
        bars: ctx.bars,
        length_steps: ctx.bars * 16,
        feel: ctx.feel,
        seed: ctx.seed,
    })
}

/// Is a progression text usable? The panel asks before committing an edit,
/// so a half-typed chord doesn't wipe the last good one. The bar count comes
/// back with it, because the hint under the field wants to say how long the
/// loop is and this is the only place that parses.
pub fn check_progression(text: &str) -> Result<u32, ChordParseError> {
    let prog = parse_progression(text)?;
    Ok(progression_bars(&prog))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BpmSuggestion {
    pub bpm: u32,
    pub in_range: bool,
    pub range: (u32, u32),
}

/// The bpm the genre suggests, and whether the transport is already there.
/// The panel offers it; nothing changes the tempo behind your back.
pub fn bpm_suggestion(genre: GenreId, bpm: u32) -> BpmSuggestion {
    let profile = genre_profile(genre);
    BpmSuggestion { bpm: profile.bpm, in_range: bpm >= profile.bpm_range.0 && bpm <= profile.bpm_range.1, range: profile.bpm_range }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_context_is_a_complete_usable_song() {
        let d = GenContext::default();
        assert!(GEN_BARS.contains(&d.bars));
        assert!(resolve_context(&d).is_ok());
        for p in &d.parts {
            assert!(p.on);
        }
    }

    #[test]
    fn the_default_parts_are_six_distinct_rows_on_six_tracks_in_order() {
        // Bass, chords, lead, then kick/snare/closed-hat — tracks 0..=5 in
        // that order, so they line up with a box's tracks 1-6 if sent as is.
        let d = GenContext::default();
        assert_eq!(d.parts.len(), 6);
        let ids: std::collections::HashSet<PartId> = d.parts.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), 6, "six distinct part identities");
        let tracks: Vec<usize> = d.parts.iter().map(|p| p.destination.track).collect();
        assert_eq!(tracks, vec![0, 1, 2, 3, 4, 5], "six distinct tracks, in row order");
        let roles: Vec<Role> = d.parts.iter().map(|p| p.role).collect();
        assert_eq!(
            roles,
            vec![Role::Bass, Role::Chords, Role::Lead, Role::Kick, Role::Snare, Role::ClosedHat]
        );
        // Register windows only mean something for the three melodic rows —
        // see `default_parts`'s doc comment on why a drum row's octave is an
        // inert placeholder rather than a fourth register.
        assert!(d.parts[0].octave < d.parts[1].octave);
        assert!(d.parts[1].octave < d.parts[2].octave);
    }

    #[test]
    fn the_melodic_defaults_sit_an_octave_above_the_designs_own_registers() {
        // Pinned rather than left to the ordering assertion above, because
        // the whole point of these three numbers is *which* octave, not the
        // gaps between them: the design's 2/4/5 generated a part low enough
        // that every run wanted transposing up by hand.
        let d = GenContext::default();
        let octaves: Vec<u8> = d.parts[..3].iter().map(|p| p.octave).collect();
        assert_eq!(octaves, vec![3, 5, 6]);
        // And each is still a register the roll can draw a full window of —
        // `window_for` pulls an octave too high back down, which would make
        // a raise silently do nothing.
        for (i, &oct) in octaves.iter().enumerate() {
            let (lo, hi) = crate::theory::window_for(24, i32::from(oct));
            assert_eq!(lo, 12 * i32::from(oct), "octave {oct} was pulled back down");
            assert!(hi - lo >= 12);
        }
    }

    #[test]
    fn the_default_part_set_generates_for_every_genre() {
        // The critical check: `GenContext::default()` is what the panel opens
        // with, and switching genre keeps the same six rows (`for_genre` only
        // re-defaults bars/progression). If any genre's `role_profile` match
        // were missing Kick, Snare or ClosedHat the crate would not compile
        // at all — the match in `genres.rs` is exhaustive over every
        // `(GenreId, Role)` pair — but that only proves a profile *exists*,
        // not that running it through the real pipeline produces music. This
        // drives `arrange::generate_arrangement` itself, so a panic or an
        // empty part hiding behind a profile that type-checks but is broken
        // for one genre would still be caught here.
        for genre in GenreId::ALL {
            let ctx = GenContext::default().for_genre(genre, false);
            assert_eq!(ctx.parts.len(), 6, "{genre:?}: default part count changed");
            let arrangement = crate::arrange::generate_arrangement(&ctx, None)
                .unwrap_or_else(|e| panic!("{genre:?}: the default six-part context failed to generate: {e:?}"));
            assert_eq!(arrangement.parts.len(), 6);
            for (part, ctx_part) in arrangement.parts.iter().zip(&ctx.parts) {
                assert_eq!(part.role, ctx_part.role);
                assert!(!part.notes.is_empty(), "{genre:?}/{:?} produced no notes", part.role);
            }
        }
    }

    #[test]
    fn part_ids_survive_insertion_and_deletion_of_other_rows() {
        // The stream-tag trap the module header warns about: adding or
        // removing a row must not change any other part's identity.
        let mut ctx = GenContext::default();
        let bass_id = ctx.parts[0].id;
        let lead_id = ctx.parts[2].id;
        ctx.parts.remove(1); // drop the chords row
        assert_eq!(ctx.parts[0].id, bass_id);
        assert_eq!(ctx.parts[1].id, lead_id);
        let mut new_part = new_part(Role::Chords, 5, 40, 4);
        let new_id = new_part.id;
        ctx.parts.insert(0, new_part.clone());
        assert_eq!(ctx.parts[0].id, new_id);
        assert_eq!(ctx.parts[1].id, bass_id);
        assert_eq!(ctx.parts[2].id, lead_id);
        new_part.variation = 1; // silence an unused-mut style complaint on some toolchains
        let _ = new_part;
    }

    #[test]
    fn sanitizing_clamps_out_of_range_numbers() {
        let mut ctx = GenContext { root: 99, bars: 3, ..GenContext::default() };
        ctx.feel = Feel { motion: 255, looseness: 0, humanize: 20 };
        ctx.parts[0].density = 255;
        ctx.parts[0].octave = 0;
        let out = ctx.sanitized();
        assert_eq!(out.root, 11);
        assert_eq!(out.bars, GenContext::default().bars); // 3 isn't an offered length
        assert_eq!(out.feel.motion, 100);
        assert_eq!(out.parts[0].density, 100);
        assert_eq!(out.parts[0].octave, 1);
    }

    #[test]
    fn sanitizing_falls_back_to_the_genres_own_progression_when_there_isnt_one() {
        let ctx = GenContext { genre: GenreId::House, progression: "   ".into(), ..GenContext::default() };
        assert_eq!(ctx.sanitized().progression, default_progression_for(GenreId::House));
    }

    #[test]
    fn switching_genre_takes_the_new_genres_bar_count_and_progression() {
        let out = GenContext::default().for_genre(GenreId::House, false);
        assert_eq!(out.genre, GenreId::House);
        assert_eq!(out.bars, genre_profile(GenreId::House).bars);
        assert_eq!(out.progression, default_progression_for(GenreId::House));
    }

    #[test]
    fn switching_genre_keeps_what_is_yours() {
        let mut before = GenContext::default();
        before.seed = 777;
        before.seed_locked = true;
        before.feel = Feel { motion: 90, looseness: 5, humanize: 50 };
        before.parts[2].on = false;
        before.parts[2].density = 20;
        let after = before.for_genre(GenreId::Electro, false);
        assert_eq!(after.seed, 777);
        assert!(after.seed_locked);
        assert_eq!(after.feel, before.feel);
        assert_eq!(after.parts, before.parts);
    }

    #[test]
    fn switching_genre_keeps_a_hand_typed_progression_when_asked_to() {
        let mine = GenContext { progression: "ii7 V7 i7".into(), ..GenContext::default() };
        assert_eq!(mine.for_genre(GenreId::House, true).progression, "ii7 V7 i7");
    }

    #[test]
    fn resolving_works_out_the_derived_values_once() {
        let ctx = GenContext {
            bars: 4,
            progression: "i VI".into(),
            scale: Scale::Dorian,
            root: 5,
            ..GenContext::default()
        };
        let resolved = resolve_context(&ctx).unwrap();
        assert_eq!(resolved.length_steps, 64);
        assert_eq!(resolved.prog.iter().map(|s| s.degree).collect::<Vec<_>>(), vec![1, 6]);
        assert_eq!(resolved.bar_slots.iter().map(|s| s.degree).collect::<Vec<_>>(), vec![1, 6, 1, 6]);
        assert_eq!((resolved.key_root, resolved.key_intervals), (5, Scale::Dorian.intervals()));
        assert_eq!(resolved.role_profile(Role::Bass).weights.len(), 16);
        assert_eq!(resolved.profile.groove.len(), 16);
    }

    #[test]
    fn resolving_gives_the_parsers_own_message_for_a_malformed_progression() {
        let ctx = GenContext { progression: "i VIII".into(), ..GenContext::default() };
        let err = resolve_context(&ctx).unwrap_err();
        assert!(err.0.contains("isn't a chord quality") || err.0.contains("roman numerals"));
    }

    #[test]
    fn resolves_every_genre() {
        for genre in GenreId::ALL {
            let ctx = GenContext::default().for_genre(genre, false);
            let resolved = resolve_context(&ctx).unwrap();
            assert_eq!(resolved.profile.id, genre);
            for role in Role::ALL {
                assert_eq!(resolved.role_profile(role).weights.len(), 16);
            }
        }
    }

    #[test]
    fn checking_a_progression_says_yes_to_a_good_one_and_why_not_to_a_bad_one() {
        assert_eq!(check_progression("i VI III VII"), Ok(4));
        assert_eq!(check_progression("i:2 VI:2"), Ok(4));
        let bad = check_progression("i H").unwrap_err();
        assert!(bad.0.contains("roman numerals"));
    }

    #[test]
    fn the_bpm_suggestion_is_the_genres_own_tempo() {
        let s = bpm_suggestion(GenreId::Dnb, 174);
        assert_eq!((s.bpm, s.in_range), (174, true));
        assert!(!bpm_suggestion(GenreId::Dnb, 120).in_range);
        let s = bpm_suggestion(GenreId::House, 124);
        assert_eq!((s.bpm, s.in_range), (124, true));
    }

    #[test]
    fn every_genre_has_a_tempo_inside_its_own_range() {
        for genre in GenreId::ALL {
            let s = bpm_suggestion(genre, 0);
            assert!(s.bpm >= s.range.0 && s.bpm <= s.range.1);
        }
    }

    #[test]
    fn target_destinations_is_the_checked_parts_destinations_in_row_order() {
        let mut ctx = GenContext::default();
        ctx.parts[1].on = false; // the chords row
        let dests = ctx.target_destinations();
        assert_eq!(dests.len(), 5, "one of six rows switched off");
        // Row order, skipping only the one turned off — named explicitly by
        // index rather than re-filtering `ctx.parts` here, so this cannot
        // become a tautology that passes however `target_destinations` is
        // implemented.
        assert_eq!(dests[0].track, ctx.parts[0].destination.track); // bass
        assert_eq!(dests[1].track, ctx.parts[2].destination.track); // lead
        assert_eq!(dests[2].track, ctx.parts[3].destination.track); // kick
        assert_eq!(dests[3].track, ctx.parts[4].destination.track); // snare
        assert_eq!(dests[4].track, ctx.parts[5].destination.track); // closed hat
    }

    #[test]
    fn bumping_one_parts_variation_leaves_the_others_at_zero() {
        let mut ctx = GenContext::default();
        let bass_id = ctx.parts[0].id;
        ctx.bump_variation(bass_id);
        assert_eq!(ctx.parts[0].variation, 1);
        for p in &ctx.parts[1..] {
            assert_eq!(p.variation, 0, "{:?} moved when only bass was bumped", p.role);
        }
        ctx.reset_variations();
        assert!(ctx.parts.iter().all(|p| p.variation == 0));
    }

    #[test]
    fn a_destination_with_no_device_refuses_to_validate() {
        let dest = Destination { device: None, slot: PatternRef::new(0, 0), track: 0 };
        let session = digi_core::default_session();
        assert_eq!(dest.validate(&session), Err(DestinationError::NoDevice));
    }

    #[test]
    fn a_destination_naming_a_device_not_in_the_session_refuses_to_validate() {
        let dest = Destination { device: Some(DeviceId::next()), slot: PatternRef::new(0, 0), track: 0 };
        let session = digi_core::default_session();
        assert_eq!(dest.validate(&session), Err(DestinationError::DeviceMissing));
    }

    #[test]
    fn a_destination_naming_a_real_device_and_track_validates() {
        let session = digi_core::default_session();
        let device = &session.devices[0];
        let dest = Destination { device: Some(device.id), slot: PatternRef::new(0, 0), track: 0 };
        assert_eq!(dest.validate(&session), Ok(()));
    }

    #[test]
    fn a_track_past_the_devices_own_count_refuses_to_validate() {
        let session = digi_core::default_session();
        let device = &session.devices[0];
        let dest = Destination { device: Some(device.id), slot: PatternRef::new(0, 0), track: device.model.num_tracks };
        assert_eq!(
            dest.validate(&session),
            Err(DestinationError::TrackOutOfRange { num_tracks: device.model.num_tracks })
        );
    }
}
