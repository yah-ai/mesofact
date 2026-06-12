//! `--legacy-bun` escape hatch (R450-F1): run today's Bun pipeline
//! (`packages/mesofact-build/src/cli.ts`) for the same project. Kept
//! alongside the Rust-native default so the two can be diffed (R450-F2)
//! and so source-adapter prerender routes keep building until the native
//! pipeline grows adapter support.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate the Bun CLI entrypoint: explicit override → MESOFACT_BUILD_CLI
/// env → walk up from the project looking for
/// `oss/mesofact/packages/mesofact-build/src/cli.ts` or a checked-out
/// `packages/mesofact-build/src/cli.ts`.
pub fn find_bun_cli(project_root: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        bail!("--legacy-bun-cli {} does not exist", p.display());
    }
    if let Some(env) = std::env::var_os("MESOFACT_BUILD_CLI") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Ok(p);
        }
        bail!("MESOFACT_BUILD_CLI={} does not exist", p.display());
    }
    let mut cur = Some(project_root);
    while let Some(dir) = cur {
        for candidate in [
            dir.join("packages/mesofact-build/src/cli.ts"),
            dir.join("oss/mesofact/packages/mesofact-build/src/cli.ts"),
            dir.join("external/mesofact/packages/mesofact-build/src/cli.ts"),
        ] {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        cur = dir.parent();
    }
    bail!(
        "could not locate the Bun mesofact-build cli.ts from {}; pass --legacy-bun-cli or set MESOFACT_BUILD_CLI",
        project_root.display()
    )
}

pub fn run_legacy_bun(project_root: &Path, cli: &Path) -> Result<()> {
    let status = Command::new("bun")
        .arg("run")
        .arg(cli)
        .arg(project_root)
        .status()
        .context("spawning bun (is bun on PATH? --legacy-bun requires it)")?;
    if !status.success() {
        bail!("legacy bun build failed with {status}");
    }
    Ok(())
}
