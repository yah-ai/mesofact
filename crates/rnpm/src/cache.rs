//! On-disk packument cache, keyed by registry and package name.
//!
//! # The directory is injected, never discovered
//!
//! [`PackumentCache::new`] takes a path. This crate does not read
//! `MESOFACT_STORE_DIR`, `XDG_CACHE_HOME` or `HOME`, and does not know that
//! `mesofact-build` exists — the consumer passes
//! `$MESOFACT_STORE_DIR/packuments` and keeps the one place on this machine
//! that decides where a cache root lives (`mesofact-build`'s `store::Store`)
//! from becoming two.
//!
//! # What is stored
//!
//! The *parsed* [`Packument`], not the raw response body, plus the `ETag` that
//! validates it and the time it was fetched. Storing the parsed form means a
//! hit deserializes the narrow struct rather than the wide document; storing
//! the ETag is what makes a revalidation a 304 instead of a re-download; and
//! storing `fetched_at` in the envelope rather than trusting the file's mtime
//! means the freshness window survives a cache directory being copied,
//! restored from a CI volume, or written by a peer process.
//!
//! # Filenames
//!
//! An npm package name can contain `/` (`@scope/name`) and is otherwise
//! attacker-influenced — it arrives from a `package.json` in the dependency
//! tree. [`cache_file_name`] therefore *validates first and mangles second*:
//! a name that is not a legal npm name never becomes a path at all, so there
//! is no `../../` to escape the cache root with. The mangling is `/` → `%2f`,
//! and it is injective because a raw `%` is refused by the validator.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::packument::Packument;
use crate::spec::validate_package_name;

/// A cached packument and the metadata that decides whether it can be served.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CachedPackument {
    /// The response's `ETag`, replayed as `If-None-Match` on revalidation.
    /// Absent for a registry that sends none, which just means every
    /// revalidation is a full download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// Unix seconds at which this entry was fetched or last revalidated.
    pub fetched_at: u64,
    pub packument: Packument,
}

impl CachedPackument {
    /// How long ago this entry was fetched or last revalidated. `None` when
    /// the entry is stamped in the future — a clock that went backwards, or a
    /// cache copied off a machine whose clock was ahead. Treated as "unknown
    /// age" rather than "infinitely fresh", so the caller revalidates.
    pub fn age(&self, now: SystemTime) -> Option<Duration> {
        let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
        now.checked_sub(self.fetched_at).map(Duration::from_secs)
    }
}

/// A cache rooted at a directory. Cheap to clone; holds no handles.
#[derive(Debug, Clone)]
pub struct PackumentCache {
    root: PathBuf,
}

impl PackumentCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the entry for `name` under `registry_id` lives, whether or not it
    /// exists. `registry_id` namespaces the cache so npm's `foo` and a private
    /// registry's `foo` cannot be each other.
    pub fn path_for(&self, registry_id: &str, name: &str) -> Result<PathBuf> {
        validate_registry_id(registry_id)?;
        Ok(self
            .root
            .join(registry_id)
            .join(format!("{}.json", cache_file_name(name)?)))
    }

    /// The entry for `name`, if one is on disk and readable.
    ///
    /// A corrupt or truncated entry is an error, not a silent miss: the cache
    /// is written atomically, so a malformed one means something else damaged
    /// it and quietly re-downloading would hide that forever.
    pub fn load(&self, registry_id: &str, name: &str) -> Result<Option<CachedPackument>> {
        let path = self.path_for(registry_id, name)?;
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let entry = serde_json::from_slice(&raw)
            .with_context(|| format!("parsing cached packument {}", path.display()))?;
        Ok(Some(entry))
    }

    /// Write (or replace) the entry for `name`.
    ///
    /// Temp file plus `rename`, on the same directory and therefore the same
    /// filesystem: this camp runs many installs at once against one cache, and
    /// a reader must never observe a half-written entry.
    pub fn store(&self, registry_id: &str, name: &str, entry: &CachedPackument) -> Result<()> {
        let path = self.path_for(registry_id, name)?;
        let dir = path
            .parent()
            .expect("path_for always joins at least one component");
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating packument cache dir {}", dir.display()))?;

        let temp = dir.join(format!(
            ".{}.{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("entry"),
            std::process::id()
        ));
        std::fs::write(&temp, serde_json::to_vec(entry)?)
            .with_context(|| format!("writing {}", temp.display()))?;
        std::fs::rename(&temp, &path).with_context(|| {
            let _ = std::fs::remove_file(&temp);
            format!("landing {}", path.display())
        })?;
        Ok(())
    }
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate an npm package name and turn it into one path component.
///
/// The validation is the security-relevant half and lives in
/// [`crate::spec::validate_package_name`] — one rule, one place, since "what
/// is a legal npm name" is the grammar's question and this module only asks
/// "and what does it look like on disk". The mangling is `/` → `%2f`, and it
/// is injective because that validator refuses a raw `%`.
pub fn cache_file_name(name: &str) -> Result<String> {
    validate_package_name(name).map_err(|kind| anyhow::anyhow!("{kind}"))?;
    Ok(name.replace('/', "%2f"))
}

/// Registry ids namespace the cache and are chosen by the facade, not by a
/// manifest — so the rule is simply "one boring path component".
fn validate_registry_id(id: &str) -> Result<()> {
    if id.is_empty() || id.starts_with('.') {
        bail!("registry id {id:?} must be a non-empty component that does not start with `.`");
    }
    if let Some(bad) = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        bail!("registry id {id:?} contains {bad:?}; use [A-Za-z0-9._-] only");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packument::{Dist, VersionManifest};
    use std::collections::BTreeMap;

    fn packument(name: &str) -> Packument {
        let mut versions = BTreeMap::new();
        versions.insert(
            "1.0.0".to_string(),
            VersionManifest {
                version: "1.0.0".into(),
                dependencies: BTreeMap::new(),
                dev_dependencies: BTreeMap::new(),
                peer_dependencies: BTreeMap::new(),
                peer_dependencies_meta: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                os: vec![],
                cpu: vec![],
                libc: vec![],
                deprecated: None,
                dist: Dist {
                    tarball: format!("https://example.test/{name}-1.0.0.tgz"),
                    integrity: Some("sha512-AAAA".into()),
                },
            },
        );
        Packument {
            name: name.into(),
            dist_tags: BTreeMap::from([("latest".to_string(), "1.0.0".to_string())]),
            versions,
        }
    }

    #[test]
    fn an_unscoped_name_is_its_own_file_name() {
        assert_eq!(cache_file_name("react").unwrap(), "react");
    }

    #[test]
    fn a_scoped_name_mangles_its_slash_and_stays_one_component() {
        assert_eq!(cache_file_name("@babel/core").unwrap(), "@babel%2fcore");
        // JSR's mangled form is itself a scoped npm name, so it round-trips
        // through exactly the same rule — no JSR-specific case here.
        assert_eq!(
            cache_file_name("@jsr/luca__flag").unwrap(),
            "@jsr%2fluca__flag"
        );
    }

    #[test]
    fn a_name_that_could_escape_the_cache_root_is_refused_not_sanitised() {
        for hostile in [
            "../../etc/passwd",
            "..",
            ".",
            "@scope/../../etc",
            "@/name",
            "@scope/",
            "@scope",
            "a/b/c",
            "with space",
            "already%2fmangled",
            "back\\slash",
            "",
        ] {
            assert!(
                cache_file_name(hostile).is_err(),
                "{hostile:?} should be refused"
            );
        }
    }

    #[test]
    fn the_mangling_cannot_collide_two_distinct_names() {
        // `%` is refused, so `@a%2fb` is not a name anyone can publish and
        // `@a/b` -> `@a%2fb` therefore has exactly one preimage.
        assert!(cache_file_name("@a%2fb").is_err());
        // A bare `@a-b` is not a legal npm name either (a leading `@` means a
        // scope, and a scope needs its `/name`), so there is no unscoped
        // spelling that could collide with a scoped one.
        assert!(cache_file_name("@a-b").is_err());
        assert_ne!(
            cache_file_name("@a/b").unwrap(),
            cache_file_name("@a/b2").unwrap()
        );
        assert_ne!(cache_file_name("a-b").unwrap(), cache_file_name("a_b").unwrap());
    }

    #[test]
    fn two_registries_do_not_share_an_entry_for_the_same_name() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PackumentCache::new(dir.path());
        assert_ne!(
            cache.path_for("npm", "@luca/flag").unwrap(),
            cache.path_for("jsr", "@luca/flag").unwrap()
        );
    }

    #[test]
    fn a_registry_id_that_is_not_one_plain_component_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PackumentCache::new(dir.path());
        for hostile in ["", "..", ".hidden", "a/b", "https://registry.npmjs.org"] {
            assert!(
                cache.path_for(hostile, "react").is_err(),
                "{hostile:?} should be refused as a registry id"
            );
        }
        assert!(cache.path_for("npm-internal_1", "react").is_ok());
    }

    #[test]
    fn an_entry_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PackumentCache::new(dir.path());
        assert_eq!(cache.load("npm", "@babel/core").unwrap(), None);

        let entry = CachedPackument {
            etag: Some("\"abc\"".into()),
            fetched_at: 1_700_000_000,
            packument: packument("@babel/core"),
        };
        cache.store("npm", "@babel/core", &entry).unwrap();
        assert_eq!(cache.load("npm", "@babel/core").unwrap(), Some(entry));
    }

    #[test]
    fn a_corrupt_entry_is_an_error_rather_than_a_silent_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PackumentCache::new(dir.path());
        let path = cache.path_for("npm", "react").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ truncated").unwrap();
        assert!(cache.load("npm", "react").is_err());
    }

    #[test]
    fn an_entry_stamped_in_the_future_reports_unknown_age_not_zero() {
        let entry = CachedPackument {
            etag: None,
            fetched_at: u64::MAX,
            packument: packument("react"),
        };
        assert_eq!(entry.age(SystemTime::now()), None);
    }
}
