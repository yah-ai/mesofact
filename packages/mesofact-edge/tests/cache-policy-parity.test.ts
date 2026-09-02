// R749-B4 — the edge half of the `cache_policy` parity fixture
// (`tests/fixtures/cache-policy/parity.json`).
//
// The Rust half asserts the same fixture in
// `crates/mesofact-core/tests/cache_policy_parity.rs`, covering the serve tier's
// middleware and the publisher's PUT. Three tiers derive a header from one
// declaration; the defect this exists to stop is any two of them disagreeing.
// Before R749-B4 all three did: the origin honoured `{ ttl: 3600 }`, the
// publisher wrote `max-age=86400` from the path prefix, and this worker
// overwrote both with a flat `no-cache`.
//
// Two layers, on purpose. The fixture drives `pageCacheHeaders` directly, which
// is the derivation; the miniflare block then proves the derivation actually
// reaches a client through the real bundle, which is the claim the ticket's
// verify line makes ("published and fetched through the Worker").

import { describe, test, expect, afterAll } from "bun:test";
import { Miniflare } from "miniflare";
import { readFileSync } from "node:fs";
import { join, dirname } from "path";
import { fileURLToPath } from "url";

import { pageCacheHeaders, type EdgeManifest } from "../src/manifest.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const BUNDLE = join(__dirname, "../dist/router.bundle.js");
const FIXTURE = join(
  __dirname,
  "../../../tests/fixtures/cache-policy/parity.json",
);

interface Case {
  name: string;
  path: string;
  cache_control: string | null;
  vary?: string;
}

const parity = JSON.parse(readFileSync(FIXTURE, "utf8")) as {
  routes: EdgeManifest["routes"];
  cases: Case[];
};

const manifest: EdgeManifest = { build_id: "b1", routes: parity.routes };

describe("cache_policy front-door parity fixture", () => {
  test("the fixture has cases", () => {
    expect(parity.cases.length).toBeGreaterThan(0);
  });

  for (const c of parity.cases) {
    test(`${c.name} (${c.path})`, () => {
      const derived = pageCacheHeaders(manifest, c.path);
      expect(derived?.cacheControl ?? null).toBe(c.cache_control);
      expect(derived?.vary ?? null).toBe(c.vary ?? null);
    });
  }
});

// ── the derived header actually reaches a client ────────────────────────────

type Assets = Record<string, { body: string; type: string }>;

function startAssetServer(assets: Assets): { port: number; stop: () => void } {
  const server = Bun.serve({
    port: 0,
    fetch(req) {
      const key = new URL(req.url).pathname.slice(1) || "index.html";
      const asset = assets[key];
      if (!asset) return new Response("not found", { status: 404 });
      return new Response(asset.body, {
        // The publisher's own header on the object — deliberately WRONG for the
        // mutable route, so a test that passed it through would fail here.
        headers: {
          "Content-Type": asset.type,
          "Cache-Control": "public, max-age=86400",
        },
      });
    },
  });
  return { port: server.port!, stop: () => server.stop(true) };
}

const assets: Assets = {
  "manifest.json": {
    body: JSON.stringify(manifest),
    type: "application/json",
  },
  "b1/html/issues.html": { body: "<h1>issues</h1>", type: "text/html" },
  "b1/html/app.html": { body: "<h1>app</h1>", type: "text/html" },
  "b1/html/lang.html": { body: "<h1>lang</h1>", type: "text/html" },
  "b1/html/index.html": { body: "<h1>home</h1>", type: "text/html" },
  // No build-tree copy — resolves through the flat layout.
  "revalidate.html": { body: "<h1>revalidate</h1>", type: "text/html" },
};

const origin = startAssetServer(assets);
const mf = new Miniflare({
  modules: true,
  scriptPath: BUNDLE,
  bindings: {
    ASSET_ORIGIN: `http://localhost:${origin.port}`,
    WORKER_MODE: "static",
    SSR_ORIGIN: "",
    SSR_PREFIXES: "[]",
  },
});

afterAll(async () => {
  await mf.dispose();
  origin.stop();
});

describe("cache_policy through the worker", () => {
  // The ticket's verify line, end to end.
  test("a declared ttl is what the client is told, not the object's header", async () => {
    const resp = await mf.dispatchFetch("http://w.test/issues");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("cache-control")).toBe(
      "public, max-age=3600, stale-while-revalidate=86400",
    );
  });

  test("a gated route is private at the edge too", async () => {
    const resp = await mf.dispatchFetch("http://w.test/app");
    expect(resp.headers.get("cache-control")).toBe("private, max-age=60");
  });

  test("a declared vary rides along", async () => {
    const resp = await mf.dispatchFetch("http://w.test/lang");
    expect(resp.headers.get("cache-control")).toBe("public, max-age=60");
    expect(resp.headers.get("vary")).toBe("accept-language, cookie");
  });

  // The pre-R749-B4 behaviour, preserved exactly for the sites that declare
  // nothing: an inert `{ ttl: 0 }` is not a declaration.
  test("an undeclared page still gets the no-cache default", async () => {
    const resp = await mf.dispatchFetch("http://w.test/");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("cache-control")).toBe("no-cache");
  });

  // A declaration must not go unenforced because the bytes happened to resolve
  // through the older flat layout rather than the build tree.
  test("the flat layout carries the declaration too", async () => {
    const resp = await mf.dispatchFetch("http://w.test/revalidate");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("cache-control")).toBe(
      "public, max-age=0, stale-while-revalidate=60",
    );
  });
});

// ── instance-addressed (deferred) routes ────────────────────────────────────
//
// The third tier default at the edge. `serveInstance` marks its response
// `immutable` on the reasoning that the bytes are content-addressed — true of
// the bytes, not of the `/c/abc123` URL they are served at. A route that
// declares a TTL is saying something about that URL, and a default that
// overrides it for a year is the same silent no-op one path over.

describe("cache_policy on a deferred route", () => {
  const deferredManifest = {
    build_id: "b1",
    routes: [
      {
        route: "/c/:slug",
        prerender: { deferred: true },
        cache_policy: { ttl: 120 },
      },
      { route: "/d/:slug", prerender: { deferred: true } },
    ],
  };
  const origin2 = startAssetServer({
    "manifest.json": {
      body: JSON.stringify(deferredManifest),
      type: "application/json",
    },
    "p/c/live": {
      body: JSON.stringify({ v: 1, pointer: { content_root: "html/x.html" } }),
      type: "application/json",
    },
    "p/d/live": {
      body: JSON.stringify({ v: 1, pointer: { content_root: "html/x.html" } }),
      type: "application/json",
    },
    "html/x.html": { body: "<h1>instance</h1>", type: "text/html" },
  });
  const mf2 = new Miniflare({
    modules: true,
    scriptPath: BUNDLE,
    bindings: {
      ASSET_ORIGIN: `http://localhost:${origin2.port}`,
      WORKER_MODE: "static",
      SSR_ORIGIN: "",
      SSR_PREFIXES: "[]",
    },
  });

  afterAll(async () => {
    await mf2.dispose();
    origin2.stop();
  });

  test("a declared ttl beats the immutable default", async () => {
    const resp = await mf2.dispatchFetch("http://w.test/c/live");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("cache-control")).toBe("public, max-age=120");
  });

  test("an undeclared deferred route keeps the immutable default", async () => {
    const resp = await mf2.dispatchFetch("http://w.test/d/live");
    expect(resp.status).toBe(200);
    expect(resp.headers.get("cache-control")).toBe(
      "public, max-age=31536000, immutable",
    );
  });
});
