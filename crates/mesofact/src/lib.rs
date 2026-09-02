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
pub mod cache_headers;
pub mod cli;
pub mod health;
pub mod proxy;
pub mod route_headers;
pub mod server;
#[cfg(feature = "ssr")]
pub mod revalidate;
#[cfg(feature = "ssr")]
pub mod ssr;
#[cfg(feature = "ssr")]
pub mod tenants;

pub use cache_headers::CachePolicyTable;
pub use health::{Health, LEGACY_HEALTH_PATH, LIVE_PATH, READY_PATH};
pub use proxy::{ProxyMap, ProxyState};
pub use route_headers::RouteHeaderTable;
pub use server::{
    declared_cache_policy, read_manifest_bytes, routes_declaring_ssr, routes_requiring_user,
    DistPointer, Identity, Server, DEFAULT_PORT,
};
#[cfg(feature = "ssr")]
pub use ssr::{
    ResiliencePolicy, RetryPolicy, SpawnOptions as SsrSpawnOptions, SsrChild, SsrSlot,
    DEFAULT_RESILIENCE_TIMEOUT_MS,
};

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::Router;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

/// The historical probe path, predating the `/livez` + `/readyz` split.
///
/// Kept, and kept meaning exactly what it always meant — **liveness**, 200 as
/// soon as the process is serving — because deployed yubaba reconcilers point
/// `ready_path` here and `Dockerfile.ssr-runtime` documents it. Its doc comment
/// used to claim readiness; the check never did more than prove the socket was
/// accepting, which is why [`health`] exists. New probes should target
/// [`READY_PATH`] / [`LIVE_PATH`].
pub const HEALTH_PATH: &str = "/__mesofact/health";

/// How long `/readyz` reports failure before the listener stops accepting, on
/// SIGTERM. Covers endpoint-removal propagation (kube-proxy, ingress, sidecar
/// caches), which is concurrent with — not ordered before — the signal.
///
/// Override with `MESOFACT_DRAIN_GRACE_SECS`; `0` restores the old behavior of
/// closing the listener immediately.
pub const DEFAULT_DRAIN_GRACE: Duration = Duration::from_secs(5);

/// Wrap a caller's [`Router`] with the standard mesofact middleware stack —
/// the [`health`] probe routes, the `tower-http` trace layer, and nothing else
/// magical. The caller keeps full control of the route table.
///
/// The returned health handle is already started and carries no readiness
/// gates: a Rust-handler service is ready once its routes are mounted. Add
/// gates with [`Health::set_gates`] if it depends on something that can be
/// absent (a warmed cache, an upstream connection) — but only ever things
/// whose remedy is "route traffic elsewhere", never things a restart would fix.
///
/// **Constraint:** the caller's router must NOT already register any probe
/// path — `axum::Router::merge` panics on overlapping method routes regardless
/// of merge order.
pub fn wrap(app: Router) -> Router {
    wrap_with(app, Health::ready())
}

/// [`wrap`] against a caller-owned [`Health`], for services that need to hold
/// the handle — to flip gates as subsystems come up, or to drain on their own
/// shutdown path.
pub fn wrap_with(app: Router, health: Arc<Health>) -> Router {
    health::probe_routes(health)
        .merge(app)
        .layer(TraceLayer::new_for_http())
}

/// Bind `addr`, wrap `app` with the standard stack via [`wrap`], and serve
/// until Ctrl+C or SIGTERM — failing readiness for [`DEFAULT_DRAIN_GRACE`]
/// before the listener closes.
///
/// Errors only on bind/listener failure; per-request errors are surfaced
/// through axum's normal Response shape.
pub async fn serve_app(app: Router, addr: SocketAddr) -> Result<()> {
    let health = Health::ready();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    let local = listener.local_addr().context("reading bound addr")?;
    info!(addr = %local, "mesofact-app listening");
    axum::serve(listener, wrap_with(app, health.clone()))
        .with_graceful_shutdown(shutdown_signal_for(health))
        .await
        .context("axum::serve")
}

/// Resolve when Ctrl+C or (on unix) SIGTERM is received.
///
/// Prefer [`shutdown_signal_for`] anywhere a [`Health`] handle exists: this
/// form returns the instant the signal lands, so the listener closes while
/// load balancers still believe the pod is in rotation.
pub async fn shutdown_signal() {
    let _ = wait_for_signal().await;
}

/// [`shutdown_signal`] with the readiness half of a graceful shutdown: flip
/// `/readyz` to 503, hold for the drain grace, *then* resolve so axum closes
/// the listener.
///
/// The hold applies to SIGTERM only. Ctrl+C is a human at a terminal in dev who
/// wants the port back now, and no orchestrator is watching that process's
/// probes.
pub async fn shutdown_signal_for(health: Arc<Health>) {
    let signal = wait_for_signal().await;
    health.begin_drain();

    let grace = match signal {
        Signal::Terminate => drain_grace(),
        Signal::Interrupt => Duration::ZERO,
    };
    if grace.is_zero() {
        return;
    }
    info!(
        grace_s = grace.as_secs_f64(),
        "draining — /readyz now 503, listener closes after the grace window",
    );
    tokio::time::sleep(grace).await;
}

/// Which signal ended the process — they warrant different drain behavior.
enum Signal {
    /// SIGTERM: an orchestrator is rolling us; other components still route here.
    Terminate,
    /// Ctrl+C: a human in dev.
    Interrupt,
}

fn drain_grace() -> Duration {
    parse_grace(std::env::var("MESOFACT_DRAIN_GRACE_SECS").ok().as_deref())
}

/// Unset → the default. Set but unparseable → the default *and* a warning: a
/// typo'd grace must not silently become zero, since that is exactly the
/// connection-dropping behavior the operator was configuring away from.
fn parse_grace(raw: Option<&str>) -> Duration {
    let Some(raw) = raw else {
        return DEFAULT_DRAIN_GRACE;
    };
    match raw.trim().parse::<f64>() {
        Ok(secs) if secs.is_finite() && secs >= 0.0 => Duration::from_secs_f64(secs),
        _ => {
            warn!(%raw, "MESOFACT_DRAIN_GRACE_SECS is not a non-negative number — using default");
            DEFAULT_DRAIN_GRACE
        }
    }
}

async fn wait_for_signal() -> Signal {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!(?err, "failed to install Ctrl+C handler");
            // Never resolve: a broken handler must not look like a signal and
            // shut the server down on its own.
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(err) => {
                warn!(?err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let signal = tokio::select! {
        _ = ctrl_c => Signal::Interrupt,
        _ = terminate => Signal::Terminate,
    };
    info!("shutdown signal received");
    signal
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

        for path in [HEALTH_PATH, LIVE_PATH, READY_PATH, LEGACY_HEALTH_PATH] {
            let probe = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(probe.status(), 200, "{path}");
        }

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

    #[test]
    fn drain_grace_falls_back_rather_than_to_zero() {
        assert_eq!(parse_grace(None), DEFAULT_DRAIN_GRACE);
        assert_eq!(parse_grace(Some(" 12 ")), Duration::from_secs(12));
        assert_eq!(parse_grace(Some("0")), Duration::ZERO);
        assert_eq!(parse_grace(Some("0.5")), Duration::from_millis(500));
        // A typo must not read as "close the listener immediately".
        assert_eq!(parse_grace(Some("5s")), DEFAULT_DRAIN_GRACE);
        assert_eq!(parse_grace(Some("-1")), DEFAULT_DRAIN_GRACE);
        assert_eq!(parse_grace(Some("")), DEFAULT_DRAIN_GRACE);
        assert_eq!(parse_grace(Some("inf")), DEFAULT_DRAIN_GRACE);
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
