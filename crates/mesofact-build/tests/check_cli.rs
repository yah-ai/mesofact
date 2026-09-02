//! `mesofact-build check` end-to-end (W174 §Full tier, R451).
//!
//! The wrapper's whole job is *wiring*: find the project's checker, spawn it
//! with the right cwd and the right args, hand its exit code back. None of
//! that needs real TypeScript to test — a shell script standing in for `tsc`
//! records the argv it was called with and exits with a code we choose, which
//! pins every seam the unit tests in `check.rs` cannot reach (the spawn's cwd,
//! the forwarded args, the exit code).
//!
//! Unix-only: the stand-in is a `#!/bin/sh` script.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A tempdir holding `proj/`, whose `node_modules/.bin/tsc` is a script that
/// writes its cwd + argv to `proj/argv.txt` and exits `code`. The project sits
/// one level down so tests have a scratch parent directory to be *invoked
/// from*, rather than reaching into the shared system temp dir.
struct Fixture {
    _tmp: tempfile::TempDir,
    /// Canonical path to the scratch parent — a safe cwd for the CLI.
    parent: PathBuf,
    /// Canonical path to the project root.
    root: PathBuf,
}

impl Fixture {
    fn new(code: i32) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().canonicalize().unwrap();
        let root = parent.join("proj");

        let bin = root.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(root.join("tsconfig.json"), b"{}\n").unwrap();

        let tsc = bin.join("tsc");
        // Capturing `pwd` is the point: it proves the child ran rooted at the
        // project, which is what makes a caller-relative program path a bug.
        std::fs::write(
            &tsc,
            format!(
                "#!/bin/sh\n\
                 {{ pwd; for a in \"$@\"; do echo \"$a\"; done; }} > \"$(dirname \"$0\")/../../argv.txt\"\n\
                 exit {code}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&tsc, std::fs::Permissions::from_mode(0o755)).unwrap();

        Self { _tmp: tmp, parent, root }
    }

    /// Lines the stand-in recorded: `[cwd, arg0, arg1, …]`.
    fn recorded(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("argv.txt"))
            .expect("the stand-in tsc never ran")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn ran(&self) -> bool {
        self.root.join("argv.txt").exists()
    }

    /// `mesofact-build check …`, invoked from the scratch parent.
    fn check(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_mesofact-build"))
            .current_dir(&self.parent)
            .arg("check")
            .args(args)
            .output()
            .unwrap()
    }
}

fn ok(out: &std::process::Output) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The regression: a project path relative to the caller's cwd. The spawn
/// re-roots the child at the project dir, so an un-absolutized program path
/// used to be looked up *inside* the project and fail with a bare
/// "No such file or directory" naming a tsc that plainly existed.
#[test]
fn a_relative_project_arg_runs_the_projects_tsc() {
    let f = Fixture::new(0);
    let out = f.check(&["proj"]);
    ok(&out);
    assert_eq!(
        Path::new(&f.recorded()[0]).canonicalize().unwrap(),
        f.root,
        "child cwd should be the project root"
    );
}

/// `--noEmit` and an absolute `--project` are the pass args, and anything
/// after `--` rides along verbatim.
#[test]
fn forwards_no_emit_the_tsconfig_and_extra_args() {
    let f = Fixture::new(0);
    let out = f.check(&["proj", "--", "--pretty", "false"]);
    ok(&out);

    let recorded = f.recorded();
    assert_eq!(
        &recorded[1..],
        [
            "--noEmit".to_string(),
            "--project".to_string(),
            f.root.join("tsconfig.json").to_string_lossy().into_owned(),
            "--pretty".to_string(),
            "false".to_string(),
        ]
    );
}

/// A failing check is the whole reason CI calls this — the checker's exit
/// code has to survive the wrapper rather than collapsing to 1.
#[test]
fn propagates_the_checkers_exit_code() {
    let f = Fixture::new(2);
    assert_eq!(f.check(&["proj"]).status.code(), Some(2));
}

/// No tsconfig is a wrapper-level error, diagnosed before anything is spawned.
#[test]
fn a_project_without_a_tsconfig_says_so_before_spawning() {
    let f = Fixture::new(0);
    std::fs::remove_file(f.root.join("tsconfig.json")).unwrap();

    let out = f.check(&["proj"]);
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no tsconfig at"), "unexpected stderr: {stderr}");
    assert!(!f.ran(), "tsc should not have been spawned");
}

/// A `--tsconfig` the caller typed relative to their own cwd must keep that
/// reading. The child's cwd is different, so an un-pinned relative path
/// resolves against the project instead — a different file, or none.
#[test]
fn a_relative_tsconfig_flag_is_read_from_the_callers_cwd() {
    let f = Fixture::new(0);
    // Same relative spelling, two meanings: `<parent>/strict.json` is what the
    // caller typed; `<root>/strict.json` is what the child would have found.
    // Only the caller's exists.
    std::fs::write(f.parent.join("strict.json"), b"{}\n").unwrap();

    let out = f.check(&["proj", "--tsconfig", "strict.json"]);
    ok(&out);
    assert_eq!(
        f.recorded()[3],
        f.parent.join("strict.json").to_string_lossy(),
        "the caller's reading of --tsconfig should survive the cwd change"
    );
}
