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

/** The manifest's `error_routes` — asset keys for the branded error pages. */
export type EdgeErrorRoutes = { "404"?: string; "5xx"?: string };

/** One route as the edge reads it. Only the deferred marker is consulted; the
 *  other `prerender` shapes are build-time and produce ordinary static HTML. */
export type EdgeRoute = {
  route: string;
  prerender?: { deferred?: boolean } | Record<string, unknown>;
};

/** The manifest slice the edge consumes. */
export type EdgeManifest = {
  routes?: EdgeRoute[];
  error_routes?: EdgeErrorRoutes;
  ssr_prefixes?: string[];
};

/**
 * Fetch the published manifest from the asset origin. Returns `null` when it is
 * absent or unreadable — a site with no `manifest.json` (or a transient origin
 * hiccup) simply falls back to binding-only behavior (no deferred routes, no
 * branded error pages). Fetched only on the slow (static-miss) path, so
 * static-heavy sites never pay for it.
 */
export async function loadManifest(
  assetOrigin: string,
): Promise<EdgeManifest | null> {
  try {
    const resp = await fetch(`${assetOrigin}/manifest.json`);
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
