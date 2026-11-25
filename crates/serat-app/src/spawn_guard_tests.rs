//! Task #189 (NFR-REL-01, NFR-MNT-01; #99/#131 determinism rule): no test in the serat-app lib test
//! binary spawns a process.
//!
//! Tests here hold and release index writer locks (`flock`, per open file description). A child
//! between `fork` and `exec` holds a copy of every one, and `FD_CLOEXEC` does not help, so a
//! parallel test's `try_lock` can see `HeldByAnotherProcess` for the length of the spawn (the
//! `writer_lock` flake of task #182). `crates/serat-app/tests/*.rs` are separate test binaries that
//! do not link this one, so they may spawn; this guard covers `src/**` only (all lib tests live
//! there, as `tests.rs` files and `#[cfg(test)]` modules).

use std::path::{Path, PathBuf};

/// `(path relative to crates/serat-app, reason)`. Expected to hold only what cannot run beside a
/// lock-holding test on the platform that has the locks.
const ALLOW: &[(&str, &str)] = &[(
    "src/asset/tests.rs",
    "`junction` runs `cmd /C mklink /J` inside a #[cfg(windows)] module; it is not compiled on unix, \
     where the fork hazard exists, and Windows locks are LockFileEx on non-inheritable handles",
)];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read_dir") {
        let p = e.expect("entry").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Spawn spellings, split so this file does not match itself.
fn needles() -> [String; 3] {
    [
        format!("Command{}new(", "::"),
        format!("process{}Command", "::"),
        format!("{}Command", "tokio::process::"),
    ]
}

fn offenders(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rs_files(&root.join("src"), &mut files);
    let me = Path::new(file!()).file_name().expect("name").to_owned();
    let mut bad = Vec::new();
    for f in files {
        if f.file_name() == Some(me.as_os_str()) {
            continue;
        }
        let rel = f
            .strip_prefix(root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOW.iter().any(|(a, _)| *a == rel) {
            continue;
        }
        let text = std::fs::read_to_string(&f).expect("read");
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if needles().iter().any(|p| code.contains(p.as_str())) {
                bad.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    bad
}

#[test]
fn no_lib_test_spawns_a_process_beside_lock_holding_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bad = offenders(root);
    assert!(
        bad.is_empty(),
        "a spawn in the serat-app lib test binary can inherit another test's index writer flock \
         between fork and exec (#182, #189); use a libc call, or move it to crates/serat-app/tests/:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_allow_list_names_only_files_that_still_spawn() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (rel, reason) in ALLOW {
        assert!(!reason.is_empty());
        let text = std::fs::read_to_string(root.join(rel)).expect("allow-listed file exists");
        assert!(
            needles().iter().any(|p| text.contains(p.as_str())),
            "{rel} no longer spawns; drop it from ALLOW"
        );
    }
}
