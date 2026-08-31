import { defineRoutes } from "@mesofact/runtime";

// Head-contract + sitemap fixture (W270 §4). Six routes exercise every
// sitemap filter:
//   /          static, indexed         → in sitemap, head woven into <head>
//   /docs      static, indexed         → in sitemap
//   /app       spa, indexed            → in sitemap (prerendered at a fixed
//                                        URL, so as enumerable as a static
//                                        route — R821-B2)
//   /404       static, error_routes    → excluded (the site's failure page)
//   /secret    static, head.noindex    → excluded (robots noindex)
//   /c/:slug   static, deferred        → excluded (instance-addressed)
export default defineRoutes({
  site_url: "https://example.test",
  routes: [
    { route: "/", mode: "static", entrypoint: "src/home.ts", cache_policy: { ttl: 3600 } },
    { route: "/docs", mode: "static", entrypoint: "src/docs.ts", cache_policy: { ttl: 3600 } },
    {
      route: "/app",
      mode: "spa",
      entrypoint: "src/app_shell.ts",
      client_entrypoint: "src/app.client.ts",
      cache_policy: { ttl: 0 },
    },
    { route: "/404", mode: "static", entrypoint: "src/not_found.ts", cache_policy: { ttl: 3600 } },
    { route: "/secret", mode: "static", entrypoint: "src/secret.ts", cache_policy: { ttl: 3600 } },
    {
      route: "/c/:slug",
      mode: "static",
      entrypoint: "src/c_slug.ts",
      cache_policy: { ttl: 3600 },
      prerender: { deferred: true },
    },
  ],
  error_routes: {
    "404": "/404",
  },
});
