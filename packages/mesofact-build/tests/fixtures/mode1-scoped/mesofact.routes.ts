import { defineRoutes } from "@mesofact/runtime";

// Failing fixture: Mode 1 reading a `project`-scoped source must be rejected
// by validate().
export default defineRoutes({
  routes: [
    {
      route: "/p/:id",
      mode: "static",
      entrypoint: "src/p_id.ts",
      cache_policy: { ttl: 60 },
      prerender: { params: [{ id: "1" }] },
    },
  ],
});
