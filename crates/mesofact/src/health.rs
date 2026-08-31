//! Kubernetes-conventional probe endpoints.
//!
//! mesofact had exactly one probe — `GET /__mesofact/health` → `200 ok` — and
//! every caller read it as readiness (the pond/cloud reconciler points
//! `ready_path` at it; `Dockerfile.ssr-runtime` documents it as *Readiness*).
//! It never was one. It answers 200 the instant axum is serving, which is the
//! definition of **liveness**, so a pod whose SSR isolate had not booted, or
//! whose `dist/` was still empty, reported ready and took traffic it could only
//! 404.
//!
//! # The three probes, and what each one is allowed to depend on
//!
//! kube-apiserver's own naming is the convention worth matching, because it is
//! what every operator, chart and dashboard already assumes:
//!
//! | Path | Probe | Failing it means | Depends on |
//! |---|---|---|---|
//! | [`LIVE_PATH`] (`/livez`) | liveness **and** startup | kubelet **restarts the container** | nothing — process is up and the runtime is scheduling |
//! | [`READY_PATH`] (`/readyz`) | readiness | endpoints controller **pulls the pod out of the Service** | can this process serve a request *right now* |
//! | [`LEGACY_HEALTH_PATH`] (`/healthz`) | deprecated aggregate | — | alias of `/readyz`, kept because charts still probe it |
//!
//! The asymmetry in consequences is the whole design. Liveness must never
//! depend on anything it cannot fix by dying: gate it on a database and one
//! database blip restarts every replica at once, turning a brownout into an
//! outage. Readiness is the opposite — it *should* fail on a missing
//! dependency, because the remedy (stop sending this pod traffic) is exactly
//! right. So a [`ReadyCheck`] only ever affects `/readyz`.
//!
//! **There is no `/startupz`,** and adding one would be cargo-culting: a
//! `startupProbe` is a *schedule* for a probe, not a fourth kind of check.
//! Point it at `/livez` with a generous `failureThreshold` — the apiserver
//! pattern — and slow SSR isolate boots get all the time they need without
//! loosening the steady-state liveness threshold.
//!
//! # Draining
//!
//! On `SIGTERM`, `/readyz` starts failing *before* the listener stops
//! accepting (see [`crate::shutdown_signal_for`]). Without that gap a rolling
//! update drops connections: kubelet sends SIGTERM and removes the endpoint
//! concurrently, so for the propagation window (kube-proxy, ingress, every
//! sidecar's own cache) traffic still arrives at a socket that has already
//! closed. `/livez` keeps answering 200 throughout — a draining process is
//! working correctly, and restarting it is the one thing that would hurt.
//!
//! # Wire format
//!
//! `200`/`503` plaintext, `ok` on success. `?verbose` returns the apiserver's
//! per-check listing, so `kubectl get --raw '/readyz?verbose'` reads the way an
//! operator expects:
//!
//! ```text
//! [+]ping ok
//! [-]ssr failed
//! readyz check failed
//! ```
//!
//! # Extending it, from either language
//!
//! mesofact is a dual-language server, so a standard route has to be
//! extensible from both sides. `/readyz` is the first one built that way, and
//! the shape it settled on is: **Rust owns the wire contract and the
//! invariants; contributions are named checks that can only make the answer
//! stricter.**
//!
//! - **Rust** — implement [`ReadyCheck`] (or hand [`Gate::new`] a closure) and
//!   install it with [`Health::set_checks`]. To replace the endpoints outright,
//!   don't merge [`probe_routes`]: build your own router and mount whatever you
//!   like. [`Server::without_standard_probes`](crate::Server::without_standard_probes)
//!   is the opt-out on the serving engine. Middleware is just `Router::layer`
//!   over what [`probe_routes`] returns.
//! - **TSX** — declare `/readyz` as an ordinary `mode: "ssr"` route in
//!   `mesofact.routes.ts`. No new config key, no manifest field: the Rust
//!   handler notices the app claimed the path and folds its response status in
//!   as a check named `app`. Worked example:
//!   `examples/hello/src/readyz.ts`; the `@mesofact/runtime` sugar is
//!   `defineReadyz`, which emits this same wire format.
//!
//! The asymmetry is deliberate and is the interesting result of the dogfood: an
//! app check is **additive**. It cannot make a not-ready process report ready,
//! because the states it would have to override — draining, isolate down — are
//! exactly the ones where app code cannot be trusted to answer (it may be the
//! thing that is broken). `/livez` takes no contributions at all, in either
//! language, for the same reason it takes no gates.

use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};

use axum::{
    extract::{RawQuery, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

/// Liveness path, and the one a `startupProbe` should target. 200 whenever the
/// process is serving — including while draining.
pub const LIVE_PATH: &str = "/livez";

/// Readiness path. 200 only when every [`ReadyCheck`] passes and the process
/// is not draining.
pub const READY_PATH: &str = "/readyz";

/// Deprecated aggregate, kept because it is what pre-1.16 charts and most
/// hand-written probe blocks still say. Behaves as [`READY_PATH`].
pub const LEGACY_HEALTH_PATH: &str = "/healthz";

/// One named readiness condition — the Rust extension point.
///
/// The name is the operator-facing string in `?verbose` output, so it should
/// read as a noun (`dist`, `ssr`, `app`), not a sentence.
///
/// Async because the interesting checks are: the TSX contribution dispatches
/// into the V8 isolate, and a future one might touch a socket. Returns a boxed
/// future rather than using `async fn` in the trait because the check list is
/// `dyn` and AFIT is not dyn-safe.
///
/// A check must be **fast and side-effect free**. Probes run on the kubelet's
/// schedule (typically every 10s, per replica) and a slow one turns a
/// readiness probe into a load generator against whatever it touches.
pub trait ReadyCheck: Send + Sync {
    fn name(&self) -> &'static str;
    fn ready(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}

/// A [`ReadyCheck`] over a synchronous closure — the common case, where
/// readiness is reading an `AtomicBool` or stat-ing a path.
///
/// Boxed at construction rather than left generic: two gates on one service are
/// two distinct closure types, and a `Vec` of them will not unify otherwise.
pub struct Gate {
    name: &'static str,
    check: Box<dyn Fn() -> bool + Send + Sync>,
}

impl Gate {
    pub fn new<F>(name: &'static str, check: F) -> Self
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        Self {
            name,
            check: Box::new(check),
        }
    }
}

impl ReadyCheck for Gate {
    fn name(&self) -> &'static str {
        self.name
    }

    fn ready(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let verdict = (self.check)();
        Box::pin(async move { verdict })
    }
}

/// Shared probe state: the `started`/`draining` lifecycle bits plus the
/// readiness checks. Cheap to clone as an `Arc`; probe handlers clone the check
/// list out from under a read lock that never contends with the request path.
#[derive(Default)]
pub struct Health {
    started: AtomicBool,
    draining: AtomicBool,
    checks: RwLock<Vec<Arc<dyn ReadyCheck>>>,
}

impl Health {
    /// A not-yet-started handle. `/readyz` fails until [`mark_started`] fires,
    /// which is what makes "bound the socket but haven't finished boot" a
    /// distinguishable state rather than an accidental 200.
    ///
    /// [`mark_started`]: Health::mark_started
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A handle that is ready the moment it exists — for services whose
    /// readiness is "the router is mounted" and nothing more (the revalidate
    /// receivers: no isolate, no served tree, just an enqueue endpoint).
    pub fn ready() -> Arc<Self> {
        let h = Self::default();
        h.started.store(true, Ordering::Release);
        Arc::new(h)
    }

    /// Replace the readiness checks. Replace rather than append so a caller
    /// that rebuilds its router (tests do; the dev watcher may) cannot silently
    /// accumulate duplicates of the same check.
    pub fn set_checks(&self, checks: Vec<Arc<dyn ReadyCheck>>) {
        *self.checks.write().expect("health checks poisoned") = checks;
    }

    /// [`set_checks`](Health::set_checks) for the synchronous-closure case.
    pub fn set_gates(&self, gates: Vec<Gate>) {
        self.set_checks(
            gates
                .into_iter()
                .map(|g| Arc::new(g) as Arc<dyn ReadyCheck>)
                .collect(),
        );
    }

    /// Boot finished; `/readyz` may now pass if its gates do.
    pub fn mark_started(&self) {
        self.started.store(true, Ordering::Release);
    }

    /// Begin refusing readiness. Irreversible — a drained process is on its way
    /// out, and a probe that flapped back to ready would invite the endpoints
    /// controller to re-add a pod that is already closing its listener.
    pub fn begin_drain(&self) {
        self.draining.store(true, Ordering::Release);
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    /// Evaluate readiness. Returns whether it passed plus the apiserver-style
    /// per-check listing, always fully evaluated — reporting the *first*
    /// failure only would hide a second broken subsystem behind the first.
    async fn readiness(&self) -> (bool, String) {
        let mut ok = true;
        let mut body = String::new();

        let line = |name: &str, pass: bool, body: &mut String| {
            body.push_str(if pass { "[+]" } else { "[-]" });
            body.push_str(name);
            body.push_str(if pass { " ok\n" } else { " failed\n" });
        };

        let started = self.started.load(Ordering::Acquire);
        line("started", started, &mut body);
        ok &= started;

        // Reported inverted (`[+]shutdown ok` = not shutting down) to match the
        // apiserver, where every line reads "this condition is satisfied".
        let live = !self.is_draining();
        line("shutdown", live, &mut body);
        ok &= live;

        // Clone the Arcs out before awaiting: an `RwLockReadGuard` is not
        // `Send`, so holding it across the first `.await` would make the whole
        // handler future non-`Send` and axum would refuse the route.
        let checks: Vec<Arc<dyn ReadyCheck>> = self
            .checks
            .read()
            .expect("health checks poisoned")
            .iter()
            .cloned()
            .collect();
        for check in checks {
            // Sequential, not `join_all`. Every check still runs — the listing
            // must stay complete — but a probe should not fan out concurrent
            // work on the kubelet's schedule, and the only expensive check
            // (dispatching into the isolate) already returns false without
            // dispatching when the isolate is down.
            let pass = check.ready().await;
            line(check.name(), pass, &mut body);
            ok &= pass;
        }

        (ok, body)
    }
}

/// Probe routes for a [`Health`] handle: `/livez`, `/readyz`, `/healthz`, and
/// the historical [`crate::HEALTH_PATH`].
///
/// Merge into a service's router. The legacy path is deliberately wired to the
/// **liveness** handler, not readiness: that is the behavior it has always had,
/// and re-pointing it at the stricter check would change what a deployed
/// yubaba reconciler's `ready_path` means in the middle of a rollout. Callers
/// migrate to `/readyz` on purpose, not by upgrading.
pub fn probe_routes(health: Arc<Health>) -> Router {
    Router::new()
        .route(LIVE_PATH, get(livez))
        .route(READY_PATH, get(readyz))
        .route(LEGACY_HEALTH_PATH, get(readyz))
        .route(crate::HEALTH_PATH, get(livez))
        .with_state(health)
}

/// Liveness: 200 as long as this task is being polled. Deliberately checks
/// nothing else — see the module docs on why a dependency-aware liveness probe
/// converts a dependency blip into a restart storm.
async fn livez(State(health): State<Arc<Health>>, RawQuery(q): RawQuery) -> Response {
    if !is_verbose(q.as_deref()) {
        return plaintext(StatusCode::OK, "ok".into());
    }
    let mut body = String::from("[+]ping ok\n");
    if health.is_draining() {
        // Not a failure — but an operator staring at a pod that is refusing
        // readiness needs to see *why* here, since /readyz alone cannot
        // distinguish "draining" from "broken".
        body.push_str("[+]draining ok\n");
    }
    body.push_str("livez check passed\n");
    plaintext(StatusCode::OK, body)
}

/// Readiness: 200 only if started, not draining, and every gate passes.
async fn readyz(State(health): State<Arc<Health>>, RawQuery(q): RawQuery) -> Response {
    let (ok, detail) = health.readiness().await;
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    if !is_verbose(q.as_deref()) {
        let body = if ok { "ok\n" } else { "readyz check failed\n" };
        return plaintext(status, body.into());
    }
    let mut body = detail;
    body.push_str(if ok {
        "readyz check passed\n"
    } else {
        "readyz check failed\n"
    });
    plaintext(status, body)
}

fn is_verbose(query: Option<&str>) -> bool {
    query.is_some_and(|q| q.split('&').any(|p| p == "verbose" || p.starts_with("verbose=")))
}

/// Probe responses must never be cached — a cached 200 readiness outlives the
/// condition it described, which is the failure mode the probe exists to catch.
fn plaintext(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    async fn probe(app: Router, uri: &str) -> (StatusCode, String) {
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn readyz_fails_before_started_but_livez_passes() {
        // The state the old single endpoint could not express: socket bound,
        // boot unfinished. Liveness must pass (nothing to restart) while
        // readiness must fail (nothing to serve).
        let health = Health::new();
        let app = probe_routes(health.clone());

        let (status, _) = probe(app.clone(), READY_PATH).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = probe(app.clone(), LIVE_PATH).await;
        assert_eq!(status, StatusCode::OK);

        health.mark_started();
        let (status, body) = probe(app, READY_PATH).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok\n");
    }

    #[tokio::test]
    async fn a_failing_gate_fails_readiness_only() {
        let health = Health::ready();
        health.set_gates(vec![Gate::new("dist", || false)]);
        let app = probe_routes(health);

        let (status, _) = probe(app.clone(), READY_PATH).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        // Restarting the container cannot conjure a dist tree, so liveness
        // must not carry the gate.
        let (status, _) = probe(app, LIVE_PATH).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn draining_fails_readiness_and_keeps_liveness() {
        let health = Health::ready();
        health.begin_drain();
        let app = probe_routes(health);

        let (status, _) = probe(app.clone(), READY_PATH).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = probe(app, LIVE_PATH).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a draining pod is healthy; restarting it mid-drain drops the connections \
             the drain exists to protect"
        );
    }

    #[tokio::test]
    async fn verbose_lists_every_check_not_just_the_first_failure() {
        let health = Health::ready();
        health.set_gates(vec![Gate::new("dist", || false), Gate::new("ssr", || false)]);
        let (status, body) = probe(probe_routes(health), "/readyz?verbose").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body,
            "[+]started ok\n[+]shutdown ok\n[-]dist failed\n[-]ssr failed\nreadyz check failed\n",
        );
    }

    #[tokio::test]
    async fn set_gates_replaces_rather_than_accumulates() {
        let health = Health::ready();
        health.set_gates(vec![Gate::new("dist", || false)]);
        health.set_gates(vec![Gate::new("dist", || true)]);
        let (status, body) = probe(probe_routes(health), "/readyz?verbose").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.matches("dist").count(), 1, "rebuilt router duplicated a gate");
    }

    #[tokio::test]
    async fn legacy_paths_keep_their_liveness_meaning() {
        // /__mesofact/health is a deployed reconciler's ready_path. It answered
        // 200-on-bind for its whole life; tightening it here would fail live
        // rollouts on upgrade rather than on a deliberate migration.
        let health = Health::new();
        health.set_gates(vec![Gate::new("dist", || false)]);
        let app = probe_routes(health);

        let (status, _) = probe(app.clone(), crate::HEALTH_PATH).await;
        assert_eq!(status, StatusCode::OK);
        // /healthz, by contrast, is new here — it can carry the strict meaning.
        let (status, _) = probe(app, LEGACY_HEALTH_PATH).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn verbose_matches_only_the_flag() {
        assert!(is_verbose(Some("verbose")));
        assert!(is_verbose(Some("verbose=1")));
        assert!(is_verbose(Some("exclude=ssr&verbose")));
        assert!(!is_verbose(Some("verbosely=1")));
        assert!(!is_verbose(None));
    }
}
