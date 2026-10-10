//! Build orchestration — the Rust-native mirror of
//! `packages/mesofact-build/src/index.ts::build()`. Phase order is kept
//! identical so failures surface at the same points:
//!
//! 1. (optional) install — lockfile-driven node_modules materialization
//! 2. routes load (bundle mesofact.routes.ts → evaluate in deno_core)
//! 3. host-lint (browser/edge forbidden imports)
//! 4. server bundles (Rolldown) + client bundles (Rolldown, hashed)
//! 5. SSR default-export probe
//! 6. source inference (regex scan, author override wins)
//! 7. static-asset discovery (public/ → dist/html/ + manifest)
//! 8. manifest assembly + validation
//! 9. prerender (deno_core SSG) → dist/html/*.html
//! 10. build id derived from the staged tree, substituted into it (R870-B12)
//! 11. manifest + tag-index emission

use anyhow::{anyhow, bail, Context, Result};
use mesofact_core::manifest::{Hydration, RouteMode};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yah_mesofact_bundle::BundleHash;

use crate::bundle::{
    assert_no_forbidden_modules, browser_forbidden, bundle_client_entrypoints, bundle_hooks,
    bundle_routes_file, bundle_server_entrypoints, edge_forbidden,
};
use crate::config::load_config;
use crate::install;
use crate::js::SsgRuntime;
use crate::prerender::{prerender, RenderTarget};
use crate::route_config::{validate_routes_config, Placement, RoutesConfig};
use crate::source_infer::infer_from_file;
use crate::tag_index::build_tag_index;

pub struct BuildOptions {
    pub project_root: PathBuf,
    pub out_dir: Option<PathBuf>,
    pub build_id: Option<String>,
    /// Run the lockfile-driven install step before building. `Auto` installs
    /// only when node_modules is missing.
    pub install: InstallMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Auto,
    Always,
    Never,
}

pub struct BuildResult {
    pub build_id: String,
    pub out_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub tag_index_path: PathBuf,
    pub html_paths: Vec<String>,
    /// `dist/sitemap.xml` when `routes.site_url` is configured, else `None`.
    pub sitemap_path: Option<PathBuf>,
}

/// What a build's id stands in for while the tree it names is still being
/// written (R870-B12).
///
/// The default build id is a hash **of the built tree**, so it cannot be known
/// until the tree exists — and the tree can't be finished without it, because
/// the hydration weave bakes `/{build_id}/hydrate/<script>` into every
/// prerendered shell. This placeholder breaks that cycle: prerender weaves it,
/// the id is derived over the resulting bytes, and every staged file carrying
/// it is rewritten in place.
///
/// It is deliberately shaped like nothing an author would type and nothing a
/// bundler emits, because the substitution is a byte-level search over the
/// staged tree.
const BUILD_ID_PLACEHOLDER: &str = "__mesofact_build_id__";

/// Hex digits of the tree hash kept as the build id.
///
/// 128 bits — collision-proof for the purpose (naming one immutable build of
/// one site) while staying short enough to read in a URL path, which is where
/// it is actually seen: `/{build_id}/hydrate/…` and `<build_id>/html/…`.
const BUILD_ID_HEX_LEN: usize = 32;

/// The staged output tree, as it stands the moment the build id is derived.
struct StagedTree {
    /// `out_dir`-relative, `/`-separated path → BLAKE3 of the staged bytes.
    hashes: BTreeMap<String, String>,
    /// Absolute paths of the staged files carrying [`BUILD_ID_PLACEHOLDER`],
    /// i.e. exactly those the substitution pass has to rewrite.
    placeholder_files: Vec<PathBuf>,
}

/// Files `build` writes *after* the id is derived. They are excluded from the
/// scan because they do not exist yet — and, more importantly, because a stale
/// copy left in `out_dir` by an earlier build must not feed into the id.
const DERIVED_AFTER_ID: [&str; 3] = ["manifest.json", "tag-index.json", "sitemap.xml"];

/// Walk the staged tree, hashing every file and noting which ones carry the
/// placeholder.
fn scan_staged_tree(out_dir: &Path) -> Result<StagedTree> {
    let mut tree = StagedTree { hashes: BTreeMap::new(), placeholder_files: Vec::new() };
    let needle = BUILD_ID_PLACEHOLDER.as_bytes();

    fn walk(dir: &Path, prefix: &str, needle: &[u8], tree: &mut StagedTree) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            // The routes bundle lives here and is deleted before `build`
            // returns, so it is not part of the tree the id names.
            if rel == ".mesofact-build" {
                continue;
            }
            if prefix.is_empty() && DERIVED_AFTER_ID.contains(&name.as_str()) {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(&path, &rel, needle, tree)?;
            } else {
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                if bytes.windows(needle.len()).any(|w| w == needle) {
                    tree.placeholder_files.push(path);
                }
                tree.hashes.insert(rel, BundleHash::of(&bytes).as_str().to_string());
            }
        }
        Ok(())
    }

    if out_dir.is_dir() {
        walk(out_dir, "", needle, &mut tree)?;
    }
    Ok(tree)
}

/// Derive the build id from what the build produced (R870-B12).
///
/// Content-addressed rather than clock-stamped, for the reason
/// [`PublishBeacon::for_bundle`] is clock-free (R703-T7): this value ends up
/// *inside* the unit being content-addressed. A wall clock made the built tree
/// different bytes on every run from an unchanged source, which flipped the
/// W272 bundle digest on every apply and cost the whole of §1 immutability —
/// no blob dedupe, a re-materialize and a serve-process re-fork per apply, and
/// a measured ~2s of 502 on a site nothing had changed.
///
/// The digest covers the staged tree in its *placeholder* form plus the
/// manifest that describes it, so it is a total function of the build's own
/// output: any byte that moves — a bundle, an asset, a rendered page, a route
/// table entry — moves the id, and nothing else does. It cannot cover its own
/// substituted form, for the same reason the bundle beacon excludes itself: a
/// digest covering itself has no fixed point.
fn derive_build_id(tree: &StagedTree, manifest_json: &str) -> String {
    let mut canonical = String::new();
    for (path, hash) in &tree.hashes {
        canonical.push_str(path);
        canonical.push('\0');
        canonical.push_str(hash);
        canonical.push('\n');
    }
    canonical.push_str("manifest\0");
    canonical.push_str(manifest_json);
    BundleHash::of(canonical.as_bytes()).as_str()[..BUILD_ID_HEX_LEN].to_string()
}

/// Rewrite the placeholder to the derived id in every staged file carrying it.
fn substitute_build_id(files: &[PathBuf], build_id: &str) -> Result<()> {
    for path in files {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let text = String::from_utf8(bytes).with_context(|| {
            format!("{} carries the build-id placeholder but is not UTF-8", path.display())
        })?;
        std::fs::write(path, text.replace(BUILD_ID_PLACEHOLDER, build_id))
            .with_context(|| format!("rewriting {} with the derived build id", path.display()))?;
    }
    Ok(())
}

pub async fn build(opts: BuildOptions) -> Result<BuildResult> {
    let project_root = opts
        .project_root
        .canonicalize()
        .with_context(|| format!("project root {}", opts.project_root.display()))?;
    let out_dir = match opts.out_dir {
        Some(d) => {
            std::fs::create_dir_all(&d)?;
            d.canonicalize()?
        }
        None => project_root.join("dist"),
    };
    // An explicit id is taken as given (tests, and any caller that wants to
    // name the build itself). Otherwise the id is derived from the built tree
    // below, and the placeholder stands in until then — see
    // [`BUILD_ID_PLACEHOLDER`].
    let explicit_build_id = opts.build_id;
    let staged_build_id =
        explicit_build_id.clone().unwrap_or_else(|| BUILD_ID_PLACEHOLDER.to_string());

    // Phase 0 — install.
    let node_modules = project_root.join("node_modules");
    let should_install = match opts.install {
        InstallMode::Always => true,
        InstallMode::Never => false,
        InstallMode::Auto => {
            // Every format `install::detect_lockfile` reads, in its order.
            !node_modules.exists()
                && (project_root.join("bun.lock").exists()
                    || project_root.join("package-lock.json").exists()
                    || project_root.join("pnpm-lock.yaml").exists())
        }
    };
    if should_install {
        // `install` is end-to-end blocking (reqwest::blocking + gzip/tar
        // extraction), and `reqwest::blocking::ClientBuilder::build` builds
        // *and drops* a private tokio runtime on the calling thread. Dropping
        // a runtime while a future is being polled panics with "Cannot drop a
        // runtime in a context where blocking is not allowed", which is what
        // took out every `desktop-release` run whose isolated worktree had no
        // node_modules yet. Hand the phase to the blocking pool, where
        // blocking — and therefore that drop — is legal.
        let root = project_root.clone();
        let report = tokio::task::spawn_blocking(move || install::install(&root))
            .await
            .context("install phase panicked")??;
        if !report.skipped_fresh {
            tracing::info!(
                installed = report.installed,
                linked = report.linked,
                "installed node_modules from the project lockfile"
            );
        }
    }

    // Phase 1 — routes: bundle the routes file, evaluate it in the SSG
    // isolate, validate with defineRoutes-parity rules.
    let scratch = out_dir.join(".mesofact-build");
    let routes_bundle = bundle_routes_file(&project_root, &scratch).await?;
    let ssg = SsgRuntime::start()?;
    let routes_json = ssg
        .eval_routes(&routes_bundle)
        .with_context(|| format!("evaluating {}", project_root.join("mesofact.routes.ts").display()))?;
    let routes_config: RoutesConfig = serde_json::from_value(routes_json)
        .context("mesofact.routes.ts evaluated to an unexpected shape")?;
    validate_routes_config(&routes_config)?;

    let config = load_config(&project_root)?;

    // Phase 2 — bundles. Server first (parity with index.ts ordering), then
    // the client tree for spa / islands / universal routes.
    let server_inputs: Vec<(String, String)> = routes_config
        .routes
        .iter()
        .map(|r| (r.route.clone(), r.entrypoint.clone()))
        .collect();
    let server_bundles = bundle_server_entrypoints(&project_root, &out_dir, &server_inputs).await?;
    let server_paths: BTreeMap<String, String> =
        server_bundles.iter().map(|b| (b.route.clone(), b.server_path.clone())).collect();
    let bundle_paths: BTreeMap<String, PathBuf> =
        server_bundles.iter().map(|b| (b.route.clone(), b.absolute_path.clone())).collect();

    // Mode 2 hook tree (R756-F6) — engine-addressed rather than path-addressed,
    // so hooks bundle beside the route tree under `dist/server/hooks/` and
    // never enter the route table or `ssr_prefixes`.
    let hook_bundles = bundle_hooks(
        &project_root,
        &out_dir,
        routes_config.hooks.as_ref().unwrap_or(&BTreeMap::new()),
    )
    .await?;
    let hook_paths: BTreeMap<String, String> =
        hook_bundles.iter().map(|h| (h.name.clone(), h.server_path.clone())).collect();

    let client_inputs: Vec<(String, String)> = routes_config
        .routes
        .iter()
        .filter_map(|r| {
            r.client_entrypoint.as_ref().map(|c| (r.route.clone(), c.clone()))
        })
        .collect();
    let client_bundles = bundle_client_entrypoints(&project_root, &out_dir, &client_inputs).await?;
    let hydration: BTreeMap<String, Hydration> = client_bundles
        .iter()
        .map(|c| {
            (
                c.route.clone(),
                Hydration { script: c.script.clone(), code_split: c.code_split.clone() },
            )
        })
        .collect();

    // Phase 3 — boundary lint over the captured module graphs (W173). The
    // TS pipeline lints pre-bundle with an onResolve walk; rolldown gives us
    // the resolved graph post-bundle. Unresolvable forbidden ids already
    // failed the bundle with the offending specifier in the error.
    for c in &client_bundles {
        assert_no_forbidden_modules(
            &c.route,
            "client_entrypoint",
            &c.module_ids,
            &c.import_ids,
            browser_forbidden,
        )?;
    }
    for r in &routes_config.routes {
        if r.mode == RouteMode::Ssr && r.placement == Some(Placement::Edge) {
            let b = server_bundles
                .iter()
                .find(|b| b.route == r.route)
                .ok_or_else(|| anyhow!("route {}: no bundled entrypoint", r.route))?;
            assert_no_forbidden_modules(
                &r.route,
                "ssr placement:\"edge\" entrypoint",
                &b.module_ids,
                &b.import_ids,
                edge_forbidden,
            )?;
        }
    }

    // Phase 4 — SSR default-export probe (before the manifest hits disk).
    for r in &routes_config.routes {
        if r.mode != RouteMode::Ssr {
            continue;
        }
        let bundle = bundle_paths
            .get(&r.route)
            .ok_or_else(|| anyhow!("route {}: no bundled entrypoint", r.route))?;
        let probe = ssg.probe_default(bundle)?;
        let kind = probe.get("kind").and_then(serde_json::Value::as_str).unwrap_or("unknown");
        if kind != "function" {
            bail!(
                "route {}: mode:\"ssr\" entrypoint must `export default` a Fetch handler `(req: Request) => Promise<Response>` (got {kind})",
                r.route
            );
        }
    }

    // Phase 4b — hook default-export probe (R756-F6). Same reasoning as the
    // SSR probe: what a hook module is *called with* varies per hook, so only
    // callability is provable from the module's static shape — but that
    // catches the common slip (a named export, or none at all) before the
    // manifest promises the engine a module it cannot invoke.
    for h in &hook_bundles {
        let probe = ssg.probe_default(&h.absolute_path)?;
        let kind = probe.get("kind").and_then(serde_json::Value::as_str).unwrap_or("unknown");
        if kind != "function" {
            bail!("hook {}: entrypoint must `export default` a function (got {kind})", h.name);
        }
    }

    // Phase 5 — source inference (author-supplied wins).
    let mut inferred_sources: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in &routes_config.routes {
        if let Some(explicit) = &r.source_reads {
            inferred_sources.insert(r.route.clone(), explicit.clone());
            continue;
        }
        let entry = project_root.join(&r.entrypoint);
        inferred_sources.insert(r.route.clone(), infer_from_file(&entry)?.source_reads);
    }

    // Phase 6 — static assets (R490-F4). Immutability is the component's own
    // `[build] immutable` declaration (MFT-R825-F1) — the same patterns the
    // publish-time asset index is built from, so the manifest and the served
    // `Cache-Control` cannot disagree.
    let declared_immutable = yah_mesofact_bundle::assets::declared_immutable(&project_root)?;
    let static_assets = crate::assets::discover_static_assets(
        &project_root,
        &out_dir,
        &config.public_dir,
        &declared_immutable,
    )?;

    // Phase 7 — manifest assembly + validation (before any HTML lands).
    let mut manifest = crate::manifest_build::assemble_manifest(crate::manifest_build::AssembleInput {
        routes: &routes_config,
        build_id: &staged_build_id,
        server_paths: &server_paths,
        inferred_sources: &inferred_sources,
        hydration: &hydration,
        static_assets,
        hook_paths: &hook_paths,
        catalog: &config.catalog,
    })?;

    // Phase 8 — prerender (SSG) for static + spa routes.
    //
    // The error pages are prerendered like any other route but must not be
    // advertised in the sitemap, so resolve which route paths `error_routes`
    // names before building the targets (R821-B2).
    let error_route_paths: Vec<&str> = routes_config
        .error_routes
        .as_ref()
        .map(|e| [e.not_found.as_deref(), e.server_error.as_deref()])
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();
    let mut targets = Vec::new();
    for r in &routes_config.routes {
        if r.mode == RouteMode::Ssr {
            continue;
        }
        let bundle_path = bundle_paths
            .get(&r.route)
            .ok_or_else(|| anyhow!("route {}: no bundled entrypoint", r.route))?;
        targets.push(RenderTarget {
            entry: r,
            bundle_path,
            hydration_script: hydration.get(&r.route).map(|h| h.script.as_str()),
            is_error_route: error_route_paths.contains(&r.route.as_str()),
        });
    }
    let outcome = prerender(&ssg, &out_dir, &project_root, &staged_build_id, &targets)?;

    // Phase 8b — the build id, derived from what phases 2–8 just staged
    // (R870-B12). Everything the id names now exists; nothing written after
    // this point feeds into it.
    let build_id = match explicit_build_id {
        Some(id) => id,
        None => {
            let staged = scan_staged_tree(&out_dir)?;
            let id = derive_build_id(&staged, &serde_json::to_string(&manifest)?);
            substitute_build_id(&staged.placeholder_files, &id)?;
            manifest.build_id = id.clone();
            id
        }
    };

    // Phase 9 — manifest + tag-index emission.
    let manifest_path = out_dir.join("manifest.json");
    let tag_index_path = out_dir.join("tag-index.json");
    std::fs::create_dir_all(&out_dir)?;
    std::fs::write(&manifest_path, format!("{}\n", serde_json::to_string_pretty(&manifest)?))?;
    let tag_index = build_tag_index(&build_id, &outcome.emissions);
    std::fs::write(&tag_index_path, format!("{}\n", serde_json::to_string_pretty(&tag_index)?))?;

    // Sitemap: emitted only when the routes config names a `site_url` origin.
    // Instance-addressed (deferred) routes, `noindex` renders and error pages
    // were already filtered out when the SSG driver collected `sitemap_paths`
    // (W270 §4, R821-B2).
    let sitemap_path = match &routes_config.site_url {
        Some(site_url) => {
            let path = out_dir.join("sitemap.xml");
            std::fs::write(&path, crate::sitemap::build_sitemap(site_url, &outcome.sitemap_paths))?;
            Some(path)
        }
        None => None,
    };

    // Scratch dir holds the routes bundle only; keep it out of dist
    // consumers' way.
    let _ = std::fs::remove_dir_all(&scratch);

    Ok(BuildResult {
        build_id,
        out_dir,
        manifest_path,
        tag_index_path,
        html_paths: outcome.html_paths,
        sitemap_path,
    })
}

/// Resolve the effective out dir for a project without building (used by
/// the diff subcommand's convenience form).
pub fn dist_dir(project_root: &Path) -> PathBuf {
    project_root.join("dist")
}
