//! `mesofact-build` — Rust-native build CLI (W174 binary surface, build
//! verb). `--legacy-bun` shells to the Bun pipeline; `diff` compares two
//! dist trees for the R450 equivalence gate.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use mesofact_build::legacy::{find_bun_cli, run_legacy_bun};
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
        /// Run the legacy Bun pipeline instead (requires bun on PATH).
        #[arg(long)]
        legacy_bun: bool,
        /// Explicit path to packages/mesofact-build/src/cli.ts for --legacy-bun.
        #[arg(long, requires = "legacy_bun")]
        legacy_bun_cli: Option<PathBuf>,
    },
    /// Install the locked dependency closure (bun.lock) into node_modules.
    Install { project: PathBuf },
    /// Diff two dist/ trees for behavioral equivalence (R450).
    Diff { legacy_dist: PathBuf, native_dist: PathBuf },
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
            legacy_bun,
            legacy_bun_cli,
        } => {
            if legacy_bun {
                let cli_path = find_bun_cli(&project, legacy_bun_cli.as_deref())?;
                run_legacy_bun(&project, &cli_path)?;
                return Ok(());
            }
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
        Command::Diff { legacy_dist, native_dist } => {
            let report = mesofact_build::diff::diff_dists(&legacy_dist, &native_dist)?;
            if report.is_equivalent() {
                println!("dist trees are behaviorally equivalent (modulo build-id + bundle hashes)");
                Ok(())
            } else {
                for f in &report.findings {
                    eprintln!("DIFF: {f}");
                }
                std::process::exit(1);
            }
        }
    }
}
