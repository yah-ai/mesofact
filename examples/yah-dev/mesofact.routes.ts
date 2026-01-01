import { defineRoutes } from "@mesofact/runtime";

export default defineRoutes({
  routes: [
    {
      route: "/",
      mode: "static",
      entrypoint: "src/render.ts",
      cache_policy: { ttl: 3600, swr: 86_400 },
    },
    {
      route: "/404",
      mode: "static",
      entrypoint: "src/not_found.ts",
      cache_policy: { ttl: 3600 },
    },
    {
      // Mode 3 (SPA shell) — built once like a static page, hydrated client-side.
      route: "/app",
      mode: "spa",
      entrypoint: "src/app_shell.ts",
      client_entrypoint: "src/app.client.ts",
      cache_policy: { ttl: 0 },
    },
  ],
  error_routes: {
    "404": "/404",
  },
});
