//! The resolver's tree, flattened into the R771-T5 lock seam (R773-T5, W318 §6
//! phase 3).
//!
//! [`crate::resolve`] does not exist and never will: resolution lives in
//! `rnpm`, and this module is the *only* thing that knows both vocabularies. It
//! reads a finished [`rnpm::ResolvedTree`] and produces [`LockEntry`] values,
//! which [`crate::lock::BunLockWriter`] renders. There is no second lock
//! renderer here and there must not be one — the writer at
//! [`crate::lock`] is the single renderer, and this module produces its input.
//!
//! # Why the adapter is on this side
//!
//! `rnpm` may not depend on `mesofact-build` (`rnpm/src/lib.rs`, and the
//! workspace manifest says the same): `mesofact-build` consumes the resolver,
//! so the reverse edge is a cycle. That fixes the direction — the crate that
//! owns [`PackageSource`] is the crate that adapts to it.
//!
//! # What the tree already decided
//!
//! Nothing here computes an install path. [`rnpm::ResolvedTree::install_path`]
//! is already bun.lock's key form (the chain of install names joined with `/`),
//! which is why a peer instance nested under its requester needs no special
//! case: it is a longer key and nothing else.
//!
//! # What travels besides identity (R773-F9)
//!
//! A node's `os` / `cpu` / `libc` and whether it is optional. Both are pure
//! pass-through — `rnpm` decides them, this maps them onto
//! [`PackageSource::Registry`], [`crate::lock`] renders them and
//! [`crate::install`] acts on them. The reason they exist at all is that
//! resolution is host-independent: the tree contains every platform's build of
//! a native package, so the lock has to say which is which, and the install
//! has to know whether a package it cannot run is one it may skip.
//!
//! # Two narrowings, both deliberate refusals
//!
//! [`rnpm::Resolution::integrity`] is optional and [`PackageSource::Registry`]'s
//! is not. That gap is the whole difference between "phase 1 records rather
//! than judges" and "a materializer must be able to fetch this": the SRI is
//! both the verification and the store key (W319 §2), so an entry without one
//! is unfetchable *and* unaddressable. A node missing it is a hard error naming
//! the package and version — never an empty string, which would render a lock
//! our own reader refuses (`install.rs`'s bun parser) one layer later and much
//! less legibly.
//!
//! [`rnpm::NodeSource::Other`] carries any non-registry spec. `file:` maps onto
//! [`PackageSource::Link`]; git and tarball-URL specs have no `PackageSource`
//! spelling at all, so they are refused by name and protocol rather than
//! silently dropped from the lock. (`workspace:`, `link:` and `catalog:` never
//! reach here — `rnpm`'s spec grammar refuses an unknown scheme at parse time,
//! and `install.rs:270` refuses the same two on the read side.)
//!
//! # A `file:` path is recorded verbatim
//!
//! bun.lock's `file:` target is relative to the *project root*, and a `file:`
//! dependency declared by the root package is written relative to that same
//! root — so the manifest string passes through unchanged. A `file:` dep
//! declared by a *nested* package would be relative to that package instead,
//! and this does not rebase it; bun.lock has no way to say "relative to the
//! requester", so there is nothing to rebase it *to*. Stated rather than
//! guarded because the alternative is refusing an install npm performs.

use anyhow::{bail, Result};
use rnpm::{NodeSource, PackageSpec, ResolvedTree};

use crate::install::{PackageSource, PlatformGates};
use crate::lock::{BunLockWriter, LockEntry, LockWriter};

/// Flatten a resolved tree into the lock seam's entries, one per node.
///
/// The root is excluded — it is not a package in `node_modules`, and its name
/// belongs in [`crate::lock::BunLockWriter::root_name`] instead.
///
/// Order follows the tree's arena, which is deterministic for a given input;
/// the writer sorts into a `BTreeMap` regardless, so the rendered lock is a
/// function of the tree and not of this iteration.
pub fn entries_from_tree(tree: &ResolvedTree) -> Result<Vec<LockEntry>> {
    let mut entries = Vec::with_capacity(tree.len().saturating_sub(1));
    for (id, node) in tree.packages() {
        let path = tree.install_path(id);
        let source = match &node.source {
            NodeSource::Registry(resolution) => {
                let Some(integrity) = resolution.integrity.clone() else {
                    bail!(
                        "{}@{} (at {path}) has no sha512 integrity — refusing to write a lock \
                         entry that can be neither verified nor addressed (W319 §2)",
                        resolution.name,
                        resolution.version
                    );
                };
                PackageSource::Registry {
                    // The *registry* name, which is not the install name under
                    // an alias: `"foo": "npm:bar@^1"` installs at `foo` and
                    // locks `bar`. The path already carries the install name.
                    name: resolution.name.clone(),
                    version: resolution.version.to_string(),
                    integrity,
                    // Straight through from the packument (R773-F9). The
                    // resolver records these and gates on nothing; carrying
                    // them here is what makes the written lock portable and
                    // what gives the materializer something to filter on.
                    platform: PlatformGates {
                        os: resolution.os.clone(),
                        cpu: resolution.cpu.clone(),
                        libc: resolution.libc.clone(),
                    },
                    // Known, so `Some`: the tree computed it over the whole
                    // graph. A lock bun wrote says nothing here, which is why
                    // the field is three-valued at all.
                    optional: Some(node.optional),
                }
            }
            NodeSource::Other(spec) => match spec.target() {
                PackageSpec::Dir { path: target_rel } => {
                    PackageSource::Link { target_rel: target_rel.clone() }
                }
                other => bail!(
                    "{} (at {path}) is a {} dependency, which has no bun.lock representation \
                     the Rust-native installer can materialize (file: and registry only)",
                    node.name,
                    protocol_of(other)
                ),
            },
            // Unreachable: `packages()` skips node 0, the only Root node. A
            // bail rather than an `unreachable!` so a future change to that
            // iterator surfaces as an error and not as a panic in a build.
            NodeSource::Root => bail!("the root node reached the lock seam as a package"),
        };
        entries.push(LockEntry { path, source });
    }
    Ok(entries)
}

/// The whole seam, as bytes: a resolved tree in, one `bun.lock` document out.
///
/// Pure — no filesystem, no clock, no network — so "the same tree renders the
/// same lock" is a property a hermetic test can assert directly, and
/// [`crate::project_lock`] is left holding nothing but the IO.
///
/// The root name comes off the tree's own root node, which is where
/// [`rnpm::RootManifest`]'s `name` ended up; reading it from `package.json` a
/// second time here would be a second source of truth for one string.
pub fn render_lock(tree: &ResolvedTree) -> Result<String> {
    let writer =
        BunLockWriter { root_name: tree.node(ResolvedTree::ROOT).name.clone() };
    writer.render(&entries_from_tree(tree)?)
}

/// How to name a spec in a refusal. `Alias` and `Dir` are unreachable through
/// [`PackageSpec::target`] + the `Dir` arm above; they are spelled out because
/// the match is exhaustive, not because they occur.
fn protocol_of(spec: &PackageSpec) -> &'static str {
    match spec {
        PackageSpec::Git(_) => "git",
        PackageSpec::Tarball { .. } => "tarball-URL",
        PackageSpec::Npm { .. } => "registry",
        PackageSpec::Alias { .. } => "aliased",
        PackageSpec::Dir { .. } => "file:",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::parse_bun_lock;
    use crate::lock::{write_lock, BunLockWriter, LockWriter};
    use rnpm::testing::FakeTransport;
    use rnpm::{
        CachePolicy, RegistryClient, RegistryEndpoint, RegistrySource, Resolver, RootManifest,
    };
    use serde_json::{json, Map, Value};
    use std::path::PathBuf;

    const BASE: &str = "https://registry.test";

    /// An abbreviated packument, mirroring `rnpm/tests/resolve_tree.rs`'s
    /// helper. `extra` is merged over the generated `version`/`dist` pair, so a
    /// row can add `dependencies` — or replace `dist` with one carrying no
    /// integrity.
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

    /// A hermetic registry plus a resolver over it. Held in a struct because
    /// the tree borrows the client.
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

        fn resolve(&self, root_json: &str) -> rnpm::ResolvedTree {
            self.resolve_preferring(root_json, &rnpm::PreferredVersions::new())
        }

        fn resolve_preferring(
            &self,
            root_json: &str,
            preferred: &rnpm::PreferredVersions,
        ) -> rnpm::ResolvedTree {
            let source = RegistrySource::new(&self.client, &self.endpoint);
            let manifest = RootManifest::from_package_json(root_json).expect("root manifest");
            Resolver::new(&source)
                .preferring(preferred.clone())
                .resolve(&manifest)
                .expect("resolve")
        }
    }

    /// Three requesters of `shared` where one disagrees, so the tree really
    /// does contain two versions of one package — the nested-instance shape the
    /// whole format choice exists for.
    fn conflict_world() -> World {
        World::new(&[
            (
                "a",
                packument("a", &[("1.0.0", deps(&[("shared", "^1.0.0")]))]),
            ),
            (
                "b",
                packument("b", &[("1.0.0", deps(&[("shared", "^1.0.0")]))]),
            ),
            (
                "c",
                packument("c", &[("1.0.0", deps(&[("shared", "^2.0.0")]))]),
            ),
            (
                "shared",
                packument(
                    "shared",
                    &[("1.0.0", json!({})), ("1.5.0", json!({})), ("2.0.0", json!({}))],
                ),
            ),
        ])
    }

    const CONFLICT_ROOT: &str = r#"{ "name": "root",
         "dependencies": { "a": "^1.0.0", "b": "^1.0.0", "c": "^1.0.0" } }"#;

    fn entry_at<'e>(entries: &'e [LockEntry], path: &str) -> &'e LockEntry {
        entries
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("no entry at {path:?}; got {:?}", paths(entries)))
    }

    fn paths(entries: &[LockEntry]) -> Vec<&str> {
        let mut out: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        out.sort_unstable();
        out
    }

    /// R773-T5's headline criterion: a tree the resolver decided, written as a
    /// lock and read back by the materializer's own parser, describes the same
    /// packages at the same paths — nothing is lost on the way through.
    ///
    /// That the *re-resolve* is a no-op is a second property and needs a moved
    /// registry to mean anything; see
    /// [`a_re_resolve_after_the_registry_moves_rewrites_the_same_lock`].
    #[test]
    fn a_resolved_tree_round_trips_through_bun_lock() {
        let world = conflict_world();
        let tree = world.resolve(CONFLICT_ROOT);
        let entries = entries_from_tree(&tree).expect("entries");

        // The conflict really nested, or this test proves nothing about the
        // shape the format was chosen for.
        assert_eq!(paths(&entries), ["a", "b", "c", "c/shared", "shared"]);

        let tmp = tempfile::tempdir().unwrap();
        let lock =
            write_lock(&BunLockWriter { root_name: "root".into() }, tmp.path(), &entries).unwrap();
        let read = parse_bun_lock(&lock).expect("the lock we wrote parses");

        assert_eq!(read.len(), entries.len());
        for (id, node) in tree.packages() {
            let install_path = tree.install_path(id);
            let got = read
                .iter()
                .find(|p| p.key == install_path)
                .unwrap_or_else(|| panic!("{install_path:?} did not survive the round trip"));
            let resolution = node.resolution().expect("every node here is a registry node");
            let PackageSource::Registry { name, version, integrity, .. } = &got.source else {
                panic!("{install_path} came back as a link");
            };
            assert_eq!(name.as_str(), resolution.name, "{install_path}");
            assert_eq!(version.as_str(), resolution.version.to_string(), "{install_path}");
            assert_eq!(Some(integrity.clone()), resolution.integrity, "{install_path}");
        }

        // The two copies are genuinely different versions at different paths —
        // the property a lockfile format that keyed on package name could not
        // express.
        let nested = read.iter().find(|p| p.key == "c/shared").unwrap();
        let hoisted = read.iter().find(|p| p.key == "shared").unwrap();
        assert_ne!(nested.source, hoisted.source);
        assert_eq!(
            nested.dest_rel,
            std::path::Path::new("node_modules/c/node_modules/shared")
        );
    }

    /// The whole R773-F9 chain, end to end: a package this machine may not be
    /// able to run is resolved, flattened, rendered and read back with its
    /// `os`/`cpu`/`libc` and its optionality intact.
    ///
    /// Read against an admit-everything host on purpose. Reading it as *this*
    /// machine is what an install does, and then two of these three entries are
    /// supposed to vanish — which would make this test assert the opposite of
    /// what it is for.
    #[test]
    fn platform_gates_and_optionality_reach_the_lock_from_the_tree() {
        let world = World::new(&[
            ("app", packument("app", &[("1.0.0", json!({}))])),
            (
                "native-darwin",
                packument("native-darwin", &[("1.0.0", json!({ "os": ["darwin"] }))]),
            ),
            (
                "native-musl",
                packument(
                    "native-musl",
                    &[("1.0.0", json!({ "os": ["linux"], "cpu": ["x64"], "libc": ["musl"] }))],
                ),
            ),
        ]);
        let tree = world.resolve(
            r#"{ "name": "root",
                 "dependencies": { "app": "^1.0.0" },
                 "optionalDependencies": {
                   "native-darwin": "^1.0.0",
                   "native-musl": "^1.0.0"
                 } }"#,
        );
        let entries = entries_from_tree(&tree).expect("entries");

        // Every platform's build is in the lock, which is the portability the
        // ticket is about — a host-specific resolve would have written one.
        assert_eq!(paths(&entries), ["app", "native-darwin", "native-musl"]);
        assert_eq!(
            entry_at(&entries, "native-musl").source,
            PackageSource::Registry {
                name: "native-musl".into(),
                version: "1.0.0".into(),
                integrity: "sha512-native-musl-1.0.0".into(),
                platform: PlatformGates {
                    os: vec!["linux".into()],
                    cpu: vec!["x64".into()],
                    libc: vec!["musl".into()],
                },
                optional: Some(true),
            }
        );
        // The required one says so, which is what turns a mismatch into an
        // EBADPLATFORM instead of a silent omission.
        assert_eq!(
            entry_at(&entries, "app").source,
            PackageSource::Registry {
                name: "app".into(),
                version: "1.0.0".into(),
                integrity: "sha512-app-1.0.0".into(),
                platform: PlatformGates::default(),
                optional: Some(false),
            }
        );

        let tmp = tempfile::tempdir().unwrap();
        let lock =
            write_lock(&BunLockWriter { root_name: "root".into() }, tmp.path(), &entries).unwrap();
        let admit_everything = rnpm::Host { os: None, cpu: None, libc: None };
        let read = crate::install::parse_bun_lock_for_host(&lock, &admit_everything).unwrap();
        assert_eq!(read.len(), 3);
        for entry in &entries {
            let got = read.iter().find(|p| p.key == entry.path).expect("round trip");
            assert_eq!(got.source, entry.source, "{}", entry.path);
        }
    }

    /// An alias installs at one name and locks another. The path is the install
    /// name; the locator names the registry package, or the installer fetches a
    /// package that does not exist.
    #[test]
    fn an_alias_locks_the_registry_package_at_the_install_path() {
        let world = World::new(&[("real", packument("real", &[("1.2.3", json!({}))]))]);
        let tree = world.resolve(
            r#"{ "name": "root", "dependencies": { "pretend": "npm:real@^1.0.0" } }"#,
        );
        let entries = entries_from_tree(&tree).expect("entries");

        assert_eq!(paths(&entries), ["pretend"]);
        assert_eq!(
            entry_at(&entries, "pretend").source,
            PackageSource::Registry {
                name: "real".into(),
                version: "1.2.3".into(),
                integrity: "sha512-real-1.2.3".into(),
                platform: PlatformGates::default(),
                optional: Some(false),
            }
        );

        // And it survives rendering: the key is the install name, the locator
        // the registry one.
        let rendered = BunLockWriter { root_name: "root".into() }.render(&entries).unwrap();
        let doc: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(doc["packages"]["pretend"][0], json!("real@1.2.3"));
    }

    /// A pre-SRI publish is refused by name rather than locked with an empty
    /// integrity — the narrowing this module's header describes.
    #[test]
    fn a_node_without_integrity_is_refused_by_name() {
        let world = World::new(&[(
            "ancient",
            packument(
                "ancient",
                &[(
                    "0.0.1",
                    json!({ "dist": { "tarball": format!("{BASE}/ancient.tgz") } }),
                )],
            ),
        )]);
        let tree = world.resolve(r#"{ "name": "root", "dependencies": { "ancient": "0.0.1" } }"#);

        let err = entries_from_tree(&tree).unwrap_err().to_string();
        assert!(err.contains("ancient"), "{err}");
        assert!(err.contains("0.0.1"), "{err}");
        assert!(err.contains("integrity"), "{err}");
    }

    /// `file:` is the one non-registry source the materializer can express.
    #[test]
    fn a_file_dependency_becomes_a_link() {
        let world = World::new(&[]);
        let tree =
            world.resolve(r#"{ "name": "root", "dependencies": { "shared": "file:packages/shared" } }"#);
        let entries = entries_from_tree(&tree).expect("entries");

        assert_eq!(
            entry_at(&entries, "shared").source,
            PackageSource::Link { target_rel: PathBuf::from("packages/shared") }
        );
    }

    // ── R773-T5's second criterion: re-resolving is a no-op ─────────────────

    /// The same four packages, after the registry has moved: `a` published a
    /// new minor, and `shared` published a new version on each of its two
    /// majors. Every one of those is inside a range this manifest already
    /// admits, so a resolver with no memory would pick all three up.
    fn moved_on_world() -> World {
        World::new(&[
            (
                "a",
                packument(
                    "a",
                    &[
                        ("1.0.0", deps(&[("shared", "^1.0.0")])),
                        ("1.1.0", deps(&[("shared", "^1.0.0")])),
                    ],
                ),
            ),
            ("b", packument("b", &[("1.0.0", deps(&[("shared", "^1.0.0")]))])),
            ("c", packument("c", &[("1.0.0", deps(&[("shared", "^2.0.0")]))])),
            (
                "shared",
                packument(
                    "shared",
                    &[
                        ("1.0.0", json!({})),
                        ("1.5.0", json!({})),
                        ("1.9.0", json!({})),
                        ("2.0.0", json!({})),
                        ("2.5.0", json!({})),
                    ],
                ),
            ),
        ])
    }

    /// **R773-T5's other headline criterion.** Write a lock, let the registry
    /// publish newer satisfying versions, re-resolve with that lock in hand,
    /// and get the same bytes back.
    ///
    /// The whole loop is real: the preferences are read out of the written file
    /// by [`crate::project_lock::preferred_from_lock`] — the same function the
    /// `lock` verb uses — rather than assembled in the test, so a lock this
    /// crate cannot read back would fail here.
    ///
    /// The unpreferred re-resolve is asserted to differ first. Without that,
    /// this test would pass just as happily against a fake registry that never
    /// moved, which is the failure mode a no-op test is most prone to.
    #[test]
    fn a_re_resolve_after_the_registry_moves_rewrites_the_same_lock() {
        let first = render_lock(&conflict_world().resolve(CONFLICT_ROOT)).expect("render");

        let moved = moved_on_world();
        let drifted = render_lock(&moved.resolve(CONFLICT_ROOT)).expect("render");
        assert_ne!(
            drifted, first,
            "the moved-on registry must actually change an unlocked resolve, or this proves nothing"
        );

        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("bun.lock"), &first).unwrap();
        let preferred =
            crate::project_lock::preferred_from_lock(project.path()).expect("preferences");
        // Four names, and `shared` carrying both of its locked versions — the
        // nested-conflict case a name→version map could not express.
        assert_eq!(preferred.len(), 4);
        assert_eq!(preferred.get("shared").map(|v| v.len()), Some(2));

        let held = render_lock(&moved.resolve_preferring(CONFLICT_ROOT, &preferred))
            .expect("render");
        assert_eq!(held, first, "re-resolving from a lock this wrote is not a no-op");
    }

    /// A first lock has nothing to be faithful to, and that is not an error.
    #[test]
    fn a_project_with_no_lock_yet_has_no_preferences() {
        let project = tempfile::tempdir().unwrap();
        assert!(crate::project_lock::preferred_from_lock(project.path())
            .expect("preferences")
            .is_empty());
    }

    /// A `file:` entry is a placement, not a registry selection, so it
    /// contributes no preference — and reading one back must not error.
    #[test]
    fn a_link_entry_contributes_no_preference() {
        let world = World::new(&[]);
        let tree = world
            .resolve(r#"{ "name": "root", "dependencies": { "shared": "file:packages/shared" } }"#);
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("bun.lock"), render_lock(&tree).expect("render"))
            .unwrap();

        assert!(crate::project_lock::preferred_from_lock(project.path())
            .expect("preferences")
            .is_empty());
    }

    /// Everything else is refused by name and protocol, not dropped.
    #[test]
    fn a_git_dependency_is_refused_naming_the_protocol() {
        let world = World::new(&[]);
        let tree = world
            .resolve(r#"{ "name": "root", "dependencies": { "forked": "github:owner/repo" } }"#);

        let err = entries_from_tree(&tree).unwrap_err().to_string();
        assert!(err.contains("forked"), "{err}");
        assert!(err.contains("git"), "{err}");
    }
}
