//! **Tier A of the R773-F7 differential conformance corpus: the gate.**
//!
//! W318 §7 says the resolution algorithm is not the long pole — proving it
//! agrees with what the ecosystem's own installers do on real manifests is.
//! This module is that proof, run as a gate: it replays a checked-in corpus of
//! recorded cases and diffs `rnpm`'s resolved tree against the `bun.lock` and
//! `pnpm-lock.yaml` that `bun install` and `pnpm install` actually produced
//! for the same `package.json`.
//!
//! # Why the harness is here and not in `rnpm`
//!
//! The "what did the ecosystem produce" side needs two lockfile readers, and
//! this crate already has both — [`crate::install::parse_bun_lock_for_host`]
//! and [`crate::pnpm::parse_pnpm_lock`], each returning
//! [`crate::install::LockedPackage`]. Putting the harness in `rnpm` (which may
//! not depend on `mesofact-build` — that is the cycle its own header forbids)
//! or in `jsget` (whose header states W318 §9 makes it "not a slot for a second
//! resolution algorithm") would mean writing a third lockfile parser. This
//! crate also already consumes `rnpm` for [`crate::lock_entries`], so both
//! vocabularies are already in scope here and nowhere else.
//!
//! # Two tiers, and why the gate is only one of them
//!
//! A differential harness that needs the network, plus `bun`, plus `pnpm`, to
//! produce a verdict is a pipeline that goes red when the network blinks, and
//! a gate that cries wolf gets ignored. So:
//!
//! - **Tier A — this module.** Hermetic. It reads a case's `package.json`,
//!   resolves it against *recorded packuments* served from an on-disk cache
//!   under [`CachePolicy::Offline`], and diffs the result against the recorded
//!   lockfiles. No network, no `bun`, no `pnpm`, no clock: `Offline` never
//!   consults [`rnpm::CachedPackument::age`], so a checked-in fixture cannot go
//!   stale, and the transport this module installs ([`NoTransport`]) *errors on
//!   every request* rather than promising not to make one. It passes on a
//!   machine with no JS toolchain at all. This is what the QED pipeline runs.
//! - **Tier B — the recorder,** in `src/bin/mesofact-conformance.rs`. It runs
//!   the real `bun install` and `pnpm install`, captures their lockfiles and
//!   every packument the resolve touched, and writes a new case. It is opt-in,
//!   needs network + both package managers, and is never on the gate's path.
//!   Only [`prune_packument`] — the pure, budget-bounded reduction of a
//!   recorded packument — lives here, because it is the one part of recording
//!   whose correctness the gate depends on.
//!
//! # What "agree" means (three different things, deliberately)
//!
//! The diff unit is **install path → `name@version`**, which is exactly what
//! [`rnpm::ResolvedTree::install_path`] plus [`rnpm::Node::resolution`] gives on
//! one side and what a [`LockedPackage`]'s `dest_rel` plus
//! [`PackageSource::Registry`] gives on the other.
//!
//! - **bun is the primary oracle, compared on layout AND identity.** Exact set
//!   equality of install path → `name@version`. W318 §6 adopted bun/npm-style
//!   hoisting, so bun's tree shape is the one this resolver is *supposed* to
//!   reproduce, and a difference in where a package landed is a real failure.
//! - **pnpm is a secondary oracle, compared on identity and version ONLY.**
//!   pnpm's isolated layout nests differently from bun's hoisted one *by
//!   design* — that is not a bug in either, and asserting on it would make the
//!   gate red for a disagreement nobody intends to fix. So the pnpm side is
//!   reduced to `name → {versions}` and compared over the names present in
//!   both trees. **Do not "fix" this into a path comparison.** Range-semantics
//!   divergence from pnpm is the specific failure R773-F7 exists to catch, and
//!   it shows up as a different *version*, which this does catch. (`rnpm` pins
//!   `node-semver` at `=2.2.0` to match pacquet for exactly this reason; see
//!   that crate's `Cargo.toml`.)
//! - **Non-registry nodes are compared on neither.** A `file:`/git/tarball dep
//!   is a [`rnpm::NodeSource::Other`] on one side and a
//!   [`PackageSource::Link`] on the other, carries no version, and has nothing
//!   to diverge about. The recorder refuses to record a case containing one.
//!
//! # There is no host in this comparison (R773-F9)
//!
//! A `bun.lock` is portable: `esbuild@0.24.0` locks all twenty-four
//! `@esbuild/*` natives, each with its own `os`/`cpu` meta. Since R773-F9 so is
//! the resolver's tree — it records the platform gates instead of applying them
//! — so the two sides are directly comparable and the diff is the same on every
//! machine. The bun oracle is therefore read with every gate wide open
//! ([`crate::install::parse_bun_lock_for_host`] against an all-`None` host),
//! which is the same reading [`lock_selections`] uses when the recorder decides
//! which packuments to keep.
//!
//! This replaces a pinned `[host]` block in every `case.toml`, which existed to
//! make a *host-specific* resolve comparable across machines: both sides were
//! filtered to one recorded host so the verdict did not vary with whoever ran
//! the gate. With nothing filtering on either side, pinning a host would only
//! be a way to compare two smaller sets.
//!
//! The pnpm side still gates on the running host —
//! [`crate::pnpm::parse_pnpm_lock`] is not parameterized. That is tolerable for
//! the same reason it always was: the pnpm comparison is over names present in
//! **both** trees, and host filtering can only shrink that intersection, never
//! invent a disagreement inside it.
//!
//! # Recorded divergences
//!
//! Where a tool legitimately disagrees, `case.toml` records it as a
//! `[[known_divergence]]` with a reason rather than the corpus pretending it
//! does not exist. A waiver names ONE divergence exactly — oracle, key, kind,
//! and the versions on both sides — so it cannot go on excusing a *different*
//! disagreement that later appears at the same key. A waiver that stops
//! matching is itself a failure ([`Divergence::StaleWaiver`]); otherwise the
//! file rots into a list of things that used to be true. See
//! [`KnownDivergence`].

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rnpm::transport::{Request, Response, Transport};
use rnpm::{
    CachePolicy, Packument, RegistryClient, RegistrySource, ResolvedTree, Resolver, RootManifest,
    Version,
};

use crate::install::PackageSource;

/// Cache namespace for the recorded packuments, and the directory component
/// under `packuments/`.
///
/// Re-exported rather than restated: it has to be the *same* string a real
/// resolve uses or a recorded cache entry cannot be read back (R773-T5 moved
/// the definition next to the registry URL it namespaces).
pub use crate::install::REGISTRY_ID;

/// Where a case keeps its recorded packuments, relative to the case directory.
pub const PACKUMENT_DIR: &str = "packuments";

/// The corpus root, relative to this crate's manifest directory.
pub const CORPUS_DIR: &str = "tests/corpus";

// ── The case format ─────────────────────────────────────────────────────────

/// A corpus case's `case.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CaseSpec {
    pub name: String,
    /// What divergence this case is *for*. The corpus is authored for
    /// divergence, not for coverage, so a case that cannot say what it stresses
    /// does not earn its bytes.
    pub description: String,
    /// When the lockfiles and packuments were captured, as `YYYY-MM-DD`. Not
    /// consulted by the gate — [`CachePolicy::Offline`] never ages an entry —
    /// but a human refreshing the corpus needs to know how old it is.
    pub recorded_at: String,
    #[serde(default)]
    pub compare: CompareSpec,
    #[serde(default)]
    pub known_divergence: Vec<KnownDivergence>,
}

/// Every platform gate wide open — `None` on an axis means *admit everything*.
///
/// Both the gate and the recorder read the bun oracle this way (R773-F9). The
/// resolver's tree is portable, so filtering the oracle to some host would
/// compare two arbitrary subsets and call the difference a divergence. Shared
/// with the lock reader that answers the same question for a different reason
/// (R773-T5).
use crate::install::admit_every_platform as admit_everything;

/// Which oracles this case is diffed against. Both default to on; a case turns
/// one off only when the recorder could not produce that lockfile, and says why
/// in `description`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompareSpec {
    #[serde(default = "yes")]
    pub bun: bool,
    #[serde(default = "yes")]
    pub pnpm: bool,
}

fn yes() -> bool {
    true
}

impl Default for CompareSpec {
    fn default() -> Self {
        Self { bun: true, pnpm: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Oracle {
    Bun,
    Pnpm,
}

impl Oracle {
    pub fn as_str(self) -> &'static str {
        match self {
            Oracle::Bun => "bun",
            Oracle::Pnpm => "pnpm",
        }
    }
}

/// Which shape of disagreement a [`Divergence`] is. Named separately from the
/// enum so a `case.toml` waiver can pin it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DivergenceKind {
    Missing,
    Extra,
    Mismatch,
}

/// A disagreement this case accepts, with the reason it is accepted.
///
/// # A waiver names ONE divergence, exactly
///
/// `oracle` + `key` alone is not enough, and the loose version of this was a
/// real hole: a waiver written because `ansi-regex@5.0.1` sat one level too
/// high went on suppressing that key even if the version changed to `2.0.0`, or
/// the divergence mutated from an [`Divergence::Extra`] into a
/// [`Divergence::Mismatch`]. That is how a conformance gate rots into
/// decoration — it keeps passing while silently excusing something nobody ever
/// looked at.
///
/// So a waiver must reproduce the divergence's `kind` **and both of its
/// version-bearing sides**, and it matches only on an exact hit. Change any of
/// them and the waiver stops matching, which surfaces as a
/// [`Divergence::StaleWaiver`] — a failure. There is no way to write a waiver
/// here that excuses a disagreement its author did not see.
///
/// This matters most on the **pnpm** side, which is the loose one: bun keys are
/// exact install paths ([`diff_by_path`]) but pnpm keys are bare package names
/// ([`diff_by_identity`]), so `key = "ansi-styles"` on its own would waive that
/// name's entire version-set anywhere in the case. Pinning `expected`/`found`
/// is what makes a pnpm waiver narrow enough to be honest.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KnownDivergence {
    pub oracle: Oracle,
    /// The install path (bun) or package name (pnpm) that diverges — the same
    /// key [`Divergence::key`] reports.
    pub key: String,
    /// Which shape of divergence is excused. Required: a `missing` waiver must
    /// not go on excusing the `mismatch` that replaces it.
    pub kind: DivergenceKind,
    /// What the oracle had, verbatim as [`Divergence`] renders it
    /// (`name@version`, or `a@1 + a@2` for a pnpm version-set). Present for
    /// `missing` and `mismatch`; absent for `extra`, where the oracle had
    /// nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// What the resolver produced. Present for `extra` and `mismatch`; absent
    /// for `missing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<String>,
    pub why: String,
}

impl KnownDivergence {
    /// Whether this waiver excuses exactly `divergence` — same oracle, same
    /// key, same kind, same versions on both sides.
    fn excuses(&self, divergence: &Divergence) -> bool {
        divergence.waivable()
            && self.oracle == divergence.oracle()
            && self.key == divergence.key()
            && Some(self.kind) == divergence.kind()
            && self.expected.as_deref() == divergence.expected()
            && self.found.as_deref() == divergence.found()
    }

    /// How the waiver reads when it has gone stale — it has to name what it
    /// pinned, or the next reader cannot tell which of the four fields moved.
    fn describe(&self) -> String {
        let sides = match (&self.expected, &self.found) {
            (Some(e), Some(f)) => format!("{e} vs {f}"),
            (Some(e), None) => e.clone(),
            (None, Some(f)) => f.clone(),
            (None, None) => "nothing".to_string(),
        };
        format!("{:?} {sides}", self.kind)
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────

/// One package identity as both sides of the diff spell it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PkgId {
    /// The *registry* name, which is not the install name under an alias:
    /// `"foo": "npm:bar@^1"` installs at `foo` and locks `bar`.
    pub name: String,
    pub version: String,
}

impl std::fmt::Display for PkgId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

/// One way a resolved tree failed to agree with an oracle.
///
/// Every variant names the package **and** the version, per R773-F7's own
/// verification bullet: "trees differ" is a useless failure message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    /// The oracle placed a package the resolver did not produce at all.
    Missing { oracle: Oracle, key: String, expected: String },
    /// The resolver produced a package the oracle did not place.
    Extra { oracle: Oracle, key: String, found: String },
    /// Both sides have this key and disagree about what is there.
    Mismatch { oracle: Oracle, key: String, expected: String, found: String },
    /// A `[[known_divergence]]` waiver that no longer matches anything. Kept as
    /// a failure so the waiver list cannot rot into a record of things that
    /// used to be true.
    StaleWaiver { oracle: Oracle, key: String, why: String },
}

impl Divergence {
    pub fn oracle(&self) -> Oracle {
        match self {
            Divergence::Missing { oracle, .. }
            | Divergence::Extra { oracle, .. }
            | Divergence::Mismatch { oracle, .. }
            | Divergence::StaleWaiver { oracle, .. } => *oracle,
        }
    }

    pub fn key(&self) -> &str {
        match self {
            Divergence::Missing { key, .. }
            | Divergence::Extra { key, .. }
            | Divergence::Mismatch { key, .. }
            | Divergence::StaleWaiver { key, .. } => key,
        }
    }

    /// Waivers are bookkeeping, not disagreements — they are never waivable
    /// themselves.
    fn waivable(&self) -> bool {
        !matches!(self, Divergence::StaleWaiver { .. })
    }

    /// Which shape this is. `None` for [`Divergence::StaleWaiver`], which is
    /// not a disagreement and has no kind a waiver could pin.
    pub fn kind(&self) -> Option<DivergenceKind> {
        match self {
            Divergence::Missing { .. } => Some(DivergenceKind::Missing),
            Divergence::Extra { .. } => Some(DivergenceKind::Extra),
            Divergence::Mismatch { .. } => Some(DivergenceKind::Mismatch),
            Divergence::StaleWaiver { .. } => None,
        }
    }

    /// What the oracle had, where it had anything.
    pub fn expected(&self) -> Option<&str> {
        match self {
            Divergence::Missing { expected, .. }
            | Divergence::Mismatch { expected, .. } => Some(expected),
            Divergence::Extra { .. } | Divergence::StaleWaiver { .. } => None,
        }
    }

    /// What the resolver produced, where it produced anything.
    pub fn found(&self) -> Option<&str> {
        match self {
            Divergence::Extra { found, .. } | Divergence::Mismatch { found, .. } => Some(found),
            Divergence::Missing { .. } | Divergence::StaleWaiver { .. } => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Divergence::Missing { oracle, key, expected } => format!(
                "{o} locked {expected} at {key:?}; the resolver produced nothing there",
                o = oracle.as_str()
            ),
            Divergence::Extra { oracle, key, found } => format!(
                "the resolver produced {found} at {key:?}; {o} locked nothing there",
                o = oracle.as_str()
            ),
            Divergence::Mismatch { oracle, key, expected, found } => format!(
                "at {key:?}: {o} locked {expected}, the resolver produced {found}",
                o = oracle.as_str()
            ),
            Divergence::StaleWaiver { oracle, key, why } => format!(
                "stale [[known_divergence]] for {o} at {key:?} ({why}) — it no longer diverges, \
                 so delete the waiver",
                o = oracle.as_str()
            ),
        }
    }
}

/// What one case's replay found.
#[derive(Debug, Clone)]
pub struct CaseReport {
    pub name: String,
    pub dir: PathBuf,
    /// Registry packages in the resolved tree (non-registry nodes excluded —
    /// see the module header).
    pub resolver_packages: usize,
    /// Registry entries the bun lock places for this host, when compared.
    pub bun_packages: Option<usize>,
    /// Distinct package names the pnpm lock places for this host, when
    /// compared.
    pub pnpm_names: Option<usize>,
    pub divergences: Vec<Divergence>,
    /// Waivers that fired, for the report. Not failures.
    pub waived: Vec<KnownDivergence>,
    /// [`rnpm::ResolveWarning`]s, rendered. Informational: every one describes
    /// a *successful* resolve, so none of them fails the gate.
    pub resolve_warnings: Vec<String>,
}

impl CaseReport {
    pub fn passed(&self) -> bool {
        self.divergences.is_empty()
    }
}

/// What a whole corpus run found.
#[derive(Debug, Clone)]
pub struct CorpusReport {
    pub root: PathBuf,
    pub cases: Vec<CaseReport>,
}

impl CorpusReport {
    pub fn passed(&self) -> bool {
        self.cases.iter().all(CaseReport::passed)
    }

    pub fn divergence_count(&self) -> usize {
        self.cases.iter().map(|c| c.divergences.len()).sum()
    }

    /// A human/CI-readable rendering. One line per case, then every divergence
    /// spelled out with its package and version — the QED step's whole output.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for case in &self.cases {
            let verdict = if case.passed() { "ok  " } else { "FAIL" };
            let _ = writeln!(
                out,
                "  {verdict} — {name} ({n} packages{bun}{pnpm}{waived})",
                name = case.name,
                n = case.resolver_packages,
                bun = case
                    .bun_packages
                    .map(|n| format!(", bun {n}"))
                    .unwrap_or_default(),
                pnpm = case
                    .pnpm_names
                    .map(|n| format!(", pnpm {n} names"))
                    .unwrap_or_default(),
                waived = if case.waived.is_empty() {
                    String::new()
                } else {
                    format!(", {} waived", case.waived.len())
                },
            );
            for d in &case.divergences {
                let _ = writeln!(out, "         {}", d.describe());
            }
        }
        let cases = self.cases.len();
        let failed = self.cases.iter().filter(|c| !c.passed()).count();
        if failed == 0 {
            let _ = writeln!(out, "PASSED: {cases} cases agree with their oracles");
        } else {
            let _ = writeln!(
                out,
                "FAILED: {failed} of {cases} cases diverged ({} divergences)",
                self.divergence_count()
            );
        }
        out
    }
}

// ── Tier A ──────────────────────────────────────────────────────────────────

/// A [`Transport`] that refuses.
///
/// [`CachePolicy::Offline`] already promises no request is made — a cache hit
/// is served without touching the transport and a miss is a hard error naming
/// the package. This makes the promise structural instead: if the policy ever
/// regressed, the corpus would fail loudly here rather than quietly reaching
/// registry.npmjs.org and passing for the wrong reason on a machine that
/// happens to have network.
pub struct NoTransport;

impl Transport for NoTransport {
    fn get(&self, request: &Request) -> Result<Response> {
        bail!(
            "the conformance corpus is offline by construction, but something requested {} — \
             either a packument is missing from the case's {PACKUMENT_DIR}/ (re-record the case) \
             or the cache policy is no longer Offline",
            request.url
        )
    }
}

/// The endpoint every case resolves against. One place, so the gate, the
/// recorder and a real `lock` run cannot disagree about which registry a cache
/// entry came from — `registry_id` is a cache namespace, so a mismatch would
/// read as a total miss.
pub use crate::install::registry_endpoint;

/// Every `(registry name, version)` an oracle lockfile selects, whatever the
/// platform gates say.
///
/// The recorder's packument prune anchors on this: a version only *bun* or only
/// *pnpm* chose has to survive, or the fixture would hide the very divergence
/// the case exists to record. Hence the deliberately empty host — every field
/// `None` means *admit everything* at every gate, which is the opposite of what
/// the gate itself wants and exactly what recording wants.
///
/// Dispatches on the file name, matching [`crate::install`]'s own vocabulary.
pub fn lock_selections(lock: &Path) -> Result<Vec<(String, String)>> {
    let name = lock
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let locked = match name {
        "bun.lock" => crate::install::parse_bun_lock_for_host(lock, &admit_everything())?,
        "pnpm-lock.yaml" => crate::pnpm::parse_pnpm_lock(lock)?,
        other => bail!("{other:?} is not a lockfile the conformance corpus records"),
    };
    Ok(locked
        .iter()
        .filter_map(|entry| match &entry.source {
            PackageSource::Registry { name, version, .. } => {
                Some((name.clone(), version.clone()))
            }
            PackageSource::Link { .. } => None,
        })
        .collect())
}

/// Every case directory under `root`, in name order.
///
/// A directory without a `case.toml` is not a case and is skipped, so scratch
/// output left next to the corpus cannot fail the gate.
pub fn case_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    let entries = std::fs::read_dir(root)
        .with_context(|| format!("reading the conformance corpus at {}", root.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() && path.join("case.toml").is_file() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Replay every case under `root`.
pub fn check_corpus(root: &Path) -> Result<CorpusReport> {
    let dirs = case_dirs(root)?;
    if dirs.is_empty() {
        bail!(
            "no conformance cases under {} — a corpus with no cases is a gate that cannot fail",
            root.display()
        );
    }
    let mut cases = Vec::with_capacity(dirs.len());
    for dir in dirs {
        cases.push(
            check_case(&dir)
                .with_context(|| format!("replaying conformance case {}", dir.display()))?,
        );
    }
    Ok(CorpusReport { root: root.to_path_buf(), cases })
}

pub fn load_spec(dir: &Path) -> Result<CaseSpec> {
    let path = dir.join("case.toml");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

/// Resolve one case's `package.json` against its recorded packuments — the
/// whole of the resolver side, and the only entry point the recorder shares
/// with the gate so the two cannot drift.
///
/// Runs **both** phases: [`Resolver::resolve`] builds the tree and
/// [`rnpm::resolve_peers`] applies the phase-2 peer post-pass. Both oracles
/// auto-install peers by default (pnpm writes `autoInstallPeers: true` into
/// every lock it mints), so skipping phase 2 would make every case containing a
/// peer dependency diverge for a reason that has nothing to do with resolution.
pub fn resolve_case(dir: &Path) -> Result<ResolvedTree> {
    let manifest_path = dir.join("package.json");
    let json = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest = RootManifest::from_package_json(&json)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let client = RegistryClient::new(dir.join(PACKUMENT_DIR), NoTransport)
        .with_policy(CachePolicy::Offline);
    let endpoint = registry_endpoint();
    let source = RegistrySource::new(&client, &endpoint);

    let mut tree = Resolver::new(&source).resolve(&manifest)?;
    rnpm::resolve_peers(&mut tree, &source)?;
    Ok(tree)
}

/// Replay one case and diff it against its oracles.
pub fn check_case(dir: &Path) -> Result<CaseReport> {
    let spec = load_spec(dir)?;
    let tree = resolve_case(dir)?;

    let ours = tree_by_install_path(&tree);
    let mut divergences = Vec::new();

    let mut bun_packages = None;
    if spec.compare.bun {
        let path = dir.join("bun.lock");
        let locked = crate::install::parse_bun_lock_for_host(&path, &admit_everything())
            .with_context(|| format!("reading the bun oracle {}", path.display()))?;
        let theirs = oracle_by_install_path(&locked);
        bun_packages = Some(theirs.len());
        diff_by_path(Oracle::Bun, &theirs, &ours, &mut divergences);
    }

    let mut pnpm_names = None;
    if spec.compare.pnpm {
        let path = dir.join("pnpm-lock.yaml");
        let locked = crate::pnpm::parse_pnpm_lock(&path)
            .with_context(|| format!("reading the pnpm oracle {}", path.display()))?;
        let theirs = versions_by_name(&oracle_by_install_path(&locked));
        pnpm_names = Some(theirs.len());
        diff_by_identity(&theirs, &versions_by_name(&ours), &mut divergences);
    }

    let waived = apply_waivers(&spec.known_divergence, &mut divergences);

    Ok(CaseReport {
        name: spec.name,
        dir: dir.to_path_buf(),
        resolver_packages: ours.len(),
        bun_packages,
        pnpm_names,
        divergences,
        waived,
        resolve_warnings: tree.warnings().iter().map(|w| w.to_string()).collect(),
    })
}

/// The resolver's registry nodes, keyed by install path.
///
/// [`rnpm::ResolvedTree::install_path`] is already the chain of install names
/// joined with `/`, which is the same shape [`oracle_by_install_path`] derives
/// from a `dest_rel` — so nothing here computes a path.
pub fn tree_by_install_path(tree: &ResolvedTree) -> BTreeMap<String, PkgId> {
    let mut out = BTreeMap::new();
    for (id, node) in tree.packages() {
        // Non-registry nodes carry no version and have nothing to diverge
        // about; see the module header.
        if let Some(resolution) = node.resolution() {
            out.insert(
                tree.install_path(id),
                PkgId {
                    name: resolution.name.clone(),
                    version: resolution.version.to_string(),
                },
            );
        }
    }
    out
}

/// An oracle's registry entries, keyed by install path in the resolver's
/// spelling.
fn oracle_by_install_path(locked: &[crate::install::LockedPackage]) -> BTreeMap<String, PkgId> {
    let mut out = BTreeMap::new();
    for entry in locked {
        if let PackageSource::Registry { name, version, .. } = &entry.source {
            out.insert(
                install_path_of(&entry.dest_rel),
                PkgId { name: name.clone(), version: version.clone() },
            );
        }
    }
    out
}

/// A `dest_rel` (`node_modules/@babel/core/node_modules/convert-source-map`) in
/// the resolver's install-path spelling (`@babel/core/convert-source-map`).
///
/// Dropping every `node_modules` segment is exact rather than approximate: npm
/// refuses `node_modules` as a package name, so no real segment can be one.
fn install_path_of(dest_rel: &Path) -> String {
    dest_rel
        .to_string_lossy()
        .split('/')
        .filter(|segment| *segment != "node_modules" && !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// `name → {versions}`, the identity-only projection the pnpm comparison uses.
fn versions_by_name(by_path: &BTreeMap<String, PkgId>) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in by_path.values() {
        out.entry(id.name.clone()).or_default().insert(id.version.clone());
    }
    out
}

/// Exact set equality over install path → identity. bun only.
fn diff_by_path(
    oracle: Oracle,
    theirs: &BTreeMap<String, PkgId>,
    ours: &BTreeMap<String, PkgId>,
    out: &mut Vec<Divergence>,
) {
    let keys: BTreeSet<&String> = theirs.keys().chain(ours.keys()).collect();
    for key in keys {
        match (theirs.get(key), ours.get(key)) {
            (Some(t), Some(o)) if t != o => out.push(Divergence::Mismatch {
                oracle,
                key: key.clone(),
                expected: t.to_string(),
                found: o.to_string(),
            }),
            (Some(_), Some(_)) => {}
            (Some(t), None) => out.push(Divergence::Missing {
                oracle,
                key: key.clone(),
                expected: t.to_string(),
            }),
            (None, Some(o)) => out.push(Divergence::Extra {
                oracle,
                key: key.clone(),
                found: o.to_string(),
            }),
            (None, None) => unreachable!("key came from one of the two maps"),
        }
    }
}

/// Version-set equality over the names present in **both** trees. pnpm only —
/// see the module header for why layout is excluded and why a name present on
/// only one side is not a failure.
fn diff_by_identity(
    theirs: &BTreeMap<String, BTreeSet<String>>,
    ours: &BTreeMap<String, BTreeSet<String>>,
    out: &mut Vec<Divergence>,
) {
    for (name, their_versions) in theirs {
        let Some(our_versions) = ours.get(name) else { continue };
        if their_versions != our_versions {
            out.push(Divergence::Mismatch {
                oracle: Oracle::Pnpm,
                key: name.clone(),
                expected: render_versions(name, their_versions),
                found: render_versions(name, our_versions),
            });
        }
    }
}

fn render_versions(name: &str, versions: &BTreeSet<String>) -> String {
    versions
        .iter()
        .map(|v| format!("{name}@{v}"))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Drop divergences a case has waived, and add a [`Divergence::StaleWaiver`]
/// for every waiver that matched nothing. Returns the waivers that fired.
///
/// Matching is exact — see [`KnownDivergence`]. A waiver whose pinned versions
/// or kind no longer describe what the replay found does NOT suppress the new
/// divergence: it fails twice over, once as the unexcused divergence itself and
/// once as the stale waiver, which is the pair of messages that tells a reader
/// both what changed and which waiver to delete.
fn apply_waivers(
    waivers: &[KnownDivergence],
    divergences: &mut Vec<Divergence>,
) -> Vec<KnownDivergence> {
    let mut fired = Vec::new();
    for waiver in waivers {
        let before = divergences.len();
        divergences.retain(|d| !waiver.excuses(d));
        if divergences.len() == before {
            divergences.push(Divergence::StaleWaiver {
                oracle: waiver.oracle,
                key: waiver.key.clone(),
                why: format!("{} — {}", waiver.describe(), waiver.why),
            });
        } else {
            fired.push(waiver.clone());
        }
    }
    fired
}

// ── The one piece of recording the gate depends on ──────────────────────────

/// Reduce a packument's `versions` map to a bounded window, preserving the
/// resolution decisions the case exists to test.
///
/// **Why this has to exist at all.** The abbreviated packument for `react` is
/// 2.9 MB and `typescript`'s is 8.7 MB, so a corpus that recorded them verbatim
/// would be tens of megabytes of fixture for a dozen cases. Corpus *data* is
/// not source and need not fit a test-to-source budget, but that is not a
/// licence to check in a registry mirror.
///
/// **Why it is safe.** Pruning can never change which version a *recorded*
/// range selected: the selected version is by definition the max-satisfying
/// one, so removing versions leaves it still selected. The real risk runs the
/// other way — prune so hard that only the right answer remains and a
/// range-semantics regression picks it by default, and the case becomes a false
/// green. So this keeps enough of the neighbourhood that the decision stays
/// non-trivial:
///
/// 1. every version in `keep` — the versions the resolver, bun **and** pnpm
///    selected. All three, because pruning to only what the resolver picked
///    would delete the very version a divergence is *about*;
/// 2. every dist-tag target, since a `foo@latest` spec resolves through the tag
///    table before it ever touches a range;
/// 3. every release (non-prerelease) sharing a major with a kept version — so
///    max-satisfying still has to beat lower candidates that also match;
/// 4. the highest release of every *other* major — so a `^18` still has to
///    reject a 19.x that is present rather than absent.
///
/// A prerelease is dropped unless rule 1 or 2 keeps it: an npm range admits a
/// prerelease only when the range itself carries one at the same
/// major.minor.patch, so an unselected prerelease cannot be the answer to any
/// range this case asks.
///
/// `budget` caps rule 3 (the only unbounded rule), dropping its lowest members
/// first. Rules 1, 2 and 4 are never dropped — they are what the case tests.
///
/// Unparseable version keys are kept verbatim: this crate does not get to
/// decide that a registry published something illegal, and `rnpm` will say so
/// far more usefully than a silent deletion here would.
pub fn prune_packument(packument: &mut Packument, keep: &BTreeSet<String>, budget: usize) {
    let mut kept: BTreeSet<String> = BTreeSet::new();

    for version in packument.versions.keys() {
        if keep.contains(version) || Version::parse(version).is_err() {
            kept.insert(version.clone());
        }
    }
    for target in packument.dist_tags.values() {
        if packument.versions.contains_key(target) {
            kept.insert(target.clone());
        }
    }

    // Releases only, parsed once, in semver order.
    let mut releases: Vec<(Version, String)> = packument
        .versions
        .keys()
        .filter_map(|raw| Version::parse(raw).ok().map(|v| (v, raw.clone())))
        .filter(|(v, _)| !v.is_prerelease())
        .collect();
    releases.sort_by(|a, b| a.0.cmp(&b.0));

    let anchor_majors: BTreeSet<u64> = kept
        .iter()
        .filter_map(|raw| Version::parse(raw).ok())
        .map(|v| v.major)
        .collect();

    // Rule 4 first: it is never dropped, so it must not compete for budget.
    let mut highest_per_major: BTreeMap<u64, String> = BTreeMap::new();
    for (version, raw) in &releases {
        highest_per_major.insert(version.major, raw.clone());
    }
    for (major, raw) in &highest_per_major {
        if !anchor_majors.contains(major) {
            kept.insert(raw.clone());
        }
    }

    // Rule 3, highest first so the budget keeps the most relevant neighbours.
    let mut room = budget;
    for (version, raw) in releases.iter().rev() {
        if !anchor_majors.contains(&version.major) || kept.contains(raw) {
            continue;
        }
        if room == 0 {
            break;
        }
        kept.insert(raw.clone());
        room -= 1;
    }

    packument.versions.retain(|raw, _| kept.contains(raw));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rnpm::{Dist, VersionManifest};

    fn manifest(version: &str) -> VersionManifest {
        VersionManifest {
            version: version.to_string(),
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            optional_dependencies: BTreeMap::new(),
            peer_dependencies: BTreeMap::new(),
            peer_dependencies_meta: BTreeMap::new(),
            os: Vec::new(),
            cpu: Vec::new(),
            libc: Vec::new(),
            deprecated: None,
            dist: Dist {
                tarball: format!("https://example.test/x-{version}.tgz"),
                integrity: Some("sha512-AAAA".to_string()),
            },
        }
    }

    fn packument(versions: &[&str], tags: &[(&str, &str)]) -> Packument {
        Packument {
            name: "x".to_string(),
            dist_tags: tags
                .iter()
                .map(|(t, v)| (t.to_string(), v.to_string()))
                .collect(),
            versions: versions
                .iter()
                .map(|v| (v.to_string(), manifest(v)))
                .collect(),
        }
    }

    fn keep(versions: &[&str]) -> BTreeSet<String> {
        versions.iter().map(|v| v.to_string()).collect()
    }

    /// The prune's whole reason to be trustworthy: the selected version
    /// survives, and so does a *higher* major it had to reject. A prune that
    /// deleted 19.0.0 would turn `^18` into a range with only one candidate,
    /// and a range-semantics regression would pass.
    #[test]
    fn pruning_keeps_the_selection_and_the_higher_major_it_rejected() {
        let mut p = packument(
            &["17.0.0", "18.0.0", "18.2.0", "18.3.1", "19.0.0", "19.1.0"],
            &[("latest", "19.1.0")],
        );
        prune_packument(&mut p, &keep(&["18.3.1"]), 8);

        let versions: Vec<&str> = p.versions.keys().map(String::as_str).collect();
        assert!(versions.contains(&"18.3.1"), "the selection: {versions:?}");
        assert!(versions.contains(&"19.1.0"), "the dist-tag target: {versions:?}");
        assert!(versions.contains(&"18.0.0"), "a lower 18.x to beat: {versions:?}");
        assert!(versions.contains(&"17.0.0"), "the highest of another major: {versions:?}");
    }

    /// An unselected prerelease is dropped: an npm range only admits one when
    /// the range itself carries a prerelease at the same major.minor.patch, so
    /// it cannot be the answer to anything this case asks.
    #[test]
    fn pruning_drops_unselected_prereleases_but_keeps_selected_ones() {
        let mut p = packument(
            &["1.0.0", "2.0.0-rc.1", "2.0.0-rc.2", "2.0.0"],
            &[("latest", "2.0.0")],
        );
        prune_packument(&mut p, &keep(&["2.0.0-rc.2"]), 8);

        let versions: Vec<&str> = p.versions.keys().map(String::as_str).collect();
        assert!(versions.contains(&"2.0.0-rc.2"), "selected prerelease: {versions:?}");
        assert!(!versions.contains(&"2.0.0-rc.1"), "unselected prerelease: {versions:?}");
    }

    /// The budget bounds rule 3 only. Rule 4 — the highest release of every
    /// other major — is what makes a range have something to reject, so it is
    /// never traded away for headroom.
    #[test]
    fn the_budget_never_evicts_the_per_major_ceiling() {
        let versions: Vec<String> = (0..40).map(|n| format!("3.{n}.0")).collect();
        let mut all: Vec<&str> = versions.iter().map(String::as_str).collect();
        all.push("4.0.0");
        all.push("2.9.9");
        let mut p = packument(&all, &[]);

        prune_packument(&mut p, &keep(&["3.5.0"]), 3);

        let kept: Vec<&str> = p.versions.keys().map(String::as_str).collect();
        assert!(kept.contains(&"3.5.0"), "the selection: {kept:?}");
        assert!(kept.contains(&"4.0.0"), "ceiling of a higher major: {kept:?}");
        assert!(kept.contains(&"2.9.9"), "ceiling of a lower major: {kept:?}");
        // 1 selection + 2 ceilings + budget 3 from the anchor major.
        assert!(kept.len() <= 6, "budget not applied: {kept:?}");
    }

    fn mismatch(oracle: Oracle, key: &str, expected: &str, found: &str) -> Divergence {
        Divergence::Mismatch {
            oracle,
            key: key.to_string(),
            expected: expected.to_string(),
            found: found.to_string(),
        }
    }

    fn waiver(oracle: Oracle, key: &str, expected: &str, found: &str) -> KnownDivergence {
        KnownDivergence {
            oracle,
            key: key.to_string(),
            kind: DivergenceKind::Mismatch,
            expected: Some(expected.to_string()),
            found: Some(found.to_string()),
            why: "recorded disagreement".to_string(),
        }
    }

    /// A waiver that no longer matches is itself a failure — otherwise the
    /// waiver list decays into a record of things that used to be true.
    #[test]
    fn a_waiver_that_matches_nothing_fails_the_case() {
        let waivers = vec![waiver(Oracle::Bun, "react", "react@18.3.1", "react@19.0.0")];
        let mut divergences = Vec::new();
        let fired = apply_waivers(&waivers, &mut divergences);

        assert!(fired.is_empty());
        assert!(matches!(divergences.as_slice(), [Divergence::StaleWaiver { .. }]));
    }

    /// A waiver is scoped to one oracle: pnpm's isolated layout is waived on
    /// pnpm, and the same key still fails on bun.
    #[test]
    fn a_waiver_is_scoped_to_its_oracle() {
        let waivers = vec![waiver(Oracle::Pnpm, "react", "react@18.3.1", "react@19.0.0")];
        let mut divergences = vec![
            mismatch(Oracle::Pnpm, "react", "react@18.3.1", "react@19.0.0"),
            mismatch(Oracle::Bun, "react", "react@18.3.1", "react@19.0.0"),
        ];
        let fired = apply_waivers(&waivers, &mut divergences);

        assert_eq!(fired.len(), 1);
        assert_eq!(divergences.len(), 1);
        assert_eq!(divergences[0].oracle(), Oracle::Bun);
    }

    /// **The tightening, stated as a test.** A waiver pins the versions it
    /// excused. When the same key diverges at a DIFFERENT version, the waiver
    /// must not swallow it — that is precisely how a conformance gate decays
    /// into decoration, still green while excusing something nobody ever saw.
    ///
    /// Two failures are expected, not one: the new divergence (unexcused) and
    /// the stale waiver. Together they tell a reader both what changed and
    /// which waiver to delete.
    #[test]
    fn a_waiver_stops_matching_when_the_version_changes() {
        let waivers = vec![waiver(
            Oracle::Bun,
            "wrap-ansi-cjs/ansi-regex",
            "ansi-regex@5.0.1",
            "ansi-regex@5.0.1",
        )];
        // The same key, but the resolver now produces a wholly different major.
        let mut divergences = vec![mismatch(
            Oracle::Bun,
            "wrap-ansi-cjs/ansi-regex",
            "ansi-regex@5.0.1",
            "ansi-regex@2.0.0",
        )];
        let fired = apply_waivers(&waivers, &mut divergences);

        assert!(fired.is_empty(), "the waiver must not have fired");
        assert_eq!(divergences.len(), 2, "{divergences:?}");
        assert!(
            divergences.iter().any(|d| d.found() == Some("ansi-regex@2.0.0")),
            "the new divergence survives unexcused: {divergences:?}"
        );
        assert!(
            divergences.iter().any(|d| matches!(d, Divergence::StaleWaiver { .. })),
            "and the waiver is reported stale: {divergences:?}"
        );
    }

    /// The same tightening on the other axis: a waiver written for one SHAPE of
    /// disagreement does not excuse another. A conflict copy that stops being
    /// merely misplaced (`extra`) and starts being the wrong package
    /// (`mismatch`) is a new fact about the resolver.
    #[test]
    fn a_waiver_stops_matching_when_the_kind_changes() {
        let waivers = vec![KnownDivergence {
            oracle: Oracle::Bun,
            key: "wrap-ansi-cjs/ansi-regex".to_string(),
            kind: DivergenceKind::Extra,
            expected: None,
            found: Some("ansi-regex@5.0.1".to_string()),
            why: "hoist depth".to_string(),
        }];
        let mut divergences = vec![mismatch(
            Oracle::Bun,
            "wrap-ansi-cjs/ansi-regex",
            "ansi-regex@5.0.1",
            "ansi-regex@5.0.1",
        )];
        let fired = apply_waivers(&waivers, &mut divergences);

        assert!(fired.is_empty());
        assert_eq!(divergences.len(), 2, "{divergences:?}");
    }

    /// A stale waiver's message has to name what it pinned. Four fields can go
    /// stale and the reader cannot tell which one moved from "it no longer
    /// diverges" alone.
    #[test]
    fn a_stale_waiver_names_the_versions_it_pinned() {
        let waivers = vec![waiver(Oracle::Pnpm, "ansi-styles", "ansi-styles@4.3.0", "ansi-styles@4.2.1")];
        let mut divergences = Vec::new();
        apply_waivers(&waivers, &mut divergences);

        let text = divergences[0].describe();
        assert!(text.contains("ansi-styles@4.3.0"), "{text}");
        assert!(text.contains("ansi-styles@4.2.1"), "{text}");
    }

    /// The failure message is the deliverable: R773-F7 requires a divergence to
    /// name the package and the version, because "trees differ" costs the next
    /// reader a full re-derivation.
    #[test]
    fn a_divergence_names_the_package_and_the_version() {
        let d = Divergence::Mismatch {
            oracle: Oracle::Bun,
            key: "chalk/ansi-styles".to_string(),
            expected: "ansi-styles@4.3.0".to_string(),
            found: "ansi-styles@6.2.1".to_string(),
        };
        let text = d.describe();
        assert!(text.contains("ansi-styles@4.3.0"), "{text}");
        assert!(text.contains("ansi-styles@6.2.1"), "{text}");
        assert!(text.contains("chalk/ansi-styles"), "{text}");
    }

    /// A nested `dest_rel` and a nested `install_path` are the same string once
    /// the `node_modules` segments come out — this is the entire reason the two
    /// vocabularies are comparable without either side computing a path.
    #[test]
    fn a_nested_dest_rel_reads_as_the_resolvers_install_path() {
        assert_eq!(install_path_of(Path::new("node_modules/react")), "react");
        assert_eq!(
            install_path_of(Path::new(
                "node_modules/@babel/core/node_modules/convert-source-map"
            )),
            "@babel/core/convert-source-map"
        );
    }

    /// The oracle is read with every gate open, which is the whole of R773-F9's
    /// change to this comparison. Read it any other way and a portable lock's
    /// platform fan-out — twenty-three of the twenty-four `@esbuild/*` entries
    /// on any given machine — reads as twenty-three missing packages.
    ///
    /// This replaced `an_unknown_host_axis_is_refused_by_name`, which guarded
    /// the `[host]` block a case no longer carries.
    #[test]
    fn the_bun_oracle_is_read_with_every_platform_gate_open() {
        let host = admit_everything();
        assert_eq!((host.os, host.cpu, host.libc), (None, None, None));
    }
}
