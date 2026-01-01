import { defineRoutes } from "@mesofact/runtime";

// prerender.from_data references a file that's NOT in data_inputs — should be
// rejected at defineRoutes time so the build fails on routes file import.
export default defineRoutes({
  routes: [
    {
      route: "/items/:id",
      mode: "static",
      entrypoint: "src/items.ts",
      cache_policy: { ttl: 60 },
      // Note: data/items.json is intentionally omitted from data_inputs.
      prerender: {
        from_data: "data/items.json",
        items_key: "items",
        param: "id",
      },
    },
  ],
});
