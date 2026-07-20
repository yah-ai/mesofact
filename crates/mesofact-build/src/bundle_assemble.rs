//! Assemble a built mesofact app into a W272 **bundle** — the content-addressed
//! unit R599-F1's store publishes and a node materializes.
//!
//! Part of R599-F2. The canonical `@yah:` ticket annotation lives in
//! `.yah/docs/working/W272-mesofact-bundles-kamaji-jit-serving.md` (one block
//! per ID); this file is the mesofact-build-side emitter. It links the
//! `yah-mesofact-bundle` crate (R599-F1) with `default-features = false`, so the
//! bundle-manifest types cross into this subcamp workspace without the
//! object-store / reqwest weight the `store` feature carries.
//!
//! A vanilla bundle's tree (W272 §1):
//!
//! ```text
//! <bundle>/
//!   manifest.toml
//!   app/
//!     mesofact.routes.ts        # routes/config
//!     dist/…                    # built TS + rendered assets (the build out_dir)
//! ```
//!
//! A custom (`runtime = "self"`) bundle additionally carries
//! `bins/<triple>/serve`. The only shape difference between the two is whether
//! `bins/` is present and what `runtime` names — there is one format.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use yah_mesofact_bundle::{BundleHash, BundleManifest, BundleRuntime, SCHEMA_VERSION};

/// One file destined for a bundle: `(path within the bundle, source on disk)`.
/// The bundle path is always relative and forward-slashed, e.g.
/// `"app/dist/index.html"` or `"bins/x86_64-unknown-linux-musl/serve"`.
pub type BundleFile = (String, PathBuf);

/// Recursively enumerate the regular files under `dir`, mapping each to a
/// bundle path `"<prefix>/<relative-path>"`. Results are sorted for determinism.
pub fn collect_dir(prefix: &str, dir: &Path) -> Result<Vec<BundleFile>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let p = entry.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                let rel = p.strip_prefix(dir).expect("walker yields paths under dir");
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                out.push((format!("{prefix}/{rel_str}"), p));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Assemble a bundle tree at `dest` from `files`, returning the [`BundleManifest`].
///
/// Copies each source file to `<dest>/<bundle-path>`, hashes it (BLAKE3) into the
/// manifest `content` map, and writes `<dest>/manifest.toml` last. Files under
/// `bins/` are made executable (0755) — they are serve binaries. Re-assembling
/// over an existing `dest` overwrites in place (idempotent). `dest` MUST NOT sit
/// inside any of the source dirs being collected, or the copy would recurse into
/// its own output.
pub fn assemble_bundle(
    dest: &Path,
    name: &str,
    runtime: BundleRuntime,
    files: &[BundleFile],
) -> Result<BundleManifest> {
    fs::create_dir_all(dest).with_context(|| format!("creating bundle dir {}", dest.display()))?;

    let mut content = BTreeMap::new();
    for (bundle_path, src) in files {
        let bytes = fs::read(src).with_context(|| format!("reading {}", src.display()))?;
        let hash = BundleHash::of(&bytes);
        let out = dest.join(bundle_path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;

        // Serve binaries must stay executable after the copy.
        #[cfg(unix)]
        if bundle_path.starts_with("bins/") {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = fs::metadata(&out)?.permissions();
            perm.set_mode(0o755);
            fs::set_permissions(&out, perm)
                .with_context(|| format!("chmod +x {}", out.display()))?;
        }

        content.insert(bundle_path.clone(), hash);
    }

    let manifest = BundleManifest {
        schema_version: SCHEMA_VERSION,
        name: name.to_string(),
        runtime,
        content,
    };
    let toml = manifest
        .to_toml_string()
        .map_err(|e| anyhow::anyhow!("serializing manifest.toml: {e}"))?;
    fs::write(dest.join("manifest.toml"), toml)
        .with_context(|| format!("writing {}", dest.join("manifest.toml").display()))?;
    Ok(manifest)
}

/// Assemble a **vanilla** bundle from a finished mesofact build: `app/` holds the
/// routes file plus the built `dist/` tree, no `bins/`, and `runtime =
/// "mesofact/<runtime_version>"` so the node resolves the stock serve runtime.
///
/// `out_dir` is the build's dist directory ([`crate::pipeline::BuildResult::out_dir`]);
/// `runtime_version` is the version of `mesofact serve` that should serve this
/// bundle (e.g. the framework version the app was built against).
pub fn assemble_vanilla_bundle(
    dest: &Path,
    name: &str,
    runtime_version: &str,
    project_root: &Path,
    out_dir: &Path,
) -> Result<BundleManifest> {
    let mut files = collect_dir("app/dist", out_dir)?;
    let routes = project_root.join("mesofact.routes.ts");
    if routes.exists() {
        files.push(("app/mesofact.routes.ts".to_string(), routes));
    }
    files.sort();
    assemble_bundle(
        dest,
        name,
        BundleRuntime::Mesofact {
            version: runtime_version.to_string(),
        },
        &files,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn collect_dir_walks_recursively_and_prefixes() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join("index.html"), b"a");
        write(&dir.path().join("assets/main.js"), b"b");
        write(&dir.path().join("assets/nested/x.css"), b"c");

        let files = collect_dir("app/dist", dir.path()).unwrap();
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "app/dist/assets/main.js",
                "app/dist/assets/nested/x.css",
                "app/dist/index.html",
            ]
        );
    }

    #[test]
    fn assemble_bundle_copies_files_hashes_and_writes_manifest() {
        let src = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();
        write(&src.path().join("index.html"), b"<html>home</html>");

        let files = vec![(
            "app/index.html".to_string(),
            src.path().join("index.html"),
        )];
        let manifest = assemble_bundle(
            dest.path(),
            "yah-marketing",
            BundleRuntime::Mesofact { version: "0.8.20".into() },
            &files,
        )
        .unwrap();

        // Manifest content matches the copied bytes' hash.
        assert_eq!(
            manifest.content.get("app/index.html").unwrap(),
            &BundleHash::of(b"<html>home</html>")
        );
        // The file was copied into the bundle tree.
        assert_eq!(
            fs::read(dest.path().join("app/index.html")).unwrap(),
            b"<html>home</html>"
        );
        // manifest.toml on disk round-trips back to the returned manifest.
        let text = fs::read_to_string(dest.path().join("manifest.toml")).unwrap();
        assert_eq!(BundleManifest::from_toml_str(&text).unwrap(), manifest);
    }

    #[test]
    fn assemble_vanilla_bundle_stages_routes_and_dist() {
        let project = TempDir::new().unwrap();
        let out_dir = project.path().join("dist");
        let dest = TempDir::new().unwrap();
        write(&project.path().join("mesofact.routes.ts"), b"export default {}");
        write(&out_dir.join("index.html"), b"<html>");
        write(&out_dir.join("hydrate/app.hash.js"), b"hydrate");

        let manifest = assemble_vanilla_bundle(
            dest.path(),
            "yah-marketing",
            "0.8.20",
            project.path(),
            &out_dir,
        )
        .unwrap();

        assert!(matches!(
            manifest.runtime,
            BundleRuntime::Mesofact { .. }
        ));
        assert!(manifest.content.contains_key("app/mesofact.routes.ts"));
        assert!(manifest.content.contains_key("app/dist/index.html"));
        assert!(manifest.content.contains_key("app/dist/hydrate/app.hash.js"));
        // A vanilla bundle carries no serve binaries.
        assert!(!manifest.content.keys().any(|k| k.starts_with("bins/")));
        assert!(dest.path().join("app/mesofact.routes.ts").exists());
    }

    #[test]
    #[cfg(unix)]
    fn bins_are_made_executable() {
        use std::os::unix::fs::PermissionsExt;
        let src = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();
        write(&src.path().join("serve"), b"\x7fELF-ish");

        let files = vec![(
            "bins/x86_64-unknown-linux-musl/serve".to_string(),
            src.path().join("serve"),
        )];
        assemble_bundle(dest.path(), "app", BundleRuntime::SelfContained, &files).unwrap();

        let mode = fs::metadata(dest.path().join("bins/x86_64-unknown-linux-musl/serve"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "serve binary should be executable");
    }
}
