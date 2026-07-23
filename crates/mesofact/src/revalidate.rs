//! `revalidate` — the ephemeral revalidate receiver: the mesofact-native
//! replacement for the standalone `almanac-serve` binary (W225 §3/§4).
//!
//! ## What it is
//!
//! §3 splits two verbs: **`build`** (source → bundle, CI-gated, carries the
//! bundler) and **`revalidate`** (data → SSG output *on the already-built
//! bundle*, no recompilation). This module is the revalidate half: on an
//! invalidation poke it re-runs the render path against fresh data and
//! republishes to the CDN. Per §4 the receiver is "a route mesofact mounts,"
//! not its own service binary — so it ships as a **mode of `mesofact-serve`**
//! (`mesofact-serve <workload> --revalidate`), not a separate executable.
//!
//! ## Why it is ephemeral (the memory-footprint property)
//!
//! Unlike `mesofact-serve`'s SSR-serving mode — which boots a **resident** V8
//! isolate and holds it for the process lifetime — the receiver spins V8 up
//! **per poke** and drops it (`render_route_all` calls `SsgRuntime::start()`
//! then discards it). Resident cost is just axum + config; V8 memory is spent
//! only while a re-render is actively running. One receiver node can therefore
//! back many static sites without holding one isolate per site.
//!
//! ## Bundler-free (W225 §3)
//!
//! `serve` must not link the bundler. The render half comes from the
//! bundler-free `mesofact-render` crate (extracted from `mesofact-build` for
//! exactly this reason, R535-T9); the publish half from `mesofact-publisher`.
//! Neither pulls `rolldown` / `lightningcss`.
//!
//! ## Scope (single-tenant, v1)
//!
//! The receiver serves **one** workload directory, matching what
//! `runner.yah.dev` actually runs today (almanac-serve's single `ALMANAC_DIR`
//! shape). The optional `mirror_key` bearer is ported from
//! `almanac::receiver` as the cross-mirror-pollution guard. A multi-tenant
//! `tenants/<id>.toml` registry — which finally settles the long-open
//! R330-F12 config format — is a follow-up; the ephemeral-V8 property is
//! identical either way.
//!
//! Getting *fresh data onto disk* (the almanac feed-fetch: gh-releases →
//! `data/*.json`) is an upstream **trigger** that plugs into the seam and then
//! pokes this receiver (§3a "domain-triggered invalidation"); it is out of
//! scope here — the receiver renders whatever data currently sits in the
//! workload and republishes.
//!
//! @yah:relay(R446, "mesofact-serve --revalidate: multi-tenant tenants/&lt;id&gt;.toml registry (R330-F12 receiver re-home)")
//! @yah:at(2026-07-15T22:29:05Z)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:gotcha("COORDINATE revalidate.rs edits with Glimmerstone (chat, sigil g-polar-star) — they are live in the mesofact tree with in-flight fixes: per-extension Content-Type in object-store r2.rs put/publish (landed, uncommitted) + the clean-URL extensionless->.html router fix (mesofact R443-B4). Those are general infra; this relay must not duplicate or collide with them. Sync before substantive revalidate.rs edits.")
//! @yah:gotcha("/releases is a STATIC prerender (releases.html) re-rendered from releases.json on revalidate — NOT a serveInstance/pointer route (W059 §3 'materialisation = build-time static, style a'). The registry routes pokes to render+publish; it does not add per-request dynamic serving.")
//! @yah:next("DESIGN (boundary decision): keep the tenant registry MESOFACT-NATIVE. Do NOT deref yah's .yah/services/<svc>/mirrors/<env>.toml inside mesofact — that couples an independently-exportable workspace to yah's config schema, and PublishConfig (mesofact-publisher) is deliberately yah-agnostic (env-named creds, no yah types). tenants/<id>.toml entry = { id, mirror_key (or *_env name), workload (dir containing dist/), publish_config (path to that tenant's mesofact.config.toml [publish]), routes? (optional allowlist) }. Registry maps mirror_key -> tenant -> (workload, publish_config) — a clean generalization of today's single-tenant RevalidateConfig. Glimmerstone's 'thin deref / compose provider ref' goal is RIGHT but belongs on the YAH side: a yah reconciler generates each tenant's mesofact.config.toml from the mirror toml (no such generator exists yet — separate yah-side ticket under R330-F12's producer track).")
//! @yah:next("IMPL: add a TenantRegistry (tenants/<id>.toml parse/load: sorted, missing-dir=empty, id==stem invariant) + a multi-tenant router that routes {route, mirror_key} through it — bearer matches no tenant -> 403; tenant doesn't serve route (allowlist) -> 404; match -> revalidate_once(tenant.workload, tenant.publish_config, route). serve.rs bin: add --tenants <dir> mode, mutually exclusive with single-tenant --workload/--publish-config. Unit-test routing with a fake render/publish callback (mirror revalidate.rs's existing serve_receiver_on split) — no V8, no network.")
//! @yah:next("RE-HOME CONTEXT: receiver half of yah-root R330-F12. almanac-serve BINARY retired for mesofact-serve --revalidate (W225 §3/§4). The gh-releases FETCH -> releases.json is the upstream PRODUCER (yubaba almanac, landed) and is OUT of this receiver's scope — it pokes this receiver after writing fresh data. F11 runner hosts mesofact-serve --revalidate, not almanac-serve.")
//! @yah:assumes("DIVERGENCE flagged to Glimmerstone: they suggested a thinner shape (tenant = {service, env, data_inputs}, deref the yah mirror toml for bucket/prefix/zone/provider). Overriding to mesofact-native on the export-boundary rationale above. The routing CORE (mirror_key->tenant, 403/404, revalidate_once dispatch) is invariant across both shapes; only the config-source detail differs. Awaiting their ack/objection before finalizing field names, but not blocked on it — routing can land first behind the config seam.")
//! @yah:assumes("data_inputs do NOT belong in the tenant registry: route<-data bindings already live in the mesofact manifest.json (RenderRequest.data), and the data SOURCE (gh-releases fetch) is the producer's concern, out of the receiver's scope.")
//! @yah:handoff("LANDED (code-complete, mesofact-dev, feature=ssr): new crate::tenants module + --tenants CLI mode. (1) tenants.rs: TenantFile (id/workload/publish_config/mirror_key_env from tenants/<id>.toml) -> ResolvedTenant (bearer resolved) -> TenantRegistry.tenant_for(mirror_key) routing; TenantJob{tenant_id,workload,publish_config,route}; load_tenants(dir) (sorted, missing-dir=empty, id==stem fail-loud) + resolve_tenants(files, env-lookup closure) (bearer via mirror_key_env, never a literal secret in git); axum router (POST /revalidate {route,mirror_key} -> bearer selects tenant -> enqueue TenantJob -> 202; absent/empty/unknown bearer -> 403) + serve() draining TenantJob through the EXISTING crate::revalidate::revalidate_once (render+publish unchanged, only multiplied). (2) lib.rs: pub mod tenants (ssr). (3) serve.rs bin: --tenants <dir> mode; workload now optional; mutually exclusive with single-tenant --workload/--publish-config. Boundary held: a tenant references its OWN mesofact.config.toml, NOT yah's mirror toml. Tests: 11 new (registry routing incl. unroutable-without-bearer; HTTP 202/403 + whole-site None-route; load sorted/missing-dir/stem-mismatch; resolve env present/absent). mesofact-dev 76->87 green; clippy clean on tenants.rs/serve.rs.")
//! @yah:handoff("REMAINING (not code in this crate): (a) YAH-SIDE generator — a yah reconciler emits each tenant's mesofact.config.toml [publish] from .yah/services/<svc>/mirrors/<env>.toml (Glimmerstone's 'thin deref' goal, kept on the yah side to preserve the export boundary); file under R330-F12's producer track. (b) DEPLOY: F11 runner hosts `mesofact-serve --tenants <dir>` (not almanac-serve), with tenants/<id>.toml + the mirror_key_env bearers set. (c) SMOKE: POST runner /revalidate {route:'/releases', mirror_key:'<yah-marketing bearer>'} -> renders+publishes to yah-marketing's R2. (d) Glimmerstone ack on the mesofact-native shape (divergence flagged; routing core is shape-invariant either way).")
//! @yah:verify("cargo test -p mesofact-dev --features ssr tenants::  # 11 pass (registry routing + HTTP 202/403 + load/resolve)")
//! @yah:verify("cargo test -p mesofact-dev --features ssr  # 87 pass (full crate)")
//! @yah:verify("cargo clippy -p mesofact-dev --features ssr  # clean on tenants.rs + serve.rs")
//! @yah:verify("SMOKE (infra-gated): mesofact-serve --tenants <dir> up; POST /revalidate {route:'/releases', mirror_key:'<bearer>'} -> 202 + renders+publishes; wrong bearer -> 403")

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use mesofact_core::manifest::{Manifest, RouteMode};
use mesofact_publisher::{
    publish_dist, CloudflareCdnPurger, PublishConfig, PublishReport, S3Store,
};
use mesofact_render::render::{render_route_all_with, RenderAllOptions};
use mesofact_render::js::SsgRuntime;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Runtime configuration for the receiver. Built by the `mesofact-serve`
/// binary from CLI flags / env.
#[derive(Debug, Clone)]
pub struct RevalidateConfig {
    /// Workload directory — the parent of `dist/` (with `dist/manifest.json`).
    pub workload: PathBuf,
    /// `mesofact.config.toml` carrying the `[publish]` block (bucket / zone /
    /// env-named credentials). Resolved lazily per poke so the process can
    /// start before creds are present.
    pub publish_config: PathBuf,
    /// Optional shared bearer secret. When `Some`, a poke must carry the same
    /// `mirror_key` or it is rejected 403 — the cross-mirror-pollution guard
    /// ported from `almanac::receiver` (R335-F2). `None` = open receiver.
    pub mirror_key: Option<String>,
}

/// Outcome of one revalidation cycle.
#[derive(Debug)]
pub struct RevalidateReport {
    /// Route patterns that were re-rendered.
    pub rendered_routes: Vec<String>,
    /// Total instances written across those routes.
    pub instances: usize,
    /// The publish leg's report (uploaded / skipped keys, purged tags).
    pub publish: PublishReport,
}

/// One full revalidation cycle: render (ephemeral V8, off the async runtime)
/// then publish. `route`: `Some` → that route only; `None` → every
/// render-eligible route in the manifest (all `static`/`spa`, non-`deferred`).
pub async fn revalidate_once(
    workload: &Path,
    publish_config: &Path,
    route: Option<String>,
) -> Result<RevalidateReport> {
    // Render is synchronous and V8 is `!Send`, so it runs on a blocking thread
    // — booting and dropping its own isolate (the ephemeral property).
    let workload_owned = workload.to_path_buf();
    let (rendered_routes, instances) =
        tokio::task::spawn_blocking(move || render_routes(&workload_owned, route))
            .await
            .context("revalidate: render task panicked")??;

    let publish = publish_built(workload, publish_config).await?;
    Ok(RevalidateReport { rendered_routes, instances, publish })
}

/// Render half — boots one `SsgRuntime`, renders every instance of each
/// target route, and writes them into `dist/`. Synchronous (V8 is `!Send`).
fn render_routes(workload: &Path, route: Option<String>) -> Result<(Vec<String>, usize)> {
    let routes = match route {
        Some(r) => vec![r],
        None => eligible_routes(workload)?,
    };

    let ssg = SsgRuntime::start().context("revalidate: booting SsgRuntime")?;
    let mut instances = 0usize;
    let mut rendered = Vec::with_capacity(routes.len());
    for r in routes {
        let outcomes = render_route_all_with(
            &ssg,
            RenderAllOptions { project_root: workload.to_path_buf(), out_dir: None, route: r.clone() },
        )
        .with_context(|| format!("revalidate: rendering route {r}"))?;
        instances += outcomes.len();
        rendered.push(r);
    }
    Ok((rendered, instances))
}

/// The render-eligible routes for a whole-site poke: everything except `ssr`
/// (rendered per-request in the SSR host, not here) and `deferred` (instances
/// minted at publish time, not enumerable). Mirrors `render_route_all_with`'s
/// own rejections so a whole-site poke skips them instead of erroring.
fn eligible_routes(workload: &Path) -> Result<Vec<String>> {
    let manifest_path = workload.join("dist").join("manifest.json");
    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).with_context(|| {
            format!("reading {} — run `mesofact-build build` first", manifest_path.display())
        })?,
    )
    .with_context(|| format!("parsing {}", manifest_path.display()))?;

    Ok(manifest
        .routes
        .into_iter()
        .filter(|r| {
            r.mode != RouteMode::Ssr
                && !r.prerender.as_ref().map(|p| p.is_deferred()).unwrap_or(false)
        })
        .map(|r| r.route)
        .collect())
}

/// Publish half — reuse the exact `mesofact-publish` construction path:
/// load `[publish]`, resolve env creds, build the S3 + Cloudflare adapters,
/// and run the idempotent `publish_dist` (content-hash skip + tag purge).
async fn publish_built(workload: &Path, config_path: &Path) -> Result<PublishReport> {
    let cfg = PublishConfig::load(config_path)
        .await
        .with_context(|| format!("revalidate: loading [publish] from {}", config_path.display()))?;
    let creds = cfg.resolve_credentials().context("revalidate: resolving publish credentials")?;
    let store = S3Store::new(
        &cfg.endpoint,
        &cfg.bucket,
        &cfg.region,
        &creds.access_key_id,
        &creds.secret_access_key,
    )
    .context("revalidate: S3 store init")?;
    let purger = CloudflareCdnPurger::new(&cfg.zone_id, &creds.cloudflare_api_token)
        .context("revalidate: Cloudflare purger init")?;
    let report = publish_dist(&workload.join("dist"), &store, &purger)
        .await
        .context("revalidate: publish_dist")?;
    Ok(report)
}

// ── HTTP receiver ────────────────────────────────────────────────────────────

/// A validated poke handed from the HTTP handler to the render/publish worker.
type Job = Option<String>; // the (optional) route to revalidate

#[derive(Clone)]
struct ReceiverState {
    tx: mpsc::Sender<Job>,
    /// When `Some`, a poke must carry a matching `mirror_key` or gets 403.
    mirror_key: Option<String>,
}

#[derive(Deserialize)]
struct RevalidateBody {
    /// Route pattern to revalidate, e.g. `/releases`. Omit to revalidate every
    /// render-eligible route in the manifest.
    #[serde(default)]
    route: Option<String>,
    /// Caller's mirror identity token; must match the receiver's configured
    /// `mirror_key` when one is set.
    #[serde(default)]
    mirror_key: Option<String>,
}

/// Build the receiver router: `POST /revalidate` (enqueue) + `GET
/// /__mesofact/health` (readiness). Decoupled from the render/publish worker
/// via `tx` so it is unit-testable without V8 or a network publish — the same
/// split `almanac::serve::serve_receiver_on` uses.
fn router(tx: mpsc::Sender<Job>, mirror_key: Option<String>) -> Router {
    Router::new()
        .route("/revalidate", post(revalidate_handler))
        .route("/__mesofact/health", get(|| async { "ok" }))
        .with_state(ReceiverState { tx, mirror_key })
}

async fn revalidate_handler(
    State(state): State<ReceiverState>,
    Json(body): Json<RevalidateBody>,
) -> StatusCode {
    if let Some(ref expected) = state.mirror_key {
        match &body.mirror_key {
            Some(provided) if provided == expected => {}
            _ => {
                warn!("revalidate rejected — mirror_key mismatch (cross-mirror pollution blocked)");
                return StatusCode::FORBIDDEN;
            }
        }
    }

    match state.tx.try_send(body.route) {
        Ok(()) => StatusCode::ACCEPTED,
        Err(mpsc::error::TrySendError::Full(_)) => {
            warn!("revalidate channel full — dropping poke");
            StatusCode::SERVICE_UNAVAILABLE
        }
        Err(mpsc::error::TrySendError::Closed(_)) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Run the receiver: bind `port`, serve the router, and drain pokes through
/// [`revalidate_once`] one at a time (renders are serialized — one V8 boot at
/// a time keeps the footprint bounded). Runs until a hard I/O error.
pub async fn serve(cfg: RevalidateConfig, host: std::net::IpAddr, port: u16) -> Result<()> {
    info!(
        workload = %cfg.workload.display(),
        publish_config = %cfg.publish_config.display(),
        mirror_key = cfg.mirror_key.is_some(),
        "mesofact-serve revalidate receiver starting (ephemeral render → publish)",
    );

    let (tx, mut rx) = mpsc::channel::<Job>(16);
    let app = router(tx, cfg.mirror_key.clone());

    let workload = cfg.workload.clone();
    let publish_config = cfg.publish_config.clone();
    tokio::spawn(async move {
        while let Some(route) = rx.recv().await {
            info!(route = ?route, "revalidate poke accepted");
            match revalidate_once(&workload, &publish_config, route.clone()).await {
                Ok(report) => info!(
                    route = ?route,
                    rendered = ?report.rendered_routes,
                    instances = report.instances,
                    uploaded = report.publish.uploaded_keys.len(),
                    skipped = report.publish.skipped_keys.len(),
                    purged = report.publish.purged_tags.len(),
                    "revalidate complete",
                ),
                Err(e) => error!(route = ?route, err = ?e, "revalidate failed"),
            }
        }
    });

    let addr = SocketAddr::new(host, port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("revalidate receiver: binding to {addr}"))?;
    info!(%addr, "revalidate receiver listening");
    axum::serve(listener, app).await.context("revalidate receiver: server error")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::util::ServiceExt;

    async fn post_json(app: Router, body: &'static str) -> axum::response::Response {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/revalidate")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        app.oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn poke_enqueues_route_and_returns_202() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None);
        let resp = post_json(app, r#"{"route":"/releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap(), Some("/releases".to_string()));
    }

    #[tokio::test]
    async fn poke_without_route_enqueues_none_whole_site() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None);
        let resp = post_json(app, r#"{}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap(), None);
    }

    #[tokio::test]
    async fn full_channel_returns_503() {
        let (tx, _rx) = mpsc::channel::<Job>(1);
        tx.try_send(Some("already-full".into())).unwrap();
        let app = router(tx, None);
        let resp = post_json(app, r#"{"route":"/x"}"#).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn correct_mirror_key_passes() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, Some("secret-abc".into()));
        let resp = post_json(app, r#"{"route":"/r","mirror_key":"secret-abc"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap(), Some("/r".to_string()));
    }

    #[tokio::test]
    async fn wrong_mirror_key_returns_403_and_does_not_enqueue() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, Some("secret-abc".into()));
        let resp = post_json(app, r#"{"route":"/r","mirror_key":"nope"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(rx.try_recv().is_err(), "rejected poke must not enqueue");
    }

    #[tokio::test]
    async fn absent_mirror_key_returns_403_when_configured() {
        let (tx, _rx) = mpsc::channel::<Job>(4);
        let app = router(tx, Some("secret-abc".into()));
        let resp = post_json(app, r#"{"route":"/r"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (tx, _rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None);
        let req = Request::builder()
            .method(Method::GET)
            .uri("/__mesofact/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Whole-site enumeration filters out `ssr` + `deferred` routes.
    #[tokio::test]
    async fn eligible_routes_filters_ssr_and_deferred() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let r = |route: &str, mode: &str, extra: &str| {
            format!(
                r#"{{"route":"{route}","mode":"{mode}","render_entrypoint":"e.js","cache_policy":{{"ttl":0}}{extra}}}"#
            )
        };
        let manifest = format!(
            r#"{{"version":"1","build_id":"b1","routes":[{},{},{},{},{}]}}"#,
            r("/", "static", ""),
            r("/releases", "static", ""),
            r("/app", "spa", ""),
            r("/api/x", "ssr", ""),
            r("/c/:id", "static", r#","prerender":{"deferred":true}"#),
        );
        std::fs::write(dist.join("manifest.json"), manifest).unwrap();

        let mut got = eligible_routes(tmp.path()).unwrap();
        got.sort();
        assert_eq!(got, vec!["/", "/app", "/releases"]);
    }
}
