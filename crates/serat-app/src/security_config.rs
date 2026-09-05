//! A regression guard for the security-relevant settings in `tauri.conf.json` and
//! `capabilities/default.json` — not a mechanism, a promise that neither file drifts
//! silently.
//!
//! # Why this exists (task #21, F-278)
//!
//! The Tauri surface audit (task #18) found that every security-relevant setting in
//! these two files was removable, or addable, with nothing going red: the CSP,
//! `capabilities/default.json`'s `core:default` grant, the window label, `withGlobalTauri`,
//! and the *absence* of `assetProtocol`. The sharp part is that this covers the CSP
//! that the same audit credited as **the one genuine T-02 enforcement in the whole
//! codebase** (F-274) — the only real control found was the one with nothing guarding
//! it. `crates/serat-app/build.rs` is the only file in the repository that references
//! either JSON file at all.
//!
//! `include_str!` compiles both files into the test binary, so a stale checkout or a
//! symlink games nothing: the bytes under test are the bytes that ship.
//!
//! # What this is not
//!
//! None of this is enforced by Tauri on Serat's behalf — see F-280: capability
//! resolution is skipped entirely for local, unmanifested application commands, so
//! `capabilities/default.json` currently constrains none of the six commands in
//! `generate_handler!`. These tests exist so a change to either file is a decision
//! someone makes on purpose, not a side effect nobody notices — the F-40 question
//! ("which of these guards can be removed without a test going red") answered for all
//! five settings rather than for none of them.

/// The window label [`crate::run`] passes to `WebviewWindowBuilder::new`.
///
/// The single source of truth for the label, so `capabilities/default.json`'s `windows`
/// entry is checked against the value the builder actually uses rather than against a
/// second, independently-typed guess of what that value probably is (task #18 finding
/// 3: the two were previously two separate spellings of "main" with nothing comparing
/// them).
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// `tauri.conf.json`, compiled in rather than read at test time.
    const TAURI_CONF: &str = include_str!("../tauri.conf.json");

    /// `capabilities/default.json`, compiled in rather than read at test time.
    const CAPABILITIES_DEFAULT: &str = include_str!("../capabilities/default.json");

    fn tauri_conf() -> Value {
        serde_json::from_str(TAURI_CONF).expect("tauri.conf.json must be valid JSON")
    }

    fn capabilities_default() -> Value {
        serde_json::from_str(CAPABILITIES_DEFAULT)
            .expect("capabilities/default.json must be valid JSON")
    }

    #[test]
    fn the_csp_still_pins_default_src_and_script_src_to_self() {
        // T-02 (a compromised or injected script reaching an arbitrary origin) has
        // exactly one enforcement in this codebase (F-274): with no `connect-src`
        // directive, `connect-src` falls back to `default-src`, so `default-src 'self'`
        // is what stops a page reaching an attacker's server. `script-src 'self'` is
        // also load-bearing on its own: F-281 found it is what makes `window.__TAURI__`
        // (exposed by `withGlobalTauri`, below) hard for injected script to reach —
        // CSP does not gate `initialization_script` at all, so this is the only CSP
        // term that finding depends on.
        let csp = tauri_conf()["app"]["security"]["csp"]
            .as_str()
            .expect("app.security.csp must be a string")
            .to_owned();
        let directives: Vec<&str> = csp.split(';').map(str::trim).collect();
        assert!(
            directives.contains(&"default-src 'self'"),
            "tauri.conf.json's CSP no longer pins default-src to 'self' (was {csp:?}) — \
             this is T-02's only enforcement in the codebase (F-274)"
        );
        assert!(
            directives.contains(&"script-src 'self'"),
            "tauri.conf.json's CSP no longer pins script-src to 'self' (was {csp:?}) — \
             F-281 found this is what makes window.__TAURI__ (withGlobalTauri) hard for \
             injected script to reach; CSP does not gate initialization_script directly"
        );
        assert!(
            !csp.contains("connect-src"),
            "tauri.conf.json's CSP now sets connect-src explicitly (was {csp:?}) — a \
             connect-src directive stops falling back to default-src and takes over \
             T-02's enforcement on its own, so confirm the new value is 'self' or \
             narrower before trusting it"
        );
    }

    #[test]
    fn capabilities_still_grant_only_core_default() {
        // capabilities/default.json's own `description` says widening this list is a
        // threat-model decision, not a convenience. Nothing previously checked that the
        // list stayed narrow; this pins it to exactly the one permission set that grants
        // no application command (NFR-SEC-02, NFR-SEC-05).
        let permissions: Vec<String> = capabilities_default()["permissions"]
            .as_array()
            .expect("capabilities/default.json must have a permissions array")
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .expect("permission entries must be strings")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            permissions,
            vec!["core:default".to_owned()],
            "capabilities/default.json's permissions changed from [\"core:default\"] to \
             {permissions:?} — its own description says widening this is a threat-model \
             decision; if that decision was made deliberately, update this assertion in \
             the same commit as the reasoning"
        );
    }

    #[test]
    fn the_granted_window_label_matches_the_one_the_builder_creates() {
        // Task #18 finding 3: WebviewWindowBuilder::new(app, ..., ...) in lib.rs and
        // capabilities/default.json's `windows` array were two independent spellings of
        // "main" with nothing checking they agreed. tauri.conf.json's own `app.windows`
        // is `[]` — the label lives only in Rust — so the capability file is compared
        // against the constant the builder actually uses. Low severity because it fails
        // closed (a window with no matching capability gets no core permissions), which
        // is a reason to keep the check cheap, not a reason to skip it.
        let windows: Vec<String> = capabilities_default()["windows"]
            .as_array()
            .expect("capabilities/default.json must have a windows array")
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .expect("window entries must be strings")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            windows,
            vec![MAIN_WINDOW_LABEL.to_owned()],
            "capabilities/default.json's windows {windows:?} no longer match the label \
             the builder creates ({MAIN_WINDOW_LABEL:?}) — this fails closed today \
             (mismatched labels get no core permissions), but the divergence should be \
             a decision, not something that lands unnoticed"
        );
    }

    #[test]
    fn with_global_tauri_and_its_covering_reason_both_still_hold() {
        // F-281: `withGlobalTauri: true` is sound only because `script-src 'self'`
        // (asserted above) makes `window.__TAURI__` hard for injected script to reach.
        // A red test here means that pairing needs re-checking, not that the setting is
        // wrong on its own — flipping it to `false` would be a strictly safer change
        // that this test would still (correctly) flag, so a reviewer re-confirms the
        // finding is now moot rather than the assumption silently going stale.
        let with_global_tauri = tauri_conf()["app"]["withGlobalTauri"]
            .as_bool()
            .expect("app.withGlobalTauri must be a bool");
        assert!(
            with_global_tauri,
            "app.withGlobalTauri changed to false — not a regression (it narrows the \
             surface), but F-281's finding was written against `true`; if this is now \
             false, close F-281 as moot instead of leaving it referring to a setting \
             that no longer exists"
        );
    }

    #[test]
    fn the_asset_protocol_is_still_absent() {
        // Task #18 finding 5: correct today because nothing serves vault content
        // through the asset protocol. FR-BIN's P2 PDF/image viewing is exactly the
        // feature that will want to turn this on, and assetProtocol.scope then decides
        // whether the symlink containment gap (task #20 — Vault::read has no
        // containment re-check) reopens on a wider path. This must fail the day
        // someone adds the key, so enabling it is a decision made next to that check
        // rather than a default nobody looked at twice.
        let security = tauri_conf()["app"]["security"].clone();
        assert!(
            security.get("assetProtocol").is_none(),
            "tauri.conf.json now declares app.security.assetProtocol ({:?}) — this is \
             FR-BIN's P2 decision arriving; re-open the symlink containment gap (task \
             #20) against assetProtocol.scope before widening it further",
            security.get("assetProtocol")
        );
    }

    #[test]
    fn each_guard_is_checked_against_a_value_that_can_actually_diverge() {
        // F-58/F-40: a guard that cannot fail is worse than no guard, because it reads
        // as coverage. This is the "setup reaches the path" half of that check, run
        // once over all five values rather than trusted by inspection: every value read
        // above is present, non-empty where a list is expected, and of the type each
        // assertion assumes — so a JSON file edited to remove a key entirely fails here
        // with a clear cause instead of a panic inside `tauri_conf()`/`capabilities_default()`
        // whose message names the accessor rather than the setting.
        let conf = tauri_conf();
        assert!(conf["app"]["security"]["csp"].is_string());
        assert!(conf["app"]["withGlobalTauri"].is_boolean());
        let caps = capabilities_default();
        assert!(caps["permissions"].is_array());
        assert!(!caps["permissions"].as_array().unwrap().is_empty());
        assert!(caps["windows"].is_array());
        assert!(!caps["windows"].as_array().unwrap().is_empty());
    }
}
