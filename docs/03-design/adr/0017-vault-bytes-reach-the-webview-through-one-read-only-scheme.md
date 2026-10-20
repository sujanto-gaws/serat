# ADR-0017: Vault bytes reach the webview through one read-only custom scheme

**Status:** proposed — stays proposed until review of task #6 is clean and tauri-app-dev has reviewed it · **Date:** 2026-09-24 · **Serves:** FR-BIN-11, FR-BIN-12, FR-EDT-03 (image embeds), NFR-SEC-06, NFR-SEC-01
**Honours:** ADR-0012 (the IPC ACL; this ADR records what it does not cover), threat model T-07 (one containment rule) and T-10

## Context

P1 ships a PDF viewer (FR-BIN-11), an image viewer (FR-BIN-12) and image embeds in notes (§9.3). All three need the bytes of a vault file inside the webview, and PDFs of a hundred megabytes must load incrementally, because pdf.js asks for byte ranges. Every path that reads a vault file has to enforce T-07's rule that nothing outside the vault root is read. The webview's CSP is `default-src 'self'` with an IPC `connect-src` (`crates/serat-app/tauri.conf.json`), so some change to it is unavoidable. The question is which change.

It is an ADR because it adds a third read path into the vault, beside `read_file` and the walk. It changes the CSP. And it sits outside ADR-0012's ACL, which governs IPC commands only.

## Options considered

1. **Tauri's asset protocol** (`convertFileSrc`, `assetProtocol.scope` widened at runtime to the opened root). Rejected on three grounds:
   - Containment would rest on Tauri's scope matcher, which checks path patterns rather than resolving symlink components. T-07's history records two implementations of one rule disagreeing as the defect.
   - The scope would span the whole directory tree, including `.md` files, `.serat/` and `.obsidian/`, because it does not know the ignore rules.
2. **An IPC command returning bytes** (`tauri::ipc::Response`). It stays inside the ACL, but pdf.js would need a custom range transport, and every image embed would pay a round trip and a blob copy. **Kept as the per-platform fallback** (§Fallback), not as the primary path.
3. **One custom URI scheme, `serat-asset`, handled by serat-app, resolving every request through the vault's containment functions.** Chosen.

## Decision

**`serat-asset://localhost/<percent-encoded vault-relative path>` is the only way vault bytes reach the webview other than `read_file`.** Tauri presents a custom scheme as `serat-asset://localhost/…` on macOS and Linux, and as `http://serat-asset.localhost/…` on Windows. **`useHttpsScheme` is pinned to `false`** in `tauri.conf.json`, so the Windows form is exactly that one, and the CSP lists both forms. The handler, in serat-app, applies these steps **in this order**:

1. **Allows `GET` and `HEAD` only.** Everything else gets 405. The one exception is `OPTIONS`, answered as in step 8, so that preflight is covered whatever a webview decides to send. The scheme cannot write.
2. **Refuses any host but `localhost`** with a 404.
3. **Decodes the path exactly once, and that is the only rule.** A decoded path containing NUL or invalid UTF-8 gets a 404. The decoded text is the file name, literally: a `%` left in it is an ordinary character and is never decoded again. So `%252e` names a file literally called `%2e`, and a real file named `a%41.pdf` stays servable, because the front end requests it as `a%2541.pdf`.
4. **Builds a `VaultPath` under task #34's rules, which this ADR relies on and does not restate as its own guard.** `VaultPath::new` rejects `..`, absolute paths and empty segments. #34 adds: **`:` in any segment**, which closes the Windows drive-relative escape (`C:Users` replaces the whole path in `PathBuf::push`) and NTFS alternate data streams; **Windows device names** (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`9`, `LPT1`–`9`, with or without an extension); and **trailing dots or spaces**. The check that existed before #34 is not relied on. **Until #34 lands, this scheme must not ship.**
5. **Refuses anything but a viewable asset**, using the one extension→MIME table in `serat-core`. `application/pdf` and `image/png|jpeg|gif|webp|avif|bmp|svg+xml` pass. Everything else gets 404, including Markdown and any path under an ignored directory. The ignored-directory check folds case, which is #34's fourth rule.
6. **Resolves through `Vault::resolve`**, which refuses a symlink component anywhere in the path (T-07). Then, per #34, it **asserts that the result `starts_with` the canonical root**. A refusal gets 404 with no detail.
7. **Opens, then re-verifies the open handle, which closes the gap between resolving a path and opening it (TOCTOU).** Between step 6 and `open`, a component could be swapped for a symlink. After `open`, the handle's real path is read back — `GetFinalPathNameByHandleW` on Windows, `fcntl(F_GETPATH)` on macOS, `readlink("/proc/self/fd/N")` on Linux — and must `starts_with` the canonical root. On Windows the result carries the `\\?\` verbatim prefix, which is stripped (`\\?\UNC\` becomes `\\`) before the comparison. **The handle's metadata must also say `is_file()`**: a FIFO or device would otherwise block the read forever. Either failure closes the handle and returns 404. This is recorded in T-10.
8. **CORS.** The scheme is a different origin from the app, so pdf.js's range `fetch` is cross-origin. Every response carries:
   - `Access-Control-Allow-Origin` set to **the exact app origin** of the platform (`tauri://localhost`, or `http://tauri.localhost` on Windows), never `*`;
   - `Access-Control-Expose-Headers: Content-Range, Accept-Ranges, Content-Length`.
   `OPTIONS` from the app origin is answered 204 with `Access-Control-Allow-Methods: GET, HEAD` and `Access-Control-Allow-Headers: Range, If-Range`, and is refused from any other origin. Because this is answered, the design does not depend on whether each webview sends a preflight for a `Range` header.
9. **Serves bounded spans, and nothing streams.** Tauri's `UriSchemeResponder` takes a whole body, so every response is one bounded buffer:
   - **Each span is capped at 1,000 KiB** (Tauri's own figure). A single `Range: bytes=a-b` gets 206 with at most that many bytes from `a`. An `a-b` wider than the cap is truncated to the cap and says so in `Content-Range`, which is valid HTTP; the client asks for the rest.
   - **A request with no `Range` depends on the type.** An `<img>` never sends `Range`, so an image cannot be split into spans. An image is **served whole, as 200, up to 64 MiB**; above that it gets **404**, and the viewer shows "couldn't be displayed". **Only `application/pdf`** answers a no-`Range` request over the cap with **206 and the first span**, plus `Content-Range: bytes 0-…/<size>` and `Accept-Ranges: bytes`. pdf.js is the only consumer that reads spans, so no PDF request ever loads a large file whole. (The first draft of this fix applied the first-span 206 to every type, which would have truncated every photo over 1,000 KiB; review caught it.)
   - **pdf.js is configured for this:** the length comes from `assets.bytes` (passed as `length`), with `rangeChunkSize` set to the cap, `disableStream: true` and `disableAutoFetch: true`. It fetches only the spans of the pages being shown.
   - Malformed ranges (a start beyond the end, an inverted range, a multi-range) get 416. A suffix range (`-n`) is served within the cap.
   - **`If-Range` is honoured, and the range transport sends it.** Every range request after the first carries the `ETag` of the first response in `If-Range`, and `If-Range` is added to `Access-Control-Allow-Headers`. When the file has changed, the handler answers 200 with the first span, and pdf.js restarts rather than mixing versions.
10. **Headers on every response:**
   - `Content-Type` from the table.
   - `X-Content-Type-Options: nosniff`.
   - `Content-Security-Policy: default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox`.
   - `Cache-Control: no-store`.
   - An `ETag` built from size and mtime (one stat). Combined with `If-Range`, a file changed during a load restarts cleanly instead of mixing versions. A false "changed" costs one reload.

**The app CSP changes by exactly two sources and two explicit pins.** It adds `img-src 'self' serat-asset: http://serat-asset.localhost`, and the same two origins in `connect-src`. It pins **`frame-src 'none'; object-src 'none'`** explicitly (control (c) below). It adds no remote origin, no `data:` (§9.3 drops `data:` URLs), no `blob:` and no `'unsafe-*'`.

**pdf.js is pinned at ≥ 4.2.67**, the release that fixes CVE-2024-4367 (arbitrary JavaScript through a crafted font). It also runs with `isEvalSupported: false`, which blocks that CVE's path independently.

**pdf.js is bundled and inert.** Its worker, cMaps and standard fonts are served from `'self'`. It runs with `isEvalSupported: false` and **`enableScripting: false`**, so a PDF's JavaScript never runs. A link inside a PDF opens only on an explicit click, and only through the system opener (the same rule as §9.3's external links). **SVG is shown only through `<img>`**: never inline in the DOM and never as a navigated document, and the per-response `sandbox` CSP backs that up.

**What stops a `serat-asset` document from reaching IPC — corrected.** ADR-0012's command ACL gates `invoke`, and a scheme handler is reached by URL loads, so the ACL does not cover it. **There is no origin-classification control.** An earlier version of this paragraph said Tauri classifies `serat-asset` content as a remote origin (`is_local_url` false), so a document loaded from it could invoke no IPC command. **That is false.** tauri-app-dev checked the tauri-2.11.5 source:

- `webview/mod.rs:1698-1739` counts every scheme registered through `register_asynchronous_uri_scheme_protocol` as local.
- `Origin::Local` matches the local capability unconditionally (`tauri-utils` `acl/authority.rs:57-66`).

So a `serat-asset` document that became the main webview's top-level document, **or a frame inside it**, **could invoke every command granted to `main`**. Three controls actually hold, and all three must stay in place:

- **(a) Top-level navigation — the main frame only.** serat-app's `on_navigation` refuses every URL that is not the app's own origin (architecture §9.3), including `serat-asset://…` and `http://serat-asset.localhost/…`. **It sees top-level navigations only.** wry-0.55.1 on WebView2 hooks `NavigationStarting`, which is main-frame only, and does not hook `FrameNavigationStarting`. So an `<iframe src="serat-asset://…">` never reaches `on_navigation`, and a framed scheme document is still `Local`. (a) keeps the scheme from becoming the top-level document; it does not keep it out of a frame.
- **(b) No script runs in a scheme document.** Every response carries `Content-Security-Policy: default-src 'none'; …; sandbox` with **no `allow-scripts`** (step 10), so even a document that did load from the scheme, framed or top-level, executes nothing. That also covers an SVG with script, and anything a PDF carries. **Whether an engine honours a `sandbox` CSP on a custom-scheme response is engine-specific**, so (b) is proven on all three engines, never assumed from one.
- **(c) No frames or plugin objects in the app document.** The app CSP pins **`frame-src 'none'; object-src 'none'`** explicitly. Until now that behaviour came, unnamed, from the `default-src 'self'` fallback, so any edit to the CSP could have removed it without a test noticing. The explicit pin is a change to `crates/serat-app/tauri.conf.json`, for task #37 or #19 to implement with its test. **The sanitiser is the markup layer of the same control**: it strips `iframe`, `object` and `embed` from rendered notes (§9.3), so a note cannot even ask for a frame.

The read-only, viewable-types-only, contained-paths handler (steps 1–10) limits what the scheme can *serve*. It does not limit what a loaded document could *invoke*, so it is not a control against IPC.

**Downloads are refused.** `<a download href="serat-asset://…">` would make the webview write a vault file somewhere outside the vault, which is a write path T-06 does not list. serat-app refuses every download: WebView2 `DownloadStarting` is cancelled, and WebKit's `download-started` is cancelled on WKWebView and WebKitGTK. The sanitiser also strips the `download` attribute, as a second layer. Recorded in T-06 and T-10.

**The default context menu is disabled in release builds** (it offers "Save image as…", "Open frame in new window" and "Inspect"). Debug builds keep it for development.

**Sidecar pairing matches bytes exactly, deliberately.** `X.PDF` and `x.pdf.md` are not a pair: ADR-0005 defines the sidecar name as the binary's full file name with `.md` appended, and on a case-sensitive filesystem the two could name different files. Folding case here would let one sidecar claim two binaries. A user who wants the pairing renames one of the two, and nothing is guessed.

### Fallback, if a webview will not deliver a custom-scheme 206

If WebKitGTK or WKWebView does not deliver a custom-scheme 206 with these headers (the open question below), that platform uses **option 2 in span form**. A `read_asset_range(path, start, len) -> bytes` IPC command applies steps 2–7 and the span cap of step 9 unchanged, granted to the main window in the capability file (ADR-0012). The viewer supplies pdf.js with a custom `PDFDataRangeTransport` that calls it. **Loading therefore stays incremental and per page on every platform**, and the viewer's loading state can be the same everywhere: a real progress indicator driven by fetched spans against `assets.bytes`, not a skeleton waiting for the whole file. Images on that platform use the same command and a `blob:` URL. **Only** on that platform, `img-src` gains `blob:`, and that addition is recorded in T-10 when it is made.

## Consequences

**Positive.** One containment rule, with #34's hardening, serves the read command, the watcher's paths and now the viewer bytes. Large PDFs load a page's spans at a time on every platform, and no request holds more than 1,000 KiB. The CSP gains one first-party scheme. Scheme content is kept from IPC by the navigation guard and by the script-free sandbox CSP. Tauri still classifies the scheme as local, so both controls are load-bearing.

**Negative.** **An image is held whole in memory while it is served**, because `<img>` cannot request spans. That costs up to 64 MiB per image request in the handler, plus the webview's decoded copy. The limit is what bounds it. A photo library with larger files degrades to "couldn't be displayed" (goal 5), never to a truncated image. It is a hand-written HTTP-ish handler, with range parsing, CORS and ten ordered steps to get right, and three platform-specific handle checks. The table of viewable types is a second allowlist beside the index's MIME table, which is why both come from the same function.

**Risks.**
- Range parsing (boundaries, overflow, suffix and multi-range) is the classic bug source.
- The TOCTOU re-check depends on three platform APIs, each needing its own test.
- The whole design depends on #34 landing first. #34's task description already carries all four rules plus `starts_with(root)`.
All of these are cases in task #19's hostile suite.

## Objections recorded

**lead** (review of task #6, 2026-09-24) raised four blocking objections, all upheld:
- the Windows drive-relative escape, now task #34, which this ADR relies on explicitly (step 4);
- the gap between resolving a path and opening it (step 7);
- CORS for cross-origin range fetches (step 8);
- `UriSchemeResponder` having no streaming, answered by the span cap (step 9).
The same review asked for the remote-origin classification as the second compensating control — **withdrawn: premise false (task #36)**; Tauri classifies the scheme as local — and for single decoding, the host check, `enableScripting: false`, SVG only through `<img>`, pinning `useHttpsScheme`, and a stated sidecar case rule. **frontend-designer** (task #8) asked whether the fallback stays incremental; it does (§Fallback).

**tauri-app-dev** has not yet reviewed this ADR; lead is arranging it. The open question for them is whether Tauri v2's `register_asynchronous_uri_scheme_protocol` delivers a 206, these headers and an `OPTIONS` response on WebKitGTK and WKWebView. §Fallback is the answer for any platform where it does not.

## Tested against

Nothing yet. The cases that would break it are listed for task #19 and belong in `crates/serat-app` beside `acl_behaviour.rs`, each shown to be able to fail:
- a symlinked file, and a symlinked directory component;
- a component swapped for a symlink between resolve and open;
- a FIFO;
- a link pointing into `.serat/`;
- `%2e%2e`, `%2f`, `%252e` and `%00`;
- `C:Users`, `C%3AUsers`, `CON.pdf` and a trailing dot;
- an ignored directory in different case;
- a `.md` path;
- a non-GET method;
- an `Origin` that is not the app's;
- an SVG with script opened as a document;
- a PDF with JavaScript;
- **a real top-level navigation of the main webview to `serat-asset://…`** (and to `http://serat-asset.localhost/…` on Windows), asserting that it is refused (control (a)). Then, with the guard disabled in the test build only, assert that the loaded document runs no script, so no command is reachable. That second arm proves control (b) holds on its own, and it **runs on all three engines** (WebView2, WKWebView and WebKitGTK), because a `sandbox` CSP on a custom-scheme response is engine-specific. tauri-app-dev writes this test;
- **an `<iframe>` and an `<object>` pointing at a scheme SVG that contains script**, inserted into the app document. The test asserts that no script runs and no `invoke` succeeds, and it **runs twice: with the `frame-src`/`object-src` pin on, and with it removed**. With the pin on, the frame must not load at all (control (c)). With it off, (b) alone must still hold. That pair is what ties a future CSP edit to a failing test;
- `<a download href="serat-asset://…">` clicked in the app document: no file may be written anywhere;
- range requests at the file boundaries and over the cap;
- an image over the cap with no `Range`, which must arrive whole; an image over 64 MiB, which must get 404; and a file changed between two range requests, which must restart through `If-Range`;
- a no-`Range` request for a file over the cap.
