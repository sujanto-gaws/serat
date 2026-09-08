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

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use serde::Serialize;
use tauri::{Emitter, WebviewUrl, WebviewWindowBuilder};

mod acl_manifest;
pub mod cli;
pub mod commands;
pub mod error;
pub mod index;
mod security_config;
pub mod settings;
pub mod startup;
pub mod state;
pub mod vault;

/// The IPC contract, taken by path from `contracts/`.
///
/// The architect owns the file; this crate compiles it. Taking it by path rather than
/// copying it into `src/` is the difference between a contract and a snapshot of one:
/// there is nothing to keep in sync, because there is only one file.
#[path = "../../../contracts/ipc.rs"]
pub mod ipc;

use cli::Boot;
use ipc::{INDEX_STATUS_EVENT, OpenVaultArgs};
use security_config::MAIN_WINDOW_LABEL;
use startup::Startup;
use state::AppState;

/// What the webview is told before it runs a line of its own script.
///
/// An initialization script rather than a command, because the page needs it in order
/// to make its *first* call and a round trip to fetch it would sit inside the 400 ms
/// budget. It adds nothing to the IPC surface: these are the process's own arguments,
/// already known before the window exists.
#[derive(Debug, Clone, Serialize)]
struct BootInfo {
    /// Canonical vault root, if one was opened at start.
    vault: Option<String>,
    /// Vault-relative note to render, if any.
    open: Option<String>,
    /// Whether this process is a benchmark run.
    exit_after_ready: bool,
}

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
#[derive(Debug, Serialize)]
struct BenchmarkExitReport<'a> {
    #[serde(flatten)]
    trace: &'a startup::Trace,
    save_path_outcomes: state::SavePathOutcomes,
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

    tauri::Builder::default()
        .manage(Arc::clone(&state))
        .invoke_handler(tauri::generate_handler![
            commands::open_vault,
            commands::read_file,
            commands::write_file,
            commands::index_status,
            commands::mark_first_paint,
            commands::mark_vault_usable,
        ])
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

            // 2. Open the vault first. This spawns the index actor and the vault thread
            //    and returns; §3.4's "in parallel" column starts here, before the
            //    expensive part of the main thread's own row.
            let mut vault_root = None;
            if let Some(root) = state.boot.vault.clone() {
                match state.open_vault(OpenVaultArgs {
                    root: root.display().to_string(),
                }) {
                    Ok(opened) => vault_root = Some(opened.root),
                    Err(err) => eprintln!("serat: {} could not be opened: {err:?}", root.display()),
                }
            }
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
                open: note,
                exit_after_ready: state.boot.exit_after_ready,
            })?;

            // Our work on the main thread ends here; everything after it until the
            // window exists belongs to Tauri and the platform webview.
            state.startup.mark_setup_done();

            // 3. Then the window.
            let geometry = settings_geometry();
            WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))
                .title("Serat")
                .inner_size(geometry.0, geometry.1)
                .initialization_script(format!("window.__SERAT_BOOT__ = {boot_info};"))
                .build()?;

            // Diagnostic only, and emphatically not "window visible": see
            // crate::startup for why the two are separate values.
            state.startup.mark_window_created();

            if state.boot.exit_after_ready {
                spawn_benchmark_exit(&state, &handle, Arc::clone(&setup_exit_code));
            }

            Ok(())
        })
        .run(tauri::generate_context!())?;

    Ok(match exit_code.load(Ordering::SeqCst) {
        0 => ExitCode::SUCCESS,
        code => ExitCode::from(u8::try_from(code).unwrap_or(1)),
    })
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
    let handle = handle.clone();

    std::thread::Builder::new()
        .name("serat-benchmark-exit".to_owned())
        .spawn(move || {
            let arrived = startup.wait_for_usable(timeout);
            if wait_for_walk && arrived {
                // FR-VLT-01's quantity, not NFR-PRF-01's: with no index the file tree
                // comes from the walk, so walk completion is when a tree could first
                // render (F-151). Only reached under --exit-after-walk; the ordinary
                // benchmark run exits at "usable" and leaves this mark absent, which is
                // correct for the budget it measures.
                startup.wait_for_walk_complete(timeout);
            }
            let code = match startup.trace() {
                Ok(trace) => {
                    let report = BenchmarkExitReport {
                        trace: &trace,
                        save_path_outcomes: state.save_path_outcomes(),
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
                    // The guard that makes a blank window a failure rather than a fast
                    // number. There is no fallback mark to fall back to.
                    eprintln!(
                        "serat: no startup trace after {timeout:?} ({}): {incomplete}",
                        if arrived { "usable" } else { "timed out" }
                    );
                    2
                }
            };
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
        let index = webview_bundle().join("index.html");
        assert!(index.is_file(), "{} is missing", index.display());
        let boot = webview_bundle().join("boot.js");
        let script = std::fs::read_to_string(&boot).expect("boot.js");
        assert!(
            script.contains("mark_first_paint"),
            "boot.js stopped marking the paint"
        );
        assert!(
            script.contains("mark_vault_usable"),
            "boot.js stopped marking usability"
        );
    }

    #[test]
    fn the_boot_info_is_json_the_page_can_read() {
        let json = serde_json::to_string(&BootInfo {
            vault: Some("D:/vault".into()),
            open: Some("a.md".into()),
            exit_after_ready: true,
        })
        .expect("serialise");
        assert!(json.contains("\"vault\":\"D:/vault\""), "{json}");
        assert!(json.contains("\"open\":\"a.md\""), "{json}");
        assert!(json.contains("\"exit_after_ready\":true"), "{json}");
    }

    #[test]
    fn a_trace_with_no_path_does_not_fail() {
        let startup = Startup::begin_with(None);
        startup.mark_window_created();
        startup
            .mark_first_paint(42.0)
            .expect("valid paint evidence");
        startup.mark_index_settled();
        startup.mark_watcher(startup::WatcherOutcome::Registered);
        startup.mark_buffer_rendered();
        let trace = startup.trace().expect("complete");
        write_trace(&trace, None).expect("stdout");

        let path = std::env::temp_dir()
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
        startup.mark_first_paint();
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

        // And through the real write path, not just serde in isolation.
        let path = std::env::temp_dir()
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
}
