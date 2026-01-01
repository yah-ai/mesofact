import { defineRoutes } from "@mesofact/runtime";

// Enumerate parametric Mode 1 routes from a local JSON file declared in
// data_inputs. The same file feeds req.data at render time, so the build
// doesn't open a network adapter for what's already on disk.
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
