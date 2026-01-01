import { defineRoutes } from "@mesofact/runtime";

export default defineRoutes({
  routes: [
    {
      route: "/releases",
      mode: "static",
      entrypoint: "src/releases.ts",
      cache_policy: { ttl: 3600 },
      data_inputs: ["data/sample.json"],
    },
    {
      route: "/bare",
      mode: "static",
      entrypoint: "src/bare.ts",
      cache_policy: { ttl: 3600 },
    },
  ],
});
