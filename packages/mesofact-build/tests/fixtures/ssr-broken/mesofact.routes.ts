import { defineRoutes } from "@mesofact/runtime";

// SSR route whose entrypoint exports `render` (static-shape) instead of a
// default Fetch handler. The build must fail before the manifest hits disk.
export default defineRoutes({
  routes: [
    {
      route: "/api/broken",
      mode: "ssr",
      entrypoint: "src/broken.ts",
      cache_policy: { ttl: 0 },
    },
  ],
});
