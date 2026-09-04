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
//! # The manifest surfaces beyond `dependencies` (R773-F6)
//!
//! Four of them, and three share one property worth stating up front: they
//! change the tree *by leaving something out of it*, which is invisible unless
//! someone says so. Hence [`ResolveWarning`] and [`ResolvedTree::warnings`] —
//! this is a library, so nothing here prints.
//!
//! - **`overrides` / `resolutions`** are applied in [`Resolver::requirement`],
//!   the single point both edge sources funnel through, which is what makes an
//!   override reach a transitive dependency and not just a direct one. A form
//!   the flat table cannot express is an error at parse time; see
//!   [`crate::overrides`] for why accept-and-ignore is the worst option here.
//! - **`optionalDependencies`** skip on *every* way an edge can come up empty,
//!   a fetch that errored included. That last one used to abort the whole
//!   resolve.
//! - **`os` / `cpu` / `libc`** are **recorded, never gated** (R773-F9). They
//!   travel onto [`Resolution`] and out into the lock, and the *materializer*
//!   filters by them (`mesofact-build`'s `install.rs`). F6 gated here instead —
//!   skipping a mismatched optional, erroring `EBADPLATFORM` on a mismatched
//!   required — which made every lock this walk produced host-specific: a lock
//!   cut on macOS simply had no linux binaries in it. bun's own format is
//!   portable (a real `bun.lock` records all twenty-four `@esbuild/*` variants
//!   with their `os`/`cpu` meta), so resolution is host-independent and no
//!   host reaches this module at all. The `EBADPLATFORM` refusal is not
//!   gone, it *moved*: it now fires at install time, where "does this run
//!   here" is a question with a right answer.
//! - **dist-tags** resolve in [`pick_version`]; what F6 added is that an
//!   unpublished tag no longer reports itself as an unsatisfiable range.
//!
//! # Peers are recorded, never followed
//!
//! [`Node::peers`] is populated here and read by nobody here. Resolving peers
//! is a post-pass over the finished tree and is R773-F4's entire job. Walking
//! them in this phase would be the exponential mistake W318 §6 warns about.
//!
//! # Attribution
//!
//! [`Resolver::place`] began as a **port** of `node-maintainer` 0.3.34's
//! `place_child` (`src/resolver.rs:346-440`, Apache-2.0, orogene), which W318
//! §5 names as the compact correct statement of this walk. The hoisting loop's
//! structure and both of [`conflicts_at`]'s conditions — including the ancestry
//! test that makes the first one safe — are still upstream's. What differs is
//! the data structure (a `Vec` arena here, `petgraph` there), the removal of
//! its lockfile `target_path` hint — a previous lock reaches this walk as
//! [`PreferredVersions`], which decides *which version*, never where a copy
//! lands (R773-T5) — and
//! **where a conflict lands**: upstream places at the level below the objector,
//! this places under the requester, because that is what bun does and bun is
//! the oracle (R773-B1 — see [`Resolver::place`] for the whole argument). See
//! `NOTICE`.
//!
//! @yah:ticket(R773-B1, "Phase-1 hoist lifts a conflict copy higher than bun does inside a nested subtree")
//! @yah:status(review)
//! @yah:at(2026-09-04T00:41:52Z)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R773)
//! @yah:severity(low)
//! @yah:verify("oss/mesofact/scripts/check-npm-conformance.sh passes with the six [[known_divergence]] entries removed from real-typescript-toolchain/case.toml")
//! @yah:handoff("Fixed. `Resolver::place` (crates/rnpm/src/resolve.rs) now has two outcomes and no third: the ROOT if nothing between the requester and the root objects, `edge.from` if anything does. It used to place at the last non-objecting level, an intermediate position, which only differs from bun when the requester is two or more levels deep — which is why every synthetic test passed and only the real-world corpus caught it.")
//! @yah:handoff("Matches bun's `Tree.hoistDependency`, where the recursion into the parent clears `as_defined` and only the still-`as_defined` frame turns a rejection into a placement — so an intermediate level is never a placement site there either. The two break conditions and the reflexive ancestry test are unchanged and moved to a free fn `conflicts_at` so `place` can borrow `&ResolvedTree` (that also drops the requesters `Vec` clone the old `&mut` borrow forced).")
//! @yah:handoff("Deleted all six [[known_divergence]] entries from crates/mesofact-build/tests/corpus/real-typescript-toolchain/case.toml (now `known_divergence = []`, matching the other 11 waiver-free cases). Beyond the ticket: added a regression test `a_conflict_below_a_nested_requester_lands_under_that_requester_not_above_it` (crates/rnpm/tests/resolve_tree.rs:326) — the corpus gate is minutes of rolldown/V8 build and lives outside `check`, so this shape needed cover in a fast test too; and corrected the now-false attribution in crates/rnpm/NOTICE and the module header, which both claimed the ancestry test lands the copy below the objector and that `target` is initialised at the requester.")
//! @yah:verify("./scripts/check-npm-conformance.sh — 13/13 cases agree with their oracles, real-typescript-toolchain waiver-free. Run with the waivers still in first: all six reported StaleWaiver and NO new divergence appeared, so the fix moved exactly the six paths it was supposed to.")
//! @yah:verify("cargo test -p rnpm — 116 pass, 0 fail (93 pre-existing, unchanged, + the new one). cargo test -p mesofact-build — 133 pass, 0 fail. cargo clippy -p rnpm --all-targets — clean apart from the pre-existing large-enum-variant warning. The new test was proved to be a real guard by temporarily restoring `target = current`: it failed with left `[\"dep\",\"mid\",\"outer\",\"outer/dep\",\"outer/mid\"]` vs right `[..., \"outer/mid\",\"outer/mid/dep\"]`, i.e. exactly the old intermediate placement.")
//! @yah:assumes("Formatting was matched by hand, not checked: `cargo fmt -p rnpm -- --check` errors with \"'cargo-fmt' is not installed for the toolchain '1.97.0-aarch64-apple-darwin'\" on this machine, so no fmt gate ran over these edits.")

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use crate::client::{RegistryClient, RegistryEndpoint};
use crate::overrides;
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
    /// The manifest's `os` list, verbatim — npm's grammar, negations included
    /// (`["!win32"]`). Empty for the overwhelming majority of packages, which
    /// declare no gate at all.
    ///
    /// Recorded here rather than acted on: R773-F9 made resolution
    /// host-independent, so these three exist to be *written into the lock*
    /// and evaluated by the materializer against the host it is installing
    /// for. They live on `Resolution` rather than on [`Node`] because they are
    /// a fact about the published registry package, exactly like `tarball` and
    /// `integrity` — a [`NodeSource::Other`] has no packument and so has
    /// nothing to say here.
    pub os: Vec<String>,
    /// The manifest's `cpu` list, verbatim. See [`Resolution::os`].
    pub cpu: Vec<String>,
    /// The manifest's `libc` list, verbatim. See [`Resolution::os`].
    pub libc: Vec<String>,
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
    ///
    /// R773-T5 emits these; R773-F6 asked whether any need following and the
    /// answer is still no. An override is the one new way to *reach* one —
    /// `"overrides": {"foo": "file:../foo"}` turns a registry edge into this —
    /// and it lands here exactly as a directly-declared `file:` dep would,
    /// because the override is applied before the spec is parsed rather than
    /// after.
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
    /// Which of this node's own [`Node::dependencies`] were declared in
    /// `optionalDependencies`, by install name. The per-*edge* half of what
    /// [`Node::optional`] summarises per node.
    pub optional_deps: BTreeSet<String>,
    /// Is this package's absence tolerable — i.e. is every path from the root
    /// to it through at least one `optionalDependencies` edge?
    ///
    /// Computed by [`recompute_optionality`] once the tree is finished, not
    /// set from the edge that happened to create the node: a package reached
    /// optionally *and* required is required, and everything below an optional
    /// package is itself optional however its parent declared it.
    ///
    /// **Load-bearing since R773-F9.** It used to be write-only. Now it rides
    /// into the lock, and the materializer reads it to decide whether a
    /// platform-mismatched entry is a skip or an `EBADPLATFORM` error — the
    /// refusal resolution stopped making.
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

/// Something resolution decided quietly that a human should still be told
/// about (R773-F6).
///
/// Every variant describes a *successful* resolve — a failure is an `Err` and
/// says so. These are the outcomes that would otherwise be invisible: a
/// dependency the tree does not contain, or a version it does contain for a
/// reason the manifest did not state. This is a library, so they are collected
/// rather than printed; [`ResolvedTree::warnings`] is the whole delivery
/// mechanism and the CLI above decides how to render them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveWarning {
    /// An `overrides` / `resolutions` entry replaced the range a requester
    /// declared. Recorded per edge, because *which* requester was overridden
    /// is the part a human needs to see.
    OverrideApplied {
        name: String,
        requester: String,
        /// The range the manifest asked for.
        from: String,
        /// The spec the override substituted.
        to: String,
    },
    /// An `optionalDependencies` entry is absent from the tree. Not an error:
    /// that is exactly what declaring it optional asks for.
    OptionalSkipped { name: String, requester: String, reason: String },
    // No `PlatformExcluded`. R773-F6 had one, because resolution dropped a
    // package whose `os`/`cpu`/`libc` excluded the host and that omission was
    // invisible unless something said so. R773-F9 stopped dropping it: the
    // package is in the tree and in the lock on every host, and the *install*
    // decides. A warning here would describe a decision this phase no longer
    // makes.
    /// The chosen version is deprecated. npm does not skip these and neither
    /// does this walk (see [`pick_version`]) — silently preferring an older
    /// version would be a divergence nobody asked for.
    Deprecated { name: String, version: String, reason: String },
}

impl fmt::Display for ResolveWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveWarning::OverrideApplied { name, requester, from, to } => write!(
                f,
                "override: {name} was pinned to {to} for {requester}, which asked for {from}"
            ),
            ResolveWarning::OptionalSkipped { name, requester, reason } => write!(
                f,
                "skipped optional dependency {name} (required by {requester}): {reason}"
            ),
            ResolveWarning::Deprecated { name, version, reason } => {
                write!(f, "{name}@{version} is deprecated: {reason}")
            }
        }
    }
}

/// The phase-1 output: R773-F4's input, and what R773-T5 flattens into
/// `LockEntry` paths.
///
/// Deliberately **not** a lockfile. Emitting `bun.lock` is R773-T5's job,
/// through the seam R771-T5 already defined on the materializer's side
/// (`mesofact-build/src/lock.rs`); a second lock writer here is the specific
/// mistake that brief forbids.
///
/// It carries the *resolution settings* — the override table and the previous
/// lock's [`PreferredVersions`] — rather than [`Resolver`], because phase 2
/// (`crate::peers`) builds a fresh `Resolver` around the same tree when it
/// auto-installs a peer. A setting on the resolver would silently stop applying
/// at that seam; a setting on the tree cannot. That is not a style preference:
/// an auto-installed peer whose version ignored the lock is precisely the one
/// node that would make an otherwise byte-identical re-resolve differ.
///
/// It used to carry a second one, the host the platform gates were read
/// against. R773-F9 removed it along with the gates: a resolve is now the same
/// on every machine, so a host here would be a knob that changed nothing.
#[derive(Debug, Clone)]
pub struct ResolvedTree {
    nodes: Vec<Node>,
    overrides: BTreeMap<String, String>,
    preferred: PreferredVersions,
    warnings: Vec<ResolveWarning>,
}

impl ResolvedTree {
    pub const ROOT: NodeId = 0;

    fn new(
        root_name: String,
        overrides: BTreeMap<String, String>,
        preferred: PreferredVersions,
    ) -> Self {
        Self {
            nodes: vec![Node {
                name: root_name,
                source: NodeSource::Root,
                parent: None,
                children: BTreeMap::new(),
                dependencies: BTreeMap::new(),
                requesters: BTreeSet::new(),
                requirements: BTreeMap::new(),
                peers: BTreeMap::new(),
                optional_deps: BTreeSet::new(),
                optional: false,
            }],
            overrides,
            preferred,
            warnings: Vec::new(),
        }
    }

    /// What resolution decided quietly, in the order it decided it.
    pub fn warnings(&self) -> &[ResolveWarning] {
        &self.warnings
    }

    /// The previous lock's selections, as this walk was given them.
    pub fn preferred(&self) -> &PreferredVersions {
        &self.preferred
    }

    pub(crate) fn warn(&mut self, warning: ResolveWarning) {
        self.warnings.push(warning);
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
    /// npm's `overrides` and yarn's `resolutions`, flattened to name → spec.
    /// Only the root's are read — npm ignores a dependency's, and so does this.
    /// See [`crate::overrides`] for which forms are supported and why the rest
    /// are refused rather than dropped.
    pub overrides: BTreeMap<String, String>,
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
            #[serde(default)]
            overrides: Option<serde_json::Value>,
            #[serde(default)]
            resolutions: Option<serde_json::Value>,
        }
        let raw: Raw = serde_json::from_str(json).context("parsing the root package.json")?;

        // A `$name` reference reads the root's own declared range, so the
        // three dependency maps have to be indexed before the tables parse.
        // npm's precedence applies here too: a name in more than one map is
        // one dependency.
        let mut declared = raw.dev_dependencies.clone();
        declared.extend(raw.optional_dependencies.clone());
        declared.extend(raw.dependencies.clone());

        let table = overrides::merge(
            overrides::parse("overrides", raw.overrides.as_ref(), &declared)?,
            overrides::parse("resolutions", raw.resolutions.as_ref(), &declared)?,
        );

        Ok(Self {
            name: raw.name,
            dependencies: raw.dependencies,
            dev_dependencies: raw.dev_dependencies,
            optional_dependencies: raw.optional_dependencies,
            overrides: table,
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

/// Versions a **previous lockfile** already chose, by registry name.
///
/// This is the `target_path` hint the `node-maintainer` port dropped, arrived at
/// last (R773-T5): without it, "re-resolve an unchanged manifest" is only a
/// no-op while the registry holds still, and the whole point of a lockfile is
/// that it is a no-op *after* the registry moves. With it, an unchanged manifest
/// plus its own lock re-renders byte-identically across a publish.
///
/// Keyed by the name the *registry* spells, which is what a lock entry's locator
/// records — so an aliased dependency (`"pretend": "npm:real@^1"`) is preferred
/// under `real`, matching the key the walk asks with.
///
/// A preference is a *preference*, never a constraint: it applies only where the
/// edge's own range still admits it and the registry still publishes it. A
/// manifest edited to demand a newer major therefore resolves forward with no
/// flag and no `--force`, which is what makes this safe to leave on by default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreferredVersions {
    by_name: BTreeMap<String, BTreeSet<Version>>,
}

impl PreferredVersions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one `(registry name, version)` the previous lock selected. A name
    /// may have several — a lock nests a second copy exactly when it needed two
    /// versions, and both are worth preferring.
    pub fn insert(&mut self, name: impl Into<String>, version: Version) {
        self.by_name.entry(name.into()).or_default().insert(version);
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// How many distinct names carry a preference. The version count is not
    /// reported because nothing needs it and a lock's duplicate entries would
    /// make it a surprising number.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// What the previous lock chose for `name`, ascending.
    pub fn get(&self, name: &str) -> Option<&BTreeSet<Version>> {
        self.by_name.get(name)
    }
}

impl<S: Into<String>> FromIterator<(S, Version)> for PreferredVersions {
    fn from_iter<I: IntoIterator<Item = (S, Version)>>(iter: I) -> Self {
        let mut out = Self::new();
        for (name, version) in iter {
            out.insert(name, version);
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
///
/// `pub(crate)` only because [`Resolver::expand`] names it. The fields stay
/// module-private on purpose: the sole in-crate caller of `expand` is
/// [`Resolver::expand_dependencies_of`], and nothing outside this file should
/// be able to hand the walk an edge pointing at an already-placed node.
#[derive(Debug, Clone)]
pub(crate) struct Edge {
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
    preferred: PreferredVersions,
}

impl<'a, S: PackumentSource> Resolver<'a, S> {
    pub fn new(source: &'a S) -> Self {
        Self { source, preferred: PreferredVersions::new() }
    }

    /// Resolve preferring what a previous lockfile chose (R773-T5).
    ///
    /// Only [`Self::resolve`] reads this — it hands the table to the tree it
    /// builds, and every later pick, phase 2's auto-install included, reads it
    /// back off the tree. A `Resolver` built around an *existing* tree
    /// (`crate::peers`) therefore inherits the preferences without being told
    /// them, which is the point.
    pub fn preferring(mut self, preferred: PreferredVersions) -> Self {
        self.preferred = preferred;
        self
    }

    /// Build the tree.
    ///
    /// The same tree on every machine (R773-F9): there is no host parameter,
    /// because nothing in this walk asks what it is running on.
    pub fn resolve(&self, root: &RootManifest) -> Result<ResolvedTree> {
        let mut tree =
            ResolvedTree::new(root.name.clone(), root.overrides.clone(), self.preferred.clone());
        let mut queue: VecDeque<Edge> = VecDeque::new();

        for (name, value, optional) in root.edges() {
            match self.requirement(&mut tree, ResolvedTree::ROOT, name, value, optional)? {
                Requirement::Registry(edge) => {
                    let range = edge.range.clone();
                    tree.nodes[ResolvedTree::ROOT]
                        .requirements
                        .insert(name.to_string(), range);
                    queue.push_back(edge);
                }
                Requirement::Other { name, spec, optional } => {
                    self.add_other_node(&mut tree, ResolvedTree::ROOT, name, spec, optional);
                }
            }
        }

        self.expand(&mut tree, queue)?;
        recompute_optionality(&mut tree);
        Ok(tree)
    }

    /// Phase 1's BFS, run into an **existing** tree.
    ///
    /// [`Self::resolve`] is this plus a seed queue built from the root
    /// manifest. Phase 2's auto-install (R773-B8) is this plus a seed queue
    /// built from the auto-installed peer's own manifest — see
    /// [`Self::expand_dependencies_of`], which is the only other caller.
    ///
    /// A seed queue holds a node's **outgoing** edges, never an edge pointing
    /// *at* a node the caller already placed: [`Self::place`] runs for every
    /// node this walk creates, so enqueueing an edge at an existing node would
    /// re-decide a placement its caller made deliberately.
    pub(crate) fn expand(
        &self,
        tree: &mut ResolvedTree,
        mut queue: VecDeque<Edge>,
    ) -> Result<()> {
        while let Some(edge) = queue.pop_front() {
            // 1. Already satisfied by something visible from the requester?
            //    This is the check that makes a hoisted tree cheap: it costs no
            //    packument fetch, which is one of this ticket's two criteria.
            if let Some(existing) = tree.resolve_from(edge.from, &edge.name) {
                if let Some(version) = tree.nodes[existing].version() {
                    if edge.range.matches(version) {
                        link(tree, edge.from, existing, &edge.name);
                        continue;
                    }
                }
            }

            // 2. Fetch, and take the highest satisfying version.
            //
            //    Every way this can come up empty is an `optionalDependencies`
            //    skip when the edge is optional and an error when it is not —
            //    including a *transport* failure, which is the case R773-F6
            //    fixed: a `?` here aborted the whole resolve over a registry
            //    hiccup on a package the manifest said was allowed to be
            //    missing.
            let packument = match self.fetch(&edge.registry_name) {
                Ok(Some(packument)) => packument,
                Ok(None) => {
                    if record_optional_skip(tree, &edge, "it is not published on this registry") {
                        continue;
                    }
                    bail!(
                        "{} is not published on this registry (required by {})",
                        edge.registry_name,
                        describe(tree, edge.from)
                    );
                }
                Err(error) => {
                    if record_optional_skip(tree, &edge, &format!("{error:#}")) {
                        continue;
                    }
                    return Err(error);
                }
            };
            let preferred = tree.preferred.get(&edge.registry_name);
            let Some(chosen) = pick_version(&packument, &edge.range, preferred) else {
                // A tag that is not published and a range nothing satisfies are
                // different failures with different fixes, and reading the tag
                // out through `Display` made them the same sentence.
                let reason = match &edge.range {
                    VersionSpec::Tag(tag) => {
                        format!("this registry publishes no `{tag}` dist-tag for it")
                    }
                    range => format!("no published version satisfies {range}"),
                };
                if record_optional_skip(tree, &edge, &reason) {
                    continue;
                }
                bail!(
                    "cannot resolve {}: {reason} (required by {})",
                    edge.registry_name,
                    describe(tree, edge.from)
                );
            };

            // No platform gate here. `chosen`'s `os`/`cpu`/`libc` are copied
            // onto the node in step 3 and written into the lock; a package
            // that cannot run on the machine doing the install is dropped
            // there, by `mesofact-build`'s materializer, which is the only
            // layer that knows what that machine is (R773-F9).

            if let Some(reason) = &chosen.deprecated {
                tree.warn(ResolveWarning::Deprecated {
                    name: edge.registry_name.clone(),
                    version: chosen.version.clone(),
                    reason: reason.clone(),
                });
            }

            // 3. Create the node and hoist it into place.
            let child = self.add_registry_node(tree, &edge, &packument, chosen)?;
            self.place(tree, &edge, child)?;
            link(tree, edge.from, child, &edge.name);

            // 4. Enqueue the new node's own runtime dependencies. Peers were
            //    recorded in step 3 and are deliberately not enqueued.
            self.enqueue_dependencies(tree, child, &edge.registry_name, chosen, &mut queue)?;

            if tree.len() > MAX_NODES {
                bail!(
                    "dependency tree exceeded {MAX_NODES} nodes; this is a resolver bug or a \
                     pathological manifest, not a legitimately large install"
                );
            }
        }

        Ok(())
    }

    /// Resolve `node`'s own dependencies into the tree it already sits in.
    ///
    /// R773-B8's entry point: phase 2 hand-builds and places an auto-installed
    /// peer, then needs phase 1's walk to fill in what that peer itself
    /// depends on. `node` never enters the queue, so its caller's placement
    /// stands — only the subtree *below* it is placed by [`Self::place`].
    pub(crate) fn expand_dependencies_of(
        &self,
        tree: &mut ResolvedTree,
        node: NodeId,
        registry_name: &str,
        manifest: &VersionManifest,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        self.enqueue_dependencies(tree, node, registry_name, manifest, &mut queue)?;
        self.expand(tree, queue)
    }

    /// Turn `manifest`'s runtime dependency entries into queued edges from
    /// `node`, recording each range as one of `node`'s own requirements.
    /// `registry_name` is only used to name `node` in error context, which is
    /// why it is passed rather than derived — under an alias it differs from
    /// the node's install name.
    fn enqueue_dependencies(
        &self,
        tree: &mut ResolvedTree,
        node: NodeId,
        registry_name: &str,
        manifest: &VersionManifest,
        queue: &mut VecDeque<Edge>,
    ) -> Result<()> {
        for (name, value, optional) in dependency_edges(manifest) {
            match self
                .requirement(tree, node, name, value, optional)
                .with_context(|| format!("of {registry_name}@{}", manifest.version))?
            {
                Requirement::Registry(next) => {
                    tree.nodes[node]
                        .requirements
                        .insert(name.to_string(), next.range.clone());
                    queue.push_back(next);
                }
                Requirement::Other { name, spec, optional } => {
                    self.add_other_node(tree, node, name, spec, optional);
                }
            }
        }
        Ok(())
    }

    fn fetch(&self, registry_name: &str) -> Result<Option<Packument>> {
        self.source
            .packument(registry_name)
            .with_context(|| format!("fetching the packument for {registry_name}"))
    }

    /// Turn a `(name, value)` manifest entry into either a registry edge to
    /// walk or a non-registry node to record as-is.
    ///
    /// **This is where an override is applied**, and it is the reason one
    /// reaches a transitive dependency rather than only a direct one: the two
    /// edge sources — [`RootManifest::edges`] and [`dependency_edges`] — both
    /// funnel through here, so a single substitution covers both. An override
    /// replaces the whole spec string, so it can also change what *kind* of
    /// dependency the entry is (a range for a `file:` path, say); re-parsing
    /// rather than patching the range is what makes that fall out.
    fn requirement(
        &self,
        tree: &mut ResolvedTree,
        from: NodeId,
        name: &str,
        value: &str,
        optional: bool,
    ) -> Result<Requirement> {
        let value = match tree.overrides.get(name) {
            Some(replacement) if replacement != value => {
                let replacement = replacement.clone();
                let warning = ResolveWarning::OverrideApplied {
                    name: name.to_string(),
                    requester: describe(tree, from),
                    from: value.to_string(),
                    to: replacement.clone(),
                };
                tree.warn(warning);
                replacement
            }
            // An override that restates what was already asked for changed
            // nothing, and saying so would be noise in the channel.
            _ => value.to_string(),
        };
        // Both edge sources funnel through here, so this is also the one place
        // that can record *which* of a node's edges were optional — the input
        // `recompute_optionality` reads once the walk is done.
        if optional {
            tree.nodes[from].optional_deps.insert(name.to_string());
        }
        let spec = PackageSpec::from_dependency(name, &value)
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
            optional_deps: BTreeSet::new(),
            optional,
        });
        tree.nodes[from].children.insert(name.clone(), id);
        link(tree, from, id, &name);
        id
    }

    /// Hoist `child` as high as it can go, then attach it.
    ///
    /// Two outcomes, and no third: **the root** if nothing between the
    /// requester and the root objects, **the requester** if anything does.
    /// Walk up from the requester asking [`conflicts_at`] of each level; the
    /// first `true` ends the walk at `edge.from`, which is the "discharge by
    /// nesting under the requester" rule this module is built on.
    ///
    /// # Why not the deepest level that would have fit (R773-B1)
    ///
    /// Because bun doesn't, and bun is the oracle W318 §6 adopted. This was a
    /// port of `node-maintainer` 0.3.34's `place_child`
    /// (`src/resolver.rs:346-440`, Apache-2.0), which on a conflict stops the
    /// climb one level *below* the objecting node and places there — an
    /// intermediate position. That agrees with bun whenever the requester is a
    /// direct child of the root, which is why it survived every synthetic case,
    /// and disagrees as soon as a conflict appears two or more levels down:
    /// upstream gives `wrap-ansi-cjs/ansi-regex`, bun gives
    /// `wrap-ansi-cjs/strip-ansi/ansi-regex`.
    ///
    /// bun's `Tree.hoistDependency` recurses at the parent with its
    /// `as_defined` flag cleared and only the still-`as_defined` frame — the
    /// requester's — turns a rejection into a placement; every intermediate
    /// frame passes the rejection straight back down. So an intermediate level
    /// is never a placement site there, and is not one here either.
    ///
    /// Node resolves both layouts identically (the intermediate position is on
    /// the lookup chain from the requester), so this buys tree *shape* parity
    /// rather than behaviour: it is what lets the conformance corpus compare
    /// install paths against a real `bun.lock` with no waivers.
    ///
    /// Upstream's third break — a lockfile-supplied `target_path` hint — is
    /// still dropped, and R773-T5 did not restore it. A previous lock now
    /// reaches the walk as [`PreferredVersions`], read in [`pick_version`],
    /// which is upstream of this function and decides only *which version*
    /// exists. Where a copy lands stays a pure function of the tree — a lock
    /// that could pin a placement could pin one this walk would never produce,
    /// and re-resolving it would no longer be re-resolving.
    fn place(&self, tree: &mut ResolvedTree, edge: &Edge, child: NodeId) -> Result<()> {
        let name = tree.nodes[child].name.clone();
        let version = tree.nodes[child].version().cloned();

        let mut target = ResolvedTree::ROOT;
        let mut cursor = Some(edge.from);
        while let Some(current) = cursor {
            if conflicts_at(tree, current, edge, &name, version.as_ref()) {
                target = edge.from;
                break;
            }
            // Nothing here objects — ask the level above.
            cursor = tree.nodes[current].parent;
        }

        tree.nodes[child].parent = Some(target);
        // `BTreeMap::insert` overwrites silently, and an overwrite here would
        // orphan an already-placed node — its `parent` still pointing at
        // `target`, `target.children` no longer naming it. That is an
        // *unplacement*, in a walk whose whole premise is that placements are
        // never undone, and it would be invisible in a release build if this
        // were only a `debug_assert!`.
        //
        // Both outcomes are unreachable for a different reason, and the
        // ancestry test in `conflicts_at` is what makes each of them hold. The
        // ROOT outcome: a clean walk means no level on the chain holds an
        // unsatisfying node of this name — the objecting node's own parent
        // level always answers `true`, since its requesters are by
        // construction inside its subtree — and a *satisfying* one would have
        // been deduped in `expand` step 1 before a node was ever created. So a
        // clean walk means no node of this name on the chain at all. The
        // `edge.from` outcome: the first iteration asks about the requester
        // itself, so if it holds a child of this name we stop there — and the
        // same argument says a non-break on iteration 1 means it does not.
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

        let peers = peer_requirements(&edge.registry_name, chosen)?;

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
                os: chosen.os.clone(),
                cpu: chosen.cpu.clone(),
                libc: chosen.libc.clone(),
            }),
            parent: None,
            children: BTreeMap::new(),
            dependencies: BTreeMap::new(),
            requesters: BTreeSet::new(),
            requirements: BTreeMap::new(),
            peers,
            optional_deps: BTreeSet::new(),
            // Provisional: the edge that created the node is one path to it,
            // not necessarily every path. `recompute_optionality` decides.
            optional: edge.optional,
        });
        Ok(id)
    }
}

/// Whether `level` refuses to hold the copy of `name` at `version` that
/// [`Resolver::place`] is trying to hoist. Two ways it can, both upstream's:
///
/// 1. Something named the same already resolves from here **and** does not
///    satisfy what we were asked for **and** was wanted by someone inside this
///    level's subtree. Hoisting past it would shadow it. The ancestry test is
///    `node-maintainer`'s and is load-bearing, not decorative: without it a
///    node would object on behalf of a conflict living in a *sibling* subtree,
///    which is none of its business — and with it, the objecting node's own
///    parent level is guaranteed to object, which is what
///    [`Resolver::place`]'s clobber argument rests on.
/// 2. This level's own manifest requires that name at a range our chosen
///    version does not satisfy — a node may not be hoisted past a level whose
///    own `require(name)` it would break.
///
/// A version-less [`NodeSource::Other`] counts as unsatisfying under (1): phase
/// 1 cannot show that a `file:`/git node satisfies a range, so it does not
/// hoist a registry copy over one.
fn conflicts_at(
    tree: &ResolvedTree,
    level: NodeId,
    edge: &Edge,
    name: &str,
    version: Option<&Version>,
) -> bool {
    if let Some(resolved) = tree.resolve_from(level, name) {
        let unsatisfying = tree.nodes[resolved]
            .version()
            .is_none_or(|v| !edge.range.matches(v));
        if unsatisfying
            && tree.nodes[resolved]
                .requesters
                .iter()
                .any(|from| tree.is_ancestor(level, *from))
        {
            return true;
        }
    }

    if let (Some(required), Some(version)) = (tree.nodes[level].requirements.get(name), version) {
        if !required.matches(version) {
            return true;
        }
    }

    false
}

/// A manifest's `peerDependencies`, parsed, with `peerDependenciesMeta`
/// folded in.
///
/// Shared with [`crate::peers`]: an auto-installed peer is a node phase 2
/// builds by hand, and a node built by hand from a manifest has to declare
/// what that manifest declares or it lies about itself. `registry_name` names
/// the package in the error context only.
pub(crate) fn peer_requirements(
    registry_name: &str,
    chosen: &VersionManifest,
) -> Result<BTreeMap<String, PeerRequirement>> {
    let mut peers = BTreeMap::new();
    for (peer, range) in &chosen.peer_dependencies {
        let parsed = parse_peer_range(range).with_context(|| {
            format!("peer dependency {peer} of {registry_name}@{}", chosen.version)
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
    Ok(peers)
}

/// If `edge` is optional, record why it is being left out of the tree and say
/// so; otherwise say nothing and leave the caller to fail.
///
/// The three ways an edge can come up empty — unpublished, no satisfying
/// version, and a fetch that errored — all reach this, so an optional
/// dependency skipped for *any* of them is observable rather than silent.
fn record_optional_skip(tree: &mut ResolvedTree, edge: &Edge, reason: &str) -> bool {
    if !edge.optional {
        return false;
    }
    let warning = ResolveWarning::OptionalSkipped {
        name: edge.registry_name.clone(),
        requester: describe(tree, edge.from),
        reason: reason.to_string(),
    };
    tree.warn(warning);
    true
}

/// Decide [`Node::optional`] for every node, from the finished graph.
///
/// A node is **required** exactly when the root can reach it through edges none
/// of which were declared in an `optionalDependencies` map; everything else is
/// optional. Both halves matter and neither falls out of the per-edge flag the
/// node was created with:
///
/// - A package reached optionally by one requester and required by another is
///   required. Trusting the creating edge would mark it optional purely because
///   the optional requester was walked first, and the materializer would then
///   *silently skip* a package the tree needs.
/// - Everything below an optional package is optional, however its own parent
///   declared it. `sharp`'s per-platform natives declare their `libvips`
///   dependency as required; erroring `EBADPLATFORM` on one because its
///   optional grandparent could not run here would fail an install npm
///   performs.
///
/// Run at the end of phase 1 and again at the end of phase 2 ([`crate::peers`]),
/// which adds nodes of its own. Cheap — one BFS over the arena — and idempotent.
pub(crate) fn recompute_optionality(tree: &mut ResolvedTree) {
    let mut required = vec![false; tree.nodes.len()];
    let mut queue = VecDeque::from([ResolvedTree::ROOT]);
    required[ResolvedTree::ROOT] = true;
    while let Some(id) = queue.pop_front() {
        let node = &tree.nodes[id];
        let next: Vec<NodeId> = node
            .dependencies
            .iter()
            .filter(|(name, _)| !node.optional_deps.contains(*name))
            .map(|(_, dep)| *dep)
            .collect();
        for dep in next {
            if !required[dep] {
                required[dep] = true;
                queue.push_back(dep);
            }
        }
    }
    for (id, node) in tree.nodes.iter_mut().enumerate() {
        node.optional = !required[id];
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

/// The dist-tag npm calls `defaultTag` — the one a range consults before it
/// falls back to "highest satisfying".
pub(crate) const DEFAULT_TAG: &str = "latest";

/// The version `spec` selects: the previous lock's pick if there is one and
/// `spec` still admits it, else the `latest` dist-tag if it satisfies, else the
/// highest satisfying version.
///
/// `preferred` sits ahead of `latest` deliberately — a lockfile that lost to a
/// tag move would not be a lockfile. See [`PreferredVersions`] for why that is
/// still not a pin.
///
/// A dist-tag is looked up rather than compared, which is why a tag edge
/// always needs the packument even when an ancestor already provides the
/// package: `latest` cannot be checked against a version without the document
/// that defines it.
///
/// # Why `latest` wins over a higher satisfying version
///
/// This used to be a pure max-satisfying pick, and **R773-F7's conformance
/// corpus caught that as a real divergence** on its first run — which is the
/// single best argument for that corpus existing. `get-intrinsic` publishes
/// `1.3.1` while its `latest` tag points at `1.3.0`; against a `^1.2.4` edge in
/// a plain `express@4` tree, `bun install` and `pnpm install` **both** locked
/// `1.3.0` and this function returned `1.3.1`. Two independent implementations
/// agreeing is the evidence; the rule they are implementing is
/// `npm-pick-manifest`'s, where the `defaultTag` version is preferred whenever
/// it satisfies the range and the highest satisfying version is only the
/// fallback.
///
/// It is not a cosmetic difference. `get-intrinsic@1.3.1` declares dependencies
/// `1.3.0` does not, so the wrong pick pulled three extra packages
/// (`async-function`, `async-generator-function`, `generator-function`) into a
/// tree neither oracle had — a resolver that picks "higher" does not converge
/// on a *smaller* wrong answer, it diverges structurally.
///
/// Publishing a version above `latest` is how a maintainer ships a release
/// without promoting it, so the tag is a deliberate signal and the registry
/// offers no other way to send it.
///
/// # What this deliberately does NOT do
///
/// `npm-pick-manifest` also prefers a non-deprecated version over a deprecated
/// one. That is not implemented here, and this is not an oversight: no case in
/// the corpus exercises it, so adding it would be an unproven behaviour change
/// to the one function whose divergences are expensive to find. Deprecated
/// versions are still not skipped — both callers warn instead, through
/// [`ResolveWarning::Deprecated`] (R773-F6). Record a case that pins it down
/// before changing that.
pub(crate) fn pick_version<'p>(
    packument: &'p Packument,
    spec: &VersionSpec,
    preferred: Option<&BTreeSet<Version>>,
) -> Option<&'p VersionManifest> {
    match spec {
        // A dist-tag is a *subscription*, not a range: `"foo": "latest"` asks
        // to track whatever `latest` points at, and there is no "does this
        // still satisfy" question a preference could answer. So the lock does
        // not pin it, and a manifest spelled this way is the one shape whose
        // re-resolve is legitimately not a no-op across a tag move.
        VersionSpec::Tag(tag) => packument.dist_tag(tag),
        _ => {
            // The previous lock first, highest-first, and only where this
            // edge's own range still admits it and the registry still
            // publishes it. Both conditions matter: the first is what lets an
            // edited manifest resolve forward, the second is what stops an
            // unpublished version from being preferred into a lock that cannot
            // be installed.
            if let Some(preferred) = preferred {
                for version in preferred.iter().rev().filter(|v| spec.matches(v)) {
                    // Compared parsed, not as strings: a packument key is the
                    // registry's spelling of a version and need not be
                    // `Version::to_string`'s.
                    if let Some(manifest) = packument
                        .versions
                        .values()
                        .find(|m| Version::parse(&m.version).is_ok_and(|v| v == *version))
                    {
                        return Some(manifest);
                    }
                }
            }
            if let Some(latest) = packument.dist_tag(DEFAULT_TAG) {
                if Version::parse(&latest.version).is_ok_and(|v| spec.matches(&v)) {
                    return Some(latest);
                }
            }
            packument
                .versions
                .values()
                .filter_map(|manifest| {
                    Version::parse(&manifest.version)
                        .ok()
                        .filter(|version| spec.matches(version))
                        .map(|version| (version, manifest))
                })
                .max_by(|(a, _), (b, _)| a.cmp(b))
                .map(|(_, manifest)| manifest)
        }
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
