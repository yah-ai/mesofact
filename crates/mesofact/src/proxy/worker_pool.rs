//! WorkerPool — manages N Bun render-pool workers.
//!
//! Responsibilities:
//! - Spawn N workers (default = `num_cpus / 2`, min 1) on a manifest.
//! - Watchdog task: ping every 30 s; kill + respawn on missed pong.
//! - Rolling reload: spawn parallel new pool, drain old.
//! - `get()`: returns any live worker (round-robin in P9+; index 0 in P7
//!   since Mode 2 is stubbed and no concurrent renders flow through).

use crate::proxy::metrics::Metrics;
use crate::proxy::worker_client::{WorkerClient, WorkerError};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

const PING_INTERVAL: Duration = Duration::from_secs(30);

pub struct WorkerPool {
    workers: Arc<RwLock<Vec<Arc<WorkerClient>>>>,
    worker_entry: PathBuf,
    manifest_path: PathBuf,
    /// `mesofact.config.toml` path passed to each worker for adapter
    /// registration (sqlite/r2 sources). `None` = no sources declared.
    config_path: Option<PathBuf>,
    #[allow(dead_code)]
    n: usize,
    /// Round-robin cursor for `get()` so concurrent Mode 2 renders spread
    /// across workers instead of all serializing on worker 0's socket mutex.
    next: AtomicUsize,
    /// Shared metrics registry (set via `attach_metrics`); the watchdog bumps
    /// the `restarting` gauge around a respawn. `None` in tests.
    metrics: Mutex<Option<Arc<Metrics>>>,
    _tmp: Arc<tempfile::TempDir>,
}

impl WorkerPool {
    /// Spawn `n` workers loading `manifest_path` and start the watchdog.
    pub async fn spawn(
        manifest_json: &[u8],
        worker_entry: PathBuf,
        n: usize,
    ) -> Result<Arc<Self>, WorkerError> {
        Self::spawn_with_config(manifest_json, worker_entry, n, None).await
    }

    /// Like `spawn`, but passes `config_path` to each worker so its render
    /// entrypoints can reach adapters declared in `mesofact.config.toml`.
    pub async fn spawn_with_config(
        manifest_json: &[u8],
        worker_entry: PathBuf,
        n: usize,
        config_path: Option<PathBuf>,
    ) -> Result<Arc<Self>, WorkerError> {
        let tmp = Arc::new(tempfile::tempdir().map_err(WorkerError::Io)?);
        let manifest_path = tmp.path().join("manifest.json");
        tokio::fs::write(&manifest_path, manifest_json)
            .await
            .map_err(WorkerError::Io)?;

        let mut workers = Vec::with_capacity(n);
        for i in 0..n {
            let sock = tmp.path().join(format!("worker-{i}.sock"));
            info!("spawning worker {i}");
            workers.push(Arc::new(
                WorkerClient::spawn(sock, &manifest_path, &worker_entry, config_path.as_deref())
                    .await?,
            ));
        }

        let pool = Arc::new(Self {
            workers: Arc::new(RwLock::new(workers)),
            worker_entry,
            manifest_path,
            config_path,
            n,
            next: AtomicUsize::new(0),
            metrics: Mutex::new(None),
            _tmp: tmp,
        });

        pool.clone().start_watchdog();
        Ok(pool)
    }

    /// Attach the shared metrics registry so respawns bump the `restarting`
    /// worker-pool gauge. Called once after both the pool and `AppState` exist.
    pub fn attach_metrics(&self, metrics: Arc<Metrics>) {
        *self.metrics.lock().unwrap() = Some(metrics);
    }

    /// Live worker count — the `ready` worker-pool gauge, read at scrape time.
    pub async fn live_count(&self) -> usize {
        self.workers.read().await.len()
    }

    fn metrics(&self) -> Option<Arc<Metrics>> {
        self.metrics.lock().unwrap().clone()
    }

    /// Return a live worker, round-robin across the pool. Per-worker I/O still
    /// serializes on that worker's socket mutex (see `worker_client` cleanup),
    /// so spreading requests across workers is what buys real concurrency.
    pub async fn get(&self) -> Option<Arc<WorkerClient>> {
        let workers = self.workers.read().await;
        if workers.is_empty() {
            return None;
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % workers.len();
        workers.get(i).cloned()
    }

    /// Drain all workers and wait for them to exit (used during rolling reload).
    pub async fn drain_all(self: Arc<Self>) {
        let workers = self.workers.read().await.clone();
        let mut joins = Vec::with_capacity(workers.len());
        for w in workers {
            joins.push(tokio::spawn(async move {
                if let Err(e) = w.drain().await {
                    warn!("drain error: {e}");
                }
                let _ = w.wait().await;
            }));
        }
        for j in joins {
            let _ = j.await;
        }
    }

    fn start_watchdog(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(PING_INTERVAL);
            ticker.tick().await; // skip immediate first tick
            loop {
                ticker.tick().await;
                self.watchdog_cycle().await;
            }
        });
    }

    async fn watchdog_cycle(&self) {
        let snapshot: Vec<Arc<WorkerClient>> = self.workers.read().await.clone();
        for (i, w) in snapshot.iter().enumerate() {
            match w.ping().await {
                Ok(()) => {}
                Err(WorkerError::PongTimeout | WorkerError::Closed | WorkerError::Io(_)) => {
                    warn!("worker {i} missed pong — respawning");
                    let _ = w.kill().await;
                    let metrics = self.metrics();
                    if let Some(m) = &metrics {
                        m.restarting_inc();
                    }
                    match self.respawn(i).await {
                        Ok(new_w) => {
                            let mut lock = self.workers.write().await;
                            if i < lock.len() {
                                lock[i] = Arc::new(new_w);
                            }
                            info!("worker {i} respawned");
                        }
                        Err(e) => error!("failed to respawn worker {i}: {e}"),
                    }
                    if let Some(m) = &metrics {
                        m.restarting_dec();
                    }
                }
                Err(e) => warn!("worker {i} ping error: {e}"),
            }
        }
    }

    async fn respawn(&self, idx: usize) -> Result<WorkerClient, WorkerError> {
        let sock = self
            ._tmp
            .path()
            .join(format!("worker-{idx}-r{}.sock", unix_now()));
        WorkerClient::spawn(sock, &self.manifest_path, &self.worker_entry, self.config_path.as_deref())
            .await
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Spawn a new pool from fresh manifest bytes and then drain the old pool.
/// The caller swaps the `Arc<WorkerPool>` reference atomically before draining.
pub async fn rolling_reload(
    old: Arc<WorkerPool>,
    manifest_json: &[u8],
    worker_entry: PathBuf,
    n: usize,
) -> Result<Arc<WorkerPool>, WorkerError> {
    let config_path = old.config_path.clone();
    let new_pool = WorkerPool::spawn_with_config(manifest_json, worker_entry, n, config_path).await?;
    tokio::spawn(old.drain_all());
    Ok(new_pool)
}
