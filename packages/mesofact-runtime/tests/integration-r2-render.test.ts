// Integration: a real render entrypoint that reads two R2 keys via the
// adapter factory, wrapped in runInTrackCtx the same way mesofact-worker
// wraps it. Asserts the resulting RenderResult.cache.tags includes both
// `r2:<bucket>:<key>` values plus any tags the render returned explicitly,
// and that `.noTrack()` / `.timeout(ms)` overrides apply per-call.
//
// This is the verify-step for R006 / P4 from the working doc.

import { afterEach, describe, expect, test } from "bun:test";
import {
  R2Adapter,
  type RenderFn,
  type RenderRequest,
  type RenderResult,
  SourceTimeoutError,
  clearR2Registry,
  parseConfig,
  r2,
  registerR2,
  registerSourcesFromConfig,
  runInTrackCtx,
} from "../src/index.js";

afterEach(() => clearR2Registry());

// Builds a fake `fetch` that returns canned 200-OK responses keyed by URL
// suffix (`/<bucket>/<key>` or `?list-type=2&prefix=...`). Records every call.
function stubFetch(
  responses: Record<string, string | Uint8Array>,
  hangFor: Set<string> = new Set(),
): { fetch: typeof fetch; urls: string[] } {
  const urls: string[] = [];
  const fn: typeof fetch = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    urls.push(url);
    for (const hang of hangFor) {
      if (url.includes(hang)) {
        return new Promise<Response>(() => {
          // never resolves — exercises the timeout path
        });
      }
    }
    for (const [needle, body] of Object.entries(responses)) {
      if (url.includes(needle)) {
        return new Response(body, { status: 200 });
      }
    }
    return new Response("not found", { status: 404 });
  };
  return { fetch: fn, urls };
}

function setupRegistry(httpFetch: typeof fetch): void {
  registerR2(
    new R2Adapter({
      name: "assets",
      bucket: "yah-assets",
      endpoint: "https://acct.r2.cloudflarestorage.com",
      accessKeyId: "AKIAIOSFODNN7EXAMPLE",
      secretAccessKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
      httpFetch,
    }),
  );
}

// Mimic the worker's wrap: invoke the render fn inside runInTrackCtx, merge
// the ctx.tags with the result's own tags. This is the exact flow at
// packages/mesofact-worker/src/worker.ts:handleRender, distilled to TS-only.
async function invokeRender(
  fn: RenderFn,
  req: RenderRequest,
): Promise<RenderResult> {
  const { value, ctx } = await runInTrackCtx(() => Promise.resolve(fn(req)));
  const merged = new Set<string>(ctx.tags);
  for (const t of value.cache.tags ?? []) merged.add(t);
  return {
    ...value,
    cache: {
      ...value.cache,
      ...(merged.size > 0 ? { tags: [...merged] } : {}),
    },
  };
}

const REQ: RenderRequest = {
  url: "/p/1",
  params: { id: "1" },
  query: {},
  headers: {},
  cookies: {},
};

describe("P4 verify: render reads two R2 keys", () => {
  test("cache.tags includes r2:<bucket>:<key> for each read plus render's own tags", async () => {
    const { fetch: f } = stubFetch({
      "/yah-assets/css/app.css": new Uint8Array([1, 2, 3]),
      "/yah-assets/img/hero.png": new Uint8Array([4, 5, 6]),
    });
    setupRegistry(f);

    const render: RenderFn = async () => {
      const css = await r2("assets").fetch("css/app.css");
      const hero = await r2("assets").fetch("img/hero.png");
      return {
        html: `<html><!-- css=${css?.length} hero=${hero?.length} --></html>`,
        cache: { ttl: 60, tags: ["page:home"] },
      };
    };

    const result = await invokeRender(render, REQ);
    expect(new Set(result.cache.tags)).toEqual(
      new Set(["r2:yah-assets:css/app.css", "r2:yah-assets:img/hero.png", "page:home"]),
    );
  });

  test(".noTrack() suppresses the tag for that read only", async () => {
    const { fetch: f } = stubFetch({
      "/yah-assets/css/app.css": new Uint8Array(),
      "/yah-assets/flags.json": new Uint8Array(),
    });
    setupRegistry(f);

    const render: RenderFn = async () => {
      await r2("assets").noTrack().fetch("flags.json");
      await r2("assets").fetch("css/app.css");
      return { html: "<html></html>", cache: { ttl: 60 } };
    };

    const result = await invokeRender(render, REQ);
    expect(result.cache.tags).toEqual(["r2:yah-assets:css/app.css"]);
  });

  test(".timeout(ms) override fires before the adapter's 2000ms default", async () => {
    const { fetch: f } = stubFetch({}, new Set(["/yah-assets/slow.bin"]));
    setupRegistry(f);

    const render: RenderFn = async () => {
      // Default would be 2000ms; with the override we expect to bail in ~50ms.
      await r2("assets").timeout(50).fetch("slow.bin");
      return { html: "<html></html>", cache: { ttl: 0 } };
    };

    const start = Date.now();
    await expect(invokeRender(render, REQ)).rejects.toBeInstanceOf(SourceTimeoutError);
    const elapsed = Date.now() - start;
    expect(elapsed).toBeGreaterThanOrEqual(40);
    expect(elapsed).toBeLessThan(500);
  });

  test("registers from a toml config and serves a real render through r2(name)", async () => {
    const cfg = parseConfig(`
      [sources.assets]
      kind = "r2"
      scope = "global"
      bucket = "yah-assets"
      endpoint_env = "R2_ENDPOINT"
    `);
    const { fetch: f } = stubFetch({
      "/yah-assets/css/app.css": "body { color: hotpink; }",
    });
    // We have to wire the stub fetch into the adapter after the config-driven
    // registry creates it. The cleanest hook for now is to call
    // registerSourcesFromConfig to validate env handling, then replace the
    // registry entry with a stub-fetch-backed adapter for the read.
    registerSourcesFromConfig(cfg, {
      R2_ENDPOINT: "https://acct.r2.cloudflarestorage.com",
      AWS_ACCESS_KEY_ID: "AKIAIOSFODNN7EXAMPLE",
      AWS_SECRET_ACCESS_KEY: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    });
    // Swap in stub-fetch-backed instance under the same name (registry uses
    // last-write-wins; same shape as a config reload).
    setupRegistry(f);

    const render: RenderFn = async () => {
      const buf = await r2("assets").fetch("css/app.css");
      const css = buf ? new TextDecoder().decode(buf) : "";
      return { html: `<style>${css}</style>`, cache: { ttl: 60 } };
    };

    const result = await invokeRender(render, REQ);
    expect(result.html).toContain("hotpink");
    expect(result.cache.tags).toEqual(["r2:yah-assets:css/app.css"]);
  });
});
