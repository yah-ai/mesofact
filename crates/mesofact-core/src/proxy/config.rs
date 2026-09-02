//! CLI configuration for the mesofact-proxy binary.

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "mesofact-proxy", about = "mesofact tri-mode web proxy (axum)")]
pub struct Config {
    /// Local path to manifest.json (required).
    #[arg(long, env = "MESOFACT_MANIFEST")]
    pub manifest: PathBuf,

    /// Bind address for the HTTP listener.
    #[arg(long, default_value = "0.0.0.0:3000", env = "MESOFACT_BIND")]
    pub bind: String,

    /// Number of Bun workers to spawn (default = num_cpus / 2, min 1).
    #[arg(long, env = "MESOFACT_WORKERS")]
    pub workers: Option<usize>,

    /// CDN base URL for Mode 1 redirect dispatch (e.g. https://cdn.yah.dev).
    /// When set, Mode 1 routes 302-redirect to `{cdn_base_url}{path}`.
    #[arg(long, env = "MESOFACT_CDN_BASE_URL")]
    pub cdn_base_url: Option<String>,

    /// Local dist/ directory for Mode 1 fallback (when CDN is not configured).
    #[arg(long, env = "MESOFACT_FALLBACK_DIR")]
    pub fallback_dir: Option<PathBuf>,

    /// Path to the mesofact-worker entry script (bun entrypoint).
    #[arg(
        long,
        env = "MESOFACT_WORKER_ENTRY",
        default_value = "packages/mesofact-worker/src/worker.ts"
    )]
    pub worker_entry: PathBuf,

    /// Path to `mesofact.config.toml`. Read for source generation tokens
    /// (cache-key input 6). Missing file → generations resolve to a placeholder.
    #[arg(long, env = "MESOFACT_SOURCES_CONFIG")]
    pub sources_config: Option<PathBuf>,

    /// Env var holding the HMAC key for `CookieSessionResolver`. When set,
    /// Mode 2 sessions resolve from the session cookie; when unset, sessions
    /// are disabled (`requires: ["user"]` routes always redirect/401).
    #[arg(long, env = "MESOFACT_SESSION_SECRET_ENV")]
    pub session_secret_env: Option<String>,

    /// Session cookie name (default `mesofact_session`).
    #[arg(long, env = "MESOFACT_SESSION_COOKIE", default_value = "mesofact_session")]
    pub session_cookie: String,

    /// Login URL for `requires: ["user"]` routes with no session. The proxy
    /// 302s here with `?next=<original-url>`. Unset → 401 instead.
    #[arg(long, env = "MESOFACT_LOGIN_URL")]
    pub login_url: Option<String>,

    /// Mode 2 LRU response-cache capacity (entries).
    #[arg(long, env = "MESOFACT_CACHE_CAPACITY", default_value_t = 4096)]
    pub cache_capacity: usize,

    /// Assert that something in front of this process enforces a route policy
    /// this tier does not (R749-T1). Comma-separated field names as written in
    /// `mesofact.routes.ts`.
    ///
    /// The proxy tier's standing case is `resilience`: W181 puts retry/timeout
    /// at the always-up edge (the CF Worker reads them from `SSR_RESILIENCE`),
    /// and this process implements none of it. Deployed behind that Worker the
    /// policy really is enforced — by the Worker — so the deployment says so
    /// here. Run without one and the refusal is correct.
    ///
    /// Same spelling and semantics as `mesofact serve`'s flag on purpose: an
    /// operator moving a workload between tiers should not have to learn a
    /// second vocabulary for the same assertion.
    #[arg(
        long,
        env = "MESOFACT_POLICY_DELEGATED",
        value_delimiter = ',',
        value_parser = parse_policy_field,
    )]
    pub policy_delegated: Vec<crate::policy::RoutePolicy>,
}

/// Parse one `--policy-delegated` field name. A typo is rejected rather than
/// ignored: a delegation that silently does not apply is a fail-open with a
/// flag in front of it.
fn parse_policy_field(raw: &str) -> Result<crate::policy::RoutePolicy, String> {
    crate::policy::RoutePolicy::parse(raw.trim()).ok_or_else(|| {
        format!(
            "unknown route policy {raw:?} — known policies are {}",
            crate::policy::RoutePolicy::ALL
                .iter()
                .map(|p| p.field())
                .collect::<Vec<_>>()
                .join(", "),
        )
    })
}

impl Config {
    /// What `mesofact proxy` implements, advertised (R749-T1). One place, and
    /// each line has a code site behind it:
    ///
    ///   - `requires` — `proxy::router`'s `Requires::User` check, the only
    ///     session-aware serving path in the system.
    ///   - `cache_policy` — `proxy::cache::ResponseCache`, built in
    ///     `cli::proxy::run`.
    ///   - `concurrency` — the worker pool's per-route semaphore
    ///     (`packages/mesofact-worker/src/pool.ts`, configured from the
    ///     manifest at `worker.ts`'s `buildRouteHandlers`). W225 §2c records
    ///     this row as unenforced everywhere; that is wrong, and the code above
    ///     is why.
    ///   - `resilience` — NOT enforced. Nothing in `mesofact-core` reads it;
    ///     W181's only consumer is the CF Worker
    ///     (`packages/mesofact-edge/src/router.ts`, from `SSR_RESILIENCE`).
    ///     Delegate it when that Worker is in front.
    pub fn policy_support(&self) -> crate::policy::PolicySupport {
        use crate::policy::RoutePolicy;
        let mut support = crate::policy::PolicySupport::new("mesofact proxy")
            .enforces(RoutePolicy::Requires)
            .enforces(RoutePolicy::CachePolicy)
            .enforces(RoutePolicy::Concurrency);
        for policy in &self.policy_delegated {
            support = support.delegate(*policy);
        }
        support
    }

    pub fn worker_count(&self) -> usize {
        self.workers
            .unwrap_or_else(|| (num_cpus::get() / 2).max(1))
    }
}
