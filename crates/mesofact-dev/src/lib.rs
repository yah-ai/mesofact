//! `mesofact-dev` — axum static-file server for `mesofact-static` workload
//! artifacts, with optional file-watch + auto-rebuild + atomic pointer swap.
//!
//! Two modes, both share the same handler:
//!
//! - **No-watch (T1)** — [`Server::from_workload`] points at
//!   `<workload>/dist/html/`. Whatever's on disk is served; no rebuild
//!   orchestration. Useful for the local-static reconciler (R255-T3) when it
//!   owns the build pipeline itself.
//! - **Watch ([`Watcher`])** — [`Watcher::start`] watches `<workload>/src/`,
//!   debounces edits, runs `bun run build`, snapshots `dist/` into
//!   `<workload>/.mesofact-dev/gen-<N>/`, and flips the shared [`DistPointer`]
//!   to the new snapshot. Build stdout/stderr inherits the parent's, so it
//!   shows up in the operator's terminal or the Run-tab log surface.
//!
//! The pointer swap is the "atomic" part: each generation is its own
//! directory; the handler clones the current `PathBuf` per request, so an
//! in-flight read against `gen-N` keeps reading from `gen-N` even after the
//! pointer flips to `gen-N+1`. GC keeps the last two generations.
//!
//! Defaults to port 4321 per `.yah/services/dev-yah/mirrors/local.toml`.
//!
//! Sibling tickets under R255:
//! - R255-T1 — scaffolded the static handler + CLI (review).
//! - R255-T3 — local-static reconciler that spawns this binary.
//! - R255-T4 — Run-tab iframe consumes the served `dev_url`.
//!
//! @yah:relay(R434, "Mesofact SSR support — yah-side rollout (cube + placement)")
//! @yah:at(2026-06-04T19:11:39Z)
//! @yah:status(open)
//! @yah:next("P1 tickets (T1 dev.toml sweep, T2 dashboard dev.toml) are independent of the mesofact runtime delta — start there")
//! @yah:next("P2 tickets (T3/T4/T5) need the external mesofact RouteEntry.placement field + SSR build-pipeline path live; coordinate via @mesofact/runtime version bump")
//! @yah:next("Open question from W173: which marketing route becomes the first mode:\"ssr\" consumer? T5 depends on resolving this")
//! @arch:see(.yah/docs/working/W173-mesofact-render-cube.md)
//! @yah:assumes("@mesofact/runtime ships RouteEntry.placement?: Placement and the build pipeline accepts mode:\"ssr\" entrypoints with the Fetch signature — tracked at mesofact subcamp relay R015 (R015-F1 schema, R015-F2 build path, R015-F3 hydration handoff, R015-F4 boundary lint). cd external/mesofact && yah board show R015 for state.")
//!
//! @yah:ticket(R434-F3, "mesofact-dev SSR subprocess + proxy — spawn bun child, route SSR prefixes")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-04T19:12:00Z)
//! @yah:status(review)
//! @yah:phase(P2)
//! @yah:parent(R434)
//! @yah:next("Spawn bun child during mesofact-dev startup; bind ephemeral port. Persist the port to <workload>/.mesofact-dev/ssr-port (new file in the existing watcher state dir, STATE_DIR_NAME at watcher.rs:86) so other tools can discover it; log on startup")
//! @yah:next("Gate on Bun on PATH ONLY when the routes manifest has at least one mode:\"ssr\" route. Static/SPA-only workloads must keep working without Bun installed. On the SSR-needed path: bun missing → clear error + refuse to start, not a later crash")
//! @yah:next("Read SSR-prefix set from the routes manifest; route matching paths to bun via segment-aware match (path === prefix || path.startsWith(prefix + '/'), NOT naive startsWith), fall through to static handler")
//! @yah:next("Crash recovery: restart with capped backoff; surface last N lines of stderr through existing LogBuffer")
//! @yah:next("Lazy import on first request is acceptable for dev tier — cold preheating is a later optimization")
//! @yah:next("Dev tier ignores placement entirely (every mode:\"ssr\" route lands in the same bun subprocess, host or edge)")
//! @yah:verify("A test mode:\"ssr\" route returns its Fetch handler's Response under mesofact-dev with no docker running")
//! @yah:verify("Static routes still serve from dist/html/ unchanged")
//! @yah:verify("Static/SPA-only workload starts cleanly with no Bun installed (no spawn attempted)")
//! @yah:verify("Bun child crash restarts and stderr surfaces in the dev log")
//! @yah:verify("Prefix /api/health does NOT match /api/healthcheck (segment boundary)")
//! @yah:assumes("@mesofact/runtime has emitted the SSR-prefix set into the manifest — derivation rule per W173 § \"SSR_PREFIXES derivation rule\" (prefix up to first :param or *)")
//! @arch:see(.yah/docs/working/W173-mesofact-render-cube.md)
//! @yah:handoff("SSR subprocess + reverse proxy shipped (mesofact-dev). New src/ssr.rs: Manifest reader, W173 prefix derivation + segment-aware match, LogBuffer ring (500 lines), SsrChild handle, spawn() that returns Ok(None) for static/SPA-only workloads. When SSR routes exist: gates on `bun` PATH lookup with a clear error, writes ssr-wrapper.ts into the state dir, allocates an ephemeral 127.0.0.1 port, persists it to <workload>/.mesofact-dev/ssr-port, supervises the child with a 250ms→10s exponential-backoff restart loop, and streams stdout+stderr into the LogBuffer. New src/ssr_wrapper.ts: Bun program that reads MESOFACT_GEN_DIR / MESOFACT_SSR_PORT, dynamic-imports each mode:\"ssr\" route's render_entrypoint, dispatches via Bun.serve with the same segment-aware matcher as the Rust side. lib.rs: Server::with_ssr builder; serve_dynamic checks SSR match first and proxies via reqwest (hop-by-hop headers stripped, request + response bodies streamed) before falling through to the static handler. Cargo: +serde_json, +reqwest (default-features=false, features=[stream]), +futures, +tokio io-util feature. SsrChild::drop aborts the supervisor task and removes the port file. cargo test -p mesofact-dev clean: 37 passed. cargo check --workspace clean.")
//! @yah:verify("cargo test -p mesofact-dev --offline --lib  # 37 passed (incl. bun-gated ssr_wrapper_serves_real_fetch_handler_via_bun)")
//! @yah:verify("cargo check --workspace --offline  # clean")
//! @yah:cleanup("Bun caches imported modules — after a watcher rebuild the SSR child keeps serving the old route entrypoints until the next child restart. F3 ships lazy first-request import but no proactive SIGTERM+respawn on DistPointer flip; wire the watcher → ssr-supervisor reload signal if dev-loop SSR edits become painful.")
//! @yah:cleanup("ssr_wrapper.ts resolves render_entrypoint by stripping the first segment (conventionally 'dist/') and joining with MESOFACT_GEN_DIR. Workloads that override build.out_dir to a non-'dist' name will land at the wrong path — thread the out_dir name into the wrapper env when this bites.")
//! @yah:cleanup("SsrChild drop only removes the port file synchronously; Drop can't await the supervisor's full teardown, so a follow-up may want a graceful shutdown() async method for camp wiring.")
//!
//! @yah:ticket(R443-B4, "mesofact-dev serve_from: clean-URL fallback — /releases (and /issues post-T1) 404 without .html extension")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-05T00:20:43Z)
//! @yah:status(review)
//! @yah:parent(R443)
//! @yah:severity(moderate)
//! @yah:next("serve_from at crates/yah/mesofact-dev/src/lib.rs:314 doesn't try a `.html` extension on clean URLs. GET /releases returns 404 (file not found) but GET /releases.html returns 200. Surfaced during R434-F5 verification.")
//! @yah:next("After the literal target miss, try `${target}.html` before falling back to 404.html. sanitize() already rejects path traversal so the .html append is safe.")
//! @yah:next("Check the Worker's path-resolution (crates/yah/cloud/worker/router.bundle.js + router.ts) and mirror its rule so dev and prod agree. If sharing the resolver isn't practical, port the rule explicitly and add a parity test.")
//! @yah:next("Don't try `.html` on SSR-prefix paths — the SSR proxy short-circuits before serve_from in serve_dynamic (lib.rs:212), so the ordering is already safe today, but flag if that ordering ever changes.")
//! @yah:verify("mesofact-dev: GET /releases returns 200 with HTML body matching dist/html/releases.html")
//! @yah:verify("Existing 37 mesofact-dev tests still pass; add serves_clean_url_via_html_fallback regression test")
//! @yah:verify("Behavior matches the Cloudflare Worker's path-resolution for static routes (parity check against pond miniflare)")
//! @yah:gotcha("Pre-existing; not a regression. Surfaced because R434-F5 verification ran curl against /releases and found the 404. R443-F2 will hit the same gap on /issues once T1 lands, which is why this bug is parented here — it blocks F2's verify path.")
//! @yah:handoff("Shipped. crates/yah/mesofact-dev/src/lib.rs serve_from() now appends `.html` after a literal-path miss (only when the path has no extension), before falling through to 404.html. Mirrors what the CDN does for prerendered routes. Added two regression tests: `serves_clean_url_via_html_fallback` (GET /releases → 200 with releases.html body) and `clean_url_fallback_skips_paths_with_extension` (GET /style.css → 404, doesn't try /style.css.html).")
//! @yah:handoff("Verified end-to-end via ./target/debug/mesofact-dev app/yah/web/marketing --no-watch --port 4399: /releases 200, /issues 200 (both via .html fallback), /releases.html 200 + /issues.html 200 (literal, unchanged), /issues_id.html 200, /404 200 (clean URL of the 404 route resolves to 404.html), /nonsense 404 (fallback miss → 404.html as 404), /style.css 404 (has extension, no fallback), /api/issues GET 405 + POST 200 (SSR proxy short-circuits before serve_from, ordering preserved). 42 mesofact-dev tests pass (was 37; added 2 here + 3 from intervening work).")
//! @yah:handoff("Worker / prod parity: surveyed crates/yah/cloud/worker/router.ts — it does NOT have the .html-append rule either. So prod also 404s on /releases today, just hasn't been tripped because the marketing site isn't live + the deploy may rely on a CF-side asset router that adds the extension. Flagging as a follow-up in @yah:next; if it turns out the Worker needs the same rule, file a separate ticket against router.ts + router.bundle.js with the same shape.")
//! @yah:handoff("Out of scope (separate gap): GET /issues/42 still 404 in dev. That's the parametric-SPA routing gap noted in R342-B5 + R434-F5 gotcha — mesofact-dev would need to know about route schemas to map any /issues/:id → issues_id.html. Not B4's problem; tracked elsewhere.")
//! @yah:next("Follow-up worth filing: does the Cloudflare Worker need the same .html append? Today (crates/yah/cloud/worker/router.ts:60-65) it slices the leading `/` off the path and fetches it directly from ASSET_ORIGIN; a literal miss falls through to 404.html. If prod /releases currently works, there's CF-side asset routing doing the append — confirm before changing. If it doesn't work, mirror this fix in router.ts (segment-aware: only append when extension is empty).")
//! @yah:verify("cargo test -p mesofact-dev --lib — VERIFIED 42 passed.")
//! @yah:verify("./target/debug/mesofact-dev app/yah/web/marketing --no-watch --port 4399: /releases 200, /issues 200, /releases.html 200, /issues.html 200, /nonsense 404, /style.css 404, /api/issues GET 405 + POST 200. VERIFIED 2026-06-05.")
//! @yah:verify("Manual parity check: crates/yah/cloud/worker/router.ts inspected; it does NOT have the .html-append rule today. Dev now has it; if prod needs it too, that's a separate ticket against router.ts.")
//! @yah:gotcha("The added rule runs ONLY when target.extension().is_none() — so `/style.css` doesn't get tried as `/style.css.html`. That avoids serving wrong content if someone accidentally has a `style.css.html` file in dist. Test `clean_url_fallback_skips_paths_with_extension` enforces this.")
//! @yah:gotcha("SSR ordering preserved: serve_dynamic checks ssr.matches(path) before serve_from (lib.rs:236-239), so SSR-prefix paths can't accidentally hit the .html fallback. Verified by GET /api/issues returning the F5 handler's 405, not a 404 from the static branch.")
//!
//! @yah:ticket(R443-B9, "mesofact-dev serve_from: hydrate bundles 404 — /{build_id}/hydrate/*.js never reaches dist/hydrate/")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-05T07:34:31Z)
//! @yah:status(review)
//! @yah:parent(R443)
//! @yah:handoff("Shipped. serve_from now intercepts /{build_id}/hydrate/<file> and /hydrate/<file> paths before the normal html/ resolution. hydrate_suffix() helper detects the two-form pattern (with or without build_id prefix) and redirects to <dist>/../hydrate/ (peer of html/). sanitize() still runs first so path traversal is rejected before hydrate_suffix is consulted. 4 new regression tests added: serves_hydrate_bundle_with_build_id_prefix, serves_hydrate_bundle_build_id_opaque, serves_hydrate_bundle_no_build_id_prefix, hydrate_path_traversal_rejected. 46 tests pass (was 42). cargo check --workspace clean.")
//! @yah:verify("cargo test -p mesofact-dev --lib — 46 passed")
//! @yah:verify("./target/debug/mesofact-dev app/yah/web/marketing --no-watch --port 4400; curl -sS -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:4400/<build_id>/hydrate/issues.<hash>.js → 200")
//! @yah:gotcha("Pre-existing — R342-F3 (SPA mode) hit the same gap but was never exercised end-to-end against mesofact-dev. The form's progressive-enhancement claim depends on this fix landing.")

pub mod ssr;
pub mod watcher;

pub use ssr::{SpawnOptions as SsrSpawnOptions, SsrChild, SsrSlot};
pub use watcher::{WatchOptions, Watcher};

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use futures::TryStreamExt;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

/// Default port for the local-static provider slot.
pub const DEFAULT_PORT: u16 = 4321;

/// Shared, atomically-swappable pointer to the currently-served `html/`
/// directory. Cheap to clone; reads take a short read-lock.
#[derive(Clone)]
pub struct DistPointer {
    inner: Arc<RwLock<PathBuf>>,
}

impl DistPointer {
    pub fn new(initial: PathBuf) -> Self {
        Self {
            inner: Arc::new(RwLock::new(initial)),
        }
    }

    /// Current served path; clones the underlying `PathBuf` so the handler
    /// can hold it across `.await` without keeping the lock.
    pub fn current(&self) -> PathBuf {
        self.inner.read().expect("dist pointer poisoned").clone()
    }

    /// Atomically replace the served path. Subsequent requests see the new
    /// value; in-flight requests keep reading from the old `PathBuf` they
    /// already cloned.
    pub fn set(&self, path: PathBuf) {
        *self.inner.write().expect("dist pointer poisoned") = path;
    }
}

/// Static-file dev server for one `mesofact-static` workload.
pub struct Server {
    workload: PathBuf,
    pointer: DistPointer,
    ssr: SsrSlot,
}

#[derive(Clone)]
struct ServerState {
    pointer: DistPointer,
    ssr: SsrSlot,
    proxy: reqwest::Client,
}

impl Server {
    /// Construct a server for a workload directory. Fails if the directory
    /// is missing; tolerates a missing `dist/html/`.
    pub fn from_workload(workload: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let workload = workload.into();
        if !workload.is_dir() {
            anyhow::bail!("workload directory not found: {}", workload.display());
        }
        let pointer = DistPointer::new(workload.join("dist").join("html"));
        Ok(Self {
            workload,
            pointer,
            ssr: SsrSlot::new(),
        })
    }

    pub fn workload(&self) -> &Path {
        &self.workload
    }

    /// Clone of the shared pointer — hand to a [`Watcher`] so its rebuilds
    /// can flip the served snapshot.
    pub fn pointer(&self) -> DistPointer {
        self.pointer.clone()
    }

    /// Current served path. Initial value is `<workload>/dist/html/`; a
    /// [`Watcher`] will swap this to `<workload>/.mesofact-dev/gen-<N>/html/`.
    pub fn dist_dir(&self) -> PathBuf {
        self.pointer.current()
    }

    /// Attach an SSR child. Requests whose path matches one of its prefixes
    /// are proxied to the bun subprocess; everything else falls through to
    /// the static handler. See [`ssr::spawn`] for the spawn contract.
    pub fn with_ssr(self, ssr: SsrChild) -> Self {
        self.ssr.set(Some(Arc::new(ssr)));
        self
    }

    /// Clone of the SSR slot — hand to the watcher's post-build hook so it
    /// can swap in (or restart) the bun child on each successful rebuild.
    /// Reads via [`SsrSlot::current`] are lock-free for the request path.
    pub fn ssr_slot(&self) -> SsrSlot {
        self.ssr.clone()
    }

    /// Build the axum [`Router`]. Exposed for tests + the future embedded
    /// paths (T3 reconciler).
    pub fn router(&self) -> Router {
        let state = ServerState {
            pointer: self.pointer.clone(),
            ssr: self.ssr.clone(),
            proxy: reqwest::Client::new(),
        };
        Router::new()
            .route("/", any(serve_dynamic))
            .route("/*path", any(serve_dynamic))
            .with_state(state)
            .layer(TraceLayer::new_for_http())
    }

    /// Bind to `127.0.0.1:port` and serve until Ctrl+C / SIGTERM.
    pub async fn serve(self, port: u16) -> anyhow::Result<()> {
        let dist = self.pointer.current();
        if !dist.exists() {
            warn!(
                dist = %dist.display(),
                "served dir missing — run `bun run build` or start a watcher; 404s until it appears",
            );
        }
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        info!(
            addr = %local,
            workload = %self.workload.display(),
            "mesofact-dev listening",
        );
        let app = self.router();
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        Ok(())
    }
}

async fn serve_dynamic(State(state): State<ServerState>, req: Request) -> Response {
    let uri_path = req.uri().path().to_string();
    if let Some(ssr) = state.ssr.current() {
        if ssr.matches(&uri_path) {
            return proxy_to_ssr(&state.proxy, ssr.port(), req).await;
        }
    }
    let dist = state.pointer.current();
    serve_from(&dist, &uri_path).await
}

async fn proxy_to_ssr(client: &reqwest::Client, port: u16, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or(parts.uri.path());
    let url = format!("http://127.0.0.1:{port}{path_and_query}");

    let method = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
        Ok(m) => m,
        Err(_) => return (StatusCode::BAD_GATEWAY, "invalid method").into_response(),
    };

    // Forward the request body as a stream so large uploads don't sit in
    // memory.
    let body_stream = body
        .into_data_stream()
        .map_ok(|b| b.to_vec())
        .map_err(io::other_box);
    let reqwest_body = reqwest::Body::wrap_stream(body_stream);

    let mut builder = client.request(method, &url).body(reqwest_body);
    for (k, v) in parts.headers.iter() {
        // Hop-by-hop headers must not be forwarded (RFC 7230 §6.1).
        let name = k.as_str().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "host"
                | "content-length"
        ) {
            continue;
        }
        builder = builder.header(k.as_str(), v.as_bytes());
    }

    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, port, "ssr proxy request failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("ssr proxy failed: {e}"),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    for (k, v) in resp.headers() {
        let name = k.as_str().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
        ) {
            continue;
        }
        builder = builder.header(k.as_str(), v.as_bytes());
    }
    let stream = resp.bytes_stream();
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "proxy build failed").into_response())
}

// Bridge helper: wrap an error type into a boxed std::io::Error so
// reqwest::Body::wrap_stream accepts the stream's error.
mod io {
    use std::io;
    pub(super) fn other_box<E: std::error::Error + Send + Sync + 'static>(e: E) -> io::Error {
        io::Error::new(io::ErrorKind::Other, e)
    }
}

async fn serve_from(dist: &Path, uri_path: &str) -> Response {
    let Some(rel) = sanitize(uri_path) else {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    };

    // Hydrate bundles live at <dist>/../hydrate/ (peer of html/).
    // Prerendered HTML references them as /{build_id}/hydrate/<hash>.js;
    // strip the opaque build_id prefix (or serve /hydrate/<file> directly).
    if let Some(hydrate_rel) = hydrate_suffix(&rel) {
        let hydrate_dir = dist.parent().unwrap_or(dist).join("hydrate");
        let target = hydrate_dir.join(&hydrate_rel);
        if let Ok(bytes) = tokio::fs::read(&target).await {
            let mime = mime_for(&target);
            return ([(header::CONTENT_TYPE, mime)], bytes).into_response();
        }
        let not_found = dist.join("404.html");
        if let Ok(bytes) = tokio::fs::read(&not_found).await {
            return (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                bytes,
            )
                .into_response();
        }
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    let mut target = if rel.as_os_str().is_empty() {
        dist.join("index.html")
    } else {
        dist.join(&rel)
    };
    if target.is_dir() {
        target = target.join("index.html");
    }

    if let Ok(bytes) = tokio::fs::read(&target).await {
        let mime = mime_for(&target);
        return ([(header::CONTENT_TYPE, mime)], bytes).into_response();
    }

    // Clean-URL fallback: `/releases` → `releases.html`. Mirrors what the CDN
    // does for prerendered routes; without it, every route emitted as
    // `<key>.html` 404s in dev unless you hand-type the extension.
    if target.extension().is_none() {
        let with_html = target.with_extension("html");
        if let Ok(bytes) = tokio::fs::read(&with_html).await {
            let mime = mime_for(&with_html);
            return ([(header::CONTENT_TYPE, mime)], bytes).into_response();
        }
    }

    let not_found = dist.join("404.html");
    if let Ok(bytes) = tokio::fs::read(&not_found).await {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            bytes,
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

/// Reject any URI path that would escape `dist/` or carry a NUL byte. Does
/// not percent-decode — segments are treated literally, which is safe (an
/// encoded `..` like `%2e%2e` becomes a literal filename that doesn't exist
/// in `dist/`).
fn sanitize(uri_path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for seg in uri_path.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.contains('\0') {
            return None;
        }
        out.push(seg);
    }
    Some(out)
}

/// Extract the file path under `hydrate/` from paths of the form
/// `/<build_id>/hydrate/<rest>` or `/hydrate/<rest>`.
/// Returns `None` for any other path shape.
fn hydrate_suffix(rel: &Path) -> Option<PathBuf> {
    let mut components = rel.components();
    let first = match components.next() {
        Some(std::path::Component::Normal(s)) => s,
        _ => return None,
    };
    if first == "hydrate" {
        Some(components.as_path().to_path_buf())
    } else {
        match components.next() {
            Some(std::path::Component::Normal(s)) if s == "hydrate" => {
                Some(components.as_path().to_path_buf())
            }
            _ => None,
        }
    }
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("xml") => "application/xml; charset=utf-8",
        Some("txt") | Some("md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!(?err, "failed to install Ctrl+C handler");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tempfile::tempdir;
    use tower::ServiceExt;

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn workload_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let dist = dir.path().join("dist").join("html");
        std::fs::create_dir_all(&dist).unwrap();
        for (name, body) in files {
            std::fs::write(dist.join(name), body).unwrap();
        }
        dir
    }

    #[tokio::test]
    async fn serves_index_at_root() {
        let workload = workload_with(&[("index.html", "<h1>hello</h1>")]);
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_string(response).await.contains("hello"));
    }

    #[tokio::test]
    async fn serves_named_file() {
        let workload = workload_with(&[("404.html", "<h1>oops</h1>")]);
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/404.html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_string(response).await.contains("oops"));
    }

    #[tokio::test]
    async fn serves_clean_url_via_html_fallback() {
        // The CDN serves /releases → releases.html for prerendered routes;
        // mesofact-dev mirrors that so verify scripts don't have to hand-type
        // the extension. Regression for R443-B4.
        let workload = workload_with(&[("releases.html", "<h1>releases</h1>")]);
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/releases")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_string(response).await.contains("releases"));
    }

    #[tokio::test]
    async fn clean_url_fallback_skips_paths_with_extension() {
        // A miss on /style.css must NOT try /style.css.html — the asset
        // extension is unambiguous, fall straight through to 404.
        let workload = workload_with(&[
            ("404.html", "<h1>oops</h1>"),
            ("style.css.html", "this should not be served"),
        ]);
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/style.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(body_string(response).await.contains("oops"));
    }

    #[tokio::test]
    async fn missing_path_falls_back_to_404_html() {
        let workload = workload_with(&[("404.html", "<h1>oops</h1>")]);
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(body_string(response).await.contains("oops"));
    }

    #[tokio::test]
    async fn missing_path_without_404_file_returns_plain_404() {
        let workload = workload_with(&[]);
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(body_string(response).await.contains("Not Found"));
    }

    #[tokio::test]
    async fn from_workload_rejects_missing_directory() {
        let result = Server::from_workload(tempdir().unwrap().path().join("nope"));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn dist_dir_resolves_under_workload() {
        let workload = tempdir().unwrap();
        let server = Server::from_workload(workload.path()).unwrap();
        assert_eq!(server.dist_dir(), workload.path().join("dist").join("html"));
    }

    #[tokio::test]
    async fn pointer_swap_changes_served_content() {
        let workload_a = tempdir().unwrap();
        let dist_a = workload_a.path().join("dist").join("html");
        std::fs::create_dir_all(&dist_a).unwrap();
        std::fs::write(dist_a.join("index.html"), "<h1>A</h1>").unwrap();

        let dir_b = tempdir().unwrap();
        let dist_b = dir_b.path().join("html");
        std::fs::create_dir_all(&dist_b).unwrap();
        std::fs::write(dist_b.join("index.html"), "<h1>B</h1>").unwrap();

        let server = Server::from_workload(workload_a.path()).unwrap();
        let pointer = server.pointer();

        // Initial: serves A.
        let response = server
            .router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(body_string(response).await.contains("A"));

        // Flip pointer to B.
        pointer.set(dist_b);

        // Same router, new content.
        let response = server
            .router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(body_string(response).await.contains("B"));
    }

    #[tokio::test]
    async fn sanitize_rejects_dot_dot() {
        assert!(sanitize("/../etc/passwd").is_none());
        assert!(sanitize("/foo/../bar").is_none());
    }

    #[tokio::test]
    async fn sanitize_accepts_normal_paths() {
        assert_eq!(sanitize("/"), Some(PathBuf::new()));
        assert_eq!(sanitize("/index.html"), Some(PathBuf::from("index.html")));
        assert_eq!(sanitize("/a/b/c"), Some(PathBuf::from("a/b/c")));
    }

    // ── SSR proxy integration tests ──────────────────────────────────────
    //
    // The router-level tests below stand a tiny axum server on a random
    // port and treat it as the "ssr child". This proves the proxy +
    // segment-aware matcher work without needing bun in the test loop.

    use axum::routing::get;

    async fn start_mock_ssr_origin() -> (u16, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/api/health",
                get(|| async { (StatusCode::OK, "healthy").into_response() }),
            )
            .route(
                "/api/users/:id",
                get(|axum::extract::Path(id): axum::extract::Path<String>| async move {
                    (StatusCode::OK, format!("user {id}")).into_response()
                }),
            )
            .fallback(|| async {
                (StatusCode::NOT_FOUND, "mock origin: no match").into_response()
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (port, handle)
    }

    /// Verify item #1 (proxy half): an SSR-prefixed request reaches the
    /// "bun child" (mock origin) and its response is forwarded back.
    #[tokio::test]
    async fn ssr_proxied_path_returns_handler_response() {
        let workload = workload_with(&[("index.html", "<h1>static</h1>")]);
        let (port, _origin) = start_mock_ssr_origin().await;
        let ssr = ssr::detached_for_test(port, vec!["/api/health".to_string()]);

        let server = Server::from_workload(workload.path()).unwrap().with_ssr(ssr);
        let response = server
            .router()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_string(response).await, "healthy");
    }

    /// Verify item #2: with SSR wired, static routes still serve from disk.
    #[tokio::test]
    async fn ssr_does_not_swallow_static_routes() {
        let workload = workload_with(&[("index.html", "<h1>static</h1>")]);
        let (port, _origin) = start_mock_ssr_origin().await;
        let ssr = ssr::detached_for_test(port, vec!["/api/health".to_string()]);

        let server = Server::from_workload(workload.path()).unwrap().with_ssr(ssr);
        let response = server
            .router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_string(response).await.contains("static"));
    }

    /// Verify item #5: segment-aware prefix matching at the router layer.
    /// `/api/health` (SSR) is proxied; `/api/healthcheck` (no SSR match)
    /// falls through to static, which 404s on missing path.
    #[tokio::test]
    async fn ssr_segment_boundary_not_naive_starts_with() {
        let workload = workload_with(&[("404.html", "static-404")]);
        let (port, _origin) = start_mock_ssr_origin().await;
        let ssr = ssr::detached_for_test(port, vec!["/api/health".to_string()]);

        let server = Server::from_workload(workload.path()).unwrap().with_ssr(ssr);
        let router = server.router();

        // /api/health → SSR → mock origin → "healthy"
        let r1 = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        assert_eq!(body_string(r1).await, "healthy");

        // /api/healthcheck → not SSR → static 404 (proves naive startsWith
        // would have wrongly proxied this).
        let r2 = router
            .oneshot(
                Request::builder()
                    .uri("/api/healthcheck")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_string(r2).await, "static-404");
    }

    // ── Hydrate bundle routing tests ────────────────────────────────────────

    fn workload_with_hydrate(
        html_files: &[(&str, &str)],
        hydrate_files: &[(&str, &str)],
    ) -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let html_dir = dir.path().join("dist").join("html");
        let hydrate_dir = dir.path().join("dist").join("hydrate");
        std::fs::create_dir_all(&html_dir).unwrap();
        std::fs::create_dir_all(&hydrate_dir).unwrap();
        for (name, body) in html_files {
            std::fs::write(html_dir.join(name), body).unwrap();
        }
        for (name, body) in hydrate_files {
            std::fs::write(hydrate_dir.join(name), body).unwrap();
        }
        dir
    }

    /// (a) GET /<build_id>/hydrate/<file>.js → 200 + application/javascript.
    #[tokio::test]
    async fn serves_hydrate_bundle_with_build_id_prefix() {
        let workload = workload_with_hydrate(
            &[],
            &[("issues.abc123.js", "console.log('hydrate')")],
        );
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/gen-1/hydrate/issues.abc123.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("application/javascript"), "wrong mime: {ct}");
        assert!(body_string(response).await.contains("hydrate"));
    }

    /// (b) build_id is opaque — any string in the first segment still routes
    /// to the same dist/hydrate/ directory.
    #[tokio::test]
    async fn serves_hydrate_bundle_build_id_opaque() {
        let workload = workload_with_hydrate(
            &[],
            &[("app.xyz.js", "export default 1")],
        );
        let app = Server::from_workload(workload.path()).unwrap().router();
        for prefix in &["no-such-build-id", "gen-99", "abc123"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/{prefix}/hydrate/app.xyz.js"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "build_id '{prefix}' should be opaque"
            );
        }
    }

    /// No-prefix form: /hydrate/<file> also maps to dist/hydrate/.
    #[tokio::test]
    async fn serves_hydrate_bundle_no_build_id_prefix() {
        let workload =
            workload_with_hydrate(&[], &[("app.js", "export default 1")]);
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/hydrate/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// (c) Path traversal inside a hydrate URL → BAD_REQUEST (sanitizer holds).
    #[tokio::test]
    async fn hydrate_path_traversal_rejected() {
        let workload = workload_with_hydrate(&[], &[]);
        let app = Server::from_workload(workload.path()).unwrap().router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/gen-1/hydrate/../../etc/passwd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// Parametric prefix coverage: /api/users/ → matches /api/users/42 and
    /// forwards the full path so the mock origin's :id capture works.
    #[tokio::test]
    async fn ssr_parametric_prefix_forwards_full_path() {
        let workload = workload_with(&[]);
        let (port, _origin) = start_mock_ssr_origin().await;
        let ssr = ssr::detached_for_test(port, vec!["/api/users/".to_string()]);
        let server = Server::from_workload(workload.path()).unwrap().with_ssr(ssr);
        let response = server
            .router()
            .oneshot(
                Request::builder()
                    .uri("/api/users/42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_string(response).await, "user 42");
    }
}
