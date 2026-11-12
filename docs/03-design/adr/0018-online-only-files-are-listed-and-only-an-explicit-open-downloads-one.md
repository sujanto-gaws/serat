# ADR-0018: Online-only files are listed, and only an explicit open downloads one
**Status:** proposed — the rule is **P2, Must**, with a **P1 interim** (§P1 interim). The owner has confirmed that the P1 gate vault is local only, so the rule stays in P2. That answer covers the primary machine. A synced replica of the real vault under OneDrive or iCloud (p1-gate-protocol.md counts any replica as the real vault) would reopen the P1 question; that is RISK-015's trigger. It stays proposed until rust-core-dev, tauri-app-dev, vue-frontend-dev and test-engineer have reviewed it (§Objections recorded) · **Date:** 2026-11-12 · **Serves:** FR-VLT-12, FR-VLT-01, FR-VLT-02, FR-VLT-04, NFR-CMP-01, NFR-PRF-01, NFR-PRF-04, NFR-SEC-02, NFR-SEC-06
**Honours:** ADR-0002 (rebuild, not migrate), ADR-0011 (the watcher path), ADR-0015 (the tree as complete directory listings; §2's catch-up is amended here), ADR-0016 (saves are patches against a base), ADR-0017 (the asset scheme); threat model T-07; assumption A-03 (sync is external)
**Round 4:** the full clippy list in serat-vault and serat-app, vault-root ignore files ruled, `write_atomic`'s expect check classified. **Round 3:** one byte-reading gate enforced by clippy (B-4), no download on the webview main thread (B-5), chunked `hydrate` for assets, classified editor arms, and RISK-015's replica trigger. **Round 2 (task #134 review):** the P2/P1 split; demote on free (B-2); locality tested before catch-up candidacy (B-3); no stat added to FR-VLT-01's no-index arm (B-1); the explicit-open table; `NotLocal` arms; the benches to re-run; legacy `.icloud` stubs moved to the exclusions.

## Context

A-03 says users sync their vault with git, Syncthing or a cloud folder. The cloud clients that matter most do not keep every file on the disk:
- **OneDrive Files-On-Demand** (Windows) leaves "online-only" files as placeholders: reparse points with `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` (`0x0040_0000`), and sometimes `RECALL_ON_OPEN` (`0x0004_0000`) or `OFFLINE` (`0x0000_1000`).
- **iCloud Drive and Dropbox on macOS** (File Provider, "Optimize Mac Storage") leave *dataless* files and directories, flagged `SF_DATALESS` (`0x4000_0000`) in `st_flags`.
- **Older iCloud Drive** replaces a file with a hidden stub, `.<name>.icloud`.

In every case the name, size and mtime are on disk and the bytes are not. Reading the bytes makes the operating system's sync client download them. That is slow, fails offline, can cost money on a metered link, and fills the disk the user freed on purpose.

**Today Serat downloads silently, from every read path.** The rebuild's `NoteSource::load` downloads, and so do the catch-up's hash of a candidate, the watcher's hash gate, `read_file`, editor reconciliation and the asset scheme's `open_verified`. The scheme follows reparse points on Windows on purpose (the comment at `crates/serat-app/src/asset.rs` lines 229–231). Over a 20,000-note vault that is mostly online-only, one rebuild downloads the whole vault. Task #77 wrote two ignored Windows tests for this, in crates/serat-app/tests/windows_vault.rs *(planned)* (`task/p1-77-windows-ci-prep` @ `ab502b6`, not yet on integration). The second, `reading_a_placeholder_is_reported_not_silent`, is expected to fail because no ADR said what correct is.

## Options considered

1. **Read transparently, downloading on demand (today; Obsidian's behaviour).** Rejected as the end state. Background work would download files the user never asked for, in bulk:
   - NFR-PRF-04 becomes network-bound.
   - Serat becomes the cause of network traffic the user did not ask for, against NFR-SEC-02's intent.
   - It fights the sync client: OneDrive frees a file, the watcher sees the attribute change, re-reads it, and downloads it again.

   **Kept as the P1 interim**, with that consequence recorded as RISK-015.
2. **Refuse online-only files like refused names**, counting them but not listing them. Rejected. An online-only note is a real note the user can see in Explorer or Finder. Hiding it breaks links to it (NFR-CMP-01) and the quick switcher, against goal 5.
3. **A reported "not local" state.** Online-only files are listed, linkable and findable by name, and indexed by path only. Background work never reads them, and an explicit open of one file downloads that file. **Chosen, for P2.**

## Decision

### P1 interim

**In P1, background work may download.** Nothing below §P2 rule is implemented in P1. The consequences are recorded as **RISK-015**:
- A rebuild over an online-only vault is network-bound and downloads the vault.
- The watcher can re-download a file the user has just freed.
- A not-local directory makes the tree walk fetch its listing over the network.

Two pieces ship in P1 because they cost little and stop the worst outcome:
1. **A failed or interrupted read is an error, never zero bytes.** This is a goal-1 invariant: a patch or a diff built on an empty "read" would truncate the note. `Vault::read` returns `Err` when the read fails part-way, and never `Ok` with what it had read. The save path and reconciliation never act on an `Err` as though it were content. Tests are in §Tested against.
2. **`serat_core::locality::is_local(platform, raw)`**, the pure predicate of §P2 rule item 1, with unit tests and no callers yet. The P2 work then starts from a tested function, and the predicate's facts are settled while #77's runner is being stood up.

### P2 rule

**One rule: an online-only file is downloaded only by an explicit open of that one file by the user (item 5), and by nothing else.**

1. **Detection is a pure function of a stat Serat already holds**, `is_local(platform, raw)`.
   - **Windows:** a file is not local if `MetadataExt::file_attributes()` intersects `RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE`. The attributes come from `DirEntry::metadata()`, which is the `FindNextFileW` data and opens nothing. They can also come from `symlink_metadata`, which opens the file with access 0 and does not download it. **Including `OFFLINE` also catches HSM storage and Offline Files (client-side caching).** That is intended: their bytes are not local either.
   - **macOS:** a file is not local if `std::os::macos::fs::MetadataExt::st_flags() & SF_DATALESS`, from an `lstat`. macOS has no `statx`.
   - **Linux:** always local. FUSE clients (rclone, onedriver) download inside `read` and give no signal (§PRD impact, assumption).
   - **To measure on the #77 runner:** whether the Windows placeholder-compatibility ("disguise") mode hides these attributes from Serat's process. If it does, detection fails open, and Serat behaves as the P1 interim does on that machine.
2. **Where the stat comes from, path by path (B-1).** No path may pay a stat it does not pay today.
   - **`Vault::walk`** (the rebuild, and the catch-up's walk) already calls `entry.metadata()` (`crates/serat-vault/src/walk.rs` line 77), so locality is free there.
   - **`walk_tree` with metadata** (the warm arm, which feeds the catch-up) already stats, so it is free there too.
   - **`walk_tree` without metadata** (the no-index arm, `crates/serat-vault/src/tree.rs` around lines 220–230) takes no stat; it uses the directory entry's type only. That is FR-VLT-01's timed arm. **Ruled: `TreeEntry.local` is `None` ("not yet known") on that arm on every platform.** It is filled by a tree update when the rebuild's `Vault::walk` reaches the file. Paying an `lstat` per file on macOS to fill it early was rejected, because it moves FR-VLT-01's no-index number. Windows could fill it for free, but one rule on every platform is one test.
   - **`Vault::resolve` returns the final component's metadata** alongside the path. It already takes an `lstat` of every component (`crates/serat-vault/src/lib.rs` around line 298), so `read`, `stat` and the scheme reuse that result instead of taking another.
3. **Not-local directories** are a `RECALL_ON_OPEN` directory on Windows or a dataless directory on macOS. `read_dir` on one makes the sync client fetch the listing over the network, inside the timed walk. **Ruled: the predicate applies to files only.** Directories are listed as today, because not descending would hide every note beneath them and break links (option 2's failure). The cost (a walk that is network-bound on such a vault, so FR-VLT-01's number does not hold there) is part of RISK-015 and is measured on the #77 runner.
4. **Reading.** `Vault::read` refuses a not-local file with `VaultError::NotLocal { path }`. It tests the metadata `resolve` returned **before** opening. On Windows it tests **again on the opened handle's metadata, before reading any byte**: `read_hooked` already calls `file.metadata()` after the open (`crates/serat-vault/src/lib.rs` around line 415). Opening a `RECALL_ON_DATA_ACCESS` file does not download it; the first read does. So the second test closes the race for those files. A `RECALL_ON_OPEN` file downloads on open, so for it the race between `resolve` and `open` remains. Losing it costs one download.

   **One private gate for every byte serat-vault reads (B-4).** "Who downloads" can only be a grep if nothing else reads. Today serat-vault reads bytes directly at these sites:
   - `crates/serat-vault/src/lib.rs`: `reload_ignore_file` (around line 174), `write_atomic`'s expect check (around 571), `write_patch`'s base read (around 717), its re-read (around 757), `diff_against_disk` (around 805) and `read_hooked` (around 415);
   - `crates/serat-vault/src/gitignore.rs` around line 240;
   - and outside the crate, the asset scheme's `open_verified` and `read_span` in `crates/serat-app/src/asset.rs`.

   **Ruled:**
   - **The gate.** Every byte read in serat-vault goes through one private gate, `open_bytes(path, Hydrate::{No, Yes})`. It does the `resolve` metadata check, the open, the handle re-test and the read.
   - **Two public wrappers.** `Vault::read` is `Hydrate::No`. `Vault::read_hydrating` is `Hydrate::Yes`, and it is the only public function that downloads.
   - **A chunked variant.** `Vault::hydrate(path) -> Result<()>` is the gate's chunked-discard variant, for an asset that must be downloaded but not loaded whole (SF-8).
   - **The asset scheme** opens through a serat-vault function built on the same gate. It keeps its own containment re-check, but not its own `File::open`.
   - **Mechanical enforcement (SF-11).** These go into `disallowed-methods`:
     - `std::fs::read`, `std::fs::read_to_string`, `std::fs::File::open` and `std::fs::OpenOptions::open`, because `asset.rs` around line 222 opens for reading through `OpenOptions`;
     - `std::fs::copy`, which reads its source.

     In `crates/serat-vault/clippy.toml` an `#[allow]` sits on three places only: the gate, `atomic.rs`'s write-only temp-file open (around line 37), and `identity.rs`'s directory handle (around line 48), which opens a directory and reads no bytes. **The same entries go into a new crates/serat-app/clippy.toml *(planned)*.** There the only `#[allow]` is on the app-data reads in `settings.rs`, which read Serat's own settings outside the vault. So the asset scheme cannot later reopen a vault file past the gate. The precedent is `canonicalize` (task #107).
   - **Files at the vault root are not vault-local (SF-12).** `.seratignore` and `.gitignore` live at the vault root and sync like any note, so they can be online-only. Both go through the gate with `Hydrate::No`:
     - **`.seratignore` online-only reads as "no user patterns"**, the existing missing-file rule, and is never downloaded. *Consequence:* folders it would ignore are walked and indexed until the file becomes local. That is visible, wasteful and recoverable, and it never loses data.
     - **What reloads it.** It is reloaded at every open. The watcher also reports it: any event on `.seratignore`, including the attribute change when it becomes local, reloads it after the coalescing window (architecture §3.1.2, *What the walk and the watcher refuse to index*). If the effective pattern set changed, the reload raises the existing `RescanRequired`.
     - **The gitignore offer treats `NotLocal` as a refusal, never as "absent".** `gitignore_offer_status` reports the offer as unavailable, and `append_gitignore_entry` refuses with `NotLocal`. Reading an online-only `.gitignore` as absent would make the append write a fresh `.gitignore` over the user's real one (goal 1). Test: `an_online_only_gitignore_is_never_replaced`.
   - **`write_atomic`'s expect check (`lib.rs` around line 571) is `Hydrate::No` (SF-13).** An existing not-local target answers "exists". Under `Expect::Absent`, and under `Expect::Hash`, it fails with `StaleHash`, with `found` empty because the bytes are unknown, and it is never read. The caller re-reads through its own explicit path if it has one.
   - The gate goes through `resolve` and the ignored-directory check (NFR-SEC-06, T-07), so `read_hydrating` and `hydrate` cannot target `.serat/` or a path outside the root.
   - A failed download is `Err`, never zero bytes (the P1 piece).
   - **To measure on the #77 runner:** `FILE_FLAG_OPEN_NO_RECALL` (`0x0010_0000`, through `OpenOptionsExt::custom_flags`, no `unsafe`) would stop any open from recalling the file and close the race entirely. macOS `setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, …)` would do the same for a process, but it needs FFI and the workspace forbids `unsafe`. It is listed and not chosen.
5. **What counts as an explicit open.** One user action, now, on one named file:

   | Action | Downloads? |
   |---|---|
   | Open a note from the tree, the switcher, a followed link, a search result or a backlink, focused or not (including open in a new tab without focus) | Yes |
   | Open a PDF or image in a viewer | Yes |
   | Click **Download** on a not-local embed, transclusion or preview | Yes |
   | Save a note open in a pane whose file was freed meanwhile (ADR-0016's base check) | Yes. The pane is an explicit open. |
   | Reconcile a remote edit to a note open in a pane | Yes, **only when mtime or size changed**. `request_external_diff` first takes a stat (locality, mtime, size) against the pane's base, **before** `diff_against_disk`, which reads the file itself (`lib.rs` around line 805) ahead of the `editor.rs` read around line 344. A file freed with mtime and size unchanged has nothing to reconcile and is not read. |
   | Export or print one note at the user's request (P2+) | Yes. Exporting a folder or vault does not download; it reports the not-local files it skipped. |
   | Tabs and panes restored at launch, the focused one included | **No.** The pane shows "Stored online only" with **Download**. Restoring is not an action taken now, and a restored session can name many files. |
   | `![[note]]` transclusion, image embeds, hover and link previews | No. Each shows a not-local placeholder with **Download**. |
   | Copy path, rename, move, delete | No. None needs the bytes. |
   | Rebuild, catch-up, watcher, backlinks or search snippets, the asset scheme serving an `<img>` | No |
   | Any agent or MCP read (P2) | No. It gets `NotLocal`. The user opens the file, and C-05's diff review is never shown a diff built on bytes Serat did not have. |

6. **The index holds what a rebuild would produce from the disk now (B-2).** That is FR-VLT-02's "identical results after rebuild", and C-02's "nothing depends on state only in `.serat/`".
   - **A rebuild** gives a not-local note a `notes` row with path, file name, title from the file name, kind, mtime, size, **`local = 0`** and `hash = ''`, and no blocks, links or properties. An empty string never equals a BLAKE3 hex digest. The note stays in the tree, the switcher and link resolution, where it is an existing target, and is absent from full-text search.
   - **Freeing space demotes the row.** When the watcher or the catch-up finds an indexed note whose file is now not local, it sets `local = 0` and `hash = ''`, drops the note's blocks and reads nothing. Keeping the old content was rejected: it would be searchable only because it lives in `.serat/`, which breaks FR-VLT-02 and C-02. Demoting reads nothing, so it re-downloads nothing.
   - **A remote edit to a not-local file** (mtime or size moved) is the same demotion. The row is already path-only, so only mtime and size change.
   - **A file becoming local** (the user opened it, or downloaded it in Explorer or Finder) is read and indexed even when mtime and size are unchanged, because its row says `local = 0`.
   - **Schema:** `local INTEGER NOT NULL CHECK (local IN (0, 1))` and `CHECK (local = 1 OR hash = '')`. Under ADR-0002 this bumps `user_version` and rebuilds. The implementing task writes the DDL in `schema/001_init.sql`.
7. **Locality is tested before catch-up candidacy, and a not-local file is never hashed (B-3).** ADR-0015 §2's racy window makes every file with mtime ≥ `caught_up_from_ms − 2000` a candidate "whatever the comparison says", and candidates are hashed, which is a read and so a download. **Amended:**
   - The catch-up tests locality first, from the walk's stat.
   - A not-local file is never a candidate and is never hashed.
   - A not-local file whose row has `local = 1` is demoted, and this includes one inside the racy window.
   - A local file whose row has `local = 0` is a candidate whatever mtime and size say.

   The watcher's hash check (architecture §3.1.2 step 3) takes the same stat first. ADR-0015 §2, architecture §3.1.1 item 6 and §3.1.2 steps 1 and 3 carry "Amended by ADR-0018", and so does `CatchUp::PathsAndMtime`'s doc comment.
8. **Every `Vault::read` consumer has an explicit `NotLocal` arm (SF-5).** Today any read error other than `NotFound` becomes `Step::Gone { not_found: false }`, "unreadable, row kept" (`crates/serat-index/src/external.rs` around lines 200–204 and 412). Under that, a rebuild would skip a not-local file instead of writing its path-only row. So:
   - `submit` (the rebuild and the watcher) maps `NotLocal` to the path-only row or the demotion of item 6;
   - `note_opened` (`crates/serat-app/src/editor.rs` around line 219) uses `Hydrate::No`. A pane restored at launch on a not-local file is accepted, **registers no base and reads nothing**, and shows item 5's "Stored online only" state. The pane registers its base when the user downloads the file.
   - `stale_against_disk` (around line 324) uses `Hydrate::No`, and `NotLocal` maps to `IpcError::NotLocal`. A stale-hash answer must not download a file to report a hash.
   - `rebase_buffer` (around line 392, "Keep mine") is explicit, so it uses `Hydrate::Yes`.
   - The editor's save, through `write_patch`'s base read (`lib.rs` around line 717) and its re-read (around 757), is explicit (item 5), so it uses `Hydrate::Yes`.
   - The reconciliation read after item 5's stat uses `Hydrate::Yes`.
   - The scheme answers 404, and `read_asset_range` answers `IpcError::NotLocal`.

   Each match on `VaultError` names `NotLocal`, with no `_` arm hiding it.
9. **The asset scheme never downloads.** A not-local file gets the scheme's ordinary 404, decided from `resolve`'s metadata before `open_verified` opens anything. On Windows it is re-tested on the handle's metadata (`asset.rs`, the `file.metadata()` after the open) before any byte is read. **This is not a security control.** Losing the race costs one download, and containment stays with ADR-0017 step 7's check on the handle's real path (T-07). A cloud placeholder is not a link: on Windows, Rust's `is_symlink` is true only for name-surrogate reparse tags (symlinks, junctions), and the cloud tags are not name surrogates.
10. **The boundary**, changed by the implementing task:
    - `ReadFileArgs::hydrate: bool` (absent means `false`) reaches `Vault::read_hydrating`.
    - `AssetArgs::hydrate: bool` reaches `Vault::hydrate`, the chunked-discard variant, so `asset_info` never loads a 100 MB PDF to download it (SF-8). `AssetArgs` is `asset_info`'s argument only; `read_asset_range` has its own type. Its doc comment (`contracts/ipc.rs` around line 1636, "Arguments to `asset_info` and `read_asset_range`'s path") is reworded when `hydrate` is added.
    - No separate download command exists.
    - **Nothing downloads on the webview main thread (B-5, goal 2).** `read_file` is today a sync `#[tauri::command]` (`crates/serat-app/src/commands.rs` around lines 52–53), so it runs on the main thread. Task #49 makes it async through `offload::blocking` (on #49's branch, `task/p1-49-io-off-main`; merging now), as `asset_info` already is through `blocking` (commands.rs around lines 389–411). `ReadFileArgs::hydrate` must not land before #49. Hydrating work inside `save_patch` and `request_external_diff` runs on the blocking pool too, as commands.rs requires around lines 149–152.
    - `IpcError::NotLocal { path }`.
    - `local: Option<bool>` on `TreeEntry` (`None` on the no-index arm, item 2) and `local: bool` on `AssetInfo` and the switcher's entries.
    - `IndexStatus::not_local: Option<u32>`, a count under the same rules as `refused_names`.
    - No ACL change, because both commands are already granted.

    The front end classifies actions by item 5. A pane shows "Downloading from OneDrive…" (or iCloud or Dropbox) as a non-blocking state, never a modal. Offline, it shows "This file is stored online only and could not be downloaded" with Retry.
11. **The user is told.** The status strip shows "N files online only" beside "names not shown", with a list behind it, using the refused-names panel's pattern and cap. Tree rows carry a cloud glyph. frontend-designer writes the copy and the glyph.

**Excluded: legacy iCloud `.icloud` stubs.** They are hidden dotfiles and stay ignored, as today (FR-VLT-07). Serat does not list `<name>` from a stub, because that would put a phantom path in the tree and the index, and it does not download them. P2 may count them ("N files iCloud has not downloaded") and nothing more.

## Consequences

**Positive (P2).**
- A vault that is mostly online-only opens and rebuilds without downloading a byte. NFR-PRF-04 can only improve, because unread notes are not parsed.
- FR-VLT-01's no-index arm takes no new stat (item 2).
- The sync client's "free up space" is respected.
- Links and the switcher keep working (NFR-CMP-01).
- The index stays exactly what a rebuild would produce (FR-VLT-02, C-02).
- "Who downloads" is one function and one table.

**Negative.**
- Search, backlinks *from* a not-local note, and properties are incomplete until the user downloads that note. The count is the only thing that says why.
- Every feature that touches bytes has to be placed in item 5's table.
- There is a schema change and a rebuild.

**Benches the implementing tasks re-run (SF-6):**
- **FR-VLT-01, both arms.** The walk's entry type gains `local`, and the warm arm reads one more field from a stat it already has.
- **NFR-PRF-04.** `resolve` now returns metadata, and the read gains a branch.
- **FR-VLT-04.** The watcher takes a locality check before its hash.

**Risks.** RISK-015, for the P1 interim and for item 3's directories. The race in item 4 is closed for `RECALL_ON_DATA_ACCESS` and open for `RECALL_ON_OPEN` unless `FILE_FLAG_OPEN_NO_RECALL` measures well. Whether OneDrive and iCloud raise watcher events when a file is freed or downloaded is unmeasured (architecture §Open design questions).

## PRD impact (for product-analyst)

**Allocated in PRD v1.11 (task #142):** FR-VLT-12, "Online-only files are not downloaded silently" (Must, P2); FR-VLT-13, legacy iCloud stubs (Won't); assumption A-09; and story US-43. The proposals below are kept as the record of what was asked for.

No requirement covers this today. **Proposed, for product-analyst to word and number:**
- **A new functional requirement, FR-VLT at the next free ID, Must, P2**, verified by Test. P2 because the owner confirmed the P1 gate vault is local only:
  - *Requirement:* "The system shall not cause a cloud-synced file whose contents are not stored locally to be downloaded, except when the user opens that file. Such files shall remain listed and linkable and shall be reported as not stored locally."
  - *Acceptance:* given a vault in which some notes are online-only, when it is opened, rebuilt or watched, then no online-only file is downloaded, and each is listed, linkable and counted; when the user opens one, that file alone is downloaded and opened.
- **For P1, a risk entry rather than a requirement.** RISK-015 in `docs/01-planning/risk-register.md` records that background work may download in P1, with its consequences. product-analyst may also add one clause to NFR-REL-01's fault-injection suite for the P1 piece: a failed or interrupted read is an error and never empty content.
- **A new assumption:** Linux FUSE cloud clients download inside `read` and give no signal, so the requirement holds on Windows and macOS only.
- **An exclusion:** Serat neither lists nor downloads legacy iCloud `.icloud` stubs.
- **No budget changes.** The bench vault has no placeholders. The three re-runs above confirm that nothing regressed.

A story to match, if wanted: *"As a user whose vault lives in OneDrive with Files-On-Demand, I want Serat to open my vault without downloading it, so that my freed disk space and my metered connection are respected."*

**Implementing tasks, for lead to file.**
- **P1, rust-core-dev, serat-core and serat-vault:** `locality::is_local` and its tests, and the failed-read test.
- **P2, rust-core-dev, serat-vault:** `resolve` returning metadata, `NotLocal`, the handle re-test, `read_hydrating` and the test seam.
- **P2, rust-core-dev, serat-index:** the `local` column and DDL, path-only rows, demotion, the catch-up and watcher rules of item 7, and the `NotLocal` arms.
- **P2, tauri-app-dev:** the `hydrate` arguments, the scheme's 404 and handle re-test, `NotLocal`, the contract fields and the count.
- **P2, vue-frontend-dev:** item 5's classification, the pane states and the count. frontend-designer writes the copy and the glyph.
- **test-engineer, with the #77 runner:** the real-machine runs, the disguise-mode, `FILE_FLAG_OPEN_NO_RECALL` and watcher-event measurements, and the three bench re-runs.

## Objections recorded

The first round of review came from code-reviewer through lead: three BLOCKING findings, seven SHOULD-FIX and five NIT. All are taken in round 2, as the Round 2 line in the header lists. **Teammates still to review:**
- **rust-core-dev:** `read_hydrating`, `resolve` returning metadata, the schema column, and the catch-up and watcher rules.
- **tauri-app-dev:** the `hydrate` arguments, the scheme's 404 and the contract fields.
- **vue-frontend-dev:** item 5's table.
- **test-engineer:** whether the seam and the real-machine runs are enough.

Their objections are recorded here when given.

## Tested against

**#77's case**, supplied by tauri-app-dev: one online-only `placeholder.md` in a OneDrive folder. Under the P2 rule:
- stat paths must not download it, which is unchanged and still to be proven;
- a background read must fail with `NotLocal` and leave it online-only. That is stronger than #77's "refused **or** reported", because it rules out "downloaded, then reported".

**P1 tests:**
- `serat-core`: `is_local` cases for each Windows attribute bit, alone and combined, and for `SF_DATALESS`, each evaluated on every platform through `Platform`. Linux is always local.
- `serat-vault`: `a_failed_or_interrupted_read_is_an_error_never_zero_bytes`, through a test reader that fails after N bytes; `Vault::read` must return `Err` and not `Ok` with a prefix.
- `serat-app`: `a_save_whose_base_read_fails_writes_nothing`, which checks that the note's bytes are unchanged.
- On the #77 runner (ignored in CI): `an_offline_placeholder_read_is_an_error_not_an_empty_note`.

**P2 tests**, in CI through a `#[cfg(test)]` locality override in serat-vault that marks chosen paths not local, so they run on Linux:
- `serat-vault`:
  - `a_not_local_file_is_listed_by_the_walk_and_refused_by_read`;
  - `read_hydrating_is_the_only_read_that_accepts_a_not_local_file`;
  - `read_hydrating_refuses_dot_serat_and_paths_outside_the_root`;
  - `the_no_index_tree_walk_takes_no_stat_and_reports_local_unknown`. Its hook is a counted `DirEntry::metadata` wrapper, with `std::fs::DirEntry::metadata` added to serat-vault's `disallowed-methods` and one `#[allow]` on the wrapper, as `canonicalize_counted` is done. The test asserts zero calls on the no-index arm.
  - `every_byte_read_goes_through_the_gate`. **The control is clippy's `disallowed-methods`** in serat-vault and serat-app. This test is evidence, not the control: it checks both modes, `Hydrate::No` refusing and `Hydrate::Yes` reading, through each listed site's caller.
  - `an_online_only_gitignore_is_never_replaced` and `write_atomic_against_a_not_local_target_is_stale_and_reads_nothing`.
- `serat-index`:
  - `a_rebuild_produces_exactly_one_path_only_row`;
  - `freeing_space_demotes_the_row_and_reads_nothing`;
  - `a_remote_edit_to_a_not_local_note_stays_path_only`;
  - `a_note_becoming_local_is_reindexed_even_with_mtime_and_size_unchanged`;
  - `a_not_local_file_in_the_racy_window_is_demoted_never_hashed`;
  - `the_index_after_any_locality_change_equals_a_rebuild`.
- `serat-app`:
  - `a_restored_pane_registers_no_base_and_reads_nothing`;
  - `the_asset_scheme_404s_a_not_local_file_without_reading_it`;
  - `read_file_refuses_not_local_unless_hydrate_is_set`;
  - `a_restored_pane_does_not_download`.
- A mutant that makes `Vault::read` download must fail the serat-vault refusal test.
- **On a real Windows machine with OneDrive** (ignored in CI, `SERAT_ONEDRIVE_PLACEHOLDER_VAULT`, on the #77 runner). Until one runs them, the Windows half is a mechanism, not a verified control.
  - #77's `stat_paths_do_not_hydrate_a_placeholder`, unchanged.
  - #77's `reading_a_placeholder_is_reported_not_silent`, **renamed** `a_background_read_of_a_placeholder_is_refused_and_does_not_hydrate`, and asserting `Err(NotLocal)` **and** unchanged attributes.
  - `read_hydrating_downloads_that_file_and_no_other`.
  - `a_rebuild_of_a_placeholder_vault_downloads_nothing`.
- **On a real Mac with iCloud Drive** and "Optimize Mac Storage": the same four, run manually with `SERAT_ICLOUD_DATALESS_VAULT`.
