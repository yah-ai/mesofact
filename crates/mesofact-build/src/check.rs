//! `mesofact check` — full TypeScript semantic pass (W174 §Fast/cheap/full
//! tiers, the "Full" row). Cadence-agnostic wrapper: QED / humans / CI decide
//! *when* it fires; mesofact only owns the seam (R451-F1). It runs the
//! project's `tsc` with `--noEmit` against the project's tsconfig and forwards
//! the diagnostics + exit code verbatim.
//!
//! W174 named "tsgo" as the target — the native (Go) TypeScript compiler. That
//! shipped: as of TypeScript 7 it GA'd *as the `typescript` package itself*
//! (`typescript@7`, `bin: tsc`, per-platform native binaries behind a thin
//! Node launcher), and the standalone `@typescript/native-preview`/`tsgo`
//! artifact collapsed into a nightly dev channel. So the native 10× checker is
//! now just `tsc` from a `typescript@7` dep — this wrapper resolves that and
//! nothing fancier. The heavy lifting is native; Node is still needed only to
//! bootstrap the launcher shim.
//!
//! Cross-file inference and generic instantiation are `tsc`'s job — this
//! module is a thin resolver + spawn, nothing more.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A resolved, ready-to-spawn `tsc` invocation. `program` + `leading_args`
/// front the command (`node <script>` for the package entry script, or the
/// `.bin`/PATH executable directly); [`check`] appends the pass args
/// (`--noEmit`, `--project`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub program: PathBuf,
    pub leading_args: Vec<String>,
}

pub struct CheckOptions {
    pub project_root: PathBuf,
    /// Explicit tsconfig; default is `<project_root>/tsconfig.json`.
    pub tsconfig: Option<PathBuf>,
    /// Extra args forwarded to `tsc` verbatim.
    pub extra_args: Vec<String>,
}

pub struct CheckOutcome {
    /// Process exit code (128 if `tsc` was killed by a signal).
    pub code: i32,
}

/// Run the full semantic pass. Streams `tsc`'s stdout/stderr to the caller's
/// terminal and returns its exit code.
pub fn check(opts: CheckOptions) -> Result<CheckOutcome> {
    let CheckOptions { project_root, tsconfig, extra_args } = opts;

    let tsconfig = tsconfig.unwrap_or_else(|| project_root.join("tsconfig.json"));
    if !tsconfig.exists() {
        bail!(
            "{}: no tsconfig at {} — mesofact check needs a TypeScript project config (pass --tsconfig to point elsewhere)",
            project_root.display(),
            tsconfig.display()
        );
    }

    let resolved = resolve(&project_root)?;
    eprintln!("mesofact check — tsc full semantic pass ({})", resolved.program.display());

    let mut cmd = Command::new(&resolved.program);
    cmd.current_dir(&project_root)
        .args(&resolved.leading_args)
        .arg("--noEmit")
        .arg("--project")
        .arg(&tsconfig)
        .args(&extra_args);

    let status = cmd
        .status()
        .with_context(|| format!("spawning tsc ({})", resolved.program.display()))?;
    Ok(CheckOutcome { code: status.code().unwrap_or(128) })
}

/// Resolve the project's `tsc`. Prefers the project-local install (the
/// `typescript` dep) over anything on PATH, matching how the `tsc` npm script
/// resolves.
pub fn resolve(project_root: &Path) -> Result<Resolved> {
    let nm = project_root.join("node_modules");

    // 1. Project-local .bin shim (its shebang runs it via env node).
    let bin = nm.join(".bin").join(exe("tsc"));
    if bin.is_file() {
        return Ok(Resolved { program: bin, leading_args: vec![] });
    }
    // 2. The typescript package's entry script, driven explicitly through
    //    Node — the mesofact installer doesn't mint .bin shims, so this is
    //    the path that lights up after `mesofact build`'s install step.
    let script = nm.join("typescript").join("bin").join("tsc");
    if script.is_file() {
        if let Some(node) = which_in_path(&exe("node")) {
            return Ok(Resolved {
                program: node,
                leading_args: vec![script.to_string_lossy().into_owned()],
            });
        }
    }
    // 3. A tsc on PATH.
    if let Some(program) = which_in_path(&exe("tsc")) {
        return Ok(Resolved { program, leading_args: vec![] });
    }

    bail!(
        "no tsc found for {} — mesofact check looked for the `typescript` dep in node_modules and a tsc on PATH; run the install step first, or add `typescript` (>=7 for the native 10x checker)",
        project_root.display()
    )
}

/// Platform executable name (`.exe` suffix on Windows).
fn exe(stem: &str) -> String {
    #[cfg(windows)]
    {
        format!("{stem}.exe")
    }
    #[cfg(not(windows))]
    {
        stem.to_string()
    }
}

/// First executable named `name` on `PATH`, if any.
fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let cand = dir.join(name);
        is_executable_file(&cand).then_some(cand)
    })
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_exec(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn resolves_local_bin_tsc_first() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("node_modules").join(".bin").join(exe("tsc"));
        touch_exec(&bin);
        let r = resolve(tmp.path()).unwrap();
        assert_eq!(r.program, bin);
        assert!(r.leading_args.is_empty());
    }

    #[test]
    fn resolves_package_script_via_node() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("node_modules").join("typescript").join("bin").join("tsc");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, b"// tsc entry\n").unwrap();
        // Only resolves via node if node is on PATH in this environment.
        match resolve(tmp.path()) {
            Ok(r) => assert_eq!(r.leading_args, vec![script.to_string_lossy().into_owned()]),
            Err(_) => { /* no node on PATH — acceptable */ }
        }
    }

    #[test]
    fn bin_shim_wins_over_package_script() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        let bin = nm.join(".bin").join(exe("tsc"));
        touch_exec(&bin);
        let script = nm.join("typescript").join("bin").join("tsc");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, b"// tsc entry\n").unwrap();
        let r = resolve(tmp.path()).unwrap();
        assert_eq!(r.program, bin, "the .bin shim should win over the raw package script");
    }
}
