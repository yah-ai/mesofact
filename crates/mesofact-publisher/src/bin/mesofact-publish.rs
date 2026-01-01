// `mesofact publish` — drives `publish_dist` (or `publish_pin` for rollback)
// against an [`ObjectStore`] + [`CdnPurger`] pair.
//
// Two execution paths:
//
// - `--in-memory` smoke: drops everything in-process (no creds needed). Used
//   by `cargo test`, local development, and `mesofact-publish --in-memory`
//   from the workspace root.
// - Real-network: loads the `[publish]` block from `mesofact.config.toml`,
//   resolves the S3 + Cloudflare credentials from env-named vars, applies any
//   `--bucket/--endpoint/--zone` overrides, and runs against
//   [`S3Store`] + [`CloudflareCdnPurger`]. CI smoke wires the env vars from
//   secrets; local runs without creds fall back to a precise error.

use clap::Parser;
use mesofact_publisher::{
    publish_dist, publish_pin, CloudflareCdnPurger, ConfigError, InMemoryPurger, InMemoryStore,
    PublishConfig, PublishReport, S3Store,
};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(
    name = "mesofact-publish",
    about = "Upload a built dist/ tree, atomically swap the manifest pointer, purge CDN tags."
)]
struct Args {
    /// Path to the build output directory (must contain manifest.json + tag-index.json).
    #[arg(default_value = "dist")]
    dist: PathBuf,

    /// Rollback: repoint /manifest.json at this prior build_id (must still be retained).
    #[arg(long)]
    pin: Option<String>,

    /// Use the in-memory store/purger. No credentials; results are dropped on exit.
    #[arg(long)]
    in_memory: bool,

    /// Path to mesofact.config.toml (default: ./mesofact.config.toml).
    #[arg(long, default_value = "mesofact.config.toml")]
    config: PathBuf,

    /// Override `[publish].bucket` from the config file.
    #[arg(long)]
    bucket: Option<String>,

    /// Override `[publish].endpoint` from the config file (S3 endpoint root).
    #[arg(long)]
    endpoint: Option<String>,

    /// Override `[publish].zone_id` from the config file (Cloudflare zone).
    #[arg(long)]
    zone: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();

    if args.in_memory {
        return run_in_memory(args).await;
    }
    run_real(args).await
}

async fn run_in_memory(args: Args) -> ExitCode {
    let store = InMemoryStore::new();
    let purger = InMemoryPurger::new();
    let result = dispatch(&args, &store, &purger).await;
    report(result)
}

async fn run_real(args: Args) -> ExitCode {
    let cfg = match PublishConfig::load(&args.config).await {
        Ok(cfg) => cfg.with_overrides(args.bucket.clone(), args.endpoint.clone(), args.zone.clone()),
        Err(ConfigError::NotFound(path)) => {
            eprintln!(
                "mesofact-publish: {path} not found. Pass --in-memory for a smoke run, \
                 or add a [publish] block to mesofact.config.toml."
            );
            return ExitCode::from(2);
        }
        Err(ConfigError::MissingPublish(path)) => {
            eprintln!(
                "mesofact-publish: no [publish] section in {path}. Declare bucket / endpoint / \
                 zone_id (and env-var names for credentials) to enable real-network publish."
            );
            return ExitCode::from(2);
        }
        Err(err) => {
            eprintln!("mesofact-publish: config error: {err}");
            return ExitCode::from(2);
        }
    };
    let creds = match cfg.resolve_credentials() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("mesofact-publish: {err}");
            return ExitCode::from(2);
        }
    };
    let store = match S3Store::new(
        &cfg.endpoint,
        &cfg.bucket,
        &cfg.region,
        &creds.access_key_id,
        &creds.secret_access_key,
    ) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("mesofact-publish: S3 store init failed: {err}");
            return ExitCode::from(1);
        }
    };
    let purger = match CloudflareCdnPurger::new(&cfg.zone_id, &creds.cloudflare_api_token) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("mesofact-publish: Cloudflare purger init failed: {err}");
            return ExitCode::from(1);
        }
    };
    let result = dispatch(&args, &store, &purger).await;
    report(result)
}

async fn dispatch(
    args: &Args,
    store: &dyn mesofact_publisher::ObjectStore,
    purger: &dyn mesofact_publisher::CdnPurger,
) -> Result<PublishReport, mesofact_publisher::PublishError> {
    if let Some(build_id) = args.pin.as_deref() {
        publish_pin(build_id, store, purger).await
    } else {
        publish_dist(&args.dist, store, purger).await
    }
}

fn report(result: Result<PublishReport, mesofact_publisher::PublishError>) -> ExitCode {
    match result {
        Ok(report) => {
            println!("publish ok — build_id={}", report.build_id);
            println!("  uploaded: {} key(s)", report.uploaded_keys.len());
            println!("  skipped:  {} key(s)", report.skipped_keys.len());
            println!("  purged:   {} tag(s)", report.purged_tags.len());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("mesofact-publish failed: {err}");
            ExitCode::from(1)
        }
    }
}
