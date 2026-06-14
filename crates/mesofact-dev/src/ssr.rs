//! SSR subprocess + reverse proxy for `mode:"ssr"` routes (R434-F3, W173).
//!
//! Reads the routes manifest from the active gen dir, derives the SSR-prefix
//! set, spawns a bun child that imports each SSR route's Fetch entrypoint, and
//! exposes a [`SsrChild`] handle the server uses to proxy matching requests.
//!
//! Bun is required only when the manifest has at least one `mode:"ssr"` route
//! — static/SPA-only workloads keep starting with no Bun toolchain.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

/// File mesofact-dev writes inside the workload's state dir so other tools can
/// discover the SSR child's port. Cleared on shutdown is best-effort.
pub const SSR_PORT_FILE: &str = "ssr-port";

/// Embedded bun wrapper script. Written to the workload's state dir at spawn
/// time so the child sees a stable on-disk path it can `import` other modules
/// alongside.
const SSR_WRAPPER_TS: &str = include_str!("ssr_wrapper.ts");
const SSR_WRAPPER_NAME: &str = "ssr-wrapper.ts";

const LOG_CAP: usize = 500;
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

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

/// W181 v1 — schema mirror of `mesofact::manifest::ResiliencePolicy`. Kept
/// local to avoid pulling the full mesofact crate into mesofact-dev's path.
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
    /// declared a `resilience` block. The prefix derivation matches
    /// `ssr_prefixes()` so [`SsrChild::policy_for`] can do a longest-prefix
    /// lookup against the same key space the matcher uses.
    pub fn resilience_policies(&self) -> Vec<(String, ResiliencePolicy)> {
        self.routes
            .iter()
            .filter(|r| r.mode == "ssr")
            .filter_map(|r| {
                r.resilience
                    .clone()
                    .map(|p| (derive_prefix(&r.route), p))
            })
            .collect()
    }

    /// SSR-prefix set per W173. Prefers the manifest's pre-derived
    /// `ssr_prefixes` (mesofact-build emits it); falls back to deriving from
    /// the route patterns when older builds don't carry the field.
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
/// When `prefix` already ends with `/` it's a parametric-derived prefix so
/// plain `startsWith` matches subpaths correctly.
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

/// Bounded ring buffer for child stderr lines. Same shape as
/// `cloud::reconciler::LogBuffer` but kept local to avoid a cross-crate dep.
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

/// Swappable holder for the SSR child. The router reads `current()` on every
/// request, so the watcher's post-build hook can lazily install a child after
/// the first build (when the manifest first exists) or rotate it when the
/// route set changes. Cheap to clone.
#[derive(Clone, Default)]
pub struct SsrSlot {
    inner: Arc<RwLock<Option<Arc<SsrChild>>>>,
}

impl SsrSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Currently-installed SSR child, if any. Cloning the `Arc` lets the
    /// request handler hold the child across `.await` without keeping the
    /// slot's lock.
    pub fn current(&self) -> Option<Arc<SsrChild>> {
        self.inner.read().ok().and_then(|g| g.clone())
    }

    /// Install or evict a child. `Some` swaps in a new one (dropping the
    /// previous), `None` clears the slot.
    pub fn set(&self, child: Option<Arc<SsrChild>>) {
        if let Ok(mut w) = self.inner.write() {
            *w = child;
        }
    }
}

/// Live SSR subprocess + the data the server needs to route to it.
pub struct SsrChild {
    /// Port the bun child binds on `127.0.0.1`.
    port: u16,
    /// Prefix set the server matches against to decide proxy vs static.
    /// Mutable so [`SsrChild::restart_with`] can refresh it from a new
    /// manifest after a watch-mode gen flip.
    prefixes: Arc<RwLock<Vec<String>>>,
    /// W181 — per-route resilience block, keyed by the route's derived
    /// prefix. Looked up on each request via [`SsrChild::policy_for`];
    /// refreshed in [`SsrChild::restart_with`] alongside prefixes.
    policies: Arc<RwLock<Vec<(String, ResiliencePolicy)>>>,
    /// Active gen dir. Shared with the supervisor — read on each (re)spawn,
    /// updated by [`SsrChild::restart_with`].
    gen_dir: Arc<RwLock<PathBuf>>,
    /// Stderr ring buffer; surfaced through the existing dev log surface.
    log_buffer: LogBuffer,
    /// Signal channel for [`SsrChild::restart_with`] — sending kills the
    /// current bun child so the supervisor re-spawns with the new gen dir.
    restart_tx: mpsc::UnboundedSender<()>,
    /// Supervisor task; aborts the child on drop.
    _supervisor: tokio::task::JoinHandle<()>,
    /// Path to the persisted port file; removed on drop best-effort.
    port_file: PathBuf,
}

impl SsrChild {
    pub fn port(&self) -> u16 {
        self.port
    }

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
    /// Returns `None` when no SSR route has declared a `resilience` block.
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

    /// Re-read the manifest from a new gen dir, refresh the prefix set, and
    /// signal the supervisor to kill the current bun child so it respawns
    /// with the new module graph.
    ///
    /// Bun caches dynamic imports, so a long-lived child keeps serving stale
    /// modules across rebuilds — the only honest fix is to restart it. The
    /// caller (mesofact-dev's watcher post-build hook) invokes this once per
    /// successful gen flip.
    pub async fn restart_with(&self, gen_dir: PathBuf) -> Result<()> {
        match Manifest::read(&gen_dir)? {
            Some(m) => {
                let new_prefixes = m.ssr_prefixes();
                let new_policies = m.resilience_policies();
                if let Ok(mut p) = self.prefixes.write() {
                    *p = new_prefixes;
                }
                if let Ok(mut p) = self.policies.write() {
                    *p = new_policies;
                }
            }
            None => {
                // Manifest missing in the new gen — leave prefixes as-is and
                // let the supervisor spawn try (and likely fail loudly via
                // the LogBuffer). Don't silently empty the matcher: a stale
                // prefix set keeps the static fallback honest until the
                // build catches up.
                warn!(
                    gen_dir = %gen_dir.display(),
                    "ssr restart_with: manifest missing in new gen — keeping prior prefixes",
                );
            }
        }
        if let Ok(mut g) = self.gen_dir.write() {
            *g = gen_dir;
        }
        let _ = self.restart_tx.send(());
        Ok(())
    }
}

impl Drop for SsrChild {
    fn drop(&mut self) {
        self._supervisor.abort();
        let _ = std::fs::remove_file(&self.port_file);
    }
}

/// Test-only constructor: assemble an [`SsrChild`] from an already-bound port
/// and prefix set, with a no-op supervisor. Lets the router proxy tests run
/// against a mock HTTP server without spawning bun.
#[cfg(test)]
pub(crate) fn detached_for_test(port: u16, prefixes: Vec<String>) -> SsrChild {
    detached_for_test_with_policies(port, prefixes, Vec::new())
}

#[cfg(test)]
pub(crate) fn detached_for_test_with_policies(
    port: u16,
    prefixes: Vec<String>,
    policies: Vec<(String, ResiliencePolicy)>,
) -> SsrChild {
    let (restart_tx, _restart_rx) = mpsc::unbounded_channel();
    SsrChild {
        port,
        prefixes: Arc::new(RwLock::new(prefixes)),
        policies: Arc::new(RwLock::new(policies)),
        gen_dir: Arc::new(RwLock::new(std::env::temp_dir())),
        log_buffer: LogBuffer::new(),
        restart_tx,
        _supervisor: tokio::spawn(async {}),
        port_file: std::env::temp_dir().join(format!("mesofact-dev-test-ssr-port-{port}")),
    }
}

/// Options for [`spawn`]. The workload directory anchors the state dir; the
/// gen_dir is the snapshot the bun child should import from (the watcher's
/// active `gen-N/`, or `<workload>/dist/` when running without a watcher).
pub struct SpawnOptions {
    pub workload: PathBuf,
    pub gen_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Program to invoke (default `bun`). Override only for tests — production
    /// always wants the system bun.
    pub program: Option<PathBuf>,
}

impl SpawnOptions {
    /// Shorthand used by production callers; `program` defaults to `bun`.
    pub fn new(workload: PathBuf, gen_dir: PathBuf, state_dir: PathBuf) -> Self {
        Self {
            workload,
            gen_dir,
            state_dir,
            program: None,
        }
    }
}

/// Inspect the manifest; if it has any SSR route, spawn the bun child and
/// return a [`SsrChild`] handle. Returns `Ok(None)` when no SSR routes are
/// declared — the caller serves static only.
///
/// Fails with a clear error when the manifest declares SSR routes but Bun is
/// missing from PATH. We bail at spawn time rather than crashing on the first
/// SSR request.
pub async fn spawn(opts: SpawnOptions) -> Result<Option<SsrChild>> {
    let manifest = match Manifest::read(&opts.gen_dir)? {
        Some(m) => m,
        None => return Ok(None),
    };
    if !manifest.has_ssr() {
        return Ok(None);
    }

    let program = opts.program.unwrap_or_else(|| PathBuf::from("bun"));
    // Only enforce the PATH check when the caller is using the default
    // program name — an explicit override is the test path and may point at
    // a script that isn't on PATH.
    if program == Path::new("bun") && which_bun().is_none() {
        anyhow::bail!(
            "bun not found on PATH; mesofact-dev requires Bun when any route is mode:\"ssr\" \
             (install via https://bun.sh or remove the SSR route)",
        );
    }

    tokio::fs::create_dir_all(&opts.state_dir)
        .await
        .with_context(|| format!("creating state dir {}", opts.state_dir.display()))?;
    let wrapper_path = opts.state_dir.join(SSR_WRAPPER_NAME);
    tokio::fs::write(&wrapper_path, SSR_WRAPPER_TS)
        .await
        .with_context(|| format!("writing {}", wrapper_path.display()))?;

    let port = pick_ephemeral_port().await?;
    let port_file = opts.state_dir.join(SSR_PORT_FILE);
    tokio::fs::write(&port_file, port.to_string())
        .await
        .with_context(|| format!("writing {}", port_file.display()))?;

    let log_buffer = LogBuffer::new();
    let prefixes = Arc::new(RwLock::new(manifest.ssr_prefixes()));
    let policies = Arc::new(RwLock::new(manifest.resilience_policies()));
    let gen_dir = Arc::new(RwLock::new(opts.gen_dir));
    let (restart_tx, restart_rx) = mpsc::unbounded_channel();

    info!(port, prefixes = ?prefixes.read().ok().map(|p| p.clone()), "mesofact-dev ssr child starting");

    let supervisor = tokio::spawn(supervise(
        program,
        wrapper_path,
        Arc::clone(&gen_dir),
        restart_rx,
        port,
        log_buffer.clone(),
    ));

    Ok(Some(SsrChild {
        port,
        prefixes,
        policies,
        gen_dir,
        log_buffer,
        restart_tx,
        _supervisor: supervisor,
        port_file,
    }))
}

/// Locate `bun` on PATH. Wrapped so tests can read intent.
fn which_bun() -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join("bun"))
        .find(|candidate| candidate.is_file())
}

/// Bind 127.0.0.1:0, read the assigned port, drop the listener. Racy in
/// theory; in practice the bun child binds on the same loopback within
/// milliseconds and there's no other allocator competing on the same port.
async fn pick_ephemeral_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding ephemeral port for ssr child")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

async fn supervise(
    program: PathBuf,
    wrapper_path: PathBuf,
    gen_dir: Arc<RwLock<PathBuf>>,
    mut restart_rx: mpsc::UnboundedReceiver<()>,
    port: u16,
    log_buffer: LogBuffer,
) {
    let mut backoff = BACKOFF_MIN;
    loop {
        let current_gen = match gen_dir.read() {
            Ok(g) => g.clone(),
            Err(_) => {
                warn!("ssr supervisor: gen_dir lock poisoned; bailing");
                return;
            }
        };
        match spawn_once(
            &program,
            &wrapper_path,
            &current_gen,
            port,
            &log_buffer,
            &mut restart_rx,
        )
        .await
        {
            SpawnOutcome::CleanExit => {
                info!("mesofact-dev ssr child exited cleanly");
                return;
            }
            SpawnOutcome::Restarted => {
                info!("mesofact-dev ssr child restarted on gen flip");
                // Skip the backoff sleep — a gen flip is an operator-driven
                // restart, not a crash loop. Reset backoff so a real crash
                // immediately after still gets the gentle ramp.
                backoff = BACKOFF_MIN;
                continue;
            }
            SpawnOutcome::Crashed { code } => {
                warn!(
                    code = code.unwrap_or(-1),
                    backoff_ms = backoff.as_millis() as u64,
                    "mesofact-dev ssr child exited; restarting after backoff",
                );
            }
            SpawnOutcome::SpawnError(e) => {
                warn!(
                    error = %e,
                    backoff_ms = backoff.as_millis() as u64,
                    "mesofact-dev ssr child spawn failed; retrying",
                );
            }
        }
        // Wait the backoff, but a gen-flip signal cuts it short.
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = restart_rx.recv() => {
                info!("ssr restart signal received during backoff; respawning immediately");
                backoff = BACKOFF_MIN;
                continue;
            }
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

enum SpawnOutcome {
    CleanExit,
    Crashed { code: Option<i32> },
    Restarted,
    SpawnError(anyhow::Error),
}

async fn spawn_once(
    program: &Path,
    wrapper_path: &Path,
    gen_dir: &Path,
    port: u16,
    log_buffer: &LogBuffer,
    restart_rx: &mut mpsc::UnboundedReceiver<()>,
) -> SpawnOutcome {
    let mut child = match Command::new(program)
        .arg("run")
        .arg(wrapper_path)
        .env("MESOFACT_GEN_DIR", gen_dir)
        .env("MESOFACT_SSR_PORT", port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return SpawnOutcome::SpawnError(
                anyhow::Error::new(e).context(format!(
                    "spawning {} {}",
                    program.display(),
                    wrapper_path.display(),
                )),
            );
        }
    };

    if let Some(stderr) = child.stderr.take() {
        let buf = log_buffer.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                // Tee to mesofact-dev's tracing surface so the operator sees
                // bun-side errors (typos, import failures) in the terminal /
                // Run-tab log, not just in the in-process ring buffer. R434-B6
                // verify item: a forced bun import failure must surface on
                // stdout, not silently in LogBuffer.
                warn!(target: "mesofact_dev::ssr::bun", "{line}");
                buf.push(line).await;
            }
        });
    }
    if let Some(stdout) = child.stdout.take() {
        let buf = log_buffer.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                info!(target: "mesofact_dev::ssr::bun", "{line}");
                buf.push(line).await;
            }
        });
    }

    // Race the child's exit against a restart signal. On signal we kill the
    // child (kill_on_drop covers the drop path, but an explicit start_kill
    // gets the process out of the way faster) and report Restarted.
    tokio::select! {
        wait = child.wait() => match wait {
            Ok(status) if status.success() => SpawnOutcome::CleanExit,
            Ok(status) => SpawnOutcome::Crashed { code: status.code() },
            Err(e) => SpawnOutcome::SpawnError(anyhow::Error::new(e).context("waiting on bun ssr child")),
        },
        _ = restart_rx.recv() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            SpawnOutcome::Restarted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_prefix_table() {
        // W173 table: /api/health → /api/health
        assert_eq!(derive_prefix("/api/health"), "/api/health");
        // /api/users/:id → /api/users/
        assert_eq!(derive_prefix("/api/users/:id"), "/api/users/");
        // /x/:a/y → /x/
        assert_eq!(derive_prefix("/x/:a/y"), "/x/");
        // /feed/* → /feed/
        assert_eq!(derive_prefix("/feed/*"), "/feed/");
        // root parametric
        assert_eq!(derive_prefix("/:id"), "/");
    }

    #[test]
    fn matches_prefix_segment_aware() {
        // The core regression in the W173 spec.
        assert!(matches_prefix("/api/health", "/api/health"));
        assert!(!matches_prefix("/api/healthcheck", "/api/health"));
        assert!(matches_prefix("/api/health/sub", "/api/health"));
        // Parametric prefix.
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
        // The pre-derived list wins, even if it differs from what we'd
        // compute. Dedup applied.
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
        let m: Manifest = serde_json::from_str(
            r#"{"routes": [{"route": "/", "mode": "static"}]}"#,
        )
        .unwrap();
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
        // Oldest 5 dropped.
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
        // No SSR route → no bun spawn → Ok(None) even when bun is absent.
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

    /// Verify item: "Bun child crash restarts and stderr surfaces in the dev
    /// log". Injects a fake program that prints an iteration counter to
    /// stderr and exits 1. The supervisor must restart it; the LogBuffer
    /// must accumulate the stderr lines.
    #[tokio::test]
    async fn supervisor_restarts_failing_child_and_captures_stderr() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "routes": [{"route": "/api/x", "mode": "ssr", "render_entrypoint": "dist/server/x.js"}]
            }"#,
        )
        .unwrap();

        // Fake program: shell script that ignores all args, prints an
        // identifying line to stderr, and exits non-zero. The supervisor
        // sees a crash, backs off, restarts.
        let fake = dir.path().join("fake-bun.sh");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho \"fake-bun stderr line\" >&2\nexit 1\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&fake, perms).unwrap();

        let opts = SpawnOptions {
            workload: dir.path().to_path_buf(),
            gen_dir: dir.path().to_path_buf(),
            state_dir: dir.path().join(".mesofact-dev"),
            program: Some(fake),
        };
        let child = spawn(opts).await.unwrap().expect("ssr present");

        // Wait long enough for at least two restarts (initial spawn = 0
        // backoff, second spawn = 250ms backoff, then a third by ~750ms).
        tokio::time::sleep(Duration::from_millis(900)).await;

        let lines = child.log_buffer().lines().await;
        let crash_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.contains("fake-bun stderr line"))
            .collect();
        assert!(
            crash_lines.len() >= 2,
            "expected ≥2 restarts captured in log buffer, got {}: {:?}",
            crash_lines.len(),
            lines,
        );
    }

    /// R434-B6: `SsrChild::restart_with(new_gen)` must kill the current bun
    /// child, point its env at the new gen dir, and respawn — otherwise the
    /// child's cached dynamic imports keep serving stale modules across
    /// rebuilds. Fake-bun echoes its MESOFACT_GEN_DIR to stderr; we assert
    /// both gens appear in the captured log.
    #[tokio::test]
    async fn restart_with_rotates_gen_dir_and_respawns() {
        let dir = tempfile::tempdir().unwrap();
        let gen_a = dir.path().join("gen-a");
        let gen_b = dir.path().join("gen-b");
        std::fs::create_dir_all(&gen_a).unwrap();
        std::fs::create_dir_all(&gen_b).unwrap();
        // Both gens declare the same single SSR route; the prefix set is
        // therefore identical so the test isolates the gen-dir rotation
        // from the prefix-refresh path.
        for g in [&gen_a, &gen_b] {
            std::fs::write(
                g.join("manifest.json"),
                r#"{
                    "routes": [{"route": "/api/x", "mode": "ssr", "render_entrypoint": "dist/server/x.js"}]
                }"#,
            )
            .unwrap();
        }

        // Fake program: echoes the gen-dir env var to stderr and sleeps
        // until killed. The supervisor's restart path SIGKILLs it; the
        // next iteration sees the new env and emits the new gen-dir line.
        let fake = dir.path().join("fake-bun.sh");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho \"gen=$MESOFACT_GEN_DIR\" >&2\nsleep 60\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&fake, perms).unwrap();

        let opts = SpawnOptions {
            workload: dir.path().to_path_buf(),
            gen_dir: gen_a.clone(),
            state_dir: dir.path().join(".mesofact-dev"),
            program: Some(fake),
        };
        let child = spawn(opts).await.unwrap().expect("ssr present");

        // Let the first child boot and write its identifying stderr line.
        for _ in 0..20 {
            if child
                .log_buffer()
                .lines()
                .await
                .iter()
                .any(|l| l.contains(&format!("gen={}", gen_a.display())))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        child.restart_with(gen_b.clone()).await.unwrap();

        for _ in 0..20 {
            if child
                .log_buffer()
                .lines()
                .await
                .iter()
                .any(|l| l.contains(&format!("gen={}", gen_b.display())))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let lines = child.log_buffer().lines().await;
        assert!(
            lines.iter().any(|l| l.contains(&format!("gen={}", gen_a.display()))),
            "first child should have logged gen-a: {:?}",
            lines,
        );
        assert!(
            lines.iter().any(|l| l.contains(&format!("gen={}", gen_b.display()))),
            "restart_with should have respawned with gen-b: {:?}",
            lines,
        );
    }

    /// R434-B6: when the new gen has different SSR routes, restart_with must
    /// refresh the prefix set the router consults. Verifies the matcher sees
    /// the new prefix without a fresh `spawn`.
    #[tokio::test]
    async fn restart_with_refreshes_prefix_set_from_new_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let gen_a = dir.path().join("gen-a");
        let gen_b = dir.path().join("gen-b");
        std::fs::create_dir_all(&gen_a).unwrap();
        std::fs::create_dir_all(&gen_b).unwrap();
        std::fs::write(
            gen_a.join("manifest.json"),
            r#"{"routes": [{"route": "/api/old", "mode": "ssr", "render_entrypoint": "dist/server/old.js"}]}"#,
        )
        .unwrap();
        std::fs::write(
            gen_b.join("manifest.json"),
            r#"{"routes": [{"route": "/api/new", "mode": "ssr", "render_entrypoint": "dist/server/new.js"}]}"#,
        )
        .unwrap();

        let fake = dir.path().join("fake-bun.sh");
        std::fs::write(&fake, "#!/bin/sh\nsleep 60\n").unwrap();
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&fake, perms).unwrap();

        let opts = SpawnOptions {
            workload: dir.path().to_path_buf(),
            gen_dir: gen_a,
            state_dir: dir.path().join(".mesofact-dev"),
            program: Some(fake),
        };
        let child = spawn(opts).await.unwrap().expect("ssr present");

        assert!(child.matches("/api/old"));
        assert!(!child.matches("/api/new"));

        child.restart_with(gen_b).await.unwrap();

        assert!(child.matches("/api/new"));
        assert!(!child.matches("/api/old"));
    }

    /// End-to-end verify item: a real bun child running the wrapper imports
    /// a Fetch entrypoint and returns its Response. Skipped when bun is not
    /// installed (CI without bun, foreign agents) so the rest of the suite
    /// stays universally runnable.
    #[tokio::test]
    async fn ssr_wrapper_serves_real_fetch_handler_via_bun() {
        if which_bun().is_none() {
            eprintln!("[skip] bun not on PATH — install via https://bun.sh to run this test");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        // Lay out a minimal gen dir: manifest.json + server/ping.js.
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "routes": [
                    {"route": "/api/ping", "mode": "ssr", "render_entrypoint": "dist/server/ping.js"}
                ]
            }"#,
        )
        .unwrap();
        let server_dir = dir.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(
            server_dir.join("ping.js"),
            "export default async function (_req) { return new Response('pong'); }\n",
        )
        .unwrap();

        let opts = SpawnOptions::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            dir.path().join(".mesofact-dev"),
        );
        let child = spawn(opts).await.unwrap().expect("ssr child spawned");

        // Wait for bun to bind. Cold-import of the wrapper + the route module
        // takes ~100-500ms on a warm machine.
        let url = format!("http://127.0.0.1:{}/api/ping", child.port());
        let client = reqwest::Client::new();
        let mut last_err: Option<String> = None;
        let mut body: Option<String> = None;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    body = Some(resp.text().await.unwrap_or_default());
                    break;
                }
                Ok(resp) => last_err = Some(format!("status {}", resp.status())),
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        assert_eq!(
            body.as_deref(),
            Some("pong"),
            "bun child never responded with pong (last error: {:?}, log: {:?})",
            last_err,
            child.log_buffer().lines().await,
        );
    }

    /// R434-B6 verify item: "Force a bun child import failure (typo in
    /// src/issues-submit.ts entrypoint) and confirm the error line appears
    /// on mesofact-dev's stdout, not silently in LogBuffer." Live half:
    /// drive a real bun child against a manifest pointing at a JS file that
    /// throws on import; assert the wrapper's `[mesofact-dev/ssr] import
    /// ... failed:` message lands in the LogBuffer (which is the same data
    /// the supervisor tees to `tracing::warn!` for the stdout surface).
    #[tokio::test]
    async fn bun_import_failure_surfaces_in_log_buffer() {
        if which_bun().is_none() {
            eprintln!("[skip] bun not on PATH — install via https://bun.sh to run this test");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "routes": [
                    {"route": "/api/broken", "mode": "ssr", "render_entrypoint": "dist/server/broken.js"}
                ]
            }"#,
        )
        .unwrap();
        let server_dir = dir.path().join("server");
        std::fs::create_dir_all(&server_dir).unwrap();
        // The module throws on import — same shape as a typo in a real
        // entrypoint that would reference a missing symbol.
        std::fs::write(
            server_dir.join("broken.js"),
            "throw new Error('synthetic import failure for R434-B6 verify');\n",
        )
        .unwrap();

        let opts = SpawnOptions::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            dir.path().join(".mesofact-dev"),
        );
        let child = spawn(opts).await.unwrap().expect("ssr child spawned");

        // Bun starts, the wrapper's `await import(path)` fails, and the
        // catch arm prints `[mesofact-dev/ssr] import .../broken.js failed:`.
        // Wait for it to show up.
        let mut surfaced = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let lines = child.log_buffer().lines().await;
            if lines.iter().any(|l| {
                l.contains("import") && l.contains("broken.js") && l.contains("failed")
            }) {
                surfaced = true;
                break;
            }
        }
        let lines = child.log_buffer().lines().await;
        assert!(
            surfaced,
            "expected wrapper import-failure line in LogBuffer; got: {:?}",
            lines,
        );
    }

    /// Verify item: port file persists under the state dir so other tools
    /// can discover it; SsrChild drop removes it.
    #[tokio::test]
    async fn spawn_persists_port_file_and_cleans_up_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{
                "routes": [{"route": "/api/x", "mode": "ssr", "render_entrypoint": "dist/server/x.js"}]
            }"#,
        )
        .unwrap();
        let fake = dir.path().join("fake-bun.sh");
        std::fs::write(&fake, "#!/bin/sh\nsleep 60\n").unwrap();
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&fake, perms).unwrap();

        let state_dir = dir.path().join(".mesofact-dev");
        let opts = SpawnOptions {
            workload: dir.path().to_path_buf(),
            gen_dir: dir.path().to_path_buf(),
            state_dir: state_dir.clone(),
            program: Some(fake),
        };
        let child = spawn(opts).await.unwrap().expect("ssr present");
        let port_file = state_dir.join(SSR_PORT_FILE);
        assert!(port_file.exists(), "port file should be written");
        let recorded: u16 = std::fs::read_to_string(&port_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(recorded, child.port());

        drop(child);
        // Drop is sync; the cleanup runs immediately.
        assert!(!port_file.exists(), "port file should be removed on drop");
    }
}
