import { defineRoutes } from "@mesofact/runtime";

// W173 Universal cell: mode:"ssr" + client_entrypoint. The Fetch handler
// renders HTML per request and inlines server-resolved data via the
// __mesofact_data__ tag; the client takes over on mount.
export default defineRoutes({
  routes: [
    {
      route: "/dashboard",
      mode: "ssr",
      entrypoint: "src/dashboard.ts",
      client_entrypoint: "src/dashboard.client.ts",
      cache_policy: { ttl: 0 },
    },
  ],
});
