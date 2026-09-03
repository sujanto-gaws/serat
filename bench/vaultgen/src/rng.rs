//! Deterministic pseudo-randomness.
//!
//! The generator owns its PRNG rather than depending on `rand`, because
//! NFR-MNT-02's determinism clause ("one seed produces byte-identical output")
//! would otherwise be hostage to a dependency's algorithm changing between
//! versions. splitmix64 and xoshiro256** are fixed, published algorithms; the
//! output of this module is stable for as long as this file is.

/// splitmix64 — used to derive independent stream seeds from one master seed.
#[inline]
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Mix a master seed with a domain tag and an ordinal into a stream seed.
///
/// This is what makes generation order-independent: note `i`'s content depends
/// only on `(seed, tag, i)`, never on how many notes were generated before it.
/// Parallel generation therefore produces the same bytes as serial generation.
pub fn stream_seed(seed: u64, tag: u64, ordinal: u64) -> u64 {
    let mut s = seed ^ tag.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    let a = splitmix64(&mut s);
    let mut t = a ^ ordinal.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    splitmix64(&mut t)
}

/// xoshiro256** — small, fast, well-distributed, and fully specified.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// Seed a stream. Any `u64` is a valid seed; the state is expanded through
    /// splitmix64 so that adjacent seeds give unrelated streams.
    pub fn new(seed: u64) -> Self {
        let mut z = seed;
        let s = [
            splitmix64(&mut z),
            splitmix64(&mut z),
            splitmix64(&mut z),
            splitmix64(&mut z),
        ];
        Rng { s }
    }

    /// The next 64 random bits.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in `[0, n)`. Lemire's multiply-shift with rejection, so the
    /// result is unbiased and identical on every platform.
    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        debug_assert!(n > 0);
        let mut x = self.next_u64();
        let mut m = (x as u128) * (n as u128);
        let mut l = m as u64;
        if l < n {
            let threshold = n.wrapping_neg() % n;
            while l < threshold {
                x = self.next_u64();
                m = (x as u128) * (n as u128);
                l = m as u64;
            }
        }
        (m >> 64) as u64
    }

    /// Uniform in `[lo, hi]`, inclusive.
    #[inline]
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        debug_assert!(hi >= lo);
        lo + self.below((hi - lo + 1) as u64) as i64
    }

    /// Uniform in `[0, 1)`, 53 significant bits.
    #[inline]
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// True with probability `p`.
    #[inline]
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// Pick one element uniformly. Panics on an empty slice.
    ///
    /// `Copy` rather than a reference: every list this is used on holds
    /// `&'static str`, and returning `&&str` would make every call site a
    /// deref.
    #[inline]
    pub fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u64) as usize]
    }

    /// Standard normal via Box-Muller (one of the pair; the other is discarded
    /// so the consumed-word count stays constant per call).
    pub fn normal(&mut self) -> f64 {
        let u1 = self.unit().max(f64::MIN_POSITIVE);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    /// Log-normal draw, clamped to `[lo, hi]`. This is the workhorse for every
    /// long-tailed quantity: note length, out-degree, task count. A uniform
    /// draw here would make the vault uniform, which §10.3 of the architecture
    /// explicitly rejects.
    pub fn lognormal(&mut self, median: f64, sigma: f64, lo: f64, hi: f64) -> f64 {
        let v = median * (self.normal() * sigma).exp();
        v.clamp(lo, hi)
    }
}

/// Rank-based Zipf–Mandelbrot sampler over `n` items.
///
/// `sample()` returns a *rank* in `[0, n)` where rank 0 is the most popular,
/// with weight `1 / (rank + 1 + offset)^exponent`. The caller maps rank to a
/// note through a fixed permutation, so popularity is spread across the vault
/// rather than concentrated in the first files written.
///
/// The resulting in-degree distribution is the point of the whole structure: a
/// few hundred hub notes accumulate hundreds of backlinks each, the median note
/// has two or three, and a tenth of the vault is orphaned. A uniform draw would
/// make backlinks and rollups look far cheaper than they are, which
/// architecture §10.3 explicitly rejects.
///
/// The `offset` is what keeps the head honest: a pure Zipf over 20,000 ranks
/// hands the single most popular note about a seventh of every link in the
/// vault, which no real vault does.
#[derive(Debug)]
pub struct Zipf {
    /// Cumulative weights, normalised to 1.0.
    cdf: Vec<f64>,
}

impl Zipf {
    /// Build the sampler. `exponent` sets the tail, `offset` flattens the head.
    pub fn new(n: usize, exponent: f64, offset: f64) -> Self {
        let mut cdf = Vec::with_capacity(n);
        let mut acc = 0.0f64;
        for k in 0..n {
            acc += 1.0 / (k as f64 + 1.0 + offset).powf(exponent);
            cdf.push(acc);
        }
        let total = acc;
        for v in cdf.iter_mut() {
            *v /= total;
        }
        Zipf { cdf }
    }

    /// Number of ranks.
    pub fn len(&self) -> usize {
        self.cdf.len()
    }

    /// Whether there are no ranks at all.
    pub fn is_empty(&self) -> bool {
        self.cdf.is_empty()
    }

    /// Inverse-transform sample: binary search the CDF.
    pub fn sample(&self, rng: &mut Rng) -> usize {
        let u = rng.unit();
        match self
            .cdf
            .binary_search_by(|probe| probe.partial_cmp(&u).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) => i,
            Err(i) => i.min(self.cdf.len() - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xoshiro_stream_is_stable() {
        // Guards the determinism clause of NFR-MNT-02 at its root: if this
        // sequence ever changes, every generated vault changes with it.
        let mut r = Rng::new(20_260_903);
        let got: Vec<u64> = (0..4).map(|_| r.next_u64()).collect();
        assert_eq!(
            got,
            vec![
                3_646_369_797_721_807_864,
                4_208_820_539_722_341_043,
                10_868_309_395_278_033_232,
                3_979_085_695_035_525_000
            ]
        );
    }

    #[test]
    fn stream_seed_is_order_independent() {
        // The same (seed, tag, ordinal) always yields the same stream, which is
        // what lets note bodies be generated in parallel without changing.
        let a = stream_seed(7, TAGLESS, 41);
        let b = stream_seed(7, TAGLESS, 41);
        assert_eq!(a, b);
        assert_ne!(a, stream_seed(7, TAGLESS, 42));
        assert_ne!(a, stream_seed(8, TAGLESS, 41));
        assert_ne!(a, stream_seed(7, TAGLESS + 1, 41));
    }
    const TAGLESS: u64 = 99;

    #[test]
    fn below_is_in_range_and_covers_it() {
        let mut r = Rng::new(1);
        let mut seen = [false; 7];
        for _ in 0..2_000 {
            let v = r.below(7);
            assert!(v < 7);
            seen[v as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }

    #[test]
    fn zipf_head_is_heavy_and_tail_is_long() {
        // The property the benchmark depends on: the top 1% of ranks take a
        // large but not absurd share of the draws.
        let z = Zipf::new(20_000, 1.0, 50.0);
        let mut r = Rng::new(42);
        let mut counts = vec![0u32; 20_000];
        let draws = 200_000;
        for _ in 0..draws {
            counts[z.sample(&mut r)] += 1;
        }
        let top_1pct: u32 = counts[..200].iter().sum();
        let share = f64::from(top_1pct) / f64::from(draws);
        assert!(
            (0.20..0.35).contains(&share),
            "top 1% of ranks took {share} of links; expected a long tail, not a uniform graph"
        );
        // And the tail is genuinely thin: most ranks are drawn a handful of times.
        let median = counts[10_000];
        assert!(
            median < 20,
            "median rank drawn {median} times; tail is not long"
        );
    }
}
