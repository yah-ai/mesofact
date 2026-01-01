import { defineRoutes } from "@mesofact/runtime";

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
      prerender: { params: [{ id: "1" }, { id: "2" }] },
    },
  ],
});
