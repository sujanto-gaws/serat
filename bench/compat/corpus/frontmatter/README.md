# Frontmatter block-list corpus (task #57; FR-STR-01, FR-NAV-01, NFR-CMP-01)

Fixtures for block-style frontmatter lists (`aliases:` followed by `- item` lines).
Run by `crates/serat-parse/tests/compat_frontmatter.rs`
(`cargo test -p serat-parse --test compat_frontmatter`). Every file carries
`title: Still Here` before the list and `status: after` behind it, so each case also
proves the neighbouring keys survive. The subset is written down in the module doc of
`crates/serat-parse/src/frontmatter.rs`.

## accepted/ : one `List` field, one index row per item

| File | Shape | Expected items |
|---|---|---|
| aliases-plain | plain scalars, spaces inside | `alpha`, `beta gamma` |
| aliases-quoted | double and single quotes holding `: ` and ` #`; parentheses | quotes removed, text kept |
| tags-column-zero | `- item` at the key's own column | `one`, `two` |
| tags-four-space | four-space indent | three items |
| comments-and-blanks | comment line, trailing comments, blank line between items | `alpha`, `beta` |
| null-item-dropped | a bare `-` between items | the bare item is dropped |
| typed-plain-items | `2026`, `"2026"`, `true` | number, text, bool: quoted is always text |
| wikilinks-quoted | `- "[[Ada Lovelace]]"` | text `[[Ada Lovelace]]` (the link scanner finds the links separately) |
| non-ascii | accented and Greek items | byte ranges stay on char boundaries |

Every accepted case is also run with CRLF line endings.

## refused/ : no field for the key, no panic, neighbours intact

| File | Why refused |
|---|---|
| nested-sequence | `- - inner` |
| mixed-indent, mixed-indent-shallower | items at different indents |
| map-items | `- name: alpha` (a list of maps) |
| flow-item, flow-map-item | `- [a, b]`, `- {a: b}` |
| unquoted-wikilink | `- [[Ada]]` is a flow sequence in YAML; quote it |
| block-scalar | `- \|` |
| anchor | `- &a alpha` |
| text-after-quote | `- "alpha" beta` |
| unterminated-quote | `- "alpha` |
| not-a-marker | `-alpha` |
| nested-map | `owner:` with indented `name:`/`type:` children; the children must not surface as top-level keys |

Whether Obsidian agrees on each refused shape (it is a full YAML reader and accepts
several of them) is not measured; refusing is the conservative side, since a skipped
field is absent from the index and a half-read one would be wrong.
