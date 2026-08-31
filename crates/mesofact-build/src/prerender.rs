//! SSG driver (R449-F1) — port of `packages/mesofact-build/src/prerender.ts`
//! with deno_core as the executor. For each static/spa route, expand its
//! prerender params, invoke render() inside the SSG isolate (the harness
//! does the track-ctx wrap, result-shape assertion, and hydration weave so
//! the emitted bytes match the Bun pipeline), and write
//! `dist/html/<key>.html`.
//!
//! @yah:ticket(R821-B2, "Sitemap omits prerendered spa routes and advertises the 404 route")
//! @yah:status(review)
//! @yah:at(2026-08-25T06:36:23Z)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R821)
//! @yah:severity(medium)
//! @yah:next("prerender.rs:105 gates sitemap collection on `r.mode == RouteMode::Static && !noindex`. A prerendered spa route emits indexable HTML at a fixed URL and is exactly as enumerable as a static one, so the gate should be `r.mode != RouteMode::Ssr` (or an explicit static|spa set), not static-only.")
//! @yah:verify("A routes file with `/` as mode spa and `/404` as mode static, plus site_url, emits a sitemap containing / and not /404")
//! @yah:next("OBSERVED on noisetable's web/landing, a two-route marketing site (/ spa, /404 static). Setting site_url produced a sitemap.xml whose ONLY entry was https://noisetable.com/404 — the error page advertised, the one page that must be indexed omitted. That consumer removed site_url and shipped with no sitemap rather than that one; see the comment in its mesofact.routes.ts naming this ticket. It should be re-added once this lands.")
//! @yah:gotcha("Tier: Cleric — the mode gate is one line, but excluding error_routes needs the route table threaded into the sitemap collection and a decision on whether noindex should be inferred for them.")
//! @yah:handoff("Sitemap gate no longer keys on route mode. prerender.rs collects a URL when the route prerendered something AND the render did not say noindex AND the route is not an error page. Deferred routes still self-exclude structurally (they expand to no params); ssr routes are never prerender targets.")
//! @yah:verify("cargo test -p mesofact-build - 32 tests across lib + pipeline + render, all pass")
//! @yah:handoff("Error routes are threaded via a new RenderTarget.is_error_route flag set in pipeline.rs (it holds routes_config.error_routes; prerender only applies the flag). Resolves both the 404 and 5xx entries, not just 404.")
//! @yah:handoff("DECIDED the open question in the gotcha: error routes are excluded from the sitemap but are NOT given an inferred noindex. The render owns its head and the build should not inject robots directives the author did not write; the sitemap is the build's own output and the only thing it gets to decide. Rationale is on RenderTarget.is_error_route in prerender.rs, including the sharper reason - the error HTML is also reachable at its literal path (dist/html/404.html, served 200), so a sitemap entry points a crawler at a textbook soft 404.")
//! @yah:handoff("Fixture head-sitemap grew from 4 routes to 6 so the pipeline test covers the two new filters: /app (spa, indexed, must be listed) and /404 (static, named by error_routes, renders no noindex, must not be listed). Test renamed to head_woven_into_shell_and_sitemap_filters_noindex_deferred_and_error_routes. It also asserts html/404.html still exists - the exclusion is from the sitemap only, not from prerendering.")
//! @yah:handoff("OUTSIDE THIS REPO, done because the ticket asked for it and the file itself said to: restored site_url in /Users/leif/ss/noisetable/web/landing/mesofact.routes.ts. That repo's external/yah is a symlink to this tree, so it consumes the fix directly. Its stale block comment explaining the revert is replaced with one recording the fix. Built it: dist/sitemap.xml now holds exactly one entry, https://noisetable.com/ - the 404 is gone and / is present. Nothing else in that repo is dirty (dist/ is gitignored); the edit is uncommitted and is that repo owner's to land.")
//! @yah:verify("End-to-end on the reporting site: built /Users/leif/ss/noisetable/web/landing and read dist/sitemap.xml - one urlset entry, https://noisetable.com/, with /404 absent. That is the ticket verify line met on the exact two-route table that reported it.")

use anyhow::{anyhow, Context, Result};
use mesofact_core::manifest::RouteMode;
use serde_json::{json, Value};
use std::path::Path;

use crate::data::{expand_prerender_params, expand_route, read_data_inputs};
use crate::js::SsgRuntime;
use crate::route_config::RouteEntry;
use crate::route_key::prerender_key;
use crate::tag_index::Emission;

pub struct PrerenderOutcome {
    pub emissions: Vec<Emission>,
    pub html_paths: Vec<String>,
    /// Root-relative paths eligible for the sitemap: prerendered emissions
    /// that did not render `noindex` and are not an error route. Deferred
    /// (instance-addressed) routes prerender nothing and so never appear here
    /// (W270 §4).
    pub sitemap_paths: Vec<String>,
}

pub struct RenderTarget<'a> {
    pub entry: &'a RouteEntry,
    /// Absolute path to the bundled server module.
    pub bundle_path: &'a Path,
    /// Hydration weave input when the route has a client bundle.
    pub hydration_script: Option<&'a str>,
    /// This route is named by `error_routes` (the 404 or 5xx page). It still
    /// prerenders — the CDN and the dev server both serve it as real HTML —
    /// but listing it in the sitemap asks a crawler to index the site's own
    /// failure page, and worse: the error HTML is ALSO reachable at its own
    /// literal path (`dist/html/404.html`, served 200), so a sitemap entry
    /// points a crawler at a textbook soft 404. Not `noindex` — the render
    /// owns its head, and the build should not inject robots directives the
    /// author did not write; the sitemap is the build's own output and the
    /// only thing it gets to decide here. The caller sets this because it
    /// holds the `error_routes` table; prerender only applies it (R821-B2).
    pub is_error_route: bool,
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
    let mut sitemap_paths = Vec::new();
    if targets.is_empty() {
        return Ok(PrerenderOutcome { emissions, html_paths, sitemap_paths });
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
            let noindex = result.get("noindex").and_then(Value::as_bool).unwrap_or(false);

            let key = prerender_key(&r.route, &url);
            let path = html_dir.join(format!("{key}.html"));
            // `key` is path-shaped (`issues/<id>`), so the emission may nest.
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, html)
                .with_context(|| format!("writing dist/html/{key}.html"))?;
            html_paths.push(format!("dist/html/{key}.html"));
            // Sitemap: everything we just prerendered, honoring the render's
            // robots directive (W270 §4). Deferred routes expand to no params
            // and so never reach here; ssr routes are never targets at all.
            //
            // Mode is deliberately NOT part of this gate. A prerendered spa
            // route emits indexable HTML at a fixed URL and is exactly as
            // enumerable as a static one — gating on `mode == Static` dropped
            // the only real page of a spa-shell marketing site while still
            // advertising its /404 (R821-B2).
            if !noindex && !target.is_error_route {
                sitemap_paths.push(url.clone());
            }
            emissions.push(Emission { url, tags });
        }
    }
    Ok(PrerenderOutcome { emissions, html_paths, sitemap_paths })
}
