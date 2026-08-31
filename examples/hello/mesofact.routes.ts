// Every mesofact route mode, once each, in one file. This is the whole config
// surface — there is no other place routing is declared.
//
// `mesofact.routes.ts` is a BUILD INPUT, not runtime config: the build reads it
// to decide what to prerender, which entrypoints to bundle for the server and
// which for the browser, and which prefixes the SSR runtime must own. Changing
// it means rebuilding, which is why "just move a route to ssr" is not a config
// edit you can hot-swap into a deployed bundle.

import { defineRoutes } from "@mesofact/runtime";

export default defineRoutes({
  routes: [
    {
      // Mode 1 — rendered at build, served as a file. Nothing runs per request.
      route: "/",
      mode: "static",
      entrypoint: "src/home.tsx",
      cache_policy: { ttl: 3600, swr: 86_400 },
    },
    {
      route: "/404",
      mode: "static",
      entrypoint: "src/not_found.tsx",
      cache_policy: { ttl: 3600 },
    },
    {
      // Mode 2 — a Fetch handler run per request in a V8 isolate. Returns HTML
      // here; `/api/hello` below returns JSON from the identical contract.
      route: "/live",
      mode: "ssr",
      entrypoint: "src/live.tsx",
      cache_policy: { ttl: 0 },
    },
    {
      route: "/api/hello",
      mode: "ssr",
      entrypoint: "src/hello_api.ts",
      cache_policy: { ttl: 0 },
    },
    {
      // Mode 3 — a static shell plus a browser bundle that takes it over.
      route: "/app",
      mode: "spa",
      entrypoint: "src/app_shell.tsx",
      client_entrypoint: "src/app.client.tsx",
      cache_policy: { ttl: 0 },
    },
  ],
  // The dual-language seam. mesofact serves /livez + /readyz itself, in Rust;
  // this block opts the app into *contributing* a check to the readiness
  // answer. A hook is not a route — mesofact decides when to call it and owns
  // the HTTP around it — so it is declared here rather than in `routes`, and
  // it never enters ssr_prefixes or gets forwarded to the SSR origin by the
  // edge. See src/readyz.ts.
  hooks: {
    readyz: "src/readyz.ts",
  },
  error_routes: {
    "404": "/404",
  },
});
