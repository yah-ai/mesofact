import { defineRoutes } from "@mesofact/runtime";

// W173 Islands cell: mode:"static" + client_entrypoint. The route prerenders
// with build-time data baked into HTML, and the client bundle hydrates per-
// route interactivity on top.
export default defineRoutes({
  routes: [
    {
      route: "/issues",
      mode: "static",
      entrypoint: "src/shell.ts",
      client_entrypoint: "src/shell.client.ts",
      data_inputs: ["data/items.json"],
      cache_policy: { ttl: 3600 },
    },
  ],
});
