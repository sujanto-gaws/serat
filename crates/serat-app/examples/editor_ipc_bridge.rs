//! A loopback bridge from the NFR-PRF-02 editor-latency harness to the real `serat-app`
//! (task #61; NFR-PRF-02, architecture.md section 9.3 "the real-IPC arm"). Not shipped: an
//! example, built only by `cargo build --example editor_ipc_bridge -p serat-app`.
//!
//! The harness runs the production Vue/CodeMirror bundle in headless Chromium with the Tauri
//! IPC internals stubbed. Normally the stub answers instantly. With this bridge the stub
//! forwards every `invoke` over loopback HTTP to a real [`AppState`] that has a real vault
//! open: `save_patch` really patches the file by byte range and submits it to the index writer,
//! `note_opened` really registers the buffer, `read_file` really reads. So a debounced save
//! that lands mid-typing costs what it costs in the application.
//!
//! What it does NOT include, and the report says so: Tauri's own transport between the
//! webview and the Rust process (WebView2's message channel and the custom-protocol
//! serialisation). The bridge is a different process on loopback, which, like the real
//! application, keeps the Rust work off the renderer's main thread.
//!
//! Protocol: `POST /invoke/<command>` with the JSON the front end would pass to
//! `invoke(command, payload)` (`{"args": {...}}`); the answer is `200` with the command's
//! result as JSON, or `200` with `{"__error": <IpcError>}`. `GET /stats` returns per-command
//! counts and p50/p95/max milliseconds measured inside the bridge. On start the bridge prints
//! one line, `READY <port> <token>`, to stdout. It exits when stdin closes.
//!
//! **It is a write-capable server and is locked down accordingly** (review of task #61):
//! a random per-run token, printed only on the READY line, is required in the
//! `x-serat-bridge-token` header of every request; the `Host` header must be the loopback
//! address and port; CORS is answered only for a loopback `http://127.0.0.1:<port>` origin and
//! never with `*`; and the vault root must be inside the system temp directory, so the bridge
//! cannot be pointed at a real vault.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serat_app::cli::Boot;
use serat_app::ipc::{
    FlushAckArgs, IndexState, NoteBufferArgs, OpenVaultArgs, ReadFileArgs, RebaseBufferArgs,
    SavePatchArgs, WriteFileArgs,
};
use serat_app::settings::Settings;
use serat_app::startup::Startup;
use serat_app::state::AppState;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

type Stats = Arc<Mutex<BTreeMap<String, Vec<f64>>>>;

fn args<T: DeserializeOwned>(payload: &Value) -> Result<T, String> {
    serde_json::from_value(payload.get("args").cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("bad args: {e}"))
}

fn answer<T: serde::Serialize, E: serde::Serialize>(result: Result<T, E>) -> Value {
    match result {
        Ok(value) => serde_json::to_value(value).unwrap_or(Value::Null),
        Err(error) => json!({ "__error": error }),
    }
}

fn dispatch(state: &AppState, command: &str, payload: &Value) -> Value {
    let bad = |message: String| json!({ "__error": { "kind": "bridge", "message": message } });
    match command {
        "read_file" => match args::<ReadFileArgs>(payload) {
            Ok(a) => answer(state.read_file(a)),
            Err(e) => bad(e),
        },
        "write_file" => match args::<WriteFileArgs>(payload) {
            Ok(a) => answer(state.write_file(a)),
            Err(e) => bad(e),
        },
        "note_opened" => match args::<NoteBufferArgs>(payload) {
            Ok(a) => answer(state.note_opened(&a)),
            Err(e) => bad(e),
        },
        "note_closed" => match args::<NoteBufferArgs>(payload) {
            Ok(a) => {
                state.note_closed(&a);
                Value::Null
            }
            Err(e) => bad(e),
        },
        "save_patch" => match args::<SavePatchArgs>(payload) {
            Ok(a) => answer(state.save_patch(&a)),
            Err(e) => bad(e),
        },
        "rebase_buffer" => match args::<RebaseBufferArgs>(payload) {
            Ok(a) => answer(state.rebase_buffer(&a)),
            Err(e) => bad(e),
        },
        "request_external_diff" => match args::<NoteBufferArgs>(payload) {
            Ok(a) => answer(state.request_external_diff(&a)),
            Err(e) => bad(e),
        },
        "flush_ack" => match args::<FlushAckArgs>(payload) {
            Ok(a) => {
                state.flush_ack(&a);
                Value::Null
            }
            Err(e) => bad(e),
        },
        "get_ui_settings" => serde_json::to_value(state.get_ui_settings()).unwrap_or(Value::Null),
        "index_status" => serde_json::to_value(state.index_status()).unwrap_or(Value::Null),
        // Everything else the editor can call is answered with null, as the stub does.
        _ => Value::Null,
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn stats_json(stats: &Stats) -> Value {
    let guard = stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut out = serde_json::Map::new();
    for (command, samples) in guard.iter() {
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        out.insert(
            command.clone(),
            json!({
                "count": sorted.len(),
                "p50_ms": percentile(&sorted, 0.5),
                "p95_ms": percentile(&sorted, 0.95),
                "max_ms": sorted.last().copied().unwrap_or(0.0),
            }),
        );
    }
    Value::Object(out)
}

/// A `http://127.0.0.1:<digits>` origin, the only kind the harness serves the bundle from.
fn loopback_origin(origin: &str) -> bool {
    origin
        .strip_prefix("http://127.0.0.1:")
        .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
}

fn respond(stream: &mut TcpStream, status: &str, body: &str, origin: Option<&str>) {
    let cors = origin
        .filter(|o| loopback_origin(o))
        .map_or_else(String::new, |o| {
            format!(
                "Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\n\
             Access-Control-Allow-Headers: content-type, x-serat-bridge-token\r\n\
             Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n"
            )
        });
    let head = format!(
        "HTTP/1.1 {status}\r\n{cors}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn serve(mut stream: TcpStream, state: &AppState, stats: &Stats, token: &str, port: u16) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let (mut length, mut origin, mut host, mut presented) =
        (0usize, None::<String>, String::new(), String::new());
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header == "\r\n" || header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap_or(0),
            "origin" => origin = Some(value.to_owned()),
            "host" => value.clone_into(&mut host),
            "x-serat-bridge-token" => value.clone_into(&mut presented),
            _ => {}
        }
    }
    let origin = origin.as_deref();
    // DNS rebinding: a page on another name resolving to 127.0.0.1 still sends its own Host.
    if host != format!("127.0.0.1:{port}") {
        respond(&mut stream, "403 Forbidden", "{}", None);
        return;
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method == "OPTIONS" {
        // The preflight carries no custom header by definition; it grants nothing but the origin check.
        respond(&mut stream, "204 No Content", "", origin);
        return;
    }
    if presented != token {
        respond(&mut stream, "403 Forbidden", "{}", None);
        return;
    }
    if method == "GET" && path == "/stats" {
        respond(
            &mut stream,
            "200 OK",
            &stats_json(stats).to_string(),
            origin,
        );
    } else if let (true, Some(command)) = (method == "POST", path.strip_prefix("/invoke/")) {
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let started = Instant::now();
        let result = dispatch(state, command, &payload);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(command.to_owned())
            .or_default()
            .push(ms);
        respond(&mut stream, "200 OK", &result.to_string(), origin);
    } else {
        respond(&mut stream, "404 Not Found", "{}", origin);
    }
}

/// 128 random bits, hex: `RandomState` is keyed from the operating system's entropy per instance.
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = || {
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

fn main() {
    let root = std::env::args()
        .nth(1)
        .expect("usage: editor_ipc_bridge <vault directory inside the temp directory>");
    // A write-capable server must not be pointable at a real vault.
    let canonical = std::fs::canonicalize(&root).expect("the vault directory exists");
    let temp = std::fs::canonicalize(std::env::temp_dir()).expect("the temp directory exists");
    assert!(
        canonical.starts_with(&temp) && canonical != temp,
        "refusing {}: the bridge only opens a directory inside {}",
        canonical.display(),
        temp.display()
    );
    let state = Arc::new(AppState::with_settings(
        Arc::new(Startup::begin()),
        Boot::default(),
        Settings::default(),
    ));
    state
        .open_vault(OpenVaultArgs { root })
        .expect("open the vault");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !matches!(state.index_status().state, IndexState::Ready { .. }) {
        assert!(Instant::now() < deadline, "the index never became ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let token = random_token();
    println!("READY {port} {token}");
    let _ = std::io::stdout().flush();

    // Exit when the harness closes our stdin.
    std::thread::spawn(|| {
        let mut sink = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut sink);
        std::process::exit(0);
    });

    let stats: Stats = Arc::default();
    for stream in listener.incoming().flatten() {
        let (state, stats, token) = (Arc::clone(&state), Arc::clone(&stats), token.clone());
        std::thread::spawn(move || serve(stream, &state, &stats, &token, port));
    }
}
