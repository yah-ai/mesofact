import { defineRoutes } from "@mesofact/runtime";

// items_key resolves, but items[*].id is a number — `id` in a route param map
// has to be a string, so expandFromData should reject with a clear error.
export default defineRoutes({
  routes: [
    {
      route: "/items/:id",
      mode: "static",
      entrypoint: "src/items.ts",
      cache_policy: { ttl: 60 },
      data_inputs: ["data/items.json"],
      prerender: {
        from_data: "data/items.json",
        items_key: "items",
        param: "id",
      },
    },
  ],
});
