//! Task #54 (NFR-MNT-01, C-06): `--root` is accepted, and a bare one is an error
//! rather than being silently ignored (which would generate into the wrong tree).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vaultgen");

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(BIN)
        .args(args)
        .env_remove("SERAT_REPO_ROOT")
        .output()
        .expect("run vaultgen");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_bare_root_flag_is_an_error() {
    for args in [
        &["--root"][..],
        &["--root="][..],
        &["--preset", "smoke", "--root"][..],
    ] {
        let (ok, err) = run(args);
        assert!(!ok, "{args:?} must fail");
        assert!(err.contains("--root"), "{args:?}: {err}");
    }
}

/// Reviewer finding on #54: from a directory with no `[workspace]` above it the
/// root is the compiled-in checkout, which vaultgen must not write into.
#[test]
fn it_refuses_to_write_under_the_compiled_in_checkout() {
    let out = Command::new(BIN)
        .args(["--preset", "smoke"])
        .current_dir(std::env::temp_dir())
        .env_remove("SERAT_REPO_ROOT")
        .output()
        .expect("run vaultgen");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("compiled-in checkout"), "{err}");
    assert!(
        err.contains("root: "),
        "the root line is printed first: {err}"
    );
}

#[test]
fn the_usage_text_documents_root() {
    let out = Command::new(BIN).arg("--help").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("--root <DIR>"));
}
