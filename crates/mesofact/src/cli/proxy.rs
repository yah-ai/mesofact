//! mesofact-proxy binary — boot the axum proxy, start the worker pool,
//! and watch for manifest reloads via SIGHUP or the 30s heartbeat.

use axum::{
    routing::{any, get},
    Router,
};
use mesofact_core::proxy::cache::ResponseCache;
use mesofact_core::proxy::config::Config;
use mesofact_core::proxy::manifest_loader::{load_from_file, watch_manifest};
use mesofact_core::proxy::metrics::Metrics;
use mesofact_core::proxy::router::{handle, metrics_handler, AppState, SharedState};
use mesofact_core::proxy::session::SessionResolver;
use mesofact_core::proxy::source_gen::Generations;
use mesofact_core::proxy::worker_pool::{rolling_reload, WorkerPool};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{watch, RwLock};
use tracing::info;
use tracing_subscriber::EnvFilter;

pub async fn run(cfg: Config) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();


    // R749-T1: refuse before the pool spawns, on the RAW bytes rather than the
    // parsed manifest — `load_from_file` deserializes into `Manifest`, and
    // serde silently drops any key this build predates, which is precisely the
    // field a check needs to see.
    let support = cfg.policy_support();
    assert_declared_policy_is_enforced(&cfg.manifest, &support).await?;

    let manifest = Arc::new(load_from_file(&cfg.manifest).await?);
    info!(
        build_id = %manifest.build_id,
        routes = manifest.routes.len(),
        enforced = ?support.enforced().map(|p| p.field()).collect::<Vec<_>>(),
        delegated = ?support.delegated().map(|p| p.field()).collect::<Vec<_>>(),
        "manifest loaded"
    );

    let manifest_json = serde_json::to_vec(&*manifest)?;
    let pool = WorkerPool::spawn_with_config(
        &manifest_json,
        cfg.worker_entry.clone(),
        cfg.worker_count(),
        cfg.sources_config.clone(),
    )
    .await?;

    // Source generation provider (cache-key input 6).
    let generations = Arc::new(match &cfg.sources_config {
        Some(path) => Generations::from_config_file(path)?,
        None => Generations::empty(),
    });

    // Session resolver — built only when a secret env var is configured.
    let session = build_session_resolver(&cfg);

    // Shared metrics registry — the `/metrics` handler and the worker pool
    // (restarting gauge) both reference this one instance.
    let metrics = Arc::new(Metrics::new());
    pool.attach_metrics(metrics.clone());

    let mut app_state = AppState::new(
        manifest.clone(),
        pool,
        cfg.cdn_base_url.clone(),
        cfg.fallback_dir.clone(),
    )
    .with_cache(Arc::new(ResponseCache::with_capacity(cfg.cache_capacity)))
    .with_generations(generations)
    .with_login_url(cfg.login_url.clone())
    .with_metrics(metrics.clone());
    if let Some(resolver) = session {
        app_state = app_state.with_session(resolver);
    }
    let state: SharedState = Arc::new(RwLock::new(app_state));

    // Watch channel: manifest loader publishes new manifests; the reload task
    // rebuilds AppState (new matcher + new pool) atomically.
    let (tx, mut rx) = watch::channel(manifest.clone());
    watch_manifest(cfg.manifest.clone(), tx);

    // Reload task: when a new manifest arrives, spawn a new pool and swap state.
    {
        let state = state.clone();
        let worker_entry = cfg.worker_entry.clone();
        let n = cfg.worker_count();
        let metrics = metrics.clone();
        let manifest_path = cfg.manifest.clone();
        let support = support.clone();
        tokio::spawn(async move {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                // R749-T1: a hot reload can introduce a policy this process
                // does not enforce just as easily as a cold start can, and it
                // does so on a running server nobody is watching. Same refusal,
                // expressed the way a reload can express one — keep the old
                // manifest, which is the failure mode this loop already has for
                // a pool that will not spawn.
                if let Err(e) = assert_declared_policy_is_enforced(&manifest_path, &support).await {
                    tracing::error!("{e}; keeping the previous manifest");
                    continue;
                }
                let new_manifest = rx.borrow().clone();
                let json = match serde_json::to_vec(&*new_manifest) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!("failed to serialise new manifest: {e}");
                        continue;
                    }
                };
                let old_pool = state.read().await.pool.clone();
                match rolling_reload(old_pool, &json, worker_entry.clone(), n).await {
                    Ok(new_pool) => {
                        new_pool.attach_metrics(metrics.clone());
                        let mut st = state.write().await;
                        st.pool = new_pool;
                        st.manifest = new_manifest;
                        st.matcher = mesofact_core::proxy::router::build_matcher(&st.manifest);
                        drop(st);
                        info!("rolling reload complete");
                    }
                    Err(e) => {
                        tracing::error!("new pool failed to start, keeping old manifest: {e}");
                    }
                }
            }
        });
    }

    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/{*path}", any(handle))
        .route("/", any(handle))
        .with_state(state);

    let listener = TcpListener::bind(&cfg.bind).await?;
    info!(addr = %cfg.bind, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// R749-T1 — refuse a manifest declaring a policy this tier does not enforce.
///
/// The proxy's counterpart to `cli::serve`'s check of the same name; the tier's
/// own claim about what it implements lives on
/// [`Config::policy_support`](mesofact_core::proxy::config::Config::policy_support).
///
/// A manifest that is absent or unreadable is an error here, unlike in `serve`:
/// `--manifest` is a required argument, so a proxy that cannot read it is not
/// going to serve anything anyway, and "we could not parse it, therefore
/// nothing is declared" is the fail-open this whole mechanism is against.
async fn assert_declared_policy_is_enforced(
    manifest: &std::path::Path,
    support: &mesofact_core::PolicySupport,
) -> anyhow::Result<()> {
    let raw = tokio::fs::read(manifest).await.map_err(|e| {
        anyhow::anyhow!(
            "refusing to serve: cannot read {} to check for declared policy this binary does \
             not enforce: {e}",
            manifest.display(),
        )
    })?;
    mesofact_core::check_manifest(&raw, support).map_err(|e| anyhow::anyhow!("{e}"))
}

/// See [`mesofact_core::proxy::session::resolver_from_env`] — the same builder
/// `mesofact serve` uses for its SSR dispatch (R750-F2).
fn build_session_resolver(cfg: &Config) -> Option<Arc<dyn SessionResolver>> {
    mesofact_core::proxy::session::resolver_from_env(
        cfg.session_secret_env.as_deref(),
        &cfg.session_cookie,
    )
}
