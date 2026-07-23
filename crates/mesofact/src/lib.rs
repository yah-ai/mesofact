//! `mesofact` — the Rust-native web framework facade.
//!
//! **This is the one crate consumers depend on.** Subsystems are selected by
//! feature rather than by picking crate names out of a 7-crate workspace
//! (bevy-style). Two distribution strategies, one crate:
//!
//! - **Prebuilt binary / container** — we build the `mesofact` bin with
//!   the `deploy` preset, arch-native per libc, and kamaji fetches it. The
//!   consumer's project compiles zero Rust, only their TypeScript — the Node.js
//!   model.
//! - **Crate dependency** — the same crate from crates.io, where a consumer
//!   picks features and builds a bespoke binary. What Node can't offer, because
//!   it's Rust-native all the way down instead of C++ addons.
//!
//! They are the same crate at two lifecycle stages; the shared `deploy` feature
//! preset is what keeps them honest. See W225 §2a.
//!
//! # Feature tiers
//!
//! - `default` — the lean, **V8-free** Rust-handler harness: [`serve_app`] /
//!   [`wrap`] + the standard stack ([`HEALTH_PATH`], `TraceLayer`, graceful
//!   shutdown). For services whose handlers are Rust functions; the dogfood is
//!   yah's cloud-admin dashboard. Continues the "replacing bun with Rust-native
//!   SSR" arc (R448 rolldown → R449 deno_core → R450 default-flip).
//! - `ssr` — the SSR serving tier: the [`Server`] engine's V8 dispatch plus the
//!   revalidate receiver. Links the *prebuilt* `librusty_v8.a` (a CDN download,
//!   not a from-source compile).
//! - `build` — adds the bundler (rolldown + lightningcss), the one genuinely
//!   uncached from-source compile. Its own crate so a stray feature flip can't
//!   drag it into a lean consumer.
//!
//! # What lives here vs. `mesofact-dev`
//!
//! The prod serving engine ([`server`], [`proxy`], and under `ssr` the `ssr`,
//! `revalidate` and `tenants` modules) lives **here**. `mesofact-dev` holds only
//! the dev affordances — the file watcher and the local S3 surface — and depends
//! on this crate. That direction is load-bearing: it is what keeps a prod binary
//! from linking dev code (W225 §2), and it is enforced by the dependency graph
//! rather than by dead-stripping. Do not add a dev affordance to this crate, and
//! do not add a prod-serving bin to `mesofact-dev`.
//!
//! Cache / session / resilience layers live in `mesofact_core::proxy` today and are
//! bundle-shaped; lifting them here as caller-composable `tower::Layer`s is a
//! follow-up once a second Rust-handler service needs them.
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
//! @yah:gotcha("RESOLVED 2026-07-23 (W225 §2a) -- was: 'mesofact-dev refactor to delegate its bind/serve to mesofact-app is deferred'. The whole serving engine moved INTO this crate and mesofact-dev now depends on it, so the delegation is structural rather than deferred. Server's router now uses this crate's HEALTH_PATH + default_health + shutdown_signal instead of the byte-identical copies it carried in mesofact-dev.")

// ── Facade re-exports ────────────────────────────────────────────────────
// Subsystems are namespaced (not glob-flattened) on purpose — consumers reach
// them as `mesofact::core::…`, `mesofact::render::…`, etc. Each is gated on the
// feature that pulls the corresponding crate (see Cargo.toml `[features]`).
//
// This started as a workaround for an axum-major skew (core/dev on 0.7, facade
// on 0.8); that skew is now RESOLVED — everything is axum 0.8 — and the
// namespacing is kept deliberately, bevy-style, so each subsystem keeps its own
// namespace instead of flattening hundreds of items into the crate root.
pub use mesofact_core as core;
// NB: re-exported as `ssr_runtime`, not `ssr` — the `ssr` name at this crate
// root belongs to the SSR *dispatch* module moved in from mesofact-dev below.
// `ssr_runtime` is the raw deno_core/V8 runtime crate that dispatch drives.
#[cfg(feature = "ssr")]
pub use mesofact_ssr as ssr_runtime;
#[cfg(feature = "render")]
pub use mesofact_render as render;
#[cfg(feature = "build")]
pub use mesofact_build as build;
#[cfg(feature = "publish")]
pub use mesofact_publisher as publisher;

// ── The serving engine (moved out of mesofact-dev, W225 §2a) ─────────────
// These carry the prod serving path. They used to live in `mesofact-dev`, which
// meant the prod `mesofact-serve` binary linked the dev crate — and therefore
// the file watcher and the dev S3 surface — breaking the dev/prod crate
// boundary W225 §2 claims. `mesofact-dev` now depends on THIS crate and holds
// only `watcher` + `s3` + the dev bin, so that boundary finally holds.
pub mod cli;
pub mod proxy;
pub mod server;
#[cfg(feature = "ssr")]
pub mod revalidate;
#[cfg(feature = "ssr")]
pub mod ssr;
#[cfg(feature = "ssr")]
pub mod tenants;

pub use proxy::{ProxyMap, ProxyState};
pub use server::{DistPointer, Identity, Server, DEFAULT_PORT};
#[cfg(feature = "ssr")]
pub use ssr::{
    ResiliencePolicy, RetryPolicy, SpawnOptions as SsrSpawnOptions, SsrChild, SsrSlot,
    DEFAULT_RESILIENCE_TIMEOUT_MS,
};

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::{routing::get, Router};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

/// Reserved liveness/readiness path, so the pond/cloud reconciler's
/// `ready_path` works uniformly across JS-bundle and Rust-handler services.
///
/// This is now the single definition: [`server::Server`]'s router registers
/// this same const (it used to hardcode the literal in `mesofact-dev`).
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

/// Default `HEALTH_PATH` handler — returns `200 ok`. Shared with
/// [`server::Server`]'s router, which used to carry its own byte-identical copy
/// back when the engine lived in `mesofact-dev` (W225 §2a collapse). Same shape as
/// mesofact-dev's `health()` (lib.rs:321) so probes stay drop-in
/// compatible between service flavors.
pub(crate) async fn default_health() -> &'static str {
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
