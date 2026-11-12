//! The Windows-only name rules through a real walk (task #35; FR-VLT-01, NFR-SEC-06,
//! NFR-CMP-01, ADR-0015 step 4, ADR-0017 step 4).
//!
//! A name only Windows cannot hold is a file like any other on Linux and macOS, so it
//! is shown there and not reported. Judged by the Windows rules, the same vault must
//! report each such name with its reason and count it, never drop it silently.
//! `Vault::with_platform` runs both rule sets from one runner.

use std::fs;

use serat_core::Platform;

use crate::Vault;

/// One file per Windows-only rule, one directory the trailing-dot rule refuses (with a
/// file beneath it), and one name legal everywhere.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    for name in ["fine.md", "aux.md", "Meeting 10:30.md", "Misc./n.md"] {
        let target = dir.path().join(name);
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        fs::write(target, b"x").expect("write");
    }
    dir
}

fn walked(vault: &Vault) -> Vec<String> {
    vault
        .walk()
        .expect("walk")
        .into_iter()
        .map(|entry| entry.path.as_str().to_owned())
        .collect()
}

fn listed(vault: &Vault) -> Vec<String> {
    let mut seen = Vec::new();
    vault
        .walk_tree("", |dir, entries| {
            for entry in entries {
                seen.push(format!("{dir}/{}", entry.name));
            }
        })
        .expect("tree walk");
    seen.sort();
    seen
}

#[test]
fn names_only_windows_refuses_are_shown_and_not_reported_on_this_platform() {
    let dir = fixture();
    let vault = Vault::open(dir.path()).expect("open");

    assert_eq!(
        walked(&vault),
        ["Meeting 10:30.md", "Misc./n.md", "aux.md", "fine.md"],
        "the walk dropped a legal name"
    );
    assert!(
        vault.refusals().is_empty(),
        "{:?}",
        vault.refusals().snapshot()
    );

    let tree = listed(&vault);
    for shown in ["/aux.md", "/Meeting 10:30.md", "/Misc.", "Misc./n.md"] {
        assert!(
            tree.iter().any(|t| t.ends_with(shown)),
            "{shown} missing from the tree: {tree:?}"
        );
    }
    assert!(
        vault.refusals().is_empty(),
        "{:?}",
        vault.refusals().snapshot()
    );
}

#[test]
fn the_windows_rules_report_each_refused_name_with_its_reason_and_count_it() {
    let dir = fixture();
    let vault = Vault::open(dir.path())
        .expect("open")
        .with_platform(Platform::Windows);

    assert_eq!(walked(&vault), ["fine.md"], "a refused name was indexed");

    let by_walk = vault.refusals().snapshot();
    let found: Vec<(&str, &str)> = by_walk
        .iter()
        .map(|r| (r.path.as_str(), r.reason.as_str()))
        .collect();
    assert_eq!(found.len(), 3, "{found:?}");
    let reason = |path: &str| {
        found
            .iter()
            .find(|(p, _)| *p == path)
            .unwrap_or_else(|| panic!("{path} not reported: {found:?}"))
            .1
    };
    assert!(reason("aux.md").contains("reserved Windows device"));
    assert!(reason("Meeting 10:30.md").contains("alternate data stream"));
    assert!(reason("Misc.").contains("trailing dot or space"));
    assert_eq!(vault.refusals().len(), 3);

    // The tree walk agrees with the walk, and the refused directory is one entry,
    // not entered (the file beneath it is not reported separately).
    assert_eq!(listed(&vault), ["/fine.md"]);
    assert_eq!(vault.refusals().snapshot(), by_walk);
}
