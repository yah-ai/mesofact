import { defineRoutes } from "@mesofact/runtime";

// JSON file is declared in data_inputs but doesn't contain `items_key` →
// expandFromData should throw a BuildError naming the file + path.
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
