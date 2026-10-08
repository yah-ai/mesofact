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
//!
//! @yah:relay(R600, "Prerender instance keys: renderer emits a flat name the edge router never resolves")
//! @yah:at(2026-08-12T17:43:59Z)
//!
//! @yah:ticket(R600-B1, "Parametric prerender instances publish under a flat key the edge router never looks up - every /route/:param page 404s")
//! @yah:status(review)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:at(2026-08-13T19:17:03Z)
//! @yah:parent(R600)
//! @yah:severity(high)
//! @yah:gotcha("MEASURED LIVE on yah.dev 2026-08-12 (yah camp R752-B6 / R330-F13), not inferred. The render and publish legs are both CORRECT and complete; only the lookup name disagrees. Receiver log: revalidate complete route=Some(\"/issues/:id\") rendered=[\"/issues/:id\"] instances=14 uploaded=3. The bytes are served: GET https://cdn.yah.dev/yah-marketing/cloud/<build>/html/issues_id__01KZVGVT0DV61ZGGNVHAWQW2CS.html -> 200. But GET https://yah.dev/issues/01KZVGVT0DV61ZGGNVHAWQW2CS -> 404.")
//! @yah:gotcha("THE MISMATCH. The renderer names each emission off the ROUTE PATTERN: prerender_key(\"/issues/:id\", {id}) = route_key + \"__\" + params = issues_id__<id>, written to dist/html/issues_id__<id>.html (crates/mesofact-render/src/route_key.rs). The edge router resolves off the REQUEST PATH: assetCandidates(\"issues/<id>\") tries issues/<id>.html then issues/<id>/index.html (packages/mesofact-edge/src/router.ts:211). Neither is ever written. Bucket listing confirms: 14 issues_id__<ulid>.html objects, zero issues/<ulid>.html.")
//! @yah:gotcha("NOT the pointer-store path, which is what everyone checks first. router.ts:240 routes through pointers only for prerender: { deferred: true }; a {from_data, items_key, param} block has no deferred flag, so matchesDeferredRoute is false and it takes the plain-asset branch. The p/ prefix is empty, consistent with that. So this is not 'instances are pointer-published and the pointer is missing' - there is no pointer involved at all.")
//! @yah:next("RECOMMENDED SIDE: move the WRITE, not the lookup. Emit parametric instances at the path-shaped location (dist/html/issues/<id>.html) so the generic publisher - which is a plain directory walker, publish.rs:128 is literally format!(\"{build_id}/{rel_str}\") and has no route knowledge - carries them to a key the edge already resolves. Teaching the router the entrypoint-derived name instead means a THIRD copy of the route_key rule (Rust render, TS build, TS worker) plus param extraction at the edge; route_key.rs's own header already warns the two existing copies must stay identical. Note the flat key and the path agree for every NON-parametric route (/releases -> releases.html), so path-shaped was always the implied contract and only parametric routes ever diverged.")
//! @yah:next("Blast radius to check before moving: prerender_key (crates/mesofact-render/src/route_key.rs) and its TS twin packages/mesofact-build/src/route-key.ts + prerender.ts must move together or the two pipelines emit different names - the header calls that out. dist/server/<key>.js is a SEPARATE concern and should NOT move; only the html emission name is wrong. Existing published builds keep the old keys, so this is fixed-forward on the next publish, not a migration.")
//! @yah:verify("Regression that would have caught this: build a fixture with a {from_data, items_key, param} prerender route, then assert the emitted dist/html key is what routeToAssetKeys / assetCandidates derives from the instance's PUBLIC path. The existing coverage only asserts the file is emitted, which is why a local build proof could not catch it - the mismatch exists only on the serve side.")
//! @yah:next("Tier: Wizard - the fix is small but it is a render/serve contract change across three packages with a naming rule that already has two copies; picking the wrong side adds a third.")
//! @yah:handoff("FIXED, moved the WRITE as recommended. prerender_key / prerenderKey no longer flatten the route pattern; each emission is named by the PUBLIC PATH it serves at, minus the leading slash. /issues/:id + id=abc now emits dist/html/issues/abc.html, exactly what assetCandidates(url.pathname.slice(1)) asks the origin for. route_key is untouched: dist/server/<key>.js and dist/hydrate/<key>.<hash>.js stay pattern-flattened, correct since nobody addresses them by URL.")
//! @yah:handoff("Signature moved on both twins from (route, params) to (route, url). Both call sites already had the expanded url in hand, so this drops a re-expansion and its error-swallow. Files: crates/mesofact-render/src/route_key.rs (rule + header explaining WHY there are two names), crates/mesofact-render/src/render.rs:246 (the revalidate / publish-once leg), crates/mesofact-build/src/prerender.rs:93 (Rust build leg), packages/mesofact-build/src/route-key.ts + prerender.ts:146 (Bun build leg). Both write legs now mkdir the parent, since a key can nest.")
//! @yah:handoff("DISCOVERED + FIXED 1 - nested literal routes were broken the same way, not just parametric ones. route_key flattens every separator, so /blog/nested emitted blog_nested.html while all three resolvers ask for blog/nested.html. The ticket's premise that flat and path agree for every non-parametric route holds only for SINGLE-SEGMENT routes. Same one-line fix covers it; pinned by a case in prerender_key_is_the_public_path.")
//! @yah:handoff("DISCOVERED + FIXED 2 - path-shaping made mesofact serve lose the LIST page, and my own new test caught it. yah.dev has both /issues (list) and /issues/:id, so dist/html now holds issues.html next to an issues/ directory. serve_static resolved a directory to <dir>/index.html BEFORE trying <key>.html, so GET /issues 404'd. Reordered crates/mesofact/src/server.rs:1052 to the edge's own candidate order - literal, then <key>.html, then <key>/index.html - which also matches serve_error_page one screen down. The edge worker already had the right order; the dev server was the odd one out.")
//! @yah:handoff("Bonus - this closes the gap R443-B4 parked as out-of-scope in the same file (GET /issues/42 still 404 in dev, would need route-schema knowledge in mesofact-dev). Moving the write means no resolver needs route knowledge at all: instances land where the plain clean-URL rule already looks. New regression serves_parametric_instance_at_its_public_path in server.rs asserts both halves.")
//! @yah:verify("LIVE PROOF on the real yah.dev workload, not a fixture. Built app/yah/web/marketing into a scratch out-dir, then ran the render verb with one seeded issue: `mesofact-build render <marketing> --route /issues/:id --param id=01KZVGVT0DV61ZGGNVHAWQW2CS --data <seed>` -> key: issues/01KZVGVT0DV61ZGGNVHAWQW2CS, written to html/issues/01KZVGVT0DV61ZGGNVHAWQW2CS.html. Served that dist with ./target/debug/mesofact serve --port 4611: GET /issues/01KZVGVT0DV61ZGGNVHAWQW2CS -> 200 with the rendered body, GET /issues -> 200 (list unshadowed), /releases 200, / 200, /nonsense 404.")
//! @yah:verify("The regression you asked for, split across the two packages that own each half - neither alone can catch a write/lookup disagreement, so both are needed. WRITE half: packages/mesofact-build/tests/build.test.ts on the existing {from_data, items_key, param} fixture now asserts html/items/a.html + html/items/b.html and, generically, that every emission's htmlPath equals dist/html<url>.html. SERVE half: new describe in packages/mesofact-edge/tests/router.test.ts drives Miniflare against a build tree holding <build>/html/issues/<ulid>.html and asserts GET /issues/<ulid> -> 200, GET /issues -> 200, GET /issues/nope -> branded 404. That last case is the negative control: under the old naming the only published object is issues_id__<ulid>.html, which is exactly the no-instance-at-this-path case.")
//! @yah:verify("Suites: cargo test -p mesofact-render -p mesofact-build -p mesofact -> 53 + 21 + 6 + 7 + 7 pass, 0 fail. bun test packages/mesofact-build -> 46 pass. bun test packages/mesofact-edge -> 43 pass (bundle rebuilt first). bun test packages/mesofact-runtime -> 99 pass. Typecheck clean on mesofact-edge. cargo clippy on the touched crates surfaces nothing new.")
//! @yah:gotcha("cargo test --workspace is RED right now and it is NOT this change. @Miravel:polaris is mid-flight on R444 in the same tree: mesofact-ssr's SsrRuntime::start took an env arg (crates/mesofact-ssr/src/ssr.rs:145) and the call site in crates/mesofact/src/ssr.rs:418 is still 0-arg, plus an unresolved R2SourceCoords import. Feature unification turns the ssr feature on for the whole workspace, so --workspace cannot compile until they land. Per-crate runs are green. Verify per-crate until R444 settles.")
//! @yah:cleanup("Stale flat emissions on the live receiver's persistent dist. The revalidate receiver renders into the workload's existing dist/ and the publisher walks the whole tree, so the 14 issues_id__<ulid>.html files already on that node will keep riding into every new build tree as dead weight. Not breakage - the correct issues/<ulid>.html is published alongside and wins - so no migration needed. Clear it whenever convenient with a fresh build of the workload, or rm dist/html/*__*.html on the node.")

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
    /// Filesystem key (`prerender_key`) — the public path minus its leading
    /// slash, so `p/3` for `/p/:id` + `id=3`. Emitted at
    /// `dist/html/<key>.html`, which is exactly what a request for `/p/3`
    /// resolves to at the edge and in `mesofact serve`.
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

    let key = prerender_key(&route.route, &url);
    let html_path = if write {
        let path = out_dir.join("html").join(format!("{key}.html"));
        // `key` is path-shaped (`issues/<id>`), so the emission may be nested.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
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
/// convention `mesofact-dev`'s watcher uses; see `WatchOptions::defaults_for_workload`
/// in `crates/mesofact-dev/src/watcher.rs` for the `build.out_dir` override).
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
