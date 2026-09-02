//! R773-F1's two verification criteria, as tests.
//!
//! 1. Resolving a package with a warm cache issues no network request.
//! 2. An npm package and a JSR package resolve through the same code path,
//!    differing only in base URL and name mangling.
//!
//! Both are proved against `rnpm::testing::FakeTransport`, which records every
//! request it is handed. Nothing here touches the network — the live checks are
//! in `live_registries.rs` and are `#[ignore]`d.

use jsget::{Acquirer, CachePolicy, Registry};
use rnpm::testing::FakeTransport;

const NPM_BODY: &str = r#"{
  "name": "react",
  "dist-tags": { "latest": "18.3.1" },
  "versions": {
    "18.3.1": {
      "version": "18.3.1",
      "dependencies": { "loose-envify": "^1.1.0" },
      "dist": {
        "tarball": "https://registry.npmjs.org/react/-/react-18.3.1.tgz",
        "integrity": "sha512-NPM"
      }
    }
  }
}"#;

const JSR_BODY: &str = r#"{
  "name": "@jsr/luca__flag",
  "dist-tags": { "latest": "1.0.0" },
  "versions": {
    "1.0.0": {
      "version": "1.0.0",
      "dist": {
        "tarball": "https://npm.jsr.io/~/11/@jsr/luca__flag/1.0.0.tgz",
        "integrity": "sha512-JSR"
      }
    }
  }
}"#;

fn transport() -> FakeTransport {
    FakeTransport::new()
        .serving("https://registry.npmjs.org/react", Some("\"n1\""), NPM_BODY)
        .serving(
            "https://npm.jsr.io/@jsr/luca__flag",
            Some("\"j1\""),
            JSR_BODY,
        )
}

/// Criterion 2. The *same* `Acquirer::packument` call resolves both, and the
/// only observable difference is the URL it produced — base URL plus the
/// `@luca/flag` → `@jsr/luca__flag` mangling. No JSR branch runs anywhere
/// below this line (W318 §9).
#[test]
fn an_npm_package_and_a_jsr_package_resolve_through_the_same_call() {
    let dir = tempfile::tempdir().unwrap();
    let acquirer = Acquirer::with_transport(dir.path(), transport());

    let react = acquirer.packument(&Registry::npm(), "react").unwrap();
    let flag = acquirer.packument(&Registry::jsr(), "@luca/flag").unwrap();

    assert_eq!(react.dist_tags["latest"], "18.3.1");
    assert_eq!(flag.dist_tags["latest"], "1.0.0");

    // Both carry the pair `mesofact-build`'s store consumes — nothing about
    // acquisition differs downstream either.
    assert_eq!(
        react.dist_tag("latest").unwrap().dist.integrity.as_deref(),
        Some("sha512-NPM")
    );
    assert_eq!(
        flag.dist_tag("latest").unwrap().dist.integrity.as_deref(),
        Some("sha512-JSR")
    );

    let requests = acquirer.client().transport().requests();
    assert_eq!(requests.len(), 2);
    // Identical in every respect except the URL.
    assert_eq!(requests[0].accept, requests[1].accept);
    assert_eq!(requests[0].authorization, requests[1].authorization);
    assert_eq!(
        requests.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
        [
            "https://registry.npmjs.org/react",
            "https://npm.jsr.io/@jsr/luca__flag",
        ]
    );
}

/// Criterion 1, at the facade rather than at `rnpm`'s own seam.
#[test]
fn a_warm_cache_issues_no_network_request_for_either_registry() {
    let dir = tempfile::tempdir().unwrap();
    let acquirer = Acquirer::with_transport(dir.path(), transport());

    acquirer.packument(&Registry::npm(), "react").unwrap();
    acquirer.packument(&Registry::jsr(), "@luca/flag").unwrap();
    assert_eq!(acquirer.client().transport().hits(), 2);

    for _ in 0..3 {
        acquirer.packument(&Registry::npm(), "react").unwrap();
        acquirer.packument(&Registry::jsr(), "@luca/flag").unwrap();
    }
    assert_eq!(
        acquirer.client().transport().hits(),
        2,
        "a warm cache went back to the network"
    );

    // And a fresh process over the same cache directory, offline, still
    // resolves both — the warmth is on disk, not in this `Acquirer`.
    let offline = Acquirer::with_transport(dir.path(), FakeTransport::new())
        .with_policy(CachePolicy::Offline);
    offline.packument(&Registry::npm(), "react").unwrap();
    offline.packument(&Registry::jsr(), "@luca/flag").unwrap();
    assert_eq!(offline.client().transport().hits(), 0);
}

/// The two registries namespace their caches, so a mirror serving a different
/// body under a colliding name cannot be mistaken for the other's entry.
#[test]
fn a_cached_entry_belongs_to_one_registry_only() {
    let dir = tempfile::tempdir().unwrap();
    let mirror = Registry::npm_compatible("internal", "https://npm.internal.test");
    let acquirer = Acquirer::with_transport(
        dir.path(),
        transport().serving(
            "https://npm.internal.test/react",
            Some("\"i1\""),
            &NPM_BODY.replace("18.3.1", "18.2.0"),
        ),
    );

    assert_eq!(
        acquirer.packument(&Registry::npm(), "react").unwrap().dist_tags["latest"],
        "18.3.1"
    );
    assert_eq!(
        acquirer.packument(&mirror, "react").unwrap().dist_tags["latest"],
        "18.2.0"
    );
    assert_eq!(acquirer.client().transport().hits(), 2);
}

/// A JSR name that is not scoped never becomes a request — the facade refuses
/// it by naming the form it wanted, rather than fetching a mangled guess.
#[test]
fn an_unresolvable_jsr_name_fails_before_the_network() {
    let dir = tempfile::tempdir().unwrap();
    let acquirer = Acquirer::with_transport(dir.path(), transport());
    let err = acquirer
        .packument(&Registry::jsr(), "flag")
        .unwrap_err()
        .to_string();
    assert!(err.contains("@scope/name"), "{err}");
    assert_eq!(acquirer.client().transport().hits(), 0);
}
