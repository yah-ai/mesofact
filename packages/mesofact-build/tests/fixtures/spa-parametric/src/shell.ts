import type { RenderFn } from "@mesofact/runtime";

// Parametric SPA shell — HTML is identical for all :id values; the client
// reads the ID from the URL at runtime.
export const render: RenderFn = async () => ({
  html:
    "<!doctype html><html><head><title>item detail</title></head>" +
    '<body><div id="root"></div></body></html>',
  cache: { ttl: 0 },
  hydration: { initial_state: {} },
});
