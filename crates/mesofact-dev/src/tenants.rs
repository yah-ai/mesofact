//! Multi-tenant `tenants/<id>.toml` registry for the revalidate receiver.
//!
//! Part of R446 — the relay annotation lives at the top of
//! [`crate::revalidate`]; this module carries the implementation.
//!
//! ## Why
//!
//! [`crate::revalidate::serve`] hosts **one** workload dir + publish config on a
//! process. The cloud-tier runner hosts **many** surfaces (yah.dev's releases
//! page today; more tenants later) on one `mesofact-serve --revalidate`
//! process. Each tenant has its own revalidate identity (a bearer the receiver
//! checks) and its own render/publish target (its built workload + its publish
//! config). This module routes an inbound poke to the right tenant by bearer,
//! then runs the *existing* [`crate::revalidate::revalidate_once`] against that
//! tenant's workload — the render/publish half is unchanged, only multiplied.
//!
//! ## Boundary (deliberate)
//!
//! A tenant references its own [`mesofact_publisher`] `mesofact.config.toml`
//! (`publish_config`), **not** yah's `.yah/services/<svc>/mirrors/<env>.toml`.
//! mesofact is an independently-exportable workspace and
//! [`mesofact_publisher::PublishConfig`] is deliberately yah-agnostic (S3
//! endpoint + env-named creds, zero yah types). Composing the publish target
//! from a yah mirror is a *yah-side* concern: a yah reconciler generates each
//! tenant's `mesofact.config.toml` from the mirror toml. Keeping that
//! translation on the yah side preserves the export boundary while still
//! letting yah stay DRY.
//!
//! ## Routing contract
//!
//! Inbound `POST /revalidate {route, mirror_key}`:
//!   - `mirror_key` absent/empty, or matching no tenant → **403** (a tenant with
//!     no configured bearer is unroutable — reject, never open; multi-tenant has
//!     no "open" mode because the bearer *is* the tenant selector).
//!   - matched → enqueue a [`TenantJob`] for that tenant's workload +
//!     publish_config → **202**.
//!
//! Secrets never live in `tenants/<id>.toml`: the bearer is named by
//! `mirror_key_env` and resolved from the environment at load, mirroring
//! `PublishConfig`'s `*_env` credential convention.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::revalidate::revalidate_once;

/// On-disk shape of one `tenants/<id>.toml` file.
#[derive(Debug, Clone, Deserialize)]
pub struct TenantFile {
    /// Tenant id; must match the filename stem (enforced by [`load_tenants`]).
    pub id: String,
    /// Workload directory — the parent of `dist/` (with `dist/manifest.json`),
    /// same shape [`crate::revalidate::serve`] takes for the single-tenant case.
    pub workload: PathBuf,
    /// Path to this tenant's `mesofact.config.toml` carrying `[publish]`.
    pub publish_config: PathBuf,
    /// Name of the env var holding this tenant's revalidate bearer. Absent →
    /// the tenant is unroutable (no poke can select it). Never a literal secret.
    #[serde(default)]
    pub mirror_key_env: Option<String>,
}

/// A tenant with its bearer resolved for the running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTenant {
    pub id: String,
    /// The literal bearer resolved from `mirror_key_env`. `None` ⇒ unroutable.
    pub mirror_key: Option<String>,
    pub workload: PathBuf,
    pub publish_config: PathBuf,
}

/// A validated poke routed to a specific tenant, handed from the HTTP handler to
/// the render/publish worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantJob {
    pub tenant_id: String,
    pub workload: PathBuf,
    pub publish_config: PathBuf,
    /// The route to revalidate; `None` = every render-eligible route.
    pub route: Option<String>,
}

/// The resolved multi-tenant routing table. Immutable for the process lifetime
/// (a config change is a redeploy).
#[derive(Debug, Clone, Default)]
pub struct TenantRegistry {
    tenants: Vec<ResolvedTenant>,
}

impl TenantRegistry {
    pub fn new(tenants: Vec<ResolvedTenant>) -> Self {
        Self { tenants }
    }

    pub fn len(&self) -> usize {
        self.tenants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tenants.is_empty()
    }

    /// Select the tenant a poke's bearer authorizes. An empty/absent bearer
    /// never matches; a tenant with no resolved bearer is never returned.
    pub fn tenant_for(&self, mirror_key: Option<&str>) -> Option<&ResolvedTenant> {
        let provided = mirror_key.filter(|k| !k.is_empty())?;
        self.tenants
            .iter()
            .find(|t| t.mirror_key.as_deref() == Some(provided))
    }
}

/// Resolve `mirror_key_env` names to bearers via a caller-supplied lookup. The
/// production path passes `|name| std::env::var(name).ok()`; tests pass an
/// in-memory map, keeping [`load_tenants`] free of process-env coupling.
pub fn resolve_tenants<F>(files: Vec<TenantFile>, mut lookup: F) -> Vec<ResolvedTenant>
where
    F: FnMut(&str) -> Option<String>,
{
    files
        .into_iter()
        .map(|f| {
            let mirror_key = match &f.mirror_key_env {
                Some(env) => {
                    let v = lookup(env);
                    if v.is_none() {
                        warn!(
                            tenant = %f.id,
                            env = %env,
                            "tenant bearer env unset — tenant will be unroutable"
                        );
                    }
                    v
                }
                None => {
                    warn!(tenant = %f.id, "tenant has no mirror_key_env — unroutable");
                    None
                }
            };
            ResolvedTenant {
                id: f.id,
                mirror_key,
                workload: f.workload,
                publish_config: f.publish_config,
            }
        })
        .collect()
}

/// Load every `*.toml` under `dir` as a [`TenantFile`], deterministically
/// (sorted by path). A missing directory is "no tenants" (empty, not an error).
/// Each file's `id` must equal its filename stem. Bearer resolution is the
/// caller's next step ([`resolve_tenants`]).
pub fn load_tenants(dir: &Path) -> Result<Vec<TenantFile>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading tenants dir {}", dir.display())),
    };

    // Sort for deterministic load order regardless of FS iteration order.
    let mut paths: BTreeMap<PathBuf, ()> = BTreeMap::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading entry in {}", dir.display()))?
            .path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            paths.insert(path, ());
        }
    }

    let mut out = Vec::with_capacity(paths.len());
    for path in paths.into_keys() {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading tenant file {}", path.display()))?;
        let file: TenantFile =
            toml::from_str(&body).with_context(|| format!("parsing tenant file {}", path.display()))?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        if stem != file.id {
            anyhow::bail!(
                "tenant id {:?} does not match filename stem {:?} in {}",
                file.id,
                stem,
                path.display()
            );
        }
        out.push(file);
    }
    Ok(out)
}

// ── HTTP receiver ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ReceiverState {
    tx: mpsc::Sender<TenantJob>,
    registry: Arc<TenantRegistry>,
}

#[derive(Deserialize)]
struct RevalidateBody {
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    mirror_key: Option<String>,
}

/// Build the multi-tenant receiver router: `POST /revalidate` routes by bearer,
/// `GET /__mesofact/health` for readiness. Decoupled from the render/publish
/// worker via `tx` so it is unit-testable without V8 or a network publish —
/// same split the single-tenant [`crate::revalidate`] receiver uses.
pub fn router(tx: mpsc::Sender<TenantJob>, registry: Arc<TenantRegistry>) -> Router {
    Router::new()
        .route("/revalidate", post(revalidate_handler))
        .route("/__mesofact/health", axum::routing::get(|| async { "ok" }))
        .with_state(ReceiverState { tx, registry })
}

async fn revalidate_handler(
    State(state): State<ReceiverState>,
    Json(body): Json<RevalidateBody>,
) -> StatusCode {
    let Some(tenant) = state.registry.tenant_for(body.mirror_key.as_deref()) else {
        warn!("revalidate rejected — mirror_key matches no tenant (cross-tenant pollution blocked)");
        return StatusCode::FORBIDDEN;
    };

    let job = TenantJob {
        tenant_id: tenant.id.clone(),
        workload: tenant.workload.clone(),
        publish_config: tenant.publish_config.clone(),
        route: body.route,
    };
    info!(tenant = %job.tenant_id, route = ?job.route, "revalidate routed to tenant");

    match state.tx.try_send(job) {
        Ok(()) => StatusCode::ACCEPTED,
        Err(mpsc::error::TrySendError::Full(_)) => {
            warn!("revalidate channel full — dropping poke");
            StatusCode::SERVICE_UNAVAILABLE
        }
        Err(mpsc::error::TrySendError::Closed(_)) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Run the multi-tenant revalidate receiver: bind `port`, serve the router, and
/// drain routed [`TenantJob`]s through [`revalidate_once`] one at a time
/// (renders serialized — one V8 boot at a time bounds the footprint). Runs
/// until a hard I/O error.
pub async fn serve(
    registry: TenantRegistry,
    host: std::net::IpAddr,
    port: u16,
) -> Result<()> {
    info!(
        tenants = registry.len(),
        "mesofact-serve revalidate receiver starting (multi-tenant, ephemeral render → publish)",
    );

    let (tx, mut rx) = mpsc::channel::<TenantJob>(16);
    let app = router(tx, Arc::new(registry));

    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            info!(tenant = %job.tenant_id, route = ?job.route, "revalidate poke accepted");
            match revalidate_once(&job.workload, &job.publish_config, job.route.clone()).await {
                Ok(report) => info!(
                    tenant = %job.tenant_id,
                    route = ?job.route,
                    rendered = ?report.rendered_routes,
                    instances = report.instances,
                    uploaded = report.publish.uploaded_keys.len(),
                    "revalidate complete",
                ),
                Err(e) => error!(tenant = %job.tenant_id, route = ?job.route, err = ?e, "revalidate failed"),
            }
        }
    });

    let addr = std::net::SocketAddr::new(host, port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("multi-tenant revalidate receiver: binding to {addr}"))?;
    info!(%addr, "multi-tenant revalidate receiver listening");
    axum::serve(listener, app)
        .await
        .context("multi-tenant revalidate receiver: server error")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use std::fs;
    use tempfile::TempDir;
    use tower::util::ServiceExt;

    fn tenant(id: &str, key: Option<&str>) -> ResolvedTenant {
        ResolvedTenant {
            id: id.to_string(),
            mirror_key: key.map(String::from),
            workload: PathBuf::from(format!("/app/{id}")),
            publish_config: PathBuf::from(format!("/app/{id}/mesofact.config.toml")),
        }
    }

    fn two_tenant_registry() -> Arc<TenantRegistry> {
        Arc::new(TenantRegistry::new(vec![
            tenant("yah-marketing", Some("key-mkt")),
            tenant("acme", Some("key-acme")),
        ]))
    }

    async fn post_json(app: Router, body: &'static str) -> axum::response::Response {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/revalidate")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        app.oneshot(req).await.unwrap()
    }

    // ── registry routing ─────────────────────────────────────────────────────

    #[test]
    fn tenant_for_matches_by_bearer() {
        let reg = two_tenant_registry();
        assert_eq!(reg.tenant_for(Some("key-acme")).unwrap().id, "acme");
        assert_eq!(reg.tenant_for(Some("key-mkt")).unwrap().id, "yah-marketing");
    }

    #[test]
    fn tenant_for_rejects_absent_empty_and_unknown() {
        let reg = two_tenant_registry();
        assert!(reg.tenant_for(None).is_none());
        assert!(reg.tenant_for(Some("")).is_none());
        assert!(reg.tenant_for(Some("intruder")).is_none());
    }

    #[test]
    fn tenant_without_bearer_is_unroutable() {
        let reg = TenantRegistry::new(vec![tenant("t", None)]);
        assert!(reg.tenant_for(None).is_none());
        assert!(reg.tenant_for(Some("")).is_none());
    }

    // ── HTTP routing ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn matching_bearer_routes_job_and_returns_202() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, two_tenant_registry());
        let resp = post_json(app, r#"{"route":"/releases","mirror_key":"key-mkt"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(
            rx.try_recv().unwrap(),
            TenantJob {
                tenant_id: "yah-marketing".into(),
                workload: PathBuf::from("/app/yah-marketing"),
                publish_config: PathBuf::from("/app/yah-marketing/mesofact.config.toml"),
                route: Some("/releases".into()),
            }
        );
    }

    #[tokio::test]
    async fn whole_site_poke_enqueues_none_route() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, two_tenant_registry());
        let resp = post_json(app, r#"{"mirror_key":"key-acme"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let job = rx.try_recv().unwrap();
        assert_eq!(job.tenant_id, "acme");
        assert_eq!(job.route, None);
    }

    #[tokio::test]
    async fn unknown_bearer_returns_403_and_enqueues_nothing() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, two_tenant_registry());
        let resp = post_json(app, r#"{"route":"/releases","mirror_key":"intruder"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn absent_bearer_returns_403() {
        let (tx, _rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, two_tenant_registry());
        let resp = post_json(app, r#"{"route":"/releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // ── load + resolve ───────────────────────────────────────────────────────

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    const YAH_MARKETING: &str = r#"
id = "yah-marketing"
workload = "/app/yah-marketing"
publish_config = "/app/yah-marketing/mesofact.config.toml"
mirror_key_env = "MESOFACT_TENANT_YAH_MARKETING_KEY"
"#;

    #[test]
    fn load_tenants_sorted_and_stem_checked() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "yah-marketing.toml", YAH_MARKETING);
        write(
            tmp.path(),
            "acme.toml",
            "id = \"acme\"\nworkload = \"/app/acme\"\npublish_config = \"/app/acme/mesofact.config.toml\"\n",
        );
        write(tmp.path(), "README.md", "not a tenant\n");

        let files = load_tenants(tmp.path()).unwrap();
        assert_eq!(
            files.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            vec!["acme", "yah-marketing"]
        );
    }

    #[test]
    fn missing_dir_is_empty_not_error() {
        let tmp = TempDir::new().unwrap();
        assert!(load_tenants(&tmp.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn id_stem_mismatch_fails_loud() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "wrong.toml", YAH_MARKETING); // id=yah-marketing, file=wrong
        let err = load_tenants(tmp.path()).unwrap_err();
        assert!(format!("{err:#}").contains("does not match filename stem"));
    }

    #[test]
    fn resolve_tenants_uses_lookup_and_flags_unset() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "yah-marketing.toml", YAH_MARKETING);
        let files = load_tenants(tmp.path()).unwrap();

        // Env var present → bearer resolves.
        let resolved = resolve_tenants(files.clone(), |name| {
            (name == "MESOFACT_TENANT_YAH_MARKETING_KEY").then(|| "secret-abc".to_string())
        });
        assert_eq!(resolved[0].mirror_key.as_deref(), Some("secret-abc"));

        // Env var absent → unroutable (mirror_key None).
        let unresolved = resolve_tenants(files, |_| None);
        assert!(unresolved[0].mirror_key.is_none());
        let reg = TenantRegistry::new(unresolved);
        assert!(reg.tenant_for(Some("secret-abc")).is_none());
    }
}
