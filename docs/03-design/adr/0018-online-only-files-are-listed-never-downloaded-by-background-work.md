# ADR-0018: Online-only files are listed, and only an explicit open downloads one
**Status:** proposed — stays proposed until rust-core-dev, tauri-app-dev and test-engineer have reviewed it (§Objections recorded) · **Date:** 2026-11-12 · **Serves:** FR-VLT-01, FR-VLT-02, FR-VLT-04, NFR-CMP-01, NFR-PRF-01, NFR-PRF-04, NFR-SEC-06, NFR-SEC-02
**Honours:** ADR-0002 (rebuild, not migrate), ADR-0011 (the watcher path), ADR-0015 (the tree as complete directory listings), ADR-0016 (saves are patches against a base), ADR-0017 (step 7 follows reparse points); threat model T-07; assumption A-03 (sync is external)

## Context

A-03 says users sync their vault with git, Syncthing or a cloud folder. The cloud clients that matter most do not keep every file on the disk. **OneDrive Files-On-Demand** (Windows) leaves "online-only" files as placeholders: reparse points marked `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` (`0x0040_0000`), sometimes `RECALL_ON_OPEN` (`0x0004_0000`) or `OFFLINE` (`0x0000_1000`). **iCloud Drive and Dropbox on macOS** (File Provider, "Optimize Mac Storage") leave *dataless* files, flagged `SF_DATALESS` (`0x4000_0000`) in `st_flags`. **Older iCloud Drive** replaces a file with a hidden stub named `.<name>.icloud`. In every case the name, size and mtime are on disk and the bytes are not. Reading the bytes makes the operating system's sync client download them. That is slow, can fail offline, can cost money on a metered link, and fills the disk the user freed on purpose.

**Today Serat downloads silently.** Every stat-only path is believed safe: the walk, `walk_tree`, `walk_dir` and `Vault::stat` call `symlink_metadata` and `read_dir` only. But every read hydrates. A rebuild (`NoteSource::load` over every note), a warm open's catch-up on a changed file, the watcher's hash gate, `read_file` and the asset scheme's `open_verified` all hydrate. `open_verified` deliberately follows reparse points so that placeholders still open (ADR-0017 step 7). On a 20,000-note vault that is mostly online-only, one rebuild downloads the whole vault. It turns NFR-PRF-04's 30 s into however long the network takes, and it undoes the user's "free up space". Task #77 wrote two ignored Windows tests for this (crates/serat-app/tests/windows_vault.rs *(planned)*, on #77's branch, not yet on integration). The second, `reading_a_placeholder_is_reported_not_silent`, is expected to fail because no ADR said what correct is.

## Options considered

1. **Read transparently, hydrating on demand (today's behaviour).** This is Obsidian's behaviour, and the simplest. Rejected. Background work (rebuild, catch-up, watcher, embeds, previews) would download files the user never asked for, in bulk. That makes NFR-PRF-04 depend on the network, and it makes Serat the cause of network traffic the user did not request, against the spirit of NFR-SEC-02. It also fights the sync client: OneDrive dehydrates a file, the watcher sees the attribute change, re-reads the file, and hydrates it again.
2. **Refuse online-only files like refused names.** The file is not shown, only counted. Rejected. An online-only note is a perfectly good note that the user can see in Explorer or Finder. Hiding it breaks links to it (NFR-CMP-01) and the quick switcher, and makes "where is my note?" Serat's fault (goal 5: capability degrades, nothing breaks).
3. **A reported "not local" state.** Online-only files are listed in the tree, resolvable by links and findable by name, and are indexed by path only. Nothing in the background reads them. **An explicit open of one file by the user downloads that file.** Chosen.

## Decision

**One rule: an online-only file is downloaded only by an explicit open of that one file by the user, and by nothing else.** Everything below applies it.

1. **Detection is a pure function of the stat Serat already takes.** It is `serat_core::locality::is_local(platform, raw)`. On Windows, a file is not local if its attributes intersect `RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE`; the attributes come from `DirEntry::metadata()` (the directory listing's own data, no open) or `symlink_metadata`. On macOS, a file is not local if `st_flags & SF_DATALESS`; the flags come from the `lstat` the walk and `Vault::resolve` already make. **No syscall is added to any path**, so the walk's cost and NFR-PRF-01 are unchanged. On Linux there is no portable signal: FUSE clients (rclone, onedriver) hydrate inside `read` and cannot be told apart. So on Linux every file is local, and this is recorded as a limit (assumption, §PRD impact).
2. **Legacy iCloud stubs.** A dotfile named exactly `.<name>.icloud` is recognised by the walk. `<name>` is listed in its directory as a not-local entry and is never downloaded by Serat. The stub is still a dotfile, so it is not itself indexed (architecture §3.1.2, FR-VLT-07). Serat has no API to request the download, so opening one says "Open it in Finder to download it."
3. **`Vault::read` refuses a not-local file** with `VaultError::NotLocal { path }`. The check uses the `lstat` of the final component that `resolve` already makes, **before** any `open`, because on Windows a `RECALL_ON_OPEN` file hydrates on open. **Refusal is the default** so that a new caller cannot hydrate by accident (goal 3: the property is a grep, not a review). **The one hydrating path is `Vault::hydrate(path)`.** It reads the file through, off the main thread, in bounded chunks, discarding the bytes. It returns when the file is local or the read fails. A failed hydration is an error, **never zero bytes**. This is a goal-1 invariant: a patch built from an empty read would truncate the note.
4. **What counts as an explicit open.** It is a user action on one file: opening a note in an editor pane (tree, switcher, link follow, search result), opening a PDF or image in a viewer, or clicking "Download" on a not-local embed. A file open in a pane stays explicitly opened. Its save path (ADR-0016's base check) and its external-change reconciliation call `Vault::hydrate` before reading. If the user frees the file's space while editing, the next save downloads it rather than failing. **Not explicit:** the rebuild, the catch-up, the watcher, image embeds and previews inside a note, backlinks and search snippets, the asset scheme serving an `<img>`, and every agent or MCP read (P2). An agent gets `NotLocal` and says so. The user can then open the file, and C-05's diff review is never shown a diff built on bytes Serat did not have.
5. **The index stores a not-local note by path.** On a rebuild, a not-local note gets a `notes` row with its path, file name, title from the file name, kind, mtime and size, a new column **`local = 0`**, and `hash = ''`. An empty string never equals a BLAKE3 hex digest, so the hash gate can never mistake it for current. It has no blocks, links or properties. It is in the tree, the switcher and link resolution (an existing target, not an unresolved link) and is absent from full-text search. That is true, because its words are not on this machine. The column is a schema change. Under ADR-0002 that bumps `user_version` and rebuilds, with no migration. The implementing task writes the DDL in `schema/001_init.sql`.
6. **Freeing space keeps what the index knows.** A row that is already indexed (`local = 1`) and whose file becomes not local, with **mtime and size unchanged**, keeps its blocks: the content did not change, only where it is stored. Neither the watcher nor the catch-up reads it. This is what stops Serat re-downloading a file OneDrive has just freed.
7. **A remote edit to a not-local file demotes the row.** If the stat shows a different mtime or size and the file is not local, the row becomes path-only (`local = 0`, `hash = ''`, blocks removed). Stale content must not be searchable as current (goal 3). Downloading the file re-indexes it on the next event (§8).
8. **Downloading is noticed by a stat, not a read.** When a file becomes local (the user opened it, or downloaded it in Explorer or Finder), the watcher's event or the catch-up's stat finds a row with `local = 0` and a file that is local. The file is then read and indexed as a change, even if mtime and size match, because hydration need not change either. The catch-up therefore compares `local` alongside mtime and size, which is still stat-only.
9. **The asset scheme never downloads.** A not-local file gets the scheme's ordinary 404, decided before `open_verified` opens anything. `open_verified` keeps following reparse points: the handle's real path is the containment check (ADR-0017 step 7, T-07). A cloud placeholder is not a link. On Windows, Rust's `is_symlink` is true only for name-surrogate reparse tags (symlinks, junctions), and the cloud tags (`IO_REPARSE_TAG_CLOUD_*`) are not name surrogates. Its real path is also itself. `asset_info` reports `local: false`, and the viewer shows "Stored online only" with a **Download** button that calls the new command. `read_asset_range` refuses with `NotLocal`. It does not download mid-PDF, because the viewer's explicit open has already called `download_file`.
10. **The boundary.** The contract is changed by the implementing task, not here:
    - a new command `download_file(path) -> Result<(), IpcError>`, whose handler is `Vault::hydrate` on a blocking thread. It is a new ACL grant, under ADR-0012's pin;
    - `IpcError::NotLocal { path }`;
    - `local: bool` on `TreeEntry`, `AssetInfo` and the switcher's entries;
    - `IndexStatus::not_local: Option<u32>`, a count with the same rules as `refused_names`.

    The front end decides "explicit" (§4) and calls `download_file` before `read_file`. A pane shows "Downloading from OneDrive…" (or iCloud, Dropbox) as a non-blocking state, never a modal. Offline, it shows "This file is stored online only and could not be downloaded" with Retry.
11. **The user is told.** The status strip shows "N files online only" next to "names not shown", and a list behind it, with the same pattern and the same cap as the refused-names panel (`vault.refused-names`). Tree rows carry a small cloud glyph. frontend-designer writes the copy and the glyph. The brief is the implementing UI task's.

## Consequences

**Positive.**
- A 20k online-only vault opens and rebuilds without downloading a byte. NFR-PRF-04 gets faster, not slower, because unread notes are not parsed.
- NFR-PRF-01 is untouched: there are no new syscalls, and the predicate is a bit test on a stat the walk already holds.
- The sync client's "free up space" is respected.
- Links and the switcher keep working for every note (NFR-CMP-01).
- "Who downloads?" has one answer in code, `Vault::hydrate`, and one in the UI, an explicit open.

**Negative.**
- Full-text search, backlinks *from* a not-local note, and properties are incomplete until the user downloads it. That is honest, but a user who freed space on a whole vault gets a weak search, and the status-strip count is the only thing that says why.
- The front end must classify actions as explicit or not. A future feature that auto-opens notes, for example a daily-note open at launch, has to decide which it is. It is explicit only if the user configured it.
- A schema change and a rebuild.

**Risks.**
- A file can be freed between the `lstat` and the `open`, which would hydrate one file silently. Only CfApi (`CfOpenFileWithOplock`, or the placeholder compatibility mode) could prevent that, and both are out of P1 scope. The window is one file and microseconds wide, so it is accepted and recorded.
- Whether OneDrive and iCloud fire a watcher event on dehydration or hydration is unmeasured. Without one, §8 rests on the catch-up at the next open. It is listed in architecture §Open design questions.
- Linux FUSE clients cannot be detected (§1).

## PRD impact (for product-analyst)

No current requirement covers this. FR-VLT-01 and FR-VLT-02 assume a file's bytes are readable, and A-03 says sync is external without saying sync may withhold bytes. **Proposed, for product-analyst to word and number:**

- **A new functional requirement**, FR-VLT, next free ID: *"Online-only files. The system shall not cause a cloud-synced file whose contents are not stored locally to be downloaded, except when the user opens that file. Such files shall remain listed and linkable and shall be reported as not stored locally."* Must, P1, verified by Test. Acceptance: given a vault in which some notes are online-only, when it is opened, rebuilt or watched, then no online-only file is downloaded and each is listed, linkable and counted; when the user opens one, it alone is downloaded and opened. The Windows and macOS halves are verified on real machines (§Tested against), and the logic on every platform through the seam.
- **A new assumption:** on Linux, FUSE-based cloud clients download inside `read` and give no signal, so the requirement holds on Windows and macOS only.
- **An exclusion:** Serat does not download legacy iCloud `.icloud` stubs itself. It lists them and points the user to Finder.
- **No budget changes.** NFR-PRF-01 is unchanged and NFR-PRF-04 can only improve. The bench vault has no placeholders, so no budget job measures this; the CI tests above do.

A story to match, if product-analyst wants one: *"As a user whose vault lives in OneDrive with Files-On-Demand, I want Serat to open my vault without downloading it, so that my freed disk space and my metered connection are respected."*

**Implementing tasks, for lead to file.**
- rust-core-dev, serat-core and serat-vault: the predicate, the `NotLocal` refusal, `Vault::hydrate`, legacy stubs and the seam.
- rust-core-dev, serat-index: the `notes.local` column and DDL, path-only rows, and the watcher and catch-up rules of §6–§8.
- tauri-app-dev, serat-app and the contract: `download_file`, the scheme 404, `NotLocal`, the `local` fields and the count.
- vue-frontend-dev: the explicit-open classification, the pane states and the status-strip count. frontend-designer writes the copy and the glyph.
- test-engineer: the Windows and macOS real-machine runs and the open design question on watcher events.

## Objections recorded

None yet; this is a ruling taken from #77's report without an exchange. **Not yet reviewed:**
- **rust-core-dev:** the `Vault::read` default refusal, `Vault::hydrate`, the schema column and the watcher or catch-up rules.
- **tauri-app-dev:** `download_file`, the scheme's 404 and the contract fields.
- **vue-frontend-dev:** what counts as explicit.
- **test-engineer:** whether the seam in §Tested against is enough.

Their objections are recorded here when given.

## Tested against

**#77's case**, supplied by tauri-app-dev in crates/serat-app/tests/windows_vault.rs *(planned)* (`task/p1-77-windows-ci-prep` @ `ab502b6`, not yet on integration): one online-only `placeholder.md` in a OneDrive folder. Checked against it, the rule gives a definite answer to both of #77's questions. Stat paths must not hydrate, which is unchanged and still to be proven. A background read must refuse with `NotLocal` and leave the file online-only. That is stronger than #77's weakest form ("refused **or** reported"), because it rules out "downloaded and then reported".

**The tests that pin it**, written by the implementing tasks:
- **On every platform, in CI**, through a test seam in serat-vault. A `#[cfg(test)]` locality override marks chosen paths not local, so the logic runs on Linux, where real placeholders do not exist.
  - `serat-core`: `is_local` unit cases for each Windows attribute bit and for `SF_DATALESS`, both evaluated on every platform through `Platform`.
  - `serat-vault`: `a_not_local_file_is_listed_by_the_walk_and_refused_by_read`, `hydrate_is_the_only_read_that_accepts_a_not_local_file`, `a_failed_hydration_is_an_error_never_an_empty_read`, and `a_legacy_icloud_stub_lists_its_note_and_is_not_itself_indexed`.
  - `serat-index`: `a_rebuild_writes_a_path_only_row_for_a_not_local_note`, `freeing_space_with_mtime_and_size_unchanged_keeps_the_indexed_content`, `a_remote_edit_to_a_not_local_note_demotes_it_to_path_only`, and `a_note_becoming_local_is_reindexed_even_with_mtime_and_size_unchanged`.
  - `serat-app`: `the_asset_scheme_404s_a_not_local_file_without_opening_it` and `read_file_refuses_not_local_and_download_file_then_read_file_succeeds`.
  - A mutant that makes `Vault::read` hydrate must fail the serat-vault refusal test.
- **On a real Windows machine with OneDrive** (ignored in CI, `SERAT_ONEDRIVE_PLACEHOLDER_VAULT`):
  - #77's `stat_paths_do_not_hydrate_a_placeholder`, unchanged.
  - #77's `reading_a_placeholder_is_reported_not_silent`, **renamed** `a_background_read_of_a_placeholder_is_refused_and_does_not_hydrate`, and tightened to assert `Err(NotLocal)` **and** that the attributes are unchanged.
  - `hydrate_downloads_that_file_and_no_other`, on a folder with two placeholders.
  - `a_rebuild_of_a_placeholder_vault_downloads_nothing`.
  - These wait on the Windows runner (#77) and a machine signed in to OneDrive. Until one runs them, the Windows half of this decision is a mechanism, not a verified control.
- **On a real Mac with iCloud Drive** and "Optimize Mac Storage", a manual run of the same four with `SERAT_ICLOUD_DATALESS_VAULT`. No runner exists; the gate protocol's manual week is where it happens.
