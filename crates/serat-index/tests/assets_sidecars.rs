//! Asset rows and sidecar association (task #17; FR-BIN-01, FR-BIN-02, FR-BIN-03,
//! FR-VLT-08; architecture §7.4; ADR-0005, ADR-0017).
//!
//! The property that matters (C-02): after any sequence of incremental operations the
//! index equals the index a fresh rebuild of the same tree produces, including `assets`
//! and the `asset_path`/`sidecar_path` columns.

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::types::Value;
use serat_core::{IndexOp, IndexWriter as _, VaultPath};
use serat_index::read::Reader;
use serat_index::{IndexHandle, index_path, rebuild, schema};
use support::{FakeVault, LineParser, SpyEcho};

type Tree = BTreeMap<String, String>;

fn vault_of(tree: &Tree) -> FakeVault {
    let files: Vec<(&str, &str)> = tree.iter().map(|(p, b)| (p.as_str(), b.as_str())).collect();
    FakeVault::with(&files)
}

/// Every note and asset row with the columns this task writes, plus the asset table.
fn snapshot(index_dir: &Path) -> Vec<String> {
    let conn = schema::open(&index_path(index_dir)).expect("open");
    let mut out = Vec::new();
    for sql in [
        "SELECT 'note', n.path, n.kind, n.asset_path, n.sidecar_path, a.mime, a.bytes
           FROM notes n LEFT JOIN assets a ON a.note_id = n.id ORDER BY n.path",
        "SELECT 'orphan', count(*) FROM assets a WHERE NOT EXISTS
                (SELECT 1 FROM notes n WHERE n.id = a.note_id AND n.kind = 'asset')",
        "SELECT 'bare', n.path FROM notes n
          WHERE n.kind = 'asset' AND NOT EXISTS (SELECT 1 FROM assets a WHERE a.note_id = n.id)",
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
        assert_eq!(
            snapshot(self.dir.path()),
            rebuilt(&self.tree),
            "after {what}: the incremental index differs from a rebuild"
        );
    }

    fn sidecar_of(&self, asset: &str) -> Option<String> {
        let conn = schema::open(&index_path(self.dir.path())).expect("open");
        conn.query_row(
            "SELECT sidecar_path FROM notes WHERE path = ?1 AND kind = 'asset'",
            [asset],
            |row| row.get(0),
        )
        .expect("asset row")
    }

    fn reader(&self) -> Reader {
        Reader::open(&index_path(self.dir.path())).expect("reader")
    }
}

#[test]
fn an_asset_row_carries_mime_from_the_extension_and_size_from_the_stat() {
    let mut live = Live::new();
    live.save("refs/report.PDF", "%PDF-1.7");
    live.save("img/a.png", "PNGDATA");
    live.save("misc/blob.weird", "xyz");
    let conn = schema::open(&index_path(live.dir.path())).expect("open");
    let rows: Vec<(String, String, i64)> = conn
        .prepare(
            "SELECT n.path, a.mime, a.bytes FROM assets a JOIN notes n ON n.id = a.note_id
              ORDER BY n.path",
        )
        .expect("prepare")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .map(|r| r.expect("row"))
        .collect();
    assert_eq!(
        rows,
        [
            ("img/a.png".to_owned(), "image/png".to_owned(), 7),
            (
                "misc/blob.weird".to_owned(),
                "application/octet-stream".to_owned(),
                3
            ),
            (
                "refs/report.PDF".to_owned(),
                "application/pdf".to_owned(),
                8
            ),
        ]
    );
    live.assert_equals_a_rebuild("three assets");
}

#[test]
fn a_sidecar_pairs_with_its_asset_in_either_order_and_clears_when_either_goes() {
    let mut live = Live::new();
    // Asset first.
    live.save("refs/x.pdf", "%PDF");
    assert_eq!(live.sidecar_of("refs/x.pdf"), None);
    live.save("refs/x.pdf.md", "# About x\n");
    assert_eq!(
        live.sidecar_of("refs/x.pdf").as_deref(),
        Some("refs/x.pdf.md")
    );
    live.assert_equals_a_rebuild("sidecar created after its asset");

    // Sidecar content save keeps the pair.
    live.save("refs/x.pdf.md", "# About x, edited\n");
    assert_eq!(
        live.sidecar_of("refs/x.pdf").as_deref(),
        Some("refs/x.pdf.md")
    );
    live.assert_equals_a_rebuild("sidecar re-saved");

    // Deleting the sidecar clears the pairing; the asset stays.
    live.delete("refs/x.pdf.md");
    assert_eq!(live.sidecar_of("refs/x.pdf"), None);
    live.assert_equals_a_rebuild("sidecar deleted");

    // Sidecar first, then the asset (also the order a rebuild may meet them in).
    live.save("refs/y.pdf.md", "# About y\n");
    live.save("refs/y.pdf", "%PDF");
    assert_eq!(
        live.sidecar_of("refs/y.pdf").as_deref(),
        Some("refs/y.pdf.md")
    );
    live.assert_equals_a_rebuild("asset created after its sidecar");

    // Deleting the asset leaves the sidecar an ordinary note.
    live.delete("refs/y.pdf");
    live.assert_equals_a_rebuild("asset deleted");
}

#[test]
fn the_names_are_compared_byte_for_byte_and_within_one_directory() {
    // ADR-0017: X.PDF and x.pdf.md are not a pair; nor are different directories.
    let mut live = Live::new();
    live.save("a/X.PDF", "%PDF");
    live.save("a/x.pdf.md", "# note\n");
    live.save("b/x.pdf", "%PDF");
    assert_eq!(live.sidecar_of("a/X.PDF"), None);
    assert_eq!(live.sidecar_of("b/x.pdf"), None);
    live.assert_equals_a_rebuild("near-miss names");
}

#[test]
fn renames_move_the_pairing_the_mime_and_the_asset_path_with_the_file() {
    let mut live = Live::new();
    live.save("x.pdf", "%PDF");
    live.save("x.pdf.md", "# x\n");
    live.save("z.png", "PNG");
    live.save("z.png.md", "# z\n");
    live.assert_equals_a_rebuild("setup");

    // The sidecar moves away: the pair breaks. Then it moves onto another asset.
    live.rename("x.pdf.md", "q.md");
    assert_eq!(live.sidecar_of("x.pdf"), None);
    live.assert_equals_a_rebuild("sidecar renamed away");
    live.rename("q.md", "z.png.md.md");
    live.rename("z.png.md.md", "w.md");
    live.assert_equals_a_rebuild("sidecar renamed twice");

    // The asset moves: its sidecar is whatever <new>.md is.
    live.rename("z.png", "x.png");
    live.save("x.png.md", "# now paired\n");
    assert_eq!(live.sidecar_of("x.png").as_deref(), Some("x.png.md"));
    live.assert_equals_a_rebuild("asset renamed then sidecar created");
    live.rename("x.png", "v.pdf");
    assert_eq!(live.sidecar_of("v.pdf"), None);
    live.assert_equals_a_rebuild("asset renamed to a different type");
}

#[test]
fn attachments_list_every_asset_with_the_shortest_resolving_link() {
    let mut live = Live::new();
    live.save("refs/report.pdf", "%PDF");
    live.save("refs/report.pdf.md", "# r\n");
    live.save("img/logo.png", "PNG");
    live.save("deep/er/logo.png", "PNG2");
    live.save("data/sheet.xlsx", "XLSX");
    live.save("notes.md", "# n\n");
    live.handle.writer().flush().expect("flush");

    let attachments = live.reader().attachments().expect("attachments");
    let by_path = |p: &str| attachments.iter().find(|a| a.path == p).expect(p);
    assert_eq!(attachments.len(), 4);
    assert!(attachments.windows(2).all(|w| w[0].path < w[1].path));

    let text = |p: &str| by_path(p).link_text.clone();
    let link = |p: &str| by_path(p).link.clone();
    let report = by_path("refs/report.pdf");
    assert_eq!(report.mime, "application/pdf");
    assert_eq!(report.bytes, 4);
    assert_eq!(report.sidecar_path.as_deref(), Some("refs/report.pdf.md"));
    assert_eq!(text("refs/report.pdf").as_deref(), Some("report.pdf"));
    assert_eq!(link("refs/report.pdf").as_deref(), Some("![[report.pdf]]"));

    // Two assets named logo.png: the bare name resolves to the shorter path only.
    assert_eq!(link("img/logo.png").as_deref(), Some("![[logo.png]]"));
    assert_eq!(
        text("deep/er/logo.png").as_deref(),
        Some("deep/er/logo.png")
    );
    assert_eq!(
        link("deep/er/logo.png").as_deref(),
        Some("![[deep/er/logo.png]]")
    );

    // Not viewable: a plain link. image/tiff is in the MIME table but is not served.
    assert_eq!(link("data/sheet.xlsx").as_deref(), Some("[[sheet.xlsx]]"));
}

#[test]
fn a_tiff_is_linked_not_embedded() {
    let mut live = Live::new();
    live.save("scan.tiff", "II*");
    let attachments = live.reader().attachments().expect("attachments");
    assert_eq!(attachments[0].link.as_deref(), Some("[[scan.tiff]]"));
}

#[test]
fn an_asset_no_link_form_resolves_to_has_no_link_text() {
    // Case-sensitive file system: refs/X.PDF and refs/x.pdf. Both bare names fold alike
    // and tie on length, so binary order picks X.PDF; the full path of x.pdf folds to
    // X.PDF's path too. No link form reaches x.pdf, and none is offered.
    let mut live = Live::new();
    live.save("refs/X.PDF", "%PDF");
    live.save("refs/x.pdf", "%PDF");
    let attachments = live.reader().attachments().expect("attachments");
    let get = |p: &str| attachments.iter().find(|a| a.path == p).expect(p);
    assert_eq!(get("refs/X.PDF").link_text.as_deref(), Some("X.PDF"));
    assert_eq!(get("refs/x.pdf").link_text, None);
    assert_eq!(get("refs/x.pdf").link, None);
}

#[test]
fn a_random_walk_over_assets_and_sidecars_always_equals_a_rebuild() {
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }
    const PATHS: [&str; 14] = [
        "x.pdf",
        "x.pdf.md",
        "X.PDF",
        "d/x.pdf",
        "d/x.pdf.md",
        "d/y.png",
        "d/y.png.md",
        "y.png",
        "y.png.md",
        "n.md",
        "d/n.md",
        "e/f.docx",
        "e/f.docx.md",
        "e/f.md",
    ];
    for seed in 1..=8u64 {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0x1234_5678_9ABC_DEF1));
        let mut live = Live::new();
        for step in 0..60 {
            let path = PATHS[rng.below(PATHS.len())];
            let is_note = |p: &str| p.ends_with(".md");
            let what = match rng.below(4) {
                0 | 1 => {
                    let body = "x".repeat(1 + rng.below(9));
                    live.save(path, &body);
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
                    if live.tree.contains_key(path)
                        && !live.tree.contains_key(to)
                        && is_note(path) == is_note(to)
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
