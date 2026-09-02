//! The abbreviated packument, modelled as a *resolution input* and nothing
//! more.
//!
//! `Accept: application/vnd.npm.install-v1+json` gets a document with the
//! per-version fields an installer needs and none of the ones a registry web
//! page needs — no `readme`, no `maintainers`, no `_npmUser`, no full
//! `repository` block. On a large package that is the difference between a few
//! hundred KB and several MB, and this camp will hold one of these per package
//! in the tree.
//!
//! **The struct is narrower still, deliberately.** Only what a resolver reads
//! is declared; serde drops the rest. That is not a size optimisation — it is
//! the boundary that stops this type from drifting into a `package.json`
//! mirror. Anything that wants a field not listed here wants the manifest out
//! of the unpacked tarball, which [`crate`]'s consumer already has via
//! `mesofact-build`'s store.
//!
//! The two Serialize impls exist for one reason: [`crate::cache`] round-trips
//! a packument to disk, and storing the *parsed* form rather than the raw body
//! means a cache hit costs one deserialize of the narrow struct instead of one
//! of the wide document.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A package's abbreviated packument: its dist-tags and its versions.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Packument {
    /// The name as the *registry* spells it — for a JSR package that is the
    /// mangled `@jsr/scope__name`, not the public `@scope/name`.
    #[serde(default)]
    pub name: String,
    /// `latest`, `next`, … → version. A spec of `foo@latest` resolves here
    /// before it ever touches a range (R773-F6 owns the dist-tag rules).
    #[serde(default, rename = "dist-tags", skip_serializing_if = "BTreeMap::is_empty")]
    pub dist_tags: BTreeMap<String, String>,
    /// Exact version → its manifest. Keyed by the version *string*: this map's
    /// ordering is lexicographic and therefore not semver order, so
    /// max-satisfying is R773-F2's `node-semver` job, never a `.last()` here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub versions: BTreeMap<String, VersionManifest>,
}

impl Packument {
    /// The version a dist-tag points at, if the registry publishes that tag.
    pub fn dist_tag(&self, tag: &str) -> Option<&VersionManifest> {
        self.dist_tags.get(tag).and_then(|v| self.versions.get(v))
    }
}

/// One published version, reduced to the fields resolution consumes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct VersionManifest {
    pub version: String,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    /// Present because the abbreviated document carries it, but followed
    /// **only for the root manifest**: a dependency's dev-dependencies are not
    /// installed, so walking them would grow the tree by a large multiple for
    /// nothing. The field is here so the root case has somewhere to read from.
    #[serde(
        default,
        rename = "devDependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub dev_dependencies: BTreeMap<String, String>,
    /// Recorded in phase 1, never followed there — R773-F4's post-pass is the
    /// only thing that resolves these (W318 §6).
    #[serde(
        default,
        rename = "peerDependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(
        default,
        rename = "peerDependenciesMeta",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub peer_dependencies_meta: BTreeMap<String, PeerDependencyMeta>,
    #[serde(
        default,
        rename = "optionalDependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub optional_dependencies: BTreeMap<String, String>,

    /// Platform gates (R773-F6). Entries may be negated (`"!win32"`), which is
    /// why these stay raw strings rather than a parsed platform enum.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub libc: Vec<String>,

    /// Present iff the version is deprecated; the string is the reason npm
    /// prints. Resolution does not skip deprecated versions (npm doesn't
    /// either) — it warns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,

    pub dist: Dist,
}

/// `peerDependenciesMeta` currently carries exactly one flag. Modelled as a
/// struct rather than a bool so a second flag is an added field, not a
/// breaking change to every match on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct PeerDependencyMeta {
    /// A missing peer marked optional is skipped instead of auto-installed.
    #[serde(default)]
    pub optional: bool,
}

/// Where the tarball is and what it must hash to.
///
/// This is the whole reason resolution can hand off to `mesofact-build`'s
/// content-addressed store without a second acquisition path: `integrity` is
/// the same SRI string `Store::insert_tarball` verifies against and keys on,
/// and `tarball` is authoritative — no URL is ever *constructed* for a
/// registry package, which is what keeps JSR from needing its own tarball
/// layout (W318 §9).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Dist {
    pub tarball: String,
    /// SRI, e.g. `sha512-…`. Optional in the type because ancient npm
    /// publishes predate SRI and carry only a hex `shasum`; a resolver that
    /// meets one must refuse it by name rather than fetch on trust — the same
    /// rule `PackageSource::Registry` enforces on the materializer side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped after a real `application/vnd.npm.install-v1+json` response,
    /// including fields we deliberately do not model (`_id`, `modified`,
    /// `bin`, `engines`, `_hasShrinkwrap`) so the drop is actually exercised.
    const FIXTURE: &str = r#"{
      "name": "demo",
      "dist-tags": { "latest": "2.0.0", "next": "3.0.0-rc.1" },
      "modified": "2026-01-01T00:00:00.000Z",
      "versions": {
        "1.0.0": {
          "name": "demo",
          "version": "1.0.0",
          "dependencies": { "left-pad": "^1.3.0" },
          "deprecated": "use 2.x",
          "dist": {
            "tarball": "https://registry.npmjs.org/demo/-/demo-1.0.0.tgz",
            "integrity": "sha512-AAAA",
            "shasum": "deadbeef",
            "fileCount": 12
          },
          "engines": { "node": ">=18" },
          "_hasShrinkwrap": false
        },
        "2.0.0": {
          "name": "demo",
          "version": "2.0.0",
          "dependencies": { "left-pad": "^1.3.0" },
          "devDependencies": { "vitest": "^2" },
          "peerDependencies": { "react": ">=18", "react-dom": ">=18" },
          "peerDependenciesMeta": { "react-dom": { "optional": true } },
          "optionalDependencies": { "fsevents": "^2.3.3" },
          "os": ["darwin", "!win32"],
          "cpu": ["arm64", "x64"],
          "libc": ["glibc"],
          "bin": { "demo": "./cli.js" },
          "dist": {
            "tarball": "https://registry.npmjs.org/demo/-/demo-2.0.0.tgz",
            "integrity": "sha512-BBBB"
          }
        }
      },
      "_id": "demo"
    }"#;

    fn fixture() -> Packument {
        serde_json::from_str(FIXTURE).expect("fixture parses")
    }

    #[test]
    fn the_abbreviated_document_parses_into_the_fields_resolution_reads() {
        let p = fixture();
        assert_eq!(p.name, "demo");
        assert_eq!(p.dist_tags["latest"], "2.0.0");
        assert_eq!(p.dist_tags["next"], "3.0.0-rc.1");
        assert_eq!(p.versions.len(), 2);

        let two = &p.versions["2.0.0"];
        assert_eq!(two.version, "2.0.0");
        assert_eq!(two.dependencies["left-pad"], "^1.3.0");
        assert_eq!(two.dev_dependencies["vitest"], "^2");
        assert_eq!(two.peer_dependencies["react"], ">=18");
        assert!(two.peer_dependencies_meta["react-dom"].optional);
        assert_eq!(two.optional_dependencies["fsevents"], "^2.3.3");
        assert_eq!(two.os, ["darwin", "!win32"]);
        assert_eq!(two.cpu, ["arm64", "x64"]);
        assert_eq!(two.libc, ["glibc"]);
        assert_eq!(two.deprecated, None);
        assert_eq!(two.dist.integrity.as_deref(), Some("sha512-BBBB"));
        assert_eq!(two.dist.tarball, "https://registry.npmjs.org/demo/-/demo-2.0.0.tgz");
    }

    #[test]
    fn a_deprecated_version_keeps_its_reason_and_is_not_dropped() {
        let p = fixture();
        assert_eq!(p.versions["1.0.0"].deprecated.as_deref(), Some("use 2.x"));
    }

    #[test]
    fn a_dist_tag_pointing_at_an_unpublished_version_is_none_not_a_panic() {
        let p = fixture();
        assert_eq!(p.dist_tag("latest").map(|v| v.version.as_str()), Some("2.0.0"));
        // `next` names 3.0.0-rc.1, which this document does not carry.
        assert!(p.dist_tag("next").is_none());
        assert!(p.dist_tag("nope").is_none());
    }

    /// The cache stores the parsed form, so a round trip has to be lossless
    /// over exactly the modelled fields — and must NOT resurrect the dropped
    /// ones.
    #[test]
    fn the_parsed_form_round_trips_through_json_without_resurrecting_dropped_fields() {
        let p = fixture();
        let re: Packument = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(p, re);

        let raw = serde_json::to_string(&p).unwrap();
        for dropped in ["readme", "maintainers", "_id", "shasum", "engines", "bin"] {
            assert!(!raw.contains(dropped), "{dropped} should not survive the model");
        }
    }

    /// JSR's `npm.jsr.io` endpoint serves an npm-shaped document, so it must
    /// land in the same struct with no JSR-specific handling (W318 §9).
    #[test]
    fn a_jsr_packument_lands_in_the_same_struct() {
        let jsr = r#"{
          "name": "@jsr/luca__flag",
          "dist-tags": { "latest": "1.0.0" },
          "versions": {
            "1.0.0": {
              "name": "@jsr/luca__flag",
              "version": "1.0.0",
              "dist": {
                "tarball": "https://npm.jsr.io/~/11/@jsr/luca__flag/1.0.0.tgz",
                "integrity": "sha512-CCCC"
              }
            }
          }
        }"#;
        let p: Packument = serde_json::from_str(jsr).unwrap();
        assert_eq!(p.name, "@jsr/luca__flag");
        assert_eq!(p.dist_tag("latest").unwrap().dist.integrity.as_deref(), Some("sha512-CCCC"));
    }

    #[test]
    fn a_version_without_integrity_parses_so_the_caller_can_refuse_it_by_name() {
        let ancient = r#"{
          "name": "old",
          "versions": {
            "0.0.1": {
              "version": "0.0.1",
              "dist": { "tarball": "https://registry.npmjs.org/old/-/old-0.0.1.tgz", "shasum": "abc" }
            }
          }
        }"#;
        let p: Packument = serde_json::from_str(ancient).unwrap();
        assert_eq!(p.versions["0.0.1"].dist.integrity, None);
    }
}
