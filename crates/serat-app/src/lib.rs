//! The Tauri shell: commands, events, wiring. No domain logic.
//!
//! `serat-app` depends on every other crate and nothing depends on it (architecture
//! §2.1). It owns the boundary — the commands and events of `contracts/ipc.rs`, the
//! settings file, the startup trace — and owns no capability: reading, writing,
//! watching and walking are `serat-vault`'s, and the index is `serat-index`'s. Where a
//! rule is enforced here (only `.md` is writable; a path may not escape the vault) it
//! is a *boundary* rule, stated in the contract, and it is enforced here because this
//! is where an agent, an MCP client or a future feature would otherwise reach past it.
//!
//! # Task S1 — what this is and is not
//!
//! P0's shell: open a vault, read one file, write one file, and publish what the index
//! is doing. No product features. In particular the index open check itself is task I6
//! and is not written; [`index::SeratIndexOpen`] publishes an honest
//! `IndexState::Unavailable` naming the task rather than a fabricated `Ready`.
//!
//! # The generated TypeScript (ADR-0007, NFR-MNT-01)
//!
//! This crate takes `contracts/ipc.rs` as its [`ipc`] module by path, so there is
//! exactly one copy of the contract in the repository and no possibility of the two
//! drifting. The *generator of record* is the `serat-contracts` crate, not this one —
//! it exists so the contract is compiled and exported by the ordinary test run even
//! before `serat-app` does:
//!
//! ```sh
//! TS_RS_EXPORT_DIR=../app/src/ipc/generated cargo test -p serat-contracts
//! ```
//!
//! The path is relative to the **crate** directory, not the workspace root. The output
//! is committed in `app/src/ipc/generated`; CI regenerates and fails on a diff.
//!
//! Because this crate includes the same file, its own test run exports the same ten
//! types a second time, into `crates/serat-app/bindings/`. That directory is ignored
//! and is not the committed copy.

// Feature unification must not smuggle serat-index's test hooks into a release build
// (task #52): `cargo build --workspace --release` unifies them in via bench/budget.
const _: () = assert!(
    !serat_index::TEST_HOOKS || cfg!(debug_assertions),
    "test-hooks must not reach a release build"
);
// The same for serat-vault's (task #35): `Vault::with_platform` can switch off the
// containment rules (T-07, T-10, NFR-SEC-06), so it must never be reachable in a release.
const _: () = assert!(
    !serat_vault::TEST_HOOKS || cfg!(debug_assertions),
    "serat-vault test-hooks must not reach a release build"
);

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use serde::Serialize;
use tauri::{Emitter, WebviewUrl, WebviewWindowBuilder};

mod acl_manifest;
mod asset;
pub mod cli;
pub mod commands;
pub mod consumer;
pub mod editor;
pub mod error;
pub mod folder;
pub mod gitignore;
pub mod index;
mod nav_guard;
pub mod reads;
mod security_config;
pub mod settings;
pub mod startup;
pub mod state;
pub mod switcher;
pub mod tree_stream;
pub mod vault;

/// The IPC contract, taken by path from `contracts/`.
///
/// The architect owns the file; this crate compiles it. Taking it by path rather than
/// copying it into `src/` is the difference between a contract and a snapshot of one:
/// there is nothing to keep in sync, because there is only one file.
#[path = "../../../contracts/ipc.rs"]
pub mod ipc;

/// Where this crate's tests put their fixture directories (NFR-REL-01, NFR-MNT-01).
///
/// Every fixture that opens an index leaves a WAL `index.db-shm`, which SQLite maps into the
/// process. On a thin-provisioned disk under a concurrent build, a page fault on that mapping
/// failed and killed the test binary with SIGBUS (`docs/05-testing/sigbus-105.md`). `/dev/shm`
/// is tmpfs, so the mapping is memory-backed and cannot fail for want of a disk block; it is
/// used when it exists and is writable, otherwise the ordinary temp dir. Test code only: the
/// shipped app keeps WAL on the user's disk (ADR-0015).
#[cfg(test)]
pub(crate) fn test_temp_dir() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let shm = std::path::Path::new("/dev/shm");
        let probe = shm.join(format!("serat-app-probe-{}", std::process::id()));
        if shm.is_dir() && std::fs::write(&probe, b"x").is_ok() {
            let _ = std::fs::remove_file(&probe);
            shm.to_path_buf()
        } else {
            std::env::temp_dir()
        }
    })
    .clone()
}

/// A fixture directory that removes itself when dropped, so a panicking test cleans up too.
/// `/dev/shm` is RAM: a fixture left behind there is memory held until the next reboot.
#[cfg(test)]
pub(crate) struct TestDir(std::path::PathBuf, std::path::PathBuf);

#[cfg(test)]
impl TestDir {
    pub(crate) fn new(path: std::path::PathBuf) -> Self {
        Self(path.clone(), path)
    }

    /// Derefs to `path` (a file inside the fixture, say) but removes `root` on drop.
    pub(crate) fn within(path: std::path::PathBuf, root: std::path::PathBuf) -> Self {
        Self(path, root)
    }
}

#[cfg(test)]
impl std::ops::Deref for TestDir {
    type Target = std::path::PathBuf;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl AsRef<std::path::Path> for TestDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TestDir {
    fn drop(&mut self) {
        // A test may have replaced the directory with a file to force a write error.
        if std::fs::remove_dir_all(&self.1).is_err() {
            let _ = std::fs::remove_file(&self.1);
        }
    }
}

use cli::Boot;
use ipc::{BootInfo, INDEX_STATUS_EVENT, OpenVaultArgs};
use security_config::MAIN_WINDOW_LABEL;
use startup::Startup;
use state::AppState;

/// What `--exit-after-ready` writes to [`crate::startup::TRACE_ENV`] (task #111).
///
/// The startup trace, flattened, plus a snapshot of [`AppState::save_path_outcomes`]
/// taken at the same moment — process-lifetime totals of a counter `bench/budget` has
/// no other way to reach (F-391): it may not link `serat-app` under the dependency
/// rule (architecture §2.1), and there is no IPC command for it either, because that
/// would mean granting a benchmark-only command the ACL surface a shipped webview
/// never calls (ADR-0012's "exactly the five commands boot.js invokes"). The trace
/// file is already the diagnostic channel a harness reads that a user's session never
/// takes, so this rides the same line rather than opening a second one.
///
/// A run over an untouched vault reports every field `0` — not the absence of a
/// measurement, the true count for a process that never called `write_file`. A
/// harness measuring the real save path (task #94) diffs two of these, the way
/// [`AppState::save_path_outcomes`]'s own doc says to.
///
/// The `save_*` fields below are `--bench-save-list`'s (test-engineer's contract for
/// #94, agreed 2026-09-08): outcome-keyed, not a flat timing series, because a
/// refused submit fails fast inside `apply()` and mixing its near-instant elapsed
/// time into the asserted series would deflate the p95 NFR-PRF-03 is judged on
/// (F-19/F-382's failure shape, the reason this whole task exists). Zero on a run
/// that did not pass `--bench-save-list` — the same "true count, not an absence"
/// rule as `save_path_outcomes` above.
#[derive(Debug, Serialize)]
struct BenchmarkExitReport<'a> {
    #[serde(flatten)]
    trace: &'a startup::Trace,
    save_path_outcomes: state::SavePathOutcomes,
    /// How many paths `--bench-save-list` named — the attempted denominator #94's
    /// assertion is against, not just a symptom count.
    save_attempted: usize,
    /// Elapsed milliseconds of every save whose outcome was `submitted`. **This is
    /// the series NFR-PRF-03's p95 is asserted against.**
    save_submitted_ms: Vec<f64>,
    /// How many attempted saves found no index handle. Test-engineer's rule: must be
    /// a hard zero for a valid run. `serat-app` only reports the count; the
    /// assertion that a non-zero value invalidates the run is `bench/budget`'s.
    save_handle_missing: u64,
    /// How many attempted saves were refused by the writer — ordinary backpressure
    /// (ADR-0011), not necessarily zero, and not asserted against the p95.
    save_writer_refused: u64,
    /// Elapsed milliseconds of every refused save, reported for diagnosing the
    /// queue and excluded from `save_submitted_ms` for the reason stated there.
    save_writer_refused_ms: Vec<f64>,
}

/// What [`run_save_benchmark`] measured, before it is folded into
/// [`BenchmarkExitReport`] beside the startup trace.
#[derive(Debug, Default)]
struct SaveBenchmarkResult {
    attempted: usize,
    submitted_ms: Vec<f64>,
    handle_missing: u64,
    writer_refused: u64,
    writer_refused_ms: Vec<f64>,
}

/// Save every vault-relative path in `list_path`, in order, through the real save
/// path (`AppState::write_file` — no watcher, no debounce, §3.1.1), timing each one
/// and bucketing it by which of [`state::SavePathOutcomes`]'s three counters moved.
///
/// Each path is read first and written back with its own current text and hash: a
/// real `write_file` call, submitted to the index unconditionally regardless of
/// whether the bytes changed (`AppState::write_file`'s own body confirms this), so
/// the save path is genuinely exercised without the benchmark vault growing a byte
/// on every run — a vault the next run cannot reproduce is worse than a synthetic
/// edit (test-engineer confirmed this reading: NFR-PRF-03 is "a single note save
/// reflected in the index", not a content diff).
///
/// # Errors
///
/// The list could not be read, or a single save in it returned an `Err` rather than
/// a degraded-but-successful outcome — a stale hash, a missing note, a non-Markdown
/// path. Those are harness bugs (a list built against a different vault snapshot, or
/// naming a file `--bench-save-list` was never meant to touch), not backpressure, and
/// the whole run aborts rather than silently reporting a shorter series than
/// `--bench-save-list` asked for.
fn run_save_benchmark(
    state: &AppState,
    list_path: &std::path::Path,
) -> Result<SaveBenchmarkResult, String> {
    let list = std::fs::read_to_string(list_path)
        .map_err(|err| format!("--bench-save-list {}: {err}", list_path.display()))?;
    let paths: Vec<&str> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    let before_all = state.save_path_outcomes();
    let mut submitted_ms = Vec::with_capacity(paths.len());
    let mut writer_refused_ms = Vec::new();

    for path in &paths {
        let before = state.save_path_outcomes();
        let read = state
            .read_file(ipc::ReadFileArgs {
                path: (*path).to_owned(),
            })
            .map_err(|err| {
                format!("--bench-save-list: reading {path} before saving it: {err:?}")
            })?;

        let started = std::time::Instant::now();
        let written = state.write_file(ipc::WriteFileArgs {
            path: (*path).to_owned(),
            text: read.text,
            base_hash: Some(read.hash),
        });
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        written.map_err(|err| format!("--bench-save-list: saving {path}: {err:?}"))?;

        let after = state.save_path_outcomes();
        if after.submitted > before.submitted {
            submitted_ms.push(elapsed_ms);
        } else if after.writer_refused > before.writer_refused {
            writer_refused_ms.push(elapsed_ms);
        }
        // A handle_missing outcome leaves nothing to bucket beyond the write itself
        // -- its count is the delta computed below, on the same clock as the other
        // two, not a per-save timing series.
    }

    let after_all = state.save_path_outcomes();
    Ok(SaveBenchmarkResult {
        attempted: paths.len(),
        submitted_ms,
        handle_missing: after_all.handle_missing - before_all.handle_missing,
        writer_refused: after_all.writer_refused - before_all.writer_refused,
        writer_refused_ms,
    })
}

/// Build and run the application. Returns only when the window closes.
///
/// The order here is architecture §3.4's first row, and the order is the point:
///
/// 1. Load settings (main thread, cheap, cannot fail the start).
/// 2. Open the vault — which starts the index actor and the vault thread and returns
///    immediately, so both are running while the webview loads.
/// 3. Create the window and load the bundle.
///
/// Nothing in steps 1 to 3 is proportional to the size of the vault.
///
/// # Errors
///
/// Tauri's, or a start that was asked to be benchmarked and could not be.
pub fn run(startup: Arc<Startup>, boot: Boot) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let (state, settings_problem) = AppState::new(startup, boot);
    if let Some(problem) = settings_problem {
        eprintln!("serat: {problem}");
    }
    let state = Arc::new(state);
    state.startup.mark_state_ready();

    // `AppHandle::exit(code)` asks the event loop to stop; it does not decide what the
    // process returns, because `Builder::run` then returns `Ok(())` and `main` would
    // report success. The first benchmark run of this shell failed to produce a trace
    // and still exited 0, which for a budget job is the worst of both. The code the
    // benchmark thread wants is recorded here and returned below.
    let exit_code = Arc::new(AtomicI32::new(0));
    let setup_exit_code = Arc::clone(&exit_code);
    let window_state = Arc::clone(&state);
    let exit_state = Arc::clone(&state);

    tauri::Builder::default()
        // §9.3: opening an external link goes through the system opener, and
        // only on an explicit click (vue-frontend-dev's side). This crate's
        // job is registering the plugin and narrowing its capability grant
        // (capabilities/default.json) to exactly http/https/mailto -- task
        // #37 -- rather than the plugin's own wider `tel:`-including default.
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::clone(&state))
        // The index is closed cleanly before the window is destroyed (architecture
        // 3.1.1 item 6): without it every session reads as unclean. The editor first
        // gets a bounded flush (`app://closing`, NFR-REL-01); the close checkpoints the
        // WAL; both run on a thread of its own and the window is destroyed when it is
        // done, so the webview thread waits for nothing. A second request while that
        // runs, or a vault switch racing it, waits for the same close and the window is
        // destroyed once (`on_close_requested`).
        .on_window_event(move |window, event| {
            if window.label() != MAIN_WINDOW_LABEL {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let target = window.clone();
                let prevent = on_close_requested(&window_state, move || {
                    if let Err(err) = target.destroy() {
                        eprintln!("serat: the window could not be destroyed: {err}");
                    }
                });
                if prevent {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_vault,
            commands::read_file,
            commands::write_file,
            commands::index_status,
            commands::mark_first_paint,
            commands::mark_vault_usable,
            commands::refused_names,
            commands::create_folder,
            commands::get_ui_settings,
            commands::set_live_preview,
            commands::set_keybinding,
            commands::gitignore_offer_status,
            commands::append_gitignore_entry,
            commands::decline_gitignore_offer,
            commands::search,
            commands::switcher_query,
            commands::backlinks,
            commands::open_tree,
            commands::mark_tree,
            commands::note_opened,
            commands::note_closed,
            commands::save_patch,
            commands::request_external_diff,
            commands::rebase_buffer,
            commands::flush_ack,
            commands::asset_info,
            commands::read_asset_range,
        ])
        // ADR-0017: the one read-only path vault bytes take into the webview.
        // Runs on the blocking pool (never the main thread, design goal 2); the
        // handler is asset::serve, which applies steps 1-10 in order.
        .register_asynchronous_uri_scheme_protocol(
            ipc::ASSET_SCHEME,
            |context, request, responder| {
                use tauri::Manager as _;
                let state = Arc::clone(&*context.app_handle().state::<Arc<AppState>>());
                tauri::async_runtime::spawn_blocking(move || {
                    let reply = state.serve_asset(nav_guard::Platform::current(), &request);
                    responder.respond(reply.into_http());
                });
            },
        )
        .setup(move |app| {
            state.startup.mark_runtime_ready();
            let handle = app.handle().clone();

            // 1. Point index://status at the front end. Installed before the vault is
            //    opened, so the first transition the actor publishes is not lost.
            let emitter = handle.clone();
            state.events.install(move |status| {
                if let Err(err) = emitter.emit(INDEX_STATUS_EVENT, status) {
                    // Reported rather than swallowed: the one way this fails is an
                    // event name Tauri will not accept, and a silently undelivered
                    // status is indistinguishable from an index that never changed.
                    eprintln!("serat: {INDEX_STATUS_EVENT} could not be emitted: {err}");
                }
            });

            // 1b. And note://external-change: unthrottled, not ordered against command
            //     responses (ADR-0016 §2), so the front end's rules decide when to apply it.
            let external = handle.clone();
            state.events.install_external(move |change| {
                if let Err(err) = external.emit(ipc::EXTERNAL_CHANGE_EVENT, change) {
                    eprintln!(
                        "serat: {} could not be emitted: {err}",
                        ipc::EXTERNAL_CHANGE_EVENT
                    );
                }
            });

            // 1c. And tree://status: the tree stream gave up (goal 5: say so).
            let tree_status = handle.clone();
            state.events.install_tree_status(move |status| {
                if let Err(err) = tree_status.emit(ipc::TREE_STATUS_EVENT, status) {
                    eprintln!(
                        "serat: {} could not be emitted: {err}",
                        ipc::TREE_STATUS_EVENT
                    );
                }
            });

            // 1d. And index://changed: invalidation only, throttled to ten per second by
            //     the notifier before it reaches this sink.
            let changed = handle.clone();
            state.events.install_changed(move |event| {
                if let Err(err) = changed.emit(ipc::INDEX_CHANGED_EVENT, event) {
                    eprintln!(
                        "serat: {} could not be emitted: {err}",
                        ipc::INDEX_CHANGED_EVENT
                    );
                }
            });

            // 1e. And app://closing: the bounded flush before the vault closes.
            let closing = handle.clone();
            state.events.install_closing(move |event| {
                if let Err(err) = closing.emit(ipc::APP_CLOSING_EVENT, event) {
                    eprintln!(
                        "serat: {} could not be emitted: {err}",
                        ipc::APP_CLOSING_EVENT
                    );
                }
            });

            // 2. Open the vault first. This spawns the index actor and the vault thread
            //    and returns; §3.4's "in parallel" column starts here, before the
            //    expensive part of the main thread's own row.
            let mut vault_root = None;
            let mut vault_id = None;
            if let Some(root) = state.boot.vault.clone() {
                match state.open_vault(OpenVaultArgs {
                    root: root.display().to_string(),
                }) {
                    Ok(opened) => {
                        vault_root = Some(opened.root);
                        vault_id = Some(opened.vault_id);
                    }
                    Err(err) => eprintln!("serat: {} could not be opened: {err:?}", root.display()),
                }
            }
            // 2b. `vault://opened` only for opens *after* launch: the window does not exist
            //     yet, `BootInfo::vault_id` carries the launch vault's id, so the sink
            //     goes in after the boot open (contract: not emitted for the launch vault).
            let opened = handle.clone();
            state.events.install_opened(move |event| {
                if let Err(err) = opened.emit(ipc::VAULT_OPENED_EVENT, event) {
                    eprintln!(
                        "serat: {} could not be emitted: {err}",
                        ipc::VAULT_OPENED_EVENT
                    );
                }
            });
            let note = state.note_to_open();

            // A benchmark run that cannot produce a number must fail cleanly and say
            // why. Deliberately not `return Err(...)` from `setup`: Tauri turns a setup
            // failure into a panic, and a budget job reading exit code 101 and a
            // backtrace learns nothing it can act on. Exit 2 with a sentence instead.
            //
            // A missing `--open` is not refused here (task #59, F-359): task #52 ruled
            // that cold start does not open a note, so t_usable is taken over an empty
            // buffer and that is exactly what NFR-PRF-01 measures. `note_to_open` still
            // declines the settings-file fallback under `--exit-after-ready` (F-169) —
            // that is what makes an absent `--open` safe to allow rather than something
            // silently answered from per-machine state.
            if state.boot.exit_after_ready && vault_root.is_none() {
                let refusal = format!(
                    "--vault {} could not be opened, so there is no cold start to measure",
                    state
                        .boot
                        .vault
                        .as_ref()
                        .map_or_else(|| "<none>".to_owned(), |v| v.display().to_string())
                );
                setup_exit_code.store(2, Ordering::SeqCst);
                exit_now(2, &refusal);
            }

            let boot_info = serde_json::to_string(&BootInfo {
                vault: vault_root,
                vault_id,
                open: note,
                exit_after_ready: state.boot.exit_after_ready,
            })?;

            // Our work on the main thread ends here; everything after it until the
            // window exists belongs to Tauri and the platform webview.
            state.startup.mark_setup_done();

            // 3. Then the window.
            let geometry = settings_geometry();
            // §9.3 / task #37: read once, here, rather than inside the closure
            // below -- `is_app_navigation_target` stays a pure function over
            // its arguments, and `debug_assertions` is a property of this
            // build regardless of what ships in the config (see that
            // function's doc comment).
            let dev_server_origin = app.config().build.dev_url.clone();
            let window = WebviewWindowBuilder::new(
                app,
                MAIN_WINDOW_LABEL,
                WebviewUrl::App("index.html".into()),
            )
            .title("Serat")
            // ADR-0017: pinned, not defaulted. On Windows the app origin is
            // `http://tauri.localhost` and the asset page URL is
            // `http://serat-asset.localhost`; `app_origin()` and the CSP hard-code
            // `http://`, so the `https` form must never be selected.
            .use_https_scheme(false)
            .inner_size(geometry.0, geometry.1)
            .initialization_script(format!("window.__SERAT_BOOT__ = {boot_info};"))
            // T-09, ADR-0017's correction (task #36): the only control
            // keeping a top-level navigation off the app's own origin --
            // including a `serat-asset://…` document, which Tauri
            // classifies as local and would otherwise reach every
            // command granted to `main` (nav_guard's doc comment).
            .on_navigation(move |url| {
                nav_guard::is_app_navigation_target(
                    url,
                    dev_server_origin.as_ref(),
                    nav_guard::Platform::current(),
                )
            })
            // §9.3: "window.open and new-window requests are refused the
            // same way." One window in P1; nothing opens a second one.
            .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny)
            // A browser-native download writes outside the vault, outside
            // write_atomic, outside every containment check serat-vault
            // owns (design goal 1, T-07). Nothing in P1 wants one.
            .on_download(|_webview, event| {
                !matches!(event, tauri::webview::DownloadEvent::Requested { .. })
            })
            .build()?;

            // Task #37, release builds only: the native right-click menu offers
            // "Open Link", "View Frame Source" and similar actions that exist
            // outside every guard above -- a user picking "Open Link in New
            // Window" from it would reach exactly the window on_new_window just
            // refused, by a path this crate never sees. Left enabled in debug
            // builds because it is how a developer reaches "Inspect Element"
            // without a keybinding. Linux/WebKitGTK only today: WebView2's
            // equivalent is `ICoreWebView2Settings::put_AreDefaultContextMenusEnabled`
            // through unsafe COM calls, and WKWebView has no settings-level
            // equivalent at all (it needs an NSView/WKUIDelegate override) --
            // both are real, separate pieces of platform-specific work, named
            // rather than silently left undone (task #37's report to the lead).
            #[cfg(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            ))]
            if !cfg!(debug_assertions) {
                nav_guard::disable_default_context_menu(&window);
            }
            // `window` is read on Linux/BSD only, above; elsewhere consume it.
            #[cfg(not(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            )))]
            let _ = window;

            // Diagnostic only, and emphatically not "window visible": see
            // crate::startup for why the two are separate values.
            state.startup.mark_window_created();

            if state.boot.exit_after_ready {
                spawn_benchmark_exit(&state, &handle, Arc::clone(&setup_exit_code));
            }

            Ok(())
        })
        .build(tauri::generate_context!())?
        .run(move |_app, event| {
            // Whatever ended the event loop, the index is closed (idempotent).
            if matches!(event, tauri::RunEvent::Exit) {
                close_with_deadline(&exit_state, EXIT_CLOSE_DEADLINE);
            }
        });

    Ok(match exit_code.load(Ordering::SeqCst) {
        0 => ExitCode::SUCCESS,
        code => ExitCode::from(u8::try_from(code).unwrap_or(1)),
    })
}

/// A window close was requested: run the bounded flush and the vault close on a thread
/// of its own, then `destroy` the window, exactly once however many requests (or racing
/// vault switches) there were. Returns whether the caller must `prevent_close` (the
/// thread will close the window itself); `false` (no thread could be had) leaves the
/// close to proceed, with `RunEvent::Exit` closing the index. Whether there is a vault
/// to close is decided on the thread, under `opening`: a request during a switch, or a
/// second one, blocks there and then finds nothing left to do (an immediate close).
pub(crate) fn on_close_requested(
    state: &Arc<AppState>,
    destroy: impl FnOnce() + Send + 'static,
) -> bool {
    let state = Arc::clone(state);
    std::thread::Builder::new()
        .name("serat-close".to_owned())
        .spawn(move || {
            {
                // Whoever holds `opening` runs the whole flush and close; a vault switch
                // in progress finishes first (lock order: `opening`, then the close wait).
                let _opening = state.lock_opening();
                state.close_after_flush(&_opening, state::CLOSING_FLUSH_DEADLINE);
            }
            if state.claim_window_destroy() {
                destroy();
            }
        })
        .is_ok()
}

/// The longest `RunEvent::Exit` (on the main thread) waits for the vault to close. Past
/// it the process exits with the vault unclosed: the clean-close marker is then unset
/// and the next open compares mtimes, which it does anyway.
const EXIT_CLOSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);

/// Close the open vault on a thread of its own and wait for it, but not for ever: the
/// close joins the vault thread and drains the index writer, either of which could be
/// slow behind a very large catch-up, and the main thread must not hang at exit.
fn close_with_deadline(state: &Arc<AppState>, deadline: std::time::Duration) {
    let (done, finished) = std::sync::mpsc::channel();
    let closing = Arc::clone(state);
    let spawned = std::thread::Builder::new()
        .name("serat-exit-close".to_owned())
        .spawn(move || {
            // Under `opening`: an exit during a vault switch waits for the switch, so the
            // vault it installs is the one closed (architecture 3.1.1 item 8).
            let _opening = closing.lock_opening();
            closing.close_vault();
            let _ = done.send(());
        });
    match spawned {
        Ok(_) => {
            if finished.recv_timeout(deadline).is_err() {
                eprintln!("serat: the vault did not close within {deadline:?}; exiting anyway");
            }
        }
        Err(_) => state.close_vault(),
    }
}

/// Window size, from the settings file's defaults.
///
/// Read through `Settings::default()` rather than through the loaded settings because
/// the loaded value is behind a mutex the setup closure does not hold; P0 does not
/// restore geometry, and the field exists so that P1 has somewhere to put it.
fn settings_geometry() -> (f64, f64) {
    let window = settings::WindowSettings::default();
    (window.width, window.height)
}

/// Wait for the vault-usable mark, write the trace, and exit.
///
/// On its own thread: the main thread is running the event loop, and a benchmark run
/// that blocked it would be measuring the harness.
fn spawn_benchmark_exit(
    state: &Arc<AppState>,
    handle: &tauri::AppHandle,
    exit_code: Arc<AtomicI32>,
) {
    // The whole state, not just `startup` (task #111): `save_path_outcomes()` is a
    // method on `AppState`, and this is the one place in the process that reads it on
    // the way out, so a harness driving real `write_file` calls before this thread's
    // wait resolves gets a snapshot of what they did, not the zero every prior benchmark
    // run reported (F-391 -- the counters existed and nothing external could read them).
    let state = Arc::clone(state);
    let startup = Arc::clone(&state.startup);
    let trace_path = state.boot.trace.clone();
    let timeout = state.boot.ready_timeout;
    let wait_for_walk = state.boot.exit_after_walk;
    let save_list = state.boot.bench_save_list.clone();
    let handle = handle.clone();

    std::thread::Builder::new()
        .name("serat-benchmark-exit".to_owned())
        .spawn(move || {
            let arrived = startup.wait_for_usable(timeout);
            if wait_for_walk && arrived {
                // The one walk at open (ADR-0015 section 2) is started by the
                // application itself after the first paint and the watcher, and stands
                // in for the tree's when nobody has opened one; the benchmark no longer
                // opens a stream, so it measures the product's own trigger.
                // FR-VLT-01's quantity, not NFR-PRF-01's: with no index the file tree
                // comes from the walk, so walk completion is when a tree could first
                // render (F-151). Only reached under --exit-after-walk; the ordinary
                // benchmark run exits at "usable" and leaves this mark absent, which is
                // correct for the budget it measures.
                startup.wait_for_walk_complete(timeout);
            }

            // Task #111: driven after the vault is usable, so the index actor and
            // writer are running, and before the trace is written, so the save
            // numbers ride the same line rather than a second report. A run with no
            // --bench-save-list attempted zero saves, reported as that rather than
            // omitted (BenchmarkExitReport's own doc).
            let save_result = if arrived {
                match &save_list {
                    Some(list_path) => run_save_benchmark(&state, list_path),
                    None => Ok(SaveBenchmarkResult::default()),
                }
            } else {
                Ok(SaveBenchmarkResult::default())
            };

            let code = match save_result {
                Err(problem) => {
                    eprintln!("serat: {problem}");
                    2
                }
                Ok(save) => match startup.trace() {
                    Ok(trace) => {
                        let report = BenchmarkExitReport {
                            trace: &trace,
                            save_path_outcomes: state.save_path_outcomes(),
                            save_attempted: save.attempted,
                            save_submitted_ms: save.submitted_ms,
                            save_handle_missing: save.handle_missing,
                            save_writer_refused: save.writer_refused,
                            save_writer_refused_ms: save.writer_refused_ms,
                        };
                        match write_trace(&report, trace_path.as_deref()) {
                            Ok(()) => 0,
                            Err(err) => {
                                eprintln!("serat: the startup trace could not be written: {err}");
                                2
                            }
                        }
                    }
                    Err(incomplete) => {
                        // The guard that makes a blank window a failure rather than a
                        // fast number. There is no fallback mark to fall back to.
                        eprintln!(
                            "serat: no startup trace after {timeout:?} ({}): {incomplete}",
                            if arrived { "usable" } else { "timed out" }
                        );
                        2
                    }
                },
            };
            // The index is closed after the last save this run submitted and before the
            // process ends; `exit_now` bypasses `RunEvent::Exit`.
            state.close_vault();
            exit_code.store(code, Ordering::SeqCst);
            if code == 0 {
                handle.exit(0);
            } else {
                exit_now(code, "");
            }
        })
        .expect("spawning the benchmark exit thread");
}

/// End the process with `code`, having said why.
///
/// `AppHandle::exit(code)` asks the runtime to stop, and the code does not survive:
/// measured, a shell that refused a benchmark run with `handle.exit(2)` still exited 0,
/// which for a budget job is the worst of both — it reads as a passing run with no
/// trace file. This path is only reached when the process is ending anyway, and it is
/// only reached under `--exit-after-ready`; a user's session never takes it.
///
/// Both streams are flushed first, because the message is the only thing the job has.
fn exit_now(code: i32, message: &str) -> ! {
    use std::io::Write as _;

    if !message.is_empty() {
        eprintln!("serat: {message}");
    }
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}

/// One line of JSON to the trace file, or to stdout if there is none.
///
/// Generic rather than `&startup::Trace` specifically (task #111) so
/// [`BenchmarkExitReport`] — the trace plus [`state::SavePathOutcomes`] — can go
/// through the same one-line-of-JSON, same env-var-named-file channel, rather than a
/// second output mechanism next to it. `Trace::to_line` used to own this; the two are
/// now the same call, `serde_json::to_string`, so there is nothing left for a
/// `Trace`-specific method to do that this function does not already do generically.
fn write_trace<T: Serialize>(value: &T, path: Option<&std::path::Path>) -> std::io::Result<()> {
    let line =
        serde_json::to_string(value).map_err(|err| std::io::Error::other(err.to_string()))?;
    match path {
        Some(path) => {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, format!("{line}\n"))
        }
        None => {
            println!("{line}");
            Ok(())
        }
    }
}

/// The webview bundle this build serves, for anyone wondering which front end a
/// measurement was taken against.
///
/// P0's is the placeholder in `crates/serat-app/webview/`, not vue-frontend-dev's
/// application: task S2 replaces it. A cold-start number taken now is the shell's, and
/// S2 will move it.
///
/// Reads `tauri.conf.json`'s `build.frontendDist` rather than repeating `"webview"` as
/// a second literal (F-378): that field and this function used to name the same
/// directory independently, so a build could repoint one, keep serving the bundle the
/// other still points at, and every guard that only reads this function — including
/// [`tests::the_placeholder_bundle_exists_and_carries_the_two_marks`] — would keep
/// passing against a file the application no longer serves. There is now exactly one
/// place `"webview"` is written down.
///
/// # Panics
///
/// If `tauri.conf.json` fails to parse or does not set `build.frontendDist` to a
/// string. Both are checked into the repository and read by `tauri-build` at compile
/// time too, so either failure means the crate cannot build correctly regardless of
/// this function, and panicking here is louder than a wrong path silently substituted.
#[must_use]
pub fn webview_bundle() -> PathBuf {
    /// The config `tauri-build` also reads at compile time (`build.rs`) — one file,
    /// read twice for two different purposes, rather than one fact written twice.
    const CONF: &str = include_str!("../tauri.conf.json");

    let parsed: serde_json::Value =
        serde_json::from_str(CONF).expect("tauri.conf.json must be valid JSON");
    let frontend_dist = parsed["build"]["frontendDist"]
        .as_str()
        .expect("tauri.conf.json must set build.frontendDist to a string path");
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(frontend_dist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_name_is_the_one_the_contract_declares() {
        // contracts/ipc.rs asks tauri-app-dev to confirm the permitted character set
        // against Tauri v2 and to tell the architect if `://` is not usable. It is:
        // Tauri v2 admits alphanumerics, `-`, `/`, `:` and `_` in an event name, and
        // the running shell emits this name. If a Tauri upgrade ever narrows that,
        // `run`'s emit closure prints the rejection rather than swallowing it.
        assert_eq!(INDEX_STATUS_EVENT, "index://status");
        assert!(
            INDEX_STATUS_EVENT
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | ':' | '_')),
            "{INDEX_STATUS_EVENT} uses a character Tauri v2 does not permit in an event name"
        );
    }

    #[test]
    fn the_placeholder_bundle_exists_and_carries_the_two_marks() {
        // A Tauri application with no frontend assets builds and then serves a blank
        // page, and the paint mark would never arrive. Assert the bundle is there and
        // that it still calls both marks — deleting either from the page is exactly how
        // a green cold-start job would start measuring nothing.
        //
        // F-395: this crate no longer owns verifying *how* the paint mark is taken.
        // `crates/serat-app/webview/` becomes task #98's Vite build output, and #108
        // puts the mechanism guard in `app/src/startup.test.ts`, against the source
        // that actually produces the bundle rather than a copy this crate could only
        // watch go stale the next `npm run build`. An earlier version of this test
        // asserted the mechanism itself (F-377/F-392's fix) directly against
        // `boot.js`; that assertion belongs beside the source now, not beside a
        // generated artefact, so it moved rather than being duplicated. What stays
        // here is the same shallow check this test has always done: the bundle
        // exists and still names both marks. `Startup::mark_first_paint`'s rejection
        // of non-finite/non-positive evidence (`startup::tests::f_377_mutation_a_shape_*`,
        // `f_377_mutation_b_shape_*`) is the load-bearing, Rust-side half of F-377's
        // fix and lives in this crate regardless of which front end calls it.
        //
        // Counted rather than merely present (#125, F-481, F-504). F-392 shipped a
        // call-site *count*; the comment above once said it "moved rather than being
        // duplicated" and named no destination, because there was none -- it was
        // dropped. F-504 then reproduced what the absence costs, against a real
        // WebView2 window rather than in prediction: a second `mark_first_paint` call
        // carrying fabricated but plausible evidence survived, exit 0, with
        // `paint_evidence_ms: 1.0` written verbatim into the audit field while the
        // honest call was silently no-opped by `mark_first_paint`'s first-wins rule.
        // "Is the good call there" and "is only the good call there" are different
        // questions, and a positive-only assertion cannot see what was added beside
        // it (F-392's own words, restored here as an assertion).
        //
        // Two things this does NOT do, stated because the defect is wider than the
        // vector this closes and a comment that implied otherwise would be the more
        // expensive error:
        //
        //   * It does not let the guard tell fabricated evidence from real. One call
        //     carrying plausible-positive values is still accepted on its own say-so
        //     (F-482, F-504), and where the resulting number lands is a property of
        //     where the call site sits, not of this check. That is ADR-0014's table
        //     to settle, not this test's.
        //   * It does not catch a second call routed through the bundle's existing
        //     `markFirstPaint` wrapper instead of a fresh literal -- that leaves the
        //     command name at one occurrence. The complementary source-side check
        //     belongs beside `app/src/` and is vue-frontend-dev's.
        //
        // What it does catch is the shape F-392 named and the shape F-504 used:
        // another literal invocation of the command in the shipped bundle, including
        // one hand-edited into it -- which F-478 records as the place a weakening
        // would hide, since nobody reads a minified bundle in a diff.
        let index = webview_bundle().join("index.html");
        assert!(index.is_file(), "{} is missing", index.display());
        let boot = webview_bundle().join("boot.js");
        let script = std::fs::read_to_string(&boot).expect("boot.js");
        for command in ["mark_first_paint", "mark_vault_usable"] {
            let occurrences = script.matches(command).count();
            assert_eq!(
                occurrences, 1,
                "boot.js names {command} {occurrences} times; exactly one is the \
                 guarantee. Zero means the page stopped taking the mark and a \
                 green cold-start job would measure nothing; more than one \
                 means a second call site can take the mark first and no-op \
                 the honest one (F-392, F-481, F-504). If you added the \
                 second site deliberately -- a retry path, say -- raising \
                 this count is not the fix, and it is not a tie to break \
                 either: with two sites, first-wins makes window_visible \
                 name whichever site executed earlier, which is code \
                 ordering rather than painting. A second site changes what \
                 S_shell measures. ADR-0014's table decides which \
                 interval the term names, and that decision has to precede \
                 the site"
            );
        }
    }

    #[test]
    fn the_boot_info_is_json_the_page_can_read() {
        let json = serde_json::to_string(&BootInfo {
            vault: Some("D:/vault".into()),
            vault_id: Some("v1".into()),
            open: Some("a.md".into()),
            exit_after_ready: true,
        })
        .expect("serialise");
        assert!(json.contains("\"vault\":\"D:/vault\""), "{json}");
        assert!(json.contains("\"vault_id\":\"v1\""), "{json}");
        assert!(json.contains("\"open\":\"a.md\""), "{json}");
        assert!(json.contains("\"exit_after_ready\":true"), "{json}");
    }

    #[test]
    fn a_trace_with_no_path_does_not_fail() {
        let startup = Startup::begin_with(None);
        startup.mark_window_created();
        startup
            .mark_first_paint(42.0, 20.0)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(startup::WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        let trace = startup.trace().expect("complete");
        write_trace(&trace, None).expect("stdout");

        let path = crate::test_temp_dir()
            .join(format!("serat-trace-{}", std::process::id()))
            .join("prf01.json");
        write_trace(&trace, Some(&path)).expect("file");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            written.lines().count(),
            1,
            "the trace is one line: {written}"
        );
        assert!(written.contains("t0_to_window_visible_ms"), "{written}");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn the_benchmark_exit_report_carries_save_path_outcomes_beside_the_trace() {
        // Task #111 / F-391: the trace and the save-path counters ride the same line
        // rather than two separate output mechanisms. `#[serde(flatten)]` on `trace`
        // is what makes this one JSON object, not two nested ones -- assert that
        // shape directly, since a flatten that silently stopped working would still
        // produce valid, plausible-looking JSON.
        let startup = Startup::begin_with(None);
        startup.mark_window_created();
        startup
            .mark_first_paint(42.0, 20.0)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(startup::WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        let trace = startup.trace().expect("complete");

        let report = BenchmarkExitReport {
            trace: &trace,
            save_path_outcomes: state::SavePathOutcomes {
                handle_missing: 1,
                writer_refused: 2,
                submitted: 997,
            },
            save_attempted: 1000,
            save_submitted_ms: vec![12.5, 8.3],
            save_handle_missing: 1,
            save_writer_refused: 2,
            save_writer_refused_ms: vec![0.4, 0.6],
        };
        let line = serde_json::to_string(&report).expect("json");
        assert!(
            line.contains("\"t0_to_window_visible_ms\""),
            "the trace fields must be flattened into the top level: {line}"
        );
        assert!(
            !line.contains("\"trace\":"),
            "flatten must not leave a nested \"trace\" object: {line}"
        );
        assert!(
            line.contains(
                r#""save_path_outcomes":{"handle_missing":1,"writer_refused":2,"submitted":997}"#
            ),
            "the save-path counters must be present verbatim, not summarised: {line}"
        );
        // test-engineer's contract (2026-09-08): outcome-keyed fields at the top
        // level, not nested under save_path_outcomes -- these are #94's own asserted
        // series and must be readable without unpacking a second object.
        assert!(
            line.contains(r#""save_attempted":1000"#),
            "the attempted count must be present: {line}"
        );
        assert!(
            line.contains(r#""save_submitted_ms":[12.5,8.3]"#),
            "the asserted p95 series must be present verbatim: {line}"
        );
        assert!(
            line.contains(r#""save_handle_missing":1"#),
            "the hard-zero-or-invalid counter must be present: {line}"
        );
        assert!(
            line.contains(r#""save_writer_refused":2"#),
            "the backpressure counter must be present: {line}"
        );
        assert!(
            line.contains(r#""save_writer_refused_ms":[0.4,0.6]"#),
            "the backpressure timings must be present, separate from the asserted series: {line}"
        );

        // And through the real write path, not just serde in isolation.
        let path = crate::test_temp_dir()
            .join(format!(
                "serat-benchmark-exit-report-{}",
                std::process::id()
            ))
            .join("prf03.json");
        write_trace(&report, Some(&path)).expect("file");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            written.trim_end(),
            line,
            "the file carries the same one line"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn an_untouched_vault_reports_every_save_path_counter_as_a_true_zero() {
        // The doc comment's claim, checked: a process that never called write_file
        // must report 0/0/0, not omit the field or report something that reads as
        // "no measurement". A harness diffing two reports needs a real starting value.
        let outcomes = state::SavePathOutcomes::default();
        let json = serde_json::to_string(&outcomes).expect("json");
        assert_eq!(
            json,
            r#"{"handle_missing":0,"writer_refused":0,"submitted":0}"#
        );
    }

    /// Same fixture and wait-for-`Ready` idiom as
    /// `state::tests::a_save_through_write_file_reaches_the_index_without_the_watcher`
    /// (`crates/serat-app/src/state.rs`), since `run_save_benchmark` drives the same
    /// real save path that test proved reaches the index without the watcher. Not
    /// reused directly: that helper is private to `state.rs`'s own test module.
    fn open_fixture_for_save_benchmark(name: &str) -> (std::path::PathBuf, AppState) {
        let dir =
            crate::test_temp_dir().join(format!("serat-app-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("10-projects")).expect("fixture");
        std::fs::write(dir.join("10-projects/a.md"), "# A\n").expect("fixture");
        std::fs::write(dir.join("10-projects/b.md"), "# B\n").expect("fixture");

        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            cli::Boot::default(),
            settings::Settings::default(),
        );
        state
            .open_vault(OpenVaultArgs {
                root: dir.display().to_string(),
            })
            .expect("open");

        // As state.rs's own comment on this idiom: writing before the vault is
        // `Ready` races the fixture's own rebuild reading the same bytes, and the
        // race can index either version -- indistinguishable from outside.
        let ready_by = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !matches!(state.index_status().state, ipc::IndexState::Ready { .. }) {
            assert!(
                std::time::Instant::now() < ready_by,
                "the fixture vault never reached IndexState::Ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));

        (dir, state)
    }

    #[test]
    fn run_save_benchmark_attempts_every_listed_path_and_buckets_it_submitted() {
        let (dir, state) = open_fixture_for_save_benchmark("save-bench-happy");

        let list_path = dir.join("save-list.txt");
        // A blank line and surrounding whitespace, deliberately: the parser must
        // skip the former and trim the latter rather than trying to save `""` or
        // `" 10-projects/a.md"` (which read_file would refuse as NotFound).
        std::fs::write(&list_path, "10-projects/a.md\n\n  10-projects/b.md  \n").expect("list");

        let result = run_save_benchmark(&state, &list_path).expect("benchmark");
        assert_eq!(result.attempted, 2, "blank lines must not be attempts");
        assert_eq!(
            result.submitted_ms.len(),
            2,
            "a warm index with its own writer running must submit every save, not \
             fall back to handle_missing or writer_refused: {result:?}"
        );
        assert_eq!(result.handle_missing, 0);
        assert_eq!(result.writer_refused, 0);
        assert!(
            result.writer_refused_ms.is_empty(),
            "no refused save means nothing to time in that bucket"
        );
        for ms in &result.submitted_ms {
            assert!(*ms >= 0.0 && ms.is_finite(), "{result:?}");
        }

        // The content on disk must be exactly what it was -- run_save_benchmark
        // writes each note back with its own bytes, precisely so a benchmark vault
        // does not grow a byte on every run.
        assert_eq!(
            std::fs::read_to_string(dir.join("10-projects/a.md")).expect("read back"),
            "# A\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("10-projects/b.md")).expect("read back"),
            "# B\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_save_benchmark_aborts_on_a_path_that_does_not_exist() {
        // A harness bug (a list built against a different vault snapshot), not
        // backpressure -- the whole run must abort with a clear reason rather than
        // silently reporting a shorter series than the list asked for.
        let (dir, state) = open_fixture_for_save_benchmark("save-bench-bad-path");

        let list_path = dir.join("save-list.txt");
        std::fs::write(
            &list_path,
            "10-projects/a.md\n10-projects/does-not-exist.md\n",
        )
        .expect("list");

        let err = run_save_benchmark(&state, &list_path)
            .expect_err("a note that does not exist must abort the run");
        assert!(
            err.contains("does-not-exist.md"),
            "the error must name the offending path: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_save_benchmark_refuses_a_list_file_that_is_not_there() {
        let missing = crate::test_temp_dir().join(format!(
            "serat-app-lib-save-bench-missing-list-{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        let state = AppState::with_settings(
            Arc::new(Startup::begin()),
            cli::Boot::default(),
            settings::Settings::default(),
        );
        let err = run_save_benchmark(&state, &missing).expect_err("the list file is not there");
        assert!(err.contains("--bench-save-list"), "{err}");
    }

    /// An opener whose handle keeps a one-deep write queue
    /// (`serat_index::IndexHandle::with_queue_depth`), the same device F-115's
    /// `a_save_does_not_wait_for_the_writer` (`crates/serat-index/tests/incremental.rs`)
    /// uses to make a refusal reachable at all: the default depth (4,096,
    /// `writer::DEFAULT_QUEUE_DEPTH`) makes `IndexError::QueueFull` practically
    /// unreachable from a test that submits far fewer ops than that.
    ///
    /// Also hands out a [`serat_index::WriterHandle`] clone the moment the handle
    /// opens, over `writer_tx` -- see the test below for why a second submitter needs
    /// one of these rather than going through `AppState::write_file` a second time.
    #[derive(Default)]
    struct ShallowQueue {
        handle: Option<serat_index::IndexHandle>,
        writer_tx: Option<std::sync::mpsc::Sender<serat_index::WriterHandle>>,
    }

    impl index::IndexOpen for ShallowQueue {
        fn open(
            &mut self,
            _root: &std::path::Path,
            dir: &std::path::Path,
            publish: &dyn Fn(ipc::IndexState),
        ) {
            match serat_index::IndexHandle::with_queue_depth(dir, 1) {
                Ok(handle) => {
                    if let Some(tx) = &self.writer_tx {
                        let _ = tx.send(handle.writer());
                    }
                    publish(ipc::IndexState::Ready { note_count: 0 });
                    self.handle = Some(handle);
                }
                Err(err) => publish(ipc::IndexState::Unavailable {
                    message: format!("could not open a one-deep index for the test: {err}"),
                    path: dir.display().to_string(),
                }),
            }
        }

        fn into_handle(self) -> Option<serat_index::IndexHandle> {
            self.handle
        }
    }

    #[test]
    fn run_save_benchmark_excludes_a_writer_refusal_from_submitted_ms() {
        // F-402, code-reviewer's mutation: an unconditional push into `submitted_ms`
        // regardless of outcome left every existing test green (94 passed, 0 failed --
        // identical to control), because nothing here ever drove a real refusal. A run
        // where every save was refused would then post a fast, clean p95 for saves that
        // never reached the writer -- F-19/F-382's exact failure shape, on the one
        // series NFR-PRF-03's p95 is asserted against.
        //
        // A one-shot race against a one-deep queue is unreliable here the way F-115's
        // own bare `apply()` loop is reliable: that loop pays for a repeated *read* of
        // an already-cached file and nothing else, so it can far outrun the writer.
        // `run_save_benchmark`'s loop pays for a real `read_file` and a real
        // `write_atomic` per path -- disk I/O on the calling thread, which gives the
        // writer time to drain a one-deep queue between iterations far more often than
        // not (measured: no refusal in 40 real saves against a lone contender). So a
        // second thread hammers the *same* queue directly through a cloned
        // `WriterHandle::submit`, bypassing the vault entirely -- no read, no write,
        // nothing but the submit itself -- which is what makes it fast enough to
        // contend continuously for the single slot for the run's whole duration.
        let dir = crate::test_temp_dir().join(format!(
            "serat-app-lib-save-bench-refusal-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("10-projects")).expect("fixture");

        const LISTED: usize = 40;
        let mut list = String::new();
        for i in 0..LISTED {
            let rel = format!("10-projects/note-{i}.md");
            std::fs::write(dir.join(&rel), format!("# note {i}\n")).expect("fixture");
            list.push_str(&rel);
            list.push('\n');
        }
        let list_path = dir.join("save-list.txt");
        std::fs::write(&list_path, &list).expect("list");

        let (writer_tx, writer_rx) = std::sync::mpsc::channel();
        let state = Arc::new(AppState::with_settings(
            Arc::new(Startup::begin()),
            cli::Boot::default(),
            settings::Settings::default(),
        ));
        state
            .open_vault_with(
                OpenVaultArgs {
                    root: dir.display().to_string(),
                },
                ShallowQueue {
                    handle: None,
                    writer_tx: Some(writer_tx),
                },
            )
            .expect("open");
        let hammer_writer = writer_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("ShallowQueue::open must hand out a writer for the one-deep queue");

        let ready_by = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !matches!(state.index_status().state, ipc::IndexState::Ready { .. }) {
            assert!(
                std::time::Instant::now() < ready_by,
                "the fixture vault never reached IndexState::Ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // As `a_save_through_write_file_reaches_the_index_without_the_watcher`
        // (state.rs): `Ready` publishes slightly before the `IndexHandleSlot` is
        // filled. Setup slack, not measurement.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Hammer the same one-deep queue from another thread for the run's duration,
        // submitting the same synthetic asset repeatedly (F-115's own idiom: identity
        // does not matter, only that the queue's single slot stays contended).
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hammer_stop = Arc::clone(&stop);
        let hammer = std::thread::spawn(move || {
            use serat_core::{IndexOp, IndexWriter as _, NoteKind, NoteMeta, VaultPath};
            let op = IndexOp::UpsertAsset {
                meta: NoteMeta {
                    path: VaultPath::new("bulk/hammer.md").expect("path"),
                    hash: serat_core::ContentHash::from_bytes([7u8; 32]),
                    kind: NoteKind::Asset,
                    size: 0,
                    mtime_ms: 0,
                },
            };
            while !hammer_stop.load(std::sync::atomic::Ordering::Relaxed) {
                // Refused submissions are exactly the point; nothing to do with the
                // result either way.
                let _ = hammer_writer.submit(op.clone());
            }
        });

        let result = run_save_benchmark(&state, &list_path).expect("benchmark");

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        hammer.join().expect("hammer thread");

        assert_eq!(result.attempted, LISTED);
        assert_eq!(
            result.handle_missing, 0,
            "the handle was installed and waited for before the run started: {result:?}"
        );
        assert!(
            result.writer_refused >= 1,
            "a one-deep queue contended by a second thread throughout the run produced \
             no refusal at all -- the contention this test relies on did not happen: \
             {result:?}"
        );
        assert_eq!(
            result.writer_refused as usize,
            result.writer_refused_ms.len(),
            "the refused count and its timing series must agree: {result:?}"
        );
        assert_eq!(
            result.submitted_ms.len() + result.writer_refused_ms.len(),
            LISTED,
            "every attempted, non-handle-missing save must land in exactly one bucket: \
             {result:?}"
        );
        for ms in &result.writer_refused_ms {
            assert!(*ms >= 0.0 && ms.is_finite(), "{result:?}");
        }
        // The load-bearing assertion: a refused save's near-instant timing must not be
        // among the samples NFR-PRF-03's p95 is judged on. Mutation-verified against
        // `else if after.writer_refused > before.writer_refused` replaced with an
        // unconditional push into `submitted_ms` -- that mutation makes this fail by
        // widening `submitted_ms.len()` past `LISTED - writer_refused_ms.len()`.
        assert_eq!(
            result.submitted_ms.len(),
            LISTED - result.writer_refused as usize,
            "a refused save's timing leaked into submitted_ms, the series NFR-PRF-03's \
             p95 is asserted against: {result:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod refusal_wiring_tests;
