//! `mesofact` — the prod binary. Subcommand-driven (W174 §Binary surface).
//!
//! Replaces the old three-binary sprawl (`mesofact-serve`, `mesofact-proxy`,
//! `mesofact-publish`). The dev binary stays separate on purpose — see
//! [`mesofact::cli`] for why that split is load-bearing rather than stylistic.
//!
//! Subcommand availability tracks features: `new`, `serve` and `proxy` are
//! always present (all V8-free at `default`; the SSR paths inside `serve` are
//! `ssr`-gated), `publish` needs the `publish` feature — which the `deploy`
//! preset pulls in via `ssr`.
//!
//! `new` is here rather than in `mesofact-dev` on purpose: it writes files and
//! carries no dev affordance, and the binary that scaffolds a project is then
//! the binary whose version that project's JS set is pinned to (R759-T4).

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "mesofact",
    version,
    about = "mesofact — Rust-native web framework runtime",
    long_about = "Scaffold a new project, serve a built bundle or host SSR routes, \
                  run the Mode-2 proxy, or publish a dist/ tree.\n\n\
                  This is the production binary: no file watcher, no bundler, no dev \
                  storage surface. For the hot-reload dev loop — and for a single CLI \
                  that also carries these verbs — use `mes`, which is a superset of \
                  this one in the dev direction."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a new mesofact project pinned to this binary's version.
    New(mesofact::cli::new::NewArgs),
    /// Serve a W272 bundle (static v0) or host SSR routes.
    Serve(mesofact::cli::serve::ServeArgs),
    /// Run the Mode-2 proxy: worker pool + manifest reload on SIGHUP/heartbeat.
    Proxy(Box<mesofact::core::proxy::config::Config>),
    /// Upload a built dist/ tree, swap the manifest pointer, purge CDN tags.
    #[cfg(feature = "publish")]
    Publish(mesofact::cli::publish::PublishArgs),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        // Synchronous: `new` only writes files. Nothing here awaits, and a
        // scaffold that cannot start without a runtime would be a strange
        // thing to have to explain.
        Command::New(args) => to_exit_code(mesofact::cli::new::run(args)),
        Command::Serve(args) => to_exit_code(mesofact::cli::serve::run(args).await),
        Command::Proxy(cfg) => to_exit_code(mesofact::cli::proxy::run(*cfg).await),
        // `publish` owns its exit codes (2 = missing config, etc.) — pass through.
        #[cfg(feature = "publish")]
        Command::Publish(args) => mesofact::cli::publish::run(args).await,
    }
}

fn to_exit_code(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mesofact: {err:#}");
            ExitCode::FAILURE
        }
    }
}
