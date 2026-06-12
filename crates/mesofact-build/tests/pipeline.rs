//! End-to-end pipeline tests over the shared fixtures in
//! `packages/mesofact-build/tests/fixtures/`. When `bun` is on PATH the
//! equivalence half also runs the Bun pipeline on the same fixture and
//! asserts `diff_dists` comes back clean (the R450-F2 gate in miniature —
//! the QED-hosted harness runs the same comparison against the real apps).

use mesofact_build::pipeline::{build, BuildOptions, InstallMode};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/mesofact-build/tests/fixtures")
        .canonicalize()
        .expect("fixtures dir")
}

fn build_native(fixture: &str, out: &Path) -> mesofact_build::pipeline::BuildResult {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(build(BuildOptions {
        project_root: fixtures_root().join(fixture),
        out_dir: Some(out.to_path_buf()),
        build_id: Some(format!("test-{fixture}")),
        install: InstallMode::Never,
    }))
    .unwrap_or_else(|e| panic!("native build of {fixture} failed: {e:?}"))
}

fn bun_available() -> bool {
    Command::new("bun").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Run the Bun pipeline on a fixture (into the fixture's own dist/, which
/// the TS cli always uses) and copy it aside.
fn build_legacy(fixture: &str, out: &Path) {
    let root = fixtures_root().join(fixture);
    let cli = root.join("../../../src/cli.ts").canonicalize().unwrap();
    let status = Command::new("bun")
        .arg("run")
        .arg(&cli)
        .arg(&root)
        // Fixture configs may declare r2 sources whose endpoints come from
        // env; the CLI registers adapters eagerly, so satisfy it with stubs.
        .env("FIXTURE_R2_ENDPOINT", "http://localhost:1")
        .env("AWS_ACCESS_KEY_ID", "stub")
        .env("AWS_SECRET_ACCESS_KEY", "stub")
        .status()
        .expect("spawning bun");
    assert!(status.success(), "legacy bun build of {fixture} failed");
    let dist = root.join("dist");
    copy_dir(&dist, out);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

#[test]
fn static_only_builds_and_matches_legacy() {
    let tmp = tempfile::tempdir().unwrap();
    let native = tmp.path().join("native");
    let result = build_native("static-only", &native);

    assert!(result.manifest_path.exists());
    assert!(result.tag_index_path.exists());
    assert!(native.join("html/index.html").exists());

    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&result.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["version"], "1");
    assert_eq!(manifest["build_id"], "test-static-only");

    if !bun_available() {
        eprintln!("bun not on PATH — skipping equivalence half");
        return;
    }
    let legacy = tmp.path().join("legacy");
    build_legacy("static-only", &legacy);
    let report = mesofact_build::diff::diff_dists(&legacy, &native).unwrap();
    assert!(
        report.is_equivalent(),
        "static-only fixture diverged:\n{}",
        report.findings.join("\n")
    );
}

#[test]
fn spa_fixture_emits_hashed_hydrate_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let native = tmp.path().join("native");
    build_native("spa", &native);

    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(native.join("manifest.json")).unwrap())
            .unwrap();
    let spa_route = manifest["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["mode"] == "spa")
        .expect("spa route in manifest");
    let script = spa_route["hydration"]["script"].as_str().unwrap();
    assert!(native.join("hydrate").join(script).exists(), "hydrate bundle {script} on disk");

    // The shell carries the module script tag (hydration weave).
    let key = mesofact_build::route_key::route_key(spa_route["route"].as_str().unwrap());
    let html = std::fs::read_to_string(native.join(format!("html/{key}.html"))).unwrap();
    assert!(html.contains(&format!("/test-spa/hydrate/{script}")), "weave in {html}");
}

#[test]
fn ssr_resilience_round_trips_natively() {
    let tmp = tempfile::tempdir().unwrap();
    let native = tmp.path().join("native");
    build_native("ssr-resilience", &native);

    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(native.join("manifest.json")).unwrap())
            .unwrap();
    let route = &manifest["routes"][0];
    assert_eq!(route["resilience"]["retry"]["attempts"], 3);
    assert_eq!(route["resilience"]["timeout_ms"], 5000);
    assert_eq!(manifest["ssr_prefixes"][0], "/api/submit");
}

#[test]
fn static_assets_overlay_copied_and_listed() {
    let tmp = tempfile::tempdir().unwrap();
    let native = tmp.path().join("native");
    build_native("static-assets", &native);

    assert!(native.join("html/illustrations/foo.webp").exists());
    assert!(native.join("html/robots.txt").exists());
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(native.join("manifest.json")).unwrap())
            .unwrap();
    let assets = manifest["static_assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0]["key"], "illustrations/foo.webp");
    assert_eq!(assets[0]["content_type"], "image/webp");
}

#[test]
fn ssr_broken_default_export_fails_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let result = rt.block_on(build(BuildOptions {
        project_root: fixtures_root().join("ssr-broken"),
        out_dir: Some(tmp.path().join("native")),
        build_id: Some("test-broken".into()),
        install: InstallMode::Never,
    }));
    let Err(err) = result else { panic!("ssr-broken must fail") };
    let msg = format!("{err:#}");
    assert!(msg.contains("export default"), "unexpected error: {msg}");
}
