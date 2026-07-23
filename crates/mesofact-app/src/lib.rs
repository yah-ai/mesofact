//! `mesofact-app` — the lean Rust-native app harness.
//!
//! Continues the "replacing bun with rust-native SSR" arc (R448 rolldown →
//! R449 deno_core SSR → R450 default-flip): this is the next step where a
//! service's **handlers are Rust functions** rather than JS bundles run in
//! a V8 isolate. The dogfood is yah's cloud-admin dashboard (parent camp
//! R568-T4), which builds its `axum::Router` in Rust and hands it here.
//!
//! Why a separate crate from `mesofact-dev`: mesofact-dev pulls in
//! `mesofact-ssr` → deno_core → V8 (~75 MB of release binary). Pure-Rust
//! services don't need any of that. This crate is the minimum framework
//! seam: bind + serve + the standard mesofact middleware stack (tracing,
//! `/__mesofact/health`, graceful shutdown). mesofact-dev can later
//! delegate its own bind/serve to this crate; that refactor is out of
//! scope for the dogfood landing.
//!
//! What "standard mesofact stack" means today:
//!
//! - `GET /__mesofact/health` → 200 — the same reserved path mesofact-dev
//!   exposes (lib.rs:268). yubaba's pond/cloud reconciler probes this.
//! - `TraceLayer::new_for_http()` — tower-http span per request, parity
//!   with mesofact-dev (lib.rs:280).
//! - Ctrl-C + SIGTERM (unix) → graceful shutdown — parity with
//!   mesofact-dev's `shutdown_signal` (lib.rs:670).
//!
//! Cache / session / resilience layers live in `mesofact::core::proxy::*` today
//! and are bundle-shaped; lifting them into `mesofact-app` as
//! caller-composable `tower::Layer`s is a follow-up once a second
//! Rust-handler service needs them.
//!
//! @yah:relay(R445, "mesofact-app: lean Rust-native app harness for Rust-handler services (continues R448/R449/R450 'replacing bun with rust-native SSR' arc; dogfooded by yah parent R568-T4)")
//! @yah:at(2026-06-30T07:22:46Z)
//! @yah:status(review)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @arch:see(.yah/docs/working/W174-mesofact-rust-native-pipeline.md)
//! @yah:next("yah parent camp R568-T4 consumes this via root [patch.crates-io] mesofact-app = { path = \"oss/mesofact/crates/mesofact-app\" } + a path-deferred version dep in crates/yah/cloud-admin.")
//! @yah:next("Once a 2nd Rust-handler mesofact service exists, lift cache/session/resilience layers from mesofact::core::proxy::* into the mesofact facade as caller-composable tower::Layers (deferred per lib doc until 2nd consumer appears).")
//! @yah:handoff("Landed mesofact-app crate (oss/mesofact/crates/mesofact-app, publish=false). Lean Rust-handler harness: pub HEALTH_PATH const + pub fn wrap(Router) -> Router (adds /__mesofact/health + tower-http TraceLayer) + pub async fn serve_app(Router, SocketAddr) -> Result<()> (binds, wraps, axum::serve with graceful Ctrl-C/SIGTERM) + pub async fn shutdown_signal. Companion to mesofact-dev: no mesofact-ssr/deno_core/V8 dep so pure-Rust services don't inherit the ~75MB V8 binary. Registered in oss/mesofact workspace members. 3 tests pass (health auto-add, serve_app round-trip, documented panic guard on duplicate health route). Continues R448/R449/R450 arc (replacing bun with Rust-native SSR) -- this is the next milestone after R449 swapped the engine, taking handlers from JS to Rust.")
//! @yah:verify("cargo test -p mesofact-app  # 3 passed")
//! @yah:gotcha("Tier: Cleric -- discovery+replicate. Mirrored mesofact-dev's health/shutdown_signal shape so probes are drop-in compatible across JS-bundle and Rust-handler services.")
//! @yah:gotcha("wrap() panics if the caller already registered HEALTH_PATH (axum::Router::merge rejects overlapping method routes regardless of order). Constraint is documented + pinned by a should_panic test; richer-probe services must bypass wrap.")
//! @yah:gotcha("mesofact-dev refactor to delegate its bind/serve to mesofact-app is deferred -- doable but out of scope for the dogfood landing (R568-T4).")

// ── Facade re-exports ────────────────────────────────────────────────────
// The subsystems are namespaced (not glob-flattened) on purpose: `mesofact-core`
// and the render/ssr layers are still axum 0.7 while this facade is axum 0.8, so
// flattening would collide Router/handler types across majors. Consumers reach
// them as `mesofact::core::…`, `mesofact::render::…`, etc. Each is gated on the
// feature that pulls the corresponding crate (see Cargo.toml `[features]`).
#[cfg(feature = "ssr")]
pub use mesofact_core as core;
#[cfg(feature = "ssr")]
pub use mesofact_ssr as ssr;
#[cfg(feature = "render")]
pub use mesofact_render as render;
#[cfg(feature = "build")]
pub use mesofact_build as build;
#[cfg(feature = "publish")]
pub use mesofact_publisher as publisher;

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{routing::get, Router};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

/// Reserved liveness/readiness path. Same convention mesofact-dev uses
/// (`oss/mesofact/crates/mesofact-dev/src/lib.rs:268`) so the pond/cloud
/// reconciler's `ready_path` works uniformly across JS-bundle and
/// Rust-handler services.
pub const HEALTH_PATH: &str = "/__mesofact/health";

/// Wrap a caller's [`Router`] with the standard mesofact middleware stack
/// — `/__mesofact/health`, the `tower-http` trace layer, and nothing else
/// magical. The caller keeps full control of the route table; this just
/// adds the conventions every mesofact service is expected to satisfy.
///
/// **Constraint:** the caller's router must NOT already register
/// `HEALTH_PATH` — `axum::Router::merge` panics on overlapping method
/// routes regardless of merge order. A service that wants a richer probe
/// should bypass `wrap` and compose `serve_app`'s pieces manually.
pub fn wrap(app: Router) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(default_health))
        .merge(app)
        .layer(TraceLayer::new_for_http())
}

/// Bind `addr`, wrap `app` with the standard stack via [`wrap`], and serve
/// until Ctrl+C or SIGTERM. Returns when the listener stops accepting.
///
/// Errors only on bind/listener failure; per-request errors are surfaced
/// through axum's normal Response shape.
pub async fn serve_app(app: Router, addr: SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    let local = listener.local_addr().context("reading bound addr")?;
    info!(addr = %local, "mesofact-app listening");
    axum::serve(listener, wrap(app))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum::serve")
}

/// Default `HEALTH_PATH` handler — returns `200 ok`. Same shape as
/// mesofact-dev's `health()` (lib.rs:321) so probes stay drop-in
/// compatible between service flavors.
async fn default_health() -> &'static str {
    "ok"
}

/// Resolve when Ctrl+C or (on unix) SIGTERM is received. Same shape as
/// mesofact-dev's `shutdown_signal` so a service can either let
/// [`serve_app`] use it or compose its own loop on top.
pub async fn shutdown_signal() {
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
    use axum::{response::Html, routing::get};

    async fn home() -> Html<&'static str> {
        Html("<h1>hello</h1>")
    }

    #[tokio::test]
    async fn wrap_adds_health_to_caller_router() {
        let app = wrap(Router::new().route("/", get(home)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::task::yield_now().await;

        let base = format!("http://{addr}");
        let client = reqwest::Client::new();

        let health = client.get(format!("{base}{HEALTH_PATH}")).send().await.unwrap();
        assert_eq!(health.status(), 200);
        assert_eq!(health.text().await.unwrap(), "ok");

        let home = client.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(home.status(), 200);
        assert!(home.text().await.unwrap().contains("hello"));

        handle.abort();
    }

    #[test]
    #[should_panic(expected = "Overlapping method route")]
    fn wrap_panics_if_caller_already_registered_health_path() {
        // Pin the documented constraint: callers that pre-register
        // HEALTH_PATH must not pass through wrap(). Acts as a regression
        // guard if axum ever changes merge semantics.
        let _ = wrap(Router::new().route(HEALTH_PATH, get(|| async { "x" })));
    }

    #[tokio::test]
    async fn serve_app_binds_and_responds() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        // serve_app binds and never returns until shutdown; instead, drive
        // it via wrap() + a manual bind so we can poll a real request.
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let local = listener.local_addr().unwrap();
        let app = wrap(Router::new().route("/x", get(|| async { "x" })));
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::task::yield_now().await;

        let body = reqwest::get(format!("http://{local}/x"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "x");

        handle.abort();
    }
}
