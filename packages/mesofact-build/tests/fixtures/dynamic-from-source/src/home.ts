import type { RenderFn } from "@mesofact/runtime";

// Mirror `static-only/src/home.ts` so the dynamic fixture's home page produces
// identical HTML.
export const render: RenderFn = async () => ({
  html: "<!doctype html><title>fixture home</title>",
  cache: { ttl: 3600, tags: ["home"] },
});
