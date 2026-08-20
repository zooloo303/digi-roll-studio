// Seeded randomness for the pattern generator.
//
// Every generator function takes an `Rng` and calls no global random. That is
// what makes a seed reproducible and a module testable — and it is the reason
// this file exists at all rather than reaching for `rand` ad hoc.
//
// The other half of the design is [`rng_for`]: each part draws from its own
// **stream**, derived from the song seed and a tag (`"bass"`, `"lead"`,
// `"lead.motif"`). One shared stream would mean nudging the lead's density
// reshuffles the bass, because every draw after the change lands one place
// further along. Independent streams are what make the seed lock feel right:
// lock it, move one slider, and only the part you touched changes.
//
// Port of `js/gen/rng.js`. `mulberry32` is the PRNG: 32 bits of state, a
// handful of integer ops, no dependencies, good enough for musical decisions
// and not for anything that needs to be unguessable.

/// A reusable seeded stream. `mulberry32` state, boxed as a closure in the JS;
/// a struct here so it can be passed by value without an `Rc`.
#[derive(Debug, Clone, Copy)]
pub struct Rng {
    a: u32,
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self { a: seed }
    }

    /// The next draw, in `[0, 1)`.
    pub fn next(&mut self) -> f64 {
        self.a = self.a.wrapping_add(0x6d2b79f5);
        let t0 = self.a;
        // Both operands of this multiply read the same pre-multiply value —
        // `t ^ (t >>> 15)` and `t | 1` in the JS both see the `t = a` copy, not
        // each other's result.
        let mut t = (t0 ^ (t0 >> 15)).wrapping_mul(t0 | 1);
        let t1 = t;
        t ^= t1.wrapping_add((t1 ^ (t1 >> 7)).wrapping_mul(t1 | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

/// Seed + tag → a 32-bit stream id. FNV-1a over the tag, then an avalanche
/// mix, because adjacent seeds and adjacent tags (`"bass"` / `"bass2"`) must
/// not produce streams that march in step with each other — which is exactly
/// what a plain `seed + tag.len()` style derivation would do.
pub fn hash_tag(tag: &str, seed: u32) -> u32 {
    let mut h: u32 = 2_166_136_261 ^ seed;
    for byte in tag.bytes() {
        h ^= u32::from(byte);
        h = h.wrapping_mul(16_777_619);
    }
    h ^= h >> 16;
    h = h.wrapping_mul(2_246_822_507);
    h ^= h >> 13;
    h = h.wrapping_mul(3_266_489_909);
    h ^= h >> 16;
    h
}

/// An independent stream for one part of the generation, from the song's seed.
pub fn rng_for(seed: u32, tag: &str) -> Rng {
    Rng::new(hash_tag(tag, seed))
}

// --- Drawing from a stream -----------------------------------------------------

pub fn chance(rng: &mut Rng, p: f64) -> bool {
    rng.next() < p
}

pub fn range(rng: &mut Rng, lo: f64, hi: f64) -> f64 {
    lo + rng.next() * (hi - lo)
}

/// Inclusive at both ends, which is what every musical use of it wants
/// (`int_range(rng, 1, 4)` is "one to four notes").
pub fn int_range(rng: &mut Rng, lo: i64, hi: i64) -> i64 {
    lo + (rng.next() * (hi - lo + 1) as f64).floor() as i64
}

pub fn pick<'a, T>(rng: &mut Rng, items: &'a [T]) -> Option<&'a T> {
    if items.is_empty() {
        return None;
    }
    let i = (rng.next() * items.len() as f64).floor() as usize;
    items.get(i.min(items.len() - 1))
}

/// Weighted pick. Non-positive weights can never be picked; all-zero weights
/// fall back to a uniform pick rather than returning `None`, because a caller
/// asking for one of N things wants one of N things.
pub fn weighted<'a, T>(rng: &mut Rng, items: &'a [T], weight_of: impl Fn(&T) -> f64) -> Option<&'a T> {
    if items.is_empty() {
        return None;
    }
    let ws: Vec<f64> = items.iter().map(|it| weight_of(it).max(0.0)).collect();
    let total: f64 = ws.iter().sum();
    if total <= 0.0 {
        return pick(rng, items);
    }
    let mut r = rng.next() * total;
    for (item, w) in items.iter().zip(ws.iter()) {
        r -= w;
        if r < 0.0 {
            return Some(item);
        }
    }
    items.last()
}

/// N distinct items, weighted, without replacement — the draw the rhythm
/// table needs (pick 9 of 32 steps, favouring the strong ones).
///
/// Efraimidis–Spirakis: give each item the key `rng() ** (1 / weight)` and
/// take the largest keys. One pass, one random number per item, and provably
/// the same distribution as repeated weighted draws with removal — which
/// matters here because the alternative (draw, remove, redraw) walks the
/// stream a variable number of times and so makes the result depend on how
/// many collisions happened, not just on the seed.
pub fn sample_weighted<'a, T>(
    rng: &mut Rng,
    items: &'a [T],
    n: usize,
    weight_of: impl Fn(&T) -> f64,
) -> Vec<&'a T> {
    if n == 0 || items.is_empty() {
        return Vec::new();
    }
    let mut keyed: Vec<(f64, &T)> = items
        .iter()
        .map(|it| {
            let w = weight_of(it).max(0.0);
            let key = if w <= 0.0 { -1.0 } else { rng.next().powf(1.0 / w) };
            (key, it)
        })
        .collect();
    keyed.retain(|(key, _)| *key >= 0.0);
    keyed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    keyed.truncate(n);
    keyed.into_iter().map(|(_, it)| it).collect()
}

/// Fisher–Yates, on a copy.
pub fn shuffle<T: Clone>(rng: &mut Rng, items: &[T]) -> Vec<T> {
    let mut out = items.to_vec();
    let mut i = out.len();
    while i > 1 {
        i -= 1;
        let j = (rng.next() * (i + 1) as f64).floor() as usize;
        out.swap(i, j);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(rng: &mut Rng, n: usize) -> Vec<f64> {
        (0..n).map(|_| rng.next()).collect()
    }

    #[test]
    fn same_seed_same_sequence() {
        assert_eq!(take(&mut Rng::new(12345), 8), take(&mut Rng::new(12345), 8));
    }

    #[test]
    fn neighbouring_seed_different_sequence() {
        assert_ne!(take(&mut Rng::new(12345), 8), take(&mut Rng::new(12346), 8));
    }

    #[test]
    fn stays_inside_zero_one() {
        for v in take(&mut Rng::new(7), 500) {
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn per_part_streams_are_independent() {
        assert_ne!(take(&mut rng_for(99, "bass"), 12), take(&mut rng_for(99, "lead"), 12));
    }

    #[test]
    fn per_part_streams_are_reproducible() {
        assert_eq!(take(&mut rng_for(99, "lead"), 12), take(&mut rng_for(99, "lead"), 12));
    }

    #[test]
    fn streams_do_not_march_in_step() {
        // The failure this guards against is a derivation like `seed + tag.len()`,
        // where two streams differ only by their starting point and so produce
        // the same numbers one draw apart.
        let a = take(&mut rng_for(1000, "bass"), 20);
        let b = take(&mut rng_for(1001, "bass"), 20);
        let c = take(&mut rng_for(1000, "bass2"), 20);
        assert_ne!(a[1..], b[..19]);
        assert_ne!(a[1..], c[..19]);
    }

    #[test]
    fn hashes_tags_to_distinct_stream_ids() {
        let ids: std::collections::HashSet<u32> =
            ["bass", "chords", "lead", "bass.lanes", "chords.lanes", "lead.lanes"]
                .iter()
                .map(|t| hash_tag(t, 4242))
                .collect();
        assert_eq!(ids.len(), 6);
    }

    #[test]
    fn chance_zero_never_fires_chance_one_always_does() {
        let mut rng = Rng::new(3);
        for _ in 0..50 {
            assert!(!chance(&mut rng, 0.0));
            assert!(chance(&mut rng, 1.0));
        }
    }

    #[test]
    fn range_and_int_range_stay_in_bounds() {
        let mut rng = Rng::new(5);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..400 {
            let f = range(&mut rng, -3.0, 3.0);
            assert!((-3.0..=3.0).contains(&f));
            let n = int_range(&mut rng, 1, 4);
            seen.insert(n);
        }
        let mut seen: Vec<i64> = seen.into_iter().collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 2, 3, 4]);
    }

    #[test]
    fn pick_returns_none_for_nothing_and_a_member_otherwise() {
        let mut rng = Rng::new(9);
        let empty: Vec<&str> = Vec::new();
        assert_eq!(pick(&mut rng, &empty), None);
        let items = ["a", "b", "c"];
        assert!(items.contains(pick(&mut rng, &items).unwrap()));
    }

    #[test]
    fn weighted_never_returns_a_zero_weight_item_while_a_positive_one_exists() {
        let mut rng = Rng::new(11);
        let items = [("no", 0.0), ("yes", 1.0)];
        for _ in 0..200 {
            assert_eq!(weighted(&mut rng, &items, |it| it.1).unwrap().0, "yes");
        }
    }

    #[test]
    fn weighted_falls_back_to_uniform_when_every_weight_is_zero() {
        let mut rng = Rng::new(13);
        let items = [("a", 0.0), ("b", 0.0)];
        let got = weighted(&mut rng, &items, |it| it.1).unwrap();
        assert!(items.contains(got));
    }

    #[test]
    fn weighted_follows_the_weights_over_many_draws() {
        let mut rng = Rng::new(17);
        let items = [("rare", 1.0), ("common", 9.0)];
        let mut common = 0;
        for _ in 0..2000 {
            if weighted(&mut rng, &items, |it| it.1).unwrap().0 == "common" {
                common += 1;
            }
        }
        assert!(common > 1600);
        assert!(common < 1980);
    }

    struct Step {
        step: usize,
        weight: f64,
    }

    fn steps() -> Vec<Step> {
        (0..16).map(|step| Step { step, weight: if step % 4 == 0 { 1.0 } else { 0.1 } }).collect()
    }

    #[test]
    fn sample_weighted_returns_exactly_n_distinct_items() {
        let steps = steps();
        let got = sample_weighted(&mut Rng::new(21), &steps, 6, |s| s.weight);
        assert_eq!(got.len(), 6);
        let set: std::collections::HashSet<usize> = got.iter().map(|s| s.step).collect();
        assert_eq!(set.len(), 6);
    }

    #[test]
    fn sample_weighted_never_returns_a_zero_weight_item() {
        let mixed = [("a", 0.0), ("b", 1.0), ("c", 0.0)];
        let got: Vec<&str> =
            sample_weighted(&mut Rng::new(22), &mixed, 3, |it| it.1).iter().map(|it| it.0).collect();
        assert_eq!(got, vec!["b"]);
    }

    #[test]
    fn sample_weighted_caps_at_the_number_of_usable_items_rather_than_padding() {
        let steps = steps();
        assert_eq!(sample_weighted(&mut Rng::new(23), &steps, 99, |s| s.weight).len(), 16);
        assert_eq!(sample_weighted(&mut Rng::new(23), &steps, 0, |s| s.weight).len(), 0);
        let empty: Vec<Step> = Vec::new();
        assert_eq!(sample_weighted(&mut Rng::new(23), &empty, 4, |s: &Step| s.weight).len(), 0);
    }

    #[test]
    fn sample_weighted_favours_the_heavy_items() {
        // The four beats have 10x the weight of the sixteenths between them, so a
        // four-of-sixteen draw should keep landing on them.
        let steps = steps();
        let mut on_beat = 0;
        let mut rng = Rng::new(24);
        for _ in 0..200 {
            on_beat += sample_weighted(&mut rng, &steps, 4, |s| s.weight)
                .iter()
                .filter(|s| s.step % 4 == 0)
                .count();
        }
        assert!(on_beat as f64 / 800.0 > 0.7);
    }

    #[test]
    fn sample_weighted_is_deterministic_for_a_seed() {
        let steps = steps();
        let a: Vec<usize> =
            sample_weighted(&mut Rng::new(25), &steps, 5, |s| s.weight).iter().map(|s| s.step).collect();
        let b: Vec<usize> =
            sample_weighted(&mut Rng::new(25), &steps, 5, |s| s.weight).iter().map(|s| s.step).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn shuffle_keeps_every_member_and_leaves_the_input_alone() {
        let input = vec![1, 2, 3, 4, 5, 6];
        let out = shuffle(&mut Rng::new(31), &input);
        assert_eq!(input, vec![1, 2, 3, 4, 5, 6]);
        let mut sorted = out.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, input);
    }

    #[test]
    fn matches_the_js_oracle_bit_for_bit() {
        // Values read straight off `js/gen/rng.js` under node: seed 12345, five
        // draws, and two tag hashes. If mulberry32's step order ever drifts this
        // is the test that catches it, not the structural ones above.
        let mut rng = Rng::new(12345);
        let got: Vec<f64> = (0..5).map(|_| rng.next()).collect();
        let want = [
            0.9797282677609473,
            0.3067522644996643,
            0.484205421525985,
            0.817934412509203,
            0.5094283693470061,
        ];
        for (g, w) in got.iter().zip(want.iter()) {
            assert!((g - w).abs() < 1e-12, "{g} vs {w}");
        }
        assert_eq!(hash_tag("bass", 4242), 2004314899);
        assert_eq!(hash_tag("lead", 4242), 400990540);
    }
}
