//! `mesofact-serve` — the SSR-host runtime for the pond/cloud `ssr_runtime`
//! container (W174 pillar 4 / R449-F3). Replaces the `bun run src/ssr.ts`
//! container with an in-process deno_core isolate.
//!
//! Unlike `mesofact-dev`, this binary does **no** build and **no** watch: it
//! serves a directory that already contains a built `dist/` (+ `manifest.json`)
//! and binds a routable address (`0.0.0.0` by default) so a sibling miniflare
//! container can proxy SSR-prefix requests to it over the pond docker bridge.
//!
//! Contract with yubaba's `pond_ssr_runtime` bring-up:
//! - The workload's built tree is bind-mounted into the container (e.g. at
//!   `/app`); the container CMD is `mesofact-serve /app --port 3000`.
//! - Static fall-through is served from `<dir>/dist/html/`; SSR-prefix routes
//!   (per `manifest.json`) dispatch to the isolate. A workload with no
//!   `mode:"ssr"` routes serves static only (the isolate is never booted).
//! - Readiness: yubaba probes `ready_path` (point it at `/__mesofact/health`
//!   for SSR-only sites that have no static `/`).
//!
//! See the [library crate](mesofact_dev) for the shared `Server` + `ssr`
//! machinery this binary composes.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use clap::Parser;
use mesofact_dev::{revalidate, ssr, tenants, Server, SsrSpawnOptions};
use tracing::{info, warn};

/// Default port the SSR host binds inside the container. Matches
/// `local_driver::pond_ssr_runtime::DEFAULT_SSR_CONTAINER_PORT`.
const DEFAULT_SERVE_PORT: u16 = 3000;

#[derive(Parser, Debug)]
#[command(version, about = "deno_core SSR-host runtime for mesofact-static workloads")]
struct Args {
    /// Workload directory — the parent of `dist/` (with `dist/html/` and
    /// `manifest.json`). Bind-mounted into the container by yubaba. Required for
    /// the SSR-host and single-tenant `--revalidate` paths; ignored (and
    /// optional) when `--tenants` selects the multi-tenant receiver.
    workload: Option<PathBuf>,

    /// TCP port to bind.
    #[arg(long, default_value_t = DEFAULT_SERVE_PORT)]
    port: u16,

    /// Address to bind. Defaults to `0.0.0.0` so sibling containers can reach
    /// the host; pass `127.0.0.1` to restrict to loopback.
    #[arg(long, default_value = "0.0.0.0")]
    host: IpAddr,

    /// Run the **revalidate receiver** instead of the SSR host (W225 §3/§4 —
    /// the mesofact-native replacement for almanac-serve). Ephemeral: no
    /// resident isolate; each `POST /revalidate` poke boots V8, re-renders the
    /// workload's routes against current data, republishes to the CDN, then
    /// drops the isolate. Does not serve the site (the CDN does).
    #[arg(long)]
    revalidate: bool,

    /// Receiver mode only: `mesofact.config.toml` carrying the `[publish]`
    /// block (bucket / zone / env-named credentials).
    #[arg(long, default_value = "mesofact.config.toml")]
    publish_config: PathBuf,

    /// Receiver mode only: shared bearer secret a poke must carry to be
    /// accepted (cross-mirror-pollution guard). Falls back to the
    /// `MESOFACT_MIRROR_KEY` env var; unset on both = open receiver.
    #[arg(long, env = "MESOFACT_MIRROR_KEY")]
    mirror_key: Option<String>,

    /// Receiver mode only: directory of `tenants/<id>.toml` files. When set,
    /// runs the **multi-tenant** receiver — each poke's `mirror_key` selects the
    /// tenant whose workload + publish_config it revalidates. Mutually exclusive
    /// with the single-tenant `workload` / `--publish-config` / `--mirror-key`.
    #[arg(long)]
    tenants: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("mesofact_serve=info,mesofact_dev=info,tower_http=info")
            }),
        )
        .init();

    let args = Args::parse();

    // Multi-tenant receiver (R446): a tenants/<id>.toml registry, one process
    // hosting many surfaces. Each poke's mirror_key selects its tenant. Takes
    // precedence over the single-tenant receiver and needs no `workload`.
    if let Some(tenants_dir) = args.tenants.as_ref() {
        if !args.revalidate {
            warn!("--tenants implies the revalidate receiver; running multi-tenant receiver");
        }
        let files = tenants::load_tenants(tenants_dir)?;
        let resolved = tenants::resolve_tenants(files, |name| std::env::var(name).ok());
        let registry = tenants::TenantRegistry::new(resolved);
        info!(tenants = registry.len(), dir = %tenants_dir.display(), "multi-tenant revalidate receiver");
        return tenants::serve(registry, args.host, args.port).await;
    }

    // Receiver mode (W225 §4): ephemeral render → publish, no resident isolate,
    // no static serving. Branches away from the SSR-host path entirely.
    if args.revalidate {
        let workload = args
            .workload
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--revalidate needs a <workload> dir (or use --tenants)"))?;
        let workload_abs = workload.canonicalize().unwrap_or(workload);
        return revalidate::serve(
            revalidate::RevalidateConfig {
                workload: workload_abs,
                publish_config: args.publish_config,
                mirror_key: args.mirror_key,
            },
            args.host,
            args.port,
        )
        .await;
    }

    let workload = args
        .workload
        .clone()
        .ok_or_else(|| anyhow::anyhow!("a <workload> dir is required for the SSR host"))?;
    let server = Server::from_workload(&workload)?;

    // Canonicalize so the isolate's manifest read + dynamic-import resolve
    // against absolute paths regardless of the container's working directory.
    let workload_abs = workload.canonicalize().unwrap_or(workload);

    // Boot the SSR isolate against the already-built dist/. `ssr::spawn`
    // returns Ok(None) for static/SPA-only workloads (no `mode:"ssr"` route or
    // no manifest yet); those serve static only with no isolate.
    let opts = SsrSpawnOptions::new(
        workload_abs.clone(),
        workload_abs.join("dist"),
        workload_abs.join(".mesofact-serve"),
    );
    let server = match ssr::spawn(opts).await? {
        Some(child) => {
            info!(prefixes = ?child.prefixes(), "mesofact-serve ssr runtime attached");
            server.with_ssr(child)
        }
        None => {
            warn!("mesofact-serve: no SSR routes (or no manifest) — serving static only");
            server
        }
    };

    let addr = SocketAddr::new(args.host, args.port);
    info!(%addr, workload = %workload_abs.display(), "mesofact-serve listening");
    server.serve_on(addr).await
}
