//! Keeps the main webview on the app's own origin, and refuses the handful of
//! things a webview can be pushed to do that this application never wants:
//! opening a second window, downloading a file the vault never asked for.
//!
//! # Why this exists (task #37)
//!
//! architecture.md §9.3: *"Navigation is guarded separately, because the CSP
//! does not cover it."* A rendered `<a href="https://…">`, a `javascript:`
//! link, a meta-refresh, or a script setting `location.href` would each
//! navigate the **top-level** webview to a different origin if nothing
//! stopped them — and `connect-src`/`img-src` in the CSP say nothing about
//! top-level navigation at all. Worse, per tauri-app-dev's review of task #7
//! (ADR-0017's correction, task #36): Tauri classifies **every scheme this
//! process registers itself** — `serat-asset` included — as a *local* origin
//! (`tauri-2.11.5/src/webview/mod.rs:1698-1739`), and a local origin's
//! `invoke()` calls are checked against the same capability grant the app
//! bundle gets (`tauri-utils-2.9.3/src/ipc/authority.rs:57-66`). So a
//! `serat-asset://…` document that became the top-level page could invoke
//! every command granted to `main` — there is no origin-classification
//! control. `on_navigation` is therefore the only thing standing between a
//! scheme this process itself registers and the full IPC surface, and it has
//! to refuse **every** origin but the app's own, not just remote ones.
//!
//! **`on_new_window`** closes the same door opened a different way:
//! `window.open(...)` and a target="_blank" link would otherwise create a
//! second webview with no guard installed on it at all. P1 has exactly one
//! window, so the answer is unconditional refusal, not a second capability
//! file to keep in step with this one.
//!
//! **`on_download`** exists because a webview's native download flow (a
//! response with `Content-Disposition: attachment`, or `<a download>`) writes
//! a file to a location outside the vault, outside `write_atomic`, and
//! outside every containment check `serat-vault` owns (design goal 1, T-07).
//! Nothing in P1 wants a browser download; refusing it is a click, not a
//! feature removed.
//!
//! # What is proven here, and what needs a real window
//!
//! [`is_app_navigation_target`] is the actual decision `on_navigation` makes,
//! extracted so it can be checked against a table of URLs headlessly and
//! deterministically — this is what `cargo test -p serat-app` exercises.
//! `on_new_window`'s and `on_download`'s answers are unconditional constants
//! with no decision to extract, so there is nothing to table-test; their
//! argument types (`NewWindowResponse`, `DownloadEvent`) are `#[non_exhaustive]`
//! and cannot even be constructed from outside the `tauri` crate, which is a
//! second reason there is no unit-level fixture available for them.
//!
//! None of the three can be driven through an actual rendered `<a>` click, a
//! `javascript:` URI, a meta-refresh tag, or a real download response without
//! a running webview — `tauri::test::mock_app()` crashes on this toolchain
//! before any Serat code runs (ADR-0012's *Tested against*, F-345), the same
//! wall the ACL verification hit. That leaves two paths to real evidence:
//! `xvfb-run` against the compiled binary (attempted for this task; see the
//! task report for what it showed and did not) and, longer term,
//! test-engineer's Playwright harness (NFR-PRF-02) driving the production
//! build against a real WebView2/WebKitGTK/WKWebView window per platform.

use tauri::Url;

/// Which custom-protocol spelling the webview serves the bundle under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    /// WebView2: `http://tauri.localhost/`.
    Windows,
    /// WebKitGTK / WKWebView: `tauri://localhost/`.
    Other,
}

impl Platform {
    pub(crate) const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// Whether `url` is somewhere `on_navigation` must allow the top-level webview
/// to go: the app's own bundle in `platform`'s spelling only, or the
/// configured dev server while `cfg!(debug_assertions)` is true. Everything
/// else — an external site, a `javascript:` URI, a meta-refresh target, and
/// deliberately including `serat-asset://…` / `http://serat-asset.localhost/…`
/// — is refused.
///
/// `dev_server_origin` is `app.config().build.dev_url` in production, read
/// once at window-build time and passed in rather than read from a global, so
/// this function stays a pure predicate over its arguments. It is `None`
/// whenever `tauri.conf.json` sets no `build.devUrl` (true of every build
/// today; P1's vue-frontend-dev dev server will set one), and in release
/// builds the caller must pass `None` regardless of what is configured —
/// `debug_assertions` is a property of *this build*, not of the config file,
/// and a release binary must never trust a dev server origin baked into a
/// config that ships with it.
pub(crate) fn is_app_navigation_target(
    url: &Url,
    dev_server_origin: Option<&Url>,
    platform: Platform,
) -> bool {
    // No port, no userinfo: wry registers the Windows filter as the prefix
    // `http://tauri.localhost/*`, so `http://tauri.localhost:8080/` is not
    // intercepted, reaches the network, resolves to loopback, and tauri's
    // `is_local_url` (scheme + domain only) would call it a local origin.
    let bare = url.port().is_none() && url.username().is_empty() && url.password().is_none();
    let is_app_bundle = bare
        && match platform {
            Platform::Windows => {
                url.scheme() == "http" && url.host_str() == Some("tauri.localhost")
            }
            Platform::Other => url.scheme() == "tauri" && url.host_str() == Some("localhost"),
        };

    let is_dev_server =
        cfg!(debug_assertions) && dev_server_origin.is_some_and(|dev| dev.origin() == url.origin());

    is_app_bundle || is_dev_server
}

/// Cancels WebKitGTK's default right-click menu on `window`. Release builds
/// only (task #37) — the menu is how a developer without a bound shortcut
/// reaches "Inspect Element" in a debug build, and P1 has no shipped
/// alternative for that yet.
///
/// Linux/WebKitGTK only. **Windows (WebView2) and macOS (WKWebView) are not
/// covered here** — see the call site's doc comment in `lib.rs` for why
/// each is a separate, real piece of platform-specific work rather than a
/// small addition to this function.
///
/// The context menu is not merely cosmetic surface: "Open Link in New
/// Window" and "Open Link" on it reach a navigation `on_navigation` and
/// `on_new_window` already refuse through this crate's own code paths, so
/// this closes the same door by a route neither of those two seed. Returning
/// `true` from WebKitGTK's `context-menu` signal handler is documented as
/// cancelling the menu outright rather than merely customising it.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
pub(crate) fn disable_default_context_menu<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    // `with_webview` runs its closure on the main thread; we are already on
    // it (called from `setup`, before the event loop starts), so this is a
    // direct call rather than a hop. The result is ignored deliberately: the
    // one way this fails is a webview handle that no longer exists, which
    // for a window `build()` just returned is a defect the caller would
    // already have hit elsewhere, and a missing menu-suppression on that dead
    // handle has nothing left to protect.
    let _ = window.with_webview(|webview| {
        use webkit2gtk::WebViewExt as _;
        webview
            .inner()
            .connect_context_menu(|_view, _menu, _event, _hit_test| true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap_or_else(|err| panic!("{s:?} must parse as a URL: {err}"))
    }

    #[test]
    fn the_app_bundle_is_allowed_on_both_platform_spellings() {
        let w = Platform::Windows;
        let o = Platform::Other;
        assert!(is_app_navigation_target(
            &u("tauri://localhost/index.html"),
            None,
            o
        ));
        assert!(is_app_navigation_target(
            &u("http://tauri.localhost/index.html"),
            None,
            w
        ));
        // Each spelling is refused on the other platform.
        assert!(!is_app_navigation_target(
            &u("http://tauri.localhost/"),
            None,
            o
        ));
        assert!(!is_app_navigation_target(&u("tauri://localhost/"), None, w));
    }

    #[test]
    fn port_and_userinfo_on_the_bundle_origin_are_refused() {
        for (url, p) in [
            ("http://tauri.localhost:8080/", Platform::Windows),
            ("http://tauri.localhost:443/", Platform::Windows),
            ("http://tauri.localhost:0/", Platform::Windows),
            ("http://user@tauri.localhost/", Platform::Windows),
            ("http://user:pw@tauri.localhost/", Platform::Windows),
            ("http://:pw@tauri.localhost/", Platform::Windows),
            ("tauri://localhost:1234/", Platform::Other),
            ("tauri://user@localhost/", Platform::Other),
        ] {
            assert!(!is_app_navigation_target(&u(url), None, p), "{url}");
        }
    }

    #[test]
    fn an_external_https_link_is_refused() {
        // §9.3's "an external link" hostile case.
        assert!(!is_app_navigation_target(
            &u("https://evil.example/"),
            None,
            Platform::current()
        ));
        assert!(!is_app_navigation_target(
            &u("http://evil.example/"),
            None,
            Platform::current()
        ));
    }

    #[test]
    fn a_javascript_uri_is_refused() {
        // §9.3's "a javascript: link" hostile case.
        assert!(!is_app_navigation_target(
            &u("javascript:alert(1)"),
            None,
            Platform::current()
        ));
    }

    #[test]
    fn a_meta_refresh_target_is_refused() {
        // A meta-refresh navigates the top-level document to an ordinary URL;
        // there is nothing meta-refresh-specific about the check, which is
        // the point -- on_navigation sees a target URL regardless of what
        // triggered the navigation attempt, and this asserts the target a
        // meta-refresh would carry is refused exactly like a clicked link.
        assert!(!is_app_navigation_target(
            &u("https://evil.example/redirected"),
            None,
            Platform::current()
        ));
    }

    #[test]
    fn a_top_level_navigation_to_serat_asset_is_refused_on_both_platform_spellings() {
        // ADR-0017's correction (task #36): serat-asset is a *local* origin to
        // Tauri, so on_navigation is the only control keeping a top-level
        // navigation to it from reaching every command granted to `main`.
        // This is arm (1) of the hostile pair the ADR names; arm (2) (a
        // guard-disabled build proving the sandbox CSP holds alone) needs the
        // scheme handler and lands with #19.
        assert!(!is_app_navigation_target(
            &u("serat-asset://localhost/note.pdf"),
            None,
            Platform::current()
        ));
        assert!(!is_app_navigation_target(
            &u("http://serat-asset.localhost/note.pdf"),
            None,
            Platform::current()
        ));
    }

    #[test]
    fn the_dev_server_is_allowed_only_in_a_debug_build_and_only_when_configured() {
        let dev = u("http://localhost:5173/");
        // cfg!(debug_assertions) is a property of *this test binary*; tests
        // always run as a debug build, so this exercises the "configured"
        // half of the rule. The "only in a debug build" half cannot be
        // exercised from within a single test binary -- it would need a
        // release build of this same crate, which is why the doc comment
        // states the rule as a fact about the caller's responsibility
        // (passing None in release) rather than something this function can
        // prove about itself from one build.
        assert!(is_app_navigation_target(
            &u("http://localhost:5173/app.js"),
            Some(&dev),
            Platform::current()
        ));
        // A different port is a different origin and must still be refused --
        // "the dev server" means exactly the one URL that was configured, not
        // "anything on localhost".
        assert!(!is_app_navigation_target(
            &u("http://localhost:5174/"),
            Some(&dev),
            Platform::current()
        ));
    }

    #[test]
    fn no_dev_server_configured_means_no_dev_exception_exists() {
        // Every build today: tauri.conf.json sets no build.devUrl. Passing
        // None must not silently become "allow anything on localhost".
        assert!(!is_app_navigation_target(
            &u("http://localhost:5173/"),
            None,
            Platform::current()
        ));
    }
}
