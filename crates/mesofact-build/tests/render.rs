//! Render-only entrypoint tests (W225 §3 revalidate / publish-once):
//! render one route of an already-built dist with new params or new data,
//! no rebuild, and prove the emitted bytes reflect the new inputs.

use mesofact_build::pipeline::{build, BuildOptions, InstallMode};
use mesofact_build::render::{render_route, render_route_all, RenderAllOptions, RenderOptions};
use std::collections::BTreeMap;
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

/// Publish-once instance: `/p/:id` enumerated ids 1 and 2 at build time;
/// rendering id=3 afterwards emits a brand-new `p_id__3.html` without a
/// rebuild — the deferred-param shape a share slug needs.
#[test]
fn renders_new_param_instance_without_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("native");
    build_native("static-only", &dist);
    assert!(dist.join("html/p_id__1.html").exists());
    assert!(!dist.join("html/p_id__3.html").exists());

    let mut params = BTreeMap::new();
    params.insert("id".to_string(), "3".to_string());
    let outcome = render_route(RenderOptions {
        project_root: fixtures_root().join("static-only"),
        out_dir: Some(dist.clone()),
        route: "/p/:id".to_string(),
        params,
        data: None,
        write: true,
    })
    .expect("render of new instance");

    assert_eq!(outcome.key, "p_id__3");
    assert_eq!(outcome.url, "/p/3");
    assert!(outcome.html.contains("<h1>3</h1>"), "html: {}", outcome.html);
    assert_eq!(outcome.tags, vec!["page:3".to_string()]);
    let on_disk = std::fs::read_to_string(dist.join("html/p_id__3.html")).unwrap();
    assert_eq!(on_disk, outcome.html);
    // The build-time instances are untouched.
    assert!(dist.join("html/p_id__1.html").exists());
}

/// Revalidate: `/releases` renders against explicit fresh data (the shape
/// the almanac dispatch hands over) — the emitted HTML reflects the new
/// data, not the build-time snapshot, and the bundle is byte-identical.
#[test]
fn rerenders_with_explicit_data_override() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("native");
    build_native("data-inputs", &dist);
    let built = std::fs::read_to_string(dist.join("html/releases.html")).unwrap();
    assert!(built.contains("r1: Release 1"));
    assert!(!built.contains("r9"));

    let mut data = serde_json::Map::new();
    data.insert(
        "data/sample.json".to_string(),
        serde_json::json!([{ "id": "r9", "title": "Fresh Release" }]),
    );
    let outcome = render_route(RenderOptions {
        project_root: fixtures_root().join("data-inputs"),
        out_dir: Some(dist.clone()),
        route: "/releases".to_string(),
        params: BTreeMap::new(),
        data: Some(data),
        write: true,
    })
    .expect("revalidate render");

    assert!(outcome.html.contains("r9: Fresh Release"), "html: {}", outcome.html);
    assert!(!outcome.html.contains("r1: Release 1"));
    let on_disk = std::fs::read_to_string(dist.join("html/releases.html")).unwrap();
    assert_eq!(on_disk, outcome.html);
}

/// Revalidate default shape: with no explicit data, declared data_inputs
/// are re-read fresh from the project root at render time.
#[test]
fn rereads_data_inputs_when_no_override() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("native");
    build_native("data-inputs", &dist);

    let outcome = render_route(RenderOptions {
        project_root: fixtures_root().join("data-inputs"),
        out_dir: Some(dist),
        route: "/releases".to_string(),
        params: BTreeMap::new(),
        data: None,
        write: false,
    })
    .expect("render with data_inputs re-read");
    assert!(outcome.html.contains("r1: Release 1"));
    assert!(outcome.html_path.is_none());
}

/// The full publish-once loop (W270 §2): a deferred-param route builds with
/// zero prerendered instances, the manifest carries the instance-addressed
/// marker, and a publish-time render with explicit params + data emits the
/// instance HTML.
#[test]
fn deferred_route_builds_empty_and_renders_instances_at_publish_time() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("native");
    let result = build_native("prerender-deferred", &dist);

    // Build emitted the literal route but zero instances of /c/:slug.
    assert!(dist.join("html/index.html").exists());
    let leftovers: Vec<_> = std::fs::read_dir(dist.join("html"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("c_slug"))
        .collect();
    assert!(leftovers.is_empty(), "unexpected build-time instances: {leftovers:?}");

    // The server bundle + instance-addressed manifest entry exist.
    assert!(dist.join("server/c_slug.js").exists());
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(result.manifest_path).unwrap()).unwrap();
    let route = manifest["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["route"] == "/c/:slug")
        .expect("deferred route in manifest");
    assert_eq!(route["prerender"]["deferred"], serde_json::Value::Bool(true));

    // Publish-time instance render: explicit param + explicit data.
    let mut params = BTreeMap::new();
    params.insert("slug".to_string(), "abc123".to_string());
    let mut data = serde_json::Map::new();
    data.insert("chat".to_string(), serde_json::json!({ "title": "Hello Chat" }));
    let outcome = render_route(RenderOptions {
        project_root: fixtures_root().join("prerender-deferred"),
        out_dir: Some(dist.clone()),
        route: "/c/:slug".to_string(),
        params,
        data: Some(data),
        write: true,
    })
    .expect("publish-time instance render");

    assert_eq!(outcome.key, "c_slug__abc123");
    assert_eq!(outcome.url, "/c/abc123");
    assert!(outcome.html.contains("<h1>Hello Chat</h1>"), "html: {}", outcome.html);
    assert_eq!(outcome.tags, vec!["chat:abc123".to_string()]);
    assert!(dist.join("html/c_slug__abc123.html").exists());
}

/// The revalidate verb (`--all`): a feed change re-expands the instance set
/// FRESH — new instances appear, all instances re-render with the new data —
/// against the unchanged bundle. The project root at render time carries the
/// mutated feed only; the dist came from the original build.
#[test]
fn render_all_reexpands_instances_from_fresh_data() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("native");
    build_native("prerender-from-data", &dist);
    assert!(dist.join("html/items_id__a.html").exists());
    assert!(!dist.join("html/items_id__c.html").exists());

    // Simulate the feed change: a re-titled, c added.
    let fresh_root = tmp.path().join("proj");
    std::fs::create_dir_all(fresh_root.join("data")).unwrap();
    std::fs::write(
        fresh_root.join("data/items.json"),
        r#"{"items":[{"id":"a","title":"Alpha v2"},{"id":"b","title":"Bravo"},{"id":"c","title":"Charlie"}]}"#,
    )
    .unwrap();

    let outcomes = render_route_all(RenderAllOptions {
        project_root: fresh_root,
        out_dir: Some(dist.clone()),
        route: "/items/:id".to_string(),
    })
    .expect("all-instances revalidate render");

    assert_eq!(outcomes.len(), 3);
    let a = std::fs::read_to_string(dist.join("html/items_id__a.html")).unwrap();
    assert!(a.contains("Alpha v2"), "a not re-rendered with fresh data: {a}");
    let c = std::fs::read_to_string(dist.join("html/items_id__c.html")).unwrap();
    assert!(c.contains("Charlie"), "new instance c missing: {c}");
}

/// `--all` on a deferred route is rejected — instances are publish-minted.
#[test]
fn render_all_rejects_deferred_routes() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("native");
    build_native("prerender-deferred", &dist);

    let err = render_route_all(RenderAllOptions {
        project_root: fixtures_root().join("prerender-deferred"),
        out_dir: Some(dist),
        route: "/c/:slug".to_string(),
    })
    .unwrap_err();
    assert!(err.to_string().contains("minted at publish time"), "err: {err}");
}

#[test]
fn unknown_route_and_missing_param_fail_loudly() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("native");
    build_native("static-only", &dist);

    let err = render_route(RenderOptions {
        project_root: fixtures_root().join("static-only"),
        out_dir: Some(dist.clone()),
        route: "/nope".to_string(),
        params: BTreeMap::new(),
        data: None,
        write: false,
    })
    .unwrap_err();
    assert!(err.to_string().contains("not in manifest"), "err: {err}");

    // `/p/:id` with a param map that names the wrong key.
    let mut params = BTreeMap::new();
    params.insert("slug".to_string(), "3".to_string());
    let err = render_route(RenderOptions {
        project_root: fixtures_root().join("static-only"),
        out_dir: Some(dist),
        route: "/p/:id".to_string(),
        params,
        data: None,
        write: false,
    })
    .unwrap_err();
    assert!(err.to_string().contains("missing param 'id'"), "err: {err}");
}
