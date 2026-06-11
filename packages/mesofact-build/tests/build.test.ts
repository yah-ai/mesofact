// End-to-end build smoke. Each test uses a fixture project under
// `tests/fixtures/<name>/`, builds into a tmp `outDir`, and asserts what
// landed on disk.

import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { R2Adapter, clearR2Registry, registerR2 } from "@mesofact/runtime";
import { build, ValidationFailed } from "../src/index.js";

const FIXTURES = fileURLToPath(new URL("./fixtures/", import.meta.url));

let tmpDirs: string[] = [];

afterEach(() => {
  for (const d of tmpDirs) rmSync(d, { recursive: true, force: true });
  tmpDirs = [];
});

function makeOut(): string {
  const d = mkdtempSync(join(tmpdir(), "mesofact-build-"));
  tmpDirs.push(d);
  return d;
}

// outDir rooted under this package so Node can resolve `@mesofact/runtime`
// from the bundled SSR module — `external: ["@mesofact/runtime"]` keeps it
// out of the bundle (see bundle.ts comment), so the import target has to be
// reachable via the parent chain. Use for tests that dynamic-import a built
// SSR bundle whose code calls into the runtime.
function makeProjectOut(): string {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const d = mkdtempSync(join(root, ".test-out-"));
  tmpDirs.push(d);
  return d;
}

describe("build (static-only fixture)", () => {
  test("emits manifest, tag-index, html for literal Mode 1 routes", async () => {
    const projectRoot = join(FIXTURES, "static-only");
    const outDir = makeOut();
    const buildId = "test-fixture-build";

    const result = await build({ projectRoot, outDir, buildId });

    expect(result.buildId).toBe(buildId);

    // Manifest landed and validates.
    expect(existsSync(result.manifestPath)).toBe(true);
    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    expect(manifest.version).toBe("1");
    expect(manifest.build_id).toBe(buildId);
    expect(manifest.routes).toHaveLength(2);

    const home = manifest.routes.find((r: { route: string }) => r.route === "/");
    expect(home).toBeDefined();
    expect(home.mode).toBe("static");
    expect(home.render_entrypoint).toBe("dist/server/index.js");
    expect(home.source_reads).toBeUndefined(); // home has no reads

    const pid = manifest.routes.find((r: { route: string }) => r.route === "/p/:id");
    expect(pid).toBeDefined();
    expect(pid.source_reads).toEqual(["assets"]); // from @mesofact-sources directive
    expect(pid.prerender).toEqual({ params: [{ id: "1" }, { id: "2" }] });

    // HTML output: index.html for `/`, p_id__1.html and p_id__2.html for `/p/:id`.
    expect(existsSync(join(outDir, "html/index.html"))).toBe(true);
    expect(existsSync(join(outDir, "html/p_id__1.html"))).toBe(true);
    expect(existsSync(join(outDir, "html/p_id__2.html"))).toBe(true);
    expect(readFileSync(join(outDir, "html/p_id__1.html"), "utf8")).toContain("<h1>1</h1>");
    expect(readFileSync(join(outDir, "html/p_id__2.html"), "utf8")).toContain("<h1>2</h1>");

    // tag-index: reverse map tag → urls
    expect(existsSync(result.tagIndexPath)).toBe(true);
    const tagIndex = JSON.parse(readFileSync(result.tagIndexPath, "utf8"));
    expect(tagIndex.build_id).toBe(buildId);
    expect(tagIndex.tags.home).toEqual(["/"]);
    expect(tagIndex.tags["page:1"]).toEqual(["/p/1"]);
    expect(tagIndex.tags["page:2"]).toEqual(["/p/2"]);
  });
});

describe("build (source-derived prerender.query)", () => {
  // Stub fetch returning a fixed S3 ListBucketResult v2 with the given keys.
  // The adapter signs the request with aws4fetch and dispatches through this
  // fn, so the test never hits the network and credentials can be dummy
  // strings.
  function stubR2List(keys: readonly string[]): typeof fetch {
    const xml =
      `<?xml version="1.0" encoding="UTF-8"?>\n` +
      `<ListBucketResult>` +
      keys
        .map(
          (k) =>
            `<Contents><Key>${k}</Key><Size>0</Size>` +
            `<LastModified>2026-01-01T00:00:00.000Z</LastModified></Contents>`,
        )
        .join("") +
      `</ListBucketResult>`;
    const impl = async () =>
      new Response(xml, {
        status: 200,
        headers: { "content-type": "application/xml" },
      });
    // `typeof fetch` includes a `preconnect` slot from lib.dom; the adapter
    // never calls it, but the assignment needs the property to satisfy TS.
    return Object.assign(impl, { preconnect: () => {} }) as typeof fetch;
  }

  afterEach(() => {
    clearR2Registry();
  });

  test("expands params via r2.list and emits HTML equivalent to literal-params fixture", async () => {
    registerR2(
      new R2Adapter({
        name: "assets",
        bucket: "fixture-assets",
        endpoint: "https://stub.r2.example",
        accessKeyId: "stub",
        secretAccessKey: "stub",
        httpFetch: stubR2List(["1", "2"]),
      }),
    );

    const dynamicOut = makeOut();
    const dynamicResult = await build({
      projectRoot: join(FIXTURES, "dynamic-from-source"),
      outDir: dynamicOut,
      buildId: "test-fixture-build",
    });

    // Manifest preserves the source-derived shape — render emitted equivalent
    // HTML, but the publisher / proxy still see the original query intent.
    const dynManifest = JSON.parse(readFileSync(dynamicResult.manifestPath, "utf8"));
    const dynPid = dynManifest.routes.find((r: { route: string }) => r.route === "/p/:id");
    expect(dynPid.prerender).toEqual({ from: "assets", query: "list:", param: "id" });

    // Build the literal fixture in parallel and compare the per-key HTML
    // byte-for-byte. If the source-derived expansion produces a different
    // param set, this fails on a missing/different file.
    const literalOut = makeOut();
    await build({
      projectRoot: join(FIXTURES, "static-only"),
      outDir: literalOut,
      buildId: "test-fixture-build",
    });

    for (const key of ["index.html", "p_id__1.html", "p_id__2.html"]) {
      const dyn = readFileSync(join(dynamicOut, "html", key), "utf8");
      const lit = readFileSync(join(literalOut, "html", key), "utf8");
      expect(dyn).toBe(lit);
    }

    // Tag-index entries for the dynamic build should match the literal one
    // for the parametric route's tags.
    const dynTags = JSON.parse(readFileSync(dynamicResult.tagIndexPath, "utf8")).tags;
    expect(dynTags["page:1"]).toEqual(["/p/1"]);
    expect(dynTags["page:2"]).toEqual(["/p/2"]);
  });

  test("surfaces a clear error when the adapter is not registered", async () => {
    // Catalog has `assets` (config.toml declares it), but the registry is
    // empty for this test — `r2('assets')` should throw with a config-toml
    // pointer so a deployer knows to register the adapter before build.
    let caught: unknown;
    try {
      await build({
        projectRoot: join(FIXTURES, "dynamic-from-source"),
        outDir: makeOut(),
        buildId: "fail",
      });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    expect((caught as Error).message).toMatch(/r2 source not registered: assets/);
  });
});

describe("build (spa / Mode 3 fixture)", () => {
  test("bundles the client tree, populates manifest hydration, and weaves the shell", async () => {
    const projectRoot = join(FIXTURES, "spa");
    const outDir = makeOut();
    const buildId = "spa-build";

    const result = await build({ projectRoot, outDir, buildId });

    // Manifest: the spa route carries a build-derived hydration block whose
    // script is content-hashed (so it can be served immutable).
    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    const app = manifest.routes.find((r: { route: string }) => r.route === "/app");
    expect(app).toBeDefined();
    expect(app.mode).toBe("spa");
    expect(app.hydration).toBeDefined();
    expect(app.hydration.script).toMatch(/^app\.[0-9a-z]+\.js$/);
    expect(Array.isArray(app.hydration.code_split)).toBe(true);

    // The hashed client entry actually landed under dist/hydrate/.
    expect(existsSync(join(outDir, "hydrate", app.hydration.script))).toBe(true);

    // The shell was prerendered AND woven: it carries the serialized initial
    // state (HTML-escaped so the `<b>`/`&` in the state can't break the tag)
    // plus the module <script> pointing at /{build_id}/hydrate/<script>.
    const shell = readFileSync(join(outDir, "html/app.html"), "utf8");
    expect(shell).toContain('<script id="__MESOFACT_STATE__" type="application/json">');
    expect(shell).toContain('"count":7');
    expect(shell).toContain("\\u003cb\\u003e"); // escaped <b>, not a live tag
    expect(shell).not.toContain("<b>build</b>"); // raw markup must not leak in
    expect(shell).toContain(
      `<script type="module" src="/${buildId}/hydrate/${app.hydration.script}"></script>`,
    );
    // Injection lands inside the document, before </body>.
    expect(shell.indexOf("__MESOFACT_STATE__")).toBeLessThan(shell.indexOf("</body>"));
  });

  test("rejects a spa route missing client_entrypoint", async () => {
    // Reuse the spa fixture but strip client_entrypoint by building a one-off
    // project dir that points its routes at the shared shell with no client.
    const projectRoot = join(FIXTURES, "spa-no-client");
    let caught: unknown;
    try {
      await build({ projectRoot, outDir: makeOut(), buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    expect((caught as Error).message).toMatch(/requires a client_entrypoint/);
  });
});

describe("build (data_inputs fixture)", () => {
  test("populates req.data from declared data_inputs and writes it to HTML", async () => {
    const projectRoot = join(FIXTURES, "data-inputs");
    const outDir = makeOut();
    const buildId = "data-inputs-test";

    const result = await build({ projectRoot, outDir, buildId });

    // HTML contains content read from the JSON file.
    const releasesHtml = readFileSync(join(outDir, "html/releases.html"), "utf8");
    expect(releasesHtml).toContain("r1: Release 1");
    expect(releasesHtml).toContain("r2: Release 2");

    // Manifest carries data_inputs for rebuild-detection metadata.
    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    const releases = manifest.routes.find((r: { route: string }) => r.route === "/releases");
    expect(releases).toBeDefined();
    expect(releases.data_inputs).toEqual(["data/sample.json"]);
  });

  test("req.data is absent when no data_inputs declared", async () => {
    const projectRoot = join(FIXTURES, "data-inputs");
    const outDir = makeOut();

    const result = await build({ projectRoot, outDir, buildId: "no-data-test" });

    // The /bare render outputs 'no-data' when req.data is undefined.
    const bareHtml = readFileSync(join(outDir, "html/bare.html"), "utf8");
    expect(bareHtml).toContain("no-data");
    expect(bareHtml).not.toContain("has-data");

    // Manifest has no data_inputs field for the route without a declaration.
    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    const bare = manifest.routes.find((r: { route: string }) => r.route === "/bare");
    expect(bare).toBeDefined();
    expect(bare.data_inputs).toBeUndefined();
  });

  test("fails with a clear error when a declared data_input file is missing", async () => {
    const projectRoot = join(FIXTURES, "data-inputs-missing");
    let caught: unknown;
    try {
      await build({ projectRoot, outDir: makeOut(), buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    // The ENOENT path contains the filename so the operator knows what to fix.
    expect((caught as Error).message).toMatch(/missing\.json/);
  });
});

describe("build (prerender.from_data fixture)", () => {
  test("expands params from a declared data_inputs JSON file", async () => {
    const projectRoot = join(FIXTURES, "prerender-from-data");
    const outDir = makeOut();
    const buildId = "from-data-test";

    const result = await build({ projectRoot, outDir, buildId });

    // Two HTML files — one per item in data/items.json.
    expect(existsSync(join(outDir, "html/items_id__a.html"))).toBe(true);
    expect(existsSync(join(outDir, "html/items_id__b.html"))).toBe(true);

    // Each render sees its own params.id AND the shared req.data payload.
    const a = readFileSync(join(outDir, "html/items_id__a.html"), "utf8");
    const b = readFileSync(join(outDir, "html/items_id__b.html"), "utf8");
    expect(a).toContain("<h1>a</h1>");
    expect(a).toContain("<p>Alpha</p>");
    expect(b).toContain("<h1>b</h1>");
    expect(b).toContain("<p>Bravo</p>");

    // tag-index carries one URL per emitted item.
    const tagIndex = JSON.parse(readFileSync(result.tagIndexPath, "utf8"));
    expect(tagIndex.tags["item:a"]).toEqual(["/items/a"]);
    expect(tagIndex.tags["item:b"]).toEqual(["/items/b"]);
  });

  test("rejects from_data referencing a path not in data_inputs (defineRoutes-time)", async () => {
    const projectRoot = join(FIXTURES, "prerender-from-data-undeclared");
    let caught: unknown;
    try {
      await build({ projectRoot, outDir: makeOut(), buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    const msg = (caught as Error).message;
    expect(msg).toMatch(/prerender\.from_data/);
    expect(msg).toMatch(/data_inputs/);
    expect(msg).toContain("/items/:id");
  });

  test("rejects items_key not found in the JSON", async () => {
    const projectRoot = join(FIXTURES, "prerender-from-data-missing-key");
    let caught: unknown;
    try {
      await build({ projectRoot, outDir: makeOut(), buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    const msg = (caught as Error).message;
    expect(msg).toMatch(/items_key='items'/);
    expect(msg).toContain("data/items.json");
  });

  test("rejects an item whose param field is not a string", async () => {
    const projectRoot = join(FIXTURES, "prerender-from-data-non-string");
    let caught: unknown;
    try {
      await build({ projectRoot, outDir: makeOut(), buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    const msg = (caught as Error).message;
    expect(msg).toMatch(/items\[0\]\.id is not a string/);
    expect(msg).toContain("data/items.json");
  });
});

describe("build (parametric spa / no prerender config)", () => {
  test("emits routeKey-named shell without prerender params workaround", async () => {
    const projectRoot = join(FIXTURES, "spa-parametric");
    const outDir = makeOut();
    const buildId = "spa-parametric-build";

    const result = await build({ projectRoot, outDir, buildId });

    // Shell is keyed by routeKey('/item/:id') = 'item_id' — no param suffix.
    expect(existsSync(join(outDir, "html/item_id.html"))).toBe(true);
    // No parameterized variants emitted.
    expect(existsSync(join(outDir, "html/item_id__something.html"))).toBe(false);

    // Manifest carries no prerender config (route had none).
    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    const item = manifest.routes.find((r: { route: string }) => r.route === "/item/:id");
    expect(item).toBeDefined();
    expect(item.mode).toBe("spa");
    expect(item.prerender).toBeUndefined();

    // Shell contains the hydration script injection.
    const shell = readFileSync(join(outDir, "html/item_id.html"), "utf8");
    expect(shell).toContain('<div id="root">');
    expect(shell).toContain(`<script type="module" src="/${buildId}/hydrate/`);
  });
});

describe("build (ssr fixture)", () => {
  test("emits manifest with placement + ssr_prefixes; skips prerender for ssr routes", async () => {
    const projectRoot = join(FIXTURES, "ssr");
    const outDir = makeOut();
    const buildId = "ssr-build";

    const result = await build({ projectRoot, outDir, buildId });

    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));

    // Top-level: ssr_prefixes derived per W173 rule.
    //   /api/health      → /api/health (exact-match prefix, non-parametric)
    //   /api/users/:id   → /api/users/ (truncated at first param)
    expect(manifest.ssr_prefixes).toEqual(["/api/health", "/api/users/"]);

    // Per-route: placement defaults to "host" (auto → host today).
    const health = manifest.routes.find((r: { route: string }) => r.route === "/api/health");
    expect(health.mode).toBe("ssr");
    expect(health.placement).toBe("host"); // explicitly declared
    expect(health.render_entrypoint).toMatch(/^dist\/server\/api_health\.js$/);

    const users = manifest.routes.find((r: { route: string }) => r.route === "/api/users/:id");
    expect(users.mode).toBe("ssr");
    expect(users.placement).toBe("host"); // default `auto` resolved to host

    // Static route in the same workload still has no placement.
    const home = manifest.routes.find((r: { route: string }) => r.route === "/");
    expect(home.placement).toBeUndefined();

    // Prerender skipped for SSR routes — only the static one emits HTML.
    expect(existsSync(join(outDir, "html/index.html"))).toBe(true);
    expect(existsSync(join(outDir, "html/api_health.html"))).toBe(false);
    expect(existsSync(join(outDir, "html/api_users_id.html"))).toBe(false);

    // SSR entrypoints still bundle to dist/server/ for the dev subprocess +
    // pond container + edge Worker bundle to consume.
    expect(existsSync(join(outDir, "server/api_health.js"))).toBe(true);
    expect(existsSync(join(outDir, "server/api_users_id.js"))).toBe(true);
  });

  test("rejects an ssr route whose entrypoint has the wrong default-export shape", async () => {
    const projectRoot = join(FIXTURES, "ssr-broken");
    const outDir = makeOut();
    let caught: unknown;
    try {
      await build({ projectRoot, outDir, buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    expect((caught as Error).message).toMatch(/mode:"ssr" entrypoint must `export default` a Fetch handler/);
    expect((caught as Error).message).toContain("/api/broken");

    // Build failed before the manifest hit disk.
    expect(existsSync(join(outDir, "manifest.json"))).toBe(false);
  });
});

describe("build (ssr Universal cell / hydration handoff)", () => {
  test("ssr route + client_entrypoint bundles hydrate + manifest carries hydration", async () => {
    const projectRoot = join(FIXTURES, "ssr-universal");
    const outDir = makeProjectOut();
    const buildId = "universal-build";

    const result = await build({ projectRoot, outDir, buildId });
    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    const dashboard = manifest.routes.find((r: { route: string }) => r.route === "/dashboard");

    // Mode + placement carried; default placement resolves to "host" (the
    // W173 Universal cell is explicitly host SSR + client_entrypoint).
    expect(dashboard.mode).toBe("ssr");
    expect(dashboard.placement).toBe("host");

    // Hydration block matches the SPA shape (script + code_split) so cloud
    // / dev consumers can read both kinds the same way.
    expect(dashboard.hydration).toBeDefined();
    expect(dashboard.hydration.script).toMatch(/^dashboard\.[0-9a-z]+\.js$/);
    expect(Array.isArray(dashboard.hydration.code_split)).toBe(true);

    // Hashed client bundle landed under dist/hydrate/.
    expect(existsSync(join(outDir, "hydrate", dashboard.hydration.script))).toBe(true);

    // SSR routes are skipped by prerender — no HTML emitted for /dashboard.
    expect(existsSync(join(outDir, "html/dashboard.html"))).toBe(false);

    // SSR server bundle is reachable so the dev subprocess / pond container
    // can import and serve it.
    expect(existsSync(join(outDir, "server/dashboard.js"))).toBe(true);
    expect(manifest.ssr_prefixes).toEqual(["/dashboard"]);
  });

  test("the SSR handler returns a response body with the __mesofact_data__ tag, XSS-escaped", async () => {
    // Drive the bundled handler end-to-end: bundle, dynamic-import, call,
    // and assert the response body shape. This is the closest we can get to
    // a real dev/pond request without standing up the subprocess.
    const projectRoot = join(FIXTURES, "ssr-universal");
    const outDir = makeProjectOut();
    const result = await build({ projectRoot, outDir, buildId: "drive-test" });

    const bundle = await import(join(outDir, "server/dashboard.js"));
    const handler = bundle.default as (req: Request) => Promise<Response>;
    expect(typeof handler).toBe("function");

    const res = await handler(new Request("http://example.test/dashboard"));
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toContain("text/html");
    const body = await res.text();

    // The data-handoff tag is present with the W173-pinned id.
    expect(body).toContain('<script id="__mesofact_data__" type="application/json">');

    // The hostile payload is XSS-escaped — no live `</script>` or `<img>`
    // breaks out of the tag boundary, and no raw `<b>` leaks into the DOM.
    expect(body).toContain("\\u003c/script\\u003e");
    expect(body).toContain("\\u003cimg");
    expect(body).toContain("\\u003cb\\u003ehtml in data\\u003c/b\\u003e");
    expect(body).not.toContain("<b>html in data</b>");
    expect(body).not.toContain("<img src=x");

    // Hydrate script tag also present, pointing at the placeholder URL.
    expect(body).toContain('<script type="module" src="/build-test/hydrate/dashboard.test.js">');

    // The data round-trips: pull out the JSON between the script tags and
    // parse it.
    const open = '<script id="__mesofact_data__" type="application/json">';
    const close = "</script>";
    const start = body.indexOf(open) + open.length;
    const end = body.indexOf(close, start);
    const inner = body.slice(start, end);
    const parsed = JSON.parse(inner);
    expect(parsed.user).toBe("ada");
    expect(parsed.count).toBe(7);
    expect(parsed.label).toBe("<b>html in data</b>");
    expect(parsed.evil).toBe("</script><img src=x onerror=alert(1)>");

    // Body excludes any HTML output emission for SSR routes.
    expect(result.htmlPaths.find((p) => p.includes("dashboard"))).toBeUndefined();
  });
});

describe("build (static-islands / Cell 2)", () => {
  test("static + client_entrypoint bundles hydrate and weaves the prerendered shell", async () => {
    const projectRoot = join(FIXTURES, "static-islands");
    const outDir = makeOut();
    const buildId = "islands-build";

    const result = await build({ projectRoot, outDir, buildId });

    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    const issues = manifest.routes.find((r: { route: string }) => r.route === "/issues");

    // Mode stays "static" — the route prerenders + ships a hydrate bundle.
    expect(issues).toBeDefined();
    expect(issues.mode).toBe("static");
    expect(issues.hydration).toBeDefined();
    expect(issues.hydration.script).toMatch(/^issues\.[0-9a-z]+\.js$/);
    expect(Array.isArray(issues.hydration.code_split)).toBe(true);

    // (c) Hashed client bundle landed under dist/hydrate/.
    expect(existsSync(join(outDir, "hydrate", issues.hydration.script))).toBe(true);

    const html = readFileSync(join(outDir, "html/issues.html"), "utf8");

    // (a) Data-driven static markup — items from JSON rendered into the list.
    expect(html).toContain('<li data-id="i1">Issue 1</li>');
    expect(html).toContain('<li data-id="i2">Issue 2</li>');

    // (b) Hydration weave — state tag + module entry script before </body>.
    expect(html).toContain('<script id="__MESOFACT_STATE__" type="application/json">');
    expect(html).toContain('"count":2');
    expect(html).toContain(
      `<script type="module" src="/${buildId}/hydrate/${issues.hydration.script}"></script>`,
    );
    expect(html.indexOf("__MESOFACT_STATE__")).toBeLessThan(html.indexOf("</body>"));

    // Manifest carries data_inputs for rebuild-detection metadata.
    expect(issues.data_inputs).toEqual(["data/items.json"]);
  });
});

describe("build (host-only API lint)", () => {
  test("spa client_entrypoint pulling node:fs transitively fails the build", async () => {
    const projectRoot = join(FIXTURES, "spa-host-only");
    const outDir = makeOut();
    let caught: unknown;
    try {
      await build({ projectRoot, outDir, buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    const msg = (caught as Error).message;

    // Error names the route + identifies the boundary that was violated.
    expect(msg).toContain("/app");
    expect(msg).toContain("client_entrypoint");

    // And surfaces the actual import chain — the helper that pulled it in,
    // not just the client_entrypoint at the top of the tree.
    expect(msg).toContain("describe.ts");
    expect(msg).toContain('"node:fs"');

    // Build failed before manifest hit disk.
    expect(existsSync(join(outDir, "manifest.json"))).toBe(false);
  });

  test('ssr placement:"edge" entrypoint importing a db driver fails the build', async () => {
    const projectRoot = join(FIXTURES, "ssr-edge-host-only");
    const outDir = makeOut();
    let caught: unknown;
    try {
      await build({ projectRoot, outDir, buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    const msg = (caught as Error).message;
    expect(msg).toContain("/api/users/:id");
    expect(msg).toContain('ssr placement:"edge" entrypoint');
    expect(msg).toContain('"pg"');
    expect(existsSync(join(outDir, "manifest.json"))).toBe(false);
  });

  test("ssr placement:'host' route can use db drivers without tripping the lint", async () => {
    // The existing `ssr` fixture has placement:"host" / default; its
    // entrypoints don't import host-only specifiers but if they did the lint
    // wouldn't fire — host placement is allowed to use them. This test
    // confirms the existing fixture still builds clean (no false positives
    // from wiring the lint in).
    const projectRoot = join(FIXTURES, "ssr");
    const outDir = makeOut();
    const result = await build({ projectRoot, outDir, buildId: "ssr-clean" });
    expect(existsSync(result.manifestPath)).toBe(true);
  });
});

describe("build (failing fixtures)", () => {
  test("Mode 1 + project-scoped source is rejected", async () => {
    const projectRoot = join(FIXTURES, "mode1-scoped");
    const outDir = makeOut();
    let caught: unknown;
    try {
      await build({ projectRoot, outDir, buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(ValidationFailed);
    const errs = (caught as ValidationFailed).errors;
    expect(errs.some((e) => e.kind === "mode1_scoped_source")).toBe(true);

    // No manifest / HTML should have hit disk.
    expect(existsSync(join(outDir, "manifest.json"))).toBe(false);
    expect(existsSync(join(outDir, "html"))).toBe(false);
  });

  test("Mode 1 + requires:user is rejected", async () => {
    const projectRoot = join(FIXTURES, "mode1-requires-user");
    const outDir = makeOut();
    let caught: unknown;
    try {
      await build({ projectRoot, outDir, buildId: "fail" });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(ValidationFailed);
    const errs = (caught as ValidationFailed).errors;
    expect(errs.some((e) => e.kind === "mode1_requires_user")).toBe(true);
    expect(existsSync(join(outDir, "manifest.json"))).toBe(false);
  });
});

describe("build (ssr resilience round-trip — W181)", () => {
  test("manifest carries the resilience block verbatim", async () => {
    const projectRoot = join(FIXTURES, "ssr-resilience");
    const outDir = makeOut();

    const result = await build({ projectRoot, outDir, buildId: "resilience-build" });

    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    const route = manifest.routes.find((r: { route: string }) => r.route === "/api/submit");
    expect(route).toBeDefined();
    expect(route.mode).toBe("ssr");
    expect(route.resilience).toEqual({
      timeout_ms: 5_000,
      retry: { attempts: 3, backoff_ms: [50, 200], retry_on: "connection" },
    });

    // The block survives a validate() round-trip (not silently stripped —
    // the data_inputs/R014 regression class).
    const { validate } = await import("@mesofact/runtime");
    const revalidated = validate(JSON.parse(readFileSync(result.manifestPath, "utf8")));
    expect(revalidated.ok).toBe(true);
    if (revalidated.ok) {
      const rt = revalidated.manifest.routes.find((r) => r.route === "/api/submit");
      expect(rt?.resilience?.retry?.attempts).toBe(3);
    }
  });
});

describe("build (static-assets overlay — R490-F4)", () => {
  test("copies public/ into dist/html/ and populates manifest.static_assets", async () => {
    const projectRoot = join(FIXTURES, "static-assets");
    const outDir = makeOut();

    const result = await build({ projectRoot, outDir, buildId: "assets-build" });

    // Files copied verbatim, preserving relative paths.
    const copied = join(outDir, "html", "illustrations", "foo.webp");
    expect(existsSync(copied)).toBe(true);
    expect(readFileSync(copied)).toEqual(readFileSync(join(projectRoot, "public/illustrations/foo.webp")));
    expect(existsSync(join(outDir, "html", "robots.txt"))).toBe(true);

    // Manifest lists both, sorted by key, with hash + content type.
    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    expect(manifest.static_assets).toHaveLength(2);
    const keys = manifest.static_assets.map((a: { key: string }) => a.key);
    expect(keys).toEqual(["illustrations/foo.webp", "robots.txt"]);
    const webp = manifest.static_assets[0];
    expect(webp.content_type).toBe("image/webp");
    expect(webp.content_hash).toMatch(/^[0-9a-f]{64}$/);
    expect(webp.immutable).toBe(false);

    // Routes still build alongside the overlay.
    expect(existsSync(join(outDir, "html", "index.html"))).toBe(true);
  });

  test("a workload without public/ emits an empty static_assets", async () => {
    const projectRoot = join(FIXTURES, "static-only");
    const outDir = makeOut();
    const result = await build({ projectRoot, outDir, buildId: "no-assets" });
    const manifest = JSON.parse(readFileSync(result.manifestPath, "utf8"));
    expect(manifest.static_assets).toEqual([]);
  });
});
