//! Stubs shared by serat-index's test files.
//!
//! No Markdown parser anywhere in here, and no filesystem walk except [`DiskVault`],
//! used only by T1 (task #97) where a real vault on real disk is the point of the
//! test (F-136). That is the dependency rule paying for itself: `serat-index` never
//! links `serat-vault` or `serat-parse`, so its tests supply both as trait objects
//! and the index can be driven end to end against inputs a real vault would
//! otherwise be tedious to arrange. `DiskVault` uses `std::fs` only -- no `walkdir`,
//! no `notify`, no `serat-vault` -- so it costs the dependency rule nothing.
#![allow(dead_code)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serat_core::{
    Block, BlockKind, ByteRange, ContentHash, EchoGuard, Field, FieldSource, FieldValue, Link,
    LinkKind, LinkTarget, LoadedNote, NoteKind, NoteMeta, NoteParser, NoteSource, ParseError,
    ParsedNote, SourceEntry, Task, VaultError, VaultPath,
};

/// A vault that lives in a `Vec`.
#[derive(Debug, Default)]
pub struct FakeVault {
    pub files: Vec<(VaultPath, Vec<u8>)>,
    /// Paths whose read fails, to exercise the skip path.
    unreadable: Vec<String>,
    /// Refuse `entries()` outright, which is the one failure a rebuild cannot skip.
    refuse_entries: bool,
    /// How many times `entries()` has been called, so a test can tell whether any
    /// rebuild work has started yet.
    entries_calls: AtomicUsize,
}

impl FakeVault {
    pub fn with(files: &[(&str, &str)]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(path, body)| {
                    (
                        VaultPath::new(path).expect("path"),
                        body.as_bytes().to_vec(),
                    )
                })
                .collect(),
            unreadable: Vec::new(),
            refuse_entries: false,
            entries_calls: AtomicUsize::new(0),
        }
    }

    /// Refuse to enumerate, so a rebuild cannot run at all.
    #[must_use]
    pub fn refusing_to_enumerate(mut self) -> Self {
        self.refuse_entries = true;
        self
    }

    /// How many times the source has been asked to enumerate.
    pub fn entries_called(&self) -> usize {
        self.entries_calls.load(Ordering::Relaxed)
    }

    pub fn unreadable(mut self, path: &str) -> Self {
        self.unreadable.push(path.to_owned());
        self
    }
}

impl NoteSource for FakeVault {
    fn entries(&self) -> Result<Vec<SourceEntry>, VaultError> {
        self.entries_calls.fetch_add(1, Ordering::Relaxed);
        if self.refuse_entries {
            return Err(VaultError::Io {
                path: "<vault root>".to_owned(),
                message: "permission denied".to_owned(),
            });
        }
        Ok(self
            .files
            .iter()
            .map(|(path, bytes)| SourceEntry {
                kind: if path.is_markdown() {
                    NoteKind::Note
                } else {
                    NoteKind::Asset
                },
                size: bytes.len() as u64,
                mtime_ms: 1_700_000_000_000,
                path: path.clone(),
            })
            .collect())
    }

    fn load(&self, path: &VaultPath) -> Result<LoadedNote, VaultError> {
        if self.unreadable.iter().any(|bad| bad == path.as_str()) {
            return Err(VaultError::Io {
                path: path.as_str().to_owned(),
                message: "permission denied".to_owned(),
            });
        }
        let (_, bytes) = self
            .files
            .iter()
            .find(|(candidate, _)| candidate == path)
            .ok_or_else(|| VaultError::NotFound {
                path: path.as_str().to_owned(),
            })?;

        Ok(LoadedNote {
            meta: NoteMeta {
                path: path.clone(),
                hash: ContentHash::from_bytes([0u8; 32]),
                kind: if path.is_markdown() {
                    NoteKind::Note
                } else {
                    NoteKind::Asset
                },
                size: bytes.len() as u64,
                mtime_ms: 1_700_000_000_000,
            },
            bytes: bytes.clone(),
        })
    }
}

/// A parser that produces one block per line and reads `key:: value` naively.
///
/// Not a Markdown parser and not pretending to be one — that is I3. It exists to give
/// the index rows with plausible shapes and exact, predictable counts.
#[derive(Debug, Default)]
pub struct LineParser {
    pub calls: AtomicUsize,
    /// Paths this parser refuses, to exercise the skip path.
    pub refuses: Mutex<Vec<String>>,
}

impl NoteParser for LineParser {
    fn parse(&self, path: &VaultPath, bytes: &[u8]) -> Result<ParsedNote, ParseError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self
            .refuses
            .lock()
            .expect("lock")
            .iter()
            .any(|bad| bad == path.as_str())
        {
            return Err(ParseError::NotUtf8 {
                path: path.as_str().to_owned(),
                offset: 0,
            });
        }

        let text = String::from_utf8_lossy(bytes).into_owned();
        let mut parsed = ParsedNote {
            title: Some(path.stem().to_owned()),
            ..ParsedNote::default()
        };

        let mut offset = 0usize;
        for (ord, line) in text.lines().enumerate() {
            let ord = ord as u32;
            let range = ByteRange::new(offset, offset + line.len());
            offset += line.len() + 1;

            if let Some((key, value)) = line.split_once(":: ") {
                parsed.fields.push(Field {
                    key: key.trim().to_owned(),
                    value: FieldValue::Text(value.trim().to_owned()),
                    source: FieldSource::Inline,
                    range,
                    value_range: ByteRange::new(range.end - value.trim().len(), range.end),
                    block_ord: Some(ord),
                });
                if key.trim() == "type" {
                    parsed.note_type = Some(value.trim().to_owned());
                }
            }
            if let Some(target) = line
                .strip_prefix("[[")
                .and_then(|rest| rest.strip_suffix("]]"))
            {
                parsed.links.push(Link {
                    kind: LinkKind::Wikilink,
                    target: LinkTarget::note(target),
                    range,
                    block_ord: Some(ord),
                });
            }
            if let Some(rest) = line.strip_prefix("- [ ] ") {
                parsed.tasks.push(Task {
                    block_ord: ord,
                    done: false,
                    marker: ' ',
                    text: rest.to_owned(),
                    range,
                    marker_range: ByteRange::new(range.start + 3, range.start + 4),
                });
            }

            parsed.blocks.push(Block {
                ord,
                kind: if line.starts_with("- [ ] ") {
                    BlockKind::Task { depth: 0 }
                } else if line.starts_with('#') {
                    BlockKind::Heading { level: 1 }
                } else {
                    BlockKind::Paragraph
                },
                range,
                text: line.to_owned(),
                heading_path: Vec::new(),
                block_ref: None,
            });
        }

        Ok(parsed)
    }
}

/// An `EchoGuard` that records every hash it is told about, in order.
///
/// Standing in for `serat_vault::Vault` in these tests, which never depend on
/// `serat-vault` at all (the dependency rule, paying for itself again). Integration
/// tests that only care about the index's own behaviour and not about what got
/// recorded can construct one and ignore it.
#[derive(Debug, Default)]
pub struct SpyEcho {
    pub recorded: Mutex<Vec<(VaultPath, ContentHash)>>,
}

impl EchoGuard for SpyEcho {
    fn record_echo(&self, path: &VaultPath, hash: ContentHash) {
        self.recorded
            .lock()
            .expect("lock")
            .push((path.clone(), hash));
    }
}

/// A `NoteSource` backed by a real directory on real disk.
///
/// Everything else in this file is a stub precisely so serat-index never links
/// `serat-vault` (module doc above); this is the one exception, and it is not a
/// dependency-rule violation because it links nothing new — `std::fs` only, no
/// `notify`, no `walkdir`, no `serat-vault`. It exists for T1 (task #97), whose
/// binding gap is that I6's tests run against `FakeVault` and prove classification
/// and ordering, not that a real index file on a real disk behaves the same
/// (F-136). It is deliberately not a general-purpose vault: no ignore rules, no
/// symlink handling, no watcher — just enough real I/O to make "delete .serat/,
/// restart, rebuild" a real filesystem event rather than a second call in the same
/// process.
#[derive(Debug)]
pub struct DiskVault {
    root: std::path::PathBuf,
}

impl DiskVault {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl NoteSource for DiskVault {
    fn entries(&self) -> Result<Vec<SourceEntry>, VaultError> {
        let mut out = Vec::new();
        walk_dir(&self.root, &self.root, &mut out)?;
        // Deterministic order (the trait's own requirement): sorted by path rather
        // than trusting read_dir's order, which the OS does not promise to repeat.
        out.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
        Ok(out)
    }

    fn load(&self, path: &VaultPath) -> Result<LoadedNote, VaultError> {
        let full = self.root.join(path.as_str());
        let bytes = std::fs::read(&full).map_err(|_| VaultError::NotFound {
            path: path.as_str().to_owned(),
        })?;
        let metadata = std::fs::metadata(&full).map_err(|err| VaultError::Io {
            path: path.as_str().to_owned(),
            message: err.to_string(),
        })?;
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis() as i64);
        Ok(LoadedNote {
            meta: NoteMeta {
                hash: disk_hash(&bytes),
                kind: if path.is_markdown() {
                    NoteKind::Note
                } else {
                    NoteKind::Asset
                },
                size: bytes.len() as u64,
                mtime_ms,
                path: path.clone(),
            },
            bytes,
        })
    }
}

fn walk_dir(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<SourceEntry>,
) -> Result<(), VaultError> {
    let read = std::fs::read_dir(dir).map_err(|err| VaultError::Io {
        path: dir.display().to_string(),
        message: err.to_string(),
    })?;
    for entry in read {
        let entry = entry.map_err(|err| VaultError::Io {
            path: dir.display().to_string(),
            message: err.to_string(),
        })?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir(root, &path, out)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("walked entries are under root")
            .to_string_lossy()
            .into_owned();
        let vault_path = VaultPath::new(relative).expect("a walked path is a valid VaultPath");
        let metadata = entry.metadata().map_err(|err| VaultError::Io {
            path: vault_path.as_str().to_owned(),
            message: err.to_string(),
        })?;
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis() as i64);
        out.push(SourceEntry {
            kind: if vault_path.is_markdown() {
                NoteKind::Note
            } else {
                NoteKind::Asset
            },
            size: metadata.len(),
            mtime_ms,
            path: vault_path,
        });
    }
    Ok(())
}

/// A hash over real bytes, so two rebuilds of unchanged files on disk produce the
/// same `notes.hash` for a reason -- the content -- rather than because every
/// fixture shares one hard-coded constant the way [`FakeVault`] does. Not BLAKE3:
/// serat-index takes no dependency that would give it one (§2.1), and this crate's
/// dev-dependencies stay to `tempfile` for the same reason a product crate's would.
/// `DefaultHasher` is unspecified across Rust versions but fixed within one build,
/// which is all a same-process, same-binary equivalence check needs.
fn disk_hash(bytes: &[u8]) -> ContentHash {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let half = hasher.finish().to_le_bytes();
    let mut out = [0u8; 32];
    for chunk in out.chunks_mut(half.len()) {
        chunk.copy_from_slice(&half[..chunk.len()]);
    }
    ContentHash::from_bytes(out)
}

pub fn vault_fixture() -> FakeVault {
    FakeVault::with(&[
        (
            "10-projects/serat.md",
            "# Serat\ntype:: project\nstatus:: active\n[[20-systems/indexing]]\n- [ ] ship P0\n",
        ),
        (
            "20-systems/indexing.md",
            "# Indexing\ntype:: system\n[[10-projects/serat]]\n",
        ),
        ("40-refs/contract.pdf", "%PDF-1.7\n"),
    ])
}

pub fn meta(path: &str) -> NoteMeta {
    let path = VaultPath::new(path).expect("path");
    NoteMeta {
        kind: if path.is_markdown() {
            NoteKind::Note
        } else {
            NoteKind::Asset
        },
        hash: ContentHash::from_bytes([1u8; 32]),
        size: 0,
        mtime_ms: 0,
        path,
    }
}

/// A note body with predictable field, link and task counts.
pub fn note_body(index: usize) -> String {
    format!(
        "# Note {index}
type:: project
status:: active
"
    )
}
