//! The case fold link resolution compares by (architecture §4.5, §9.3; NFR-CMP-01).
//!
//! One function, used on both sides of every comparison: the writer folds
//! `notes.file_name`, `notes.path` and `links.dst_path` into the stored `*_fold`
//! columns with it, and a reader folds its bound parameter with it before an `=`.
//! No SQL function is registered (C-03), so SQLite never folds anything itself.

/// Per-character Unicode lowercasing, then final sigma (`ς`, U+03C2) to `σ` (U+03C3).
///
/// This is deliberately **not** `str::to_lowercase`. That function is context
/// sensitive for Greek final sigma, so it is not compositional: `"ΟΔΟΣ.md"` lowers to
/// `"οδοσ.md"` but `"ΟΔΟΣ"` followed by `".md"` lowers to `"οδος.md"`, and the resolver
/// compares `fold(file_name)` with `fold(target) + ".md"`. Per-character mapping has
/// `fold(a + b) == fold(a) + fold(b)` by construction, and mapping `ς` to `σ` makes the
/// result independent of where in a word a sigma falls. It is not ICU and not full case
/// folding (`ß` stays `ß`, as in Obsidian, whose base mapping is JavaScript's
/// `toLowerCase`). Changing this function is a schema change: drop and rebuild
/// (ADR-0002).
#[must_use]
pub fn fold_case(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == '\u{3C2}' { '\u{3C3}' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::fold_case;

    /// xorshift64*: a deterministic generator, so a failure is reproducible from the
    /// seed in its message without adding a property-test dependency to serat-core.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        /// A character biased toward the ones that make case mapping interesting:
        /// both sigmas, capital sigma, dotted I, `ß`, ASCII, and the rest of Unicode.
        fn character(&mut self) -> char {
            const SPECIAL: &[char] = &[
                'Σ', 'σ', 'ς', 'Ο', 'Δ', 'ο', 'δ', 'İ', 'I', 'ı', 'ß', 'ẞ', 'É', 'é', '/', '.',
                'ǅ', 'Σ',
            ];
            match self.below(4) {
                0 => SPECIAL[self.below(SPECIAL.len() as u64) as usize],
                1 => char::from(b' ' + self.below(95) as u8),
                2 => char::from_u32(0x370 + self.below(0x90) as u32).unwrap_or('x'),
                _ => loop {
                    // Any scalar value, surrogates excluded by from_u32.
                    if let Some(c) = char::from_u32(self.below(0x11_0000) as u32) {
                        return c;
                    }
                },
            }
        }

        fn string(&mut self) -> String {
            let len = self.below(12);
            (0..len).map(|_| self.character()).collect()
        }
    }

    #[test]
    fn folding_is_compositional_over_arbitrary_unicode() {
        // REQUIRED (architecture §4.5): fold(a + b) == fold(a) + fold(b). It is what
        // lets the resolver compare fold(file_name) with fold(target) + ".md" and
        // build path_fold across '/'.
        for seed in 1..=20_000u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let (a, b) = (rng.string(), rng.string());
            let joined = format!("{a}{b}");
            assert_eq!(
                fold_case(&joined),
                format!("{}{}", fold_case(&a), fold_case(&b)),
                "seed {seed}: a = {a:?}, b = {b:?}"
            );
        }
    }

    #[test]
    fn all_caps_greek_ending_in_sigma_resolves_with_the_extension_appended() {
        // The case str::to_lowercase gets wrong (code-reviewer-2): the file name
        // carries ".md", the target does not.
        assert_eq!(fold_case("ΟΔΟΣ.md"), fold_case("ΟΔΟΣ") + ".md");
        assert_eq!(fold_case("ΟΔΟΣ.md"), "οδοσ.md");
        assert_eq!(fold_case("οδος"), fold_case("ΟΔΟΣ"));
        assert_eq!(fold_case("a/ΟΔΟΣ/b"), "a/οδοσ/b");
    }

    #[test]
    fn it_is_unicode_lowercasing_and_nothing_wider() {
        assert_eq!(fold_case("Émile"), "émile");
        assert_eq!(fold_case("Straße"), "straße", "ß does not become ss");
        assert_eq!(fold_case("My Note"), "my note");
        assert_eq!(fold_case(""), "");
    }

    #[test]
    fn folding_is_idempotent() {
        for seed in 1..=2_000u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let s = rng.string();
            let once = fold_case(&s);
            assert_eq!(fold_case(&once), once, "seed {seed}: {s:?}");
        }
    }
}
