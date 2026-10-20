//! The tree producer end to end on a real directory: the walk's shape, the session's
//! source rules, and the ordering obligations under real concurrency (ADR-0015 §1–§2;
//! FR-VLT-01, FR-VLT-04).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use serat_core::{TreeKind, TreeOp};
use serat_vault::{TreeOut, TreeSession, TreeSource, Vault};

fn write(root: &Path, relative: &str) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    fs::write(target, b"x").expect("write");
}

fn session() -> (TreeSession, Arc<Mutex<Vec<TreeOut>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    (
        TreeSession::new(move |out| sink.lock().expect("log").push(out)),
        log,
    )
}

/// The smallest front end that follows the contract (see serat-core's tree_stream).
#[derive(Default)]
struct Front {
    dirs: BTreeMap<String, Vec<String>>,
}

impl Front {
    fn apply(&mut self, stream: &[TreeOut]) {
        for out in stream {
            let TreeOut::Op(op) = out else { continue };
            match op {
                TreeOp::Listing { dir, entries } => {
                    self.dirs.insert(
                        dir.clone(),
                        entries.iter().map(|e| e.name.clone()).collect(),
                    );
                }
                TreeOp::Upsert { dir, entry, after } => {
                    let Some(list) = self.dirs.get_mut(dir) else {
                        continue;
                    };
                    list.retain(|n| *n != entry.name);
                    let at = after.as_ref().map_or(0, |a| {
                        list.iter().position(|n| n == a).expect("after exists") + 1
                    });
                    list.insert(at, entry.name.clone());
                }
                TreeOp::Remove { dir, name } => {
                    if let Some(list) = self.dirs.get_mut(dir) {
                        list.retain(|n| n != name);
                    }
                }
            }
        }
    }
}

fn fixture() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path();
    write(root, "a.md");
    write(root, "10-projects/serat.md");
    write(root, "10-projects/notes/deep/meeting.md");
    write(root, "40-refs/contract.pdf");
    fs::create_dir_all(root.join("empty/inner")).expect("mkdir");
    fs::create_dir_all(root.join("only-ignored")).expect("mkdir");
    write(root, "only-ignored/.DS_Store");
    write(root, ".obsidian/workspace.json");
    write(root, ".serat/index.db");
    let vault = Vault::open(root).expect("open");
    (dir, vault)
}

#[test]
fn the_walk_lists_root_first_then_breadth_first_and_counts_empty_folders() {
    let (_dir, vault) = fixture();
    let mut order = Vec::new();
    let summary = vault
        .walk_tree("", |dir, _| order.push(dir.to_owned()))
        .expect("walk");

    assert_eq!(order[0], "", "the root listing is first");
    let depth = |d: &String| {
        if d.is_empty() {
            0
        } else {
            d.matches('/').count() + 1
        }
    };
    assert!(
        order.windows(2).all(|w| depth(&w[0]) <= depth(&w[1])),
        "not breadth-first: {order:?}"
    );
    assert!(
        !order
            .iter()
            .any(|d| d.starts_with(".obsidian") || d.starts_with(".serat"))
    );
    assert_eq!(summary.files, 4);
    // empty, empty/inner and only-ignored hold no file beneath them.
    assert_eq!(summary.empty_folders, 3);
}

#[cfg(unix)]
#[test]
fn the_walk_does_not_follow_a_symlink() {
    let (dir, vault) = fixture();
    std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).expect("symlink");
    std::os::unix::fs::symlink(dir.path().join("a.md"), dir.path().join("link.md")).expect("link");
    let mut seen = Vec::new();
    vault
        .walk_tree("", |d, raw| {
            seen.push((
                d.to_owned(),
                raw.into_iter().map(|e| e.name).collect::<Vec<_>>(),
            ));
        })
        .expect("walk");
    assert!(seen.iter().all(|(d, _)| !d.starts_with("loop")));
    assert!(
        seen.iter()
            .all(|(_, names)| !names.iter().any(|n| n == "loop" || n == "link.md"))
    );
}

#[test]
fn a_walk_found_tree_completes_with_source_walk_and_matches_the_vault_walk() {
    let (_dir, vault) = fixture();
    let (session, log) = session();
    session.populate_from_walk(&vault).expect("walk");
    let stream = log.lock().expect("log").clone();

    let TreeOut::Op(TreeOp::Listing { dir, .. }) = &stream[0] else {
        panic!("first message is not the root listing: {:?}", stream[0]);
    };
    assert_eq!(dir, "");
    let expected = u32::try_from(vault.walk().expect("walk").len()).expect("fits");
    assert_eq!(
        stream[stream.len() - 2],
        TreeOut::Complete {
            entries: expected,
            source: TreeSource::Walk
        }
    );
    assert_eq!(
        stream[stream.len() - 1],
        TreeOut::WalkSettled { empty_folders: 0 }
    );
}

#[test]
fn an_index_populated_tree_is_corrected_by_the_walk_with_only_the_differences() {
    let (dir, vault) = fixture();
    let (session, log) = session();
    // The index knows every file except one, and one it should not.
    let rows: Vec<(String, TreeKind)> = vault
        .walk()
        .expect("walk")
        .into_iter()
        .filter(|e| e.path.as_str() != "10-projects/serat.md")
        .map(|e| {
            let kind = match e.kind {
                serat_core::NoteKind::Note => TreeKind::Note,
                serat_core::NoteKind::Asset => TreeKind::Asset,
            };
            (e.path.to_string(), kind)
        })
        .chain([("ghost/gone.md".to_owned(), TreeKind::Note)])
        .collect();
    assert!(session.populate_from_index(rows.iter().map(|(p, k)| (p.as_str(), *k))));

    {
        let stream = log.lock().expect("log");
        assert!(matches!(
            stream.last(),
            Some(TreeOut::Complete {
                source: TreeSource::Index,
                ..
            })
        ));
        let TreeOut::Op(TreeOp::Listing { dir, .. }) = &stream[0] else {
            panic!("root first");
        };
        assert_eq!(dir, "");
    }
    let before = log.lock().expect("log").len();

    session.populate_from_walk(&vault).expect("walk");
    let stream = log.lock().expect("log").clone();
    let listings: Vec<&str> = stream[before..]
        .iter()
        .filter_map(|o| match o {
            TreeOut::Op(TreeOp::Listing { dir, .. }) => Some(dir.as_str()),
            _ => None,
        })
        .collect();
    // The root differs (ghost dir) and 10-projects differs (serat.md); the walk also
    // finds the folders the index could not know, but sends nothing for a directory
    // whose listing it already sent identically.
    assert!(
        listings.contains(&"") && listings.contains(&"10-projects"),
        "{listings:?}"
    );
    assert!(
        !listings.contains(&"40-refs"),
        "an identical listing was re-sent: {listings:?}"
    );
    assert_eq!(
        stream.last(),
        Some(&TreeOut::WalkSettled { empty_folders: 3 })
    );

    let mut front = Front::default();
    front.apply(&stream);
    assert!(
        front.dirs[""].iter().all(|n| n != "ghost"),
        "a stale index row survived the walk"
    );
    assert!(front.dirs["10-projects"].contains(&"serat.md".to_owned()));
    drop(dir);
}

#[test]
fn once_the_walk_has_populated_the_index_tree_is_not_sent() {
    let (_dir, vault) = fixture();
    let (session, log) = session();
    session.populate_from_walk(&vault).expect("walk");
    let before = log.lock().expect("log").len();
    assert!(!session.populate_from_index([("a.md", TreeKind::Note)]));
    assert_eq!(log.lock().expect("log").len(), before);
}

#[test]
fn live_changes_are_single_messages_and_a_modification_is_none() {
    let (_dir, vault) = fixture();
    let (session, log) = session();
    session.populate_from_walk(&vault).expect("walk");
    let before = log.lock().expect("log").len();

    session.created("10-projects/new.md", TreeKind::Note);
    session.deleted("a.md");
    let stream = log.lock().expect("log").clone();
    assert_eq!(stream.len() - before, 2);
    assert!(
        matches!(&stream[before], TreeOut::Op(TreeOp::Upsert { dir, .. }) if dir == "10-projects")
    );
    assert!(
        matches!(&stream[before + 1], TreeOut::Op(TreeOp::Remove { dir, name }) if dir.is_empty() && name == "a.md")
    );
}

#[test]
fn creates_and_deletes_racing_the_walk_never_leave_the_front_end_wrong() {
    // The watcher path in miniature: change the disk, then tell the session, while the
    // walk is reading the same directories on another thread. Whatever the
    // interleaving, once both finish the stream must describe the disk: a deleted file
    // that a stale directory read still contained must not come back, and a created
    // one must be present whether or not that read saw it. No final rescan, which
    // would repair a bug rather than expose it.
    const DIRS: usize = 24;
    const FILES: usize = 40;
    for round in 0..12 {
        let dir = tempfile::tempdir().expect("temp");
        for d in 0..DIRS {
            for f in 0..FILES {
                write(dir.path(), &format!("d{d:02}/f{f:02}.md"));
            }
        }
        let vault = Arc::new(Vault::open(dir.path()).expect("open"));
        let (session, log) = session();
        let session = Arc::new(session);

        let walker = {
            let (vault, session) = (Arc::clone(&vault), Arc::clone(&session));
            thread::spawn(move || session.populate_from_walk(&vault).expect("walk"))
        };
        let mutator = {
            let (root, session) = (dir.path().to_path_buf(), Arc::clone(&session));
            thread::spawn(move || {
                for d in (0..DIRS).rev() {
                    for f in (0..FILES).step_by(2) {
                        let gone = format!("d{d:02}/f{f:02}.md");
                        fs::remove_file(root.join(&gone)).expect("remove");
                        session.deleted(&gone);
                        let made = format!("d{d:02}/new{f:02}.md");
                        write(&root, &made);
                        session.created(&made, TreeKind::Note);
                    }
                }
            })
        };
        walker.join().expect("walker");
        mutator.join().expect("mutator");

        let mut front = Front::default();
        front.apply(&log.lock().expect("log"));
        for d in 0..DIRS {
            let mut disk: Vec<String> = fs::read_dir(dir.path().join(format!("d{d:02}")))
                .expect("read_dir")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            disk.sort_by_key(|n| n.to_lowercase());
            let shown = front
                .dirs
                .get(&format!("d{d:02}"))
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                shown, disk,
                "round {round}, d{d:02}: the stream disagrees with the disk"
            );
        }
    }
}
