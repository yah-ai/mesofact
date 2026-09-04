//! R773-F7's conformance corpus, as `cargo test`.
//!
//! The corpus's primary home is the `mesofact-npm-conformance` QED pipeline,
//! which runs `mesofact-conformance check` through
//! `oss/mesofact/scripts/check-npm-conformance.sh`. This file is the same gate
//! wired into the ordinary test run, because a differential harness that only
//! ever executes in CI is one nobody notices breaking while they work.
//!
//! Everything here is hermetic — no network, no `bun`, no `pnpm`, no clock.
//! That is not a convention this file follows, it is enforced by the transport
//! `mesofact_build::conformance` installs, which errors on every request. See
//! that module's header.

use mesofact_build::conformance;
use std::path::PathBuf;

/// The corpus, resolved off `CARGO_MANIFEST_DIR` so it is found from any
/// working directory.
///
/// `None` when the directory is absent, which happens in exactly one place: a
/// crate unpacked from the `.crate` tarball, since `Cargo.toml` excludes the
/// corpus from the published package (3 MB of registry fixtures is not
/// something a downstream consumer should pay to depend on this crate). The
/// tests below skip rather than fail there — the *gate* cannot be fooled by
/// this, because `check-npm-conformance.sh` asserts the corpus exists and has
/// a floor of cases before it runs anything.
fn corpus() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(conformance::CORPUS_DIR);
    dir.is_dir().then_some(dir)
}

/// The gate itself: every recorded case still resolves to the tree `bun
/// install` and `pnpm install` produced for the same manifest.
///
/// A failure here names the package and the version that differed. It is a real
/// finding, not a flake — this test reads no clock and no network, so the only
/// things that can change its answer are the resolver and the corpus.
#[test]
fn the_corpus_agrees_with_bun_and_pnpm() {
    let Some(root) = corpus() else { return };
    let report = conformance::check_corpus(&root).expect("replaying the corpus");
    assert!(report.passed(), "\n{}", report.render());
}

/// A gate that cannot fail is not a gate. The corpus emptying out — a bad
/// merge, a `.gitignore` that swallowed the fixtures, a recorder that wrote
/// somewhere else — would otherwise read as a pass.
#[test]
fn the_corpus_is_large_enough_to_gate_anything() {
    let Some(root) = corpus() else { return };
    let cases = conformance::case_dirs(&root).expect("listing cases");
    assert!(
        cases.len() >= 10,
        "the corpus is down to {} cases; R773-F7 sized it at 10-20 covering nested conflicts, \
         peers, overrides, platform gating, dist-tags, scopes, aliases and real-world trees",
        cases.len()
    );
}

/// Hermeticity, proved rather than asserted: a case whose packument is missing
/// fails by NAME instead of quietly reaching registry.npmjs.org.
///
/// This is the property that makes the whole corpus trustworthy. If a miss fell
/// through to the network, a fixture could rot away entirely and the gate would
/// keep passing — against whatever the registry happens to hold today, which is
/// the opposite of what a recorded corpus is for.
#[test]
fn a_missing_packument_is_an_error_not_a_download() {
    let Some(root) = corpus() else { return };
    let scratch = tempfile::tempdir().expect("tempdir");
    let case = scratch.path().join("no-such-packument");
    std::fs::create_dir_all(case.join(conformance::PACKUMENT_DIR)).expect("case dir");
    std::fs::copy(
        root.join("nested-version-conflict/case.toml"),
        case.join("case.toml"),
    )
    .expect("borrow a case.toml");
    std::fs::write(
        case.join("package.json"),
        r#"{ "name": "x", "dependencies": { "left-pad": "^1.0.0" } }"#,
    )
    .expect("manifest");

    let err = conformance::check_case(&case).unwrap_err().to_string();
    let chain = format!("{err} {:?}", conformance::check_case(&case).unwrap_err());
    assert!(chain.contains("left-pad"), "{chain}");
    assert!(chain.contains("offline"), "{chain}");
}

/// Every case says what it stresses.
///
/// This used to also assert that every case pinned a host npm models, back when
/// a case was resolved and filtered against one. R773-F9 made both sides of the
/// diff host-independent and `[host]` came out of the format; what remains is
/// the description, which is the part a reader needs.
#[test]
fn every_case_says_what_it_stresses() {
    let Some(root) = corpus() else { return };
    for dir in conformance::case_dirs(&root).expect("listing cases") {
        let spec = conformance::load_spec(&dir).expect("case.toml");
        assert!(
            !spec.description.trim().is_empty(),
            "{} has no description; the corpus is authored for divergence, and a case that \
             cannot say what it stresses does not earn its bytes",
            dir.display()
        );
    }
}
