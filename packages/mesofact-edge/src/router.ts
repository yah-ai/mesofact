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
//
// Manifest-driven behavior on a static miss (W270 §3):
//   * a path matching an instance-addressed (deferred) route resolves through
//     the pointer store — present → render-root bytes (immutable cache), deleted
//     → 410, absent → the manifest's error_routes.404 page;
//   * error_routes.{404,5xx} are honored (branded pages), replacing the old
//     hardcoded plaintext 404.

import {
  loadManifest,
  matchesDeferredRoute,
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

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
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

    // Fetch from asset origin — the common (build-time HTML/asset) hit.
    const assetResp = await fetch(`${env.ASSET_ORIGIN}/${key}`);
    if (assetResp.ok) {
      return assetResp;
    }

    // ── static miss ──────────────────────────────────────────────────────────
    // Only here (never on the static-hit fast path) do we consult the published
    // manifest, so static-heavy sites pay nothing for the manifest fetch.
    const manifest = await loadManifest(env.ASSET_ORIGIN);

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
    const lastSegment = key.slice(key.lastIndexOf("/") + 1);
    if (!lastSegment.includes(".")) {
      for (const candidate of [`${key}.html`, `${key}/index.html`]) {
        const cleanResp = await fetch(`${env.ASSET_ORIGIN}/${candidate}`);
        if (cleanResp.ok) {
          return cleanResp;
        }
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
  },
};

/**
 * Serve an instance-addressed route (`prerender: { deferred: true }`) by
 * resolving its pointer. The pointer key is the request path minus its leading
 * slash (`/c/abc123` → `c/abc123`); the publisher flips the same key. Present →
 * the render-root bytes with immutable cache headers (content-addressed);
 * deleted → 410; absent → the branded 404 page; malformed record → 5xx.
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
    headers.set("Cache-Control", IMMUTABLE_CACHE_CONTROL);
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
 * Resolve a route path (`/404`, `/errors/nf`) to the ordered asset keys a
 * prerendered static route emits — the same clean-URL rule the static-serving
 * paths use: extensionless routes try `<rel>.html` and `<rel>/index.html`.
 */
function routeToAssetKeys(routePath: string): string[] {
  const rel = routePath.replace(/^\/+/, "");
  if (rel === "") return ["index.html"];
  const last = rel.split("/").pop() ?? "";
  if (last.includes(".")) return [rel];
  return [rel, `${rel}.html`, `${rel}/index.html`];
}

function defaultStatusText(status: number): string {
  if (status === 410) return "Gone";
  if (status >= 500) return "Internal Server Error";
  return "Not Found";
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
