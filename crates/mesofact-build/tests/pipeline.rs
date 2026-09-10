//! End-to-end pipeline tests over the shared fixtures in
//! `packages/mesofact-build/tests/fixtures/`, asserting the Rust-native
//! pipeline emits the expected manifest, hydrate bundles, asset overlay,
//! and SSR probe behavior.

use mesofact_build::pipeline::{build, BuildOptions, InstallMode};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/mesofact-build/tests/fixtures")
        .canonicalize()
        .expect("fixtures dir")
}

fn build_native(fixture: &str, out: &Path) -> mesofact_build::pipeline::BuildResult {
    build_native_with_id(fixture, out, Some(format!("test-{fixture}")))
}

fn build_native_with_id(
    fixture: &str,
    out: &Path,
    build_id: Option<String>,
) -> mesofact_build::pipeline::BuildResult {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(build(BuildOptions {
        project_root: fixtures_root().join(fixture),
        out_dir: Some(out.to_path_buf()),
        build_id,
        install: InstallMode::Never,
    }))
    .unwrap_or_else(|e| panic!("native build of {fixture} failed: {e:?}"))
}

/// Every file under `dir`, as `relative path → bytes`.
fn tree_files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).expect("read_dir").map(Result::unwrap) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), &rel, out);
            } else {
                out.insert(rel, std::fs::read(entry.path()).expect("read"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

/// Paths whose bytes differ between two built trees.
fn drifting_files(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut drifted: Vec<String> = a
        .iter()
        .filter(|(path, bytes)| b.get(*path).map(|o| o != *bytes).unwrap_or(true))
        .map(|(path, _)| path.clone())
        .collect();
    drifted.extend(b.keys().filter(|p| !a.contains_key(*p)).cloned());
    drifted.sort();
    drifted.dedup();
    drifted
}

#[test]
fn static_only_builds_manifest_and_html() {
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
fn head_woven_into_shell_and_sitemap_filters_noindex_deferred_and_error_routes() {
    let tmp = tempfile::tempdir().unwrap();
    let native = tmp.path().join("native");
    let result = build_native("head-sitemap", &native);

    // Head woven into the home shell — inside </head>, framework-escaped.
    let home = std::fs::read_to_string(native.join("html/index.html")).unwrap();
    let head_end = home.find("</head>").expect("home has a </head>");
    let title_at = home.find("<title>Home &amp; &lt;friends&gt;</title>").expect("escaped title");
    assert!(title_at < head_end, "head tags land before </head>: {home}");
    assert!(home.contains(r#"<meta property="og:title" content="Home">"#), "og woven: {home}");
    assert!(home.contains(r#"<link rel="canonical" href="https://example.test/">"#));
    assert!(!home.contains("<friends>"), "raw angle brackets must not survive: {home}");

    // noindex route still gets its robots meta woven.
    let secret = std::fs::read_to_string(native.join("html/secret.html")).unwrap();
    assert!(secret.contains(r#"<meta name="robots" content="noindex">"#));

    // Sitemap: every indexable prerendered route, whatever its mode.
    let sitemap_path = result.sitemap_path.expect("site_url set → sitemap emitted");
    assert!(sitemap_path.ends_with("sitemap.xml"), "sitemap at dist root: {sitemap_path:?}");
    let sitemap = std::fs::read_to_string(&sitemap_path).unwrap();
    assert!(sitemap.contains("<loc>https://example.test/</loc>"), "home in sitemap: {sitemap}");
    assert!(sitemap.contains("<loc>https://example.test/docs</loc>"), "docs in sitemap");
    // A prerendered spa shell sits at a fixed URL and serves indexable HTML,
    // so it belongs in the sitemap exactly as much as a static route does.
    // The old `mode == Static` gate dropped it — which on a spa-shell
    // marketing site meant the sitemap listed the 404 and nothing else
    // (R821-B2).
    assert!(sitemap.contains("<loc>https://example.test/app</loc>"), "spa shell in sitemap: {sitemap}");
    assert!(!sitemap.contains("/secret"), "noindex route excluded: {sitemap}");
    assert!(!sitemap.contains("/c/"), "deferred route excluded: {sitemap}");
    // The 404 renders indexable HTML and declares no `noindex`; it is excluded
    // solely because `error_routes` names it. Asking a crawler to index the
    // site's own failure page is never what the author meant.
    assert!(!sitemap.contains("/404"), "error route excluded: {sitemap}");
    // …and it is still prerendered — the exclusion is from the sitemap only.
    assert!(native.join("html/404.html").exists(), "error route still emits html");
    // Deferred route prerendered nothing. Emissions are path-shaped, so an
    // instance of /c/:slug would appear as `html/c/<slug>.html`.
    assert!(!native.join("html/c").exists(), "deferred route emits no html");
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

// ── build-id determinism (R870-B12 / W272 §1) ────────────────────────────────

/// Two builds of an unchanged tree must be byte-identical — including the
/// build id, which is derived from the built tree rather than stamped from a
/// clock.
///
/// This is not cosmetic and it is not a build-speed concern. A `mesofact-spa`
/// component's `dist/` IS the W272 bundle's content, so a byte that moves
/// between two builds of the same source moves the bundle digest, and a moving
/// digest defeats every part of §1 immutability: blob dedupe stops matching,
/// the node re-materializes, and the serve process re-forks — a measured ~2s of
/// 502 on an apply that changed nothing (R876-T1, us-east-001 2026-09-09).
///
/// Before R870-B12 this failed on exactly three files — `manifest.json`,
/// `tag-index.json` and every prerendered shell — because `default_build_id()`
/// was a one-second-resolution UTC stamp woven into the hydrate URL.
/// `spa` is the fixture that carries all three carriers at once.
#[test]
fn two_builds_of_an_unchanged_tree_are_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let first = tmp.path().join("first");
    let second = tmp.path().join("second");

    let a = build_native_with_id("spa", &first, None);
    let b = build_native_with_id("spa", &second, None);

    assert_eq!(a.build_id, b.build_id, "build id drifted across two identical builds");
    let drifted = drifting_files(&tree_files(&first), &tree_files(&second));
    assert!(drifted.is_empty(), "these files drifted between two identical builds: {drifted:?}");
}

/// The derived id is a hash *of the built tree*, so it has to move when the
/// tree does — a stable-but-constant id would pass the test above while
/// serving stale bytes from an immutable `<build_id>/` prefix forever.
#[test]
fn a_different_project_derives_a_different_build_id() {
    let tmp = tempfile::tempdir().unwrap();
    let spa = build_native_with_id("spa", &tmp.path().join("spa"), None);
    let islands = build_native_with_id("static-islands", &tmp.path().join("islands"), None);

    assert_ne!(spa.build_id, islands.build_id, "two different sites share a build id");
    assert_eq!(spa.build_id.len(), 32, "build id is a 32-hex-digit tree hash");
    assert!(spa.build_id.bytes().all(|b| b.is_ascii_hexdigit()));
}

/// The placeholder is an implementation detail of the derivation and must
/// never survive into the published tree — a shell that shipped it would
/// request `/__mesofact_build_id__/hydrate/…` and never hydrate.
#[test]
fn no_placeholder_survives_into_the_built_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("native");
    let result = build_native_with_id("spa", &out, None);

    for (path, bytes) in tree_files(&out) {
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("__mesofact_build_id__"), "placeholder survived in {path}");
    }
    let html = std::fs::read_to_string(out.join("html/app.html")).unwrap();
    assert!(
        html.contains(&format!("/{}/hydrate/", result.build_id)),
        "shell weaves the derived id: {html}"
    );
}

// ── Mode 2 hooks (R756-F6 / W311 §2) ─────────────────────────────────────────

/// The Rust-native pipeline is the sole production build path, so the hook
/// declaration site has to work here, not only in the TS pipeline. Same
/// fixture both sides.
#[test]
fn declared_hook_bundles_and_lands_in_the_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let native = tmp.path().join("native");
    build_native("hooks", &native);

    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(native.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["hooks"]["readyz"]["entrypoint"], "dist/server/hooks/readyz.js");
    assert!(native.join("server/hooks/readyz.js").exists());

    // A hook is not a route — it stays out of the route table and out of
    // ssr_prefixes, which is the whole reason the declaration site exists.
    let routes = manifest["routes"].as_array().unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0]["route"], "/");
    assert!(manifest["ssr_prefixes"].is_null());
}

#[test]
fn a_workload_without_hooks_emits_no_hooks_block() {
    let tmp = tempfile::tempdir().unwrap();
    let native = tmp.path().join("native");
    build_native("static-only", &native);

    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(native.join("manifest.json")).unwrap())
            .unwrap();
    assert!(manifest["hooks"].is_null(), "hook-free manifests stay byte-identical to pre-F6");
}

#[test]
fn broken_hook_default_export_fails_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let result = rt.block_on(build(BuildOptions {
        project_root: fixtures_root().join("hooks-broken"),
        out_dir: Some(tmp.path().join("native")),
        build_id: Some("test-broken-hook".into()),
        install: InstallMode::Never,
    }));
    let Err(err) = result else { panic!("hooks-broken must fail") };
    let msg = format!("{err:#}");
    assert!(msg.contains("hook readyz"), "unexpected error: {msg}");
    assert!(msg.contains("export default"), "unexpected error: {msg}");
}
