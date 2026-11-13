//! The hostile suite for the `serat-asset` scheme (ADR-0017 *Tested against*,
//! task #19). Every case that can run without a window runs here, through the
//! real [`serve`] on both platform spellings; the cases that need a rendered
//! document are at the bottom as `#[ignore]` tests with the reason.
//!
//! Each refusal test has a **control** beside it (the same request against a
//! legitimate file succeeds), so a test cannot pass merely because the fixture
//! was broken and everything 404s.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

const BOTH: [Platform; 2] = [Platform::Windows, Platform::Other];

/// Small limits so boundaries are exercised with tiny files.
const SMALL: Limits = Limits {
    span_cap: 10,
    image_cap: 100,
};

struct Fx {
    /// The directory holding the vault and a sibling the vault must never reach.
    base: PathBuf,
    vault: Vault,
    budget: ByteBudget,
}

impl Fx {
    fn new(name: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let base = crate::test_temp_dir().join(format!(
            "serat-asset-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("vault")).unwrap();
        fs::create_dir_all(base.join("outside")).unwrap();
        fs::write(base.join("outside/secret.png"), b"OUTSIDE-SECRET").unwrap();
        let vault = Vault::open(base.join("vault")).unwrap();
        Self {
            base,
            vault,
            budget: ByteBudget::new(IN_FLIGHT_BYTES),
        }
    }

    fn put(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.base.join("vault").join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn outside(&self) -> PathBuf {
        self.base.join("outside")
    }

    fn get(&self, platform: Platform, encoded_path: &str) -> Reply {
        self.send(platform, Limits::default(), &Spec::get(encoded_path))
    }

    fn send(&self, platform: Platform, limits: Limits, spec: &Spec<'_>) -> Reply {
        let uri = uri(platform, spec.path);
        serve(
            &self.vault,
            &self.budget,
            platform,
            limits,
            &Request {
                method: spec.method,
                uri: &uri,
                origin: spec.origin,
                range: spec.range,
                if_range: spec.if_range,
            },
        )
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

struct Spec<'a> {
    method: &'a str,
    path: &'a str,
    origin: Option<&'a str>,
    range: Option<&'a str>,
    if_range: Option<&'a str>,
}

impl<'a> Spec<'a> {
    fn get(path: &'a str) -> Self {
        Self {
            method: "GET",
            path,
            origin: None,
            range: None,
            if_range: None,
        }
    }
    fn range(path: &'a str, range: &'a str) -> Self {
        Self {
            range: Some(range),
            ..Self::get(path)
        }
    }
}

/// The form the handler is actually given on every platform: wry reverts
/// Windows' `http://serat-asset.localhost/...` to this before calling it
/// (`revert_uri_work_around`), so the platform does not change the URI.
fn uri(_platform: Platform, encoded_path: &str) -> String {
    format!("serat-asset://localhost/{encoded_path}")
}

fn body_text(reply: &Reply) -> String {
    String::from_utf8_lossy(&reply.body).into_owned()
}

fn assert_refused(reply: &Reply, what: &str) {
    assert_eq!(
        reply.status, 404,
        "{what}: expected 404, got {}",
        reply.status
    );
    assert!(reply.body.is_empty(), "{what}: a refusal carries no detail");
}

// -- the happy path and the headers every response carries -------------------

#[test]
fn serves_a_png_whole_on_both_platform_spellings_with_every_step_10_header() {
    let fx = Fx::new("happy");
    fx.put("img/a.png", b"PNGBYTES");
    for platform in BOTH {
        let reply = fx.get(platform, "img/a.png");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"PNGBYTES");
        assert_eq!(reply.header("Content-Type"), Some("image/png"));
        assert_eq!(reply.header("X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(reply.header("Cache-Control"), Some("no-store"));
        assert_eq!(
            reply.header("Content-Security-Policy"),
            Some("default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox")
        );
        assert_eq!(
            reply.header("Access-Control-Allow-Origin"),
            Some(platform.app_origin())
        );
        assert_eq!(
            reply.header("Access-Control-Expose-Headers"),
            Some("Content-Range, Accept-Ranges, Content-Length")
        );
        assert!(reply.header("ETag").is_some());
        assert_eq!(reply.header("Accept-Ranges"), Some("bytes"));
        assert_eq!(reply.header("Content-Length"), Some("8"));
    }
}

#[test]
fn the_sandbox_csp_never_grants_scripts_on_any_response() {
    // Control (b): no script runs in a scheme document. The CSP has `sandbox` and
    // no `allow-scripts`, on success and on every refusal.
    let fx = Fx::new("csp");
    fx.put(
        "a.svg",
        b"<svg onload=\"alert(1)\"><script>alert(1)</script></svg>",
    );
    fx.put("a.pdf", b"%PDF-1.7 /JS (app.alert(1))");
    for platform in BOTH {
        for path in ["a.svg", "a.pdf", "missing.png", "note.md"] {
            let csp = fx
                .get(platform, path)
                .header("Content-Security-Policy")
                .map(str::to_owned);
            let csp = csp.unwrap_or_else(|| panic!("{path}: no CSP header"));
            assert!(csp.contains("sandbox"), "{path}: {csp}");
            assert!(!csp.contains("allow-scripts"), "{path}: {csp}");
            assert!(csp.starts_with("default-src 'none'"), "{path}: {csp}");
        }
        // An SVG is delivered as an image type, never as text/html.
        assert_eq!(
            fx.get(platform, "a.svg").header("Content-Type"),
            Some("image/svg+xml")
        );
    }
}

#[test]
fn errors_carry_the_same_security_headers() {
    let fx = Fx::new("errheaders");
    for platform in BOTH {
        for reply in [
            fx.get(platform, "nope.png"),
            fx.send(
                platform,
                Limits::default(),
                &Spec {
                    method: "POST",
                    ..Spec::get("x.png")
                },
            ),
        ] {
            assert_eq!(reply.header("X-Content-Type-Options"), Some("nosniff"));
            assert_eq!(reply.header("Cache-Control"), Some("no-store"));
            assert!(reply.header("Content-Security-Policy").is_some());
        }
    }
}

// -- steps 1 and 8: methods, OPTIONS, CORS ------------------------------------

#[test]
fn only_get_head_and_options_are_answered() {
    let fx = Fx::new("methods");
    fx.put("a.png", b"x");
    for platform in BOTH {
        for method in ["POST", "PUT", "DELETE", "PATCH", "TRACE", "CONNECT"] {
            let reply = fx.send(
                platform,
                Limits::default(),
                &Spec {
                    method,
                    ..Spec::get("a.png")
                },
            );
            assert_eq!(reply.status, 405, "{method}");
            assert!(reply.body.is_empty());
        }
        assert_eq!(
            fx.send(
                platform,
                Limits::default(),
                &Spec {
                    method: "GET",
                    ..Spec::get("a.png")
                }
            )
            .status,
            200
        );
    }
}

#[test]
fn head_has_the_headers_of_get_and_no_body() {
    let fx = Fx::new("head");
    fx.put("a.png", b"12345");
    let get = fx.get(Platform::Other, "a.png");
    let head = fx.send(
        Platform::Other,
        Limits::default(),
        &Spec {
            method: "HEAD",
            ..Spec::get("a.png")
        },
    );
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(head.header("Content-Length"), get.header("Content-Length"));
    assert_eq!(head.header("ETag"), get.header("ETag"));
}

#[test]
fn options_from_the_app_origin_is_204_and_from_anyone_else_is_refused() {
    let fx = Fx::new("options");
    for platform in BOTH {
        let ok = fx.send(
            platform,
            Limits::default(),
            &Spec {
                method: "OPTIONS",
                origin: Some(platform.app_origin()),
                ..Spec::get("a.png")
            },
        );
        assert_eq!(ok.status, 204);
        assert_eq!(ok.header("Access-Control-Allow-Methods"), Some("GET, HEAD"));
        assert_eq!(
            ok.header("Access-Control-Allow-Headers"),
            Some("Range, If-Range")
        );
        assert_eq!(
            ok.header("Access-Control-Allow-Origin"),
            Some(platform.app_origin())
        );

        for origin in [
            Some("https://evil.example"),
            Some("null"),
            Some("*"),
            None,
            // The other platform's app origin is a foreign origin here.
            Some(if platform == Platform::Windows {
                "tauri://localhost"
            } else {
                "http://tauri.localhost"
            }),
        ] {
            let refused = fx.send(
                platform,
                Limits::default(),
                &Spec {
                    method: "OPTIONS",
                    origin,
                    ..Spec::get("a.png")
                },
            );
            assert_eq!(refused.status, 403, "{origin:?}");
            assert_ne!(refused.header("Access-Control-Allow-Origin"), Some("*"));
        }
    }
}

#[test]
fn a_foreign_origin_never_gets_a_wildcard_or_its_own_origin_echoed() {
    let fx = Fx::new("cors");
    fx.put("a.png", b"x");
    for platform in BOTH {
        let reply = fx.send(
            platform,
            Limits::default(),
            &Spec {
                origin: Some("https://evil.example"),
                ..Spec::get("a.png")
            },
        );
        assert_eq!(
            reply.header("Access-Control-Allow-Origin"),
            Some(platform.app_origin()),
            "the browser, not this handler, refuses the foreign page: ACAO stays the app origin"
        );
    }
}

// -- steps 2 and 3: host, scheme, single decoding ---------------------------

#[test]
fn any_host_but_the_platforms_own_is_a_404() {
    let fx = Fx::new("host");
    fx.put("a.png", b"x");
    let cases = [
        (Platform::Other, "serat-asset://evil.example/a.png"),
        (Platform::Other, "serat-asset://localhost:8080/a.png"),
        (Platform::Other, "serat-asset://user@localhost/a.png"),
        (Platform::Other, "serat-asset://localhost.evil/a.png"),
        (Platform::Other, "http://serat-asset.localhost/a.png"), // the Windows spelling elsewhere
        (Platform::Windows, "http://serat-asset.localhost/a.png"), // wry never delivers this form
        (Platform::Windows, "http://serat-asset.localhost:8080/a.png"),
        (Platform::Windows, "http://evil.localhost/a.png"),
        (Platform::Windows, "http://serat-asset.localhost.evil/a.png"),
        (Platform::Windows, "https://serat-asset.localhost/a.png"),
    ];
    for (platform, uri) in cases {
        let reply = serve(
            &fx.vault,
            &fx.budget,
            platform,
            Limits::default(),
            &Request {
                method: "GET",
                uri,
                origin: None,
                range: None,
                if_range: None,
            },
        );
        assert_refused(&reply, uri);
    }
    // Control: the same file under the right host.
    for platform in BOTH {
        assert_eq!(fx.get(platform, "a.png").status, 200);
    }
}

#[test]
fn encoded_traversal_is_refused_and_the_outside_file_is_never_served() {
    let fx = Fx::new("traversal");
    fx.put("sub/a.png", b"INSIDE");
    fx.put("secret.png", b"ROOT-LEVEL-NOT-OUTSIDE");
    for platform in BOTH {
        for path in [
            "sub/%2e%2e/%2e%2e/outside/secret.png",
            "%2e%2e/outside/secret.png",
            "sub/%2E%2E/%2E%2E/outside/secret.png",
            "..%2foutside%2fsecret.png",
            "sub/..%2f..%2foutside/secret.png",
            "../outside/secret.png",
            "sub/../../outside/secret.png",
            "..%5coutside%5csecret.png",
            "sub%5c..%5c..%5coutside%5csecret.png",
        ] {
            let reply = fx.get(platform, path);
            assert_refused(&reply, path);
        }
        // `..` is refused even where it would land back inside the vault: no
        // lexical cleverness, the segment itself is the error (VaultPath).
        assert_refused(
            &fx.get(platform, "sub/%2e%2e/secret.png"),
            "dot-dot that stays inside",
        );
        assert_eq!(fx.get(platform, "sub/a.png").status, 200, "control");
    }
}

#[test]
fn an_encoded_slash_is_one_decode_and_stays_contained() {
    let fx = Fx::new("slash");
    fx.put("dir/a.png", b"DIRA");
    for platform in BOTH {
        let reply = fx.get(platform, "dir%2fa.png");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"DIRA");
        assert_refused(
            &fx.get(platform, "%2foutside%2fsecret.png"),
            "leading encoded slash",
        );
    }
}

#[test]
fn decoding_happens_exactly_once() {
    let fx = Fx::new("once");
    fx.put("a%41.png", b"LITERAL-PERCENT-41");
    fx.put("aA.png", b"PLAIN-A");
    fx.put("%2e.png", b"LITERAL-2E");
    for platform in BOTH {
        // The front end asks for the literal name `a%41.png` as `a%2541.png`.
        assert_eq!(fx.get(platform, "a%2541.png").body, b"LITERAL-PERCENT-41");
        // And `a%41.png` on the wire is `aA.png` -- decoded once, not zero, not twice.
        assert_eq!(fx.get(platform, "a%41.png").body, b"PLAIN-A");
        // `%252e` is a file literally named `%2e`, never `.`.
        assert_eq!(fx.get(platform, "%252e.png").body, b"LITERAL-2E");
        // Double-encoded traversal names a directory called `%2e%2e`, which is not `..`.
        assert_refused(
            &fx.get(platform, "%252e%252e/outside/secret.png"),
            "%252e%252e",
        );
    }
}

#[test]
fn nul_and_invalid_utf8_are_404() {
    let fx = Fx::new("nul");
    fx.put("a.png", b"x");
    for platform in BOTH {
        for path in [
            "a.png%00",
            "a%00.png",
            "%00",
            "a%ff.png",
            "%c0%af.png",
            "%80.png",
        ] {
            assert_refused(&fx.get(platform, path), path);
        }
        assert_eq!(fx.get(platform, "a.png").status, 200);
    }
}

#[test]
fn a_query_and_a_fragment_are_not_part_of_the_path() {
    let fx = Fx::new("query");
    fx.put("a.png", b"x");
    for platform in BOTH {
        assert_eq!(fx.get(platform, "a.png?v=1").status, 200);
        assert_eq!(fx.get(platform, "a.png#frag").status, 200);
        // An encoded '?' is part of the name.
        assert_refused(&fx.get(platform, "a.png%3Fv=1"), "encoded question mark");
    }
}

#[test]
fn decode_once_unit_cases() {
    assert_eq!(decode_once("/a%20b").as_deref(), Some("/a b"));
    assert_eq!(decode_once("/100%").as_deref(), Some("/100%"));
    assert_eq!(decode_once("/%zz").as_deref(), Some("/%zz"));
    assert_eq!(decode_once("/%2").as_deref(), Some("/%2"));
    assert_eq!(decode_once("/%252e").as_deref(), Some("/%2e"));
    assert_eq!(decode_once("/%e2%82%ac").as_deref(), Some("/\u{20ac}"));
    assert_eq!(decode_once("/a%00"), None);
    assert_eq!(decode_once("/%ff"), None);
}

// -- step 4: name rules, split by platform -----------------------------------

#[test]
fn a_leading_drive_is_refused_on_every_platform() {
    let fx = Fx::new("drive");
    fx.put("Users/x.png", b"x");
    for platform in BOTH {
        for path in [
            "C:/Users/x.png",
            "C%3A/Users/x.png",
            "c:/Users/x.png",
            "C%3a%5cUsers%5cx.png",
        ] {
            assert_refused(&fx.get(platform, path), path);
        }
    }
}

/// Linux and macOS: `C:Users.png`, `CON.pdf` and `aux.png` are ordinary files a
/// user can have. Refusing them would be a defect, and what is served must be
/// that file's own bytes and nothing outside the root.
#[cfg(not(windows))]
#[test]
fn windows_reserved_looking_names_are_served_as_ordinary_files_elsewhere() {
    let fx = Fx::new("winnames-elsewhere");
    fx.put("C:Users.png", b"COLON-FILE");
    fx.put("CON.pdf", b"CON-FILE");
    fx.put("aux.png", b"AUX-FILE");
    fx.put("a.png:hidden", b"STREAMISH"); // not viewable (extension "png:hidden")
    for platform in BOTH {
        for (path, expected) in [
            ("C%3AUsers.png", &b"COLON-FILE"[..]),
            ("C:Users.png", b"COLON-FILE"),
            ("CON.pdf", b"CON-FILE"),
            ("aux.png", b"AUX-FILE"),
        ] {
            let reply = fx.get(platform, path);
            assert_eq!(reply.status, 200, "{path} is a legitimate file here");
            assert_eq!(reply.body, expected, "{path}: served bytes are its own");
            assert!(!body_text(&reply).contains("OUTSIDE"));
        }
        assert_refused(&fx.get(platform, "a.png%3Ahidden"), "no viewable extension");
    }
}

/// `photo.png.` passes step 4 off Windows and is judged by step 5: no viewable
/// extension, so 404, even though the file exists.
#[cfg(not(windows))]
#[test]
fn a_trailing_dot_name_is_refused_by_step_5_not_by_step_4_elsewhere() {
    let fx = Fx::new("trailing-dot");
    fx.put("photo.png.", b"DOTTED");
    fx.put("photo.png", b"REAL");
    for platform in BOTH {
        assert_refused(&fx.get(platform, "photo.png."), "photo.png.");
        assert_eq!(fx.get(platform, "photo.png").body, b"REAL");
    }
}

/// The Windows half of the split: #34's Windows-only `VaultPath` rules (`:` in a
/// segment, device names, trailing dot/space), reached through the scheme.
#[cfg(windows)]
#[test]
fn windows_name_rules_are_enforced_on_windows() {
    let fx = Fx::new("winnames-windows");
    for path in [
        "C:Users.png",
        "C%3AUsers.png",
        "CON.pdf",
        "con%20.pdf",
        "a.png%3Ahidden",
        "photo.png.",
        "aux.png",
        "NUL",
        "COM1.pdf",
        "LPT9.png",
    ] {
        assert_refused(&fx.get(Platform::Windows, path), path);
    }
}

// -- step 5: viewable assets only, ignored directories ----------------------

#[test]
fn only_pdf_and_images_are_served() {
    let fx = Fx::new("viewable");
    for name in [
        "note.md",
        "note.markdown",
        "a.txt",
        "a.html",
        "a.json",
        "a.exe",
        "noext",
        ".hidden",
        "a.zip",
    ] {
        fx.put(name, b"x");
        for platform in BOTH {
            assert_refused(&fx.get(platform, name), name);
        }
    }
    for name in [
        "a.pdf", "a.png", "a.jpg", "a.jpeg", "a.gif", "a.webp", "a.avif", "a.bmp", "a.svg",
        "UP.PNG",
    ] {
        fx.put(name, b"x");
        assert_eq!(fx.get(Platform::Other, name).status, 200, "{name}");
    }
}

// Task #127 (ADR-0017 item 6): pdf.js's cMaps and standard fonts ship in the app bundle and are
// fetched as `./cmaps/<name>.bcmap` and `./standard_fonts/<name>.pfb` from the app's own origin.
// The vault scheme is not their path: a vault file with such a name is not viewable, so the
// scheme, the info command and the span fallback all refuse it, with either platform spelling
// and through an encoded traversal, and the bundle's bytes can never be had by naming them.
#[test]
fn bundle_cmaps_and_fonts_are_never_served_from_the_vault() {
    let fx = Fx::new("bundle-names");
    fx.put("cmaps/Adobe-Japan1-UCS2.bcmap", b"VAULT-CMAP");
    fx.put("standard_fonts/FoxitSans.pfb", b"VAULT-FONT");
    fx.put("a.pdf", b"%PDF");
    for name in [
        "cmaps/Adobe-Japan1-UCS2.bcmap",
        "standard_fonts/FoxitSans.pfb",
        "./cmaps/Adobe-Japan1-UCS2.bcmap",
        "./standard_fonts/FoxitSans.pfb",
        "..%2Fcmaps%2FAdobe-Japan1-UCS2.bcmap",
        "x/..%2F..%2Fstandard_fonts%2FFoxitSans.pfb",
    ] {
        for platform in BOTH {
            let reply = fx.get(platform, name);
            assert_refused(&reply, name);
            assert!(
                !body_text(&reply).contains("VAULT-"),
                "{name}: vault bytes reached the response"
            );
        }
    }
    for path in [
        "cmaps/Adobe-Japan1-UCS2.bcmap",
        "standard_fonts/FoxitSans.pfb",
    ] {
        let described = super::info(&fx.vault, &vp(path)).expect("info describes any vault file");
        assert!(!described.viewable, "{path} must not be viewable");
        assert!(
            read_range(&fx.vault, &fx.budget, &vp(path), 0, 5, None).is_err(),
            "{path}"
        );
    }
    // A control: the scheme is not simply refusing everything in these directories' neighbourhood.
    assert_eq!(fx.get(Platform::Other, "a.pdf").status, 200);
}

#[test]
fn ignored_directories_are_refused_whatever_their_case() {
    let fx = Fx::new("ignored");
    for dir in [
        ".serat",
        ".obsidian",
        ".git",
        ".trash",
        "node_modules",
        "Node_Modules",
        "NODE_MODULES",
        ".Serat",
        ".OBSIDIAN",
    ] {
        fx.put(&format!("{dir}/x.png"), b"HIDDEN");
        for platform in BOTH {
            assert_refused(&fx.get(platform, &format!("{dir}/x.png")), dir);
            assert_refused(&fx.get(platform, &format!("sub/{dir}/x.png")), dir);
        }
    }
    fx.put("notes_modules/x.png", b"FINE");
    assert_eq!(
        fx.get(Platform::Other, "notes_modules/x.png").status,
        200,
        "control"
    );
}

// -- steps 6 and 7: symlinks, handles, FIFOs ------------------------------------

#[cfg(unix)]
mod unix_only {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_symlinked_file_is_refused_inside_and_out() {
        let fx = Fx::new("symfile");
        fx.put("real.png", b"REAL");
        symlink(
            fx.base.join("vault/real.png"),
            fx.base.join("vault/inside.png"),
        )
        .unwrap();
        symlink(
            fx.outside().join("secret.png"),
            fx.base.join("vault/escape.png"),
        )
        .unwrap();
        for platform in BOTH {
            assert_eq!(fx.get(platform, "real.png").status, 200, "control");
            assert_refused(&fx.get(platform, "inside.png"), "link to a file inside");
            assert_refused(&fx.get(platform, "escape.png"), "link to a file outside");
        }
    }

    #[test]
    fn a_symlinked_directory_component_is_refused() {
        let fx = Fx::new("symdir");
        fx.put("real/a.png", b"REAL");
        symlink(fx.base.join("vault/real"), fx.base.join("vault/alias")).unwrap();
        symlink(fx.outside(), fx.base.join("vault/escape")).unwrap();
        for platform in BOTH {
            assert_eq!(fx.get(platform, "real/a.png").status, 200, "control");
            assert_refused(&fx.get(platform, "alias/a.png"), "dir link inside");
            assert_refused(&fx.get(platform, "escape/secret.png"), "dir link outside");
        }
    }

    #[test]
    fn a_link_pointing_into_dot_serat_is_refused() {
        let fx = Fx::new("serat-link");
        fx.put(".serat/x.png", b"INDEX-SIDE");
        symlink(
            fx.base.join("vault/.serat/x.png"),
            fx.base.join("vault/via.png"),
        )
        .unwrap();
        symlink(fx.base.join("vault/.serat"), fx.base.join("vault/viadir")).unwrap();
        for platform in BOTH {
            assert_refused(&fx.get(platform, "via.png"), "file link into .serat");
            assert_refused(&fx.get(platform, "viadir/x.png"), "dir link into .serat");
            assert_refused(&fx.get(platform, ".serat/x.png"), "direct");
        }
    }

    /// Step 7 on its own: handed a path that step 6 never saw (as if a component
    /// were swapped for a symlink after resolution), the open-handle check alone
    /// must refuse. `open_verified` is called directly, bypassing `Vault::resolve`.
    #[test]
    fn the_handle_check_alone_catches_a_component_swapped_after_resolve() {
        let fx = Fx::new("toctou");
        fx.put("dir/a.png", b"INSIDE");
        let root = fx.vault.root().to_path_buf();
        let honest = root.join("dir/a.png");
        assert!(
            open_verified(&honest, &fx.vault).is_some(),
            "control: an honest path opens"
        );

        // Swap `dir` for a symlink to the outside directory.
        fs::rename(root.join("dir"), root.join("dir.moved")).unwrap();
        fs::write(fx.outside().join("a.png"), b"OUTSIDE").unwrap();
        symlink(fx.outside(), root.join("dir")).unwrap();
        assert!(
            open_verified(&honest, &fx.vault).is_none(),
            "the handle now lives outside the root; step 7 must refuse it"
        );
        // And the full pipeline refuses too (step 6 sees the link).
        assert_refused(&fx.get(Platform::Other, "dir/a.png"), "swapped component");
    }

    /// Step 7 re-runs the ignored-directory rule on the handle's real path: a
    /// directory swapped for a link into `.serat/` after step 5 is refused.
    #[test]
    fn a_directory_swapped_for_a_link_into_serat_is_refused_by_step_7() {
        let fx = Fx::new("toctou-serat");
        fx.put("dir/x.png", b"INSIDE");
        fx.put(".serat/x.png", b"INDEX");
        let root = fx.vault.root().to_path_buf();
        let honest = root.join("dir/x.png");
        assert!(open_verified(&honest, &fx.vault).is_some(), "control");
        fs::rename(root.join("dir"), root.join("dir.moved")).unwrap();
        symlink(root.join(".serat"), root.join("dir")).unwrap();
        assert!(
            open_verified(&honest, &fx.vault).is_none(),
            "the handle lives in .serat/, an ignored directory"
        );
    }

    #[test]
    fn a_final_component_swapped_for_a_symlink_is_refused_by_the_open() {
        let fx = Fx::new("toctou-final");
        let root = fx.vault.root().to_path_buf();
        symlink(fx.outside().join("secret.png"), root.join("a.png")).unwrap();
        assert!(open_verified(&root.join("a.png"), &fx.vault).is_none());
    }

    #[test]
    fn a_handle_outside_the_root_is_refused_even_by_an_honest_path() {
        let fx = Fx::new("outside-handle");
        assert!(open_verified(&fx.outside().join("secret.png"), &fx.vault).is_none());
    }

    #[test]
    fn a_fifo_is_refused_and_never_blocks_the_handler() {
        let fx = Fx::new("fifo");
        fx.put("real.png", b"x");
        let fifo = fx.base.join("vault/pipe.png");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(
            made.is_ok_and(|s| s.success()),
            "mkfifo must be available to run this test"
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let base = fx.base.clone();
        std::thread::spawn(move || {
            let vault = Vault::open(base.join("vault")).unwrap();
            let budget = ByteBudget::new(IN_FLIGHT_BYTES);
            let reply = serve(
                &vault,
                &budget,
                Platform::Other,
                Limits::default(),
                &Request {
                    method: "GET",
                    uri: "serat-asset://localhost/pipe.png",
                    origin: None,
                    range: None,
                    if_range: None,
                },
            );
            let _ = tx.send(reply.status);
        });
        let status = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("a FIFO must be refused promptly, not read");
        assert_eq!(status, 404);
        assert_eq!(fx.get(Platform::Other, "real.png").status, 200, "control");
    }
}

#[test]
fn a_directory_named_like_an_image_is_refused() {
    let fx = Fx::new("dirfile");
    fs::create_dir_all(fx.base.join("vault/d.png")).unwrap();
    assert_refused(&fx.get(Platform::Other, "d.png"), "directory");
}

// -- step 9: ranges, caps, If-Range ---------------------------------------------

const ALPHABET: &[u8] = b"0123456789abcdefghijklmno"; // 25 bytes

fn pdf_fixture(name: &str) -> Fx {
    let fx = Fx::new(name);
    fx.put("a.pdf", ALPHABET);
    fx
}

fn ranged(fx: &Fx, range: &str) -> Reply {
    fx.send(Platform::Other, SMALL, &Spec::range("a.pdf", range))
}

#[test]
fn range_boundaries_are_exact() {
    let fx = pdf_fixture("bounds");
    let first = ranged(&fx, "bytes=0-0");
    assert_eq!((first.status, first.body.as_slice()), (206, &b"0"[..]));
    assert_eq!(first.header("Content-Range"), Some("bytes 0-0/25"));

    let last = ranged(&fx, "bytes=24-24");
    assert_eq!((last.status, last.body.as_slice()), (206, &b"o"[..]));
    assert_eq!(last.header("Content-Range"), Some("bytes 24-24/25"));

    let mid = ranged(&fx, "bytes=3-6");
    assert_eq!(mid.body, b"3456");
    assert_eq!(mid.header("Content-Length"), Some("4"));

    let open_ended = ranged(&fx, "bytes=20-");
    assert_eq!(open_ended.body, b"klmno");
    assert_eq!(open_ended.header("Content-Range"), Some("bytes 20-24/25"));

    let past_end = ranged(&fx, "bytes=20-1000");
    assert_eq!(past_end.body, b"klmno");
    assert_eq!(past_end.header("Content-Range"), Some("bytes 20-24/25"));
}

#[test]
fn a_range_wider_than_the_cap_is_truncated_and_says_so() {
    let fx = pdf_fixture("overcap");
    let reply = ranged(&fx, "bytes=0-24");
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body, b"0123456789");
    assert_eq!(reply.header("Content-Range"), Some("bytes 0-9/25"));
    let open = ranged(&fx, "bytes=5-");
    assert_eq!(open.body, b"56789abcde");
    assert_eq!(open.header("Content-Range"), Some("bytes 5-14/25"));
}

#[test]
fn unsatisfiable_inverted_multi_and_malformed_ranges_are_416() {
    let fx = pdf_fixture("416");
    for range in [
        "bytes=25-",
        "bytes=25-30",
        "bytes=1000-",
        "bytes=5-2",
        "bytes=0-1,3-4",
        "bytes=abc",
        "bytes=-",
        "bytes=",
        "bytes=-0",
        "bytes=1-2-3",
        "bytes=99999999999999999999-",
        "bytes=0-99999999999999999999",
    ] {
        let reply = ranged(&fx, range);
        assert_eq!(reply.status, 416, "{range}");
        assert!(reply.body.is_empty(), "{range}");
        assert_eq!(reply.header("Content-Range"), Some("bytes */25"), "{range}");
    }
}

#[test]
fn a_range_in_another_unit_is_ignored() {
    let fx = pdf_fixture("unit");
    let reply = ranged(&fx, "items=0-1");
    // Ignored, so the no-Range rule applies: a 25-byte PDF over a 10-byte cap.
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body, b"0123456789");
}

#[test]
fn a_suffix_range_is_served_within_the_cap() {
    let fx = pdf_fixture("suffix");
    let tail = ranged(&fx, "bytes=-5");
    assert_eq!((tail.status, tail.body.as_slice()), (206, &b"klmno"[..]));
    assert_eq!(tail.header("Content-Range"), Some("bytes 20-24/25"));
    // Longer than the file: the whole file from 0, still capped.
    let all = ranged(&fx, "bytes=-100");
    assert_eq!(all.body, b"0123456789");
    assert_eq!(all.header("Content-Range"), Some("bytes 0-9/25"));
}

#[test]
fn a_range_on_an_empty_file_is_416() {
    let fx = Fx::new("emptyfile");
    fx.put("a.pdf", b"");
    assert_eq!(
        fx.send(Platform::Other, SMALL, &Spec::range("a.pdf", "bytes=0-0"))
            .status,
        416
    );
    assert_eq!(
        fx.send(Platform::Other, SMALL, &Spec::range("a.pdf", "bytes=-1"))
            .status,
        416
    );
    let whole = fx.send(Platform::Other, SMALL, &Spec::get("a.pdf"));
    assert_eq!((whole.status, whole.body.len()), (200, 0));
}

#[test]
fn a_pdf_with_no_range_over_the_cap_gets_206_and_the_first_span() {
    let fx = pdf_fixture("pdf-norange");
    let reply = fx.send(Platform::Other, SMALL, &Spec::get("a.pdf"));
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body, b"0123456789");
    assert_eq!(reply.header("Content-Range"), Some("bytes 0-9/25"));
    assert_eq!(reply.header("Accept-Ranges"), Some("bytes"));
    // At or under the cap it is simply a 200.
    fx.put("small.pdf", b"0123456789");
    let at_cap = fx.send(Platform::Other, SMALL, &Spec::get("small.pdf"));
    assert_eq!((at_cap.status, at_cap.body.len()), (200, 10));
    fx.put("one-over.pdf", b"0123456789A");
    let over = fx.send(Platform::Other, SMALL, &Spec::get("one-over.pdf"));
    assert_eq!((over.status, over.body.len()), (206, 10));
}

#[test]
fn a_pdf_over_the_real_cap_is_never_loaded_whole() {
    let fx = Fx::new("pdf-real-cap");
    let size = usize::try_from(SPAN_CAP).unwrap() + 1;
    fx.put("big.pdf", &vec![b'p'; size]);
    let reply = fx.get(Platform::Other, "big.pdf");
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body.len() as u64, SPAN_CAP);
    assert_eq!(
        reply.header("Content-Range"),
        Some(format!("bytes 0-{}/{size}", SPAN_CAP - 1).as_str())
    );
}

#[test]
fn an_image_over_the_span_cap_arrives_whole_with_no_range() {
    // The first draft truncated every photo over 1,000 KiB; review caught it.
    let fx = Fx::new("image-whole");
    fx.put("photo.jpg", &[7u8; 50]); // over the 10-byte span cap, under the 100 image cap
    let reply = fx.send(Platform::Other, SMALL, &Spec::get("photo.jpg"));
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body.len(), 50);
    assert!(
        reply.header("Content-Range").is_none(),
        "a whole image is not a span"
    );
}

#[test]
fn an_image_over_the_image_cap_is_404_not_a_truncated_image() {
    let fx = Fx::new("image-too-big");
    fx.put("photo.jpg", &[7u8; 101]);
    assert_refused(
        &fx.send(Platform::Other, SMALL, &Spec::get("photo.jpg")),
        "101 > 100",
    );
    fx.put("edge.jpg", &[7u8; 100]);
    assert_eq!(
        fx.send(Platform::Other, SMALL, &Spec::get("edge.jpg"))
            .status,
        200,
        "exactly the cap"
    );
}

#[test]
fn the_real_image_cap_is_64_mib() {
    assert_eq!(IMAGE_CAP, 64 * 1024 * 1024);
    assert_eq!(SPAN_CAP, 1000 * 1024);
    assert_eq!(IN_FLIGHT_BYTES, 128 * 1024 * 1024);
    let fx = Fx::new("image-real-cap");
    // Sparse: costs no disk. One byte over the cap must be refused before any read.
    let path = fx.put("huge.png", b"");
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(IMAGE_CAP + 1)
        .unwrap();
    assert_refused(&fx.get(Platform::Other, "huge.png"), "64 MiB + 1");
}

#[test]
fn a_range_request_on_an_image_over_the_cap_still_works_in_spans() {
    let fx = Fx::new("image-range");
    fx.put("photo.jpg", &[b'x'; 101]);
    let reply = fx.send(
        Platform::Other,
        SMALL,
        &Spec::range("photo.jpg", "bytes=0-4"),
    );
    assert_eq!((reply.status, reply.body.len()), (206, 5));
}

#[test]
fn if_range_restarts_when_the_file_changed_and_continues_when_it_did_not() {
    let fx = pdf_fixture("ifrange");
    let first = fx.send(Platform::Other, SMALL, &Spec::get("a.pdf"));
    let etag = first.header("ETag").unwrap().to_owned();

    let same = fx.send(
        Platform::Other,
        SMALL,
        &Spec {
            if_range: Some(&etag),
            ..Spec::range("a.pdf", "bytes=10-14")
        },
    );
    assert_eq!((same.status, same.body.as_slice()), (206, &b"abcde"[..]));

    // The file changes between two range requests.
    fx.put("a.pdf", b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
    let changed = fx.send(
        Platform::Other,
        SMALL,
        &Spec {
            if_range: Some(&etag),
            ..Spec::range("a.pdf", "bytes=10-14")
        },
    );
    assert_eq!(changed.status, 200, "restart, not a mixed-version 206");
    assert_eq!(
        changed.body, b"ABCDEFGHIJ",
        "the first span of the NEW file"
    );
    assert_ne!(changed.header("ETag"), Some(etag.as_str()));
    assert_eq!(changed.header("Content-Range"), Some("bytes 0-9/36"));

    // Without If-Range the same Range is honoured against the new file.
    let no_validator = fx.send(Platform::Other, SMALL, &Spec::range("a.pdf", "bytes=10-14"));
    assert_eq!(
        (no_validator.status, no_validator.body.as_slice()),
        (206, &b"KLMNO"[..])
    );
}

#[test]
fn a_weak_or_garbage_if_range_validator_is_treated_as_changed() {
    let fx = pdf_fixture("ifrange-weak");
    let etag = fx
        .send(Platform::Other, SMALL, &Spec::get("a.pdf"))
        .header("ETag")
        .unwrap()
        .to_owned();
    for validator in [
        format!("W/{etag}"),
        "garbage".to_owned(),
        "Wed, 21 Oct 2015 07:28:00 GMT".to_owned(),
    ] {
        let reply = fx.send(
            Platform::Other,
            SMALL,
            &Spec {
                if_range: Some(&validator),
                ..Spec::range("a.pdf", "bytes=10-14")
            },
        );
        assert_eq!(reply.status, 200, "{validator}");
    }
}

#[test]
fn parse_range_unit_cases() {
    assert_eq!(parse_range(None), RangeSpec::Absent);
    assert_eq!(parse_range(Some("bytes=0-4")), RangeSpec::From(0, Some(4)));
    assert_eq!(parse_range(Some("bytes=7-")), RangeSpec::From(7, None));
    assert_eq!(parse_range(Some("bytes=-3")), RangeSpec::Suffix(3));
    assert_eq!(parse_range(Some("bytes=4-3")), RangeSpec::Bad);
    assert_eq!(parse_range(Some("bytes=1-2,4-5")), RangeSpec::Bad);
    assert_eq!(parse_range(Some("bytes=+1-2")), RangeSpec::Bad);
    assert_eq!(parse_range(Some("bytes=0x1-2")), RangeSpec::Bad);
    assert_eq!(parse_range(Some("lines=1-2")), RangeSpec::Absent);
}

// -- the in-flight budget ----------------------------------------------------

#[test]
fn the_byte_budget_bounds_what_is_held_at_once() {
    let budget = ByteBudget::new(100);
    let a = budget.try_acquire(60).expect("fits");
    assert!(budget.try_acquire(50).is_none(), "60 + 50 > 100 must wait");
    let b = budget.try_acquire(40).expect("60 + 40 == 100 fits");
    assert!(budget.try_acquire(1).is_none());
    drop(a);
    assert!(
        budget.try_acquire(60).is_some(),
        "released bytes are reusable"
    );
    drop(b);
}

#[test]
fn a_request_larger_than_the_whole_budget_is_clamped_not_deadlocked() {
    let budget = ByteBudget::new(100);
    let permit = budget.acquire(10_000);
    assert!(budget.try_acquire(1).is_none(), "it took everything");
    drop(permit);
    assert!(budget.try_acquire(100).is_some());
}

#[test]
fn a_blocked_acquire_resumes_when_another_request_finishes() {
    let budget = std::sync::Arc::new(ByteBudget::new(100));
    let held = budget.try_acquire(80).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let waiter = {
        let budget = std::sync::Arc::clone(&budget);
        std::thread::spawn(move || {
            let _permit = budget.acquire(50);
            tx.send(()).unwrap();
        })
    };
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "must still be waiting"
    );
    drop(held);
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("resumed after release");
    waiter.join().unwrap();
}

#[test]
fn serving_returns_its_bytes_to_the_budget() {
    let fx = Fx::new("budget-release");
    fx.put("a.png", &[1u8; 1000]);
    for platform in BOTH {
        assert_eq!(fx.get(platform, "a.png").status, 200);
    }
    assert!(
        fx.budget.try_acquire(IN_FLIGHT_BYTES).is_some(),
        "nothing leaked"
    );
}

// -- asset_info and read_asset_range (the fallback) --------------------------

fn vp(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

#[test]
fn asset_info_reports_mime_size_viewability_and_the_byte_exact_sidecar() {
    let fx = Fx::new("info");
    fx.put("docs/a.pdf", ALPHABET);
    fx.put("docs/a.pdf.md", b"sidecar");
    fx.put("docs/X.PDF", b"12");
    fx.put("docs/x.pdf.md", b"not X.PDF's sidecar");
    fx.put("docs/z.zip", b"zip");

    let info = super::info(&fx.vault, &vp("docs/a.pdf")).unwrap();
    assert_eq!(info.mime, "application/pdf");
    assert_eq!(info.bytes, 25);
    assert!(info.viewable);
    assert_eq!(info.sidecar_path.as_deref(), Some("docs/a.pdf.md"));

    // ADR-0017: X.PDF and x.pdf.md are not a pair.
    assert_eq!(
        super::info(&fx.vault, &vp("docs/X.PDF"))
            .unwrap()
            .sidecar_path,
        None
    );

    let zip = super::info(&fx.vault, &vp("docs/z.zip")).unwrap();
    assert!(!zip.viewable);
    assert_eq!(zip.mime, "application/zip");
}

#[test]
fn asset_info_refuses_missing_ignored_and_escaping_paths_alike() {
    let fx = Fx::new("info-refuse");
    fx.put(".serat/x.png", b"x");
    assert!(super::info(&fx.vault, &vp("nope.png")).is_err());
    assert!(super::info(&fx.vault, &vp(".serat/x.png")).is_err());
    assert!(VaultPath::new("../outside/secret.png").is_err());
}

#[cfg(unix)]
#[test]
fn asset_info_and_read_range_refuse_symlinks() {
    let fx = Fx::new("info-symlink");
    std::os::unix::fs::symlink(fx.outside().join("secret.png"), fx.base.join("vault/l.png"))
        .unwrap();
    assert!(super::info(&fx.vault, &vp("l.png")).is_err());
    assert!(read_range(&fx.vault, &fx.budget, &vp("l.png"), 0, 5, None).is_err());
}

#[test]
fn read_asset_range_returns_the_span_and_applies_the_same_cap() {
    let fx = Fx::new("fallback");
    fx.put("a.pdf", ALPHABET);
    assert_eq!(
        read_range(&fx.vault, &fx.budget, &vp("a.pdf"), 3, 4, None).unwrap(),
        b"3456"
    );
    assert_eq!(
        read_range(&fx.vault, &fx.budget, &vp("a.pdf"), 20, 100, None).unwrap(),
        b"klmno"
    );
    assert!(
        read_range(&fx.vault, &fx.budget, &vp("a.pdf"), 25, 4, None)
            .unwrap()
            .is_empty()
    );
    assert!(
        read_range(&fx.vault, &fx.budget, &vp("a.pdf"), 1000, 4, None)
            .unwrap()
            .is_empty()
    );

    let size = usize::try_from(SPAN_CAP).unwrap() * 2;
    fx.put("big.pdf", &vec![b'q'; size]);
    let capped = read_range(&fx.vault, &fx.budget, &vp("big.pdf"), 0, u32::MAX, None).unwrap();
    assert_eq!(
        capped.len() as u64,
        SPAN_CAP,
        "len is clamped to the span cap"
    );
}

#[test]
fn read_asset_range_serves_only_what_the_scheme_would() {
    let fx = Fx::new("fallback-refuse");
    fx.put("note.md", b"# secret note");
    fx.put(".serat/x.png", b"index-side");
    fx.put("a.exe", b"MZ");
    for path in ["note.md", ".serat/x.png", "a.exe", "missing.pdf"] {
        assert!(
            read_range(&fx.vault, &fx.budget, &vp(path), 0, 5, None).is_err(),
            "{path}"
        );
    }
}

fn changed_validator(result: Result<Vec<u8>, IpcError>) -> String {
    match result {
        Err(IpcError::AssetChanged { validator, .. }) => validator,
        other => panic!("expected AssetChanged, got {other:?}"),
    }
}

fn set_mtime(path: &std::path::Path, seconds_from_now: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(
            std::time::SystemTime::now() + std::time::Duration::from_secs(seconds_from_now),
        )
        .unwrap();
}

#[test]
fn an_unchanged_file_keeps_its_etag_and_span_across_info_and_read_range() {
    let fx = Fx::new("validator-stable");
    fx.put("a.pdf", ALPHABET);
    let etag = super::info(&fx.vault, &vp("a.pdf")).unwrap().etag.unwrap();
    assert_eq!(
        super::info(&fx.vault, &vp("a.pdf")).unwrap().etag.unwrap(),
        etag
    );
    let span = read_range(&fx.vault, &fx.budget, &vp("a.pdf"), 3, 4, Some(&etag)).unwrap();
    assert_eq!(span, b"3456");
    // The format is the scheme's: quoted, size then mtime ns, both hex.
    assert!(
        etag.starts_with('"') && etag.ends_with('"') && etag.contains('-'),
        "{etag}"
    );
}

#[test]
fn a_same_size_rewrite_with_a_new_mtime_is_asset_changed_and_info_agrees() {
    let fx = Fx::new("validator-rewrite");
    let file = fx.put("a.pdf", ALPHABET);
    let old = super::info(&fx.vault, &vp("a.pdf")).unwrap().etag.unwrap();
    let mut rewritten = ALPHABET.to_vec();
    rewritten[0] = b'Z';
    std::fs::write(&file, &rewritten).unwrap();
    set_mtime(&file, 100);
    let new = changed_validator(read_range(
        &fx.vault,
        &fx.budget,
        &vp("a.pdf"),
        0,
        4,
        Some(&old),
    ));
    assert_ne!(new, old);
    assert_eq!(
        super::info(&fx.vault, &vp("a.pdf")).unwrap().etag.unwrap(),
        new
    );
    assert_eq!(
        read_range(&fx.vault, &fx.budget, &vp("a.pdf"), 0, 4, Some(&new)).unwrap(),
        b"Z123"
    );
}

#[test]
fn a_shrunk_file_is_asset_changed_not_a_short_span() {
    let fx = Fx::new("validator-shrunk");
    let file = fx.put("a.pdf", ALPHABET);
    let old = super::info(&fx.vault, &vp("a.pdf")).unwrap().etag.unwrap();
    std::fs::write(&file, b"0123").unwrap();
    let new = changed_validator(read_range(
        &fx.vault,
        &fx.budget,
        &vp("a.pdf"),
        0,
        20,
        Some(&old),
    ));
    assert_eq!(
        super::info(&fx.vault, &vp("a.pdf")).unwrap().etag.unwrap(),
        new
    );
    assert!(new.starts_with("\"4-"), "{new}");
}

#[test]
fn a_truncation_between_the_stat_and_the_read_is_asset_changed_not_io() {
    let fx = Fx::new("validator-race-truncate");
    let file = fx.put("a.pdf", ALPHABET);
    let old = super::info(&fx.vault, &vp("a.pdf")).unwrap().etag.unwrap();
    let new = changed_validator(super::read_range_with(
        &fx.vault,
        &fx.budget,
        &vp("a.pdf"),
        0,
        20,
        Some(&old),
        || std::fs::write(&file, b"0123").unwrap(),
    ));
    assert!(new.starts_with("\"4-"), "{new}");
}

#[test]
fn a_rewrite_between_the_stat_and_the_read_is_asset_changed_not_stale_bytes() {
    let fx = Fx::new("validator-race-rewrite");
    let file = fx.put("a.pdf", ALPHABET);
    // No if_match: bytes read across a change are not served even so.
    let result = super::read_range_with(&fx.vault, &fx.budget, &vp("a.pdf"), 0, 4, None, || {
        let mut rewritten = ALPHABET.to_vec();
        rewritten[0] = b'Z';
        std::fs::write(&file, &rewritten).unwrap();
        set_mtime(&file, 100);
    });
    changed_validator(result);
}

#[test]
fn info_gives_no_etag_for_an_asset_the_scheme_will_not_serve() {
    let fx = Fx::new("validator-nonviewable");
    fx.put("a.exe", b"MZ");
    assert!(super::info(&fx.vault, &vp("a.exe")).unwrap().etag.is_none());
}

#[test]
fn an_absent_if_match_still_serves_a_changed_file() {
    let fx = Fx::new("validator-absent");
    let file = fx.put("a.pdf", ALPHABET);
    std::fs::write(&file, b"0123456789").unwrap();
    set_mtime(&file, 100);
    assert_eq!(
        read_range(&fx.vault, &fx.budget, &vp("a.pdf"), 0, 4, None).unwrap(),
        b"0123"
    );
}

#[test]
fn refusals_come_before_any_validator_logic() {
    let fx = Fx::new("validator-refusals");
    fx.put("note.md", b"# n");
    fx.put(".serat/x.png", b"x");
    for path in ["note.md", ".serat/x.png", "missing.pdf"] {
        let err = read_range(&fx.vault, &fx.budget, &vp(path), 0, 5, Some("\"1-1\"")).unwrap_err();
        assert!(matches!(err, IpcError::NotFound { .. }), "{path}: {err:?}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(fx.outside().join("secret.png"), fx.base.join("vault/l.png"))
            .unwrap();
        let err =
            read_range(&fx.vault, &fx.budget, &vp("l.png"), 0, 5, Some("\"1-1\"")).unwrap_err();
        assert!(matches!(err, IpcError::NotFound { .. }), "{err:?}");
    }
}

// -- what needs a real window ------------------------------------------------
//
// ADR-0017 lists these as cases of task #19, and `tauri::test::mock_app()`
// crashes on this toolchain before any Serat code runs (ADR-0012 *Tested
// against*, F-345), so a rendered document cannot be driven headlessly here.
// They are present, named and ignored with the reason, so the list in the ADR
// maps one-to-one onto a test in this file. How to run them for real is in the
// task report: `xvfb-run -a` against a debug binary on WebKitGTK; WebView2 and
// WKWebView need test-engineer's per-platform Playwright harness (NFR-PRF-02).

macro_rules! needs_a_window {
    ($name:ident, $what:literal) => {
        #[test]
        #[ignore = concat!("needs a real webview window: ", $what)]
        fn $name() {
            panic!("run under xvfb-run / the per-platform harness; see the task #19 report");
        }
    };
}

needs_a_window!(
    top_level_navigation_to_the_scheme_is_refused_by_on_navigation,
    "navigate the main webview to serat-asset://localhost/a.pdf (and http://serat-asset.localhost/ on Windows); assert it stays on the app document (control a)"
);
needs_a_window!(
    a_scheme_document_runs_no_script_even_with_the_navigation_guard_disabled,
    "guard-disabled test build only: load an SVG with script as the top-level document; assert no script runs and no invoke succeeds, on WebView2, WKWebView and WebKitGTK (control b)"
);
needs_a_window!(
    an_iframe_and_an_object_at_a_scheme_svg_with_script_run_nothing_with_the_pin_on_and_off,
    "insert <iframe>/<object> at a scheme SVG; with frame-src/object-src 'none' it must not load at all, with the pin removed the sandbox CSP alone must hold (control c)"
);
needs_a_window!(
    a_download_link_at_the_scheme_writes_no_file,
    "click <a download href=serat-asset://...> in the app document; no file may be written anywhere (on_download is unconstructible outside tauri)"
);
needs_a_window!(
    a_pdf_with_javascript_runs_nothing_in_the_viewer,
    "open a PDF carrying /JS in the bundled pdf.js with enableScripting false (the viewer is vue-frontend-dev's, P1 #40)"
);
needs_a_window!(
    webkitgtk_and_wkwebview_deliver_a_custom_scheme_206_with_these_headers,
    "ADR-0017's open question: if either does not, that platform uses read_asset_range"
);
