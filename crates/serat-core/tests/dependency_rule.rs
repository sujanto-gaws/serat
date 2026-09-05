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
    // The IPC contract. Takes no internal dependency and never will: serat-app
    // includes contracts/ipc.rs as its own module, so this crate exists to compile
    // and export it, not to be linked. Wiring it to serat-core would let the wire
    // types drift into domain types, which ADR-0007 relies on them not being.
    ("serat-contracts", &[]),
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
    ("budget", BUDGET_MAY_LINK),
    // doclint (task #11, F-239) checks that paths cited in the design documents
    // resolve. It reads Markdown, not the product, and takes no dependency at
    // all -- the same reasoning as vaultgen: it must not need the product to
    // build in order to say something about the product's documentation.
    ("doclint", &[]),
];

/// What the budget harness may link.
///
/// The product crates it measures, plus `vaultgen`. `serat-app` is absent
/// deliberately: measuring the shell means driving it as a process, which is what
/// the cold-start budget (NFR-PRF-01) times anyway.
///
/// `vaultgen` is a bench-to-bench edge, added by test-engineer in task B2. Every job
/// verifies the vault against its manifest before it starts the clock, and there must
/// be exactly one definition of the tree digest doing that — duplicating a SHA-256
/// into the harness so the graph looks tidier would be the tail wagging the dog. The
/// invariant the rule exists to protect is untouched: nothing under `crates/` depends
/// on either of them (`nothing_in_the_product_depends_on_a_bench_crate`).
const BUDGET_MAY_LINK: &[&str] = &[
    "serat-core",
    "serat-vault",
    "serat-parse",
    "serat-index",
    "vaultgen",
];

/// Workspace members that are tools rather than product.
///
/// The direction of the arrow is the whole point: a bench crate may depend on the
/// product, and nothing in the product may depend on a bench crate.
const BENCH_CRATES: &[&str] = &["vaultgen", "budget"];

/// External crates each product crate may **not** take, and why.
///
/// This is architecture §2.1's "notably does not own" column made mechanical. That
/// table is the load-bearing half of the crate decomposition — a crate that quietly
/// acquires a capability it was defined not to have does not break the dependency
/// graph, so nothing else here would notice, and the design is wrong from then on
/// without a single failing test.
///
/// A forbid-list rather than an allow-list, deliberately: these crates legitimately
/// take supporting dependencies nobody needs to approve, and an allow-list would turn
/// every serde or thiserror into a policy question. `serat-core` is the exception and
/// keeps its allow-list, because "performs no I/O whatsoever" is a claim about
/// everything it links rather than about a handful of named crates.
///
/// The reason strings are not decoration. A failure here should say what capability
/// was acquired and why the crate was defined not to have it, because the person
/// reading it will otherwise conclude the rule is arbitrary and add an exception.
const FORBIDDEN_EXTERNAL: &[(&str, &[&str], &str)] = &[
    (
        "serat-index",
        &[
            "comrak",
            "pulldown-cmark",
            "markdown",
            "notify",
            "walkdir",
            "ignore",
        ],
        "serat-index owns the index and no knowledge of Markdown or of the vault's          filesystem (§2.1). A vault path reaches it only as a VaultPath inside a row —          a string it stores, not a file it opens. It owns .serat/ and may create and          delete the index there; the vault belongs to serat-vault, and what the bytes          mean belongs to serat-parse, which arrive as NoteSource and NoteParser.",
    ),
    (
        "serat-parse",
        &["rusqlite", "notify", "walkdir", "ignore", "tokio"],
        "serat-parse takes bytes and returns structures and performs no file access at          all (§2.1, §3.1 step 4). That purity is what lets the parse pool run it across          cores with no coordination; a parser that could read a file could not be run          that way and could not be tested with a string literal.",
    ),
    (
        "serat-vault",
        &["comrak", "pulldown-cmark", "markdown", "rusqlite"],
        "serat-vault owns watching, reading, atomic writes and path canonicalisation,          and no knowledge of Markdown (§2.1). It hands out bytes; deciding what they          mean is serat-parse's, and storing them is serat-index's.",
    ),
    (
        "serat-sq",
        &["rusqlite"],
        "serat-sq emits a statement and a plan and does not execute (§2.1). Linking a          database is how a compiler becomes an engine, and it is what lets SQ be          typechecked against a SchemaProvider without one.",
    ),
];

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
fn no_crate_acquires_a_capability_it_was_defined_not_to_have() {
    // The other half of §2.1. The dependency rule keeps the graph acyclic; this keeps
    // each crate's charter honest. Nothing else would catch serat-index taking comrak:
    // the graph stays perfectly acyclic, every existing test passes, and the crate has
    // simply stopped being what the design says it is.
    let metadata = workspace_metadata();
    let mut violations = Vec::new();

    for package in &metadata {
        let Some((_, forbidden, reason)) = FORBIDDEN_EXTERNAL
            .iter()
            .find(|(name, _, _)| *name == package.name)
        else {
            continue;
        };

        for dependency in &package.dependencies {
            if forbidden.contains(&dependency.name.as_str()) {
                violations.push(format!(
                    "{} takes {} ({}).
    {reason}",
                    package.name,
                    dependency.name,
                    dependency.kind_label()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "a crate acquired a capability §2.1 says it does not own:
  - {}",
        violations.join(
            "
  - "
        )
    );
}

#[test]
fn every_forbid_list_names_a_crate_the_policy_knows_about() {
    // A forbid-list entry for a crate that has been renamed or dropped protects
    // nothing while looking like it does. This is the same failure as a guard that
    // cannot fail, one level up in the file.
    for (name, _, _) in FORBIDDEN_EXTERNAL {
        assert!(
            policy_for(name).is_some(),
            "{name} has a forbid-list but is not in POLICY; one of the two is stale"
        );
    }
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
fn no_worktree_copy_is_part_of_this_workspace() {
    // F-28. The previous version of this test could not fail, and neither could the
    // version before that: `members` is an explicit list with no globs, so
    // `cargo metadata --no-deps` can never report a manifest under `.worktrees/`
    // whether the exclude is there or not. code-reviewer proved it by putting a full
    // workspace copy in a worktree, deleting the exclude, and watching the test pass.
    //
    // So this asserts the two properties that actually protect the invariant, against
    // the manifest TEXT, where they can genuinely break:
    //
    //   1. `members` stays glob-free. A `crates/*` or `**` pattern is what would let
    //      cargo discover a worktree copy, and it is the plausible future edit — it
    //      looks tidier than a list.
    //   2. `exclude = [".worktrees"]` is present, which is what stops cargo treating a
    //      nested manifest as part of this workspace for the commands that do walk.
    //
    // Keeping the cargo-metadata check as well, because it is the one that would catch
    // a worktree copy arriving by some route neither of us has thought of.
    let workspace = workspace();
    let manifest_path = std::path::Path::new(&workspace.root).join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display()));

    let members =
        section(&manifest, "members = [").expect("the root manifest must list workspace members");
    let globbed: Vec<&str> = members
        .lines()
        .map(str::trim)
        .filter(|line| line.contains('*'))
        .collect();
    assert!(
        globbed.is_empty(),
        "workspace.members contains a glob: {globbed:?}. Cargo expands globs by walking          the tree, which would discover every teammate's worktree copy as a member.          List members explicitly."
    );

    assert!(
        manifest.contains(r#"exclude = [".worktrees"]"#),
        "the root manifest no longer excludes .worktrees. Every teammate's worktree          holds a full copy of this workspace (coding-standards.md), and cargo commands          that walk the tree will treat those copies as nested packages."
    );

    // And the belt to that pair of braces: nothing cargo reports may live in one.
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
        "cargo reports workspace members inside .worktrees/: {}",
        strays.join(", ")
    );
}

/// The bracketed body following `header` in a manifest, for the crude text checks
/// above. Not a TOML parser, and it does not need to be: it is looking for the
/// presence of a character, not for structure.
fn section<'a>(manifest: &'a str, header: &str) -> Option<&'a str> {
    let start = manifest.find(header)? + header.len();
    let end = manifest[start..].find(']')? + start;
    Some(&manifest[start..end])
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
