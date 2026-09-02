//! R773-F3's two verification criteria, as tests.
//!
//! 1. On a dependency set with a version conflict, the conflicting package is
//!    nested under its requester and the majority version stays hoisted.
//! 2. Resolution of a package already satisfied by an ancestor issues no
//!    packument fetch.
//!
//! Nothing here touches the network. Criterion 2 is proved at two depths: a
//! [`Counting`] `PackumentSource` records what the *walk* asked for, and
//! `FakeTransport::hits()` records what actually left the process. The first
//! is the load-bearing one — the walk must not even ask — and it is only a
//! real proof because `Resolver` deliberately keeps no packument memo of its
//! own (see its doc comment). A memo there would make this test pass with the
//! ancestor check deleted.

use rnpm::resolve::{
    NodeId, PackumentSource, RegistrySource, ResolvedTree, Resolver, RootManifest,
};
use rnpm::testing::FakeTransport;
use rnpm::{CachePolicy, Node, NodeSource, Packument, RegistryClient, RegistryEndpoint, Version};
use serde_json::{json, Map, Value};
use std::cell::RefCell;

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
    let map: Map<String, Value> = pairs
        .iter()
        .map(|(name, range)| ((*name).to_string(), Value::from(*range)))
        .collect();
    json!({ "dependencies": map })
}

/// Counts what the walk asked for, by name.
struct Counting<S: PackumentSource> {
    inner: S,
    asked: RefCell<Vec<String>>,
}

impl<S: PackumentSource> Counting<S> {
    fn new(inner: S) -> Self {
        Self { inner, asked: RefCell::new(Vec::new()) }
    }

    fn asked_for(&self, name: &str) -> usize {
        self.asked.borrow().iter().filter(|n| *n == name).count()
    }

    fn total(&self) -> usize {
        self.asked.borrow().len()
    }
}

impl<S: PackumentSource> PackumentSource for Counting<S> {
    fn packument(&self, registry_name: &str) -> anyhow::Result<Option<Packument>> {
        self.asked.borrow_mut().push(registry_name.to_string());
        self.inner.packument(registry_name)
    }
}

/// A whole hermetic world: a fake registry, a real `RegistryClient` over it,
/// and the counting source the resolver walks.
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
            // Freshness policy is not what these tests are about; pin it wide
            // so a cache-window change can never quietly alter a fetch count.
            .with_policy(CachePolicy::MaxAge(std::time::Duration::from_secs(3600)));
        Self { _dir: dir, client, endpoint: RegistryEndpoint::new("npm", BASE) }
    }

    fn source(&self) -> Counting<RegistrySource<'_, FakeTransport>> {
        Counting::new(RegistrySource::new(&self.client, &self.endpoint))
    }

    fn resolve(
        &self,
        root_json: &str,
    ) -> (ResolvedTree, Counting<RegistrySource<'_, FakeTransport>>) {
        let source = self.source();
        let manifest = RootManifest::from_package_json(root_json).expect("root manifest");
        let tree = Resolver::new(&source).resolve(&manifest).expect("resolve");
        (tree, source)
    }

    fn try_resolve(&self, root_json: &str) -> anyhow::Result<ResolvedTree> {
        let source = self.source();
        let manifest = RootManifest::from_package_json(root_json)?;
        Resolver::new(&source).resolve(&manifest)
    }

    /// Requests that actually left the process.
    fn hits(&self) -> usize {
        self.client.transport().hits()
    }
}

/// Every install path in the tree, sorted — the arena's insertion order is an
/// implementation detail and no assertion should depend on it.
fn paths(tree: &ResolvedTree) -> Vec<String> {
    let mut out: Vec<String> = tree.packages().map(|(id, _)| tree.install_path(id)).collect();
    out.sort();
    out
}

/// The single node at a given install path, e.g. `"c/shared"`.
fn at<'t>(tree: &'t ResolvedTree, path: &str) -> (NodeId, &'t Node) {
    tree.packages()
        .find(|(id, _)| tree.install_path(*id) == path)
        .unwrap_or_else(|| panic!("no node at {path:?}; tree has {:?}", paths(tree)))
}

fn version_at(tree: &ResolvedTree, path: &str) -> String {
    at(tree, path)
        .1
        .version()
        .unwrap_or_else(|| panic!("{path} has no version"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Criterion 1 — a conflict nests, the majority stays hoisted
// ---------------------------------------------------------------------------

/// Three requesters of `shared`: `a` and `b` want `^1`, `c` wants `^2`.
///
/// The two placements have to be genuinely distinguishable, so this is a
/// *two-plus-a-dissenter* rather than a two-way tie: one copy stays hoisted and
/// the other lands under the exact package that disagreed. A resolver that
/// nested both, hoisted both, or got the two the wrong way round fails a
/// different assertion each.
///
/// The hoisted version here is both the majority choice and the first-walked
/// requester's choice — see
/// [`the_first_walked_requester_owns_the_hoisted_slot_not_the_majority`] for
/// which of those two the code actually implements.
fn conflict_world() -> World {
    World::new(&[
        ("a", packument("a", &[("1.0.0", deps(&[("shared", "^1.0.0")]))])),
        ("b", packument("b", &[("1.0.0", deps(&[("shared", "^1.0.0")]))])),
        ("c", packument("c", &[("1.0.0", deps(&[("shared", "^2.0.0")]))])),
        (
            "shared",
            packument(
                "shared",
                &[("1.0.0", json!({})), ("1.5.0", json!({})), ("2.0.0", json!({}))],
            ),
        ),
    ])
}

const CONFLICT_ROOT: &str = r#"{
  "name": "root",
  "dependencies": { "a": "^1.0.0", "b": "^1.0.0", "c": "^1.0.0" }
}"#;

#[test]
fn a_version_conflict_nests_under_its_requester_and_leaves_the_other_hoisted() {
    let world = conflict_world();
    let (tree, _) = world.resolve(CONFLICT_ROOT);

    // The `^1` version is hoisted to the root, where `a` and `b` see it.
    assert_eq!(version_at(&tree, "shared"), "1.5.0");
    // The dissenter's copy is nested under the dissenter, and nowhere else.
    assert_eq!(version_at(&tree, "c/shared"), "2.0.0");
    assert_eq!(paths(&tree), ["a", "b", "c", "c/shared", "shared"]);
}

#[test]
fn each_requester_resolves_to_the_copy_it_asked_for() {
    let world = conflict_world();
    let (tree, _) = world.resolve(CONFLICT_ROOT);

    let hoisted = at(&tree, "shared").0;
    let nested = at(&tree, "c/shared").0;
    assert_ne!(hoisted, nested);

    for requester in ["a", "b"] {
        let (id, node) = at(&tree, requester);
        assert_eq!(node.dependencies["shared"], hoisted, "{requester}");
        // Node's own resolution algorithm agrees with the logical edge, which
        // is the property that makes this tree materializable at all.
        assert_eq!(tree.resolve_from(id, "shared"), Some(hoisted), "{requester}");
    }

    let (c, c_node) = at(&tree, "c");
    assert_eq!(c_node.dependencies["shared"], nested);
    assert_eq!(tree.resolve_from(c, "shared"), Some(nested));

    // The nested copy shadows the hoisted one for `c` only, not for the root.
    assert_eq!(tree.resolve_from(ResolvedTree::ROOT, "shared"), Some(hoisted));
}

/// The mirror image of [`conflict_world`]: the dissenter nests whichever
/// version it happens to hold, so a rule that merely prefers lower (or higher)
/// versions fails here.
///
/// Note what this does *not* prove. In this fixture, and in
/// [`conflict_world`], the hoisted version is both the majority choice and the
/// one the first-walked requester asked for — `a` sorts first in the root's
/// `BTreeMap`, so it is the first BFS edge. Those two explanations are
/// perfectly confounded here, and
/// [`the_first_walked_requester_owns_the_hoisted_slot_not_the_majority`]
/// separates them. The real rule is walk order, which is npm's and bun's.
#[test]
fn a_dissenter_nests_whichever_version_it_holds() {
    let world = World::new(&[
        ("a", packument("a", &[("1.0.0", deps(&[("shared", "^2.0.0")]))])),
        ("b", packument("b", &[("1.0.0", deps(&[("shared", "^2.0.0")]))])),
        ("c", packument("c", &[("1.0.0", deps(&[("shared", "^1.0.0")]))])),
        (
            "shared",
            packument("shared", &[("1.0.0", json!({})), ("2.0.0", json!({}))]),
        ),
    ]);
    let (tree, _) = world.resolve(CONFLICT_ROOT);

    assert_eq!(version_at(&tree, "shared"), "2.0.0");
    assert_eq!(version_at(&tree, "c/shared"), "1.0.0");
}

/// The fixture that separates "the majority wins" from "the first requester in
/// walk order wins": `a` alone wants `^2`, while `b` and `c` both want `^1`.
///
/// The majority is `^1`. The winner is `^2`, because `a` is the first edge the
/// BFS walks and hoisting is first-come — which is npm's and bun's actual rule,
/// and is why no vote is counted anywhere in `place`. The two `^1` requesters
/// each get their own nested copy; note there are now **two** copies of the
/// same version, which is the visible cost of first-come hoisting and exactly
/// what a majority rule would have avoided.
#[test]
fn the_first_walked_requester_owns_the_hoisted_slot_not_the_majority() {
    let world = World::new(&[
        ("a", packument("a", &[("1.0.0", deps(&[("shared", "^2.0.0")]))])),
        ("b", packument("b", &[("1.0.0", deps(&[("shared", "^1.0.0")]))])),
        ("c", packument("c", &[("1.0.0", deps(&[("shared", "^1.0.0")]))])),
        (
            "shared",
            packument(
                "shared",
                &[("1.0.0", json!({})), ("1.5.0", json!({})), ("2.0.0", json!({}))],
            ),
        ),
    ]);
    let (tree, _) = world.resolve(CONFLICT_ROOT);

    assert_eq!(
        version_at(&tree, "shared"),
        "2.0.0",
        "the minority version `a` asked for should hold the hoisted slot"
    );
    assert_eq!(version_at(&tree, "b/shared"), "1.5.0");
    assert_eq!(version_at(&tree, "c/shared"), "1.5.0");
    assert_eq!(
        paths(&tree),
        ["a", "b", "b/shared", "c", "c/shared", "shared"]
    );
}

/// A conflict two levels down nests at the level that disagreed — not at the
/// root, and not so deep that the disagreeing package cannot see it.
#[test]
fn a_deep_conflict_nests_at_the_level_that_disagreed() {
    let world = World::new(&[
        ("top", packument("top", &[("1.0.0", deps(&[("mid", "^1.0.0")]))])),
        ("mid", packument("mid", &[("1.0.0", deps(&[("shared", "^2.0.0")]))])),
        (
            "shared",
            packument("shared", &[("1.0.0", json!({})), ("2.0.0", json!({}))]),
        ),
    ]);
    let (tree, _) = world.resolve(
        r#"{ "name": "root", "dependencies": { "top": "^1.0.0", "shared": "^1.0.0" } }"#,
    );

    // The root asked for ^1, so ^1 owns the hoisted slot and `mid` gets its own.
    assert_eq!(version_at(&tree, "shared"), "1.0.0");
    assert_eq!(version_at(&tree, "mid/shared"), "2.0.0");
    // `mid` itself still hoisted all the way to the root: only the conflicting
    // package nests, not its requester's whole subtree.
    assert_eq!(paths(&tree), ["mid", "mid/shared", "shared", "top"]);
}

/// No backtracking: nothing is ever unplaced or re-parented, so the same input
/// resolves identically every time.
#[test]
fn resolution_is_deterministic_and_never_unplaces_a_node() {
    let world = conflict_world();
    let (first, _) = world.resolve(CONFLICT_ROOT);
    let (second, _) = world.resolve(CONFLICT_ROOT);

    let shape = |tree: &ResolvedTree| -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = tree
            .packages()
            .map(|(id, node)| {
                (
                    tree.install_path(id),
                    node.version().map(|v| v.to_string()).unwrap_or_default(),
                )
            })
            .collect();
        out.sort();
        out
    };
    assert_eq!(shape(&first), shape(&second));
}

// ---------------------------------------------------------------------------
// Criterion 2 — an ancestor-satisfied dependency issues no fetch
// ---------------------------------------------------------------------------

#[test]
fn a_dependency_already_satisfied_by_an_ancestor_issues_no_packument_fetch() {
    let world = World::new(&[
        ("a", packument("a", &[("1.0.0", deps(&[("shared", "^1.0.0")]))])),
        ("b", packument("b", &[("1.0.0", deps(&[("shared", "^1.0.0")]))])),
        ("shared", packument("shared", &[("1.0.0", json!({}))])),
    ]);
    let (tree, source) =
        world.resolve(r#"{ "name": "root", "dependencies": { "a": "^1.0.0", "b": "^1.0.0" } }"#);

    assert_eq!(version_at(&tree, "shared"), "1.0.0");

    // `a` triggered the one lookup; by the time `b`'s edge is walked, `shared`
    // is hoisted to the root and visible from `b`, so the walk does not ask.
    assert_eq!(source.asked_for("shared"), 1, "the walk asked for shared twice");
    assert_eq!(source.total(), 3, "one lookup each for a, b and shared");
    assert_eq!(world.hits(), 3, "and one network request each");
}

/// The same property down a chain rather than across siblings: four packages
/// all requiring `dep@^1`, at four different depths.
#[test]
fn an_ancestor_satisfies_a_dependency_at_any_depth() {
    let world = World::new(&[
        (
            "a",
            packument("a", &[("1.0.0", deps(&[("b", "^1.0.0"), ("dep", "^1.0.0")]))]),
        ),
        (
            "b",
            packument("b", &[("1.0.0", deps(&[("c", "^1.0.0"), ("dep", "^1.0.0")]))]),
        ),
        ("c", packument("c", &[("1.0.0", deps(&[("dep", "^1.0.0")]))])),
        ("dep", packument("dep", &[("1.0.0", json!({}))])),
    ]);
    let (tree, source) =
        world.resolve(r#"{ "name": "root", "dependencies": { "a": "^1.0.0", "dep": "^1.0.0" } }"#);

    assert_eq!(source.asked_for("dep"), 1);
    assert_eq!(source.total(), 4, "a, b, c and dep, once each");
    // Everything hoists flat, because nothing conflicts.
    assert_eq!(paths(&tree), ["a", "b", "c", "dep"]);
}

/// A dependency cycle terminates, and terminates through the ancestor check
/// rather than through a visited-set hack: `a` needs `b`, `b` needs `a`, and
/// `b`'s edge back to `a` is answered by the already-placed node.
#[test]
fn a_dependency_cycle_terminates_through_the_ancestor_check() {
    let world = World::new(&[
        ("a", packument("a", &[("1.0.0", deps(&[("b", "^1.0.0")]))])),
        ("b", packument("b", &[("1.0.0", deps(&[("a", "^1.0.0")]))])),
    ]);
    let (tree, source) = world.resolve(r#"{ "name": "root", "dependencies": { "a": "^1.0.0" } }"#);

    assert_eq!(source.total(), 2, "each package looked up exactly once");
    assert_eq!(paths(&tree), ["a", "b"]);
    // The cycle is a real edge in the graph, not a dropped one.
    assert_eq!(at(&tree, "b").1.dependencies["a"], at(&tree, "a").0);
}

/// An *unsatisfying* ancestor does not suppress the lookup — the check is
/// "already satisfied", not "already present".
#[test]
fn an_incompatible_ancestor_does_not_suppress_the_lookup() {
    let world = conflict_world();
    let (_, source) = world.resolve(CONFLICT_ROOT);

    // `a` looked it up; `b` was satisfied and did not; `c` wanted ^2 and had to.
    assert_eq!(source.asked_for("shared"), 2);
    assert_eq!(source.total(), 5, "a, b, c, and shared twice");
    // Still only four network requests: the second `shared` lookup was served
    // by the R773-F1 disk cache, which is the layer allowed to memoize.
    assert_eq!(world.hits(), 4);
}

// ---------------------------------------------------------------------------
// Peers are recorded, never followed
// ---------------------------------------------------------------------------

#[test]
fn peer_dependencies_are_recorded_as_data_and_never_walked() {
    let world = World::new(&[
        (
            "widget",
            packument(
                "widget",
                &[(
                    "1.0.0",
                    json!({
                        "peerDependencies": { "react": ">=18", "react-dom": ">=18" },
                        "peerDependenciesMeta": { "react-dom": { "optional": true } }
                    }),
                )],
            ),
        ),
        // Both are published, so a walk that followed peers would find them
        // and these assertions would notice.
        ("react", packument("react", &[("18.3.1", json!({}))])),
        ("react-dom", packument("react-dom", &[("18.3.1", json!({}))])),
    ]);
    let (tree, source) =
        world.resolve(r#"{ "name": "root", "dependencies": { "widget": "^1.0.0" } }"#);

    let (_, widget) = at(&tree, "widget");
    assert_eq!(widget.peers.len(), 2);
    assert!(!widget.peers["react"].optional);
    assert!(widget.peers["react-dom"].optional);
    assert!(widget.peers["react"]
        .range
        .matches(&Version::parse("18.3.1").unwrap()));
    assert!(!widget.peers["react"]
        .range
        .matches(&Version::parse("17.0.0").unwrap()));

    // The whole point: nothing was installed for them and nothing was fetched.
    assert_eq!(paths(&tree), ["widget"]);
    assert_eq!(source.asked_for("react"), 0);
    assert_eq!(source.asked_for("react-dom"), 0);
    assert_eq!(source.total(), 1);
}

// ---------------------------------------------------------------------------
// Version selection and the rest of the surface
// ---------------------------------------------------------------------------

#[test]
fn the_highest_satisfying_version_is_taken_not_the_first_or_the_newest() {
    let world = World::new(&[(
        "pkg",
        packument(
            "pkg",
            &[
                ("1.0.0", json!({})),
                ("1.9.3", json!({})),
                ("2.0.0", json!({})),
                ("1.10.0", json!({})),
            ],
        ),
    )]);
    let (tree, _) = world.resolve(r#"{ "name": "root", "dependencies": { "pkg": "^1.0.0" } }"#);

    // 1.10.0 beats 1.9.3 by semver ordering, not by the lexicographic order of
    // the packument's version map — and 2.0.0 is out of range.
    assert_eq!(version_at(&tree, "pkg"), "1.10.0");
}

#[test]
fn a_resolved_node_carries_the_pair_the_store_consumes() {
    let world = World::new(&[("pkg", packument("pkg", &[("1.2.3", json!({}))]))]);
    let (tree, _) = world.resolve(r#"{ "name": "root", "dependencies": { "pkg": "^1.0.0" } }"#);

    let resolution = at(&tree, "pkg").1.resolution().expect("a registry node");
    assert_eq!(resolution.name, "pkg");
    assert_eq!(resolution.version.to_string(), "1.2.3");
    assert_eq!(resolution.tarball, format!("{BASE}/pkg/-/pkg-1.2.3.tgz"));
    assert_eq!(resolution.integrity.as_deref(), Some("sha512-pkg-1.2.3"));
}

#[test]
fn a_dist_tag_resolves_through_the_packument_that_defines_it() {
    let world = World::new(&[(
        "pkg",
        packument("pkg", &[("1.0.0", json!({})), ("2.0.0", json!({}))]),
    )]);
    let (tree, _) = world.resolve(r#"{ "name": "root", "dependencies": { "pkg": "latest" } }"#);
    assert_eq!(version_at(&tree, "pkg"), "2.0.0");
}

#[test]
fn an_alias_installs_the_real_package_under_the_requested_name() {
    let world = World::new(&[("real", packument("real", &[("1.0.0", json!({}))]))]);
    let (tree, source) =
        world.resolve(r#"{ "name": "root", "dependencies": { "alias": "npm:real@^1.0.0" } }"#);

    // The directory is the manifest key; the resolution names the registry
    // package. Both matter: the first is where Node looks, the second is what
    // gets fetched.
    let (_, node) = at(&tree, "alias");
    assert_eq!(node.name, "alias");
    assert_eq!(node.resolution().unwrap().name, "real");
    assert_eq!(source.asked_for("real"), 1);
    assert_eq!(source.asked_for("alias"), 0);
}

#[test]
fn a_file_dependency_is_recorded_where_it_was_asked_for_and_never_fetched() {
    let world = World::new(&[("pkg", packument("pkg", &[("1.0.0", json!({}))]))]);
    let (tree, source) = world.resolve(
        r#"{ "name": "root",
             "dependencies": { "pkg": "^1.0.0", "runtime": "file:../runtime" } }"#,
    );

    let (_, runtime) = at(&tree, "runtime");
    assert!(matches!(runtime.source, NodeSource::Other(_)));
    assert!(runtime.resolution().is_none());
    assert_eq!(source.asked_for("runtime"), 0);
    assert_eq!(source.total(), 1);
    assert_eq!(paths(&tree), ["pkg", "runtime"]);
}

#[test]
fn dev_dependencies_are_followed_for_the_root_and_not_for_a_dependency() {
    let world = World::new(&[
        (
            "pkg",
            packument(
                "pkg",
                &[("1.0.0", json!({ "devDependencies": { "never": "^1.0.0" } }))],
            ),
        ),
        ("tool", packument("tool", &[("1.0.0", json!({}))])),
        ("never", packument("never", &[("1.0.0", json!({}))])),
    ]);
    let (tree, source) = world.resolve(
        r#"{ "name": "root",
             "dependencies": { "pkg": "^1.0.0" },
             "devDependencies": { "tool": "^1.0.0" } }"#,
    );

    assert_eq!(paths(&tree), ["pkg", "tool"]);
    assert_eq!(
        source.asked_for("never"),
        0,
        "a dependency's devDependencies were walked"
    );
}

#[test]
fn a_missing_optional_dependency_is_skipped_and_a_missing_required_one_is_not() {
    let world = World::new(&[("pkg", packument("pkg", &[("1.0.0", json!({}))]))]);

    let (tree, _) = world.resolve(
        r#"{ "name": "root",
             "dependencies": { "pkg": "^1.0.0" },
             "optionalDependencies": { "absent": "^1.0.0" } }"#,
    );
    assert_eq!(paths(&tree), ["pkg"]);

    let err = world
        .try_resolve(r#"{ "name": "root", "dependencies": { "absent": "^1.0.0" } }"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("absent"), "{err}");
}

#[test]
fn a_range_no_published_version_satisfies_fails_naming_the_package_and_the_requester() {
    let world = World::new(&[("pkg", packument("pkg", &[("1.0.0", json!({}))]))]);
    let err = world
        .try_resolve(r#"{ "name": "root", "dependencies": { "pkg": "^9.0.0" } }"#)
        .unwrap_err()
        .to_string();

    assert!(err.contains("pkg"), "{err}");
    assert!(err.contains('9'), "{err}");
    assert!(err.contains("root manifest"), "{err}");
}

#[test]
fn install_paths_are_bun_lock_key_shaped() {
    let world = conflict_world();
    let (tree, _) = world.resolve(CONFLICT_ROOT);

    // The chain of install names joined with `/` — exactly the key form
    // `bun.lock` uses, so R773-T5 needs no path computation of its own.
    let nested = at(&tree, "c/shared").0;
    assert_eq!(tree.path(nested), ["c", "shared"]);
    assert_eq!(tree.install_path(ResolvedTree::ROOT), "");
}
