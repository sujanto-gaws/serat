//! Hostile tests for the two note write paths (NFR-REL-01, NFR-SEC-06, FR-VLT-03, T-07;
//! ADR-0008, ADR-0016 §2): a planted symlink at a temp name, and two in-process writers
//! racing on one path.

use std::fs;
use std::sync::mpsc;
use std::time::Duration;

use serat_core::{VaultError, VaultPath};

use crate::{Edit, Position, Vault};

fn vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().expect("fixture");
    let vault = Vault::open(dir.path()).expect("open");
    (dir, vault)
}

fn edit(text: &str) -> Vec<Edit> {
    vec![Edit::new(
        Position::new(0, 0),
        Position::new(0, 3),
        text.to_owned(),
    )]
}

fn temps(dir: &std::path::Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".serat-tmp-"))
        .collect()
}

#[cfg(unix)]
#[test]
fn hostile_a_symlink_planted_at_a_temp_name_is_not_written_through() {
    let (dir, vault) = vault();
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim");
    fs::write(&victim, "precious\n").unwrap();

    let path = VaultPath::new("note.md").unwrap();
    let base = vault.write_atomic(&path, b"old\n").unwrap();

    // The exclusive create refuses a link at a name it is asked to use.
    let planted = dir.path().join(".serat-tmp-note.md.guess");
    std::os::unix::fs::symlink(&victim, &planted).unwrap();
    let err = crate::atomic::write_exclusive(&planted, b"new\n").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

    // And links at the names the old scheme used (pid.counter) change nothing for
    // either write path, because the new names are not guessable from them.
    for n in 0..8 {
        let old = dir
            .path()
            .join(format!(".serat-tmp-note.md.{}.{n}", std::process::id()));
        std::os::unix::fs::symlink(&victim, &old).unwrap();
    }
    let patched = vault.write_patch(&path, base, &edit("NEW")).unwrap();
    vault.write_atomic(&path, b"again\n").unwrap();
    assert_ne!(patched, base);
    assert_eq!(fs::read(dir.path().join("note.md")).unwrap(), b"again\n");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n");
}

#[test]
fn temp_names_are_not_a_function_of_pid_or_a_counter() {
    let dir = tempfile::tempdir().unwrap();
    let a = crate::atomic::write_temp(dir.path(), "n.md", b"a", None).unwrap();
    let b = crate::atomic::write_temp(dir.path(), "n.md", b"b", None).unwrap();
    assert_ne!(a, b);
    let name = a.file_name().unwrap().to_string_lossy().into_owned();
    assert!(!name.contains(&std::process::id().to_string()), "{name}");
}

/// Two writers start from one base. A is held between its re-hash and its rename (the
/// window ADR-0016 §2 names); B starts meanwhile. Without the per-path lock both pass
/// the re-hash and both return `Ok`, one with its bytes already lost. With it, B waits
/// for A and then finds the file moved: exactly one `Ok`, one `StaleHash`.
#[test]
fn two_writers_racing_on_one_path_yield_one_ok_and_one_stale_hash() {
    let (dir, vault) = vault();
    let path = VaultPath::new("note.md").unwrap();
    let base = vault.write_atomic(&path, b"aaa\nrest\n").unwrap();

    let (parked_tx, parked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (b_started_tx, b_started_rx) = mpsc::channel();

    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            vault.write_patch_hooked(
                &path,
                base,
                &edit("AAA"),
                || {},
                move || {
                    parked_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                },
            )
        });
        parked_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let b = scope.spawn(|| {
            b_started_tx.send(()).unwrap();
            vault.write_patch(&path, base, &edit("BBB"))
        });
        b_started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // Give B every chance to get past the re-hash if nothing stops it.
        std::thread::sleep(Duration::from_millis(300));
        release_tx.send(()).unwrap();
        (a.join().unwrap(), b.join().unwrap())
    });

    assert!(a.is_ok(), "{a:?}");
    assert!(matches!(b, Err(VaultError::StaleHash { .. })), "{b:?}");
    assert_eq!(
        fs::read(dir.path().join("note.md")).unwrap(),
        b"AAA\nrest\n"
    );
    assert!(temps(dir.path()).is_empty());
}

#[test]
fn many_unhooked_writers_from_one_base_yield_exactly_one_ok() {
    let (dir, vault) = vault();
    let path = VaultPath::new("note.md").unwrap();
    let base = vault.write_atomic(&path, b"aaa\nrest\n").unwrap();
    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let (vault, path) = (&vault, &path);
                scope.spawn(move || vault.write_patch(path, base, &edit(&format!("w{i}"))))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let stale = results
        .iter()
        .filter(|r| matches!(r, Err(VaultError::StaleHash { .. })))
        .count();
    assert_eq!((ok, stale), (1, 7), "{results:?}");
    assert!(temps(dir.path()).is_empty());
}

/// `write_atomic` takes the same lock: while a patch holds the path, a whole-file
/// write waits for it instead of renaming underneath it.
#[test]
fn write_atomic_waits_for_a_patch_in_flight_on_the_same_path() {
    let (dir, vault) = vault();
    let path = VaultPath::new("note.md").unwrap();
    let base = vault.write_atomic(&path, b"aaa\nrest\n").unwrap();

    let (parked_tx, parked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel();

    std::thread::scope(|scope| {
        let patch = scope.spawn(|| {
            vault.write_patch_hooked(
                &path,
                base,
                &edit("AAA"),
                || {},
                move || {
                    parked_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                },
            )
        });
        parked_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let whole = scope.spawn(|| {
            let r = vault.write_atomic(&path, b"whole\n");
            done_tx.send(()).unwrap();
            r
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "write_atomic finished while a patch held the path"
        );
        release_tx.send(()).unwrap();
        assert!(patch.join().unwrap().is_ok());
        assert!(whole.join().unwrap().is_ok());
    });
    // The whole-file write ran second, so it is what is on disk.
    assert_eq!(fs::read(dir.path().join("note.md")).unwrap(), b"whole\n");
}

#[test]
fn writes_to_different_paths_do_not_wait_for_each_other() {
    let (_dir, vault) = vault();
    let a = VaultPath::new("a.md").unwrap();
    let b = VaultPath::new("b.md").unwrap();
    let base = vault.write_atomic(&a, b"aaa\n").unwrap();

    let (parked_tx, parked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let held = scope.spawn(|| {
            vault.write_patch_hooked(
                &a,
                base,
                &edit("AAA"),
                || {},
                move || {
                    parked_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                },
            )
        });
        parked_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // Would deadlock the test (and fail on the timeout above) if the lock were global.
        vault.write_atomic(&b, b"bbb\n").unwrap();
        release_tx.send(()).unwrap();
        held.join().unwrap().unwrap();
    });
}

#[test]
fn the_lock_map_is_bounded_by_concurrent_writes_not_by_vault_size() {
    let (_dir, vault) = vault();
    for i in 0..50 {
        let path = VaultPath::new(format!("n{i}.md")).unwrap();
        vault.write_atomic(&path, b"x\n").unwrap();
    }
    assert_eq!(vault.path_locks().len(), 0);
    // A failed write releases its entry too.
    let missing = VaultPath::new("missing.md").unwrap();
    let _ = vault.write_patch(&missing, crate::hash::hash_bytes(b"x"), &edit("y"));
    assert_eq!(vault.path_locks().len(), 0);
}

use crate::Expect;

/// `write_file` with a base hash racing `save_patch`: the patch is parked between its
/// re-hash and its rename. The guarded whole-file write must wait, then see the patched
/// file and refuse; without the check under the lock it passes the check first and both
/// return `Ok`.
#[test]
fn write_atomic_if_hash_racing_write_patch_yields_one_ok_and_one_stale_hash() {
    let (dir, vault) = vault();
    let path = VaultPath::new("note.md").unwrap();
    let base = vault.write_atomic(&path, b"aaa\nrest\n").unwrap();
    let (parked_tx, parked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            vault.write_patch_hooked(
                &path,
                base,
                &edit("AAA"),
                || {},
                move || {
                    parked_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                },
            )
        });
        parked_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let b = scope.spawn(|| vault.write_atomic_if(&path, Expect::Hash(base), b"whole\n"));
        std::thread::sleep(Duration::from_millis(300));
        release_tx.send(()).unwrap();
        (a.join().unwrap(), b.join().unwrap())
    });
    assert!(a.is_ok(), "{a:?}");
    assert!(matches!(b, Err(VaultError::StaleHash { .. })), "{b:?}");
    assert_eq!(
        fs::read(dir.path().join("note.md")).unwrap(),
        b"AAA\nrest\n"
    );
    assert!(temps(dir.path()).is_empty());
}

/// Two creates of one path: the first is parked before its rename; the second must
/// wait and then find the file there.
#[test]
fn two_concurrent_creates_yield_one_ok_and_one_stale_hash() {
    let (dir, vault) = vault();
    let path = VaultPath::new("new.md").unwrap();
    let (parked_tx, parked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            vault.write_atomic_hooked(&path, b"first\n", Some(Expect::Absent), move || {
                parked_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            })
        });
        parked_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let b = scope.spawn(|| vault.write_atomic_if(&path, Expect::Absent, b"second\n"));
        std::thread::sleep(Duration::from_millis(300));
        release_tx.send(()).unwrap();
        (a.join().unwrap(), b.join().unwrap())
    });
    assert!(a.is_ok(), "{a:?}");
    assert!(matches!(b, Err(VaultError::StaleHash { .. })), "{b:?}");
    assert_eq!(fs::read(dir.path().join("new.md")).unwrap(), b"first\n");
}

#[test]
fn write_atomic_if_refuses_a_mismatch_and_a_vanished_file() {
    let (_dir, vault) = vault();
    let path = VaultPath::new("n.md").unwrap();
    let x = crate::hash::hash_bytes(b"x");
    let missing = vault.write_atomic_if(&path, Expect::Hash(x), b"y");
    assert!(
        matches!(missing, Err(VaultError::NotFound { .. })),
        "{missing:?}"
    );
    vault
        .write_atomic_if(&path, Expect::Absent, b"one\n")
        .unwrap();
    let over = vault.write_atomic_if(&path, Expect::Absent, b"two\n");
    assert!(
        matches!(over, Err(VaultError::StaleHash { .. })),
        "{over:?}"
    );
    let wrong = vault.write_atomic_if(&path, Expect::Hash(x), b"y");
    assert!(
        matches!(wrong, Err(VaultError::StaleHash { .. })),
        "{wrong:?}"
    );
    let ok = crate::hash::hash_bytes(b"one\n");
    vault
        .write_atomic_if(&path, Expect::Hash(ok), b"three\n")
        .unwrap();
}
