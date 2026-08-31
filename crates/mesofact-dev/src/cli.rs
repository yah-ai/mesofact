//! `mes` — the mesofact dev toolchain.
//!
//! The whole CLI lives here in the library rather than in a bin target,
//! mirroring [`mesofact::cli`], so that the two bin targets — `mes` and the
//! transitional `mesofact-dev` alias — are each three lines over one
//! definition. (They cannot simply share one `main.rs`: cargo warns when a
//! file backs two targets, and it compiles the whole CLI twice.)
//!
//! ## Why this is a superset of `mesofact`, not an alias of it
//!
//! `mes` carries the prod verbs (`serve`, `publish`) alongside the dev ones
//! (`dev`, `build`), so a consumer learns one CLI. That does **not** weaken
//! the W225 §2 dev/prod boundary, because that boundary is about which crates
//! land in a *binary*, not about which verbs a binary spells. It only ever
//! runs one direction: this crate already depends on the `mesofact` facade, so
//! `mes serve` is an in-process call into [`mesofact::cli`] — free. The
//! reverse (making the shipped `mesofact` binary a shim over `mes`) would drag
//! the watcher, the dev S3 surface and rolldown into the prod closure, and is
//! exactly what §2 forbids. `mesofact` stays a standalone binary.
//!
//! Two verbs are deliberately absent:
//!
//! - **`proxy`** — Mode-2 deployment plumbing (worker pool, manifest reload on
//!   SIGHUP). It has no dev-loop meaning; `mesofact proxy` remains its home.
//! - **`check`** (`tsc --noEmit`) — running TypeScript needs node or bun on
//!   `PATH`, and the entire point of the build path is that it does not. A
//!   `mes check` that dies with `bun: command not found` on precisely the
//!   machines this design promises don't need bun would be a worse affordance
//!   than the `npm run typecheck` the scaffold already documents.
//!
//! ## Bare invocation
//!
//! `mes` with no subcommand is `mes dev .` — the 95% command, so it is what
//! you get for typing the least. This is also what keeps the legacy
//! `mesofact-dev <DIR> --port N …` form working verbatim: it parses as the
//! bare form, so the ~30 spawn sites in the parent camp need no flag day. The
//! one divergence is a workload directory literally named after a subcommand
//! (`mesofact-dev serve`); spell it `./serve` if that ever comes up.

use std::path::PathBuf;
#[cfg(feature = "ssr")]
use std::sync::Arc;

use clap::{Parser, Subcommand};
#[cfg(feature = "ssr")]
use mesofact::{ssr, SsrSpawnOptions};
use mesofact::{Server, DEFAULT_PORT};
use crate::{watcher, WatchOptions};
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "mes",
    version,
    about = "mes — the mesofact dev toolchain",
    long_about = "Build, serve and ship a mesofact project. `mes` on its own runs the \
                  hot-reload dev loop in the current directory.\n\n\
                  The `serve` and `publish` verbs are the same code the shipped `mesofact` \
                  binary runs; `dev` and `build` are dev-tier and exist only here.",
    // A subcommand and the bare-form args are mutually exclusive: `mes dev .`
    // must not also try to bind `.` to the top-level positional.
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Bare form — `mes [DIR] [--port …]` is `mes dev [DIR] [--port …]`.
    #[command(flatten)]
    dev: DevArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Hot-reload dev loop: build in-process, serve, rebuild on every edit.
    Dev(DevArgs),
    /// One-shot production build into `dist/`. No watcher, no server.
    #[cfg(feature = "build")]
    Build(BuildArgs),
    /// Scaffold a new mesofact project pinned to this binary's version.
    New(mesofact::cli::new::NewArgs),
    /// Serve a built bundle or host SSR routes — the prod serving path.
    Serve(mesofact::cli::serve::ServeArgs),
    /// Upload a built dist/ tree, swap the manifest pointer, purge CDN tags.
    #[cfg(feature = "publish")]
    Publish(mesofact::cli::publish::PublishArgs),
}

/// One-shot build (`mes build`).
///
/// Drives [`mesofact::build::pipeline`] directly rather than going through
/// [`crate::Watcher::rebuild`]: `rebuild` additionally snapshots into a
/// `.mesofact-dev/gen-N/` directory and swaps the live pointer, which is the
/// watcher's business. A one-shot build should leave `dist/` and nothing else.
#[cfg(feature = "build")]
#[derive(clap::Args, Debug)]
struct BuildArgs {
    /// Project directory — the parent of `mesofact.routes.ts`.
    #[arg(default_value = ".")]
    workload: PathBuf,

    /// Output directory. Defaults to `<workload>/dist`.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct DevArgs {
    /// Workload directory — the parent of `dist/html/` (e.g. `app/yah/web`).
    /// Defaults to the current directory.
    #[arg(default_value = ".")]
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

    /// Logical service this dev server serves (R602-B4). Paired with
    /// `--component`, it's surfaced at `/__mesofact/info` so the cloud
    /// reconciler can confirm a listener on the configured port is *this*
    /// server before adopting it — refusing a colliding foreign listener
    /// instead of silently hijacking it.
    #[arg(long, requires = "component")]
    service: Option<String>,

    /// Logical component id this dev server serves (R602-B4). See `--service`.
    #[arg(long, requires = "service")]
    component: Option<String>,
}

pub async fn run() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("mesofact_dev=info,tower_http=info")
            }),
        )
        .init();

    let command = match resolve(Cli::parse()) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("mes: {err:#}");
            return std::process::ExitCode::FAILURE;
        }
    };

    match command {
        Command::Dev(args) => to_exit_code(dev(args).await),
        #[cfg(feature = "build")]
        Command::Build(args) => to_exit_code(build(args).await),
        Command::New(args) => to_exit_code(mesofact::cli::new::run(args)),
        Command::Serve(args) => to_exit_code(mesofact::cli::serve::run(args).await),
        // `publish` owns its exit codes (2 = missing config, etc.) — pass through.
        #[cfg(feature = "publish")]
        Command::Publish(args) => mesofact::cli::publish::run(args).await,
    }
}

/// Apply the bare-form fallthrough: no subcommand means `dev`.
///
/// Guards the one sharp edge a default subcommand buys. `mes --port 3000 dev`
/// parses as *"run dev against a directory named `dev`"* — clap fills the
/// positional because the leading flag already moved the parser past the
/// subcommand slot. Silently serving the wrong directory is a poor answer to
/// an obvious word-order typo, so name it instead. The subcommand list comes
/// from clap rather than a literal, so a verb added above cannot forget to
/// appear here.
fn resolve(cli: Cli) -> anyhow::Result<Command> {
    use clap::CommandFactory;

    if let Some(command) = cli.command {
        return Ok(command);
    }
    if let Some(name) = Cli::command()
        .get_subcommands()
        .map(clap::Command::get_name)
        .find(|name| std::path::Path::new(name) == cli.dev.workload)
    {
        anyhow::bail!(
            "`{name}` is a subcommand, but a flag came first so it was read as a \
             directory. Put the subcommand first: `mes {name} …` \
             (or say `./{name}` if you did mean the directory)."
        );
    }
    Ok(Command::Dev(cli.dev))
}

fn to_exit_code(result: anyhow::Result<()>) -> std::process::ExitCode {
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mes: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// `mes build` — run the pipeline once and stop.
#[cfg(feature = "build")]
async fn build(args: BuildArgs) -> anyhow::Result<()> {
    let project_root = args
        .workload
        .canonicalize()
        .unwrap_or_else(|_| args.workload.clone());
    let out_dir = args.out_dir.unwrap_or_else(|| project_root.join("dist"));
    info!(root = %project_root.display(), out = %out_dir.display(), "mes build");
    mesofact::build::pipeline::build(mesofact::build::pipeline::BuildOptions {
        project_root,
        out_dir: Some(out_dir),
        build_id: None,
        // Same `Auto` as the watcher: materialize `node_modules` from the
        // lockfile only when missing, so the build needs no package manager.
        install: mesofact::build::pipeline::InstallMode::Auto,
    })
    .await?;
    Ok(())
}

/// `mes dev` — the hot-reload loop. This is the whole of what the
/// `mesofact-dev` binary used to be.
async fn dev(args: DevArgs) -> anyhow::Result<()> {
    let mut server = Server::from_workload(&args.workload)?;

    // Logical identity (R602-B4): stamp the server with the (service, component)
    // the reconciler spawned it for so `/__mesofact/info` lets the adopt path
    // verify the port holds *this* server. clap's `requires` couples the pair,
    // so both-or-neither is guaranteed here.
    if let (Some(service), Some(component)) = (&args.service, &args.component) {
        info!(service, component, "mesofact-dev: identity stamped (R602-B4)");
        server = server.with_identity(service.clone(), component.clone());
    }

    // Same-origin reverse proxy (R513-F10): forward `/auth/*` etc. to the
    // camp-vended backend ports so the dashboard E2E (Option B) browser stays
    // single-origin. No map → no proxy (the Option A static path is unchanged).
    if let Some(map_path) = &args.proxy_map {
        let map = mesofact::ProxyMap::from_json_file(map_path)?;
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

    // Dev-tier S3 surface (R490-F7): host a local s3s-fs bucket so a workload's
    // @mesofact/runtime R2Adapter can resolve against it during dev instead of
    // real Cloudflare R2. Coords go to .mesofact-dev/s3.json for discovery,
    // into the build child's env below, AND (R444) into the in-process SSR
    // isolate's env — started before the SSR spawn below so the first boot
    // already has coordinates, not just post-build respawns.
    let dev_s3 = crate::DevS3::start(state_dir.join("s3"), crate::DEV_S3_BUCKET).await?;
    info!(endpoint = %dev_s3.endpoint, bucket = %dev_s3.bucket, "dev S3 surface ready");

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
        )
        .with_env(dev_s3.env_vars());
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

    // Instance-addressed (deferred) route resolution (W270 §9): point an
    // S3Store at the dev-S3 surface so a static miss on a `prerender:
    // { deferred: true }` route resolves through the pointer store against the
    // same local bucket the publisher flips into — the local mirror of the edge
    // worker's R2 resolution. Region "auto" + dummy creds match R2 / the
    // anonymous dev surface (s3s skips signature verification).
    #[cfg(feature = "ssr")]
    {
        use mesofact_publisher::{ObjectStore, S3Store};
        match S3Store::new(dev_s3.endpoint.clone(), dev_s3.bucket.clone(), "auto", "dev", "dev") {
            Ok(store) => {
                server = server.with_instance_store(Arc::new(store) as Arc<dyn ObjectStore>);
                info!("mesofact-dev: instance-addressed route resolution wired to dev S3 (W270 §9)");
            }
            Err(e) => {
                info!(error = %e, "mesofact-dev: could not build dev pointer store; deferred routes 404")
            }
        }
    }

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
    // `workload_abs`, not `args.workload` (R759-T4). The watcher's paths flow
    // into the post-build hook's `gen_dir`, and the SSR pool registers each
    // entrypoint as a `file://` module URL — which cannot be built from a
    // relative path. Running `mesofact-dev .` (the invocation the scaffold's
    // README gives, and the natural one from inside a project) therefore lost
    // EVERY `mode: "ssr"` route: the initial spawn logs "no SSR routes" because
    // no manifest exists yet, the post-build respawn then fails, and the only
    // trace is one WARN about a "publish hook" that has nothing to do with
    // publishing. Routes 404 as if they were never declared.
    let mut opts = WatchOptions::defaults_for_workload(&workload_abs);
    opts.initial_build = !args.no_initial_build;
    opts.build_env = dev_s3.env_vars();

    let watcher_obj = crate::Watcher::new(workload_abs.clone(), pointer, opts);

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
        let dev_s3_for_hook = dev_s3.clone();
        let hook: watcher::PostBuildFn = Box::new(move |gen_dir: PathBuf| {
            let slot = slot_for_hook.clone();
            let workload = workload_for_hook.clone();
            let state_dir = state_dir_for_hook.clone();
            let env = dev_s3_for_hook.env_vars();
            Box::pin(async move {
                let opts = SsrSpawnOptions::new(workload, gen_dir, state_dir).with_env(env);
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(argv: &[&str]) -> Cli {
        Cli::try_parse_from(argv).unwrap_or_else(|e| panic!("{argv:?} failed to parse:\n{e}"))
    }

    /// Resolve exactly as `main` does, so these tests exercise the real
    /// bare-form fallthrough rather than a restatement of it.
    fn dispatch(argv: &[&str]) -> Command {
        resolve(parse(argv)).unwrap_or_else(|e| panic!("{argv:?} did not resolve:\n{e}"))
    }

    #[test]
    fn clap_definition_is_well_formed() {
        Cli::command().debug_assert();
    }

    /// THE compatibility contract. Every `mesofact-dev` spawn site in the
    /// parent camp (kamaji, desktop, `serve_build.rs`, `local.sh`, …) uses
    /// this shape; the rename is only safe while it keeps parsing. A failure
    /// here means the second `[[bin]]` target is a lie.
    #[test]
    fn legacy_mesofact_dev_invocation_still_parses() {
        let Command::Dev(args) = dispatch(&[
            "mesofact-dev",
            "app/yah/web",
            "--port",
            "8080",
            "--no-watch",
            "--service",
            "dashboard",
            "--component",
            "web",
        ]) else {
            panic!("legacy invocation did not resolve to `dev`");
        };
        assert_eq!(args.workload, PathBuf::from("app/yah/web"));
        assert_eq!(args.port, 8080);
        assert!(args.no_watch);
        assert_eq!(args.service.as_deref(), Some("dashboard"));
        assert_eq!(args.component.as_deref(), Some("web"));
    }

    #[test]
    fn bare_mes_is_dev_in_the_current_directory() {
        let Command::Dev(args) = dispatch(&["mes"]) else {
            panic!("bare `mes` did not resolve to `dev`");
        };
        assert_eq!(args.workload, PathBuf::from("."));
        assert_eq!(args.port, DEFAULT_PORT);
    }

    #[test]
    fn bare_form_takes_dev_flags() {
        let Command::Dev(args) = dispatch(&["mes", "site", "--port", "3000"]) else {
            panic!("bare form with flags did not resolve to `dev`");
        };
        assert_eq!(args.workload, PathBuf::from("site"));
        assert_eq!(args.port, 3000);
    }

    #[test]
    fn explicit_dev_subcommand_binds_its_own_args() {
        let Command::Dev(args) = dispatch(&["mes", "dev", "site", "--port", "3000"]) else {
            panic!("`mes dev` did not resolve to `dev`");
        };
        assert_eq!(args.workload, PathBuf::from("site"));
        assert_eq!(args.port, 3000);
    }

    /// A leading flag pushes clap past the subcommand slot, so `dev` lands on
    /// the positional. Serving a directory named `dev` is not what anyone
    /// typing this meant — `resolve` has to catch it.
    #[test]
    fn subcommand_after_a_flag_is_rejected_not_silently_served() {
        let cli = parse(&["mes", "--port", "3000", "dev"]);
        assert!(cli.command.is_none(), "clap bound `dev` as a subcommand after all");
        assert_eq!(cli.dev.workload, PathBuf::from("dev"));

        let err = resolve(cli).expect_err("`mes --port 3000 dev` should not resolve").to_string();
        assert!(err.contains("`dev` is a subcommand"), "unhelpful message: {err}");
        assert!(err.contains("mes dev"), "message omits the fix: {err}");
    }

    /// The guard keys off the subcommand names, so a real directory that
    /// merely starts the same way must still go through.
    #[test]
    fn guard_does_not_swallow_ordinary_directories() {
        let Command::Dev(args) = dispatch(&["mes", "developer-site"]) else {
            panic!("`developer-site` was not treated as a workload directory");
        };
        assert_eq!(args.workload, PathBuf::from("developer-site"));

        // And the escape hatch the error message promises actually works.
        let Command::Dev(args) = dispatch(&["mes", "./dev"]) else {
            panic!("`./dev` was not treated as a workload directory");
        };
        assert_eq!(args.workload, PathBuf::from("./dev"));
    }

    #[test]
    #[cfg(feature = "build")]
    fn build_defaults_to_cwd_and_takes_out_dir() {
        let Command::Build(args) = dispatch(&["mes", "build"]) else {
            panic!("`mes build` did not resolve to `build`");
        };
        assert_eq!(args.workload, PathBuf::from("."));
        assert_eq!(args.out_dir, None);
    }

    /// The prod verbs are reachable from `mes` — that is the whole point of
    /// the superset, and it is the half a `cargo check` cannot confirm.
    #[test]
    fn prod_verbs_are_reachable() {
        assert!(matches!(dispatch(&["mes", "serve"]), Command::Serve(_)));
        assert!(matches!(dispatch(&["mes", "new", "hello"]), Command::New(_)));
        #[cfg(feature = "publish")]
        assert!(matches!(dispatch(&["mes", "publish"]), Command::Publish(_)));
    }
}
