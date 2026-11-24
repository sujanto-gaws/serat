//! The test-hooks release guard, exercised against the real compiler (task #172;
//! NFR-PRF-03 evidence, ADR-0019, NFR-SEC-06).
//!
//! `lib.rs` const-asserts that serat-index's and serat-vault's `test-hooks` never reach a
//! release build, with one named exception: the `bench-bridge` feature, which lets the IPC
//! bridge example be built with `--release` so `save_patch` can be timed as shipped. The
//! exception is safe only because the `serat` binary refuses to compile with that feature.
//! Each claim is a compile outcome, so each is checked by compiling.
//!
//! These run `cargo check --release` (a release-profile build of the whole dependency
//! graph, several minutes cold), so they are `#[ignore]`d from `cargo test --workspace`
//! and run by CI's `serat-app-release-guard` job as the same four commands, and by hand:
//!
//! ```text
//! cargo test -p serat-app --test release_guard -- --ignored --test-threads=1
//! ```
//!
//! The manifest tests below are fast and always run.

use std::process::Command;

const GUARD: &str = "test-hooks must not reach a release build";

fn manifest() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("read serat-app's manifest")
}

/// `cargo check --release -p serat-app <args>`; returns (succeeded, combined output).
fn check_release(args: &[&str]) -> (bool, String) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let out = Command::new(cargo)
        .args(["check", "--release", "-p", "serat-app"])
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn bench_bridge_is_not_a_default_feature_and_enables_nothing_but_timing() {
    let manifest = manifest();
    let features = manifest
        .split("[features]")
        .nth(1)
        .expect("a [features] table")
        .split("\n[")
        .next()
        .expect("its body");
    // The table's own comments talk about test-hooks; the assertions are about what it enables.
    let features: String = features
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !features.contains("default"),
        "no default feature may carry bench-bridge in"
    );
    assert!(
        features.contains(r#"bench-bridge = ["serat-vault/phase-timing"]"#),
        "bench-bridge must enable only the inert phase timing, never a test hook: {features}"
    );
    assert!(
        !features.contains("test-hooks"),
        "bench-bridge must not turn test-hooks on itself"
    );
}

#[test]
fn only_dev_dependencies_enable_test_hooks() {
    let manifest = manifest();
    let (normal, dev) = manifest
        .split_once("[dev-dependencies]")
        .expect("a [dev-dependencies] table");
    assert!(
        !normal.contains("test-hooks\"]") && !normal.contains("\"test-hooks\""),
        "a normal dependency enables test-hooks"
    );
    assert!(
        dev.contains("test-hooks"),
        "the premise moved: re-read this test"
    );
}

#[test]
#[ignore = "release-profile cargo check; see the module docs"]
fn the_shipped_build_compiles_in_release() {
    let (ok, text) = check_release(&[]);
    assert!(ok, "serat-app must build in release as shipped:\n{text}");
}

#[test]
#[ignore = "release-profile cargo check; see the module docs"]
fn the_guard_fires_when_test_hooks_is_unified_into_a_release_build() {
    let (ok, text) = check_release(&["--features", "serat-index/test-hooks"]);
    assert!(!ok, "a release build with test-hooks on must fail");
    assert!(
        text.contains(GUARD),
        "it failed, but not on the guard:\n{text}"
    );
    let (ok, text) = check_release(&["--features", "serat-vault/test-hooks"]);
    assert!(!ok, "serat-vault's test-hooks too");
    assert!(text.contains(GUARD), "not on the guard:\n{text}");
}

#[test]
#[ignore = "release-profile cargo check; see the module docs"]
fn the_serat_binary_refuses_bench_bridge_so_the_exception_cannot_ship() {
    let (ok, text) = check_release(&["--bin", "serat", "--features", "bench-bridge"]);
    assert!(!ok, "the serat binary built with bench-bridge");
    assert!(
        text.contains("must not be built with the `bench-bridge` feature"),
        "it failed, but not on the binary's own refusal:\n{text}"
    );
}

#[test]
#[ignore = "release-profile cargo check; see the module docs"]
fn bench_bridge_lets_the_example_through_with_test_hooks_on() {
    // The example's dev-dependencies enable test-hooks; `--example` builds no binary.
    let (ok, text) = check_release(&[
        "--features",
        "bench-bridge",
        "--example",
        "editor_ipc_bridge",
    ]);
    assert!(ok, "the bridge's release build must compile:\n{text}");
}
