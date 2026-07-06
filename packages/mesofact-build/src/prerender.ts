//! @yah:relay(R014, "data_inputs prerender binding — add end-to-end test coverage")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-05-27T01:28:07Z)
//! @yah:status(review)
//! @yah:gotcha("The binding is ALREADY implemented and works (routes.ts:37 -> index.ts:138 -> prerender.ts:72 -> contract.ts:40 -> manifest.ts:37). This ticket is ONLY the missing test coverage — do NOT re-implement the feature.")
//! @yah:next("Add a build-test fixture under packages/mesofact-build/tests/fixtures/: a mode=static route declaring data_inputs:['data/sample.json'] + a render echoing req.data into HTML; assert prerender reads it and req.data['data/sample.json'] reaches render(). Mirror the dynamic-from-source fixture.")
//! @yah:next("Cover edges: no data_inputs -> req.data empty; a declared-but-missing artifact fails the build with a clear error (not a silent empty render).")
//! @yah:next("Assert the emitted manifest carries data_inputs (manifest-build.ts:66) so rebuild-detection metadata is present.")
//! @yah:next("Context: surfaces from yah-root R330-F4 (yah.dev releases feed), which consumes this binding to render /releases from releases.json.")
//! @yah:verify("cd external/mesofact && bun test packages/mesofact-build — new data_inputs test passes; existing build tests stay green.")
//! @yah:handoff("Added fixture at tests/fixtures/data-inputs/ (2 routes: /releases with data_inputs, /bare without) + tests/fixtures/data-inputs-missing/ for the missing-file error case. Three new tests in build.test.ts covering: data populates HTML + manifest carries data_inputs; req.data absent when no data_inputs; missing file fails with ENOENT naming the file. Also fixed a gap in mesofact-runtime/src/validate.ts — checkRoute() never copied data_inputs from the validated object, so it was silently stripped from the emitted manifest.json. Rebuilt runtime dist/ for the fix to take effect. All 18 build tests + 49 runtime tests pass.")
//! @yah:next("operator to sign off and archive, or flag if the validate.ts fix should be a separate ticket.")
//!
//! @yah:ticket(R015-F3, "Hydration handoff for Universal cell — __mesofact_data__ script tag")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-04T19:32:36Z)
//! @yah:status(review)
//! @yah:phase(P2)
//! @yah:parent(R015)
//! @yah:next("W173 Universal cell = mode:\"ssr\" + placement:\"host\" + client_entrypoint. Server-resolved data inlines into the SSR response as `<script id=\"__mesofact_data__\" type=\"application/json\">…</script>` (NOT __INITIAL_DATA__ — the W173 sidebar pins the name).")
//! @yah:next("Client hydrate entry reads the JSON back on mount.")
//! @yah:next("Conventional shape — disposable when RSC arrives in v2.")
//! @yah:next("Hard-gated on R015-F2 (need SSR render path) and on the yah-side R434-F5 (need a real Universal consumer to validate against — not a hard build dep, but don't ship blind).")
//! @yah:verify("A fixture mode:\"ssr\" route with client_entrypoint emits the __mesofact_data__ script tag in the SSR response body")
//! @yah:verify("The hydrate bundle correctly parses the inlined JSON on mount (use a jsdom or browser-runner harness)")
//! @yah:depends_on(R015-F2)
//! @yah:handoff("Universal-cell hydration handoff shipped (dogfood API; revise as consumers iterate). Changes: (1) New packages/mesofact-runtime/src/hydration.ts exports `escapeJsonForScriptTag(value)` (single source of truth for the W173 XSS rule: <, >, &, U+2028, U+2029 → \\uXXXX), `hydrationDataTag(data)` (returns the W173-pinned `<script id=\"__mesofact_data__\" type=\"application/json\">…</script>` tag), `hydrationScriptTag(src)` (module-script tag with attribute-escaped src), plus `SSR_DATA_SCRIPT_ID`/`SPA_STATE_SCRIPT_ID` constants. (2) prerender.ts now imports `escapeJsonForScriptTag`/`SPA_STATE_SCRIPT_ID` from the runtime instead of keeping its own serializeState copy — one escape impl for both SPA __MESOFACT_STATE__ and SSR __mesofact_data__. (3) Build relaxed: ssr routes MAY declare client_entrypoint (Universal cell); the client bundle goes through the same bundleClientEntrypoints path as spa, ManifestRoute.hydration populated identically. static routes declaring client_entrypoint still throw. (4) New fixture tests/fixtures/ssr-universal: SSR handler renders HTML and inlines hostile-looking data (‘</script>’, ‘<b>’, ‘<img onerror>’) via the helpers. Two build tests: manifest+bundle shape (hydration block + dist/hydrate emission + skipped prerender + ssr_prefixes); end-to-end drive (dynamic-import the SSR bundle, invoke the Fetch handler, assert the response body has the data tag with all hostile chars escaped and the parsed JSON round-trips). (5) Added makeProjectOut() helper for tests that need a project-rooted outDir — needed when the dynamic-imported SSR bundle imports @mesofact/runtime (which is `external` per bundle.ts comment) so Node's module resolution can climb to packages/mesofact-build/node_modules/@mesofact/runtime. (6) 9 new runtime hydration tests covering the escape rule + tag shapes. Totals: mesofact-runtime 66 pass (was 57), mesofact-build 35 pass (was 33), typecheck clean across runtime/build/worker, marketing + dashboard `bun run build` clean. API surface is intentionally minimal — two pure helpers, consumer composes its own HTML shell, consumer is responsible for sourcing buildId + hashed script name (via manifest read or env). When R434-F5 ships the first real Universal consumer, the seam to revisit is `hydrationScriptTag` — it may grow into a `hydrationHead(routeManifest)` helper that takes a ManifestHydration block and emits both tags at once. Recorded as @yah:assumes.")
//! @yah:verify("cd packages/mesofact-runtime && bun test — 66 pass")
//! @yah:verify("cd packages/mesofact-build && bun test — 35 pass")
//! @yah:verify("cd packages/mesofact-runtime && bun run typecheck — clean")
//! @yah:verify("cd packages/mesofact-build && bun run typecheck — clean")
//! @yah:verify("cd packages/mesofact-worker && bun run typecheck — clean")
//! @yah:verify("cd app/yah/web/marketing && bun run build — clean")
//! @yah:verify("cd app/yah/web/dashboard && bun run build — clean")
//! @yah:assumes("The runtime hydration API surface (two pure helpers + script-id constants) will likely need revising when R434-F5 lands a real Universal consumer — in particular, `hydrationScriptTag(src)` may grow into a `hydrationHead(routeManifest)` that takes a ManifestHydration block and emits both data + script tags from one call. Dogfood-only contract; safe to break until external builders consume @mesofact/runtime.")

// Phase 5 — prerender driver. For each Mode 1 (static) and Mode 3 (spa) route,
// expand its `prerender.params` list (literal or source-derived query),
// invoke the entrypoint's `render()` once per param map inside
// `runInTrackCtx`, and write HTML to `dist/html/<key>.html`. (SSR routes render
// per-request and are never prerendered.)
//
// Per-route returned tags = `result.cache.tags ?? []` ∪ trackCtx-collected
// tags. They feed `tag-index.json` (see `tag-index.ts`).
//
// Mode 3 shell: when an input carries `hydration`, the driver weaves the
// client bundle into the emitted shell — it serializes `result.hydration
// .initial_state` into a `<script id="__MESOFACT_STATE__" type="application/
// json">` tag and appends `<script type="module" src="/{build_id}/hydrate/
// <script>">` before `</body>`. The client snippet reads the state tag and
// calls its framework's hydrate (see contract.ts).

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import {
  escapeJsonForScriptTag,
  runInTrackCtx,
  SPA_STATE_SCRIPT_ID,
  weaveHead,
  type RenderRequest,
  type RenderResult,
} from "@mesofact/runtime";
import { BuildError } from "./load-routes.js";
import { prerenderKey } from "./route-key.js";

export type PrerenderInput = {
  route: string;
  // Absolute path to the bundled server module (output of `bundle.ts`).
  bundlePath: string;
  // Literal param maps. A non-parametric route uses [{}].
  params: ReadonlyArray<Record<string, string>>;
  // Mode 3 only — weave the client bundle into the emitted shell.
  hydration?: {
    buildId: string;
    // Content-hashed client entry filename, relative to `/{build_id}/hydrate/`.
    script: string;
  };
  // Absolute-path + relative-key pairs for declared `data_inputs`. Each file
  // is read as JSON and passed to render() as `req.data[relPath]`.
  dataInputs?: ReadonlyArray<{ relPath: string; absPath: string }>;
};

export type PrerenderEmission = {
  route: string;
  // Path written, relative to `outDir`.
  htmlPath: string;
  // Resolved URL for this emission (e.g. "/p/42").
  url: string;
  // Tags this emission claims — `result.cache.tags` ∪ trackCtx tags.
  tags: readonly string[];
};

export async function prerender(
  outDir: string,
  inputs: readonly PrerenderInput[],
): Promise<PrerenderEmission[]> {
  if (inputs.length === 0) return [];
  const htmlDir = join(outDir, "html");
  await mkdir(htmlDir, { recursive: true });

  const emissions: PrerenderEmission[] = [];
  for (const input of inputs) {
    const mod = (await import(pathToFileURL(input.bundlePath).href)) as {
      render?: unknown;
      default?: unknown;
    };
    const renderFn = pickRenderFn(mod);
    if (typeof renderFn !== "function") {
      throw new BuildError(
        `route ${input.route}: bundle at ${input.bundlePath} has no \`render\` export`,
      );
    }

    const data: Record<string, unknown> = {};
    for (const { relPath, absPath } of input.dataInputs ?? []) {
      const raw = await readFile(absPath, "utf8");
      data[relPath] = JSON.parse(raw) as unknown;
    }

    for (const params of input.params) {
      const url = expandRoute(input.route, params);
      const req: RenderRequest = {
        url,
        params,
        query: {},
        headers: {},
        cookies: {},
        ...(Object.keys(data).length > 0 ? { data } : {}),
      };
      const { value: result, ctx } = await runInTrackCtx(() => renderFn(req));
      assertRenderResult(input.route, url, result);

      // Head weave first (into </head>), hydration weave second (into
      // </body>); the two target disjoint regions of the document.
      let html = result.head ? weaveHead(result.html, result.head) : result.html;
      if (input.hydration) {
        html = injectHydration(
          html,
          input.hydration.buildId,
          input.hydration.script,
          result.hydration?.initial_state,
        );
      }

      const key = prerenderKey(input.route, params);
      const htmlPath = `dist/html/${key}.html`;
      await writeFile(join(htmlDir, `${key}.html`), html, "utf8");

      const combined = new Set<string>([...(result.cache.tags ?? []), ...ctx.tags]);
      emissions.push({
        route: input.route,
        htmlPath,
        url,
        tags: [...combined].sort(),
      });
    }
  }
  return emissions;
}

function pickRenderFn(mod: { render?: unknown; default?: unknown }): unknown {
  if (typeof mod.render === "function") return mod.render;
  if (typeof mod.default === "function") return mod.default;
  if (
    typeof mod.default === "object" &&
    mod.default !== null &&
    "render" in mod.default &&
    typeof (mod.default as { render: unknown }).render === "function"
  ) {
    return (mod.default as { render: unknown }).render;
  }
  return undefined;
}

function assertRenderResult(route: string, url: string, v: unknown): asserts v is RenderResult {
  if (
    typeof v !== "object" ||
    v === null ||
    typeof (v as { html?: unknown }).html !== "string" ||
    typeof (v as { cache?: unknown }).cache !== "object" ||
    (v as { cache: { ttl?: unknown } }).cache === null ||
    typeof (v as { cache: { ttl: unknown } }).cache.ttl !== "number"
  ) {
    throw new BuildError(
      `route ${route}: render(${JSON.stringify(url)}) did not return { html, cache: { ttl } }`,
    );
  }
}

// Weave the Mode 3 client bundle into a rendered shell: a JSON state tag the
// client reads on boot, then the module entry script. Both go before `</body>`
// (case-insensitive); a shell without one gets them appended.
function injectHydration(
  html: string,
  buildId: string,
  script: string,
  initialState: unknown,
): string {
  const tags: string[] = [];
  if (initialState !== undefined) {
    tags.push(
      `<script id="${SPA_STATE_SCRIPT_ID}" type="application/json">${escapeJsonForScriptTag(initialState)}</script>`,
    );
  }
  tags.push(`<script type="module" src="/${buildId}/hydrate/${script}"></script>`);
  const injection = tags.join("");

  const idx = html.toLowerCase().lastIndexOf("</body>");
  if (idx === -1) return html + injection;
  return html.slice(0, idx) + injection + html.slice(idx);
}

function expandRoute(route: string, params: Record<string, string>): string {
  // SPA shells have param-agnostic HTML; when no params are provided, return
  // the route pattern as the URL so the file is keyed by routeKey(route) alone.
  if (Object.keys(params).length === 0) return route;
  return route.replace(/:([A-Za-z0-9_]+)/g, (_, key: string) => {
    const value = params[key];
    if (value === undefined) {
      throw new BuildError(`route ${route}: missing param '${key}' in prerender map`);
    }
    return encodeURIComponent(value);
  });
}
