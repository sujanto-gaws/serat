//! The tree stream's ordering obligations, asserted on what a front end would see
//! (ADR-0015 §1; task #13: "a remove racing a pending listing, and an upsert racing
//! a pending listing, each asserting the stream the front end would see").
//!
//! [`Front`] is the smallest front end that follows the contract: a listing replaces a
//! directory, an upsert inserts after the named sibling, a remove deletes (and takes a
//! directory's subtree), and a remove or upsert for a directory it has no listing for is
//! a no-op. Every test drives [`TreeModel`] and feeds the messages to it.

use std::collections::BTreeMap;

use serat_core::{
    NoteKind, RawEntry, TreeEntry, TreeKind, TreeModel, TreeOp, group_listings, source_entries,
};

#[derive(Default)]
struct Front {
    dirs: BTreeMap<String, Vec<TreeEntry>>,
}

impl Front {
    fn apply(&mut self, ops: impl IntoIterator<Item = TreeOp>) {
        for op in ops {
            match op {
                TreeOp::Listing { dir, entries } => {
                    self.dirs.insert(dir, entries);
                }
                TreeOp::Upsert { dir, entry, after } => {
                    let Some(list) = self.dirs.get_mut(&dir) else {
                        continue;
                    };
                    list.retain(|e| e.name != entry.name);
                    let at = match after {
                        None => 0,
                        Some(name) => {
                            list.iter()
                                .position(|e| e.name == name)
                                .unwrap_or_else(|| panic!("after {name:?} is not in {dir:?}"))
                                + 1
                        }
                    };
                    list.insert(at, entry);
                }
                TreeOp::Remove { dir, name } => {
                    if let Some(list) = self.dirs.get_mut(&dir) {
                        list.retain(|e| e.name != name);
                    }
                    let path = if dir.is_empty() {
                        name
                    } else {
                        format!("{dir}/{name}")
                    };
                    let prefix = format!("{path}/");
                    self.dirs
                        .retain(|d, _| *d != path && !d.starts_with(&prefix));
                }
            }
        }
    }

    fn names(&self, dir: &str) -> Vec<String> {
        self.dirs
            .get(dir)
            .map(|l| l.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default()
    }
}

fn file(name: &str) -> RawEntry {
    RawEntry::new(name, TreeKind::Note)
}

fn folder(name: &str) -> RawEntry {
    RawEntry::new(name, TreeKind::Dir)
}

fn list(model: &mut TreeModel, dir: &str, raw: Vec<RawEntry>) -> Option<TreeOp> {
    let snap = model.begin_snapshot();
    let op = model.list(snap, dir, raw).into_iter().next();
    model.end_snapshot(snap);
    op
}

#[test]
fn a_listing_is_directories_first_then_case_folded_then_byte_order() {
    let mut model = TreeModel::new();
    let op = list(
        &mut model,
        "",
        vec![
            file("b.md"),
            file("B.md"),
            folder("zeta"),
            file("a.md"),
            folder("Alpha"),
            file("Ä.md"),
            file("ä.md"),
        ],
    )
    .expect("first listing is always sent");
    let TreeOp::Listing { entries, .. } = op else {
        panic!("not a listing");
    };
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    // B.md sorts before b.md: they fold alike and 'B' < 'b' in bytes.
    assert_eq!(
        names,
        ["Alpha", "zeta", "a.md", "B.md", "b.md", "Ä.md", "ä.md"]
    );
}

#[test]
fn an_identical_catch_up_listing_is_not_sent_again() {
    let mut model = TreeModel::new();
    assert!(list(&mut model, "", vec![file("a.md"), file("b.md")]).is_some());
    assert!(list(&mut model, "", vec![file("b.md"), file("a.md")]).is_none());
    assert!(list(&mut model, "", vec![file("a.md")]).is_some());
}

#[test]
fn a_listing_sent_after_a_remove_reflects_the_remove() {
    // The walk began, read the root (with "gone.md" in it), and a delete event then
    // reached the stream before the walk's listing did.
    let mut model = TreeModel::new();
    let mut front = Front::default();
    let snap = model.begin_snapshot();
    let read = vec![file("gone.md"), file("kept.md")];

    // The directory is not listed yet: the remove has nothing to say to the front end.
    let ops = model.remove("", "gone.md");
    assert!(ops.is_empty());
    front.apply(ops);

    front.apply(model.list(snap, "", read));
    model.end_snapshot(snap);
    assert_eq!(front.names(""), ["kept.md"]);
}

#[test]
fn a_remove_after_a_listing_was_sent_still_beats_an_older_catch_up_listing() {
    // Index listing sent; then the file is deleted; then the walk's listing, read
    // before the delete, arrives. It must not bring the file back.
    let mut model = TreeModel::new();
    let mut front = Front::default();
    front.apply(list(&mut model, "", vec![file("a.md"), file("gone.md")]));

    let snap = model.begin_snapshot();
    let stale_read = vec![file("a.md"), file("gone.md"), file("new.md")];
    front.apply(model.remove("", "gone.md"));
    assert_eq!(front.names(""), ["a.md"]);

    front.apply(model.list(snap, "", stale_read));
    model.end_snapshot(snap);
    assert_eq!(front.names(""), ["a.md", "new.md"]);
}

#[test]
fn a_listing_of_a_removed_directory_or_a_descendant_of_one_is_dropped() {
    let mut model = TreeModel::new();
    let mut front = Front::default();
    front.apply(list(&mut model, "", vec![folder("old")]));

    let snap = model.begin_snapshot();
    front.apply(model.remove("", "old"));
    assert!(model.list(snap, "old", vec![file("x.md")]).is_empty());
    assert!(model.list(snap, "old/deep", vec![file("x.md")]).is_empty());
    model.end_snapshot(snap);
    assert!(front.dirs.keys().all(|d| !d.starts_with("old")));
}

#[test]
fn an_upsert_into_a_directory_not_yet_listed_is_superseded_by_its_listing() {
    let mut model = TreeModel::new();
    let mut front = Front::default();
    front.apply(list(&mut model, "", vec![folder("docs")]));

    // The walk is reading docs/ when a create arrives for docs/fresh.md.
    let snap = model.begin_snapshot();
    let read_before_create = vec![file("a.md")];
    let ops = model.upsert("docs", file("fresh.md"));
    assert!(
        ops.is_empty(),
        "nothing to say about an unlisted directory: {ops:?}"
    );
    front.apply(ops);

    front.apply(model.list(snap, "docs", read_before_create));
    model.end_snapshot(snap);
    assert_eq!(front.names("docs"), ["a.md", "fresh.md"]);
}

#[test]
fn an_upsert_names_the_sibling_it_follows() {
    let mut model = TreeModel::new();
    let mut front = Front::default();
    front.apply(list(
        &mut model,
        "",
        vec![folder("d"), file("a.md"), file("c.md")],
    ));
    let ops = model.upsert("", file("b.md"));
    assert_eq!(
        ops,
        [TreeOp::Upsert {
            dir: String::new(),
            entry: TreeEntry {
                name: "b.md".into(),
                kind: TreeKind::Note,
                sidecar: false
            },
            after: Some("a.md".into())
        }]
    );
    front.apply(ops);
    assert_eq!(front.names(""), ["d", "a.md", "b.md", "c.md"]);

    // First in its group: a folder sorts before every file, and after "d".
    let ops = model.upsert("", folder("a-dir"));
    front.apply(ops);
    assert_eq!(front.names(""), ["a-dir", "d", "a.md", "b.md", "c.md"]);
}

#[test]
fn a_repeated_create_or_delete_sends_nothing() {
    let mut model = TreeModel::new();
    list(&mut model, "", vec![file("a.md")]);
    assert!(model.upsert("", file("a.md")).is_empty());
    assert!(model.remove("", "never-there.md").is_empty());
}

#[test]
fn a_new_folder_is_listed_empty_and_a_removed_folder_takes_its_subtree() {
    let mut model = TreeModel::new();
    let mut front = Front::default();
    front.apply(list(&mut model, "", vec![folder("a")]));
    front.apply(list(&mut model, "a", vec![folder("b")]));
    front.apply(list(&mut model, "a/b", vec![file("n.md")]));

    front.apply(model.upsert("", folder("fresh")));
    assert!(
        front.dirs.contains_key("fresh"),
        "a created folder stayed pending"
    );

    front.apply(model.remove("", "a"));
    assert_eq!(front.names(""), ["fresh"]);
    assert!(front.dirs.keys().all(|d| !d.starts_with('a')));
    assert!(!model.is_listed("a/b"));
}

#[test]
fn an_asset_folds_its_sidecar_and_unfolds_it_when_the_asset_goes() {
    let mut model = TreeModel::new();
    let mut front = Front::default();
    front.apply(list(
        &mut model,
        "",
        vec![
            RawEntry::new("x.pdf", TreeKind::Asset),
            file("x.pdf.md"),
            file("y.md"),
        ],
    ));
    assert_eq!(front.names(""), ["x.pdf", "y.md"]);
    assert!(front.dirs[""][0].sidecar);
    assert_eq!(model.file_count(), 3, "the fold is display only");

    front.apply(model.remove("", "x.pdf"));
    assert_eq!(front.names(""), ["x.pdf.md", "y.md"]);

    front.apply(model.upsert("", RawEntry::new("x.pdf", TreeKind::Asset)));
    assert_eq!(front.names(""), ["x.pdf", "y.md"]);
    assert!(front.dirs[""][0].sidecar);

    front.apply(model.remove("", "x.pdf.md"));
    assert!(!front.dirs[""][0].sidecar);
}

#[test]
fn group_listings_is_root_first_then_breadth_first_and_omits_empty_directories() {
    let rows = [
        ("z/deep/deeper/n.md", TreeKind::Note),
        ("a.md", TreeKind::Note),
        ("z/n.md", TreeKind::Note),
        ("b/x.pdf", TreeKind::Asset),
    ];
    let listings = group_listings(rows);
    let dirs: Vec<_> = listings.iter().map(|(d, _)| d.as_str()).collect();
    assert_eq!(dirs, ["", "b", "z", "z/deep", "z/deep/deeper"]);
    let root: Vec<_> = listings[0].1.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(listings[0].1.len(), 3);
    assert!(root.contains(&"a.md") && root.contains(&"b") && root.contains(&"z"));
    assert_eq!(
        group_listings(std::iter::empty()).len(),
        1,
        "an empty vault still has a root"
    );
}

/// xorshift64*: deterministic, so a failure reproduces from its seed.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }
}

#[test]
fn a_random_create_delete_replay_ends_where_a_fresh_listing_does() {
    // ADR-0015's "Negative" clause: an upsert at the wrong `after` is wrong silently,
    // so replay random live changes (names chosen to collide in case, folding and
    // sidecar pairing) and compare the front end with a listing built from scratch.
    const NAMES: &[&str] = &[
        "a.md", "A.md", "b.md", "B.md", "ä.md", "Ä.md", "x.pdf", "x.pdf.md", "y.png", "y.png.md",
        "docs", "Docs", "z", "ZZ.md",
    ];
    for seed in 1..=60u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut model = TreeModel::new();
        let mut front = Front::default();
        let mut truth: BTreeMap<&str, TreeKind> = BTreeMap::new();
        front.apply(list(&mut model, "", Vec::new()));

        for step in 0..120 {
            let name = NAMES[rng.below(NAMES.len())];
            let kind = if matches!(name, "docs" | "Docs" | "z") {
                TreeKind::Dir
            } else if name.ends_with(".md") {
                TreeKind::Note
            } else {
                TreeKind::Asset
            };
            // "A.md" and "a.md" cannot coexist on a case-insensitive file system, but
            // the model must not care; distinct names are distinct entries to it.
            let ops = if rng.below(3) == 0 {
                truth.remove(name);
                model.remove("", name)
            } else {
                truth.insert(name, kind);
                model.upsert("", RawEntry::new(name, kind))
            };
            front.apply(ops);

            let mut fresh = TreeModel::new();
            let raw = truth.iter().map(|(n, k)| RawEntry::new(*n, *k)).collect();
            let expected = list(&mut fresh, "", raw).expect("listing");
            let TreeOp::Listing { entries, .. } = expected else {
                unreachable!()
            };
            assert_eq!(
                front.dirs[""], entries,
                "seed {seed}, step {step}: the incrementally maintained list diverged"
            );
        }
    }
}

#[test]
fn a_directory_removed_and_recreated_does_not_resurrect_its_stale_read() {
    // code-reviewer-3's probe 1 (B1). The walk read a/b, then a/b was deleted and
    // created again; the recreate replaced the overlay's "removed" mark, and the read
    // was listed after Remove(a, b).
    let mut model = TreeModel::new();
    let mut front = Front::default();
    front.apply(list(&mut model, "", vec![folder("a")]));
    front.apply(list(&mut model, "a", vec![folder("b")]));
    front.apply(list(&mut model, "a/b", vec![file("x.md")]));

    let snap = model.begin_snapshot();
    let stale = vec![file("x.md"), file("y.md")];
    front.apply(model.remove("a", "b"));
    front.apply(model.upsert("a", folder("b")));
    let late = model.list(snap, "a/b", stale);
    model.end_snapshot(snap);

    assert!(late.is_empty(), "a pre-remove read was listed: {late:?}");
    front.apply(late);
    assert_eq!(front.names("a/b"), Vec::<String>::new());
}

#[test]
fn an_ancestor_removed_and_recreated_does_not_resurrect_a_stale_read_beneath_it() {
    // Probe 2: the same through an ancestor, and the later re-create of a/b must not
    // find a stale listing already sitting in the model.
    let mut model = TreeModel::new();
    let mut front = Front::default();
    front.apply(list(&mut model, "", vec![folder("a")]));
    front.apply(list(&mut model, "a", vec![folder("b")]));

    let snap = model.begin_snapshot();
    let stale = vec![file("x.md")];
    front.apply(model.remove("", "a"));
    front.apply(model.upsert("", folder("a")));
    let late = model.list(snap, "a/b", stale);
    model.end_snapshot(snap);
    assert!(late.is_empty(), "{late:?}");

    front.apply(model.upsert("a", folder("b")));
    assert_eq!(front.names("a/b"), Vec::<String>::new());
    assert!(
        front.dirs.contains_key("a/b"),
        "the re-created folder has no listing"
    );
}

#[test]
fn a_folder_created_between_its_parents_read_and_listing_gets_a_listing() {
    // Probe 3 (S1): the overlay adds the folder to the parent's listing, and no walk
    // will ever queue it, so it would sit pending for ever.
    let mut model = TreeModel::new();
    let mut front = Front::default();
    let snap = model.begin_snapshot();
    let early = model.upsert("", folder("newdir"));
    assert!(early.is_empty());
    front.apply(model.list(snap, "", vec![file("a.md")]));
    model.end_snapshot(snap);
    assert_eq!(front.names(""), ["newdir", "a.md"]);
    assert!(front.dirs.contains_key("newdir"), "newdir stayed pending");
    assert!(model.is_listed("newdir"));
}

#[test]
fn a_listing_converts_to_source_entries_without_its_directories() {
    // ADR-0015 section 2, One walk at open: the tree walk feeds the catch-up.
    let listing = [
        RawEntry::new("notes", TreeKind::Dir),
        RawEntry::new("a.md", TreeKind::Note).with_metadata(12, 1_700_000_000_123),
        RawEntry::new("scan.pdf", TreeKind::Asset).with_metadata(900, 5),
    ];
    let converted = source_entries("10-projects", &listing);
    let shown: Vec<_> = converted
        .iter()
        .map(|e| (e.path.as_str(), e.kind, e.size, e.mtime_ms))
        .collect();
    assert_eq!(
        shown,
        [
            ("10-projects/a.md", NoteKind::Note, 12, 1_700_000_000_123),
            ("10-projects/scan.pdf", NoteKind::Asset, 900, 5),
        ]
    );
    let root = source_entries("", &listing);
    assert_eq!(root[0].path.as_str(), "a.md");
}
