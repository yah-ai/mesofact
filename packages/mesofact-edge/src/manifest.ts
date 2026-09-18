// @mesofact/edge — manifest fetch + deferred-route matching.
//
// Part of R595-F3 — annotation in
// .yah/docs/working/W270-yah-share-mesofact-gap-closure.md
//
// The worker is manifest-driven: on a static miss it reads the published
// manifest to learn `error_routes` and which routes are instance-addressed
// (`prerender.deferred === true` → served via the pointer store, not build-time
// HTML). Only this slice of the manifest matters at the edge, so the types are
// declared locally — the worker stays a self-contained browser bundle with no
// `@mesofact/runtime` import. The canonical shapes live in
// `oss/mesofact/crates/mesofact/src/manifest.rs` (Rust) /
// `packages/mesofact-runtime/src/manifest.ts` (TS); keep this subset in step.

import {
  derivePageCacheHeaders,
  type DerivedCacheHeaders,
  type EdgeCachePolicy,
} from "./cache-policy.js";

/** The manifest's `error_routes` — asset keys for the branded error pages. */
export type EdgeErrorRoutes = { "404"?: string; "5xx"?: string };

/** One route as the edge reads it. The deferred marker selects pointer
 *  resolution (the other `prerender` shapes are build-time and produce ordinary
 *  static HTML); `cache_policy` + `requires` are what a served page's
 *  `Cache-Control` is derived from (R749-B4). */
export type EdgeRoute = {
  route: string;
  requires?: readonly string[];
  cache_policy?: EdgeCachePolicy;
  prerender?: { deferred?: boolean } | Record<string, unknown>;
};

/** The manifest slice the edge consumes. */
export type EdgeManifest = {
  /**
   * The publisher's commit point. `publish_dist` uploads an immutable
   * `<build_id>/` tree and then flips this field in the root `manifest.json`
   * LAST, so it is the site-level root pointer in everything but name — see
   * `mesofact-publisher/src/pointer.rs`, which says so directly ("The
   * site-level root pointer is conceptually `key = ""` of this store").
   */
  build_id?: string;
  routes?: EdgeRoute[];
  error_routes?: EdgeErrorRoutes;
  ssr_prefixes?: string[];
};

/**
 * The prefix a build's *page* assets live under, or `null` for a manifest that
 * names no build.
 *
 * `publish_dist` uploads `dist/` verbatim beneath `<build_id>/`, so the pages
 * that `dist/html/` holds land at `<build_id>/html/<key>`. Publishers that
 * predate the build tree flatten instead — they strip the `html/` segment and
 * write `<key>` at the prefix root — which is why the edge has to try both.
 */
export function buildPageRoot(manifest: EdgeManifest | null): string | null {
  const id = manifest?.build_id;
  return typeof id === "string" && id.length > 0 ? `${id}/html` : null;
}

/**
 * Fetch the published manifest from the asset origin. Returns `null` when it is
 * absent or unreadable — a site with no `manifest.json` (or a transient origin
 * hiccup) simply falls back to binding-only behavior (no deferred routes, no
 * branded error pages, no build tree).
 *
 * Read on PAGE requests and on any static miss. It used to be static-miss only,
 * on the reasoning that static-heavy sites should never pay for it; resolving
 * the build pointer needs it up front (R330-B44). Requests for assets that
 * carry a non-HTML extension — the hashed bundles and images that are most of a
 * static site's request volume — still skip it entirely, so the fast path is
 * preserved where it actually carries traffic. The manifest is published
 * `no-cache` and is deliberately NOT given a `cacheTtl` override here: it is the
 * pointer, and a cached pointer is a stale site.
 */
export async function loadManifest(
  get: (key: string) => Promise<Response>,
): Promise<EdgeManifest | null> {
  try {
    // `get` reads one key from wherever the matched route's bytes live — an
    // HTTP origin or an R2 binding (R560-F13).
    const resp = await get("manifest.json");
    if (!resp.ok) {
      return null;
    }
    return (await resp.json()) as EdgeManifest;
  } catch {
    return null;
  }
}

/** True when `pathname` matches an instance-addressed (deferred) route. */
export function matchesDeferredRoute(
  manifest: EdgeManifest | null,
  pathname: string,
): boolean {
  if (!manifest?.routes) {
    return false;
  }
  return manifest.routes.some(
    (r) => isDeferred(r) && matchRoutePattern(r.route, pathname),
  );
}

/**
 * The `Cache-Control` (+ `Vary`) the manifest declares for a page served at
 * `pathname`, or `null` when no route declared one.
 *
 * FIRST MATCH WINS among the routes that actually produce a rule — a route with
 * the inert `{ ttl: 0 }` does not shadow a later one that declares a real
 * policy. That is the same order `CachePolicyTable` uses in
 * `crates/mesofact-core/src/cache_policy.rs`, which drops inert routes when it
 * builds the table and then takes the first match.
 */
export function pageCacheHeaders(
  manifest: EdgeManifest | null,
  pathname: string,
): DerivedCacheHeaders | null {
  for (const route of manifest?.routes ?? []) {
    const derived = derivePageCacheHeaders(route.cache_policy, isGated(route));
    if (derived && matchRoutePattern(route.route, pathname)) {
      return derived;
    }
  }
  return null;
}

function isGated(route: EdgeRoute): boolean {
  return Array.isArray(route.requires) && route.requires.length > 0;
}

function isDeferred(route: EdgeRoute): boolean {
  const p = route.prerender as { deferred?: unknown } | undefined;
  return !!p && p.deferred === true;
}

/**
 * Segment-aware match of a route pattern (`/c/:slug`) against a concrete path
 * (`/c/abc123`). A `:param` segment matches any single non-empty segment; the
 * segment counts must be equal, so a trailing `:param` does not swallow extra
 * segments (one instance per slug — `/c/a/b` does not match `/c/:slug`).
 */
export function matchRoutePattern(pattern: string, pathname: string): boolean {
  const pat = splitSegments(pattern);
  const path = splitSegments(pathname);
  if (pat.length !== path.length) {
    return false;
  }
  for (let i = 0; i < pat.length; i++) {
    const seg = pat[i];
    if (seg.startsWith(":")) {
      if (path[i].length === 0) {
        return false;
      }
    } else if (seg !== path[i]) {
      return false;
    }
  }
  return true;
}

function splitSegments(p: string): string[] {
  return p.split("/").filter((s) => s.length > 0);
}
