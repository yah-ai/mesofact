//! R773-F4's three verification criteria, as tests.
//!
//! 1. A dependency set where two packages need incompatible peers produces two
//!    instances of the dependent, each seeing its own peer — and Node resolves
//!    each to the intended one.
//! 2. A deep tree with no peer dependencies anywhere resolves without the walk
//!    recursing (the pure fast path fires).
//! 3. A peer dependency cycle terminates.
//!
//! Plus the four manifest-level semantics the ticket spells out: a satisfied
//! peer records its provider, a missing non-optional peer is auto-installed and
//! warned about (npm 7+), a missing peer marked `optional` is silently skipped,
//! and a provider that is present but wrong warns without failing the resolve.
//!
//! Nothing here touches the network. Criterion 2 is only checkable with a
//! counter, which is why [`PeerStats`] exists: "without recursing" is a claim
//! about work *not* done, and the only honest proof is that `visited` stayed at
//! zero while `pure_skips` fired once at the root.

use rnpm::peers::{resolve_peers, PeerIssue, PeerReport};
use rnpm::resolve::{NodeId, RegistrySource, ResolvedTree, Resolver, RootManifest};
use rnpm::testing::FakeTransport;
use rnpm::{CachePolicy, Node, RegistryClient, RegistryEndpoint};
use serde_json::{json, Map, Value};

const BASE: &str = "https://registry.test";

/// Build an abbreviated packument. `versions` is `(version, extra fields)`,
/// where the extra object is merged over the generated `version`/`dist` pair,
/// so a row can add `dependencies`, `peerDependencies` and so on.
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
    json!({ "dependencies": object(pairs) })
}

fn peers(pairs: &[(&str, &str)]) -> Value {
    json!({ "peerDependencies": object(pairs) })
}

fn object(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::from(*value)))
        .collect()
}

/// A hermetic world: a fake registry, a real `RegistryClient` over it, and the
/// two-phase resolve the tests actually exercise.
struct World {
    _dir: tempfile::TempDir,
    client: RegistryClient<FakeTransport>,
    endpoint: RegistryEndpoint,
}

impl World {
    fn new(packuments: &[(&str, String)]) -> Self {
        let mut transport = FakeTransport::new();
        for (name, body) in packuments {
            transport = transport.serving(&format!("{BASE}/{name}"), Some("\"v\""), body);
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let client = RegistryClient::new(dir.path(), transport)
            .with_policy(CachePolicy::MaxAge(std::time::Duration::from_secs(3600)));
        Self { _dir: dir, client, endpoint: RegistryEndpoint::new("npm", BASE) }
    }

    fn source(&self) -> RegistrySource<'_, FakeTransport> {
        RegistrySource::new(&self.client, &self.endpoint)
    }

    /// Phase 1 then phase 2, which is the only order either is defined in.
    fn resolve(&self, root_json: &str) -> (ResolvedTree, PeerReport) {
        let source = self.source();
        let manifest = RootManifest::from_package_json(root_json).expect("root manifest");
        let mut tree = Resolver::new(&source).resolve(&manifest).expect("phase 1");
        let report = resolve_peers(&mut tree, &source).expect("phase 2");
        (tree, report)
    }

    /// Phase 1 alone, for the tests that need to compare before and after.
    fn resolve_tree_only(&self, root_json: &str) -> ResolvedTree {
        let source = self.source();
        let manifest = RootManifest::from_package_json(root_json).expect("root manifest");
        Resolver::new(&source).resolve(&manifest).expect("phase 1")
    }
}

/// Every install path in the tree, sorted — the arena's insertion order is an
/// implementation detail and no assertion should depend on it.
fn paths(tree: &ResolvedTree) -> Vec<String> {
    let mut out: Vec<String> = tree.packages().map(|(id, _)| tree.install_path(id)).collect();
    out.sort();
    out
}

/// The single node at a given install path, e.g. `"host2/plugin"`.
fn at<'t>(tree: &'t ResolvedTree, path: &str) -> (NodeId, &'t Node) {
    tree.packages()
        .find(|(id, _)| tree.install_path(*id) == path)
        .unwrap_or_else(|| panic!("no node at {path:?}; tree has {:?}", paths(tree)))
}

fn id_at(tree: &ResolvedTree, path: &str) -> NodeId {
    at(tree, path).0
}

fn version_at(tree: &ResolvedTree, path: &str) -> String {
    at(tree, path)
        .1
        .version()
        .unwrap_or_else(|| panic!("{path} has no version"))
        .to_string()
}

/// What `name` resolves to *from* `path`, by version — i.e. what Node's own
/// `node_modules` walk would find standing in that directory.
fn resolves_from(tree: &ResolvedTree, path: &str, name: &str) -> String {
    let from = id_at(tree, path);
    let provider = tree
        .resolve_from(from, name)
        .unwrap_or_else(|| panic!("{name} does not resolve from {path}"));
    tree.node(provider)
        .version()
        .unwrap_or_else(|| panic!("{name} resolved from {path} has no version"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Criterion 1 — incompatible peers produce two instances
// ---------------------------------------------------------------------------

/// Three hosts and one plugin. `host1` and `host3` pull `core@^1`, `host2`
/// pulls `core@^2`, and all three depend on the same `plugin`, whose peer range
/// accepts either major.
///
/// Phase 1 hoists one `plugin` to the root, shared by all three hosts — it has
/// no version conflict, so phase 1 has no reason to nest it. That is exactly
/// the situation phase 2 exists for: the shared node is correct for `host1` and
/// `host3` and wrong for `host2`, and only a peer-aware pass can tell.
///
/// `host3` is not decoration. It is what makes the `(identity, peer context)`
/// cache observable: the root `plugin` is reached twice in the same context, so
/// a cache hit is a fact the test can assert rather than an internal detail.
fn instancing_world() -> World {
    World::new(&[
        ("host1", packument("host1", &[("1.0.0", deps(&[("core", "^1.0.0"), ("plugin", "^1.0.0")]))])),
        ("host2", packument("host2", &[("1.0.0", deps(&[("core", "^2.0.0"), ("plugin", "^1.0.0")]))])),
        ("host3", packument("host3", &[("1.0.0", deps(&[("core", "^1.0.0"), ("plugin", "^1.0.0")]))])),
        ("core", packument("core", &[("1.5.0", json!({})), ("2.0.0", json!({}))])),
        ("plugin", packument("plugin", &[("1.0.0", peers(&[("core", "^1.0.0 || ^2.0.0")]))])),
    ])
}

const INSTANCING_ROOT: &str = r#"{
  "name": "root",
  "dependencies": { "host1": "^1.0.0", "host2": "^1.0.0", "host3": "^1.0.0" }
}"#;

#[test]
fn phase_one_leaves_exactly_one_plugin_which_is_what_makes_this_test_meaningful() {
    // The negative control for the whole criterion. If phase 1 already nested
    // the plugin, criterion 1 would pass without phase 2 doing anything, and
    // every assertion below would be measuring the wrong pass.
    let tree = instancing_world().resolve_tree_only(INSTANCING_ROOT);
    assert_eq!(paths(&tree), ["core", "host1", "host2", "host2/core", "host3", "plugin"]);
}

#[test]
fn incompatible_peers_produce_one_instance_per_distinct_peer_context() {
    let (tree, report) = instancing_world().resolve(INSTANCING_ROOT);

    // One new node, and only one: `host2` gets its own plugin, `host1` and
    // `host3` keep sharing the hoisted one.
    assert_eq!(
        paths(&tree),
        ["core", "host1", "host2", "host2/core", "host2/plugin", "host3", "plugin"]
    );
    assert_eq!(report.stats.instances, 1);
    // Nothing was unplaced to get there — the phase-1 nodes all survive.
    assert_eq!(version_at(&tree, "core"), "1.5.0");
    assert_eq!(version_at(&tree, "host2/core"), "2.0.0");
    assert_eq!(version_at(&tree, "host2/plugin"), "1.0.0");
    assert_eq!(version_at(&tree, "plugin"), "1.0.0");
}

#[test]
fn each_instance_sees_the_peer_it_was_instantiated_for() {
    let (tree, report) = instancing_world().resolve(INSTANCING_ROOT);

    // The claim as the report states it...
    let core_v1 = id_at(&tree, "core");
    let core_v2 = id_at(&tree, "host2/core");
    assert_eq!(report.providers[&id_at(&tree, "plugin")]["core"], core_v1);
    assert_eq!(report.providers[&id_at(&tree, "host2/plugin")]["core"], core_v2);

    // ...and the same claim re-derived from the tree by Node's own resolution
    // algorithm, which is the one that will actually run at require() time.
    // These two agreeing is the point: a report that named a provider the
    // directory layout does not actually expose would be worse than useless.
    assert_eq!(resolves_from(&tree, "plugin", "core"), "1.5.0");
    assert_eq!(resolves_from(&tree, "host2/plugin", "core"), "2.0.0");

    // Every peer here is satisfiable, so nothing is warned about.
    assert_eq!(report.issues, []);
}

#[test]
fn the_hosts_edges_point_at_the_copy_each_one_is_supposed_to_get() {
    let (tree, _) = instancing_world().resolve(INSTANCING_ROOT);

    let hoisted = id_at(&tree, "plugin");
    let nested = id_at(&tree, "host2/plugin");
    assert_eq!(at(&tree, "host1").1.dependencies["plugin"], hoisted);
    assert_eq!(at(&tree, "host3").1.dependencies["plugin"], hoisted);
    assert_eq!(at(&tree, "host2").1.dependencies["plugin"], nested);

    // The re-pointed requester is off the original's list and on the copy's;
    // the two requesters that did not move are untouched.
    assert_eq!(
        at(&tree, "plugin").1.requesters,
        [id_at(&tree, "host1"), id_at(&tree, "host3")].into_iter().collect()
    );
    assert_eq!(
        at(&tree, "host2/plugin").1.requesters,
        [id_at(&tree, "host2")].into_iter().collect()
    );
}

#[test]
fn a_revisit_in_the_same_peer_context_is_answered_from_the_cache() {
    let (_, report) = instancing_world().resolve(INSTANCING_ROOT);

    // `host1` and `host3` reach the same plugin node in the same context. The
    // second arrival must not re-analyse it — that re-analysis is what makes
    // the walk exponential on a real tree.
    assert_eq!(report.stats.cache_hits, 1);
    // But the cache did NOT swallow the one arrival that mattered: `host2`
    // reaches the plugin in a different context and gets a full analysis, which
    // is why `instances` is 1 rather than 0.
    assert_eq!(report.stats.instances, 1);
}

// ---------------------------------------------------------------------------
// Criterion 2 — a peer-free tree is skipped whole, without recursing
// ---------------------------------------------------------------------------

const DEEP_ROOT: &str = r#"{ "name": "root", "dependencies": { "l1": "^1.0.0" } }"#;

fn pure_world() -> World {
    World::new(&[
        ("l1", packument("l1", &[("1.0.0", deps(&[("l2", "^1.0.0")]))])),
        ("l2", packument("l2", &[("1.0.0", deps(&[("l3", "^1.0.0")]))])),
        ("l3", packument("l3", &[("1.0.0", deps(&[("l4", "^1.0.0")]))])),
        ("l4", packument("l4", &[("1.0.0", deps(&[("l5", "^1.0.0")]))])),
        ("l5", packument("l5", &[("1.0.0", json!({}))])),
    ])
}

#[test]
fn a_tree_with_no_peers_anywhere_is_skipped_at_the_root_without_recursing() {
    let (tree, report) = pure_world().resolve(DEEP_ROOT);

    // Five levels exist, so "did not recurse" is a real claim about this tree
    // and not an artefact of it being empty.
    assert_eq!(paths(&tree), ["l1", "l2", "l3", "l4", "l5"]);

    // The fast path fired exactly once, at the root, and the walk analysed
    // nothing. `visited == 0` is the whole criterion: any descent at all would
    // have incremented it.
    assert_eq!(report.stats.pure_skips, 1);
    assert_eq!(report.stats.visited, 0);
    assert_eq!(report.stats.instances, 0);
    assert_eq!(report.stats.cache_hits, 0);
    assert!(report.providers.is_empty());
    assert_eq!(report.issues, []);
}

#[test]
fn one_peer_five_levels_down_is_enough_to_defeat_the_fast_path() {
    // The negative control for criterion 2. Without this, `visited == 0` above
    // could just as well mean the walk never runs at all.
    let world = World::new(&[
        ("l1", packument("l1", &[("1.0.0", deps(&[("l2", "^1.0.0")]))])),
        ("l2", packument("l2", &[("1.0.0", deps(&[("l3", "^1.0.0")]))])),
        ("l3", packument("l3", &[("1.0.0", deps(&[("l4", "^1.0.0")]))])),
        ("l4", packument("l4", &[("1.0.0", deps(&[("l5", "^1.0.0")]))])),
        ("l5", packument("l5", &[("1.0.0", peers(&[("l1", "^1.0.0")]))])),
        ("core", packument("core", &[("1.0.0", json!({}))])),
    ]);
    let (tree, report) = world.resolve(DEEP_ROOT);

    assert_eq!(paths(&tree), ["l1", "l2", "l3", "l4", "l5"]);
    // Impurity propagates all the way up the dependency chain, so the root is
    // no longer skippable and every level on the path to the peer is analysed.
    assert_eq!(report.stats.pure_skips, 0);
    assert_eq!(report.stats.visited, 6);
    // `l5` peers on `l1`, which is hoisted and satisfies it.
    assert_eq!(report.providers[&id_at(&tree, "l5")]["l1"], id_at(&tree, "l1"));
    assert_eq!(report.issues, []);
}

// ---------------------------------------------------------------------------
// Criterion 3 — a peer cycle terminates
// ---------------------------------------------------------------------------

#[test]
fn a_peer_dependency_cycle_terminates() {
    // `a` and `b` depend on each other and both peer on `shared`. The
    // dependency cycle is what the walk actually re-enters; the peers are what
    // stop the purity fast path from skipping the whole thing before the cycle
    // is ever reached.
    let world = World::new(&[
        (
            "a",
            packument(
                "a",
                &[(
                    "1.0.0",
                    json!({
                        "dependencies": object(&[("b", "^1.0.0")]),
                        "peerDependencies": object(&[("shared", "^1.0.0")])
                    }),
                )],
            ),
        ),
        (
            "b",
            packument(
                "b",
                &[(
                    "1.0.0",
                    json!({
                        "dependencies": object(&[("a", "^1.0.0")]),
                        "peerDependencies": object(&[("shared", "^1.0.0")])
                    }),
                )],
            ),
        ),
        ("shared", packument("shared", &[("1.0.0", json!({}))])),
    ]);

    let (tree, report) = world.resolve(
        r#"{ "name": "root", "dependencies": { "a": "^1.0.0", "shared": "^1.0.0" } }"#,
    );

    // Terminating is most of the criterion — reaching this line at all is the
    // assertion. The rest says it terminated for the RIGHT reason.
    assert_eq!(report.stats.cycle_breaks, 1);
    assert_eq!(report.stats.visited, 3); // root, a, b
    assert_eq!(report.stats.instances, 0);

    // The cycle is still a cycle: nothing was cut to make the walk terminate.
    assert_eq!(at(&tree, "a").1.dependencies["b"], id_at(&tree, "b"));
    assert_eq!(at(&tree, "b").1.dependencies["a"], id_at(&tree, "a"));

    // And both ends got their peer resolved.
    let shared = id_at(&tree, "shared");
    assert_eq!(report.providers[&id_at(&tree, "a")]["shared"], shared);
    assert_eq!(report.providers[&id_at(&tree, "b")]["shared"], shared);
    assert_eq!(report.issues, []);
}

// ---------------------------------------------------------------------------
// The four per-peer verdicts
// ---------------------------------------------------------------------------

const WIDGET_ROOT: &str = r#"{ "name": "root", "dependencies": { "widget": "^1.0.0" } }"#;

#[test]
fn a_missing_non_optional_peer_is_auto_installed_and_warned_about() {
    let world = World::new(&[
        ("widget", packument("widget", &[("1.0.0", peers(&[("core", "^1.0.0")]))])),
        ("core", packument("core", &[("1.0.0", json!({})), ("1.5.0", json!({})), ("2.0.0", json!({}))])),
    ]);
    let (tree, report) = world.resolve(WIDGET_ROOT);

    // npm 7+ installs it rather than leaving the tree broken, and picks the
    // highest version satisfying the declared range — not the highest overall.
    assert_eq!(paths(&tree), ["core", "widget"]);
    assert_eq!(version_at(&tree, "core"), "1.5.0");
    assert_eq!(report.stats.auto_installed, 1);
    assert_eq!(report.providers[&id_at(&tree, "widget")]["core"], id_at(&tree, "core"));
    // It resolves from where the declaring package actually stands.
    assert_eq!(resolves_from(&tree, "widget", "core"), "1.5.0");

    // Auto-installing is still a warning: the user asked for a tree that did
    // not name this package, and got one that does.
    match report.issues.as_slice() {
        [PeerIssue::Missing { at, peer, auto_installed, .. }] => {
            assert_eq!(at, "widget");
            assert_eq!(peer, "core");
            assert!(auto_installed);
        }
        other => panic!("expected one auto-installed Missing, got {other:?}"),
    }
}

#[test]
fn an_auto_installed_peers_own_dependencies_are_resolved() {
    // R773-B8. The auto-install re-enters phase 1's BFS seeded with the peer's
    // own edges, so `core` arrives with `helper` rather than as a node that
    // resolves cleanly and then fails at require().
    let world = World::new(&[
        ("widget", packument("widget", &[("1.0.0", peers(&[("core", "^1.0.0")]))])),
        ("core", packument("core", &[("1.0.0", deps(&[("helper", "^1.0.0")]))])),
        ("helper", packument("helper", &[("1.0.0", json!({}))])),
    ]);
    let (tree, report) = world.resolve(WIDGET_ROOT);

    assert_eq!(report.stats.auto_installed, 1);
    assert_eq!(paths(&tree), ["core", "helper", "widget"]);
    assert_eq!(at(&tree, "core").1.dependencies["helper"], id_at(&tree, "helper"));
    // Nothing conflicts, so phase 1's ordinary hoist applies to the subtree —
    // `helper` goes to the root, not under `core`.
    assert_eq!(resolves_from(&tree, "core", "helper"), "1.0.0");
    // And the placement of the peer ITSELF is untouched by the expansion.
    assert_eq!(resolves_from(&tree, "widget", "core"), "1.0.0");
}

#[test]
fn an_auto_installed_peers_dependency_nests_rather_than_clobbering_a_hoisted_one() {
    // The other half of R773-B8: expanding the peer's subtree runs phase 1's
    // real placement rules, so a dependency that conflicts with something
    // already hoisted lands nested under the peer. Hoisting it would overwrite
    // the root's own `helper@1` — an unplacement, which phase 1 forbids.
    let world = World::new(&[
        ("widget", packument("widget", &[("1.0.0", peers(&[("core", "^1.0.0")]))])),
        ("core", packument("core", &[("1.0.0", deps(&[("helper", "^2.0.0")]))])),
        ("helper", packument("helper", &[("1.0.0", json!({})), ("2.0.0", json!({}))])),
    ]);
    let (tree, report) = world.resolve(
        r#"{ "name": "root", "dependencies": { "widget": "^1.0.0", "helper": "^1.0.0" } }"#,
    );

    assert_eq!(report.stats.auto_installed, 1);
    assert_eq!(paths(&tree), ["core", "core/helper", "helper", "widget"]);
    assert_eq!(version_at(&tree, "helper"), "1.0.0");
    assert_eq!(version_at(&tree, "core/helper"), "2.0.0");
    // Each side sees its own, which is the whole point of nesting.
    assert_eq!(resolves_from(&tree, "core", "helper"), "2.0.0");
    assert_eq!(resolves_from(&tree, "widget", "helper"), "1.0.0");
    assert_eq!(resolves_from(&tree, "widget", "core"), "1.0.0");
}

#[test]
fn an_auto_installed_peer_gets_its_own_peers_resolved() {
    // R773-B8's second half: the auto-installed node declares what its manifest
    // declares, and phase 2 descends into it from the auto-install itself — so
    // a peer one level *inside* an auto-install is an ordinary peer, not a hole.
    let world = World::new(&[
        ("widget", packument("widget", &[("1.0.0", peers(&[("core", "^1.0.0")]))])),
        ("core", packument("core", &[("1.0.0", peers(&[("shared", "^1.0.0")]))])),
        ("shared", packument("shared", &[("1.0.0", json!({}))])),
    ]);

    // (a) The inner peer's provider is already in the tree: matched, no install.
    let (tree, report) = world.resolve(
        r#"{ "name": "root", "dependencies": { "widget": "^1.0.0", "shared": "^1.0.0" } }"#,
    );
    assert_eq!(report.stats.auto_installed, 1); // `core` only
    assert_eq!(paths(&tree), ["core", "shared", "widget"]);
    // The peer map is parsed from the manifest rather than left empty, which is
    // what makes the lookup below possible at all.
    assert_eq!(at(&tree, "core").1.peers.len(), 1);
    assert_eq!(report.providers[&id_at(&tree, "core")]["shared"], id_at(&tree, "shared"));
    // One warning, for `core` — `shared` was there to be found.
    match report.issues.as_slice() {
        [PeerIssue::Missing { at, peer, .. }] => {
            assert_eq!(at, "widget");
            assert_eq!(peer, "core");
        }
        other => panic!("expected one Missing, got {other:?}"),
    }

    // (b) Nothing provides it: the inner peer is auto-installed and warned
    // about in its own right.
    let (tree, report) = world.resolve(WIDGET_ROOT);
    assert_eq!(report.stats.auto_installed, 2); // `core`, then `shared`
    assert_eq!(paths(&tree), ["core", "shared", "widget"]);
    assert_eq!(report.providers[&id_at(&tree, "core")]["shared"], id_at(&tree, "shared"));
    let warned: Vec<(&str, &str)> = report
        .issues
        .iter()
        .map(|issue| match issue {
            PeerIssue::Missing { at, peer, auto_installed, .. } => {
                assert!(auto_installed, "{peer} should have been auto-installed");
                (at.as_str(), peer.as_str())
            }
            other => panic!("expected only Missing, got {other:?}"),
        })
        .collect();
    // Innermost first: `match_own_peers` pushes its own `Missing` only after
    // `auto_install` returns, and `auto_install` walks what it installed before
    // returning — so the peer's peer is warned about before the peer is.
    assert_eq!(warned, [("core", "shared"), ("widget", "core")]);
}

#[test]
fn a_cycle_reachable_only_through_an_auto_installed_subtree_terminates() {
    // Criterion 3's other entry point. `a_peer_dependency_cycle_terminates`
    // covers a cycle phase 1 built; this one exists nowhere until the
    // auto-install of `core` drags it in, so it is reached by the recursion
    // inside `auto_install` rather than by the outer descent.
    let world = World::new(&[
        ("widget", packument("widget", &[("1.0.0", peers(&[("core", "^1.0.0")]))])),
        ("core", packument("core", &[("1.0.0", deps(&[("a", "^1.0.0")]))])),
        (
            "a",
            packument(
                "a",
                &[(
                    "1.0.0",
                    json!({
                        "dependencies": object(&[("b", "^1.0.0")]),
                        "peerDependencies": object(&[("shared", "^1.0.0")])
                    }),
                )],
            ),
        ),
        (
            "b",
            packument(
                "b",
                &[(
                    "1.0.0",
                    json!({
                        "dependencies": object(&[("a", "^1.0.0")]),
                        "peerDependencies": object(&[("shared", "^1.0.0")])
                    }),
                )],
            ),
        ),
        ("shared", packument("shared", &[("1.0.0", json!({}))])),
    ]);
    let (tree, report) = world.resolve(WIDGET_ROOT);

    // Reaching this line is most of the criterion; the rest says it terminated
    // for the right reason rather than by running out of something.
    assert_eq!(report.stats.cycle_breaks, 1);
    assert_eq!(paths(&tree), ["a", "b", "core", "shared", "widget"]);
    assert_eq!(report.stats.auto_installed, 2); // `core`, and `shared` for `a`
    assert_eq!(report.stats.instances, 0);

    // The cycle is still a cycle, and both ends got their peer.
    assert_eq!(at(&tree, "a").1.dependencies["b"], id_at(&tree, "b"));
    assert_eq!(at(&tree, "b").1.dependencies["a"], id_at(&tree, "a"));
    let shared = id_at(&tree, "shared");
    assert_eq!(report.providers[&id_at(&tree, "a")]["shared"], shared);
    assert_eq!(report.providers[&id_at(&tree, "b")]["shared"], shared);
}

#[test]
fn a_missing_peer_that_cannot_be_satisfied_warns_without_inventing_a_version() {
    let world = World::new(&[
        ("widget", packument("widget", &[("1.0.0", peers(&[("core", "^9.0.0")]))])),
        ("core", packument("core", &[("1.0.0", json!({}))])),
    ]);
    let (tree, report) = world.resolve(WIDGET_ROOT);

    // Nothing satisfies `^9`, so nothing is installed — an auto-install that
    // planted a version the range rejects would be a silently wrong tree.
    assert_eq!(paths(&tree), ["widget"]);
    assert_eq!(report.stats.auto_installed, 0);
    assert!(report.providers.is_empty());
    match report.issues.as_slice() {
        [PeerIssue::Missing { peer, auto_installed, .. }] => {
            assert_eq!(peer, "core");
            assert!(!auto_installed);
        }
        other => panic!("expected one unsatisfiable Missing, got {other:?}"),
    }
}

#[test]
fn a_missing_peer_marked_optional_is_skipped_silently() {
    let world = World::new(&[(
        "widget",
        packument(
            "widget",
            &[(
                "1.0.0",
                json!({
                    "peerDependencies": object(&[("core", "^1.0.0")]),
                    "peerDependenciesMeta": { "core": { "optional": true } }
                }),
            )],
        ),
    )]);
    let (tree, report) = world.resolve(WIDGET_ROOT);

    // No install, no warning, no provider. `optional` means the package works
    // without it, so there is nothing to tell the user.
    assert_eq!(paths(&tree), ["widget"]);
    assert_eq!(report.stats.auto_installed, 0);
    assert_eq!(report.issues, []);
    assert!(report.providers.is_empty());
}

#[test]
fn a_provider_that_is_present_but_wrong_warns_and_does_not_fail_the_resolve() {
    let world = World::new(&[
        ("widget", packument("widget", &[("1.0.0", peers(&[("core", "^2.0.0")]))])),
        ("core", packument("core", &[("1.0.0", json!({}))])),
    ]);
    let (tree, report) = world.resolve(
        r#"{ "name": "root", "dependencies": { "core": "^1.0.0", "widget": "^1.0.0" } }"#,
    );

    // `core@1.0.0` is in scope and is the only thing there is; no requester
    // has a better one, so there is no instance that would help. npm warns and
    // proceeds, and so does this — a peer mismatch is not a resolve failure.
    assert_eq!(paths(&tree), ["core", "widget"]);
    assert_eq!(report.stats.instances, 0);
    assert!(report.providers.is_empty());
    match report.issues.as_slice() {
        [PeerIssue::Unsatisfied { at, peer, found, .. }] => {
            assert_eq!(at, "widget");
            assert_eq!(peer, "core");
            assert_eq!(found, "1.0.0");
        }
        other => panic!("expected one Unsatisfied, got {other:?}"),
    }
}
