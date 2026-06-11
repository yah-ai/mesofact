import { defineRoutes } from "@mesofact/runtime";

// R490-F4 fixture: a static route plus a public/ overlay. The build copies
// public/** verbatim into dist/html/ and lists each file in
// manifest.static_assets.
export default defineRoutes({
  routes: [
    {
      route: "/",
      mode: "static",
      entrypoint: "src/home.ts",
      cache_policy: { ttl: 3600 },
    },
  ],
});
