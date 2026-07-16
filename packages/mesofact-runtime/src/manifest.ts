// Manifest types — the single document the build emits and the proxy boots
// from. Authored shape lives in `routes.ts` (RoutesConfig); the build enriches
// it with `build_id`, `static_assets`, hydration, and resolved entrypoint
// paths.
//
// Versioned independently of the mesofact binary; major bumps force restart.
// See `.yah/docs/architecture/mesofact.md` §"Manifest schema".

import type { Placement, ResiliencePolicy, RouteMode, Requires } from "./routes.js";

// Placement as carried in the manifest — the build resolves `"auto"` to
// `"host"` or `"edge"` before emission, so consumers never see `"auto"`.
// See W173 § "Placement: validation rules".
export type ResolvedPlacement = Exclude<Placement, "auto">;

export const MANIFEST_VERSION = "1" as const;

export type ManifestVersion = typeof MANIFEST_VERSION;

export type ManifestCachePolicy = {
  ttl: number;
  swr?: number;
  negative_ttl?: number;
  vary?: readonly string[];
};

export type ManifestHydration = {
  script: string;
  code_split: readonly string[];
};

export type ManifestPrerender =
  | { params: ReadonlyArray<Record<string, string>> }
  | { from: string; query: string; param: string }
  | { from_data: string; items_key: string; param: string }
  // Instance-addressed: params are minted after the build (publish time); the
  // build emits the server bundle + manifest entry and prerenders nothing.
  // Serving resolves each instance through the pointer store (W270 §2). Mirrors
  // `Prerender::Deferred` in `crates/mesofact/src/manifest.rs`.
  | { deferred: true };

export type ManifestRoute = {
  route: string;
  mode: RouteMode;
  render_entrypoint: string;
  requires?: readonly Requires[];
  source_reads?: readonly string[];
  // Build-time data artifact paths (relative to project root). Present when
  // the route declared `data_inputs`; used by the reconciler to detect which
  // file changes trigger a rebuild of this route.
  data_inputs?: readonly string[];
  cache_policy: ManifestCachePolicy;
  concurrency?: number;
  hydration?: ManifestHydration;
  prerender?: ManifestPrerender;
  // SSR routes only; never "auto" (resolved at build time per W173).
  placement?: ResolvedPlacement;
  // SSR routes only (W181). Carried verbatim from the route declaration so
  // the Worker (prod) and the mesofact-dev proxy (dev) apply the same retry/
  // timeout policy around the origin hop. Absent = one attempt, default
  // timeout, fail with 502 — exactly the pre-W181 behavior.
  resilience?: ResiliencePolicy;
};

export type ManifestStaticAsset = {
  key: string;
  content_hash: string;
  content_type: string;
  immutable: boolean;
};

export type ManifestErrorRoutes = {
  "404"?: string;
  "5xx"?: string;
};

export type Manifest = {
  version: ManifestVersion;
  build_id: string;
  routes: readonly ManifestRoute[];
  static_assets: readonly ManifestStaticAsset[];
  error_routes?: ManifestErrorRoutes;
  // Derived from every `mode:"ssr"` route per W173 § "SSR_PREFIXES derivation
  // rule". Used by mesofact-dev (proxy) and the CF Worker to forward matching
  // paths to the SSR runtime. Segment-aware match: `path === p || path.startsWith(p + "/")`.
  // Absent when the workload has no SSR routes.
  ssr_prefixes?: readonly string[];
};
