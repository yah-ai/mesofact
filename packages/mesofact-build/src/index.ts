//! @yah:relay(R016, "Cell 2 (Islands) build support — static + client_entrypoint")
//! @yah:at(2026-06-05T00:52:24Z)
//! @yah:status(open)
//! @yah:next("W173 Cell 2 = mode:'static' + client_entrypoint. Current build rejects this combination; this relay lifts the limit. R015-F3 shipped the analogous wiring for Cell 4 (ssr+client_entrypoint); Cell 2 is the same machinery on the static prerender path.")
//! @yah:next("Single feature unit — see child F-ticket for the concrete edits. No ordering with other relays.")
//! @yah:next("Real consumer: yah-camp R443-F2 is blocked on this (issues tracker on yah.dev marketing site).")
//! @yah:gotcha("W173 § 'Coverage matrix' lists Cell 2 as `(none)` with 'Cheap candidate if a real need shows up'. R443 is that need.")
//! @yah:assumes("The spa-mode hydration weave (__MESOFACT_STATE__ JSON tag + module script before </body>) is the right shape for Cell 2. If hydration semantics need to differ (e.g. static skips initial_state because the HTML already pre-rendered everything), that's an implementation-time design call.")
//! @arch:see(.yah/docs/working/W173-mesofact-render-cube.md)
//!
//! @yah:relay(R017, "Parametric prerender enumeration from local data_inputs")
//! @yah:at(2026-06-05T00:52:31Z)
//! @yah:status(open)
//! @yah:next("Parametric routes today enumerate IDs either via literal `prerender:{params:[...]}` or via a registered R2-shaped source adapter (BlobSource). There's no path to enumerate from a local-JSON data_inputs file at build time. This relay adds a third shape.")
//! @yah:next("Single feature unit — see child F-ticket. Independent from the Cell 2 relay; either can ship first.")
//! @yah:next("Real consumer: yah-camp R443-F2's /issues/:id wants one static HTML per issue, enumerated from src/data/issues.json (the same file feeding data_inputs).")
//! @yah:gotcha("Don't confuse with the existing `from`/query/param shape — that one walks a registered source adapter (R2 BlobSource) via async load. from_data is intentionally synchronous + local-file-only, riding the data_inputs read that already happens at prerender.ts:114-118.")
//! @yah:assumes("Naming the new field `from_data` (rather than reusing `from`) is the right disambiguation — `from` already means 'registered async source adapter', from_data means 'local JSON file already declared in data_inputs'. Verify with one consumer before locking the name.")
//! @arch:see(.yah/docs/working/W173-mesofact-render-cube.md)
//!
//! @yah:ticket(R016-F1, "Relax static + client_entrypoint validator + weave hydrate into prerendered static HTML")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-05T00:52:43Z)
//! @yah:status(review)
//! @yah:parent(R016)
//! @yah:next("Validator at packages/mesofact-build/src/index.ts:126-129 currently throws `client_entrypoint is only valid for mode 'spa' or 'ssr'`. Relax to allow `mode:'static'` + `client_entrypoint`. Add a third arm mirroring the ssr arm at index.ts:124-125: `else if (r.mode === 'static' && r.client_entrypoint) { clientInputs.push({ route: r.route, clientEntrypoint: r.client_entrypoint }); }`.")
//! @yah:next("Extend the prerender driver at packages/mesofact-build/src/index.ts:186-189 — currently only `r.mode === 'spa'` sets `input.hydration` on the PrerenderInput. Add the same wiring for static+client_entrypoint routes (same buildId + script source from `hydration.get(r.route)`).")
//! @yah:next("The existing injectHydration() at packages/mesofact-build/src/prerender.ts:188-208 already does the right thing (inlines __MESOFACT_STATE__ JSON tag + module script before </body>). No change needed to the weave itself once the input is plumbed.")
//! @yah:next("Add a fixture at packages/mesofact-build/tests/fixtures/static-islands/: a mode:'static' route with client_entrypoint + data_inputs. Tests assert (a) build emits HTML with data-driven static markup, (b) HTML contains the hydrate script tag, (c) client bundle ships in dist/hydrate/.")
//! @yah:verify("bun test packages/mesofact-build — new Cell 2 fixture passes + 35 baseline tests still green")
//! @yah:verify("bun test packages/mesofact-runtime — 66 baseline still passes")
//! @yah:verify("Regression: cd app/yah/web/marketing && bun run build clean (existing routes unbroken)")
//! @yah:verify("End-to-end: yah-camp R443-F2 builds /issues route as mode:'static' + client_entrypoint + data_inputs and emits prerendered list + form + hydrate")
//! @yah:handoff("Cell 2 (Islands) build support shipped. (1) packages/mesofact-build/src/index.ts: dropped the 'static route declaring client_entrypoint is a config error' arm of the client-tree validator and added a third clientInputs arm for 'static + client_entrypoint' that mirrors the ssr-Universal arm; updated the client-tree + prerender doc-comments to cite W173 Islands. (2) Simplified the prerender hydration wiring — was 'if (r.mode === \"spa\") { const h = hydration.get(r.route); if (h) ... }', now an unconditional 'const h = hydration.get(r.route); if (h) input.hydration = { buildId, script: h.script }'. The hydration map only has entries for routes whose client_entrypoint was bundled, so the map-presence check is the right invariant ('weave hydrate when there's a hydrate bundle for this route, regardless of mode'). SSR routes are 'continue'd above the loop, so they can't false-positive. (3) New fixture tests/fixtures/static-islands/ — mode:'static' + client_entrypoint:'src/shell.client.ts' + data_inputs:['data/items.json']. shell.ts bakes a data-driven list (li per item) into static HTML and returns hydration.initial_state with the item count; shell.client.ts reads the __MESOFACT_STATE__ tag and inserts a small count badge. (4) New describe block in build.test.ts asserts: (a) data-driven static markup (Issue 1 / Issue 2 li tags), (b) hydration weave (__MESOFACT_STATE__ tag + module script tag before </body>), (c) hashed client bundle landed in dist/hydrate/<routeKey>.<hash>.js, plus manifest carries data_inputs + hydration block. Verify: mesofact-build 40 pass (35 baseline + 4 from concurrent R017-F1 + 1 new); mesofact-runtime 66 pass; mesofact-build typecheck clean; yah-marketing build clean (5 HTML files). Note on assumption from the relay: stuck with the spa-mode hydration weave verbatim — __MESOFACT_STATE__ JSON tag + module entry script before </body>, with injection auto-skipping the state tag when render() omits hydration.initial_state. Cell 2 routes that pre-render fully and want no state handoff can omit initial_state and still get the entry script (existing injectHydration() at packages/mesofact-build/src/prerender.ts:190-208 handles that case unchanged). Scope notes: did NOT add BROWSER_FORBIDDEN linting for static+client_entrypoint at the host-lint pass (index.ts:130-148). Cell 4 (ssr+client_entrypoint) doesn't get its client_entrypoint browser-linted either, so this preserves parity — both gaps belong in a follow-up. Concurrency note: R017-F1 was actively editing the same file (added a projectRoot arg to expandPrerenderParams + new prerender-from-data fixture/test); both sets of changes coexist without conflict.")
//! @yah:next("operator to verify end-to-end against yah-camp R443-F2 (issues tracker on yah.dev) and sign off, or flag the deferred browser-lint gap as a follow-up ticket.")
//! @yah:cleanup("BROWSER_FORBIDDEN host-lint should fire on client_entrypoint regardless of mode (currently only fires for mode:'spa'). Cell 2 + Cell 4 both ship a browser bundle but skip the lint. Open a follow-up to unify the lint dispatcher around 'has client_entrypoint' rather than 'mode === spa'.")
//!
//! @yah:ticket(R017-F1, "Add prerender.from_data shape — enumerate params from a declared data_inputs file")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-05T00:52:55Z)
//! @yah:status(review)
//! @yah:parent(R017)
//! @yah:next("Add a third shape to RouteEntry.prerender alongside the existing literal `{params}` and adapter `{from, query, param}`. Strawman: `{from_data: 'src/data/issues.json', items_key: 'items', param: 'id'}` — read the JSON (already loaded by data_inputs), walk items_key as a dotted/array path, map each item to `{[param]: item[param]}`.")
//! @yah:next("Schema validation in packages/mesofact-runtime/src/validate.ts: from_data must reference a declared data_inputs entry on the same route (cross-reference at defineRoutes time, ValidationFailed if missing); items_key + param both required when from_data is set; mutually exclusive with literal params and from/query/param.")
//! @yah:next("expandPrerenderParams() at packages/mesofact-build/src/index.ts:226-256 is the extension site. Read the data_inputs file (or reuse the read from the prerender driver if it already happened), walk items_key, map to `[{[param]: items[i][param]}, ...]`. Handle missing items_key path with a BuildError naming the file + path.")
//! @yah:next("Optionally refactor prerender.ts:114-118 so the same data file isn't re-parsed N times (once per prerender param) — cache by absPath. Optional optimization, ship correct first.")
//! @yah:next("Fixture at packages/mesofact-build/tests/fixtures/prerender-from-data/: mode:'static' route, data_inputs:['data/items.json'] (content `{items:[{id:'a'},{id:'b'}]}`), prerender:{from_data:'data/items.json', items_key:'items', param:'id'}. Assert build emits TWO HTML files with each render's req.params.id correctly set.")
//! @yah:verify("bun test packages/mesofact-build — new prerender-from-data fixture passes; reject paths covered (from_data → undeclared data_inputs; items_key missing in JSON; param not a string)")
//! @yah:verify("bun test packages/mesofact-runtime — baseline still passes (incl. validate.ts changes)")
//! @yah:verify("End-to-end: yah-camp R443-F2 declares /issues/:id with prerender.from_data on src/data/issues.json and gets one static HTML per issue ID")
//! @yah:handoff("Shipped prerender.from_data — third PrerenderConfig variant alongside literal {params} and adapter {from,query,param}. Changes: (1) packages/mesofact-runtime/src/routes.ts — extended PrerenderConfig union with {from_data,items_key,param}; defineRoutes now throws when from_data is set but not declared in data_inputs (names the offending route + the declared list). (2) packages/mesofact-runtime/src/manifest.ts — added matching ManifestPrerender variant so the publisher/proxy keep round-tripping the original intent. (3) packages/mesofact-build/src/index.ts — new expandFromData branch reads JSON from projectRoot, walks items_key as a dotted/array path, maps each item to {[param]: stringValue}; BuildError names route + file path on every reject (read failure, missing key, non-array, non-object item, non-string param). (4) packages/mesofact-build/src/manifest-build.ts — toManifestPrerender preserves the from_data shape in the emitted manifest. (5) crates/mesofact/src/manifest.rs — Rust Prerender enum gained FromData variant (untagged serde) so a manifest carrying from_data deserializes in the proxy/publisher. (6) Fixtures: prerender-from-data/ (happy), -undeclared/, -missing-key/, -non-string/. (7) Tests: 4 in build.test.ts (happy expands a+b → two HTML; reject undeclared / missing key / non-string param — each names the file and field in the BuildError); 3 in routes.test.ts (accept when declared; reject when declared list differs; reject when data_inputs entirely omitted). Verify: bun test mesofact-build = 40 pass; bun test mesofact-runtime = 69 pass; mesofact-build/runtime/worker typecheck clean (the duplicate-State errors in tests/fixtures/spa/static-islands/ssr-universal client files are pre-existing R016-F1/R015-F3 territory, not introduced here).")
//! @yah:handoff("Did NOT do the optional prerender.ts:114-118 cache-by-absPath refactor (relay marks it optional, \"ship correct first\"). Today a route with from_data on data/items.json reads + JSON.parses the file once at expand time, then again once per emitted param when prerender.ts builds req.data. For the R443-F2 issues fixture that is small (one file, N=2-50 issues), the cost is negligible — fine to defer to a follow-on F-ticket if it shows up in profiles.")
//! @yah:handoff("Rust crate has a pre-existing build break (crates/mesofact/src/proxy/session.rs:28 imports cheers_core::PasetoV4Codec, which is gone). The Prerender variant I added compiles in isolation but cargo build -p mesofact errors on the unrelated import. Flag for whoever fixes the cheers_core mismatch — once that lands, the new FromData variant rides for free via the same untagged-serde pattern as SourceDerived.")
//! @yah:next("operator to sign off or send back. Optional follow-ups: (a) prerender.ts read-cache by absPath; (b) confirm walkDottedPath shape against R443-F2 issues.json; (c) once Rust crate builds again, sanity-check that a manifest with from_data round-trips through crates/mesofact/src/manifest.rs (a small unit test).")
//! @yah:verify("cd packages/mesofact-build && bun test — 40 pass")
//! @yah:verify("cd packages/mesofact-runtime && bun test — 69 pass")
//! @yah:verify("cd packages/mesofact-runtime && bun run typecheck — clean")
//! @yah:verify("cd packages/mesofact-worker && bun run typecheck — clean")
//! @yah:assumes("Cross-check at defineRoutes time (not at build-time validate) is the right home for \"from_data must be in data_inputs\" — it fails fast at config import, before any bundling work. Matches how placement rejection is wired (R015-F1).")
//! @yah:assumes("walkDottedPath uses \".\" as the only separator and treats numeric segments inside arrays as indices. The relay strawman example just uses bare \"items\" (single segment) so any deeper shape (e.g. \"feed.posts.0.children\") is untested by the fixture suite. Real R443-F2 may want either confirmation that this is enough or escape syntax for keys with literal dots.")


// @mesofact/build — Mode 1 build pipeline. See README and
// `.yah/docs/architecture/mesofact.md` §"Build pipeline".

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import type { ManifestHydration, RouteEntry, SourceCatalog } from "@mesofact/runtime";
import { r2 } from "@mesofact/runtime";
import {
  assertSsrEntrypoint,
  bundleClientEntrypoints,
  bundleEntrypoints,
  type BundleInput,
  type ClientBundleInput,
} from "./bundle.js";
import {
  assertNoForbiddenImports,
  BROWSER_FORBIDDEN,
  EDGE_FORBIDDEN,
} from "./host-lint.js";
import { BuildError, loadRoutes } from "./load-routes.js";
import { assembleManifest, ValidationFailed } from "./manifest-build.js";
import { prerender, type PrerenderInput } from "./prerender.js";
import { loadCatalog } from "./source-catalog.js";
import { inferFromFile } from "./source-infer.js";
import { buildTagIndex } from "./tag-index.js";

export { BuildError } from "./load-routes.js";
export { ValidationFailed } from "./manifest-build.js";
export type { BundleInput, BundleOutput, ClientBundleInput, ClientBundleOutput } from "./bundle.js";
export { bundleClientEntrypoints } from "./bundle.js";
export type { PrerenderEmission, PrerenderInput } from "./prerender.js";
export type { TagIndex } from "./tag-index.js";
export { buildTagIndex } from "./tag-index.js";
export { inferFromFile, inferFromSource } from "./source-infer.js";
export type { InferenceResult } from "./source-infer.js";
export { routeKey, prerenderKey } from "./route-key.js";

export type BuildOptions = {
  // Project root containing `mesofact.routes.ts` (+ optional
  // `mesofact.config.toml`). Entrypoints are resolved against this dir.
  projectRoot: string;
  // Output directory. Defaults to `<projectRoot>/dist`.
  outDir?: string;
  // Build identifier baked into manifest.json. Caller-supplied so reproducible
  // builds can pin it; defaults to ISO timestamp.
  buildId?: string;
};

export type BuildResult = {
  buildId: string;
  outDir: string;
  manifestPath: string;
  tagIndexPath: string;
  htmlPaths: readonly string[];
};

export async function build(opts: BuildOptions): Promise<BuildResult> {
  const projectRoot = resolve(opts.projectRoot);
  const outDir = resolve(opts.outDir ?? join(projectRoot, "dist"));
  const buildId = opts.buildId ?? defaultBuildId();

  const routesFile = join(projectRoot, "mesofact.routes.ts");
  if (!existsSync(routesFile)) {
    throw new BuildError(`expected ${routesFile} to exist`);
  }
  const { config: routesConfig } = await loadRoutes(routesFile);

  const catalog = loadCatalog(join(projectRoot, "mesofact.config.toml"));

  // Server/client module boundary lint (W173 § "Server/client module boundary
  // lint"). Runs BEFORE bundling so a forbidden specifier short-circuits the
  // pipeline with an actionable error rather than a downstream "Bundle failed"
  // cascade (e.g. when the offending dep isn't even installed). Two passes:
  //   - spa client_entrypoint: forbid node:* + bare-name builtins (browser
  //     can't reach them).
  //   - ssr + placement:"edge": forbid the same set plus common db drivers
  //     (workerd can't link native modules).
  // Placement default is `auto`, which resolves to `host` today — only
  // explicit `placement:"edge"` triggers the edge lint until the
  // auto-classifier (W173 § "Future auto-classifier criteria") ships.
  for (const r of routesConfig.routes) {
    if (r.mode === "spa" && r.client_entrypoint) {
      await assertNoForbiddenImports({
        route: r.route,
        absEntry: resolve(projectRoot, r.client_entrypoint),
        target: "browser",
        forbidden: BROWSER_FORBIDDEN,
        kind: "client_entrypoint",
      });
    } else if (r.mode === "ssr" && r.placement === "edge") {
      await assertNoForbiddenImports({
        route: r.route,
        absEntry: resolve(projectRoot, r.entrypoint),
        target: "bun",
        forbidden: EDGE_FORBIDDEN,
        kind: 'ssr placement:"edge" entrypoint',
      });
    }
  }

  // Bundle (phase 1)
  const bundleInputs: BundleInput[] = routesConfig.routes.map((r) => ({
    route: r.route,
    entrypoint: r.entrypoint,
  }));
  const bundles = await bundleEntrypoints(projectRoot, outDir, bundleInputs);
  const serverPaths = new Map(bundles.map((b) => [b.route, b.serverPath]));
  const bundlePaths = new Map(bundles.map((b) => [b.route, b.absolutePath]));

  // Client tree (phase 1b) — bundle a browser hydration entry to
  // `dist/hydrate/`. A `spa` route MUST declare `client_entrypoint`; an
  // `ssr` route MAY (= the W173 Universal cell — Fetch handler ships
  // server-rendered HTML + the hydrate bundle for client takeover); a
  // `static` route MAY (= the W173 Islands cell — prerendered HTML with
  // a hydrate bundle woven in for per-route interactivity).
  const clientInputs: ClientBundleInput[] = [];
  for (const r of routesConfig.routes) {
    if (r.mode === "spa") {
      if (!r.client_entrypoint) {
        throw new BuildError(`route ${r.route}: mode 'spa' requires a client_entrypoint`);
      }
      clientInputs.push({ route: r.route, clientEntrypoint: r.client_entrypoint });
    } else if (r.mode === "ssr" && r.client_entrypoint) {
      clientInputs.push({ route: r.route, clientEntrypoint: r.client_entrypoint });
    } else if (r.mode === "static" && r.client_entrypoint) {
      clientInputs.push({ route: r.route, clientEntrypoint: r.client_entrypoint });
    }
  }
  const clientBundles = await bundleClientEntrypoints(projectRoot, outDir, clientInputs);
  const hydration = new Map<string, ManifestHydration>(
    clientBundles.map((c) => [c.route, { script: c.script, code_split: c.code_split }]),
  );

  // SSR routes are never prerendered, so prove the Fetch handler shape here
  // (before the manifest hits disk) instead of waiting for the dev proxy / Worker
  // to discover the missing default at first request. See W173 § "v1 schema
  // delta / Entrypoint signatures".
  for (const r of routesConfig.routes) {
    if (r.mode !== "ssr") continue;
    const bundlePath = bundlePaths.get(r.route);
    if (!bundlePath) throw new BuildError(`route ${r.route}: no bundled entrypoint`);
    await assertSsrEntrypoint(r.route, bundlePath);
  }

  // Source inference (phase 3) — scan the *source* file, not the bundle, so
  // adapter calls aren't obscured by minification or barrel-reexport
  // collapsing.
  const inferredSources = new Map<string, readonly string[]>();
  for (const r of routesConfig.routes) {
    if (r.source_reads !== undefined) {
      // Author-supplied wins; skip inference entirely for this route.
      inferredSources.set(r.route, r.source_reads);
      continue;
    }
    const entry = resolve(projectRoot, r.entrypoint);
    inferredSources.set(r.route, inferFromFile(entry).source_reads);
  }

  // Validate + assemble (phases 4 & 6) — throws ValidationFailed before any
  // HTML hits disk.
  const manifest = assembleManifest({
    routes: routesConfig,
    buildId,
    serverPaths,
    inferredSources,
    hydration,
    catalog,
  });

  // Prerender (phase 5) — Mode 1 (static) and Mode 3 (spa) routes render at
  // build time. SSR routes render per-request and are skipped. When a route
  // has a hydrate bundle (Mode 3, or Mode 1 + client_entrypoint = the W173
  // Islands cell), the input carries the resolved client bundle so the
  // shell gets its state + entry script woven in.
  const prerenderInputs: PrerenderInput[] = [];
  for (const r of routesConfig.routes) {
    if (r.mode === "ssr") continue;
    const params = await expandPrerenderParams(r, catalog, projectRoot);
    const bundlePath = bundlePaths.get(r.route);
    if (!bundlePath) {
      throw new BuildError(`route ${r.route}: no bundled entrypoint`);
    }
    const input: PrerenderInput = { route: r.route, bundlePath, params };
    const h = hydration.get(r.route);
    if (h) input.hydration = { buildId, script: h.script };
    if (r.data_inputs && r.data_inputs.length > 0) {
      input.dataInputs = r.data_inputs.map((rel) => ({
        relPath: rel,
        absPath: resolve(projectRoot, rel),
      }));
    }
    prerenderInputs.push(input);
  }
  const emissions = await prerender(outDir, prerenderInputs);

  // Manifest + tag-index emission
  const manifestPath = join(outDir, "manifest.json");
  const tagIndexPath = join(outDir, "tag-index.json");
  await mkdir(dirname(manifestPath), { recursive: true });
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  const tagIndex = buildTagIndex(buildId, emissions);
  await writeFile(tagIndexPath, `${JSON.stringify(tagIndex, null, 2)}\n`, "utf8");

  return {
    buildId,
    outDir,
    manifestPath,
    tagIndexPath,
    htmlPaths: emissions.map((e) => e.htmlPath),
  };
}

// Expand `prerender` into the concrete param maps render() will be invoked
// against. Four shapes:
//   - undefined         → one render with no params
//   - { params }        → literal list, returned as-is
//   - { from, query, param } → resolve `from` against the catalog, run the
//     query, map each result to `{ [param]: <key> }`. MVP supports only the
//     `list:<prefix>` verb against an r2 (BlobSource) backend; the adapter
//     must already be registered (CLI does this via `registerSourcesFromConfig`;
//     tests register stubs).
//   - { from_data, items_key, param } → read the local JSON file (already
//     declared on the route's `data_inputs`), walk `items_key` as a dotted
//     path to an array, map each element's `[param]` field. Synchronous +
//     local-only, no source adapter.
async function expandPrerenderParams(
  r: RouteEntry,
  catalog: SourceCatalog,
  projectRoot: string,
): Promise<ReadonlyArray<Record<string, string>>> {
  if (r.prerender === undefined) return [{}];
  if ("params" in r.prerender) return r.prerender.params;
  if ("from_data" in r.prerender) {
    return await expandFromData(r, r.prerender, projectRoot);
  }

  const { from, query, param } = r.prerender;
  if (!(from in catalog)) {
    throw new BuildError(
      `route ${r.route}: prerender.from='${from}' is not declared in mesofact.config.toml`,
    );
  }
  const prefix = parseListQuery(r.route, query);
  const objects = await r2(from).list(prefix);
  return objects.map((obj) => ({ [param]: obj.key }));
}

async function expandFromData(
  r: RouteEntry,
  cfg: { from_data: string; items_key: string; param: string },
  projectRoot: string,
): Promise<ReadonlyArray<Record<string, string>>> {
  const { from_data, items_key, param } = cfg;
  const absPath = resolve(projectRoot, from_data);
  let parsed: unknown;
  try {
    parsed = JSON.parse(await readFile(absPath, "utf8"));
  } catch (e) {
    throw new BuildError(
      `route ${r.route}: failed reading prerender.from_data='${from_data}' (${(e as Error).message})`,
    );
  }
  const items = walkDottedPath(parsed, items_key);
  if (items === undefined) {
    throw new BuildError(
      `route ${r.route}: prerender.items_key='${items_key}' not found in ${from_data}`,
    );
  }
  if (!Array.isArray(items)) {
    throw new BuildError(
      `route ${r.route}: prerender.items_key='${items_key}' in ${from_data} is not an array`,
    );
  }
  return items.map((item, i) => {
    if (typeof item !== "object" || item === null) {
      throw new BuildError(
        `route ${r.route}: prerender.from_data='${from_data}' items[${i}] is not an object`,
      );
    }
    const value = (item as Record<string, unknown>)[param];
    if (typeof value !== "string") {
      throw new BuildError(
        `route ${r.route}: prerender.from_data='${from_data}' items[${i}].${param} is not a string (got ${typeof value})`,
      );
    }
    return { [param]: value };
  });
}

// Walk a dotted/array path through a JSON value. "items" → obj.items,
// "data.list" → obj.data.list, "rows.0.children" → obj.rows[0].children.
function walkDottedPath(root: unknown, path: string): unknown {
  let cur: unknown = root;
  for (const segment of path.split(".")) {
    if (cur === null || cur === undefined) return undefined;
    if (Array.isArray(cur)) {
      const idx = Number.parseInt(segment, 10);
      if (Number.isNaN(idx)) return undefined;
      cur = cur[idx];
    } else if (typeof cur === "object") {
      cur = (cur as Record<string, unknown>)[segment];
    } else {
      return undefined;
    }
  }
  return cur;
}

// `list:<prefix>` — verb identifies the operation; prefix is opaque to the
// build (empty prefix lists every object in the bucket). Future BlobSource
// verbs (or KeyValueSource SQL queries) plug in here without changing the
// type-level shape.
function parseListQuery(route: string, query: string): string {
  const PREFIX = "list:";
  if (!query.startsWith(PREFIX)) {
    throw new BuildError(
      `route ${route}: unsupported prerender.query '${query}' (expected '${PREFIX}<prefix>')`,
    );
  }
  return query.slice(PREFIX.length);
}

function defaultBuildId(): string {
  const iso = new Date().toISOString();
  // 2026-05-15T17:00:00.123Z → 2026-05-15T17-00-00
  return iso.replace(/[:.]/g, "-").replace(/-\d+Z$/, "Z").slice(0, 20);
}

// Re-export `ValidationFailed` for instanceof checks at the CLI / test
// boundary without forcing imports of a private path.
export { assembleManifest } from "./manifest-build.js";