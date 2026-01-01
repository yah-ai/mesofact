import { defineRoutes } from "@mesofact/runtime";

// A spa route that omits client_entrypoint — the build must reject it before
// emitting anything.
export default defineRoutes({
  routes: [
    {
      route: "/app",
      mode: "spa",
      entrypoint: "src/shell.ts",
      cache_policy: { ttl: 0 },
    },
  ],
});
