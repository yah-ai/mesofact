import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "<!doctype html><p>feed</p>",
  cache: { ttl: 3600, tags: [] },
});
