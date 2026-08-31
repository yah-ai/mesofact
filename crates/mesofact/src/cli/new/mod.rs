//! `mesofact new` — scaffold a project that builds and serves with the two
//! shipped binaries and nothing else.
//!
//! W225 §2 names this as a deliverable ("A `mesofact new` scaffold lays down
//! the shape … so consumers get this right without thinking about it");
//! R759-T4 is it. The relay it belongs to makes `.mesofact-version` the one
//! dial that selects a mesofact, so what this command emits has to be
//! coherent under that dial: the pinned JS set ([`curated`]) and the vended
//! runtime barrel both come from *this binary's* version, and the scaffolded
//! project carries a `.mesofact-version` naming it.
//!
//! **Why this lives in the prod binary.** `new` writes files and reads
//! nothing; it carries no dev affordance and no runtime capability, so it
//! does not touch the W225 §2 dev/prod boundary — that boundary is about
//! `mesofact-dev`'s watcher, live-reload injection and dev S3 surface never
//! entering the prod dependency closure. Putting `new` here is also what
//! makes the pin trustworthy: the binary that scaffolds is the binary that
//! serves.
//!
//! **Which tier this scaffolds.** The *standalone* tier — a project the
//! shipped `mesofact` / `mesofact-dev` pair runs directly, with no consumer
//! Rust. W225 §2's Consumer DX also describes a *library* tier (one crate,
//! `lib.rs` plus two thin bin targets). That is deliberately not emitted
//! here, and not because it was skipped: it is not scaffoldable today. See
//! the module note in [`self`] below.
//!
//! ## The library tier is blocked, not skipped
//!
//! Two independent blockers, both verified rather than assumed:
//!
//! 1. **`mesofact-dev` is not publishable.** Its manifest carries
//!    `publish = false`, and crates.io has no `mesofact-dev` (checked
//!    2026-08-28; `mesofact` itself is there at 0.8.26). A scaffolded
//!    `Cargo.toml` naming it as a plain dependency — which is exactly what
//!    W225 §2 prescribes — cannot resolve for anyone outside this monorepo.
//! 2. **The dev tier has no library entry point.** W225 §2 sketches
//!    `mesofact_dev::serve(app::router())`, but no such function exists. The
//!    dev affordances are composed in `mesofact-dev`'s own `main.rs` around a
//!    *workload* (a built `dist/` tree plus a watcher), not around a caller's
//!    `axum::Router`. Emitting the sketch verbatim would produce a project
//!    that does not compile.
//!
//! Both are real work with a design question in front of them (what *is* the
//! dev half of a library-tier consumer?), which is why they are a followup
//! rather than a silent omission here.

pub mod curated;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;

use curated::{MESOFACT_VERSION, RUNTIME_BARREL_DIR};

/// One template file: where it lands, and its bytes as compiled in.
///
/// `include_str!` rather than a string literal so the templates stay real
/// `.ts` / `.tsx` / `.toml` files on disk — editable, diffable, and readable
/// by anyone auditing what a scaffold emits without reading Rust.
struct Template {
    dest: &'static str,
    body: &'static str,
}

const TEMPLATES: &[Template] = &[
    Template { dest: "README.md", body: include_str!("template/README.md") },
    Template { dest: ".gitignore", body: include_str!("template/gitignore") },
    Template { dest: "tsconfig.json", body: include_str!("template/tsconfig.json") },
    Template { dest: "workload.toml", body: include_str!("template/workload.toml") },
    Template { dest: "mesofact.routes.ts", body: include_str!("template/mesofact.routes.ts") },
    Template { dest: "src/Page.tsx", body: include_str!("template/src/Page.tsx") },
    Template { dest: "src/home.tsx", body: include_str!("template/src/home.tsx") },
    Template { dest: "src/not_found.tsx", body: include_str!("template/src/not_found.tsx") },
    Template { dest: "src/api.ts", body: include_str!("template/src/api.ts") },
];

const RUNTIME_BARREL_TYPES: &str = include_str!("template/vendor-runtime.d.ts");

#[derive(Args, Debug)]
pub struct NewArgs {
    /// Directory to create. Its file name becomes the project name unless
    /// `--name` overrides it.
    pub path: PathBuf,

    /// Project name written into package.json, when it should differ from the
    /// directory name.
    #[arg(long)]
    pub name: Option<String>,

    /// Scaffold into a directory that already exists. Files that are already
    /// there are still never overwritten — this only permits a non-empty
    /// target.
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: NewArgs) -> Result<()> {
    let name = match &args.name {
        Some(n) => n.clone(),
        None => args
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot derive a project name from {} — pass --name",
                    args.path.display()
                )
            })?,
    };
    validate_name(&name)?;

    let root = &args.path;
    if root.exists() {
        if !root.is_dir() {
            bail!("{} exists and is not a directory", root.display());
        }
        let empty = root.read_dir()?.next().is_none();
        if !empty && !args.force {
            bail!(
                "{} is not empty — pass --force to scaffold into it anyway (existing files are never overwritten)",
                root.display()
            );
        }
    }
    std::fs::create_dir_all(root)
        .with_context(|| format!("creating {}", root.display()))?;

    let mut written = Vec::new();
    for t in TEMPLATES {
        write_new(root, t.dest, &expand(t.body, &name), &mut written)?;
    }

    // Generated, not templated: both derive from `curated::CURATED`, which is
    // the only place a version is written down.
    write_new(root, "package.json", &curated::package_json(&name), &mut written)?;
    write_new(root, "bun.lock", &curated::bun_lock(&name), &mut written)?;

    // The pin the R759 shim reads on every invocation. Written last of the
    // root files so a half-finished scaffold is not a directory that claims a
    // version it has no project for.
    write_new(root, ".mesofact-version", &format!("{MESOFACT_VERSION}\n"), &mut written)?;

    // The vended runtime barrel — types only; see the header of
    // `template/vendor-runtime.d.ts` for why it is not an npm dependency.
    let barrel_pkg = format!("{RUNTIME_BARREL_DIR}/package.json");
    let barrel_types = format!("{RUNTIME_BARREL_DIR}/index.d.ts");
    write_new(root, &barrel_pkg, &curated::barrel_package_json(), &mut written)?;
    write_new(root, &barrel_types, &expand(RUNTIME_BARREL_TYPES, &name), &mut written)?;

    println!("scaffolded {} ({} files)", root.display(), written.len());
    println!("  mesofact  {MESOFACT_VERSION}  (pinned in .mesofact-version)");
    println!();
    println!("  cd {}", root.display());
    println!("  mes --port 3000");
    println!();
    println!("The first build materializes node_modules/ from the committed bun.lock —");
    println!("no npm, bun or node needed. See README.md.");
    Ok(())
}

/// Substitute the two placeholders the templates carry. Deliberately not a
/// template engine: two literal replacements are auditable by reading the
/// template files, and a scaffold is exactly the place where "what did it
/// actually write" needs to be answerable without running it.
fn expand(body: &str, name: &str) -> String {
    body.replace("__PROJECT_NAME__", name)
        .replace("__MESOFACT_VERSION__", MESOFACT_VERSION)
}

/// Write a file, refusing to clobber. `--force` permits a non-empty target
/// directory but never an overwrite: a scaffold that can destroy work is a
/// scaffold nobody can safely run twice.
fn write_new(root: &Path, rel: &str, body: &str, written: &mut Vec<String>) -> Result<()> {
    let dest = root.join(rel);
    if dest.exists() {
        bail!("{} already exists — refusing to overwrite it", dest.display());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&dest, body).with_context(|| format!("writing {}", dest.display()))?;
    written.push(rel.to_string());
    Ok(())
}

/// The npm package-name rules that matter here. The name goes into
/// `package.json`, into `bun.lock`, and (unless `--name` was passed) is also
/// the directory name, so it has to be safe in all three — and
/// `curated::json_str` only escapes quotes and backslashes on the assumption
/// this ran first.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("project name is empty");
    }
    if name.len() > 214 {
        bail!("project name is {} chars; npm's limit is 214", name.len());
    }
    if name.starts_with('.') || name.starts_with('_') {
        bail!("project name {name:?} may not start with '.' or '_' (npm rule)");
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')))
    {
        bail!(
            "project name {name:?} contains {bad:?} — use lowercase letters, digits, '-', '_' or '.' (npm rule); pass --name to scaffold into a differently-named directory"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold(name: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join(name);
        run(NewArgs { path: root.clone(), name: None, force: false }).expect("scaffold");
        (tmp, root)
    }

    #[test]
    fn scaffolds_a_complete_project() {
        let (_tmp, root) = scaffold("demo");
        for f in [
            "package.json",
            "bun.lock",
            "tsconfig.json",
            "workload.toml",
            "mesofact.routes.ts",
            ".mesofact-version",
            ".gitignore",
            "README.md",
            "src/Page.tsx",
            "src/home.tsx",
            "src/not_found.tsx",
            "src/api.ts",
            "vendor/mesofact-runtime/package.json",
            "vendor/mesofact-runtime/index.d.ts",
        ] {
            assert!(root.join(f).is_file(), "{f} missing from the scaffold");
        }
    }

    /// Every entrypoint `mesofact.routes.ts` names must exist, or the first
    /// build fails on a file the scaffold itself was responsible for. This is
    /// the cheap half of the end-to-end check in
    /// `scripts/check-mesofact-new.sh`.
    #[test]
    fn every_declared_entrypoint_exists() {
        let (_tmp, root) = scaffold("demo");
        let routes = std::fs::read_to_string(root.join("mesofact.routes.ts")).unwrap();
        let mut found = 0;
        for line in routes.lines() {
            let Some(rest) = line.trim().strip_prefix("entrypoint: \"") else {
                continue;
            };
            let path = rest.trim_end_matches("\",").trim_end_matches('"');
            assert!(root.join(path).is_file(), "declared entrypoint {path} does not exist");
            found += 1;
        }
        assert!(found >= 3, "expected the scaffold to declare at least 3 entrypoints, saw {found}");
    }

    /// The dial. `.mesofact-version`, the vended barrel and the binary must
    /// name one version — that identity is the whole reason the scaffold
    /// lives in the binary rather than in a repo of templates.
    #[test]
    fn the_pin_is_this_binarys_version_everywhere() {
        let (_tmp, root) = scaffold("demo");
        let pin = std::fs::read_to_string(root.join(".mesofact-version")).unwrap();
        assert_eq!(pin.trim(), MESOFACT_VERSION);
        let barrel =
            std::fs::read_to_string(root.join("vendor/mesofact-runtime/package.json")).unwrap();
        assert!(barrel.contains(&format!("\"version\": \"{MESOFACT_VERSION}\"")), "{barrel}");
        let types =
            std::fs::read_to_string(root.join("vendor/mesofact-runtime/index.d.ts")).unwrap();
        assert!(types.contains(MESOFACT_VERSION), "the vended types name no version");
        assert!(!types.contains("__MESOFACT_VERSION__"), "placeholder left unexpanded");
    }

    #[test]
    fn no_placeholder_survives_into_the_output() {
        let (_tmp, root) = scaffold("my-app");
        for t in TEMPLATES {
            let body = std::fs::read_to_string(root.join(t.dest)).unwrap();
            assert!(!body.contains("__PROJECT_NAME__"), "{} kept a placeholder", t.dest);
            assert!(!body.contains("__MESOFACT_VERSION__"), "{} kept a placeholder", t.dest);
        }
        let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
        assert!(readme.contains("my-app"));
    }

    #[test]
    fn refuses_a_non_empty_directory_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("keep.txt"), "mine").unwrap();
        let err = run(NewArgs { path: root.clone(), name: None, force: false }).unwrap_err();
        assert!(err.to_string().contains("not empty"), "{err}");
        // …and --force still refuses to clobber the file that is there.
        assert!(root.join("keep.txt").is_file());
    }

    #[test]
    fn force_scaffolds_beside_existing_files_without_overwriting_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("keep.txt"), "mine").unwrap();
        run(NewArgs { path: root.clone(), name: None, force: true }).expect("forced scaffold");
        assert_eq!(std::fs::read_to_string(root.join("keep.txt")).unwrap(), "mine");
        assert!(root.join("package.json").is_file());
    }

    #[test]
    fn refuses_to_overwrite_a_file_it_would_have_written() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("package.json"), "{\"mine\":true}").unwrap();
        let err = run(NewArgs { path: root.clone(), name: None, force: true }).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"), "{err}");
        assert_eq!(std::fs::read_to_string(root.join("package.json")).unwrap(), "{\"mine\":true}");
    }

    #[test]
    fn rejects_names_that_are_not_legal_package_names() {
        for bad in ["MyApp", "my app", "_hidden", ".hidden", "a/b", ""] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
        for good in ["demo", "my-app", "my_app", "app.v2", "a1"] {
            assert!(validate_name(good).is_ok(), "{good:?} should be accepted");
        }
    }

    /// `--name` is what makes a directory whose name is not a legal package
    /// name usable, so it has to actually reach package.json.
    #[test]
    fn name_override_reaches_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("My Checkout");
        run(NewArgs { path: root.clone(), name: Some("my-app".into()), force: false })
            .expect("scaffold");
        let pkg = std::fs::read_to_string(root.join("package.json")).unwrap();
        assert!(pkg.contains("\"name\": \"my-app\""), "{pkg}");
    }
}
