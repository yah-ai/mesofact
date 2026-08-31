import { defineRoutes } from "@mesofact/runtime";

// The Mode 2 hook declaration site (R756-F6 / W311 §2). `readyz` is declared
// as a HOOK, not as a route: the module is engine-addressed, so it must not
// appear in `routes`, must not enter `ssr_prefixes`, and must bundle to
// `dist/server/hooks/readyz.js`.
export default defineRoutes({
  routes: [
    {
      route: "/",
      mode: "static",
      entrypoint: "src/home.ts",
      cache_policy: { ttl: 3600 },
    },
  ],
  hooks: {
    readyz: "src/readyz.ts",
  },
});
