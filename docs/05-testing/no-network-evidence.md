# No outbound network request: the static evidence (task #20)

Serves NFR-SEC-01 ("no network request the user didn't directly cause"), NFR-SEC-06, T-04, US-07; ADR-0017 (the two scheme origins).

## What is asserted, and where

| # | Claim | Test | Fails when |
|---|---|---|---|
| 1 | `serat-app`'s resolved dependency graph, for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc` and `aarch64-apple-darwin` (normal and build edges), holds no HTTP, TLS, WebSocket, QUIC or DNS-client crate | `crates/serat-app/tests/no_network.rs` `no_network_client_crate_is_in_the_resolved_graph_of_any_desktop_target` | a denied crate name (exact or prefix) appears; the message carries `cargo tree -i`'s path |
| 2 | `tokio` is built without its `net` feature, and `socket2` is absent | same file, `tokio_is_built_without_its_net_feature` | someone enables `net` or adds a socket crate |
| 3 | No `.rs` file under `crates/`, `bench/` or `contracts/` names a socket API (`std::net`, `tokio::net`, `TcpStream`, `UdpSocket`, `reqwest`, ...) | same file, `no_source_file_names_a_socket_api_unless_the_allowlist_says_so`, with `crates/serat-app/tests/network_use_allowlist.txt` | a new use appears without an allow-list line (with a reason); also fails on a stale allow-list entry |
| 4 | The CSP admits only `'self'`, the two ADR-0017 scheme origins and Tauri's IPC channel (`connect-src` only); `frame-src` and `object-src` are `'none'`; the whole string is pinned | `crates/serat-app/src/security_config.rs` `no_csp_directive_admits_a_remote_origin_and_the_whole_string_is_pinned` (beside the older per-directive pins) | any directive or source is added or changed |
| 5 | The bundled webview (`app/dist-bench`, built by the test; and the committed `crates/serat-app/webview`) has no `http(s)://` or `ws(s)://` string except XML namespaces, the scheme origin and a named list of inert strings | `app/src/security/noNetwork.test.ts` | a new URL-shaped string appears in the bundle |
| 6 | pdf.js is handed no `url`, `cMapUrl`, `standardFontDataUrl`, `wasmUrl` or `iccUrl`, and its own defaults for them are `null` | same file | the call site sets one, or a pdf.js upgrade changes a default |

Each scanner has a test that it fails on the thing it exists to find (the F-30 lesson: a guard that cannot fail is not a guard).

## Findings recorded while writing it (not silenced)

- `tauri` lists `reqwest` as a dependency for Android and iOS only; `cargo metadata` without a platform filter therefore shows `reqwest` and `hyper` in the graph. The test uses per-target `cargo tree`, which is what is compiled. It must be kept to desktop targets: a mobile target would fail it, correctly.
- `tokio`, `mio`, `http` and `url` are present and are not network clients here (`tokio` without `net`; `mio` is `notify`'s inotify poller on Linux; `http` and `url` are data types). They are listed in `KNOWN_NON_NETWORK` so the reason is on record.
- The `webview` bundle contains documentation-link strings from Vue, Pinia and a Vite plugin warning (`https://vuejs.org/...`) and pdf.js's URL-parsing bases (`https://foo.bar`, `http://example.com`). None is requested; each is on the `INERT` list in `noNetwork.test.ts` with its reason.
- The CSP has no `form-action` directive. A form submission is a navigation, which `on_navigation` (`nav_guard.rs`) is responsible for, not the fetch directives pinned here. Not changed by this task.

## Limits (read before relying on this)

This proves the **absence of code paths** in what we compile and ship. It does not observe a running process, so it cannot show that no connection is opened by the operating system's webview (update checks, safe-browsing lookups, DNS prefetch, proxy auto-detection), by a library through a path the scanners do not know, or by a crate added later under a name not on the deny-list. The deny-list is a list of names; it is the graph review at each dependency change that keeps it honest. The webview scan reads strings, so a URL assembled at run time from pieces is invisible to it, which is why the CSP pin (4) is the enforcement and (5) is the evidence.

The runtime check (a packet capture or per-process connection log on the reference machine through the gate week) belongs to `p1-gate-protocol.md`, which carries one line for it.
