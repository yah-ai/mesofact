//! `mesofact-serve` — the stock mesofact runtime binary: the W272 serve-bin
//! kamaji forks to serve a **bundle** (`mesofact-serve --bundle <dir> --listen
//! <addr>`), plus the legacy pond/cloud SSR-host container mode.
//!
//! Two things this binary does NOT do (like `mesofact-dev`, unlike a full
//! build): no bundler, no watch. It serves an already-built tree.
//!
//! ## Bundle mode (R599-F3, W272 §3) — the v0 static tier
//!
//! `mesofact-serve --bundle <cache-dir> --listen <addr>` serves a materialized
//! W272 bundle: `<bundle>/manifest.toml` + `<bundle>/app/dist/{html,manifest.json}`.
//! v0 is **static only** — clean-URLs + 404, no V8 — so it builds and runs with
//! the crate compiled `--no-default-features` (the `ssr` feature off), which is
//! how it dogfoods on the current glibc fleet ahead of the musl-static V8
//! runtime (W272 §5). Executing `mesofact.routes.ts` (SSR / islands) and the
//! on-demand JIT lifecycle are R599-F6 follow-on.
//!
//! ## SSR-host mode (R449-F3, behind the `ssr` feature)
//!
//! `mesofact-serve <workload> --port 3000` binds a routable address
//! (`0.0.0.0` by default) and boots an in-process deno_core isolate for the
//! workload's `mode:"ssr"` routes; static fall-through serves from
//! `<workload>/dist/html/`. Also carries the `--revalidate` / `--tenants`
//! receiver modes (W225 §3/§4). All of this needs V8, so it is compiled only
//! when the `ssr` feature is enabled; without it, passing those flags is a
//! clear error rather than a silent no-op.
//!
//! Part of R599-F3 — the canonical `@yah:ticket(R599-F3, …)` annotation lives
//! in the parent-camp W272 doc (one block per ID; a second `@yah:` block in this
//! subcamp file would register a parent-camp R599 id against the mesofact board
//! scanner). See [`mesofact_dev::Server::from_bundle`].
//!
//! See the [library crate](mesofact_dev) for the shared `Server` + `ssr`
//! machinery this binary composes.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use clap::Parser;
use mesofact_dev::Server;
use tracing::info;
#[cfg(feature = "ssr")]
use tracing::warn;

/// Default port the SSR host binds inside the container. Matches
/// `local_driver::pond_ssr_runtime::DEFAULT_SSR_CONTAINER_PORT`.
const DEFAULT_SERVE_PORT: u16 = 3000;

#[derive(Parser, Debug)]
#[command(version, about = "mesofact runtime: serve a W272 bundle (static v0) or host SSR routes")]
struct Args {
    /// Workload directory — the parent of `dist/` (with `dist/html/` and
    /// `manifest.json`). Bind-mounted into the container by yubaba. Used by the
    /// SSR-host and single-tenant `--revalidate` paths. Ignored when `--bundle`
    /// or `--tenants` is set.
    workload: Option<PathBuf>,

    /// Serve a materialized **W272 bundle** directory (`manifest.toml` +
    /// `app/dist/…`) as a static site — clean-URLs + 404, no V8 (R599-F3). This
    /// is the stock runtime's v0 tier; kamaji forks `mesofact-serve --bundle
    /// <cache-dir> --listen <addr>` per W272 §3. Takes precedence over a
    /// positional `workload`.
    #[arg(long)]
    bundle: Option<PathBuf>,

    /// Full bind address (`host:port`), the W272 §3 form. When set it wins over
    /// `--host` / `--port`.
    #[arg(long)]
    listen: Option<SocketAddr>,

    /// TCP port to bind (when `--listen` is not given).
    #[arg(long, default_value_t = DEFAULT_SERVE_PORT)]
    port: u16,

    /// Address to bind (when `--listen` is not given). Defaults to `0.0.0.0` so
    /// sibling containers can reach the host; pass `127.0.0.1` for loopback.
    #[arg(long, default_value = "0.0.0.0")]
    host: IpAddr,

    /// Run the **revalidate receiver** instead of serving (W225 §3/§4 — the
    /// mesofact-native replacement for almanac-serve). Ephemeral: each `POST
    /// /revalidate` poke boots V8, re-renders + republishes, then drops the
    /// isolate. Requires the `ssr` build feature.
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
    /// tenant whose workload + publish_config it revalidates. Requires the `ssr`
    /// build feature.
    #[arg(long)]
    tenants: Option<PathBuf>,
}

impl Args {
    /// Resolve the bind address: `--listen` wins, else `host:port`.
    fn bind_addr(&self) -> SocketAddr {
        self.listen
            .unwrap_or_else(|| SocketAddr::new(self.host, self.port))
    }
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

    // Bundle mode (R599-F3): the v0 static tier. Always available — no V8 — so
    // it is checked before any `ssr`-gated branch.
    if let Some(bundle) = args.bundle.as_ref() {
        let bundle_abs = bundle.canonicalize().unwrap_or_else(|_| bundle.clone());
        let server = Server::from_bundle(&bundle_abs)?;
        let addr = args.bind_addr();
        info!(%addr, bundle = %bundle_abs.display(), "mesofact-serve listening (bundle, static v0)");
        return server.serve_on(addr).await;
    }

    run_workload_modes(args).await
}

/// The V8-backed modes (SSR host + revalidate/tenants receivers). Compiled only
/// with the `ssr` feature; without it, any of these invocations is a clear
/// error instead of a silent static fallthrough.
#[cfg(feature = "ssr")]
async fn run_workload_modes(args: Args) -> anyhow::Result<()> {
    use mesofact_dev::{revalidate, ssr, tenants, SsrSpawnOptions};

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
        .ok_or_else(|| anyhow::anyhow!("a <workload> dir is required for the SSR host (or --bundle for static serving)"))?;
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

    let addr = args.bind_addr();
    info!(%addr, workload = %workload_abs.display(), "mesofact-serve listening");
    server.serve_on(addr).await
}

/// Static-only build (`--no-default-features`): the SSR host + receiver modes
/// aren't compiled in. A bare `mesofact-serve <workload>` still serves that
/// workload's static tree; the V8-only flags are a hard error rather than a
/// silent no-op.
#[cfg(not(feature = "ssr"))]
async fn run_workload_modes(args: Args) -> anyhow::Result<()> {
    if args.revalidate || args.tenants.is_some() {
        anyhow::bail!(
            "--revalidate / --tenants need the `ssr` build feature (V8); this is a static-only build"
        );
    }
    let workload = args
        .workload
        .clone()
        .ok_or_else(|| anyhow::anyhow!("a <workload> dir is required (or --bundle to serve a W272 bundle)"))?;
    let server = Server::from_workload(&workload)?;
    let addr = args.bind_addr();
    info!(%addr, workload = %workload.display(), "mesofact-serve listening (static only, no ssr)");
    server.serve_on(addr).await
}
