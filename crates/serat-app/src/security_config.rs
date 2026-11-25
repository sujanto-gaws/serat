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
//! Historically none of this was enforced by Tauri on Serat's behalf — F-280/F-336
//! found that capability resolution was skipped entirely for local, unmanifested
//! application commands, so `capabilities/default.json` constrained none of the six
//! commands in `generate_handler!`. ADR-0012 changed that: `build.rs` now declares an
//! app ACL manifest, so the `permissions` list this module checks is real enforcement
//! for five of the six commands (`open_vault` deliberately withheld, F-342). See
//! `acl_manifest.rs` for the guard that keeps `generate_handler!`, `build.rs`'s
//! manifest and this file's grants from drifting apart — a concern this module's tests
//! do not cover, because they check values within *one* file at a time rather than
//! agreement across three. These tests exist so a change to either file is a decision
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
    use crate::acl_manifest::permission_identifier;
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
        // hard for injected script to reach when `withGlobalTauri` was `true` — CSP
        // does not gate `initialization_script` at all, so this was the only CSP term
        // that finding depended on (moot now that `withGlobalTauri` is `false`, below,
        // but the directive stays pinned regardless).
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
             F-281 found this is what made window.__TAURI__ hard for injected script to \
             reach while withGlobalTauri was true; CSP does not gate initialization_script \
             directly"
        );
        assert!(
            directives.contains(&"style-src 'self'"),
            "tauri.conf.json's CSP no longer pins style-src to 'self' (was {csp:?}) — \
             F-417/task #116 (measured over the DevTools Protocol against a real \
             WebView2 window) found this directive genuinely blocks CodeMirror's own \
             runtime stylesheet unless the editor is mounted into a shadow root \
             (src/editor/CodeMirrorEditor.vue); before this test existed, a later \
             'unsafe-inline' added to work around that would have landed with nothing \
             going red"
        );
        assert!(
            directives.contains(
                &"connect-src 'self' ipc: http://ipc.localhost serat-asset: http://serat-asset.localhost"
            ),
            "tauri.conf.json's CSP no longer pins connect-src to the canonical narrow \
             value (was {csp:?}) — task P0 #117 / F-423(c): with no connect-src directive \
             the value fell back to default-src 'self', which blocks the event|listen \
             IPC call @tauri-apps/api/event's listen() makes, so onIndexStatus()'s push \
             subscription was silently dead in a real WebView2 window. `ipc: \
             http://ipc.localhost` is Tauri's own documented minimal directive for its \
             IPC channel (tauri-utils-2.9.3/src/config.rs:2741) — the narrowest value \
             that restores the call without opening connect-src to anything else. \
             T-02's enforcement moves from default-src's fallback to this directive \
             once it is set explicitly, so a wider value here would take over that \
             enforcement silently; confirmed with vue-frontend-dev that nothing in \
             app/src makes any other network-shaped call (F-423(c), P0 #117). \
             `'self'` was added by task #127 (ADR-0017 item 6, Amendment 2): pdf.js \
             fetches its bundled cMaps and standard fonts as same-origin relative URLs, \
             which `connect-src` governs; it is first-party only, and `form-action` and \
             `base-uri` stay 'none' so a re-pointed base cannot redirect those fetches."
        );
        assert!(
            directives.contains(&"img-src 'self' serat-asset: http://serat-asset.localhost"),
            "tauri.conf.json's CSP img-src is not exactly 'self' plus the two serat-asset \
             origins (was {csp:?}) -- ADR-0017: the only CSP widening task #19 makes. No \
             remote origin, no data:, no blob: (section 9.3 drops data: URLs)"
        );
        for forbidden in ["data:", "blob:", "'unsafe-eval'", "'unsafe-inline'", "*"] {
            assert!(
                !csp.split_whitespace().any(|token| token == forbidden),
                "tauri.conf.json's CSP contains {forbidden:?} (was {csp:?}) -- ADR-0017 \
                 allows no data:, blob:, wildcard or unsafe-* source in the app CSP"
            );
        }
        assert!(
            directives.contains(&"frame-src 'none'"),
            "tauri.conf.json's CSP no longer pins frame-src to 'none' (was {csp:?}) — \
             task #37, from the ADR-0017 correction review: WebView2's \
             NavigationStarting event in wry only fires for main-frame navigation, so \
             `on_navigation` (nav_guard.rs) never sees a navigation inside an <iframe>. \
             A framed serat-asset:// (or any other) document would still be classified \
             `Origin::Local` by Tauri and able to invoke every command granted to \
             `main` (ADR-0017's correction), with no guard watching it. default-src \
             'self' would fall back to permitting a same-origin frame; frame-src \
             'none' is the only directive that forbids framing at all, from any origin, \
             so it has to be explicit rather than left to the fallback."
        );
        assert!(
            directives.contains(&"object-src 'none'"),
            "tauri.conf.json's CSP no longer pins object-src to 'none' (was {csp:?}) — \
             task #37: <object>/<embed> are another way to load a plugin-handled \
             sub-document (including, on some engines, one served from a registered \
             URI scheme) outside on_navigation's main-frame-only reach, and ADR-0017's \
             own §Decision step 10 already forbids `object`/`embed` from rendered \
             Markdown; this is the same rule enforced at the CSP layer rather than \
             only by the sanitiser."
        );
        assert!(
            directives.contains(&"form-action 'none'"),
            "tauri.conf.json's CSP no longer pins form-action to 'none' (was {csp:?}) -- \
             task #115, ADR-0017 item 7 / T-09: form-action does NOT fall back to \
             default-src, so without it a form in the app document may submit to any \
             origin. on_navigation refuses a top-level submission, but it sees the main \
             frame only; this is the layer that does not depend on it. The one app form \
             (FileTree.vue's prompt) cancels its submit event, which never reaches the \
             form-action check."
        );
        assert!(
            directives.contains(&"base-uri 'none'"),
            "tauri.conf.json's CSP no longer pins base-uri to 'none' (was {csp:?}) -- \
             task #115, ADR-0017 item 7 / T-09: base-uri does not fall back to \
             default-src either, and an injected <base href> would re-point every \
             relative URL the app resolves (pdf.js's worker, a future cMapUrl). The \
             app emits no <base> element (Vite's base: \"./\" is a build setting)."
        );
    }

    #[test]
    fn no_csp_directive_admits_a_remote_origin_and_the_whole_string_is_pinned() {
        // NFR-SEC-01 / NFR-SEC-02 / T-02 / T-09. The individual directives above are
        // each pinned; this is the other direction: whatever the string says, every
        // source of every fetch directive is `'self'`, one of the two ADR-0017 scheme
        // origins, or Tauri's own IPC channel (connect-src only), and framing/embedding
        // is 'none'. A new directive, or a new source on an old one, fails here.
        const EXPECTED: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
            img-src 'self' serat-asset: http://serat-asset.localhost; \
            connect-src 'self' ipc: http://ipc.localhost serat-asset: http://serat-asset.localhost; \
            frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";
        let csp = tauri_conf()["app"]["security"]["csp"]
            .as_str()
            .expect("app.security.csp must be a string")
            .to_owned();
        assert_eq!(
            csp, EXPECTED,
            "the CSP changed: re-run the no-network review (NFR-SEC-01)"
        );

        const SCHEME_ORIGINS: [&str; 2] = ["serat-asset:", "http://serat-asset.localhost"];
        const IPC_ORIGINS: [&str; 2] = ["ipc:", "http://ipc.localhost"];
        const FETCH_DIRECTIVES: [&str; 10] = [
            "default-src",
            "script-src",
            "style-src",
            "img-src",
            "font-src",
            "media-src",
            "worker-src",
            "connect-src",
            "manifest-src",
            "prefetch-src",
        ];
        // `worker-src` is absent and falls back to `script-src 'self'`, which is right for the
        // same-origin module worker pdf.js is given (viewers/pdfEngine.ts); adding it later
        // is checked by the loop below.
        for directive in csp.split(';').map(str::trim).filter(|d| !d.is_empty()) {
            let mut tokens = directive.split_whitespace();
            let name = tokens.next().expect("a directive name");
            let sources: Vec<&str> = tokens.collect();
            if FETCH_DIRECTIVES.contains(&name) {
                for source in sources {
                    let ok = source == "'self'"
                        || SCHEME_ORIGINS.contains(&source)
                        || (name == "connect-src" && IPC_ORIGINS.contains(&source));
                    assert!(
                        ok,
                        "{name} admits {source:?}: not 'self' or a scheme origin"
                    );
                }
            } else {
                assert!(
                    matches!(
                        name,
                        "frame-src" | "object-src" | "form-action" | "base-uri"
                    ) && sources == ["'none'"],
                    "unexpected CSP directive {directive:?}"
                );
            }
        }
    }

    #[test]
    fn capabilities_still_grant_exactly_core_default_the_five_adr_0012_commands_and_opener() {
        // capabilities/default.json's own `description` says widening this list is a
        // threat-model decision, not a convenience. ADR-0012 widened it once, on
        // purpose, from ["core:default"] to core:default plus the five commands
        // boot.js invokes; task #37 widened it a second time, on purpose, to a
        // *scoped* opener grant (see the dedicated scope test below). This pins the
        // *new* narrow set so the next widening is equally deliberate.
        // acl_manifest.rs checks the app-command grant against generate_handler! and
        // build.rs directly — this test only guards against the list drifting
        // silently within this one file, for both command grants and scoped ones.
        let mut permissions: Vec<String> = capabilities_default()["permissions"]
            .as_array()
            .expect("capabilities/default.json must have a permissions array")
            .iter()
            .map(permission_identifier)
            .collect();
        permissions.sort();
        let mut expected: Vec<String> = vec![
            "allow-append-gitignore-entry",
            "allow-backlinks",
            "allow-resolve-links",
            "allow-note-anchors",
            "allow-decline-gitignore-offer",
            "allow-gitignore-offer-status",
            "allow-get-ui-settings",
            "allow-set-keybinding",
            "allow-set-live-preview",
            "allow-create-folder",
            "allow-asset-info",
            "allow-choose-vault",
            "allow-index-status",
            "allow-mark-first-paint",
            "allow-mark-tree",
            "allow-mark-vault-usable",
            "allow-note-closed",
            "allow-note-opened",
            "allow-open-tree",
            "allow-read-asset-range",
            "allow-read-file",
            "allow-rebase-buffer",
            "allow-flush-ack",
            "allow-refused-names",
            "allow-request-external-diff",
            "allow-save-patch",
            "allow-search",
            "allow-switcher-query",
            "allow-write-file",
            "core:default",
            "opener:allow-open-url",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        expected.sort();
        assert_eq!(
            permissions, expected,
            "capabilities/default.json's permissions are {permissions:?}, expected \
             {expected:?} (ADR-0012's five commands, core:default, and task #37's scoped \
             opener grant) — its own description says widening this is a threat-model \
             decision; if that decision was made deliberately, update this assertion in \
             the same commit as the reasoning"
        );
    }

    #[test]
    fn the_opener_grant_is_scoped_to_exactly_http_https_and_mailto() {
        // Task #37, §9.3, T-09: the opener plugin's own `default` permission set
        // (tauri-plugin-opener-2.5.5/permissions/allow-default-urls.toml) also grants
        // `tel:*` and reveal-in-file-explorer; Serat wants neither. This asserts the
        // capability file grants the narrower, hand-scoped set instead of ever
        // widening to the plugin's own `opener:default` -- a switch to `"opener:default"`
        // would pass `capabilities_still_grant_exactly_core_default_the_five_adr_0012_commands_and_opener`
        // if its expected set were updated carelessly to match, but this test reads the
        // actual scope urls and would still catch `tel:*` arriving with it.
        let permissions = capabilities_default()["permissions"]
            .as_array()
            .expect("capabilities/default.json must have a permissions array")
            .clone();
        let opener_entry = permissions
            .iter()
            .find(|entry| permission_identifier(entry) == "opener:allow-open-url")
            .expect("capabilities/default.json must grant opener:allow-open-url");
        let mut urls: Vec<String> = opener_entry["allow"]
            .as_array()
            .expect("the opener grant must carry an \"allow\" scope array")
            .iter()
            .map(|scope_entry| {
                scope_entry["url"]
                    .as_str()
                    .expect("each opener scope entry must have a \"url\" string")
                    .to_owned()
            })
            .collect();
        urls.sort();
        let mut expected: Vec<String> = vec!["http://*", "https://*", "mailto:*"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        expected.sort();
        assert_eq!(
            urls, expected,
            "opener:allow-open-url's scope is {urls:?}, expected exactly {expected:?} — \
             in particular this must never include \"tel:*\", which the plugin's own \
             default permission set grants and Serat does not want"
        );
    }

    #[test]
    fn the_main_window_pins_the_http_custom_protocol_scheme() {
        // ADR-0017: `app_origin()` and the asset CSP hard-code `http://tauri.localhost`
        // and `http://serat-asset.localhost`. A builder that left the Windows scheme
        // at Tauri's default (or chose https) would make every CORS origin check
        // compare against the wrong string, so the choice is explicit and pinned.
        const LIB_RS: &str = include_str!("lib.rs");
        assert!(
            LIB_RS.contains(".use_https_scheme(false)"),
            "lib.rs must call .use_https_scheme(false) on the main WebviewWindowBuilder"
        );
        assert!(
            !LIB_RS.contains("use_https_scheme(true)"),
            "use_https_scheme(true) would move the Windows origin to https://"
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
    fn with_global_tauri_is_off_now_that_nothing_reads_it() {
        // F-281 closed `withGlobalTauri: true` as sound (the load-bearing control was
        // `boot_info`'s JSON escaping, not the CSP), but sound is not the same as
        // needed. Task #116's sweep found the built bundle contains zero `__TAURI__`
        // occurrences — the frontend goes through `@tauri-apps/api`'s module IPC
        // exclusively, since #98 replaced the pre-#98 plain-script `boot.js` that
        // needed the global. Turning it off removes the object F-281's reasoning had
        // to defend, strictly narrows the surface, and costs nothing this codebase
        // uses. A red test here means something started reading `window.__TAURI__`
        // again — re-open F-281 rather than flip this assertion back without asking
        // why.
        let with_global_tauri = tauri_conf()["app"]["withGlobalTauri"]
            .as_bool()
            .expect("app.withGlobalTauri must be a bool");
        assert!(
            !with_global_tauri,
            "app.withGlobalTauri is back to true — confirm something actually reads \
             window.__TAURI__ before restoring it; task #116 found nothing did"
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
