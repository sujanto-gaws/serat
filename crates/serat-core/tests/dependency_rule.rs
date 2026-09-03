//! The dependency rule, checked mechanically.
//!
//! `docs/03-design/architecture.md` §2.1 states the rule and then says it is "strict
//! enough to be checked mechanically". This file is what makes that sentence true:
//!
//! 1. `serat-core` takes no internal dependency, and its external dependencies stay
//!    inside a short allow-list — which is how "performs no I/O whatsoever" is
//!    enforced rather than remembered.
//! 2. Every other product crate depends on `serat-core` and on nothing else
//!    internal.
//! 3. Nothing depends on `serat-app`.
//! 4. Every workspace member is named in [`POLICY`]. A new crate fails this test
//!    until someone writes down where it sits, so the graph cannot grow an edge that
//!    nobody decided on.
//! 5. Bench crates may link the product in order to measure it; nothing in the
//!    product links a bench crate.
//! 6. No manifest inside a teammate's `.worktrees/` copy is ever considered.
//!
//! It lives in `serat-core` because that is the crate the rule is about, and because
//! a workspace-level check needs a crate to live in and inventing one for a single
//! test would be more machinery than the rule is worth.
//!
//! Run it alone with:
//!
//! ```text
//! cargo test -p serat-core --test dependency_rule
//! ```

use std::collections::BTreeSet;
use std::process::Command;

/// Where each crate sits in the graph: the internal dependencies it may take.
///
/// Crates that do not exist yet are listed so that adding one is a manifest change
/// and not a policy change. The ordering mirrors Fig. 1: every arrow points down.
const POLICY: &[(&str, &[&str])] = &[
    // Tier 0 — types and traits, no I/O, no internal dependencies at all.
    ("serat-core", &[]),
    // Tier 1 — capability crates. serat-core and nothing else internal.
    ("serat-vault", &["serat-core"]),
    ("serat-parse", &["serat-core"]),
    ("serat-index", &["serat-core"]),
    ("serat-sq", &["serat-core"]),
    ("serat-extract", &["serat-core"]),
    ("serat-ai", &["serat-core"]),
    ("serat-mcp", &["serat-core"]),
    // Tier 2 — the shell. Depends on everything; nothing depends on it.
    (
        "serat-app",
        &[
            "serat-core",
            "serat-vault",
            "serat-parse",
            "serat-index",
            "serat-sq",
            "serat-extract",
            "serat-ai",
            "serat-mcp",
        ],
    ),
    // Tool tier — in the workspace so it builds and tests with everything else, but
    // outside the product graph, and depended on by nothing inside it (see
    // `nothing_in_the_product_depends_on_a_bench_crate`).
    //
    // The benchmark vault generator (NFR-MNT-02) takes no dependency at all, internal
    // or external, by test-engineer's decision: it must build before any product
    // crate does, and its determinism must not depend on another crate's PRNG staying
    // stable. Recorded here so that stops being a comment in its manifest and starts
    // being an assertion.
    ("vaultgen", &[]),
    // The budget harness (task B2, NFR-PRF-08) has the opposite need: it cannot
    // measure a rebuild without linking the crates that perform one. That is not a
    // hole in the rule. The rule exists to keep the product graph acyclic, and a
    // crate nothing in the product depends on cannot put a cycle in it.
    ("budget", BENCH_MAY_MEASURE),
];

/// Product crates the budget harness may link in order to measure them.
///
/// `serat-app` is absent deliberately: measuring the shell means driving it as a
/// process, which is what the cold-start budget (NFR-PRF-01) times anyway.
const BENCH_MAY_MEASURE: &[&str] = &["serat-core", "serat-vault", "serat-parse", "serat-index"];

/// Workspace members that are tools rather than product.
///
/// The direction of the arrow is the whole point: a bench crate may depend on the
/// product, and nothing in the product may depend on a bench crate.
const BENCH_CRATES: &[&str] = &["vaultgen", "budget"];

/// External dependencies `serat-core` is allowed to have.
///
/// The list is short because the argument for it is short: a crate that performs no
/// I/O needs no crate that performs I/O. Adding `notify`, `rusqlite`, `tokio` or
/// anything with a socket in it fails here, which is the point.
const CORE_ALLOWED_EXTERNAL: &[&str] = &["serde", "thiserror"];

/// Extra crates `serat-core` may use in tests only.
const CORE_ALLOWED_DEV: &[&str] = &["serde_json"];

#[test]
fn the_dependency_rule_holds() {
    let violations = violations_in(&workspace_metadata());
    assert!(
        violations.is_empty(),
        "dependency rule violated:\n  - {}",
        violations.join("\n  - ")
    );
}

/// Every way `packages` breaks the rule, as sentences a reader can act on.
///
/// Separated from the test that calls it so the check itself can be tested against
/// graphs that do violate the rule (see the bottom of this file). A guard nobody has
/// ever seen fail is not yet a guard.
fn violations_in(packages: &[Package]) -> Vec<String> {
    let members: BTreeSet<&str> = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();

    let mut violations = Vec::new();

    for package in packages {
        let Some(allowed) = policy_for(&package.name) else {
            violations.push(format!(
                "{} is a workspace member but is not in POLICY in {}. Decide where it sits in \
                 the graph (architecture §2.1) and add it.",
                package.name,
                file!()
            ));
            continue;
        };

        for dependency in &package.dependencies {
            // Only internal edges are governed; external crates are a separate
            // question, handled for serat-core below.
            if !members.contains(dependency.name.as_str()) {
                continue;
            }
            if dependency.name == package.name {
                continue; // a crate's own dev-dependency on itself, if it ever appears
            }
            if !allowed.contains(&dependency.name.as_str()) {
                violations.push(format!(
                    "{} depends on {} ({}), which the dependency rule forbids. Allowed internal \
                     dependencies for {}: {}. Crossing this needs an ADR first (CLAUDE.md, \
                     working agreement 3).",
                    package.name,
                    dependency.name,
                    dependency.kind_label(),
                    package.name,
                    describe(allowed),
                ));
            }
        }
    }

    violations
}

#[test]
fn nothing_depends_on_serat_app() {
    let metadata = workspace_metadata();
    let offenders: Vec<String> = metadata
        .iter()
        .filter(|package| package.name != "serat-app")
        .filter(|package| {
            package
                .dependencies
                .iter()
                .any(|dependency| dependency.name == "serat-app")
        })
        .map(|package| package.name.clone())
        .collect();

    assert!(
        offenders.is_empty(),
        "serat-app owns wiring, not domain logic; nothing may depend on it (architecture §2.1). \
         Offenders: {}",
        offenders.join(", ")
    );
}

#[test]
fn nothing_in_the_product_depends_on_a_bench_crate() {
    // The bench tier is allowed to link the product in order to measure it; the
    // reverse would put a test harness in a shipped binary. This falls out of POLICY
    // already — no product crate lists a bench crate — but it is stated separately
    // because it is the half of the arrangement that is easy to break by accident,
    // and a failure here should name the actual problem rather than read as a generic
    // policy violation.
    let metadata = workspace_metadata();
    let offenders: Vec<String> = metadata
        .iter()
        .filter(|package| !BENCH_CRATES.contains(&package.name.as_str()))
        .flat_map(|package| {
            package
                .dependencies
                .iter()
                .filter(|dependency| BENCH_CRATES.contains(&dependency.name.as_str()))
                .map(|dependency| format!("{} -> {}", package.name, dependency.name))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "bench crates measure the product; the product does not link the bench crates. \
         Offenders: {}",
        offenders.join(", ")
    );
}

#[test]
fn the_check_ignores_teammate_worktrees() {
    // Every teammate's worktree under .worktrees/ holds a full copy of this
    // workspace (coding-standards.md), so a checker that walked the tree would find
    // six copies of every crate and report violations that do not exist. This one
    // asks cargo instead, and the root manifest carries exclude = [".worktrees"];
    // between them, no manifest inside a worktree is ever considered.
    //
    // The comparison is relative to the workspace root, not absolute: run from inside
    // .worktrees/rust-core, every path in the workspace legitimately contains
    // ".worktrees" and only a nested one is a problem.
    let workspace = workspace();
    let strays: Vec<String> = workspace
        .packages
        .iter()
        .map(|package| package.manifest_path.as_str())
        .filter(|path| {
            path.strip_prefix(&workspace.root)
                .unwrap_or(path)
                .contains(".worktrees")
        })
        .map(ToOwned::to_owned)
        .collect();

    assert!(
        strays.is_empty(),
        "the workspace has discovered crates inside .worktrees/. Check that the root \
         Cargo.toml still carries exclude = [\".worktrees\"]. Found: {}",
        strays.join(", ")
    );
}

#[test]
fn serat_core_performs_no_io() {
    let metadata = workspace_metadata();
    let core = metadata
        .iter()
        .find(|package| package.name == "serat-core")
        .expect("serat-core is a workspace member");

    let mut violations = Vec::new();
    for dependency in &core.dependencies {
        let allowed = if dependency.is_dev() {
            CORE_ALLOWED_EXTERNAL.contains(&dependency.name.as_str())
                || CORE_ALLOWED_DEV.contains(&dependency.name.as_str())
        } else {
            CORE_ALLOWED_EXTERNAL.contains(&dependency.name.as_str())
        };
        if !allowed {
            violations.push(format!("{} ({})", dependency.name, dependency.kind_label()));
        }
    }

    assert!(
        violations.is_empty(),
        "serat-core performs no I/O whatsoever (architecture §2.1), so its dependencies stay \
         inside {} (plus {} for tests). Unexpected: {}",
        describe(CORE_ALLOWED_EXTERNAL),
        describe(CORE_ALLOWED_DEV),
        violations.join(", "),
    );
}

#[test]
fn the_policy_table_is_itself_acyclic() {
    // A cycle in POLICY would let the test pass while the graph is wrong, so the
    // table is checked rather than trusted: a crate may only depend on crates listed
    // before it.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (name, allowed) in POLICY {
        for dependency in *allowed {
            assert!(
                seen.contains(dependency),
                "POLICY lists {name} -> {dependency}, but {dependency} is not above it. Either \
                 the table has a cycle or it is in the wrong order."
            );
        }
        seen.insert(name);
    }
}

fn policy_for(name: &str) -> Option<&'static [&'static str]> {
    POLICY
        .iter()
        .find(|(crate_name, _)| *crate_name == name)
        .map(|(_, allowed)| *allowed)
}

fn describe(names: &[&str]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// One workspace member as `cargo metadata` reports it.
#[derive(Debug)]
struct Package {
    name: String,
    /// Absolute path to the crate's manifest, used to prove no worktree copy was
    /// picked up.
    manifest_path: String,
    dependencies: Vec<Dependency>,
}

impl Package {
    /// A synthetic package, for testing the check itself.
    fn new(name: &str, dependencies: &[(&str, Option<&str>)]) -> Self {
        Self {
            name: name.to_owned(),
            manifest_path: format!("crates/{name}/Cargo.toml"),
            dependencies: dependencies
                .iter()
                .map(|(name, kind)| Dependency {
                    name: (*name).to_owned(),
                    kind: kind.map(ToOwned::to_owned),
                })
                .collect(),
        }
    }
}

/// One declared dependency edge, of any kind.
///
/// Dev- and build-dependencies count. A dev-dependency does not create a runtime
/// cycle, but it does make one crate's tests unbuildable without another crate, and
/// that is exactly the coupling the rule exists to prevent.
#[derive(Debug)]
struct Dependency {
    name: String,
    kind: Option<String>,
}

impl Dependency {
    fn is_dev(&self) -> bool {
        self.kind.as_deref() == Some("dev")
    }

    fn kind_label(&self) -> &str {
        match self.kind.as_deref() {
            Some(kind) => kind,
            None => "normal",
        }
    }
}

/// Ask cargo what the workspace actually contains.
///
/// `--no-deps` keeps this to workspace members and their declared edges: no registry
/// resolution, no network, and a runtime in the tens of milliseconds, so this can sit
/// in the ordinary test run rather than in a job someone remembers to add.
/// The workspace as cargo sees it.
#[derive(Debug)]
struct Workspace {
    /// Absolute path of the directory holding the root manifest.
    root: String,
    /// Every workspace member.
    packages: Vec<Package>,
}

/// Just the members, for the checks that do not care where the workspace lives.
fn workspace_metadata() -> Vec<Package> {
    workspace().packages
}

fn workspace() -> Workspace {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");

    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .args(["--manifest-path", manifest])
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata emits json");
    let packages = json["packages"]
        .as_array()
        .expect("metadata has a packages array");

    let root = json["workspace_root"]
        .as_str()
        .expect("metadata has a workspace root")
        .to_owned();

    let packages = packages
        .iter()
        .map(|package| Package {
            name: package["name"]
                .as_str()
                .expect("package has a name")
                .to_owned(),
            manifest_path: package["manifest_path"]
                .as_str()
                .expect("package has a manifest path")
                .to_owned(),
            dependencies: package["dependencies"]
                .as_array()
                .expect("package has dependencies")
                .iter()
                .map(|dependency| Dependency {
                    name: dependency["name"]
                        .as_str()
                        .expect("dependency has a name")
                        .to_owned(),
                    kind: dependency["kind"].as_str().map(ToOwned::to_owned),
                })
                .collect(),
        })
        .collect();

    Workspace { root, packages }
}

/// Tests of the check itself, against graphs that are wrong on purpose.
///
/// Without these, `the_dependency_rule_holds` passing would be consistent with the
/// check never looking at anything.
mod the_check_catches {
    use super::{Package, violations_in};

    #[test]
    fn a_forbidden_edge_between_capability_crates() {
        let packages = [
            Package::new("serat-core", &[]),
            Package::new("serat-parse", &[("serat-core", None)]),
            // The edge someone would add the first time serat-index wants to parse
            // something instead of being handed a ParsedNote.
            Package::new(
                "serat-index",
                &[("serat-core", None), ("serat-parse", None)],
            ),
        ];
        let violations = violations_in(&packages);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("serat-index depends on serat-parse"),
            "{}",
            violations[0]
        );
    }

    #[test]
    fn a_forbidden_edge_hidden_in_dev_dependencies() {
        // The tempting one: "it is only for tests". It still makes serat-vault's
        // test build depend on serat-index.
        let packages = [
            Package::new("serat-core", &[]),
            Package::new("serat-index", &[("serat-core", None)]),
            Package::new(
                "serat-vault",
                &[("serat-core", None), ("serat-index", Some("dev"))],
            ),
        ];
        let violations = violations_in(&packages);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("(dev)"), "{}", violations[0]);
    }

    #[test]
    fn anything_depending_on_the_app() {
        let packages = [
            Package::new("serat-core", &[]),
            Package::new("serat-app", &[("serat-core", None)]),
            Package::new("serat-mcp", &[("serat-core", None), ("serat-app", None)]),
        ];
        let violations = violations_in(&packages);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("serat-mcp depends on serat-app"),
            "{}",
            violations[0]
        );
    }

    #[test]
    fn an_internal_dependency_in_serat_core() {
        let packages = [
            Package::new("serat-core", &[("serat-vault", None)]),
            Package::new("serat-vault", &[]),
        ];
        let violations = violations_in(&packages);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("Allowed internal dependencies"),
            "{}",
            violations[0]
        );
        assert!(violations[0].contains("none"), "{}", violations[0]);
    }

    #[test]
    fn a_crate_nobody_has_placed_in_the_graph() {
        let packages = [
            Package::new("serat-core", &[]),
            Package::new("serat-mystery", &[("serat-core", None)]),
        ];
        let violations = violations_in(&packages);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("not in POLICY"), "{}", violations[0]);
    }

    #[test]
    fn a_product_crate_reaching_into_the_bench_tier() {
        // The direction that matters. serat-index linking the generator to build a
        // fixture would put a benchmark tool in a shipped binary.
        let packages = [
            Package::new("serat-core", &[]),
            Package::new(
                "serat-index",
                &[("serat-core", None), ("vaultgen", Some("dev"))],
            ),
            Package::new("vaultgen", &[]),
        ];
        let violations = violations_in(&packages);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(
            violations[0].contains("serat-index depends on vaultgen"),
            "{}",
            violations[0]
        );
    }

    #[test]
    fn but_allows_the_budget_harness_to_link_what_it_measures() {
        let packages = [
            Package::new("serat-core", &[]),
            Package::new("serat-vault", &[("serat-core", None)]),
            Package::new("serat-parse", &[("serat-core", None)]),
            Package::new("serat-index", &[("serat-core", None)]),
            Package::new(
                "budget",
                &[
                    ("serat-core", None),
                    ("serat-vault", None),
                    ("serat-parse", None),
                    ("serat-index", None),
                ],
            ),
        ];
        assert!(violations_in(&packages).is_empty());
    }

    #[test]
    fn and_passes_the_graph_the_architecture_describes() {
        let packages = [
            Package::new("serat-core", &[("serde", None), ("thiserror", None)]),
            Package::new("serat-vault", &[("serat-core", None), ("notify", None)]),
            Package::new("serat-parse", &[("serat-core", None), ("comrak", None)]),
            Package::new("serat-index", &[("serat-core", None), ("rusqlite", None)]),
            Package::new(
                "serat-app",
                &[
                    ("serat-core", None),
                    ("serat-vault", None),
                    ("serat-parse", None),
                    ("serat-index", None),
                ],
            ),
            Package::new("vaultgen", &[]),
        ];
        assert!(violations_in(&packages).is_empty());
    }
}
