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
use mesofact_dev::{ssr, Server, SsrSpawnOptions};
use tracing::{info, warn};

/// Default port the SSR host binds inside the container. Matches
/// `local_driver::pond_ssr_runtime::DEFAULT_SSR_CONTAINER_PORT`.
const DEFAULT_SERVE_PORT: u16 = 3000;

#[derive(Parser, Debug)]
#[command(version, about = "deno_core SSR-host runtime for mesofact-static workloads")]
struct Args {
    /// Workload directory — the parent of `dist/` (with `dist/html/` and
    /// `manifest.json`). Bind-mounted into the container by yubaba.
    workload: PathBuf,

    /// TCP port to bind.
    #[arg(long, default_value_t = DEFAULT_SERVE_PORT)]
    port: u16,

    /// Address to bind. Defaults to `0.0.0.0` so sibling containers can reach
    /// the host; pass `127.0.0.1` to restrict to loopback.
    #[arg(long, default_value = "0.0.0.0")]
    host: IpAddr,
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
    let server = Server::from_workload(&args.workload)?;

    // Canonicalize so the isolate's manifest read + dynamic-import resolve
    // against absolute paths regardless of the container's working directory.
    let workload_abs = args
        .workload
        .canonicalize()
        .unwrap_or_else(|_| args.workload.clone());

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
