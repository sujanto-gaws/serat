//! Stubs shared by serat-index's test files.
//!
//! No filesystem walk and no Markdown parser anywhere in here. That is the dependency
//! rule paying for itself: `serat-index` never links `serat-vault` or `serat-parse`,
//! so its tests supply both as trait objects and the index can be driven end to end
//! against inputs a real vault would be tedious to arrange.
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
        }
    }

    pub fn unreadable(mut self, path: &str) -> Self {
        self.unreadable.push(path.to_owned());
        self
    }
}

impl NoteSource for FakeVault {
    fn entries(&self) -> Result<Vec<SourceEntry>, VaultError> {
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
    pub recorded: Mutex<Vec<ContentHash>>,
}

impl EchoGuard for SpyEcho {
    fn record_echo(&self, hash: ContentHash) {
        self.recorded.lock().expect("lock").push(hash);
    }
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
