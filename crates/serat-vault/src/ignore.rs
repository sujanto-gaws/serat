//! What the walk and the watcher refuse to look at.
//!
//! Two layers (FR-VLT-07). The built-in rules below are fixed: `.serat/`, `.git/`,
//! `.obsidian/` and every other dotted directory. On top of them sits the **ignore
//! file**, [`IGNORE_FILE`] (`.seratignore`) at the vault root, in gitignore syntax. It
//! is the user's, plain text that Serat only ever reads, and a vault without one
//! behaves exactly as before.

use std::sync::{Arc, PoisonError, RwLock};

/// Directory names that are never descended into.
///
/// `.obsidian` is the interesting one. The charter's zero-switching-cost promise is
/// that Serat reads an existing Obsidian vault unmodified, with `.obsidian/` "ignored
/// but preserved" — so it must not be indexed, and it must not be touched. Ignoring
/// it here does both, because nothing outside a walk ever names a file.
const IGNORED_DIRS: &[&str] = &[
    ".serat",    // our own index. Indexing it would index a description of itself.
    ".obsidian", // Obsidian's configuration. Preserved, never read, never written.
    ".git",      // and its packfiles, which are large and binary and meaningless here
    ".trash",    // Obsidian's soft-delete directory
    ".stfolder", // Syncthing
    ".stversions",
    "node_modules",
];

/// File extensions treated as Markdown notes.
///
/// Everything else in the vault is an asset: recorded, given a sidecar, extracted in
/// P2 (architecture §7). Nothing is skipped for being unrecognised — design goal 5
/// says capability degrades and nothing breaks, so an unknown file type is still a
/// row, still linkable, still queryable.
const NOTE_EXTENSIONS: &[&str] = &["md", "markdown"];

/// The user's ignore file, at the vault root.
///
/// Its own name rather than `.gitignore`: a `.gitignore` says what *git* should not
/// track (a vault under git commonly lists notes it keeps off the remote but still
/// wants searched), so reading it would hide notes from the index for a reason the
/// user never gave us. `.gitignore` is touched by exactly one thing here, the
/// `/.serat/` offer (NFR-CMP-04), and is never read for indexing.
pub const IGNORE_FILE: &str = ".seratignore";

/// The most bytes of the ignore file that are read. Past it the file is treated as
/// absent: matching cost is linear in its lines for every entry of every walk.
const MAX_IGNORE_FILE_BYTES: usize = 256 * 1024;

/// The most lines read, as well as the bytes: one short pattern per line would otherwise
/// allow a quarter of a million of them. 1,000 is far above any real ignore file and keeps the
/// walk to seconds, not minutes, in the worst case.
const MAX_IGNORE_LINES: usize = 1_000;

/// The rules the walk and the watcher share.
///
/// One type, used by both, because a file the walk skips and the watcher accepts
/// would be indexed on edit and vanish on rebuild — a difference that only shows up
/// as "my vault has different contents after a rebuild", which is the one thing
/// C-02 promises cannot happen.
///
/// Clones share the ignore file's patterns, so reloading them through one (the
/// watcher does, when `.seratignore` changes) is seen by the walk holding another.
#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {
    extra_dirs: Vec<String>,
    user: Arc<RwLock<Arc<Patterns>>>,
}

impl IgnoreRules {
    /// The default rules.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Also ignore a directory name, anywhere in the vault.
    #[must_use]
    pub fn ignoring_dir(mut self, name: impl Into<String>) -> Self {
        self.extra_dirs.push(name.into());
        self
    }

    /// Whether a directory should be descended into.
    #[must_use]
    pub fn is_ignored_dir(&self, name: &str) -> bool {
        // Case-folded (task #34, NFR-SEC-06): Windows and macOS filesystems are
        // case-insensitive by default, so ".Obsidian" or ".GIT" names the same
        // directory a case-sensitive comparison would walk straight into —
        // indexing a vault's own index, or a packfile, on those platforms only.
        IGNORED_DIRS
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(name))
            || self
                .extra_dirs
                .iter()
                .any(|extra| extra.eq_ignore_ascii_case(name))
            // Every remaining dotted directory. A vault is a folder of files someone
            // reads; a hidden directory in it belongs to a tool, not to them.
            || name.starts_with('.')
    }

    /// Whether a file should be indexed at all.
    #[must_use]
    pub fn is_ignored_file(&self, name: &str) -> bool {
        // Dotfiles are configuration. .gitignore, .DS_Store and .editorconfig are not
        // notes, and indexing them would put them in search results forever.
        name.starts_with('.') || is_temp_name(name)
    }
}

impl IgnoreRules {
    /// Replace the ignore file's patterns with those in `bytes` (`None`: no file).
    /// Returns whether the effective patterns changed, which is what tells the caller
    /// the vault must be walked again.
    ///
    /// Bytes that are not UTF-8 are read lossily and an oversized file is treated as
    /// absent; neither is an error, because an unreadable ignore file must not stop a
    /// vault opening (design goal 5).
    pub fn set_ignore_file(&self, bytes: Option<&[u8]>) -> bool {
        let patterns = match bytes {
            Some(bytes) if bytes.len() <= MAX_IGNORE_FILE_BYTES => {
                Patterns::parse(&String::from_utf8_lossy(bytes))
            }
            _ => Patterns::default(),
        };
        let mut current = self.user.write().unwrap_or_else(PoisonError::into_inner);
        if **current == patterns {
            return false;
        }
        *current = Arc::new(patterns);
        true
    }

    /// The ignore file's patterns as a snapshot a walk can hold for its whole length
    /// without taking the lock per entry.
    #[must_use]
    pub fn user_patterns(&self) -> Arc<Patterns> {
        Arc::clone(&self.user.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Whether the ignore file hides the file at `relative`, or a directory above it.
    /// Built-in rules are not consulted: this is what `NoteSource::load` refuses.
    #[must_use]
    pub fn user_ignores_path(&self, relative: &str) -> bool {
        let user = self.user_patterns();
        if user.is_empty() {
            return false;
        }
        let mut end = 0;
        let mut segments = relative.split('/').peekable();
        while let Some(segment) = segments.next() {
            end += segment.len();
            let is_dir = segments.peek().is_some();
            if user.is_ignored(&relative[..end], is_dir) {
                return true;
            }
            end += 1;
        }
        false
    }

    /// Whether the directory `relative`, or any directory above it, is excluded: a
    /// directory under an ignored one is not reachable by a walk from the root.
    #[must_use]
    pub fn is_ignored_dir_chain(&self, relative: &str) -> bool {
        let mut end = 0;
        for segment in relative.split('/') {
            end += segment.len();
            if self.is_ignored_at(&relative[..end], true) {
                return true;
            }
            end += 1;
        }
        false
    }

    /// Whether the entry at vault-relative path `relative` is excluded: by the
    /// built-in rules on its name, or by the ignore file on its path.
    #[must_use]
    pub fn is_ignored_at(&self, relative: &str, is_dir: bool) -> bool {
        let name = relative.rsplit('/').next().unwrap_or(relative);
        let built_in = if is_dir {
            self.is_ignored_dir(name)
        } else {
            self.is_ignored_file(name)
        };
        built_in || self.user_patterns().is_ignored(relative, is_dir)
    }
}

/// One parsed line of the ignore file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    /// `!pattern`: a match re-includes.
    negated: bool,
    /// A trailing `/`: matches directories only.
    dir_only: bool,
    /// The glob, split on `/`.
    segments: Vec<String>,
    /// Contained a `/` other than a trailing one: matches from the vault root only.
    /// Otherwise it matches an entry's own name at any depth.
    anchored: bool,
}

/// The ignore file, parsed. Gitignore syntax: blank lines and `#` comments, `!`
/// negation, an inner or leading `/` anchoring to the vault root, a trailing `/` for
/// directories only, `*` `?` `[a-z]` within a name and `**` across directories. The
/// last matching line wins. As in git, a file under an ignored directory cannot be
/// re-included, because the directory is never entered.
///
/// Matching folds ASCII case on every platform, as the built-in directory names do
/// (task #34): the same vault should index the same notes on every machine (design
/// goal 1, the vault survives the application). ASCII only: `É` and `é` are different
/// names to a pattern, as they are to the built-in rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patterns {
    list: Vec<Pattern>,
}

impl Patterns {
    /// Parse the text of an ignore file.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self {
            list: text
                .lines()
                .take(MAX_IGNORE_LINES)
                .filter_map(parse_line)
                .collect(),
        }
    }

    /// Whether there is nothing to match, so a walk can skip building paths.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Whether `relative` (vault-relative, `/`-separated) is ignored.
    #[must_use]
    pub fn is_ignored(&self, relative: &str, is_dir: bool) -> bool {
        if self.list.is_empty() {
            return false;
        }
        // Folded once per entry, not once per pattern.
        let lowered = relative.to_ascii_lowercase();
        let parts: Vec<&str> = lowered.split('/').filter(|p| !p.is_empty()).collect();
        let mut ignored = false;
        for pattern in &self.list {
            if (pattern.dir_only && !is_dir) || ignored != pattern.negated {
                // Wrong kind of entry, or it cannot change the verdict: an ignoring
                // line after an ignore, or a negation after an include.
                continue;
            }
            if pattern.matches(&parts) {
                ignored = !pattern.negated;
            }
        }
        ignored
    }
}

fn parse_line(line: &str) -> Option<Pattern> {
    // Trailing spaces are not part of a pattern unless escaped (`name\ `), as in git.
    let mut line = line.trim_end_matches('\r');
    while let Some(rest) = line.strip_suffix([' ', '\t']) {
        if rest.ends_with('\\') {
            break;
        }
        line = rest;
    }
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (negated, line) = match line.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    // `\#` and `\!` are a literal first character.
    let line = match line.strip_prefix('\\') {
        Some(rest) if rest.starts_with(['#', '!']) => rest,
        _ => line,
    };
    let (line, dir_only) = match line.strip_suffix('/') {
        Some(rest) => (rest, true),
        None => (line, false),
    };
    let anchored = line.contains('/');
    let mut segments: Vec<String> = line
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if segments.is_empty() {
        return None;
    }
    // `a/**` is what is inside `a`, not `a`: one segment, then any number more.
    if anchored && segments.len() > 1 && segments.last().is_some_and(|s| s == "**") {
        let last = segments.len() - 1;
        segments.insert(last, "*".to_owned());
    }
    Some(Pattern {
        negated,
        dir_only,
        segments,
        anchored,
    })
}

impl Pattern {
    fn matches(&self, path: &[&str]) -> bool {
        let Some(name) = path.last() else {
            return false;
        };
        if !self.anchored {
            return glob_segment(&self.segments[0], name);
        }
        // No `**`: one segment against one segment, no table. Nearly every anchored
        // pattern, and the walk asks this of every entry.
        if !self.segments.iter().any(|segment| segment == "**") {
            return self.segments.len() == path.len()
                && self
                    .segments
                    .iter()
                    .zip(path)
                    .all(|(pattern, part)| glob_segment(pattern, part));
        }
        // Reachability over (pattern segment, path segment): linear in the product,
        // so a pattern full of `**` cannot make a walk exponential.
        let (p, t) = (self.segments.len(), path.len());
        let mut reach = vec![vec![false; t + 1]; p + 1];
        reach[0][0] = true;
        for i in 0..p {
            for j in 0..=t {
                if !reach[i][j] {
                    continue;
                }
                if self.segments[i] == "**" {
                    reach[i + 1][j] = true;
                    if j < t {
                        reach[i][j + 1] = true;
                    }
                } else if j < t && glob_segment(&self.segments[i], path[j]) {
                    reach[i + 1][j + 1] = true;
                }
            }
        }
        reach[p][t]
    }
}

/// Match one name against one glob segment (`*`, `?`, `[...]`, `\x`). The pattern
/// side is already lower-case, and `name` must already be ASCII-lower-cased.
///
/// All-ASCII input, which is nearly every name, is matched as bytes without allocating;
/// anything else as `char`s. The walk calls this for every entry against every
/// pattern, so an allocation here is paid 20,000 times per line of the ignore file.
fn glob_segment(pattern: &str, name: &str) -> bool {
    if pattern.is_ascii() && name.is_ascii() {
        return glob(pattern.as_bytes(), name.as_bytes());
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    glob(&pattern, &name)
}

fn glob<T: Copy + PartialOrd + From<u8>>(pattern: &[T], name: &[T]) -> bool {
    let (star_c, any_c, open_c, escape_c) =
        (T::from(b'*'), T::from(b'?'), T::from(b'['), T::from(b'\\'));
    let (mut p, mut n) = (0, 0);
    // Where the last `*` was, and how much of the name it has taken so far.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        let advance = match pattern.get(p) {
            Some(&c) if c == star_c => {
                star = Some((p, n));
                p += 1;
                continue;
            }
            Some(&c) if c == any_c => Some((p + 1, n + 1)),
            Some(&c) if c == open_c => match class(pattern, p, name[n]) {
                Some((true, next)) => Some((next, n + 1)),
                Some((false, _)) => None,
                // Never closed: a literal `[`.
                None => (name[n] == open_c).then_some((p + 1, n + 1)),
            },
            Some(&c) if c == escape_c && pattern.get(p + 1) == Some(&name[n]) => {
                Some((p + 2, n + 1))
            }
            Some(&c) if c != escape_c && c == name[n] => Some((p + 1, n + 1)),
            _ => None,
        };
        match (advance, star) {
            (Some((next_p, next_n)), _) => (p, n) = (next_p, next_n),
            (None, Some((star_p, star_n))) => {
                p = star_p + 1;
                n = star_n + 1;
                star = Some((star_p, star_n + 1));
            }
            (None, None) => return false,
        }
    }
    while pattern.get(p) == Some(&star_c) {
        p += 1;
    }
    p == pattern.len()
}

/// A `[...]` class starting at `pattern[start]`: whether `c` is in it, and where the
/// pattern continues. `None` when the bracket is never closed.
fn class<T: Copy + PartialOrd + From<u8>>(
    pattern: &[T],
    start: usize,
    c: T,
) -> Option<(bool, usize)> {
    let (bang, caret, close, dash) = (T::from(b'!'), T::from(b'^'), T::from(b']'), T::from(b'-'));
    let mut i = start + 1;
    let negate = matches!(pattern.get(i), Some(&x) if x == bang || x == caret);
    if negate {
        i += 1;
    }
    let first = i;
    let mut hit = false;
    while let Some(&current) = pattern.get(i) {
        if current == close && i > first {
            return Some((hit != negate, i + 1));
        }
        if pattern.get(i + 1) == Some(&dash) && pattern.get(i + 2).is_some_and(|&end| end != close)
        {
            hit |= current <= c && c <= pattern[i + 2];
            i += 3;
        } else {
            hit |= current == c;
            i += 1;
        }
    }
    None
}

/// The prefix of the temporary files [`crate::Vault::write_atomic`] creates.
///
/// Recognisable on sight and skipped by the walk, so a crash mid-write leaves
/// litter rather than a phantom note. Naming them after us also means a user who
/// finds one knows who to blame.
pub(crate) const TEMP_PREFIX: &str = ".serat-tmp-";

/// Whether a filename is one of our in-flight writes, or another tool's.
fn is_temp_name(name: &str) -> bool {
    name.starts_with(TEMP_PREFIX)
        || name.ends_with('~')       // emacs, gedit
        || name.ends_with(".swp")    // vim
        || name.ends_with(".tmp")
}

/// Whether a filename is a Markdown note rather than an asset.
#[must_use]
pub fn is_note_file(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, extension)| NOTE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_index_and_obsidians_config_are_never_descended_into() {
        let rules = IgnoreRules::new();
        for ignored in [".serat", ".obsidian", ".git", ".trash", "node_modules"] {
            assert!(rules.is_ignored_dir(ignored), "{ignored} was not ignored");
        }
        for kept in ["10-projects", "20-systems", "attachments", "Daily Notes"] {
            assert!(!rules.is_ignored_dir(kept), "{kept} was ignored");
        }
    }

    fn ignored(file: &str, path: &str, is_dir: bool) -> bool {
        Patterns::parse(file).is_ignored(path, is_dir)
    }

    #[test]
    fn an_anchored_directory_entry_ignores_that_directory_only() {
        // FR-VLT-07's acceptance example: an entry for /archive.
        assert!(ignored("/archive\n", "archive", true));
        assert!(!ignored("/archive\n", "notes/archive", true));
        assert!(ignored("archive/\n", "notes/archive", true));
        assert!(!ignored("archive/\n", "archive", false), "dir_only");
    }

    #[test]
    fn wildcards_stay_inside_a_name_and_double_star_crosses_directories() {
        assert!(ignored("*.pdf\n", "a/b/c.pdf", false));
        assert!(!ignored("*.pdf\n", "a/b/c.md", false));
        assert!(ignored("drafts/*.md\n", "drafts/x.md", false));
        assert!(!ignored("drafts/*.md\n", "drafts/deep/x.md", false));
        assert!(ignored("drafts/**/x.md\n", "drafts/a/b/x.md", false));
        assert!(ignored("drafts/**/x.md\n", "drafts/x.md", false));
        assert!(ignored("**/tmp\n", "a/b/tmp", true));
        assert!(ignored("a/**\n", "a/b/c.md", false));
        assert!(!ignored("a/**\n", "a", true), "a/** is inside a, not a");
        assert!(ignored("note-?.md\n", "note-1.md", false));
        assert!(!ignored("note-?.md\n", "note-12.md", false));
        assert!(ignored("v[0-9].md\n", "v3.md", false));
        assert!(!ignored("v[!0-9].md\n", "v3.md", false));
    }

    #[test]
    fn the_last_matching_line_wins_and_comments_and_blanks_are_skipped() {
        let file = "# private\n\n*.md\n!keep.md\n";
        assert!(ignored(file, "a.md", false));
        assert!(!ignored(file, "keep.md", false));
        assert!(!ignored(file, "# private", false));
        assert!(ignored("\\#odd.md\n", "#odd.md", false));
    }

    #[test]
    fn matching_folds_case_and_tolerates_crlf_and_trailing_spaces() {
        assert!(ignored("Archive/  \r\n", "ARCHIVE", true));
        assert!(ignored("*.PDF\n", "x.pdf", false));
    }

    #[test]
    fn a_pathological_pattern_is_linear_not_exponential() {
        let pattern = format!("{}x\n", "**/".repeat(40));
        let path = "a/".repeat(60);
        let started = std::time::Instant::now();
        assert!(!ignored(&pattern, &format!("{path}y"), false));
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
        let stars = format!("{}b\n", "*a".repeat(30));
        assert!(!ignored(&stars, &"a".repeat(200), false));
    }

    #[test]
    fn the_rules_see_the_ignore_file_and_a_reload_is_visible_to_clones() {
        let rules = IgnoreRules::new();
        let clone = rules.clone();
        assert!(!rules.is_ignored_at("archive/x.md", false));
        assert!(rules.set_ignore_file(Some(b"/archive\n")));
        assert!(clone.is_ignored_at("archive", true));
        assert!(!clone.is_ignored_at("notes/archive", true));
        assert!(
            !rules.set_ignore_file(Some(b"/archive\n")),
            "same file, no change"
        );
        assert!(rules.set_ignore_file(None));
        assert!(!clone.is_ignored_at("archive", true));
        // The built-in layer still applies on top.
        assert!(clone.is_ignored_at(".obsidian", true));
    }

    #[test]
    fn an_oversized_ignore_file_is_treated_as_absent() {
        let rules = IgnoreRules::new();
        let big = "x\n".repeat(MAX_IGNORE_FILE_BYTES);
        assert!(!rules.set_ignore_file(Some(big.as_bytes())));
        assert!(rules.user_patterns().is_empty());
    }

    #[test]
    fn extra_ignores_compose() {
        let rules = IgnoreRules::new().ignoring_dir("archive");
        assert!(rules.is_ignored_dir("archive"));
        assert!(rules.is_ignored_dir(".serat"));
        assert!(!rules.is_ignored_dir("10-projects"));
    }

    #[test]
    fn ignored_dir_matching_folds_case() {
        // Task #34, NFR-SEC-06. Windows and macOS default to case-insensitive
        // filesystems, so a case-sensitive comparison here would walk straight
        // into ".Obsidian", ".GIT" or "Node_Modules" on those platforms while
        // refusing to on Linux — the exact "my vault has different contents
        // depending on the machine" outcome design goal 1 rules out.
        let rules = IgnoreRules::new().ignoring_dir("archive");
        for ignored in [
            ".Serat",
            ".OBSIDIAN",
            ".Git",
            ".Trash",
            "Node_Modules",
            "NODE_MODULES",
            "ARCHIVE",
            "Archive",
        ] {
            assert!(rules.is_ignored_dir(ignored), "{ignored} was not ignored");
        }
    }

    #[test]
    fn our_own_temp_files_are_not_notes() {
        let rules = IgnoreRules::new();
        assert!(rules.is_ignored_file(".serat-tmp-note.md.1"));
        assert!(rules.is_ignored_file(".DS_Store"));
        assert!(rules.is_ignored_file("note.md~"));
        assert!(rules.is_ignored_file(".note.md.swp"));
        assert!(!rules.is_ignored_file("note.md"));
        assert!(!rules.is_ignored_file("contract.pdf"));
    }

    #[test]
    fn note_extensions_are_case_insensitive_and_assets_are_not_notes() {
        assert!(is_note_file("note.md"));
        assert!(is_note_file("NOTE.MD"));
        assert!(is_note_file("note.markdown"));
        assert!(!is_note_file("contract.pdf"));
        assert!(!is_note_file("README"));
        assert!(!is_note_file("archive.md.zip"));
    }
}
