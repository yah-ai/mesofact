//! End-to-end pipeline tests over the shared fixtures in
//! `packages/mesofact-build/tests/fixtures/`, asserting the Rust-native
//! pipeline emits the expected manifest, hydrate bundles, asset overlay,
//! and SSR probe behavior.

use mesofact_build::pipeline::{build, BuildOptions, InstallMode};
use std::path::{Path, PathBuf};

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
fn head_woven_into_shell_and_sitemap_filters_noindex_and_deferred() {
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

    // Sitemap: indexed static routes only.
    let sitemap_path = result.sitemap_path.expect("site_url set → sitemap emitted");
    assert!(sitemap_path.ends_with("sitemap.xml"), "sitemap at dist root: {sitemap_path:?}");
    let sitemap = std::fs::read_to_string(&sitemap_path).unwrap();
    assert!(sitemap.contains("<loc>https://example.test/</loc>"), "home in sitemap: {sitemap}");
    assert!(sitemap.contains("<loc>https://example.test/docs</loc>"), "docs in sitemap");
    assert!(!sitemap.contains("/secret"), "noindex route excluded: {sitemap}");
    assert!(!sitemap.contains("/c/"), "deferred route excluded: {sitemap}");
    // Deferred route prerendered nothing.
    assert!(!native.join("html/c_slug.html").exists(), "deferred route emits no html");
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
