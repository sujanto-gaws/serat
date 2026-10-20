//! Serat IPC contract — P0, plus the P1 subset (task #7).
//!
//! Serves:  FR-VLT-01, FR-VLT-02, FR-VLT-03, NFR-PRF-01, NFR-MNT-01, NFR-REL-02;
//!          P1: FR-VLT-04, FR-VLT-05, FR-VLT-08, FR-NAV-01, FR-NAV-02, FR-NAV-04,
//!          FR-NAV-06, FR-STR-03, FR-EDT-03, FR-EDT-10, FR-BIN-01, FR-BIN-02,
//!          FR-BIN-03, FR-BIN-11, FR-BIN-12, NFR-CMP-04, NFR-USE-01
//! Honours: ADR-0007 (generated TypeScript from Rust), ADR-0002 (rebuild, never
//!          migrate), ADR-0001 (the index is reached through one actor; no
//!          command handler ever opens it)
//! Design:  docs/03-design/architecture.md §2.2, §3.2, §3.4, §6.1;
//!          schema/001_init.sql (`index_meta`); threat-model.md T-06
//!
//! This is the contract between `crates/serat-app` and `app/`, not a sketch.
//! tauri-app-dev implements exactly these commands and emits exactly these
//! events; the TypeScript is generated from these types by `ts-rs` (ADR-0007)
//! and never hand-written, and CI fails if the committed output is stale
//! (NFR-MNT-01).
//!
//! # Where this file lives
//!
//! It is the design artifact. `serat-app` takes it as its `ipc` module — by
//! `#[path = "../../../contracts/ipc.rs"] mod ipc;` or by moving it into the
//! crate and leaving a pointer here; tauri-app-dev's call, because the wiring is
//! cheap to change and the shape is not. It compiles anywhere `serde` and
//! `ts-rs` are available and depends on nothing else, deliberately: a contract
//! that drags in the implementation is not a contract.
//!
//! `serat-app` will need, in its `Cargo.toml`:
//!
//! ```toml
//! serde = { workspace = true }
//! ts-rs = "12"
//! ```
//!
//! # Generating the TypeScript
//!
//! `#[ts(export)]` writes each type when the crate's tests run, so the generator
//! is `cargo test -p serat-app` and the destination is set once, by environment,
//! rather than repeated on every type:
//!
//! ```sh
//! TS_RS_EXPORT_DIR=../app/src/ipc/generated cargo test -p serat-contracts
//! ```
//!
//! The path is relative to the CRATE directory, not the workspace root — which
//! is why it starts with `../`, and is worth knowing because the version without
//! it silently writes a whole tree to `contracts/app/` and reports success.
//!
//! The output is committed, and CI regenerates and fails on a diff — which is
//! the whole of NFR-MNT-01 and the reason ADR-0007 chose generation over
//! agreement. This was verified against ts-rs 12.0.1: all ten types export, the
//! tagged enums become discriminated unions on `kind`, and the doc comments here
//! arrive in the `.ts` files as TSDoc, so the front end reads the same rules
//! this file states without anyone copying them.
//!
//! # What P0 has
//!
//! Open a vault, read one file, write one file, and know what the index is
//! doing. Four commands and one event. That is the whole surface.
//!
//! # What P1 adds (task #7) — the section headed "P1" at the end of this file
//!
//! | Surface | Commands / channels / events | Design |
//! |---|---|---|
//! | File tree | `open_tree` + `Channel<TreeMessage>` (streaming), `mark_tree` | ADR-0015, §3.5 |
//! | Creating notes and folders | `write_file` with `base_hash: None` (a note), `create_folder` (a folder) | FR-VLT-11, ADR-0016, T-07 |
//! | Choosing a vault | `choose_vault` (native dialog, no path parameter) | T-08 |
//! | Editor buffers | `note_opened`, `note_closed`, `save_patch`, `request_external_diff`, `rebase_buffer`, event `note://external-change` | ADR-0016, §3.3 |
//! | Invalidation | event `index://changed` (throttled, invalidation only) | §3.1 step 6 |
//! | Switcher + link completion | `switcher_query` | §9.2 |
//! | Search, property lookup | `search` | §4.5 R1/R3 |
//! | Backlinks | `backlinks` | §4.5 R2 |
//! | Links in the editor | `resolve_links` (batched per viewport), `note_anchors` | §9.3 |
//! | Assets | `asset_info`; bytes via the `serat-asset` scheme (not a command); fallback `read_asset_range` (raw bytes) | §7.4, ADR-0017 |
//! | Settings, command registry | `get_ui_settings`, `set_live_preview`, `set_keybinding` | §9.3, FR-PLT-05 |
//! | `.serat/` ignore offer | `gitignore_offer_status`, `append_gitignore_entry`, `decline_gitignore_offer` | NFR-CMP-04, T-06 |
//!
//! `write_file` stays, and in P1 its **only** job is creating a note: the editor
//! saves through `save_patch` (ADR-0016). Extraction state (`ExtractionState`) is
//! P2's, with extraction.
//!
//! **Streaming**, stated per surface because the front end must treat them
//! differently: the tree is an ordered, lossless per-session `Channel`;
//! `Channel::send` never blocks, so the transport guarantees ordering and
//! nothing else. What bounds the backlog is ADR-0015 §2's batching (at most
//! 256 KiB or 16 ms of walking per message), not the channel;
//! `note://external-change` is an event (unthrottled, unordered against command
//! responses — ADR-0016's ordering rule is what makes that safe);
//! `index://changed` and `index://status` are throttled and are for invalidation,
//! never for establishing that something happened (§3.4). Everything else is
//! request/response.
//!
//! **ACL (ADR-0012).** Every P1 command above is granted to the `main` window in
//! `capabilities/default.json`, and the app manifest names them all. `open_vault`
//! stays withheld: a webview must not move the boundary T-07 guards, which is why
//! a user-initiated open is `choose_vault`, whose only input is the user's choice
//! in a native dialog.
//!
//! P2 adds the query surface — `run_query`, streamed rows, cell anchors and
//! write-back; P4 the proposal queue. None of them changes a type below. They
//! are added beside them.
//!
//! # Five rules that hold for every command here
//!
//! 1. **No command blocks on the index.** Reading and writing a file is a vault
//!    operation and completes whatever the index is doing (design goal 2). A
//!    command that needs the index returns the current `IndexState` instead of
//!    waiting; in P0 no command needs it, which is why none of them can fail
//!    because of it.
//! 2. **Every path crossing this boundary is vault-relative, `/`-separated and
//!    canonical**, the `VaultPath` of `serat-core`. Absolute paths, `..`
//!    segments and symlinks leaving the vault are refused with
//!    `IpcError::PathEscapesVault` before any filesystem call. The one
//!    exception is `OpenVaultArgs::root`, which is a host path by necessity.
//! 3. **Only Markdown files are writable**, and "Markdown" means exactly what
//!    `serat_core::VaultPath::is_markdown()` means — `.md` or `.markdown`.
//!    Anything else is `IpcError::NotWritable` — FR-BIN-19 and threat T-06.
//!    Enforced at the boundary because this is where an agent, an MCP client or
//!    a future feature would otherwise reach past it. Deferring to that one
//!    function rather than restating the rule is deliberate: an earlier draft
//!    said "`.md`", which would have walked, indexed and read a `README.markdown`
//!    and then refused to save it — over-refusing breaks C-01's plain-Markdown
//!    promise just as surely as under-refusing breaks T-06.
//! 4. **A write is optimistically locked on the file hash** (§6.1). The caller
//!    passes the hash it read; a mismatch returns `IpcError::StaleHash` with
//!    the current hash. That is an expected outcome, not a fault, and the UI
//!    resolves it in place — no modal on the write path (§9.1).
//! 5. **Integers that cross this boundary are `number`, not `bigint`.** Tauri's
//!    IPC is JSON, so an `i64` arrives in JavaScript as a `number` whatever the
//!    Rust type says. `ts-rs` maps 64-bit integers to `bigint` by default, which
//!    type-checks and then fails at run time the first time anyone writes
//!    `file.mtime_ms > 0n`. Every 64-bit field here therefore carries
//!    `#[ts(type = "number")]`, which is accurate rather than a convenience: a
//!    millisecond timestamp is about 1.8e12 and a file size fits comfortably
//!    below `Number.MAX_SAFE_INTEGER` at 9.0e15. A future field that could
//!    exceed that must cross as a string, and saying so must not be left to
//!    whoever adds it.
//! 6. **Errors are a closed set.** `IpcError` is the only error type that
//!    crosses, and `serat-app` maps `serat_core::VaultError` onto it with an
//!    exhaustive `match` and no `_` arm, so a new variant in the core taxonomy
//!    is a compile error here rather than a `String` at the boundary. That
//!    property is the entire point of NFR-MNT-01.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ============================================================================
// Commands
// ============================================================================

/// Arguments to `open_vault`.
///
/// `open_vault(args: OpenVaultArgs) -> Result<VaultOpened, IpcError>`
///
/// Serves FR-VLT-01, FR-VLT-02, NFR-PRF-01.
///
/// Points the application at a folder. Returns as soon as the root is
/// canonicalised and readable. It does **not** wait for the walk, the index
/// open, or a rebuild; all three report over `INDEX_STATUS_EVENT`. This is the
/// contract that keeps the window inside its 400 ms budget (§3.4): the front end
/// renders the shell from `VaultOpened` alone, and nothing in that value is
/// proportional to the size of the vault.
///
/// **It does not wait for the open check either**, and the ordinary value of
/// `VaultOpened::index` is therefore `IndexState::Opening`. The check is only
/// 5-8 ms on a warm index (task #68), which is what makes waiting for it
/// tempting — but it is 5-8 ms *when the index is healthy*, and the cases that
/// take longer are
/// exactly the ones where the window must already be up. Rule 1 has no
/// exceptions, including small ones.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OpenVaultArgs {
    /// Absolute host path the user chose. The one absolute path in this file.
    pub root: String,
}

/// A vault is open. Everything else is still arriving.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VaultOpened {
    /// Canonicalised absolute root, for the title bar.
    pub root: String,
    /// Identifies this vault for the life of the session. Every other command is
    /// implicitly scoped to the open vault; the id exists so the front end can
    /// discard events that arrive after the user has switched away.
    ///
    /// Opaque, unique per open, and never parsed: the front end only ever
    /// compares it for equality. Its format is `serat-app`'s business and may
    /// change without this being a contract change.
    pub vault_id: String,
    /// What the index was found to be by the open check (§3.4), before any
    /// rebuild has made progress. `IndexState::Rebuilding` here is a normal
    /// outcome and not a failure (NFR-REL-02).
    pub index: IndexState,
}

/// Arguments to `read_file`.
///
/// `read_file(args: ReadFileArgs) -> Result<FileContents, IpcError>`
///
/// Serves FR-VLT-02. Reads the filesystem, never the index: the file is the
/// truth (design goal 1), and the index may be mid-rebuild or absent entirely.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReadFileArgs {
    /// Vault-relative, `/`-separated.
    pub path: String,
}

/// A file, as read from disk.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileContents {
    /// Vault-relative path, echoed back.
    pub path: String,
    /// UTF-8 contents. A file that is not valid UTF-8 returns
    /// `IpcError::NotUtf8` rather than being lossily decoded: a lossy read
    /// followed by a save silently rewrites the user's bytes, which design goal
    /// 1 and C-04 both forbid.
    pub text: String,
    /// BLAKE3 of the bytes on disk at the moment of the read, lowercase hex —
    /// the wire form of `serat_core::ContentHash`. The caller passes it back as
    /// `WriteFileArgs::base_hash`; it is the stale guard of §6.1.
    pub hash: String,
    /// Filesystem mtime in unix **milliseconds**, the unit and the name
    /// `serat_core::NoteMeta::mtime_ms` uses. One concept, one unit, one name
    /// across the boundary.
    #[ts(type = "number")]
    pub mtime_ms: i64,
    /// Byte length on disk. Not `text.len()`: the front end should not have to
    /// re-encode to learn how large the file was.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// Arguments to `write_file`.
///
/// `write_file(args: WriteFileArgs) -> Result<FileWritten, IpcError>`
///
/// Serves FR-VLT-02, FR-VLT-03. Writes by temp-file-and-rename (§6.1 step 5).
/// The write records **no** echo (§3.2): a hash is recorded only after a
/// successful submit to the index, and in P0 this command has no submit behind
/// it. So the watcher sees the rename, matches nothing, and indexes the note.
/// That is correct, and it puts the write on FR-VLT-04's 500ms path rather
/// than NFR-PRF-03's 50ms one — a save timed through this command is a
/// *watcher* number. NFR-PRF-03 measures the save path of §3.1.1, which
/// arrives with the direct submit in I5.
///
/// **From P1 this command only creates notes** (`base_hash: None`). Editor saves
/// go through `save_patch` (ADR-0016), because CodeMirror joins lines with `\n`
/// and a whole-document save would rewrite every line ending of a CRLF note.
/// `serat-app` refuses a `Some(base_hash)` here with `IpcError::NotWritable`
/// once `save_patch` lands, so the old path cannot be reached by accident. The
/// byte-range patcher of ADR-0008 (view write-back) arrives in P2 as another
/// separate command. This command never becomes either path — both patch a
/// range and this writes a document, and conflating them is how C-04 gets
/// broken by accident.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WriteFileArgs {
    /// Vault-relative, `/`-separated. Must end in `.md` (rule 3).
    pub path: String,
    /// The full new contents.
    pub text: String,
    /// The hash the caller last read. `None` asserts the file does not exist,
    /// which is how a new note is created — and why creating a note over an
    /// existing one is a `IpcError::StaleHash` rather than a silent overwrite.
    pub base_hash: Option<String>,
}

/// A file was written.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileWritten {
    /// Vault-relative path, echoed back.
    pub path: String,
    /// BLAKE3 of what is now on disk, lowercase hex. The caller keeps it as the
    /// next `base_hash`.
    pub hash: String,
    /// Filesystem mtime after the write, unix milliseconds. See
    /// `FileContents::mtime_ms`.
    #[ts(type = "number")]
    pub mtime_ms: i64,
    /// Byte length after the write.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// `index_status() -> IndexStatus`
///
/// Serves NFR-PRF-01, NFR-REL-02. A poll returning the same value the event
/// channel pushes, for a front end that subscribed late or reloaded. It never
/// blocks and never touches SQLite: it reads the status the actor last
/// published, which is what lets it be called from a render path.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IndexStatus {
    /// The vault this status belongs to. A front end that has switched vaults
    /// discards statuses that do not match.
    pub vault_id: String,
    /// What the index is doing.
    pub state: IndexState,
    /// Milliseconds since process start. The cold-start benchmark (NFR-PRF-01)
    /// reads this instead of timing an IPC round trip, so the number it reports
    /// is the application's, not the harness's.
    #[ts(type = "number")]
    pub since_process_start_ms: u64,
}

// ============================================================================
// Events
// ============================================================================

/// The one event channel P0 has. Its payload is `IndexStatus`.
///
/// Emitted on every state transition and, while rebuilding, throttled to ten per
/// second (§3.1 step 6). A rebuild that emitted per note would spend its time in
/// the IPC bridge instead of in the parser, which is the failure this throttle
/// exists to prevent rather than a politeness.
///
/// Naming: `<domain>://<event>`. §3.1 writes this event as `index.changed` in
/// prose; the transport name is `index://changed` when P1 adds it. tauri-app-dev
/// confirms the permitted character set against Tauri v2 at S1 and tells the
/// architect if `://` is not usable, in which case this constant changes and
/// nothing else does.
pub const INDEX_STATUS_EVENT: &str = "index://status";

// ============================================================================
// Index state
// ============================================================================

/// What the index is doing, in the terms the front end has to render.
///
/// The set is exhaustive and every member has a defined presentation (§3.4): a
/// quiet line in the status area, never a modal and never a blocking spinner.
/// Editing works in all of them, because editing reads and writes files and
/// never the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum IndexState {
    /// The open check has not finished. This is the **normal** value in the
    /// `VaultOpened` returned by `open_vault`, because that command does not
    /// wait for the check; the front end should render the shell in this state
    /// and expect a transition over `INDEX_STATUS_EVENT` a few milliseconds
    /// later. "Not known yet" is a state to render, not a null to interpret.
    Opening,

    /// Usable.
    Ready {
        /// `index_meta.note_count`, which the open check verified against
        /// `count(*) FROM notes`.
        note_count: u32,
    },

    /// Being built from the vault. Not an error, and the reason is shown,
    /// because "why is it rebuilding" is the first thing a user asks
    /// (NFR-REL-02, ADR-0002).
    Rebuilding {
        /// Why this rebuild started.
        reason: RebuildReason,
        /// Notes indexed so far.
        done: u32,
        /// Notes the walk has found so far. This grows while the walk runs, so
        /// a proportion computed from it can move backwards; render a count
        /// until `walk_complete` is true, and a bar afterwards.
        total: u32,
        /// Whether `total` has stopped moving.
        walk_complete: bool,
    },

    /// The index cannot be built here — the directory is read-only, the disk is
    /// full, or a rebuild has now failed twice in one session (§3.4). Search and
    /// queries are unavailable and the vault remains fully editable, which is
    /// design goal 1 doing its job. This is the one index state the user has to
    /// act on, so it carries what they need to act with.
    Unavailable {
        /// Operator-readable explanation.
        message: String,
        /// The index path that could not be built.
        path: String,
    },
}

/// Why a rebuild is running.
///
/// Every outcome of the open check in architecture.md §3.4 has a variant here,
/// plus `UserRequested`, which the check cannot produce. The correspondence is
/// coverage, not ordering — §3.4 reaches `Corrupt` at both its second and its
/// last step, so there is no order to share. If a variant is added here, that
/// section is what changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RebuildReason {
    /// No `.serat/index.db`: a first open, or the user deleted it (US-07).
    NoIndex,
    /// `PRAGMA user_version` is not the version this build writes. There is no
    /// migration path and never will be (ADR-0002); this is the whole of it.
    ///
    /// Named for the *index* rather than the schema, deliberately. The stamp is
    /// bumped when the DDL changes **or** when the indexer starts extracting
    /// something different from unchanged input, and a user told their "schema
    /// changed" when their parser improved has been told the wrong thing. There
    /// is no second variant for the derivation case: the check cannot tell the
    /// two apart from one number, and the sentence a user needs is the same for
    /// both. The distinction that matters is this variant against `Corrupt`
    /// (F-134, F-175).
    IndexVersion,
    /// `index_meta.state` is not `'ready'`: a previous rebuild did not commit.
    Interrupted,
    /// `index_meta.vault_root` names a different vault. The folder moved, or a
    /// `.serat/` directory was copied between vaults.
    VaultMoved,
    /// SQLite reported corruption, or `count(*) FROM notes` disagreed with
    /// `index_meta.note_count`, or an FTS integrity check failed. Raised at open
    /// or at any point in the session, from the one classifier of §3.4.
    Corrupt,
    /// The user asked, from the command palette.
    UserRequested,
}

// ============================================================================
// Errors
// ============================================================================

/// The only error type that crosses the boundary.
///
/// Note what is absent: there is no index error. A broken index is a *state* —
/// `IndexState::Rebuilding` or `IndexState::Unavailable` — and not a failed
/// command, because no command in this file depends on the index (rule 1).
///
/// `serat-app` builds these from `serat_core::VaultError` by an exhaustive
/// match. The mapping is total and is part of the contract:
///
/// | `VaultError`        | `IpcError`                                    |
/// |---------------------|-----------------------------------------------|
/// | `PathEscapesVault`  | `IpcError::PathEscapesVault`                |
/// | `InvalidPath`       | `IpcError::PathEscapesVault`                |
/// | `NotFound`          | `IpcError::NotFound`                        |
/// | `MalformedHash`     | `IpcError::MalformedHash`                   |
/// | `Io`                | `IpcError::Io`                              |
/// | `AtomicWriteFailed` | `IpcError::Io`                              |
/// | `Watcher`           | never reaches a command; reported as status   |
/// | `MalformedPatch`    | `IpcError::InvalidPatch`                    |
/// | `LineEndingsWouldFuse` | `IpcError::LineEndingsWouldFuse`         |
///
/// The last two rows arrive with task #33's patch path in serat-vault (ADR-0016
/// §2). Until it merges, no `VaultError` produces either `IpcError`.
///
/// `serat_core::ParseError::NotUtf8` maps to `IpcError::NotUtf8`. The two
/// variants with no counterpart in the core taxonomy — `IpcError::NotWritable`
/// and `IpcError::StaleHash` — are boundary rules (3 and 4), which is exactly
/// where they belong: the core refuses nothing on the grounds of a file
/// extension, and the stale guard is a decision about a user's intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum IpcError {
    /// The chosen root does not exist, is not a directory, or is not readable.
    VaultNotFound {
        /// The path as given.
        path: String,
    },

    /// The path was absolute, contained `..`, or resolved outside the vault
    /// root. Refused before any filesystem call (rule 2).
    PathEscapesVault {
        /// The offending path, as given.
        path: String,
    },

    /// No such file in the vault.
    NotFound {
        /// Vault-relative path.
        path: String,
    },

    /// The write target is not a `.md` file (FR-BIN-19, threat T-06). Binaries
    /// are read, extracted, annotated and queried — never rewritten.
    NotWritable {
        /// Vault-relative path of the refused target.
        path: String,
    },

    /// The file is not valid UTF-8. Deliberately not a lossy read.
    NotUtf8 {
        /// Vault-relative path.
        path: String,
    },

    /// `base_hash` was not 64 hex digits. A client bug, and deliberately not a
    /// `IpcError::StaleHash`: routing it there would send the front end down
    /// the re-read-and-offer-the-choice path of §9.1 and show the user a
    /// conflict with another writer that never happened.
    MalformedHash {
        /// The text that failed to parse.
        value: String,
    },

    /// The optimistic lock failed: the file changed since `base_hash` was read.
    /// Expected rather than exceptional, and it carries the current hash so the
    /// caller can re-read and offer the choice in place (§9.1).
    StaleHash {
        /// Vault-relative path.
        path: String,
        /// The hash the file has now, lowercase hex.
        current_hash: String,
    },

    /// A `save_patch` whose ranges are out of order, overlapping, or outside the
    /// base's lines or columns (P1, ADR-0016). A client bug, never a conflict:
    /// the file is untouched.
    InvalidPatch {
        /// Vault-relative path.
        path: String,
        /// What was wrong, for the log.
        message: String,
    },

    /// A `save_patch` that is well formed but would delete the bytes between a
    /// lone `\r` and a `\n` in a mixed-ending note, so the two would fuse into
    /// one `\r\n` and swallow a line (P1, ADR-0016 §2). **Not a client bug,
    /// and not a conflict**: the edit is one the user can legitimately make, and
    /// the only byte-preserving outcome is to refuse it. The file is untouched
    /// and the buffer stays dirty. The front end shows a clear, user-facing
    /// error naming the line: the note mixes line-ending styles there, and the
    /// user can edit that line differently or normalise the note's endings in
    /// another tool. A save that is refused this way is never retried
    /// automatically, because it would be refused again.
    LineEndingsWouldFuse {
        /// Vault-relative path.
        path: String,
        /// 0-based line, in base coordinates, of the `\r` that would fuse.
        line: u32,
    },

    /// The operating system refused.
    Io {
        /// Vault-relative path, or the best available description of the target.
        path: String,
        /// The operating system's message, for the user.
        message: String,
    },
}

// ============================================================================
// P1 (task #7)
// ============================================================================
//
// Every command below is scoped to the open vault, like the P0 ones. Each one
// that reads the index returns `IndexRead::NotReady` rather than waiting (rule
// 1). Integer fields that are 64-bit carry `#[ts(type = "number")]` (rule 5).
// Positions in a note are ADR-0016's `Pos`: 0-based line, UTF-16 column.

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// The answer of any command that reads the index. Rule 1: no command waits for
/// the index; one that needs it says what the index is doing instead.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum IndexRead<T> {
    /// The index is `Ready` and this is its answer.
    Ready {
        /// The answer.
        value: T,
    },
    /// The index is not `Ready`. Render the state; retry on `index://status`.
    NotReady {
        /// What the index is doing.
        state: IndexState,
    },
}

/// A position in a note: ADR-0016's one coordinate system. `line` is 0-based;
/// `col` counts UTF-16 code units within the line (CodeMirror's unit). Lines
/// split on `\r\n`, `\n` and a lone `\r`, exactly as CodeMirror splits them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Pos {
    /// 0-based line.
    pub line: u32,
    /// UTF-16 code units from the start of the line.
    pub col: u32,
}

/// One replacement, in the coordinates of a named base. Used in both
/// directions: a save sends them (`SavePatchArgs`), an external change
/// receives them (`ExternalChange::Changed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Patch {
    /// Start of the replaced range, in base coordinates.
    pub from: Pos,
    /// End of the replaced range (exclusive), in base coordinates.
    pub to: Pos,
    /// Replacement text. A `\n` in it is written as the note's own line ending
    /// (ADR-0016 §2 step 3); the front end never writes `\r`.
    pub insert: String,
}

/// A half-open range of UTF-16 code units in a string, for highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Span {
    /// First code unit.
    pub start: u32,
    /// One past the last code unit.
    pub end: u32,
}

// ---------------------------------------------------------------------------
// Choosing a vault (T-08)
// ---------------------------------------------------------------------------

/// `choose_vault() -> Result<Option<VaultOpened>, IpcError>`
///
/// Serves FR-VLT-01. Opens the operating system's folder picker from Rust and,
/// if the user picks a folder, opens it exactly as `open_vault` does; `None` if
/// the user cancelled. **Takes no path**, which is the point: `open_vault`
/// moves the boundary T-07 guards and stays withheld from the webview
/// (ADR-0012), and this command moves it only to where the user pointed in a
/// native dialog. The FR-VLT-01 clock (ADR-0015 §3) starts when it returns.
pub const CHOOSE_VAULT_COMMAND: &str = "choose_vault";

/// Arguments to `create_folder`.
///
/// `create_folder(args: CreateFolderArgs) -> Result<FolderCreated, IpcError>`
///
/// Serves FR-VLT-11 (US-41), the folder half. A note is created through
/// `write_file` with `base_hash: None`. A folder has no content, so it does not
/// go through `write_file`: ADR-0016 restricts that command to creating notes, and
/// creating a placeholder file to make a folder exist would write something the
/// user did not ask for (goal 1).
///
/// The T-07 boundary is the same as every other command that takes a path:
/// - `path` becomes a `VaultPath` under task #34's rules, or it is refused as
///   `IpcError::PathEscapesVault`;
/// - it is resolved through `Vault::resolve`, which refuses a symlink component;
/// - a path inside an ignored directory (`.serat/`, `.obsidian/`, `.git/`, ...)
///   is refused as `IpcError::NotWritable`. NFR-CMP-01 keeps `.obsidian/`
///   untouched, and `.serat/` is the index's.
///
/// It creates exactly one directory. **The parent must already exist**
/// (`IpcError::NotFound` otherwise), so a typo cannot create a chain of
/// folders. It writes no file content. **It is idempotent**: an existing
/// directory at `path` returns `created: false` and changes nothing. A file
/// already at `path` is `IpcError::Io`, and the front end picks a unique name
/// first, as it does for notes. The tree shows the new folder through
/// ADR-0015's live `Upsert` from the watcher, with no rebuild, and it is an empty
/// folder (FR-VLT-01's gated mark does not count it). The index is not
/// touched, because the index has no rows for folders. Granted to `main` in the
/// ACL (ADR-0012).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateFolderArgs {
    /// Vault-relative, `/`-separated path of the folder to create.
    pub path: String,
}

/// A `create_folder` that succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderCreated {
    /// `false` when the directory already existed and nothing was done.
    pub created: bool,
}

// ---------------------------------------------------------------------------
// File tree (ADR-0015, §3.5) — streaming
// ---------------------------------------------------------------------------

/// `open_tree(on_message: Channel<TreeMessage>) -> Result<(), IpcError>`
///
/// Serves FR-VLT-01, FR-VLT-04, FR-VLT-08, FR-BIN-03. Starts delivering the
/// open vault's tree on a Tauri `ipc::Channel`: ordered and lossless, bound to
/// this session, **never** the throttled event bus (a listing is data, not an
/// invalidation — §3.4). Returns immediately. Calling it again (a reload)
/// replaces the channel and re-sends the whole tree from the root.
///
/// Message order: the root `Listing` alone first, then further `Listing`s
/// breadth-first, then `Complete`; afterwards any mix of `Upsert`, `Remove`,
/// differing catch-up `Listing`s and, once, `WalkSettled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum TreeMessage {
    /// The complete, final-order contents of one directory. Replaces any
    /// earlier listing for `dir`. The front end never sorts.
    Listing {
        /// Vault-relative directory; `""` is the root.
        dir: String,
        /// Display order: directories first, then files, case-folded by name.
        entries: Vec<TreeEntry>,
    },
    /// One entry created (or the new name of a rename). Insert after `after`.
    Upsert {
        /// Parent directory.
        dir: String,
        /// The entry. Replaces an existing entry of the same name.
        entry: TreeEntry,
        /// The sibling this entry follows; `None` = first in `dir`. Decided in
        /// Rust so the front end never compares names.
        after: Option<String>,
    },
    /// One entry deleted (or the old name of a rename). Removing a directory
    /// removes its whole subtree from the model.
    Remove {
        /// Parent directory.
        dir: String,
        /// Entry name.
        name: String,
    },
    /// The initial population is complete. The front end stamps
    /// `TreeMark::Complete` after painting it.
    Complete {
        /// File entries delivered — every file the ignore rules admit. The
        /// budget job compares it with the vault manifest (ADR-0015 §3).
        entries: u32,
        /// Which source populated the tree.
        source: TreeSource,
    },
    /// The authoritative walk has finished: every catch-up listing and every
    /// empty folder has been sent. Reported, never gated (ADR-0015 §3).
    WalkSettled {
        /// Empty folders the walk added after `Complete`.
        empty_folders: u32,
    },
}

/// One entry in a directory listing. Deliberately no size or mtime: the tree
/// shows neither, and an entry carrying them would change on every save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TreeEntry {
    /// File or directory name, no path.
    pub name: String,
    /// What it is.
    pub kind: TreeEntryKind,
    /// `true` when this asset has a sidecar note (`<name>.md`, ADR-0005) that
    /// is folded into this entry and not listed separately (FR-BIN-03). Always
    /// `false` for notes and directories.
    pub sidecar: bool,
}

/// Kind of a tree entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TreeEntryKind {
    /// A Markdown note.
    Note,
    /// Any other file (FR-VLT-08).
    Asset,
    /// A directory.
    Dir,
}

/// Which source produced the initial population.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TreeSource {
    /// One read of the `Ready` index.
    Index,
    /// The breadth-first streaming walk.
    Walk,
}

/// Arguments to `mark_tree`.
///
/// `mark_tree(args: MarkTreeArgs) -> ()`
///
/// The front end's two FR-VLT-01 marks (ADR-0015 §3), stamped on the Rust
/// monotonic clock like `mark_first_paint`: after the tree component's
/// post-flush watcher for the commit, on the second `requestAnimationFrame`.
/// `FirstScreen` is also NFR-PRF-01's tree precondition (ADR-0015 §5).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MarkTreeArgs {
    /// Which mark.
    pub mark: TreeMark,
    /// File entries the front-end model holds at this mark; the budget job
    /// checks it against the manifest at `Complete`.
    pub entries_in_model: u32,
}

/// The two tree marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TreeMark {
    /// Root listing applied and painted.
    FirstScreen,
    /// `TreeMessage::Complete` applied and painted.
    Complete,
}

// ---------------------------------------------------------------------------
// Editor buffers (ADR-0016, §3.3)
// ---------------------------------------------------------------------------

/// Arguments to `note_opened`, `note_closed` and `request_external_diff`.
///
/// `note_opened(args: NoteBufferArgs) -> Result<(), IpcError>`
/// `note_closed(args: NoteBufferArgs) -> ()`
/// `request_external_diff(args: NoteBufferArgs) -> Result<Option<ExternalChange>, IpcError>`
///
/// Serves FR-VLT-05, FR-NAV-01. `note_opened` tells `serat-app` that an editor
/// pane holds this note at this base, so it keeps the base bytes and emits
/// `note://external-change` for it (ADR-0016 §3); it also records one open in
/// `.serat/history.json` for the switcher's frecency (§9.2). It is called for a
/// note opened in an editor pane **only** — never for an embed, a preview or a
/// search snippet. `note_closed` releases the base.
///
/// `request_external_diff` is ADR-0016 §2's recovery path: the front end
/// dropped an `ExternalChange` whose `base_hash` did not match, or a save came
/// back `StaleHash` (which **always** calls it). `serat-app` diffs
/// `base_hash`'s bytes against the disk as it is now; `None` means the disk
/// equals the base. When `serat-app` no longer holds the bytes for
/// `base_hash`, the answer is `IpcError::StaleHash` with the current disk hash
/// — never a guessed diff — and the front end takes the conflict path
/// (ADR-0016 §2, §4).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NoteBufferArgs {
    /// Vault-relative path.
    pub path: String,
    /// The hash of the text the buffer is based on (`FileContents::hash`, or
    /// the last `FileWritten::hash` / `ExternalChange::Changed::new_hash`).
    pub base_hash: String,
}

/// Arguments to `save_patch`.
///
/// `save_patch(args: SavePatchArgs) -> Result<FileWritten, IpcError>`
///
/// Serves FR-VLT-03, NFR-REL-01, C-04. The editor's only save (ADR-0016 §2):
/// patches in base coordinates, applied by serat-vault to the base bytes under
/// the hash lock, re-hashed immediately before the rename. Errors: `StaleHash`
/// (leave the buffer dirty, no blind retry — wait for or request the
/// `ExternalChange`), `InvalidPatch` (a client bug; the file is untouched),
/// `LineEndingsWouldFuse` (a deletion in a mixed-ending note would fuse a lone
/// `\r` with a `\n`; shown to the user, the file is untouched, never retried),
/// `NotWritable` (not Markdown, rule 3). On success `serat-app` moves its
/// recorded base for this note to the new hash.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SavePatchArgs {
    /// Vault-relative path of an existing Markdown note.
    pub path: String,
    /// The hash the patches are expressed against.
    pub base_hash: String,
    /// Ordered, non-overlapping patches in base coordinates. May be empty (a
    /// no-op that still checks the hash).
    pub patches: Vec<Patch>,
}

/// Arguments to `rebase_buffer`.
///
/// `rebase_buffer(args: RebaseBufferArgs) -> Result<Vec<Patch>, IpcError>`
///
/// ADR-0016 §4's *Keep mine*. Sent once, on that explicit action, with the
/// whole buffer text. `serat-app` checks that `disk_hash` is the file's current
/// hash (otherwise `StaleHash`), then serat-vault computes the edit script from
/// the disk text to `buffer` — the same function as `ExternalChange` — and it
/// returns those patches in **disk** coordinates. The front end then makes the
/// disk text its base and those patches its pending changes, so the next
/// `save_patch` turns the disk into exactly the buffer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RebaseBufferArgs {
    /// Vault-relative path.
    pub path: String,
    /// The disk hash the user chose to keep their text over.
    pub disk_hash: String,
    /// The buffer's full text, as CodeMirror holds it.
    pub buffer: String,
}

/// Event `note://external-change`, and the answer of `request_external_diff`.
///
/// Emitted for a note some editor pane holds when the disk bytes no longer
/// equal its base (ADR-0016 §3). **Not throttled**, and **not ordered against
/// command responses**: the front end applies it only when `base_hash` equals
/// its current base, holds it while a save is in flight, and otherwise drops
/// it and calls `request_external_diff` (ADR-0016 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ExternalChange {
    /// The file changed on disk.
    Changed {
        /// Session scope; discard if it is not the open vault.
        vault_id: String,
        /// Vault-relative path.
        path: String,
        /// The base the edits are expressed against.
        base_hash: String,
        /// The disk hash after the edits; the new base once applied.
        new_hash: String,
        /// Minimal edit script, base coordinates, ordered. When `whole` is
        /// true it is one edit covering the whole document (§3.3 fallback).
        edits: Vec<Patch>,
        /// The diff exceeded half the file; position is not preserved.
        whole: bool,
    },
    /// The file was deleted on disk. Keep the buffer; saving recreates it via
    /// `write_file` with `base_hash: None`.
    Deleted {
        /// Session scope.
        vault_id: String,
        /// Vault-relative path.
        path: String,
    },
}

/// The external-change event name. Not throttled.
pub const EXTERNAL_CHANGE_EVENT: &str = "note://external-change";

// ---------------------------------------------------------------------------
// Invalidation (§3.1 step 6)
// ---------------------------------------------------------------------------

/// Event `index://changed`: something in the index changed. Throttled to ten
/// per second and coalesced. **Invalidation only**: open views, link caches and
/// backlinks re-query; nothing may treat it as proof that a particular write
/// was indexed (§3.4).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IndexChanged {
    /// Session scope.
    pub vault_id: String,
    /// Monotonic per session; results from an older generation are stale.
    #[ts(type = "number")]
    pub generation: u64,
}

/// The invalidation event name.
pub const INDEX_CHANGED_EVENT: &str = "index://changed";

// ---------------------------------------------------------------------------
// Quick switcher and link completion (§9.2, §9.3)
// ---------------------------------------------------------------------------

/// Arguments to `switcher_query`.
///
/// `switcher_query(args: SwitcherQueryArgs) -> SwitcherResults`
///
/// Serves FR-NAV-01, NFR-PRF-06, FR-EDT-03. Never fails and never waits:
/// before the index is `Ready` it answers from the tree's paths
/// (`corpus: Tree`). The newest `seq` wins; `serat-app` discards an older query
/// still waiting, and the front end drops any response whose `seq` is not its
/// latest.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SwitcherQueryArgs {
    /// Increases with every keystroke.
    pub seq: u32,
    /// As typed. A `#` switches to heading mode (§9.2); empty returns the most
    /// frecent notes.
    pub query: String,
    /// At most 50.
    pub limit: u16,
}

/// Results of one switcher query.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SwitcherResults {
    /// Echo of the query's `seq`.
    pub seq: u32,
    /// What the answer was computed from.
    pub corpus: SwitcherCorpus,
    /// Ranked, best first, one per note.
    pub items: Vec<SwitcherItem>,
}

/// The corpus an answer came from (goal 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SwitcherCorpus {
    /// Titles, paths, aliases and headings from the `Ready` index.
    Index,
    /// Paths and file names only, from the tree, before the index is `Ready`.
    Tree,
}

/// One switcher result.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SwitcherItem {
    /// Vault-relative path of the note.
    pub path: String,
    /// `notes.title`, when known.
    pub title: Option<String>,
    /// Which candidate matched.
    pub matched: SwitcherMatch,
    /// The matched candidate's text (a title, an alias, a path or a heading).
    pub candidate: String,
    /// Matched ranges in `candidate`, for highlighting.
    pub highlight: Vec<Span>,
    /// The heading, when `matched` is `Heading`.
    pub heading: Option<HeadingRef>,
    /// The shortest link text that resolves back to this note (§9.3), without
    /// brackets — what link completion inserts.
    pub link_text: String,
}

/// Which candidate of a note matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SwitcherMatch {
    /// Frontmatter title, H1 or file name.
    Title,
    /// A frontmatter alias.
    Alias,
    /// The vault-relative path.
    Path,
    /// A heading (heading mode).
    Heading,
}

/// A heading inside a note.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HeadingRef {
    /// Heading text.
    pub text: String,
    /// Byte offset of the heading block as indexed; map with ADR-0016's
    /// function when opening (the note may have changed since).
    #[ts(type = "number")]
    pub byte_start: u64,
}

// ---------------------------------------------------------------------------
// Search and property lookup (§4.5 R1, R3)
// ---------------------------------------------------------------------------

/// Arguments to `search`.
///
/// `search(args: SearchArgs) -> Result<IndexRead<SearchOutcome>, IpcError>`
///
/// Serves FR-NAV-02 (note bodies; FR-NAV-08 is P2), FR-STR-03, FR-BIN-02. The
/// query is free text, `"phrases"` and at most one `[key:value]` (§4.5); it is
/// parsed in Rust into one of three fixed statements and never reaches FTS5 as
/// syntax.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchArgs {
    /// As typed.
    pub query: String,
    /// Hits to skip.
    pub offset: u32,
    /// At most 50.
    pub limit: u16,
}

/// What a search produced. Degenerate input is an outcome, never an error.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum SearchOutcome {
    /// Hits, best first. Empty is a valid answer.
    Hits {
        /// The hits.
        items: Vec<SearchHit>,
    },
    /// The query tokenised to nothing.
    NothingToSearch,
    /// More than one `[key:value]` term.
    TooManyPropertyTerms,
    /// An unbalanced `"`. The search panel does not submit one; this answers
    /// one that arrives anyway, without running a statement (§4.5).
    UnbalancedQuote,
}

/// One search or property-lookup hit.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchHit {
    /// Vault-relative path. For a sidecar hit, the **asset's** path (FR-BIN-02).
    pub path: String,
    /// Note title, when known.
    pub title: Option<String>,
    /// `note` or `asset`.
    pub kind: TreeEntryKind,
    /// For an asset hit, its MIME type.
    pub mime: Option<String>,
    /// For an asset hit, the sidecar note that matched.
    pub sidecar_path: Option<String>,
    /// The matching block for a full-text hit; `None` for a property-only hit.
    pub block: Option<BlockLocation>,
    /// Snippet with matches delimited by U+0002 … U+0003. Build text nodes only
    /// (T-09); the delimiters can also occur in note text, harmlessly.
    pub snippet: Option<String>,
}

/// Where a block is, as indexed.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BlockLocation {
    /// `blocks.id`, stable until the note is re-indexed.
    #[ts(type = "number")]
    pub block_id: i64,
    /// Position of the block in the note.
    pub ord: u32,
    /// Heading breadcrumb, when inside a section.
    pub heading_path: Option<String>,
    /// Byte range of the block as indexed.
    #[ts(type = "number")]
    pub byte_start: u64,
    /// End of that range.
    #[ts(type = "number")]
    pub byte_end: u64,
    /// `notes.hash` the range was indexed from. If the file's hash differs when
    /// it is opened, locate by `ord` and mark the location approximate (§4.5).
    pub note_hash: String,
}

// ---------------------------------------------------------------------------
// Backlinks and anchors (§4.5 R2, §9.3)
// ---------------------------------------------------------------------------

/// Arguments to `backlinks` and `note_anchors`.
///
/// `backlinks(args: NotePathArgs) -> Result<IndexRead<Vec<Backlink>>, IpcError>`
/// `note_anchors(args: NotePathArgs) -> Result<IndexRead<NoteAnchors>, IpcError>`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NotePathArgs {
    /// Vault-relative path of the target note.
    pub path: String,
}

/// One backlink entry: one per distinct (source note, source block), FR-NAV-04.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Backlink {
    /// Vault-relative path of the linking note.
    pub source_path: String,
    /// Its title, when known.
    pub source_title: Option<String>,
    /// The linking block; `None` when the link is in frontmatter.
    pub block: Option<BlockLocation>,
    /// Block text, cut to 300 characters in Rust; empty for frontmatter.
    pub excerpt: String,
}

/// What `[[note#` and `[[note^` complete against (§9.3).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NoteAnchors {
    /// Heading texts, in document order.
    pub headings: Vec<String>,
    /// Block ids (`^id`), in document order.
    pub block_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Link resolution in the editor (§9.3)
// ---------------------------------------------------------------------------

/// Arguments to `resolve_links`.
///
/// `resolve_links(args: ResolveLinksArgs) -> Vec<LinkResolution>`
///
/// Serves FR-EDT-03, NFR-CMP-01. One call per viewport change, all targets at
/// once. Never fails and never waits: before `Ready` it resolves file names
/// against the tree (`via: Tree`). Same rule as the index's resolver:
/// case-insensitive, exact path first, then file name with shortest path.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResolveLinksArgs {
    /// The note the links are written in (for `[[#Heading]]`).
    pub source_path: String,
    /// Link targets as written, without brackets or `|display` text.
    pub targets: Vec<String>,
}

/// How one link target resolved.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LinkResolution {
    /// The target as sent.
    pub target: String,
    /// The note or asset it names; `None` = unresolved.
    pub path: Option<String>,
    /// For a `#heading` or `^block` subpath: whether it exists in the target;
    /// `None` when there is no subpath or it cannot be checked yet.
    pub subpath_found: Option<bool>,
    /// What answered.
    pub via: SwitcherCorpus,
}

// ---------------------------------------------------------------------------
// Assets (§7.4, ADR-0017)
// ---------------------------------------------------------------------------

/// The custom scheme viewer bytes come through (ADR-0017). Not a command and
/// not in the command ACL: `serat-asset://localhost/<percent-encoded path>`,
/// or `http://serat-asset.localhost/<path>` on Windows (`useHttpsScheme:
/// false`). GET/HEAD only, PDF and images only, spans capped at 1,000 KiB.
pub const ASSET_SCHEME: &str = "serat-asset";

/// Arguments to `asset_info` and `read_asset_range`'s path.
///
/// `asset_info(args: AssetArgs) -> Result<AssetInfo, IpcError>`
///
/// Serves FR-BIN-01, FR-BIN-11, FR-BIN-12. A vault operation, not an index
/// read: MIME from the extension table, size from a stat, so pdf.js gets its
/// `length` before the index is `Ready`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssetArgs {
    /// Vault-relative path.
    pub path: String,
}

/// An asset's metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssetInfo {
    /// Vault-relative path.
    pub path: String,
    /// From `serat-core`'s one extension table.
    pub mime: String,
    /// Size on disk.
    #[ts(type = "number")]
    pub bytes: u64,
    /// Whether the scheme will serve it (PDF or image).
    pub viewable: bool,
    /// Its sidecar note, if one exists (ADR-0005, byte-exact name).
    pub sidecar_path: Option<String>,
}

/// Arguments to `read_asset_range`, ADR-0017 §Fallback only.
///
/// `read_asset_range(args: ReadAssetRangeArgs) -> Result<ArrayBuffer, IpcError>`
///
/// Returns **raw bytes** (`tauri::ipc::Response`), which arrive in JavaScript
/// as an `ArrayBuffer`, not JSON. Granted only on a platform whose webview will
/// not deliver a custom-scheme 206; applies the scheme's steps 2–7 and its span
/// cap.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReadAssetRangeArgs {
    /// Vault-relative path.
    pub path: String,
    /// First byte.
    #[ts(type = "number")]
    pub start: u64,
    /// Bytes wanted; served up to 1,000 KiB.
    pub len: u32,
}

// ---------------------------------------------------------------------------
// Settings and the command registry (§9.3, FR-PLT-05, FR-NAV-06, NFR-USE-01)
// ---------------------------------------------------------------------------

/// `get_ui_settings() -> UiSettings`
///
/// The front end's slice of the settings TOML (FR-PLT-05). The command registry
/// itself lives in the front end; Rust stores only the user's overrides, keyed
/// by command id.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UiSettings {
    /// `editor.live_preview`; `false` selects split preview (FR-EDT-10).
    pub live_preview: bool,
    /// User rebindings, `[keybindings]` in the TOML.
    pub keybindings: Vec<KeybindingOverride>,
}

/// One user rebinding.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KeybindingOverride {
    /// Registry id, e.g. `editor.toggle-bold`.
    pub command_id: String,
    /// CodeMirror key notation (`Mod-b`); `None` = deliberately unbound.
    pub key: Option<String>,
}

/// Arguments to `set_live_preview`.
///
/// `set_live_preview(args: SetLivePreviewArgs) -> Result<(), IpcError>`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetLivePreviewArgs {
    /// The new value.
    pub enabled: bool,
}

/// Arguments to `set_keybinding`.
///
/// `set_keybinding(args: SetKeybindingArgs) -> Result<(), IpcError>`
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetKeybindingArgs {
    /// Registry id.
    pub command_id: String,
    /// What to do with it.
    pub change: KeybindingChange,
}

/// A rebinding.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum KeybindingChange {
    /// Bind to this key (CodeMirror notation).
    Set {
        /// The key.
        key: String,
    },
    /// Leave the command with no binding.
    Unbind,
    /// Remove the override; the registry default applies again.
    Reset,
}

// ---------------------------------------------------------------------------
// The `.serat/` ignore offer (NFR-CMP-04, T-06)
// ---------------------------------------------------------------------------

/// `gitignore_offer_status() -> GitignoreOffer`
///
/// Serves NFR-CMP-04. Whether to show the offer to add `/.serat/` to the
/// vault's ignore file. "Don't ask again" persists in `.serat/prefs.json`,
/// which is safe under C-02: losing the file only shows the offer again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GitignoreOffer {
    /// There is no repository directory at the vault root; nothing to offer.
    NotAGitRepo,
    /// The root ignore file already has a line ignoring `.serat` (`.serat`,
    /// `.serat/`, `/.serat` or `/.serat/`).
    AlreadyIgnored,
    /// The user chose "Don't ask again".
    Declined,
    /// Show the offer.
    Offer,
}

/// `append_gitignore_entry() -> Result<GitignoreAppended, IpcError>`
///
/// The one write in P1 that is not a Markdown note: **the single named
/// exception to T-06**. Takes **no path**. It appends exactly the line
/// `/.serat/` to the ignore file at the vault root, creating it if absent and
/// adding a newline first if the file does not end in one. Idempotent (a second
/// call finds the line and writes nothing). Atomic (temp-and-rename, the file's
/// existing bytes and line endings preserved). Called only from the offer's
/// explicit confirmation; granted to `main` in the ACL (ADR-0012).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitignoreAppended {
    /// The ignore file did not exist and was created.
    pub created: bool,
    /// The entry was already there; nothing was written.
    pub already_present: bool,
}

/// `decline_gitignore_offer() -> Result<(), IpcError>`
///
/// Records "Don't ask again" in `.serat/prefs.json`. Writes nothing in the
/// vault outside `.serat/`.
pub const DECLINE_GITIGNORE_OFFER_COMMAND: &str = "decline_gitignore_offer";
