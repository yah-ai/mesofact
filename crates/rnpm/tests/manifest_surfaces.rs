//! R773-F6's four manifest surfaces: `overrides`/`resolutions`, the
//! `optionalDependencies` failure paths, the `os`/`cpu`/`libc` platform gates,
//! and dist-tags.
//!
//! Two of the four are only observable through
//! [`ResolvedTree::warnings`][rnpm::ResolvedTree::warnings] — a skipped
//! optional and a deprecated pick both leave a tree that is *correct*, so a
//! test that only asserted on the tree would pass with the whole feature
//! deleted. Each of those therefore asserts on the tree **and** on what the
//! resolver said about it.
//!
//! Nothing here touches the network: the registry is a `FakeTransport` table.
//! No test states a host, and none can: R773-F9 made resolution
//! host-independent, so surface 3 is now about what a node *records* — its
//! `os`/`cpu`/`libc` and whether it is optional — rather than about what the
//! walk left out. The gates themselves are applied one layer over, by
//! `mesofact-build`'s materializer, and tested there.

use rnpm::resolve::{RegistrySource, ResolvedTree, Resolver, RootManifest};
use rnpm::testing::FakeTransport;
use rnpm::{CachePolicy, PeerIssue, PeerReport, RegistryClient, RegistryEndpoint, ResolveWarning};
use serde_json::{json, Map, Value};

const BASE: &str = "https://registry.test";

/// Build an abbreviated packument. `versions` is `(version, extra fields)`,
/// merged over the generated `version`/`dist` pair.
fn packument(name: &str, versions: &[(&str, Value)]) -> String {
    let mut map = Map::new();
    let mut latest = "";
    for (version, extra) in versions {
        let mut entry = json!({
            "version": version,
            "dist": {
                "tarball": format!("{BASE}/{name}/-/{name}-{version}.tgz"),
                "integrity": format!("sha512-{name}-{version}")
            }
        });
        if let Some(fields) = extra.as_object() {
            let target = entry.as_object_mut().expect("the generated entry is an object");
            for (key, value) in fields {
                target.insert(key.clone(), value.clone());
            }
        }
        map.insert((*version).to_string(), entry);
        latest = version;
    }
    json!({ "name": name, "dist-tags": { "latest": latest }, "versions": map }).to_string()
}

fn deps(pairs: &[(&str, &str)]) -> Value {
    let map: Map<String, Value> = pairs
        .iter()
        .map(|(name, range)| ((*name).to_string(), Value::from(*range)))
        .collect();
    json!({ "dependencies": map })
}

struct World {
    _dir: tempfile::TempDir,
    client: RegistryClient<FakeTransport>,
    endpoint: RegistryEndpoint,
}

impl World {
    fn new(packuments: &[(&str, String)]) -> Self {
        Self::with_failures(packuments, &[])
    }

    /// `failures` name packages whose *fetch* errors — a transport or HTTP
    /// failure, which is a different thing from a package the registry does
    /// not have. A name absent from both tables 404s.
    fn with_failures(packuments: &[(&str, String)], failures: &[(&str, &str)]) -> Self {
        let mut transport = FakeTransport::new();
        for (name, body) in packuments {
            transport = transport.serving(&format!("{BASE}/{name}"), Some("\"v\""), body);
        }
        for (name, message) in failures {
            transport = transport.failing(&format!("{BASE}/{name}"), message);
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let client = RegistryClient::new(dir.path(), transport)
            .with_policy(CachePolicy::MaxAge(std::time::Duration::from_secs(3600)));
        Self { _dir: dir, client, endpoint: RegistryEndpoint::new("npm", BASE) }
    }

    fn source(&self) -> RegistrySource<'_, FakeTransport> {
        RegistrySource::new(&self.client, &self.endpoint)
    }

    fn resolve(&self, root_json: &str) -> ResolvedTree {
        self.try_resolve(root_json).expect("resolve")
    }

    fn try_resolve(&self, root_json: &str) -> anyhow::Result<ResolvedTree> {
        let source = self.source();
        let manifest = RootManifest::from_package_json(root_json)?;
        Resolver::new(&source).resolve(&manifest)
    }

    /// Phase 1 then phase 2, which is the only order either is defined in.
    /// Surface 3's peer tests need the whole resolve because the two phases
    /// reach a packument by different routes and must agree about it.
    fn resolve_peers(&self, root_json: &str) -> (ResolvedTree, PeerReport) {
        let source = self.source();
        let manifest = RootManifest::from_package_json(root_json).expect("root manifest");
        let mut tree = Resolver::new(&source).resolve(&manifest).expect("phase 1");
        let report = rnpm::resolve_peers(&mut tree, &source).expect("phase 2");
        (tree, report)
    }
}

/// `install path → is it optional`, which is the surface-3 assertion now that
/// the gates are recorded rather than applied.
fn optionality(tree: &ResolvedTree) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = tree
        .packages()
        .map(|(id, node)| (tree.install_path(id), node.optional))
        .collect();
    out.sort();
    out
}

/// The `os` / `cpu` / `libc` a node carries into the lock.
fn gates_at(tree: &ResolvedTree, path: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let resolution = tree
        .packages()
        .find(|(id, _)| tree.install_path(*id) == path)
        .unwrap_or_else(|| panic!("no node at {path:?}; tree has {:?}", paths(tree)))
        .1
        .resolution()
        .expect("a registry node");
    (resolution.os.clone(), resolution.cpu.clone(), resolution.libc.clone())
}

fn paths(tree: &ResolvedTree) -> Vec<String> {
    let mut out: Vec<String> = tree.packages().map(|(id, _)| tree.install_path(id)).collect();
    out.sort();
    out
}

fn version_at(tree: &ResolvedTree, path: &str) -> String {
    tree.packages()
        .find(|(id, _)| tree.install_path(*id) == path)
        .unwrap_or_else(|| panic!("no node at {path:?}; tree has {:?}", paths(tree)))
        .1
        .version()
        .unwrap_or_else(|| panic!("{path} has no version"))
        .to_string()
}

/// Every warning rendered through `Display`, joined — what a CLI above this
/// crate would print, and the shape a human reads a failure in.
fn rendered(tree: &ResolvedTree) -> String {
    tree.warnings().iter().map(|w| format!("{w}\n")).collect()
}

// ---------------------------------------------------------------------------
// Surface 1 — overrides / resolutions
// ---------------------------------------------------------------------------

/// `app` and `other` both ask for `left-pad@^1`, which would resolve to
/// `1.3.0`. Neither is a *direct* dependency of the root, so an override that
/// only rewrote the root's own edges would leave both at `1.3.0` — this is the
/// ticket's first acceptance criterion and it is the assertion that fails if
/// overrides are applied at the wrong layer.
fn transitive_world() -> World {
    World::new(&[
        ("app", packument("app", &[("1.0.0", deps(&[("left-pad", "^1.0.0")]))])),
        ("other", packument("other", &[("1.0.0", deps(&[("left-pad", "^1.0.0")]))])),
        (
            "left-pad",
            packument(
                "left-pad",
                &[("1.0.0", json!({})), ("1.3.0", json!({})), ("2.0.0", json!({}))],
            ),
        ),
    ])
}

const TRANSITIVE_DEPS: &str = r#""dependencies": { "app": "^1.0.0", "other": "^1.0.0" }"#;

#[test]
fn without_an_override_a_transitive_dependency_takes_the_declared_range() {
    let tree = transitive_world().resolve(&format!(r#"{{ "name": "root", {TRANSITIVE_DEPS} }}"#));
    assert_eq!(version_at(&tree, "left-pad"), "1.3.0");
    assert!(tree.warnings().is_empty(), "{}", rendered(&tree));
}

#[test]
fn an_override_redirects_a_transitive_dependency_throughout_the_tree() {
    let world = transitive_world();
    let tree = world.resolve(&format!(
        r#"{{ "name": "root", {TRANSITIVE_DEPS}, "overrides": {{ "left-pad": "2.0.0" }} }}"#
    ));

    // One copy, at the hoisted position, at the overridden version — both
    // requesters were redirected, not just whichever was walked first.
    assert_eq!(paths(&tree), ["app", "left-pad", "other"]);
    assert_eq!(version_at(&tree, "left-pad"), "2.0.0");

    // And both redirections are on the record, naming who was overridden.
    let overridden: Vec<&ResolveWarning> = tree
        .warnings()
        .iter()
        .filter(|w| matches!(w, ResolveWarning::OverrideApplied { .. }))
        .collect();
    assert_eq!(overridden.len(), 2, "{}", rendered(&tree));
    let text = rendered(&tree);
    assert!(text.contains("app@1.0.0"), "{text}");
    assert!(text.contains("other@1.0.0"), "{text}");
    assert!(text.contains("^1.0.0"), "{text}");
}

#[test]
fn an_override_reaches_a_direct_dependency_too() {
    let world = transitive_world();
    let tree = world.resolve(
        r#"{ "name": "root",
             "dependencies": { "left-pad": "^1.0.0" },
             "overrides": { "left-pad": "2.0.0" } }"#,
    );
    assert_eq!(version_at(&tree, "left-pad"), "2.0.0");
}

#[test]
fn yarns_resolutions_is_an_alias_for_the_same_flat_form() {
    let world = transitive_world();
    let tree = world.resolve(&format!(
        r#"{{ "name": "root", {TRANSITIVE_DEPS}, "resolutions": {{ "left-pad": "2.0.0" }} }}"#
    ));
    assert_eq!(version_at(&tree, "left-pad"), "2.0.0");
}

#[test]
fn overrides_wins_over_resolutions_when_a_manifest_carries_both() {
    let world = transitive_world();
    let tree = world.resolve(&format!(
        r#"{{ "name": "root", {TRANSITIVE_DEPS},
              "overrides": {{ "left-pad": "2.0.0" }},
              "resolutions": {{ "left-pad": "1.0.0" }} }}"#
    ));
    assert_eq!(version_at(&tree, "left-pad"), "2.0.0");
}

/// The one outcome this surface must never produce is a wrong tree that looks
/// right, so an override form the flat table cannot express is refused at
/// parse time, by name.
#[test]
fn an_unsupported_override_form_is_refused_by_name_rather_than_dropped() {
    let cases = [
        (r#""overrides": { "app": { "left-pad": "2.0.0" } }"#, "nested form"),
        (r#""overrides": { ".": "2.0.0" }"#, "self-reference"),
        (r#""overrides": { "left-pad@^1": "2.0.0" }"#, "version selector"),
        (r#""resolutions": { "**/left-pad": "2.0.0" }"#, "path-scoped"),
        (r#""resolutions": { "app/left-pad": "2.0.0" }"#, "dependency path"),
    ];
    let world = transitive_world();
    for (fragment, expected) in cases {
        let err = world
            .try_resolve(&format!(r#"{{ "name": "root", {TRANSITIVE_DEPS}, {fragment} }}"#))
            .expect_err(&format!("{fragment} must be refused"))
            .to_string();
        assert!(err.contains(expected), "{fragment} => {err}");
    }
}

// ---------------------------------------------------------------------------
// Surface 2 — optionalDependencies
// ---------------------------------------------------------------------------

/// The bug R773-F6 fixed. A packument fetch that *errors* — DNS, TLS, a 500 —
/// used to propagate through `?` regardless of `optional` and abort the whole
/// resolve; only `Ok(None)` reached the skip. A registry hiccup on a package
/// the manifest explicitly said could be missing must not fail an install.
#[test]
fn an_optional_dependency_whose_fetch_errors_leaves_a_working_tree() {
    let world = World::with_failures(
        &[("pkg", packument("pkg", &[("1.0.0", json!({}))]))],
        &[("flaky", "connection reset by peer")],
    );

    let tree = world.resolve(
        r#"{ "name": "root",
             "dependencies": { "pkg": "^1.0.0" },
             "optionalDependencies": { "flaky": "^1.0.0" } }"#,
    );
    assert_eq!(paths(&tree), ["pkg"]);

    let text = rendered(&tree);
    assert!(text.contains("flaky"), "{text}");
    assert!(text.contains("connection reset by peer"), "{text}");
    assert!(text.contains("the root manifest"), "{text}");
}

#[test]
fn the_same_fetch_failure_on_a_required_dependency_still_fails() {
    let world = World::with_failures(&[], &[("flaky", "connection reset by peer")]);
    let err = world
        .try_resolve(r#"{ "name": "root", "dependencies": { "flaky": "^1.0.0" } }"#)
        .expect_err("a required dependency cannot be skipped")
        .to_string();
    assert!(err.contains("flaky"), "{err}");
}

/// The two pre-existing `Ok(None)` skips still skip — and now say why.
#[test]
fn an_unpublished_optional_and_an_unsatisfiable_one_are_both_skipped_with_a_reason() {
    let world = World::new(&[("pkg", packument("pkg", &[("1.0.0", json!({}))]))]);
    let tree = world.resolve(
        r#"{ "name": "root",
             "optionalDependencies": { "absent": "^1.0.0", "pkg": "^9.0.0" } }"#,
    );
    assert!(paths(&tree).is_empty(), "{:?}", paths(&tree));

    let text = rendered(&tree);
    assert!(text.contains("absent"), "{text}");
    assert!(text.contains("not published"), "{text}");
    // `Range` renders normalised — `^9.0.0` prints as `>=9.0.0 <10.0.0-0` — so
    // this asserts on the sentence and the bound, not on the source spelling.
    assert!(text.contains("no published version satisfies >=9.0.0"), "{text}");
}

// ---------------------------------------------------------------------------
// Surface 3 — os / cpu / libc
// ---------------------------------------------------------------------------

fn native_world() -> World {
    World::new(&[
        ("pkg", packument("pkg", &[("1.0.0", json!({}))])),
        (
            "native-darwin",
            packument("native-darwin", &[("1.0.0", json!({ "os": ["darwin"] }))]),
        ),
        (
            "native-arm",
            packument("native-arm", &[("1.0.0", json!({ "os": ["linux"], "cpu": ["arm64"] }))]),
        ),
        (
            "native-musl",
            packument(
                "native-musl",
                &[("1.0.0", json!({ "os": ["linux"], "cpu": ["x64"], "libc": ["musl"] }))],
            ),
        ),
        (
            "native-not-win",
            packument("native-not-win", &[("1.0.0", json!({ "os": ["!win32"] }))]),
        ),
        // Two peer-declaring packages: one whose peer this host cannot run,
        // one whose peer it can. Both peers carry an `os` field, so the pair
        // separates "the gate fired" from "the gate fires on anything".
        (
            "peer-host-bad",
            packument(
                "peer-host-bad",
                &[("1.0.0", json!({ "peerDependencies": { "native-darwin": "^1.0.0" } }))],
            ),
        ),
        (
            "peer-host-ok",
            packument(
                "peer-host-ok",
                &[("1.0.0", json!({ "peerDependencies": { "native-not-win": "^1.0.0" } }))],
            ),
        ),
    ])
}

/// The ticket's second acceptance criterion, as R773-F9 restated it. This is
/// the real-world shape — esbuild, swc and friends publish one native binary
/// per platform as `optionalDependencies` — and **every one of them is in the
/// tree**, on every host. A `bun.lock` records all twenty-four `@esbuild/*`
/// variants; a resolver that kept only the local one could not write that lock.
///
/// What used to be a resolve-time skip is now an install-time one, so what this
/// asserts is that the metadata needed to make that decision survived.
#[test]
fn a_platform_excluded_optional_dependency_is_resolved_and_recorded_not_skipped() {
    let tree = native_world().resolve(
        r#"{ "name": "root",
             "dependencies": { "pkg": "^1.0.0" },
             "optionalDependencies": {
               "native-darwin": "^1.0.0",
               "native-arm": "^1.0.0",
               "native-musl": "^1.0.0",
               "native-not-win": "^1.0.0"
             } }"#,
    );

    assert_eq!(
        paths(&tree),
        ["native-arm", "native-darwin", "native-musl", "native-not-win", "pkg"]
    );
    // Nothing was left out, so there is nothing to warn about.
    assert!(tree.warnings().is_empty(), "{}", rendered(&tree));

    // Each node carries its own gates, verbatim and negations included — this
    // is what `mesofact-build` writes into the lock and filters on.
    assert_eq!(
        gates_at(&tree, "native-arm"),
        (vec!["linux".to_string()], vec!["arm64".to_string()], vec![])
    );
    assert_eq!(
        gates_at(&tree, "native-musl"),
        (
            vec!["linux".to_string()],
            vec!["x64".to_string()],
            vec!["musl".to_string()]
        )
    );
    assert_eq!(
        gates_at(&tree, "native-not-win"),
        (vec!["!win32".to_string()], vec![], vec![])
    );
    // An ordinary package declares nothing, which is the common case.
    assert_eq!(gates_at(&tree, "pkg"), (vec![], vec![], vec![]));

    // And the flag that decides whether a mismatch is a skip or a failure.
    assert_eq!(
        optionality(&tree),
        [
            ("native-arm".to_string(), true),
            ("native-darwin".to_string(), true),
            ("native-musl".to_string(), true),
            ("native-not-win".to_string(), true),
            ("pkg".to_string(), false),
        ]
    );
}

/// npm's `EBADPLATFORM` is not gone, it **moved** (R773-F9). A required
/// dependency this machine cannot run resolves like any other — the refusal is
/// `mesofact-build`'s, at install time, where the host is known. Failing here
/// instead would mean a lock cut on macOS had no linux binaries in it.
///
/// See `mesofact-build`'s `install.rs`
/// (`a_required_platform_mismatch_is_an_error_naming_the_package_and_the_host`)
/// for the other end of this pair.
#[test]
fn a_required_dependency_whose_platform_excludes_a_host_still_resolves() {
    let tree = native_world()
        .resolve(r#"{ "name": "root", "dependencies": { "native-darwin": "^1.0.0" } }"#);

    assert_eq!(paths(&tree), ["native-darwin"]);
    assert_eq!(gates_at(&tree, "native-darwin").0, ["darwin"]);
    // Required, so a materializer that cannot run it must say so rather than
    // quietly install a tree missing a package it needs.
    assert_eq!(optionality(&tree), [("native-darwin".to_string(), false)]);
    assert!(tree.warnings().is_empty(), "{}", rendered(&tree));
}

/// The `libc` axis specifically — the one this camp was missing, and the one a
/// lock has to carry because bun does not write it. `os`/`cpu` alone cannot
/// tell a musl build of a package from its glibc twin.
#[test]
fn the_libc_axis_reaches_the_tree_so_a_musl_and_a_glibc_build_stay_distinguishable() {
    let tree = native_world().resolve(
        r#"{ "name": "root", "dependencies": { "native-musl": "^1.0.0", "pkg": "^1.0.0" } }"#,
    );

    assert_eq!(paths(&tree), ["native-musl", "pkg"]);
    assert_eq!(gates_at(&tree, "native-musl").2, ["musl"]);
    // An axis nobody declared stays empty rather than becoming a gate that
    // admits nothing.
    assert!(gates_at(&tree, "pkg").2.is_empty());
}

/// A package reached through an optional edge *and* a required one is
/// **required**, and everything under an optional package is optional however
/// its own parent declared it.
///
/// Load-bearing since R773-F9: this flag is what tells the materializer whether
/// a platform mismatch is a skip or an `EBADPLATFORM`. Reading it off the edge
/// that happened to create the node — which is all the resolver did while it
/// was write-only — would silently drop a required package whose optional
/// requester was walked first.
#[test]
fn optionality_is_a_property_of_every_path_to_a_node_not_of_the_edge_that_made_it() {
    let world = World::new(&[
        ("pkg", packument("pkg", &[("1.0.0", json!({}))])),
        // Declares `pkg` as an ordinary required dependency, and is itself
        // only ever reached optionally.
        (
            "native-darwin",
            packument(
                "native-darwin",
                &[("1.0.0", json!({ "os": ["darwin"], "dependencies": { "pkg": "^1.0.0" } }))],
            ),
        ),
        ("leaf", packument("leaf", &[("1.0.0", json!({}))])),
    ]);

    // `pkg` is wanted optionally (through native-darwin) and required (by the
    // root); `leaf` only optionally.
    let tree = world.resolve(
        r#"{ "name": "root",
             "dependencies": { "pkg": "^1.0.0" },
             "optionalDependencies": { "native-darwin": "^1.0.0", "leaf": "^1.0.0" } }"#,
    );

    assert_eq!(
        optionality(&tree),
        [
            ("leaf".to_string(), true),
            ("native-darwin".to_string(), true),
            ("pkg".to_string(), false),
        ]
    );

    // The other direction: with no required requester, the shared package is
    // optional too — including the one its optional parent declared as a
    // plain `dependencies` entry.
    let tree = world.resolve(
        r#"{ "name": "root", "optionalDependencies": { "native-darwin": "^1.0.0" } }"#,
    );
    assert_eq!(
        optionality(&tree),
        [("native-darwin".to_string(), true), ("pkg".to_string(), true)]
    );
}

/// Phase 2's auto-install reaches a packument through `peers::auto_install`
/// rather than through `expand`, and the two must agree about a package.
/// R773-F6 made them agree by gating both; R773-F9 makes them agree by gating
/// neither — a peer whose `os` excludes some host is installed and recorded
/// like anything else, or it would be the one node missing from an otherwise
/// portable lock.
#[test]
fn a_platform_excluded_peer_is_auto_installed_like_any_other() {
    let (tree, report) = native_world()
        .resolve_peers(r#"{ "name": "root", "dependencies": { "peer-host-bad": "^1.0.0" } }"#);

    assert_eq!(paths(&tree), ["native-darwin", "peer-host-bad"]);
    assert_eq!(report.stats.auto_installed, 1);
    // Reported as a peer nobody provided *and repaired* — under R773-F6 the
    // same line read `auto_installed: false`, which is the one bit that moved.
    assert!(
        matches!(
            report.issues.as_slice(),
            [PeerIssue::Missing { peer, auto_installed: true, .. }] if peer == "native-darwin"
        ),
        "{:?}",
        report.issues
    );
    // Recorded, so the install can still decide: gates present, and required
    // (an auto-installed peer is the resolver's own repair, not an optional).
    assert_eq!(gates_at(&tree, "native-darwin").0, ["darwin"]);
    assert_eq!(
        optionality(&tree),
        [("native-darwin".to_string(), false), ("peer-host-bad".to_string(), false)]
    );
    assert!(tree.warnings().is_empty(), "{}", rendered(&tree));
}

/// The other half of the same seam: an auto-install that fired for a
/// host-compatible peer proves the route works at all, so the test above is
/// about the gate and not about auto-install being broken.
#[test]
fn a_platform_compatible_peer_still_auto_installs() {
    let (tree, report) = native_world()
        .resolve_peers(r#"{ "name": "root", "dependencies": { "peer-host-ok": "^1.0.0" } }"#);

    assert_eq!(paths(&tree), ["native-not-win", "peer-host-ok"]);
    assert_eq!(report.stats.auto_installed, 1);
    assert!(tree.warnings().is_empty(), "{}", rendered(&tree));
}

// ---------------------------------------------------------------------------
// Surface 4 — dist-tags
// ---------------------------------------------------------------------------

fn tagged_world() -> World {
    World::new(&[(
        "pkg",
        packument("pkg", &[("1.0.0", json!({ "deprecated": "use 2.x" })), ("2.0.0", json!({}))]),
    )])
}

#[test]
fn a_dist_tag_resolves_to_the_version_it_names() {
    let tree = tagged_world().resolve(r#"{ "name": "root", "dependencies": { "pkg": "latest" } }"#);
    assert_eq!(version_at(&tree, "pkg"), "2.0.0");
}

/// "that tag is not published" and "no version satisfies that range" are
/// different failures with different fixes. Reading the tag out through
/// `Display` made them the same sentence — `no version of pkg satisfies
/// nightly` reads as a broken range rather than a missing tag.
#[test]
fn an_unpublished_dist_tag_says_so_rather_than_blaming_the_range() {
    let world = tagged_world();

    let tag = world
        .try_resolve(r#"{ "name": "root", "dependencies": { "pkg": "nightly" } }"#)
        .expect_err("no such tag")
        .to_string();
    assert!(tag.contains("no `nightly` dist-tag"), "{tag}");
    assert!(tag.contains("pkg"), "{tag}");
    assert!(tag.contains("the root manifest"), "{tag}");

    // The range failure keeps saying what it always said.
    let range = world
        .try_resolve(r#"{ "name": "root", "dependencies": { "pkg": "^9.0.0" } }"#)
        .expect_err("no such version")
        .to_string();
    assert!(range.contains("no published version satisfies >=9.0.0"), "{range}");
    assert!(!range.contains("dist-tag"), "{range}");
}

/// npm installs a deprecated version rather than stepping around it, because
/// silently preferring an older one is a divergence nobody asked for. What it
/// does instead is say so — which until R773-F6 this resolver had no channel
/// for.
#[test]
fn a_deprecated_version_is_installed_and_warned_about() {
    let tree = tagged_world().resolve(r#"{ "name": "root", "dependencies": { "pkg": "1.0.0" } }"#);

    assert_eq!(version_at(&tree, "pkg"), "1.0.0");
    assert_eq!(
        tree.warnings(),
        [ResolveWarning::Deprecated {
            name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            reason: "use 2.x".to_string(),
        }]
    );
    assert!(rendered(&tree).contains("pkg@1.0.0 is deprecated: use 2.x"), "{}", rendered(&tree));
}

#[test]
fn an_undeprecated_version_says_nothing() {
    let tree = tagged_world().resolve(r#"{ "name": "root", "dependencies": { "pkg": "^2.0.0" } }"#);
    assert!(tree.warnings().is_empty(), "{}", rendered(&tree));
}
