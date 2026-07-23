//! In-process SSR dispatch for `mode:"ssr"` routes (R449-F2; supersedes the
//! bun-subprocess implementation R434-F3 shipped).
//!
//! On startup the manifest is read; if any route is `mode:"ssr"` an
//! [`mesofact_ssr::SsrRuntime`] is booted in this process. Each route's
//! `render_entrypoint` is pre-loaded into the isolate (paid once at startup),
//! keyed by its derived URL prefix. The dev server then calls
//! [`SsrChild::dispatch`] for each matching request — one V8 turn, no
//! cross-process hop, no HTTP serialisation.
//!
//! The bun subprocess + ssr-wrapper.ts + reverse-proxy machinery is gone. So
//! is the requirement for `bun` on `PATH`; the dev server now works on a
//! plain Rust toolchain.
//!
//! @yah:relay(R444, "Plumb dev S3 coords into in-process SSR isolate so R2Adapter resolves at runtime in dev hotreload")
//! @yah:at(2026-06-20T20:37:04Z)
//! @yah:status(open)
//! @yah:next("Thread the dev S3 coords from mesofact-dev's main/ssr::spawn into SsrRuntime::start (crates/mesofact-dev/src/ssr.rs:380 + the SpawnOptions struct) and on into the mesofact-ssr isolate bootstrap.")
//! @yah:next("Expose them to JS inside the isolate so @mesofact/runtime config.ts requireEnv(env, ...) resolves: either inject a process.env shim (globalThis.process = { env: {...} }) in the bootstrap, or pass an explicit env map the runtime's registerSourcesFromConfig consumes. Decide which the runtime should read (process.env shim is least-invasive to existing TS).")
//! @yah:next("Verify end-to-end (this completes R490-F7's PENDING criterion): a mesofact dev app with a [sources.r2] source doing r2.fetch/list inside an SSR render handler resolves against mesofact-dev's s3s-fs in `bun run dev` and returns the bytes (PUT one, fetch it back through the rendered route).")
//! @yah:next("Coordinate the env-var-name convention with R490-F7: today mesofact-dev injects conventional R2_* names; keep the isolate shim consistent (or have mesofact-dev read the workload's mesofact.config.toml to learn the declared endpoint_env names).")
//! @yah:gotcha("Cross-camp seam: this is the runtime-reads half of the PARENT camp's R490-F7 (in the yah camp at /Users/leif/ss/yah). R490-F7 landed the dev S3 surface (s3s-fs in mesofact-dev) + BUILD-TIME r2 reads via the build subprocess env. The blocker for RUNTIME reads is here in the subcamp: the in-process V8 SSR runtime (SsrRuntime, R449-F2) can't inherit process.env, so the @mesofact/runtime R2Adapter executing inside SSR render code never sees R2_ENDPOINT.")
//! @yah:gotcha("mesofact-dev already computes the coords (DevS3::env_vars(): R2_ENDPOINT/R2_BUCKET/R2_ACCESS_KEY_ID/R2_SECRET_ACCESS_KEY) and writes .mesofact-dev/s3.json. The missing piece is getting those into the isolate's JS env so registerSourcesFromConfig() can resolve [sources.r2].")

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use mesofact_ssr::{DispatchRequest, DispatchResponse, SsrRuntime};
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Parsed `manifest.json` slice. Only the fields the SSR path cares about.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub routes: Vec<RouteEntry>,
    #[serde(default)]
    pub ssr_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteEntry {
    pub route: String,
    pub mode: String,
    #[serde(default)]
    pub render_entrypoint: Option<String>,
    /// W181 resilience block (retry + timeout). Only `mode:"ssr"` routes
    /// carry it; defineRoutes rejects it on other modes upstream.
    #[serde(default)]
    pub resilience: Option<ResiliencePolicy>,
}

/// W181 v1 — schema mirror of `mesofact_core::manifest::ResiliencePolicy`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResiliencePolicy {
    #[serde(default)]
    pub retry: Option<RetryPolicy>,
    /// Queue is reserved for v2; rejected upstream in `defineRoutes`, but
    /// we accept the field shape so v2 manifests deserialize cleanly.
    #[serde(default)]
    pub queue: Option<serde_json::Value>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub backoff_ms: Vec<u64>,
    #[serde(default)]
    pub retry_on: Option<String>,
    #[serde(default)]
    pub budget_ms: Option<u64>,
}

/// Default per-attempt request timeout when `resilience.timeout_ms` is unset.
pub const DEFAULT_RESILIENCE_TIMEOUT_MS: u64 = 30_000;

impl Manifest {
    /// Read `<gen_dir>/manifest.json`. Returns `Ok(None)` when the file is
    /// absent (pre-build or non-mesofact workload); other I/O errors bubble.
    pub fn read(gen_dir: &Path) -> Result<Option<Self>> {
        let path = gen_dir.join("manifest.json");
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                let m: Self = serde_json::from_str(&s)
                    .with_context(|| format!("parsing {}", path.display()))?;
                Ok(Some(m))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn has_ssr(&self) -> bool {
        self.routes.iter().any(|r| r.mode == "ssr")
    }

    /// W181 — `(derived_prefix, policy)` pairs for every SSR route that
    /// declared a `resilience` block.
    pub fn resilience_policies(&self) -> Vec<(String, ResiliencePolicy)> {
        self.routes
            .iter()
            .filter(|r| r.mode == "ssr")
            .filter_map(|r| r.resilience.clone().map(|p| (derive_prefix(&r.route), p)))
            .collect()
    }

    /// SSR-prefix set per W173.
    pub fn ssr_prefixes(&self) -> Vec<String> {
        if !self.ssr_prefixes.is_empty() {
            let mut p = self.ssr_prefixes.clone();
            p.sort();
            p.dedup();
            return p;
        }
        let mut p: Vec<String> = self
            .routes
            .iter()
            .filter(|r| r.mode == "ssr")
            .map(|r| derive_prefix(&r.route))
            .collect();
        p.sort();
        p.dedup();
        p
    }

    /// `(derived_prefix, render_entrypoint)` pairs for every SSR route that
    /// declared an entrypoint. Used to pre-load the SsrRuntime.
    pub fn ssr_entrypoints(&self) -> Vec<(String, String)> {
        self.routes
            .iter()
            .filter(|r| r.mode == "ssr")
            .filter_map(|r| {
                r.render_entrypoint
                    .clone()
                    .map(|ep| (derive_prefix(&r.route), ep))
            })
            .collect()
    }
}

/// W173 derivation: prefix is everything up to the first `:param` or `*`
/// segment. Non-parametric SSR routes use the full path.
pub fn derive_prefix(route: &str) -> String {
    let mut out = String::new();
    for seg in route.split('/') {
        if seg.is_empty() {
            continue;
        }
        if seg.starts_with(':') || seg.starts_with('*') {
            if !out.ends_with('/') {
                out.push('/');
            }
            return out;
        }
        out.push('/');
        out.push_str(seg);
    }
    out
}

/// W173 segment-aware match: `path == prefix || path.startsWith(prefix + "/")`.
pub fn matches_prefix(path: &str, prefix: &str) -> bool {
    if path == prefix {
        return true;
    }
    if prefix.ends_with('/') {
        return path.starts_with(prefix);
    }
    let mut needle = String::with_capacity(prefix.len() + 1);
    needle.push_str(prefix);
    needle.push('/');
    path.starts_with(&needle)
}

const LOG_CAP: usize = 500;

/// Bounded ring buffer for SSR runtime log lines. Kept around so the dev log
/// surface that expected stderr lines from the bun child still has something
/// to render — though the in-process runtime writes far fewer lines.
#[derive(Debug, Clone, Default)]
pub struct LogBuffer(Arc<Mutex<LogRing>>);

#[derive(Debug, Default)]
struct LogRing {
    lines: VecDeque<String>,
    total: usize,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn push(&self, line: String) {
        let mut ring = self.0.lock().await;
        ring.total += 1;
        ring.lines.push_back(line);
        if ring.lines.len() > LOG_CAP {
            ring.lines.pop_front();
        }
    }

    /// Snapshot the current ring contents.
    pub async fn lines(&self) -> Vec<String> {
        let ring = self.0.lock().await;
        ring.lines.iter().cloned().collect()
    }

    /// Incremental tail. `since` is the cursor from the previous call (0 =
    /// nothing seen yet). Returns `(new_lines, new_cursor)`.
    pub async fn since(&self, since: usize) -> (Vec<String>, usize) {
        let ring = self.0.lock().await;
        let oldest = ring.total.saturating_sub(ring.lines.len());
        let skip = since.saturating_sub(oldest);
        let new_lines: Vec<String> = ring.lines.iter().skip(skip).cloned().collect();
        (new_lines, ring.total)
    }
}

/// Dispatch target: production carries an `SsrRuntime`; tests can inject a
/// closure to model failures/successes without booting V8.
enum DispatchTarget {
    Runtime {
        runtime: SsrRuntime,
        /// derived_prefix → absolute path of the registered render_entrypoint.
        /// `dispatch` does longest-prefix lookup here to pick the bundle.
        bundles: HashMap<String, PathBuf>,
    },
    #[cfg(test)]
    Mock(Box<dyn Fn(DispatchRequest) -> Result<DispatchResponse> + Send + Sync>),
}

impl DispatchTarget {
    async fn dispatch(&self, path: &str, req: DispatchRequest) -> Result<DispatchResponse> {
        match self {
            DispatchTarget::Runtime { runtime, bundles } => {
                let bundle = longest_prefix_match(bundles, path)
                    .ok_or_else(|| anyhow::anyhow!("no SSR bundle registered for {path}"))?;
                // SsrRuntime::dispatch is blocking; isolate-thread message
                // round-trip is fast (no I/O) so spawn_blocking would just add
                // overhead. Call it directly.
                runtime.dispatch(&bundle, req)
            }
            #[cfg(test)]
            DispatchTarget::Mock(f) => f(req),
        }
    }
}

fn longest_prefix_match(map: &HashMap<String, PathBuf>, path: &str) -> Option<PathBuf> {
    let mut best: Option<(&String, &PathBuf)> = None;
    for (prefix, bundle) in map.iter() {
        if !matches_prefix(path, prefix) {
            continue;
        }
        match best {
            Some((p, _)) if p.len() >= prefix.len() => {}
            _ => best = Some((prefix, bundle)),
        }
    }
    best.map(|(_, b)| b.clone())
}

/// Swappable holder for the SSR child. The router reads `current()` on every
/// request; the watcher's post-build hook installs (or rotates) the child
/// after each successful gen flip. Cheap to clone.
#[derive(Clone, Default)]
pub struct SsrSlot {
    inner: Arc<RwLock<Option<Arc<SsrChild>>>>,
}

impl SsrSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<Arc<SsrChild>> {
        self.inner.read().ok().and_then(|g| g.clone())
    }

    pub fn set(&self, child: Option<Arc<SsrChild>>) {
        if let Ok(mut w) = self.inner.write() {
            *w = child;
        }
    }
}

/// In-process SSR dispatch + the data the router needs to use it.
pub struct SsrChild {
    target: DispatchTarget,
    /// W173 prefix set for the router's `ssr.matches(path)` gate. Refreshed
    /// by [`SsrChild::restart_with`] on gen flip.
    prefixes: Arc<RwLock<Vec<String>>>,
    /// W181 — per-route resilience block, keyed by derived prefix.
    policies: Arc<RwLock<Vec<(String, ResiliencePolicy)>>>,
    log_buffer: LogBuffer,
}

impl SsrChild {
    pub fn prefixes(&self) -> Vec<String> {
        self.prefixes.read().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn log_buffer(&self) -> LogBuffer {
        self.log_buffer.clone()
    }

    /// True when the request path matches any SSR prefix.
    pub fn matches(&self, path: &str) -> bool {
        let guard = match self.prefixes.read() {
            Ok(g) => g,
            Err(_) => return false,
        };
        guard.iter().any(|p| matches_prefix(path, p))
    }

    /// Resolve the resilience policy for `path` by longest-prefix match.
    pub fn policy_for(&self, path: &str) -> Option<ResiliencePolicy> {
        let guard = self.policies.read().ok()?;
        let mut best: Option<(&String, &ResiliencePolicy)> = None;
        for (prefix, policy) in guard.iter() {
            if !matches_prefix(path, prefix) {
                continue;
            }
            match best {
                Some((p, _)) if p.len() >= prefix.len() => {}
                _ => best = Some((prefix, policy)),
            }
        }
        best.map(|(_, p)| p.clone())
    }

    /// Dispatch `req` to the registered SSR handler whose derived prefix
    /// matches `path`. Returns the handler's Response.
    pub async fn dispatch(&self, path: &str, req: DispatchRequest) -> Result<DispatchResponse> {
        self.target.dispatch(path, req).await
    }

    /// Re-read the manifest from a new gen dir, tear down the old SsrRuntime,
    /// and boot a fresh one against the new bundles. Mirrors the gen-flip
    /// semantics R434-B6 added — the old isolate's module cache would serve
    /// stale routes across rebuilds, so the only honest answer is a restart.
    pub async fn restart_with(&self, _gen_dir: PathBuf) -> Result<()> {
        // The in-process model needs to swap the runtime, but `self.target`
        // is owned by `SsrChild`. Production callers swap the whole
        // `Arc<SsrChild>` in `SsrSlot` instead — main.rs's post-build hook
        // calls [`spawn`] and `slot.set(Some(...))` to rotate.
        //
        // This method is kept as a compatibility no-op for callers that used
        // to invoke it during the bun era; the slot-swap path is now the
        // sole way to install a fresh module graph. Returning Ok keeps the
        // watcher's post-build chain unbroken.
        warn!(
            "SsrChild::restart_with is a no-op under the in-process model; \
             the post-build hook should call ssr::spawn + slot.set instead",
        );
        Ok(())
    }
}

/// Options for [`spawn`]. The workload directory anchors the state dir; the
/// gen_dir is the snapshot the SSR runtime should resolve entrypoints
/// against.
pub struct SpawnOptions {
    pub workload: PathBuf,
    pub gen_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl SpawnOptions {
    pub fn new(workload: PathBuf, gen_dir: PathBuf, state_dir: PathBuf) -> Self {
        Self {
            workload,
            gen_dir,
            state_dir,
        }
    }
}

/// Inspect the manifest; if it has any SSR route, boot an SsrRuntime, pre-
/// load each route's render_entrypoint, and return a [`SsrChild`] handle.
/// Returns `Ok(None)` when no SSR routes are declared — the caller serves
/// static only.
pub async fn spawn(opts: SpawnOptions) -> Result<Option<SsrChild>> {
    let manifest = match Manifest::read(&opts.gen_dir)? {
        Some(m) => m,
        None => return Ok(None),
    };
    if !manifest.has_ssr() {
        return Ok(None);
    }

    let log_buffer = LogBuffer::new();
    let prefixes = manifest.ssr_prefixes();
    let policies = manifest.resilience_policies();
    let entrypoints = manifest.ssr_entrypoints();

    info!(
        prefixes = ?prefixes,
        entrypoints = entrypoints.len(),
        "mesofact-dev ssr runtime starting",
    );

    // Boot the isolate off the async runtime — start() blocks until the
    // bootstrap evaluates, and SsrRuntime::register is also blocking. Both
    // are CPU-bound (V8 init); spawn_blocking keeps the tokio runtime free.
    let state_dir = opts.state_dir.clone();
    let gen_dir_for_blocking = opts.gen_dir.clone();
    let log = log_buffer.clone();
    let _ = tokio::fs::create_dir(&state_dir).await; // best-effort; ignore EEXIST
    let (runtime, bundles) = tokio::task::spawn_blocking(move || -> Result<_> {
        let runtime = SsrRuntime::start().context("starting SsrRuntime")?;
        let mut bundles: HashMap<String, PathBuf> = HashMap::new();
        for (prefix, rel) in entrypoints {
            let bundle = resolve_entrypoint(&gen_dir_for_blocking, &rel);
            runtime
                .register(&bundle)
                .with_context(|| format!("registering SSR entrypoint {}", bundle.display()))?;
            bundles.insert(prefix, bundle);
        }
        Ok((runtime, bundles))
    })
    .await
    .context("ssr runtime init task panicked")??;

    // Surface readiness to the dev log surface in the same shape the bun
    // child used to (the operator's eyes are tuned to "ssr ... ready").
    log.push(format!(
        "[mesofact-dev/ssr] ready: {} handler(s) in-process",
        bundles.len()
    ))
    .await;

    Ok(Some(SsrChild {
        target: DispatchTarget::Runtime { runtime, bundles },
        prefixes: Arc::new(RwLock::new(prefixes)),
        policies: Arc::new(RwLock::new(policies)),
        log_buffer,
    }))
}

/// Strip the first segment of `render_entrypoint` (conventionally `dist/`)
/// and join with the gen dir, mirroring the previous ssr_wrapper.ts rule.
fn resolve_entrypoint(gen_dir: &Path, rel: &str) -> PathBuf {
    let sub = match rel.find('/') {
        Some(i) => &rel[i + 1..],
        None => rel,
    };
    gen_dir.join(sub)
}

#[cfg(test)]
pub(crate) fn detached_for_test(prefixes: Vec<String>) -> SsrChild {
    detached_for_test_with_policies(prefixes, Vec::new(), |_| {
        Ok(DispatchResponse {
            status: 200,
            headers: vec![],
            body: b"mock".to_vec(),
        })
    })
}

#[cfg(test)]
pub(crate) fn detached_for_test_with_policies(
    prefixes: Vec<String>,
    policies: Vec<(String, ResiliencePolicy)>,
    dispatch_fn: impl Fn(DispatchRequest) -> Result<DispatchResponse> + Send + Sync + 'static,
) -> SsrChild {
    SsrChild {
        target: DispatchTarget::Mock(Box::new(dispatch_fn)),
        prefixes: Arc::new(RwLock::new(prefixes)),
        policies: Arc::new(RwLock::new(policies)),
        log_buffer: LogBuffer::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_prefix_table() {
        assert_eq!(derive_prefix("/api/health"), "/api/health");
        assert_eq!(derive_prefix("/api/users/:id"), "/api/users/");
        assert_eq!(derive_prefix("/x/:a/y"), "/x/");
        assert_eq!(derive_prefix("/feed/*"), "/feed/");
        assert_eq!(derive_prefix("/:id"), "/");
    }

    #[test]
    fn matches_prefix_segment_aware() {
        assert!(matches_prefix("/api/health", "/api/health"));
        assert!(!matches_prefix("/api/healthcheck", "/api/health"));
        assert!(matches_prefix("/api/health/sub", "/api/health"));
        assert!(matches_prefix("/api/users/42", "/api/users/"));
        assert!(!matches_prefix("/api/usersdata", "/api/users/"));
    }

    #[test]
    fn manifest_prefers_pre_derived_prefixes() {
        let m: Manifest = serde_json::from_str(
            r#"{
                "routes": [
                    {"route": "/api/users/:id", "mode": "ssr"},
                    {"route": "/", "mode": "static"}
                ],
                "ssr_prefixes": ["/api/users/", "/api/users/", "/feed/"]
            }"#,
        )
        .unwrap();
        assert_eq!(m.ssr_prefixes(), vec!["/api/users/", "/feed/"]);
    }

    #[test]
    fn manifest_derives_when_no_prefixes_field() {
        let m: Manifest = serde_json::from_str(
            r#"{
                "routes": [
                    {"route": "/api/health", "mode": "ssr"},
                    {"route": "/api/users/:id", "mode": "ssr"},
                    {"route": "/", "mode": "static"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(m.ssr_prefixes(), vec!["/api/health", "/api/users/"]);
    }

    #[test]
    fn manifest_has_ssr_false_for_static_only() {
        let m: Manifest =
            serde_json::from_str(r#"{"routes": [{"route": "/", "mode": "static"}]}"#).unwrap();
        assert!(!m.has_ssr());
        assert!(m.ssr_prefixes().is_empty());
    }

    #[test]
    fn manifest_read_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Manifest::read(dir.path()).unwrap().is_none());
    }

    #[tokio::test]
    async fn log_buffer_ring_drops_oldest() {
        let buf = LogBuffer::new();
        for n in 0..(LOG_CAP + 5) {
            buf.push(format!("line {n}")).await;
        }
        let lines = buf.lines().await;
        assert_eq!(lines.len(), LOG_CAP);
        assert_eq!(lines.first().unwrap(), "line 5");
        assert_eq!(lines.last().unwrap(), &format!("line {}", LOG_CAP + 4));
    }

    #[tokio::test]
    async fn log_buffer_since_returns_incremental_tail() {
        let buf = LogBuffer::new();
        buf.push("a".into()).await;
        buf.push("b".into()).await;
        let (lines, cursor) = buf.since(0).await;
        assert_eq!(lines, vec!["a", "b"]);
        assert_eq!(cursor, 2);
        buf.push("c".into()).await;
        let (lines, cursor) = buf.since(cursor).await;
        assert_eq!(lines, vec!["c"]);
        assert_eq!(cursor, 3);
    }

    #[tokio::test]
    async fn spawn_returns_none_for_static_only_workload() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"routes": [{"route": "/", "mode": "static"}]}"#,
        )
        .unwrap();
        let opts = SpawnOptions::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            dir.path().join(".mesofact-dev"),
        );
        let res = spawn(opts).await.unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn spawn_returns_none_for_missing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let opts = SpawnOptions::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            dir.path().join(".mesofact-dev"),
        );
        assert!(spawn(opts).await.unwrap().is_none());
    }

    /// End-to-end via the real SsrRuntime — lay out a minimal gen dir +
    /// manifest pointing at a fixture render_entrypoint, spawn, dispatch.
    /// Replaces the bun-gated test the prior implementation carried.
    #[tokio::test]
    async fn spawn_dispatches_through_real_ssr_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let server_dir = dir.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(
            server_dir.join("ping.js"),
            "export default async function (_req) {\n\
                return new Response('pong');\n\
              }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "routes": [
                    {"route": "/api/ping", "mode": "ssr", "render_entrypoint": "dist/server/ping.js"}
                ]
            }"#,
        )
        .unwrap();

        let opts = SpawnOptions::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            dir.path().join(".mesofact-dev"),
        );
        let child = spawn(opts).await.unwrap().expect("ssr present");

        let resp = child
            .dispatch(
                "/api/ping",
                DispatchRequest {
                    method: "GET".into(),
                    url: "http://dev/api/ping".into(),
                    headers: vec![],
                    body: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(String::from_utf8(resp.body).unwrap(), "pong");
    }
}
