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
//! # Two tiers
//!
//! - **Standalone** (default) — a TypeScript project the shipped `mesofact` /
//!   `mesofact-dev` pair runs directly, with no consumer Rust. [`TEMPLATES`],
//!   plus the curated JS set derived from [`curated`]. The paragraph above is
//!   about this tier.
//! - **Library** (`--lib`, R832-T2) — W225 §2's Consumer DX shape: one Rust
//!   crate, `src/lib.rs` holding `router()` and the handlers, two thin bin
//!   targets over it, and CI that release-builds both and ships only the prod
//!   one. [`LIB_TEMPLATES`]. No JS half at all, and its version dial is
//!   `Cargo.toml` rather than `.mesofact-version` — it builds its own binaries
//!   instead of being run by the shipped pair.
//!
//! ## What the library tier waited on
//!
//! Both blockers were cleared by R832-T1 and are recorded because the shape of
//! the answer constrains what may be added later:
//!
//! 1. **`mesofact-dev` is publishable** — the `publish = false` its manifest
//!    carried since its first commit is gone, and it declares the `version` on
//!    its path dep that publishing requires. It is still *unpublished*: the
//!    name was unclaimed on crates.io (sparse-index 404, checked 2026-09-01)
//!    and lands on the next `scripts/oss-publish.sh oss/mesofact`. **Until
//!    that upload an emitted `Cargo.toml` resolves only in-tree**, which is
//!    why `scripts/check-mesofact-new.sh` patches the two deps to workspace
//!    paths before building the scaffold. Drop that patch once the upload has
//!    happened; the cell should still pass.
//! 2. **The dev tier has a library entry point** — `mesofact_dev::serve_app`
//!    (`mesofact-dev/src/app.rs`), the dev counterpart of
//!    [`crate::serve_app`]. W225 §2 sketched `mesofact_dev::serve(…)` and
//!    nothing implemented it, because the dev affordances were composed in
//!    the dev CLI's own entry point around a *workload* — a built `dist/`
//!    tree plus a watcher — not around a caller's `axum::Router`. That
//!    module's doc records what R832-T1 settled: for a Rust-handler consumer
//!    the watcher and live-reload cannot carry over at all (a `.rs` edit
//!    re-links the very process that would perform the reload), the dev object
//!    store is the whole of the difference, and the pattern's value is the
//!    link-graph boundary rather than the size of the dev-side delta.

pub mod curated;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;

use curated::MESOFACT_VERSION;

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

/// The **library tier** (`--lib`) — W225 §2's Consumer DX shape: one crate,
/// `src/lib.rs` holding `router()` and the handlers, two thin bin targets over
/// it, and CI that release-builds both and ships only the prod one.
///
/// A separate table rather than a flag threaded through [`TEMPLATES`] because
/// the two tiers share no file. The standalone tier is a TypeScript project
/// the shipped binaries run; this one is a Rust crate that builds its own
/// binaries and has no JS half at all — no curated set, no `bun.lock`, no
/// vended barrel, no `.mesofact-version` (its `Cargo.toml` is the dial).
///
/// Note the two `dest` paths carrying `__PROJECT_NAME__`: cargo names a bin
/// target after its file stem, so the file names *are* the binary names.
/// [`expand`] runs over `dest` as well as `body` for exactly this.
///
/// The two on-disk names that differ from their `dest` are load-bearing, not
/// cosmetic. `gitignore` keeps a `.gitignore` here from making git ignore the
/// template's own files; `Cargo.toml.tmpl` keeps `cargo package` from seeing a
/// *nested package* under `src/` and silently dropping the entire
/// `template-lib/` directory from the tarball — which it did, turning these
/// `include_str!`s into "no such file or directory" during publish verification
/// while the local build stayed green (R620/oss-publish, 2026-09-02).
const LIB_TEMPLATES: &[Template] = &[
    Template { dest: "README.md", body: include_str!("template-lib/README.md") },
    Template { dest: ".gitignore", body: include_str!("template-lib/gitignore") },
    Template { dest: "Cargo.toml", body: include_str!("template-lib/Cargo.toml.tmpl") },
    Template { dest: "src/lib.rs", body: include_str!("template-lib/src/lib.rs") },
    Template {
        dest: "src/bin/__PROJECT_NAME__.rs",
        body: include_str!("template-lib/src/bin/__PROJECT_NAME__.rs"),
    },
    Template {
        dest: "src/bin/__PROJECT_NAME__-dev.rs",
        body: include_str!("template-lib/src/bin/__PROJECT_NAME__-dev.rs"),
    },
    Template {
        dest: ".github/workflows/ci.yml",
        body: include_str!("template-lib/.github/workflows/ci.yml"),
    },
];

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

    /// Scaffold the **library tier** instead of the standalone tier: a Rust
    /// crate whose routes are handlers, building its own prod and dev
    /// binaries over `mesofact` / `mesofact-dev` (W225 §2 Consumer DX).
    ///
    /// Without it you get the standalone tier — a TypeScript project the two
    /// shipped binaries run directly, needing no consumer Rust.
    #[arg(long)]
    pub lib: bool,
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
    if args.lib {
        validate_crate_name(&name)?;
    }

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
    if args.lib {
        return scaffold_library(root, &name, &mut written);
    }

    for t in TEMPLATES {
        write_new(root, &expand(t.dest, &name), &expand(t.body, &name), &mut written)?;
    }

    // Generated, not templated: both derive from `curated::CURATED`, which is
    // the only place a version is written down.
    write_new(root, "package.json", &curated::package_json(&name), &mut written)?;
    write_new(root, "bun.lock", &curated::bun_lock(&name), &mut written)?;

    // The pin the R759 shim reads on every invocation. Written last of the
    // root files so a half-finished scaffold is not a directory that claims a
    // version it has no project for.
    write_new(root, ".mesofact-version", &format!("{MESOFACT_VERSION}\n"), &mut written)?;

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

/// The library tier: a Rust crate with two bin targets over one `router()`.
///
/// Nothing derived from [`curated`] is written here. The curated JS set is the
/// standalone tier's promise — this tier has no JS, and its version dial is
/// `Cargo.toml` rather than `.mesofact-version`, because it builds its own
/// binaries instead of being run by the shipped pair.
fn scaffold_library(root: &Path, name: &str, written: &mut Vec<String>) -> Result<()> {
    for t in LIB_TEMPLATES {
        write_new(root, &expand(t.dest, name), &expand(t.body, name), written)?;
    }

    println!("scaffolded {} ({} files, library tier)", root.display(), written.len());
    println!("  mesofact / mesofact-dev  {MESOFACT_VERSION}  (pinned in Cargo.toml)");
    println!();
    println!("  cd {}", root.display());
    println!("  cargo run --bin {name}-dev");
    println!();
    println!("`cargo build --release` emits both binaries with no flags. Ship {name};");
    println!("{name}-dev links the dev affordances and stays on your machine. See README.md.");
    Ok(())
}

/// Substitute the placeholders the templates carry. Deliberately not a
/// template engine: three literal replacements are auditable by reading the
/// template files, and a scaffold is exactly the place where "what did it
/// actually write" needs to be answerable without running it.
///
/// Runs over a template's `dest` as well as its `body`: the library tier's bin
/// targets are named after their file stems, so `src/bin/__PROJECT_NAME__.rs`
/// is how the binary gets the project's name.
fn expand(body: &str, name: &str) -> String {
    body.replace("__PROJECT_NAME__", name)
        .replace("__CRATE_NAME__", &crate_name(name))
        .replace("__MESOFACT_VERSION__", MESOFACT_VERSION)
}

/// The Rust identifier cargo derives from a package name — hyphens become
/// underscores. `src/bin/my-app.rs` has to say `my_app::router()`, so the
/// templates need both spellings.
fn crate_name(name: &str) -> String {
    name.replace('-', "_")
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

/// The extra rules `--lib` adds on top of [`validate_name`], because the name
/// becomes a **cargo** package name and a Rust identifier as well as an npm
/// one. Cargo's set is the narrower of the two: `.` is legal in an npm package
/// name and illegal in a crate name, so `mesofact new app.v2` is fine and
/// `mesofact new --lib app.v2` must not be — a scaffold that emits a
/// `Cargo.toml` cargo refuses to parse is worse than a clear refusal here.
fn validate_crate_name(name: &str) -> Result<()> {
    if name.contains('.') {
        bail!(
            "project name {name:?} contains '.', which cargo does not allow in a package name — pass --name to scaffold into a differently-named directory"
        );
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        bail!("project name {name:?} starts with a digit; a Rust crate name may not");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold(name: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join(name);
        run(NewArgs { path: root.clone(), name: None, force: false, lib: false }).expect("scaffold");
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
        ] {
            assert!(root.join(f).is_file(), "{f} missing from the scaffold");
        }
        // R832-T3 retired the vended barrel for the published package. Asserted
        // as an absence because a leftover `vendor/` would still typecheck —
        // it would just silently shadow the registry types with a stale subset.
        assert!(
            !root.join("vendor").exists(),
            "the scaffold still vends a runtime barrel; it takes @mesofact/runtime from the lock now"
        );
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

    /// The dial. `.mesofact-version`, the emitted `@mesofact/runtime` pin and
    /// the binary must name one version — that identity is the whole reason
    /// the scaffold lives in the binary rather than in a repo of templates.
    ///
    /// Checked here on the *emitted files* rather than only on the table
    /// (`curated::tests::runtime_barrel_is_pinned_to_this_binarys_version`
    /// does that) because the two artifacts are what a consumer actually
    /// installs from.
    #[test]
    fn the_pin_is_this_binarys_version_everywhere() {
        let (_tmp, root) = scaffold("demo");
        let pin = std::fs::read_to_string(root.join(".mesofact-version")).unwrap();
        assert_eq!(pin.trim(), MESOFACT_VERSION);
        let pkg = std::fs::read_to_string(root.join("package.json")).unwrap();
        assert!(
            pkg.contains(&format!("\"@mesofact/runtime\": \"{MESOFACT_VERSION}\"")),
            "package.json does not pin the barrel to {MESOFACT_VERSION}:\n{pkg}"
        );
        let lock = std::fs::read_to_string(root.join("bun.lock")).unwrap();
        assert!(
            lock.contains(&format!("\"@mesofact/runtime@{MESOFACT_VERSION}\"")),
            "bun.lock does not lock the barrel at {MESOFACT_VERSION}:\n{lock}"
        );
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
        let err = run(NewArgs { path: root.clone(), name: None, force: false, lib: false }).unwrap_err();
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
        run(NewArgs { path: root.clone(), name: None, force: true, lib: false }).expect("forced scaffold");
        assert_eq!(std::fs::read_to_string(root.join("keep.txt")).unwrap(), "mine");
        assert!(root.join("package.json").is_file());
    }

    #[test]
    fn refuses_to_overwrite_a_file_it_would_have_written() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("demo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("package.json"), "{\"mine\":true}").unwrap();
        let err = run(NewArgs { path: root.clone(), name: None, force: true, lib: false }).unwrap_err();
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
        run(NewArgs { path: root.clone(), name: Some("my-app".into()), force: false, lib: false })
            .expect("scaffold");
        let pkg = std::fs::read_to_string(root.join("package.json")).unwrap();
        assert!(pkg.contains("\"name\": \"my-app\""), "{pkg}");
    }

    // ── the library tier (--lib) ────────────────────────────────────────────

    fn scaffold_lib(name: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join(name);
        run(NewArgs { path: root.clone(), name: None, force: false, lib: true })
            .expect("lib scaffold");
        (tmp, root)
    }

    #[test]
    fn lib_tier_scaffolds_one_crate_and_two_bins() {
        let (_tmp, root) = scaffold_lib("my-app");
        for f in [
            "Cargo.toml",
            "README.md",
            ".gitignore",
            "src/lib.rs",
            "src/bin/my-app.rs",
            "src/bin/my-app-dev.rs",
            ".github/workflows/ci.yml",
        ] {
            assert!(root.join(f).is_file(), "{f} missing from the --lib scaffold");
        }
    }

    /// The tiers are disjoint, and the library tier must not quietly inherit
    /// the standalone one's JS half — a Rust-handler service has no bundler,
    /// no curated set and no `.mesofact-version` dial.
    #[test]
    fn lib_tier_emits_no_js_half() {
        let (_tmp, root) = scaffold_lib("my-app");
        for f in [
            "package.json",
            "bun.lock",
            ".mesofact-version",
            "mesofact.routes.ts",
            "workload.toml",
            "node_modules",
        ] {
            assert!(!root.join(f).exists(), "{f} leaked into the --lib scaffold");
        }
    }

    /// Lines that are not comments. Both files under test explain the
    /// prod/dev boundary in prose, so they legitimately *name* `mesofact_dev`
    /// and `target/release/<name>*` while containing neither — the assertions
    /// below are about what the file does, not what it says.
    fn code_only(body: &str, marker: &str) -> String {
        body.lines()
            .filter(|l| !l.trim_start().starts_with(marker))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The one edit that silently breaks the prod/dev boundary is a
    /// `mesofact_dev` reference reachable from the prod binary. The scaffold
    /// must not ship one already made.
    #[test]
    fn only_the_dev_bin_reaches_the_dev_crate() {
        let (_tmp, root) = scaffold_lib("my-app");
        let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
        let prod = std::fs::read_to_string(root.join("src/bin/my-app.rs")).unwrap();
        let dev = std::fs::read_to_string(root.join("src/bin/my-app-dev.rs")).unwrap();

        let lib_code = code_only(&lib, "//");
        let prod_code = code_only(&prod, "//");
        assert!(!lib_code.contains("mesofact_dev"), "src/lib.rs reaches the dev crate:\n{lib}");
        assert!(
            !prod_code.contains("mesofact_dev"),
            "the prod bin reaches the dev crate:\n{prod}"
        );
        assert!(prod_code.contains("mesofact::serve_app"), "{prod}");
        assert!(code_only(&dev, "//").contains("mesofact_dev::serve_app"), "{dev}");
    }

    /// Both deps plain and non-optional, at this binary's version, and no
    /// feature-flag regression of the two-bin pattern.
    #[test]
    fn lib_tier_manifest_names_both_crates_plainly() {
        let (_tmp, root) = scaffold_lib("my-app");
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(manifest.contains(&format!("mesofact = \"{MESOFACT_VERSION}\"")), "{manifest}");
        assert!(manifest.contains(&format!("mesofact-dev = \"{MESOFACT_VERSION}\"")), "{manifest}");
        assert!(manifest.contains("name = \"my-app\""), "{manifest}");
        assert!(!manifest.contains("optional = true"), "a dep was made optional:\n{manifest}");
        assert!(!manifest.contains("[features]"), "the two-bin shape grew a feature:\n{manifest}");
    }

    /// `src/bin/my-app.rs` has to say `my_app::router()` — cargo derives the
    /// crate's Rust name by swapping hyphens for underscores, and a scaffold
    /// that emits `my-app::router()` does not compile.
    #[test]
    fn the_snake_case_crate_name_reaches_both_bins() {
        let (_tmp, root) = scaffold_lib("my-app");
        for bin in ["src/bin/my-app.rs", "src/bin/my-app-dev.rs"] {
            let body = std::fs::read_to_string(root.join(bin)).unwrap();
            assert!(body.contains("my_app::router()"), "{bin} does not call my_app::router():\n{body}");
            assert!(!body.contains("my-app::"), "{bin} used the package name as an identifier");
        }
    }

    #[test]
    fn no_placeholder_survives_into_the_lib_output() {
        let (_tmp, root) = scaffold_lib("my-app");
        for t in LIB_TEMPLATES {
            let dest = expand(t.dest, "my-app");
            let body = std::fs::read_to_string(root.join(&dest)).unwrap();
            for placeholder in ["__PROJECT_NAME__", "__CRATE_NAME__", "__MESOFACT_VERSION__"] {
                assert!(!body.contains(placeholder), "{dest} kept {placeholder}");
            }
            assert!(!dest.contains("__"), "{dest} kept a placeholder in its path");
        }
    }

    /// CI ships one binary. A glob here is the failure mode — it would upload
    /// the dev binary alongside the prod one, which is the whole thing the
    /// tier's structure exists to prevent.
    #[test]
    fn ci_uploads_the_prod_binary_only() {
        let (_tmp, root) = scaffold_lib("my-app");
        let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
        let steps = code_only(&ci, "#");
        assert!(steps.contains("cargo build --release"), "{ci}");
        assert!(steps.contains("path: target/release/my-app\n"), "{ci}");
        assert!(!steps.contains("target/release/my-app*"), "the upload path is a glob:\n{ci}");
        assert!(!steps.contains("target/release/my-app-dev"), "CI ships the dev binary:\n{ci}");
    }

    /// npm allows `.` in a package name and cargo does not, so a name the
    /// standalone tier accepts can still be illegal here. Refusing is better
    /// than emitting a `Cargo.toml` cargo will not parse.
    #[test]
    fn lib_tier_rejects_names_cargo_would_not_take() {
        assert!(validate_name("app.v2").is_ok(), "npm accepts a dot");
        assert!(validate_crate_name("app.v2").is_err(), "cargo does not");
        assert!(validate_crate_name("2fast").is_err(), "a crate name may not start with a digit");
        for good in ["my-app", "my_app", "demo", "a1"] {
            assert!(validate_crate_name(good).is_ok(), "{good:?} should be accepted");
        }

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app.v2");
        let err = run(NewArgs { path: root.clone(), name: None, force: false, lib: true })
            .unwrap_err();
        assert!(err.to_string().contains("cargo does not allow"), "{err}");
        assert!(!root.exists(), "a rejected --lib scaffold left a directory behind");
    }
}
