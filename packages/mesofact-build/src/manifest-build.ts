//! @yah:ticket(R015-F2, "Build pipeline accepts mode:\"ssr\" + emits derived SSR-prefix set in manifest")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-04T19:32:26Z)
//! @yah:status(review)
//! @yah:phase(P1)
//! @yah:parent(R015)
//! @yah:next("Accept mode:\"ssr\" entrypoints in the bundle/prerender pipeline. SSR routes use the Web Fetch handler shape: `export default async (req: Request): Promise<Response>`. Skip prerender for ssr routes; bundle the entrypoint module for the dev subprocess + pond container + Worker bundle to consume.")
//! @yah:next("Build-time error if the export shape disagrees with mode (RenderFn for static/spa, Fetch for ssr).")
//! @yah:next("Derive SSR-prefix set per W173 § 'SSR_PREFIXES derivation rule': prefix is the route string up to (but not including) the first parametric segment (`:foo`) or wildcard (`*`); non-parametric SSR routes use the full route. Examples: /api/health → /api/health, /api/users/:id → /api/users/, /x/:a/y → /x/, /feed/* → /feed/.")
//! @yah:next("Emit the prefix set as a new field in the manifest. Pick the field name carefully — it becomes a contract with crates/yah/cloud/src/reconciler/mesofact_static.rs (R434-F4) and crates/yah/mesofact-dev/src/lib.rs (R434-F3). Suggest `ssr_prefixes: string[]`.")
//! @yah:next("data_inputs plumbing to SSR entrypoints is deferred per W173 Open questions — start with raw Fetch handlers, add data_inputs only if the first consumer needs it.")
//! @yah:verify("A fixture routes file with one mode:\"ssr\" route builds successfully")
//! @yah:verify("Manifest output contains the ssr_prefixes field with the derived set")
//! @yah:verify("Build fails (with a useful error chain) when an ssr route's default export has the wrong call shape")
//! @yah:depends_on(R015-F1)
//! @yah:handoff("Build path for mode:\"ssr\" + manifest delta shipped. Changes: (1) Manifest gained `ssr_prefixes?: readonly string[]` (top-level, derived from SSR routes) + ManifestRoute gained `placement?: ResolvedPlacement` (\"host\"|\"edge\", auto resolved at build time per W173). (2) validate.ts validates both, rejects \"auto\" / placement-on-non-ssr in the manifest. (3) New packages/mesofact-build/src/ssr-prefix.ts holds the pure W173 derivation (segment-aware: /api/health stays full, /api/users/:id → /api/users/, /x/:a/y → /x/, /feed/* → /feed/) plus a deduper. (4) assembleManifest now resolves placement (auto/undefined → host today; classifier hook at resolvePlacement) and emits the derived ssr_prefixes. (5) New assertSsrEntrypoint(bundlePath) in bundle.ts dynamic-imports each SSR bundle post-bundling and rejects when `default` isn't a function — names the route + describes what was seen. Wired into build pipeline before manifest emission, so wrong-shape entrypoints fail BEFORE manifest hits disk. (6) Fixtures: tests/fixtures/ssr (mixed-mode: static / + ssr /api/health w/ explicit placement:\"host\" + ssr /api/users/:id w/ default placement) and tests/fixtures/ssr-broken (ssr route exporting `render` instead of default Fetch). (7) tests/ssr-prefix.test.ts (9 unit tests against the W173 table — derive + filter+dedupe+sort) and 2 new build-level tests in build.test.ts. SSR routes are correctly skipped by prerender (already true via existing `if r.mode === \"ssr\" continue`); SSR entrypoints still bundle to dist/server/ for dev subprocess / pond container / Worker bundle consumption. Verified: mesofact-build 30 pass (was 18); mesofact-runtime 57 pass; typecheck clean across runtime/build/worker; yah-side marketing+dashboard route files still parse.")
//! @yah:verify("cd packages/mesofact-build && bun test — 30 pass")
//! @yah:verify("cd packages/mesofact-runtime && bun test — 57 pass")
//! @yah:verify("cd packages/mesofact-build && bun run typecheck — clean")
//! @yah:verify("cd packages/mesofact-worker && bun run typecheck — clean")

// Phase 6 — manifest emission. Combine RoutesConfig + bundle outputs +
// inferred `source_reads` into a Manifest, run runtime's `validate()` over
// it, and write `dist/manifest.json`.

import type {
  Manifest,
  ManifestHydration,
  ManifestPrerender,
  ManifestRoute,
  ResolvedPlacement,
  RouteEntry,
  RoutesConfig,
  SourceCatalog,
  ValidationError,
} from "@mesofact/runtime";
import { MANIFEST_VERSION, validate } from "@mesofact/runtime";
import { BuildError } from "./load-routes.js";
import { deriveSsrPrefixes } from "./ssr-prefix.js";

export type AssembleInput = {
  routes: RoutesConfig;
  buildId: string;
  // route → `dist/server/<key>.js` (returned by `bundleEntrypoints`)
  serverPaths: ReadonlyMap<string, string>;
  // route → inferred or overridden source_reads
  inferredSources: ReadonlyMap<string, readonly string[]>;
  // route → resolved client bundle (Mode 3 only; from `bundleClientEntrypoints`)
  hydration?: ReadonlyMap<string, ManifestHydration>;
  catalog: SourceCatalog;
};

export function assembleManifest(input: AssembleInput): Manifest {
  const routes = input.routes.routes.map((entry) => buildRoute(entry, input));
  const ssrPrefixes = deriveSsrPrefixes(input.routes.routes);
  const manifest: Manifest = {
    version: MANIFEST_VERSION,
    build_id: input.buildId,
    routes,
    static_assets: [],
    ...(input.routes.error_routes ? { error_routes: input.routes.error_routes } : {}),
    ...(ssrPrefixes.length > 0 ? { ssr_prefixes: ssrPrefixes } : {}),
  };

  const result = validate(manifest, input.catalog);
  if (!result.ok) {
    throw new ValidationFailed(result.errors);
  }
  return result.manifest;
}

// Resolve `placement: "auto" | undefined` to a concrete `"host" | "edge"`.
// Today this is trivial: `auto`/undefined → `host`. The W173 auto-classifier
// (Future auto-classifier criteria) will swap in here later.
function resolvePlacement(entry: RouteEntry): ResolvedPlacement {
  if (entry.placement === "edge") return "edge";
  return "host";
}

function buildRoute(entry: RouteEntry, input: AssembleInput): ManifestRoute {
  const serverPath = input.serverPaths.get(entry.route);
  if (!serverPath) {
    throw new BuildError(`route ${entry.route}: no bundled entrypoint`);
  }
  const source_reads =
    entry.source_reads !== undefined
      ? entry.source_reads
      : input.inferredSources.get(entry.route) ?? [];

  const hydration = input.hydration?.get(entry.route);

  const out: ManifestRoute = {
    route: entry.route,
    mode: entry.mode,
    render_entrypoint: serverPath,
    cache_policy: entry.cache_policy,
    ...(entry.requires ? { requires: entry.requires } : {}),
    ...(source_reads.length > 0 ? { source_reads } : {}),
    ...(entry.data_inputs && entry.data_inputs.length > 0
      ? { data_inputs: entry.data_inputs }
      : {}),
    ...(entry.concurrency !== undefined ? { concurrency: entry.concurrency } : {}),
    ...(hydration ? { hydration } : {}),
    ...(entry.prerender ? { prerender: toManifestPrerender(entry.prerender) } : {}),
    ...(entry.mode === "ssr" ? { placement: resolvePlacement(entry) } : {}),
  };
  return out;
}

function toManifestPrerender(p: NonNullable<RouteEntry["prerender"]>): ManifestPrerender {
  if ("params" in p) return { params: p.params };
  if ("from_data" in p) {
    return { from_data: p.from_data, items_key: p.items_key, param: p.param };
  }
  return { from: p.from, query: p.query, param: p.param };
}

export class ValidationFailed extends Error {
  constructor(public readonly errors: ValidationError[]) {
    super(
      `manifest validation failed:\n${errors
        .map((e) => `  - [${e.kind}] ${e.path}: ${e.message}`)
        .join("\n")}`,
    );
    this.name = "ValidationFailed";
  }
}
