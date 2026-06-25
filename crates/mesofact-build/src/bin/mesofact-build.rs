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
    }
}
