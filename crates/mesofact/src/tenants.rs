//! Multi-tenant `tenants/<id>.toml` registry for the revalidate receiver.
//!
//! Part of R446 — the relay annotation lives at the top of
//! [`crate::revalidate`]; this module carries the implementation.
//!
//! ## Why
//!
//! [`crate::revalidate::serve`] hosts **one** workload dir + publish config on a
//! process. The cloud-tier runner hosts **many** surfaces (yah.dev's releases
//! page today; more tenants later) on one `mesofact serve --revalidate`
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
//! Inbound `POST /revalidate {route, mirror_key, data_inputs?}`:
//!   - `mirror_key` absent/empty, or matching no tenant → **403** (a tenant with
//!     no configured bearer is unroutable — reject, never open; multi-tenant has
//!     no "open" mode because the bearer *is* the tenant selector).
//!   - an explicit `route` outside the matched tenant's `routes` allowlist →
//!     **403** (scoping, not authentication — see [`TenantFile::routes`]).
//!   - a `data_inputs` key that escapes the workload → **400**.
//!   - matched → enqueue a [`TenantJob`] for that tenant's workload +
//!     publish_config → **202**.
//!
//! The poke's carried `data_inputs` (yah R330-F33) matter more here than in the
//! single-tenant case: a multi-tenant runner is exactly the box that gets
//! replicated for capacity, and the payload is what makes any replica able to
//! service any tenant's poke. See [`crate::revalidate`]'s module docs.
//!
//! Secrets never live in `tenants/<id>.toml`: the bearer is named by
//! `mirror_key_env` and resolved from the environment at load, mirroring
//! `PublishConfig`'s `*_env` credential convention.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::revalidate::{check_data_input_paths, revalidate_once, DataInputs};

/// On-disk shape of one `tenants/<id>.toml` file.
///
/// `deny_unknown_fields` on purpose: every optional key here (`mirror_key_env`,
/// `routes`) fails *open-ish* when absent — no bearer means unroutable, no
/// routes means unrestricted. A typo (`route = [...]`, `mirror_key = "…"`)
/// would therefore not error, it would silently produce a tenant the operator
/// believes is scoped or authenticated and isn't. Rejecting the key is the only
/// place that mistake is visible.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Route allowlist for this tenant. Empty = every render-eligible route in
    /// its own manifest.
    ///
    /// Same two-place enforcement, and the same reasoning, as the single-tenant
    /// [`crate::revalidate::RevalidateConfig::routes`] (yah R752-B7): an
    /// explicit out-of-list route is refused 403, a whole-site poke is NARROWED
    /// to the list. This is scoping, not authentication — the bearer already
    /// bounds a poke to one tenant's workload; `routes` bounds it further
    /// *within* that workload, so a compromised bearer still cannot re-render
    /// (and republish) a surface the deployment never declared pokeable.
    #[serde(default)]
    pub routes: Vec<String>,
}

/// A tenant with its bearer resolved for the running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTenant {
    pub id: String,
    /// The literal bearer resolved from `mirror_key_env`. `None` ⇒ unroutable.
    pub mirror_key: Option<String>,
    pub workload: PathBuf,
    pub publish_config: PathBuf,
    /// See [`TenantFile::routes`]. Empty = unrestricted within this tenant.
    pub routes: Vec<String>,
}

/// A validated poke routed to a specific tenant, handed from the HTTP handler to
/// the render/publish worker.
///
/// Not `Eq`: `data_inputs` holds arbitrary JSON, and `serde_json::Value` is only
/// `PartialEq` (floats).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TenantJob {
    pub tenant_id: String,
    pub workload: PathBuf,
    pub publish_config: PathBuf,
    /// The route to revalidate; `None` = every render-eligible route *within
    /// [`Self::allow`]*.
    pub route: Option<String>,
    /// The tenant's route allowlist, carried on the job rather than looked up
    /// again in the worker — the job is already a self-contained copy of
    /// everything the render needs, and re-reading the registry there would let
    /// the enforced scope drift from the scope the handler checked.
    pub allow: Vec<String>,
    /// Render inputs the poke carried, written into the tenant's workload
    /// before the render. Empty = render from what is on disk.
    pub data_inputs: DataInputs,
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

    /// Every registered tenant, routable or not — for startup logging and
    /// operator introspection.
    pub fn tenants(&self) -> &[ResolvedTenant] {
        &self.tenants
    }

    /// Reject a registry that cannot route unambiguously. Separate from
    /// [`Self::new`] so tests and introspection can build any registry they
    /// like; the binary calls this before serving.
    ///
    /// Two tenants sharing a resolved bearer is the dangerous case:
    /// [`Self::tenant_for`] returns the first match, so one tenant silently
    /// absorbs every poke meant for the other — and absorbing a poke means
    /// rendering *its* workload and publishing to *its* bucket. That is
    /// cross-tenant publication caused by config alone, with a 202 on the wire
    /// and nothing in the logs to distinguish it from success. Duplicate ids are
    /// rejected in the same pass because [`load_tenants`] pins id to filename
    /// stem, so a duplicate can only come from a hand-built registry — but an id
    /// is what every log line and error message identifies a tenant by.
    ///
    /// Errors name the ids, never the bearer.
    pub fn validate(&self) -> Result<()> {
        let mut by_id: BTreeMap<&str, usize> = BTreeMap::new();
        for t in &self.tenants {
            *by_id.entry(t.id.as_str()).or_default() += 1;
        }
        if let Some((id, _)) = by_id.iter().find(|(_, n)| **n > 1) {
            anyhow::bail!("duplicate tenant id {id:?} in the registry");
        }

        let mut by_key: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for t in &self.tenants {
            if let Some(key) = t.mirror_key.as_deref() {
                by_key.entry(key).or_default().push(t.id.as_str());
            }
        }
        if let Some((_, ids)) = by_key.iter().find(|(_, ids)| ids.len() > 1) {
            anyhow::bail!(
                "tenants {} resolve to the same bearer — pokes for one would publish to the other",
                ids.join(", ")
            );
        }
        Ok(())
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
                routes: f.routes,
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
    /// Render inputs the poke carries. Absent → render from the tenant's disk.
    #[serde(default)]
    data_inputs: DataInputs,
}

/// Build the multi-tenant receiver router: `POST /dawn` routes by bearer, plus
/// the [`crate::health`] probes. Decoupled from the render/publish worker via
/// `tx` so it is unit-testable without V8 or a network publish — same split the
/// single-tenant [`crate::revalidate`] receiver uses.
///
/// Readiness is unconditional for the same reason as the single-tenant
/// receiver: no isolate, no served tree. Note that it is *not* per-tenant — a
/// registry entry with a broken workload does not take the whole process out of
/// rotation, and shouldn't, since every other tenant on it still renders.
pub fn router(tx: mpsc::Sender<TenantJob>, registry: Arc<TenantRegistry>) -> Router {
    router_with_health(tx, registry, crate::Health::ready())
}

pub fn router_with_health(
    tx: mpsc::Sender<TenantJob>,
    registry: Arc<TenantRegistry>,
    health: Arc<crate::Health>,
) -> Router {
    Router::new()
        // Same two paths as the single-tenant receiver (yah R752-T10): `/dawn`
        // is the name, `/revalidate` is the transitional alias kept because
        // deployed callers roll separately from the receiver.
        .route("/dawn", post(revalidate_handler))
        .route("/revalidate", post(revalidate_handler))
        // This router accepts `data_inputs` exactly like the single-tenant one,
        // and it is the shape a runner actually hosts — so it needs the same
        // STATED limit rather than axum's inherited 2 MiB default. Sharing the
        // constant is the point: two receivers on the same wire contract with
        // two different ceilings is a payload that works on one node and 413s
        // on the next. See [`crate::revalidate::MAX_REVALIDATE_BODY_BYTES`].
        .layer(DefaultBodyLimit::max(crate::revalidate::MAX_REVALIDATE_BODY_BYTES))
        .with_state(ReceiverState { tx, registry })
        // After `with_state` — see the note in `crate::revalidate::router`.
        .merge(crate::health::probe_routes(health))
}

async fn revalidate_handler(
    State(state): State<ReceiverState>,
    Json(body): Json<RevalidateBody>,
) -> StatusCode {
    let Some(tenant) = state.registry.tenant_for(body.mirror_key.as_deref()) else {
        warn!("revalidate rejected — mirror_key matches no tenant (cross-tenant pollution blocked)");
        return StatusCode::FORBIDDEN;
    };

    // Scope check, after tenant selection because the allowlist is per-tenant.
    // 403 rather than 404 for the same reason as the single-tenant receiver: the
    // route may well exist on that tenant's site; what the caller lacks is
    // authority over it (yah R752-B7).
    if let Some(ref route) = body.route {
        if !tenant.routes.is_empty() && !tenant.routes.iter().any(|a| a == route) {
            warn!(
                tenant = %tenant.id,
                route = %route,
                allowed = %tenant.routes.join(", "),
                "revalidate rejected — route outside this tenant's allowlist",
            );
            return StatusCode::FORBIDDEN;
        }
    }

    if let Err(e) = check_data_input_paths(&body.data_inputs) {
        warn!(tenant = %tenant.id, err = %e, "revalidate rejected — bad data_inputs path");
        return StatusCode::BAD_REQUEST;
    }

    let job = TenantJob {
        tenant_id: tenant.id.clone(),
        workload: tenant.workload.clone(),
        publish_config: tenant.publish_config.clone(),
        route: body.route,
        allow: tenant.routes.clone(),
        data_inputs: body.data_inputs,
    };
    info!(
        tenant = %job.tenant_id,
        route = ?job.route,
        carried_inputs = job.data_inputs.len(),
        "revalidate routed to tenant"
    );

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
        "mesofact serve revalidate receiver starting (multi-tenant, ephemeral render → publish)",
    );
    // One line per tenant, for the same reason the single-tenant receiver logs
    // its resolved allowlist: "which routes may this poke touch" is the first
    // question when a poke returned 202 and nothing changed, and on a
    // multi-tenant runner the answer differs per tenant.
    for t in registry.tenants() {
        info!(
            tenant = %t.id,
            workload = %t.workload.display(),
            publish_config = %t.publish_config.display(),
            routable = t.mirror_key.is_some(),
            allowed_routes = %if t.routes.is_empty() { "<all>".to_string() } else { t.routes.join(", ") },
            "tenant registered",
        );
    }
    // Not fatal — the binary already refuses an empty registry, and a single
    // tenant whose bearer env is missing should not take its co-tenants down —
    // but it must be loud: this process will 403 every poke and still pass
    // /readyz.
    if !registry.is_empty() && registry.tenants().iter().all(|t| t.mirror_key.is_none()) {
        warn!("no tenant has a resolved bearer — every poke will be rejected 403");
    }

    let (tx, mut rx) = mpsc::channel::<TenantJob>(16);
    let health = crate::Health::ready();
    let app = router_with_health(tx, Arc::new(registry), health.clone());

    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            info!(tenant = %job.tenant_id, route = ?job.route, "revalidate poke accepted");
            match revalidate_once(
                &job.workload,
                &job.publish_config,
                job.route.clone(),
                &job.data_inputs,
                // The tenant's own allowlist. The handler already refused an
                // explicit out-of-list route; this second pass is what NARROWS a
                // whole-site poke (`route: None`) to the declared list, exactly
                // as the single-tenant receiver does (yah R752-B7).
                &job.allow,
            )
            .await
            {
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
    // Same drain the single-tenant receiver gained: HTTP side only, worker task
    // still cut on signal. See `crate::revalidate::serve`.
    axum::serve(listener, app)
        .with_graceful_shutdown(crate::shutdown_signal_for(health))
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
            routes: Vec::new(),
        }
    }

    fn scoped(id: &str, key: &str, routes: &[&str]) -> ResolvedTenant {
        ResolvedTenant {
            routes: routes.iter().map(|s| (*s).to_string()).collect(),
            ..tenant(id, Some(key))
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
    fn validate_accepts_a_well_formed_registry() {
        two_tenant_registry().validate().unwrap();
        // Several unroutable tenants are NOT duplicates of each other: `None` is
        // "no bearer", not a bearer value they share.
        TenantRegistry::new(vec![tenant("a", None), tenant("b", None)])
            .validate()
            .unwrap();
    }

    #[test]
    fn validate_rejects_two_tenants_sharing_a_bearer() {
        let reg = TenantRegistry::new(vec![
            tenant("a", Some("s3cr3t-bearer")),
            tenant("b", Some("s3cr3t-bearer")),
        ]);
        let err = format!("{:#}", reg.validate().unwrap_err());
        assert!(err.contains("a, b"), "{err}");
        // The bearer itself must not reach the log an operator pastes into chat.
        assert!(!err.contains("s3cr3t-bearer"), "{err}");
    }

    #[test]
    fn validate_rejects_duplicate_ids() {
        let reg = TenantRegistry::new(vec![tenant("dup", Some("k1")), tenant("dup", Some("k2"))]);
        let err = format!("{:#}", reg.validate().unwrap_err());
        assert!(err.contains("duplicate tenant id"), "{err}");
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
                allow: Vec::new(),
                data_inputs: DataInputs::new(),
            }
        );
    }

    // ── Per-tenant route allowlist (R446, mirroring yah R752-B7) ─────────────

    fn scoped_registry() -> Arc<TenantRegistry> {
        Arc::new(TenantRegistry::new(vec![
            scoped("yah-marketing", "key-mkt", &["/releases"]),
            // Unrestricted: an allowlist on one tenant must not leak onto another.
            tenant("acme", Some("key-acme")),
        ]))
    }

    #[tokio::test]
    async fn route_outside_the_tenants_allowlist_is_403_and_does_not_enqueue() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, scoped_registry());
        let resp = post_json(app, r#"{"route":"/pricing","mirror_key":"key-mkt"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(rx.try_recv().is_err(), "a refused poke must not enqueue");
    }

    #[tokio::test]
    async fn route_inside_the_tenants_allowlist_carries_it_onto_the_job() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, scoped_registry());
        let resp = post_json(app, r#"{"route":"/releases","mirror_key":"key-mkt"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let job = rx.try_recv().unwrap();
        assert_eq!(job.tenant_id, "yah-marketing");
        // The worker re-applies it to narrow whole-site pokes; if it did not
        // travel on the job, the enforced scope would be "<all>".
        assert_eq!(job.allow, vec!["/releases".to_string()]);
    }

    /// The allowlist is per-tenant, not per-process: a route one tenant refuses
    /// is still fine for a tenant that declared none. A shared runner must not
    /// let one tenant's scoping narrow another's.
    #[tokio::test]
    async fn one_tenants_allowlist_does_not_bind_another_tenant() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, scoped_registry());
        let resp = post_json(app, r#"{"route":"/pricing","mirror_key":"key-acme"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let job = rx.try_recv().unwrap();
        assert_eq!(job.tenant_id, "acme");
        assert!(job.allow.is_empty());
    }

    /// A whole-site poke is NARROWED rather than refused — the handler admits
    /// it and the list rides along for the worker to intersect. Same "empty =
    /// all routes reads the same from both ends" rule as the single-tenant
    /// receiver.
    #[tokio::test]
    async fn whole_site_poke_at_a_scoped_tenant_is_accepted_and_carries_the_scope() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, scoped_registry());
        let resp = post_json(app, r#"{"mirror_key":"key-mkt"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let job = rx.try_recv().unwrap();
        assert_eq!(job.route, None);
        assert_eq!(job.allow, vec!["/releases".to_string()]);
    }

    /// An unknown bearer is refused before the allowlist is ever consulted —
    /// a caller must not be able to probe which routes a tenant declares.
    #[tokio::test]
    async fn allowlist_is_not_reachable_without_a_valid_bearer() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, scoped_registry());
        let resp = post_json(app, r#"{"route":"/releases","mirror_key":"intruder"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(rx.try_recv().is_err());
    }

    /// A payload-carrying poke reaches the right tenant with its data intact —
    /// the multi-tenant half of R330-F33.
    #[tokio::test]
    async fn a_carried_payload_routes_to_the_tenant_that_owns_the_bearer() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, two_tenant_registry());
        let resp = post_json(
            app,
            r#"{"route":"/releases","mirror_key":"key-mkt",
                "data_inputs":{"src/data/releases.json":{"releases":[{"version":"0.8.21"}]}}}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let job = rx.try_recv().unwrap();
        assert_eq!(job.tenant_id, "yah-marketing");
        assert_eq!(
            job.data_inputs["src/data/releases.json"]["releases"][0]["version"],
            "0.8.21"
        );
    }

    /// Path containment is enforced per-tenant too — a bearer authorizes one
    /// tenant's workload, not the filesystem the runner happens to share.
    #[tokio::test]
    async fn an_escaping_data_input_path_returns_400_and_enqueues_nothing() {
        let (tx, mut rx) = mpsc::channel::<TenantJob>(4);
        let app = router(tx, two_tenant_registry());
        let resp = post_json(
            app,
            r#"{"route":"/releases","mirror_key":"key-mkt",
                "data_inputs":{"../acme/src/data/releases.json":{}}}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(rx.try_recv().is_err(), "a rejected poke must not enqueue");
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
routes = ["/releases"]
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
        // `routes` is optional: omitted (acme) = unrestricted, declared
        // (yah-marketing) = exactly that list, and it survives to ResolvedTenant.
        assert!(files[0].routes.is_empty());
        assert_eq!(files[1].routes, vec!["/releases".to_string()]);
        let resolved = resolve_tenants(files, |_| Some("k".to_string()));
        assert!(resolved[0].routes.is_empty());
        assert_eq!(resolved[1].routes, vec!["/releases".to_string()]);
    }

    /// A misspelled optional key must not read as "unset" — `route` instead of
    /// `routes` would otherwise yield a tenant with no allowlist at all.
    #[test]
    fn an_unknown_key_fails_loud() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "acme.toml",
            "id = \"acme\"\nworkload = \"/app/acme\"\npublish_config = \"/app/acme/c.toml\"\nroute = [\"/releases\"]\n",
        );
        let err = format!("{:#}", load_tenants(tmp.path()).unwrap_err());
        assert!(err.contains("route"), "{err}");
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
