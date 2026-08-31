//! `mesofact serve` — the stock mesofact runtime subcommand: the W272 serve-bin
//! kamaji forks to serve a **bundle** (`mesofact serve --bundle <dir> --listen
//! <addr>`), plus the legacy pond/cloud SSR-host container mode.
//!
//! Two things this binary does NOT do (like `mesofact-dev`, unlike a full
//! build): no bundler, no watch. It serves an already-built tree.
//!
//! ## Bundle mode (R599-F3, W272 §3) — the v0 static tier
//!
//! `mesofact serve --bundle <cache-dir> --listen <addr>` serves a materialized
//! W272 bundle: `<bundle>/manifest.toml` + `<bundle>/app/dist/{html,manifest.json}`.
//! v0 is **static only** — clean-URLs + 404, no V8 — so it builds and runs with
//! the crate compiled `--no-default-features` (the `ssr` feature off), which is
//! how it dogfoods on the current glibc fleet ahead of the musl-static V8
//! runtime (W272 §5). Executing `mesofact.routes.ts` (SSR / islands) and the
//! on-demand JIT lifecycle are R599-F6 follow-on.
//!
//! ## SSR-host mode (R449-F3, behind the `ssr` feature)
//!
//! `mesofact serve <workload> --port 3000` binds a routable address
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
//! scanner). See [`crate::Server::from_bundle`].
//!
//! See the [library crate](crate) for the shared `Server` + `ssr`
//! machinery this binary composes.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use crate::Server;
use tracing::info;
#[cfg(feature = "ssr")]
use tracing::warn;

/// Default port the SSR host binds inside the container. Matches
/// `local_driver::pond_ssr_runtime::DEFAULT_SSR_CONTAINER_PORT`.
const DEFAULT_SERVE_PORT: u16 = 3000;

#[derive(clap::Args, Debug)]
pub struct ServeArgs {
    /// Workload directory — the parent of `dist/` (with `dist/html/` and
    /// `manifest.json`). Bind-mounted into the container by yubaba. Used by the
    /// SSR-host and single-tenant `--revalidate` paths. Ignored when `--bundle`
    /// or `--tenants` is set.
    workload: Option<PathBuf>,

    /// Serve a materialized **W272 bundle** directory (`manifest.toml` +
    /// `app/dist/…`) as a static site — clean-URLs + 404, no V8 (R599-F3). This
    /// is the stock runtime's v0 tier; kamaji forks `mesofact serve --bundle
    /// <cache-dir> --listen <addr>` per W272 §3. Takes precedence over a
    /// positional `workload`.
    #[arg(long)]
    bundle: Option<PathBuf>,

    /// Full bind address (`host:port`), the W272 §3 form. When set it wins over
    /// `--host` / `--port`. Ignored when the process was handed a listen socket
    /// via `LISTEN_FDS` (socket-activation) — it then adopts that fd instead.
    #[arg(long)]
    listen: Option<SocketAddr>,

    /// On-demand ("serverless") JIT idle TTL, in seconds (R599-F6, bundle mode).
    /// After this long with no in-flight request the runtime self-exits; kamaji
    /// holds the listen socket and re-forks on the next connection. `0` / unset
    /// = keep-alive (resident). Meant to be set by kamaji from the bundle's
    /// `BundleLifecycle::OnDemand { idle_ttl }`.
    #[arg(long)]
    idle_ttl: Option<u64>,

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

    /// Receiver mode only: route allowlist — repeat the flag per route
    /// (`--allow-route /releases --allow-route /issues`). Unset = every
    /// render-eligible route in the manifest.
    ///
    /// Orthogonal to `--mirror-key`: this is what the receiver may re-render,
    /// not who may ask. A deployment may set either, both or neither.
    #[arg(long = "allow-route", value_name = "ROUTE")]
    allow_route: Vec<String>,

    /// Receiver mode only: directory of `tenants/<id>.toml` files. When set,
    /// runs the **multi-tenant** receiver — each poke's `mirror_key` selects the
    /// tenant whose workload + publish_config it revalidates. Requires the `ssr`
    /// build feature.
    ///
    /// Conflicts with the single-tenant receiver flags rather than quietly
    /// winning over them: every one of `workload` / `--publish-config` /
    /// `--allow-route` is per-tenant in this mode, and a silently-ignored
    /// `--allow-route` is an allowlist an operator believes is enforced.
    /// `--mirror-key` is only warned about — it carries `env =
    /// MESOFACT_MIRROR_KEY`, which a runner may have set process-wide for
    /// reasons that have nothing to do with this invocation.
    #[arg(long, conflicts_with_all = ["workload", "publish_config", "allow_route"])]
    tenants: Option<PathBuf>,

    /// Assert that an **authenticating edge** fronts this process, so routes
    /// declaring `requires: ["user"]` may be served (R556-B13).
    ///
    /// `serve` has no session resolver — that check lives only in
    /// `mesofact proxy`'s router — so a declared-authed route served by this
    /// binary is enforced by the edge and by nothing else. Without this flag,
    /// a workload declaring any such route makes `serve` refuse to start,
    /// naming the routes. Passing it is the operator stating, in the
    /// invocation, that (for example) passway's cheers-verify is in front.
    ///
    /// The env form is how the bundle tier will set it: kamaji forks `serve`
    /// with a deploy-resolved env, so this rides the same channel the R2
    /// credentials do rather than needing a new argv flag threaded through
    /// three crates.
    /// Accepts `1`/`true`/`yes`/`on` (and their negatives), case-insensitively,
    /// in both the flag and the env form. Plain `bool` with `env` would make
    /// clap demand literally `true`/`false`, so `MESOFACT_TRUST_EDGE_AUTH=1` —
    /// the form every operator and every deploy config reaches for first —
    /// would abort the process with a clap error. Empty reads as unset, so an
    /// exported-but-blank var fails closed rather than crashing the serve.
    #[arg(
        long,
        env = "MESOFACT_TRUST_EDGE_AUTH",
        num_args = 0..=1,
        default_value_t = false,
        default_missing_value = "true",
        value_parser = parse_truthy,
    )]
    trust_edge_auth: bool,
}

/// Parse a human/config truthy string. See [`ServeArgs::trust_edge_auth`].
fn parse_truthy(raw: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" | "no" | "off" => Ok(false),
        "1" | "true" | "yes" | "on" => Ok(true),
        other => Err(format!(
            "expected a boolean (1/true/yes/on or 0/false/no/off), got {other:?}"
        )),
    }
}

impl ServeArgs {
    /// Resolve the bind address: `--listen` wins, else `host:port`.
    fn bind_addr(&self) -> SocketAddr {
        self.listen
            .unwrap_or_else(|| SocketAddr::new(self.host, self.port))
    }
}

pub async fn run(args: ServeArgs) -> anyhow::Result<()> {
    // `mesofact` is the crate this binary's own code lives in (W174 folded the
    // `mesofact-serve` binary in here) — so it is the target every `info!` in
    // this file and in `crate::revalidate` carries. The default filter named
    // `mesofact_serve`, a crate that no longer exists, which meant a receiver
    // deployed with `RUST_LOG` unset dropped ALL of its own output: no
    // "listening" line at boot, no per-poke render/publish report, and no
    // `error!("revalidate failed")`. yah R330-T35 found the yah.dev receiver
    // running with two 0-byte log files and no way to tell a successful
    // re-render from a silent failure.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("mesofact=info,mesofact_dev=info,tower_http=info")
            }),
        )
        .init();


    // Bundle mode (R599-F3): the v0 static tier. Always available — no V8 — so
    // it is checked before any `ssr`-gated branch. R599-F6 layers the JIT
    // lifecycle here: adopt a handed-over listen socket + self-reap on idle.
    if let Some(bundle) = args.bundle.as_ref() {
        let bundle_abs = bundle.canonicalize().unwrap_or_else(|_| bundle.clone());
        let server = Server::from_bundle(&bundle_abs)?;
        // R556-B13: a bundle's app tree is `<bundle>/app`; its built route
        // manifest is what declares the auth gate this binary cannot enforce.
        assert_declared_auth_is_enforced(&bundle_abs.join("app"), args.trust_edge_auth)?;
        // R746-B7: a bundle can declare `mode:"ssr"` routes same as any other
        // workload. `Server::from_bundle` alone never attaches an isolate —
        // either attach one now (ssr feature) or refuse to serve routes this
        // binary cannot execute (static-only build), rather than silently
        // 404ing them one request at a time.
        let server = attach_bundle_ssr(server, &bundle_abs).await?;
        let idle_ttl = args.idle_ttl.filter(|s| *s > 0).map(Duration::from_secs);

        // Prefer an inherited socket-activation fd (kamaji's custodian handoff,
        // R599-F6/F9) over binding fresh, so the socket outlives this process.
        let listener = match socket_activation_listener()? {
            Some(l) => {
                info!(bundle = %bundle_abs.display(), "mesofact serve: serving bundle on inherited LISTEN_FDS socket");
                l
            }
            None => {
                let addr = args.bind_addr();
                info!(%addr, bundle = %bundle_abs.display(), "mesofact-serve listening (bundle, static v0)");
                tokio::net::TcpListener::bind(addr).await?
            }
        };
        return server.serve_on_listener(listener, idle_ttl).await;
    }

    run_workload_modes(args).await
}

/// R556-B13 — fail CLOSED on a route whose declared auth gate this binary does
/// not enforce.
///
/// `mesofact serve` has no session resolver. `requires: ["user"]` is checked
/// only by `mesofact_core::proxy::router` (the `mesofact proxy` subcommand),
/// and the W272 bundle tier forks `serve` — so before this, a route declaring
/// the gate was served to anything that reached the port, with nothing in any
/// log saying so. The manifest and the binary disagreed and the binary won
/// silently.
///
/// The remedy is a *statement*, not a resolver: auth for these surfaces is an
/// edge concern (passway terminates TLS and verifies cheers bearers), so the
/// operator asserts the edge is there with `--trust-edge-auth` /
/// `MESOFACT_TRUST_EDGE_AUTH`. Wiring a session resolver into `serve` instead
/// would need more than calling the router — `mesofact_ssr::DispatchRequest`
/// has no `user` field, so a resolved session has no channel into the V8
/// isolate (W225 §2b, measured by R637).
///
/// **Refusing to start** rather than 401-ing the route: a process that boots
/// and serves is the state an operator reads as "deployed and fine", and a
/// per-route 401 buried in a mixed site is easy to miss for weeks. A failed
/// fork is not. The cost is that one declared-authed route takes down a site
/// whose other routes are public — deliberate, and one flag away from being
/// exactly what the operator meant.
fn assert_declared_auth_is_enforced(
    workload: &std::path::Path,
    trust_edge_auth: bool,
) -> anyhow::Result<()> {
    let gated = crate::routes_requiring_user(workload).map_err(|e| {
        anyhow::anyhow!(
            "refusing to start: cannot read the route manifest under {} to check for \
             declared-authed routes: {e}",
            workload.display(),
        )
    })?;
    if gated.is_empty() {
        return Ok(());
    }
    if trust_edge_auth {
        info!(
            routes = ?gated,
            "serving routes that declare requires:[\"user\"] — this process does NOT enforce \
             that gate; --trust-edge-auth asserts an authenticating edge is in front",
        );
        return Ok(());
    }
    anyhow::bail!(
        "refusing to start: {} route(s) declare `requires: [\"user\"]` but `mesofact serve` \
         does not enforce it — {}. That check lives only in `mesofact proxy`'s router, so \
         serving these here would expose a confidential surface to anything that reaches the \
         port. Either front this process with an authenticating edge (passway cheers-verify) \
         and pass `--trust-edge-auth` / `MESOFACT_TRUST_EDGE_AUTH=1` to say so, or drop \
         `requires` from the route if it was never meant to be gated.",
        gated.len(),
        gated.join(", "),
    );
}

/// Attach an SSR isolate to a bundle server, same as the SSR-host path does
/// for a plain workload (serve.rs `run_workload_modes`). A bundle's app tree
/// is `<bundle>/app` (structurally a workload dir — see
/// [`Server::from_bundle`]), so this is `ssr::spawn` pointed there plus
/// `with_ssr`. `ssr::spawn` itself returns `Ok(None)` for a bundle with no
/// `mode:"ssr"` routes, so the common static-only bundle is unaffected.
///
/// R444: this is the prod receiver path, so hand the isolate the real process
/// env (yubaba-injected secrets) same as `run_workload_modes` does — the
/// isolate cannot inherit it on its own.
#[cfg(feature = "ssr")]
async fn attach_bundle_ssr(server: crate::Server, bundle: &std::path::Path) -> anyhow::Result<crate::Server> {
    use crate::{ssr, SsrSpawnOptions};

    let app = bundle.join("app");
    let opts = SsrSpawnOptions::new(app.clone(), app.join("dist"), app.join(".mesofact-serve"))
        .with_env(std::env::vars().collect());
    Ok(match ssr::spawn(opts).await? {
        Some(child) => {
            info!(prefixes = ?child.prefixes(), "mesofact serve --bundle: ssr runtime attached");
            server.with_ssr(child)
        }
        None => server,
    })
}

/// Static-only build (no V8): refuse rather than silently 404 every
/// `mode:"ssr"` route in the bundle. R746-B7's minimum-acceptable interim —
/// a loud refusal at apply time beats a 404 at request time that looks like a
/// routing typo (same discipline `assert_declared_auth_is_enforced` /
/// R330-B43 already paid for once).
#[cfg(not(feature = "ssr"))]
async fn attach_bundle_ssr(server: crate::Server, bundle: &std::path::Path) -> anyhow::Result<crate::Server> {
    let app = bundle.join("app");
    let ssr_routes = crate::routes_declaring_ssr(&app).map_err(|e| {
        anyhow::anyhow!(
            "refusing to start: cannot read the route manifest under {} to check for \
             mode:\"ssr\" routes: {e}",
            app.display(),
        )
    })?;
    if ssr_routes.is_empty() {
        return Ok(server);
    }
    anyhow::bail!(
        "refusing to start: {} route(s) declare mode:\"ssr\" ({}) but this `mesofact serve` \
         binary was built without the `ssr` feature (no V8) — it can only serve them as a 404. \
         Serve this bundle with an ssr-enabled build (the shipped stock runtime is built \
         `--features deploy`), or drop the ssr route(s) if this bundle is meant to be static.",
        ssr_routes.len(),
        ssr_routes.join(", "),
    );
}

/// Adopt a listening socket handed over via the systemd socket-activation
/// convention (`LISTEN_FDS` + `LISTEN_PID`; first fd is `SD_LISTEN_FDS_START`
/// = 3). This is how kamaji's JIT custodian (R599-F6/F9) hands mesofact-serve
/// the socket it binds+holds — plain `LISTEN_FDS` inheritance, since
/// mesofact-serve is our own binary (no pingora upgrade-socket dance). Returns
/// `None` when no fd was passed (the normal `--listen`/bind path).
#[cfg(unix)]
fn socket_activation_listener() -> anyhow::Result<Option<tokio::net::TcpListener>> {
    use std::os::fd::FromRawFd;

    let n_fds: i32 = std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if n_fds < 1 {
        return Ok(None);
    }
    // If LISTEN_PID names a process, honor it — the fd was for that pid, and a
    // grandchild fork shouldn't accidentally adopt a socket meant for its parent.
    if let Ok(pid) = std::env::var("LISTEN_PID") {
        if pid.parse::<u32>().ok() != Some(std::process::id()) {
            return Ok(None);
        }
    }
    const SD_LISTEN_FDS_START: i32 = 3;
    // SAFETY: the socket-activation contract guarantees fd 3.. are the passed
    // listening sockets and that we are their sole owner; we take exclusive
    // ownership of exactly one and never touch fd 3 again.
    let std_listener = unsafe { std::net::TcpListener::from_raw_fd(SD_LISTEN_FDS_START) };
    std_listener
        .set_nonblocking(true)
        .map_err(|e| anyhow::anyhow!("set_nonblocking on inherited LISTEN_FDS socket: {e}"))?;
    let listener = tokio::net::TcpListener::from_std(std_listener)
        .map_err(|e| anyhow::anyhow!("adopting inherited LISTEN_FDS socket: {e}"))?;
    Ok(Some(listener))
}

#[cfg(not(unix))]
fn socket_activation_listener() -> anyhow::Result<Option<tokio::net::TcpListener>> {
    Ok(None)
}

/// The V8-backed modes (SSR host + revalidate/tenants receivers). Compiled only
/// with the `ssr` feature; without it, any of these invocations is a clear
/// error instead of a silent static fallthrough.
#[cfg(feature = "ssr")]
async fn run_workload_modes(args: ServeArgs) -> anyhow::Result<()> {
    use crate::{revalidate, ssr, tenants, SsrSpawnOptions};

    // `--listen host:port` is the W272 canonical bind form and the one kamaji
    // passes when it forks a receiver alongside a bundle's static server. The
    // receiver modes used to read `--host`/`--port` directly, so a `--listen`
    // was silently ignored and the receiver bound 0.0.0.0:3000 — publicly, and
    // on whatever port the caller thought it had moved off of. Resolve through
    // `bind_addr()` so all modes agree on one precedence.
    let addr = args.bind_addr();

    // Multi-tenant receiver (R446): a tenants/<id>.toml registry, one process
    // hosting many surfaces. Each poke's mirror_key selects its tenant. Takes
    // precedence over the single-tenant receiver and needs no `workload`.
    if let Some(tenants_dir) = args.tenants.as_ref() {
        if !args.revalidate {
            warn!("--tenants implies the revalidate receiver; running multi-tenant receiver");
        }
        if args.mirror_key.is_some() {
            warn!(
                "--mirror-key / MESOFACT_MIRROR_KEY is ignored in --tenants mode — \
                 each tenant's bearer comes from its own mirror_key_env",
            );
        }
        let files = tenants::load_tenants(tenants_dir)?;
        let resolved = tenants::resolve_tenants(files, |name| std::env::var(name).ok());
        let registry = tenants::TenantRegistry::new(resolved);
        // An empty registry is a receiver that 403s every poke while looking
        // perfectly healthy on /readyz — the exact silent-failure shape yah
        // R330-T35 spent a day on. A missing or empty `--tenants` dir is a typo
        // or a bad mount, never an intended deployment, so refuse to boot.
        if registry.is_empty() {
            anyhow::bail!(
                "--tenants {} contains no tenants/<id>.toml files — a receiver with an \
                 empty registry rejects every poke",
                tenants_dir.display()
            );
        }
        // Two tenants resolving to the SAME bearer means one of them silently
        // wins every poke — and "wins" here is publishing to a bucket the poke
        // did not authorize. Fail at boot, not at the first cross-published page.
        registry.validate()?;
        info!(tenants = registry.len(), dir = %tenants_dir.display(), "multi-tenant revalidate receiver");
        return tenants::serve(registry, addr.ip(), addr.port()).await;
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
                routes: args.allow_route,
            },
            addr.ip(),
            addr.port(),
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

    // R556-B13. Checked on the SSR-host path too, not just the bundle tier:
    // `mode: "ssr"` + `requires: ["user"]` is the exact shape the gate is for,
    // and this path is the one that actually renders it.
    assert_declared_auth_is_enforced(&workload_abs, args.trust_edge_auth)?;

    // Boot the SSR isolate against the already-built dist/. `ssr::spawn`
    // returns Ok(None) for static/SPA-only workloads (no `mode:"ssr"` route or
    // no manifest yet); those serve static only with no isolate.
    // R444: the receiver's own process env is real (yubaba injects secrets
    // into it directly), so hand it straight through — the isolate can't
    // inherit process env on its own, but this process already has it.
    let opts = SsrSpawnOptions::new(
        workload_abs.clone(),
        workload_abs.join("dist"),
        workload_abs.join(".mesofact-serve"),
    )
    .with_env(std::env::vars().collect());
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
async fn run_workload_modes(args: ServeArgs) -> anyhow::Result<()> {
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
    // R556-B13: the static-only build has even less chance of enforcing the
    // gate than the ssr one — no isolate, no router, nothing that reads a
    // session. Same refusal.
    assert_declared_auth_is_enforced(&workload, args.trust_edge_auth)?;
    let addr = args.bind_addr();
    info!(%addr, workload = %workload.display(), "mesofact-serve listening (static only, no ssr)");
    server.serve_on(addr).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workload_with_manifest(json: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("manifest.json"), json).unwrap();
        dir
    }

    const AUTHED: &str = r#"{"routes":[{"route":"/","mode":"ssr","requires":["user"]}]}"#;

    /// R556-B13, the bug itself: a route declaring `requires: ["user"]` was
    /// served by `mesofact serve` to anything that reached the port, because
    /// the check lives only in `mesofact proxy`'s router. Fail closed.
    #[test]
    fn a_declared_authed_route_refuses_to_start_without_an_asserted_edge() {
        let dir = workload_with_manifest(AUTHED);
        let err = assert_declared_auth_is_enforced(dir.path(), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("refusing to start"), "{err}");
        assert!(
            err.contains('/'),
            "the message must name the route the operator has to look at: {err}"
        );
        assert!(
            err.contains("--trust-edge-auth"),
            "and the remedy, or the operator has a refusal with no next move: {err}"
        );
    }

    /// The remedy: the operator states that an authenticating edge (passway
    /// cheers-verify) is in front. That is a claim this process cannot verify,
    /// which is exactly why it has to be said out loud in the invocation.
    #[test]
    fn an_asserted_edge_allows_the_declared_authed_route() {
        let dir = workload_with_manifest(AUTHED);
        assert!(assert_declared_auth_is_enforced(dir.path(), true).is_ok());
    }

    /// The overwhelmingly common case — no route declares the gate — must not
    /// need the flag. Every mesofact site in tree except analytics is this.
    #[test]
    fn a_workload_declaring_no_authed_route_starts_unchanged() {
        let dir = workload_with_manifest(r#"{"routes":[{"route":"/","mode":"static"}]}"#);
        assert!(assert_declared_auth_is_enforced(dir.path(), false).is_ok());
        // …and so does one with nothing built yet.
        let empty = tempfile::tempdir().unwrap();
        assert!(assert_declared_auth_is_enforced(empty.path(), false).is_ok());
    }

    /// `MESOFACT_TRUST_EDGE_AUTH=1` is the form a deploy config reaches for
    /// first, and a plain `#[arg(long, env)] bool` rejects it with a clap error
    /// that aborts the process. Measured against the real analytics workload
    /// before this parser existed.
    #[test]
    fn the_edge_assertion_accepts_the_truthy_forms_an_operator_will_type() {
        for yes in ["1", "true", "TRUE", "yes", "on", " true "] {
            assert_eq!(parse_truthy(yes), Ok(true), "{yes:?}");
        }
        for no in ["", "0", "false", "no", "off"] {
            assert_eq!(parse_truthy(no), Ok(false), "{no:?}");
        }
        // An exported-but-blank var reads as unset, so it fails CLOSED rather
        // than crashing a serve that would otherwise have refused anyway.
        assert_eq!(parse_truthy(""), Ok(false));
        assert!(parse_truthy("maybe").is_err());
    }

    /// A manifest that exists but does not parse is a refusal, not a shrug:
    /// "we could not read the file, therefore nothing is gated" is the same
    /// fail-open wearing a different hat.
    #[test]
    fn an_unreadable_manifest_refuses_to_start() {
        let dir = workload_with_manifest("{ not json");
        let err = assert_declared_auth_is_enforced(dir.path(), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("refusing to start"), "{err}");
    }

    /// R746-B7, the bug itself: `serve --bundle` never attached an SSR
    /// isolate, so a `mode:"ssr"` route silently 404'd. On a static-only
    /// build there is no isolate to attach — refuse loudly instead, naming
    /// the route, rather than serving a 404 that looks like a routing typo.
    #[cfg(not(feature = "ssr"))]
    #[tokio::test]
    async fn a_bundle_declaring_ssr_refuses_to_start_on_a_static_only_build() {
        let bundle_dir = tempfile::tempdir().unwrap();
        let bundle = bundle_dir.path();
        let app = bundle.join("app");
        std::fs::create_dir_all(app.join("dist")).unwrap();
        std::fs::write(
            app.join("dist").join("manifest.json"),
            r#"{"routes":[{"route":"/live","mode":"ssr"},{"route":"/","mode":"static"}]}"#,
        )
        .unwrap();
        let server = crate::Server::from_workload(&app).unwrap();
        let err = match attach_bundle_ssr(server, bundle).await {
            Ok(_) => panic!("expected a refusal — bundle declares mode:\"ssr\""),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("refusing to start"), "{err}");
        assert!(err.contains("/live"), "must name the route: {err}");
        assert!(err.contains("ssr"), "and say why: {err}");
    }

    /// The overwhelmingly common bundle — no `mode:"ssr"` route — must not
    /// need an isolate and must not be refused on a static-only build.
    #[cfg(not(feature = "ssr"))]
    #[tokio::test]
    async fn a_static_only_bundle_starts_unchanged_on_a_static_only_build() {
        let bundle_dir = tempfile::tempdir().unwrap();
        let bundle = bundle_dir.path();
        let app = bundle.join("app");
        std::fs::create_dir_all(app.join("dist")).unwrap();
        std::fs::write(
            app.join("dist").join("manifest.json"),
            r#"{"routes":[{"route":"/","mode":"static"}]}"#,
        )
        .unwrap();
        let server = crate::Server::from_workload(&app).unwrap();
        assert!(attach_bundle_ssr(server, bundle).await.is_ok());
    }
}
