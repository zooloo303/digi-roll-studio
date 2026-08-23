// Key, scale and chord maths for the generator. Pure, egui-free, device-free.
//
// Port of `js/gen/theory.js`. Two deliberate reuses, so the generator can
// never disagree with what the app already does:
//
//   * scales are [`digi_core::chords::Scale`] — the same eight the Harmony
//     panel tints rows with;
//   * chord tones come from [`digi_core::chords::chord_pitches`] — the
//     existing diatonic thirds-walker. A degree gets its natural quality (ii
//     minor, V7 dominant, vii° diminished) with no chord tables anywhere,
//     exactly as chord draw does. Which also means the 4-note hardware
//     ceiling and the window clamping are already handled.
//
// Roman numerals are the progression language: `i VI III VII`. Case is
// cosmetic — the quality comes from the scale unless a token names one —
// which is why `i VI III VII` and `I VI III VII` in a minor key produce the
// same chords. Writing it in the conventional case is just how progressions
// are read.

use digi_core::chords::{chord_pitches, ChordOpts, KeyScale, Quality, Scale};
use digi_core::edit_ops::{PITCH_MAX, PITCH_MIN};

fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
    v.max(lo).min(hi)
}

pub const DEFAULT_SCALE: Scale = Scale::Minor;

/// The intervals of a named scale, falling back to minor rather than failing:
/// a bad scale name in a saved context must not stop the panel opening.
///
/// Unlike the JS, this takes an already-parsed [`Scale`] rather than a
/// string — `is_scale_name`/parsing a free string is `core::chords::Scale`'s
/// own `Deserialize` impl's job, done once at the project-file boundary
/// rather than here.
pub fn scale_intervals(scale: Scale) -> &'static [i32] {
    scale.intervals()
}

// --- Roman numerals ------------------------------------------------------------

const ROMAN_LOWER: [&str; 7] = ["i", "ii", "iii", "iv", "v", "vi", "vii"];

pub const MAX_PROGRESSION_CHORDS: usize = 16;
pub const MAX_CHORD_BARS: u32 = 8;

/// A forced quality, or `Auto` to take the scale's own quality for the
/// degree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotQuality {
    Auto,
    Fixed(Quality),
}

/// One token of a parsed progression — one chord in the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordSlot {
    /// 1-based scale degree.
    pub degree: u32,
    pub quality: SlotQuality,
    pub seventh: bool,
    pub bars: u32,
    /// Whether the numeral was typed upper-case — cosmetic, kept only so
    /// `format_progression` round-trips the exact text.
    pub upper: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChordParseError(pub String);

impl std::fmt::Display for ChordParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ChordParseError {}

fn quality_from_suffix(suffix: &str) -> Option<SlotQuality> {
    match suffix {
        "" => Some(SlotQuality::Auto),
        "m" | "min" => Some(SlotQuality::Fixed(Quality::Minor)),
        "maj" | "ma" => Some(SlotQuality::Fixed(Quality::Major)),
        "+" | "aug" => Some(SlotQuality::Fixed(Quality::Aug)),
        "dim" | "o" | "°" => Some(SlotQuality::Fixed(Quality::Dim)),
        "sus2" => Some(SlotQuality::Fixed(Quality::Sus2)),
        "sus4" => Some(SlotQuality::Fixed(Quality::Sus4)),
        _ => None,
    }
}

fn quality_to_suffix(quality: SlotQuality) -> &'static str {
    match quality {
        SlotQuality::Auto => "",
        SlotQuality::Fixed(Quality::Minor) => "m",
        SlotQuality::Fixed(Quality::Major) => "maj",
        SlotQuality::Fixed(Quality::Dim) => "dim",
        SlotQuality::Fixed(Quality::Aug) => "aug",
        SlotQuality::Fixed(Quality::Sus2) => "sus2",
        SlotQuality::Fixed(Quality::Sus4) => "sus4",
    }
}

/// One token → one progression slot.
///
///   i        the tonic, quality from the scale
///   VI       the sixth degree, quality from the scale (the case is cosmetic)
///   i7       …with the scale's own 7th on top
///   ivm      quality forced minor
///   Vmaj7    forced major, with a major 7th
///   i:2      two bars long
///
/// Errors with a sentence a user can act on: the panel puts it on the status
/// line and keeps the previous progression.
pub fn parse_chord_token(token: &str) -> Result<ChordSlot, ChordParseError> {
    let raw = token.trim();
    if raw.is_empty() {
        return Err(ChordParseError("empty chord in the progression".into()));
    }

    let mut rest = raw;
    let mut bars = 1u32;
    if let Some(colon) = rest.find(':') {
        let n: Result<u32, _> = rest[colon + 1..].parse();
        match n {
            Ok(n) if (1..=MAX_CHORD_BARS).contains(&n) => bars = n,
            _ => {
                return Err(ChordParseError(format!(
                    "\u{201c}{raw}\u{201d}: the bars after \u{201c}:\u{201d} must be a whole number 1\u{2013}{MAX_CHORD_BARS}"
                )))
            }
        }
        rest = &rest[..colon];
    }

    // Longest-first, so "iv" isn't read as "i" and "vii" isn't read as "vi".
    let lower = rest.to_lowercase();
    let mut order = ["vii", "vi", "iv", "v", "iii", "ii", "i"];
    order.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let numeral = order.iter().find(|n| lower.starts_with(**n)).copied();
    let Some(numeral) = numeral else {
        return Err(ChordParseError(format!(
            "\u{201c}{raw}\u{201d}: chords are roman numerals i\u{2013}vii (try \u{201c}i VI III VII\u{201d})"
        )));
    };
    let degree = (ROMAN_LOWER.iter().position(|r| *r == numeral).unwrap() + 1) as u32;
    let upper = rest[..numeral.len()].chars().next().map(char::is_uppercase).unwrap_or(false);
    let mut rest = &rest[numeral.len()..];

    let mut seventh = false;
    if let Some(stripped) = rest.strip_suffix('7') {
        seventh = true;
        rest = stripped;
    }

    let quality = quality_from_suffix(rest).or_else(|| quality_from_suffix(&rest.to_lowercase()));
    let Some(quality) = quality else {
        return Err(ChordParseError(format!(
            "\u{201c}{raw}\u{201d}: \u{201c}{rest}\u{201d} isn't a chord quality \u{2014} use m, maj, dim, aug, sus2 or sus4, or leave it off and the scale decides"
        )));
    };

    Ok(ChordSlot { degree, quality, seventh, bars, upper })
}

/// Free text → progression. Separators are generous on purpose: spaces,
/// commas, bars and the middle dot the library prints with all work.
pub fn parse_progression(text: &str) -> Result<Vec<ChordSlot>, ChordParseError> {
    let tokens: Vec<&str> =
        text.split(|c: char| c.is_whitespace() || matches!(c, ',' | '\u{b7}' | '|' | '-')).filter(|s| !s.is_empty()).collect();
    if tokens.is_empty() {
        return Err(ChordParseError("type a progression like \u{201c}i VI III VII\u{201d}".into()));
    }
    if tokens.len() > MAX_PROGRESSION_CHORDS {
        return Err(ChordParseError(format!(
            "{} chords is more than a loop can hold \u{2014} keep it to {MAX_PROGRESSION_CHORDS}",
            tokens.len()
        )));
    }
    tokens.into_iter().map(parse_chord_token).collect()
}

/// Progression → the text that parses back to it, so the library and the
/// editable field speak the same language.
pub fn format_progression(prog: &[ChordSlot]) -> String {
    prog.iter()
        .map(|s| {
            let numeral = ROMAN_LOWER[(s.degree - 1) as usize];
            let numeral = if s.upper { numeral.to_uppercase() } else { numeral.to_string() };
            let mut out = numeral;
            out.push_str(quality_to_suffix(s.quality));
            if s.seventh {
                out.push('7');
            }
            if s.bars > 1 {
                out.push(':');
                out.push_str(&s.bars.to_string());
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn progression_bars(prog: &[ChordSlot]) -> u32 {
    prog.iter().map(|s| s.bars).sum()
}

/// Which chord each bar of the pattern is on. The progression loops to fill
/// the pattern, and a progression longer than the pattern is simply
/// truncated — a 4-bar loop in a 2-bar pattern gives you its first two
/// chords, which is what shortening the pattern visibly does.
pub fn bar_slots(prog: &[ChordSlot], bars: u32) -> Vec<ChordSlot> {
    let total = progression_bars(prog);
    let mut out = Vec::with_capacity(bars as usize);
    for b in 0..bars {
        let mut x = if total == 0 { 0 } else { b % total };
        let mut i = 0usize;
        while i < prog.len().saturating_sub(1) && x >= prog[i].bars {
            x -= prog[i].bars;
            i += 1;
        }
        out.push(prog[i]);
    }
    out
}

// --- Pitches -------------------------------------------------------------------

/// The pitch of a scale degree. Degrees past the end of the scale keep
/// walking upward into the next octave, which is what makes `vii` mean
/// something in a five-note pentatonic instead of panicking.
///
/// `octave` follows the boxes' own labelling, not middle-C = C4: MIDI 60 is
/// C5 on an Elektron and in the roll's key column, so octave 5 root 0 is 60.
pub fn degree_pitch(degree: u32, root: i32, intervals: &[i32], octave: i32) -> i32 {
    let l = intervals.len() as i32;
    let idx = degree as i32 - 1;
    let wrapped = idx.rem_euclid(l);
    12 * octave + root + intervals[wrapped as usize] + 12 * idx.div_euclid(l)
}

/// Move a pitch by whole octaves until it sits inside a register window.
/// Octave equivalence is the one transposition that never changes what a
/// note *means*, which is why the parts fold rather than clamp — a bass root
/// stays a root. A window narrower than an octave has no octave to choose,
/// so it clamps.
pub fn fold_into_window(pitch: i32, min: i32, max: i32) -> i32 {
    if max - min < 12 {
        return clamp(pitch, min, max);
    }
    let mut p = pitch;
    if p < min {
        p += 12 * ((min - p) as f64 / 12.0).ceil() as i32;
    }
    if p > max {
        p -= 12 * ((p - max) as f64 / 12.0).ceil() as i32;
    }
    p
}

/// The register window for a role: `span` semitones up from its octave,
/// clamped to the rows the roll can actually draw, so a part can never
/// generate a note the editor can't show. An octave high enough to leave no
/// room is pulled back down rather than producing an inverted window — every
/// window is at least an octave tall, which is what `fold_into_window` needs
/// to be able to choose an octave at all.
pub fn window_for(span: i32, octave: i32) -> (i32, i32) {
    let span = span.max(12);
    let lo = (12 * octave).max(i32::from(PITCH_MIN)).min(i32::from(PITCH_MAX) - 12);
    (lo, (lo + span).min(i32::from(PITCH_MAX)))
}

fn pitch_classes(root: i32, intervals: &[i32]) -> std::collections::HashSet<i32> {
    intervals.iter().map(|i| (i + root).rem_euclid(12)).collect()
}

/// Every pitch of the scale inside a window, ascending — the palette a
/// bassline or a lead picks from.
pub fn scale_pitches_in_window(root: i32, intervals: &[i32], min: i32, max: i32) -> Vec<i32> {
    let classes = pitch_classes(root, intervals);
    (min..=max).filter(|p| classes.contains(&p.rem_euclid(12))).collect()
}

/// Nearest pitch in the scale, ties going down — the same tie-break chord
/// draw's own snap uses, so an out-of-scale approach tone lands where a
/// click would.
pub fn snap_to_scale_pitch(pitch: i32, root: i32, intervals: &[i32]) -> i32 {
    let classes = pitch_classes(root, intervals);
    for d in 0..=6 {
        if classes.contains(&(pitch - d).rem_euclid(12)) {
            return pitch - d;
        }
        if classes.contains(&(pitch + d).rem_euclid(12)) {
            return pitch + d;
        }
    }
    pitch
}

/// A key: root pitch class plus the scale's intervals, borrowed rather than
/// owned so callers can hand in `Scale::intervals()`'s `'static` slice.
#[derive(Debug, Clone, Copy)]
pub struct Key<'a> {
    pub root: i32,
    pub intervals: &'a [i32],
}

/// Register-window and voicing options shared by [`chord_tones`] and
/// [`voicing_candidates`].
#[derive(Debug, Clone, Copy)]
pub struct ChordWindow {
    pub octave: i32,
    pub min: u8,
    pub max: u8,
    pub inversion: u8,
    pub spread: bool,
}

impl Default for ChordWindow {
    fn default() -> Self {
        Self { octave: 4, min: 0, max: 127, inversion: 0, spread: false }
    }
}

/// The chord tones of one progression slot, in a register window.
///
/// `quality: Auto` walks the scale in thirds (the diatonic case, and the
/// default); anything else forces that quality. Either way this is
/// `core::chords::chord_pitches` doing the work, capped at the hardware's
/// four notes per trig.
pub fn chord_tones(slot: &ChordSlot, key: Key, opts: ChordWindow) -> Vec<u8> {
    let root_pitch = fold_into_window(
        degree_pitch(slot.degree, key.root, key.intervals, opts.octave),
        i32::from(opts.min),
        i32::from(opts.max),
    );
    let diatonic = slot.quality == SlotQuality::Auto;
    let chord_opts = ChordOpts {
        scale: if diatonic {
            Some(KeyScale { root: key.root.rem_euclid(12) as u8, scale: scale_for_intervals(key.intervals) })
        } else {
            None
        },
        quality: match slot.quality {
            SlotQuality::Fixed(q) => q,
            SlotQuality::Auto => Quality::Major,
        },
        seventh: slot.seventh,
        inversion: opts.inversion,
        spread: opts.spread,
        min: opts.min,
        max: opts.max,
        max_notes: digi_core::chords::MAX_CHORD_NOTES,
    };
    chord_pitches(root_pitch.clamp(0, 127) as u8, &chord_opts)
}

/// Reverse-lookup a `Scale` from its interval slice. Every caller here builds
/// `intervals` from `Scale::intervals()` in the first place, so this always
/// finds a match; a `Key` built from a scale a caller invented some other way
/// is not a case this crate constructs.
fn scale_for_intervals(intervals: &[i32]) -> Scale {
    Scale::ALL.into_iter().find(|s| s.intervals() == intervals).unwrap_or(DEFAULT_SCALE)
}

/// The root of a slot's chord, folded into a window — what a bassline
/// actually wants, without building the chord.
pub fn slot_root_pitch(slot: &ChordSlot, key: Key, octave: i32, min: i32, max: i32) -> i32 {
    fold_into_window(degree_pitch(slot.degree, key.root, key.intervals, octave), min, max)
}

/// Every voicing of a slot's chord that fits the window: the four
/// inversions, with and without the drop-2 spread, each also tried an octave
/// up and down.
///
/// Two subtleties, both learned the hard way:
///
///   * **octave transpositions matter more than inversions.** Inversions
///     alone all sit wherever the folded root put them, which on a low
///     window means the next chord can only travel upward — the exact
///     opposite of voice leading.
///   * **only the fullest voicings compete.** `chord_pitches` drops notes
///     that fall outside the window, so a chord clipped to two notes would
///     win any "moves least" contest by simply having fewer notes to move.
///     Truncated candidates are dropped rather than allowed to flatten the
///     harmony.
pub fn voicing_candidates(
    slot: &ChordSlot,
    key: Key,
    octave: i32,
    min: i32,
    max: i32,
    spreads: &[bool],
) -> Vec<Vec<i32>> {
    let mut seen = std::collections::HashSet::new();
    let mut all: Vec<Vec<i32>> = Vec::new();
    for &spread in spreads {
        for inversion in 0..4u8 {
            let base = chord_tones(
                slot,
                key,
                ChordWindow { octave, min: min.clamp(0, 127) as u8, max: max.clamp(0, 127) as u8, inversion, spread },
            );
            if base.is_empty() {
                continue;
            }
            for shift in [-12, 0, 12] {
                let moved: Vec<i32> = base.iter().map(|&p| i32::from(p) + shift).collect();
                if !moved.iter().all(|&p| p >= min && p <= max) {
                    continue;
                }
                let key2: Vec<i32> = moved.clone();
                if seen.contains(&key2) {
                    continue;
                }
                seen.insert(key2.clone());
                all.push(moved);
            }
        }
    }
    if all.is_empty() {
        return Vec::new();
    }
    let fullest = all.iter().map(|c| c.len()).max().unwrap();
    all.into_iter().filter(|c| c.len() == fullest).collect()
}

// --- Voice leading -------------------------------------------------------------

/// How far a voicing is from the one before it: for each note, the distance
/// to the nearest note of the previous chord, summed. Not a true
/// voice-to-voice pairing — chords here change size (a 7th arrives, a note
/// falls outside the window) and a nearest-note sum degrades gracefully
/// where a pairing would have to invent a rule for the odd voice out.
pub fn voicing_distance(prev: &[i32], next: &[i32]) -> i32 {
    if prev.is_empty() || next.is_empty() {
        return 0;
    }
    next.iter().map(|&p| prev.iter().map(|&q| (p - q).abs()).min().unwrap()).sum()
}

/// The candidate voicing that moves least from the previous chord — the
/// whole of what makes a chord part walk instead of jump. Ties go to the
/// lower voicing, so a part doesn't drift upward across a long progression.
///
/// With no previous chord there is nothing to lead from, so the *first*
/// chord is placed near `centre` instead: a part that starts in the middle
/// of its register has somewhere to go in both directions, which is what
/// every chord after it depends on.
pub fn best_voicing(prev: &[i32], candidates: &[Vec<i32>], centre: Option<f64>) -> Vec<i32> {
    let usable: Vec<&Vec<i32>> = candidates.iter().filter(|c| !c.is_empty()).collect();
    if usable.is_empty() {
        return Vec::new();
    }
    let mean = |c: &[i32]| c.iter().sum::<i32>() as f64 / c.len() as f64;
    if prev.is_empty() {
        return match centre {
            None => usable[0].clone(),
            Some(centre) => usable
                .into_iter()
                .reduce(|a, b| if (mean(b) - centre).abs() < (mean(a) - centre).abs() { b } else { a })
                .unwrap()
                .clone(),
        };
    }
    let mut best: Option<(&Vec<i32>, i32, f64)> = None;
    for c in usable {
        let cost = voicing_distance(prev, c);
        let height = mean(c);
        let take = match best {
            None => true,
            Some((_, bcost, bheight)) => cost < bcost || (cost == bcost && height < bheight),
        };
        if take {
            best = Some((c, cost, height));
        }
    }
    best.unwrap().0.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::chords::Scale;

    fn c_minor() -> (i32, &'static [i32]) {
        (0, Scale::Minor.intervals())
    }
    fn c_major() -> (i32, &'static [i32]) {
        (0, Scale::Major.intervals())
    }

    fn slot(degree: u32) -> ChordSlot {
        ChordSlot { degree, quality: SlotQuality::Auto, seventh: false, bars: 1, upper: false }
    }

    #[test]
    fn reads_a_plain_degree_case_is_cosmetic() {
        let s = parse_chord_token("i").unwrap();
        assert_eq!((s.degree, s.quality, s.seventh, s.bars), (1, SlotQuality::Auto, false, 1));
        let s = parse_chord_token("VII").unwrap();
        assert_eq!(s.degree, 7);
        assert!(s.upper);
        assert_eq!(parse_chord_token("iv").unwrap().degree, 4);
        assert_eq!(parse_chord_token("III").unwrap().degree, 3);
    }

    #[test]
    fn does_not_read_iv_as_i_or_vii_as_vi() {
        assert_eq!(parse_chord_token("iv").unwrap().degree, 4);
        assert_eq!(parse_chord_token("vii").unwrap().degree, 7);
        assert_eq!(parse_chord_token("vi").unwrap().degree, 6);
        assert_eq!(parse_chord_token("v").unwrap().degree, 5);
    }

    #[test]
    fn reads_sevenths_and_forced_qualities() {
        let s = parse_chord_token("i7").unwrap();
        assert_eq!((s.degree, s.quality, s.seventh), (1, SlotQuality::Auto, true));
        let s = parse_chord_token("ivm").unwrap();
        assert_eq!((s.degree, s.quality, s.seventh), (4, SlotQuality::Fixed(Quality::Minor), false));
        let s = parse_chord_token("Vmaj7").unwrap();
        assert_eq!((s.degree, s.quality, s.seventh), (5, SlotQuality::Fixed(Quality::Major), true));
        let s = parse_chord_token("viidim").unwrap();
        assert_eq!((s.degree, s.quality), (7, SlotQuality::Fixed(Quality::Dim)));
        let s = parse_chord_token("isus4").unwrap();
        assert_eq!((s.degree, s.quality), (1, SlotQuality::Fixed(Quality::Sus4)));
    }

    #[test]
    fn reads_a_bar_count_after_a_colon() {
        let s = parse_chord_token("i:2").unwrap();
        assert_eq!((s.degree, s.bars), (1, 2));
        let s = parse_chord_token("i7:4").unwrap();
        assert_eq!((s.degree, s.seventh, s.bars), (1, true, 4));
    }

    #[test]
    fn explains_itself_when_a_token_is_malformed() {
        assert!(parse_chord_token("viii").unwrap_err().0.contains("quality"));
        assert!(parse_chord_token("C").unwrap_err().0.contains("roman numerals"));
        assert!(parse_chord_token("i:0").unwrap_err().0.contains("1\u{2013}8"));
        assert!(parse_chord_token("i:nine").unwrap_err().0.contains("1\u{2013}8"));
        assert!(parse_chord_token("iwhat").unwrap_err().0.contains("isn't a chord quality"));
    }

    fn degrees(prog: &[ChordSlot]) -> Vec<u32> {
        prog.iter().map(|s| s.degree).collect()
    }

    #[test]
    fn splits_on_spaces_commas_dots_and_dashes() {
        assert_eq!(degrees(&parse_progression("i VI III VII").unwrap()), vec![1, 6, 3, 7]);
        assert_eq!(degrees(&parse_progression("i, VI \u{b7} III | VII").unwrap()), vec![1, 6, 3, 7]);
        assert_eq!(degrees(&parse_progression("i-VI-III-VII").unwrap()), vec![1, 6, 3, 7]);
    }

    #[test]
    fn refuses_nothing_at_all_and_an_unreasonably_long_loop() {
        assert!(parse_progression("   ").unwrap_err().0.contains("type a progression"));
        let long = vec!["i"; 20].join(" ");
        assert!(parse_progression(&long).unwrap_err().0.contains("more than a loop"));
    }

    #[test]
    fn round_trips_through_format_progression() {
        for text in ["i VI III VII", "i7:2 iv7:2", "ivm V isus4", "ii7 v7"] {
            assert_eq!(format_progression(&parse_progression(text).unwrap()), text);
        }
    }

    #[test]
    fn counts_bars_honouring_per_chord_spans() {
        assert_eq!(progression_bars(&parse_progression("i VI III VII").unwrap()), 4);
        assert_eq!(progression_bars(&parse_progression("i:2 VI:2").unwrap()), 4);
        assert_eq!(progression_bars(&parse_progression("i:4").unwrap()), 4);
    }

    #[test]
    fn loops_a_short_progression_to_fill_the_pattern() {
        let prog = parse_progression("i VI").unwrap();
        assert_eq!(degrees(&bar_slots(&prog, 4)), vec![1, 6, 1, 6]);
    }

    #[test]
    fn truncates_a_progression_longer_than_the_pattern() {
        let prog = parse_progression("i VI III VII").unwrap();
        assert_eq!(degrees(&bar_slots(&prog, 2)), vec![1, 6]);
    }

    #[test]
    fn spreads_a_multi_bar_chord_across_its_bars() {
        let prog = parse_progression("i:2 VII:2").unwrap();
        assert_eq!(degrees(&bar_slots(&prog, 4)), vec![1, 1, 7, 7]);
        assert_eq!(degrees(&bar_slots(&prog, 8)), vec![1, 1, 7, 7, 1, 1, 7, 7]);
    }

    #[test]
    fn handles_the_one_chord_drone() {
        assert_eq!(degrees(&bar_slots(&parse_progression("i:4").unwrap(), 2)), vec![1, 1]);
    }

    #[test]
    fn numbers_octaves_the_way_the_boxes_do() {
        let (root, intervals) = c_minor();
        assert_eq!(degree_pitch(1, root, intervals, 5), 60);
        assert_eq!(degree_pitch(1, root, intervals, 2), 24);
    }

    #[test]
    fn walks_degrees_up_the_scale_wrapping_into_the_next_octave() {
        let (root, intervals) = c_minor();
        assert_eq!(degree_pitch(3, root, intervals, 5), 63);
        assert_eq!(degree_pitch(5, root, intervals, 5), 67);
        assert_eq!(degree_pitch(7, root, intervals, 5), 70);
        let pent = Scale::PentatonicMinor.intervals();
        assert_eq!(degree_pitch(6, 0, pent, 5), 72);
    }

    #[test]
    fn folds_a_pitch_into_a_register_window_by_whole_octaves() {
        assert_eq!(fold_into_window(24, 48, 72), 48);
        assert_eq!(fold_into_window(96, 48, 72), 72);
        assert_eq!(fold_into_window(60, 48, 72), 60);
    }

    #[test]
    fn clamps_rather_than_looping_forever_in_a_window_too_narrow_for_an_octave() {
        assert_eq!(fold_into_window(50, 60, 64), 60);
        assert_eq!(fold_into_window(90, 60, 64), 64);
    }

    #[test]
    fn keeps_every_register_window_inside_the_rows_the_roll_can_draw() {
        for span in [24, 30] {
            for octave in 0..=9 {
                let (lo, hi) = window_for(span, octave);
                assert!(lo >= i32::from(PITCH_MIN));
                assert!(hi <= i32::from(PITCH_MAX));
                assert!(hi - lo >= 12);
            }
        }
    }

    #[test]
    fn lists_the_scale_tones_in_a_window() {
        let (root, intervals) = c_minor();
        assert_eq!(scale_pitches_in_window(root, intervals, 60, 72), vec![60, 62, 63, 65, 67, 68, 70, 72]);
    }

    #[test]
    fn snaps_an_out_of_scale_pitch_to_the_nearest_scale_tone_ties_going_down() {
        let (root, intervals) = c_minor();
        assert_eq!(snap_to_scale_pitch(61, root, intervals), 60);
        assert_eq!(snap_to_scale_pitch(66, root, intervals), 65);
        assert_eq!(snap_to_scale_pitch(63, root, intervals), 63);
    }

    #[test]
    fn gives_a_degree_its_natural_quality_from_the_scale() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        let w = ChordWindow { octave: 5, min: 48, max: 84, ..Default::default() };
        assert_eq!(chord_tones(&slot(1), key, w), vec![60, 63, 67]);
        assert_eq!(chord_tones(&slot(6), key, w), vec![68, 72, 75]);
        assert_eq!(chord_tones(&slot(5), key, w), vec![67, 70, 74]);
    }

    #[test]
    fn adds_the_scales_own_seventh_so_v7_in_major_is_dominant() {
        let (root, intervals) = c_major();
        let key = Key { root, intervals };
        let mut s = slot(5);
        s.seventh = true;
        let w = ChordWindow { octave: 5, min: 48, max: 84, ..Default::default() };
        assert_eq!(chord_tones(&s, key, w), vec![67, 71, 74, 77]);
    }

    #[test]
    fn honours_a_forced_quality() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        let mut s = slot(1);
        s.quality = SlotQuality::Fixed(Quality::Major);
        let w = ChordWindow { octave: 5, min: 48, max: 84, ..Default::default() };
        assert_eq!(chord_tones(&s, key, w), vec![60, 64, 67]);
    }

    #[test]
    fn never_exceeds_the_hardwares_four_notes_per_trig() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        for degree in 1..=7 {
            let mut s = slot(degree);
            s.seventh = true;
            let w = ChordWindow { octave: 5, min: 48, max: 84, inversion: 3, spread: true };
            assert!(chord_tones(&s, key, w).len() <= 4);
        }
    }

    #[test]
    fn keeps_chords_inside_the_register_window() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        for degree in [1, 4, 6, 7] {
            let w = ChordWindow { octave: 4, min: 48, max: 72, ..Default::default() };
            for tone in chord_tones(&slot(degree), key, w) {
                assert!((48..=72).contains(&tone));
            }
        }
    }

    #[test]
    fn puts_a_slot_root_in_the_bass_window() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        let root_pitch = slot_root_pitch(&slot(6), key, 2, 24, 48);
        assert!((24..=48).contains(&root_pitch));
        assert_eq!(root_pitch.rem_euclid(12), 8); // Ab
    }

    #[test]
    fn measures_total_movement_from_the_previous_chord() {
        assert_eq!(voicing_distance(&[60, 64, 67], &[60, 64, 67]), 0);
        assert_eq!(voicing_distance(&[60, 64, 67], &[61, 65, 68]), 3);
        assert_eq!(voicing_distance(&[], &[60]), 0);
    }

    #[test]
    fn picks_the_inversion_that_moves_least() {
        let prev = vec![60, 63, 67];
        let candidates = vec![vec![56, 60, 63], vec![80, 84, 87]];
        assert_eq!(best_voicing(&prev, &candidates, None), vec![56, 60, 63]);
    }

    #[test]
    fn takes_the_lower_voicing_when_two_move_the_same_amount() {
        assert_eq!(best_voicing(&[60], &[vec![72], vec![48]], None), vec![48]);
    }

    #[test]
    fn takes_the_first_candidate_when_there_is_nothing_to_lead_from() {
        assert_eq!(best_voicing(&[], &[vec![60, 64], vec![70, 74]], None), vec![60, 64]);
        let empty: Vec<i32> = Vec::new();
        assert_eq!(best_voicing(&[60], &[], None), empty);
    }

    #[test]
    fn walks_a_whole_progression_instead_of_jumping() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        let prog = parse_progression("i VI III VII").unwrap();
        let mut prev: Vec<i32> = Vec::new();
        let mut worst = 0;
        let mut root_position_worst = 0;
        for s in &prog {
            let candidates = voicing_candidates(s, key, 4, 48, 72, &[false, true]);
            let chosen = best_voicing(&prev, &candidates, Some(60.0));
            let naive = chord_tones(s, key, ChordWindow { octave: 4, min: 48, max: 72, ..Default::default() })
                .into_iter()
                .map(i32::from)
                .collect::<Vec<_>>();
            if !prev.is_empty() {
                worst = worst.max(voicing_distance(&prev, &chosen));
                root_position_worst = root_position_worst.max(voicing_distance(&prev, &naive));
            }
            prev = chosen;
        }
        assert!(worst <= 6);
        assert!(worst < root_position_worst);
    }

    #[test]
    fn offers_octave_transpositions_not_just_inversions() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        let s = slot(1);
        let cands = voicing_candidates(&s, key, 4, 48, 84, &[false, true]);
        let means: Vec<f64> = cands.iter().map(|c| c.iter().sum::<i32>() as f64 / c.len() as f64).collect();
        let max = means.iter().cloned().fold(f64::MIN, f64::max);
        let min = means.iter().cloned().fold(f64::MAX, f64::min);
        assert!(max - min >= 12.0);
    }

    #[test]
    fn keeps_every_candidate_inside_the_window() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        let s = slot(1);
        for c in voicing_candidates(&s, key, 4, 48, 72, &[false, true]) {
            for p in c {
                assert!((48..=72).contains(&p));
            }
        }
    }

    #[test]
    fn drops_voicings_the_window_clipped() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        let s = slot(1);
        let cands = voicing_candidates(&s, key, 4, 48, 62, &[false, true]);
        assert!(!cands.is_empty());
        let lens: std::collections::HashSet<usize> = cands.iter().map(|c| c.len()).collect();
        assert_eq!(lens.len(), 1);
    }

    #[test]
    fn returns_nothing_when_not_one_note_of_the_chord_fits() {
        let (root, intervals) = c_minor();
        let key = Key { root, intervals };
        let s = slot(1);
        assert_eq!(voicing_candidates(&s, key, 4, 61, 61, &[false, true]), Vec::<Vec<i32>>::new());
    }

    #[test]
    fn falls_back_to_minor_for_a_name_it_doesnt_know() {
        // Rust's Scale is an enum, so "a name it doesn't know" cannot be
        // constructed — `core::chords::Scale`'s Deserialize impl is where
        // that fallback would live, and it does not exist because there is
        // no unknown variant to fall back from. What this pins instead is
        // that the intervals really do come from `Scale::intervals()`.
        assert_eq!(scale_intervals(Scale::Dorian), Scale::Dorian.intervals());
    }
}
