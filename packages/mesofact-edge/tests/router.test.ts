import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { Miniflare } from "miniflare";
import { readFileSync } from "node:fs";
import { join, dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const BUNDLE = join(__dirname, "../dist/router.bundle.js");

type Assets = Record<string, { body: string; type: string }>;

// One declared rule as the fixture tables spell it — `DomainConfig::route_headers_json`'s
// wire shape, which the stand-in producer below widens into a ROUTE_TABLE entry.
type RouteHeaderRule = {
  path: string;
  headers: Record<string, string>;
  cors_origins?: string[];
};

interface MfSetup {
  mf: Miniflare;
  port: number;
  stop: () => void;
}

// Serves every asset in the map by key. Doubles as the pointer origin — pointer
// records live under `p/<key>` and are ordinary objects in the same bucket.
function startAssetServer(assets: Assets): { port: number; stop: () => void } {
  const server = Bun.serve({
    port: 0,
    fetch(req) {
      const key = new URL(req.url).pathname.slice(1) || "index.html";
      const asset = assets[key];
      if (!asset) return new Response("not found", { status: 404 });
      return new Response(asset.body, {
        headers: { "Content-Type": asset.type },
      });
    },
  });
  return { port: server.port!, stop: () => server.stop(true) };
}

async function makeMf(cfg: {
  mode: "static" | "spa" | "ssr";
  assets: Assets;
  ssrOrigin?: string;
  ssrPrefixes?: string[];
  mesofactBackendOrigin?: string;
  issuesOrigin?: string;
  uploadOrigin?: string;
  routeHeaders?: RouteHeaderRule[];
  /** Escape hatch for the malformed-binding cases, which the typed fields
   *  cannot express. */
  rawRouteTable?: string;
}): Promise<MfSetup> {
  const { port, stop } = startAssetServer(cfg.assets);
  const assetOrigin = `http://localhost:${port}`;
  const bindings: Record<string, string> = {
    ASSET_ORIGIN: assetOrigin,
    WORKER_MODE: cfg.mode,
    ROUTE_TABLE: cfg.rawRouteTable ?? JSON.stringify(routeTable(cfg, assetOrigin)),
  };
  const mf = new Miniflare({ modules: true, scriptPath: BUNDLE, bindings });
  return { mf, port, stop };
}

/**
 * The fixture's stand-in for the producer — it composes the same table
 * `worker_route_table_json` (oss/yubaba/crates/cloud/src/reconciler/mesofact_static.rs)
 * emits from a mirror's slot fields plus the domain manifest, IN THE SAME
 * ORDER, because order is the contract: the interception entries precede the
 * declared ones, which is the precedence the four deleted `if` blocks had
 * (backends over SSR, SSR over uploads, everything over the static catch-all).
 *
 * Keeping the per-seam `cfg` fields is deliberate: the behaviour tests below
 * were written against those seams and must go on asserting exactly what they
 * asserted before R898-F3 — only the way the config reaches the Worker moved.
 */
function routeTable(
  cfg: {
    ssrOrigin?: string;
    ssrPrefixes?: string[];
    mesofactBackendOrigin?: string;
    issuesOrigin?: string;
    uploadOrigin?: string;
    routeHeaders?: RouteHeaderRule[];
  },
  assetOrigin: string,
): unknown[] {
  const entries: unknown[] = [];
  if (cfg.issuesOrigin) {
    entries.push({
      path: "/api/issues*",
      mode: "backend",
      origin: cfg.issuesOrigin,
      rewrite: { from: "/api/issues", to: "/issues" },
    });
  }
  if (cfg.mesofactBackendOrigin) {
    entries.push({
      path: "/api/releases*",
      mode: "backend",
      origin: cfg.mesofactBackendOrigin,
      rewrite: { from: "/api/releases", to: "/releases" },
    });
  }
  if (cfg.ssrOrigin) {
    for (const prefix of cfg.ssrPrefixes ?? []) {
      entries.push({ path: `${prefix}*`, mode: "backend", origin: cfg.ssrOrigin });
    }
  }
  if (cfg.uploadOrigin) {
    entries.push({
      path: "/uploads/*",
      mode: "backend",
      origin: cfg.uploadOrigin,
    });
  }
  for (const rule of cfg.routeHeaders ?? []) {
    entries.push({
      path: rule.path,
      mode: "static",
      origin: assetOrigin,
      headers: rule.headers,
      ...(rule.cors_origins ? { cors_origins: rule.cors_origins } : {}),
    });
  }
  return entries;
}

// ── static mode ──────────────────────────────────────────────────────────────

describe("static mode", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: {
        "index.html": { body: "<h1>hello</h1>", type: "text/html" },
        "app.js": { body: "console.log('hi')", type: "application/javascript" },
        "releases.html": { body: "<h1>releases</h1>", type: "text/html" },
        "404.html": { body: "<h1>not found</h1>", type: "text/html" },
      },
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("/ maps to index.html", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("hello");
  });

  test("trailing-slash directory resolves to index.html", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/subdir/");
    // subdir/index.html not in assets → 404 with 404.html body
    expect(resp.status).toBe(404);
    expect(await resp.text()).toContain("not found");
  });

  test("known asset served directly", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/app.js");
    expect(resp.status).toBe(200);
  });

  test("clean URL resolves to prerendered .html (extensionless route)", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/releases");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("releases");
  });

  test("unknown path returns 404 with 404.html body", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/nope.html");
    expect(resp.status).toBe(404);
    expect(await resp.text()).toContain("not found");
  });

  test("unknown path returns 404 plain when 404.html absent", async () => {
    // Use a fresh mf without a 404.html asset
    const no404 = await makeMf({ mode: "static", assets: {} });
    const resp = await no404.mf.dispatchFetch("http://w.test/nope");
    expect(resp.status).toBe(404);
    expect(await resp.text()).toBe("Not Found");
    await no404.mf.dispose();
    no404.stop();
  });
});

// ── /uploads/ prefix routing (R490-T8) ──────────────────────────────────────

describe("uploads prefix", () => {
  // R898-F3: the reserved upload seam (R490-T8) is a TABLE ENTRY now, not a
  // binding plus an `if`. "Unset" therefore means "no /uploads/* entry", and
  // the path falls to the static tier — which 404s it, since no upload key is
  // a published asset. The old seam short-circuited to a hand-built 404 to
  // keep the SPA shell out of an upload miss; a matched backend entry never
  // falls back to static serving either, so that invariant is unchanged.
  test("/uploads/* 404s when no upload entry is declared", async () => {
    const setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hi</h1>", type: "text/html" } },
    });
    const resp = await setup.mf.dispatchFetch("http://w.test/uploads/pic.png");
    expect(resp.status).toBe(404);
    expect(await resp.text()).toBe("Not Found");
    await setup.mf.dispose();
    setup.stop();
  });

  test("/uploads/* routes to the upload origin when declared", async () => {
    const uploads = Bun.serve({
      port: 0,
      fetch(req) {
        const key = new URL(req.url).pathname.slice(1);
        if (key === "uploads/pic.png") {
          return new Response("UPLOADBYTES", { status: 200 });
        }
        return new Response("not found", { status: 404 });
      },
    });
    const setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hi</h1>", type: "text/html" } },
      uploadOrigin: `http://localhost:${uploads.port}`,
    });
    const hit = await setup.mf.dispatchFetch("http://w.test/uploads/pic.png");
    expect(hit.status).toBe(200);
    expect(await hit.text()).toBe("UPLOADBYTES");
    // The upstream's own 404 now reaches the client verbatim, where the old
    // seam replaced any non-ok response with a hand-built "Not Found". The
    // property that matters is unchanged and asserted: a miss is a REAL 404
    // and never the static site's shell or 404.html.
    const miss = await setup.mf.dispatchFetch("http://w.test/uploads/absent.png");
    expect(miss.status).toBe(404);
    expect(await miss.text()).not.toContain("<h1>hi</h1>");
    await setup.mf.dispose();
    setup.stop();
    uploads.stop(true);
  });

  test("non-/uploads/ paths still served from ASSET_ORIGIN unchanged", async () => {
    const setup = await makeMf({
      mode: "static",
      assets: {
        "index.html": { body: "<h1>hi</h1>", type: "text/html" },
        "illustrations/x.webp": { body: "WEBP", type: "image/webp" },
      },
      uploadOrigin: "http://localhost:1",
    });
    const resp = await setup.mf.dispatchFetch(
      "http://w.test/illustrations/x.webp",
    );
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("WEBP");
    await setup.mf.dispose();
    setup.stop();
  });
});

// ── SPA mode ─────────────────────────────────────────────────────────────────

describe("spa mode", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "spa",
      assets: {
        "index.html": { body: "<h1>spa shell</h1>", type: "text/html" },
        "app.js": { body: "console.log('spa')", type: "application/javascript" },
      },
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("/ serves index.html", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("spa shell");
  });

  test("unknown deep path falls back to index.html (client-side routing)", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/deep/route");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("spa shell");
  });

  test("known asset served directly without fallback", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/app.js");
    expect(resp.status).toBe(200);
  });
});

// ── SSR mode ──────────────────────────────────────────────────────────────────

describe("ssr mode", () => {
  let setup: MfSetup;
  let ssrPort: number;
  let stopSsr: () => void;

  beforeAll(async () => {
    const ssr = Bun.serve({
      port: 0,
      fetch(req) {
        const path = new URL(req.url).pathname;
        return new Response(`ssr:${path}`, { status: 200 });
      },
    });
    ssrPort = ssr.port!;
    stopSsr = () => ssr.stop(true);

    setup = await makeMf({
      mode: "ssr",
      assets: {
        "index.html": { body: "<h1>ssr shell</h1>", type: "text/html" },
      },
      ssrOrigin: `http://localhost:${ssrPort}`,
      ssrPrefixes: ["/api/", "/rpc/"],
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
    stopSsr();
  });

  test("/api/ prefix proxied to SSR origin", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/data");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("ssr:/api/data");
  });

  test("/rpc/ prefix proxied to SSR origin", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/rpc/call");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("ssr:/rpc/call");
  });

  test("non-prefixed paths fall back to index.html shell", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/some-page");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("ssr shell");
  });
});

// ── SSR matcher: W173 segment-aware boundary ─────────────────────────────────

describe("ssr segment-aware matcher (W173)", () => {
  let setup: MfSetup;
  let ssrPort: number;
  let stopSsr: () => void;

  beforeAll(async () => {
    const ssr = Bun.serve({
      port: 0,
      fetch(req) {
        const path = new URL(req.url).pathname;
        return new Response(`ssr:${path}`, { status: 200 });
      },
    });
    ssrPort = ssr.port!;
    stopSsr = () => ssr.stop(true);

    setup = await makeMf({
      mode: "ssr",
      assets: {
        "index.html": { body: "<h1>shell</h1>", type: "text/html" },
        "api/healthcheck.html": { body: "<p>sibling</p>", type: "text/html" },
      },
      ssrOrigin: `http://localhost:${ssrPort}`,
      ssrPrefixes: ["/api/health"],
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
    stopSsr();
  });

  test("/api/health matches exactly and proxies to SSR origin", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/health");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("ssr:/api/health");
  });

  test("/api/health/sub matches descendant segment and proxies", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/health/sub");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("ssr:/api/health/sub");
  });

  test("/api/healthcheck does NOT proxy (segment boundary)", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/healthcheck");
    expect(await resp.text()).not.toContain("ssr:");
  });
});

describe("ssr matcher with trailing-slash prefix (parametric route)", () => {
  let setup: MfSetup;
  let ssrPort: number;
  let stopSsr: () => void;

  beforeAll(async () => {
    const ssr = Bun.serve({
      port: 0,
      fetch(req) {
        const path = new URL(req.url).pathname;
        return new Response(`ssr:${path}`, { status: 200 });
      },
    });
    ssrPort = ssr.port!;
    stopSsr = () => ssr.stop(true);

    setup = await makeMf({
      mode: "ssr",
      assets: { "index.html": { body: "<h1>shell</h1>", type: "text/html" } },
      ssrOrigin: `http://localhost:${ssrPort}`,
      ssrPrefixes: ["/api/users/"],
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
    stopSsr();
  });

  test("/api/users/42 proxies to origin", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/users/42");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("ssr:/api/users/42");
  });

  test("/api/usersx does NOT proxy (trailing slash boundary)", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/usersx");
    expect(await resp.text()).not.toContain("ssr:");
  });
});

// ── backend API routing (R455-T4) ────────────────────────────────────────────

describe("backend API routing — ISSUES_ORIGIN", () => {
  let setup: MfSetup;
  let issuesPort: number;
  let stopIssues: () => void;

  beforeAll(async () => {
    const issues = Bun.serve({
      port: 0,
      fetch(req) {
        const path = new URL(req.url).pathname;
        return new Response(`issues:${req.method}:${path}`, { status: 201 });
      },
    });
    issuesPort = issues.port!;
    stopIssues = () => issues.stop(true);

    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>home</h1>", type: "text/html" } },
      issuesOrigin: `http://localhost:${issuesPort}`,
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
    stopIssues();
  });

  test("POST /api/issues proxied to ISSUES_ORIGIN/issues", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/issues", {
      method: "POST",
      body: JSON.stringify({ title: "test" }),
      headers: { "Content-Type": "application/json" },
    });
    expect(resp.status).toBe(201);
    expect(await resp.text()).toBe("issues:POST:/issues");
  });

  test("GET /api/issues/123 proxied to ISSUES_ORIGIN/issues/123", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/issues/123");
    expect(resp.status).toBe(201);
    expect(await resp.text()).toBe("issues:GET:/issues/123");
  });

  test("/api/issues routing absent when ISSUES_ORIGIN not set", async () => {
    const noBackend = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>home</h1>", type: "text/html" } },
    });
    const resp = await noBackend.mf.dispatchFetch("http://w.test/api/issues", {
      method: "POST",
      body: "{}",
      headers: { "Content-Type": "application/json" },
    });
    expect(resp.status).not.toBe(201);
    await noBackend.mf.dispose();
    noBackend.stop();
  });
});

describe("backend API routing — MESOFACT_BACKEND_ORIGIN", () => {
  let setup: MfSetup;
  let backendPort: number;
  let stopBackend: () => void;

  beforeAll(async () => {
    const backend = Bun.serve({
      port: 0,
      fetch(req) {
        const path = new URL(req.url).pathname;
        return new Response(`backend:${req.method}:${path}`, { status: 200 });
      },
    });
    backendPort = backend.port!;
    stopBackend = () => backend.stop(true);

    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>home</h1>", type: "text/html" } },
      mesofactBackendOrigin: `http://localhost:${backendPort}`,
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
    stopBackend();
  });

  test("GET /api/releases proxied to MESOFACT_BACKEND_ORIGIN/releases", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/releases");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("backend:GET:/releases");
  });

  test("GET /api/releases/v1.2.3 proxied with sub-path", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/releases/v1.2.3");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("backend:GET:/releases/v1.2.3");
  });

  test("ISSUES_ORIGIN takes priority over SSR for /api/issues", async () => {
    const ssr = Bun.serve({
      port: 0,
      fetch() {
        return new Response("ssr-hit", { status: 200 });
      },
    });
    const mixedSetup = await makeMf({
      mode: "ssr",
      assets: { "index.html": { body: "<h1>shell</h1>", type: "text/html" } },
      ssrOrigin: `http://localhost:${ssr.port}`,
      ssrPrefixes: ["/api/issues"],
      issuesOrigin: `http://localhost:${backendPort}`,
    });
    const resp = await mixedSetup.mf.dispatchFetch("http://w.test/api/issues", {
      method: "POST",
      body: "{}",
      headers: { "Content-Type": "application/json" },
    });
    const body = await resp.text();
    expect(body).not.toContain("ssr-hit");
    expect(body).toContain("backend:POST:");
    await mixedSetup.mf.dispose();
    mixedSetup.stop();
    ssr.stop(true);
  });
});

// ── manifest-driven: instance-addressed routes + error_routes (W270 §3) ──────

describe("instance-addressed (deferred) routes + error_routes", () => {
  // A published-chat-shaped site: a `static` route `/c/:slug` with
  // `prerender: { deferred: true }` is served through the pointer store, and
  // error_routes point at branded pages.
  // error_routes values are ROUTE PATHS: /404 and /5xx are static routes whose
  // prerendered output lands at 404.html / 5xx.html (clean-URL resolution).
  const manifest = {
    version: "1",
    build_id: "b1",
    routes: [
      { route: "/c/:slug", mode: "static", prerender: { deferred: true } },
      { route: "/404", mode: "static" },
      { route: "/5xx", mode: "static" },
    ],
    error_routes: { "404": "/404", "5xx": "/5xx" },
  };
  const baseAssets: Assets = {
    "index.html": { body: "<h1>home</h1>", type: "text/html" },
    "manifest.json": { body: JSON.stringify(manifest), type: "application/json" },
    "404.html": { body: "<h1>branded 404</h1>", type: "text/html" },
    "5xx.html": { body: "<h1>branded 5xx</h1>", type: "text/html" },
    // A live instance: pointer `p/c/live` → render-root `html/c_live.html`.
    "p/c/live": {
      body: JSON.stringify({ v: 1, pointer: { content_root: "html/c_live.html" } }),
      type: "application/json",
    },
    "html/c_live.html": { body: "<h1>chat live</h1>", type: "text/html" },
    // A tombstoned instance: pointer record with no `pointer` (deleted).
    "p/c/gone": {
      body: JSON.stringify({ v: 1, deleted_at: "2026-07-14T00:00:00Z" }),
      type: "application/json",
    },
    // A record from a future edge (unknown version) → 5xx.
    "p/c/badver": {
      body: JSON.stringify({ v: 99, pointer: { content_root: "x" } }),
      type: "application/json",
    },
    // A pointer under a path that is NOT a valid deferred-route match (`/c/a/b`
    // is two segments; `/c/:slug` is one param) — must never be served.
    "p/c/a/b": {
      body: JSON.stringify({ v: 1, pointer: { content_root: "html/c_live.html" } }),
      type: "application/json",
    },
  };

  let setup: MfSetup;
  beforeAll(async () => {
    setup = await makeMf({ mode: "static", assets: baseAssets });
  });
  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("present pointer → render-root bytes with immutable cache headers", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/c/live");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("chat live");
    expect(resp.headers.get("cache-control")).toContain("immutable");
  });

  test("deleted pointer → 410 Gone", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/c/gone");
    expect(resp.status).toBe(410);
  });

  test("absent pointer → 404 with branded error_routes.404 page", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/c/never");
    expect(resp.status).toBe(404);
    expect(await resp.text()).toContain("branded 404");
  });

  test("malformed pointer (unknown version) → 500 with branded error_routes.5xx", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/c/badver");
    expect(resp.status).toBe(500);
    expect(await resp.text()).toContain("branded 5xx");
  });

  test("non-matching path is NOT served as an instance even if a pointer exists", async () => {
    // `/c/a/b` does not match `/c/:slug` (segment count) → ordinary 404, the
    // p/c/a/b pointer is never consulted.
    const resp = await setup.mf.dispatchFetch("http://w.test/c/a/b");
    expect(resp.status).toBe(404);
    expect(await resp.text()).toContain("branded 404");
  });

  test("non-deferred unknown path honors error_routes.404", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/random-page");
    expect(resp.status).toBe(404);
    expect(await resp.text()).toContain("branded 404");
  });
});

// ── build-pointer resolution (yah R330-B44) ─────────────────────────────────
//
// A published prefix carries BOTH layouts at once: the immutable `<build_id>/`
// tree the current publisher writes, and the flat copy older publishers wrote
// and never swept. These pin that the pointer wins, because the whole point of
// a revalidate is to publish a new build tree and flip `build_id` — an edge
// that prefers the flat copy makes every push a silent no-op.

const BUILD = "2026-08-10T21-24-08Z";

function manifestAsset(m: unknown) {
  return { body: JSON.stringify(m), type: "application/json" };
}

describe("build-pointer resolution", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: {
        "manifest.json": manifestAsset({
          build_id: BUILD,
          routes: [{ route: "/releases" }, { route: "/" }],
        }),
        // Flat copies — what the site served before the pointer moved.
        "index.html": { body: "<h1>stale home</h1>", type: "text/html" },
        "releases.html": { body: "<h1>stale 0.8.21</h1>", type: "text/html" },
        "404.html": { body: "<h1>not found</h1>", type: "text/html" },
        "app.chunk-abc123.js": {
          body: "console.log('asset')",
          type: "application/javascript",
        },
        // The build tree the manifest points at.
        [`${BUILD}/html/index.html`]: {
          body: "<h1>fresh home</h1>",
          type: "text/html",
        },
        [`${BUILD}/html/releases.html`]: {
          body: "<h1>fresh 0.8.22</h1>",
          type: "text/html",
        },
      },
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("clean URL serves the build tree, not the stale flat copy", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/releases");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("fresh 0.8.22");
  });

  test("/ serves the build tree's index", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("fresh home");
  });

  test("an explicit .html request also resolves through the pointer", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/releases.html");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("fresh 0.8.22");
  });

  test("a build-tree page is served no-cache, not the object's own max-age", async () => {
    // The object is immutable at `<build_id>/…` and mutable at `/releases`;
    // passing its header through would pin a client to one release.
    const resp = await setup.mf.dispatchFetch("http://w.test/releases");
    expect(resp.headers.get("cache-control")).toBe("no-cache");
  });

  test("a page absent from the build tree falls back to the flat copy", async () => {
    // Publishers that predate the build tree wrote flat keys only; a mixed
    // prefix must keep serving them rather than 404ing the difference.
    const mixed = await makeMf({
      mode: "static",
      assets: {
        "manifest.json": manifestAsset({ build_id: BUILD, routes: [] }),
        "legacy.html": { body: "<h1>legacy page</h1>", type: "text/html" },
      },
    });
    const resp = await mixed.mf.dispatchFetch("http://w.test/legacy");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("legacy page");
    await mixed.mf.dispose();
    mixed.stop();
  });

  test("a manifest with no build_id behaves exactly as before", async () => {
    const noBuild = await makeMf({
      mode: "static",
      assets: {
        "manifest.json": manifestAsset({ routes: [] }),
        "releases.html": { body: "<h1>flat only</h1>", type: "text/html" },
      },
    });
    const resp = await noBuild.mf.dispatchFetch("http://w.test/releases");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("flat only");
    await noBuild.mf.dispose();
    noBuild.stop();
  });

  test("a site with no manifest at all still serves its flat assets", async () => {
    const bare = await makeMf({
      mode: "static",
      assets: { "releases.html": { body: "<h1>bare</h1>", type: "text/html" } },
    });
    const resp = await bare.mf.dispatchFetch("http://w.test/releases");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("bare");
    await bare.mf.dispose();
    bare.stop();
  });

  test("hashed assets bypass the pointer and keep their own headers", async () => {
    // They are content-addressed, so both layouts hold identical bytes and the
    // indirection would only cost a manifest fetch on the majority of requests.
    const resp = await setup.mf.dispatchFetch("http://w.test/app.chunk-abc123.js");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("cache-control")).not.toBe("no-cache");
  });
});

// R600-B1, serve half. The build half lives in
// packages/mesofact-build/tests/build.test.ts ("expands params from a declared
// data_inputs JSON file"), which pins every emission to `dist/html<url>.html`;
// this pins that the worker asks for exactly that key. The bug was that the two
// halves disagreed — the renderer named instances off the route PATTERN
// (`issues_id__<ulid>`), the worker resolves off the REQUEST PATH — so 14
// correctly-rendered, correctly-published pages sat at keys no request could
// produce. Neither half alone could catch it; keep both.
describe("parametric prerender instances (non-deferred)", () => {
  const ID = "01KZVGVT0DV61ZGGNVHAWQW2CS";
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: {
        "manifest.json": manifestAsset({
          build_id: BUILD,
          // A `{from_data, items_key, param}` prerender block carries no
          // `deferred` flag, so this route never touches the pointer store —
          // its instances are plain published assets.
          routes: [
            { route: "/issues" },
            {
              route: "/issues/:id",
              prerender: { from_data: "data/issues.json", items_key: "issues", param: "id" },
            },
          ],
        }),
        [`${BUILD}/html/issues.html`]: { body: "<h1>issue list</h1>", type: "text/html" },
        [`${BUILD}/html/issues/${ID}.html`]: {
          body: "<h1>issue detail</h1>",
          type: "text/html",
        },
        "404.html": { body: "<h1>not found</h1>", type: "text/html" },
      },
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("an instance page resolves from the build tree at its public path", async () => {
    const resp = await setup.mf.dispatchFetch(`http://w.test/issues/${ID}`);
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("issue detail");
  });

  test("the list route at the parent path is unshadowed by the instance dir", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/issues");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("issue list");
  });

  test("an id with no published instance is a branded 404, not a stale page", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/issues/nope");
    expect(resp.status).toBe(404);
    expect(await resp.text()).toContain("not found");
  });
});

// ── R746: per-route response headers ─────────────────────────────────────────
//
// The domain manifest's `[[routes]].headers` reach the Worker as the `headers`
// column of ROUTE_TABLE (R898-F3 folded the separate ROUTE_HEADERS binding into
// it) and are stamped onto the response. The motivating case is COOP/COEP on a wasm
// sub-app: without both headers the document loses `SharedArrayBuffer` silently,
// so "the header made it onto the bytes the browser actually got" is the only
// assertion that means anything — hence these go through miniflare rather than
// unit-testing the matcher.
//
// Before this existed the headers were declared in a `_headers` file, which is a
// Cloudflare Pages / Netlify convention nothing in this serving path reads.
const ISOLATION = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

describe("per-route response headers", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: {
        "index.html": { body: "<h1>marketing</h1>", type: "text/html" },
        "app/index.html": { body: "<h1>wasm app</h1>", type: "text/html" },
        "app/bundle.wasm": { body: "\0asm", type: "application/wasm" },
        "404.html": { body: "<h1>not found</h1>", type: "text/html" },
      },
      // Manifest order: the specific route sits above the catch-all, and the
      // catch-all declares a header of its own so "first match wins, no merge"
      // is observable rather than inferred.
      routeHeaders: [
        { path: "/app/*", headers: ISOLATION },
        { path: "/*", headers: { "X-Tier": "marketing" } },
      ],
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("the mounted sub-app's index carries both isolation headers", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/app/");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("wasm app");
    expect(resp.headers.get("Cross-Origin-Opener-Policy")).toBe("same-origin");
    expect(resp.headers.get("Cross-Origin-Embedder-Policy")).toBe("require-corp");
  });

  test("the bare prefix matches too — /app is the URL the CTA links to", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/app");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("wasm app");
    expect(resp.headers.get("Cross-Origin-Opener-Policy")).toBe("same-origin");
  });

  test("subresources under the prefix get them as well", async () => {
    // COEP require-corp is worth nothing if the wasm itself is served from a
    // document that is isolated but the fetch is not.
    const resp = await setup.mf.dispatchFetch("http://w.test/app/bundle.wasm");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("Cross-Origin-Embedder-Policy")).toBe("require-corp");
  });

  test("first match wins — the catch-all does not merge into the app route", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/app/");
    expect(resp.headers.get("X-Tier")).toBeNull();
  });

  test("the catch-all applies to paths the specific route misses", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("X-Tier")).toBe("marketing");
    expect(resp.headers.get("Cross-Origin-Opener-Policy")).toBeNull();
  });

  test("matching is segment-aware — /apple is not under /app/*", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/apple");
    expect(resp.headers.get("Cross-Origin-Opener-Policy")).toBeNull();
    expect(resp.headers.get("X-Tier")).toBe("marketing");
  });

  test("error responses carry the route's headers too", async () => {
    // A 404 under /app/* is still a document the isolated app may be showing.
    const resp = await setup.mf.dispatchFetch("http://w.test/app/missing.js");
    expect(resp.status).toBe(404);
    expect(resp.headers.get("Cross-Origin-Opener-Policy")).toBe("same-origin");
  });

  test("Content-Type and body survive the header rewrite", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/app/");
    expect(resp.headers.get("Content-Type")).toContain("text/html");
    expect(await resp.text()).toContain("wasm app");
  });
});

// ── R749-F3: front-door parity ───────────────────────────────────────────────
//
// The SAME fixture the sovereign path asserts in
// `crates/mesofact/tests/route_headers_parity.rs`. One description of what a
// domain's declared headers must do, asserted against both front doors, because
// the defect this exists to stop is the two doors disagreeing: until R749-F3
// only this Worker read the table, so flipping `front_door` from `worker` to
// `passway` silently dropped COOP/COEP — correct bytes, 200 OK, dead wasm app.
// See tests/fixtures/route-headers/README.md.
type ParityFixture = {
  table: RouteHeaderRule[];
  assets: Assets;
  cases: {
    name: string;
    path: string;
    status: number;
    expect: Record<string, string>;
    absent?: string[];
    /** Sent as the request's `Origin` (R826's `cors_origins` cases). */
    origin?: string;
  }[];
};

// `join(__dirname, …)` rather than `fileURLToPath(new URL(…))`: this package's
// tsconfig pulls in the DOM `URL`, which is not assignable to node's, so the
// URL form fails `bun run typecheck` while the test itself passes. A red
// typecheck nobody can act on costs the gate its signal.
const PARITY: ParityFixture = JSON.parse(
  readFileSync(join(__dirname, "../../../tests/fixtures/route-headers/parity.json"), "utf8"),
);

describe("route-header front-door parity fixture", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: PARITY.assets,
      routeHeaders: PARITY.table,
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  for (const c of PARITY.cases) {
    test(`${c.name} (${c.path})`, async () => {
      const resp = await setup.mf.dispatchFetch(`http://w.test${c.path}`, {
        headers: c.origin ? { Origin: c.origin } : {},
      });
      expect(resp.status).toBe(c.status);
      for (const [name, value] of Object.entries(c.expect)) {
        expect(resp.headers.get(name)).toBe(value);
      }
      for (const name of c.absent ?? []) {
        expect(resp.headers.get(name)).toBeNull();
      }
    });
  }
});

describe("no ROUTE_TABLE binding", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("responses pass through untouched", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("hello");
    expect(resp.headers.get("Cross-Origin-Opener-Policy")).toBeNull();
  });
});

describe("malformed ROUTE_TABLE binding", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
      rawRouteTable: "{not json",
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  // R898-F3 decision 2, and a DELIBERATE change of posture from R749-T1's
  // header-only leniency. A table that will not parse is not a missing
  // header — this door does not know where anything goes, and the catch-all
  // it would otherwise fall through to is exactly how /api/releases 404'd
  // silently while the site looked healthy. The Rust origin refuses to start
  // on the same input; failing the request closed is the Worker's analogue.
  test("fails the request closed instead of falling through to the catch-all", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(503);
    expect(await resp.text()).not.toContain("hello");
  });
});

// R749-T1. A table with one bad rule used to have its OTHER rules applied, so
// the site looked configured while one path silently was not. All-or-nothing
// still holds, and R898-F3 hardens it: an entry that is not shaped like an
// entry means the ROUTING is untrustworthy, not just the headers, so the
// request fails closed rather than serving with the half that parsed.
describe("partially malformed ROUTE_TABLE binding", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        { path: "/*", mode: "static", headers: { "X-Good": "1" } },
        { path: 42, mode: "static", headers: {} },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("applies none of the table and serves none of it", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(503);
    expect(resp.headers.get("X-Good")).toBeNull();
  });
});

// R749-T5. A shape-valid rule whose header cannot actually be SET used to pass
// validation and then throw inside the exported `fetch`, so the whole site
// 500'd — the dead site the catch above exists to prevent, reached by the most
// likely typo in the hand-written manifest that produces this table. The
// producer now refuses it at `yah cloud apply`
// (`DomainConfig::validate_route_headers`); the edge degrades if one gets past.
describe("ROUTE_TABLE entry whose header cannot be set", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        {
          path: "/*",
          mode: "static",
          headers: { "Cross Origin Opener Policy": "same-origin" },
        },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("serves without the headers instead of throwing out of fetch", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("hello");
    // Not asserted by reading the header back: `Headers.get` rejects the name
    // too, which is the whole reason the edge could not have applied it.
  });
});

// R826, the two hand-edited-binding postures for `cors_origins`. A list that is
// not an array of strings is mis-SHAPED — `includes` on a bare string is a
// substring test, so "https://noisetable.com.evil" would pass — and fails the
// request closed like any other malformed entry. A well-shaped list naming
// `null` (or `*`, or beside a literal ACAO) is a header-level fault: the table's
// headers AND lists are dropped, logged, and routing continues — which for a
// CORS grant fails closed by construction, since no grant means no read.
describe("ROUTE_TABLE entry whose cors_origins is not a string array", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        { path: "/*", mode: "static", cors_origins: "https://noisetable.com" },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("fails closed rather than substring-matching origins", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/", {
      headers: { Origin: "https://noisetable.com" },
    });
    expect(resp.status).toBe(503);
    expect(resp.headers.get("Access-Control-Allow-Origin")).toBeNull();
  });
});

describe("ROUTE_TABLE entry whose cors_origins grants null", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        {
          path: "/*",
          mode: "static",
          headers: { "X-Good": "1" },
          cors_origins: ["https://noisetable.com", "null"],
        },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("serves with no grant and no headers, to anyone", async () => {
    for (const origin of ["null", "https://noisetable.com"]) {
      const resp = await setup.mf.dispatchFetch("http://w.test/", {
        headers: { Origin: origin },
      });
      expect(resp.status).toBe(200);
      expect(resp.headers.get("Access-Control-Allow-Origin")).toBeNull();
      expect(resp.headers.get("X-Good")).toBeNull();
    }
  });
});

describe("ROUTE_TABLE entry with cors_origins beside a literal ACAO header", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        {
          path: "/*",
          mode: "static",
          headers: { "access-control-allow-origin": "https://noisetable.com" },
          cors_origins: ["https://staging.noisetable.com"],
        },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("applies neither answer", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/", {
      headers: { Origin: "https://staging.noisetable.com" },
    });
    expect(resp.status).toBe(200);
    expect(resp.headers.get("Access-Control-Allow-Origin")).toBeNull();
  });
});

// R826 on a proxied entry: the origin's own `Access-Control-Allow-Origin: *`
// does not survive a declared list, and its own `Vary` is merged into, not
// replaced.
describe("cors_origins on a backend entry whose origin sets its own CORS", () => {
  let setup: MfSetup;
  let backend: ReturnType<typeof Bun.serve>;

  beforeAll(async () => {
    backend = Bun.serve({
      port: 0,
      fetch() {
        return new Response("api", {
          headers: {
            "Access-Control-Allow-Origin": "*",
            Vary: "Accept-Encoding",
          },
        });
      },
    });
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        {
          path: "/api/*",
          mode: "backend",
          origin: `http://localhost:${backend.port}`,
          cors_origins: ["https://noisetable.com"],
        },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
    backend.stop(true);
  });

  test("an unlisted origin loses the upstream's wildcard grant", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/x", {
      headers: { Origin: "https://evil.example" },
    });
    expect(resp.status).toBe(200);
    expect(resp.headers.get("Access-Control-Allow-Origin")).toBeNull();
    expect(resp.headers.get("Vary")).toBe("Accept-Encoding, Origin");
  });

  test("a listed origin gets itself, not the wildcard", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/x", {
      headers: { Origin: "https://noisetable.com" },
    });
    expect(resp.headers.get("Access-Control-Allow-Origin")).toBe("https://noisetable.com");
  });
});

// A non-string value is where the two doors disagree in the DANGEROUS
// direction: `Headers.set` coerces it and serves the header, while
// `RouteHeaderTable::parse` refuses the table, so the same manifest is enforced
// at one front door and refused at the other.
describe("ROUTE_TABLE entry with a non-string header value", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>hello</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        { path: "/*", mode: "static", headers: { "X-Count": 1 } },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("applies nothing rather than serving a coerced value the origin rejects", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("X-Count")).toBeNull();
  });
});

// ── R898-F3: the table's own vocabulary ──────────────────────────────────────
//
// The two `/api/*` describes above prove the rewrite on the two seams that
// happen to exist today. These prove the MECHANISM — that a prefix swap is
// route data any entry can carry, which is what makes the next backend prefix
// a manifest edit instead of a fifth `if` block in this file.

describe("backend entry with a declared rewrite", () => {
  let setup: MfSetup;
  let stopOrigin: () => void;

  beforeAll(async () => {
    const origin = Bun.serve({
      port: 0,
      fetch(req) {
        const url = new URL(req.url);
        return new Response(`origin:${url.pathname}${url.search}`, { status: 200 });
      },
    });
    stopOrigin = () => origin.stop(true);
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>home</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        {
          path: "/docs/*",
          mode: "backend",
          origin: `http://localhost:${origin.port}`,
          rewrite: { from: "/docs", to: "/v2/documentation" },
        },
        {
          path: "/raw/*",
          mode: "backend",
          origin: `http://localhost:${origin.port}`,
        },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
    stopOrigin();
  });

  test("the sub-path survives the prefix swap", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/docs/guide/intro");
    expect(await resp.text()).toBe("origin:/v2/documentation/guide/intro");
  });

  test("the bare prefix rewrites too — it is the URL a link points at", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/docs");
    expect(await resp.text()).toBe("origin:/v2/documentation");
  });

  test("the query string is preserved across the rewrite", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/docs/x?q=1&r=2");
    expect(await resp.text()).toBe("origin:/v2/documentation/x?q=1&r=2");
  });

  test("an entry with no rewrite is an identity proxy", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/raw/thing");
    expect(await resp.text()).toBe("origin:/raw/thing");
  });

  test("a path no entry claims is still served from ASSET_ORIGIN", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("home");
  });
});

describe("redirect entry", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>home</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([
        { path: "/old", mode: "redirect", target: "/new", status: 301 },
        { path: "/moved", mode: "redirect", target: "https://elsewhere.test/" },
      ]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("emits the declared status and Location", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/old", {
      redirect: "manual",
    });
    expect(resp.status).toBe(301);
    expect(resp.headers.get("Location")).toBe("/new");
  });

  // 308 keeps the method, so a deprecated POST endpoint does not silently
  // become a GET at its new home — the reason the Rust default is 308.
  test("defaults to 308 when the entry declares no status", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/moved", {
      redirect: "manual",
    });
    expect(resp.status).toBe(308);
    expect(resp.headers.get("Location")).toBe("https://elsewhere.test/");
  });
});

// A backend entry the producer could never emit (it refuses an unresolved
// origin), reachable only by hand-editing the binding. It must not fall
// through to the asset bucket: a 200 of the wrong bytes is the failure mode
// this whole relay exists to remove.
// ── R560-F13: a static entry serves from ITS OWN source ─────────────────────
//
// One Worker fronting several R2 buckets (cdn.noisetable.com) plus a component
// entry with its own origin. Every bucket that could answer for a key is seeded
// with a decoy under that key, so a hit proves WHICH source answered.

describe("static entries serve from the matched entry's source", () => {
  let mf: Miniflare;
  let fallback: { port: number; stop: () => void };
  let docs: { port: number; stop: () => void };

  beforeAll(async () => {
    fallback = startAssetServer({
      "other.txt": { body: "from ASSET_ORIGIN", type: "text/plain" },
      "docs/guide.html": { body: "WRONG: the catch-all", type: "text/html" },
      "engine/dev/latest.txt": { body: "WRONG: the catch-all", type: "text/plain" },
    });
    docs = startAssetServer({
      "docs/guide.html": { body: "from the docs entry's origin", type: "text/html" },
    });
    const table = [
      {
        path: "/engine/*",
        mode: "static",
        bucket: "noisetable-releases",
        binding: "R2_NOISETABLE_RELEASES",
        auth: "anonymous",
      },
      {
        path: "/nt-cas/*",
        mode: "static",
        bucket: "noisetable-assets",
        binding: "R2_NOISETABLE_ASSETS",
        auth: "anonymous",
      },
      {
        path: "/docs/*",
        mode: "static",
        component: "handbook/site",
        origin: `http://localhost:${docs.port}`,
        auth: "anonymous",
      },
      {
        path: "/unbound/*",
        mode: "static",
        bucket: "nowhere",
        binding: "R2_NOWHERE",
        auth: "anonymous",
      },
    ];
    mf = new Miniflare({
      modules: true,
      scriptPath: BUNDLE,
      bindings: {
        ASSET_ORIGIN: `http://localhost:${fallback.port}`,
        WORKER_MODE: "static",
        ROUTE_TABLE: JSON.stringify(table),
      },
      r2Buckets: ["R2_NOISETABLE_RELEASES", "R2_NOISETABLE_ASSETS"],
    });
    const releases = await mf.getR2Bucket("R2_NOISETABLE_RELEASES");
    await releases.put("engine/dev/latest.txt", "0.9.1", {
      httpMetadata: { contentType: "text/plain" },
    });
    await releases.put("engine/index.html", "<h1>engine</h1>", {
      httpMetadata: { contentType: "text/html" },
    });
    await releases.put("nt-cas/ab/cd", "WRONG: the releases bucket");
    const assets = await mf.getR2Bucket("R2_NOISETABLE_ASSETS");
    await assets.put("nt-cas/ab/cd", "cas blob", {
      httpMetadata: {
        contentType: "application/octet-stream",
        cacheControl: "public, max-age=31536000, immutable",
      },
    });
  });

  afterAll(async () => {
    await mf.dispose();
    fallback.stop();
    docs.stop();
  });

  test("a bucket entry reads its binding, keyed by the request path unchanged", async () => {
    const resp = await mf.dispatchFetch("http://w.test/engine/dev/latest.txt");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("0.9.1");
    expect(resp.headers.get("content-type")).toBe("text/plain");
    expect(resp.headers.get("etag")).toBeTruthy();
  });

  test("each bucket entry reads its OWN bucket, with the object's metadata", async () => {
    const resp = await mf.dispatchFetch("http://w.test/nt-cas/ab/cd");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("cas blob");
    expect(resp.headers.get("cache-control")).toBe(
      "public, max-age=31536000, immutable",
    );
  });

  test("a directory path under a bucket entry resolves index.html in that bucket", async () => {
    const resp = await mf.dispatchFetch("http://w.test/engine/");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toContain("engine");
  });

  test("a key the bucket lacks is a 404, never a fall-through to ASSET_ORIGIN", async () => {
    const resp = await mf.dispatchFetch("http://w.test/engine/nope.bin");
    expect(resp.status).toBe(404);
  });

  test("a component entry serves from its own origin, not ASSET_ORIGIN", async () => {
    const resp = await mf.dispatchFetch("http://w.test/docs/guide.html");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("from the docs entry's origin");
  });

  test("a path no entry claims still falls back to ASSET_ORIGIN", async () => {
    const resp = await mf.dispatchFetch("http://w.test/other.txt");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("from ASSET_ORIGIN");
  });

  test("an entry naming a binding the Worker lacks fails closed with 502", async () => {
    const resp = await mf.dispatchFetch("http://w.test/unbound/x.txt");
    expect(resp.status).toBe(502);
  });
});

// ── MFT-R825-F2: bucket reads go through the zone's edge cache ──────────────
//
// A hit is proved by CONTENT, not by a header: after the first GET the object
// is overwritten in R2, so only a cached copy can still answer with the old
// bytes. The cache fills under `ctx.waitUntil`, so a hit is polled for rather
// than expected on the very next request.

describe("bucket entries serve cacheable objects through caches.default", () => {
  let mf: Miniflare;
  let bucket: Awaited<ReturnType<Miniflare["getR2Bucket"]>>;
  const IMMUTABLE = "public, max-age=31536000, immutable";

  beforeAll(async () => {
    const table = [
      {
        path: "/*",
        mode: "static",
        bucket: "cdn",
        binding: "R2_CDN",
        headers: { "Cross-Origin-Resource-Policy": "cross-origin" },
        cors_origins: ["https://app.test"],
        auth: "anonymous",
      },
    ];
    mf = new Miniflare({
      modules: true,
      scriptPath: BUNDLE,
      bindings: { ASSET_ORIGIN: "", WORKER_MODE: "static", ROUTE_TABLE: JSON.stringify(table) },
      r2Buckets: ["R2_CDN"],
    });
    bucket = await mf.getR2Bucket("R2_CDN");
  });

  afterAll(async () => {
    await mf.dispose();
  });

  async function put(key: string, body: string, cacheControl?: string) {
    await bucket.put(key, body, {
      httpMetadata: { contentType: "application/wasm", cacheControl },
    });
  }

  /** GET until the body stops being `fresh` (a cache hit) or tries run out. */
  async function getUntilStale(
    url: string,
    fresh: string,
    init?: { headers: Record<string, string> },
  ) {
    for (let i = 0; i < 20; i++) {
      const resp = await mf.dispatchFetch(url, init);
      const text = await resp.text();
      if (text !== fresh) return { resp, text };
      await new Promise((r) => setTimeout(r, 25));
    }
    const resp = await mf.dispatchFetch(url, init);
    return { resp, text: await resp.text() };
  }

  test("an immutable object is served from the edge cache once warmed", async () => {
    await put("app/abc.wasm", "v1", IMMUTABLE);
    const first = await mf.dispatchFetch("http://w.test/app/abc.wasm");
    expect(await first.text()).toBe("v1");
    const etag = first.headers.get("etag");

    // Let the waitUntil put land, then change what R2 would say.
    await new Promise((r) => setTimeout(r, 100));
    await put("app/abc.wasm", "v2-r2-only", IMMUTABLE);

    const { resp, text } = await getUntilStale(
      "http://w.test/app/abc.wasm",
      "v2-r2-only",
      { headers: { Origin: "https://app.test" } },
    );
    expect(text).toBe("v1");
    expect(resp.headers.get("etag")).toBe(etag);
    expect(resp.headers.get("content-type")).toBe("application/wasm");
    expect(resp.headers.get("cache-control")).toBe(IMMUTABLE);
    // Route headers are stamped AFTER the cache, per request.
    expect(resp.headers.get("cross-origin-resource-policy")).toBe("cross-origin");
    expect(resp.headers.get("access-control-allow-origin")).toBe("https://app.test");
  });

  test("a hit never replays one Origin's CORS grant to another", async () => {
    await put("app/cors.wasm", "c1", IMMUTABLE);
    await mf.dispatchFetch("http://w.test/app/cors.wasm", {
      headers: { Origin: "https://app.test" },
    });
    await new Promise((r) => setTimeout(r, 100));
    await put("app/cors.wasm", "c2-r2-only", IMMUTABLE);
    const { resp, text } = await getUntilStale(
      "http://w.test/app/cors.wasm",
      "c2-r2-only",
      { headers: { Origin: "https://evil.test" } },
    );
    expect(text).toBe("c1");
    expect(resp.headers.get("access-control-allow-origin")).toBeNull();
  });

  test("a no-cache pointer is read from R2 every time", async () => {
    await put("latest.txt", "0.1.0", "no-cache");
    expect(await (await mf.dispatchFetch("http://w.test/latest.txt")).text()).toBe("0.1.0");
    await new Promise((r) => setTimeout(r, 100));
    await put("latest.txt", "0.2.0", "no-cache");
    expect(await (await mf.dispatchFetch("http://w.test/latest.txt")).text()).toBe("0.2.0");
  });

  test("an object with no Cache-Control is not cached", async () => {
    await put("plain.json", "a");
    await mf.dispatchFetch("http://w.test/plain.json");
    await new Promise((r) => setTimeout(r, 100));
    await put("plain.json", "b");
    expect(await (await mf.dispatchFetch("http://w.test/plain.json")).text()).toBe("b");
  });

  test("s-maxage=0 keeps an object out of the shared cache despite max-age", async () => {
    await put("shared.json", "a", "public, max-age=600, s-maxage=0");
    await mf.dispatchFetch("http://w.test/shared.json");
    await new Promise((r) => setTimeout(r, 100));
    await put("shared.json", "b", "public, max-age=600, s-maxage=0");
    expect(await (await mf.dispatchFetch("http://w.test/shared.json")).text()).toBe("b");
  });

  test("a 404 is never cached: the object is visible as soon as it lands", async () => {
    const miss = await mf.dispatchFetch("http://w.test/app/late.wasm");
    expect(miss.status).toBe(404);
    await miss.arrayBuffer();
    await new Promise((r) => setTimeout(r, 100));
    await put("app/late.wasm", "here", IMMUTABLE);
    const resp = await mf.dispatchFetch("http://w.test/app/late.wasm");
    expect(resp.status).toBe(200);
    expect(await resp.text()).toBe("here");
  });
});

describe("backend entry with no origin", () => {
  let setup: MfSetup;

  beforeAll(async () => {
    setup = await makeMf({
      mode: "static",
      assets: { "index.html": { body: "<h1>home</h1>", type: "text/html" } },
      rawRouteTable: JSON.stringify([{ path: "/api/*", mode: "backend" }]),
    });
  });

  afterAll(async () => {
    await setup.mf.dispose();
    setup.stop();
  });

  test("fails the path rather than serving it from the catch-all", async () => {
    const resp = await setup.mf.dispatchFetch("http://w.test/api/thing");
    expect(resp.status).toBe(502);
    expect(await resp.text()).not.toContain("home");
  });
});
