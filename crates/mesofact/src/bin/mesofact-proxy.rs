//! mesofact-proxy binary — boot the axum proxy, start the worker pool,
//! and watch for manifest reloads via SIGHUP or the 30s heartbeat.

use axum::{
    routing::{any, get},
    Router,
};
use clap::Parser;
use mesofact::proxy::cache::ResponseCache;
use mesofact::proxy::config::Config;
use mesofact::proxy::manifest_loader::{load_from_file, watch_manifest};
use mesofact::proxy::metrics::Metrics;
use mesofact::proxy::router::{handle, metrics_handler, AppState, SharedState};
use mesofact::proxy::session::{CookieSessionResolver, SessionResolver};
use mesofact::proxy::source_gen::Generations;
use mesofact::proxy::worker_pool::WorkerPool;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{watch, RwLock};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cfg = Config::parse();

    let manifest = Arc::new(load_from_file(&cfg.manifest).await?);
    info!(
        build_id = %manifest.build_id,
        routes = manifest.routes.len(),
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
        let sources_config = cfg.sources_config.clone();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let new_manifest = rx.borrow().clone();
                let json = match serde_json::to_vec(&*new_manifest) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!("failed to serialise new manifest: {e}");
                        continue;
                    }
                };
                match WorkerPool::spawn_with_config(&json, worker_entry.clone(), n, sources_config.clone()).await {
                    Ok(new_pool) => {
                        new_pool.attach_metrics(metrics.clone());
                        let mut st = state.write().await;
                        let old_pool = std::mem::replace(
                            &mut st.pool,
                            new_pool.clone(),
                        );
                        st.manifest = new_manifest;
                        st.matcher = mesofact::proxy::router::build_matcher(&st.manifest);
                        drop(st);
                        tokio::spawn(old_pool.drain_all());
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
        .route("/*path", any(handle))
        .route("/", any(handle))
        .with_state(state);

    let listener = TcpListener::bind(&cfg.bind).await?;
    info!(addr = %cfg.bind, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build a `CookieSessionResolver` when `--session-secret-env` names a set env
/// var. A configured-but-unset/empty env var is a deploy error: warn and run
/// without sessions rather than crash (the route-level `requires` check still
/// redirects/401s, so this fails safe — not open).
fn build_session_resolver(cfg: &Config) -> Option<Arc<dyn SessionResolver>> {
    let env_name = cfg.session_secret_env.as_ref()?;
    match std::env::var(env_name) {
        Ok(secret) if !secret.is_empty() => {
            info!(cookie = %cfg.session_cookie, "session resolver enabled");
            Some(Arc::new(CookieSessionResolver::new(
                cfg.session_cookie.clone(),
                secret.into_bytes(),
            )))
        }
        _ => {
            warn!(
                env = %env_name,
                "session secret env var is unset/empty — sessions disabled"
            );
            None
        }
    }
}
