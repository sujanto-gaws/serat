# ADR-0017: Vault bytes reach the webview through one read-only custom scheme

**Status:** proposed — accepted once tauri-app-dev's consultation reply is recorded, or once review of task #6 finds no objection · **Date:** 2026-09-24 · **Serves:** FR-BIN-11, FR-BIN-12, FR-EDT-03 (image embeds), NFR-SEC-06, NFR-SEC-01
**Honours:** ADR-0012 (the IPC ACL; this ADR records what it does not cover), threat model T-07 (one containment rule)

## Context

P1 ships a PDF viewer (FR-BIN-11), an image viewer (FR-BIN-12) and image embeds in notes (§9.3). All three need the bytes of a vault file inside the webview. A PDF of a hundred megabytes has to arrive incrementally, because pdf.js asks for byte ranges. Every path that reads a vault file has to enforce T-07's rule, that nothing outside the vault root is ever read, and a symlink is the case that rule was written for. The webview's CSP is `default-src 'self'` with an IPC `connect-src` (`crates/serat-app/tauri.conf.json`), so some change to it is unavoidable. The question is which change.

It is an ADR because it adds a third read path into the vault beside `read_file` and the walk. It also changes the CSP, and it sits outside ADR-0012's ACL, which governs IPC commands only.

## Options considered

1. **Tauri's asset protocol** (`convertFileSrc`, `assetProtocol.scope` widened at runtime to the opened root). This needs the least code. But containment then rests on Tauri's scope matcher, which checks path patterns rather than resolving symlink components the way `Vault::resolve` does (T-07, task #20). The first rule would get a second, differently-behaved implementation, which is the disagreement T-07's history records as the defect. The scope would also be widened to a whole directory tree at runtime, and it would serve `.md` files, `.serat/` and `.obsidian/`, because the scope does not know the ignore rules. Rejected.
2. **An IPC command returning bytes** (`tauri::ipc::Response`), with the front end making `blob:` URLs. It stays inside the ACL. But pdf.js's range loading would need a custom transport, every image embed would pay a round trip plus a blob copy, and the CSP would need `blob:` for images. Rejected for the viewer path; nothing prevents it later for small payloads.
3. **One custom URI scheme, `serat-asset`, handled by serat-app, resolving every request through `Vault::resolve`.** Chosen.

## Decision

**`serat-asset://localhost/<percent-encoded vault-relative path>` is the only way vault bytes reach the webview other than `read_file`.** On Windows, Tauri presents custom schemes as `http://serat-asset.localhost/…`, and the CSP names both forms. The handler, in serat-app, **in this order**:

1. **Allows `GET` and `HEAD` only.** Any other method gets 405. The scheme can never write.
2. **Decodes, then builds a `VaultPath` from the result.** `VaultPath::new` rejects `..`, absolute paths and empty paths before any filesystem call, which is `contracts/ipc.rs` rule 2 applied to a URL. If no vault is open, the response is 404.
3. **Refuses anything but a viewable asset.** The extension must map, through one table in `serat-core` (the same table the index uses for `assets.mime`), to `application/pdf` or to `image/png|jpeg|gif|webp|avif|bmp|svg+xml`. Everything else gets 404, including Markdown, which the editor reads through `read_file`, and any path under an ignored directory (`.serat/`, `.obsidian/`, `.git/`, the user's ignore file).
4. **Resolves through `Vault::resolve`**, the one function that already refuses a symlink component anywhere in the path (T-07). A refusal gets 404 with no detail, because a 403 would confirm that the target exists.
5. **Serves a single `Range: bytes=a-b` as 206 with `Accept-Ranges: bytes`**, reading only that span. A request without a range gets the whole file. pdf.js loads large files incrementally through this, and no request holds more than one span of a large PDF in memory.
6. **Sets `Content-Type` from the table and adds `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox` and `Cache-Control: no-store`.** `nosniff` stops a file from being executed as some other type. The per-response CSP with `sandbox` means that even an SVG with script, opened as a document rather than through `<img>`, runs nothing. `no-store` stops the webview caching bytes the user has since changed on disk.

**The app CSP changes by exactly two sources.** `img-src 'self' serat-asset: http://serat-asset.localhost` is added for images, and the same two origins are added to `connect-src` for pdf.js's range fetches. No remote origin, no `blob:` and no `'unsafe-*'` is added. pdf.js is bundled with its worker, its cMaps and its standard fonts, all served from `'self'`, and it runs with `isEvalSupported: false`, so it never needs a network source or `eval` (NFR-SEC-01).

**ADR-0012's ACL does not govern a URI scheme, and this ADR is where that is recorded.** Command ACLs gate `invoke`, while a scheme handler is reached by URL loads. Access to the scheme is therefore bounded by the scheme itself (read-only, viewable types only, contained paths) and by the fact that only the app's own `'self'` content runs in the webview. Remote content cannot load at all, as the threat model's *Webview ↔ app* boundary records. The scheme's hostile tests (task #19) are its ACL.

## Consequences

**Positive.** One containment rule has one implementation, used by the read command, the watcher's paths and now the viewer bytes. Large PDFs load incrementally. Image embeds cost one HTTP-style request and no IPC. The CSP gains one first-party scheme and nothing else.

**Negative.** It is a hand-written HTTP-ish handler, with range parsing and the order of six checks to get right. The table of viewable types is a second allowlist to keep beside the index's MIME table, which is why both come from the same function.

**Risks.** Range parsing is a classic source of off-by-one and overflow bugs: a start beyond the end of the file, an inverted range, a suffix range. Task #19's hostile suite covers each. A request for a file that is replaced while pdf.js is mid-load can mix old and new bytes across ranges. The handler therefore sends an `ETag` built from the file's size and mtime, which costs one stat rather than a hash of a large file on every range. With `If-Range`, a file changed mid-load then restarts cleanly instead of mixing versions. mtime is not a content hash, and §3.1 says why it is not trusted for change detection. Here it only has to change when the bytes do, and a false "changed" costs one reload.

## Objections recorded

**tauri-app-dev** owns serat-app, the CSP and ADR-0012's manifest. They were consulted on 2026-09-24, with the specific question of whether Tauri v2's `register_asynchronous_uri_scheme_protocol` supports the response headers and 206 status above on all three webviews. **No reply when this was committed, so the objection is absent, not waived.** That is where I expect it: WebKitGTK's and WKWebView's handling of custom-scheme range responses is not something I have seen work. If either refuses a 206, the fallback for that platform is option 2's IPC bytes for PDFs only, behind the same six checks.

## Tested against

Nothing yet. The cases that would break it are the T-07 symlink fixtures (a symlinked file, a symlinked directory component, a link pointing into `.serat/`), `%2e%2e` and `%2f` encodings, an ignored-directory path, a `.md` path, a non-GET method, an SVG with script opened as a document, and range requests at the file boundaries. They are named for task #19 and belong in `crates/serat-app` beside `acl_behaviour.rs`, in the same *prove it can fail* form P0 required of every guard.
