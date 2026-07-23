//! Render-only entrypoint (W225 §3 "revalidate"; parent-camp W270 §1) —
//! render **one route of an already-built bundle** with explicit params and
//! data, with no bundler, no install, no manifest rewrite.
//!
//! This is the data-half of the build/revalidate split: `pipeline::build`
//! is source → bundle (recompilation, CI-gated); this module is
//! data → HTML against the bundle `build` already emitted. Two callers by
//! design:
//!
//! - **revalidate** — a feed/data change re-renders an enumerable route
//!   (fresh `data_inputs` read, same bundle), no `build.command`;
//! - **publish-once instances** — a parametric static route renders one
//!   instance for a param value that was *not* enumerated at build time
//!   (e.g. a share slug minted at publish time). The emitted
//!   `dist/html/<key>.html` is content-addressable by the caller.
//!
//! Everything is resolved from `dist/manifest.json` — the route table,
//! server-bundle path, hydration script, and declared `data_inputs` — so a
//! prebuilt `dist/` is the only input this needs besides the project root
//! (used solely to re-read `data_inputs` files when no explicit data is
//! given).

use anyhow::{anyhow, bail, Context, Result};
use mesofact_core::manifest::{Manifest, Route, RouteMode};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::data::{expand_route, read_data_inputs};
use crate::js::SsgRuntime;
use crate::route_key::prerender_key;

pub struct RenderOptions {
    /// Project root — only consulted to re-read declared `data_inputs`
    /// when [`RenderOptions::data`] is `None`.
    pub project_root: PathBuf,
    /// Built output dir (default `<project_root>/dist`). Must contain
    /// `manifest.json` + `server/` from a prior `pipeline::build`.
    pub out_dir: Option<PathBuf>,
    /// Declared route pattern, exactly as in `mesofact.routes.ts`
    /// (e.g. `/releases`, `/p/:id`).
    pub route: String,
    /// Values for every `:param` in the pattern; empty for literal routes.
    pub params: BTreeMap<String, String>,
    /// Explicit `req.data` map. `None` → the route's declared
    /// `data_inputs` are re-read fresh from the project root (the
    /// revalidate shape). Keys must match what the render fn reads
    /// (`req.data["<declared path>"]` for `data_inputs` consumers).
    pub data: Option<serde_json::Map<String, Value>>,
    /// Write `dist/html/<key>.html` (the build-parity location). When
    /// false the HTML is only returned.
    pub write: bool,
}

#[derive(Debug)]
pub struct RenderOutcome {
    pub html: String,
    /// Filesystem key (`prerender_key`) — `p_id__3` for `/p/:id` + `id=3`.
    pub key: String,
    /// Concrete URL the params expand to, e.g. `/p/3`.
    pub url: String,
    /// Cache tags the render emitted (same stream the tag-index consumes).
    pub tags: Vec<String>,
    /// Where the HTML landed when `write` was set.
    pub html_path: Option<PathBuf>,
}

/// One-shot form — boots a fresh [`SsgRuntime`] per call. Callers rendering
/// many instances against the same dist (a publish loop, the revalidate
/// reconciler) should boot one runtime and use [`render_route_with`].
pub fn render_route(opts: RenderOptions) -> Result<RenderOutcome> {
    let ssg = SsgRuntime::start()?;
    render_route_with(&ssg, opts)
}

pub fn render_route_with(ssg: &SsgRuntime, opts: RenderOptions) -> Result<RenderOutcome> {
    let (project_root, out_dir) = resolve_dirs(&opts.project_root, opts.out_dir.as_deref())?;
    let manifest = load_manifest(&out_dir)?;
    let route = find_route(&manifest, &opts.route)?;
    let bundle_path = resolve_bundle(&out_dir, route)?;

    let data = match opts.data {
        Some(explicit) => Some(explicit),
        None => read_declared_data(route, &project_root)?,
    };

    render_instance(
        ssg,
        &manifest.build_id,
        route,
        &bundle_path,
        &opts.params,
        data.as_ref(),
        &out_dir,
        opts.write,
    )
}

/// All-instances form — the revalidate verb. Re-expands the route's
/// `prerender` params **fresh** (a feed change may have added/removed
/// instances) and re-reads `data_inputs`, then renders and writes every
/// instance. Rejects `deferred` routes (their instances are minted at
/// publish time, not enumerable) and `ssr` routes.
pub struct RenderAllOptions {
    pub project_root: PathBuf,
    pub out_dir: Option<PathBuf>,
    pub route: String,
}

pub fn render_route_all(opts: RenderAllOptions) -> Result<Vec<RenderOutcome>> {
    let ssg = SsgRuntime::start()?;
    render_route_all_with(&ssg, opts)
}

pub fn render_route_all_with(
    ssg: &SsgRuntime,
    opts: RenderAllOptions,
) -> Result<Vec<RenderOutcome>> {
    let (project_root, out_dir) = resolve_dirs(&opts.project_root, opts.out_dir.as_deref())?;
    let manifest = load_manifest(&out_dir)?;
    let route = find_route(&manifest, &opts.route)?;
    let bundle_path = resolve_bundle(&out_dir, route)?;

    let params_list =
        crate::data::expand_prerender(&route.route, route.prerender.as_ref(), &project_root)?;
    let data = read_declared_data(route, &project_root)?;

    let mut outcomes = Vec::with_capacity(params_list.len());
    for params in &params_list {
        outcomes.push(render_instance(
            ssg,
            &manifest.build_id,
            route,
            &bundle_path,
            params,
            data.as_ref(),
            &out_dir,
            true,
        )?);
    }
    Ok(outcomes)
}

fn resolve_dirs(project_root: &Path, out_dir: Option<&Path>) -> Result<(PathBuf, PathBuf)> {
    let project_root = project_root
        .canonicalize()
        .with_context(|| format!("project root {}", project_root.display()))?;
    let out_dir = match out_dir {
        Some(d) => d.canonicalize().with_context(|| format!("out dir {}", d.display()))?,
        None => project_root.join("dist"),
    };
    Ok((project_root, out_dir))
}

fn load_manifest(out_dir: &Path) -> Result<Manifest> {
    let manifest_path = out_dir.join("manifest.json");
    serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {} — run `mesofact-build build` first", manifest_path.display()))?,
    )
    .with_context(|| format!("parsing {}", manifest_path.display()))
}

fn find_route<'m>(manifest: &'m Manifest, route: &str) -> Result<&'m Route> {
    let found = manifest.routes.iter().find(|r| r.route == route).ok_or_else(|| {
        let have: Vec<&str> = manifest.routes.iter().map(|r| r.route.as_str()).collect();
        anyhow!("route {} not in manifest (routes: {})", route, have.join(", "))
    })?;
    if found.mode == RouteMode::Ssr {
        bail!(
            "route {}: mode:\"ssr\" renders per-request in the SSR host; the render verb covers static/spa routes only",
            found.route
        );
    }
    Ok(found)
}

fn read_declared_data(
    route: &Route,
    project_root: &Path,
) -> Result<Option<serde_json::Map<String, Value>>> {
    match &route.data_inputs {
        Some(inputs) if !inputs.is_empty() => Ok(Some(read_data_inputs(inputs, project_root)?)),
        _ => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_instance(
    ssg: &SsgRuntime,
    build_id: &str,
    route: &Route,
    bundle_path: &Path,
    params: &BTreeMap<String, String>,
    data: Option<&serde_json::Map<String, Value>>,
    out_dir: &Path,
    write: bool,
) -> Result<RenderOutcome> {
    let url = expand_route(&route.route, params)?;
    let mut req = json!({
        "url": url,
        "params": params,
        "query": {},
        "headers": {},
        "cookies": {},
    });
    if let Some(data) = data {
        req["data"] = Value::Object(data.clone());
    }
    let mut input = json!({ "route": route.route, "url": url, "req": req });
    if let Some(h) = &route.hydration {
        input["hydration"] = json!({ "buildId": build_id, "script": h.script });
    }

    let result = ssg
        .render(bundle_path, input)
        .with_context(|| format!("route {} ({url}): render failed", route.route))?;
    let html = result
        .get("html")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("route {}: render returned no html", route.route))?
        .to_string();
    let tags: Vec<String> = result
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();

    let key = prerender_key(&route.route, params);
    let html_path = if write {
        let html_dir = out_dir.join("html");
        std::fs::create_dir_all(&html_dir)?;
        let path = html_dir.join(format!("{key}.html"));
        std::fs::write(&path, &html).with_context(|| format!("writing {}", path.display()))?;
        Some(path)
    } else {
        None
    };

    Ok(RenderOutcome { html, key, url, tags, html_path })
}

/// `render_entrypoint` is emitted as `dist/server/<key>.js` — relative to
/// the *project root* with the conventional `dist/` first segment. Resolve
/// it against the actual out dir by stripping that first segment (the same
/// convention mesofact-dev's SSR loader uses; see its lib.rs cleanup note
/// about non-`dist` out_dir overrides).
fn resolve_bundle(out_dir: &Path, route: &Route) -> Result<PathBuf> {
    let rel = route
        .render_entrypoint
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or(&route.render_entrypoint);
    let path = out_dir.join(rel);
    if !path.exists() {
        bail!(
            "route {}: server bundle {} missing — the dist is incomplete; run `mesofact-build build`",
            route.route,
            path.display()
        );
    }
    Ok(path)
}
