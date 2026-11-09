//! What `index_meta.vault_root` is, and what a mismatch is worth.
//!
//! Architecture §3.4 step 5, task #120, F-458 and F-459.
//!
//! # Two questions, one column, and only one of them is worth a rebuild
//!
//! *Is this index this vault's index?* is worth acting on. *Has this vault's path
//! changed?* is not: `notes.path` is vault-relative (`schema/001_init.sql:134`) and so
//! is `asset_path` (`apply::upsert_asset` stores the same `VaultPath`), so a vault
//! that moved carries an index in which every row is still exactly right. The column
//! was answering the second question and charging the price of the first — a full
//! rebuild, 28.6 s on the 20,200-note fixture, with no index for its duration and
//! nothing in NFR-REL-02's log to tell it apart from real corruption (F-459).
//!
//! So this module does two things and they are deliberately separate:
//!
//! 1. [`canonical_root`] makes the recorded identity a *resolved directory* rather
//!    than the bytes of whatever argument a caller happened to hold. That is not a new
//!    decision — the DDL has documented the column as *"canonical vault path this
//!    index was built from"* since it was written, and the code recorded
//!    `to_string_lossy()` instead.
//! 2. [`describes`] answers the question the string comparison could not, at a price
//!    the open path can afford: does this index name any file that is actually here?
//!
//! # Why a probe and not a bare demotion
//!
//! Task #120's ruling said to report a mismatch and fall through to *"the
//! reconciliation the open path already performs"*. There is none — nothing consumes
//! `serat_vault::walk::paths_of`, and §3.4's catch-up reconciliation is in the *after
//! usable* column with nothing built in it. A bare demotion would therefore have
//! served a genuinely foreign index as `Ready`, indefinitely, with no mechanism that
//! would ever notice: the ruling's own accepted cost ("reconciliation discovers every
//! file differs, which costs about what the rebuild would have cost") is a cost
//! nothing is currently paying.
//!
//! [`describes`] is that finding, sampled: it is *reconciliation's own answer* to the
//! only question the trigger needs, taken over [`SAMPLE`] rows instead of the whole
//! tree. It keeps the ruling's point — a rebuild is triggered by evidence about the
//! index's contents, never by two strings differing — and drops its dependency on
//! machinery that does not exist.

use std::path::Path;

use rusqlite::Connection;

/// How many of the index's own rows are checked before a mismatch is believed.
///
/// Bounded, because this sits inside NFR-PRF-01's window: 32 `metadata()` calls
/// against §3.4's ~393 ms of headroom after the open check, on a path that only runs
/// at all when the recorded root differs. Sampled from both ends of `notes.id` rather
/// than the head alone, so deleting one directory tree cannot look like a foreign
/// vault.
const SAMPLE: i64 = 16;

/// What the index's own rows say about the tree at this root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// At least one path in the index names a file that is here. The index is this
    /// vault's; the root it was recorded under is out of date and nothing more.
    Describes,
    /// Not one sampled path is here. This index was built somewhere else.
    Foreign,
}

/// The identity to record and to compare against: the resolved directory.
///
/// A mapped drive, a junction, a UNC form, a differently-cased root and a trailing
/// separator are five spellings of one directory, and every one of them used to buy a
/// full rebuild (F-459). `canonicalize` collapses them.
///
/// Falls back to the path as given when it cannot be resolved — a root that has just
/// become unreadable is the caller's problem to report, not a reason for this to fail
/// — and strips Windows' verbatim `\\?\` prefix so the recorded value matches
/// `serat_vault::Vault::root()`, which strips it for the same reason. The two agreeing
/// **by construction rather than by coincidence** is the whole of F-458's lesson.
#[must_use]
pub fn canonical_root(vault_root: &Path) -> String {
    let resolved = std::fs::canonicalize(vault_root).unwrap_or_else(|_| vault_root.to_path_buf());
    let text = resolved.to_string_lossy();
    let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);

    // Only for the fallback above: `canonicalize` never returns a trailing separator,
    // but a raw path may, and `C:\vault\` and `C:\vault` are one directory. Guarded so
    // a root that is nothing but separators does not become the empty string.
    let trimmed = plain.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        plain.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Ask the index whether it describes the tree at `vault_root`.
///
/// Reconciliation's finding, over [`SAMPLE`] rows from each end of `notes` rather than
/// over all of them. The threshold is deliberately the weakest one that can refute
/// anything: **one** surviving path is enough to call the index this vault's.
///
/// That asymmetry is the point. A false `Foreign` costs a silent 28.6 s rebuild, which
/// is the defect this whole change exists to remove; a false `Describes` costs stale
/// query results on an index C-02 already calls disposable, and is what task #120's
/// ruling accepted *unconditionally* for every mismatch. So the bar is set where it
/// can only ever improve on the ruling as written, never regress it — and the one case
/// it does catch, a `.serat/` copied between two vaults, is the only case the check
/// was ever really for.
///
/// An index with no rows cannot refute anything and is reported as [`Verdict::Describes`]:
/// there is nothing in it to be wrong, and an empty index costs nothing to keep.
pub(crate) fn describes(conn: &Connection, vault_root: &Path) -> Verdict {
    let mut sampled: Vec<String> = Vec::new();
    for sql in [
        "SELECT path FROM notes ORDER BY id LIMIT ?1",
        "SELECT path FROM notes ORDER BY id DESC LIMIT ?1",
    ] {
        let Ok(mut statement) = conn.prepare(sql) else {
            // A `notes` table we cannot read is not evidence of a foreign vault. The
            // stamp's own corruption steps own that case; this one declines to guess.
            return Verdict::Describes;
        };
        let Ok(rows) = statement.query_map([SAMPLE], |row| row.get::<_, String>(0)) else {
            return Verdict::Describes;
        };
        sampled.extend(rows.flatten());
    }

    if sampled.is_empty() {
        return Verdict::Describes;
    }

    // `notes.path` is vault-relative with `/` separators, which `Path::join` accepts on
    // both platforms. `symlink_metadata` rather than `exists()`: the question is
    // whether the name is occupied, and a broken symlink still means this index is
    // describing this tree.
    if sampled
        .iter()
        .any(|path| vault_root.join(path).symlink_metadata().is_ok())
    {
        Verdict::Describes
    } else {
        Verdict::Foreign
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_spelling_of_one_directory_canonicalises_to_one_identity() {
        // F-459's whole content, at the level of the function that fixes it.
        let dir = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(dir.path().join("sub")).expect("sub");

        let plain = canonical_root(dir.path());
        for spelling in [
            dir.path().join("."),
            dir.path().join("sub").join(".."),
            std::path::PathBuf::from(format!("{}/", dir.path().display())),
        ] {
            assert_eq!(
                canonical_root(&spelling),
                plain,
                "{} must be the same vault as {}",
                spelling.display(),
                dir.path().display()
            );
        }
    }

    #[test]
    fn an_unresolvable_root_falls_back_rather_than_failing() {
        // Design goal 5. A root that has gone is reported by whoever tried to open it;
        // this must still produce a comparable identity rather than panicking.
        let missing = Path::new("C:/no-such-vault-anywhere/x");
        assert!(!canonical_root(missing).is_empty());
        assert_eq!(
            canonical_root(Path::new("C:/no-such-vault-anywhere/x/")),
            canonical_root(missing),
            "the trailing separator must not make a second vault out of one path"
        );
    }

    #[test]
    fn two_vaults_are_two_identities() {
        // The guard that keeps the test above from passing on a function that returns
        // a constant.
        let a = tempfile::tempdir().expect("temp");
        let b = tempfile::tempdir().expect("temp");
        assert_ne!(canonical_root(a.path()), canonical_root(b.path()));
    }

    #[test]
    fn an_index_naming_files_that_are_here_describes_this_vault() {
        let dir = tempfile::tempdir().expect("temp");
        std::fs::write(dir.path().join("a.md"), "a").expect("write");
        let conn = seeded(&["a.md", "gone.md"]);
        assert_eq!(describes(&conn, dir.path()), Verdict::Describes);
    }

    #[test]
    fn an_index_naming_nothing_that_is_here_is_foreign() {
        let dir = tempfile::tempdir().expect("temp");
        std::fs::write(dir.path().join("a.md"), "a").expect("write");
        let conn = seeded(&["somewhere/else.md", "another.md"]);
        assert_eq!(describes(&conn, dir.path()), Verdict::Foreign);
    }

    #[test]
    fn one_surviving_path_out_of_many_is_enough() {
        // The threshold, pinned. A vault most of whose notes were deleted while the
        // application was closed is still that vault, and must not buy a rebuild for
        // it — that is F-459's cost arriving by a second route.
        let dir = tempfile::tempdir().expect("temp");
        std::fs::write(dir.path().join("survivor.md"), "s").expect("write");
        let mut paths: Vec<String> = (0..200).map(|n| format!("deleted/{n}.md")).collect();
        paths.push("survivor.md".to_owned());
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        assert_eq!(describes(&seeded(&refs), dir.path()), Verdict::Describes);
    }

    #[test]
    fn an_empty_index_cannot_refute_anything() {
        let dir = tempfile::tempdir().expect("temp");
        assert_eq!(describes(&seeded(&[]), dir.path()), Verdict::Describes);
    }

    /// An in-memory index holding just these note paths.
    fn seeded(paths: &[&str]) -> Connection {
        let conn = crate::schema::open_in_memory().expect("open");
        crate::schema::bootstrap(&conn).expect("bootstrap");
        for (n, path) in paths.iter().enumerate() {
            conn.execute(
                "INSERT INTO notes (id, path, kind, mtime, size, hash) VALUES (?1, ?2, 'note', 0, 0, 'h')",
                rusqlite::params![n as i64 + 1, path],
            )
            .expect("insert");
        }
        conn
    }
}
