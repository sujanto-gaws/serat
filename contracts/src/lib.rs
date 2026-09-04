//! The repo-ready IPC contract, compiled.
//!
//! The contract itself is `contracts/ipc.rs`, one directory up, so that the path
//! named throughout `docs/03-design/architecture.md` stays the path the file is
//! at. This crate is the thin wrapper that makes the workspace build it and run
//! its `ts-rs` export, so a break is a failed test rather than a discovery made
//! by whoever includes it next.
//!
//! Generate the TypeScript with:
//!
//! ```sh
//! TS_RS_EXPORT_DIR=../app/src/ipc/generated cargo test -p serat-contracts
//! ```

#[path = "../ipc.rs"]
pub mod ipc;
