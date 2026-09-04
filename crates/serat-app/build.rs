//! Tauri's build step: it reads `tauri.conf.json` and generates the context the
//! `tauri::generate_context!` macro expands to.
//!
//! Nothing Serat-specific happens here. The `ts-rs` export that ADR-0007 depends on is
//! *not* a build step: it runs with the crate's tests, which is what makes a stale
//! committed `.ts` file a test failure rather than something a build silently fixes.

fn main() {
    tauri_build::build();
}
