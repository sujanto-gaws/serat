# Third-party notices

Serves NFR-SEC-07 and the P5 release checklist (task #149); honours ADR-0017 Amendment 1 and C-01. This lists every third-party asset Serat ships in the webview bundle (`crates/serat-app/webview/`), with the licence, the copyright holder as the shipped licence file states it, and where the licence text is. It is checked by `app/src/build/notices.test.ts`: every `LICENSE*` file under the webview directory must be named here, and every webview path named here must exist.

Holders and terms below are read from the licence files themselves, not from memory. Where a file does not name a holder, that is said rather than filled in.

## 1. Assets shipped in the webview bundle

| Asset | Version / source | Licence | Copyright holder (as stated) | Licence text ships at |
|---|---|---|---|---|
| pdf.js (`boot-chunk-pdf.js`, `assets/pdf.worker.min.js`) | pdfjs-dist 6.5.438 (`app/package.json`, `app/package-lock.json`) | Apache-2.0 | Mozilla Foundation (source header of `build/pdf.mjs`: "Copyright 2024 Mozilla Foundation"; the package `LICENSE` is the unmodified Apache text and names no holder) | `crates/serat-app/webview/assets/LICENSE_pdfjs` (the unmodified `pdfjs-dist/LICENSE`, emitted beside `assets/pdf.worker.min.js` by `app/src/build/fontLicenses.ts`) |
| pdf.js CMaps (`cmaps/*.bcmap`, 168 files) | pdfjs-dist 6.5.438, copied by `app/src/build/pdfjsData.ts` | BSD-3-Clause-style (Adobe's CMap licence: source and binary redistribution with notice, no endorsement; the file does not name an SPDX id) | Adobe Systems Incorporated ("Copyright 1990-2009 Adobe Systems Incorporated. All rights reserved.") | `crates/serat-app/webview/cmaps/LICENSE` |
| Foxit standard fonts (`standard_fonts/Foxit*.pfb`, 10 files) | pdfjs-dist 6.5.438 | BSD-3-Clause-style (three clauses as written; the file does not name an SPDX id) | PDFium Authors ("Copyright 2014 PDFium Authors. All rights reserved."); the neutral-name clause names Google Inc. | `crates/serat-app/webview/standard_fonts/LICENSE_FOXIT` |
| Liberation fonts (`standard_fonts/LiberationSans-*.ttf`, 4 files) | pdfjs-dist 6.5.438 | GNU GPL v2 with the Liberation font exceptions (NOT OFL-1.1; the shipped file is the Red Hat Liberation 1.x agreement and carries the GPL-2.0 text) | Red Hat, Inc. ("Copyright © 2007-2011 Red Hat, Inc. All rights reserved."; "LIBERATION" is a Red Hat trademark) | `crates/serat-app/webview/standard_fonts/LICENSE_LIBERATION` |
| Source Serif 4 (`assets/source-serif-4-latin-wght-*.woff2`, 2 files) | `@fontsource-variable/source-serif-4` 5.3.1 | OFL-1.1 | The licence file's first line reads only "Google Inc." (also the package `author`); it carries no "Copyright" line and does not name the typeface's designers | `crates/serat-app/webview/assets/LICENSE_source-serif-4` |
| Bricolage Grotesque (`assets/bricolage-grotesque-latin-wght-normal.woff2`) | `@fontsource-variable/bricolage-grotesque` 5.3.0 | OFL-1.1 | "Copyright 2022 The Bricolage Grotesque Project Authors (https://github.com/ateliertriay/bricolage)" | `crates/serat-app/webview/assets/LICENSE_bricolage-grotesque` |
| JetBrains Mono (`assets/jetbrains-mono-latin-wght-normal.woff2`) | `@fontsource-variable/jetbrains-mono` 5.3.0 | OFL-1.1 | "Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono)" | `crates/serat-app/webview/assets/LICENSE_jetbrains-mono` |

The three OFL licence files are emitted by `app/src/build/fontLicenses.ts` from each package's `LICENSE`; the CMap and standard-font files are emitted by `app/src/build/pdfjsData.ts`. OFL-1.1 section 2 requires the licence to travel with each redistributed copy of the fonts, and the BSD notices require the same for binary redistribution; the bundle does that for the seven licence files above.

Terms worth knowing before release (read from the files):

- **Liberation** is GPL-2.0 with two exceptions: a document embedding the font is not covered by the GPL (exception (a)), and a physical product distributing the object code must give the user the right to access and modify the Software's source and reinstall it (exception (b)). The fonts are data files that pdf.js loads only to substitute for unembedded standard fonts. Serat does not modify them. Whether shipping them GPL-2.0 alongside a differently licensed app needs any further ruling is the lead's call; deny.toml and npm-deny.json do not see them, because they arrive as data inside `pdfjs-dist`, whose own `license` field is Apache-2.0.
- **OFL-1.1** fonts may be bundled and redistributed with software but not sold on their own, and a modified font may not use a Reserved Font Name. Serat ships them unmodified (woff2 subsets produced by the fontsource packages).

## 2. Open items (not satisfied by the bundle today)

1. **Bundled npm code ships without notices.** Vue, Pinia, CodeMirror, Lezer, markdown-it and its dependencies (MIT, BSD-2-Clause, PSF-2.0 for argparse) are compiled into `boot.js` and `boot-chunk-markdown-it.js`, which carry no licence banners. Section 3 lists them by licence family, but the licence texts of those packages ship nowhere in the app. P5 follow-up (banners or a generated notice for bundled npm code). MIT, BSD and PSF-2.0 each ask for the notice to accompany copies.
2. **Rust crates**: the same applies to the 426 crates compiled into the Tauri binary (section 3). A generated all-crates notices file (for example `cargo about`) is not in the repository. P5 follow-up.
3. **Source Serif 4 holder** is whatever Fontsource's `LICENSE` says ("Google Inc."), which is not an upstream copyright line. It is reported as shipped.

## 3. Dependency licence summaries

### 3.1 Rust workspace

Policy: `deny.toml` `[licenses].allow` (all permissive; MPL-2.0 is the only weak-copyleft entry), enforced by the `deny` CI job (`cargo deny --workspace check`; TC-129).

Regenerate the authoritative list on a machine with cargo-deny: `cargo deny --workspace list` (or `cargo deny --workspace check licenses`).

The counts below were NOT produced by cargo-deny, which was not installed in the environment that wrote this file. They come from `cargo metadata --format-version 1 --offline --filter-platform <target>` for the four targets in `deny.toml` (`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`), taking the union of packages reachable from the workspace members through normal and build dependencies (dev-dependencies excluded), minus the workspace's own crates, and grouping the `license` field as declared in each crate's manifest. Re-run it when `Cargo.lock` changes.

| Licence family (declared SPDX expression) | Crates |
|---|---|
| MIT OR Apache-2.0 (spelled `MIT OR Apache-2.0`, `Apache-2.0 OR MIT`, `MIT/Apache-2.0`, `Apache-2.0/MIT`, `Apache-2.0 / MIT`) | 253 |
| MIT only | 105 |
| Unicode-3.0 (the `unicode-*`, ICU family) | 18 |
| Unlicense OR MIT (including `Unlicense/MIT`) | 11 |
| Three-way and wider permissive ORs (Zlib, 0BSD, CC0-1.0, MIT-0, LLVM-exception arms) | 18 |
| MPL-2.0 (cssparser, cssparser-macros, dtoa-short, option-ext, selectors) | 5 |
| ISC (inotify, inotify-sys, libloading) | 3 |
| BSD-2-Clause (comrak) / BSD-3-Clause (alloc-stdlib, alloc-no-stdlib) / BSD-3-Clause AND MIT (brotli) / BSD-3-Clause/MIT (brotli-decompressor) | 5 |
| Zlib (foldhash, zlib-rs) | 2 |
| CC0-1.0 (notify) | 1 |
| Compound AND expressions: `(MIT OR Apache-2.0) AND Unicode-3.0` (unicode-ident), `(MIT OR Apache-2.0) AND Unicode-DFS-2016` (finl_unicode), `Apache-2.0 AND MIT` (dpi) | 3 |
| Apache-2.0 only (tao) / Apache-2.0 WITH LLVM-exception (target-lexicon) | 2 |
| Total third-party crates | 426 |

MPL-2.0 is file-level copyleft: the five crates are used unmodified from crates.io, and their source is public at the registry. LGPL-2.1-or-later appears only as one arm of `r-efi`'s OR expression, which the MIT arm satisfies (`deny.toml`).

### 3.2 npm production dependencies (`app/`)

Policy: `/npm-deny.json` `licenses.allow`, enforced by the `npm-deny` CI job (`bench/npm-deny/README.md`).

Regenerate: `node bench/npm-deny/npm-deny.mjs licenses` (pass/fail against the allow-list, the same check CI runs), and for this table, group `license` over the entries of `app/package-lock.json` whose `dev` flag is not set:

```
node -e 'const l=require("./app/package-lock.json").packages,m={};for(const[k,v]of Object.entries(l)){if(!k||v.dev)continue;(m[v.license]??=[]).push(k.replace(/.*node_modules\//,""))}for(const[k,v]of Object.entries(m))console.log(k,v.length)'
```

The 2026-11-23 run of both commands on this tree: `licenses` passed; 83 non-dev lockfile entries.

| Licence | Entries | Notes |
|---|---|---|
| MIT | 71 | Vue, Pinia, CodeMirror 6, Lezer, markdown-it and the rest of the tree |
| Apache-2.0 | 2 | pdfjs-dist (section 1) and typescript (listed non-dev in the lockfile; build tooling, not in the bundle) |
| BSD-3-Clause | 2 | source-map-js, speakingurl |
| BSD-2-Clause | 2 | entities (two versions) |
| OFL-1.1 | 3 | the three font packages (section 1) |
| Apache-2.0 OR MIT | 1 | `@tauri-apps/api` |
| ISC | 1 | picocolors |
| PSF-2.0 | 1 | argparse, through markdown-it (accepted by lead ruling 2026-11-12, `npm-deny.json`) |

## 4. Keeping this file true

`cd app && npm test` runs `src/build/notices.test.ts`. It fails when a `LICENSE*` file appears under `crates/serat-app/webview/` that this file does not name, when this file names a webview path that no longer exists, and when the pdfjs-dist version stated above differs from the lockfile's. Adding an asset to the bundle therefore requires an edit here in the same change.
