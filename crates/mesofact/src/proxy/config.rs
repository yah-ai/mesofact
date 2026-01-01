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
}

impl Config {
    pub fn worker_count(&self) -> usize {
        self.workers
            .unwrap_or_else(|| (num_cpus::get() / 2).max(1))
    }
}
