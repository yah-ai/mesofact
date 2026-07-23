//! `mesofact` — the prod binary. Subcommand-driven (W174 §Binary surface).
//!
//! Replaces the old three-binary sprawl (`mesofact-serve`, `mesofact-proxy`,
//! `mesofact-publish`). The dev binary stays separate on purpose — see
//! [`mesofact::cli`] for why that split is load-bearing rather than stylistic.
//!
//! Subcommand availability tracks features: `serve` and `proxy` are always
//! present (both are V8-free at `default`; the SSR paths inside `serve` are
//! `ssr`-gated), `publish` needs the `publish` feature — which the `deploy`
//! preset pulls in via `ssr`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "mesofact",
    version,
    about = "mesofact — Rust-native web framework runtime",
    long_about = "Serve a built bundle or host SSR routes, run the Mode-2 proxy, \
                  or publish a dist/ tree. For the hot-reload dev server, use the \
                  separate `mesofact-dev` binary."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
