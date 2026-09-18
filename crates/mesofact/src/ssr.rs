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
//! @yah:status(review)
//! @yah:assignee(agent:bundle-anthropic-miravel)
//! @yah:at(2026-08-13T19:29:07Z)
//! @yah:next("Thread the dev S3 coords from mesofact-dev's main/ssr::spawn into SsrRuntime::start (crates/mesofact-dev/src/ssr.rs:380 + the SpawnOptions struct) and on into the mesofact-ssr isolate bootstrap.")
//! @yah:next("Expose them to JS inside the isolate so @mesofact/runtime config.ts requireEnv(env, ...) resolves: either inject a process.env shim (globalThis.process = { env: {...} }) in the bootstrap, or pass an explicit env map the runtime's registerSourcesFromConfig consumes. Decide which the runtime should read (process.env shim is least-invasive to existing TS).")
//! @yah:next("Verify end-to-end (this completes R490-F7's PENDING criterion): a mesofact dev app with a [sources.r2] source doing r2.fetch/list inside an SSR render handler resolves against mesofact-dev's s3s-fs in `bun run dev` and returns the bytes (PUT one, fetch it back through the rendered route).")
//! @yah:next("Coordinate the env-var-name convention with R490-F7: today mesofact-dev injects conventional R2_* names; keep the isolate shim consistent (or have mesofact-dev read the workload's mesofact.config.toml to learn the declared endpoint_env names).")
//! @yah:gotcha("Cross-camp seam: this is the runtime-reads half of the PARENT camp's R490-F7 (in the yah camp at /Users/leif/ss/yah). R490-F7 landed the dev S3 surface (s3s-fs in mesofact-dev) + BUILD-TIME r2 reads via the build subprocess env. The blocker for RUNTIME reads is here in the subcamp: the in-process V8 SSR runtime (SsrRuntime, R449-F2) can't inherit process.env, so the @mesofact/runtime R2Adapter executing inside SSR render code never sees R2_ENDPOINT.")
//! @yah:gotcha("mesofact-dev already computes the coords (DevS3::env_vars(): R2_ENDPOINT/R2_BUCKET/R2_ACCESS_KEY_ID/R2_SECRET_ACCESS_KEY) and writes .mesofact-dev/s3.json. The missing piece is getting those into the isolate's JS env so registerSourcesFromConfig() can resolve [sources.r2].")
//! @yah:handoff("Dev S3 coords now reach the in-process SSR isolate end-to-end. Rust: SpawnOptions gained env: Vec<(String,String)> + with_env() (crates/mesofact/src/ssr.rs); mesofact-dev's main.rs starts DevS3 BEFORE the first ssr::spawn (was after) and threads dev_s3.env_vars() into both the initial spawn and the post-build respawn hook; mesofact/src/cli/serve.rs (prod) passes std::env::vars() since that process genuinely has yubaba-injected secrets.")
//! @yah:handoff("mesofact::ssr::spawn resolves [sources.r2] from the workload's mesofact.config.toml against opts.env (new resolve_r2_sources, fail-fast on a missing/empty declared env var, same boot-time contract mesofact-worker's runWorker uses) and calls the new SsrRuntime::register_r2_sources before any route registers.")
//! @yah:handoff("mesofact-ssr: SsrRuntime::start(env) Object.assign's env onto globalThis.process.env right after the bootstrap script (before harness/any route loads) via a new mesofact-ssr:env execute_script. New Job::RegisterR2 + call_bridge (a call_harness variant with no bundle-URL keying) push resolved R2SourceCoords into the isolate.")
//! @yah:handoff("ssr_runtime_shim.js's r2()/sqlite() were a HARDCODED THROWING STUB pre-R444 (registerSourcesFromConfig was a no-op) — discovered mid-relay, not just missing env plumbing as the ticket text assumed. Replaced with a real per-isolate registry + a ported R2Adapter (BaseSource noTrack/timeout/tag-emit, S3 GET/LIST-v2, XML parser) using plain unsigned fetch() — works against mesofact-dev's anonymous s3s-fs (AllowAllAccess) but NOT real Cloudflare R2 (needs SigV4). sqlite() stays a throwing stub (out of R444's r2-only scope).")
//! @yah:handoff("DISCOVERED + FIXED (blocking, in-blast-radius): DispatchTarget::dispatch (mesofact/src/ssr.rs) called the blocking SsrRuntime::dispatch round-trip INLINE on the caller's async executor thread ('isolate round-trip is fast, no I/O' — no longer true once route code does real fetch()). On a single-threaded runtime that also hosts the I/O the route awaits (mesofact-dev's own dev-S3 surface in this crate's tests; any single-worker deployment), that's a hard deadlock: the one thread able to service the connection is the one now blocked waiting on it. Fixed by Arc<SsrRuntime> + spawn_blocking. Found via a raw-TCP-mock vs real-s3s-server A/B (mock worked, s3s hung) that isolated it to a same-thread scheduling conflict, not a protocol/TLS issue.")
//! @yah:handoff("Also fixed: deno_fetch's HTTP client needs a process-wide rustls CryptoProvider even for plain http:// (both aws-lc-rs and ring end up in the graph, so rustls can't auto-pick) — first real fetch() panicked without it. Added rustls (aws-lc-rs feature) + a Once-guarded install in SsrRuntime::start.")
//! @yah:handoff("Filed R820 (open) for the SigV4 gap: verified deno_crypto 0.271 hard-pins deno_core=0.410 vs this crate's 0.404 line (cargo check fails on deno_v8's v8/quickjs feature_error) — a whole-extension-set version bump, correctly out of R444's blast radius.")
//! @yah:verify("cargo test --workspace (oss/mesofact) — all green: mesofact 136, mesofact-ssr 3, mesofact-dev 15, plus every other workspace member, 0 failed")
//! @yah:verify("cargo test -p mesofact-dev --features ssr ssr_route_resolves_r2_source_against_dev_s3 — the ticket's exact verify criterion: an SSR route importing r2 from @mesofact/runtime, .fetch()-ing a key PUT into mesofact-dev's real DevS3/s3s-fs surface through SsrSpawnOptions::with_env(dev.env_vars()), dispatched through the real in-process isolate, returns the bytes")
//! @yah:verify("cargo build --workspace and cargo check -p mesofact-dev --no-default-features — clean (ssr feature stays fully optional)")
//! @yah:verify("cargo clippy -p mesofact-ssr -p mesofact --features ssr -p mesofact-dev --features ssr — no new warnings on any touched file (one pre-existing unrelated warning in mesofact-core/proxy/router.rs, not touched here)")
//! @yah:gotcha("Production R2 reads through this isolate are UNSIGNED (see R820) — fine against mesofact-dev's anonymous s3s-fs, will 403 against real Cloudflare R2 until R820 lands the deno_core bump + vendored aws4fetch.")

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use mesofact_ssr::{DispatchRequest, DispatchResponse, R2SourceCoords, SsrPool};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Parsed `manifest.json` slice. Only the fields the SSR path cares about.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub routes: Vec<RouteEntry>,
    #[serde(default)]
    pub ssr_prefixes: Vec<String>,
    /// Declared Mode 2 hooks, name → bundled module (W311 §2 / R756-F6).
    #[serde(default)]
    pub hooks: HashMap<String, HookEntry>,
}

/// One declared Mode 2 hook — schema mirror of
/// `mesofact_core::manifest::Hook`.
#[derive(Debug, Clone, Deserialize)]
pub struct HookEntry {
    pub entrypoint: String,
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

    /// `(hook_name, entrypoint)` pairs for every Mode 2 hook this workload
    /// declares (W311 §2 / R756-F6), from either declaration site.
    ///
    /// Two sites, one registry. The `hooks` block is the general one — a hook
    /// is engine-addressed, so most hooks have no route to claim. `readyz` is
    /// the exception: it shipped (R756-F5) with "claiming the route IS the
    /// opt-in", so an app that declares a `mode:"ssr"` `/readyz` route still
    /// gets the `readyz` hook bound to that route's bundle. Collapsing both
    /// into one name→bundle map here is what lets everything downstream —
    /// `SsrChild::invoke_hook`, `AppReadyCheck`, the JS harness — stop caring
    /// which site a hook came from.
    ///
    /// `hooks` wins if a workload somehow carries both; the two declaration
    /// sites are rejected together at config-evaluation time
    /// (`validate_routes_config`), so this is a defence against a
    /// hand-written manifest, not a supported configuration.
    pub fn hook_entrypoints(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (name, claimed) in HOOK_ROUTE_CLAIMS {
            let bundle = self.routes.iter().find(|r| {
                r.mode == "ssr" && r.route == *claimed && r.render_entrypoint.is_some()
            });
            if let Some(r) = bundle {
                out.push((
                    (*name).to_string(),
                    r.render_entrypoint.clone().expect("checked above"),
                ));
            }
        }
        for (name, hook) in &self.hooks {
            out.retain(|(n, _)| n != name);
            out.push((name.clone(), hook.entrypoint.clone()));
        }
        out
    }
}

/// Hooks that may alternatively be declared by claiming a route — mirrors
/// `HOOK_ROUTE_CLAIMS` in `mesofact_render::route_config`. Kept as a local
/// const rather than a dependency edge because this crate's manifest struct is
/// a deliberately thin slice of the real one, and the SSR path must not need
/// the whole build-side schema to boot.
const HOOK_ROUTE_CLAIMS: &[(&str, &str)] = &[("readyz", crate::READY_PATH)];

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

/// Dispatch target: production carries an `SsrPool`; tests can inject a
/// closure to model failures/successes without booting V8.
enum DispatchTarget {
    Runtime {
        // Arc'd (R444) so `dispatch` can move a handle into `spawn_blocking`
        // — see the doc comment on the `dispatch` match arm below. R756-F2:
        // holds a pool of isolates rather than one, so N in-flight SSR
        // requests get N-way parallelism instead of queuing on one isolate.
        pool: Arc<SsrPool>,
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
            DispatchTarget::Runtime { pool, bundles } => {
                let bundle = longest_prefix_match(bundles, path)
                    .ok_or_else(|| anyhow::anyhow!("no SSR bundle registered for {path}"))?;
                // R444: route code can now do real I/O during dispatch (an
                // r2().fetch() against mesofact-dev's own dev S3 surface, or
                // real R2 in prod), so "isolate-thread round-trip is fast, no
                // I/O" no longer holds — calling SsrPool::dispatch (a
                // blocking std::sync::mpsc round-trip under the hood) inline
                // here would block the caller's own async executor thread
                // until the isolate replied. On a single-threaded runtime
                // that ALSO hosts the I/O the route is waiting on
                // (mesofact-dev's dev S3 surface in this crate's own tests,
                // or any single-worker deployment) that is a permanent
                // deadlock: the one thread able to service the connection is
                // the one now blocked waiting on it. spawn_blocking moves the
                // block off the executor thread — and R756-F2's pool means
                // that block only ever holds up one isolate's worth of
                // capacity, not the only one there is.
                let pool = Arc::clone(pool);
                tokio::task::spawn_blocking(move || pool.dispatch(&bundle, req))
                    .await
                    .context("ssr dispatch task panicked")?
            }
            #[cfg(test)]
            DispatchTarget::Mock(f) => f(req),
        }
    }

    /// Mode 2 (R756-F3 / W311 §2): invoke a hook, plain JSON in and out, no
    /// `Request`/`Response` envelope. Still `spawn_blocking` — the round
    /// trip still takes an isolate off the pool, same as `dispatch`.
    ///
    /// `bundle` is resolved by the caller from the hook registry
    /// ([`SsrChild::invoke_hook`]), not by prefix-matching a path: a hook is
    /// addressed by name, which is the whole point of R756-F6's declaration
    /// site — most hooks have no path to match on.
    async fn invoke(&self, bundle: PathBuf, hook: &str, input: Value) -> Result<Value> {
        match self {
            DispatchTarget::Runtime { pool, .. } => {
                let pool = Arc::clone(pool);
                let hook = hook.to_string();
                tokio::task::spawn_blocking(move || pool.invoke(&bundle, &hook, input))
                    .await
                    .context("ssr invoke task panicked")?
            }
            // Tests mock `dispatch` only (it's the one production entrypoint
            // that existed before this ticket); synthesize the invoke
            // boundary on top of it so every existing mock keeps working
            // unchanged — a mocked handler that ignores its request already
            // ignores `hook`/`input` too, and the ones that care about
            // status (readyz's tests) get it from the same closure either
            // way.
            #[cfg(test)]
            DispatchTarget::Mock(f) => {
                let req = DispatchRequest {
                    method: input
                        .get("method")
                        .and_then(|v| v.as_str())
                        .unwrap_or("GET")
                        .to_string(),
                    url: input
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    headers: Vec::new(),
                    body: None,
                };
                let resp = f(req)?;
                Ok(serde_json::json!({ "status": resp.status }))
            }
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
    /// R756-F6 — Mode 2 hook name → registered bundle, from either
    /// declaration site (see [`Manifest::hook_entrypoints`]). A hook is
    /// addressed by name, never by path.
    hooks: Arc<RwLock<HashMap<String, PathBuf>>>,
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

    /// Whether this workload declared Mode 2 hook `name` (R756-F6).
    ///
    /// Cheap enough to call on every probe tick, and the reason a workload
    /// that declares no hooks pays nothing: the engine checks the registry
    /// rather than dispatching into V8 to find out.
    pub fn has_hook(&self, name: &str) -> bool {
        self.hooks
            .read()
            .map(|h| h.contains_key(name))
            .unwrap_or(false)
    }

    /// Invoke Mode 2 hook `name` (R756-F3's wire verb, R756-F6's declaration
    /// site). Plain JSON in, plain JSON out — no `Request`/`Response`
    /// envelope crosses the boundary.
    ///
    /// Errors when the hook is not declared, rather than returning a neutral
    /// verdict: a caller asking for a hook this workload never declared has
    /// already gone wrong, and for `readyz` specifically a silent "no opinion"
    /// would read as "ready".
    pub async fn invoke_hook(&self, name: &str, input: Value) -> Result<Value> {
        let bundle = {
            let guard = self
                .hooks
                .read()
                .map_err(|_| anyhow::anyhow!("hook registry lock poisoned"))?;
            guard.get(name).cloned()
        };
        let bundle = bundle.ok_or_else(|| anyhow::anyhow!("no hook registered for {name:?}"))?;
        self.target.invoke(bundle, name, input).await
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
    /// Env map the isolate should see (R444) — the in-process V8 runtime
    /// can't inherit the host process's real env the way the old bun
    /// subprocess did. `Object.assign`-ed onto `globalThis.process.env` at
    /// boot, and also what `[sources.r2]` credentials in
    /// `mesofact.config.toml` resolve against. Dev callers (`mesofact-dev`)
    /// pass `DevStore::env_vars()`; the prod receiver (`mesofact serve`) passes
    /// its own real `std::env::vars()`, since that process genuinely has
    /// yubaba-injected secrets in its env. Empty by default — a workload with
    /// no `[sources.r2]` and no env-reading route code needs nothing here.
    pub env: Vec<(String, String)>,
    /// Isolate count for the SSR pool (R756-F2). `None` resolves to
    /// [`resolve_pool_size`]'s default (env override, else
    /// [`mesofact_ssr::DEFAULT_POOL_SIZE`]) at spawn time.
    pub pool_size: Option<usize>,
}

impl SpawnOptions {
    pub fn new(workload: PathBuf, gen_dir: PathBuf, state_dir: PathBuf) -> Self {
        Self {
            workload,
            gen_dir,
            state_dir,
            env: Vec::new(),
            pool_size: None,
        }
    }

    /// Attach the env map the isolate should see. See the `env` field doc.
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// Override the SSR isolate pool size. See the `pool_size` field doc.
    pub fn with_pool_size(mut self, size: usize) -> Self {
        self.pool_size = Some(size);
        self
    }
}

/// Resolve the pool size for [`spawn`]: `opts.pool_size` if the caller set
/// one, else the `MESOFACT_SSR_POOL_SIZE` env var, else
/// [`mesofact_ssr::DEFAULT_POOL_SIZE`]. An unparseable env var falls back to
/// the default *and* warns — same contract as `MESOFACT_DRAIN_GRACE_SECS`
/// (mesofact/src/lib.rs): a typo must not silently become the smallest/
/// least-capable setting.
fn resolve_pool_size(opts_pool_size: Option<usize>) -> usize {
    if let Some(n) = opts_pool_size {
        return n.max(1);
    }
    match std::env::var("MESOFACT_SSR_POOL_SIZE") {
        Ok(raw) => match raw.trim().parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => {
                warn!(%raw, "MESOFACT_SSR_POOL_SIZE is not a positive integer — using default");
                mesofact_ssr::DEFAULT_POOL_SIZE
            }
        },
        Err(_) => mesofact_ssr::DEFAULT_POOL_SIZE,
    }
}

/// One `[sources.<name>]` entry of `kind = "r2"` from `mesofact.config.toml`,
/// mirroring `packages/mesofact-runtime/src/config.ts`'s `R2SourceConfig` —
/// the file carries only env var *names*, never secrets.
#[derive(Debug, Clone, Deserialize)]
struct RawR2Source {
    kind: String,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    endpoint_env: Option<String>,
    #[serde(default)]
    access_key_id_env: Option<String>,
    #[serde(default)]
    secret_access_key_env: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SourcesFile {
    #[serde(default)]
    sources: HashMap<String, RawR2Source>,
}

const DEFAULT_ACCESS_KEY_ID_ENV: &str = "AWS_ACCESS_KEY_ID";
const DEFAULT_SECRET_ACCESS_KEY_ENV: &str = "AWS_SECRET_ACCESS_KEY";

/// Read `<workload>/mesofact.config.toml` (absent → no sources, not an error
/// — most workloads don't declare any) and resolve every `kind = "r2"` entry's
/// credentials against `env`. A declared source with a missing/empty env var
/// fails the whole spawn — mirrors `mesofact-worker`'s `runWorker`, which
/// registers adapters from the same file before loading entrypoints and fails
/// fast at boot rather than at the first request.
fn resolve_r2_sources(workload: &Path, env: &[(String, String)]) -> Result<Vec<R2SourceCoords>> {
    let path = workload.join("mesofact.config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let file: SourcesFile =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    let mut out = Vec::new();
    for (name, src) in file.sources {
        if src.kind != "r2" {
            continue;
        }
        let bucket = src
            .bucket
            .with_context(|| format!("[sources.{name}] missing `bucket`"))?;
        let endpoint_env = src
            .endpoint_env
            .with_context(|| format!("[sources.{name}] missing `endpoint_env`"))?;
        let access_key_id_env = src
            .access_key_id_env
            .unwrap_or_else(|| DEFAULT_ACCESS_KEY_ID_ENV.to_string());
        let secret_access_key_env = src
            .secret_access_key_env
            .unwrap_or_else(|| DEFAULT_SECRET_ACCESS_KEY_ENV.to_string());
        out.push(R2SourceCoords {
            endpoint: env_required(env, &endpoint_env, &name, "endpoint_env")?,
            access_key_id: env_required(env, &access_key_id_env, &name, "access_key_id_env")?,
            secret_access_key: env_required(
                env,
                &secret_access_key_env,
                &name,
                "secret_access_key_env",
            )?,
            name,
            bucket,
        });
    }
    Ok(out)
}

fn env_required(env: &[(String, String)], key: &str, source: &str, field: &str) -> Result<String> {
    env.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty())
        .with_context(|| format!("[sources.{source}] env var `{key}` (from {field}) is unset or empty"))
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
    let hook_entrypoints = manifest.hook_entrypoints();

    // R444: resolve [sources.r2] against opts.env before booting the isolate
    // — a bad/missing credential env var should fail the whole spawn, same as
    // mesofact-worker's boot-time registerSourcesFromConfig.
    let r2_sources = resolve_r2_sources(&opts.workload, &opts.env)?;

    let pool_size = resolve_pool_size(opts.pool_size);
    info!(
        prefixes = ?prefixes,
        entrypoints = entrypoints.len(),
        hooks = ?hook_entrypoints.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        r2_sources = r2_sources.len(),
        pool_size,
        "mesofact-dev ssr runtime starting",
    );

    // Boot the pool off the async runtime — SsrPool::start blocks until
    // every isolate's bootstrap evaluates, and register/register_r2_sources
    // are also blocking. All CPU-bound (V8 init x N); spawn_blocking keeps
    // the tokio runtime free.
    let state_dir = opts.state_dir.clone();
    let gen_dir_for_blocking = opts.gen_dir.clone();
    let env = opts.env.clone();
    let log = log_buffer.clone();
    let _ = tokio::fs::create_dir(&state_dir).await; // best-effort; ignore EEXIST
    let (pool, bundles, hooks) = tokio::task::spawn_blocking(move || -> Result<_> {
        let pool = SsrPool::start(env, pool_size).context("starting SsrPool")?;
        if !r2_sources.is_empty() {
            pool.register_r2_sources(&r2_sources)
                .context("registering r2 sources in SSR isolate pool")?;
        }
        let mut bundles: HashMap<String, PathBuf> = HashMap::new();
        for (prefix, rel) in entrypoints {
            let bundle = resolve_entrypoint(&gen_dir_for_blocking, &rel);
            pool.register(&bundle)
                .with_context(|| format!("registering SSR entrypoint {}", bundle.display()))?;
            bundles.insert(prefix, bundle);
        }
        // Hook modules register exactly like route bundles — the harness keys
        // its handler map by module URL either way, so `invoke` finds a hook's
        // default export through the same path `dispatch` finds a route's. A
        // route-claimed hook resolves to a bundle already registered above;
        // `register` is idempotent, so re-registering it is free.
        let mut hooks: HashMap<String, PathBuf> = HashMap::new();
        for (name, rel) in hook_entrypoints {
            let bundle = resolve_entrypoint(&gen_dir_for_blocking, &rel);
            pool.register(&bundle)
                .with_context(|| format!("registering hook {name} ({})", bundle.display()))?;
            hooks.insert(name, bundle);
        }
        Ok((pool, bundles, hooks))
    })
    .await
    .context("ssr runtime init task panicked")??;

    // Surface readiness to the dev log surface in the same shape the bun
    // child used to (the operator's eyes are tuned to "ssr ... ready").
    log.push(format!(
        "[mesofact-dev/ssr] ready: {} handler(s) + {} hook(s) in-process, {} isolate(s)",
        bundles.len(),
        hooks.len(),
        pool.size(),
    ))
    .await;

    Ok(Some(SsrChild {
        target: DispatchTarget::Runtime { pool: Arc::new(pool), bundles },
        prefixes: Arc::new(RwLock::new(prefixes)),
        policies: Arc::new(RwLock::new(policies)),
        hooks: Arc::new(RwLock::new(hooks)),
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
pub(crate) fn detached_for_test_with_policies(
    prefixes: Vec<String>,
    policies: Vec<(String, ResiliencePolicy)>,
    dispatch_fn: impl Fn(DispatchRequest) -> Result<DispatchResponse> + Send + Sync + 'static,
) -> SsrChild {
    // Model the route-claim declaration site: a mock whose prefixes cover
    // `/readyz` has a `readyz` hook, exactly as `Manifest::hook_entrypoints`
    // would derive for a workload that claims the route. The bundle path is
    // never dereferenced — `DispatchTarget::Mock` ignores it — so a
    // placeholder is honest here rather than sloppy.
    let hooks: HashMap<String, PathBuf> = HOOK_ROUTE_CLAIMS
        .iter()
        .filter(|(_, claimed)| prefixes.iter().any(|p| matches_prefix(claimed, p)))
        .map(|(name, _)| ((*name).to_string(), PathBuf::from(format!("mock://{name}"))))
        .collect();
    SsrChild {
        target: DispatchTarget::Mock(Box::new(dispatch_fn)),
        prefixes: Arc::new(RwLock::new(prefixes)),
        policies: Arc::new(RwLock::new(policies)),
        hooks: Arc::new(RwLock::new(hooks)),
        log_buffer: LogBuffer::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_r2_sources_missing_config_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_r2_sources(dir.path(), &[]).unwrap().is_empty());
    }

    #[test]
    fn resolve_r2_sources_resolves_declared_env_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mesofact.config.toml"),
            r#"
            [sources.assets]
            kind = "r2"
            bucket = "dev"
            endpoint_env = "R2_ENDPOINT"
            access_key_id_env = "R2_ACCESS_KEY_ID"
            secret_access_key_env = "R2_SECRET_ACCESS_KEY"

            [sources.project_db]
            kind = "sqlite"
            path = "/tmp/x.db"
            "#,
        )
        .unwrap();
        let env = vec![
            ("R2_ENDPOINT".to_string(), "http://127.0.0.1:1".to_string()),
            ("R2_ACCESS_KEY_ID".to_string(), "dev".to_string()),
            ("R2_SECRET_ACCESS_KEY".to_string(), "dev".to_string()),
        ];
        let sources = resolve_r2_sources(dir.path(), &env).unwrap();
        assert_eq!(sources.len(), 1, "sqlite entries are not r2 sources");
        assert_eq!(sources[0].name, "assets");
        assert_eq!(sources[0].bucket, "dev");
        assert_eq!(sources[0].endpoint, "http://127.0.0.1:1");
        assert_eq!(sources[0].access_key_id, "dev");
        assert_eq!(sources[0].secret_access_key, "dev");
    }

    #[test]
    fn resolve_r2_sources_defaults_credential_env_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mesofact.config.toml"),
            "[sources.assets]\nkind = \"r2\"\nbucket = \"dev\"\nendpoint_env = \"R2_ENDPOINT\"\n",
        )
        .unwrap();
        let env = vec![
            ("R2_ENDPOINT".to_string(), "http://127.0.0.1:1".to_string()),
            ("AWS_ACCESS_KEY_ID".to_string(), "aki".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "sak".to_string()),
        ];
        let sources = resolve_r2_sources(dir.path(), &env).unwrap();
        assert_eq!(sources[0].access_key_id, "aki");
        assert_eq!(sources[0].secret_access_key, "sak");
    }

    #[test]
    fn resolve_r2_sources_fails_fast_on_missing_env_var() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mesofact.config.toml"),
            "[sources.assets]\nkind = \"r2\"\nbucket = \"dev\"\nendpoint_env = \"R2_ENDPOINT\"\n",
        )
        .unwrap();
        let err = resolve_r2_sources(dir.path(), &[]).unwrap_err();
        assert!(
            err.to_string().contains("R2_ENDPOINT"),
            "error should name the missing env var, got {err}"
        );
    }

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

    // ── R756-F6: the Mode 2 hook declaration site ────────────────────────────

    /// The general declaration site: a hook that is not a route at all. Proves
    /// the whole chain the `hooks` block exists for — manifest → registered
    /// bundle → `invoke_hook(name)` — with nothing path-shaped anywhere in it.
    #[tokio::test]
    async fn spawn_registers_a_declared_hook_that_has_no_route() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_dir = dir.path().join("server").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(
            hooks_dir.join("readyz.js"),
            "export default async function (_req) {\n\
                return new Response('ok');\n\
              }\n",
        )
        .unwrap();
        let server_dir = dir.path().join("server");
        std::fs::write(
            server_dir.join("ping.js"),
            "export default async function () { return new Response('pong'); }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "routes": [
                    {"route": "/api/ping", "mode": "ssr", "render_entrypoint": "dist/server/ping.js"}
                ],
                "hooks": {"readyz": {"entrypoint": "dist/server/hooks/readyz.js"}}
            }"#,
        )
        .unwrap();

        let opts = SpawnOptions::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            dir.path().join(".mesofact-dev"),
        );
        let child = spawn(opts).await.unwrap().expect("ssr present");

        assert!(child.has_hook("readyz"));
        assert!(!child.has_hook("onRequest"), "undeclared hooks stay absent");
        // The hook module is NOT a route: it never entered the prefix set, so
        // the edge never forwards `/readyz` to the SSR origin on its account.
        assert!(!child.matches(crate::READY_PATH), "a hook must not become a route");

        let verdict = child
            .invoke_hook("readyz", serde_json::json!({"method": "GET", "url": "http://dev/readyz"}))
            .await
            .unwrap();
        assert_eq!(verdict, serde_json::json!({"status": 200}));
    }

    /// The legacy declaration site still binds the same hook: an app that
    /// claims `/readyz` as an SSR route (how this shipped in R756-F5) keeps
    /// working, with its route bundle bound under the hook name.
    #[tokio::test]
    async fn a_route_claimed_readyz_still_binds_the_hook() {
        let dir = tempfile::tempdir().unwrap();
        let server_dir = dir.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(
            server_dir.join("readyz.js"),
            "export default async function (req) {\n\
                const fail = new URL(req.url).searchParams.has('fail');\n\
                return new Response('x', { status: fail ? 503 : 200 });\n\
              }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "routes": [
                    {"route": "/readyz", "mode": "ssr", "render_entrypoint": "dist/server/readyz.js"}
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

        assert!(child.has_hook("readyz"));
        let verdict = child
            .invoke_hook(
                "readyz",
                serde_json::json!({"method": "GET", "url": "http://dev/readyz?fail"}),
            )
            .await
            .unwrap();
        assert_eq!(verdict, serde_json::json!({"status": 503}));
    }

    /// An undeclared hook errors rather than answering neutrally — for
    /// `readyz` a silent "no opinion" reads as "ready", which is the one
    /// wrong answer a readiness probe can give.
    #[tokio::test]
    async fn invoking_an_undeclared_hook_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let server_dir = dir.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(
            server_dir.join("ping.js"),
            "export default async function () { return new Response('pong'); }\n",
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

        assert!(!child.has_hook("readyz"));
        let err = child.invoke_hook("readyz", serde_json::json!({})).await.unwrap_err();
        assert!(err.to_string().contains("readyz"), "error should name the hook, got {err}");
    }

    /// `hooks` and the route claim are rejected together upstream
    /// (`validate_routes_config`), so this only pins the tie-break for a
    /// hand-written manifest that carries both: the explicit declaration wins,
    /// and the hook is bound exactly once.
    #[test]
    fn an_explicit_hook_declaration_wins_over_the_route_claim() {
        let manifest: Manifest = serde_json::from_str(
            r#"{
                "routes": [
                    {"route": "/readyz", "mode": "ssr", "render_entrypoint": "dist/server/claimed.js"}
                ],
                "hooks": {"readyz": {"entrypoint": "dist/server/hooks/declared.js"}}
            }"#,
        )
        .unwrap();
        assert_eq!(
            manifest.hook_entrypoints(),
            vec![("readyz".to_string(), "dist/server/hooks/declared.js".to_string())],
        );
    }

    /// R756-T4 (W311 §1's second measurement, pinned against the R756-T1
    /// fix): on a 2-worker tokio runtime, 2 in-flight SSR dispatches must not
    /// stall an unrelated task that does no I/O of its own — this stands in
    /// for /livez, /readyz, and static files sharing the same runtime as SSR.
    /// Pre-fix, `DispatchTarget::dispatch` called the blocking
    /// `SsrPool::dispatch` round-trip inline on the async fn, parking a
    /// worker thread per in-flight SSR request; W311 measured 167ms for the
    /// unrelated task under exactly this shape (2 workers, 2 in-flight). The
    /// fix moves that block onto tokio's blocking-thread pool via
    /// `spawn_blocking`, so the 2 async worker threads stay free. Asserts an
    /// order of magnitude below the pre-fix number, not a millisecond figure,
    /// per T4's flake-discipline guidance.
    #[test]
    fn ssr_saturation_does_not_starve_an_unrelated_instant_task() {
        let dir = tempfile::tempdir().unwrap();
        let server_dir = dir.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(
            server_dir.join("slow.js"),
            "export default async function () {\n\
               await new Promise((res) => setTimeout(res, 200));\n\
               return new Response('done');\n\
             }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "routes": [
                    {"route": "/api/slow", "mode": "ssr", "render_entrypoint": "dist/server/slow.js"}
                ]
            }"#,
        )
        .unwrap();

        // Exactly the shape W311 §1 measured: 2 tokio workers.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let opts = SpawnOptions::new(
                dir.path().to_path_buf(),
                dir.path().to_path_buf(),
                dir.path().join(".mesofact-dev"),
            )
            // pool_size 1: this test pins T1's spawn_blocking property (the
            // async workers don't park), not F2's pool parallelism (a
            // separate, already-covered property in mesofact-ssr).
            .with_pool_size(1);
            let child = Arc::new(spawn(opts).await.unwrap().expect("ssr present"));

            let dispatch = |child: Arc<SsrChild>| {
                tokio::spawn(async move {
                    child
                        .dispatch(
                            "/api/slow",
                            DispatchRequest {
                                method: "GET".into(),
                                url: "http://dev/api/slow".into(),
                                headers: vec![],
                                body: None,
                            },
                        )
                        .await
                })
            };
            // Saturate both workers with in-flight SSR dispatches.
            let ssr1 = dispatch(Arc::clone(&child));
            let ssr2 = dispatch(Arc::clone(&child));
            // Let both dispatches actually reach the blocking recv before
            // timing the unrelated task.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;

            let start = std::time::Instant::now();
            tokio::spawn(async {}).await.unwrap();
            let elapsed = start.elapsed();

            ssr1.await.unwrap().unwrap();
            ssr2.await.unwrap().unwrap();

            assert!(
                elapsed.as_millis() < 100,
                "unrelated instant task took {elapsed:?} while 2 SSR dispatches \
                 were in flight on a 2-worker runtime (W311 §1 measured 167ms \
                 pre-fix; should now be single-digit ms)",
            );
        });
    }
}
