//! `pnpm-lock.yaml` v9 → install paths (R771-F4, W319 §4).
//!
//! The other two formats this installer reads are keyed by install *path*, so
//! parsing them is most of the work. This one is keyed by package *identity*
//! and **says nothing about where anything goes on disk** — pnpm derives its
//! layout from the depPath at install time. So this module does not read a
//! tree, it derives one, and that is the whole reason W319 sized it on its
//! own.
//!
//! # The layout, decided
//!
//! **A hoisted npm/bun-style tree, with nesting only where a name collides —
//! not pnpm's isolated `node_modules/.pnpm` layout.** The reasoning is
//! recorded in W319 §4.1; in one line, it is the layout W318 §6 already
//! adopted for the stage-2 resolver, so choosing it here keeps one layout in
//! the codebase instead of two, and the disk cost it used to carry is now
//! paid by the content-addressed store (R771-F1) plus reflink (R771-F2)
//! rather than by duplicated bytes.
//!
//! # The three sections
//!
//! - `importers:` — one entry per project directory (`.` is the root),
//!   listing direct deps as `name: {specifier, version}`. The `version` is a
//!   depPath *suffix*: joined to the name it forms the snapshot key.
//! - `packages:` — keyed by bare identity `name@version` (no peer suffix),
//!   carrying `resolution.integrity`, `engines`, and the `cpu`/`os` platform
//!   gates.
//! - `snapshots:` — the graph. Keyed by depPath *with* peer suffixes
//!   (`'@dnd-kit/core@6.3.1(react-dom@18.3.1(react@18.3.1))(react@18.3.1)'`),
//!   each carrying `dependencies` and `optionalDependencies` maps in the same
//!   name → suffix form.
//!
//! A peer suffix distinguishes two *instances* of one package, not two sets
//! of bytes: both resolve to the same tarball and therefore the same store
//! entry. The suffix matters here only for walking the graph — instance A and
//! instance B can have different dependency edges, and where that forces two
//! versions of one name into one directory, the hoist nests instead.

use anyhow::{bail, Context, Result};
use serde_yaml::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::install::{LockedPackage, PackageSource};

/// Parse `pnpm-lock.yaml` (v9) and derive the install layout.
pub(crate) fn parse_pnpm_lock(lock_path: &Path) -> Result<Vec<LockedPackage>> {
    let raw = std::fs::read_to_string(lock_path)
        .with_context(|| format!("reading {}", lock_path.display()))?;
    let doc: Value = serde_yaml::from_str(&raw)
        .with_context(|| format!("parsing {}", lock_path.display()))?;
    let lock = PnpmLock::from_yaml(&doc, lock_path)?;
    lock.derive_layout()
}

/// What a `packages:` entry contributes: how to fetch it, and whether it can
/// run here at all.
#[derive(Debug, Default)]
struct PackageMeta {
    integrity: Option<String>,
    /// A `resolution.tarball` pointing off the public registry, kept so the
    /// refusal can name it.
    foreign_tarball: Option<String>,
    /// `resolution: {type: directory, directory: ../x}` — a local path dep.
    directory: Option<String>,
    cpu: Vec<String>,
    os: Vec<String>,
}

#[derive(Debug, Clone)]
struct Edge {
    name: String,
    /// The depPath suffix as written (`18.3.1`, `6.3.1(react@18.3.1)`,
    /// `link:../shared`).
    spec: String,
    /// From `optionalDependencies` rather than `dependencies`. Only these are
    /// subject to the platform gate — see [`PnpmLock::derive_layout`].
    optional: bool,
}

/// Who asked for an edge. A `link:` target is relative to the requester's
/// directory, which only exists for an importer — so the two cases have to
/// stay distinguishable all the way down the walk.
#[derive(Debug, Clone)]
enum Requester {
    /// An `importers:` key (`.`, `packages/ui`).
    Importer(String),
    /// A depPath, i.e. another package.
    Package(String),
}

impl std::fmt::Display for Requester {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Requester::Importer(dir) => write!(f, "importers[{dir:?}]"),
            Requester::Package(dep_path) => write!(f, "{dep_path}"),
        }
    }
}

#[derive(Debug)]
struct PnpmLock {
    /// Bare `name@version` → metadata.
    packages: BTreeMap<String, PackageMeta>,
    /// Full depPath (peer suffixes included) → its outgoing edges.
    snapshots: BTreeMap<String, Vec<Edge>>,
    /// Importer directory (`.` for the root) → its direct deps.
    importers: BTreeMap<String, Vec<Edge>>,
}

impl PnpmLock {
    fn from_yaml(doc: &Value, lock_path: &Path) -> Result<Self> {
        let where_ = lock_path.display().to_string();

        // v9 writes the version as a string ('9.0'). Nothing older carries
        // the three-section shape this reads.
        match doc.get("lockfileVersion") {
            Some(Value::String(v)) if v == "9.0" || v == "9" => {}
            Some(v) => bail!(
                "{where_}: lockfileVersion {v:?} is not supported — this reads v9 (the importers/packages/snapshots shape); re-run `pnpm install` with pnpm 9 or newer"
            ),
            None => bail!("{where_}: no \"lockfileVersion\""),
        }

        // A patched dependency is not the package its integrity names, and
        // the patch lives outside the lock. Refusing is the only honest
        // option; silently installing the unpatched original is not.
        if doc.get("patchedDependencies").is_some() {
            bail!(
                "{where_}: patchedDependencies is not supported — the installed tree would silently be the unpatched packages"
            );
        }

        let mut packages = BTreeMap::new();
        for (key, entry) in map_of(doc, "packages") {
            let mut meta = PackageMeta {
                cpu: string_list(entry.get("cpu")),
                os: string_list(entry.get("os")),
                ..PackageMeta::default()
            };
            if let Some(res) = entry.get("resolution") {
                meta.integrity = res.get("integrity").and_then(Value::as_str).map(str::to_string);
                meta.directory = res.get("directory").and_then(Value::as_str).map(str::to_string);
                if let Some(tarball) = res.get("tarball").and_then(Value::as_str) {
                    if !tarball.starts_with(&format!("{}/", crate::install::REGISTRY)) {
                        meta.foreign_tarball = Some(tarball.to_string());
                    }
                }
            }
            packages.insert(key.to_string(), meta);
        }

        let mut snapshots = BTreeMap::new();
        for (key, entry) in map_of(doc, "snapshots") {
            let mut edges = Vec::new();
            collect_edges(&mut edges, entry.get("dependencies"), false, key, &where_)?;
            collect_edges(
                &mut edges,
                entry.get("optionalDependencies"),
                true,
                key,
                &where_,
            )?;
            snapshots.insert(key.to_string(), edges);
        }

        let mut importers = BTreeMap::new();
        for (dir, entry) in map_of(doc, "importers") {
            let mut edges = Vec::new();
            for (field, optional) in [
                ("dependencies", false),
                ("devDependencies", false),
                ("optionalDependencies", true),
            ] {
                // An importer's dep is `name: {specifier, version}` rather
                // than `name: version`; only `version` is a depPath suffix.
                for (name, spec) in map_of(entry, field) {
                    let Some(version) = spec.get("version").and_then(Value::as_str) else {
                        bail!(
                            "{where_}: importers[{dir:?}].{field}[{name:?}] has no \"version\""
                        );
                    };
                    check_package_name(name)
                        .with_context(|| format!("{where_}: importers[{dir:?}].{field}"))?;
                    edges.push(Edge {
                        name: name.to_string(),
                        spec: version.to_string(),
                        optional,
                    });
                }
            }
            importers.insert(dir.to_string(), edges);
        }

        if importers.is_empty() {
            bail!("{where_}: no \"importers\" — nothing to install");
        }
        Ok(Self { packages, snapshots, importers })
    }

    /// Walk the graph breadth-first from every importer's direct deps,
    /// hoisting each package to the shallowest `node_modules` on its
    /// requester's resolution path where its name is free.
    ///
    /// Breadth-first is what makes the result a *hoisted* tree rather than a
    /// nested one: a package reached at depth 1 claims the root directory
    /// before any deeper requester can, and deeper requesters that want a
    /// different version get a nested copy. That is npm's and bun's shape,
    /// and Node's resolution algorithm — walk up from the requiring file
    /// through each ancestor `node_modules` — is what makes it correct.
    ///
    /// **Only `optionalDependencies` are platform-gated.** An optional edge
    /// whose target declares a `cpu`/`os` this machine is not is skipped:
    /// that is what npm, pnpm and bun all do, and it is the difference
    /// between installing esbuild and installing esbuild plus twenty foreign
    /// platform binaries. A *required* dep with a foreign gate is installed
    /// anyway — that is a broken lock, and refusing to guess is consistent
    /// with the rest of this installer.
    fn derive_layout(&self) -> Result<Vec<LockedPackage>> {
        let mut out = Vec::new();
        // (directory owning a node_modules, package name) → the identity
        // occupying it: a depPath, or `link:<project-relative target>`.
        let mut placements: HashMap<(PathBuf, String), String> = HashMap::new();
        // Install directories whose own dependencies have been queued.
        let mut expanded: HashSet<PathBuf> = HashSet::new();
        let mut queue: VecDeque<(Vec<PathBuf>, Requester, Edge)> = VecDeque::new();

        // The root importer goes first so its direct deps win the root
        // node_modules; workspaces then hoist into whatever is left, which is
        // the order npm resolves a workspace repo in.
        let mut importer_dirs: Vec<&String> = self.importers.keys().collect();
        importer_dirs.sort_by_key(|d| (d.as_str() != ".", d.as_str()));

        for dir in importer_dirs {
            let importer_rel = importer_dir(dir)?;
            // Node walks up from the importer, so the root's node_modules is
            // a candidate for a workspace's deps too — shallowest first.
            let chain: Vec<PathBuf> = if importer_rel.as_os_str().is_empty() {
                vec![PathBuf::new()]
            } else {
                vec![PathBuf::new(), importer_rel]
            };
            for edge in &self.importers[dir] {
                queue.push_back((
                    chain.clone(),
                    Requester::Importer(dir.clone()),
                    edge.clone(),
                ));
            }
        }

        while let Some((chain, requester, edge)) = queue.pop_front() {
            let Some(resolved) = self.resolve_edge(&edge, &requester)? else {
                continue; // platform-gated optional
            };

            // The shallowest node_modules on the requester's resolution path
            // that this package can occupy. The last link in the chain is the
            // requester's *own* node_modules, which only its own deps reach —
            // and a dependencies map cannot name one package twice — so there
            // is always a slot.
            let mut chosen = None;
            for (i, dir) in chain.iter().enumerate() {
                match placements.get(&(dir.clone(), edge.name.clone())) {
                    None => {
                        chosen = Some((i, false));
                        break;
                    }
                    Some(existing) if *existing == resolved.identity => {
                        chosen = Some((i, true));
                        break;
                    }
                    // A different version of this name lives here; go deeper.
                    Some(_) => continue,
                }
            }
            let Some((i, already_placed)) = chosen else {
                bail!(
                    "{}: no free node_modules on the resolution path of {requester} — this is a bug in the layout derivation, not in the lockfile",
                    resolved.identity
                );
            };

            let dir = chain[i].clone();
            let install_dir = dir.join("node_modules").join(&edge.name);
            if !already_placed {
                placements.insert((dir, edge.name.clone()), resolved.identity.clone());
                out.push(LockedPackage {
                    key: resolved.identity.clone(),
                    dest_rel: install_dir.clone(),
                    source: resolved.source,
                });
            }

            // A link has no graph in the lockfile: its target is a directory
            // whose own deps belong to its importer entry, not to this edge.
            if !resolved.has_snapshot || !expanded.insert(install_dir.clone()) {
                continue;
            }
            let Some(edges) = self.snapshots.get(&resolved.identity) else {
                bail!(
                    "{}: no snapshots entry (required by {requester}) — its dependency graph is missing; re-run `pnpm install`",
                    resolved.identity
                );
            };
            let mut child_chain: Vec<PathBuf> = chain[..=i].to_vec();
            child_chain.push(install_dir);
            for child in edges {
                queue.push_back((
                    child_chain.clone(),
                    Requester::Package(resolved.identity.clone()),
                    child.clone(),
                ));
            }
        }

        Ok(out)
    }

    /// One edge reduced to an identity (what would occupy a directory) and a
    /// source. `Ok(None)` is the platform-gated optional: not an error, not
    /// installed.
    fn resolve_edge(&self, edge: &Edge, requester: &Requester) -> Result<Option<Resolved>> {
        // `link:` is a path relative to the *requester's* directory and has
        // no `packages:` entry at all.
        if let Some(target) = edge.spec.strip_prefix("link:") {
            let target_rel = rebase_link(requester, target)?;
            return Ok(Some(Resolved {
                // Two importers can spell one directory differently
                // (`link:packages/shared` from the root,
                // `link:../../packages/shared` from `apps/web`); the resolved
                // path is what makes them the same placement.
                identity: format!("link:{}", target_rel.display()),
                source: PackageSource::Link { target_rel },
                has_snapshot: false,
            }));
        }

        let dep_path = format!("{}@{}", edge.name, edge.spec);
        let base = base_identity(&dep_path);
        let Some(meta) = self.packages.get(base) else {
            bail!(
                "{dep_path}: no packages[{base:?}] entry (required by {requester}) — the lockfile's snapshots and packages sections disagree; re-run `pnpm install`"
            );
        };
        if edge.optional && !platform_supported(meta) {
            return Ok(None);
        }
        if let Some(directory) = &meta.directory {
            let target_rel = rebase_link(requester, directory)?;
            return Ok(Some(Resolved {
                identity: dep_path,
                source: PackageSource::Link { target_rel },
                has_snapshot: false,
            }));
        }
        if let Some(tarball) = &meta.foreign_tarball {
            bail!(
                "{dep_path}: resolution.tarball is {tarball}, which is not on {} — alternate registries, git and local-tarball sources are not supported by the Rust-native installer",
                crate::install::REGISTRY
            );
        }
        let (name, version) = split_identity(base)?;
        let Some(integrity) = &meta.integrity else {
            bail!(
                "{dep_path}: packages[{base:?}] has no resolution.integrity — refusing to install unverified content (W319 §4)"
            );
        };
        if !integrity.starts_with("sha512-") {
            bail!(
                "{dep_path}: packages[{base:?}] has integrity {integrity:?}; only sha512 is verified"
            );
        }
        Ok(Some(Resolved {
            source: PackageSource::Registry {
                name: name.to_string(),
                version: version.to_string(),
                integrity: integrity.clone(),
            },
            identity: dep_path,
            has_snapshot: true,
        }))
    }
}

struct Resolved {
    /// What occupies a directory — the depPath, or `link:<target>`.
    identity: String,
    source: PackageSource,
    /// Whether `snapshots:` is expected to carry this identity's edges.
    has_snapshot: bool,
}

/// `'@dnd-kit/core@6.3.1(react@18.3.1)'` → `'@dnd-kit/core@6.3.1'`. The peer
/// suffix names an *instance*; the bytes it resolves to are the base
/// identity's, which is what `packages:` is keyed by.
fn base_identity(dep_path: &str) -> &str {
    match dep_path.find('(') {
        Some(i) => &dep_path[..i],
        None => dep_path,
    }
}

/// Split a bare `name@version`. The `@` of a scope is at index 0, so the
/// separator is the last one — and the peer suffix, which contains `@`s of
/// its own, must already be gone (see [`base_identity`]).
fn split_identity(base: &str) -> Result<(&str, &str)> {
    let at = base
        .rfind('@')
        .filter(|&i| i > 0)
        .with_context(|| format!("unparseable package identity {base:?}"))?;
    Ok((&base[..at], &base[at + 1..]))
}

/// A package name becomes a *directory* under `node_modules`, so it is a
/// write destination and a lockfile is attacker-editable content in a pull
/// request. Only the two shapes npm allows are accepted: `name`, and
/// `@scope/name`. Anything with a traversal, a root, or extra segments is
/// refused.
///
/// The other two parsers get this for free — their keys go through
/// `install_path` — but a pnpm name is joined onto a derived path, so the
/// check has to live here.
fn check_package_name(name: &str) -> Result<()> {
    let plain = |seg: &str| !seg.is_empty() && seg != "." && seg != ".." && !seg.contains('\\');
    let ok = match name.strip_prefix('@') {
        Some(scoped) => match scoped.split_once('/') {
            Some((scope, rest)) => plain(scope) && plain(rest) && !rest.contains('/'),
            None => false,
        },
        None => plain(name) && !name.contains('/'),
    };
    if !ok {
        bail!("{name:?} is not a package name — it would install outside node_modules");
    }
    Ok(())
}

/// An importer key (`.`, `packages/ui`) as a project-root-relative directory.
/// Refused if it escapes the root — a lockfile is attacker-editable content
/// in a pull request, and this becomes a write destination.
fn importer_dir(key: &str) -> Result<PathBuf> {
    if key == "." {
        return Ok(PathBuf::new());
    }
    let p = Path::new(key);
    if p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        bail!("importer {key:?} is not a plain path under the project root");
    }
    Ok(p.to_path_buf())
}

/// A `link:` target is relative to the requester's directory; the walk wants
/// it relative to the project root. Resolved lexically, since the target need
/// not exist yet.
///
/// Link *targets* are deliberately not constrained to the project root
/// (`link:../sibling` is legitimate and the bun path has always allowed it) —
/// only destinations are. That asymmetry matches `install_path`.
fn rebase_link(requester: &Requester, target: &str) -> Result<PathBuf> {
    let mut out = match requester {
        Requester::Importer(dir) => importer_dir(dir)?,
        // A transitive dependency's `link:`/`directory:` target is relative
        // to wherever pnpm's isolated store put that package — a location
        // this layout deliberately does not reproduce, so there is nothing
        // honest to resolve it against.
        Requester::Package(dep_path) => bail!(
            "{dep_path}: a link: or directory: target on a transitive dependency is not supported by the Rust-native installer"
        ),
    };
    for component in Path::new(target).components() {
        match component {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    bail!("link target {target:?} from {requester:?} escapes the project root");
                }
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(seg) => out.push(seg),
            _ => bail!("link target {target:?} from {requester:?} is not a relative path"),
        }
    }
    Ok(out)
}

/// npm's `os`/`cpu` gate. A plain list is an allowlist; entries prefixed `!`
/// are a denylist. `libc` is deliberately not checked — it cannot be detected
/// reliably from here, and guessing wrong would skip a package the project
/// needs.
fn platform_supported(meta: &PackageMeta) -> bool {
    matches(&meta.os, npm_os()) && matches(&meta.cpu, npm_cpu())
}

fn matches(list: &[String], current: &str) -> bool {
    if list.is_empty() {
        return true;
    }
    let (denied, allowed): (Vec<&String>, Vec<&String>) =
        list.iter().partition(|v| v.starts_with('!'));
    if denied.iter().any(|v| &v[1..] == current) {
        return false;
    }
    allowed.is_empty() || allowed.iter().any(|v| v.as_str() == current)
}

fn npm_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn npm_cpu() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    }
}

/// The string-keyed entries of `value[field]`, or nothing if it is absent.
/// A non-string key cannot name a package, so it is skipped rather than
/// guessed at.
fn map_of<'a>(value: &'a Value, field: &str) -> Vec<(&'a str, &'a Value)> {
    value
        .get(field)
        .and_then(Value::as_mapping)
        .map(|m| m.iter().filter_map(|(k, v)| Some((k.as_str()?, v))).collect())
        .unwrap_or_default()
}

/// npm's `cpu`/`os` fields are YAML sequences of strings.
fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_sequence)
        .map(|s| s.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn collect_edges(
    out: &mut Vec<Edge>,
    field: Option<&Value>,
    optional: bool,
    key: &str,
    where_: &str,
) -> Result<()> {
    let Some(map) = field.and_then(Value::as_mapping) else {
        return Ok(());
    };
    for (name, spec) in map {
        let (Some(name), Some(spec)) = (name.as_str(), spec.as_str()) else {
            bail!("{where_}: snapshots[{key:?}] has a non-string dependency entry");
        };
        check_package_name(name)
            .with_context(|| format!("{where_}: snapshots[{key:?}]"))?;
        out.push(Edge {
            name: name.to_string(),
            spec: spec.to_string(),
            optional,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Result<Vec<LockedPackage>> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pnpm-lock.yaml");
        std::fs::write(&path, body).unwrap();
        parse_pnpm_lock(&path)
    }

    fn dests(pkgs: &[LockedPackage]) -> Vec<String> {
        let mut v: Vec<String> = pkgs
            .iter()
            .map(|p| p.dest_rel.to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn version_at(pkgs: &[LockedPackage], dest: &str) -> String {
        let pkg = pkgs
            .iter()
            .find(|p| p.dest_rel == Path::new(dest))
            .unwrap_or_else(|| panic!("nothing installed at {dest}; have {:?}", dests(pkgs)));
        match &pkg.source {
            PackageSource::Registry { version, .. } => version.clone(),
            PackageSource::Link { target_rel } => format!("link:{}", target_rel.display()),
        }
    }

    /// Two versions of one name: the one reached first (breadth-first, from
    /// the root importer) takes the root, the other nests under its
    /// requester. This is the hoist, and it is the whole ticket.
    const CONFLICT: &str = r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      alpha:
        specifier: ^1.0.0
        version: 1.0.0
      shared:
        specifier: ^2.0.0
        version: 2.0.0
packages:
  alpha@1.0.0:
    resolution: {integrity: sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==}
  shared@1.0.0:
    resolution: {integrity: sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb==}
  shared@2.0.0:
    resolution: {integrity: sha512-cccccccccccccccccccccccccccccccccccccccc==}
snapshots:
  alpha@1.0.0:
    dependencies:
      shared: 1.0.0
  shared@1.0.0: {}
  shared@2.0.0: {}
"#;

    #[test]
    fn a_version_conflict_nests_under_its_requester() {
        let pkgs = parse(CONFLICT).unwrap();
        assert_eq!(
            dests(&pkgs),
            vec![
                "node_modules/alpha",
                "node_modules/alpha/node_modules/shared",
                "node_modules/shared",
            ]
        );
        // The root importer's own dep wins the root directory...
        assert_eq!(version_at(&pkgs, "node_modules/shared"), "2.0.0");
        // ...and alpha's incompatible copy lands inside alpha.
        assert_eq!(version_at(&pkgs, "node_modules/alpha/node_modules/shared"), "1.0.0");
    }

    /// The same package reached twice is installed once — the second arrival
    /// finds its own depPath already at the root and reuses it.
    #[test]
    fn a_shared_dependency_is_hoisted_once() {
        let pkgs = parse(
            r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      alpha:
        specifier: ^1.0.0
        version: 1.0.0
      beta:
        specifier: ^1.0.0
        version: 1.0.0
packages:
  alpha@1.0.0:
    resolution: {integrity: sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==}
  beta@1.0.0:
    resolution: {integrity: sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb==}
  shared@1.0.0:
    resolution: {integrity: sha512-cccccccccccccccccccccccccccccccccccccccc==}
snapshots:
  alpha@1.0.0:
    dependencies:
      shared: 1.0.0
  beta@1.0.0:
    dependencies:
      shared: 1.0.0
  shared@1.0.0: {}
"#,
        )
        .unwrap();
        assert_eq!(
            dests(&pkgs),
            vec!["node_modules/alpha", "node_modules/beta", "node_modules/shared"]
        );
    }

    /// Peer-suffixed depPaths are two *instances* of one identity. Both fetch
    /// the same tarball; the suffix only changes which edges they carry, and
    /// a scoped name must survive the split.
    #[test]
    fn peer_suffixed_instances_share_one_identity() {
        let pkgs = parse(
            r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      '@dnd-kit/core':
        specifier: ^6.3.1
        version: 6.3.1(react@18.3.1)
      react:
        specifier: ^18.3.1
        version: 18.3.1
packages:
  '@dnd-kit/core@6.3.1':
    resolution: {integrity: sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==}
  react@18.3.1:
    resolution: {integrity: sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb==}
snapshots:
  '@dnd-kit/core@6.3.1(react@18.3.1)':
    dependencies:
      react: 18.3.1
  react@18.3.1: {}
"#,
        )
        .unwrap();
        assert_eq!(
            dests(&pkgs),
            vec!["node_modules/@dnd-kit/core", "node_modules/react"]
        );
        let dnd = pkgs
            .iter()
            .find(|p| p.dest_rel == Path::new("node_modules/@dnd-kit/core"))
            .unwrap();
        match &dnd.source {
            PackageSource::Registry { name, version, .. } => {
                assert_eq!((name.as_str(), version.as_str()), ("@dnd-kit/core", "6.3.1"));
            }
            other => panic!("{other:?}"),
        }
        // The key keeps the instance, so a diagnostic can name which one.
        assert_eq!(dnd.key, "@dnd-kit/core@6.3.1(react@18.3.1)");
    }

    /// A workspace importer installs into its own node_modules only where the
    /// root is already taken by a different version — the same hoist rule,
    /// with the root as the shallower candidate.
    #[test]
    fn workspace_importers_hoist_to_the_root_when_free() {
        let pkgs = parse(
            r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      shared:
        specifier: ^2.0.0
        version: 2.0.0
  docs:
    dependencies:
      shared:
        specifier: ^1.0.0
        version: 1.0.0
      only-docs:
        specifier: ^1.0.0
        version: 1.0.0
packages:
  shared@1.0.0:
    resolution: {integrity: sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==}
  shared@2.0.0:
    resolution: {integrity: sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb==}
  only-docs@1.0.0:
    resolution: {integrity: sha512-cccccccccccccccccccccccccccccccccccccccc==}
snapshots:
  shared@1.0.0: {}
  shared@2.0.0: {}
  only-docs@1.0.0: {}
"#,
        )
        .unwrap();
        assert_eq!(
            dests(&pkgs),
            vec![
                "docs/node_modules/shared",
                "node_modules/only-docs",
                "node_modules/shared",
            ]
        );
        assert_eq!(version_at(&pkgs, "node_modules/shared"), "2.0.0");
        assert_eq!(version_at(&pkgs, "docs/node_modules/shared"), "1.0.0");
    }

    /// An optional dep for another platform is skipped — that is what npm,
    /// pnpm and bun do, and it is the difference between installing esbuild
    /// and installing twenty foreign binaries alongside it.
    #[test]
    fn foreign_platform_optionals_are_skipped() {
        let pkgs = parse(
            r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      bundler:
        specifier: ^1.0.0
        version: 1.0.0
packages:
  bundler@1.0.0:
    resolution: {integrity: sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==}
  '@bundler/aix-ppc64@1.0.0':
    resolution: {integrity: sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb==}
    cpu: [ppc64]
    os: [aix]
  '@bundler/everywhere@1.0.0':
    resolution: {integrity: sha512-cccccccccccccccccccccccccccccccccccccccc==}
snapshots:
  bundler@1.0.0:
    optionalDependencies:
      '@bundler/aix-ppc64': 1.0.0
      '@bundler/everywhere': 1.0.0
  '@bundler/aix-ppc64@1.0.0': {}
  '@bundler/everywhere@1.0.0': {}
"#,
        )
        .unwrap();
        assert_eq!(
            dests(&pkgs),
            vec!["node_modules/@bundler/everywhere", "node_modules/bundler"]
        );
    }

    #[test]
    fn platform_gates_read_allow_and_deny_lists() {
        let os = |v: &[&str]| {
            matches(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>(), npm_os())
        };
        assert!(os(&[]), "an empty list gates nothing");
        assert!(os(&[npm_os()]));
        assert!(!os(&["plan9"]));
        assert!(!os(&[&format!("!{}", npm_os())]));
        assert!(os(&["!plan9"]));
    }

    /// `link:` in an importer is a workspace dependency: a symlink, not a
    /// fetch, and its target is relative to the importer.
    #[test]
    fn workspace_links_become_relative_symlinks() {
        let pkgs = parse(
            r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      shared:
        specifier: workspace:*
        version: link:packages/shared
  apps/web:
    dependencies:
      shared:
        specifier: workspace:*
        version: link:../../packages/shared
packages: {}
snapshots: {}
"#,
        )
        .unwrap();
        assert_eq!(version_at(&pkgs, "node_modules/shared"), "link:packages/shared");
        // The root claimed `shared` first, and the workspace's link resolves
        // to the same directory, so nothing is placed twice.
        assert_eq!(dests(&pkgs), vec!["node_modules/shared"]);
    }

    #[test]
    fn refusals_name_what_they_refuse() {
        let cases = [
            ("lockfileVersion: '5.4'\nimporters: {}\n", "lockfileVersion"),
            (
                "lockfileVersion: '9.0'\npatchedDependencies: {foo@1.0.0: {path: p, hash: h}}\nimporters: {}\n",
                "patchedDependencies",
            ),
            ("lockfileVersion: '9.0'\nimporters: {}\n", "importers"),
        ];
        for (body, needle) in cases {
            let err = format!("{:#}", parse(body).unwrap_err());
            assert!(err.contains(needle), "{needle}: {err}");
        }
    }

    #[test]
    fn a_registry_entry_without_integrity_is_refused() {
        let err = format!(
            "{:#}",
            parse(
                r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      alpha:
        specifier: ^1.0.0
        version: 1.0.0
packages:
  alpha@1.0.0:
    engines: {node: '>=18'}
snapshots:
  alpha@1.0.0: {}
"#,
            )
            .unwrap_err()
        );
        assert!(err.contains("alpha@1.0.0"), "{err}");
        assert!(err.contains("integrity"), "{err}");
    }

    #[test]
    fn a_foreign_registry_tarball_is_refused() {
        let err = format!(
            "{:#}",
            parse(
                r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      alpha:
        specifier: ^1.0.0
        version: 1.0.0
packages:
  alpha@1.0.0:
    resolution:
      integrity: sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==
      tarball: https://npm.internal.example.com/alpha/-/alpha-1.0.0.tgz
snapshots:
  alpha@1.0.0: {}
"#,
            )
            .unwrap_err()
        );
        assert!(err.contains("npm.internal.example.com"), "{err}");
    }

    #[test]
    fn a_snapshot_missing_from_packages_is_refused_by_name() {
        let err = format!(
            "{:#}",
            parse(
                r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      alpha:
        specifier: ^1.0.0
        version: 1.0.0
packages: {}
snapshots:
  alpha@1.0.0: {}
"#,
            )
            .unwrap_err()
        );
        assert!(err.contains("alpha@1.0.0"), "{err}");
    }

    /// A cycle (a → b → a) must terminate; pnpm graphs contain them.
    #[test]
    fn a_dependency_cycle_terminates() {
        let pkgs = parse(
            r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      alpha:
        specifier: ^1.0.0
        version: 1.0.0
packages:
  alpha@1.0.0:
    resolution: {integrity: sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==}
  beta@1.0.0:
    resolution: {integrity: sha512-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb==}
snapshots:
  alpha@1.0.0:
    dependencies:
      beta: 1.0.0
  beta@1.0.0:
    dependencies:
      alpha: 1.0.0
"#,
        )
        .unwrap();
        assert_eq!(dests(&pkgs), vec!["node_modules/alpha", "node_modules/beta"]);
    }

    #[test]
    fn identities_split_on_the_version_at_not_the_scope_at() {
        assert_eq!(split_identity("react@18.3.1").unwrap(), ("react", "18.3.1"));
        assert_eq!(
            split_identity("@babel/core@7.28.0").unwrap(),
            ("@babel/core", "7.28.0")
        );
        assert_eq!(
            base_identity("@dnd-kit/core@6.3.1(react-dom@18.3.1(react@18.3.1))(react@18.3.1)"),
            "@dnd-kit/core@6.3.1"
        );
        assert_eq!(base_identity("react@18.3.1"), "react@18.3.1");
    }

    /// Point the derivation at a real lock without installing anything:
    ///
    /// ```text
    /// MESOFACT_PNPM_LOCK=path/to/pnpm-lock.yaml \
    ///   cargo test -p mesofact-build --lib pnpm::tests::derives_a_real_lock -- --ignored --nocapture
    /// ```
    ///
    /// Ignored by default because it needs a file this repo does not carry.
    /// It is the cheapest way to test the parser against a pathological lock
    /// (deep peer suffixes, hundreds of platform-gated optionals) before
    /// paying for a real install.
    #[test]
    #[ignore = "needs MESOFACT_PNPM_LOCK pointing at a real lockfile"]
    fn derives_a_real_lock() {
        let path = std::env::var("MESOFACT_PNPM_LOCK")
            .expect("set MESOFACT_PNPM_LOCK to a pnpm-lock.yaml");
        let pkgs = parse_pnpm_lock(Path::new(&path)).unwrap();
        let nested = pkgs
            .iter()
            .filter(|p| {
                p.dest_rel.to_string_lossy().matches("node_modules").count() > 1
            })
            .count();
        println!(
            "{}: {} placements, {nested} nested by conflict",
            path,
            pkgs.len()
        );
        for p in pkgs.iter().filter(|p| {
            p.dest_rel.to_string_lossy().matches("node_modules").count() > 1
        }) {
            println!("  nested: {} ← {}", p.dest_rel.display(), p.key);
        }
        assert!(!pkgs.is_empty());
    }

    #[test]
    fn importer_paths_cannot_escape_the_project_root() {
        assert_eq!(importer_dir(".").unwrap(), Path::new(""));
        assert_eq!(importer_dir("packages/ui").unwrap(), Path::new("packages/ui"));
        assert!(importer_dir("../outside").is_err());
        assert!(importer_dir("/etc").is_err());
    }

    /// A dependency *name* is joined onto a derived install path here, so it
    /// is a write destination — unlike the other two formats, whose keys go
    /// through `install_path`.
    #[test]
    fn package_names_that_are_paths_are_refused() {
        for good in ["react", "@babel/core", "@e2e/shared"] {
            check_package_name(good).unwrap_or_else(|e| panic!("{good}: {e}"));
        }
        for bad in ["", "..", "../evil", "a/b", "@scope", "@scope/a/b", "@/x", "."] {
            assert!(check_package_name(bad).is_err(), "{bad:?} should be refused");
        }

        let err = format!(
            "{:#}",
            parse(
                r#"
lockfileVersion: '9.0'
importers:
  .:
    dependencies:
      alpha:
        specifier: ^1.0.0
        version: 1.0.0
packages:
  alpha@1.0.0:
    resolution: {integrity: sha512-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa==}
snapshots:
  alpha@1.0.0:
    dependencies:
      '../../.ssh/authorized_keys': 1.0.0
"#,
            )
            .unwrap_err()
        );
        assert!(err.contains("authorized_keys"), "{err}");
        assert!(err.contains("not a package name"), "{err}");
    }
}
