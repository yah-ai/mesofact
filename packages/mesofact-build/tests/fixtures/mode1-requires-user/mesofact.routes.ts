import { defineRoutes } from "@mesofact/runtime";

// Failing fixture: Mode 1 + requires:["user"] must be rejected.
export default defineRoutes({
  routes: [
    {
      route: "/dashboard",
      mode: "static",
      entrypoint: "src/dashboard.ts",
      requires: ["user"],
      cache_policy: { ttl: 60 },
    },
  ],
});
