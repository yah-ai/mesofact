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
//! 10. manifest + tag-index emission

use anyhow::{anyhow, bail, Context, Result};
use mesofact::manifest::{Hydration, RouteMode};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::bundle::{
    assert_no_forbidden_modules, browser_forbidden, bundle_client_entrypoints,
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

pub fn default_build_id() -> String {
    // ISO timestamp shaped like the TS pipeline's defaultBuildId():
    // 2026-05-15T17:00:00.123Z → "2026-05-15T17-00-00Z"-ish 20-char prefix.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let secs = now % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}-{:02}-{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

// Howard Hinnant's days→civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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
    let build_id = opts.build_id.unwrap_or_else(default_build_id);

    // Phase 0 — install.
    let node_modules = project_root.join("node_modules");
    let should_install = match opts.install {
        InstallMode::Always => true,
        InstallMode::Never => false,
        InstallMode::Auto => !node_modules.exists() && project_root.join("bun.lock").exists(),
    };
    if should_install {
        let report = install::install(&project_root)?;
        if !report.skipped_fresh {
            tracing::info!(
                installed = report.installed,
                linked = report.linked,
                "installed node_modules from bun.lock"
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

    // Phase 6 — static assets (R490-F4).
    let static_assets =
        crate::assets::discover_static_assets(&project_root, &out_dir, &config.public_dir)?;

    // Phase 7 — manifest assembly + validation (before any HTML lands).
    let manifest = crate::manifest_build::assemble_manifest(crate::manifest_build::AssembleInput {
        routes: &routes_config,
        build_id: &build_id,
        server_paths: &server_paths,
        inferred_sources: &inferred_sources,
        hydration: &hydration,
        static_assets,
        catalog: &config.catalog,
    })?;

    // Phase 8 — prerender (SSG) for static + spa routes.
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
        });
    }
    let outcome = prerender(&ssg, &out_dir, &project_root, &build_id, &targets)?;

    // Phase 9 — manifest + tag-index emission.
    let manifest_path = out_dir.join("manifest.json");
    let tag_index_path = out_dir.join("tag-index.json");
    std::fs::create_dir_all(&out_dir)?;
    std::fs::write(&manifest_path, format!("{}\n", serde_json::to_string_pretty(&manifest)?))?;
    let tag_index = build_tag_index(&build_id, &outcome.emissions);
    std::fs::write(&tag_index_path, format!("{}\n", serde_json::to_string_pretty(&tag_index)?))?;

    // Sitemap: emitted only when the routes config names a `site_url` origin.
    // Instance-addressed (deferred) routes and `noindex` renders were already
    // filtered out when the SSG driver collected `sitemap_paths` (W270 §4).
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
