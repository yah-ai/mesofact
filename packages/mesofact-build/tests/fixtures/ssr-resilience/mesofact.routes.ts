import { defineRoutes } from "@mesofact/runtime";

// W181 resilience round-trip fixture: an SSR route declaring the v1 shape
// (retry + timeout). The build must carry the block verbatim into
// manifest.json so the Worker / mesofact-dev proxy can apply it.
export default defineRoutes({
  routes: [
    {
      route: "/api/submit",
      mode: "ssr",
      entrypoint: "src/submit.ts",
      cache_policy: { ttl: 0 },
      resilience: {
        timeout_ms: 5_000,
        retry: {
          attempts: 3,
          backoff_ms: [50, 200],
          retry_on: "connection",
        },
      },
    },
  ],
});
