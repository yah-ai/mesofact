//! Phase 2 (R773-F4, W318 §6): resolve peers as a post-pass over the built
//! tree.
//!
//! Peers are resolved **after** the phase-1 tree exists, never during the
//! search. A DFS descends from the root propagating the peer context each
//! position provides, and at every node matches that node's declared
//! `peerDependencies` against what is actually visible from where it sits.
//!
//! # Instantiation, not backtracking
//!
//! When a package is hoisted somewhere its peer provider is wrong for one of
//! its requesters, the answer is to **nest a second copy of it under that
//! requester**, where the right provider is in scope. Nothing is re-resolved
//! and nothing is unplaced — the same no-backtracking premise phase 1 rests
//! on. npm's arborist does re-resolve; pnpm never does, and both Rust
//! implementations chose instantiation because it is simpler and produces
//! trees Node resolves identically (W318 §6).
//!
//! # The two caches, and why one of them is the opposite call from phase 1
//!
//! [`Walker::pure`] and [`Walker::seen`] are **load-bearing for complexity**:
//! without them this walk is exponential in the number of peer-carrying
//! packages, because the same package gets re-analysed once per distinct path
//! that reaches it.
//!
//! That is the *opposite* of the call taken in [`crate::resolve`], where an
//! in-walk packument memo was deliberately removed (R773-F3) — there the memo
//! made the "already satisfied by an ancestor issues no fetch" criterion a
//! tautology, because it answered the second lookup regardless of whether the
//! ancestor check worked. The distinction is what each cache is keyed on and
//! what it hides:
//!
//! | | phase 1's removed memo | phase 2's caches here |
//! |---|---|---|
//! | keyed on | package name | (identity, peer context) / subtree purity |
//! | hid | whether the walk *asked* | nothing a criterion measures |
//! | needed for | nothing — the disk cache already memoized | avoiding exponential re-analysis |
//!
//! **Do not "make these consistent."** They are different decisions about
//! different caches, and the tests measure different things:
//! `tests/resolve_tree.rs` counts `PackumentSource` calls, while
//! `tests/peer_resolution.rs` counts [`PeerStats::visited`] and asserts the
//! purity fast path fires.
//!
//! # Attribution
//!
//! The algorithm is pnpm's, from `pnpm/pnpm`
//! `pnpm/crates/resolving-deps-resolver/src/resolve_peers.rs` and its
//! `resolve_peers/{walker,cache}.rs` submodules (MIT). Taken from it: the
//! depth-first walk propagating a parent-refs map down the chain; the
//! `peersCache` keyed on package identity plus the resolved peer context, so a
//! revisit in a compatible context short-circuits; the `purePkgs` fast path,
//! where a package with no peers anywhere in its subtree is skipped without
//! recursing, populated bottom-up; and the cycle break by an `in_progress` set
//! over a post-order traversal.
//!
//! Not taken, because they are pnpm's *model* rather than the algorithm:
//! `depPath` construction and its peer suffix, per-importer/catalog handling,
//! `link:` remapping, and the `dedupe_peer_dependents` / `hoist_peers` passes.
//! Our layout is npm/bun-style hoisting with nesting (W318 §6's adopted layout
//! call), so an instance is a *second node in the tree* rather than a second
//! `depPath` key. See `NOTICE`.

use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::resolve::{
    describe, link, pick_version, Node, NodeId, NodeSource, PackumentSource, Resolution,
    ResolvedTree, MAX_NODES,
};
use crate::spec::{Version, VersionSpec};

/// Guard on how deep the DFS will go before deciding it is looking at a bug
/// rather than a dependency tree. Real trees are tens of levels deep.
const MAX_DEPTH: usize = 512;

/// What the walk did. Every field exists because a test asserts on it — the
/// purity fast path in particular is only provable with a counter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerStats {
    /// Nodes the walk actually descended into and analysed.
    pub visited: usize,
    /// Subtrees skipped whole because nothing in them declares a peer.
    pub pure_skips: usize,
    /// Revisits short-circuited by the `(identity, peer context)` cache.
    pub cache_hits: usize,
    /// Copies nested to give a requester its own peer context.
    pub instances: usize,
    /// Missing non-optional peers installed automatically (npm 7+ semantics).
    pub auto_installed: usize,
    /// Re-entries onto a node already on the walk stack.
    pub cycle_breaks: usize,
}

/// Something the install should tell the user about. npm warns and proceeds
/// for all of these; none of them fails a resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIssue {
    /// A provider is in scope but its version does not satisfy the range, and
    /// no requester's scope had a better one.
    Unsatisfied {
        /// Install path of the package that declared the peer.
        at: String,
        peer: String,
        range: String,
        found: String,
    },
    /// Nothing provided it. `auto_installed` is `false` when the automatic
    /// install could not find a satisfying version either.
    Missing {
        at: String,
        peer: String,
        range: String,
        auto_installed: bool,
    },
}

/// Phase 2's output. The tree itself is mutated in place.
#[derive(Debug, Clone, Default)]
pub struct PeerReport {
    /// Per node, which node provides each of its declared peers. Only
    /// satisfied peers appear; the rest are in [`Self::issues`].
    pub providers: BTreeMap<NodeId, BTreeMap<String, NodeId>>,
    pub issues: Vec<PeerIssue>,
    pub stats: PeerStats,
}

/// Resolve every peer requirement in `tree`, mutating it to add the instances
/// and auto-installs that requires.
pub fn resolve_peers<S: PackumentSource>(
    tree: &mut ResolvedTree,
    source: &S,
) -> Result<PeerReport> {
    let mut walker = Walker::new(tree);
    walker.walk(tree, ResolvedTree::ROOT, 0, source)?;
    Ok(walker.report)
}

/// The peer context a node sits in: which node provides each of its declared
/// peers, or `None` where nothing does. Half of the cache key.
type PeerContext = Vec<(String, Option<NodeId>)>;

struct Walker {
    /// Bottom-up purity: `pure[n]` iff neither `n` nor anything reachable
    /// through its dependency edges declares a peer. pnpm's `purePkgs`.
    pure: Vec<bool>,
    /// pnpm's `peersCache`: `(identity, peer context)` already analysed.
    ///
    /// Conservative by construction — the context holds `NodeId`s, so two
    /// structurally identical contexts built from different instances miss.
    /// A miss costs a re-walk; a false *hit* would be a wrong tree, and this
    /// key cannot produce one.
    seen: HashSet<(String, PeerContext)>,
    /// pnpm's `in_progress`: nodes currently on the walk stack.
    in_progress: HashSet<NodeId>,
    report: PeerReport,
}

impl Walker {
    fn new(tree: &ResolvedTree) -> Self {
        Self {
            pure: compute_purity(tree),
            seen: HashSet::new(),
            in_progress: HashSet::new(),
            report: PeerReport::default(),
        }
    }

    /// Depth-first, carrying the peer context implicitly: a node's context is
    /// whatever [`ResolvedTree::resolve_from`] yields at its position, which is
    /// exactly what Node itself will see at runtime.
    fn walk<S: PackumentSource>(
        &mut self,
        tree: &mut ResolvedTree,
        node: NodeId,
        depth: usize,
        source: &S,
    ) -> Result<()> {
        // The fast path, and the reason it is worth having: a subtree that
        // declares no peer anywhere is skipped WITHOUT RECURSING. On a tree
        // with no peers at all this fires once, at the root, and the whole
        // pass costs nothing.
        if self.pure[node] {
            self.report.stats.pure_skips += 1;
            return Ok(());
        }
        if depth > MAX_DEPTH {
            bail!(
                "peer resolution exceeded {MAX_DEPTH} levels at {}; this is a resolver bug",
                describe(tree, node)
            );
        }
        if !self.in_progress.insert(node) {
            // A peer cycle: `a` peers on `b`, `b` peers on `a`. Re-entry
            // terminates the descent; the outer frame's verdict stands.
            self.report.stats.cycle_breaks += 1;
            return Ok(());
        }

        let context = peer_context(tree, node);
        let key = (tree.identity(node), context);
        if self.seen.contains(&key) {
            self.report.stats.cache_hits += 1;
            self.in_progress.remove(&node);
            return Ok(());
        }
        self.seen.insert(key);
        self.report.stats.visited += 1;

        self.match_own_peers(tree, node, source)?;

        // Descend. A dependency whose peer context differs from what it
        // currently sees gets its own instance under this node FIRST, so the
        // descent walks the copy in its correct context.
        for (name, dependency) in tree.node(node).dependencies.clone() {
            let target = if self.needs_own_instance(tree, dependency, node) {
                self.instantiate(tree, dependency, node, &name)?
            } else {
                dependency
            };
            self.walk(tree, target, depth + 1, source)?;
        }

        self.in_progress.remove(&node);
        Ok(())
    }

    /// Match `node`'s declared peers against what is visible from its
    /// position, recording a provider or an issue for each.
    fn match_own_peers<S: PackumentSource>(
        &mut self,
        tree: &mut ResolvedTree,
        node: NodeId,
        source: &S,
    ) -> Result<()> {
        for (peer, requirement) in tree.node(node).peers.clone() {
            match tree.resolve_from(node, &peer) {
                Some(provider) => {
                    let satisfied = tree
                        .node(provider)
                        .version()
                        .is_some_and(|v| requirement.range.matches(v));
                    if satisfied {
                        self.record(node, &peer, provider);
                    } else {
                        // Instantiation under a requester with a better
                        // provider already happened on the way down, if such a
                        // requester existed. Reaching here means none did, so
                        // this is a genuine mismatch: npm warns and proceeds.
                        self.report.issues.push(PeerIssue::Unsatisfied {
                            at: tree.install_path(node),
                            peer: peer.clone(),
                            range: requirement.range.to_string(),
                            found: tree
                                .node(provider)
                                .version()
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "<non-registry>".to_string()),
                        });
                    }
                }
                None if requirement.optional => {}
                None => {
                    let installed =
                        self.auto_install(tree, node, &peer, &requirement.range, source)?;
                    self.report.issues.push(PeerIssue::Missing {
                        at: tree.install_path(node),
                        peer: peer.clone(),
                        range: requirement.range.to_string(),
                        auto_installed: installed.is_some(),
                    });
                    if let Some(provider) = installed {
                        self.record(node, &peer, provider);
                    }
                }
            }
        }
        Ok(())
    }

    fn record(&mut self, node: NodeId, peer: &str, provider: NodeId) {
        self.report
            .providers
            .entry(node)
            .or_default()
            .insert(peer.to_string(), provider);
    }

    /// npm 7+ auto-installs a missing non-optional peer.
    ///
    /// The ticket says "at the requester's level". Placed here at the *node's
    /// own* level — its hierarchy parent's `node_modules` — because those two
    /// differ once hoisting has moved the node: a package hoisted to the root
    /// does not have its requester's directory on its resolution chain, so a
    /// peer installed beside the requester would be invisible to the very
    /// package that declared it. Installing beside the declaring package is
    /// what makes it resolvable, which is the behaviour npm actually produces.
    ///
    /// # KNOWN GAP (R773-B8): the installed node's own dependencies are not
    ///
    /// The node created here has no dependency edges, and nothing enqueues it
    /// into phase 1's BFS — phase 1 is over by the time this runs, and its
    /// walk loop has no entry point that extends an existing tree. So
    /// auto-installing `react` gets you `react` and none of what `react`
    /// itself depends on, which materializes to an install that fails at
    /// `require()` rather than at resolve time.
    ///
    /// Pinned by `an_auto_installed_peers_own_dependencies_are_not_resolved`
    /// so it is a recorded limitation rather than a surprise. The fix is a
    /// seam in [`crate::resolve`] — factor the BFS body out of
    /// `Resolver::resolve` into something that takes an existing tree and a
    /// seed queue — which is a phase-1 change, and R773-F3 is already in
    /// review; hence a separate ticket rather than a widened one.
    fn auto_install<S: PackumentSource>(
        &mut self,
        tree: &mut ResolvedTree,
        node: NodeId,
        peer: &str,
        range: &VersionSpec,
        source: &S,
    ) -> Result<Option<NodeId>> {
        let level = tree.node(node).parent.unwrap_or(ResolvedTree::ROOT);
        if tree.node(level).children.contains_key(peer) {
            // Something already occupies the slot; `resolve_from` would have
            // found it, so reaching here means it is not this name.
            return Ok(None);
        }
        let Some(packument) = source.packument(peer)? else {
            return Ok(None);
        };
        let Some(chosen) = pick_version(&packument, range) else {
            return Ok(None);
        };
        let Ok(version) = Version::parse(&chosen.version) else {
            return Ok(None);
        };

        let seed = Node {
            name: peer.to_string(),
            source: NodeSource::Registry(Resolution {
                name: if packument.name.is_empty() {
                    peer.to_string()
                } else {
                    packument.name.clone()
                },
                version,
                tarball: chosen.dist.tarball.clone(),
                integrity: chosen.dist.integrity.clone(),
            }),
            parent: Some(level),
            children: BTreeMap::new(),
            dependencies: BTreeMap::new(),
            requesters: BTreeSet::new(),
            requirements: BTreeMap::new(),
            peers: BTreeMap::new(),
            optional: false,
        };
        // Declares no peers and has no dependency edges, so it is pure by
        // construction — the walk will reach it and skip it.
        let id = self.push_node(tree, seed, true)?;
        tree.node_mut(level).children.insert(peer.to_string(), id);
        link(tree, level, id, peer);
        self.report.stats.auto_installed += 1;
        Ok(Some(id))
    }

    /// Would `dependency` see a different peer provider from `parent`'s
    /// position than it sees from its own?
    ///
    /// This is the whole instantiation test. It is `false` for a package with
    /// no peers, and `false` when `dependency` is already a child of `parent`
    /// (its resolution chain then starts at itself and continues through
    /// `parent`, so the two agree) — which is what stops a copy from
    /// immediately wanting another copy.
    fn needs_own_instance(
        &self,
        tree: &ResolvedTree,
        dependency: NodeId,
        parent: NodeId,
    ) -> bool {
        let node = tree.node(dependency);
        if node.peers.is_empty() {
            return false;
        }
        node.peers
            .keys()
            .any(|peer| tree.resolve_from(parent, peer) != tree.resolve_from(dependency, peer))
    }

    /// Nest a copy of `dependency` under `parent` and re-point `parent`'s
    /// dependency edge at it.
    ///
    /// Additive only: the original node keeps its position and its other
    /// requesters. This is the instantiation that replaces npm's peer
    /// backtracking.
    fn instantiate(
        &mut self,
        tree: &mut ResolvedTree,
        dependency: NodeId,
        parent: NodeId,
        name: &str,
    ) -> Result<NodeId> {
        if let Some(existing) = tree.node(parent).children.get(name).copied() {
            // Already instantiated here on an earlier edge.
            link(tree, parent, existing, name);
            return Ok(existing);
        }
        let copy = self.deep_copy(tree, dependency, parent, name)?;
        self.report.stats.instances += 1;
        Ok(copy)
    }

    /// Copy `node` under `parent`, then reproduce its dependency resolutions.
    ///
    /// A dependency that still resolves to the same node from the copy's new
    /// position is simply re-linked. One that does not — because `parent`'s
    /// scope shadows it — is itself copied under the copy, so the copy's
    /// subtree resolves the way the original's did except where the peer
    /// context was deliberately changed.
    fn deep_copy(
        &mut self,
        tree: &mut ResolvedTree,
        node: NodeId,
        parent: NodeId,
        name: &str,
    ) -> Result<NodeId> {
        let original = tree.node(node);
        let seed = Node {
            name: name.to_string(),
            source: original.source.clone(),
            parent: Some(parent),
            children: BTreeMap::new(),
            dependencies: BTreeMap::new(),
            requesters: BTreeSet::new(),
            requirements: original.requirements.clone(),
            peers: original.peers.clone(),
            optional: original.optional,
        };
        // A copy declares exactly what the original declared, so it inherits
        // the original's purity verdict rather than being recomputed.
        let purity = self.pure[node];
        let copy = self.push_node(tree, seed, purity)?;
        tree.node_mut(parent).children.insert(name.to_string(), copy);
        // The requester now wants the copy, not the original. The original
        // keeps every other requester — nothing is unplaced.
        tree.node_mut(parent)
            .dependencies
            .insert(name.to_string(), copy);
        tree.node_mut(node).requesters.remove(&parent);
        tree.node_mut(copy).requesters.insert(parent);

        for (dep_name, dep) in tree.node(node).dependencies.clone() {
            if tree.resolve_from(copy, &dep_name) == Some(dep) {
                link(tree, copy, dep, &dep_name);
            } else {
                let nested = self.deep_copy(tree, dep, copy, &dep_name)?;
                link(tree, copy, nested, &dep_name);
            }
        }
        Ok(copy)
    }

    /// The only way this walk is allowed to add a node.
    ///
    /// [`Self::pure`] is indexed by `NodeId`, so a `tree.push` that did not
    /// extend it in the same step would panic the moment the walk reached the
    /// new node — and the walk reaches every instance it creates, by
    /// construction. Keeping the two in one function is what makes that
    /// impossible to get wrong rather than merely unlikely.
    fn push_node(&mut self, tree: &mut ResolvedTree, node: Node, pure: bool) -> Result<NodeId> {
        let id = tree.push(node);
        debug_assert_eq!(id, self.pure.len(), "purity vector fell out of step with the arena");
        self.pure.push(pure);
        self.grow(tree)?;
        Ok(id)
    }

    fn grow(&self, tree: &ResolvedTree) -> Result<()> {
        if tree.len() > MAX_NODES {
            bail!(
                "peer resolution exceeded {MAX_NODES} nodes; instantiation is not converging, \
                 which is a resolver bug"
            );
        }
        Ok(())
    }
}

/// What each of `node`'s declared peers currently resolves to. Half of the
/// cache key; empty for a package that declares none.
fn peer_context(tree: &ResolvedTree, node: NodeId) -> PeerContext {
    tree.node(node)
        .peers
        .keys()
        .map(|peer| (peer.clone(), tree.resolve_from(node, peer)))
        .collect()
}

/// pnpm's `purePkgs`, computed bottom-up: a node is pure when neither it nor
/// anything reachable through its dependency edges declares a peer.
///
/// Done as a monotone fixpoint rather than a topological sweep because the
/// dependency graph has cycles by construction, and purity only ever moves
/// from `true` to `false` — so the iteration converges, and it converges to the
/// answer a cycle-aware traversal would give.
fn compute_purity(tree: &ResolvedTree) -> Vec<bool> {
    let mut pure: Vec<bool> = tree
        .ids()
        .map(|id| tree.node(id).peers.is_empty())
        .collect();

    loop {
        let mut changed = false;
        for id in tree.ids() {
            if !pure[id] {
                continue;
            }
            if tree
                .node(id)
                .dependencies
                .values()
                .any(|dependency| !pure[*dependency])
            {
                pure[id] = false;
                changed = true;
            }
        }
        if !changed {
            return pure;
        }
    }
}
