import { defineRoutes } from "@mesofact/runtime";

// Exercises the SSR build path: one non-parametric ssr route (full-route
// prefix) + one parametric ssr route (truncated prefix). Plus a static route
// to confirm mixed-mode workloads still build.
export default defineRoutes({
  routes: [
    {
      route: "/",
      mode: "static",
      entrypoint: "src/home.ts",
      cache_policy: { ttl: 3600 },
    },
    {
      route: "/api/health",
      mode: "ssr",
      entrypoint: "src/health.ts",
      placement: "host",
      cache_policy: { ttl: 0 },
    },
    {
      route: "/api/users/:id",
      mode: "ssr",
      entrypoint: "src/users.ts",
      cache_policy: { ttl: 0 },
    },
  ],
});
