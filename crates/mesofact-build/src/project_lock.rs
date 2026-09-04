//! Resolve a project's `package.json` and write its `bun.lock` (R773-T5).
//!
//! The production caller of the R773-T5 seam, and the half of it that touches
//! the world: [`crate::lock_entries`] turns a tree into lock bytes without a
//! filesystem or a network, and everything that needs either is here.
//!
//! Until this module existed, `mesofact-build` could *install* a lock
//! ([`crate::install`]) but not *make* one — a project with only a
//! `package.json` had to run `bun install` first. That is the gap this closes,
//! and it is why the `lock` verb is separate from `install`: writing a lock
//! reaches the registry and rewrites a file a human reviews, while installing
//! one is a mechanical consequence of a file already in the repo. `install`
//! therefore still refuses a project with no lockfile rather than quietly
//! resolving one — it just names this verb when it does.
//!
//! # Re-resolving is a no-op, and that is a feature with a mechanism
//!
//! An existing `bun.lock` is read back in before the walk and handed to the
//! resolver as [`rnpm::PreferredVersions`], so re-running `lock` on an
//! unchanged manifest re-renders the same bytes *even after the registry
//! publishes a newer satisfying version*. Without that the command would be a
//! silent upgrade-everything button. Preferences are not pins: an edited range
//! that no longer admits the locked version resolves forward on its own. See
//! [`rnpm::PreferredVersions`].
//!
//! Only `bun.lock` is read for preferences, though [`crate::install`] also
//! reads `package-lock.json` and `pnpm-lock.yaml`. Deliberate: converting a
//! foreign lock is a migration, and a migration that silently half-preserved
//! another tool's selections would be the hardest kind to review. Delete the
//! foreign lock and this resolves fresh.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rnpm::{
    HttpTransport, PreferredVersions, RegistryClient, RegistryEndpoint, RegistrySource,
    ResolvedTree, Resolver, RootManifest, Version,
};

use crate::install::{
    admit_every_platform, parse_bun_lock_for_host, registry_endpoint, PackageSource,
};
use crate::lock::BUN_LOCK_FILE;
use crate::lock_entries::render_lock;

/// Overrides where packument metadata is cached, for a build box that wants it
/// on a volume that outlives the job — same role [`crate::store::STORE_DIR_ENV`]
/// plays for tarballs, and named the same way.
pub const PACKUMENT_DIR_ENV: &str = "MESOFACT_PACKUMENT_DIR";

/// What one `lock` run decided.
#[derive(Debug, Clone)]
pub struct LockReport {
    /// The lockfile written.
    pub path: PathBuf,
    /// Packages in the closure — lock entries, so a package nested twice
    /// counts twice, exactly as the file does.
    pub packages: usize,
    /// Names carried over from the previous lock as preferences. Zero on a
    /// first run, and the number that makes a re-run a no-op after that.
    pub preferred: usize,
    /// Whether the bytes on disk changed. A `lock` that changes nothing is the
    /// expected outcome of a re-run and worth saying out loud.
    pub changed: bool,
    /// What resolution decided quietly — deprecations, optional skips — already
    /// rendered, since [`rnpm::ResolveWarning`] is not this crate's vocabulary
    /// to re-interpret.
    pub warnings: Vec<String>,
}

/// `$MESOFACT_PACKUMENT_DIR`, else `$XDG_CACHE_HOME/mesofact/packuments`, else
/// `$HOME/.cache/mesofact/packuments`.
///
/// The same base as [`crate::store::Store::open`], one directory over: metadata
/// and tarballs have different lifetimes (a packument goes stale, a tarball
/// never does) and clearing one should not cost the other.
pub fn packument_cache_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(PACKUMENT_DIR_ENV) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| {
            anyhow!("neither {PACKUMENT_DIR_ENV}, XDG_CACHE_HOME nor HOME is set")
        })?;
    Ok(base.join("mesofact").join("packuments"))
}

/// What the project's current `bun.lock` selected, ready to hand to the walk.
///
/// Empty — not an error — when there is no `bun.lock`: a first resolve has
/// nothing to be faithful to.
///
/// A lock entry whose version does not parse is skipped rather than fatal. It
/// cannot come from [`crate::lock_entries`] (those versions are
/// [`rnpm::Version`]s already), so the only way to get one is a hand-edited or
/// foreign file, where dropping *one* preference degrades to a fresh resolve of
/// that package and refusing would strand the project.
pub fn preferred_from_lock(project_root: &Path) -> Result<PreferredVersions> {
    let lock = project_root.join(BUN_LOCK_FILE);
    if !lock.is_file() {
        return Ok(PreferredVersions::new());
    }
    // Read with every gate open: the lock is host-independent (R773-F9), so
    // filtering to this machine here would drop every other platform's
    // selections and make a re-resolve on a mac rewrite the linux entries.
    Ok(parse_bun_lock_for_host(&lock, &admit_every_platform())
        .with_context(|| format!("reading the existing lock at {}", lock.display()))?
        .into_iter()
        .filter_map(|entry| match entry.source {
            // A link has no version to prefer — its target is wherever the
            // manifest says, and nothing about it is a registry selection.
            PackageSource::Link { .. } => None,
            PackageSource::Registry { name, version, .. } => {
                Some((name, Version::parse(&version).ok()?))
            }
        })
        .collect())
}

/// Resolve `project_root`'s manifest against the live npm registry.
///
/// Both phases, in the only order they run in: the phase-1 walk and then the
/// phase-2 peer post-pass.
pub fn resolve_project(
    project_root: &Path,
    preferred: PreferredVersions,
) -> Result<ResolvedTree> {
    let manifest_path = project_root.join("package.json");
    let manifest_json = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest = RootManifest::from_package_json(&manifest_json)
        .with_context(|| format!("in {}", manifest_path.display()))?;

    // The client's default cache policy (five minutes) is written for exactly
    // this caller — see `rnpm::CachePolicy`'s `Default` — so it is taken rather
    // than restated.
    let client = RegistryClient::new(packument_cache_dir()?, HttpTransport::new()?);
    let endpoint: RegistryEndpoint = registry_endpoint();
    let source = RegistrySource::new(&client, &endpoint);

    let mut tree = Resolver::new(&source).preferring(preferred).resolve(&manifest)?;
    rnpm::resolve_peers(&mut tree, &source)?;
    Ok(tree)
}

/// Resolve `project_root` and write its `bun.lock`.
pub fn write_project_lock(project_root: &Path) -> Result<LockReport> {
    let preferred = preferred_from_lock(project_root)?;
    let preferred_names = preferred.len();
    let tree = resolve_project(project_root, preferred)?;

    let rendered = render_lock(&tree)?;
    let path = project_root.join(BUN_LOCK_FILE);
    let changed = std::fs::read_to_string(&path).map(|prev| prev != rendered).unwrap_or(true);
    if changed {
        std::fs::write(&path, &rendered)
            .with_context(|| format!("writing {}", path.display()))?;
    }

    Ok(LockReport {
        path,
        packages: tree.len().saturating_sub(1),
        preferred: preferred_names,
        changed,
        warnings: tree.warnings().iter().map(|w| w.to_string()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hermetic half of this module's behaviour is tested where the seam it
    /// drives lives ([`crate::lock_entries`]) — that is where the fake registry
    /// and the round-trip assertions are. What is left here is the part that
    /// cannot be faked: the real npm registry, real tarballs, and a real
    /// `node_modules` on disk.
    ///
    /// **R773-T5's first criterion, end to end.** A lock this wrote, installed
    /// by the R771 materializer, produces the tree the lock describes — which
    /// is the tree the resolver decided, since
    /// `a_resolved_tree_round_trips_through_bun_lock` proves nothing is lost in
    /// between. The check is per package: the directory Node's own resolution
    /// algorithm would walk to exists, and the `package.json` at it carries the
    /// name and version the lock committed to.
    ///
    /// Ignored because it reaches the network — a gate that needs the registry
    /// up is a gate that fails for reasons that are not about this code.
    ///
    /// ```text
    /// cargo test -p mesofact-build --lib \
    ///   project_lock::tests::a_lock_this_wrote_installs_the_tree_it_describes \
    ///   -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "reaches the npm registry and installs for real"]
    fn a_lock_this_wrote_installs_the_tree_it_describes() {
        let project = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            project.path().join("package.json"),
            r#"{ "name": "seam-e2e", "dependencies": { "react": "^18.3.1" } }"#,
        )
        .unwrap();

        let report = write_project_lock(project.path()).expect("lock");
        assert!(report.changed, "a first lock is always a write");
        assert!(report.packages >= 3, "react's closure is react + 2; got {}", report.packages);
        assert_eq!(report.preferred, 0, "there was no previous lock to prefer");

        // The no-op, against the live registry rather than a fake one.
        let again = write_project_lock(project.path()).expect("re-lock");
        assert!(!again.changed, "re-locking an unchanged manifest rewrote the file");
        assert!(again.preferred > 0, "the second run had a lock to read");

        let installed = crate::install::install(project.path()).expect("install");
        assert_eq!(installed.installed, report.packages);

        // Every locked package is where Node would look for it, and is the
        // package the lock said. `parse_bun_lock` computes `dest_rel` with the
        // same `node_modules/` interposition the installer used, so this reads
        // the layout rather than restating it.
        for pkg in crate::install::parse_bun_lock(&report.path).expect("parse") {
            let manifest = project.path().join(&pkg.dest_rel).join("package.json");
            let raw = std::fs::read_to_string(&manifest)
                .unwrap_or_else(|e| panic!("{}: {e}", manifest.display()));
            let json: serde_json::Value = serde_json::from_str(&raw).expect("package.json");
            let crate::install::PackageSource::Registry { name, version, .. } = &pkg.source else {
                continue;
            };
            assert_eq!(json["name"].as_str(), Some(name.as_str()), "{}", pkg.key);
            assert_eq!(json["version"].as_str(), Some(version.as_str()), "{}", pkg.key);
        }
    }
}
