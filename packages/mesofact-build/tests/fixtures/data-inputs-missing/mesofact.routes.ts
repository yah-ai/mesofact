import { defineRoutes } from "@mesofact/runtime";

export default defineRoutes({
  routes: [
    {
      route: "/feed",
      mode: "static",
      entrypoint: "src/feed.ts",
      cache_policy: { ttl: 3600 },
      data_inputs: ["data/missing.json"],
    },
  ],
});
