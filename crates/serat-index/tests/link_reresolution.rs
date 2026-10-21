//! Link re-resolution on create, delete and rename (task #15; FR-NAV-04, NFR-CMP-01;
//! architecture §4.5 A1/A2/B1 and the rename steps).
//!
//! The property: after any sequence of incremental operations, the index equals
//! the index a fresh rebuild of the same tree would produce -- row for row, with
//! every note id re-expressed as its path (C-02: the index is disposable, so an
//! index that drifts from its own rebuild is a defect, not a quirk).

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::types::Value;
use serat_core::{IndexOp, IndexWriter as _, VaultPath};
use serat_index::{IndexHandle, index_path, rebuild, schema};
use support::{FakeVault, LineParser, SpyEcho};

type Tree = BTreeMap<String, String>;

fn vault_of(tree: &Tree) -> FakeVault {
    let files: Vec<(&str, &str)> = tree.iter().map(|(p, b)| (p.as_str(), b.as_str())).collect();
    FakeVault::with(&files)
}

/// What a caller of the link graph can see: every note and every link, with the
/// destination named by path rather than by id.
fn snapshot(index_dir: &Path) -> Vec<String> {
    let conn = schema::open(&index_path(index_dir)).expect("open");
    let mut out = Vec::new();
    for sql in [
        "SELECT 'note', path, file_name, kind, path_fold, file_name_fold FROM notes ORDER BY path",
        "SELECT 'link', src.path, l.kind, l.dst_path, l.dst_fold, dst.path
           FROM links l
           JOIN notes src ON src.id = l.src_note
           LEFT JOIN notes dst ON dst.id = l.dst_note
          ORDER BY src.path, l.kind, l.dst_path, l.id",
    ] {
        let mut stmt = conn.prepare(sql).expect("prepare");
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                let mut parts = Vec::new();
                for i in 0..n {
                    let v: Value = row.get(i)?;
                    parts.push(format!("{v:?}"));
                }
                Ok(parts.join("|"))
            })
            .expect("query");
        out.extend(rows.map(|r| r.expect("row")));
    }
    out
}

fn rebuilt(tree: &Tree) -> Vec<String> {
    let dir = tempfile::tempdir().expect("temp");
    rebuild(
        &vault_of(tree),
        &LineParser::default(),
        Path::new("/vault"),
        dir.path(),
    )
    .expect("rebuild");
    snapshot(dir.path())
}

/// An incrementally maintained index and the tree it is supposed to describe.
struct Live {
    dir: tempfile::TempDir,
    handle: IndexHandle,
    tree: Tree,
}

impl Live {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp");
        let handle = IndexHandle::open(dir.path()).expect("open");
        Self {
            dir,
            handle,
            tree: Tree::new(),
        }
    }

    fn save(&mut self, path: &str, body: &str) {
        self.tree.insert(path.to_owned(), body.to_owned());
        self.handle
            .apply(
                &vault_of(&self.tree),
                &LineParser::default(),
                &SpyEcho::default(),
                &VaultPath::new(path).expect("path"),
            )
            .expect("apply");
        self.handle.writer().flush().expect("flush");
    }

    fn delete(&mut self, path: &str) {
        self.tree.remove(path);
        self.handle
            .writer()
            .submit(IndexOp::Remove {
                path: VaultPath::new(path).expect("path"),
            })
            .expect("submit");
        self.handle.writer().flush().expect("flush");
    }

    fn rename(&mut self, from: &str, to: &str) {
        let body = self.tree.remove(from).expect("exists");
        self.tree.insert(to.to_owned(), body);
        self.handle
            .writer()
            .submit(IndexOp::Rename {
                from: VaultPath::new(from).expect("path"),
                to: VaultPath::new(to).expect("path"),
            })
            .expect("submit");
        self.handle.writer().flush().expect("flush");
    }

    fn assert_equals_a_rebuild(&self, what: &str) {
        let live = snapshot(self.dir.path());
        let fresh = rebuilt(&self.tree);
        assert_eq!(
            live, fresh,
            "after {what}: the incremental index differs from a rebuild"
        );
    }

    /// The path `src`'s link with written target `target` points at, if resolved.
    fn dst_of(&self, src: &str, target: &str) -> Option<String> {
        let conn = schema::open(&index_path(self.dir.path())).expect("open");
        conn.query_row(
            "SELECT dst.path FROM links l
               JOIN notes s ON s.id = l.src_note
               LEFT JOIN notes dst ON dst.id = l.dst_note
              WHERE s.path = ?1 AND l.dst_path = ?2",
            [src, target],
            |row| row.get(0),
        )
        .expect("link row")
    }
}

#[test]
fn a_shorter_path_appearing_takes_over_the_links_that_resolved_to_a_longer_one() {
    // A2. Obsidian resolves [[Meeting]] to the shortest path.
    let mut live = Live::new();
    live.save("refs.md", "[[Meeting]]\n");
    live.save("a/b/Meeting.md", "# long\n");
    assert_eq!(
        live.dst_of("refs.md", "Meeting").as_deref(),
        Some("a/b/Meeting.md")
    );
    live.save("a/Meeting.md", "# shorter\n");
    assert_eq!(
        live.dst_of("refs.md", "Meeting").as_deref(),
        Some("a/Meeting.md")
    );
    live.assert_equals_a_rebuild("a shorter same-named note appeared");
    live.save("Meeting.md", "# root\n");
    assert_eq!(
        live.dst_of("refs.md", "Meeting").as_deref(),
        Some("Meeting.md")
    );
    live.assert_equals_a_rebuild("a root note appeared");
}

#[test]
fn a_content_save_of_a_linked_note_with_a_rival_leaves_links_as_a_rebuild_would() {
    // NFR-PRF-03: a content save no longer runs RIVALS. The links must come out the
    // same anyway -- inbound links to the saved note, and links that point at the
    // longer-path rival (via a path-qualified spelling) alike.
    let mut live = Live::new();
    live.save("a/Meeting.md", "# one\n");
    live.save("a/b/Meeting.md", "# rival\n");
    live.save(
        "refs.md",
        "[[Meeting]]\n[[a/b/Meeting]]\n[[a/Meeting]]\n[[Missing]]\n",
    );
    live.save("other.md", "[[Meeting]]\n");
    live.assert_equals_a_rebuild("setup");
    live.save("a/Meeting.md", "# one, edited\n[[refs]]\n");
    assert_eq!(
        live.dst_of("refs.md", "Meeting").as_deref(),
        Some("a/Meeting.md")
    );
    assert_eq!(
        live.dst_of("refs.md", "a/b/Meeting").as_deref(),
        Some("a/b/Meeting.md")
    );
    live.assert_equals_a_rebuild("a content save of a linked note with a rival");
    live.save("a/b/Meeting.md", "# rival, edited\n");
    live.assert_equals_a_rebuild("a content save of the rival");
}

#[test]
fn deleting_the_preferred_note_hands_its_links_to_the_next_one_or_unresolves_them() {
    // B1.
    let mut live = Live::new();
    live.save("a/Meeting.md", "# one\n");
    live.save("a/b/Meeting.md", "# two\n");
    live.save("refs.md", "[[Meeting]]\n[[a/Meeting]]\n");
    assert_eq!(
        live.dst_of("refs.md", "Meeting").as_deref(),
        Some("a/Meeting.md")
    );
    live.delete("a/Meeting.md");
    assert_eq!(
        live.dst_of("refs.md", "Meeting").as_deref(),
        Some("a/b/Meeting.md")
    );
    assert_eq!(live.dst_of("refs.md", "a/Meeting"), None);
    live.assert_equals_a_rebuild("the preferred note was deleted");
    live.delete("a/b/Meeting.md");
    assert_eq!(live.dst_of("refs.md", "Meeting"), None);
    live.assert_equals_a_rebuild("the last note was deleted");
}

#[test]
fn a_rename_unresolves_links_written_for_the_old_name_and_resolves_those_for_the_new() {
    let mut live = Live::new();
    live.save("old/Plan.md", "# plan\n");
    live.save(
        "refs.md",
        "[[old/Plan]]\n[[Plan]]\n[[new/Plan]]\n[[Other]]\n",
    );
    assert_eq!(
        live.dst_of("refs.md", "old/Plan").as_deref(),
        Some("old/Plan.md")
    );
    assert_eq!(live.dst_of("refs.md", "new/Plan"), None);
    live.rename("old/Plan.md", "new/Plan.md");
    assert_eq!(live.dst_of("refs.md", "old/Plan"), None, "stale spelling");
    assert_eq!(
        live.dst_of("refs.md", "new/Plan").as_deref(),
        Some("new/Plan.md")
    );
    assert_eq!(
        live.dst_of("refs.md", "Plan").as_deref(),
        Some("new/Plan.md")
    );
    live.assert_equals_a_rebuild("a rename across directories");
    live.rename("new/Plan.md", "new/Other.md");
    assert_eq!(live.dst_of("refs.md", "Plan"), None);
    assert_eq!(
        live.dst_of("refs.md", "Other").as_deref(),
        Some("new/Other.md")
    );
    live.assert_equals_a_rebuild("a rename that changes the file name");
}

#[test]
fn resolution_ignores_ascii_case() {
    let mut live = Live::new();
    live.save("Projects/Serat.md", "# serat\n");
    live.save(
        "refs.md",
        "[[serat]]\n[[SERAT]]\n[[projects/serat]]\n[[PROJECTS/SERAT.md]]\n[[Projects/Serat]]\n",
    );
    for target in [
        "serat",
        "SERAT",
        "projects/serat",
        "PROJECTS/SERAT.md",
        "Projects/Serat",
    ] {
        assert_eq!(
            live.dst_of("refs.md", target).as_deref(),
            Some("Projects/Serat.md"),
            "{target}"
        );
    }
    live.assert_equals_a_rebuild("case-insensitive links");
    // And a case-only rename keeps resolving.
    live.rename("Projects/Serat.md", "Projects/SERAT.md");
    assert_eq!(
        live.dst_of("refs.md", "serat").as_deref(),
        Some("Projects/SERAT.md")
    );
    live.assert_equals_a_rebuild("a case-only rename");
}

#[test]
fn resolution_folds_unicode_case_not_just_ascii() {
    // NFR-CMP-01, architecture §4.5: [[émile]] reaches Émile.md as in Obsidian, which
    // COLLATE NOCASE (ASCII only) would not do.
    let mut live = Live::new();
    live.save("people/Émile.md", "# Émile\n");
    live.save(
        "refs.md",
        "[[émile]]\n[[ÉMILE]]\n[[people/émile]]\n[[Émile]]\n",
    );
    for target in ["émile", "ÉMILE", "people/émile", "Émile"] {
        assert_eq!(
            live.dst_of("refs.md", target).as_deref(),
            Some("people/Émile.md"),
            "{target}"
        );
    }
    live.assert_equals_a_rebuild("a non-ASCII name");
}

#[test]
fn an_all_caps_greek_name_ending_in_sigma_resolves_in_both_cases() {
    // The case str::to_lowercase gets wrong: the file name carries ".md", the written
    // target does not. Compositional folding keeps the final sigma the same on both sides.
    let mut live = Live::new();
    live.save("ΟΔΟΣ.md", "# road\n");
    live.save("dir/ΟΔΟΣ/inner.md", "# inner\n");
    live.save(
        "refs.md",
        "[[ΟΔΟΣ]]\n[[οδος]]\n[[οδοσ]]\n[[dir/οδος/inner]]\n",
    );
    for target in ["ΟΔΟΣ", "οδος", "οδοσ"] {
        assert_eq!(
            live.dst_of("refs.md", target).as_deref(),
            Some("ΟΔΟΣ.md"),
            "{target}"
        );
    }
    assert_eq!(
        live.dst_of("refs.md", "dir/οδος/inner").as_deref(),
        Some("dir/ΟΔΟΣ/inner.md"),
        "a sigma before a '/' folds the same as one at the end of a name"
    );
    live.assert_equals_a_rebuild("a Greek name");
}

#[test]
fn an_asset_embed_resolves_case_insensitively() {
    let mut live = Live::new();
    live.save("refs.md", "[[Report.PDF]]\n");
    live.save("files/report.pdf", "%PDF");
    assert_eq!(
        live.dst_of("refs.md", "Report.PDF").as_deref(),
        Some("files/report.pdf")
    );
    live.assert_equals_a_rebuild("an asset appeared");
    live.delete("files/report.pdf");
    live.assert_equals_a_rebuild("an asset disappeared");
}

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

#[test]
fn a_random_walk_of_creates_deletes_renames_and_edits_always_equals_a_rebuild() {
    const PATHS: [&str; 12] = [
        "Meeting.md",
        "a/Meeting.md",
        "a/b/Meeting.md",
        "b/meeting.md",
        "Plan.md",
        "a/Plan.md",
        "x/y/Notes.md",
        "Notes.md",
        "files/pic.png",
        "pic.png",
        "Émile.md",
        "x/ΟΔΟΣ.md",
    ];
    const TARGETS: [&str; 16] = [
        "Meeting",
        "meeting",
        "a/Meeting",
        "A/B/MEETING",
        "Plan.md",
        "plan",
        "Notes",
        "x/y/notes",
        "pic.png",
        "PIC.PNG",
        "files/pic.png",
        "nothing",
        "émile",
        "ÉMILE",
        "οδος",
        "x/οδοσ",
    ];
    for seed in 1..=6u64 {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0x1234_5678_9ABC_DEF1));
        let mut live = Live::new();
        for step in 0..60 {
            let path = PATHS[rng.below(PATHS.len())];
            let what = match rng.below(4) {
                0 | 1 => {
                    if path.ends_with(".png") {
                        live.save(path, "PNG");
                    } else {
                        let body: String = (0..1 + rng.below(4))
                            .map(|_| format!("[[{}]]\n", TARGETS[rng.below(TARGETS.len())]))
                            .collect();
                        live.save(path, &body);
                    }
                    format!("save {path}")
                }
                2 => {
                    if live.tree.contains_key(path) {
                        live.delete(path);
                    }
                    format!("delete {path}")
                }
                _ => {
                    let to = PATHS[rng.below(PATHS.len())];
                    let is_png = |p: &str| p.ends_with(".png");
                    if live.tree.contains_key(path)
                        && !live.tree.contains_key(to)
                        && is_png(path) == is_png(to)
                    {
                        live.rename(path, to);
                    }
                    format!("rename {path} -> {to}")
                }
            };
            live.assert_equals_a_rebuild(&format!("seed {seed} step {step}: {what}"));
        }
    }
}
