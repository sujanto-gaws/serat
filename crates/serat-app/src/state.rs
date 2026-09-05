//! Application state, and the four commands' logic.
//!
//! The logic lives here rather than in `commands.rs` so that every rule in
//! `contracts/ipc.rs` can be asserted without a running Tauri application. The
//! `#[tauri::command]` functions next door are three lines each and contain no
//! decisions — which is the shape §2.1 asks for from `serat-app`: wiring, not domain
//! logic. Every capability below comes from `serat-vault` or `serat-index`.
//!
//! The five boundary rules of `contracts/ipc.rs`, and where each is enforced:
//!
//! | Rule | Here |
//! |---|---|
//! | 1 · no command blocks on the index | [`AppState::index_status`] reads a published value; [`AppState::read_file`] and [`AppState::write_file`] never mention the index at all |
//! | 2 · every path is a vault-relative `VaultPath` | [`AppState::resolve`], before any filesystem call |
//! | 3 · only `.md` is writable | [`AppState::write_file`], first |
//! | 4 · a write is optimistically locked on the file hash | [`AppState::write_file`], §6.1 step 2 |
//! | 5 · errors are a closed set | [`crate::error::from_vault_error`], an exhaustive match |

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serat_core::{ContentHash, VaultError, VaultPath};
use serat_vault::Vault;

use crate::cli::Boot;
use crate::error::from_vault_error;
use crate::index::{
    IndexActor, IndexOpen, SeratIndexOpen, StatusPublisher, StatusSink, index_dir_for,
};
use crate::ipc::{
    FileContents, FileWritten, IndexState, IndexStatus, IpcError, OpenVaultArgs, ReadFileArgs,
    VaultOpened, WriteFileArgs,
};
use crate::settings::Settings;
use crate::startup::{Startup, WatcherOutcome};
use crate::vault::{Stage, VaultThread};

/// Where `index://status` goes.
///
/// Installed once, after the Tauri application exists. Before that — and in every test —
/// emitting is a no-op, which is deliberate: the index actor starts publishing before
/// anything has subscribed, and a status nobody heard is not an error. The poll
/// (`index_status`) is what a front end that subscribed late uses to catch up, which is
/// exactly why the contract has both.
#[derive(Default)]
pub struct Events {
    sink: RwLock<Option<StatusFn>>,
}

/// The installed sink. Named because the type is long enough that clippy is right to
/// ask, and because `install` and the field should be describing the same thing.
type StatusFn = Box<dyn Fn(&IndexStatus) + Send + Sync>;

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").finish_non_exhaustive()
    }
}

impl Events {
    /// Direct every future status at `sink`.
    pub fn install(&self, sink: impl Fn(&IndexStatus) + Send + Sync + 'static) {
        *self
            .sink
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(sink));
    }
}

impl StatusSink for Arc<Events> {
    fn emit(&self, status: &IndexStatus) {
        if let Some(sink) = self
            .sink
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            sink(status);
        }
    }
}

/// The status publisher as this crate instantiates it.
pub type Publisher = StatusPublisher<Arc<Events>>;

/// How long the vault thread waits for the first painted frame before walking anyway.
///
/// A window that never painted is already a failed start; refusing to walk the vault as
/// well would turn one problem into two, and a user staring at a broken shell is better
/// served by an index that is at least being built.
const WALK_GATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// One open vault and the two threads that serve it.
struct OpenVault {
    vault: Arc<Vault>,
    publisher: Arc<Publisher>,
    /// Kept so the actor outlives the vault it belongs to, and no longer.
    _index: IndexActor,
    /// Dropping this stops the watcher and the walk.
    _walk: VaultThread,
}

/// Everything the shell owns.
pub struct AppState {
    /// The cold-start timeline. Public because `main` and the window builder mark it.
    pub startup: Arc<Startup>,
    /// The event channel, installed once the Tauri application exists.
    pub events: Arc<Events>,
    /// What the process was asked to do.
    pub boot: Boot,
    settings: Mutex<Settings>,
    settings_path: Option<PathBuf>,
    open: RwLock<Option<OpenVault>>,
    vaults_opened: AtomicU64,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("boot", &self.boot)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// Load settings and build the state. Runs on the main thread, first row of §3.4.
    ///
    /// Returns the state and anything that went wrong reading settings, which is a
    /// message to log rather than an error to handle: a broken settings file must not
    /// stop the window opening.
    #[must_use]
    pub fn new(startup: Arc<Startup>, boot: Boot) -> (Self, Option<String>) {
        let loaded = Settings::load();
        let state = Self {
            startup,
            events: Arc::new(Events::default()),
            boot,
            settings: Mutex::new(loaded.settings),
            settings_path: loaded.path,
            open: RwLock::new(None),
            vaults_opened: AtomicU64::new(0),
        };
        (state, loaded.problem)
    }

    /// As [`AppState::new`], with settings supplied rather than loaded. For tests.
    #[must_use]
    pub fn with_settings(startup: Arc<Startup>, boot: Boot, settings: Settings) -> Self {
        Self {
            startup,
            events: Arc::new(Events::default()),
            boot,
            settings: Mutex::new(settings),
            settings_path: None,
            open: RwLock::new(None),
            vaults_opened: AtomicU64::new(0),
        }
    }

    // -- commands -----------------------------------------------------------

    /// `open_vault`. FR-VLT-01, FR-VLT-02, NFR-PRF-01.
    ///
    /// Returns as soon as the root is canonicalised and readable. It does not wait for
    /// the walk, the index open, or a rebuild — all three report over
    /// `index://status`. That is the contract that keeps the window inside 400 ms
    /// (§3.4), and it is what `the_command_returns_without_waiting_for_the_index`
    /// asserts.
    ///
    /// # Errors
    ///
    /// [`IpcError::VaultNotFound`] if the root does not exist, is not a directory, or
    /// cannot be read; [`IpcError::Io`] if a thread cannot be created.
    pub fn open_vault(&self, args: OpenVaultArgs) -> Result<VaultOpened, IpcError> {
        self.open_vault_with(args, SeratIndexOpen)
    }

    /// [`AppState::open_vault`] with the open check injected, so the actor's
    /// asynchrony can be tested against something slow enough to catch a caller that
    /// waits.
    ///
    /// # Errors
    ///
    /// As [`AppState::open_vault`].
    pub fn open_vault_with<O: IndexOpen>(
        &self,
        args: OpenVaultArgs,
        opener: O,
    ) -> Result<VaultOpened, IpcError> {
        let requested = PathBuf::from(&args.root);
        let vault = Arc::new(
            Vault::open(&requested).map_err(|_| IpcError::VaultNotFound { path: args.root })?,
        );
        let root = vault.root().to_path_buf();

        let vault_id = format!(
            "v{}",
            self.vaults_opened.fetch_add(1, Ordering::Relaxed) + 1
        );
        let publisher = Arc::new(Publisher::new(
            vault_id.clone(),
            Arc::clone(&self.startup),
            Arc::clone(&self.events),
        ));

        // §3.4's first `t_usable` precondition: the vault is not usable until the open
        // check has finished and the index state is *known*. The actor tells us by
        // publishing anything other than `Opening`; `Rebuilding` and `Unavailable`
        // settle it as much as `Ready` does.
        let settled = Arc::clone(&self.startup);
        publisher.on_publish(move |state| {
            if !matches!(state, IndexState::Opening) {
                settled.mark_index_settled();
            }
        });

        let index = IndexActor::spawn(
            Arc::clone(&publisher),
            opener,
            root.clone(),
            index_dir_for(&root),
        )
        .map_err(|err| IpcError::Io {
            path: root.display().to_string(),
            message: format!("could not start the index thread: {err}"),
        })?;

        // §3.4 puts the walk off the critical path; measured, being on another thread
        // is not enough to achieve that (see `crate::vault`). The gate holds the walk
        // until the shell has painted — after the watcher is registered, so nothing
        // changing in the meantime is missed.
        let gate = Arc::clone(&self.startup);
        let startup = Arc::clone(&self.startup);
        let walk = VaultThread::spawn(
            Arc::clone(&vault),
            move || {
                gate.wait_for_first_paint(WALK_GATE_TIMEOUT);
            },
            move |stage| match stage {
                // Half of "vault usable": §3.4 will not call a vault usable while
                // nothing is watching it, because every edit made in that window is
                // lost silently.
                Stage::WatcherRegistered => startup.mark_watcher(WatcherOutcome::Registered),
                Stage::WatcherFailed => startup.mark_watcher(WatcherOutcome::Failed),
                Stage::WalkComplete(entries) => startup.mark_walk_complete(entries),
                Stage::WalkStarted | Stage::WalkFailed => {}
            },
        )
        .map_err(|err| IpcError::Io {
            path: root.display().to_string(),
            message: format!("could not start the vault thread: {err}"),
        })?;

        self.startup.set_vault(Some(root.display().to_string()));

        // Read the state before publishing the handle, so the value returned is the one
        // the actor had at this moment rather than one that raced it.
        let index_state = publisher.status().state;

        *self.open_lock_mut() = Some(OpenVault {
            vault,
            publisher,
            _index: index,
            _walk: walk,
        });

        Ok(VaultOpened {
            root: root.display().to_string(),
            vault_id,
            index: index_state,
        })
    }

    /// `read_file`. FR-VLT-02.
    ///
    /// Reads the filesystem, never the index: the file is the truth (design goal 1),
    /// and the index may be mid-rebuild or absent entirely.
    ///
    /// # Errors
    ///
    /// [`IpcError::PathEscapesVault`], [`IpcError::NotFound`], [`IpcError::NotUtf8`] for
    /// a file that is not valid UTF-8 — deliberately not a lossy read — or
    /// [`IpcError::Io`].
    pub fn read_file(&self, args: ReadFileArgs) -> Result<FileContents, IpcError> {
        let path = Self::vault_path(&args.path)?;
        let vault = self.vault()?;
        let loaded = vault.read(&path).map_err(from_vault_error)?;

        let text = String::from_utf8(loaded.bytes).map_err(|_| IpcError::NotUtf8 {
            path: path.to_string(),
        })?;

        Ok(FileContents {
            path: path.to_string(),
            text,
            hash: loaded.meta.hash.to_hex(),
            mtime_ms: loaded.meta.mtime_ms,
            bytes: loaded.meta.size,
        })
    }

    /// `write_file`. FR-VLT-02, FR-VLT-03.
    ///
    /// Rule 3 then rule 4 then the write, in that order: an extension check is free and
    /// a stale check costs a read, so refusing a `.pdf` must not first read it.
    ///
    /// # Errors
    ///
    /// [`IpcError::NotWritable`] for anything that is not `.md` (FR-BIN-19, threat
    /// T-06); [`IpcError::StaleHash`] when the optimistic lock fails, which is an
    /// expected outcome and not a fault (§6.1, §9.1); [`IpcError::NotFound`] when a
    /// caller holding a hash finds the file gone; otherwise as `serat-vault`.
    pub fn write_file(&self, args: WriteFileArgs) -> Result<FileWritten, IpcError> {
        let path = Self::vault_path(&args.path)?;

        // Rule 3, which defers to `serat_core::VaultPath::is_markdown()` — `.md` or
        // `.markdown` — rather than restating it. One definition of what a note is: a
        // `README.markdown` is walked, indexed and readable through `read_file`, so
        // refusing to save it would be a bug on the C-01 side rather than a guard on
        // the T-06 side. (code-reviewer's D3 finding; the contract now says this.)
        if !path.is_markdown() {
            return Err(IpcError::NotWritable {
                path: path.to_string(),
            });
        }

        // Rule 4's argument, checked before any I/O: a `base_hash` that is not 64 hex
        // digits is a client bug, and the contract gives it its own variant precisely
        // so it is not reported as a conflict with a writer who does not exist.
        let base_hash = args
            .base_hash
            .as_deref()
            .map(ContentHash::from_hex)
            .transpose()
            .map_err(from_vault_error)?;

        let vault = self.vault()?;

        // Rule 4 — the optimistic lock of §6.1 step 2.
        match vault.read(&path) {
            Ok(current) => {
                // `None` asserts the file does not exist. It does, so this is a create
                // over something, which is a stale guard rather than a silent
                // overwrite.
                if base_hash != Some(current.meta.hash) {
                    return Err(IpcError::StaleHash {
                        path: path.to_string(),
                        current_hash: current.meta.hash.to_hex(),
                    });
                }
            }
            Err(VaultError::NotFound { .. }) => {
                // The caller read a hash and the file has since been deleted. That is
                // not a stale guard the caller can resolve by re-reading, so it is
                // reported as what it is.
                if base_hash.is_some() {
                    return Err(IpcError::NotFound {
                        path: path.to_string(),
                    });
                }
            }
            Err(err) => return Err(from_vault_error(err)),
        }

        vault
            .write_atomic(&path, args.text.as_bytes())
            .map_err(from_vault_error)?;

        // Re-read for the mtime and size the contract returns. It costs a second read
        // of a file we just wrote, and it buys a `hash` that is genuinely "BLAKE3 of
        // what is now on disk" rather than of what we intended to put there. P0 writes
        // whole documents from a text box; when the byte-range patcher arrives in P2
        // this is a stat, not a read.
        let written = vault.read(&path).map_err(from_vault_error)?;

        Ok(FileWritten {
            path: path.to_string(),
            hash: written.meta.hash.to_hex(),
            mtime_ms: written.meta.mtime_ms,
            bytes: written.meta.size,
        })
    }

    /// `index_status`. NFR-PRF-01, NFR-REL-02.
    ///
    /// Never blocks and never touches SQLite. With no vault open it reports
    /// [`IndexState::Opening`] against an empty vault id, which is what "not known yet"
    /// is for.
    #[must_use]
    pub fn index_status(&self) -> IndexStatus {
        match self.open_lock().as_ref() {
            Some(open) => open.publisher.status(),
            None => IndexStatus {
                vault_id: String::new(),
                state: IndexState::Opening,
                since_process_start_ms: self.startup.elapsed_ms() as u64,
            },
        }
    }

    // -- settings -----------------------------------------------------------

    /// The note to restore for the open vault: `--open` if given, else the settings
    /// file's memory of this vault.
    #[must_use]
    pub fn note_to_open(&self) -> Option<String> {
        if let Some(open) = self.boot.open.clone() {
            return Some(open);
        }
        let root = self.vault_root()?;
        self.settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_open_for(&root)
            .map(ToOwned::to_owned)
    }

    /// Remember the open note and write the settings file.
    ///
    /// Never on the critical path: the shell calls this after the vault is usable, and
    /// on shutdown. A failure is returned for logging and changes nothing.
    ///
    /// # Errors
    ///
    /// The filesystem's complaint, as a sentence.
    pub fn remember_open(&self, note: &str) -> Result<(), String> {
        let Some(root) = self.vault_root() else {
            return Ok(());
        };
        let settings = {
            let mut settings = self
                .settings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            settings.remember_open(&root, note);
            settings.clone()
        };
        match &self.settings_path {
            Some(path) => settings.save_to(path),
            None => Ok(()),
        }
    }

    /// The canonical root of the open vault, if one is open.
    #[must_use]
    pub fn vault_root(&self) -> Option<PathBuf> {
        self.open_lock()
            .as_ref()
            .map(|open| open.vault.root().to_path_buf())
    }

    // -- internals ----------------------------------------------------------

    /// Rule 2, applied before any filesystem call.
    fn vault_path(raw: &str) -> Result<VaultPath, IpcError> {
        VaultPath::new(raw).map_err(from_vault_error)
    }

    /// The open vault, or the boundary's way of saying there isn't one.
    fn vault(&self) -> Result<Arc<Vault>, IpcError> {
        self.open_lock()
            .as_ref()
            .map(|open| Arc::clone(&open.vault))
            .ok_or_else(|| IpcError::VaultNotFound {
                path: String::new(),
            })
    }

    /// Resolve a vault-relative path against the open vault. Exposed for the doc table
    /// above and used by tests; the commands go through [`AppState::vault_path`].
    ///
    /// # Errors
    ///
    /// As [`AppState::vault_path`], plus [`IpcError::VaultNotFound`] with no vault open.
    pub fn resolve(&self, raw: &str) -> Result<PathBuf, IpcError> {
        let path = Self::vault_path(raw)?;
        self.vault()?.resolve(&path).map_err(from_vault_error)
    }

    fn open_lock(&self) -> std::sync::RwLockReadGuard<'_, Option<OpenVault>> {
        self.open
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn open_lock_mut(&self) -> std::sync::RwLockWriteGuard<'_, Option<OpenVault>> {
        self.open
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::RebuildReason;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("serat-app-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("10-projects")).expect("fixture");
        std::fs::write(
            dir.join("10-projects/serat.md"),
            "---\ntype: project\n---\n\n# Serat\n",
        )
        .expect("fixture");
        std::fs::write(dir.join("contract.pdf"), [0x25, 0x50, 0x44, 0x46]).expect("fixture");
        std::fs::write(dir.join("broken.md"), [0xff, 0xfe, 0x00, 0x9f]).expect("fixture");
        dir
    }

    fn state(dir: &Path) -> AppState {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        state
    }

    #[test]
    fn opening_a_vault_that_is_not_there_is_refused_by_name() {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let err = state
            .open_vault(OpenVaultArgs {
                root: std::env::temp_dir()
                    .join("serat-no-such-vault")
                    .display()
                    .to_string(),
            })
            .expect_err("must not open");
        assert!(matches!(err, IpcError::VaultNotFound { .. }), "{err:?}");
    }

    #[test]
    fn the_command_returns_without_waiting_for_the_index() {
        // §3.4's load-bearing property, at the boundary rather than at the seam: no IPC
        // command blocks on the index. Add a `join` to `open_vault_with` and this fails
        // by a factor of four.
        struct Slow;
        impl IndexOpen for Slow {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                std::thread::sleep(Duration::from_millis(250));
                publish(IndexState::Ready { note_count: 1 });
            }
        }

        let dir = fixture("nonblocking");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );

        let started = Instant::now();
        let opened = state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                Slow,
            )
            .expect("open");
        let cost = started.elapsed();

        assert!(
            cost < Duration::from_millis(60),
            "open_vault waited {cost:?} for the index actor"
        );
        assert_eq!(
            opened.index,
            IndexState::Opening,
            "the shell paints whatever state the actor has published, which is nothing yet"
        );
        // And index_status, called immediately, must also not block.
        let started = Instant::now();
        let polled = state.index_status();
        assert!(started.elapsed() < Duration::from_millis(20));
        assert_eq!(polled.vault_id, opened.vault_id);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_a_note_returns_its_bytes_its_hash_and_its_size() {
        let dir = fixture("read");
        let state = state(&dir);
        let file = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");

        assert_eq!(file.path, "10-projects/serat.md");
        assert!(file.text.starts_with("---\ntype: project"));
        assert_eq!(file.hash.len(), 64);
        assert_eq!(file.bytes, file.text.len() as u64);
        assert!(
            file.mtime_ms > 1_600_000_000_000,
            "mtime_ms is unix milliseconds, not seconds: {}",
            file.mtime_ms
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_utf8_is_refused_rather_than_decoded() {
        // C-04 and design goal 1: a lossy read followed by a save silently rewrites the
        // user's bytes. Replace the `String::from_utf8` with `from_utf8_lossy` and this
        // fails.
        let dir = fixture("notutf8");
        let state = state(&dir);
        let err = state
            .read_file(ReadFileArgs {
                path: "broken.md".into(),
            })
            .expect_err("must refuse");
        assert!(matches!(err, IpcError::NotUtf8 { .. }), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_that_leave_the_vault_are_refused_before_any_filesystem_call() {
        let dir = fixture("escape");
        let state = state(&dir);
        for bad in [
            "../outside.md",
            "/etc/passwd",
            r"C:\Windows\system.ini",
            "a/../../b.md",
        ] {
            let err = state
                .read_file(ReadFileArgs { path: bad.into() })
                .expect_err("must refuse {bad}");
            assert!(
                matches!(err, IpcError::PathEscapesVault { .. }),
                "{bad}: {err:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_markdown_is_writable_and_the_refusal_costs_no_read() {
        // FR-BIN-19 and threat T-06, which P0 finding F-14 found unguarded one layer
        // down. The bytes of the pdf must be exactly what they were.
        let dir = fixture("notwritable");
        let state = state(&dir);
        let before = std::fs::read(dir.join("contract.pdf")).expect("read pdf");

        let err = state
            .write_file(WriteFileArgs {
                path: "contract.pdf".into(),
                text: "destroyed".into(),
                base_hash: None,
            })
            .expect_err("must refuse");
        assert!(matches!(err, IpcError::NotWritable { .. }), "{err:?}");
        assert_eq!(
            std::fs::read(dir.join("contract.pdf")).expect("read pdf"),
            before,
            "the binary was rewritten by a command that claimed to refuse"
        );

        // The refusal is on the extension, so it holds for a file that does not exist
        // either — nothing is read before the rule is applied.
        assert!(matches!(
            state.write_file(WriteFileArgs {
                path: "never/existed.png".into(),
                text: "x".into(),
                base_hash: None,
            }),
            Err(IpcError::NotWritable { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_with_the_right_hash_lands_and_a_write_with_a_stale_one_does_not() {
        let dir = fixture("stale");
        let state = state(&dir);
        let file = state
            .read_file(ReadFileArgs {
                path: "10-projects/serat.md".into(),
            })
            .expect("read");

        let written = state
            .write_file(WriteFileArgs {
                path: "10-projects/serat.md".into(),
                text: "# Serat, edited\n".into(),
                base_hash: Some(file.hash.clone()),
            })
            .expect("write");
        assert_ne!(written.hash, file.hash);
        assert_eq!(written.bytes, "# Serat, edited\n".len() as u64);
        assert_eq!(
            std::fs::read_to_string(dir.join("10-projects/serat.md")).expect("read back"),
            "# Serat, edited\n"
        );

        // The same base hash a second time is now stale, and the refusal carries the
        // hash the caller needs in order to retry (§6.1, §9.1).
        let err = state
            .write_file(WriteFileArgs {
                path: "10-projects/serat.md".into(),
                text: "# clobbered\n".into(),
                base_hash: Some(file.hash),
            })
            .expect_err("must refuse");
        match err {
            IpcError::StaleHash { current_hash, .. } => assert_eq!(current_hash, written.hash),
            other => panic!("expected StaleHash, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("10-projects/serat.md")).expect("read back"),
            "# Serat, edited\n",
            "a refused write changed the file anyway"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creating_a_note_over_an_existing_one_is_a_stale_guard_not_an_overwrite() {
        let dir = fixture("create");
        let state = state(&dir);

        // A genuine create: no hash, no file.
        let created = state
            .write_file(WriteFileArgs {
                path: "00-inbox/today.md".into(),
                text: "# today\n".into(),
                base_hash: None,
            })
            .expect("create");
        assert_eq!(created.path, "00-inbox/today.md");
        assert!(dir.join("00-inbox/today.md").exists());

        // The same create again is refused, because `None` asserts the file is absent.
        let err = state
            .write_file(WriteFileArgs {
                path: "00-inbox/today.md".into(),
                text: "# clobbered\n".into(),
                base_hash: None,
            })
            .expect_err("must refuse");
        assert!(matches!(err, IpcError::StaleHash { .. }), "{err:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("00-inbox/today.md")).expect("read back"),
            "# today\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hash_for_a_file_that_has_been_deleted_is_not_found_rather_than_stale() {
        // The two cases differ in what the caller can do next: a stale hash resolves by
        // re-reading, a deleted file does not.
        let dir = fixture("deleted");
        let state = state(&dir);
        let err = state
            .write_file(WriteFileArgs {
                path: "never/existed.md".into(),
                text: "x".into(),
                base_hash: Some("00".repeat(32)),
            })
            .expect_err("must refuse");
        assert!(matches!(err, IpcError::NotFound { .. }), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_command_before_a_vault_is_open_is_refused_and_does_not_panic() {
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        assert!(matches!(
            state.read_file(ReadFileArgs {
                path: "a.md".into()
            }),
            Err(IpcError::VaultNotFound { .. })
        ));
        assert_eq!(state.index_status().state, IndexState::Opening);
        assert_eq!(state.index_status().vault_id, "");
    }

    #[test]
    fn each_open_vault_gets_its_own_id_so_late_events_can_be_discarded() {
        let dir = fixture("ids");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let first = state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        let second = state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("reopen");
        assert_ne!(first.vault_id, second.vault_id);
        assert_eq!(state.index_status().vault_id, second.vault_id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_reaches_an_installed_sink() {
        let dir = fixture("events");
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            Settings::default(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        state.events.install(move |status: &IndexStatus| {
            sink.lock()
                .expect("not poisoned")
                .push(status.state.clone());
        });

        struct Rebuild;
        impl IndexOpen for Rebuild {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 3,
                    walk_complete: false,
                });
                publish(IndexState::Ready { note_count: 3 });
            }
        }

        state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                Rebuild,
            )
            .expect("open");

        for _ in 0..200 {
            if seen.lock().expect("not poisoned").len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let seen = seen.lock().expect("not poisoned").clone();
        assert!(
            matches!(seen.first(), Some(IndexState::Rebuilding { .. })),
            "{seen:?}"
        );
        assert!(
            matches!(seen.last(), Some(IndexState::Ready { note_count: 3 })),
            "{seen:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_note_to_open_is_the_flag_first_and_the_settings_file_second() {
        let dir = fixture("lastopen");
        let mut settings = Settings::default();
        let canonical = Vault::open(&dir).expect("open").root().to_path_buf();
        settings.remember_open(&canonical, "10-projects/serat.md");

        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot::default(),
            settings.clone(),
        );
        assert_eq!(
            state.note_to_open(),
            None,
            "no vault open, nothing to restore"
        );
        state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        assert_eq!(
            state.note_to_open().as_deref(),
            Some("10-projects/serat.md")
        );

        let flagged = AppState::with_settings(
            Arc::new(Startup::begin()),
            Boot {
                open: Some("other.md".into()),
                ..Boot::default()
            },
            settings,
        );
        flagged
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");
        assert_eq!(flagged.note_to_open().as_deref(), Some("other.md"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
