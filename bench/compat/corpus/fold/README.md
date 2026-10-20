# Fold corpus (task #12; architecture section 4.5, NFR-CMP-01)

Fixtures for the link-resolution case fold, to be checked against Obsidian.
Run by `crates/serat-index/tests/compat_fold.rs` (`cargo test -p serat-index --test compat_fold`).
Each directory is one case; `link-*.md` files hold exactly one `[[link]]` line.

| Case | Note | Link spellings | Expected |
|---|---|---|---|
| greek | `ΟΔΟΣ.md` (ends in capital sigma) | `[[ΟΔΟΣ]]`, `[[οδος]]`, `[[οδοσ]]` | all three resolve; the fold maps final sigma to sigma |
| turkish | `İstanbul.md` (U+0130) | `[[İstanbul]]` resolves; `[[istanbul]]` and `[[ıstanbul]]` do NOT | matches Obsidian: JavaScript lowercases U+0130 to `i` + U+0307, never to plain `i`, so `[[istanbul]]` does not find `İstanbul.md` |
| nfd | `Café` spelled NFD (`e` + U+0301) | `[[Café]]` in NFC and in NFD | built at test time, Linux only |

## Why the NFD case is not committed

APFS (macOS) normalises file names, so an NFC and an NFD name cannot sit side by
side there, and git on macOS (`core.precomposeunicode`) rewrites NFD names to NFC on
checkout. A committed NFD name would silently become the NFC case on a Mac. The test
therefore creates the NFD file in a temp directory with the name spelled out in
escapes, and it is a Linux-only case (`#[cfg(target_os = "linux")]`).

Whether Obsidian resolves a link across the two spellings is not yet measured: it
needs one run of Obsidian against this folder, and architecture section 10 records
that as an open question. Until it is measured the cross-spelling test is
`#[ignore]`d with that reason; the same-spelling case is asserted.
