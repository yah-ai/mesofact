// @mesofact/edge — the manifest-driven Cloudflare Worker that fronts every
// mesofact site.
//
// Part of R595-F3 — annotation in
// .yah/docs/working/W270-yah-share-mesofact-gap-closure.md (W270 §3). This is
// the versioned serving artifact yah's cloud reconciler deploys; it supersedes
// the camp-local worker that lived at oss/yubaba/crates/cloud/worker/router.ts.
//
// Config is injected via plain_text Worker bindings, plus the published
// manifest (read lazily on a static miss):
//   ASSET_ORIGIN             — base URL for static (build-output) assets (no
//                              trailing slash); the catch-all for non-route URLs
//   POINTER_ORIGIN           — base URL the pointer store is read from
//                              (`p/<key>` objects). Defaults to ASSET_ORIGIN
//                              (pointers live under `p/` in the same bucket);
//                              kept distinct so a future consumer can front the
//                              (uncached) pointer reads separately.
//   UPLOAD_ORIGIN            — base URL for dynamic /uploads/* content (R490-T8).
//                              Reserved seam; absent → /uploads/* returns 404.
//   WORKER_MODE              — "static" | "spa" | "ssr"
//   SSR_ORIGIN               — SSR proxy origin URL (empty for non-SSR modes)
//   SSR_PREFIXES             — JSON array of path prefixes to proxy to SSR_ORIGIN
//                              (the escape hatch; normally manifest-derived)
//   SSR_RESILIENCE           — JSON `{ [prefix]: ResiliencePolicy }` (W181 v1);
//                              optional; absent/invalid → one attempt, no timeout
//   MESOFACT_BACKEND_ORIGIN  — almanac surface; /api/releases* proxied here
//   ISSUES_ORIGIN            — issue-tracker surface; /api/issues* proxied here
//   ROUTE_HEADERS            — JSON `[{ path, headers }]`, the domain manifest's
//                              per-route response headers in manifest order (see
//                              `applyRouteHeaders`); optional, absent → no-op
//
// Manifest-driven behavior:
//   * PAGE requests resolve against the build tree the manifest's `build_id`
//     points at (`<build_id>/html/<key>`) before the flat copy at the prefix
//     root — the site-level root pointer, which is what makes a revalidate
//     visible at the edge at all (yah R330-B44);
//   * on a static miss, a path matching an instance-addressed (deferred) route
//     resolves through the pointer store — present → render-root bytes
//     (immutable cache), deleted → 410, absent → the manifest's
//     error_routes.404 page;
//   * error_routes.{404,5xx} are honored (branded pages), replacing the old
//     hardcoded plaintext 404.

import {
  buildPageRoot,
  loadManifest,
  matchesDeferredRoute,
  pageCacheHeaders,
  type EdgeErrorRoutes,
  type EdgeManifest,
} from "./manifest.js";
import { PointerMalformed, resolvePointer } from "./pointer.js";

interface Env {
  ASSET_ORIGIN: string;
  POINTER_ORIGIN?: string;
  UPLOAD_ORIGIN?: string;
  WORKER_MODE: string;
  SSR_ORIGIN: string;
  SSR_PREFIXES: string;
  SSR_RESILIENCE?: string;
  MESOFACT_BACKEND_ORIGIN?: string;
  ISSUES_ORIGIN?: string;
  ROUTE_HEADERS?: string;
}

/** One entry of the `ROUTE_HEADERS` table — mirrors `DomainRoute` in
 *  `oss/yubaba/crates/cloud/src/config.rs` (path + its `headers` map). */
interface RouteHeaderRule {
  path: string;
  headers: Record<string, string>;
}

// W181 v1 schema mirror — see oss/mesofact/packages/mesofact-runtime/src/routes.ts.
// Worker only consumes retry+timeout; queue is rejected upstream at defineRoutes.
type RetryOn = "connection" | "5xx" | "any";
interface RetryPolicy {
  attempts: number;
  backoff_ms: number[];
  retry_on?: RetryOn;
  budget_ms?: number;
}
interface ResiliencePolicy {
  retry?: RetryPolicy;
  timeout_ms?: number;
}
type ResilienceMap = Record<string, ResiliencePolicy>;

// Content-addressed responses (hashed assets, published instance pages) are
// immutable — the pointer is the only mutable object.
const IMMUTABLE_CACHE_CONTROL = "public, max-age=31536000, immutable";

// A page served out of the build tree is bytes at an IMMUTABLE url being served
// at a MUTABLE one (`/x`, whose content changes when the pointer moves).
// Passing the object's own header through would let a client hold a day-old
// release page after a revalidate, which is the freshness bug this indirection
// exists to fix, reintroduced one layer down.
//
// This is the DEFAULT, not the answer: since R749-B4 a route that declares a
// `cache_policy` gets that policy's derived header instead (`withPageCache`).
// The default is what a page whose author said nothing about caching gets, and
// "nothing" cannot safely mean "hold it" at a mutable URL.
const PAGE_CACHE_CONTROL = "no-cache";

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    // Route first, then stamp the matching route's declared response headers
    // onto whatever came back. Wrapping the whole router — rather than each
    // `return` inside it — is what makes the guarantee total: a header a
    // domain declares for a path applies to the asset hit, the clean-URL hit,
    // the SPA shell, the branded 404 and the SSR proxy alike. A header set
    // that only held on the happy path would be worse than none for the case
    // that motivated this (COOP/COEP: a document served without them silently
    // loses SharedArrayBuffer instead of failing loudly).
    const resp = await route(request, env);
    return applyRouteHeaders(resp, new URL(request.url).pathname, env.ROUTE_HEADERS);
  },
};

/** The router proper. Every `return` here is post-processed by
 *  [`applyRouteHeaders`] in the exported `fetch` above. */
async function route(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const path = url.pathname;
  const resilience = parseResilience(env.SSR_RESILIENCE);

  // Backend API routing (R455-T4): /api/issues* → ISSUES_ORIGIN,
  // /api/releases* → MESOFACT_BACKEND_ORIGIN. Takes priority over SSR
  // routing so pond/prod paths hit the backend container directly.
  if (env.ISSUES_ORIGIN && path.startsWith("/api/issues")) {
    const target =
      env.ISSUES_ORIGIN +
      "/issues" +
      path.slice("/api/issues".length) +
      url.search;
    return proxyWithResilience(request, target, policyFor(resilience, path));
  }
  if (env.MESOFACT_BACKEND_ORIGIN && path.startsWith("/api/releases")) {
    const target =
      env.MESOFACT_BACKEND_ORIGIN +
      "/releases" +
      path.slice("/api/releases".length) +
      url.search;
    return proxyWithResilience(request, target, policyFor(resilience, path));
  }

  // SSR: proxy matching prefixes to origin
  if (env.WORKER_MODE === "ssr" && env.SSR_ORIGIN) {
    let prefixes: string[] = [];
    try {
      prefixes = JSON.parse(env.SSR_PREFIXES);
    } catch {
      // malformed JSON — fall through to asset serving
    }
    // Segment-aware match (W173): exact prefix OR descendant under prefix.
    // Naive `path.startsWith(p)` would proxy /api/healthcheck to an
    // /api/health origin — bytes match, segments don't.
    const matched = prefixes.find(
      (p) => path === p || path.startsWith(p.endsWith("/") ? p : p + "/"),
    );
    if (matched) {
      const target = env.SSR_ORIGIN + path + url.search;
      return proxyWithResilience(request, target, policyFor(resilience, path));
    }
  }

  // Dynamic user content (R490-T8): /uploads/* routes to UPLOAD_ORIGIN,
  // separate from the build-output static assets on ASSET_ORIGIN. No writer
  // exists yet — the binding is a reserved seam; absent → clean 404, and a
  // miss is a real 404 (never the SPA shell or 404.html, which belong to the
  // static site). Segment-aware: the trailing slash keeps /uploadsfoo out.
  if (path.startsWith("/uploads/")) {
    if (!env.UPLOAD_ORIGIN) {
      return new Response("Not Found", { status: 404 });
    }
    const uploadResp = await fetch(`${env.UPLOAD_ORIGIN}/${path.slice(1)}`);
    if (uploadResp.ok) {
      return uploadResp;
    }
    return new Response("Not Found", { status: 404 });
  }

  // Resolve asset key from URL path
  let key: string;
  if (path === "/" || path.endsWith("/")) {
    key = (path === "/" ? "" : path.slice(1)) + "index.html";
  } else {
    key = path.slice(1);
  }

  // ── build-pointer resolution (R330-B44) ──────────────────────────────────
  // A page is served from the build tree the root manifest points at, in
  // preference to the flat copy at the prefix root. Both exist: `publish_dist`
  // writes `<build_id>/html/<key>` and swaps the pointer, while the older flat
  // publishers write `<key>` directly and never sweep what they replaced.
  //
  // Reading the pointer is what makes a push actually land. The revalidate
  // receiver re-renders a route and publishes a NEW build tree — so an edge
  // that only ever reads the flat copy shows the last flat publish forever,
  // and every revalidate is a no-op no matter how correct its output. That is
  // exactly what froze yah.dev/releases: the data was fresh in R2, the render
  // was correct on the node, and the page never moved.
  //
  // Scoped to PAGE requests on purpose. Hashed bundles and images are
  // content-addressed and byte-identical in both layouts, so pointer
  // indirection buys them nothing and would cost every one of them a manifest
  // fetch — and they are the bulk of a static site's requests.
  let manifest: EdgeManifest | null = null;
  let manifestLoaded = false;
  if (isPageKey(key)) {
    manifest = await loadManifest(env.ASSET_ORIGIN);
    manifestLoaded = true;
    const pageRoot = buildPageRoot(manifest);
    if (pageRoot) {
      for (const candidate of assetCandidates(key)) {
        const resp = await fetch(`${env.ASSET_ORIGIN}/${pageRoot}/${candidate}`);
        if (resp.ok) {
          return routePage(resp, manifest, path);
        }
      }
    }
  }

  // Fetch from asset origin — the common (build-time HTML/asset) hit.
  const assetResp = await fetch(`${env.ASSET_ORIGIN}/${key}`);
  if (assetResp.ok) {
    // A page here came from the flat layout (no build tree, or no copy of this
    // key in it); the manifest is already loaded for any page key, so the
    // declared policy still reaches it. Non-page assets are content-hashed and
    // keep the publisher's `immutable` verbatim.
    return isPageKey(key)
      ? withDeclaredCache(assetResp, manifest, path)
      : assetResp;
  }

  // ── static miss ──────────────────────────────────────────────────────────
  // Asset requests still reach the manifest only here, so a site of hashed
  // bundles pays for it exactly once per miss rather than once per request.
  if (!manifestLoaded) {
    manifest = await loadManifest(env.ASSET_ORIGIN);
  }

  // Instance-addressed (deferred) route → resolve through the pointer store.
  if (matchesDeferredRoute(manifest, path)) {
    return serveInstance(env, path, manifest);
  }

  // Clean-URL resolution: an extensionless path (e.g. `/releases`) maps to
  // its prerendered static asset — try `<key>.html` then `<key>/index.html`,
  // the same convention the error-page resolver uses (routeToAssetKeys).
  // This is what lets build-time-static routes serve without a trailing
  // slash or explicit `.html`. Deferred/instance routes are handled above,
  // so they keep priority; assets that already carry an extension (fetched
  // verbatim on the fast path) never reach here.
  for (const candidate of assetCandidates(key).slice(1)) {
    const cleanResp = await fetch(`${env.ASSET_ORIGIN}/${candidate}`);
    if (cleanResp.ok) {
      return withDeclaredCache(cleanResp, manifest, path);
    }
  }

  // static → error page; spa/ssr → index.html shell (client-side routing).
  if (env.WORKER_MODE === "static") {
    return errorResponse(404, env.ASSET_ORIGIN, manifest?.error_routes);
  }
  const shellResp = await fetch(`${env.ASSET_ORIGIN}/index.html`);
  if (shellResp.ok) {
    return new Response(shellResp.body, {
      status: 200,
      headers: shellResp.headers,
    });
  }
  return errorResponse(404, env.ASSET_ORIGIN, manifest?.error_routes);
}

/**
 * Serve an instance-addressed route (`prerender: { deferred: true }`) by
 * resolving its pointer. The pointer key is the request path minus its leading
 * slash (`/c/abc123` → `c/abc123`); the publisher flips the same key. Present →
 * the render-root bytes with immutable cache headers (content-addressed);
 * deleted → 410; absent → the branded 404 page; malformed record → 5xx.
 *
 * "Immutable" is the default here, not the answer: a deferred route that
 * declares a `cache_policy` gets that policy instead (R749-B4). The default
 * reads the response as content-addressed, which is true of the bytes and not
 * of the URL they are served at — so a route whose author put a TTL on it is
 * saying something the default is otherwise free to ignore for a year, and a
 * declaration nothing can override the tier default with is the silent no-op
 * this relay exists to remove.
 */
async function serveInstance(
  env: Env,
  path: string,
  manifest: EdgeManifest | null,
): Promise<Response> {
  const pointerOrigin = env.POINTER_ORIGIN || env.ASSET_ORIGIN;
  const key = path.slice(1);

  let state;
  try {
    state = await resolvePointer(pointerOrigin, key);
  } catch (err) {
    if (err instanceof PointerMalformed) {
      return errorResponse(500, env.ASSET_ORIGIN, manifest?.error_routes);
    }
    throw err;
  }

  if (state.kind === "present") {
    const contentResp = await fetch(
      `${env.ASSET_ORIGIN}/${state.pointer.content_root}`,
    );
    if (!contentResp.ok) {
      // Pointer names bytes that aren't there — treat as not found.
      return errorResponse(404, env.ASSET_ORIGIN, manifest?.error_routes);
    }
    const headers = new Headers(contentResp.headers);
    stampPageCache(headers, manifest, path, IMMUTABLE_CACHE_CONTROL);
    return new Response(contentResp.body, { status: 200, headers });
  }

  if (state.kind === "deleted") {
    // Published then unpublished — 410 Gone, distinct from a never-existed 404.
    return errorResponse(410, env.ASSET_ORIGIN, manifest?.error_routes, "410 Gone");
  }

  // absent
  return errorResponse(404, env.ASSET_ORIGIN, manifest?.error_routes);
}

/**
 * Build an error response honoring the manifest's `error_routes` (W270 §3).
 *
 * `error_routes` values are ROUTE PATHS (e.g. `"/404"` → the marketing `/404`
 * static route), not asset keys — so each is resolved to its prerendered asset
 * exactly like a normal static request (`/404` → `404.html`). Prefers the
 * manifest's branded page for the status class, then the legacy `404.html` (for
 * 4xx), then a plaintext fallback. The chosen page body is served *with the
 * requested status* (e.g. the 404 page under a 410).
 */
async function errorResponse(
  status: number,
  assetOrigin: string,
  errorRoutes: EdgeErrorRoutes | undefined,
  fallbackText?: string,
): Promise<Response> {
  const brandedRoute = status >= 500 ? errorRoutes?.["5xx"] : errorRoutes?.["404"];
  const keys: string[] = [];
  if (brandedRoute) keys.push(...routeToAssetKeys(brandedRoute));
  if (status < 500) keys.push("404.html"); // legacy default when no error_routes
  for (const k of keys) {
    const resp = await fetch(`${assetOrigin}/${k}`);
    if (resp.ok) {
      return new Response(resp.body, { status, headers: resp.headers });
    }
  }
  return new Response(fallbackText ?? defaultStatusText(status), { status });
}

/**
 * True when `key` addresses a page rather than a build asset — extensionless
 * (a clean URL) or an explicit `.html`.
 *
 * This is the gate on pointer resolution, so it decides which requests pay a
 * manifest fetch. Pages are the only mutable output: everything else the
 * builder emits is content-hashed, so it is byte-identical in the flat and
 * build-scoped layouts and gains nothing from the indirection.
 */
function isPageKey(key: string): boolean {
  const last = key.slice(key.lastIndexOf("/") + 1);
  return !last.includes(".") || last.endsWith(".html");
}

/**
 * The ordered asset keys a request may resolve to — the clean-URL rule shared
 * by build-tree lookup, flat lookup, and the error-page resolver: a key that
 * already carries an extension is taken verbatim, an extensionless one also
 * tries `<key>.html` then `<key>/index.html`.
 *
 * One producer on purpose. These three call sites each grew their own copy of
 * this rule, and a fourth layout (the build tree) is exactly the kind of change
 * that makes divergent copies disagree about what a clean URL means.
 *
 * The write-side twin is `prerenderKey`
 * (packages/mesofact-build/src/route-key.ts, ported in
 * crates/mesofact-render/src/route_key.rs): it names each emitted page by the
 * public path it serves at, precisely so this function's candidates find it.
 * R600-B1 was those two disagreeing — do not teach this function any
 * route-pattern rule; move the write instead.
 */
function assetCandidates(key: string): string[] {
  const last = key.slice(key.lastIndexOf("/") + 1);
  if (last.includes(".")) return [key];
  return [key, `${key}.html`, `${key}/index.html`];
}

/**
 * Re-header a page response with the `cache_policy` its route declared
 * (R749-B4).
 *
 * `fallback` is what a page gets when no route declared anything, and is the
 * only difference between this function's two callers:
 *
 *   - a **build-tree** page (`routePage`) falls back to `PAGE_CACHE_CONTROL`,
 *     because immutable bytes served at a mutable URL must not be held;
 *   - a **flat-layout** page (`withDeclaredCache`) falls back to the object's
 *     own header, which is what that path has always returned — the older
 *     publishers that write the flat copy are not `publish_dist` and never
 *     saw a route.
 *
 * Derived from the manifest, not forwarded from the object, for the reason
 * `cache-policy.ts` states at length: the object's header describes the
 * immutable build-scoped URL, and this response is being served at the mutable
 * route.
 */
function withPageCache(
  resp: Response,
  manifest: EdgeManifest | null,
  path: string,
  fallback: string | null,
): Response {
  const headers = new Headers(resp.headers);
  if (!stampPageCache(headers, manifest, path, fallback)) return resp;
  return new Response(resp.body, { status: resp.status, headers });
}

/**
 * Write the declared (or fallback) cache headers into `headers` in place.
 * Returns false when nothing was written, so a caller holding a response it
 * would rather not rebuild can hand back the original.
 *
 * `serveInstance` uses this directly because it re-statuses its response to 200
 * anyway; everything else goes through `withPageCache`.
 */
function stampPageCache(
  headers: Headers,
  manifest: EdgeManifest | null,
  path: string,
  fallback: string | null,
): boolean {
  const declared = pageCacheHeaders(manifest, path);
  if (!declared && fallback === null) return false;
  headers.set("Cache-Control", declared?.cacheControl ?? fallback!);
  if (declared?.vary) headers.set("Vary", declared.vary);
  return true;
}

/** A page out of the build tree, re-headered for service at its stable route. */
function routePage(
  resp: Response,
  manifest: EdgeManifest | null,
  path: string,
): Response {
  return withPageCache(resp, manifest, path, PAGE_CACHE_CONTROL);
}

/**
 * A page out of the flat layout. Untouched unless its route declared a policy —
 * a declaration must not go silently unenforced just because the bytes happened
 * to resolve through the pre-build-tree layout, but neither may this path start
 * inventing headers for the sites that declare nothing.
 */
function withDeclaredCache(
  resp: Response,
  manifest: EdgeManifest | null,
  path: string,
): Response {
  return withPageCache(resp, manifest, path, null);
}

/**
 * Resolve a route path (`/404`, `/errors/nf`) to the ordered asset keys a
 * prerendered static route emits.
 */
function routeToAssetKeys(routePath: string): string[] {
  const rel = routePath.replace(/^\/+/, "");
  if (rel === "") return ["index.html"];
  return assetCandidates(rel);
}

function defaultStatusText(status: number): string {
  if (status === 410) return "Gone";
  if (status >= 500) return "Internal Server Error";
  return "Not Found";
}

// ── per-route response headers ──────────────────────────────────────────────
//
// The domain manifest is the only place that knows about path routing, so it is
// also where a path's *response headers* belong — not in a `_headers` file (a
// Pages/Netlify convention no Worker reads) and not hardcoded here for one
// site's needs. `ROUTE_HEADERS` carries that table verbatim, in manifest order;
// the producer is `DomainConfig::route_headers_json` in
// oss/yubaba/crates/cloud/src/config.rs.
//
// FIRST MATCH WINS, with no merging across rules — the same rule the manifest
// already states for routing ("vec order = match order, first match wins"). One
// path therefore has one header set, decided where the route was decided.

/** Statuses whose responses must carry a null body (constructing one with a
 *  body throws in workerd). */
const NULL_BODY_STATUS = new Set([101, 103, 204, 205, 304]);

/** Memoized parse of the `ROUTE_HEADERS` binding — the value is fixed for the
 *  isolate's lifetime, so re-parsing it per request buys nothing. */
let routeHeaderCache: { raw: string; rules: RouteHeaderRule[] } | undefined;

function applyRouteHeaders(
  resp: Response,
  path: string,
  raw: string | undefined,
): Response {
  const matched = parseRouteHeaders(raw).find((r) =>
    matchesRoutePattern(r.path, path),
  );
  if (!matched) return resp;
  const entries = Object.entries(matched.headers);
  if (entries.length === 0) return resp;
  const headers = new Headers(resp.headers);
  for (const [name, value] of entries) headers.set(name, value);
  return new Response(NULL_BODY_STATUS.has(resp.status) ? null : resp.body, {
    status: resp.status,
    statusText: resp.statusText,
    headers,
  });
}

function parseRouteHeaders(raw: string | undefined): RouteHeaderRule[] {
  if (!raw) return [];
  if (routeHeaderCache?.raw === raw) return routeHeaderCache.rules;
  let rules: RouteHeaderRule[] = [];
  try {
    rules = validateRouteHeaderTable(raw);
  } catch (err) {
    // Malformed binding — serve without extra headers rather than 500 every
    // request. The Rust side serializes this, so a malformed value is a bug
    // there, and a dead site is a worse symptom than a missing header.
    //
    // R749-T1: this is the ONE place in the system where a declared policy is
    // still allowed not to run, and it stays that way deliberately — the Rust
    // origin refuses the start for the same input (`RouteHeaderTable::parse`),
    // so the strictness belongs at the producer, not on the last hop before a
    // user. What changed is that it is no longer silent, and no longer
    // *partial*: a table with one bad rule used to apply the other rules, so a
    // site looked configured while one path was not. All-or-nothing plus a log
    // line is the honest version of "we could not enforce this".
    //
    // R749-T5: the producer-side check that makes this posture defensible now
    // exists — `DomainConfig::validate_route_headers`
    // (`oss/yubaba/crates/cloud/src/config.rs`) fails `yah cloud apply` at
    // manifest load on anything `validateRouteHeaderTable` would throw on, so
    // reaching this catch means a hand-edited binding, not a normal deploy.
    console.error(
      `mesofact: ROUTE_HEADERS binding is malformed, serving with NO route headers — ${
        err instanceof Error ? err.message : String(err)
      }`,
    );
    rules = [];
  }
  routeHeaderCache = { raw, rules };
  return rules;
}

/**
 * Parse and fully validate a `ROUTE_HEADERS` table, throwing on anything this
 * Worker would otherwise have dropped (R749-T1).
 *
 * Exported so the producing side can fail a deploy rather than ship a table
 * whose broken half disappears at the edge — the build-validation direction
 * R749-F3 named. The Rust half of the same check is
 * `mesofact::route_headers::RouteHeaderTable::parse`; keep the two agreeing.
 */
export function validateRouteHeaderTable(raw: string): RouteHeaderRule[] {
  const v: unknown = JSON.parse(raw);
  if (!Array.isArray(v)) {
    throw new Error("expected a JSON array of {path, headers} rules");
  }
  return v.map((rule, i) => {
    if (!isRouteHeaderRule(rule)) {
      throw new Error(`rule ${i} is not a {path: string, headers: object}`);
    }
    if (rule.path === "") {
      throw new Error(
        `rule ${i} has an empty path — a rule that matches nothing (or everything, depending on who reads it) is not a policy`,
      );
    }
    assertSettableHeaders(rule, i);
    return rule;
  });
}

/**
 * R749-T5 — a rule must be *applicable*, not merely shaped like one.
 *
 * Shape validation alone let `"Cross Origin Opener Policy"` (spaces, not
 * hyphens — one keystroke away in the hand-written `.yah/domains/*.toml` this
 * table comes from) through to `applyRouteHeaders`, where `Headers.set` throws
 * inside the exported `fetch` and every request 500s. That is precisely the
 * dead site `parseRouteHeaders`'s catch exists to prevent, so the check belongs
 * on this side of it: the same input now degrades to serve-without-headers plus
 * a log line, and the Rust origin still refuses the start.
 *
 * A non-string value is the other divergence: `Headers.set` coerces `1` to
 * `"1"` and serves it, while `RouteHeaderTable::parse` refuses the table — one
 * door applying a header the other rejects is the failure this fixture set was
 * built to catch, so reject it here too.
 */
function assertSettableHeaders(rule: RouteHeaderRule, i: number): void {
  const probe = new Headers();
  for (const [name, value] of Object.entries(rule.headers)) {
    if (typeof value !== "string") {
      throw new Error(
        `rule ${i} declares ${JSON.stringify(name)} = ${JSON.stringify(value)}, which is not a string`,
      );
    }
    try {
      probe.set(name, value);
    } catch (err) {
      throw new Error(
        `rule ${i} declares ${JSON.stringify(name)} = ${JSON.stringify(value)}, which cannot be set as a response header — ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
    }
  }
}

function isRouteHeaderRule(v: unknown): v is RouteHeaderRule {
  if (!v || typeof v !== "object") return false;
  const r = v as RouteHeaderRule;
  return (
    typeof r.path === "string" &&
    !!r.headers &&
    typeof r.headers === "object" &&
    // An array passes `typeof === "object"` and would then be walked by its
    // indices, so `[["a","b"]]` becomes the settable header `0` at the edge
    // while the Rust side rejects the table outright.
    !Array.isArray(r.headers)
  );
}

/**
 * Match a domain-manifest route pattern against a request path.
 *
 * `"/*"` matches everything. `"/app/*"` matches `/app`, `/app/` and everything
 * below — segment-aware (so `/apple` stays out), the same rule SSR_PREFIXES
 * uses above, and deliberately including the BARE prefix: `/app` is the URL a
 * link points at, it resolves to `app/index.html` through the clean-URL rule,
 * and a header set that skipped it would miss the very document it exists for.
 * Any pattern without a trailing `*` is an exact path match.
 */
function matchesRoutePattern(pattern: string, path: string): boolean {
  if (!pattern.endsWith("*")) return path === pattern;
  const prefix = pattern.slice(0, -1).replace(/\/+$/, "");
  if (prefix === "") return true;
  return path === prefix || path.startsWith(prefix + "/");
}

function parseResilience(raw: string | undefined): ResilienceMap {
  if (!raw) return {};
  try {
    const v = JSON.parse(raw);
    return v && typeof v === "object" ? (v as ResilienceMap) : {};
  } catch {
    return {};
  }
}

// W173 segment-aware match: pick the longest matching prefix.
function policyFor(
  map: ResilienceMap,
  path: string,
): ResiliencePolicy | undefined {
  let best: { prefix: string; policy: ResiliencePolicy } | undefined;
  for (const [prefix, policy] of Object.entries(map)) {
    const matches =
      path === prefix ||
      path.startsWith(prefix.endsWith("/") ? prefix : prefix + "/");
    if (!matches) continue;
    if (!best || prefix.length > best.prefix.length) {
      best = { prefix, policy };
    }
  }
  return best?.policy;
}

// Proxy `request` to `targetUrl`, applying the route's resilience policy.
// On no policy: one attempt, no per-attempt timeout — today's behavior.
async function proxyWithResilience(
  request: Request,
  targetUrl: string,
  policy: ResiliencePolicy | undefined,
): Promise<Response> {
  const method = request.method;
  const hasBody = !["GET", "HEAD"].includes(method);

  // Buffer the body once so retries don't try to re-read a consumed stream.
  // ReadableStreams are one-shot; if we hand the same body to two fetches the
  // second call sees an empty body. Bodies are bounded by Worker request
  // limits (100MB) — buffering in memory is acceptable for retry budgets.
  let bodyBuf: ArrayBuffer | undefined;
  if (hasBody) {
    bodyBuf = await request.arrayBuffer();
  }

  const retry = policy?.retry;
  const attempts = Math.max(1, retry?.attempts ?? 1);
  const backoffMs = retry?.backoff_ms ?? [];
  const retryOn: RetryOn = retry?.retry_on ?? "connection";
  const timeoutMs = policy?.timeout_ms;
  const budgetMs = retry?.budget_ms;
  const start = Date.now();

  let lastErr: unknown;
  let lastResp: Response | undefined;

  for (let attempt = 0; attempt < attempts; attempt++) {
    if (attempt > 0) {
      const gap = backoffMs[attempt - 1] ?? 0;
      if (gap > 0) await sleep(gap);
    }
    if (budgetMs !== undefined && Date.now() - start >= budgetMs) {
      break;
    }
    const init: RequestInit = {
      method,
      headers: request.headers,
      body: hasBody ? bodyBuf : undefined,
      redirect: "follow",
    };
    const controller = timeoutMs !== undefined ? new AbortController() : undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (controller) {
      init.signal = controller.signal;
      timer = setTimeout(() => controller.abort(), timeoutMs);
    }
    try {
      const resp = await fetch(targetUrl, init);
      if (timer) clearTimeout(timer);
      // HTTP-level success — return verbatim unless policy retries on 5xx/any.
      if (!shouldRetryOnStatus(resp.status, retryOn)) {
        emitTelemetry(targetUrl, attempt + 1, "ok", Date.now() - start);
        return resp;
      }
      lastResp = resp;
      // Consume body so the connection can be released before retrying.
      try {
        await resp.arrayBuffer();
      } catch {
        // best-effort
      }
    } catch (err) {
      if (timer) clearTimeout(timer);
      lastErr = err;
      if (retryOn !== "connection" && retryOn !== "5xx" && retryOn !== "any") {
        break;
      }
      // Connection-level errors are always retryable when ANY retry policy is
      // declared — `retry_on: "5xx"` still retries connection failures (they
      // strictly subsume the 5xx case).
    }
  }

  const latency = Date.now() - start;
  if (lastResp) {
    emitTelemetry(targetUrl, attempts, "exhausted_5xx", latency);
    return lastResp;
  }
  emitTelemetry(targetUrl, attempts, "exhausted_connection", latency);
  return new Response(`upstream unreachable: ${stringifyErr(lastErr)}`, {
    status: 502,
    headers: { "Content-Type": "text/plain" },
  });
}

function shouldRetryOnStatus(status: number, retryOn: RetryOn): boolean {
  if (status < 400) return false;
  if (retryOn === "any") return status >= 400;
  if (retryOn === "5xx") return status >= 500;
  return false;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function stringifyErr(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return "unknown error";
}

// W181 v1 telemetry: emit one structured log per request. CF Workers picks up
// console.log; downstream is OTel export, deferred per W181 § "Deferred to v2".
function emitTelemetry(
  target: string,
  attempts: number,
  outcome: "ok" | "exhausted_connection" | "exhausted_5xx",
  latencyMs: number,
): void {
  console.log(
    JSON.stringify({
      kind: "mesofact.resilience",
      target,
      attempts,
      outcome,
      latency_ms: latencyMs,
    }),
  );
}
