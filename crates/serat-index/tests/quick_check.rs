//! `quick_check` on its own read connection (task #69; NFR-REL-01, NFR-REL-02, C-02;
//! ADR-0002, ADR-0011, ADR-0001/#154).

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serat_core::{IndexWriter as _, VaultPath};
use serat_index::lock::LOCK_FILE;
use serat_index::{IndexHandle, QuickCheck, index_path, quick_check, rebuild, schema};
use support::{FakeVault, LineParser, SpyEcho, note_body};

const ROOT: &str = "C:/vaults/serat";

/// A rebuilt index of `n` notes, closed: one file, no writer.
fn built(n: usize) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp");
    let index_dir = dir.path().join(".serat");
    let (names, bodies): (Vec<String>, Vec<String>) = (0..n)
        .map(|i| (format!("notes/n{i:04}.md"), note_body(i)))
        .unzip();
    let files: Vec<(&str, &str)> = names
        .iter()
        .zip(&bodies)
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    rebuild(
        &FakeVault::with(&files),
        &LineParser::default(),
        Path::new(ROOT),
        &index_dir,
    )
    .expect("rebuild");
    (dir, index_dir)
}

#[test]
fn a_clean_index_is_clean() {
    let (_dir, index_dir) = built(200);
    assert_eq!(quick_check(&index_dir).expect("check"), QuickCheck::Clean);
}

#[test]
fn a_missing_index_is_an_error_not_a_verdict() {
    let dir = tempfile::tempdir().expect("temp");
    assert!(quick_check(&dir.path().join(".serat")).is_err());
}

/// Overwrite the root page of `table` (writer closed, so the file is whole) with 0xFF.
fn damage_root_page_of(index_dir: &Path, table: &str) {
    let path = index_path(index_dir);
    let (root, page_size): (i64, i64) = {
        let conn = schema::open(&path).expect("open");
        let root = conn
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .expect("rootpage");
        let size = conn
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .expect("page size");
        (root, size)
    };
    let mut bytes = std::fs::read(&path).expect("read");
    let start = usize::try_from((root - 1) * page_size).expect("offset");
    let end = start + usize::try_from(page_size).expect("size");
    for byte in &mut bytes[start..end] {
        *byte = 0xFF;
    }
    std::fs::write(&path, bytes).expect("write");
}

#[test]
fn a_damaged_page_is_reported_as_problems_and_never_panics() {
    for table in ["notes", "blocks", "links"] {
        let (_dir, index_dir) = built(200);
        damage_root_page_of(&index_dir, table);
        match quick_check(&index_dir).expect("a damaged page is a verdict, not an error") {
            QuickCheck::Problems(found) => {
                assert!(
                    !found.is_empty() && found.iter().all(|f| !f.is_empty()),
                    "{table}"
                );
            }
            QuickCheck::Clean => panic!("{table}: a 0xFF root page passed quick_check"),
        }
    }
}

/// `PRAGMA quick_check` straight on the file: `Ok(rows)` if it answers, `Err` if it raises.
fn raw_quick_check(index_dir: &Path) -> Result<Vec<String>, rusqlite::Error> {
    let conn = rusqlite::Connection::open_with_flags(
        index_path(index_dir),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut stmt = conn.prepare("PRAGMA quick_check")?;
    stmt.query_map([], |row| row.get::<_, String>(0))?.collect()
}

/// Zero the cell-pointer array of one table-leaf page, keeping its header: the cells then
/// point at offset 0 and SQLite describes the damage as rows instead of raising.
fn zero_cell_pointers(index_dir: &Path, page: usize) {
    let path = index_path(index_dir);
    let mut bytes = std::fs::read(&path).expect("read");
    let size = 4096;
    let base = page * size;
    let cells = usize::from(u16::from_be_bytes([bytes[base + 3], bytes[base + 4]]));
    for byte in &mut bytes[base + 8..base + 8 + 2 * cells] {
        *byte = 0;
    }
    std::fs::write(&path, bytes).expect("write");
}

#[test]
fn damage_that_quick_check_describes_as_rows_is_problems_with_those_rows() {
    // The other way SQLite says corrupt (architecture §3.4): no error, a first row that is
    // not `ok`. A leaf page of a big table, header kept, pointers zeroed.
    let (dir, index_dir) = built(400);
    let size = 4096;
    let pages = std::fs::metadata(index_path(&index_dir))
        .expect("meta")
        .len() as usize
        / size;
    let mut checked = 0;
    for page in 1..pages {
        let leaf = {
            let bytes = std::fs::read(index_path(&index_dir)).expect("read");
            bytes[page * size] == 0x0D && bytes[page * size + 4] != 0
        };
        if !leaf {
            continue;
        }
        let copy = dir.path().join(format!("copy-{page}"));
        std::fs::create_dir_all(&copy).expect("mkdir");
        std::fs::copy(index_path(&index_dir), index_path(&copy)).expect("copy");
        zero_cell_pointers(&copy, page);
        let Ok(rows) = raw_quick_check(&copy) else {
            continue; // this page makes SQLite raise; the other tests cover that path
        };
        if rows.first().map(String::as_str) == Some("ok") {
            continue;
        }
        assert_eq!(
            quick_check(&copy).expect("verdict"),
            QuickCheck::Problems(rows),
            "page {page}"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "no leaf page produced findings as rows; the fixture proves nothing"
    );
}

#[test]
fn a_file_that_is_not_a_database_is_problems() {
    let (_dir, index_dir) = built(5);
    std::fs::write(index_path(&index_dir), vec![0x41u8; 8192]).expect("clobber");
    // Stale WAL/SHM from the closed rebuild would describe the old file; the rebuild
    // checkpointed them away, so what is left is the garbage alone.
    assert!(matches!(
        quick_check(&index_dir).expect("verdict"),
        QuickCheck::Problems(_)
    ));
}

#[test]
fn it_completes_beside_a_live_writer_and_does_not_take_its_lock() {
    let (_dir, index_dir) = built(200);
    let handle = IndexHandle::open(&index_dir).expect("writer");
    assert!(index_dir.join(LOCK_FILE).exists());
    let vault = FakeVault::with(&[("extra/new.md", "# New\n")]);
    let parser = LineParser::default();
    let echo = SpyEcho::default();

    std::thread::scope(|s| {
        let writer = s.spawn(|| {
            for i in 0..50 {
                let _ = i;
                handle
                    .apply(
                        &vault,
                        &parser,
                        &echo,
                        &VaultPath::new("extra/new.md").expect("path"),
                    )
                    .expect("save");
                handle.writer().flush().expect("flush");
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        for _ in 0..5 {
            assert_eq!(
                quick_check(&index_dir).expect("check beside a writer"),
                QuickCheck::Clean
            );
        }
        writer.join().expect("writer thread");
    });
    // The writer still holds its lock: quick_check neither took nor released it.
    assert!(
        IndexHandle::open(&index_dir).is_err(),
        "the check disturbed the writer lock"
    );
    handle.close().expect("close");
}
