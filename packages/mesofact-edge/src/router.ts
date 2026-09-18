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
//                              trailing slash); the catch-all for a URL NO
//                              table entry claims. May be empty when every
//                              static entry reads a bucket (→ unclaimed 404)
//   R2_<BUCKET>              — R2 bucket bindings, one per bucket a `static`
//                              entry names by `binding` (R560-F13)
//   POINTER_ORIGIN           — base URL the pointer store is read from
//                              (`p/<key>` objects). Defaults to ASSET_ORIGIN
//                              (pointers live under `p/` in the same bucket);
//                              kept distinct so a future consumer can front the
//                              (uncached) pointer reads separately.
//   WORKER_MODE              — "static" | "spa" | "ssr". Selects what a static
//                              MISS becomes — a branded 404, or the SPA/SSR
//                              shell. It no longer selects a ROUTE.
//   SSR_RESILIENCE           — JSON `{ [prefix]: ResiliencePolicy }` (W181 v1);
//                              optional; absent/invalid → one attempt, no timeout
//   ROUTE_TABLE              — JSON `[{ path, mode, origin?, rewrite?, target?,
//                              status?, headers?, auth? }]`, the domain's
//                              compiled route table in manifest order. The
//                              producer is `RouteTable::to_json` in
//                              oss/yubaba/crates/cloud/src/route_table.rs.
//                              Optional; absent → every path is a static one.
//
// ROUTING IS THE TABLE, AND ONLY THE TABLE (R898-F3 / W348 §2.2). This Worker
// carried four hardcoded prefix seams until R898-F3 — `ISSUES_ORIGIN`
// intercepting /api/issues*, `MESOFACT_BACKEND_ORIGIN` intercepting
// /api/releases*, `SSR_PREFIXES` proxying page routes, and `UPLOAD_ORIGIN`
// claiming /uploads/* — each one an `if` block plus a binding, so a fifth
// prefix meant a fifth of both, in a file the domain manifest could not reach.
// They are ordinary table entries now. DO NOT ADD A FIFTH: a prefix that needs
// its own origin is an entry, and if the table cannot express what you need,
// widen the table (`ResolvedRouteMode`) rather than reaching around it.
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
  WORKER_MODE: string;
  SSR_RESILIENCE?: string;
  ROUTE_TABLE?: string;
  /** The R2 bucket bindings `static` entries name by `binding` (R560-F13). */
  [binding: string]: unknown;
}

/** How a matched path is rewritten before it reaches its origin — mirrors
 *  `RouteRewrite` in `oss/yubaba/crates/cloud/src/route_table.rs`.
 *
 *  Both halves travel on the wire so this door never re-derives a prefix from
 *  a pattern: `/api/issues/42` under `{from: "/api/issues", to: "/issues"}`
 *  reaches the origin as `/issues/42`. */
interface RouteRewrite {
  from: string;
  to: string;
}

/** One entry of the `ROUTE_TABLE` — mirrors `RouteTableEntry` in
 *  `oss/yubaba/crates/cloud/src/route_table.rs`, whose `mode` tag is flattened
 *  into the object.
 *
 *  A `static` entry is served from ITS OWN source (R560-F13): the R2 bucket
 *  bound as `binding` when it names one — the Rust side's
 *  `ResolvedRouteMode::StaticBucket`, keyed by the request path minus its
 *  leading slash, unchanged — otherwise its `origin`. `ASSET_ORIGIN` serves
 *  only a path no entry claims. One Worker can therefore front several
 *  buckets, and an alias-tier domain's second site no longer serves out of the
 *  first one's origin. */
interface RouteEntry {
  path: string;
  mode: "static" | "backend" | "redirect";
  component?: string;
  origin?: string;
  bucket?: string;
  binding?: string;
  rewrite?: RouteRewrite;
  target?: string;
  status?: number;
  headers?: Record<string, string>;
  auth?: string;
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

/** A read of one object by bucket key. A miss is a non-ok `Response`, never a
 *  throw, so every caller keeps the `resp.ok` shape it had over `fetch`. */
type AssetGet = (key: string) => Promise<Response>;

/** Where a static request's bytes come from — and the manifest, pointer
 *  records and error pages published beside them, which is why the whole
 *  static pipeline below runs over one of these rather than over a URL. */
interface AssetSource {
  get: AssetGet;
  /** `p/<key>` pointer records. The same source as `get` for an entry; the
   *  catch-all keeps its distinct `POINTER_ORIGIN` binding. */
  pointers: AssetGet;
}

function httpGet(origin: string): AssetGet {
  return (key) => fetch(`${origin}/${key}`);
}

/** An R2 binding as an `AssetGet`. The object's own HTTP metadata
 *  (content-type, cache-control, …) and its etag travel on the response, which
 *  is what an R2 custom domain would have served for the same key. */
function r2Get(bucket: R2Bucket): AssetGet {
  return async (key) => {
    const obj = await bucket.get(key);
    if (!obj) return new Response(null, { status: 404 });
    const headers = new Headers();
    obj.writeHttpMetadata(headers);
    headers.set("ETag", obj.httpEtag);
    return new Response(obj.body, { headers });
  };
}

/** The source the MATCHED entry serves from (R560-F13), or the `Response` that
 *  ends the request when it cannot be served at all.
 *
 *  A bucket entry whose binding this Worker was not deployed with fails CLOSED
 *  with a 502, like a backend entry with no origin: the producer derives the
 *  binding list from the very table it ships, so this is a hand-edited or
 *  half-deployed Worker, and falling through would serve the path out of the
 *  catch-all with a 200. */
function assetSource(
  env: Env,
  entry: RouteEntry | undefined,
): AssetSource | Response {
  if (entry?.mode === "static" && entry.binding !== undefined) {
    const bucket = env[entry.binding] as R2Bucket | undefined;
    if (!bucket || typeof bucket.get !== "function") {
      console.error(
        `mesofact: ROUTE_TABLE entry ${JSON.stringify(entry.path)} reads R2 binding ${JSON.stringify(entry.binding)}, which this Worker was not deployed with`,
      );
      return new Response("Bad Gateway", { status: 502 });
    }
    const get = r2Get(bucket);
    return { get, pointers: get };
  }
  if (entry?.mode === "static" && entry.origin) {
    const get = httpGet(entry.origin);
    return { get, pointers: get };
  }
  if (!env.ASSET_ORIGIN) {
    // Every static entry reads a bucket and none claimed this path.
    return new Response("Not Found", { status: 404 });
  }
  return {
    get: httpGet(env.ASSET_ORIGIN),
    pointers: httpGet(env.POINTER_ORIGIN || env.ASSET_ORIGIN),
  };
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    // ONE walk of the table decides both halves of this request: which origin
    // serves it, and which declared headers it carries. They were two
    // independent decisions before R898-F3 — four `if` blocks for routing and
    // a separate `ROUTE_HEADERS` match for headers — which is how a path could
    // be routed by one rule and headered by another.
    const path = new URL(request.url).pathname;
    let entry: RouteEntry | undefined;
    try {
      entry = matchRoute(env.ROUTE_TABLE, path);
    } catch (err) {
      return routingUnavailable(path, err);
    }

    // Headers are stamped onto whatever came back, rather than at each
    // `return` inside the router, and that is what makes the guarantee total:
    // a header a domain declares for a path applies to the asset hit, the
    // clean-URL hit, the SPA shell, the branded 404 and the proxied response
    // alike. A header set that only held on the happy path would be worse
    // than none for the case that motivated this (COOP/COEP: a document
    // served without them silently loses SharedArrayBuffer instead of failing
    // loudly).
    const resp = await route(request, env, entry);
    return applyRouteHeaders(resp, entry);
  },
};

/** The router proper. Every `return` here is post-processed by
 *  [`applyRouteHeaders`] in the exported `fetch` above.
 *
 *  `entry` is the ONE table entry that claimed this path, already matched.
 *  A `static` entry is served from its own source (`assetSource`); only
 *  `undefined` falls through to `ASSET_ORIGIN`, the catch-all. */
async function route(
  request: Request,
  env: Env,
  entry: RouteEntry | undefined,
): Promise<Response> {
  const url = new URL(request.url);
  const path = url.pathname;
  const resilience = parseResilience(env.SSR_RESILIENCE);

  if (entry?.mode === "redirect") {
    // `Response.redirect` demands an absolute URL; a manifest may declare a
    // path, and a declared redirect that throws at the edge is worse than one
    // the browser resolves relative to the request.
    return new Response(null, {
      status: entry.status ?? 308,
      headers: { Location: entry.target ?? "/" },
    });
  }

  if (entry?.mode === "backend") {
    if (!entry.origin) {
      // The producer refuses to compile an entry with no resolved origin, so
      // this is a hand-edited table. Failing the path is right: falling
      // through would serve the backend's path out of the asset bucket with a
      // 200, which is the silent failure the table exists to end.
      console.error(
        `mesofact: ROUTE_TABLE entry ${JSON.stringify(entry.path)} is mode=backend with no origin`,
      );
      return new Response("Bad Gateway", { status: 502 });
    }
    const target = entry.origin + rewritePath(entry, path) + url.search;
    return proxyWithResilience(request, target, policyFor(resilience, path));
  }

  // Static: served from the MATCHED entry's own source, not a Worker-wide one.
  const source = assetSource(env, entry);
  if (source instanceof Response) return source;

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
    manifest = await loadManifest(source.get);
    manifestLoaded = true;
    const pageRoot = buildPageRoot(manifest);
    if (pageRoot) {
      for (const candidate of assetCandidates(key)) {
        const resp = await source.get(`${pageRoot}/${candidate}`);
        if (resp.ok) {
          return routePage(resp, manifest, path);
        }
      }
    }
  }

  // Fetch from asset origin — the common (build-time HTML/asset) hit.
  const assetResp = await source.get(key);
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
    manifest = await loadManifest(source.get);
  }

  // Instance-addressed (deferred) route → resolve through the pointer store.
  if (matchesDeferredRoute(manifest, path)) {
    return serveInstance(source, path, manifest);
  }

  // Clean-URL resolution: an extensionless path (e.g. `/releases`) maps to
  // its prerendered static asset — try `<key>.html` then `<key>/index.html`,
  // the same convention the error-page resolver uses (routeToAssetKeys).
  // This is what lets build-time-static routes serve without a trailing
  // slash or explicit `.html`. Deferred/instance routes are handled above,
  // so they keep priority; assets that already carry an extension (fetched
  // verbatim on the fast path) never reach here.
  for (const candidate of assetCandidates(key).slice(1)) {
    const cleanResp = await source.get(candidate);
    if (cleanResp.ok) {
      return withDeclaredCache(cleanResp, manifest, path);
    }
  }

  // static → error page; spa/ssr → index.html shell (client-side routing).
  if (env.WORKER_MODE === "static") {
    return errorResponse(404, source.get, manifest?.error_routes);
  }
  const shellResp = await source.get("index.html");
  if (shellResp.ok) {
    return new Response(shellResp.body, {
      status: 200,
      headers: shellResp.headers,
    });
  }
  return errorResponse(404, source.get, manifest?.error_routes);
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
  source: AssetSource,
  path: string,
  manifest: EdgeManifest | null,
): Promise<Response> {
  const key = path.slice(1);

  let state;
  try {
    state = await resolvePointer(source.pointers, key);
  } catch (err) {
    if (err instanceof PointerMalformed) {
      return errorResponse(500, source.get, manifest?.error_routes);
    }
    throw err;
  }

  if (state.kind === "present") {
    const contentResp = await source.get(state.pointer.content_root);
    if (!contentResp.ok) {
      // Pointer names bytes that aren't there — treat as not found.
      return errorResponse(404, source.get, manifest?.error_routes);
    }
    const headers = new Headers(contentResp.headers);
    stampPageCache(headers, manifest, path, IMMUTABLE_CACHE_CONTROL);
    return new Response(contentResp.body, { status: 200, headers });
  }

  if (state.kind === "deleted") {
    // Published then unpublished — 410 Gone, distinct from a never-existed 404.
    return errorResponse(410, source.get, manifest?.error_routes, "410 Gone");
  }

  // absent
  return errorResponse(404, source.get, manifest?.error_routes);
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
  get: AssetGet,
  errorRoutes: EdgeErrorRoutes | undefined,
  fallbackText?: string,
): Promise<Response> {
  const brandedRoute = status >= 500 ? errorRoutes?.["5xx"] : errorRoutes?.["404"];
  const keys: string[] = [];
  if (brandedRoute) keys.push(...routeToAssetKeys(brandedRoute));
  if (status < 500) keys.push("404.html"); // legacy default when no error_routes
  for (const k of keys) {
    const resp = await get(k);
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

// ── the route table ─────────────────────────────────────────────────────────
//
// The domain manifest is the only place that knows about path routing, so it is
// also where a path's ORIGIN and its *response headers* belong — not in a
// `_headers` file (a Pages/Netlify convention no Worker reads) and not
// hardcoded here for one site's needs. `ROUTE_TABLE` carries the compiled table
// verbatim, in manifest order; the producer is `RouteTable::to_json` in
// oss/yubaba/crates/cloud/src/route_table.rs.
//
// FIRST MATCH WINS, with no merging across entries — the same rule the manifest
// already states ("vec order = match order, first match wins"). One path
// therefore has one origin and one header set, both decided where the route was
// decided.
//
// TWO POSTURES ON A BAD TABLE, AND THE SPLIT IS THE POINT (R898-F3 decision 2):
//
//   * A table that cannot be PARSED or whose entries are not shaped like
//     entries means this door does not know where anything goes. It fails the
//     request CLOSED, loudly (`routingUnavailable`). Serving everything from
//     the catch-all instead is the /api/releases silent-404 with extra steps —
//     the incident W348 exists to prevent. The Rust origin refuses to start on
//     the same input, so this is the Worker's analogue of that refusal.
//   * A header that cannot be SET is cosmetic by comparison and keeps R749-T1's
//     lenient posture: all headers are dropped (never a partial application,
//     which makes a site look configured while one path is not), one log line
//     is emitted, and ROUTING CONTINUES. A typo'd header name must not take a
//     page down — that was the dead site R749-T5 fixed, and reaching it at all
//     means a hand-edited binding, since `DomainConfig::validate_route_headers`
//     fails `yah cloud apply` on anything `validateRouteTable` throws on.

/** Statuses whose responses must carry a null body (constructing one with a
 *  body throws in workerd). */
const NULL_BODY_STATUS = new Set([101, 103, 204, 205, 304]);

/** Memoized parse of the `ROUTE_TABLE` binding — the value is fixed for the
 *  isolate's lifetime, so re-parsing it per request buys nothing. */
let routeTableCache: { raw: string; entries: RouteEntry[] } | undefined;

/** The entry that claims `path`, or `undefined` for a plain static request.
 *  Throws when the table itself cannot be trusted — see the posture note
 *  above; the caller turns that into a closed failure, never a fall-through. */
function matchRoute(
  raw: string | undefined,
  path: string,
): RouteEntry | undefined {
  return parseRouteTable(raw).find((e) => matchesRoutePattern(e.path, path));
}

/** `to + <the rest of the path>`, or the path unchanged when the entry
 *  declares no rewrite. `from` is the matched prefix, so slicing by its length
 *  is exact — `/api/issues/42` under `{from:"/api/issues",to:"/issues"}` is
 *  `/issues/42`, and the bare `/api/issues` is `/issues`. */
function rewritePath(entry: RouteEntry, path: string): string {
  const rw = entry.rewrite;
  if (!rw) return path;
  return rw.to + path.slice(rw.from.length);
}

/** A table this door cannot trust — 503, logged, and NOT served from the
 *  catch-all. See the posture note above for why this one is closed. */
function routingUnavailable(path: string, err: unknown): Response {
  console.error(
    `mesofact: ROUTE_TABLE binding is malformed — refusing to route ${path} rather than serving it from the catch-all — ${
      err instanceof Error ? err.message : String(err)
    }`,
  );
  return new Response("Service Unavailable: routing table is unreadable", {
    status: 503,
    headers: { "Content-Type": "text/plain" },
  });
}

function applyRouteHeaders(
  resp: Response,
  entry: RouteEntry | undefined,
): Response {
  const entries = Object.entries(entry?.headers ?? {});
  if (entries.length === 0) return resp;
  const headers = new Headers(resp.headers);
  for (const [name, value] of entries) headers.set(name, value);
  return new Response(NULL_BODY_STATUS.has(resp.status) ? null : resp.body, {
    status: resp.status,
    statusText: resp.statusText,
    headers,
  });
}

function parseRouteTable(raw: string | undefined): RouteEntry[] {
  if (!raw) return [];
  if (routeTableCache?.raw === raw) return routeTableCache.entries;
  // A structural failure propagates: the caller fails the request closed.
  // Only the header half is caught, and only to drop headers.
  let entries = validateRouteTable(raw);
  try {
    assertSettableHeaders(entries);
  } catch (err) {
    console.error(
      `mesofact: ROUTE_TABLE carries a header that cannot be applied, serving with NO route headers — ${
        err instanceof Error ? err.message : String(err)
      }`,
    );
    entries = entries.map(({ headers: _drop, ...rest }) => rest);
  }
  routeTableCache = { raw, entries };
  return entries;
}

/**
 * Parse and structurally validate a `ROUTE_TABLE`, throwing on anything this
 * Worker could not route from (R749-T1's all-or-nothing, widened past headers).
 *
 * Exported so the producing side can fail a deploy rather than ship a table
 * whose broken half disappears at the edge — the build-validation direction
 * R749-F3 named. The Rust half of the same check is
 * `mesofact::route_headers::RouteHeaderTable::parse` for the header column and
 * `DomainConfig::route_table` for the whole thing; keep them agreeing.
 */
export function validateRouteTable(raw: string): RouteEntry[] {
  const v: unknown = JSON.parse(raw);
  if (!Array.isArray(v)) {
    throw new Error("expected a JSON array of route entries");
  }
  return v.map((entry, i) => {
    if (!isRouteEntry(entry)) {
      throw new Error(
        `entry ${i} is not a {path: string, mode: "static"|"backend"|"redirect", headers?: object}`,
      );
    }
    if (entry.path === "") {
      throw new Error(
        `entry ${i} has an empty path — an entry that matches nothing (or everything, depending on who reads it) is not a route`,
      );
    }
    return entry;
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
function assertSettableHeaders(entries: RouteEntry[]): void {
  const probe = new Headers();
  entries.forEach((entry, i) => {
    for (const [name, value] of Object.entries(entry.headers ?? {})) {
      if (typeof value !== "string") {
        throw new Error(
          `entry ${i} declares ${JSON.stringify(name)} = ${JSON.stringify(value)}, which is not a string`,
        );
      }
      try {
        probe.set(name, value);
      } catch (err) {
        throw new Error(
          `entry ${i} declares ${JSON.stringify(name)} = ${JSON.stringify(value)}, which cannot be set as a response header — ${
            err instanceof Error ? err.message : String(err)
          }`,
        );
      }
    }
  });
}

function isRouteEntry(v: unknown): v is RouteEntry {
  if (!v || typeof v !== "object") return false;
  const e = v as RouteEntry;
  if (typeof e.path !== "string") return false;
  if (e.mode !== "static" && e.mode !== "backend" && e.mode !== "redirect") {
    return false;
  }
  // A static entry's source fields are optional, but a present one that is
  // not a string would read `env[<object>]` or fetch `[object Object]/key`.
  if (e.origin !== undefined && typeof e.origin !== "string") return false;
  if (e.binding !== undefined && typeof e.binding !== "string") return false;
  // `headers` is optional (the producer omits it when empty) but must be a
  // plain object when present. An array passes `typeof === "object"` and would
  // then be walked by its indices, so `[["a","b"]]` becomes the settable
  // header `0` at the edge while the Rust side rejects the table outright.
  if (e.headers !== undefined) {
    if (!e.headers || typeof e.headers !== "object" || Array.isArray(e.headers)) {
      return false;
    }
  }
  return true;
}

/**
 * Match a domain-manifest route pattern against a request path.
 *
 * `"/*"` matches everything. `"/app/*"` matches `/app`, `/app/` and everything
 * below — segment-aware (so `/apple` stays out), the same rule the SSR prefix
 * matcher used before it became table entries (W173: a naive
 * `path.startsWith(p)` proxies `/api/healthcheck` to an `/api/health` origin —
 * bytes match, segments don't), and deliberately including the BARE prefix:
 * `/app` is the URL a
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
