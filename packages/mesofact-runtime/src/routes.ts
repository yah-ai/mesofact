//! @yah:relay(R015, "Render cube support — placement axis + SSR build path + lint (W173)")
//! @yah:at(2026-06-04T19:31:37Z)
//! @yah:status(open)
//! @yah:next("W173 lives in the yah parent camp at .yah/docs/working/W173-mesofact-render-cube.md (relative from mesofact root: ../../.yah/docs/working/W173-mesofact-render-cube.md). Read § 'v1 schema delta' and § 'SSR_PREFIXES derivation rule' before T1/T2.")
//! @yah:next("yah-side consumer relay is R434 in the parent camp — R434-F3 (mesofact-dev SSR subprocess), R434-F4 (pond reconciler ssr_runtime), R434-F5 (first SSR consumer route) all assume this relay ships first.")
//! @yah:next("Coordinate handoff via @mesofact/runtime version bump: yah-side consumes via packages/yah/workload-spec/index.ts and crates/yah/cloud/src/reconciler/mesofact_static.rs.")
//! @yah:next("Order: T1 (schema) → T2 (build path) → then T3 + T4 unblock once a real SSR consumer exists on the yah side (R434-F5).")
//!
//! @yah:ticket(R015-F1, "Add Placement axis + placement?: field to RouteEntry + defineRoutes validation")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-04T19:32:10Z)
//! @yah:status(review)
//! @yah:phase(P1)
//! @yah:parent(R015)
//! @yah:next("Add `export type Placement = \"host\" | \"edge\" | \"auto\"` alongside the existing RouteMode at routes.ts:5.")
//! @yah:next("Add `placement?: Placement` to RouteEntry. ssr-only — reject (loud, at defineRoutes call site) on any non-\"ssr\" mode. Default \"auto\" → \"host\" today (auto-classifier deferred per W173).")
//! @yah:next("RouteMode \"ssr\" slot already exists in this file — do NOT regress it. Do not remove or rename existing fields (requires, source_reads, concurrency, prerender, cache_policy.negative_ttl/vary).")
//! @yah:next("Add a unit test that defineRoutes throws when placement is set on a static or spa route.")
//! @yah:next("Place where the build classifier eventually slots in: comment that `placement: \"auto\"` resolves to \"host\" until the auto-classifier (W173 § 'Future auto-classifier criteria') lands.")
//! @yah:verify("defineRoutes accepts every existing yah-side routes file (../../app/yah/web/marketing/mesofact.routes.ts and ../../app/yah/web/dashboard/mesofact.routes.ts) unchanged")
//! @yah:verify("defineRoutes throws on `mode:\"static\", placement:\"host\"` etc.")
//! @yah:verify("bun test passes for the new placement validation cases")
//! @yah:handoff("Placement axis shipped. Changes to packages/mesofact-runtime/src/routes.ts: added `export type Placement = \"host\" | \"edge\" | \"auto\"`; added `placement?: Placement` to RouteEntry; defineRoutes now throws when placement is set on a non-ssr route (loud, names the offending route, default left undefined — auto-resolution happens at build time per W173). Exported Placement from src/index.ts. New tests/routes.test.ts covers 8 cases (ssr+host/edge/auto/undefined accepted; static+placement and spa+placement rejected; error message names route; mixed-mode workload accepted). Verified: bun test → 57 pass across 7 files; tsc --noEmit clean for runtime + build + worker; existing yah-side route files (marketing 5, dashboard 7, yah-dev 3) still parse unchanged.")
//! @yah:verify("cd packages/mesofact-runtime && bun test — 57 pass")
//! @yah:verify("cd packages/mesofact-runtime && bun run typecheck — clean")
//! @yah:verify("cd packages/mesofact-build && bun run typecheck — clean")
//! @yah:verify("cd packages/mesofact-worker && bun run typecheck — clean")

// `mesofact.routes.ts` — user-authored route table. Build phase 2 reads this,
// phase 3 infers `source_reads`, phase 4 validates, phase 6 emits the manifest.
// See `.yah/docs/architecture/mesofact.md` §"Build pipeline".

export type RouteMode = "static" | "ssr" | "spa";

// Where per-request SSR rendering runs. Only meaningful for `mode:"ssr"`;
// rejected at defineRoutes for static/spa. `"auto"` is the default — today
// it resolves to `"host"` at build time. A future auto-classifier may pick
// `"edge"` when criteria match (data-only sources, no host-only imports,
// cacheable). See W173 § "Future auto-classifier criteria".
export type Placement = "host" | "edge" | "auto";

export type Requires = "user" | "project" | "region";

export type CachePolicyConfig = {
  ttl: number;
  swr?: number;
  negative_ttl?: number;
  vary?: readonly string[];
};

// Literal param maps OR a source-derived query the publisher runs at build
// time. Mode 1 routes only; non-parametric Mode 1 routes omit it.
//
// Three shapes:
//   - { params }                    literal list, used as-is
//   - { from, query, param }        registered source adapter (R2 BlobSource)
//                                   walked at build time via async load
//   - { from_data, items_key, param }
//                                   local-JSON file already declared in the
//                                   same route's `data_inputs`. Read
//                                   synchronously, walked via `items_key` as
//                                   a dotted/array path.
export type PrerenderConfig =
  | { params: ReadonlyArray<Record<string, string>> }
  | { from: string; query: string; param: string }
  | { from_data: string; items_key: string; param: string };

export type RouteEntry = {
  route: string;
  mode: RouteMode;
  entrypoint: string;
  // Mode 3 (spa) only — the browser hydration entry. Required for `spa`
  // routes; the build bundles it (browser target, content-hashed, code-split)
  // to `dist/hydrate/` and records the result in the manifest's `hydration`.
  client_entrypoint?: string;
  requires?: readonly Requires[];
  // Usually inferred by the build's adapter-import analysis. Setting it here
  // is an explicit override (e.g. third-party module re-exporting an adapter).
  source_reads?: readonly string[];
  // Paths (relative to project root) of JSON files read as build-time data.
  // Parsed content is passed to render() as `req.data[path]`. Mode 1 only.
  // When any listed file changes, the route should be rebuilt.
  data_inputs?: readonly string[];
  cache_policy: CachePolicyConfig;
  concurrency?: number;
  prerender?: PrerenderConfig;
  // SSR-only: where per-request rendering runs. Default `"auto"` resolves to
  // `"host"` until the W173 auto-classifier ships.
  placement?: Placement;
};

export type ErrorRoutes = {
  "404"?: string;
  "5xx"?: string;
};

export type RoutesConfig = {
  routes: readonly RouteEntry[];
  error_routes?: ErrorRoutes;
};

export function defineRoutes(config: RoutesConfig): RoutesConfig {
  for (const r of config.routes) {
    if (r.placement !== undefined && r.mode !== "ssr") {
      throw new Error(
        `defineRoutes: route ${r.route} has placement=${JSON.stringify(r.placement)} but mode=${JSON.stringify(r.mode)}; placement is only valid on mode:"ssr"`,
      );
    }
    if (r.prerender && "from_data" in r.prerender) {
      const declared = r.data_inputs ?? [];
      if (!declared.includes(r.prerender.from_data)) {
        throw new Error(
          `defineRoutes: route ${r.route} has prerender.from_data=${JSON.stringify(r.prerender.from_data)} but that path is not in data_inputs (${JSON.stringify(declared)}); declare the file in data_inputs first so the build reads it once`,
        );
      }
    }
  }
  return config;
}
