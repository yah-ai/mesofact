import { defineRoutes } from "@mesofact/runtime";

// Hook entrypoint with no default export. The build must fail before the
// manifest promises the engine a module it cannot invoke.
export default defineRoutes({
  routes: [
    {
      route: "/",
      mode: "static",
      entrypoint: "src/home.ts",
      cache_policy: { ttl: 3600 },
    },
  ],
  hooks: {
    readyz: "src/readyz.ts",
  },
});
