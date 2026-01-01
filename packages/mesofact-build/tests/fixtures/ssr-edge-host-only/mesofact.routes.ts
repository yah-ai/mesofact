import { defineRoutes } from "@mesofact/runtime";

// SSR + placement:"edge" that imports a host-only db driver. Workerd can't
// link native modules, so the lint must reject this before manifest emission.
export default defineRoutes({
  routes: [
    {
      route: "/api/users/:id",
      mode: "ssr",
      entrypoint: "src/users.ts",
      placement: "edge",
      cache_policy: { ttl: 60 },
    },
  ],
});
