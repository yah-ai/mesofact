//! SSG driver (R449-F1) — port of `packages/mesofact-build/src/prerender.ts`
//! with deno_core as the executor. For each static/spa route, expand its
//! prerender params, invoke render() inside the SSG isolate (the harness
//! does the track-ctx wrap, result-shape assertion, and hydration weave so
//! the emitted bytes match the Bun pipeline), and write
//! `dist/html/<key>.html`.

use anyhow::{anyhow, Context, Result};
use mesofact::manifest::RouteMode;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

use crate::data::{expand_prerender_params, expand_route, read_data_inputs};
use crate::js::SsgRuntime;
use crate::route_config::RouteEntry;
use crate::route_key::prerender_key;
use crate::tag_index::Emission;

pub struct PrerenderOutcome {
    pub emissions: Vec<Emission>,
    pub html_paths: Vec<String>,
    /// Root-relative paths eligible for the sitemap: enumerable static-route
    /// emissions that did not render `noindex`. Deferred (instance-addressed)
    /// routes prerender nothing and so never appear here (W270 §4).
    pub sitemap_paths: Vec<String>,
}

pub struct RenderTarget<'a> {
    pub entry: &'a RouteEntry,
    /// Absolute path to the bundled server module.
    pub bundle_path: &'a Path,
    /// Hydration weave input when the route has a client bundle.
    pub hydration_script: Option<&'a str>,
}

pub fn prerender(
    ssg: &SsgRuntime,
    out_dir: &Path,
    project_root: &Path,
    build_id: &str,
    targets: &[RenderTarget<'_>],
) -> Result<PrerenderOutcome> {
    let html_dir = out_dir.join("html");
    let mut emissions = Vec::new();
    let mut html_paths = Vec::new();
    if targets.is_empty() {
        return Ok(PrerenderOutcome { emissions, html_paths });
    }
    std::fs::create_dir_all(&html_dir)?;

    for target in targets {
        let r = target.entry;
        debug_assert!(r.mode != RouteMode::Ssr, "ssr routes are never prerendered");
        let params_list = expand_prerender_params(r, project_root)?;

        let data = match &r.data_inputs {
            Some(inputs) if !inputs.is_empty() => Some(read_data_inputs(inputs, project_root)?),
            _ => None,
        };

        for params in &params_list {
            let url = expand_route(&r.route, params)?;
            let mut req = json!({
                "url": url,
                "params": params,
                "query": {},
                "headers": {},
                "cookies": {},
            });
            if let Some(data) = &data {
                req["data"] = Value::Object(data.clone());
            }
            let mut input = json!({ "route": r.route, "url": url, "req": req });
            if let Some(script) = target.hydration_script {
                input["hydration"] = json!({ "buildId": build_id, "script": script });
            }

            let result = ssg
                .render(target.bundle_path, input)
                .with_context(|| format!("route {} ({url}): prerender failed", r.route))?;
            let html = result
                .get("html")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("route {}: SSG returned no html", r.route))?;
            let tags: Vec<String> = result
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                .unwrap_or_default();

            let key = prerender_key(&r.route, &to_btree(params));
            std::fs::write(html_dir.join(format!("{key}.html")), html)
                .with_context(|| format!("writing dist/html/{key}.html"))?;
            html_paths.push(format!("dist/html/{key}.html"));
            emissions.push(Emission { url, tags });
        }
    }
    Ok(PrerenderOutcome { emissions, html_paths })
}

fn to_btree(params: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    params.clone()
}
