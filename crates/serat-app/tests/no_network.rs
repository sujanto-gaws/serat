//! NFR-SEC-01 / NFR-SEC-06 / T-04 / US-07: the Rust side of "the app makes no network
//! request the user did not directly cause".
//!
//! Two static properties, each with a test that proves the check is not vacuous (the F-30
//! lesson: a guard that cannot fail is not a guard):
//!
//! 1. **The dependency graph.** `serat-app`'s resolved dependency graph, for each desktop
//!    target, contains no HTTP, TLS, WebSocket, QUIC or DNS-client crate.
//! 2. **The source.** No file under `crates/` or `bench/` names a socket API, unless the
//!    allow-list file `network_use_allowlist.txt` says so, with a reason.
//!
//! What this does *not* show: that a running process on a given OS opens no connection. It
//! proves the absence of code paths in what we compile; the runtime observation belongs to
//! the reference-machine protocol (`docs/05-testing/p1-gate-protocol.md`). Method and limits:
//! `docs/05-testing/no-network-evidence.md`.
//!
//! The CSP half lives beside the other `tauri.conf.json` pins, in `security_config.rs`; the
//! webview half is `app/src/security/noNetwork.test.ts`.

use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// 1. The dependency graph
// ---------------------------------------------------------------------------

/// Crates that are, or bring in, a network client. Matched by exact name or by prefix.
const DENIED_EXACT: &[&str] = &[
    "reqwest",
    "hyper",
    "h2",
    "h3",
    "ureq",
    "curl",
    "curl-sys",
    "isahc",
    "surf",
    "attohttpc",
    "minreq",
    "ehttp",
    "tungstenite",
    "tokio-tungstenite",
    "async-tungstenite",
    "websocket",
    "ws",
    "native-tls",
    "tokio-native-tls",
    "rustls",
    "tokio-rustls",
    "openssl",
    "openssl-sys",
    "quinn",
    "quinn-proto",
    "quinn-udp",
    "s2n-quic",
    "trust-dns-resolver",
    "hickory-resolver",
    "hickory-client",
    "c-ares",
    "tauri-plugin-http",
    "tauri-plugin-websocket",
    "tauri-plugin-updater",
    "tauri-plugin-shell",
];

/// Prefixes: families of crates.
const DENIED_PREFIX: &[&str] = &["hyper-", "tower-http", "rustls-", "trust-dns-", "hickory-"];

/// Crates that match a denied pattern and are known not to be a network client, each with
/// the reason. Empty today; an entry is a decision, not a convenience.
const ALLOWED_MATCHES: &[(&str, &str)] = &[];

/// Crates that open sockets or speak a wire protocol locally but are not clients of a remote
/// peer: they are *not* denied, and are listed so the reason is on record. `tokio` is
/// allowed only without its `net` feature (asserted below); `mio` is `notify`'s inotify
/// poller on Linux; `http` and `url` are plain data types.
const KNOWN_NON_NETWORK: &[&str] = &["tokio", "mio", "http", "url", "urlpattern"];

/// The three desktop targets Serat ships on (C-07, charter).
const TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repository root")
}

fn cargo_tree(args: &[&str]) -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .current_dir(repo_root())
        .arg("tree")
        .args(args)
        .args(["--offline", "--locked"])
        .output()
        .expect("run cargo tree");
    assert!(
        output.status.success(),
        "cargo tree {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

fn is_denied(name: &str) -> bool {
    if ALLOWED_MATCHES.iter().any(|(allowed, _)| *allowed == name) {
        return false;
    }
    DENIED_EXACT.contains(&name) || DENIED_PREFIX.iter().any(|prefix| name.starts_with(prefix))
}

/// Package names in a `cargo tree --prefix none` listing (`name vX.Y.Z (path)`).
fn names(listing: &str) -> Vec<String> {
    let mut found: Vec<String> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    found.sort();
    found.dedup();
    found
}

#[test]
fn no_network_client_crate_is_in_the_resolved_graph_of_any_desktop_target() {
    let mut offences = Vec::new();
    for target in TARGETS {
        let listing = cargo_tree(&[
            "-p",
            "serat-app",
            "-e",
            "normal,build",
            "--target",
            target,
            "--prefix",
            "none",
        ]);
        for name in names(&listing).into_iter().filter(|name| is_denied(name)) {
            // Report how it got there: the path is what a reviewer needs.
            let why = cargo_tree(&[
                "-p",
                "serat-app",
                "-e",
                "normal,build",
                "--target",
                target,
                "-i",
                &name,
            ]);
            offences.push(format!("{target}: {name} is in the graph:\n{why}"));
        }
    }
    assert!(
        offences.is_empty(),
        "a network client crate is reachable from serat-app (NFR-SEC-01, T-04):\n{}",
        offences.join("\n")
    );
}

#[test]
fn tokio_is_built_without_its_net_feature() {
    // tokio is present (tauri's runtime); its `net` feature is what would give the process
    // `tokio::net`. mio and socket2 arrive only with it or with `notify`.
    for target in TARGETS {
        let listing = cargo_tree(&[
            "-p",
            "serat-app",
            "-e",
            "normal,build",
            "--target",
            target,
            "-i",
            "tokio",
            "-e",
            "features",
        ]);
        assert!(
            !listing.contains("tokio feature \"net\""),
            "{target}: tokio's `net` feature is enabled (NFR-SEC-01):\n{listing}"
        );
        assert!(
            !names(&cargo_tree(&[
                "-p",
                "serat-app",
                "-e",
                "normal,build",
                "--target",
                target,
                "--prefix",
                "none"
            ]))
            .contains(&"socket2".to_owned()),
            "{target}: socket2 is in the graph"
        );
    }
}

#[test]
fn the_deny_list_catches_what_it_names_and_spares_what_it_allows() {
    for bad in [
        "reqwest",
        "hyper",
        "hyper-util",
        "ureq",
        "tokio-tungstenite",
        "rustls-pki-types",
        "hickory-proto",
    ] {
        assert!(is_denied(bad), "{bad} should be denied");
    }
    for fine in KNOWN_NON_NETWORK {
        assert!(!is_denied(fine), "{fine} should not be denied");
    }
    for fine in ["serde", "notify", "tauri", "tauri-plugin-opener"] {
        assert!(!is_denied(fine), "{fine} should not be denied");
    }
    // And the parser reads a real listing line.
    assert_eq!(
        names("reqwest v0.12.0\nserde v1.0.0 (/x)\nreqwest v0.12.0\n"),
        ["reqwest", "serde"]
    );
}

// ---------------------------------------------------------------------------
// 2. The source
// ---------------------------------------------------------------------------

/// Patterns that name a socket or a network client. A line containing one is a finding
/// unless the allow-list covers `path:pattern`.
const SOURCE_PATTERNS: &[&str] = &[
    "std::net",
    "core::net",
    "tokio::net",
    "mio::net",
    "std::os::unix::net",
    "socket2",
    "TcpStream",
    "TcpListener",
    "TcpSocket",
    "UdpSocket",
    "UnixStream",
    "UnixListener",
    "UnixDatagram",
    "ToSocketAddrs",
    "reqwest",
    "hyper::",
    "ureq::",
    "curl::",
    "tungstenite",
    "WebSocket",
];

/// This file spells every pattern, so it is the one file not scanned.
const SELF_RELATIVE: &str = "crates/serat-app/tests/no_network.rs";

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "target" && name != "node_modules" && !name.starts_with('.') {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// `(line number, pattern)` for every non-comment line of `source` that names a pattern.
/// `use std::{io, net}` and `use std::{net::TcpStream}` are caught by the `net` segment rule.
fn findings(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        for pattern in SOURCE_PATTERNS {
            if code.contains(pattern) {
                out.push((index + 1, (*pattern).to_owned()));
            }
        }
        // A `net` path segment inside a braced import: `use std::{io, net};`.
        let braced_net = code.starts_with("use ")
            && (code.contains("std::{") || code.contains("tokio::{") || code.contains("core::{"))
            && code
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|word| word == "net");
        if braced_net {
            out.push((index + 1, "net (braced import)".to_owned()));
        }
    }
    out
}

/// `path:pattern` lines; `#` comments and blanks ignored.
fn allowlist() -> Vec<String> {
    let text = include_str!("network_use_allowlist.txt");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

#[test]
fn no_source_file_names_a_socket_api_unless_the_allowlist_says_so() {
    let root = repo_root();
    let allowed = allowlist();
    let mut files = Vec::new();
    for dir in ["crates", "bench", "contracts"] {
        rust_files(&root.join(dir), &mut files);
    }
    assert!(
        files.len() > 50,
        "the scan found only {} files",
        files.len()
    );
    let mut offences = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == SELF_RELATIVE {
            continue;
        }
        let source = std::fs::read_to_string(file).expect("utf-8 source");
        for (line, pattern) in findings(&source) {
            if !allowed.contains(&format!("{relative}:{pattern}")) {
                offences.push(format!("{relative}:{line}: {pattern}"));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "socket or network-client use found outside the allow-list (NFR-SEC-01, T-04). Remove it, \
         or add `path:pattern` with a reason to tests/network_use_allowlist.txt:\n{}",
        offences.join("\n")
    );
}

#[test]
fn the_allowlist_holds_no_stale_entry() {
    // An entry for a file that no longer matches would let the pattern come back unseen.
    let root = repo_root();
    for entry in allowlist() {
        let (path, pattern) = entry.rsplit_once(':').expect("path:pattern");
        let source = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|_| panic!("allow-listed file {path} is gone"));
        assert!(
            findings(&source).iter().any(|(_, found)| found == pattern),
            "allow-list entry {entry} no longer matches anything; remove it"
        );
    }
}

#[test]
fn the_source_scan_fails_on_what_someone_might_add() {
    for code in [
        "use std::net::TcpStream;",
        "let s = std::net::UdpSocket::bind(\"0.0.0.0:0\");",
        "use std::{io, net};",
        "use tokio::net::TcpListener;",
        "use std::os::unix::net::UnixStream;",
        "let c = reqwest::Client::new();",
    ] {
        assert!(!findings(code).is_empty(), "the scan missed: {code}");
    }
    assert!(findings("// std::net is not used here").is_empty());
    assert!(findings("let net = 1; // a variable").is_empty());
    assert!(findings("use std::{io, fs};").is_empty());
}
