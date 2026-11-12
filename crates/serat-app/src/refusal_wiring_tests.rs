//! Task #35 (FR-VLT-01, NFR-SEC-06, NFR-CMP-01): the refused-name count reaches the front
//! end as an event when a name is refused after the open, and the two caps agree.

use crate::ipc::REFUSED_NAMES_CAP;

// The vault's cap and the contract's must be one number: the status count saturates at the
// contract's, and a drift would make "N names not shown" lie at the boundary.
#[test]
fn the_vaults_refusal_capacity_is_the_contracts_cap() {
    assert_eq!(serat_vault::refusal::CAPACITY as u32, REFUSED_NAMES_CAP);
}

// Mutant, run by hand on this commit: removing `publisher.watch_refusals()` in
// `open_vault_with` leaves the count only in the polled status and this times out.
#[cfg(unix)]
#[test]
fn a_name_refused_after_the_open_is_announced_by_an_index_status_event() {
    // Unix only: it creates `aux.md`, which NTFS cannot (a device name). Its imports
    // live here so the Windows build has none unused.
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::cli::Boot;
    use crate::ipc::{IndexState, IndexStatus, OpenVaultArgs};
    use crate::settings::Settings;
    use crate::startup::Startup;
    use crate::state::{AppState, OpenHooks};

    let dir = std::env::temp_dir().join(format!("serat-refusal-wiring-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("fixture");
    std::fs::write(dir.join("ok.md"), "# ok\n").expect("fixture");

    let state = AppState::with_settings(
        Arc::new(Startup::begin()),
        Boot::default(),
        Settings::default(),
    )
    .with_walk_gate(Duration::ZERO)
    .with_open_hooks(OpenHooks {
        platform: Some(serat_core::Platform::Windows),
        ..OpenHooks::default()
    });
    let seen: Arc<Mutex<Vec<IndexStatus>>> = Arc::default();
    let sink = Arc::clone(&seen);
    state
        .events
        .install(move |status| sink.lock().expect("seen").push(status.clone()));
    state
        .open_vault(OpenVaultArgs {
            root: dir.display().to_string(),
        })
        .expect("open");

    let wait = |what: &str, done: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait("Ready", &|| {
        matches!(state.index_status().state, IndexState::Ready { .. })
    });
    let before = seen.lock().expect("seen").len();

    // A device name: refused on Windows, a plain file on this runner.
    std::fs::write(dir.join("aux.md"), "# not listable\n").expect("create");
    wait(
        "an index://status event with refused_names == Some(1)",
        &|| {
            seen.lock().expect("seen")[before..]
                .iter()
                .any(|s| s.refused_names == Some(1))
        },
    );

    state.close_vault();
    let _ = std::fs::remove_dir_all(&dir);
}
