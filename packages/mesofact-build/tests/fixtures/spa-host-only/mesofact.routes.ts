import { defineRoutes } from "@mesofact/runtime";

// SPA whose client_entrypoint reaches a host-only API through one indirection.
// The lint should follow the import chain and reject the build.
export default defineRoutes({
  routes: [
    {
      route: "/app",
      mode: "spa",
      entrypoint: "src/shell.ts",
      client_entrypoint: "src/app.client.ts",
      cache_policy: { ttl: 0 },
    },
  ],
});
