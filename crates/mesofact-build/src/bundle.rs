//! Rolldown bundling (W174 pillar 2 / R448). Mirrors
//! `packages/mesofact-build/src/bundle.ts`:
//!
//! - server tree: one bundle per route entrypoint → `dist/server/<key>.js`,
//!   ESM, no splitting, `@mesofact/runtime` external.
//! - client tree: one bundle per client entrypoint → `dist/hydrate/
//!   <key>.<hash>.js` (+ `<key>.chunk-<hash>.js` shared chunks), browser
//!   platform, content-hashed.
//!
//! Divergence from the Bun pipeline, by design: server bundles resolve with
//! browser-ish conditions (Platform::Browser) instead of Bun's node-flavored
//! target, because the SSG/SSR executor is deno_core (workerd-semantics V8)
//! — react-dom must resolve to `server.browser` rather than the
//! node-streams build. The Fetch-handler contract is identical either way
//! (W173 § "Entrypoint signatures"); the W174 amendment records this.

use anyhow::{anyhow, bail, Result};
use rolldown::{
    Bundler, BundlerOptions, ChunkFilenamesOutputOption, InputItem, IsExternal, OutputFormat,
    Platform,
};
use rolldown_common::Output;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::route_key::route_key;

pub struct ServerBundle {
    pub route: String,
    /// Manifest `render_entrypoint` value (`dist/server/<key>.js`).
    pub server_path: String,
    /// Absolute path for the SSG executor to import.
    pub absolute_path: PathBuf,
    /// Module ids that ended up in the bundle (for the host-lint pass).
    pub module_ids: Vec<String>,
    /// Chunk-level `imports` (cross-chunk filenames + external module
    /// specifiers, e.g. `node:fs`) for the host-lint pass. Rolldown
    /// externalizes `node:*`/bare builtins rather than bundling them, and
    /// externalized ids are NOT included in `module_ids` — this is where
    /// they show up instead (R513-B11). See `assert_no_forbidden_modules`.
    pub import_ids: Vec<String>,
}

pub struct ClientBundle {
    pub route: String,
    /// Content-hashed entry filename relative to `dist/hydrate/`.
    pub script: String,
    /// Code-split chunk filenames (sorted), also under `dist/hydrate/`.
    pub code_split: Vec<String>,
    pub module_ids: Vec<String>,
    /// See `ServerBundle::import_ids` — same rationale, aggregated across
    /// every chunk this client bundle emitted (entry + code-split).
    pub import_ids: Vec<String>,
}

fn base_options(project_root: &Path) -> BundlerOptions {
    BundlerOptions {
        cwd: Some(project_root.to_path_buf()),
        format: Some(OutputFormat::Esm),
        // Renders execute with production React; the Bun pipeline relies on
        // Bun's own NODE_ENV inlining for browser targets — pin it here so
        // both pipelines drop the dev branches.
        define: Some(
            [(
                "process.env.NODE_ENV".to_string(),
                "\"production\"".to_string(),
            )]
            .into_iter()
            .collect(),
        ),
        ..Default::default()
    }
}

async fn run_bundler(options: BundlerOptions, what: &str) -> Result<Vec<Output>> {
    let mut bundler =
        Bundler::new(options).map_err(|e| anyhow!("{what}: bundler init failed: {e:?}"))?;
    let result = bundler.write().await;
    let output = match result {
        Ok(o) => o,
        Err(errs) => bail!("{what}: bundle failed: {errs:?}"),
    };
    bundler.close().await.map_err(|e| anyhow!("{what}: bundler close failed: {e:?}"))?;
    Ok(output.assets)
}

/// Bundle each route's server entrypoint to `dist/server/<key>.js`. One
/// bundler invocation per route — deterministic names, no cross-route chunk
/// sharing (parity with the Bun pipeline's per-route `Bun.build` calls).
pub async fn bundle_server_entrypoints(
    project_root: &Path,
    out_dir: &Path,
    inputs: &[(String, String)], // (route, entrypoint)
) -> Result<Vec<ServerBundle>> {
    let server_dir = out_dir.join("server");
    std::fs::create_dir_all(&server_dir)?;

    let mut outputs = Vec::new();
    for (route, entrypoint) in inputs {
        let key = route_key(route);
        let abs_entry = project_root.join(entrypoint);
        let options = BundlerOptions {
            input: Some(vec![InputItem {
                name: Some(key.clone()),
                import: abs_entry.to_string_lossy().into_owned(),
            }]),
            dir: Some(server_dir.to_string_lossy().into_owned()),
            platform: Some(Platform::Browser),
            external: Some(IsExternal::from(vec!["@mesofact/runtime".to_string()])),
            entry_filenames: Some(ChunkFilenamesOutputOption::String("[name].js".to_string())),
            ..base_options(project_root)
        };
        let assets = run_bundler(options, &format!("route {route}")).await?;
        let entry = assets
            .iter()
            .find_map(|a| match a {
                Output::Chunk(c) if c.is_entry => Some(c),
                _ => None,
            })
            .ok_or_else(|| anyhow!("bundle for route {route} produced no entry-point output"))?;
        outputs.push(ServerBundle {
            route: route.clone(),
            server_path: format!("dist/server/{key}.js"),
            absolute_path: server_dir.join(entry.filename.as_str()),
            module_ids: entry.module_ids.iter().map(|m| m.to_string()).collect(),
            import_ids: entry.imports.iter().map(|m| m.to_string()).collect(),
        });
    }
    Ok(outputs)
}

/// Bundle each client entrypoint to `dist/hydrate/<key>.<hash>.js` with
/// code splitting. `@mesofact/runtime` is NOT external here (parity with
/// bundle.ts — client code must not touch the server adapter registry).
pub async fn bundle_client_entrypoints(
    project_root: &Path,
    out_dir: &Path,
    inputs: &[(String, String)], // (route, client_entrypoint)
) -> Result<Vec<ClientBundle>> {
    let hydrate_dir = out_dir.join("hydrate");
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(&hydrate_dir)?;

    let mut outputs = Vec::new();
    for (route, client_entrypoint) in inputs {
        let key = route_key(route);
        let abs_entry = project_root.join(client_entrypoint);
        let options = BundlerOptions {
            input: Some(vec![InputItem {
                name: Some(key.clone()),
                import: abs_entry.to_string_lossy().into_owned(),
            }]),
            dir: Some(hydrate_dir.to_string_lossy().into_owned()),
            platform: Some(Platform::Browser),
            entry_filenames: Some(ChunkFilenamesOutputOption::String(format!("{key}.[hash].js"))),
            chunk_filenames: Some(ChunkFilenamesOutputOption::String(format!("{key}.chunk-[hash].js"))),
            ..base_options(project_root)
        };
        let assets = run_bundler(options, &format!("client bundle for route {route}")).await?;
        let mut script = None;
        let mut code_split = Vec::new();
        let mut module_ids = Vec::new();
        let mut import_ids = Vec::new();
        for a in &assets {
            if let Output::Chunk(c) = a {
                module_ids.extend(c.module_ids.iter().map(|m| m.to_string()));
                import_ids.extend(c.imports.iter().map(|m| m.to_string()));
                if c.is_entry {
                    script = Some(c.filename.to_string());
                } else {
                    code_split.push(c.filename.to_string());
                }
            }
        }
        code_split.sort();
        let script = script
            .ok_or_else(|| anyhow!("client bundle for route {route} produced no entry-point output"))?;
        outputs.push(ClientBundle { route: route.clone(), script, code_split, module_ids, import_ids });
    }
    Ok(outputs)
}

/// Bundle the routes file itself (`mesofact.routes.ts`) for evaluation in
/// the SSG runtime. `@mesofact/runtime` stays external (the executor maps it
/// to the embedded shim); everything else the file imports is bundled in.
pub async fn bundle_routes_file(project_root: &Path, scratch_dir: &Path) -> Result<PathBuf> {
    let routes_file = project_root.join("mesofact.routes.ts");
    if !routes_file.exists() {
        bail!("expected {} to exist", routes_file.display());
    }
    std::fs::create_dir_all(scratch_dir)?;
    let options = BundlerOptions {
        input: Some(vec![InputItem {
            name: Some("routes".to_string()),
            import: routes_file.to_string_lossy().into_owned(),
        }]),
        dir: Some(scratch_dir.to_string_lossy().into_owned()),
        platform: Some(Platform::Neutral),
        external: Some(IsExternal::from(vec!["@mesofact/runtime".to_string()])),
        entry_filenames: Some(ChunkFilenamesOutputOption::String("[name].mjs".to_string())),
        ..base_options(project_root)
    };
    run_bundler(options, "mesofact.routes.ts").await?;
    Ok(scratch_dir.join("routes.mjs"))
}

/// Map of forbidden import → reason, mirroring host-lint.ts's
/// BROWSER_FORBIDDEN / EDGE_FORBIDDEN sets.
pub fn browser_forbidden(module_id: &str) -> bool {
    const BARE_BUILTINS: &[&str] = &[
        "fs", "path", "net", "os", "child_process", "worker_threads", "cluster", "dgram",
        "dns", "tls", "http", "https", "http2", "stream", "zlib", "crypto", "vm",
    ];
    module_id.starts_with("node:")
        || BARE_BUILTINS.contains(&module_id)
        || module_id.starts_with("bun:")
}

pub fn edge_forbidden(module_id: &str) -> bool {
    const DB_DRIVERS: &[&str] = &["pg", "mysql2", "mongodb", "redis", "ioredis", "better-sqlite3"];
    browser_forbidden(module_id)
        || DB_DRIVERS.iter().any(|d| module_id == *d || module_id.starts_with(&format!("{d}/")))
}

/// Post-bundle lint over the captured module graph. The TS pipeline walks
/// the graph with an onResolve plugin pre-bundle; here the bundle already
/// ran (rolldown reports unresolved forbidden ids as bundle errors, which
/// surface first with the same actionable specifier), and resolved-but-
/// forbidden ids are caught here.
///
/// Two sources are scanned, not one (R513-B11): `module_ids` covers
/// resolved-and-bundled files, but rolldown externalizes `node:*` and other
/// bare builtins instead of bundling them — an externalized
/// `import "node:fs"` never appears in `module_ids`, only in the chunk's
/// `imports` list (`import_ids` here). A hydrate bundle that ships a
/// side-effectful `node:*` import used to sail through this lint untouched.
pub fn assert_no_forbidden_modules(
    route: &str,
    kind: &str,
    module_ids: &[String],
    import_ids: &[String],
    forbidden: fn(&str) -> bool,
) -> Result<()> {
    // module ids are absolute paths for resolved files; bare/builtin ids
    // stay as written (external or shimmed). import_ids mixes cross-chunk
    // filenames (never forbidden) with external specifiers (may be).
    let offenders: BTreeMap<&str, ()> = module_ids
        .iter()
        .chain(import_ids.iter())
        .filter(|id| forbidden(id))
        .map(|id| (id.as_str(), ()))
        .collect();
    if offenders.is_empty() {
        return Ok(());
    }
    bail!(
        "route {route}: {kind} reaches host-only module(s) {:?} (W173 server/client boundary lint)",
        offenders.keys().collect::<Vec<_>>()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allows_clean_module_and_import_graph() {
        let module_ids = strings(&["/proj/src/app.tsx", "/proj/src/host/browser.ts"]);
        let import_ids = strings(&["route.chunk-abcdef.js"]);
        assert_no_forbidden_modules("/", "client_entrypoint", &module_ids, &import_ids, browser_forbidden)
            .expect("clean graph must pass");
    }

    #[test]
    fn catches_forbidden_resolved_module_id() {
        // Pre-existing behavior: a forbidden id that rolldown actually
        // resolved and bundled shows up in `module_ids`.
        let module_ids = strings(&["/proj/src/app.tsx", "pg"]);
        let import_ids = strings(&[]);
        let err = assert_no_forbidden_modules("/", "client_entrypoint", &module_ids, &import_ids, edge_forbidden)
            .expect_err("bare `pg` import must be caught");
        assert!(err.to_string().contains("pg"));
    }

    #[test]
    fn catches_forbidden_id_reachable_only_via_externalized_import() {
        // R513-B11 regression: rolldown externalizes `node:*` instead of
        // bundling it, so the offending id never lands in `module_ids` — it
        // only shows up in the chunk's `imports` (our `import_ids`). Before
        // this fix, a hydrate bundle that side-effect-imported `node:fs`
        // (transitively, via the `@mesofact/runtime` barrel) sailed through
        // this lint because `module_ids` alone was clean.
        let module_ids = strings(&["/proj/src/host/browser.ts"]);
        let import_ids = strings(&["route.chunk-abcdef.js", "node:fs", "node:async_hooks"]);
        let err =
            assert_no_forbidden_modules("/", "client_entrypoint", &module_ids, &import_ids, browser_forbidden)
                .expect_err("externalized node:* imports must be caught even when module_ids is clean");
        let msg = err.to_string();
        assert!(msg.contains("node:fs"), "expected node:fs in error, got: {msg}");
        assert!(msg.contains("node:async_hooks"), "expected node:async_hooks in error, got: {msg}");
    }

    #[test]
    fn edge_forbidden_also_scans_import_ids() {
        let module_ids = strings(&[]);
        let import_ids = strings(&["better-sqlite3"]);
        let err = assert_no_forbidden_modules(
            "/edge",
            "ssr placement:\"edge\" entrypoint",
            &module_ids,
            &import_ids,
            edge_forbidden,
        )
        .expect_err("db driver reachable only via import_ids must be caught");
        assert!(err.to_string().contains("better-sqlite3"));
    }
}
