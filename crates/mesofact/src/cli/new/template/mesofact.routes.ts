// The whole routing surface. There is no other place routes are declared.
//
// This file is a BUILD INPUT, not runtime config: the build reads it to decide
// what to prerender, which entrypoints to bundle for the server, and which
// prefixes the SSR runtime owns. Changing a route's mode means rebuilding, not
// restarting.

import { defineRoutes } from "@mesofact/runtime";

export default defineRoutes({
  routes: [
    {
      // Rendered once at build time into `dist/html/index.html`. Nothing runs
      // per request — the server hands over bytes.
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
      // A Fetch handler run per request in a V8 isolate the runtime owns.
      // `(Request) => Response` — it owns status, headers and body.
      route: "/api/hello",
      mode: "ssr",
      entrypoint: "src/api.ts",
      cache_policy: { ttl: 0 },
    },
  ],
  error_routes: {
    "404": "/404",
  },
});
