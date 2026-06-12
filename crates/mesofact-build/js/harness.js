// SSG harness module ("mesofact:harness"). Loaded once per build runtime;
// installs `globalThis.__mesofact` — the call surface the Rust prerender
// driver drives via execute_script + resolve_value. All HTML weaving happens
// here (not in Rust) so the bytes match the Bun pipeline's prerender.ts
// exactly: same escape helper, same injection point, same error wording.

import { escapeJsonForScriptTag, runInTrackCtx, SPA_STATE_SCRIPT_ID } from "@mesofact/runtime";

function pickRenderFn(mod) {
  if (typeof mod.render === "function") return mod.render;
  if (typeof mod.default === "function") return mod.default;
  if (
    typeof mod.default === "object" &&
    mod.default !== null &&
    "render" in mod.default &&
    typeof mod.default.render === "function"
  ) {
    return mod.default.render;
  }
  return undefined;
}

function assertRenderResult(route, url, v) {
  if (
    typeof v !== "object" ||
    v === null ||
    typeof v.html !== "string" ||
    typeof v.cache !== "object" ||
    v.cache === null ||
    typeof v.cache.ttl !== "number"
  ) {
    throw new Error(`route ${route}: render(${JSON.stringify(url)}) did not return { html, cache: { ttl } }`);
  }
}

// Mirror of prerender.ts's injectHydration — state tag + module entry script
// before the last </body> (case-insensitive), appended when absent.
function injectHydration(html, buildId, script, initialState) {
  const tags = [];
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

globalThis.__mesofact = {
  // Evaluate a bundled mesofact.routes.ts module and hand back its config as
  // plain JSON (default export, or named `routes` / `config`).
  async evalRoutes(modUrl) {
    const mod = await import(modUrl);
    const candidate = mod.default ?? mod.routes ?? mod.config;
    if (typeof candidate !== "object" || candidate === null || !Array.isArray(candidate.routes)) {
      const kind =
        candidate === null ? "null" : Array.isArray(candidate) ? "array" : typeof candidate;
      throw new Error(`expected default (or named 'routes') export of RoutesConfig — got ${kind}`);
    }
    return JSON.parse(JSON.stringify(candidate));
  },

  // Probe a bundled SSR entrypoint: the manifest contract requires
  // `export default (req: Request) => Promise<Response>`.
  async probeDefault(modUrl) {
    const mod = await import(modUrl);
    const d = mod.default;
    if (typeof d === "function") return { kind: "function" };
    if (d === undefined) return { kind: "no default export" };
    if (d === null) return { kind: "null" };
    return { kind: typeof d };
  },

  // Render one prerender emission. `input` mirrors prerender.ts's loop body:
  // { route, url, req, hydration?: { buildId, script } }. Returns the final
  // HTML (hydration woven in) plus the merged, sorted tag set.
  async render(modUrl, input) {
    const mod = await import(modUrl);
    const renderFn = pickRenderFn(mod);
    if (typeof renderFn !== "function") {
      throw new Error(`route ${input.route}: bundle at ${modUrl} has no \`render\` export`);
    }
    const { value: result, ctx } = await runInTrackCtx(() => renderFn(input.req));
    assertRenderResult(input.route, input.url, result);

    const html = input.hydration
      ? injectHydration(
          result.html,
          input.hydration.buildId,
          input.hydration.script,
          result.hydration?.initial_state,
        )
      : result.html;

    const combined = new Set([...(result.cache.tags ?? []), ...ctx.tags]);
    return { html, tags: [...combined].sort() };
  },
};
