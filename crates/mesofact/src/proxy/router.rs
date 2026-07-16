//! axum router and request dispatch for the mesofact proxy.
//!
//! Mode dispatch:
//! - **Mode 1 (static)**: 302 redirect to `cdn_base_url{path}`, or stream
//!   from `fallback_dir/{path}.html` / `fallback_dir/{path}/index.html`.
//! - **Mode 2 (ssr)**: session resolve → cache lookup (fresh / stale-SWR /
//!   miss) → Bun pool render → LRU store. See §"Cache-key composition" and
//!   §"Mode 2 caching beyond TTL".
//! - **Mode 3 (spa)**: serve the prerendered SPA shell — identical delivery to
//!   Mode 1 (302 to CDN or local fallback). The shell is built once like a
//!   static page; the client hydrates from the embedded `__MESOFACT_STATE__`
//!   and takes over (mesofact is then out of the request path). See
//!   architecture §"Bundle splitting & hydration boundary (Mode 3)".
//! - **No match**: 404.
//!
//! The route table is rebuilt from the manifest on each reload by calling
//! `build_matcher`. The matchit `Router` is stored in `AppState` alongside
//! the current `Arc<WorkerPool>` so both can be swapped atomically via
//! `Arc<RwLock<AppState>>`. Per-request work clones the Arcs it needs and
//! drops the read guard before the (potentially slow) render await.
//!
//! @yah:ticket(R012-T2, "Proxy Mode 3 dispatch: serve the prerendered SPA shell (CDN redirect / local fallback), replacing the 501 stub")
//! @yah:at(2026-05-26T16:12:02Z)
//! @yah:status(review)
//! @yah:assignee(agent:claude)
//! @yah:phase(P10)
//! @yah:parent(R012)
//! @yah:handoff("Proxy Mode 3 dispatch shipped. router.rs handle() now routes RouteMode::Static | RouteMode::Spa through the same dispatch_static() path — the prerendered SPA shell is delivered identically to a Mode 1 static page (302 to cdn_base_url{path}, or local fallback serve_local() → {path}.html / {path}/index.html). Removed the not_implemented() 501 stub (no longer referenced). The client hydrates from the build-injected __MESOFACT_STATE__ and takes over; mesofact is then out of the request path (architecture §Bundle splitting & hydration boundary). Module doc updated. Two new proxy tests (bun-guarded like the Mode 1 ones): mode3_spa_redirects_to_cdn (302 → cdn/app) and mode3_spa_serves_shell_from_fallback (200, body carries the state tag).")
//! @yah:verify("cargo test -p mesofact --test proxy")
//! @yah:verify("cargo check --workspace")

use crate::manifest::{ErrorRoutes, Manifest, Requires, Route, RouteMode};
use crate::proxy::cache::{cache_window, compose_key, CacheEntry, CacheState, KeyInputs, ResponseCache};
use crate::proxy::metrics::Metrics;
use crate::proxy::session::{SessionResolver, User};
use crate::proxy::source_gen::Generations;
use crate::proxy::trace::TraceParent;
use crate::proxy::worker_client::{RenderResult, WorkerError};
use crate::proxy::worker_pool::WorkerPool;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Per-render envelope id (`id=0` is reserved for lifecycle). Process-global so
/// ids stay unique across workers even though matching is per-connection.
static RENDER_ID: AtomicU32 = AtomicU32::new(1);

/// Render deadline handed to the worker (`deadline_ms` in the IPC envelope).
/// The architecture's worked example uses 2000ms; not yet per-route configurable.
const RENDER_DEADLINE_MS: u64 = 2000;

/// Default negative-cache TTL when a route omits `cache_policy.negative_ttl`.
const DEFAULT_NEGATIVE_TTL: u64 = 10;

/// Shared state accessed by every request handler.
pub struct AppState {
    pub manifest: Arc<Manifest>,
    /// matchit router mapping URL patterns → index into `manifest.routes`.
    pub matcher: matchit::Router<usize>,
    pub pool: Arc<WorkerPool>,
    /// If set, Mode 1 routes redirect here: `{cdn_base_url}{path}`.
    pub cdn_base_url: Option<String>,
    /// Local fallback directory for Mode 1 when no CDN is configured.
    pub fallback_dir: Option<PathBuf>,
    /// Mode 2 LRU response cache.
    pub cache: Arc<ResponseCache>,
    /// Source generation provider (cache-key input 6).
    pub generations: Arc<Generations>,
    /// Session resolver (`None` = sessions disabled; `requires: ["user"]`
    /// routes then always redirect/401).
    pub session: Option<Arc<dyn SessionResolver>>,
    /// Where to 302 a request that lacks a session on a `requires: ["user"]`
    /// route. `?next=<original-url>` is appended. `None` → 401.
    pub login_url: Option<String>,
    /// Prometheus metrics registry, shared with the `/metrics` handler and the
    /// worker pool (restarting gauge).
    pub metrics: Arc<Metrics>,
}

impl AppState {
    pub fn new(
        manifest: Arc<Manifest>,
        pool: Arc<WorkerPool>,
        cdn_base_url: Option<String>,
        fallback_dir: Option<PathBuf>,
    ) -> Self {
        let matcher = build_matcher(&manifest);
        Self {
            manifest,
            matcher,
            pool,
            cdn_base_url,
            fallback_dir,
            cache: Arc::new(ResponseCache::new()),
            generations: Arc::new(Generations::empty()),
            session: None,
            login_url: None,
            metrics: Arc::new(Metrics::new()),
        }
    }

    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = metrics;
        self
    }

    pub fn with_cache(mut self, cache: Arc<ResponseCache>) -> Self {
        self.cache = cache;
        self
    }

    pub fn with_generations(mut self, generations: Arc<Generations>) -> Self {
        self.generations = generations;
        self
    }

    pub fn with_session(mut self, session: Arc<dyn SessionResolver>) -> Self {
        self.session = Some(session);
        self
    }

    pub fn with_login_url(mut self, login_url: Option<String>) -> Self {
        self.login_url = login_url;
        self
    }
}

/// Build a matchit router from manifest routes (route pattern → route index).
pub fn build_matcher(manifest: &Manifest) -> matchit::Router<usize> {
    let mut router = matchit::Router::new();
    for (i, route) in manifest.routes.iter().enumerate() {
        // matchit uses `:param` syntax, which matches the mesofact route format.
        if let Err(e) = router.insert(&route.route, i) {
            tracing::warn!("failed to register route '{}': {e}", route.route);
        }
    }
    router
}

pub type SharedState = Arc<RwLock<AppState>>;

/// Everything Mode 2 dispatch needs, snapshotted under the read guard so the
/// guard can be dropped before the render await (a reload only blocks briefly).
struct SsrCall {
    path: String,
    route: Route,
    build_id: String,
    params: BTreeMap<String, String>,
    query: BTreeMap<String, String>,
    headers: HeaderMap,
    pool: Arc<WorkerPool>,
    cache: Arc<ResponseCache>,
    generations: Arc<Generations>,
    session: Option<Arc<dyn SessionResolver>>,
    login_url: Option<String>,
    metrics: Arc<Metrics>,
    /// `traceparent` value passed to the worker as `req.ctx.trace`.
    trace: String,
}

/// The single catch-all axum handler. Route matching, dispatch, traceparent,
/// and metrics recording all funnel through here so every response is observed
/// exactly once.
pub async fn handle(State(state): State<SharedState>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    // Continue an inbound trace or mint a fresh one (§"Observability").
    let trace = TraceParent::incoming_or_new(
        req.headers().get("traceparent").and_then(|v| v.to_str().ok()),
    );

    // Snapshot the route + everything dispatch needs under the read guard, then
    // drop it before the (possibly slow) render await.
    enum Plan {
        NotFound,
        Static { cdn: Option<String>, fallback: Option<PathBuf> },
        Ssr(Box<SsrCall>),
    }

    let (route_label, mode_label, metrics, error_pages, plan) = {
        let st = state.read().await;
        let metrics = st.metrics.clone();
        let error_pages = ErrorPages::from_state(&st);
        match st.matcher.at(&path) {
            Err(_) => (
                "<unmatched>".to_string(),
                "none",
                metrics,
                error_pages,
                Plan::NotFound,
            ),
            Ok(m) => {
                let idx = *m.value;
                let params = m
                    .params
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<BTreeMap<_, _>>();
                let route = st.manifest.routes[idx].clone();
                let route_label = route.route.clone();
                let mode_label = mode_str(&route.mode);
                debug!(trace = %trace.trace_id, "dispatching {} → mode={:?}", path, route.mode);
                let plan = match route.mode {
                    // Mode 1 (static) and Mode 3 (spa) both deliver a
                    // prerendered HTML document from the CDN/local fallback.
                    RouteMode::Static | RouteMode::Spa => Plan::Static {
                        cdn: st.cdn_base_url.clone(),
                        fallback: st.fallback_dir.clone(),
                    },
                    RouteMode::Ssr => Plan::Ssr(Box::new(SsrCall {
                        path: path.clone(),
                        route,
                        build_id: st.manifest.build_id.clone(),
                        params,
                        query: parse_kv(req.uri().query()),
                        headers: req.headers().clone(),
                        pool: st.pool.clone(),
                        cache: st.cache.clone(),
                        generations: st.generations.clone(),
                        session: st.session.clone(),
                        login_url: st.login_url.clone(),
                        metrics: metrics.clone(),
                        trace: trace.header_value(),
                    })),
                };
                (route_label, mode_label, metrics, error_pages, plan)
            }
        }
    };

    let mut resp = match plan {
        Plan::NotFound => error_pages.not_found().await,
        Plan::Static { cdn, fallback } => {
            dispatch_static(&path, cdn, fallback, &error_pages).await
        }
        Plan::Ssr(call) => dispatch_ssr(*call).await,
    };

    // Echo the traceparent so a downstream collector can stitch the trace, then
    // record the request outcome (route × mode × status).
    if let Ok(v) = HeaderValue::from_str(&trace.header_value()) {
        resp.headers_mut()
            .insert(HeaderName::from_static("traceparent"), v);
    }
    metrics.record_request(&route_label, mode_label, resp.status().as_u16());
    resp
}

/// `/metrics` — Prometheus text exposition. Mounted as a dedicated route in the
/// proxy binary so it bypasses the manifest matcher (a manifest `/metrics`
/// route would never be reached, by design).
pub async fn metrics_handler(State(state): State<SharedState>) -> Response {
    let (metrics, ready) = {
        let st = state.read().await;
        (st.metrics.clone(), st.pool.live_count().await)
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")
        .body(Body::from(metrics.render(ready)))
        .unwrap()
}

fn mode_str(mode: &RouteMode) -> &'static str {
    match mode {
        RouteMode::Static => "static",
        RouteMode::Ssr => "ssr",
        RouteMode::Spa => "spa",
    }
}

// ─── Mode 1 ───────────────────────────────────────────────────────────────

async fn dispatch_static(
    path: &str,
    cdn: Option<String>,
    fallback: Option<PathBuf>,
    error_pages: &ErrorPages,
) -> Response {
    // CDN redirect takes priority over local fallback.
    if let Some(cdn) = cdn {
        let target = format!("{cdn}{path}");
        return Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, target)
            .body(Body::empty())
            .unwrap();
    }

    if let Some(dir) = fallback {
        return serve_local(path, &dir, error_pages).await;
    }

    // Neither CDN nor fallback configured — return 502.
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(Body::from("no CDN or fallback configured for Mode 1 route"))
        .unwrap()
}

/// Serve a static file from the local fallback directory.
///
/// Path mapping: `/` → `index.html`, `/about` → `about.html`,
/// then falls back to `{path}/index.html` for directory-style routes.
async fn serve_local(path: &str, dir: &PathBuf, error_pages: &ErrorPages) -> Response {
    let relative = path.trim_start_matches('/');

    let candidates: Vec<PathBuf> = if relative.is_empty() {
        vec![dir.join("index.html")]
    } else if relative.ends_with('/') {
        vec![dir.join(relative).join("index.html")]
    } else {
        vec![
            dir.join(format!("{relative}.html")),
            dir.join(relative).join("index.html"),
        ]
    };

    for candidate in &candidates {
        match tokio::fs::read(candidate).await {
            Ok(bytes) => {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .body(Body::from(bytes))
                    .unwrap();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::error!("fallback read error for {}: {e}", candidate.display());
                return error_pages.internal_error().await;
            }
        }
    }

    error_pages.not_found().await
}

// ─── Mode 2 ───────────────────────────────────────────────────────────────

async fn dispatch_ssr(call: SsrCall) -> Response {
    let requires_user = call
        .route
        .requires
        .as_ref()
        .is_some_and(|r| r.contains(&Requires::User));

    // 1. Session resolution. The proxy owns this lookup (§"Request context").
    let cookie_header = call
        .headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok());
    let user = call.session.as_ref().and_then(|s| s.resolve(cookie_header));

    if requires_user && user.is_none() {
        return redirect_to_login(&call.path, &call.query, call.login_url.as_deref());
    }

    // 2. Cache key (§"Cache-key composition"). User id folds in only when the
    //    route requires user; the resolved (or `_anon`) id keys the entry.
    let user_key = if requires_user {
        Some(user.as_ref().map_or("_anon", |u| u.id.as_str()))
    } else {
        None
    };
    let vary = collect_vary(&call.route, &call.headers);
    let gens = collect_generations(&call.route, &call.generations);
    let key = compose_key(&KeyInputs {
        build_id: &call.build_id,
        route_pattern: &call.route.route,
        params: &call.params,
        query: &call.query,
        vary: &vary,
        source_generations: &gens,
        user_id: user_key,
    });

    let cp = &call.route.cache_policy;
    let ttl = Duration::from_secs(cp.ttl);
    let swr = Duration::from_secs(cp.swr.unwrap_or(0));
    let negative_ttl = Duration::from_secs(cp.negative_ttl.unwrap_or(DEFAULT_NEGATIVE_TTL));

    // The RenderRequest JSON is independent of cache state — build it once so a
    // background SWR refresh can reuse it.
    let req_json = build_render_request(&call, user.as_ref());

    // 3. Cache lookup.
    if let Some(entry) = call.cache.get(&key) {
        match entry.state() {
            CacheState::Fresh => {
                call.metrics.record_cache(&call.route.route, "fresh");
                return build_cached_response(&entry, "fresh", false);
            }
            CacheState::Stale => {
                // Serve stale immediately; refresh in the background.
                call.metrics.record_cache(&call.route.route, "stale");
                spawn_refresh(
                    call.pool.clone(),
                    call.cache.clone(),
                    call.metrics.clone(),
                    call.route.route.clone(),
                    req_json.clone(),
                    key.clone(),
                    ttl,
                    swr,
                    negative_ttl,
                );
                return build_cached_response(&entry, "stale", false);
            }
            CacheState::Expired => {
                // Fall through to a synchronous miss, but keep the expired body
                // for the on-error stale fallback below.
                return render_miss(&call, &key, req_json, ttl, swr, negative_ttl, Some(entry)).await;
            }
        }
    }

    render_miss(&call, &key, req_json, ttl, swr, negative_ttl, None).await
}

/// Synchronous miss: render through the pool, cache per the status class, and
/// serve. On render error, serve a still-present (expired) entry marked
/// `X-Mesofact-Stale: true`, else 503 + `Retry-After`.
async fn render_miss(
    call: &SsrCall,
    key: &str,
    req_json: serde_json::Value,
    ttl: Duration,
    swr: Duration,
    negative_ttl: Duration,
    fallback: Option<CacheEntry>,
) -> Response {
    call.metrics.inflight_inc();
    let started = Instant::now();
    let outcome = render_once(&call.pool, &call.route.route, req_json).await;
    call.metrics
        .observe_render(&call.route.route, started.elapsed().as_secs_f64());
    call.metrics.inflight_dec();

    match outcome {
        Ok(rr) => {
            let status = 200; // RenderResult carries no status; Mode 2 render → 200.
            let entry = entry_from_render(status, rr, ttl, swr);
            if let Some((store_ttl, store_swr)) =
                cache_window(status, ttl, swr, negative_ttl)
            {
                let mut to_store = entry.clone();
                to_store.ttl = store_ttl;
                to_store.swr = store_swr;
                call.cache.insert(key.to_string(), to_store);
            }
            call.metrics.record_cache(&call.route.route, "miss");
            build_cached_response(&entry, "miss", false)
        }
        Err(e) => {
            warn!("render failed for {}: {e}", call.route.route);
            match fallback {
                // On-error stale fallback within an existing entry.
                Some(entry) => {
                    call.metrics.record_cache(&call.route.route, "stale");
                    build_cached_response(&entry, "stale", true)
                }
                None => service_unavailable(ttl),
            }
        }
    }
}

/// Background SWR re-render: render and replace the entry under the same key.
/// Best-effort — errors leave the stale entry in place for the next request.
#[allow(clippy::too_many_arguments)]
fn spawn_refresh(
    pool: Arc<WorkerPool>,
    cache: Arc<ResponseCache>,
    metrics: Arc<Metrics>,
    route_pattern: String,
    req_json: serde_json::Value,
    key: String,
    ttl: Duration,
    swr: Duration,
    negative_ttl: Duration,
) {
    tokio::spawn(async move {
        metrics.inflight_inc();
        let started = Instant::now();
        let outcome = render_once(&pool, &route_pattern, req_json).await;
        metrics.observe_render(&route_pattern, started.elapsed().as_secs_f64());
        metrics.inflight_dec();
        match outcome {
            Ok(rr) => {
                let status = 200;
                if let Some((store_ttl, store_swr)) = cache_window(status, ttl, swr, negative_ttl) {
                    let mut entry = entry_from_render(status, rr, store_ttl, store_swr);
                    entry.ttl = store_ttl;
                    entry.swr = store_swr;
                    cache.insert(key, entry);
                } else {
                    cache.remove(&key);
                }
            }
            Err(e) => warn!("SWR refresh failed for {route_pattern}: {e}"),
        }
    });
}

/// Pick a worker and invoke render. Maps `queue_overflow` to a retryable error;
/// the caller's error path handles all worker errors uniformly.
async fn render_once(
    pool: &WorkerPool,
    route_pattern: &str,
    req_json: serde_json::Value,
) -> Result<RenderResult, WorkerError> {
    let worker = pool.get().await.ok_or(WorkerError::Closed)?;
    let id = RENDER_ID.fetch_add(1, Ordering::Relaxed);
    worker.render(id, route_pattern, req_json, RENDER_DEADLINE_MS).await
}

fn entry_from_render(status: u16, rr: RenderResult, ttl: Duration, swr: Duration) -> CacheEntry {
    CacheEntry {
        status,
        html: rr.html,
        headers: rr.headers.into_iter().collect(),
        ttl,
        swr,
        stored_at: Instant::now(),
    }
}

/// Construct the `RenderRequest` JSON the worker expects (§"Request context").
/// `project`/`region` resolution is post-MVP; omitted here.
fn build_render_request(call: &SsrCall, user: Option<&User>) -> serde_json::Value {
    let url = match call.query.is_empty() {
        true => call.path.clone(),
        false => format!("{}?{}", call.path, encode_query(&call.query)),
    };
    let headers: BTreeMap<&str, String> = call
        .headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let cookies = parse_cookies(&call.headers);

    let mut req = serde_json::json!({
        "url": url,
        "params": call.params,
        "query": call.query,
        "headers": headers,
        "cookies": cookies,
        // W3C trace context handed to the worker (§"Observability"). `ctx` is
        // the per-deployment escape hatch; `trace` is the one key mesofact owns.
        "ctx": { "trace": call.trace },
    });
    if let Some(u) = user {
        req["user"] = serde_json::json!({ "id": u.id, "attrs": u.attrs });
    }
    req
}

/// Collect the cache-key's `vary` inputs: each `cache_policy.vary` header's
/// value (missing headers contribute the empty string so presence/absence is
/// itself part of the key).
fn collect_vary(route: &Route, headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(vary) = &route.cache_policy.vary {
        for name in vary {
            let value = headers
                .get(name.as_str())
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            out.insert(name.clone(), value);
        }
    }
    out
}

/// Resolve a generation token for every source the route reads.
fn collect_generations(route: &Route, generations: &Generations) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(reads) = &route.source_reads {
        for name in reads {
            out.insert(name.clone(), generations.token(name));
        }
    }
    out
}

// ─── response builders ──────────────────────────────────────────────────────

fn build_cached_response(entry: &CacheEntry, cache_label: &str, stale: bool) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(entry.status).unwrap_or(StatusCode::OK));
    let mut has_content_type = false;
    for (k, v) in &entry.headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        builder = builder.header(k, v);
    }
    if !has_content_type {
        builder = builder.header(header::CONTENT_TYPE, "text/html; charset=utf-8");
    }
    builder = builder.header("x-mesofact-cache", cache_label);
    if stale {
        builder = builder.header("x-mesofact-stale", "true");
    }
    builder.body(Body::from(entry.html.clone())).unwrap()
}

fn redirect_to_login(path: &str, query: &BTreeMap<String, String>, login_url: Option<&str>) -> Response {
    let Some(login) = login_url else {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from("401 Unauthorized — session required"))
            .unwrap();
    };
    let original = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", encode_query(query))
    };
    let sep = if login.contains('?') { '&' } else { '?' };
    let target = format!("{login}{sep}next={}", percent_encode(&original));
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, target)
        .body(Body::empty())
        .unwrap()
}

fn service_unavailable(retry_after: Duration) -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::RETRY_AFTER, retry_after.as_secs().max(1).to_string())
        .body(Body::from("503 Service Unavailable"))
        .unwrap()
}

/// Renders the manifest's `error_routes` pages (W270 §3, R595-T5), retiring the
/// old hardcoded plaintext 404. Snapshotted from [`AppState`] under the read
/// guard so the error builders can run after it is dropped.
///
/// `error_routes` values are ROUTE PATHS (e.g. `"/404"` → the `/404` static
/// route), not asset keys — each is resolved to its prerendered asset in the
/// local fallback dir exactly like a normal static request (`/404` →
/// `404.html`). CDN-only deployments (no `fallback_dir`) fall back to plaintext:
/// in prod the always-up edge (`@mesofact/edge`) serves the branded page, and
/// this proxy sits behind it.
#[derive(Clone)]
struct ErrorPages {
    routes: Option<ErrorRoutes>,
    fallback_dir: Option<PathBuf>,
}

impl ErrorPages {
    fn from_state(st: &AppState) -> Self {
        Self {
            routes: st.manifest.error_routes.clone(),
            fallback_dir: st.fallback_dir.clone(),
        }
    }

    async fn not_found(&self) -> Response {
        self.render(StatusCode::NOT_FOUND, "404 Not Found").await
    }

    async fn internal_error(&self) -> Response {
        self.render(StatusCode::INTERNAL_SERVER_ERROR, "500 Internal Server Error")
            .await
    }

    /// Serve the branded error page for `status` from the fallback dir, or the
    /// plaintext `default_text` when unconfigured / no fallback dir / page
    /// missing on disk. Served *with* `status`.
    async fn render(&self, status: StatusCode, default_text: &'static str) -> Response {
        if let Some(dir) = &self.fallback_dir {
            if let Some(route) = self
                .routes
                .as_ref()
                .and_then(|r| error_route_for(r, status))
            {
                for candidate in route_to_candidates(route) {
                    if let Ok(bytes) = tokio::fs::read(dir.join(&candidate)).await {
                        return Response::builder()
                            .status(status)
                            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                            .body(Body::from(bytes))
                            .unwrap();
                    }
                }
            }
        }
        Response::builder()
            .status(status)
            .body(Body::from(default_text))
            .unwrap()
    }
}

/// The configured error route for an HTTP status class: 5xx → `5xx`, 404 → `404`.
fn error_route_for(routes: &ErrorRoutes, status: StatusCode) -> Option<&str> {
    if status.as_u16() >= 500 {
        routes.server_error.as_deref()
    } else if status == StatusCode::NOT_FOUND {
        routes.not_found.as_deref()
    } else {
        None
    }
}

/// A route path (`/404`) → the ordered relative asset candidates a prerendered
/// static route emits — the same clean-URL rule [`serve_local`] uses.
fn route_to_candidates(route: &str) -> Vec<PathBuf> {
    let rel = route.trim_start_matches('/');
    if rel.is_empty() {
        return vec![PathBuf::from("index.html")];
    }
    let last = rel.rsplit('/').next().unwrap_or(rel);
    if last.contains('.') {
        vec![PathBuf::from(rel)]
    } else {
        vec![
            PathBuf::from(format!("{rel}.html")),
            PathBuf::from(rel).join("index.html"),
        ]
    }
}

// ─── small parsers ──────────────────────────────────────────────────────────

/// Parse `a=b&c=d` into a sorted map. Values are kept raw (already percent-
/// encoded as the client sent them) — the cache key only needs determinism.
fn parse_kv(query: Option<&str>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(q) = query {
        for pair in q.split('&').filter(|p| !p.is_empty()) {
            match pair.split_once('=') {
                Some((k, v)) => out.insert(k.to_string(), v.to_string()),
                None => out.insert(pair.to_string(), String::new()),
            };
        }
    }
    out
}

fn parse_cookies(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(raw) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        for pair in raw.split(';') {
            if let Some((k, v)) = pair.split_once('=') {
                out.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    out
}

fn encode_query(query: &BTreeMap<String, String>) -> String {
    query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Minimal percent-encoding for a redirect `next=` value — encodes the
/// characters that would break the query string. Sufficient for the login
/// round-trip; a full RFC 3986 encoder isn't warranted here.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
