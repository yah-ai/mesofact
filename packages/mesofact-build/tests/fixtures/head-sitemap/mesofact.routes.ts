import { defineRoutes } from "@mesofact/runtime";

// Head-contract + sitemap fixture (W270 §4). Four routes exercise every
// sitemap filter:
//   /          static, indexed         → in sitemap, head woven into <head>
//   /docs      static, indexed         → in sitemap
//   /secret    static, head.noindex    → excluded (robots noindex)
//   /c/:slug   static, deferred        → excluded (instance-addressed)
export default defineRoutes({
  site_url: "https://example.test",
  routes: [
    { route: "/", mode: "static", entrypoint: "src/home.ts", cache_policy: { ttl: 3600 } },
    { route: "/docs", mode: "static", entrypoint: "src/docs.ts", cache_policy: { ttl: 3600 } },
    { route: "/secret", mode: "static", entrypoint: "src/secret.ts", cache_policy: { ttl: 3600 } },
    {
      route: "/c/:slug",
      mode: "static",
      entrypoint: "src/c_slug.ts",
      cache_policy: { ttl: 3600 },
      prerender: { deferred: true },
    },
  ],
});
