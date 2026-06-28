//! File-watch + auto-rebuild loop for `mesofact-static` workloads.
//!
//! Watches `<workload>/src/` via [`notify`], debounces edits, runs the
//! workload's `build.command` (default `bun run build`), then snapshots
//! `<workload>/dist/` into `<workload>/.mesofact-dev/gen-<N>/` and flips a
//! [`DistPointer`](super::DistPointer) so the running server starts serving
//! the new artifact on the next request. Build stdout/stderr inherits the
//! parent's, so output shows up in the operator's terminal or the Run-tab
//! log surface.
//!
//! Atomicity story: each generation is a separate directory. `dist/` is
//! copied into `.mesofact-dev/gen-<N>-staging/` and then atomic-renamed to
//! `.mesofact-dev/gen-<N>/` on the same filesystem; the pointer flip is a
//! single `RwLock` write; in-flight reads keep using the `PathBuf` they
//! already cloned, so a request that started reading
//! `gen-<N-1>/html/index.html` doesn't observe a torn write. `dist/` itself
//! is left intact so concurrent publishers (mesofact-publisher to R2,
//! qed-run to pond/MinIO) can read it without racing the watcher. GC keeps
//! the last two generations.
//!
//! @yah:ticket(R255-S5, "Decide tier-1 object store: s3s-fs vs serve-off-disk vs self-mock")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-05-25T20:08:09Z)
//! @yah:kind(spike)
//! @yah:status(review)
//! @yah:parent(R255)
//! @yah:next("decide on the single fact: does the almanac/runtime fetch via S3 API calls or plain CDN GET? if S3, tier-1 needs s3s-fs regardless")
//! @yah:next("if s3s-fs: run the existing publish_to_local_sim unchanged against it so tier-1 becomes a strict subset of tier-2 (only diff: in-process s3s-fs vs containerized MinIO+Caddy+yubaba)")
//! @yah:next("reject self-mock: reimplementing SigV4 + bucket-policy + error shapes drifts from MinIO and defeats the point")
//! @yah:gotcha("license-check s3s before adopting — must be MIT/BSD/Apache-2.0/ISC (believed Apache-2.0)")
//! @yah:assumes("tier-1 read path is plain HTTP-GET (Caddy proxies the public bucket), so serve-off-disk is faithful for reads; only the build->PUT->read publish contract is unexercised at tier 1")
//! @yah:handoff("Spike closed: serve-off-disk wins for tier-1 (dev). Read path is plain HTTP GET via axum ServeDir — browsers never call S3 APIs. The build→PUT→serve publish contract is exercised at tier-2 (sim+MinIO, R256 T1–T5 in review). No s3s-fs needed at tier-1; self-mock rejected as before. The @yah:assumes fact was correct. Only a tier-2 concern: R256-F8 tracks the publish-to-MinIO watcher sink for hot-ish sim reload, which uses the real MinIO container (not s3s-fs).")
//! @yah:verify("No code change needed — tier-1 is already serve-off-disk. Verify the assumes by checking mesofact-dev routes: nothing in app/yah/web/src/ or mesofact-runtime makes S3 API calls client-side.")
//!
//! @yah:ticket(R256-F8, "Parameterize watcher sink: DistPointer (serve off disk) vs publish-to-MinIO")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-05-25T20:30:17Z)
//! @yah:status(review)
//! @yah:parent(R256)
//! @yah:next("introduce a sink abstraction so the same watch->rebuild loop can target either the in-process DistPointer (dev tier) or publish_to_local_sim (sim tier -> MinIO container)")
//! @yah:next("this is what gives the containerized sim a hot-ish reload (edit -> rebuild -> republish -> Caddy serves new artifact) WITHOUT putting mesofact-dev inside a container")
//! @yah:next("keep the host-side watcher as the only dev-mode component; the sim containers stay pure mesofact-core + Caddy + MinIO")
//! @yah:assumes("today the watcher's only sink is the in-process DistPointer (build_and_swap -> pointer.set at watcher.rs:253); there is no publish sink")
//! @arch:see(.yah/docs/working/mesofact-dev-camp-embedding.md)
//! @yah:handoff("PostBuildFn + PostBuildFuture type aliases added to watcher.rs. Watcher gains post_build: Option<PostBuildFn> field and with_post_build(self, f) builder. build_and_swap calls the hook after the pointer flip — failure logs a warning but does not fail the rebuild (in-process dev server stays healthy). No new dep on cloud: the hook is a generic async closure; camp.rs (which already depends on both mesofact-dev and cloud) will wire publish_to_local_sim into the closure. Two new tests: post_build_hook_receives_gen_dir_on_success + post_build_hook_failure_does_not_fail_rebuild. All 20 mesofact-dev tests pass; cargo check cloud+yah+desktop clean.")
//! @yah:verify("cargo test -p mesofact-dev --locked  # 20 passed")
//! @yah:verify("cargo check -p cloud -p yah -p desktop --locked")
//!
//! @arch:see(app/yah/cli/src/camp.rs)
//!

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher as _};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::DistPointer;

// ── Sink hook ─────────────────────────────────────────────────────────────────

/// Boxed future returned by a [`PostBuildFn`].
pub type PostBuildFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'static>>;

/// Optional post-build hook. Called with the snapshot directory (the `gen-N/`
/// directory, whose `html/` subdirectory is what the DistPointer points at)
/// after every successful build, immediately after the pointer flip.
///
/// Failure is logged as a warning but does not roll back the pointer flip —
/// the in-process server continues to serve the new snapshot; only the
/// secondary sink (e.g. MinIO publish for the sim tier) is stale.
///
/// Wired by camp for the sim tier: the closure calls
/// `cloud::publish_to_local_sim` so the Caddy+MinIO stack serves the updated
/// artifact without requiring a full container restart. The dev tier leaves
/// this `None` — the DistPointer alone is sufficient.
pub type PostBuildFn =
    Box<dyn Fn(std::path::PathBuf) -> PostBuildFuture + Send + Sync + 'static>;

const DEBOUNCE_MS: u64 = 200;
const GENS_TO_KEEP: usize = 2;
const STATE_DIR_NAME: &str = ".mesofact-dev";

/// Knobs for the watch loop.
#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// Directory to watch recursively for source edits.
    pub watch_dir: PathBuf,
    /// Shell command run via `sh -c` from the workload root.
    pub build_command: String,
    /// Directory the build writes into (the parent of `html/`). Relative
    /// paths resolve against the workload root.
    pub build_out_dir: PathBuf,
    /// Where generation snapshots live. Defaults to `<workload>/.mesofact-dev`.
    pub state_dir: PathBuf,
    /// Coalesce events landing within this window into one rebuild.
    pub debounce: Duration,
    /// Run an initial build at startup even if `dist/html/` already exists.
    pub initial_build: bool,
    /// Extra env vars injected into the build subprocess (R490-F7). Carries
    /// the dev S3 coords (`R2_ENDPOINT`, `R2_BUCKET`, dummy creds) so a
    /// workload's build-time `r2` reads resolve against mesofact-dev's local
    /// s3s-fs surface instead of real R2.
    pub build_env: Vec<(String, String)>,
}

impl WatchOptions {
    /// Best-effort defaults: watch `<workload>/src`, build with
    /// `bun run build`, output to `<workload>/dist`, snapshot under
    /// `<workload>/.mesofact-dev`. Reads `<workload>/workload.toml` if
    /// present to override `build.command` / `build.out_dir` (so the dev
    /// server agrees with the production reconciler).
    pub fn defaults_for_workload(workload: &Path) -> Self {
        let mut opts = Self {
            watch_dir: workload.join("src"),
            build_command: "bun run build".to_string(),
            build_out_dir: workload.join("dist"),
            state_dir: workload.join(STATE_DIR_NAME),
            debounce: Duration::from_millis(DEBOUNCE_MS),
            initial_build: true,
            build_env: Vec::new(),
        };
        if let Ok(text) = std::fs::read_to_string(workload.join("workload.toml")) {
            if let Ok(parsed) = toml::from_str::<WorkloadTomlPartial>(&text) {
                if let Some(build) = parsed.build {
                    if let Some(cmd) = build.command {
                        opts.build_command = cmd;
                    }
                    if let Some(out) = build.out_dir {
                        opts.build_out_dir = if out.is_absolute() {
                            out
                        } else {
                            workload.join(out)
                        };
                    }
                }
            }
        }
        opts
    }
}

#[derive(Deserialize, Default)]
struct WorkloadTomlPartial {
    build: Option<BuildPartial>,
}

#[derive(Deserialize, Default)]
struct BuildPartial {
    command: Option<String>,
    out_dir: Option<PathBuf>,
}

/// File-watch + rebuild loop. Construct with [`Watcher::new`] and drive with
/// [`Watcher::run`]; pair with a [`crate::Server`] sharing the same
/// [`DistPointer`].
///
/// For the sim tier, set a post-build publish hook with [`Watcher::with_post_build`]
/// so each successful rebuild also pushes the artifact to MinIO. The pointer
/// flip and the MinIO publish are independent: a failing publish logs a warning
/// but does not prevent the pointer from advancing.
pub struct Watcher {
    workload: PathBuf,
    pointer: DistPointer,
    options: WatchOptions,
    post_build: Option<PostBuildFn>,
}

impl Watcher {
    pub fn new(workload: impl Into<PathBuf>, pointer: DistPointer, options: WatchOptions) -> Self {
        Self {
            workload: workload.into(),
            pointer,
            options,
            post_build: None,
        }
    }

    /// Attach a post-build publish hook (sim tier).
    ///
    /// After every successful build the hook receives the snapshot directory
    /// (`gen-N/`, whose `html/` is what the pointer already points at). Camp
    /// uses this to call `cloud::publish_to_local_sim` so Caddy+MinIO stays
    /// in sync without a container restart.
    pub fn with_post_build(mut self, f: PostBuildFn) -> Self {
        self.post_build = Some(f);
        self
    }

    /// Drives the watch loop until the notify channel closes or the inner
    /// task is cancelled. Logs failures rather than exiting — a failed
    /// rebuild leaves the previous snapshot in place.
    pub async fn run(self) -> Result<()> {
        tokio::fs::create_dir_all(&self.options.state_dir)
            .await
            .with_context(|| {
                format!("creating state dir {}", self.options.state_dir.display())
            })?;

        // Pre-seed the gen counter from existing snapshot dirs so a restart
        // doesn't try to rename into a name that already exists.
        let mut next_gen = next_gen_number(&self.options.state_dir).await?;

        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();

        // Keep the watcher alive for the lifetime of this task.
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
            Ok(ev) => {
                let _ = event_tx.send(ev);
            }
            Err(e) => warn!(error = %e, "notify watcher error"),
        })?;

        if !self.options.watch_dir.exists() {
            warn!(
                dir = %self.options.watch_dir.display(),
                "watch dir missing — auto-rebuild disabled until it appears",
            );
        } else {
            watcher
                .watch(&self.options.watch_dir, RecursiveMode::Recursive)
                .with_context(|| {
                    format!("watching {}", self.options.watch_dir.display())
                })?;
            info!(dir = %self.options.watch_dir.display(), "watching for changes");
        }

        if self.options.initial_build {
            info!("running initial build");
            if let Err(e) = self.build_and_swap(&mut next_gen).await {
                error!(error = %e, "initial build failed; serving existing snapshot if any");
            }
        }

        // Debounce loop.
        loop {
            let Some(first) = event_rx.recv().await else {
                info!("watcher channel closed");
                break;
            };
            let mut actionable = is_actionable(&first);

            let deadline = tokio::time::Instant::now() + self.options.debounce;
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(remaining, event_rx.recv()).await {
                    Ok(Some(ev)) => {
                        actionable |= is_actionable(&ev);
                    }
                    Ok(None) => return Ok(()),
                    Err(_) => break,
                }
            }

            if !actionable {
                continue;
            }

            info!("source change detected; rebuilding");
            if let Err(e) = self.build_and_swap(&mut next_gen).await {
                error!(error = %e, "rebuild failed; keeping previous snapshot");
            }
        }

        Ok(())
    }

    /// One-shot rebuild — run the build command, snapshot, swap. Exposed
    /// for the reconciler to drive builds explicitly (R255-T3).
    pub async fn rebuild(&self) -> Result<PathBuf> {
        let mut next_gen = next_gen_number(&self.options.state_dir).await?;
        self.build_and_swap(&mut next_gen).await
    }

    async fn build_and_swap(&self, next_gen: &mut u64) -> Result<PathBuf> {
        let status = Command::new("sh")
            .arg("-c")
            .arg(&self.options.build_command)
            .current_dir(&self.workload)
            .envs(self.options.build_env.iter().cloned())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .with_context(|| format!("spawning build: {}", self.options.build_command))?;
        if !status.success() {
            anyhow::bail!("build exited with {}", status);
        }

        let html_src = self.options.build_out_dir.join("html");
        if !html_src.is_dir() {
            anyhow::bail!(
                "build did not produce {} (expected html/ under {})",
                html_src.display(),
                self.options.build_out_dir.display(),
            );
        }

        tokio::fs::create_dir_all(&self.options.state_dir)
            .await
            .with_context(|| format!("creating state dir {}", self.options.state_dir.display()))?;

        let n = *next_gen;
        *next_gen = n + 1;
        let gen_dir = self.options.state_dir.join(format!("gen-{n}"));
        if gen_dir.exists() {
            tokio::fs::remove_dir_all(&gen_dir).await.ok();
        }
        // Copy build_out_dir into a staging dir, then atomic-rename staging
        // → gen-N. Leaves build_out_dir intact for concurrent publishers
        // (mesofact-publisher to R2, qed-run to pond/MinIO) that read the
        // same dist/ tree. Atomicity of the gen-N flip is preserved by the
        // staging → gen-N rename, which is a single-fs rename of a directory
        // that no one else knows about yet.
        let staging = self.options.state_dir.join(format!("gen-{n}-staging"));
        if staging.exists() {
            tokio::fs::remove_dir_all(&staging).await.ok();
        }
        copy_dir_recursive(&self.options.build_out_dir, &staging)
            .await
            .with_context(|| {
                format!(
                    "copying {} -> {}",
                    self.options.build_out_dir.display(),
                    staging.display(),
                )
            })?;
        tokio::fs::rename(&staging, &gen_dir)
            .await
            .with_context(|| {
                format!(
                    "renaming {} -> {}",
                    staging.display(),
                    gen_dir.display(),
                )
            })?;

        let served = gen_dir.join("html");
        self.pointer.set(served.clone());
        info!(gen = n, served = %served.display(), "snapshot ready; pointer updated");

        // Optional post-build hook: publish to sim-tier object store (MinIO).
        // Failure is non-fatal — the pointer is already flipped and the
        // in-process server serves the new snapshot. A stale sim tier logs a
        // warning so operators notice without breaking the dev loop.
        if let Some(hook) = &self.post_build {
            if let Err(e) = hook(gen_dir.clone()).await {
                warn!(error = %e, gen = n, "post-build publish hook failed; sim tier may be stale");
            }
        }

        if let Err(e) = gc_generations(&self.options.state_dir, GENS_TO_KEEP).await {
            warn!(error = %e, "gc of old generations failed");
        }

        Ok(served)
    }
}

/// Spawn a watch loop on a background tokio task. Returns a handle that
/// stops the loop when dropped (the watcher's internal channel closes when
/// the task exits, which is fine for a process-lifetime dev server).
pub fn spawn(watcher: Watcher) -> WatcherHandle {
    let running = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&running);
    let join = tokio::spawn(async move {
        let result = watcher.run().await;
        flag.store(false, Ordering::SeqCst);
        if let Err(e) = result {
            error!(error = %e, "watcher exited with error");
        }
    });
    WatcherHandle { running, _join: join }
}

/// Handle that keeps a spawned watcher alive. Dropping it does not cancel
/// the task (tokio detaches on drop); use [`WatcherHandle::is_running`] to
/// observe lifecycle.
pub struct WatcherHandle {
    running: Arc<AtomicBool>,
    _join: tokio::task::JoinHandle<()>,
}

impl WatcherHandle {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

fn is_actionable(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dst)
        .await
        .with_context(|| format!("creating {}", dst.display()))?;
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((s, d)) = stack.pop() {
        let mut rd = tokio::fs::read_dir(&s)
            .await
            .with_context(|| format!("reading {}", s.display()))?;
        while let Some(ent) = rd.next_entry().await? {
            let ft = ent.file_type().await?;
            let from = ent.path();
            let to = d.join(ent.file_name());
            if ft.is_dir() {
                tokio::fs::create_dir_all(&to)
                    .await
                    .with_context(|| format!("creating {}", to.display()))?;
                stack.push((from, to));
            } else {
                tokio::fs::copy(&from, &to)
                    .await
                    .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
            }
        }
    }
    Ok(())
}

async fn next_gen_number(state_dir: &Path) -> Result<u64> {
    let mut max_seen: Option<u64> = None;
    let mut rd = match tokio::fs::read_dir(state_dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e).context("reading state dir for gen numbering"),
    };
    while let Some(ent) = rd.next_entry().await? {
        let name = ent.file_name();
        let s = name.to_string_lossy();
        if let Some(rest) = s.strip_prefix("gen-") {
            if let Ok(n) = rest.parse::<u64>() {
                max_seen = Some(max_seen.map_or(n, |m| m.max(n)));
            }
        }
    }
    Ok(max_seen.map(|m| m + 1).unwrap_or(0))
}

async fn gc_generations(state_dir: &Path, keep: usize) -> Result<()> {
    let mut entries: Vec<(u64, PathBuf)> = Vec::new();
    let mut rd = tokio::fs::read_dir(state_dir).await?;
    while let Some(ent) = rd.next_entry().await? {
        let s = ent.file_name();
        let s = s.to_string_lossy();
        if let Some(rest) = s.strip_prefix("gen-") {
            if let Ok(n) = rest.parse::<u64>() {
                entries.push((n, ent.path()));
            }
        }
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (_, path) in entries.into_iter().skip(keep) {
        if let Err(e) = tokio::fs::remove_dir_all(&path).await {
            warn!(error = %e, dir = %path.display(), "failed to gc snapshot");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    /// Build script: `mkdir -p dist/html && cat src/index.txt > dist/html/index.html`.
    /// Idempotent, no external deps; lets us simulate a real bun build.
    const FAKE_BUILD: &str = "mkdir -p dist/html && cp src/index.txt dist/html/index.html";

    #[tokio::test]
    async fn defaults_for_workload_reads_workload_toml() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("workload.toml"),
            r#"
kind = "mesofact-static"
[build]
command = "echo built"
out_dir = "outdir"
"#,
        );
        let opts = WatchOptions::defaults_for_workload(dir.path());
        assert_eq!(opts.build_command, "echo built");
        assert_eq!(opts.build_out_dir, dir.path().join("outdir"));
        assert_eq!(opts.watch_dir, dir.path().join("src"));
        assert_eq!(opts.state_dir, dir.path().join(".mesofact-dev"));
    }

    #[tokio::test]
    async fn defaults_for_workload_without_toml_uses_hardcoded_defaults() {
        let dir = tempdir().unwrap();
        let opts = WatchOptions::defaults_for_workload(dir.path());
        assert_eq!(opts.build_command, "bun run build");
        assert_eq!(opts.build_out_dir, dir.path().join("dist"));
    }

    #[tokio::test]
    async fn next_gen_number_seeds_from_existing_snapshots() {
        let dir = tempdir().unwrap();
        for n in [0u64, 3, 7] {
            std::fs::create_dir_all(dir.path().join(format!("gen-{n}"))).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("not-a-gen")).unwrap();
        let n = next_gen_number(dir.path()).await.unwrap();
        assert_eq!(n, 8);
    }

    #[tokio::test]
    async fn next_gen_number_returns_zero_for_missing_dir() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert_eq!(next_gen_number(&missing).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn gc_keeps_last_n_generations() {
        let dir = tempdir().unwrap();
        for n in 0..5u64 {
            std::fs::create_dir_all(dir.path().join(format!("gen-{n}"))).unwrap();
        }
        gc_generations(dir.path(), 2).await.unwrap();
        assert!(dir.path().join("gen-4").is_dir());
        assert!(dir.path().join("gen-3").is_dir());
        assert!(!dir.path().join("gen-2").exists());
        assert!(!dir.path().join("gen-0").exists());
    }

    #[tokio::test]
    async fn rebuild_runs_build_command_and_swaps_pointer() {
        let workload = tempdir().unwrap();
        write(&workload.path().join("src/index.txt"), "<h1>A</h1>");

        let pointer = DistPointer::new(workload.path().join("dist").join("html"));
        let mut opts = WatchOptions::defaults_for_workload(workload.path());
        opts.build_command = FAKE_BUILD.to_string();
        let watcher = Watcher::new(workload.path(), pointer.clone(), opts);

        let served = watcher.rebuild().await.unwrap();
        assert!(served.starts_with(workload.path().join(".mesofact-dev")));
        assert_eq!(pointer.current(), served);
        let body = std::fs::read_to_string(served.join("index.html")).unwrap();
        assert!(body.contains("<h1>A</h1>"));

        // dist/ must remain intact for concurrent publishers (R492-B1).
        let dist_html = workload.path().join("dist").join("html").join("index.html");
        let dist_body = std::fs::read_to_string(&dist_html)
            .expect("dist/html/index.html should still exist after rebuild");
        assert_eq!(dist_body, body, "dist/ and gen-N/ must hold the same bytes");

        // Staging dir must be cleaned up by the rename.
        let staging = workload.path().join(".mesofact-dev").join("gen-0-staging");
        assert!(!staging.exists(), "staging dir should be gone after rename");
    }

    #[tokio::test]
    async fn post_build_hook_receives_gen_dir_on_success() {
        use std::sync::{Arc, Mutex};

        let workload = tempdir().unwrap();
        write(&workload.path().join("src/index.txt"), "<h1>hook</h1>");

        let pointer = DistPointer::new(workload.path().join("dist").join("html"));
        let mut opts = WatchOptions::defaults_for_workload(workload.path());
        opts.build_command = FAKE_BUILD.to_string();

        let received: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(vec![]));
        let recv2 = Arc::clone(&received);
        let hook: PostBuildFn = Box::new(move |gen_dir: PathBuf| {
            let inner = Arc::clone(&recv2);
            Box::pin(async move {
                inner.lock().unwrap().push(gen_dir);
                Ok(())
            })
        });

        let watcher = Watcher::new(workload.path(), pointer.clone(), opts)
            .with_post_build(hook);

        watcher.rebuild().await.unwrap();

        let calls = received.lock().unwrap();
        assert_eq!(calls.len(), 1, "hook should be called once per rebuild");
        // The hook receives the gen-N directory (parent of html/).
        assert!(calls[0].ends_with("gen-0"), "hook receives gen dir, got {:?}", calls[0]);
        // pointer points at gen-0/html.
        assert_eq!(pointer.current(), calls[0].join("html"));
    }

    #[tokio::test]
    async fn post_build_hook_failure_does_not_fail_rebuild() {
        let workload = tempdir().unwrap();
        write(&workload.path().join("src/index.txt"), "<h1>hook-err</h1>");

        let pointer = DistPointer::new(workload.path().join("dist").join("html"));
        let mut opts = WatchOptions::defaults_for_workload(workload.path());
        opts.build_command = FAKE_BUILD.to_string();

        let hook: PostBuildFn = Box::new(move |_gen_dir: PathBuf| {
            Box::pin(async move { anyhow::bail!("publish failed (test)") })
        });

        let watcher = Watcher::new(workload.path(), pointer.clone(), opts)
            .with_post_build(hook);

        // rebuild() should succeed even though the hook fails.
        let served = watcher.rebuild().await.unwrap();
        // Pointer still flipped.
        assert_eq!(pointer.current(), served);
    }

    #[tokio::test]
    async fn rebuild_fails_when_html_dir_missing() {
        let workload = tempdir().unwrap();
        write(&workload.path().join("src/index.txt"), "x");

        let pointer = DistPointer::new(workload.path().join("dist").join("html"));
        let mut opts = WatchOptions::defaults_for_workload(workload.path());
        // Build "succeeds" but writes the wrong place (dist/, not dist/html/).
        opts.build_command = "mkdir -p dist && echo x > dist/marker".to_string();
        let watcher = Watcher::new(workload.path(), pointer.clone(), opts);

        let err = watcher.rebuild().await.unwrap_err();
        assert!(err.to_string().contains("html/"));
        // Pointer unchanged because rebuild bailed before set().
        assert_eq!(pointer.current(), workload.path().join("dist").join("html"));
    }

    #[tokio::test]
    async fn rebuild_fails_when_build_exits_nonzero() {
        let workload = tempdir().unwrap();
        write(&workload.path().join("src/index.txt"), "x");

        let pointer = DistPointer::new(workload.path().join("dist").join("html"));
        let mut opts = WatchOptions::defaults_for_workload(workload.path());
        opts.build_command = "exit 7".to_string();
        let watcher = Watcher::new(workload.path(), pointer.clone(), opts);

        let err = watcher.rebuild().await.unwrap_err();
        assert!(err.to_string().contains("exited"));
    }

    #[tokio::test]
    async fn watcher_run_rebuilds_on_file_change() {
        let workload = tempdir().unwrap();
        write(&workload.path().join("src/index.txt"), "<h1>A</h1>");

        let pointer = DistPointer::new(workload.path().join("dist").join("html"));
        let mut opts = WatchOptions::defaults_for_workload(workload.path());
        opts.build_command = FAKE_BUILD.to_string();
        opts.debounce = Duration::from_millis(50);
        let watcher = Watcher::new(workload.path(), pointer.clone(), opts);

        let task = tokio::spawn(async move { watcher.run().await });

        // Wait for the initial build to flip the pointer off the original
        // dist/html path.
        let initial_path = workload.path().join("dist").join("html");
        for _ in 0..40 {
            if pointer.current() != initial_path {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let first = pointer.current();
        assert_ne!(first, initial_path, "initial build did not swap pointer");
        let body = std::fs::read_to_string(first.join("index.html")).unwrap();
        assert!(body.contains("<h1>A</h1>"));

        // Edit the source. Add a tiny sleep to let notify register the
        // watch before the modification.
        tokio::time::sleep(Duration::from_millis(100)).await;
        write(&workload.path().join("src/index.txt"), "<h1>B</h1>");

        // Wait for pointer to advance again.
        for _ in 0..60 {
            if pointer.current() != first {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let second = pointer.current();
        assert_ne!(second, first, "file edit did not trigger rebuild");
        let body = std::fs::read_to_string(second.join("index.html")).unwrap();
        assert!(body.contains("<h1>B</h1>"));

        task.abort();
    }
}
