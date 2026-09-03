---
project: Serat
artifact: Benchmark vault — shape, invocation and caveats
phase: 05-testing
owner: test-engineer
serves: NFR-MNT-02, US-35, C-06
implements: docs/03-design/architecture.md §10.3
---

# The Benchmark Vault

Every performance requirement in the baseline is measured against one artefact (architecture §10.3). This is that artefact's documentation: what the generator produces, how to run it, and what it deliberately does *not* do.

**The generator lives in the repository; the vault does not.** `bench/vaultgen/` is committed, `bench/vaults/` is gitignored. The vault is derived state — regenerate it, never commit it. That is NFR-MNT-02's whole point: a benchmark nobody can reproduce is a number nobody can argue with.

## The one command

NFR-MNT-02's acceptance criterion is "repository review confirms the generator and its documented invocation". This is the documented invocation:

```
cargo run --release -p vaultgen -- --preset 20k
```

Three presets, all on the canonical seed `20260903`:

| Preset | Notes | Assets | Output | Purpose |
|---|---|---|---|---|
| `--preset smoke` | 400 | 24 | `bench/vaults/vault-smoke` | Fast local check; not a budget target |
| `--preset 20k` | 20,000 | 200 | `bench/vaults/vault-20k` | **The P0 gate vault** — NFR-PRF-01/03/04 |
| `--preset 50k` | 50,000 | 500 | `bench/vaults/vault-50k` | The scale run — P5 |

Regenerating over an existing vault needs `--force`. `--force` will only ever delete a directory carrying the `.vaultgen` stamp file the generator writes; pointed at anything else it refuses, so a mistyped path cannot destroy a real vault.

Other flags: `--out`, `--notes`, `--assets`, `--seed`, `--manifest`, `--jobs`, `--expect <sha256>`, `--quiet`. `--help` prints them. `--jobs` changes throughput and nothing else — the bytes are identical at any thread count, and a test asserts it.

## Determinism, and why it is built the way it is

**One seed produces byte-identical output.** Not "statistically similar" — the same bytes, the same paths, on any thread count. A regression in a benchmark is then attributable to a code change, never to a different vault.

Three consequences shaped the implementation:

- **Zero dependencies.** Not `rand`, not `clap`, not `serde`. A PRNG crate that changes its algorithm in a minor version would silently change the vault; a registry outage would stop CI generating it at all. The crate carries its own xoshiro256\*\*, its own SHA-256, its own CRC-32 and its own argument parsing, all of which are small, fixed, published algorithms with unit tests against published vectors.
- **Per-note stream seeding.** Note *i*'s content is a pure function of `(seed, tag, i)` — never of how many notes came before it. That is what lets notes be generated on 16 threads and come out in the same bytes as on one.
- **No clock, anywhere.** Dates are derived from a fixed anchor (2026-08-31) and the seed. A vault whose `created:` values moved with the calendar would not be byte-identical tomorrow.

Each run writes a manifest to `bench/vaults/<name>.manifest.json` — *outside* the vault, so the vault contains nothing a real vault would not. It carries the seed, the tree digest, and the expected counts a test can assert against.

### Canonical digests

Recorded on RM-1 (`reference-machine.md`), generator format version 1, seed 20260903. Verify with `--expect`:

| Preset | `tree_digest_sha256` |
|---|---|
| smoke | `c461c84347186fa66ff0be205d2a2e2aab1ed5bd9f281c2fba8e8b746bd9f3e9` |
| 20k | `35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e` |
| 50k | `42fb99b2c7fcef77c26d6a578d63efd4169f68236c4a4b066608d3e0f0276aa1` |

The digest is SHA-256 over every file's path and content in sorted path order. **If a change to the generator changes these, `VAULT_FORMAT_VERSION` is bumped in the same commit and every recorded benchmark number is re-taken.** A number from format version 1 and a number from format version 2 are not comparable, and reports record the version.

## What the 20k vault contains

Measured, not asserted — this table is the manifest of the current 20k vault.

| | |
|---|---|
| Files | 20,205 — 20,000 notes, 200 assets, 4 files a walk must ignore, 1 stamp |
| Size | 39.4 MiB (41,327,493 bytes), mean note ~2.0 KiB |
| Directories | 103 |

**Notes by kind** — 65% typed, 35% untyped:

| Kind | Count | Folder | Carries |
|---|---|---|---|
| `type` | 12 | `_types/` | Schema-defining notes with a `fields:` block (FR-STR-01) |
| `daily` | 1,395 | `daily/YYYY/MM/` | Consecutive days back from the anchor |
| `project` | 797 | `projects/` | `status`, `priority`, `owner` (a wikilink to a person), `due`, `est` |
| `person` | 598 | `people/` | `org`, `email`, `role`; the backlink hubs |
| `meeting` | 1,794 | `meetings/YYYY-MM/` | `date`, an `attendees:` list of wikilinks, tasks |
| `literature` | 1,595 | `literature/` | `author`, `year`, `venue`, `doi`, `rating` |
| `area` | 199 | `areas/` | Maps of content; the heaviest hubs |
| `note` | 6,579 | `notes/` | Ordinary evergreen notes |
| `untyped` | 6,981 | `inbox/`, `notes/`, vault root | No `type:` key; a third have **no frontmatter at all** |
| `sidecar` | 50 | `assets/` | Companion notes for binaries (ADR-0005) |

**The link graph** — long-tailed, which is the point:

| | |
|---|---|
| Edges | 216,971 wikilinks and embeds |
| Max in-degree | 679 |
| p50 / p90 / p99 in-degree | 4 / 20 / 141 |
| Orphans | 1,217 notes (6%) with no inbound link |
| Top 1% of notes hold | 25.0% of all inbound links |

A uniform graph would give every note ~11 backlinks and make rollups and backlink queries look far cheaper than they are (§10.3 is explicit about this). Hubs are drawn from the kinds real vaults link to — areas, type notes, projects, people — and daily notes are strongly *dis*favoured, so the shape is not merely long-tailed but long-tailed in the right places. Link shapes present: `[[Note]]`, `[[Note|alias]]`, `[[Note#Heading]]`, `[[Note#^blockid]]`, `![[Note]]`, `![[asset.pdf]]`, and Markdown links to binaries. Every wikilink resolves to a note that exists; a test asserts it, because a dangling link would make every backlink count wrong.

**Content, per the whole 20k vault:**

| | Count | Why it is there |
|---|---|---|
| Frontmatter keys | 92,814 | Both YAML shapes for `tags:` (block list and flow sequence), `aliases:`, nested `fields:` |
| Inline `key:: value` fields | 174,532 | FR-STR-04. Line form, `(key:: value)` mid-sentence, and `[key:: value]` on tasks |
| **Decoy fields** | **51,600** | `key:: value` inside fenced code (``` and `~~~`) and inside code spans. FR-STR-04 says these must produce **no** field — the manifest gives a parser test a number to check against |
| Tasks | 77,530 (27,158 done) | FR-LNS-05: `[due:: ...]`, `[est:: ...]`, `[owner:: [[Person]]]`, plus the Obsidian Tasks emoji form (`📅`, `⏫`) |
| Embeds | 5,484 | Note embeds and asset embeds |
| Block anchors | 4,000 | Every fifth note, so `[[Note#^id]]` has a real target |
| Notes with no frontmatter | 2,418 | The schema-less path |
| CRLF notes | 399 | |
| BOM notes | 79 | |

**Deliberate hostility.** CRLF line endings, UTF-8 BOMs and multi-byte characters (em dashes, emoji) are in the corpus on purpose: they are what breaks byte-range patching (C-04, ADR-0008) and what makes byte offsets and character offsets diverge. They are a small, fixed fraction — a vault that was mostly CRLF would not be realistic either. The same goes for `.trash/` and `.obsidian/`, which exist so that a walker which ignores nothing fails here rather than on a user's real vault: a correct walk finds exactly `counts.notes` notes and never the two notes in `.trash/`.

## Assets — read this before believing anything about extraction

200 binaries in `assets/`, and one in four has a sidecar note.

| Format | Count | What is actually generated |
|---|---|---|
| PDF | 90 | **Structurally valid.** One page, real xref table with correct byte offsets, Helvetica text in a content stream. Text is extractable. |
| PNG | 50 | **Structurally valid.** 8-bit RGB, real IHDR/IDAT/IEND chunks, real CRC-32s, a zlib stream of stored deflate blocks with a correct Adler-32, plus a `tEXt` chunk. |
| DOCX | 24 | **Structurally valid OOXML.** A ZIP of stored entries with real CRC-32s containing `[Content_Types].xml`, `_rels/.rels` and `word/document.xml`. |
| XLSX | 20 | **Structurally valid OOXML.** As above plus `xl/workbook.xml` and one worksheet of numbers. |
| JPG | 16 | **Placeholder.** Correct SOI / JFIF APP0 / COM / EOI framing and nothing else — no SOF, no SOS, no image data. **It will not decode.** |

P0 extracts nothing; extraction is P2 with its own budget (NFR-PRF-09, ADR-0004), and a JPEG baseline encoder written to produce a file nothing opens would be effort in the wrong place. The four valid formats are valid because it cost little and makes the same vault usable in P2. Every text-bearing asset contains a unique token `serat-asset-NNNN-marker`, so the P2 gate ("a phrase inside a PDF findable from the note search box") has something specific to search for.

**Open question for the architect:** ADR-0005 says a binary's metadata lives in "a companion Markdown file" but does not pin the filename. The generator uses `<binary-filename>.md` (so `report-0001.pdf.md`), because that cannot collide when `report-0001.pdf` and `report-0001.png` both exist. If ADR-0005 is later pinned to a different convention, the generator changes and the digests are re-taken.

## What the generator is not

- It is not a Markdown *parser* and does not validate its own output as one. Whether `serat-parse` agrees with the manifest's counts is a test in `serat-parse`, not an assumption here.
- It does not model realistic *prose*. Sentences are assembled from fixed clauses. Full-text search ranking against this corpus tells you about the index, not about English.
- It does not model file modification times. Every file is written in one run, so on-disk mtimes are all "now". Anything that tests mtime-based change detection must set them itself.
- Its `--seed` is settable, but only the canonical seed produces the vault that recorded numbers were taken against. Running with another seed prints a warning saying exactly that.

## Layout

```
bench/
  README.md                  what lives here
  vaultgen/                  the generator (committed)
    src/{main,lib,plan,note,assets,rng,hash,dates,words}.rs
    tests/{determinism,shape}.rs
  vaults/                    generated vaults (gitignored, never committed)
    vault-20k/               the vault
    vault-20k.manifest.json  its manifest, outside the vault
```

## Traceability

| Test | Verifies | Location |
|---|---|---|
| `same_seed_produces_byte_identical_output` | NFR-MNT-02 | `bench/vaultgen/tests/determinism.rs` |
| `thread_count_does_not_change_the_bytes` | NFR-MNT-02 | same |
| `a_different_seed_produces_a_different_vault` | NFR-MNT-02 | same |
| `the_manifest_digest_matches_the_files_on_disk` | NFR-MNT-02 | same |
| `the_manifest_lands_outside_the_vault` | C-02 (in spirit) | same |
| `force_refuses_to_delete_a_directory_it_did_not_create` | safety | same |
| `a_non_empty_target_needs_force` | safety | same |
| `the_note_budget_is_exact_and_the_mix_is_typed_and_untyped` | §10.3, US-35 | `bench/vaultgen/tests/shape.rs` |
| `filenames_are_legal_and_unambiguous_on_every_platform` | NFR-MNT-02 | same |
| `the_link_distribution_is_long_tailed_not_uniform` | §10.3, US-35 | same |
| `notes_carry_fields_tasks_links_and_deliberate_decoys` | FR-STR-04, FR-LNS-05 | same |
| `a_generated_vault_is_walkable_valid_utf8_with_ignorable_folders` | FR-VLT-02 | same |
| `every_wikilink_resolves_except_the_documented_decoy` | §10.3 | same |
| `assets_span_four_formats_with_their_magic_bytes` | §10.3, ADR-0005 | same |
| `the_vault_contains_the_encodings_that_break_byte_range_patching` | C-04, ADR-0008, NFR-MNT-03 | same |
| `xoshiro_stream_is_stable`, `zipf_head_is_heavy_and_tail_is_long` | NFR-MNT-02 | `bench/vaultgen/src/rng.rs` |
| `sha256_matches_published_vectors`, `crc32_…`, `adler32_…` | NFR-MNT-02 | `bench/vaultgen/src/hash.rs` |
| `every_format_has_its_magic_bytes`, `text_bearing_assets_carry_their_marker` | §10.3 | `bench/vaultgen/src/assets.rs` |
| `epoch_round_trips`, `every_day_of_a_decade_round_trips` | NFR-MNT-02 | `bench/vaultgen/src/dates.rs` |
