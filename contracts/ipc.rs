//! Serat IPC contract — the P0 subset.
//!
//! Serves:  FR-VLT-01, FR-VLT-02, FR-VLT-03, NFR-PRF-01, NFR-MNT-01, NFR-REL-02
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
//! # What P1 adds, named so their absence reads as a decision
//!
//! - `list_notes`, `search`, `resolve_link`, `backlinks` — the navigation surface.
//! - `index://changed`, the throttled invalidation event of §3.1 step 6. P0 has
//!   nothing to invalidate: the shell is one text box (task S2).
//! - `ExternalChange` carrying the minimal edit script of §3.3. P0 reloads the
//!   buffer instead, because P0 ships no editor a user could lose their place in.
//! - Asset and extraction state (`ExtractionState`, FR-BIN-*).
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
/// 2.4 ms on a warm index, which is what makes waiting for it tempting — but it
/// is 2.4 ms *when the index is healthy*, and the cases that take longer are
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
/// P0 writes a whole file because P0 has a text box and no views. The byte-range
/// patcher of ADR-0008 arrives in P2 as a separate command. This command never
/// becomes the write-back path — write-back patches a range and this replaces a
/// document, and conflating them is how C-04 gets broken by accident.
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

    /// The operating system refused.
    Io {
        /// Vault-relative path, or the best available description of the target.
        path: String,
        /// The operating system's message, for the user.
        message: String,
    },
}
