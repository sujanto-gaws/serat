//! The link-resolution statements, written once.
//!
//! Three callers resolve links -- the rebuild's one full pass (`writer`), a save's
//! inbound and outbound passes and the delete/rename passes (`apply`) -- and they must
//! agree on what "names a note" means, or an incrementally maintained index drifts
//! from a rebuilt one (C-02). So the matching rule is one string, spliced into each
//! statement at compile time, rather than copies that a later edit could change one of.
//!
//! # The rule (architecture §4.5, §9.3; NFR-CMP-01)
//!
//! A link's written target names a note `n` when, **after `serat_core::fold_case` on
//! both sides**: `n.path` equals it, or equals it plus `.md`, or -- when the target has
//! no `/`, so it names a file rather than a path -- `n.file_name` equals it or equals it
//! plus `.md`. Among several, the shortest path wins, then the smaller (binary) path,
//! so the choice is total and a rebuild cannot resolve one link two ways on two runs.
//!
//! # Why every comparison is a binary `=` on a stored column
//!
//! `notes.file_name_fold`, `notes.path_fold` and `links.dst_fold` hold the folded
//! strings, computed in Rust when the row is written; `ix_notes_file_name`,
//! `ix_notes_path_fold` and `ix_links_unresolved` index them. A parameter is folded in
//! Rust by the same function before it is bound. No SQL function is registered (C-03),
//! so each statement here is fixed text and SQLite folds nothing. A comparison left on
//! the raw column would make the per-link join a scan -- F-371 -- which
//! `tests::no_resolving_statement_scans_notes_per_link` pins.

/// The notes a link's written target names, over the outer `links` row (columns
/// `id`, `path`).
macro_rules! candidates {
    () => {
        "SELECT n.id AS id, n.path AS path FROM notes n
                    WHERE n.path_fold = links.dst_fold
                  UNION ALL
                  SELECT n.id, n.path FROM notes n
                    WHERE n.path_fold = links.dst_fold || '.md'
                  UNION ALL
                  SELECT n.id, n.path FROM notes n
                    WHERE instr(links.dst_fold, '/') = 0
                      AND n.file_name_fold = links.dst_fold
                  UNION ALL
                  SELECT n.id, n.path FROM notes n
                    WHERE instr(links.dst_fold, '/') = 0
                      AND n.file_name_fold = links.dst_fold || '.md'"
    };
}

/// The best note for the outer link: shortest path, then binary order of the path.
macro_rules! best_note {
    () => {
        concat!(
            "(SELECT id FROM (",
            candidates!(),
            ") ORDER BY length(path), path LIMIT 1)"
        )
    };
}

/// The guard that keeps an `UPDATE` from writing NULL over a link whose target is
/// missing, which would count a dangling link as "resolved".
macro_rules! some_note_exists {
    () => {
        concat!("EXISTS (SELECT 1 FROM (", candidates!(), "))")
    };
}

/// Resolve every unresolved link in the vault. The rebuild's one pass.
pub(crate) const RESOLVE_ALL: &str = concat!(
    "UPDATE links SET dst_note = ",
    best_note!(),
    " WHERE dst_note IS NULL AND kind <> 'tag' AND dst_fold IS NOT NULL AND ",
    some_note_exists!()
);

/// Resolve the unresolved links that could name a note at one path: `?1..?4` are the
/// path's four folded spellings (path, path without `.md`, file name, file name
/// without `.md`). Served by `ix_links_unresolved`. Architecture §4.5 A1 and B1.
pub(crate) const RESOLVE_INBOUND: &str = concat!(
    "UPDATE links SET dst_note = ",
    best_note!(),
    " WHERE dst_note IS NULL AND kind <> 'tag'
        AND dst_fold IN (?1, ?2, ?3, ?4) AND ",
    some_note_exists!()
);

/// Re-point the *resolved* links that carry one of the same spellings but currently
/// point at another note, because the shortest-path rule may now prefer the note that
/// just appeared (architecture §4.5 A2). `?1..?4` are the four folded spellings and
/// `?5..?8` those plus `.md`: the targets a link may carry to reach a note spelled any
/// of them.
///
/// The rivals are found through `ix_notes_path_fold` and `ix_notes_file_name`, and
/// their inbound links through `ix_links_dst`; no index on `links(dst_fold)` beyond
/// the partial one is added, because every index on `links` costs the rebuild once
/// per link (F-373).
pub(crate) const RESOLVE_RIVALS: &str = concat!(
    "UPDATE links SET dst_note = ",
    best_note!(),
    " WHERE kind <> 'tag' AND dst_fold IN (?1, ?2, ?3, ?4)
        AND dst_note IN (
            SELECT id FROM notes
             WHERE path_fold IN (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            UNION
            SELECT id FROM notes
             WHERE file_name_fold IN (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8))
        AND ",
    some_note_exists!()
);

/// Resolve one note's own unresolved outbound links (`?1` = `src_note`).
pub(crate) const RESOLVE_OUTBOUND: &str = concat!(
    "UPDATE links SET dst_note = ",
    best_note!(),
    " WHERE dst_note IS NULL AND kind <> 'tag' AND src_note = ?1
        AND dst_fold IS NOT NULL AND ",
    some_note_exists!()
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    /// The plan of `sql` on an empty, bootstrapped schema, one line per row.
    fn plan(sql: &str) -> String {
        let conn = schema::open_in_memory().expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("prepare");
        let n = stmt.parameter_count();
        let rows: Vec<String> = stmt
            .query_map(
                rusqlite::params_from_iter(std::iter::repeat_n("x", n)),
                |row| row.get::<_, String>(3),
            )
            .expect("plan")
            .map(|r| r.expect("row"))
            .collect();
        rows.join("\n")
    }

    #[test]
    fn no_resolving_statement_scans_notes_per_link() {
        // F-371: one scan of `notes` per link is 217,000 scans and a rebuild that does
        // not finish. Every lookup of a note must be a SEARCH on one of the folded
        // indexes, and the only SCAN allowed is of `links`.
        for (name, sql) in [
            ("ALL", RESOLVE_ALL),
            ("INBOUND", RESOLVE_INBOUND),
            ("RIVALS", RESOLVE_RIVALS),
            ("OUTBOUND", RESOLVE_OUTBOUND),
        ] {
            let plan = plan(sql);
            for line in plan.lines() {
                assert!(
                    !line.starts_with("SCAN n") && !line.starts_with("SCAN notes"),
                    "{name} scans notes:\n{plan}"
                );
            }
            assert!(
                plan.contains("ix_notes_path_fold") && plan.contains("ix_notes_file_name"),
                "{name} never uses the folded indexes:\n{plan}"
            );
        }
    }
}
