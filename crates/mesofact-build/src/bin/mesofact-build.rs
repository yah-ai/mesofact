//! `mesofact-build` — Rust-native build CLI (W174 binary surface, build
//! verb).

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use mesofact_build::pipeline::{build, BuildOptions, InstallMode};

#[derive(Parser)]
#[command(name = "mesofact-build", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the build pipeline for a workload dir (mesofact.routes.ts root).
    Build {
        /// Project root containing mesofact.routes.ts.
        project: PathBuf,
        /// Output directory (default: <project>/dist).
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Build id baked into manifest.json (default: UTC stamp).
        #[arg(long)]
        build_id: Option<String>,
        /// Run the lockfile-driven install step even if node_modules exists.
        #[arg(long, conflicts_with = "no_install")]
        install: bool,
        /// Never run the install step (build against existing node_modules).
        #[arg(long)]
        no_install: bool,
    },
    /// Install the locked dependency closure (bun.lock) into node_modules.
    Install { project: PathBuf },
    /// Render one route of an already-built dist with explicit params/data
    /// (no bundler, no install) — the revalidate / publish-once verb.
    Render {
        /// Project root containing mesofact.routes.ts (consulted only to
        /// re-read declared data_inputs when --data is not given).
        project: PathBuf,
        /// Declared route pattern, e.g. "/releases" or "/p/:id".
        #[arg(long)]
        route: String,
        /// Param value for each :param segment, repeatable: --param id=42.
        #[arg(long = "param", value_name = "KEY=VALUE", conflicts_with = "all")]
        params: Vec<String>,
        /// JSON file whose top-level object becomes req.data verbatim,
        /// overriding the route's declared data_inputs read.
        #[arg(long, conflicts_with = "all")]
        data: Option<PathBuf>,
        /// Built output directory (default: <project>/dist).
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Print the HTML to stdout instead of writing dist/html/<key>.html.
        #[arg(long, conflicts_with = "all")]
        stdout: bool,
        /// Re-expand the route's prerender params fresh and render every
        /// instance (the revalidate verb for a data/feed change). Rejects
        /// deferred routes — their instances are minted at publish time.
        #[arg(long)]
        all: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build {
            project,
            out_dir,
            build_id,
            install,
            no_install,
        } => {
            let install = if install {
                InstallMode::Always
            } else if no_install {
                InstallMode::Never
            } else {
                InstallMode::Auto
            };
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            let result = rt.block_on(build(BuildOptions {
                project_root: project,
                out_dir,
                build_id,
                install,
            }))?;
            println!(
                "mesofact build ok — build_id={}\n  manifest:  {}\n  tag-index: {}\n  html:      {} file(s)",
                result.build_id,
                result.manifest_path.display(),
                result.tag_index_path.display(),
                result.html_paths.len(),
            );
            Ok(())
        }
        Command::Install { project } => {
            let report = mesofact_build::install::install(&project)?;
            if report.skipped_fresh {
                println!("install fresh — lockfile unchanged, nothing to do");
            } else {
                println!(
                    "install ok — {} registry package(s), {} link(s)",
                    report.installed, report.linked
                );
            }
            Ok(())
        }
        Command::Render { project, route, params, data, out_dir, stdout } => {
            let params = parse_params(&params)?;
            let data = match data {
                Some(path) => Some(read_data_file(&path)?),
                None => None,
            };
            let outcome = mesofact_build::render::render_route(mesofact_build::render::RenderOptions {
                project_root: project,
                out_dir,
                route,
                params,
                data,
                write: !stdout,
            })?;
            if stdout {
                println!("{}", outcome.html);
            } else {
                let path = outcome.html_path.expect("write mode emits a path");
                println!(
                    "mesofact render ok — {} → {}\n  key:  {}\n  tags: {}",
                    outcome.url,
                    path.display(),
                    outcome.key,
                    if outcome.tags.is_empty() { "(none)".to_string() } else { outcome.tags.join(", ") },
                );
            }
            Ok(())
        }
    }
}

fn parse_params(raw: &[String]) -> Result<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    for p in raw {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--param '{p}' is not KEY=VALUE"))?;
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

fn read_data_file(path: &std::path::Path) -> Result<serde_json::Map<String, serde_json::Value>> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("reading --data {}: {e}", path.display()))?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("parsing --data {}: {e}", path.display()))?;
    match parsed {
        serde_json::Value::Object(map) => Ok(map),
        other => anyhow::bail!(
            "--data {} must be a JSON object at the top level (got {})",
            path.display(),
            match other {
                serde_json::Value::Null => "null",
                serde_json::Value::Bool(_) => "boolean",
                serde_json::Value::Number(_) => "number",
                serde_json::Value::String(_) => "string",
                serde_json::Value::Array(_) => "array",
                serde_json::Value::Object(_) => unreachable!(),
            }
        ),
    }
}
