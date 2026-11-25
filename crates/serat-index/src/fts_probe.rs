//! A read-only look at how many FTS5 segments `blocks_fts` holds (task #196).
//!
//! The save path's tail is an FTS5 merge inside one writer commit (task #183, found on
//! Linux with the checkpointer disabled). SQLite offers no per-statement signal for that
//! from outside, so the one observable fact is the shape of the index: a merge replaces
//! several segments with one, so the segment count **falls** across the commit that
//! merged. The benchmarks (`p1-external-change`, `prf03-save-to-index`) read it before
//! and after a unit of work and call a fall "a merge landed in this window".
//!
//! Read-only on a connection of its own: no transaction is left open (each read is one
//! statement, finished), so it pins no WAL, and it touches no `notes` row, so it warms
//! nothing the measured write targets. Not on any product path.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::index_path;

/// A read-only connection that counts `blocks_fts` segments.
#[derive(Debug)]
pub struct FtsProbe {
    conn: Connection,
}

impl FtsProbe {
    /// Open `index_dir`'s `index.db` read-only.
    ///
    /// # Errors
    /// The database cannot be opened.
    pub fn open(index_dir: &Path) -> Result<Self, String> {
        let conn =
            Connection::open_with_flags(index_path(index_dir), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("opening the index read-only: {e}"))?;
        Ok(Self { conn })
    }

    /// Segments across every level of `blocks_fts` right now.
    ///
    /// # Errors
    /// The structure record is missing or does not decode.
    pub fn segments(&self) -> Result<u32, String> {
        segment_count(&self.conn)
    }
}

/// Segments across every level of the FTS5 table `blocks_fts` on `conn`.
///
/// # Errors
/// The structure record (`blocks_fts_data`, id 10) is missing or does not decode.
pub fn segment_count(conn: &Connection) -> Result<u32, String> {
    let blob: Vec<u8> = conn
        .query_row("SELECT block FROM blocks_fts_data WHERE id = 10", [], |r| {
            r.get(0)
        })
        .map_err(|e| format!("blocks_fts_data structure record: {e}"))?;
    decode_segments(&blob)
}

/// Sum of per-level segment counts from an FTS5 structure record
/// (`fts5StructureDecode`: 4-byte cookie, optional 4-byte V2 marker, varints nLevel,
/// nSegment, nWriteCounter, then per level nMerge, nSeg and nSeg segment records).
fn decode_segments(blob: &[u8]) -> Result<u32, String> {
    let bad = || "the FTS5 structure record is truncated".to_owned();
    let mut i = 4;
    if blob.len() < i {
        return Err(bad());
    }
    let v2 = blob.get(i..i + 4) == Some(&[0xFF, 0, 0, 1][..]);
    if v2 {
        i += 4;
    }
    let n_level = varint(blob, &mut i).ok_or_else(bad)?;
    let _n_segment = varint(blob, &mut i).ok_or_else(bad)?;
    let _counter = varint(blob, &mut i).ok_or_else(bad)?;
    let mut total = 0u64;
    for _ in 0..n_level {
        let _n_merge = varint(blob, &mut i).ok_or_else(bad)?;
        let n_seg = varint(blob, &mut i).ok_or_else(bad)?;
        for _ in 0..n_seg {
            for _ in 0..(if v2 { 8 } else { 3 }) {
                varint(blob, &mut i).ok_or_else(bad)?;
            }
        }
        total += n_seg;
    }
    Ok(u32::try_from(total).unwrap_or(u32::MAX))
}

/// SQLite's varint: up to nine bytes, the ninth contributing all eight bits.
fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for n in 0..9 {
        let byte = *b.get(*i)?;
        *i += 1;
        if n == 8 {
            return Some((v << 8) | u64::from(byte));
        }
        v = (v << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            break;
        }
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fts() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch("CREATE VIRTUAL TABLE blocks_fts USING fts5(text);")
            .expect("fts5");
        conn
    }

    #[test]
    fn each_commit_adds_a_segment_and_a_merge_makes_the_count_fall() {
        // Task #196: the signal is deterministic, no timing involved. `automerge` and
        // `crisismerge` are set out of reach so only the explicit merge folds segments.
        let conn = fts();
        conn.execute_batch(
            "INSERT INTO blocks_fts(blocks_fts, rank) VALUES('automerge', 0);
             INSERT INTO blocks_fts(blocks_fts, rank) VALUES('crisismerge', 64);",
        )
        .expect("config");
        let mut last = segment_count(&conn).expect("empty table");
        for i in 0..6 {
            conn.execute(
                "INSERT INTO blocks_fts(text) VALUES (?1)",
                [format!("note {i}")],
            )
            .expect("insert");
            let now = segment_count(&conn).expect("count");
            assert!(now >= last, "a plain commit never reduces the count");
            last = now;
        }
        assert!(last >= 2, "six commits left {last} segments");
        conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES('optimize');")
            .expect("optimize");
        let after = segment_count(&conn).expect("count after");
        assert!(after < last, "optimize merged: {last} -> {after}");
        assert_eq!(after, 1);
    }

    #[test]
    fn a_truncated_structure_record_is_an_error_not_a_panic() {
        assert!(decode_segments(&[0, 0, 0]).is_err());
        assert!(decode_segments(&[0, 0, 0, 0, 1, 1, 1, 0]).is_err());
        assert_eq!(decode_segments(&[0, 0, 0, 0, 0, 0, 0]), Ok(0), "no levels");
    }

    #[test]
    fn varint_reads_one_to_nine_bytes() {
        let mut i = 0;
        assert_eq!(varint(&[0x7f], &mut i), Some(127));
        let mut i = 0;
        assert_eq!(varint(&[0x81, 0x00], &mut i), Some(128));
        let mut i = 0;
        assert_eq!(varint(&[0x81], &mut i), None, "ends mid-varint");
    }
}
