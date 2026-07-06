import { defineRoutes } from "@mesofact/runtime";

// Instance-addressed fixture (W225 §3a publish-once): `/c/:slug` declares
// its params as publish-time-deferred, so the build emits the server bundle
// + manifest entry but prerenders zero instances. Instances are produced
// afterwards through the render-only entrypoint with explicit params/data.
export default defineRoutes({
  routes: [
    {
      route: "/",
      mode: "static",
      entrypoint: "src/home.ts",
      cache_policy: { ttl: 3600 },
    },
    {
      route: "/c/:slug",
      mode: "static",
      entrypoint: "src/c_slug.ts",
      cache_policy: { ttl: 3600 },
      prerender: { deferred: true },
    },
  ],
});
