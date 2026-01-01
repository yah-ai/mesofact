import { defineRoutes } from "@mesofact/runtime";

export default defineRoutes({
  routes: [
    {
      route: "/item/:id",
      mode: "spa",
      entrypoint: "src/shell.ts",
      client_entrypoint: "src/app.client.ts",
      cache_policy: { ttl: 0 },
    },
  ],
});
