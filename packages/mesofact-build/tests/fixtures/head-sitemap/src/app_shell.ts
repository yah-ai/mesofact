import type { RenderFn } from "@mesofact/runtime";

// Mode 3 shell at a fixed URL. Prerendered like any static route, so it is
// exactly as enumerable — this is the route R821-B2 was silently dropping
// from the sitemap because the gate keyed on mode instead of on whether
// anything was prerendered.
export const render: RenderFn = async () => ({
  html:
    "<!doctype html><html><head></head>" +
    '<body><div id="root"></div></body></html>',
  cache: { ttl: 0 },
  head: { title: "App" },
  hydration: { initial_state: { ready: true } },
});
