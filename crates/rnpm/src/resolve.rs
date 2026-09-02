//! Phase 1 (R773-F3, W318 §6): build the tree.
//!
//! BFS from the root manifest. For each `(name, range)` edge: first ask
//! whether an ancestor already provides a satisfying version; otherwise fetch
//! the abbreviated packument, take the highest satisfying version, and hoist
//! the new node as high up the hierarchy as it can go without shadowing an
//! incompatible sibling or violating a parent's own requirement.
//!
//! # No backtracking, ever
//!
//! There is always a legal answer, because `node_modules` permits the same
//! package at many tree positions: a conflict is discharged by *nesting a
//! second copy under its requester*, never by undoing a decision and trying
//! again. That is the whole reason no SAT solver appears anywhere in R773
//! (W318 §1). Nothing in this module removes or re-parents a placed node — if
//! a future edit wants to, the walk has been mis-modelled.
//!
//! # Peers are recorded, never followed
//!
//! [`Node::peers`] is populated here and read by nobody here. Resolving peers
//! is a post-pass over the finished tree and is R773-F4's entire job. Walking
//! them in this phase would be the exponential mistake W318 §6 warns about.
//!
//! # Attribution
//!
//! [`Resolver::place`] is a **port** of `node-maintainer` 0.3.34's
//! `place_child` (`src/resolver.rs:346-440`, Apache-2.0, orogene), which W318
//! §5 names as the compact correct statement of this walk. The hoisting loop's
//! structure, its two break conditions and the ancestry test that makes the
//! second one safe are all upstream's; what differs is the data structure (a
//! `Vec` arena here, `petgraph` there) and the removal of its lockfile
//! `target_path` hint, which is R773-T5's concern and not phase 1's. See
//! `NOTICE`.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::client::{RegistryClient, RegistryEndpoint};
use crate::packument::{Packument, VersionManifest};
use crate::spec::{PackageSpec, Version, VersionSpec};
use crate::transport::Transport;

/// Index into [`ResolvedTree`]'s arena.
pub type NodeId = usize;

/// A safety bound, not a semantic one. The walk terminates on every input we
/// know of — a finite packument set bounds the distinct `(name, version)`
/// pairs, and the ancestor check discharges dependency cycles without a new
/// node. This exists so a pathological input fails with a message instead of
/// hanging a build.
pub(crate) const MAX_NODES: usize = 100_000;

/// What phase 1 decided about one registry package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// The *registry* name, which differs from the install name under an
    /// alias (`"foo": "npm:bar@^1"` installs `bar` in a directory `foo`).
    pub name: String,
    pub version: Version,
    /// Absolute, straight off the packument — never constructed. This is the
    /// pair `mesofact-build`'s content-addressed store consumes.
    pub tarball: String,
    /// `None` only for pre-SRI publishes. A materializer must refuse such an
    /// entry by name rather than fetch on trust; phase 1 records rather than
    /// judges.
    pub integrity: Option<String>,
}

/// A peer requirement, carried as data for R773-F4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRequirement {
    pub range: VersionSpec,
    /// From `peerDependenciesMeta.<name>.optional` — a missing optional peer
    /// is skipped rather than auto-installed.
    pub optional: bool,
}

/// Where a node's content comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeSource {
    /// The synthetic node 0.
    Root,
    Registry(Resolution),
    /// A `file:`, git or tarball spec. Recorded verbatim and **not walked
    /// into**: phase 1 has no packument for one, and reading its manifest
    /// means fetching, which this phase does not do for non-registry sources.
    /// R773-T5 emits these; R773-F6 decides whether any need following.
    Other(Box<PackageSpec>),
}

/// One position in the tree — a directory in some `node_modules`.
#[derive(Debug, Clone)]
pub struct Node {
    /// The directory name this installs under, which is the *install* name,
    /// not necessarily the registry name (see [`Resolution::name`]).
    pub name: String,
    pub source: NodeSource,
    /// Hierarchy location: whose `node_modules` this sits in.
    pub parent: Option<NodeId>,
    /// Hierarchy: what sits in *this* node's `node_modules`.
    pub children: BTreeMap<String, NodeId>,
    /// Logical edges: what this node's declared dependencies resolved to.
    /// Distinct from `children` — a dependency usually resolves to a node
    /// hoisted somewhere above.
    pub dependencies: BTreeMap<String, NodeId>,
    /// Reverse of `dependencies`, maintained because the hoist needs to ask
    /// who wanted an already-placed node.
    pub requesters: BTreeSet<NodeId>,
    /// This node's own declared requirements, by install name. Read by the
    /// hoist's second break condition: a node may not be hoisted past a level
    /// whose own requirement it would violate.
    pub requirements: BTreeMap<String, VersionSpec>,
    /// Recorded, never followed. R773-F4's input.
    pub peers: BTreeMap<String, PeerRequirement>,
    /// Reached through `optionalDependencies`. A failure to resolve one is not
    /// a failed install.
    pub optional: bool,
}

impl Node {
    pub fn resolution(&self) -> Option<&Resolution> {
        match &self.source {
            NodeSource::Registry(r) => Some(r),
            NodeSource::Root | NodeSource::Other(_) => None,
        }
    }

    pub fn version(&self) -> Option<&Version> {
        self.resolution().map(|r| &r.version)
    }
}

/// The phase-1 output: R773-F4's input, and what R773-T5 flattens into
/// `LockEntry` paths.
///
/// Deliberately **not** a lockfile. Emitting `bun.lock` is R773-T5's job,
/// through the seam R771-T5 already defined on the materializer's side
/// (`mesofact-build/src/lock.rs`); a second lock writer here is the specific
/// mistake that brief forbids.
#[derive(Debug, Clone)]
pub struct ResolvedTree {
    nodes: Vec<Node>,
}

impl ResolvedTree {
    pub const ROOT: NodeId = 0;

    fn new(root_name: String, requirements: BTreeMap<String, VersionSpec>) -> Self {
        Self {
            nodes: vec![Node {
                name: root_name,
                source: NodeSource::Root,
                parent: None,
                children: BTreeMap::new(),
                dependencies: BTreeMap::new(),
                requesters: BTreeSet::new(),
                requirements,
                peers: BTreeMap::new(),
                optional: false,
            }],
        }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        false // the root always exists
    }

    /// Every node except the root, with its id.
    pub fn packages(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().skip(1)
    }

    /// `self` and its ancestors, innermost first — the chain Node's own
    /// resolution algorithm walks.
    fn ancestry(&self, from: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cursor = Some(from);
        std::iter::from_fn(move || {
            let current = cursor?;
            cursor = self.nodes[current].parent;
            Some(current)
        })
    }

    /// What `require(name)` resolves to from `from` — this node's
    /// `node_modules`, then each ancestor's in turn. This is the single
    /// function that makes "already satisfied by an ancestor" and the hoist
    /// agree about what is visible from where.
    pub fn resolve_from(&self, from: NodeId, name: &str) -> Option<NodeId> {
        self.ancestry(from)
            .find_map(|id| self.nodes[id].children.get(name).copied())
    }

    /// Whether `ancestor` is `descendant` or one of its hierarchy ancestors.
    /// Reflexive, matching `node-maintainer`'s `is_ancestor`.
    pub fn is_ancestor(&self, ancestor: NodeId, descendant: NodeId) -> bool {
        self.ancestry(descendant).any(|id| id == ancestor)
    }

    /// The chain of install names from the root down to `id`, e.g.
    /// `["@scope/pkg", "react"]` for a copy of `react` nested under
    /// `@scope/pkg`. Joined with `/` this is bun.lock's key form, which is why
    /// R773-T5 needs no path computation of its own.
    pub fn path(&self, id: NodeId) -> Vec<&str> {
        let mut path: Vec<&str> = self
            .ancestry(id)
            .filter(|n| *n != Self::ROOT)
            .map(|n| self.nodes[n].name.as_str())
            .collect();
        path.reverse();
        path
    }

    /// [`Self::path`] joined with `/`.
    pub fn install_path(&self, id: NodeId) -> String {
        self.path(id).join("/")
    }

    /// Registry identity — `name@version` — or the install name for a
    /// non-registry node. Used by [`crate::peers`] as half of its memo key.
    pub fn identity(&self, id: NodeId) -> String {
        match self.nodes[id].resolution() {
            Some(r) => format!("{}@{}", r.name, r.version),
            None => self.nodes[id].name.clone(),
        }
    }

    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub(crate) fn push(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    pub(crate) fn ids(&self) -> std::ops::Range<NodeId> {
        0..self.nodes.len()
    }
}

/// Where packuments come from.
///
/// A trait rather than a concrete client so that a multi-registry caller (the
/// `jsget` facade) can route by ecosystem without this module learning that
/// registries differ — and so a test can count fetches.
pub trait PackumentSource {
    /// `None` means the registry has no such package, which is not an error
    /// for an `optionalDependency`.
    fn packument(&self, registry_name: &str) -> Result<Option<Packument>>;
}

/// [`PackumentSource`] over one [`RegistryEndpoint`].
pub struct RegistrySource<'a, T: Transport> {
    client: &'a RegistryClient<T>,
    endpoint: &'a RegistryEndpoint,
}

impl<'a, T: Transport> RegistrySource<'a, T> {
    pub fn new(client: &'a RegistryClient<T>, endpoint: &'a RegistryEndpoint) -> Self {
        Self { client, endpoint }
    }
}

impl<T: Transport> PackumentSource for RegistrySource<'_, T> {
    fn packument(&self, registry_name: &str) -> Result<Option<Packument>> {
        self.client.try_packument(self.endpoint, registry_name)
    }
}

/// The root `package.json`, reduced to what phase 1 reads.
#[derive(Debug, Clone, Default)]
pub struct RootManifest {
    pub name: String,
    pub dependencies: BTreeMap<String, String>,
    /// Followed for the root only — a dependency's dev-dependencies are not
    /// installed.
    pub dev_dependencies: BTreeMap<String, String>,
    pub optional_dependencies: BTreeMap<String, String>,
}

impl RootManifest {
    pub fn from_package_json(json: &str) -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default)]
            name: String,
            #[serde(default)]
            dependencies: BTreeMap<String, String>,
            #[serde(default, rename = "devDependencies")]
            dev_dependencies: BTreeMap<String, String>,
            #[serde(default, rename = "optionalDependencies")]
            optional_dependencies: BTreeMap<String, String>,
        }
        let raw: Raw = serde_json::from_str(json).context("parsing the root package.json")?;
        Ok(Self {
            name: raw.name,
            dependencies: raw.dependencies,
            dev_dependencies: raw.dev_dependencies,
            optional_dependencies: raw.optional_dependencies,
        })
    }

    /// Declared dependencies in npm's precedence order, deduplicated by name.
    ///
    /// A name appearing in more than one map is one dependency, not several —
    /// which also keeps the invariant [`Resolver::place`] relies on: no node
    /// ever requests the same install name twice.
    fn edges(&self) -> Vec<(&str, &str, bool)> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for (map, optional) in [
            (&self.dependencies, false),
            (&self.optional_dependencies, true),
            (&self.dev_dependencies, false),
        ] {
            for (name, value) in map {
                if seen.insert(name.as_str()) {
                    out.push((name.as_str(), value.as_str(), optional));
                }
            }
        }
        out
    }
}

/// What a manifest entry turned out to be.
enum Requirement {
    /// A registry package: walk it.
    Registry(Edge),
    /// A `file:`/git/tarball dep: record it, do not walk into it.
    Other { name: String, spec: PackageSpec, optional: bool },
}

/// One `(requester, name, range)` edge waiting to be resolved.
#[derive(Debug, Clone)]
struct Edge {
    from: NodeId,
    /// Directory name, i.e. the manifest key.
    name: String,
    /// What to ask the registry for; differs from `name` under an alias.
    registry_name: String,
    range: VersionSpec,
    optional: bool,
}

/// Phase 1's walk.
///
/// Deliberately holds **no packument memo of its own.** [`RegistryClient`] is
/// already the cache (R773-F1), and a second layer here would answer a
/// question the walk should never have asked — masking exactly the invariant
/// this phase exists to maintain, that a dependency already satisfied by an
/// ancestor is never looked up at all. Keeping the only memo below this layer
/// is what lets a test count [`PackumentSource`] calls and get a real answer.
pub struct Resolver<'a, S: PackumentSource> {
    source: &'a S,
}

impl<'a, S: PackumentSource> Resolver<'a, S> {
    pub fn new(source: &'a S) -> Self {
        Self { source }
    }

    /// Build the tree.
    pub fn resolve(&self, root: &RootManifest) -> Result<ResolvedTree> {
        let mut requirements = BTreeMap::new();
        let mut queue: VecDeque<Edge> = VecDeque::new();
        let mut others = Vec::new();

        for (name, value, optional) in root.edges() {
            match self.requirement(ResolvedTree::ROOT, name, value, optional)? {
                Requirement::Registry(edge) => {
                    requirements.insert(name.to_string(), edge.range.clone());
                    queue.push_back(edge);
                }
                Requirement::Other { name, spec, optional } => others.push((name, spec, optional)),
            }
        }

        let mut tree = ResolvedTree::new(root.name.clone(), requirements);
        for (name, spec, optional) in others {
            self.add_other_node(&mut tree, ResolvedTree::ROOT, name, spec, optional);
        }

        while let Some(edge) = queue.pop_front() {
            // 1. Already satisfied by something visible from the requester?
            //    This is the check that makes a hoisted tree cheap: it costs no
            //    packument fetch, which is one of this ticket's two criteria.
            if let Some(existing) = tree.resolve_from(edge.from, &edge.name) {
                if let Some(version) = tree.nodes[existing].version() {
                    if edge.range.matches(version) {
                        link(&mut tree, edge.from, existing, &edge.name);
                        continue;
                    }
                }
            }

            // 2. Fetch, and take the highest satisfying version.
            let Some(packument) = self.fetch(&edge.registry_name)? else {
                if edge.optional {
                    continue;
                }
                bail!(
                    "{} is not published on this registry (required by {})",
                    edge.registry_name,
                    describe(&tree, edge.from)
                );
            };
            let Some(chosen) = pick_version(&packument, &edge.range) else {
                if edge.optional {
                    continue;
                }
                bail!(
                    "no version of {} satisfies {} (required by {})",
                    edge.registry_name,
                    edge.range,
                    describe(&tree, edge.from)
                );
            };

            // 3. Create the node and hoist it into place.
            let child = self.add_registry_node(&mut tree, &edge, &packument, chosen)?;
            self.place(&mut tree, &edge, child)?;
            link(&mut tree, edge.from, child, &edge.name);

            // 4. Enqueue the new node's own runtime dependencies. Peers were
            //    recorded in step 3 and are deliberately not enqueued.
            for (name, value, optional) in dependency_edges(chosen) {
                match self
                    .requirement(child, name, value, optional)
                    .with_context(|| format!("of {}@{}", edge.registry_name, chosen.version))?
                {
                    Requirement::Registry(next) => {
                        tree.nodes[child]
                            .requirements
                            .insert(name.to_string(), next.range.clone());
                        queue.push_back(next);
                    }
                    Requirement::Other { name, spec, optional } => {
                        self.add_other_node(&mut tree, child, name, spec, optional);
                    }
                }
            }

            if tree.len() > MAX_NODES {
                bail!(
                    "dependency tree exceeded {MAX_NODES} nodes; this is a resolver bug or a \
                     pathological manifest, not a legitimately large install"
                );
            }
        }

        Ok(tree)
    }

    fn fetch(&self, registry_name: &str) -> Result<Option<Packument>> {
        self.source
            .packument(registry_name)
            .with_context(|| format!("fetching the packument for {registry_name}"))
    }

    /// Turn a `(name, value)` manifest entry into either a registry edge to
    /// walk or a non-registry node to record as-is.
    fn requirement(
        &self,
        from: NodeId,
        name: &str,
        value: &str,
        optional: bool,
    ) -> Result<Requirement> {
        let spec = PackageSpec::from_dependency(name, value)
            .with_context(|| format!("dependency {name}"))?;
        match spec.target() {
            PackageSpec::Npm { name: registry_name, requested, .. } => {
                Ok(Requirement::Registry(Edge {
                    from,
                    name: name.to_string(),
                    registry_name: registry_name.clone(),
                    // A spec that stated no version means `*`, which is what
                    // npm reads it as.
                    range: requested
                        .clone()
                        .unwrap_or(VersionSpec::Range(default_range())),
                    optional,
                }))
            }
            _ => Ok(Requirement::Other { name: name.to_string(), spec, optional }),
        }
    }

    /// Record a `file:`/git/tarball dependency at its requester's level.
    ///
    /// Not hoisted, deliberately: hoisting exists to deduplicate *versions* of
    /// a registry package, and there is no version here to compare. A link is
    /// a link to one path, so putting it anywhere but where it was asked for
    /// would be motion without meaning.
    fn add_other_node(
        &self,
        tree: &mut ResolvedTree,
        from: NodeId,
        name: String,
        spec: PackageSpec,
        optional: bool,
    ) -> NodeId {
        let id = tree.nodes.len();
        tree.nodes.push(Node {
            name: name.clone(),
            source: NodeSource::Other(Box::new(spec)),
            parent: Some(from),
            children: BTreeMap::new(),
            dependencies: BTreeMap::new(),
            requesters: BTreeSet::new(),
            requirements: BTreeMap::new(),
            peers: BTreeMap::new(),
            optional,
        });
        tree.nodes[from].children.insert(name.clone(), id);
        link(tree, from, id, &name);
        id
    }

    /// `place_child` — a port of `node-maintainer` 0.3.34
    /// `src/resolver.rs:346-440` (Apache-2.0). Hoist `child` as high as it can
    /// go, then attach it.
    ///
    /// Walk up from the requester. At each candidate level, stop if either
    /// break condition holds, otherwise take that level and keep climbing:
    ///
    /// 1. Something named the same already resolves from here **and** does not
    ///    satisfy what we were asked for **and** was wanted by someone inside
    ///    this level's subtree. Hoisting past it would shadow it. The ancestry
    ///    test is upstream's and is load-bearing, not decorative: it is what
    ///    stops the climb one level *below* the incompatible node rather than
    ///    on top of it, which is what would clobber.
    /// 2. This level's own manifest requires that name at a range our chosen
    ///    version does not satisfy.
    ///
    /// `target` starts at the requester, so a conflict on the very first
    /// iteration nests the copy under its requester — which is exactly the
    /// "discharge by nesting" rule, falling out of the loop rather than being
    /// a special case.
    ///
    /// Upstream's third break — a lockfile-supplied `target_path` hint — is
    /// dropped: honouring a previous lock is R773-T5's concern, not phase 1's.
    fn place(&self, tree: &mut ResolvedTree, edge: &Edge, child: NodeId) -> Result<()> {
        let name = tree.nodes[child].name.clone();
        let version = tree.nodes[child].version().cloned();

        let mut target = edge.from;
        let mut cursor = Some(edge.from);
        'outer: while let Some(current) = cursor {
            if let Some(resolved) = tree.resolve_from(current, &name) {
                let unsatisfying = tree.nodes[resolved]
                    .version()
                    .is_none_or(|v| !edge.range.matches(v));
                if unsatisfying {
                    let requesters: Vec<NodeId> =
                        tree.nodes[resolved].requesters.iter().copied().collect();
                    for from in requesters {
                        if tree.is_ancestor(current, from) {
                            break 'outer;
                        }
                    }
                }
            }

            if let (Some(required), Some(version)) =
                (tree.nodes[current].requirements.get(&name), version.as_ref())
            {
                if !required.matches(version) {
                    break 'outer;
                }
            }

            // No conflict at this level — take it and try to go higher.
            target = current;
            cursor = tree.nodes[current].parent;
        }

        tree.nodes[child].parent = Some(target);
        // `BTreeMap::insert` overwrites silently, and an overwrite here would
        // orphan an already-placed node — its `parent` still pointing at
        // `target`, `target.children` no longer naming it. That is an
        // *unplacement*, in a walk whose whole premise is that placements are
        // never undone, and it would be invisible in a release build if this
        // were only a `debug_assert!`. The hoist's ancestry test is what makes
        // it unreachable; this is the check that says so out loud if it ever
        // stops being true.
        if let Some(clobbered) = tree.nodes[target].children.insert(name.clone(), child) {
            bail!(
                "placing {name} at {:?} displaced the already-placed node {} — the hoist's \
                 ancestry test should make this unreachable, so this is a resolver bug",
                tree.install_path(target),
                describe(tree, clobbered)
            );
        }
        Ok(())
    }

    fn add_registry_node(
        &self,
        tree: &mut ResolvedTree,
        edge: &Edge,
        packument: &Packument,
        chosen: &VersionManifest,
    ) -> Result<NodeId> {
        let version = Version::parse(&chosen.version).with_context(|| {
            format!("{}@{} has an unparseable version", edge.registry_name, chosen.version)
        })?;

        let mut peers = BTreeMap::new();
        for (peer, range) in &chosen.peer_dependencies {
            let parsed = parse_peer_range(range).with_context(|| {
                format!(
                    "peer dependency {peer} of {}@{}",
                    edge.registry_name, chosen.version
                )
            })?;
            peers.insert(
                peer.clone(),
                PeerRequirement {
                    range: parsed,
                    optional: chosen
                        .peer_dependencies_meta
                        .get(peer)
                        .is_some_and(|m| m.optional),
                },
            );
        }

        let id = tree.nodes.len();
        tree.nodes.push(Node {
            name: edge.name.clone(),
            source: NodeSource::Registry(Resolution {
                name: if packument.name.is_empty() {
                    edge.registry_name.clone()
                } else {
                    packument.name.clone()
                },
                version,
                tarball: chosen.dist.tarball.clone(),
                integrity: chosen.dist.integrity.clone(),
            }),
            parent: None,
            children: BTreeMap::new(),
            dependencies: BTreeMap::new(),
            requesters: BTreeSet::new(),
            requirements: BTreeMap::new(),
            peers,
            optional: edge.optional,
        });
        Ok(id)
    }
}

/// Record the logical edge in both directions.
pub(crate) fn link(tree: &mut ResolvedTree, from: NodeId, to: NodeId, name: &str) {
    tree.nodes[from].dependencies.insert(name.to_string(), to);
    tree.nodes[to].requesters.insert(from);
}

/// A transitive dependency's own edges: runtime and optional only. Its
/// `devDependencies` are not installed, which is why they are absent here and
/// present on [`RootManifest`].
fn dependency_edges(manifest: &VersionManifest) -> Vec<(&str, &str, bool)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (map, optional) in [
        (&manifest.dependencies, false),
        (&manifest.optional_dependencies, true),
    ] {
        for (name, value) in map {
            if seen.insert(name.as_str()) {
                out.push((name.as_str(), value.as_str(), optional));
            }
        }
    }
    out
}

/// The highest version satisfying `spec`.
///
/// A dist-tag is looked up rather than compared, which is why a tag edge
/// always needs the packument even when an ancestor already provides the
/// package: `latest` cannot be checked against a version without the document
/// that defines it.
///
/// Deprecated versions are *not* skipped — npm does not skip them either, and
/// silently preferring an older version would be a divergence nobody asked
/// for. R773-F6 owns surfacing the warning.
pub(crate) fn pick_version<'p>(packument: &'p Packument, spec: &VersionSpec) -> Option<&'p VersionManifest> {
    match spec {
        VersionSpec::Tag(tag) => packument.dist_tag(tag),
        _ => packument
            .versions
            .values()
            .filter_map(|manifest| {
                Version::parse(&manifest.version)
                    .ok()
                    .filter(|version| spec.matches(version))
                    .map(|version| (version, manifest))
            })
            .max_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, manifest)| manifest),
    }
}

fn parse_peer_range(range: &str) -> Result<VersionSpec> {
    match PackageSpec::from_dependency("peer", range)? {
        PackageSpec::Npm { requested: Some(spec), .. } => Ok(spec),
        _ => Ok(VersionSpec::Range(default_range())),
    }
}

fn default_range() -> crate::spec::Range {
    crate::spec::Range::any()
}

pub(crate) fn describe(tree: &ResolvedTree, id: NodeId) -> String {
    if id == ResolvedTree::ROOT {
        return "the root manifest".to_string();
    }
    match tree.node(id).version() {
        Some(version) => format!("{}@{version}", tree.install_path(id)),
        None => tree.install_path(id),
    }
}
