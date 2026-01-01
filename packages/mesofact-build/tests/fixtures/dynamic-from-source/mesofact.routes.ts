import { defineRoutes } from "@mesofact/runtime";

// Same shape as `static-only/`, but the `/p/:id` route expands its params via
// `r2.list` instead of a literal list. The test stubs the `assets` adapter to
// return the keys "1" and "2", so the expanded HTML should match `static-only`
// byte-for-byte.
export default defineRoutes({
  routes: [
    {
      route: "/",
      mode: "static",
      entrypoint: "src/home.ts",
      cache_policy: { ttl: 3600 },
    },
    {
      route: "/p/:id",
      mode: "static",
      entrypoint: "src/p_id.ts",
      cache_policy: { ttl: 60 },
      prerender: { from: "assets", query: "list:", param: "id" },
    },
  ],
});
