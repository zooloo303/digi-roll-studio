// The progression library, tagged by genre.
//
// Port of `js/gen/progressions.js`. Roman numerals in the key the Harmony
// panel is set to, so `i VI III VII` in C minor and in F# minor are the same
// progression — which is the point of writing them this way rather than as
// chord names.
//
// These are loops, not songs: two to four chords, because a pattern is one
// to eight bars and the box loops it. Anything longer belongs in the
// editable field.
//
// Every entry's `text` is parsed by `theory::parse_progression`, and the
// test suite parses all of them — a typo here is a test failure rather than
// a status-line error in front of a user.

use crate::genres::GenreId;

pub struct ProgressionEntry {
    pub text: &'static str,
    pub genres: &'static [GenreId],
    pub note: &'static str,
}

use GenreId::{Breaks, Dnb, Electro, House, Rollers, Techno};

pub const PROGRESSIONS: &[ProgressionEntry] = &[
    // --- minor loops, the shared backbone -----------------------------------------
    ProgressionEntry {
        text: "i VI III VII",
        genres: &[Dnb, Breaks, Electro],
        note: "the minor four-chord workhorse — descending, never resolves",
    },
    ProgressionEntry {
        text: "i VII VI VII",
        genres: &[Dnb, Electro],
        note: "rocking minor vamp, stays close to home",
    },
    ProgressionEntry {
        text: "i VII IV VI",
        genres: &[Dnb, Breaks, Electro, Techno],
        note: "Elektron's own — the Analog Four's factory A01 walks Am G D F under a chord lead",
    },
    ProgressionEntry {
        text: "i iv VI v",
        genres: &[Breaks, Dnb],
        note: "minor with a real subdominant — more movement, more soul",
    },
    ProgressionEntry {
        text: "i VI iv VII",
        genres: &[Breaks, Electro],
        note: "lifts on the iv, lands on the VII",
    },
    // --- DnB: pedal and move ------------------------------------------------------
    ProgressionEntry {
        text: "i:2 VI:2",
        genres: &[Dnb],
        note: "two bars on the tonic, two on the relative major — room to breathe",
    },
    ProgressionEntry {
        text: "i:2 VII:2",
        genres: &[Dnb, Rollers],
        note: "pedal-and-move: sit on i, drop a tone",
    },
    ProgressionEntry {
        text: "i7:2 iv7:2",
        genres: &[Dnb, Rollers],
        note: "liquid: minor sevenths, two bars each",
    },
    // --- house: seventh vamps -----------------------------------------------------
    ProgressionEntry {
        text: "i7 iv7",
        genres: &[House],
        note: "the two-chord house vamp — everything else is groove",
    },
    ProgressionEntry {
        text: "i7 VI7 iv7 v7",
        genres: &[House],
        note: "four sevenths round the loop, deep-house flavoured",
    },
    ProgressionEntry {
        text: "ii7 v7",
        genres: &[House],
        note: "ii–v that never resolves, which is why it loops forever",
    },
    ProgressionEntry {
        text: "i7:2 VII7:2",
        genres: &[House, Dnb, Rollers],
        note: "sevenths, two bars each — pads more than stabs",
    },
    // --- electro: static and mechanical -------------------------------------------
    ProgressionEntry {
        text: "i i VI VI",
        genres: &[Electro, Techno, Rollers],
        note: "barely moves — the riff does the work",
    },
    ProgressionEntry {
        text: "i III VII iv",
        genres: &[Electro, Breaks, Rollers],
        note: "brighter middle, dark landing",
    },
    ProgressionEntry {
        text: "i:4",
        genres: &[Electro, Dnb, Breaks, House, Techno, Rollers],
        note: "one chord, four bars — a modal drone for a riff to sit on",
    },
    // --- rollers: pedal, and one move ----------------------------------------------
    //
    // **Rollers is deliberately absent from the four-chord backbone at the
    // top of this list**, which is the only genre here that skips it, and the
    // omission is the point rather than an oversight. A chord a bar fights a
    // bassline that spends the bar hammering one root: the ear hears the
    // third of bar 2 against a root that has not moved, and the roll stops
    // being a pedal. So every loop tagged `Rollers` either holds a chord for
    // two bars or barely moves at all — which is also why the tags above sit
    // on the DnB pedal-and-move entries and on Electro's static ones, and
    // nowhere else.
    //
    // `default_progression_for` takes the *first* tagged entry by the order
    // of this list, so the default is `i:2 VII:2` — the one Neil was already
    // working from.
    ProgressionEntry {
        text: "i:2 iv:2",
        genres: &[Rollers],
        note: "two bars home, two on the minor subdominant — moves without leaving",
    },
    // --- techno: hypnotic and static -----------------------------------------------
    ProgressionEntry {
        text: "i i i VII",
        genres: &[Techno],
        note: "three bars pedalling before the one move — built for a long, hypnotic loop",
    },
];

pub fn progressions_for(genre: GenreId) -> Vec<&'static ProgressionEntry> {
    PROGRESSIONS.iter().filter(|p| p.genres.contains(&genre)).collect()
}

/// The one a genre starts on: its first entry, which is the most
/// characteristic of that genre by the order above.
pub fn default_progression_for(genre: GenreId) -> &'static str {
    progressions_for(genre).first().map(|p| p.text).unwrap_or(PROGRESSIONS[0].text)
}

/// The ↻ button: the next progression of this genre after the current text,
/// wrapping. Text the library doesn't have (something typed by hand) starts
/// the cycle from the beginning rather than being treated as an error — the
/// button means "show me another", not "validate what I typed".
pub fn next_progression_for(genre: GenreId, text: &str) -> &'static str {
    let list = progressions_for(genre);
    if list.is_empty() {
        return default_progression_for(genre);
    }
    let trimmed = text.trim();
    let at = list.iter().position(|p| p.text == trimmed);
    match at {
        Some(i) => list[(i + 1) % list.len()].text,
        None => list[0].text,
    }
}

pub fn progression_note(text: &str) -> &'static str {
    let trimmed = text.trim();
    PROGRESSIONS.iter().find(|p| p.text == trimmed).map(|p| p.note).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genres::GenreId;
    use crate::theory::{format_progression, parse_progression};

    #[test]
    fn parses_every_entry_in_the_library() {
        for p in PROGRESSIONS {
            let prog = parse_progression(p.text).unwrap_or_else(|e| panic!("{}: {e}", p.text));
            assert_eq!(format_progression(&prog), p.text);
        }
    }

    #[test]
    fn describes_each_entry_for_the_hint_under_the_field() {
        for p in PROGRESSIONS {
            assert!(p.note.len() > 5);
        }
    }

    #[test]
    fn has_something_for_every_genre() {
        for genre in GenreId::ALL {
            let list = progressions_for(genre);
            assert!(!list.is_empty());
            assert_eq!(list[0].text, default_progression_for(genre));
        }
    }

    #[test]
    fn cycles_through_a_genres_own_progressions_and_wraps() {
        let list: Vec<&str> = progressions_for(GenreId::House).iter().map(|p| p.text).collect();
        let mut at = list[0];
        for want in &list[1..] {
            at = next_progression_for(GenreId::House, at);
            assert_eq!(at, *want);
        }
        assert_eq!(next_progression_for(GenreId::House, at), list[0]);
    }

    #[test]
    fn starts_the_cycle_from_the_top_for_something_typed_by_hand() {
        assert_eq!(
            next_progression_for(GenreId::Dnb, "ii V i"),
            progressions_for(GenreId::Dnb)[0].text
        );
    }
}
