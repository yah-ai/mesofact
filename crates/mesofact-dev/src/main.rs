//! `mesofact-dev` CLI entrypoint. See the [library crate](mesofact_dev) for
//! the server + watcher implementations.

use std::path::PathBuf;
#[cfg(feature = "ssr")]
use std::sync::Arc;

use clap::Parser;
#[cfg(feature = "ssr")]
use mesofact_dev::{ssr, SsrSpawnOptions};
use mesofact_dev::{watcher, Server, WatchOptions, DEFAULT_PORT};
use tracing::info;

#[derive(Parser, Debug)]
#[command(version, about = "Static-file dev server for mesofact-static workloads")]
struct Args {
    /// Workload directory — the parent of `dist/html/` (e.g. `app/yah/web`).
    workload: PathBuf,

    /// TCP port to bind on 127.0.0.1.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// Disable the file-watch + auto-rebuild loop; serve whatever's on disk.
    #[arg(long)]
    no_watch: bool,

    /// Skip the initial build at startup (watch mode only).
    #[arg(long)]
    no_initial_build: bool,

    /// Path to a JSON `prefix → backend base URL` map for the same-origin
    /// reverse proxy (R513-F10), e.g.
    /// `{"/auth": "http://127.0.0.1:8745", "/dev": "http://127.0.0.1:8745"}`.
    /// Matching requests are forwarded to the backend before static serving so
    /// the SPA stays single-origin (no CORS). Camp-emitted at SPA-service spawn.
    #[arg(long, value_name = "PATH")]
    proxy_map: Option<PathBuf>,

    /// Path to a JSON file served verbatim at `/config.json` (R513-F5/F10): the
    /// SPA's `DashboardConfig` (apiBaseUrl / authBaseUrl / env …). Injected by
    /// the server — NOT placed in the served `dist/` — so an Option-A pipeline
    /// serving the same bundle never inherits a stale `env:ci` config.
    #[arg(long, value_name = "PATH")]
    config_json: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("mesofact_dev=info,tower_http=info")
            }),
        )
        .init();

    let args = Args::parse();
    let mut server = Server::from_workload(&args.workload)?;

    // Same-origin reverse proxy (R513-F10): forward `/auth/*` etc. to the
    // camp-vended backend ports so the dashboard E2E (Option B) browser stays
    // single-origin. No map → no proxy (the Option A static path is unchanged).
    if let Some(map_path) = &args.proxy_map {
        let map = mesofact_dev::ProxyMap::from_json_file(map_path)?;
        info!(
            map = %map_path.display(),
            routes = ?map.routes(),
            "mesofact-dev: same-origin reverse proxy installed",
        );
        server = server.with_proxy(map);
    }

    // Server-injected runtime config (R513-F5/F10) served at /config.json.
    if let Some(config_path) = &args.config_json {
        let bytes = std::fs::read(config_path)
            .map_err(|e| anyhow::anyhow!("reading config json {}: {e}", config_path.display()))?;
        info!(config = %config_path.display(), "mesofact-dev: serving /config.json (R513-F10)");
        server = server.with_config_json(bytes);
    }

    // Canonicalize so the bun child's manifest read + dynamic-import use
    // absolute paths regardless of the cwd mesofact-dev was invoked from.
    let workload_abs = args
        .workload
        .canonicalize()
        .unwrap_or_else(|_| args.workload.clone());
    let state_dir = workload_abs.join(".mesofact-dev");
    #[cfg(feature = "ssr")]
    let ssr_slot = server.ssr_slot();

    // Attach an SSR child if the workload's manifest declares any mode:"ssr"
    // routes. ssr::spawn returns Ok(None) for static/SPA-only workloads (or
    // when no build has emitted a manifest yet); the no-bun path is preserved
    // and the post-build hook below retries lazily. (Compiled out entirely
    // under --no-default-features: static/SPA/proxy serving without V8.)
    #[cfg(feature = "ssr")]
    {
        let ssr_opts = SsrSpawnOptions::new(
            workload_abs.clone(),
            workload_abs.join("dist"),
            state_dir.clone(),
        );
        match ssr::spawn(ssr_opts).await? {
            Some(child) => {
                info!(prefixes = ?child.prefixes(), "mesofact-dev ssr child attached");
                ssr_slot.set(Some(Arc::new(child)));
            }
            None => {
                info!("mesofact-dev: no SSR routes (or no manifest yet); static-only");
            }
        }
    }

    // Dev-tier S3 surface (R490-F7): host a local s3s-fs bucket so a workload's
    // @mesofact/runtime R2Adapter can resolve against it during dev instead of
    // real Cloudflare R2. Coords go to .mesofact-dev/s3.json for discovery and
    // into the build child's env below. (The in-process V8 SSR runtime can't
    // inherit process env, so wiring its runtime reads is a separate step —
    // tracked under R490-F7.)
    let dev_s3 = mesofact_dev::DevS3::start(state_dir.join("s3"), mesofact_dev::DEV_S3_BUCKET).await?;
    info!(endpoint = %dev_s3.endpoint, bucket = %dev_s3.bucket, "dev S3 surface ready");
    if let Err(e) = std::fs::write(
        state_dir.join("s3.json"),
        serde_json::json!({ "endpoint": dev_s3.endpoint, "bucket": dev_s3.bucket }).to_string(),
    ) {
        info!(error = %e, "dev S3: could not write s3.json discovery file");
    }

    if args.no_watch {
        info!("watch mode disabled");
        return server.serve(args.port).await;
    }

    let pointer = server.pointer();
    let mut opts = WatchOptions::defaults_for_workload(&args.workload);
    opts.initial_build = !args.no_initial_build;
    opts.build_env = dev_s3.env_vars();

    let watcher_obj = mesofact_dev::Watcher::new(args.workload.clone(), pointer, opts);

    // Post-build hook: each successful rebuild rotates dist into
    // .mesofact-dev/gen-N/, so the SSR runtime must be re-spawned against
    // the new gen dir — V8's module cache would otherwise keep serving the
    // old route entrypoints. Under R449-F2 the in-process model swaps the
    // whole SsrChild in the slot (no SIGKILL/respawn dance the bun era
    // needed); the prior Arc<SsrChild> drops, which joins the isolate
    // thread.
    #[cfg(feature = "ssr")]
    let watcher_obj = {
        let slot_for_hook = ssr_slot.clone();
        let workload_for_hook = workload_abs.clone();
        let state_dir_for_hook = state_dir.clone();
        let hook: watcher::PostBuildFn = Box::new(move |gen_dir: PathBuf| {
            let slot = slot_for_hook.clone();
            let workload = workload_for_hook.clone();
            let state_dir = state_dir_for_hook.clone();
            Box::pin(async move {
                let opts = SsrSpawnOptions::new(workload, gen_dir, state_dir);
                match ssr::spawn(opts).await? {
                    Some(child) => {
                        info!(
                            prefixes = ?child.prefixes(),
                            "mesofact-dev ssr runtime re-spawned against new gen",
                        );
                        slot.set(Some(Arc::new(child)));
                    }
                    None => {
                        // Manifest declares no SSR routes; clear any prior child.
                        slot.set(None);
                    }
                }
                Ok(())
            })
        });
        watcher_obj.with_post_build(hook)
    };

    let watcher_task = watcher::spawn(watcher_obj);

    // Server owns the foreground; the spawned watcher continues until the
    // process exits.
    let result = server.serve(args.port).await;
    drop(watcher_task);
    result
}
