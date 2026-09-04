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
//! now just `tsc` from a `typescript@7` dep.
//!
//! ## Why this does not need Node
//!
//! `typescript@7`'s `bin/tsc` is three lines of JS whose only job is to
//! `execve` a per-platform native executable, shipped as an optional dep and
//! unpacked at `node_modules/@typescript/typescript-<platform>-<arch>/lib/tsc`
//! (verified 2026-09-01 against 7.0.2: a plain Mach-O/ELF binary that answers
//! `--version` on its own). [`resolve`] takes that binary **directly** — so
//! `mesofact check` on a `typescript@7` project runs the Go checker with no
//! Node, no bun, and nothing on `PATH`, which is the same promise the rest of
//! the build path makes (W174 §Framing: "no Node on PATH for build or
//! runtime").
//!
//! The Node ladder below it is the compatibility tail, not the design: a
//! `typescript@5`/`@6` project has no native binary to find, and there the
//! launcher *is* the compiler, so a `node` is genuinely required. That case
//! reports itself as such rather than dying inside a shebang.
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
///
/// `program` is always absolute. That is a contract, not an accident: the
/// spawn sets `current_dir` to the project root, so a program path relative
/// to the *caller's* cwd would be re-resolved against the project dir and
/// fail (see [`resolve`]).
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

    // Everything below this line is absolute. The spawn re-roots the child at
    // `project_root`, so any path still relative to the caller's cwd — the
    // program, the tsconfig — would be re-resolved against the project dir by
    // the child instead. `mesofact-build check app/yah/web/marketing` used to
    // die with a bare "No such file or directory" naming a tsc that plainly
    // existed, because `app/yah/web/marketing/node_modules/.bin/tsc` was
    // looked up from inside `app/yah/web/marketing`.
    let project_root = project_root
        .canonicalize()
        .with_context(|| format!("resolving project root {}", project_root.display()))?;

    let tsconfig = tsconfig.unwrap_or_else(|| project_root.join("tsconfig.json"));
    if !tsconfig.exists() {
        bail!(
            "{}: no tsconfig at {} — mesofact check needs a TypeScript project config (pass --tsconfig to point elsewhere)",
            project_root.display(),
            tsconfig.display()
        );
    }
    // A `--tsconfig` the caller typed is relative to THEIR cwd, which is where
    // the `exists` above just checked it. Pin that reading before the child
    // gets a different cwd and a different answer.
    let tsconfig = tsconfig
        .canonicalize()
        .with_context(|| format!("resolving tsconfig {}", tsconfig.display()))?;

    let resolved = resolve(&project_root)?;
    eprintln!("mesofact check — tsc full semantic pass ({})", resolved.program.display());

    let mut cmd = Command::new(&resolved.program);
    cmd.current_dir(&project_root)
        .args(&resolved.leading_args)
        .arg("--noEmit")
        .arg("--project")
        .arg(&tsconfig)
        .args(&extra_args);

    let status = match cmd.status() {
        Ok(status) => status,
        Err(err) => return Err(spawn_error(err, &resolved.program)),
    };
    Ok(CheckOutcome { code: status.code().unwrap_or(128) })
}

/// Explain a failed spawn. The trap worth naming is a `NotFound` reported
/// against a program that plainly exists: on Unix that ENOENT came from the
/// kernel failing to resolve the script's `#!` interpreter, not the script,
/// and the raw message ("No such file or directory") points at the wrong file.
fn spawn_error(err: std::io::Error, program: &Path) -> anyhow::Error {
    if err.kind() == std::io::ErrorKind::NotFound && program.is_file() {
        if let Some(interp) = shebang_interpreter(program) {
            return anyhow::anyhow!(
                "cannot run {}: its `#!{interp}` interpreter is not available — that is what the \
                 \"not found\" is about, not the checker itself. A `typescript` >=7 dep carries a \
                 native checker that needs no interpreter at all.",
                program.display()
            );
        }
    }
    anyhow::Error::new(err).context(format!("spawning tsc ({})", program.display()))
}

/// The interpreter named by `path`'s `#!` line, if it has one. Bounded read —
/// this is also reachable for a native binary, and the first line of one is
/// not something to pull into memory whole.
fn shebang_interpreter(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut head = [0u8; 256];
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    let line = head[..n].split(|b| *b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
    Some(line.strip_prefix("#!")?.trim().to_string())
}

/// Resolve the project's `tsc`. Prefers the project-local install (the
/// `typescript` dep) over anything on PATH, matching how the `tsc` npm script
/// resolves — and within that, prefers the native binary over the Node
/// launcher that would only have `execve`'d it anyway.
///
/// `project_root` may be relative; it is canonicalized here so every path in
/// the returned [`Resolved`] is absolute regardless of who calls this.
pub fn resolve(project_root: &Path) -> Result<Resolved> {
    let project_root = project_root
        .canonicalize()
        .with_context(|| format!("resolving project root {}", project_root.display()))?;
    let nm = project_root.join("node_modules");

    // 0. The native (Go) checker itself — `typescript@7`'s per-platform
    //    optional dep. This is what `bin/tsc` would have spawned; taking it
    //    directly is both faster (no Node process in front) and the reason
    //    this verb keeps W174's no-Node-on-PATH promise.
    if let Some(dir) = native_package_dir() {
        let native = nm.join("@typescript").join(dir).join("lib").join(exe("tsc"));
        if is_executable_file(&native) {
            return Ok(Resolved { program: native, leading_args: vec![] });
        }
    }
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
        bail!(
            "{}: found the `typescript` dep but no `node` on PATH to run it — TypeScript 7 ships a native checker at node_modules/@typescript/typescript-<platform>-<arch>/lib/tsc that needs no Node, so upgrading the dep to `typescript` >=7 removes this requirement outright",
            project_root.display()
        );
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

/// The `@typescript/typescript-<platform>-<arch>` directory name for the host,
/// or `None` on a target TypeScript publishes no native binary for.
///
/// The halves are npm's, i.e. Node's `process.platform` / `process.arch`, not
/// Rust's — `typescript@7.0.2`'s `optionalDependencies` is the authoritative
/// list and every name below appears in it.
fn native_package_dir() -> Option<String> {
    node_platform_arch(std::env::consts::OS, std::env::consts::ARCH)
        .map(|(platform, arch)| format!("typescript-{platform}-{arch}"))
}

/// Rust's `OS`/`ARCH` in Node's spelling.
///
/// The table itself moved to [`rnpm::node_platform_arch`] in R773-F6 — an
/// `os`/`cpu` field is npm manifest grammar, and the resolver reads it too, so
/// the one copy belongs in the crate that models npm manifests. This alias
/// stays because [`native_package_dir`] and [`crate::install`]'s gate both call
/// it by this name, and because the tests below pin the mapping off a host
/// triple rather than off whatever machine runs them.
///
/// Deliberately absent from that table: `mips64`. npm publishes only the
/// little-endian `mips64el` build and Rust spells both `mips64`, so there is no
/// honest mapping — such a host falls through to the Node ladder, which still
/// works.
pub(crate) fn node_platform_arch(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
    rnpm::node_platform_arch(os, arch)
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

/// First executable named `name` on `PATH`, if any. Always absolute — a
/// relative `PATH` entry (`.`, `node_modules/.bin`, both real in the wild)
/// would otherwise be re-resolved against the child's cwd, not ours.
fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let cand = dir.join(name);
        if !is_executable_file(&cand) {
            return None;
        }
        Some(cand.canonicalize().unwrap_or(cand))
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

    /// `tempfile::tempdir()` hands back the pre-symlink path (`/var/…` on
    /// macOS, where `/var` is a link to `/private/var`), and `resolve`
    /// canonicalizes. Compare against the same reading it will produce.
    fn canonical(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().canonicalize().unwrap()
    }

    /// Where `typescript@7` unpacks the native checker for THIS host, relative
    /// to `node_modules`.
    fn native_rel() -> PathBuf {
        Path::new("@typescript")
            .join(native_package_dir().expect("this host should have a native package name"))
            .join("lib")
            .join(exe("tsc"))
    }

    #[test]
    fn resolves_local_bin_tsc_first() {
        let tmp = tempfile::tempdir().unwrap();
        touch_exec(&tmp.path().join("node_modules").join(".bin").join(exe("tsc")));
        let r = resolve(tmp.path()).unwrap();
        assert_eq!(r.program, canonical(&tmp).join("node_modules").join(".bin").join(exe("tsc")));
        assert!(r.leading_args.is_empty());
    }

    /// The point of the whole verb: with a `typescript@7` dep on disk, the
    /// checker we spawn is the native binary itself, not the Node launcher
    /// that would only have `execve`'d it.
    #[test]
    fn the_native_binary_outranks_the_node_launcher() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        touch_exec(&nm.join(native_rel()));
        // Both Node-shaped rungs present and losing.
        touch_exec(&nm.join(".bin").join(exe("tsc")));
        let script = nm.join("typescript").join("bin").join("tsc");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, b"// tsc entry\n").unwrap();

        let r = resolve(tmp.path()).unwrap();
        assert_eq!(r.program, canonical(&tmp).join("node_modules").join(native_rel()));
        assert!(r.leading_args.is_empty(), "the native binary takes no launcher args");
    }

    /// A `typescript@5`/`@6` project has no native binary — that must fall
    /// through rather than resolve to a path that isn't there.
    ///
    /// Unix-only: "not executable" is a mode bit, and on Windows
    /// [`is_executable_file`] is just `is_file`, so the premise doesn't exist.
    #[cfg(unix)]
    #[test]
    fn a_non_executable_native_path_does_not_win() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        let native = nm.join(native_rel());
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&native, b"not executable\n").unwrap(); // mode 644
        touch_exec(&nm.join(".bin").join(exe("tsc")));

        let r = resolve(tmp.path()).unwrap();
        assert_eq!(r.program, canonical(&tmp).join("node_modules").join(".bin").join(exe("tsc")));
    }

    /// npm's spelling, not Rust's — the names have to match
    /// `typescript@7`'s `optionalDependencies` exactly or nothing is found.
    #[test]
    fn platform_arch_uses_nodes_spelling() {
        assert_eq!(node_platform_arch("macos", "aarch64"), Some(("darwin", "arm64")));
        assert_eq!(node_platform_arch("linux", "x86_64"), Some(("linux", "x64")));
        assert_eq!(node_platform_arch("windows", "aarch64"), Some(("win32", "arm64")));
        assert_eq!(node_platform_arch("illumos", "x86_64"), Some(("sunos", "x64")));
        assert_eq!(node_platform_arch("linux", "loongarch64"), Some(("linux", "loong64")));
        // Unpublished halves opt out rather than guessing a name.
        assert_eq!(node_platform_arch("android", "aarch64"), None);
        assert_eq!(node_platform_arch("linux", "mips64"), None);
    }

    /// The four triples this camp actually builds and ships resolve to
    /// packages `typescript@7.0.2` really publishes.
    #[test]
    fn the_blessed_triples_all_map() {
        for (os, arch, expect) in [
            ("macos", "aarch64", "typescript-darwin-arm64"),
            ("macos", "x86_64", "typescript-darwin-x64"),
            ("linux", "aarch64", "typescript-linux-arm64"),
            ("linux", "x86_64", "typescript-linux-x64"),
        ] {
            let (p, a) = node_platform_arch(os, arch).unwrap();
            assert_eq!(format!("typescript-{p}-{a}"), expect);
        }
    }

    #[test]
    fn a_shebang_is_read_back_from_a_script() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("shim");
        std::fs::write(&script, b"#!/usr/bin/env node\nimport 'x';\n").unwrap();
        assert_eq!(shebang_interpreter(&script).as_deref(), Some("/usr/bin/env node"));

        let plain = tmp.path().join("plain");
        std::fs::write(&plain, b"\x7fELF not a script").unwrap();
        assert_eq!(shebang_interpreter(&plain), None);
    }

    #[test]
    fn resolves_package_script_via_node() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("node_modules").join("typescript").join("bin").join("tsc");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, b"// tsc entry\n").unwrap();
        let expected = canonical(&tmp).join("node_modules").join("typescript").join("bin").join("tsc");
        // Only resolves via node if node is on PATH in this environment.
        match resolve(tmp.path()) {
            Ok(r) => {
                assert_eq!(r.leading_args, vec![expected.to_string_lossy().into_owned()]);
                assert!(Path::new(&r.leading_args[0]).is_absolute());
            }
            Err(_) => { /* no node on PATH — acceptable */ }
        }
    }

    /// The bug this module shipped with: every path handed to the spawn has
    /// to be absolute, because the spawn re-roots the child at the project
    /// dir. A relative `project_root` is the shape a shell hands us.
    #[test]
    fn a_relative_project_root_still_resolves_to_an_absolute_program() {
        let tmp = tempfile::tempdir().unwrap();
        let root = canonical(&tmp);
        touch_exec(&root.join("node_modules").join(".bin").join(exe("tsc")));

        // Absolute-but-not-canonical stands in for the general case without
        // needing a process-global chdir (tests run in parallel threads).
        let indirect = root.join("node_modules").join("..");
        let r = resolve(&indirect).unwrap();
        assert!(r.program.is_absolute(), "program must be absolute: {}", r.program.display());
        assert_eq!(r.program, root.join("node_modules").join(".bin").join(exe("tsc")));
    }

    #[test]
    fn a_missing_project_root_names_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let err = resolve(&tmp.path().join("nope")).unwrap_err().to_string();
        assert!(err.contains("resolving project root"), "unexpected error: {err}");
        assert!(err.contains("nope"), "error should name the path: {err}");
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
        assert_eq!(
            r.program,
            canonical(&tmp).join("node_modules").join(".bin").join(exe("tsc")),
            "the .bin shim should win over the raw package script"
        );
    }
}
