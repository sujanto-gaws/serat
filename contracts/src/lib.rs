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

#[cfg(test)]
mod wire {
    //! Wire-shape tests for contract changes made ahead of their implementation.

    use crate::ipc::{IndexState, OpeningReason};

    /// Task #115: `Opening` gained an optional `reason`. The unit-shaped
    /// `{"kind":"opening"}` that every existing sender and receiver uses must stay
    /// valid in both directions, or the contract change is a breaking one.
    #[test]
    fn opening_without_a_reason_keeps_its_old_wire_shape() {
        let bare = IndexState::Opening { reason: None };
        assert_eq!(
            serde_json::from_str::<IndexState>(r#"{"kind":"opening"}"#).expect("parses"),
            bare
        );
        assert_eq!(
            serde_json::to_string(&bare).expect("serialises"),
            r#"{"kind":"opening"}"#
        );
    }

    #[test]
    fn opening_with_a_reason_round_trips() {
        let waiting = IndexState::Opening {
            reason: Some(OpeningReason::PreviousIndexStillClosing),
        };
        let json = serde_json::to_string(&waiting).expect("serialises");
        assert_eq!(
            json,
            r#"{"kind":"opening","reason":"previous_index_still_closing"}"#
        );
        assert_eq!(
            serde_json::from_str::<IndexState>(&json).expect("parses"),
            waiting
        );
    }
}
