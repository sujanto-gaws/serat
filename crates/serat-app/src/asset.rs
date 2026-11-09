//! The `serat-asset` scheme handler, `asset_info` and `read_asset_range`
//! (task #19; ADR-0017 steps 1-10; FR-BIN-01, FR-BIN-11, FR-BIN-12;
//! NFR-SEC-01, NFR-SEC-06; T-07, T-09, T-10).
//!
//! # Where the decisions live
//!
//! [`serve`] is the whole of ADR-0017's ten steps as a pure function over a
//! [`Vault`], a byte budget and a plain [`Request`]; [`handle`] is the thin
//! adapter from Tauri's `http::Request` to it. That split is what lets every
//! hostile case in the ADR's *Tested against* list run headlessly on any runner,
//! with the platform an explicit argument so both spellings are testable from
//! one machine. What a pure function cannot show -- that a real webview honours
//! the sandbox CSP, refuses a framed scheme document, cancels a download -- is
//! listed under the `ignored` tests at the bottom, each with the reason.
//!
//! # Containment
//!
//! One rule, in the order the ADR gives: `VaultPath::new` (step 4), the
//! viewable-extension allowlist and the ignored-directory check folded to lower
//! case (step 5), `Vault::resolve` plus `starts_with(root)` (step 6), then
//! [`open_verified`] (step 7). Step 7 opens the file and asks the **open handle**
//! where it really is; nothing between step 6 and the read trusts a path.
//! `Vault::resolve` is not re-implemented here and neither are #34's name rules:
//! this module calls `VaultPath::new` and takes whatever it decides.
//!
//! # Platform notes for step 7
//!
//! The workspace forbids `unsafe_code`, so the handle-to-path query goes through
//! the `filepath` crate's `File::path()`: `GetFinalPathNameByHandleW` on Windows,
//! `fcntl(F_GETPATH)` on macOS, `/proc/self/fd/N` on Linux. That is ADR-0017 step
//! 7 literally on every platform: the open handle is asked where it really is, and
//! the answer must be under the vault root.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use serat_core::VaultPath;
use serat_core::mime::{OCTET_STREAM, VIEWABLE, is_viewable, mime_for_extension};
use serat_core::verbatim::strip_verbatim;

use serat_vault::Vault;

use crate::ipc::{AssetInfo, IpcError};
use crate::nav_guard::Platform;

/// A viewable image type (served whole: an `<img>` cannot ask for spans).
fn is_image(mime: &str) -> bool {
    mime != "application/pdf" && VIEWABLE.contains(&mime)
}

/// Tauri's own span figure, ADR-0017 step 9: 1,000 KiB.
pub(crate) const SPAN_CAP: u64 = 1000 * 1024;
/// An image is served whole up to this size; above it the answer is 404.
pub(crate) const IMAGE_CAP: u64 = 64 * 1024 * 1024;
/// Bytes the handler may hold in flight across all concurrent requests.
pub(crate) const IN_FLIGHT_BYTES: u64 = 128 * 1024 * 1024;

/// The two size limits, a value so tests can exercise the boundaries with small
/// files. Production always uses [`Limits::default`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub span_cap: u64,
    pub image_cap: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            span_cap: SPAN_CAP,
            image_cap: IMAGE_CAP,
        }
    }
}

// ---------------------------------------------------------------------------
// The in-flight byte budget
// ---------------------------------------------------------------------------

/// A counting semaphore over bytes: the sum of the buffers the handler holds at
/// once never exceeds `capacity`. `UriSchemeResponder` takes whole bodies, so
/// without this N concurrent 64 MiB images would be N x 64 MiB resident.
///
/// A request larger than the whole capacity is clamped to it (it can then only
/// run alone) rather than waiting forever.
#[derive(Debug)]
pub(crate) struct ByteBudget {
    capacity: u64,
    used: Mutex<u64>,
    released: Condvar,
}

/// Bytes held; returned to the budget on drop.
#[derive(Debug)]
pub(crate) struct Permit<'a> {
    budget: &'a ByteBudget,
    bytes: u64,
}

impl ByteBudget {
    pub(crate) fn new(capacity: u64) -> Self {
        Self {
            capacity,
            used: Mutex::new(0),
            released: Condvar::new(),
        }
    }

    /// Blocks until `bytes` fit. Runs on the blocking pool, never the main thread.
    pub(crate) fn acquire(&self, bytes: u64) -> Permit<'_> {
        let bytes = bytes.min(self.capacity);
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        while *used + bytes > self.capacity {
            used = self
                .released
                .wait(used)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *used += bytes;
        Permit {
            budget: self,
            bytes,
        }
    }

    /// As [`ByteBudget::acquire`] without waiting.
    #[cfg(test)]
    fn try_acquire(&self, bytes: u64) -> Option<Permit<'_>> {
        let bytes = bytes.min(self.capacity);
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        if *used + bytes > self.capacity {
            return None;
        }
        *used += bytes;
        Some(Permit {
            budget: self,
            bytes,
        })
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut used = self
            .budget
            .used
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *used -= self.bytes;
        self.budget.released.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Steps 4-7, shared by the scheme, `asset_info` and `read_asset_range`
// ---------------------------------------------------------------------------

/// Step 5: the MIME type if (and only if) this is a viewable asset outside every
/// ignored directory. The directory check folds case (#34's fourth rule), so
/// `Node_Modules/` on a case-insensitive filesystem is the directory it names.
fn viewable_mime(vault: &Vault, path: &VaultPath) -> Option<&'static str> {
    if !under_no_ignored_dir(vault, path.parent()) {
        return None;
    }
    let mime = mime_for_extension(&path.extension()?).unwrap_or(OCTET_STREAM);
    is_viewable(mime).then_some(mime)
}

/// The same rule the walk applies (`IgnoreRules::is_ignored_dir_chain`: built-ins,
/// case-folded, and the user's `.seratignore` patterns), on the file's parent.
fn under_no_ignored_dir(vault: &Vault, parent: Option<&str>) -> bool {
    parent.is_none_or(|parent| !vault.ignore_rules().is_ignored_dir_chain(parent))
}

/// An opened, verified vault file.
#[derive(Debug)]
struct Opened {
    file: File,
    size: u64,
    etag: String,
}

/// Steps 6 and 7. `None` is the only refusal and carries no detail.
fn locate(vault: &Vault, path: &VaultPath) -> Option<Opened> {
    let absolute = vault.resolve(path).ok()?;
    if !absolute.starts_with(vault.root()) {
        return None;
    }
    let file = open_verified(&absolute, vault)?;
    let meta = file.metadata().ok()?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_nanos());
    Some(Opened {
        size: meta.len(),
        etag: format!("\"{:x}-{:x}\"", meta.len(), mtime_ns),
        file,
    })
}

/// Step 7: open, then check the **handle**. Fails closed.
///
/// `O_NONBLOCK` so opening a FIFO that a (hostile) vault contains returns at once
/// instead of waiting for a writer; `O_NOFOLLOW` so a final component swapped for a
/// symlink after step 6 is refused by the kernel; the `is_file` check after the
/// open so a FIFO or device is never read (it would block forever).
///
/// After the handle's own path is read back and found under the root, the
/// ignored-directory rule is re-run on that *real* relative path: a directory
/// swapped for a link into `.serat/` after step 5 is caught here.
pub(crate) fn open_verified(absolute: &Path, vault: &Vault) -> Option<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    // Windows: no reparse flag. A final-component link is followed by the open and
    // the handle's final path below says where it really landed; refusing every
    // reparse point would also refuse OneDrive Files-On-Demand placeholders.
    let file = options.open(absolute).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let real = filepath::FilePath::path(&file).ok()?;
    let relative = relative_to_root(&real, vault.root())?;
    let parent = relative
        .parent()
        .and_then(Path::to_str)
        .map(|text| text.replace('\\', "/"))
        .filter(|text| !text.is_empty());
    under_no_ignored_dir(vault, parent.as_deref()).then_some(file)
}

/// `real` relative to `root`, or `None` when it is not under it. Windows reports
/// handle paths in `\\?\` form; both sides are normalised before comparing.
fn relative_to_root(real: &Path, root: &Path) -> Option<PathBuf> {
    let real = strip_verbatim(real);
    let root = strip_verbatim(root);
    real.strip_prefix(&root).ok().map(Path::to_path_buf)
}

/// Reads exactly `len` bytes at `start`; a short read (file truncated since the
/// stat) is an error, never a silently shorter body.
fn read_span(file: &mut File, start: u64, len: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(len).read_to_end(&mut buf)?;
    if buf.len() as u64 != len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(buf)
}

// ---------------------------------------------------------------------------
// The scheme handler
// ---------------------------------------------------------------------------

/// What [`serve`] needs from a request, as plain strings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Request<'a> {
    pub method: &'a str,
    pub uri: &'a str,
    pub origin: Option<&'a str>,
    pub range: Option<&'a str>,
    pub if_range: Option<&'a str>,
}

/// A response, independent of Tauri's `http` types.
#[derive(Debug)]
pub(crate) struct Reply {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    #[cfg(test)]
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Into the `http` type Tauri's responder wants.
    pub(crate) fn into_http(self) -> tauri::http::Response<std::borrow::Cow<'static, [u8]>> {
        let mut builder = tauri::http::Response::builder().status(self.status);
        for (name, value) in self.headers {
            builder = builder.header(name, value);
        }
        builder
            .body(std::borrow::Cow::Owned(self.body))
            .unwrap_or_else(|_| {
                let mut fallback = tauri::http::Response::new(std::borrow::Cow::Borrowed(&[][..]));
                *fallback.status_mut() = tauri::http::StatusCode::INTERNAL_SERVER_ERROR;
                fallback
            })
    }
}

const CSP: &str = "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox";
const EXPOSE: &str = "Content-Range, Accept-Ranges, Content-Length";

/// Headers every response carries, errors included (steps 8 and 10).
fn base_headers(platform: Platform) -> Vec<(&'static str, String)> {
    vec![
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Content-Security-Policy", CSP.to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        (
            "Access-Control-Allow-Origin",
            platform.app_origin().to_owned(),
        ),
        ("Access-Control-Expose-Headers", EXPOSE.to_owned()),
        ("Vary", "Origin".to_owned()),
    ]
}

/// An answer with no detail: the status and nothing else.
fn bare(platform: Platform, status: u16) -> Reply {
    let mut headers = base_headers(platform);
    headers.push(("Content-Type", "text/plain; charset=utf-8".to_owned()));
    headers.push(("Content-Length", "0".to_owned()));
    Reply {
        status,
        headers,
        body: Vec::new(),
    }
}

fn not_found(platform: Platform) -> Reply {
    bare(platform, 404)
}

/// The raw, still percent-encoded path of a request for this platform's spelling,
/// or `None` when scheme or host is not exactly ours (step 2). Parsed by hand: a
/// URL parser would collapse `%2e%2e` and `..` before step 3 sees them.
///
/// Every platform delivers `serat-asset://localhost/...` to the handler: the page
/// uses `http://serat-asset.localhost/...` on Windows, but wry reverts that URI
/// before calling the handler (wry 0.55.1 `webview2/mod.rs`
/// `revert_uri_work_around`), as Tauri's own scheme relies on
/// (`tauri-2.11.5/src/protocol/tauri.rs`). `Platform` matters only for the CORS
/// origin in the response.
fn request_path(uri: &str) -> Option<&str> {
    let (scheme, rest) = uri.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("serat-asset") {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if !authority.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let path_end = tail.find(['?', '#']).unwrap_or(tail.len());
    let path = &tail[..path_end];
    path.starts_with('/').then_some(path)
}

/// Step 3: decodes exactly once. A `%` not followed by two hex digits stays a
/// literal `%`. NUL or invalid UTF-8 in the result is `None`.
fn decode_once(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).and_then(|b| hex(*b));
            let lo = bytes.get(i + 2).and_then(|b| hex(*b));
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    if out.contains(&0) {
        return None;
    }
    String::from_utf8(out).ok()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// A parsed `Range` header (single range only).
#[derive(Debug, PartialEq, Eq)]
enum RangeSpec {
    /// No header, or a unit other than `bytes` (ignored, per HTTP).
    Absent,
    /// Syntactically wrong, inverted, or multi-range: 416.
    Bad,
    /// `a-` or `a-b`.
    From(u64, Option<u64>),
    /// `-n`.
    Suffix(u64),
}

fn parse_range(header: Option<&str>) -> RangeSpec {
    let Some(header) = header else {
        return RangeSpec::Absent;
    };
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return RangeSpec::Absent;
    };
    if spec.contains(',') {
        return RangeSpec::Bad;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeSpec::Bad;
    };
    let digits = |text: &str| -> Option<u64> {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u64>().ok())
            .flatten()
    };
    match (first.trim(), last.trim()) {
        ("", n) => digits(n).map_or(RangeSpec::Bad, RangeSpec::Suffix),
        (a, "") => digits(a).map_or(RangeSpec::Bad, |a| RangeSpec::From(a, None)),
        (a, b) => match (digits(a), digits(b)) {
            (Some(a), Some(b)) if a <= b => RangeSpec::From(a, Some(b)),
            _ => RangeSpec::Bad,
        },
    }
}

/// What to send for a file of `size`.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// 200, the whole file.
    Whole,
    /// `len` bytes from `start`, with this status (206, or 200 for an `If-Range`
    /// restart of an over-cap PDF).
    Span { start: u64, len: u64, status: u16 },
    /// 416.
    Unsatisfiable,
    /// 404 (an image over the whole-image cap).
    Missing,
}

fn plan(size: u64, spec: &RangeSpec, restart: bool, mime: &str, limits: Limits) -> Plan {
    match spec {
        RangeSpec::Absent => {
            if size <= limits.span_cap {
                Plan::Whole
            } else if mime == "application/pdf" {
                Plan::Span {
                    start: 0,
                    len: limits.span_cap,
                    status: if restart { 200 } else { 206 },
                }
            } else if is_image(mime) && size <= limits.image_cap {
                Plan::Whole
            } else {
                Plan::Missing
            }
        }
        RangeSpec::Bad => Plan::Unsatisfiable,
        RangeSpec::From(start, end) => {
            if *start >= size {
                return Plan::Unsatisfiable;
            }
            let last = end.unwrap_or(u64::MAX).min(size - 1);
            let len = (last - start + 1).min(limits.span_cap);
            Plan::Span {
                start: *start,
                len,
                status: 206,
            }
        }
        RangeSpec::Suffix(n) => {
            if *n == 0 || size == 0 {
                return Plan::Unsatisfiable;
            }
            let start = size - (*n).min(size);
            Plan::Span {
                start,
                len: (size - start).min(limits.span_cap),
                status: 206,
            }
        }
    }
}

/// ADR-0017 steps 1-10 over one request.
pub(crate) fn serve(
    vault: &Vault,
    budget: &ByteBudget,
    platform: Platform,
    limits: Limits,
    request: &Request<'_>,
) -> Reply {
    // Steps 2-3. The host check comes before OPTIONS so a foreign host gets 404
    // whatever the method.
    let Some(raw) = request_path(request.uri) else {
        return not_found(platform);
    };
    // Step 1 / step 8: OPTIONS is answered, from the app origin only.
    if request.method.eq_ignore_ascii_case("OPTIONS") {
        if request.origin != Some(platform.app_origin()) {
            return bare(platform, 403);
        }
        let mut reply = bare(platform, 204);
        reply
            .headers
            .push(("Access-Control-Allow-Methods", "GET, HEAD".to_owned()));
        reply
            .headers
            .push(("Access-Control-Allow-Headers", "Range, If-Range".to_owned()));
        return reply;
    }
    let is_head = request.method.eq_ignore_ascii_case("HEAD");
    if !is_head && !request.method.eq_ignore_ascii_case("GET") {
        return bare(platform, 405);
    }

    let Some(decoded) = decode_once(raw) else {
        return not_found(platform);
    };
    let relative = decoded.strip_prefix('/').unwrap_or(&decoded);

    // Step 4 (#34's rules, as VaultPath::new has them), step 5.
    let Ok(path) = VaultPath::new(relative) else {
        return not_found(platform);
    };
    let Some(mime) = viewable_mime(vault, &path) else {
        return not_found(platform);
    };

    // Steps 6-7.
    let Some(mut opened) = locate(vault, &path) else {
        return not_found(platform);
    };

    // Step 9.
    let spec = parse_range(request.range);
    let (spec, restart) = match (&spec, request.if_range) {
        (RangeSpec::Absent, _) => (spec, false),
        (_, Some(validator)) if validator != opened.etag => (RangeSpec::Absent, true),
        _ => (spec, false),
    };

    let mut headers = base_headers(platform);
    headers.push(("Content-Type", mime.to_owned()));
    headers.push(("Accept-Ranges", "bytes".to_owned()));
    headers.push(("ETag", opened.etag.clone()));

    let size = opened.size;
    let (status, start, len) = match plan(size, &spec, restart, mime, limits) {
        Plan::Missing => return not_found(platform),
        Plan::Unsatisfiable => {
            let mut reply = bare(platform, 416);
            reply
                .headers
                .push(("Content-Range", format!("bytes */{size}")));
            return reply;
        }
        Plan::Whole => (200, 0, size),
        Plan::Span { start, len, status } => (status, start, len),
    };
    if len < size || status == 206 {
        headers.push((
            "Content-Range",
            format!("bytes {start}-{}/{size}", start + len.max(1) - 1),
        ));
    }
    headers.push(("Content-Length", len.to_string()));

    let body = if is_head {
        Vec::new()
    } else {
        let _permit = budget.acquire(len);
        match read_span(&mut opened.file, start, len) {
            Ok(body) => body,
            Err(_) => return not_found(platform),
        }
    };
    Reply {
        status,
        headers,
        body,
    }
}

/// The adapter Tauri's scheme registration calls (on the blocking pool).
pub(crate) fn handle(
    vault: Option<&Vault>,
    budget: &ByteBudget,
    platform: Platform,
    request: &tauri::http::Request<Vec<u8>>,
) -> Reply {
    let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok());
    let uri = request.uri().to_string();
    let parsed = Request {
        method: request.method().as_str(),
        uri: &uri,
        origin: header("origin"),
        range: header("range"),
        if_range: header("if-range"),
    };
    match vault {
        Some(vault) => serve(vault, budget, platform, Limits::default(), &parsed),
        None => not_found(platform),
    }
}

// ---------------------------------------------------------------------------
// asset_info and read_asset_range
// ---------------------------------------------------------------------------

/// `asset_info`: a vault operation, not an index read. MIME from the extension
/// table, size from the verified open handle, sidecar by byte-exact name.
///
/// # Errors
///
/// [`IpcError::NotFound`] for anything that is missing or refused, with no
/// distinction (a refusal gets no detail on this path either).
pub(crate) fn info(vault: &Vault, path: &VaultPath) -> Result<AssetInfo, IpcError> {
    let missing = || IpcError::NotFound {
        path: path.to_string(),
    };
    if !under_no_ignored_dir(vault, path.parent()) {
        return Err(missing());
    }
    let opened = locate(vault, path).ok_or_else(missing)?;
    let mime = mime_for_extension(&path.extension().unwrap_or_default()).unwrap_or(OCTET_STREAM);
    Ok(AssetInfo {
        path: path.to_string(),
        mime: mime.to_owned(),
        bytes: opened.size,
        viewable: is_viewable(mime),
        sidecar_path: sidecar_of(vault, path),
        // Contract ahead of implementation (task #94, ADR-0017 Amendment 1): the
        // follow-up sets Some(opened.etag) and checks ReadAssetRangeArgs::if_match.
        etag: None,
    })
}

/// ADR-0005: the sidecar is the binary's full file name with `.md` appended,
/// matched byte-exactly (a directory listing, not an `exists()` that a
/// case-insensitive filesystem would answer for a different spelling).
fn sidecar_of(vault: &Vault, path: &VaultPath) -> Option<String> {
    let wanted = format!("{}.md", path.file_name());
    let directory = match path.parent() {
        Some(parent) => vault.resolve(&VaultPath::new(parent).ok()?).ok()?,
        None => vault.root().to_path_buf(),
    };
    let found = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .any(|entry| {
            entry.file_name().to_str() == Some(wanted.as_str())
                && entry.file_type().is_ok_and(|kind| kind.is_file())
        });
    found.then(|| match path.parent() {
        Some(parent) => format!("{parent}/{wanted}"),
        None => wanted,
    })
}

/// `read_asset_range`: ADR-0017 §Fallback. Steps 2-7 and the span cap, unchanged;
/// returns raw bytes. A start at or past the end is an empty read.
///
/// # Errors
///
/// [`IpcError::NotFound`] for every refusal; [`IpcError::Io`] if the read fails.
pub(crate) fn read_range(
    vault: &Vault,
    budget: &ByteBudget,
    path: &VaultPath,
    start: u64,
    len: u32,
) -> Result<Vec<u8>, IpcError> {
    let missing = || IpcError::NotFound {
        path: path.to_string(),
    };
    viewable_mime(vault, path).ok_or_else(missing)?;
    let mut opened = locate(vault, path).ok_or_else(missing)?;
    if start >= opened.size {
        return Ok(Vec::new());
    }
    let len = u64::from(len).min(SPAN_CAP).min(opened.size - start);
    let _permit = budget.acquire(len);
    read_span(&mut opened.file, start, len).map_err(|err| IpcError::Io {
        path: path.to_string(),
        message: err.to_string(),
    })
}

#[cfg(test)]
mod tests;
