//! The live half of R773-F1's verification: does the real npm registry, and
//! the real JSR npm-compatibility endpoint, actually answer the way W318 §9
//! says they do?
//!
//! **`#[ignore]`d, so `cargo test` stays hermetic.** These are the checks that
//! catch a *protocol* change — a media type stopping being honoured, JSR
//! changing its mangling — which no fixture can. Run them deliberately:
//!
//! ```text
//! cargo test --manifest-path oss/mesofact/Cargo.toml -p jsget -- --ignored
//! ```

use jsget::{Acquirer, CachePolicy, Registry};

fn acquirer() -> (tempfile::TempDir, Acquirer) {
    let dir = tempfile::tempdir().expect("tempdir");
    let acquirer = Acquirer::new(dir.path()).expect("http acquirer");
    (dir, acquirer)
}

#[test]
#[ignore = "hits registry.npmjs.org"]
fn npm_serves_an_abbreviated_packument_we_can_resolve_from() {
    let (_dir, acquirer) = acquirer();
    let p = acquirer.packument(&Registry::npm(), "react").unwrap();
    assert_eq!(p.name, "react");
    let latest = p
        .dist_tag("latest")
        .expect("npm publishes a `latest` for react");
    assert!(
        latest.dist.integrity.as_deref().is_some_and(|i| i.starts_with("sha512-")),
        "expected an SRI integrity, got {:?}",
        latest.dist.integrity
    );
    assert!(latest.dist.tarball.ends_with(".tgz"), "{}", latest.dist.tarball);
}

/// W318 §9's live finding, kept honest: `npm.jsr.io` serves a *complete* npm
/// packument, so JSR needs no second resolver.
#[test]
#[ignore = "hits npm.jsr.io"]
fn jsr_serves_a_complete_npm_packument_under_the_mangled_name() {
    let (_dir, acquirer) = acquirer();
    let p = acquirer.packument(&Registry::jsr(), "@luca/flag").unwrap();
    assert_eq!(p.name, "@jsr/luca__flag");
    assert!(!p.versions.is_empty());
    let latest = p
        .dist_tag("latest")
        .expect("jsr publishes a `latest` for @luca/flag");
    assert!(
        latest.dist.integrity.as_deref().is_some_and(|i| i.starts_with("sha512-")),
        "expected an SRI integrity, got {:?}",
        latest.dist.integrity
    );
}

/// The warm-cache criterion against a real registry: the second fetch is
/// served from disk, which an `Offline` client makes unambiguous.
#[test]
#[ignore = "hits registry.npmjs.org"]
fn a_live_fetch_warms_a_cache_a_later_offline_client_can_read() {
    let dir = tempfile::tempdir().unwrap();
    Acquirer::new(dir.path())
        .unwrap()
        .packument(&Registry::npm(), "left-pad")
        .unwrap();

    let offline = Acquirer::new(dir.path()).unwrap().with_policy(CachePolicy::Offline);
    let p = offline.packument(&Registry::npm(), "left-pad").unwrap();
    assert_eq!(p.name, "left-pad");
}

/// A package that does not exist is a 404, and a 404 is `None` — not an error
/// message a caller has to pattern-match.
#[test]
#[ignore = "hits registry.npmjs.org"]
fn an_unpublished_name_comes_back_as_none() {
    let (_dir, acquirer) = acquirer();
    let absent = acquirer
        .try_packument(
            &Registry::npm(),
            "rnpm-r773-f1-this-package-does-not-exist",
        )
        .unwrap();
    assert_eq!(absent, None);
}
