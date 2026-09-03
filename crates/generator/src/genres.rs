// The genre profiles: plain data, no logic.
//
// Port of `js/gen/genres.js`. A profile is the rhythmic and dynamic grammar
// of a genre, per role. Everything the generator decides that *isn't*
// harmony comes from here, which is what makes "DnB" and "house" produce
// different music from the same progression in the same key.
//
// Three things worth knowing before editing one:
//
//   * **`weights` is one bar of sixteenths**, index 0 = the downbeat, 4/8/12
//     the other beats, 2/6/10/14 the eighth-note "and"s. It is a *relative*
//     likelihood that a step gets a trig, not a rule — 0 means never, and
//     the density slider decides how many of the likely ones actually fire.
//   * **`groove` is per-note micro-timing**, in fractions of a step, and it
//     is how genre feel is expressed. The generator may not touch `swing`:
//     that byte re-times all sixteen tracks in the destination pattern, so a
//     generator setting it would change parts it wasn't asked to touch.
//     Micro-timing is per note, stored on the box, and harmless to the other
//     fifteen tracks — hence house's shuffle living in this array.
//   * **`bpm` is a suggestion**, offered by the panel with a Set button.
//     Nothing here changes the transport behind your back.
//   * **`len` is in steps, and every bass profile's is doubled from the
//     first cut's.** The numbers carried over from `js/gen/genres.js` made
//     a bassline that read as short in every genre — notes stopping well
//     inside the gap to the next trig, so the line never joined up — and
//     wanted lengthening by hand after every run. `normal` and `ghost` are
//     now exactly twice what they were, and `anchor_len` is raised only
//     where the anchor would otherwise have ended up shorter than the
//     ordinary notes around it (electro, house, techno, breaks); DnB's
//     four-step anchor was always long and is untouched. `max` is
//     untouched everywhere: the ceiling was never the problem, and
//     `bass.rs` still takes the smaller of the length, the gap to the next
//     trig and that ceiling, so a longer `normal` lengthens a note only
//     where there is room for one. Electro and techno stay staccato
//     because their `max` of one step caps them there regardless.
//
// Register windows are `[12 * octave, 12 * octave + span]` with the octave
// coming from the song context, so moving a part's octave moves its window
// rather than transposing notes out of the range the roll can draw. `span`
// is the height; the window itself is worked out by `theory::window_for`.
//
// This module imports nothing from `theory` — the register-window maths
// lives there. Data only, same as the JS.

/// The genres this ships with. Four are ports — `js/gen/genres.js` has
/// `dnb`, `breaks`, `electro` and `house` and nothing else — and Techno and
/// Rollers are this codebase's own, so their tables are convention tuned by
/// ear rather than values pinned by an oracle. The same caveat as the drum
/// voices, and for the same reason.
///
/// Unlike the JS's bare strings this is closed, so a caller cannot ask for a
/// genre that was never in `GENRES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GenreId {
    Dnb,
    Breaks,
    Electro,
    House,
    Techno,
    /// Funk-break drums under a rolling bassline, with the simplest chords
    /// and the shortest hooks of any profile here. Added 2026-09-03 on
    /// Neil's ask, from four named references (Pendulum, Sub Focus, Nero,
    /// The Prodigy) rather than from `js/gen/` — there is no oracle for it,
    /// the same as the drum voices. Named for the DnB scene's own word for
    /// a track built on a rolling bassline.
    Rollers,
}

impl GenreId {
    pub const ALL: [Self; 6] =
        [Self::Dnb, Self::Breaks, Self::Electro, Self::House, Self::Techno, Self::Rollers];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dnb => "dnb",
            Self::Breaks => "breaks",
            Self::Electro => "electro",
            Self::House => "house",
            Self::Techno => "techno",
            Self::Rollers => "rollers",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.as_str() == s)
    }
}

/// The three roles the JS ships parts for, plus eight drum voices that are
/// not a port — see the header on [`role_profile`]'s drum arms, and PLAN.md
/// Phase 7 stage 5. Settled with Neil 2026-08-19: drums work on either box,
/// one part per voice, the same as bass/chords/lead — the destination track
/// *is* the drum, so a voice needs no pitch or register logic at all, only
/// its own rhythm. Four voices shipped first (kick, snare, closed and open
/// hat); clap, rimshot, ride and tom were added on Neil's ask the same week,
/// and cost nothing but their weight tables precisely *because* a voice is
/// only a rhythm. Shaker joined them 2026-08-22, for the same reason and at
/// the same price: it is the kit's dedicated sixteenth-note voice, the one
/// thing the eight before it could not be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Bass,
    Chords,
    /// A chord part shaped like the Analog Four's factory A01: straight
    /// eighths, two pedal tones held per bar while the root leaps octaves
    /// under them. Added 2026-09-02 for the A4, whose ARP NO2/NO3/NO4 carry
    /// the upper notes — see `parts::chord_lead` — and not gated to it: the
    /// notes are ordinary same-step chords any polyphonic track can play.
    ChordLead,
    Lead,
    /// One half of a call-and-response pair — see `parts::lead`'s "Taking
    /// turns". A pair is two ordinary rows, paired by row order the way
    /// everything else in the panel is: the nearest `LeadCall` above a
    /// `LeadResponse` is the one it answers.
    LeadCall,
    LeadResponse,
    Kick,
    Snare,
    Clap,
    Rimshot,
    ClosedHat,
    OpenHat,
    Ride,
    Shaker,
    Tom,
}

impl Role {
    /// Melodic first, then the drum voices in kit order — which is the order
    /// the panel's role picker draws, so a kick sits next to a snare rather
    /// than next to whatever was added last.
    pub const ALL: [Self; 15] = [
        Self::Bass,
        Self::Chords,
        Self::ChordLead,
        Self::Lead,
        Self::LeadCall,
        Self::LeadResponse,
        Self::Kick,
        Self::Snare,
        Self::Clap,
        Self::Rimshot,
        Self::ClosedHat,
        Self::OpenHat,
        Self::Ride,
        Self::Shaker,
        Self::Tom,
    ];

    /// The roles `theory`/`chord`-aware generation applies to —
    /// [`generate_for_role`](crate::arrange) reads this split to decide
    /// whether a part needs an octave and a key at all. A call and a
    /// response are leads, so they are here: both pick a register and both
    /// resolve degrees against the bar's chord.
    pub const MELODIC: [Self; 6] =
        [Self::Bass, Self::Chords, Self::ChordLead, Self::Lead, Self::LeadCall, Self::LeadResponse];
    pub const DRUM_VOICES: [Self; 9] = [
        Self::Kick,
        Self::Snare,
        Self::Clap,
        Self::Rimshot,
        Self::ClosedHat,
        Self::OpenHat,
        Self::Ride,
        Self::Shaker,
        Self::Tom,
    ];

    pub fn is_drum_voice(self) -> bool {
        Self::DRUM_VOICES.contains(&self)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bass => "bass",
            Self::Chords => "chords",
            Self::ChordLead => "chord_lead",
            Self::Lead => "lead",
            Self::LeadCall => "lead_call",
            Self::LeadResponse => "lead_response",
            Self::Kick => "kick",
            Self::Snare => "snare",
            Self::Clap => "clap",
            Self::Rimshot => "rimshot",
            Self::ClosedHat => "closed_hat",
            Self::OpenHat => "open_hat",
            Self::Ride => "ride",
            Self::Shaker => "shaker",
            Self::Tom => "tom",
        }
    }

    /// What the panel labels it — `js/gen/context.js`'s `ROLE_LABELS`, plus
    /// the nine drum voices it never had.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bass => "Bass",
            Self::Chords => "Chords",
            Self::ChordLead => "Chord lead",
            Self::Lead => "Lead",
            Self::LeadCall => "Lead (call)",
            Self::LeadResponse => "Lead (response)",
            Self::Kick => "Kick",
            Self::Snare => "Snare",
            Self::Clap => "Clap",
            Self::Rimshot => "Rimshot",
            Self::ClosedHat => "Closed hat",
            Self::OpenHat => "Open hat",
            Self::Ride => "Ride",
            Self::Shaker => "Shaker",
            Self::Tom => "Tom",
        }
    }
}

/// A step is offered one of these condition kinds when a FILL is held. `On`
/// only exists while FILL is held, `Off` steps aside during one, and
/// `Either` coin-flips between sounding and stepping aside — the JS supports
/// all three even though no shipped profile currently reaches for `Either`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillMode {
    On,
    Off,
    Either,
}

/// A condition the rhythm engine may write onto a trig, used musically.
/// `chance` is at Looseness 100 and scales down with the slider; at 0 no
/// condition is written at all. `keys` is which COND values a rule may
/// choose between — always present here (rather than `Option` with a
/// fallback resolved at use time) so `rhythm::trig_feel_for` never has to
/// know each kind's default.
#[derive(Debug, Clone, Copy)]
pub enum ConditionRecipe {
    AltBar { chance: f64, keys: &'static [&'static str] },
    EveryFourth { chance: f64, keys: &'static [&'static str] },
    ProbGhost { chance: f64, range: (i64, i64) },
    ProbWeak { chance: f64, range: (i64, i64) },
    Fill { chance: f64, mode: FillMode },
    /// `PRE`/`NEI` — answering logic between trigs.
    Logic { chance: f64, keys: &'static [&'static str] },
}

const ALT_BARS: ConditionRecipe = ConditionRecipe::AltBar { chance: 0.4, keys: &["1:2", "2:2"] };
const EVERY_FOURTH: ConditionRecipe = ConditionRecipe::EveryFourth { chance: 0.18, keys: &["3:4", "4:4"] };
const GHOST_PROB: ConditionRecipe = ConditionRecipe::ProbGhost { chance: 0.85, range: (60, 85) };
const WEAK_PROB: ConditionRecipe = ConditionRecipe::ProbWeak { chance: 0.3, range: (70, 90) };
const FILL_EXTRA: ConditionRecipe = ConditionRecipe::Fill { chance: 0.15, mode: FillMode::On };
const FILL_STEP_ASIDE: ConditionRecipe = ConditionRecipe::Fill { chance: 0.1, mode: FillMode::Off };
const ANSWERING: ConditionRecipe = ConditionRecipe::Logic { chance: 0.1, keys: &["PRE", "NEI"] };

// --- Drum PROB/COND: a sprinkle, not the melodic dose --------------------
//
// Settled with Neil 2026-08-20: drums get PROB and COND too, but lighter
// than bass/chords/lead — "a sprinkle... wouldn't go amiss", not the full
// dose. New consts rather than reusing `ALT_BARS`/`WEAK_PROB`/friends on
// purpose: those are tuned for the melodic roles, and a shared const is
// lesson 5 waiting to happen (DEVELOPMENT.md's "a rule that lives in three places
// will be forgotten in one of them") — one tweak meant for a hi-hat would
// silently retune every bassline. All starting points, tuned by ear, not
// derived from any oracle the way the melodic values are.
//
// **The one safety fact this all turns on**: `rhythm.rs:139` sets
// `accent: is_beat(step) || weight >= 0.8`, so every trig on a beat is an
// accent. `ProbWeak`, `EveryFourth`, `Logic` and `Fill` all skip accented
// trigs by construction (see `rhythm::trig_feel_for`'s match arms), and
// `ProbGhost` only ever fires on a ghost, which is off-beat by definition.
// `AltBar` is the *only* recipe kind that does not check `trig.accent` — it
// will happily put `1:2`/`2:2` on a downbeat and silence it on alternate
// loops. So the spine of the kit (kick, snare, clap) gets `ProbWeak` only,
// and never `AltBar`. Everything else is safe by construction.
const DRUM_WEAK_PROB: ConditionRecipe = ConditionRecipe::ProbWeak { chance: 0.18, range: (80, 95) };
const DRUM_GHOST_PROB: ConditionRecipe = ConditionRecipe::ProbGhost { chance: 0.5, range: (65, 88) };
const DRUM_ALT_BARS: ConditionRecipe = ConditionRecipe::AltBar { chance: 0.15, keys: &["1:2", "2:2"] };
const DRUM_EVERY_FOURTH: ConditionRecipe = ConditionRecipe::EveryFourth { chance: 0.12, keys: &["3:4", "4:4"] };
const DRUM_FILL_EXTRA: ConditionRecipe = ConditionRecipe::Fill { chance: 0.2, mode: FillMode::On };
// The texture tier's own four, every one a shade under the colour tier's.
// A sixteenth-note voice fires four times as many trigs per bar as an
// off-beat open hat, so an identical *per-trig* chance buys four times the
// locks — which is how opening the hats to sixteenths first pushed the
// whole kit's lock rate above the melodic roles'. These keep the sprinkle a
// sprinkle by the count, not just by the rate.
const DRUM_TEXTURE_GHOST_PROB: ConditionRecipe = ConditionRecipe::ProbGhost { chance: 0.18, range: (70, 92) };
const DRUM_TEXTURE_WEAK_PROB: ConditionRecipe = ConditionRecipe::ProbWeak { chance: 0.1, range: (82, 96) };
const DRUM_TEXTURE_ALT_BARS: ConditionRecipe = ConditionRecipe::AltBar { chance: 0.08, keys: &["1:2", "2:2"] };
const DRUM_TEXTURE_EVERY_FOURTH: ConditionRecipe = ConditionRecipe::EveryFourth { chance: 0.07, keys: &["3:4", "4:4"] };

/// Spine tier — Kick, Snare, Clap. Must be there on the first pass, so the
/// only recipe is `ProbWeak`, which never touches an accented trig. Never
/// `AltBar`: see the block comment above this section for why the spine can
/// never carry it. A house-style four-on-the-floor kick (every trig on a
/// beat, hence every trig an accent) needs no per-genre override to stay
/// untouched — `ProbWeak` simply never fires for it. Do not "fix" that by
/// adding a recipe there; the silence is the design working.
const DRUM_SPINE_RECIPE: &[ConditionRecipe] = &[DRUM_WEAK_PROB];

/// Colour tier — Rimshot and OpenHat. These carry ornament, not the pulse,
/// so they can afford to lose a hit: a hat that thins on alternate bars is
/// the point, not a defect. Both are sparse by construction (an open hat
/// only reaches the four "and"s), which is what lets the chances here stay
/// as high as they are.
const DRUM_COLOUR_RECIPE: &[ConditionRecipe] = &[DRUM_GHOST_PROB, DRUM_WEAK_PROB, DRUM_ALT_BARS, DRUM_EVERY_FOURTH];

/// Texture tier — ClosedHat, Ride, Shaker. The colour tier's intent at a
/// quarter of its chances, because these are the three voices whose density
/// slider now runs all the way to sixteenths and a texture is not a texture
/// if it flickers. Split out 2026-08-22 rather than retuning
/// `DRUM_COLOUR_RECIPE`: a rimshot and a shaker want genuinely different
/// doses, and the sparse voices' numbers were already right.
const DRUM_TEXTURE_RECIPE: &[ConditionRecipe] =
    &[DRUM_TEXTURE_GHOST_PROB, DRUM_TEXTURE_WEAK_PROB, DRUM_TEXTURE_ALT_BARS, DRUM_TEXTURE_EVERY_FOURTH];

/// Fill tier — Tom. The weight tables already call it "fill material,
/// weighted to the back half of the bar", so this is the one voice built to
/// be conditional throughout.
const DRUM_FILL_RECIPE: &[ConditionRecipe] = &[DRUM_FILL_EXTRA, DRUM_EVERY_FOURTH, DRUM_ALT_BARS, DRUM_GHOST_PROB];

/// A p-lock lane a role wants, and the shape its motion follows across a bar.
#[derive(Debug, Clone, Copy)]
pub struct LaneRecipe {
    /// Canonical parameter name, e.g. `"filter.cutoff"` — translated to a
    /// per-box CC/NRPN the way `copy_track`'s p-lock translation already
    /// does, not restated here.
    pub name: &'static str,
    pub shape: LaneShape,
    pub from: u8,
    pub to: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneShape {
    Rise,
    /// The reverse of `Rise` — closes as the loop goes on. Not used by any
    /// shipped genre's recipe, but part of the general vocabulary
    /// `plockdesign` exports (`js/gen/plockdesign.js`'s `LANE_SHAPES.fall`).
    Fall,
    Accent,
    Swell,
    Arc,
    Wander,
    Pulse,
}

/// How long a role's notes run, and the two ways the JS expresses it.
#[derive(Debug, Clone, Copy)]
pub enum LenProfile {
    /// A plain normal/ghost/max triple, in steps. `ghost` is `None` for every
    /// role that never plays a ghost note — the JS simply omits the field —
    /// so a caller falls back to `normal` rather than reading a bogus zero.
    Plain { normal: f64, ghost: Option<f64>, max: f64 },
    /// `mode: 'sustain' | 'stab'`, holding through its own step count.
    Mode { mode: LenMode, normal: f64, max: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LenMode {
    Sustain,
    Stab,
}

#[derive(Debug, Clone, Copy)]
pub struct Velocity {
    pub accent: u8,
    pub normal: u8,
    pub ghost: u8,
}

/// A short melodic shape: how many notes to move and the window (in
/// semitones) they may roam across — `motif.js`'s `{ notes, window }`.
#[derive(Debug, Clone, Copy)]
pub struct MotifProfile {
    pub notes: (u32, u32),
    pub window: i32,
}

/// The rhythmic and dynamic grammar for one role within one genre.
#[derive(Debug, Clone)]
pub struct RoleProfile {
    /// One bar of sixteenths, relative likelihood of a trig.
    pub weights: [f64; 16],
    pub trigs_per_bar: (u32, u32),
    /// Register-window height in semitones; see `theory::window_for`.
    pub span: i32,
    /// Bass-only: the anchor note's length, in steps.
    pub anchor_len: Option<f64>,
    pub len: LenProfile,
    pub velocity: Velocity,
    /// Bass-only: how often an approach tone leans into the next chord.
    pub approach: Option<f64>,
    /// Bass-only: how often an ordinary (non-accent, non-ghost) trig reaches
    /// for a chord tone above the root instead of restating the root.
    ///
    /// `None` means [`parts::bass`](crate::parts::bass)'s own default of
    /// 0.45, which is the number every genre carried inline before this
    /// field existed — so leaving it `None` is exactly the old behaviour.
    /// A *rolling* bassline is the case that needs it lower: at twelve or
    /// sixteen trigs a bar, a 45% chance of moving off the root turns a roll
    /// into a melody, and a roll is a rhythm part. Rollers sets it to 0.15.
    pub chord_tone: Option<f64>,
    /// Bass: how often the anchor leaps an octave. Chord lead: how often the
    /// root, already at the top of its cycle, leaps one octave further.
    pub octave_leap: Option<f64>,
    /// Chords-only: strum stagger, in fractions of a step.
    pub strum: Option<f64>,
    /// Chords-only: how often the drop-2 spread is used.
    pub spread: Option<f64>,
    /// Lead-only.
    pub motif: Option<MotifProfile>,
    pub conditions: &'static [ConditionRecipe],
    pub lanes: &'static [LaneRecipe],
}

#[derive(Debug, Clone, Copy)]
pub struct GenreProfile {
    pub id: GenreId,
    pub label: &'static str,
    pub bpm: u32,
    pub bpm_range: (u32, u32),
    pub bars: u32,
    pub groove: [f64; 16],
}

fn swung(amount: f64) -> [f64; 16] {
    let mut out = [0.0; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        if i % 2 == 1 {
            *slot = amount;
        }
    }
    out
}

const DNB_GROOVE: [f64; 16] =
    [0.0, 0.05, 0.0, 0.06, 0.0, 0.05, 0.04, 0.07, 0.0, 0.05, 0.0, 0.06, 0.0, 0.05, 0.04, 0.07];

/// Rollers' groove: the eighths dead straight, the sixteenths between them
/// swung — and the "a" (steps 3, 7, 11, 15) leaning later than the "e"
/// (1, 5, 9, 13), which is what a shuffle *is* on a sixteenth grid. See the
/// shaker profiles, where the same asymmetry is spelled as weights.
///
/// It is written per-step rather than with [`swung`] for exactly that
/// asymmetry: `swung` pushes every odd step by one amount, which straightens
/// out the thing this genre is for. The eighths stay at zero because a
/// rolling bass at density 50 lands almost entirely on them, and a roll that
/// drags on its own pulse reads as late rather than as funk.
const ROLLERS_GROOVE: [f64; 16] =
    [0.0, 0.045, 0.0, 0.085, 0.0, 0.045, 0.0, 0.085, 0.0, 0.045, 0.0, 0.085, 0.0, 0.045, 0.0, 0.085];

pub fn genre_profile(id: GenreId) -> GenreProfile {
    match id {
        GenreId::Dnb => {
            GenreProfile { id, label: "DnB", bpm: 174, bpm_range: (172, 176), bars: 2, groove: DNB_GROOVE }
        }
        GenreId::Breaks => {
            GenreProfile { id, label: "Breaks", bpm: 135, bpm_range: (130, 140), bars: 2, groove: swung(0.1) }
        }
        GenreId::Electro => {
            GenreProfile { id, label: "Electro", bpm: 130, bpm_range: (125, 135), bars: 2, groove: swung(0.03) }
        }
        GenreId::House => {
            // House shuffle would normally be swing, which the generator may
            // not touch, so it comes out here — every off-16th pushed late,
            // which is the same musical result on one track.
            GenreProfile { id, label: "House", bpm: 124, bpm_range: (120, 128), bars: 1, groove: swung(0.14) }
        }
        GenreId::Techno => {
            // Straighter and harder than house: the same four-on-the-floor
            // kick, but the swing that makes house shuffle is exactly what
            // techno's hypnotic grid must not have, so the shuffle here is
            // barely there — just enough to keep the hats off a metronome.
            GenreProfile { id, label: "Techno", bpm: 130, bpm_range: (125, 145), bars: 2, groove: swung(0.02) }
        }
        GenreId::Rollers => {
            // 168, and the range is deliberately narrow. This is the deep
            // end of DnB rather than the middle of it: fast enough that the
            // bassline's sixteenths roll, slow enough that a chopped funk
            // break still reads as funk instead of as a blur. Neil picked
            // the band 2026-09-03; the four references bracket it from both
            // sides (Prodigy well under, Nero and Sub Focus a little over).
            GenreProfile { id, label: "Rollers", bpm: 168, bpm_range: (165, 170), bars: 2, groove: ROLLERS_GROOVE }
        }
    }
}

/// A drum voice's profile: a rhythm, plus a light sprinkle of PROB/COND
/// (`conditions`), and nothing else. Every field `theory`/pitch-aware
/// generation would read (`span`, `anchor_len`, `approach`, `octave_leap`,
/// `strum`, `spread`, `motif`) is `None`/inert, because a drum voice's
/// destination track *is* the sound — there is no register to choose and no
/// chord to answer. `lanes` stays empty: p-lock automation for a drum voice
/// is still future scope, not this stage's — only `conditions` graduated
/// out of "deliberately empty for the first cut".
///
/// **None of this is a port.** `js/gen/` has no drums role at all — see
/// [`Role::DRUM_VOICES`]'s doc comment — so every weight table below is
/// composed from ordinary genre convention (a DnB break's syncopated kick,
/// a four-on-the-floor house kick, a backbeat snare) rather than derived
/// from anything measured. Treat it as a reasonable starting point to tune
/// by ear, not as a value pinned by an oracle the way the melodic roles are.
/// The same is true of `conditions`: see `DRUM_SPINE_RECIPE`/
/// `DRUM_COLOUR_RECIPE`/`DRUM_FILL_RECIPE`'s doc comments for the tiering
/// and why the spine never carries `AltBar`.
///
/// # Reaching sixteenths
///
/// A `weights` slot of `0.0` is not "unlikely", it is *unreachable*:
/// `rhythm::rhythm_for` drops any step whose weight is `<= 0.0` before it
/// samples, so a table with zeroed odd slots caps the density slider at
/// eighths however far it is pushed. That is deliberate for a voice whose
/// whole identity is the off-beat (OpenHat sits on the "and"s and nowhere
/// else), and was a bug for the voices meant to be a texture — House and
/// Electro closed hats, and every genre's ride, could not play sixteenths
/// at all until 2026-08-22.
///
/// The fix, and the pattern to copy for any future texture voice: stock the
/// off-sixteenths at roughly a *tenth* of an eighth's weight and set
/// `trigs_per_bar`'s upper bound to 16.
///
/// A tenth looks brutal and is not. `rng::sample_weighted` is
/// Efraimidis–Spirakis (`key = U^(1/w)`), which draws each slot with
/// probability *proportional* to its weight — it does not rank the heavy
/// slots ahead of the light ones and work down. So the weight ratio sets
/// the eighth/sixteenth *mix* directly: stocking the odd slots at a quarter
/// still put a third of a sparse hat on the sixteenth grid, which is not a
/// house hat. A tenth holds the sparse end near nine-tenths eighths, and
/// costs nothing at the top of the slider, where the draw asks for all 16
/// candidates and therefore returns all 16 whatever their weights. Full
/// sixteenths at density 100 are guaranteed rather than likely — which is
/// what `parts::drums`'s
/// `a_texture_voice_reaches_full_sixteenths_at_full_density` pins, and
/// `a_texture_voice_still_sounds_like_eighths_at_the_sparse_end` pins the
/// other end.
fn drum_profile(
    weights: [f64; 16],
    trigs_per_bar: (u32, u32),
    len: LenProfile,
    velocity: Velocity,
    conditions: &'static [ConditionRecipe],
) -> RoleProfile {
    RoleProfile {
        weights,
        trigs_per_bar,
        span: 0,
        anchor_len: None,
        len,
        velocity,
        approach: None,
        chord_tone: None,
        octave_leap: None,
        strum: None,
        spread: None,
        motif: None,
        conditions,
        lanes: &[],
    }
}

pub fn role_profile(id: GenreId, role: Role) -> RoleProfile {
    match (id, role) {
        // **A call and a response are one voice taking turns**, so both play
        // the genre's own lead grammar rather than restating five weight
        // tables twice over. Everything that makes the pair read as a
        // conversation is *phrasing* — which turn a voice plays in, and what
        // it answers — and phrasing is `parts::lead`'s business, not this
        // file's. The one thing decided here is that a response speaks a few
        // points under the call: an answer is spoken, not shouted, and
        // without it two identical velocity curves alternating read as one
        // lead with holes in it.
        (g, Role::LeadCall) => role_profile(g, Role::Lead),
        // **The chord lead is one grammar in every genre**, because it is a
        // transcription of one pattern — the Analog Four's factory A01 — rather
        // than a genre's idiom: straight eighths, flat velocity, no conditions,
        // a length that breathes before the next trig. What a genre lends it is
        // its chord lanes, so a DN2 row still gets a filter to move.
        (g, Role::ChordLead) => RoleProfile {
            weights: [1.0, 0.0, 0.9, 0.0, 1.0, 0.0, 0.9, 0.0, 1.0, 0.0, 0.9, 0.0, 1.0, 0.0, 0.9, 0.0],
            trigs_per_bar: (4, 8),
            span: 36,
            anchor_len: None,
            len: LenProfile::Plain { normal: 1.75, ghost: None, max: 4.0 },
            velocity: Velocity { accent: 118, normal: 110, ghost: 96 },
            approach: None,
            chord_tone: None,
            octave_leap: Some(0.12),
            strum: None,
            spread: None,
            motif: None,
            conditions: &[],
            lanes: role_profile(g, Role::Chords).lanes,
        },
        (g, Role::LeadResponse) => {
            let lead = role_profile(g, Role::Lead);
            let softer = |v: u8| v.saturating_sub(6).max(1);
            RoleProfile {
                velocity: Velocity {
                    accent: softer(lead.velocity.accent),
                    normal: softer(lead.velocity.normal),
                    ghost: softer(lead.velocity.ghost),
                },
                ..lead
            }
        }

        (GenreId::Dnb, Role::Bass) => RoleProfile {
            // A long root anchor on the 1, then syncopated stabs off the
            // grid. The quarters at 4/8/12 are deliberately weak:
            // four-on-the-floor is the one thing a DnB bassline must not do.
            weights: [1.0, 0.15, 0.35, 0.7, 0.25, 0.2, 0.6, 0.5, 0.5, 0.2, 0.65, 0.55, 0.3, 0.25, 0.7, 0.45],
            trigs_per_bar: (2, 7),
            span: 24,
            anchor_len: Some(4.0),
            len: LenProfile::Plain { normal: 1.5, ghost: Some(0.5), max: 6.0 },
            velocity: Velocity { accent: 120, normal: 100, ghost: 66 },
            approach: Some(0.35),
            chord_tone: None,
            octave_leap: Some(0.12),
            strum: None,
            spread: None,
            motif: None,
            conditions: &[ALT_BARS, GHOST_PROB, FILL_EXTRA],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Rise, from: 40, to: 105 },
                LaneRecipe { name: "fx.overdrive", shape: LaneShape::Accent, from: 20, to: 90 },
                LaneRecipe { name: "lfo1.depth", shape: LaneShape::Swell, from: 64, to: 96 },
            ],
        },
        (GenreId::Dnb, Role::Chords) => RoleProfile {
            // Sparse sustained stabs — often only the 1 and the "and" of 3.
            weights: [1.0, 0.05, 0.1, 0.15, 0.2, 0.05, 0.12, 0.1, 0.35, 0.05, 0.8, 0.15, 0.2, 0.05, 0.25, 0.1],
            trigs_per_bar: (1, 3),
            span: 24,
            anchor_len: None,
            len: LenProfile::Mode { mode: LenMode::Sustain, normal: 8.0, max: 16.0 },
            velocity: Velocity { accent: 104, normal: 92, ghost: 72 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: Some(0.04),
            spread: Some(0.4),
            motif: None,
            conditions: &[ALT_BARS, EVERY_FOURTH, WEAK_PROB],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Arc, from: 50, to: 100 },
                LaneRecipe { name: "fx.reverbSend", shape: LaneShape::Swell, from: 30, to: 95 },
            ],
        },
        (GenreId::Dnb, Role::Lead) => RoleProfile {
            // Half-time feeling: strong on 1 and the "and" of 2, wide space
            // between.
            weights: [0.9, 0.1, 0.3, 0.2, 0.4, 0.15, 0.6, 0.3, 0.7, 0.15, 0.4, 0.35, 0.5, 0.2, 0.55, 0.4],
            trigs_per_bar: (2, 6),
            span: 30,
            anchor_len: None,
            len: LenProfile::Plain { normal: 1.0, ghost: None, max: 4.0 },
            velocity: Velocity { accent: 112, normal: 96, ghost: 70 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: None,
            spread: None,
            motif: Some(MotifProfile { notes: (3, 5), window: 8 }),
            conditions: &[ALT_BARS, EVERY_FOURTH, WEAK_PROB, ANSWERING],
            lanes: &[
                LaneRecipe { name: "amp.pan", shape: LaneShape::Wander, from: 40, to: 88 },
                LaneRecipe { name: "fx.delaySend", shape: LaneShape::Swell, from: 20, to: 90 },
            ],
        },

        (GenreId::Breaks, Role::Bass) => RoleProfile {
            // Funk-leaning: busy, syncopated, and full of low-velocity ghosts.
            weights: [1.0, 0.2, 0.45, 0.3, 0.3, 0.5, 0.55, 0.3, 0.6, 0.25, 0.5, 0.45, 0.35, 0.5, 0.6, 0.4],
            trigs_per_bar: (3, 9),
            span: 24,
            anchor_len: Some(2.0),
            len: LenProfile::Plain { normal: 1.0, ghost: Some(0.5), max: 4.0 },
            velocity: Velocity { accent: 118, normal: 98, ghost: 58 },
            approach: Some(0.3),
            chord_tone: None,
            octave_leap: Some(0.18),
            strum: None,
            spread: None,
            motif: None,
            conditions: &[GHOST_PROB, ALT_BARS, FILL_EXTRA, FILL_STEP_ASIDE],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Wander, from: 45, to: 100 },
                LaneRecipe { name: "fx.overdrive", shape: LaneShape::Accent, from: 15, to: 75 },
            ],
        },
        (GenreId::Breaks, Role::Chords) => RoleProfile {
            // Stabs off the beat, the way a chopped break's keys land.
            weights: [0.5, 0.1, 0.7, 0.3, 0.2, 0.1, 0.75, 0.25, 0.4, 0.1, 0.7, 0.3, 0.25, 0.15, 0.6, 0.3],
            trigs_per_bar: (2, 5),
            span: 24,
            anchor_len: None,
            len: LenProfile::Mode { mode: LenMode::Stab, normal: 0.75, max: 4.0 },
            velocity: Velocity { accent: 108, normal: 94, ghost: 74 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: Some(0.08),
            spread: Some(0.3),
            motif: None,
            conditions: &[WEAK_PROB, EVERY_FOURTH, ALT_BARS],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Accent, from: 55, to: 110 },
                LaneRecipe { name: "fx.delaySend", shape: LaneShape::Swell, from: 25, to: 85 },
            ],
        },
        (GenreId::Breaks, Role::Lead) => RoleProfile {
            // Answering in the gaps — which the busy-step penalty does, not
            // this.
            weights: [0.5, 0.2, 0.35, 0.45, 0.5, 0.25, 0.4, 0.5, 0.55, 0.25, 0.4, 0.5, 0.45, 0.3, 0.5, 0.55],
            trigs_per_bar: (3, 8),
            span: 30,
            anchor_len: None,
            len: LenProfile::Plain { normal: 0.75, ghost: None, max: 3.0 },
            velocity: Velocity { accent: 110, normal: 94, ghost: 66 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: None,
            spread: None,
            motif: Some(MotifProfile { notes: (3, 6), window: 8 }),
            conditions: &[WEAK_PROB, ALT_BARS, EVERY_FOURTH, ANSWERING],
            lanes: &[
                LaneRecipe { name: "amp.pan", shape: LaneShape::Wander, from: 36, to: 92 },
                LaneRecipe { name: "fx.reverbSend", shape: LaneShape::Swell, from: 20, to: 80 },
            ],
        },

        (GenreId::Electro, Role::Bass) => RoleProfile {
            // Sixteenth-driven and staccato, with octave leaps doing the
            // melody.
            weights: [1.0, 0.6, 0.7, 0.6, 0.75, 0.6, 0.7, 0.6, 0.85, 0.6, 0.7, 0.6, 0.8, 0.6, 0.75, 0.65],
            trigs_per_bar: (6, 14),
            span: 24,
            anchor_len: Some(1.0),
            len: LenProfile::Plain { normal: 0.5, ghost: Some(0.5), max: 1.0 },
            velocity: Velocity { accent: 122, normal: 100, ghost: 72 },
            approach: Some(0.15),
            chord_tone: None,
            octave_leap: Some(0.45),
            strum: None,
            spread: None,
            motif: None,
            conditions: &[ALT_BARS, WEAK_PROB, EVERY_FOURTH],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Wander, from: 40, to: 110 },
                LaneRecipe { name: "filter.resonance", shape: LaneShape::Rise, from: 20, to: 80 },
                LaneRecipe { name: "lfo1.depth", shape: LaneShape::Pulse, from: 64, to: 100 },
            ],
        },
        (GenreId::Electro, Role::Chords) => RoleProfile {
            // Held, or pulsing on the beat — the machine, not the band.
            weights: [1.0, 0.1, 0.2, 0.1, 0.7, 0.1, 0.2, 0.1, 0.8, 0.1, 0.2, 0.1, 0.7, 0.1, 0.25, 0.15],
            trigs_per_bar: (1, 4),
            span: 24,
            anchor_len: None,
            len: LenProfile::Mode { mode: LenMode::Sustain, normal: 4.0, max: 16.0 },
            velocity: Velocity { accent: 106, normal: 92, ghost: 76 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: Some(0.0),
            spread: Some(0.2),
            motif: None,
            conditions: &[ALT_BARS, EVERY_FOURTH],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Arc, from: 45, to: 100 },
                LaneRecipe { name: "fx.chorusSend", shape: LaneShape::Rise, from: 20, to: 80 },
            ],
        },
        (GenreId::Electro, Role::Lead) => RoleProfile {
            // Arpeggio-ish: even, mechanical, chord tones climbing.
            weights: [0.8, 0.3, 0.6, 0.35, 0.7, 0.3, 0.6, 0.35, 0.75, 0.3, 0.6, 0.35, 0.7, 0.3, 0.6, 0.4],
            trigs_per_bar: (4, 12),
            span: 30,
            anchor_len: None,
            len: LenProfile::Plain { normal: 0.5, ghost: None, max: 2.0 },
            velocity: Velocity { accent: 112, normal: 96, ghost: 74 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: None,
            spread: None,
            motif: Some(MotifProfile { notes: (4, 6), window: 4 }),
            conditions: &[ALT_BARS, EVERY_FOURTH, WEAK_PROB],
            lanes: &[
                LaneRecipe { name: "amp.pan", shape: LaneShape::Pulse, from: 44, to: 84 },
                LaneRecipe { name: "filter.envDepth", shape: LaneShape::Accent, from: 64, to: 100 },
            ],
        },

        (GenreId::House, Role::Bass) => RoleProfile {
            // The off-beat bass: a note on every "and", almost nothing on
            // the beat.
            weights: [0.35, 0.05, 1.0, 0.1, 0.3, 0.05, 1.0, 0.1, 0.3, 0.05, 1.0, 0.1, 0.3, 0.05, 1.0, 0.15],
            trigs_per_bar: (4, 8),
            span: 24,
            anchor_len: Some(1.0),
            len: LenProfile::Plain { normal: 1.0, ghost: Some(0.5), max: 2.0 },
            velocity: Velocity { accent: 112, normal: 100, ghost: 78 },
            approach: Some(0.2),
            chord_tone: None,
            octave_leap: Some(0.2),
            strum: None,
            spread: None,
            motif: None,
            conditions: &[WEAK_PROB, ALT_BARS],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Rise, from: 45, to: 95 },
                LaneRecipe { name: "fx.chorusSend", shape: LaneShape::Swell, from: 20, to: 70 },
            ],
        },
        (GenreId::House, Role::Chords) => RoleProfile {
            // Seventh stabs on the off-beat, the sound of the whole genre.
            weights: [0.5, 0.05, 0.9, 0.15, 0.25, 0.05, 0.85, 0.15, 0.35, 0.05, 0.9, 0.15, 0.25, 0.05, 0.8, 0.2],
            trigs_per_bar: (2, 6),
            span: 24,
            anchor_len: None,
            len: LenProfile::Mode { mode: LenMode::Stab, normal: 1.0, max: 4.0 },
            velocity: Velocity { accent: 106, normal: 94, ghost: 78 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: Some(0.06),
            spread: Some(0.5),
            motif: None,
            conditions: &[WEAK_PROB, ALT_BARS, EVERY_FOURTH],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Accent, from: 55, to: 105 },
                LaneRecipe { name: "fx.reverbSend", shape: LaneShape::Swell, from: 25, to: 85 },
            ],
        },
        (GenreId::House, Role::Lead) => RoleProfile {
            // Simple and hooky — a few notes you can hum.
            weights: [0.7, 0.15, 0.4, 0.2, 0.5, 0.15, 0.45, 0.25, 0.6, 0.15, 0.4, 0.25, 0.5, 0.2, 0.45, 0.3],
            trigs_per_bar: (2, 6),
            span: 30,
            anchor_len: None,
            len: LenProfile::Plain { normal: 1.0, ghost: None, max: 4.0 },
            velocity: Velocity { accent: 108, normal: 94, ghost: 72 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: None,
            spread: None,
            motif: Some(MotifProfile { notes: (3, 5), window: 8 }),
            conditions: &[WEAK_PROB, EVERY_FOURTH, ANSWERING],
            lanes: &[
                LaneRecipe { name: "fx.delaySend", shape: LaneShape::Swell, from: 25, to: 90 },
                LaneRecipe { name: "amp.pan", shape: LaneShape::Wander, from: 44, to: 84 },
            ],
        },

        (GenreId::Techno, Role::Bass) => RoleProfile {
            // A rolling, hypnotic pulse — mostly eighths with the beat
            // favoured, closer to Electro's staccato than DnB's syncopation:
            // techno's low end is a loop to sink into, not a line to follow.
            weights: [1.0, 0.1, 0.5, 0.1, 0.6, 0.1, 0.5, 0.1, 0.6, 0.1, 0.5, 0.1, 0.6, 0.1, 0.5, 0.1],
            trigs_per_bar: (6, 12),
            span: 24,
            anchor_len: Some(0.8),
            len: LenProfile::Plain { normal: 0.8, ghost: Some(0.4), max: 1.0 },
            velocity: Velocity { accent: 116, normal: 100, ghost: 70 },
            approach: Some(0.1),
            chord_tone: None,
            octave_leap: Some(0.15),
            strum: None,
            spread: None,
            motif: None,
            conditions: &[ALT_BARS, WEAK_PROB],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Wander, from: 35, to: 100 },
                LaneRecipe { name: "fx.overdrive", shape: LaneShape::Accent, from: 20, to: 85 },
            ],
        },
        (GenreId::Techno, Role::Chords) => RoleProfile {
            // Barely there: one long pad hit a bar, sometimes a second at the
            // half — the harmonic wash under the loop, not a progression
            // anyone is meant to follow.
            weights: [1.0, 0.05, 0.1, 0.05, 0.1, 0.05, 0.1, 0.05, 0.6, 0.05, 0.1, 0.05, 0.1, 0.05, 0.1, 0.05],
            trigs_per_bar: (1, 3),
            span: 24,
            anchor_len: None,
            len: LenProfile::Mode { mode: LenMode::Sustain, normal: 12.0, max: 16.0 },
            velocity: Velocity { accent: 100, normal: 88, ghost: 70 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: Some(0.0),
            spread: Some(0.3),
            motif: None,
            conditions: &[ALT_BARS, EVERY_FOURTH],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Arc, from: 40, to: 95 },
                LaneRecipe { name: "fx.reverbSend", shape: LaneShape::Swell, from: 35, to: 100 },
            ],
        },
        (GenreId::Techno, Role::Lead) => RoleProfile {
            // The acid line: even sixteenths in a narrow window, the same
            // few notes circling — the hypnotic repetition is the hook, so
            // `motif`'s window is the tightest of any genre's lead.
            weights: [0.8, 0.4, 0.6, 0.4, 0.7, 0.4, 0.6, 0.4, 0.8, 0.4, 0.6, 0.4, 0.7, 0.4, 0.6, 0.4],
            trigs_per_bar: (6, 14),
            span: 24,
            anchor_len: None,
            len: LenProfile::Plain { normal: 0.25, ghost: None, max: 1.0 },
            velocity: Velocity { accent: 114, normal: 98, ghost: 74 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: None,
            spread: None,
            motif: Some(MotifProfile { notes: (5, 8), window: 5 }),
            conditions: &[ALT_BARS, WEAK_PROB, EVERY_FOURTH],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Wander, from: 30, to: 115 },
                LaneRecipe { name: "filter.resonance", shape: LaneShape::Rise, from: 30, to: 95 },
                LaneRecipe { name: "lfo1.depth", shape: LaneShape::Pulse, from: 60, to: 100 },
            ],
        },

        // --- drums: new design, no oracle — see `drum_profile`'s header ---------

        (GenreId::Dnb, Role::Kick) => drum_profile(
            // The 1, and the syncopated "and of 3" a breakbeat kick leans on.
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.0, 0.3],
            (1, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 118, normal: 102, ghost: 70 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Dnb, Role::Snare) => drum_profile(
            // The backbeat (2 and 4), with a ghost flourish leading into each.
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.2],
            (2, 4),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 115, normal: 100, ghost: 65 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Dnb, Role::Clap) => drum_profile(
            // Doubling the snare's backbeat rather than arguing with it — a
            // clap in DnB is a layer on the 2 and the 4, so this is the snare
            // table a shade softer, with its own pickup into the second hit.
            [0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0, 0.2, 0.0, 0.0, 0.0, 0.3, 0.9, 0.0, 0.0, 0.0],
            (1, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 110, normal: 94, ghost: 62 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Dnb, Role::Rimshot) => drum_profile(
            // The opposite job to the clap: every beat is 0, so a rimshot can
            // only land in the gaps the kick and snare leave. Weights under
            // `rhythm::GHOST_WEIGHT` off the beat make most of them ghosts,
            // which is what a rimshot tick should be.
            [0.0, 0.3, 0.0, 0.5, 0.0, 0.0, 0.45, 0.0, 0.0, 0.35, 0.0, 0.5, 0.0, 0.0, 0.45, 0.0],
            (1, 4),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 104, normal: 84, ghost: 56 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Dnb, Role::ClosedHat) => drum_profile(
            [0.8, 0.5, 0.6, 0.5, 0.7, 0.5, 0.6, 0.5, 0.8, 0.5, 0.6, 0.5, 0.7, 0.5, 0.6, 0.5],
            (10, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 95, normal: 80, ghost: 55 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Dnb, Role::OpenHat) => drum_profile(
            // The off-beat "and"s — steps 2, 6, 10, 14.
            [0.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.5, 0.0],
            (2, 4),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 100, normal: 85, ghost: 60 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Dnb, Role::Ride) => drum_profile(
            // Steady eighths with the beats favoured — a ride is the hat's
            // job played longer and quieter, so the table is the closed hat's
            // shape thinned to the "and"s and the length doubled.
            [0.7, 0.07, 0.5, 0.07, 0.6, 0.07, 0.5, 0.07, 0.7, 0.07, 0.5, 0.07, 0.6, 0.07, 0.5, 0.07],
            (5, 16),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 88, normal: 74, ghost: 52 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Dnb, Role::Shaker) => drum_profile(
            // Eighths on the pulse with whispered sixteenths between them —
            // a DnB shaker's job is to fill the space the syncopated kick
            // leaves, not to argue with it, so the "e" and "a" sit far under
            // `rhythm::GHOST_WEIGHT` and come out as ghosts.
            [0.65, 0.06, 0.5, 0.06, 0.6, 0.06, 0.5, 0.06, 0.65, 0.06, 0.5, 0.06, 0.6, 0.06, 0.5, 0.06],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 78, normal: 66, ghost: 46 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Dnb, Role::Tom) => drum_profile(
            // Fill material, weighted to the back half of the bar so it reads
            // as a run into the next 1 rather than a competing pulse.
            [0.0, 0.0, 0.0, 0.2, 0.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.4, 0.0, 0.0, 0.35, 0.0, 0.5],
            (1, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 112, normal: 92, ghost: 60 },
            DRUM_FILL_RECIPE,
        ),

        (GenreId::Breaks, Role::Kick) => drum_profile(
            // Funk-breakbeat syncopation: busier than DnB's, still leaning
            // on the 1.
            [1.0, 0.0, 0.3, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.4, 0.0, 0.3, 0.0, 0.0, 0.6, 0.0],
            (2, 5),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 116, normal: 98, ghost: 62 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Breaks, Role::Snare) => drum_profile(
            [0.0, 0.0, 0.2, 0.0, 1.0, 0.0, 0.0, 0.3, 0.0, 0.2, 0.0, 0.0, 1.0, 0.0, 0.0, 0.4],
            (2, 5),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 112, normal: 96, ghost: 60 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Breaks, Role::Clap) => drum_profile(
            // Same backbeat as the snare, but funk-syncopated on the way in —
            // the extra weights at 2 and 9 are the ones a break's clap uses
            // to sit slightly ahead of the band.
            [0.0, 0.0, 0.25, 0.0, 0.9, 0.0, 0.0, 0.3, 0.0, 0.25, 0.0, 0.0, 0.9, 0.0, 0.0, 0.35],
            (2, 4),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 108, normal: 92, ghost: 58 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Breaks, Role::Rimshot) => drum_profile(
            // Off-grid only, and busier than DnB's: a break tolerates chatter
            // between the hits in a way a DnB two-step does not.
            [0.0, 0.25, 0.0, 0.4, 0.0, 0.3, 0.0, 0.45, 0.0, 0.3, 0.0, 0.4, 0.0, 0.25, 0.0, 0.5],
            (2, 5),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 102, normal: 82, ghost: 54 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Breaks, Role::ClosedHat) => drum_profile(
            [0.8, 0.5, 0.6, 0.5, 0.7, 0.5, 0.6, 0.5, 0.8, 0.5, 0.6, 0.5, 0.7, 0.5, 0.6, 0.5],
            (10, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 92, normal: 78, ghost: 52 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Breaks, Role::OpenHat) => drum_profile(
            [0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.6, 0.0],
            (2, 4),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 98, normal: 82, ghost: 58 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Breaks, Role::Ride) => drum_profile(
            // Eighths, which the genre's `swung(0.1)` groove then pushes late
            // — the ride is where that shuffle is most audible.
            [0.7, 0.07, 0.55, 0.07, 0.6, 0.07, 0.55, 0.07, 0.7, 0.07, 0.55, 0.07, 0.6, 0.07, 0.55, 0.07],
            (5, 16),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 86, normal: 72, ghost: 50 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Breaks, Role::Shaker) => drum_profile(
            // The "a" outweighs the "e", which is what a shuffle *is* on a
            // sixteenth grid: the second half of each beat leans late, so as
            // the slider fills in the sixteenths the "a"s arrive first. The
            // genre's `swung(0.1)` groove then pushes them later still.
            [0.65, 0.05, 0.5, 0.09, 0.6, 0.05, 0.5, 0.09, 0.65, 0.05, 0.5, 0.09, 0.6, 0.05, 0.5, 0.09],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 76, normal: 64, ghost: 45 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Breaks, Role::Tom) => drum_profile(
            // A funk tom answers the kick's syncopation, so this leans on the
            // steps the Breaks kick table leaves at 0. Two of them sit above
            // `rhythm::GHOST_WEIGHT` and the rest below, which is what makes
            // a run of toms a fill with dynamics rather than an even mutter.
            [0.0, 0.0, 0.0, 0.3, 0.0, 0.45, 0.0, 0.0, 0.0, 0.35, 0.0, 0.3, 0.0, 0.0, 0.0, 0.5],
            (1, 4),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 110, normal: 90, ghost: 58 },
            DRUM_FILL_RECIPE,
        ),

        (GenreId::Electro, Role::Kick) => drum_profile(
            // Mechanical, every beat plus a secondary hit before it.
            [1.0, 0.0, 0.0, 0.0, 0.7, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.7, 0.0, 0.0, 0.0],
            (4, 6),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.25), max: 0.5 },
            Velocity { accent: 120, normal: 104, ghost: 74 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Electro, Role::Snare) => drum_profile(
            // Clean backbeat, no ghosts — the machine, not the band.
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            (2, 2),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.25), max: 0.5 },
            Velocity { accent: 114, normal: 100, ghost: 70 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Electro, Role::Clap) => drum_profile(
            // The 808 clap: dead on 2 and 4, nothing else, no ghosts. Same
            // table as the Electro snare on purpose — layering the two *is*
            // the electro backbeat, and `parts::drums` ignores the busy map
            // precisely so they may share a step.
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            (2, 2),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.25), max: 0.5 },
            Velocity { accent: 112, normal: 100, ghost: 72 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Electro, Role::Rimshot) => drum_profile(
            // Every off-sixteenth, evenly — the machine tick, not a player's
            // accent. Flat weights let density alone decide how many fire.
            [0.0, 0.4, 0.0, 0.4, 0.0, 0.4, 0.0, 0.4, 0.0, 0.4, 0.0, 0.4, 0.0, 0.4, 0.0, 0.4],
            (3, 6),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 100, normal: 86, ghost: 58 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Electro, Role::ClosedHat) => drum_profile(
            // Eighths at the bottom of the slider, sixteenths at the top:
            // the off-sixteenths are stocked at a *tenth* of an eighth's
            // weight, which is what it costs to keep the sparse end sounding
            // like electro — see `drum_profile`'s "Reaching sixteenths" for
            // why a tenth and not the quarter this first shipped as.
            [0.8, 0.08, 0.6, 0.08, 0.8, 0.08, 0.6, 0.08, 0.8, 0.08, 0.6, 0.08, 0.8, 0.08, 0.6, 0.08],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 96, normal: 80, ghost: 55 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Electro, Role::OpenHat) => drum_profile(
            [0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 0.0],
            (2, 4),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 100, normal: 84, ghost: 58 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Electro, Role::Ride) => drum_profile(
            // Flat eighths, no beat emphasis: electro's pulse comes from the
            // kick, and a ride that also accented the beats would double it.
            [0.6, 0.06, 0.6, 0.06, 0.6, 0.06, 0.6, 0.06, 0.6, 0.06, 0.6, 0.06, 0.6, 0.06, 0.6, 0.06],
            (5, 16),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 84, normal: 72, ghost: 50 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Electro, Role::Shaker) => drum_profile(
            // Machine-flat: every eighth carries the same weight and every
            // off-sixteenth the same as every other, so the texture fills in
            // evenly rather than shuffled. Electro's swing is zero
            // (`straight()`), and the shaker is where that shows.
            [0.6, 0.07, 0.6, 0.07, 0.6, 0.07, 0.6, 0.07, 0.6, 0.07, 0.6, 0.07, 0.6, 0.07, 0.6, 0.07],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 74, normal: 63, ghost: 45 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Electro, Role::Tom) => drum_profile(
            // The 808 tom run: three steps, each later and heavier than the
            // last, so what comes out is a descent into the bar line.
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.0, 0.35, 0.0, 0.0, 0.0, 0.5, 0.0],
            (1, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 114, normal: 96, ghost: 64 },
            DRUM_FILL_RECIPE,
        ),

        (GenreId::House, Role::Kick) => drum_profile(
            // Four-on-the-floor — the genre-defining kick, steady every beat.
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            (4, 4),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.5), max: 1.0 },
            Velocity { accent: 118, normal: 108, ghost: 80 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::House, Role::Snare) => drum_profile(
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            (2, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 110, normal: 96, ghost: 68 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::House, Role::Clap) => drum_profile(
            // The house clap, on the 2 and the 4 with the snare, plus the
            // pickup at 11 that gives it the flam house claps are known for.
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.25, 1.0, 0.0, 0.0, 0.0],
            (2, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 112, normal: 98, ghost: 66 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::House, Role::Rimshot) => drum_profile(
            // Off-sixteenth ticks with the second half of each beat favoured,
            // which is where a house rim sits against the four-on-the-floor.
            [0.0, 0.3, 0.0, 0.45, 0.0, 0.3, 0.0, 0.45, 0.0, 0.3, 0.0, 0.45, 0.0, 0.3, 0.0, 0.45],
            (3, 6),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 98, normal: 82, ghost: 56 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::House, Role::ClosedHat) => drum_profile(
            // Flat eighths, plus the off-sixteenths the slider can now climb
            // into — see Electro's closed hat for why they are stocked so
            // light. A house hat at full sixteenths is a ride-out, which is
            // exactly what the top of the slider should be for.
            [0.7, 0.08, 0.7, 0.08, 0.7, 0.08, 0.7, 0.08, 0.7, 0.08, 0.7, 0.08, 0.7, 0.08, 0.7, 0.08],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 94, normal: 78, ghost: 54 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::House, Role::OpenHat) => drum_profile(
            // The classic house off-beat open hat.
            [0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.8, 0.0],
            (3, 4),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 104, normal: 88, ghost: 62 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::House, Role::Ride) => drum_profile(
            // Eighths under the shuffle — the genre's `swung(0.14)` groove
            // pushes every odd step late, and the ride carries that.
            [0.6, 0.06, 0.5, 0.06, 0.6, 0.06, 0.5, 0.06, 0.6, 0.06, 0.5, 0.06, 0.6, 0.06, 0.5, 0.06],
            (5, 16),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 86, normal: 72, ghost: 50 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::House, Role::Shaker) => drum_profile(
            // House's shuffle carrier — the same late-leaning "a" as Breaks
            // but more pronounced, because `swung(0.14)` is the deepest
            // groove any genre here ships and the shaker is the voice dense
            // enough to make it audible.
            [0.65, 0.04, 0.5, 0.1, 0.6, 0.04, 0.5, 0.1, 0.65, 0.04, 0.5, 0.1, 0.6, 0.04, 0.5, 0.1],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 76, normal: 64, ghost: 45 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::House, Role::Tom) => drum_profile(
            // Sparse and entirely off the beat: with a kick on all four, a
            // tom's only useful place is between them. Straddles
            // `rhythm::GHOST_WEIGHT` for the same reason the Breaks tom does.
            [0.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.45, 0.0, 0.0, 0.0, 0.0, 0.35, 0.0, 0.0, 0.5, 0.0],
            (1, 4),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 108, normal: 90, ghost: 58 },
            DRUM_FILL_RECIPE,
        ),

        (GenreId::Techno, Role::Kick) => drum_profile(
            // Four-on-the-floor again, but harder and louder than House's —
            // techno's kick is the room, not the pulse under the room.
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            (4, 4),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.5), max: 1.0 },
            Velocity { accent: 122, normal: 112, ghost: 84 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Techno, Role::Snare) => drum_profile(
            // A clean, mechanical backbeat with no ghosts — same shape as
            // Electro's, because techno's snare is a machine hit too, not a
            // player's — just harder.
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            (2, 2),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.25), max: 0.5 },
            Velocity { accent: 116, normal: 102, ghost: 74 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Techno, Role::Clap) => drum_profile(
            // Doubles the snare's backbeat with a syncopated pickup at the
            // "and" of 3 — the extra push into the back half every peak-time
            // techno clap track leans on.
            [0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3, 0.0, 0.9, 0.0, 0.0, 0.0],
            (2, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 114, normal: 98, ghost: 68 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Techno, Role::Rimshot) => drum_profile(
            // Dense off-sixteenth ticks — the percussive chatter that fills
            // the space a sparse techno arrangement leaves open.
            [0.0, 0.35, 0.0, 0.4, 0.0, 0.35, 0.0, 0.4, 0.0, 0.35, 0.0, 0.4, 0.0, 0.35, 0.0, 0.4],
            (4, 8),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 100, normal: 84, ghost: 56 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Techno, Role::ClosedHat) => drum_profile(
            // The genre's signature: relentless, near-even sixteenths with
            // the off-16ths a shade louder — the "tss-tss" that never lets up.
            [0.6, 0.7, 0.6, 0.7, 0.6, 0.7, 0.6, 0.7, 0.6, 0.7, 0.6, 0.7, 0.6, 0.7, 0.6, 0.7],
            (12, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 92, normal: 78, ghost: 52 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Techno, Role::OpenHat) => drum_profile(
            // The off-beat open hat, same slot as House's — the "and" of
            // every beat, techno's other constant besides the kick.
            [0.0, 0.0, 0.7, 0.0, 0.0, 0.0, 0.7, 0.0, 0.0, 0.0, 0.7, 0.0, 0.0, 0.0, 0.7, 0.0],
            (3, 4),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 100, normal: 84, ghost: 58 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Techno, Role::Ride) => drum_profile(
            // Flat, quiet eighths with no beat emphasis — a metallic texture
            // under the loop rather than a voice of its own, the same
            // reasoning as Electro's ride.
            [0.5, 0.05, 0.5, 0.05, 0.5, 0.05, 0.5, 0.05, 0.5, 0.05, 0.5, 0.05, 0.5, 0.05, 0.5, 0.05],
            (5, 16),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 80, normal: 68, ghost: 48 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Techno, Role::Shaker) => drum_profile(
            // The off-sixteenths carry twice the weight of any other genre's:
            // a techno shaker should arrive at its relentless full grid early
            // on the slider, because that grid *is* the genre. That is done
            // with the weights and not by raising `trigs_per_bar`'s floor —
            // a floor of eight asks for every eighth in the bar, and the
            // draw then has to reach into the light odd slots to fill the
            // last of them, which scatters the sparse end instead of
            // pulsing. Six leaves it room.
            [0.6, 0.12, 0.55, 0.12, 0.6, 0.12, 0.55, 0.12, 0.6, 0.12, 0.55, 0.12, 0.6, 0.12, 0.55, 0.12],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 74, normal: 64, ghost: 46 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Techno, Role::Tom) => drum_profile(
            // Sparse and off the kick, same logic as House's: with the kick
            // on all four, a tom's only room is the gaps between.
            [0.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.4, 0.0, 0.0, 0.0, 0.0, 0.35, 0.0, 0.0, 0.5, 0.0],
            (1, 4),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 110, normal: 90, ghost: 58 },
            DRUM_FILL_RECIPE,
        ),

        // --- Rollers ---------------------------------------------------------
        //
        // The bass is the genre; everything else is written to stay out of
        // its way. Two rules run through the twelve arms below:
        //
        //   * **The eighths carry the weight, the sixteenths are stocked at a
        //     tenth of it.** That is the texture-voice pattern from
        //     [`drum_profile`]'s "Reaching sixteenths", used here on a
        //     *melodic* role for the first time, and it is what makes one
        //     slider cross from an eighth-note pulse to a full roll. Because
        //     `rng::sample_weighted` draws proportionally, the eight heavy
        //     slots are taken first and the light ones fill in behind them.
        //   * **Everything else is simpler than its DnB or Breaks
        //     equivalent.** Fewer chord trigs than any other genre, a shorter
        //     motif in a narrower window, and no `WEAK_PROB` on the lead — a
        //     hook with random holes in it is not a hook.
        (GenreId::Rollers, Role::Bass) => RoleProfile {
            // Straight eighths with the off-sixteenths stocked at a tenth, so
            // `trigs_per_bar` alone decides where on the slider the roll
            // arrives. With `(4, 16)`: density 0 is quarters, 50 is the eight
            // eighths plus two, 75 is eight plus five, and 100 is all sixteen
            // — guaranteed, not likely, because a draw of 16 from 16
            // candidates returns them all whatever the weights say.
            //
            // The last "a" (step 15) is the one light slot lifted above the
            // rest: it is the pickup into the next bar's downbeat, and a
            // roller that turns over there sounds intentional in a way the
            // other seven do not.
            //
            // **The "and"s sit at 0.6, not up with the beats**, and that gap
            // is load-bearing rather than cosmetic. `rhythm::rhythm_for`
            // reads the same number twice: once to choose the step, and once
            // to *label* it — `accent: is_beat(step) || weight >= 0.8`. A
            // table with all eight eighths up at 0.85 therefore made all
            // eight accents, and `bass.rs` returns the bare root on every
            // accent, so the first cut of this profile played one pitch for
            // sixteen steps. At 0.6 the "and"s are ordinary trigs: normal
            // velocity, and eligible for the octave leap and the chord tone.
            // The beats stay accents regardless of their weight, via
            // `is_beat`, which is what puts the root back on the pulse.
            weights: [1.0, 0.09, 0.6, 0.1, 0.95, 0.09, 0.6, 0.11, 1.0, 0.09, 0.6, 0.1, 0.95, 0.09, 0.6, 0.13],
            trigs_per_bar: (4, 16),
            span: 24,
            // Two steps, not DnB's four. A four-step anchor is most of the
            // reason DnB's bass cannot roll: it eats the first quarter of the
            // bar before the line has started. Two is long enough to land the
            // 1 and short enough to get out of the way — and `bass.rs` caps
            // it at the gap to the next trig regardless, so at full density
            // it is one step like everything else.
            anchor_len: Some(2.0),
            // `normal` is 1.6 against a `max` of 2.5, which reads two ways on
            // purpose: at eighths the gap is 2 steps and a note fills 1.6 of
            // them (driving, but articulated), and at sixteenths the gap is 1
            // and `bass.rs` clamps to it, so the notes butt up into a
            // continuous roll. One number, both behaviours, no branch.
            len: LenProfile::Plain { normal: 1.6, ghost: Some(0.75), max: 2.5 },
            // **A narrow spread, on purpose.** The off-sixteenths are all
            // below `rhythm::GHOST_WEIGHT`, so every one of them is labelled
            // a ghost — which is correct (a rolling bassline *is* accents
            // with ghosted sixteenths between them, the way it would be
            // played) but only if the ghost is a shade rather than a hole.
            // DnB's 66 against an accent of 120 is a whisper, and a whisper
            // every other step at density 100 is a roll with gaps in it. 88
            // against 118 keeps the accents on top and the roll continuous.
            velocity: Velocity { accent: 118, normal: 104, ghost: 88 },
            approach: Some(0.3),
            chord_tone: Some(0.15),
            // Low, and much lower than Electro's 0.45: a roller stays in its
            // register. The octave is punctuation here, not the melody.
            octave_leap: Some(0.15),
            strum: None,
            spread: None,
            motif: None,
            // **No `GHOST_PROB`**, and this is the only bass profile without
            // it. It puts a 60–85% PROB lock on every ghost, and every ghost
            // here is a sixteenth of the roll — so it would drop a quarter of
            // them at random, which is the exact opposite of driving.
            // `ALT_BARS` still gives the two bars different shapes.
            conditions: &[ALT_BARS, FILL_EXTRA],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Wander, from: 40, to: 105 },
                LaneRecipe { name: "fx.overdrive", shape: LaneShape::Accent, from: 25, to: 95 },
                LaneRecipe { name: "lfo1.depth", shape: LaneShape::Swell, from: 64, to: 100 },
            ],
        },
        (GenreId::Rollers, Role::Chords) => RoleProfile {
            // The sparsest chord part of any genre — `(1, 4)` against DnB's
            // `(1, 3)` only because the ceiling wants somewhere to go. Held
            // rather than stabbed: with sixteen bass trigs a bar underneath,
            // a stab is one more transient in a bar that has no room left,
            // and a sustain is the thing the roll is rolling *under*.
            weights: [1.0, 0.05, 0.3, 0.08, 0.2, 0.05, 0.45, 0.12, 0.6, 0.05, 0.7, 0.1, 0.25, 0.05, 0.4, 0.18],
            trigs_per_bar: (1, 4),
            span: 24,
            anchor_len: None,
            len: LenProfile::Mode { mode: LenMode::Sustain, normal: 6.0, max: 12.0 },
            velocity: Velocity { accent: 106, normal: 94, ghost: 74 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: Some(0.05),
            spread: Some(0.35),
            motif: None,
            conditions: &[ALT_BARS, EVERY_FOURTH, WEAK_PROB],
            lanes: &[
                LaneRecipe { name: "filter.cutoff", shape: LaneShape::Arc, from: 50, to: 100 },
                LaneRecipe { name: "fx.reverbSend", shape: LaneShape::Swell, from: 25, to: 90 },
            ],
        },
        (GenreId::Rollers, Role::Lead) => RoleProfile {
            // Catchy is fewer notes held longer, so this is the only lead
            // here with a ceiling under six: `(2, 5)`, a motif of three or
            // four notes, and a `window` of 7 semitones — a fifth, which is
            // about as far as a hook can roam and still be hummable. DnB and
            // Breaks both use 8.
            //
            // `WEAK_PROB` is deliberately absent, and it is the only melodic
            // role in the file without it. It drops weak trigs at random,
            // which is texture on a busy part and damage on a four-note hook:
            // the whole point is that the same four notes come back.
            weights: [0.9, 0.08, 0.5, 0.12, 0.7, 0.08, 0.55, 0.18, 0.85, 0.08, 0.5, 0.15, 0.65, 0.08, 0.5, 0.28],
            trigs_per_bar: (2, 5),
            span: 30,
            anchor_len: None,
            len: LenProfile::Plain { normal: 1.5, ghost: None, max: 4.0 },
            velocity: Velocity { accent: 114, normal: 98, ghost: 72 },
            approach: None,
            chord_tone: None,
            octave_leap: None,
            strum: None,
            spread: None,
            motif: Some(MotifProfile { notes: (3, 4), window: 7 }),
            conditions: &[ALT_BARS, EVERY_FOURTH, ANSWERING],
            lanes: &[
                LaneRecipe { name: "amp.pan", shape: LaneShape::Wander, from: 40, to: 88 },
                LaneRecipe { name: "fx.delaySend", shape: LaneShape::Swell, from: 20, to: 85 },
            ],
        },

        // The kit is Breaks', which is the point — the funk lives in the
        // drums and the drive lives in the bass. What is changed from Breaks
        // is the hats: at 168 with sixteen bass trigs under them, a
        // sixteenth-note closed hat is mud, so the table leans on the eighths
        // and reaches its sixteenths only near the top of the slider.
        (GenreId::Rollers, Role::Kick) => drum_profile(
            // Funk-breakbeat syncopation, holding the 1 firmly. The "and of
            // 3" and the last "and" are the two syncopations a break leans
            // on; everything else is left for the snare and the bass.
            [1.0, 0.0, 0.25, 0.0, 0.0, 0.0, 0.45, 0.0, 0.0, 0.35, 0.0, 0.3, 0.0, 0.0, 0.5, 0.0],
            (2, 5),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 118, normal: 100, ghost: 66 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Rollers, Role::Snare) => drum_profile(
            // Breaks' backbeat and Breaks' ghosts, unchanged: the pickups at
            // 2, 7, 9 and 15 are the funk, and they are the reason this is
            // not simply DnB's two-step.
            [0.0, 0.0, 0.2, 0.0, 1.0, 0.0, 0.0, 0.3, 0.0, 0.25, 0.0, 0.0, 1.0, 0.0, 0.0, 0.4],
            (2, 5),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 114, normal: 98, ghost: 60 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Rollers, Role::Clap) => drum_profile(
            // Layering the backbeat rather than arguing with it, with the
            // snare's syncopations kept but thinned — two voices playing the
            // same ghost is a flam, not a groove.
            [0.0, 0.0, 0.2, 0.0, 0.9, 0.0, 0.0, 0.25, 0.0, 0.2, 0.0, 0.0, 0.9, 0.0, 0.0, 0.3],
            (1, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 108, normal: 92, ghost: 58 },
            DRUM_SPINE_RECIPE,
        ),
        (GenreId::Rollers, Role::Rimshot) => drum_profile(
            // Off-grid only, and the busiest of the three break-flavoured
            // kits: this is the voice that fills the space between the kick
            // and the snare, which at 168 is most of the bar.
            [0.0, 0.28, 0.0, 0.45, 0.0, 0.3, 0.0, 0.5, 0.0, 0.3, 0.0, 0.45, 0.0, 0.28, 0.0, 0.55],
            (2, 5),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 102, normal: 82, ghost: 54 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Rollers, Role::ClosedHat) => drum_profile(
            // Eighths first, sixteenths last — Breaks stocks its off-slots at
            // 0.5 and gets a sixteenth hat at any density, which is right at
            // 135 with a sparse bass and wrong here. A tenth, and a floor of
            // 6, means the hat is an eighth-note pulse until the slider is
            // pushed and the roll is already carrying the bar.
            [0.8, 0.08, 0.6, 0.1, 0.7, 0.08, 0.6, 0.1, 0.8, 0.08, 0.6, 0.1, 0.7, 0.08, 0.6, 0.12],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 92, normal: 78, ghost: 52 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Rollers, Role::OpenHat) => drum_profile(
            // The "and"s and nowhere else — see `drum_profile`'s note on why
            // the zeroes here are deliberate rather than a texture bug.
            [0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.6, 0.0],
            (2, 4),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 98, normal: 82, ghost: 58 },
            DRUM_COLOUR_RECIPE,
        ),
        (GenreId::Rollers, Role::Ride) => drum_profile(
            // Eighths, which `ROLLERS_GROOVE` leaves straight — the ride is
            // the one voice here that should not shuffle, because it is what
            // the shuffled sixteenths are heard against.
            [0.7, 0.07, 0.55, 0.07, 0.6, 0.07, 0.55, 0.07, 0.7, 0.07, 0.55, 0.07, 0.6, 0.07, 0.55, 0.07],
            (5, 16),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 86, normal: 72, ghost: 50 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Rollers, Role::Shaker) => drum_profile(
            // The "a" outweighs the "e", the same as Breaks' and DnB's, and
            // `ROLLERS_GROOVE` then pushes the "a"s later still — the weights
            // and the groove table saying the same thing twice, which is what
            // makes the shuffle audible rather than theoretical.
            [0.65, 0.05, 0.5, 0.09, 0.6, 0.05, 0.5, 0.09, 0.65, 0.05, 0.5, 0.09, 0.6, 0.05, 0.5, 0.09],
            (6, 16),
            LenProfile::Plain { normal: 0.25, ghost: Some(0.125), max: 0.5 },
            Velocity { accent: 80, normal: 68, ghost: 48 },
            DRUM_TEXTURE_RECIPE,
        ),
        (GenreId::Rollers, Role::Tom) => drum_profile(
            // Fill material, weighted to the back half so it runs into the
            // next 1 rather than competing with the bass for the front of the
            // bar.
            [0.0, 0.0, 0.0, 0.2, 0.0, 0.0, 0.0, 0.3, 0.0, 0.0, 0.4, 0.0, 0.0, 0.35, 0.0, 0.5],
            (1, 3),
            LenProfile::Plain { normal: 0.5, ghost: Some(0.25), max: 1.0 },
            Velocity { accent: 112, normal: 92, ghost: 60 },
            DRUM_FILL_RECIPE,
        ),
    }
}

pub fn genre_label(id: GenreId) -> &'static str {
    genre_profile(id).label
}

/// The height of a role's register window; `theory::window_for` turns it and
/// the part's octave into the window itself.
pub fn role_span(id: GenreId, role: Role) -> i32 {
    role_profile(id, role).span
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_genre_has_every_role() {
        for id in GenreId::ALL {
            for role in Role::ALL {
                assert_eq!(role_profile(id, role).weights.len(), 16);
            }
        }
    }

    #[test]
    fn genre_ids_round_trip_through_their_string() {
        for id in GenreId::ALL {
            assert_eq!(GenreId::parse(id.as_str()), Some(id));
        }
        assert_eq!(GenreId::parse("jungle"), None);
    }

    #[test]
    fn every_bpm_sits_inside_its_own_range() {
        for id in GenreId::ALL {
            let p = genre_profile(id);
            assert!(p.bpm >= p.bpm_range.0 && p.bpm <= p.bpm_range.1);
        }
    }
}
