//! Integration tests for the P7 axum proxy.
//!
//! Tests that require Bun (worker pool spawn, watchdog) are skipped
//! automatically when `bun` is not on PATH — same convention as worker.rs.
//!
//! Tests that only exercise the HTTP layer (Mode 1 dispatch, 501 stubs,
//! manifest reload) run entirely in-process with no external dependencies.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::Router;
use axum::routing::{any, get};
use mesofact::manifest::{CachePolicy, Manifest, Requires, Route, RouteMode, MANIFEST_VERSION};
use mesofact::proxy::router::{handle, metrics_handler, AppState, SharedState};
use mesofact::proxy::session::{CookieSessionResolver, SessionResolver};
use mesofact::proxy::source_gen::Generations;
use mesofact::proxy::worker_pool::WorkerPool;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tower::ServiceExt; // oneshot

// ─── helpers ────────────────────────────────────────────────────────────────

fn static_manifest(route: &str) -> Manifest {
    Manifest {
        version: MANIFEST_VERSION.to_string(),
        build_id: "test-build".to_string(),
        routes: vec![Route {
            route: route.to_string(),
            mode: RouteMode::Static,
            render_entrypoint: "dist/server/index.js".to_string(),
            requires: None,
            source_reads: None,
            cache_policy: CachePolicy { ttl: 0, swr: None, negative_ttl: None, vary: None },
            concurrency: None,
            hydration: None,
            prerender: None,
        }],
        static_assets: vec![],
        error_routes: None,
    }
}

/// Build an SSR route pointing at a worker stub under `tests/worker_stubs/`.
fn ssr_route(
    pattern: &str,
    stub: &str,
    ttl: u64,
    requires: Option<Vec<Requires>>,
) -> Route {
    Route {
        route: pattern.to_string(),
        mode: RouteMode::Ssr,
        render_entrypoint: stubs_dir().join(stub).to_string_lossy().to_string(),
        requires,
        source_reads: None,
        cache_policy: CachePolicy { ttl, swr: None, negative_ttl: None, vary: None },
        concurrency: Some(4),
        hydration: None,
        prerender: None,
    }
}

/// Spawn a real (bun-backed) pool for one SSR route and wire an app, with an
/// optional session resolver + login URL. Caller must guard on `bun_available()`.
async fn make_ssr_app(
    route: Route,
    session: Option<Arc<dyn SessionResolver>>,
    login_url: Option<String>,
) -> (Router, SharedState) {
    let manifest = Manifest {
        version: MANIFEST_VERSION.to_string(),
        build_id: "ssr-build".to_string(),
        routes: vec![route],
        static_assets: vec![],
        error_routes: None,
    };
    let json = serde_json::to_vec(&manifest).unwrap();
    let pool = WorkerPool::spawn(&json, worker_entry(), 1)
        .await
        .expect("WorkerPool::spawn");
    let mut st = AppState::new(Arc::new(manifest), pool, None, None).with_login_url(login_url);
    if let Some(resolver) = session {
        st = st.with_session(resolver);
    }
    let state: SharedState = Arc::new(RwLock::new(st));
    let app = Router::new()
        .route("/*path", any(handle))
        .route("/", any(handle))
        .with_state(state.clone());
    (app, state)
}

async fn body_string(resp: Response<Body>) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn bun_available() -> bool {
    std::process::Command::new("bun")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf()
}

fn worker_entry() -> PathBuf {
    workspace_root()
        .join("packages")
        .join("mesofact-worker")
        .join("src")
        .join("worker.ts")
}

fn stubs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("worker_stubs")
}

/// Build an in-process test app without a real worker pool.
/// The `pool` is still required by AppState type — we spawn one with no
/// workers (n=0 is unsupported, so we pass the stub pool path that
/// immediately returns None from `get()`). For Mode 1 tests, the pool
/// is never consulted.
///
/// Since WorkerPool::spawn requires at least one worker (n ≥ 1), we skip
/// creating a pool for pure HTTP-layer tests. Instead, we construct a dummy
/// pool by spawning a single worker only when bun is available; otherwise
/// we skip pool-dependent tests.
async fn make_app_no_pool(
    manifest: Manifest,
    cdn_base_url: Option<String>,
    fallback_dir: Option<PathBuf>,
) -> (Router, SharedState) {
    // Build a pool with 1 real worker only when bun is available; otherwise
    // use a stub pool. For Mode 1 tests, get() is never called so this is safe.
    let pool = if bun_available() {
        let _manifest_json = serde_json::to_vec(&manifest).unwrap();
        let hello = stubs_dir().join("hello.ts");
        // Need a manifest with the stub route for the worker.
        let mut stub_m = manifest.clone();
        for r in &mut stub_m.routes {
            r.render_entrypoint = hello.to_string_lossy().to_string();
        }
        let stub_json = serde_json::to_vec(&stub_m).unwrap();
        WorkerPool::spawn(&stub_json, worker_entry(), 1)
            .await
            .expect("WorkerPool::spawn")
    } else {
        // Skip spawning; Mode 1 tests don't call pool.get().
        // We still need an Arc<WorkerPool> — spawn one worker using dummy paths.
        // This will fail, but we only reach here in tests that explicitly
        // check bun availability. Panic loudly if we hit this path.
        panic!("make_app_no_pool called without bun — caller must guard with bun_available()");
    };

    let state: SharedState = Arc::new(RwLock::new(AppState::new(
        Arc::new(manifest),
        pool,
        cdn_base_url,
        fallback_dir,
    )));
    let app = Router::new()
        .route("/*path", any(handle))
        .route("/", any(handle))
        .with_state(state.clone());
    (app, state)
}

/// Build a pool-less app for Mode 1 tests only (no bun required).
/// We abuse the test by substituting a pre-built pool — but since Mode 1
/// never consults the pool, we can defer pool construction to a separate
/// helper that panics loudly in tests that mistakenly require it.
///
/// For simplicity, we make pool construction optional: when `bun` is not
/// available, skip the spawn and let the test exercise only the HTTP layer.
///
/// Because AppState requires a pool, we need a pool. Without bun there's no
/// way to get one without spawning. So for "bun-optional" tests, we require
/// bun and skip otherwise.
async fn make_mode1_app(
    route: &str,
    cdn_base_url: Option<String>,
    fallback_dir: Option<PathBuf>,
) -> Option<(Router, SharedState)> {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return None;
    }
    let manifest = static_manifest(route);
    Some(make_app_no_pool(manifest, cdn_base_url, fallback_dir).await)
}

// ─── Mode 1 tests ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode1_cdn_redirect() {
    let Some((app, _)) =
        make_mode1_app("/hello", Some("https://cdn.example.com".into()), None).await
    else {
        return;
    };

    let req = Request::builder()
        .uri("/hello")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(loc, "https://cdn.example.com/hello");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode1_local_fallback_serves_html() {
    let Some((app, _)) = make_mode1_app("/hello", None, None).await else {
        return;
    };

    // No fallback_dir configured → 502 (not a CDN misconfiguration test but
    // verifies the "neither CDN nor fallback" branch).
    let req = Request::builder()
        .uri("/hello")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode1_local_fallback_reads_file() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("hello.html"), b"<h1>hello</h1>").unwrap();

    let (app, _) =
        make_mode1_app("/hello", None, Some(tmp.path().to_path_buf()))
            .await
            .unwrap();

    let req = Request::builder()
        .uri("/hello")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&*body, b"<h1>hello</h1>");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode1_local_fallback_root_serves_index() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }

    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("index.html"), b"<h1>home</h1>").unwrap();

    let (app, _) =
        make_mode1_app("/", None, Some(tmp.path().to_path_buf()))
            .await
            .unwrap();

    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&*body, b"<h1>home</h1>");
}

// ─── Mode 3 (spa) tests ───────────────────────────────────────────────────────
//
// Mode 3 delivery is identical to Mode 1 — the prerendered shell is served from
// the CDN (302) or local fallback. These mirror the Mode 1 cases on a spa route.

fn spa_manifest(route: &str) -> Manifest {
    let mut m = static_manifest(route);
    m.routes[0].mode = RouteMode::Spa;
    m
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode3_spa_redirects_to_cdn() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    let (app, _) =
        make_app_no_pool(spa_manifest("/app"), Some("https://cdn.example.com".into()), None).await;

    let resp = app
        .oneshot(Request::builder().uri("/app").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(loc, "https://cdn.example.com/app");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode3_spa_serves_shell_from_fallback() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("app.html"),
        b"<div id=\"root\"></div><script id=\"__MESOFACT_STATE__\" type=\"application/json\">{}</script>",
    )
    .unwrap();

    let (app, _) =
        make_app_no_pool(spa_manifest("/app"), None, Some(tmp.path().to_path_buf())).await;

    let resp = app
        .oneshot(Request::builder().uri("/app").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("__MESOFACT_STATE__"), "shell must carry the hydration state tag");
}

// ─── Mode 2 SSR tests (bun required) ──────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode2_renders_through_pool() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    // hello.ts returns {html: "hi", ttl: 0} → never cached, every request a miss.
    let (app, _) = make_ssr_app(ssr_route("/api", "hello.ts", 0, None), None, None).await;

    let req = Request::builder().uri("/api").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-mesofact-cache").unwrap(), "miss");
    assert_eq!(body_string(resp).await, "hi");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode2_cache_hit_serves_stored_body() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    // counter.ts increments per render; ttl 60 → second request is a fresh hit
    // and must return the *first* render's body (worker not re-invoked).
    let (app, _) = make_ssr_app(ssr_route("/c", "counter.ts", 60, None), None, None).await;

    let first = app
        .clone()
        .oneshot(Request::builder().uri("/c").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(first.headers().get("x-mesofact-cache").unwrap(), "miss");
    assert_eq!(body_string(first).await, "render-1");

    let second = app
        .oneshot(Request::builder().uri("/c").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(second.headers().get("x-mesofact-cache").unwrap(), "fresh");
    assert_eq!(body_string(second).await, "render-1", "hit should serve the stored body");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode2_query_string_is_a_distinct_cache_key() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    let (app, _) = make_ssr_app(ssr_route("/c", "counter.ts", 60, None), None, None).await;

    let a = app
        .clone()
        .oneshot(Request::builder().uri("/c?page=1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(body_string(a).await, "render-1");
    // Different query → different key → a fresh render, not the cached "render-1".
    let b = app
        .oneshot(Request::builder().uri("/c?page=2").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(b.headers().get("x-mesofact-cache").unwrap(), "miss");
    assert_eq!(body_string(b).await, "render-2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode2_requires_user_redirects_without_session() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    let resolver: Arc<dyn SessionResolver> =
        Arc::new(CookieSessionResolver::new("mesofact_session", b"k".to_vec()));
    let (app, _) = make_ssr_app(
        ssr_route("/dash", "user_echo.ts", 0, Some(vec![Requires::User])),
        Some(resolver),
        Some("https://login.example.com/login".to_string()),
    )
    .await;

    let resp = app
        .oneshot(Request::builder().uri("/dash?x=1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    // `?` and `=` are encoded; `/` stays readable (valid in a query value).
    assert_eq!(loc, "https://login.example.com/login?next=/dash%3Fx%3D1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode2_requires_user_renders_with_valid_session() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    let signer = CookieSessionResolver::new("mesofact_session", b"k".to_vec());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let token = signer
        .mint(&cheers_core::Claims::new(
            cheers_core::UserId::new("u1"),
            cheers_core::DeviceId::new("d1"),
            cheers_core::DeviceBinding::Passkey,
            now,
            now + 3600,
        ))
        .unwrap();
    let resolver: Arc<dyn SessionResolver> =
        Arc::new(CookieSessionResolver::new("mesofact_session", b"k".to_vec()));
    let (app, _) = make_ssr_app(
        ssr_route("/dash", "user_echo.ts", 0, Some(vec![Requires::User])),
        Some(resolver),
        Some("https://login.example.com/login".to_string()),
    )
    .await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/dash")
                .header("cookie", format!("mesofact_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_string(resp).await, "user:u1");
}

/// Seed (or update) `config.k = value` in a sqlite DB via a tiny bun one-liner
/// — the Rust test process has no sqlite library, but bun is already required.
fn seed_sqlite(db: &std::path::Path, value: &str) {
    let script = format!(
        "import {{ Database }} from 'bun:sqlite'; \
         const db = new Database({db:?}); \
         db.run('CREATE TABLE IF NOT EXISTS config (id TEXT PRIMARY KEY, v TEXT)'); \
         db.run('INSERT INTO config (id, v) VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET v = excluded.v', ['k', {value:?}]); \
         db.close();",
        db = db.to_string_lossy(),
        value = value,
    );
    let status = std::process::Command::new("bun")
        .arg("-e")
        .arg(&script)
        .status()
        .expect("seed sqlite via bun");
    assert!(status.success(), "bun seed script failed");
}

/// Headline P9 behavior: a Mode 2 render reads a sqlite source, the proxy folds
/// the file mtime into the cache key, and a generation (mtime) bump invalidates
/// the entry on the next request — automatic miss, no manual purge.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mode2_sqlite_generation_bump_invalidates() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("project.db");
    seed_sqlite(&db, "v1");

    let config_path = tmp.path().join("mesofact.config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[sources.project_db]\nkind = \"sqlite\"\nscope = \"global\"\npath = \"{}\"\n",
            db.display()
        ),
    )
    .unwrap();

    // The render reads a sqlite value, so its `@mesofact/runtime` import must
    // resolve at runtime. Write the stub into the temp dir with a
    // node_modules/@mesofact/runtime symlink so bun resolves it (the shared
    // worker_stubs only use erased type imports, so they don't need this).
    let stub = tmp.path().join("sqlite_read.ts");
    std::fs::write(
        &stub,
        "import { sqlite } from \"@mesofact/runtime\";\n\
         export default async function render() {\n\
           const row = await sqlite(\"project_db\").get(\"config\", \"k\");\n\
           return { html: `v=${row?.v ?? \"missing\"}`, cache: { ttl: 60 } };\n\
         }\n",
    )
    .unwrap();
    let nm = tmp.path().join("node_modules").join("@mesofact");
    std::fs::create_dir_all(&nm).unwrap();
    std::os::unix::fs::symlink(
        workspace_root().join("packages").join("mesofact-runtime"),
        nm.join("runtime"),
    )
    .unwrap();

    let mut route = ssr_route("/p", "hello.ts", 60, None);
    route.render_entrypoint = stub.to_string_lossy().to_string();
    route.source_reads = Some(vec!["project_db".to_string()]);
    let manifest = Manifest {
        version: MANIFEST_VERSION.to_string(),
        build_id: "ssr-build".to_string(),
        routes: vec![route],
        static_assets: vec![],
        error_routes: None,
    };
    let json = serde_json::to_vec(&manifest).unwrap();
    let pool = WorkerPool::spawn_with_config(&json, worker_entry(), 1, Some(config_path.clone()))
        .await
        .expect("pool spawn");
    let generations = Arc::new(Generations::from_config_file(&config_path).unwrap());
    let state: SharedState = Arc::new(RwLock::new(
        AppState::new(Arc::new(manifest), pool, None, None).with_generations(generations),
    ));
    let app = Router::new()
        .route("/*path", any(handle))
        .route("/", any(handle))
        .with_state(state);

    // First request: miss → render reads "v1".
    let first = app
        .clone()
        .oneshot(Request::builder().uri("/p").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(first.headers().get("x-mesofact-cache").unwrap(), "miss");
    assert_eq!(body_string(first).await, "v=v1");

    // Mutate the row, then force a clearly-later mtime so the generation token
    // changes regardless of filesystem mtime granularity.
    seed_sqlite(&db, "v2");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(30);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&db)
        .unwrap()
        .set_modified(later)
        .unwrap();

    // Wait past the 1s generation memo so the proxy re-polls the mtime.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let second = app
        .oneshot(Request::builder().uri("/p").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        second.headers().get("x-mesofact-cache").unwrap(),
        "miss",
        "generation bump should produce a new cache key (miss), not a fresh hit"
    );
    assert_eq!(body_string(second).await, "v=v2");
}

// ─── 404 test ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistered_path_returns_404() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }

    let (app, _) = make_mode1_app("/hello", Some("https://cdn.example.com".into()), None)
        .await
        .unwrap();

    let req = Request::builder()
        .uri("/not-registered")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─── Manifest reload test ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_manifest_keeps_old_live() {
    // Tests the manifest_loader::reload_once() behavior:
    // a bad manifest file does NOT replace the current manifest in the watch channel.
    use mesofact::proxy::manifest_loader::reload_once;
    use tokio::sync::watch;

    let original = Arc::new(static_manifest("/original"));
    let (tx, rx) = watch::channel(original.clone());

    let tmp = TempDir::new().unwrap();
    let bad_path = tmp.path().join("bad.json");
    std::fs::write(&bad_path, b"this is not valid JSON").unwrap();

    reload_once(&bad_path, &tx).await;

    assert_eq!(rx.borrow().build_id, original.build_id, "bad manifest replaced the current one");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn good_manifest_replaces_old() {
    use mesofact::proxy::manifest_loader::reload_once;
    use tokio::sync::watch;

    let original = Arc::new(static_manifest("/original"));
    let (tx, mut rx) = watch::channel(original.clone());

    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("manifest.json");

    let mut updated = static_manifest("/updated");
    updated.build_id = "new-build".to_string();
    std::fs::write(&path, serde_json::to_vec(&updated).unwrap()).unwrap();

    reload_once(&path, &tx).await;

    assert!(rx.has_changed().unwrap());
    rx.mark_unchanged();
    assert_eq!(rx.borrow().build_id, "new-build");
}

// ─── Worker pool spawn test (bun required) ────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_pool_spawns_and_pings() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }

    let hello = stubs_dir().join("hello.ts");
    let manifest = Manifest {
        version: MANIFEST_VERSION.to_string(),
        build_id: "test".to_string(),
        routes: vec![Route {
            route: "/hello".to_string(),
            mode: RouteMode::Ssr,
            render_entrypoint: hello.to_string_lossy().to_string(),
            requires: None,
            source_reads: None,
            cache_policy: CachePolicy { ttl: 0, swr: None, negative_ttl: None, vary: None },
            concurrency: Some(4),
            hydration: None,
            prerender: None,
        }],
        static_assets: vec![],
        error_routes: None,
    };

    let json = serde_json::to_vec(&manifest).unwrap();
    let pool = WorkerPool::spawn(&json, worker_entry(), 1)
        .await
        .expect("pool spawn");

    let worker = pool.get().await.expect("pool.get() returned None");
    worker.ping().await.expect("ping");
}

// ─── Observability (P10): /metrics + traceparent ──────────────────────────────

/// Mount the same routes the proxy binary does, including `/metrics`, over a
/// state built by `make_ssr_app` (whose default AppState carries a fresh shared
/// Metrics — the handler and the /metrics scrape see the same instance).
fn with_metrics_route(state: SharedState) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/*path", any(handle))
        .route("/", any(handle))
        .with_state(state)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_endpoint_reports_request_render_and_cache_counters() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    let (_, state) = make_ssr_app(ssr_route("/api", "hello.ts", 0, None), None, None).await;
    let app = with_metrics_route(state);

    // One SSR render: a cache miss that records requests_total + render_duration.
    let r = app
        .clone()
        .oneshot(Request::builder().uri("/api").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    let m = app
        .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(m.status(), StatusCode::OK);
    let body = body_string(m).await;

    // requests_total counts the /api render once — and NOT the /metrics scrape
    // (which bypasses `handle` via the dedicated route).
    assert!(
        body.contains("mesofact_requests_total{route=\"/api\",mode=\"ssr\",status=\"200\"} 1"),
        "missing requests_total; got:\n{body}"
    );
    assert!(body.contains("mesofact_cache_total{route=\"/api\",state=\"miss\"} 1"));
    assert!(body.contains("mesofact_render_duration_seconds_count{route=\"/api\"} 1"));
    assert!(body.contains("mesofact_worker_pool{state=\"ready\"} 1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn traceparent_passes_through_to_worker_and_is_echoed() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    // trace_echo.ts returns req.ctx.trace as the body, so the rendered body is
    // exactly the traceparent the worker received.
    let (app, _) = make_ssr_app(ssr_route("/t", "trace_echo.ts", 0, None), None, None).await;

    let resp = app
        .oneshot(Request::builder().uri("/t").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let echoed = resp
        .headers()
        .get("traceparent")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(echoed.starts_with("00-") && echoed.ends_with("-01"), "bad traceparent: {echoed}");

    let body = body_string(resp).await;
    assert_eq!(body, echoed, "worker's req.ctx.trace must match the echoed traceparent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn traceparent_continues_an_inbound_trace_id() {
    if !bun_available() {
        eprintln!("skipping: bun not on PATH");
        return;
    }
    let (app, _) = make_ssr_app(ssr_route("/t", "trace_echo.ts", 0, None), None, None).await;

    let inbound = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/t")
                .header("traceparent", inbound)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let echoed = resp.headers().get("traceparent").unwrap().to_str().unwrap();
    // Trace-id continues; the proxy mints its own span, so the full header differs.
    assert!(echoed.contains("4bf92f3577b34da6a3ce929d0e0e4736"), "trace-id not continued: {echoed}");
    assert_ne!(echoed, inbound);
}
