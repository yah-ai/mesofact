import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "<!doctype html><title>deferred fixture home</title>",
  cache: { ttl: 3600, tags: ["home"] },
});
