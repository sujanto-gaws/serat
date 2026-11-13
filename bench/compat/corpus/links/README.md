# Link-target corpus (task #139; NFR-CMP-01, FR-NAV-04, FR-EDT-03, FR-NAV-01; ADR-0003)

Salvaged from the P0-era `bench/compat` skeleton (1a081c7), with its two wrong
premises corrected: Obsidian's block subpath is `#^id` (there is no bare `^id`),
and `[[Alias]]` is not a link target (aliases are quick-switcher candidates only,
architecture §9.2).

| File | Run by | Pins |
|---|---|---|
| heading-and-block-target.md | the link tests | the target: two headings, one `^block1` anchor |
| heading-block-link.md | `crates/serat-index/tests/compat_links.rs`, `crates/serat-parse/tests/compat_links.rs` | `#Heading`, `#^block1`, `#A#B` resolve to the base note; only the block form fills `dst_block_ref` |
| embeds.md, asset-image.png | the same two | `![[x.png]]`, `![[x.png\|300]]`, `![[note#Heading]]` resolve; width is display-only |
| alias-link.md, aliases-target.md | `compat_links.rs` (index) | `[[Ali]]`, `[[Target Alias]]` stay unresolved; `[[aliases-target]]` resolves |
| callouts.md | `app/src/editor/calloutFold.test.ts` | `> [!tip]-` is a widget or source, never rewritten (C-01) |

Avoid a backticked `#word` in prose here: the parser indexes tags inside inline
code spans (a defect against Obsidian's behaviour, reported separately), which
would add `Tag` links to the parser-side expectations.
