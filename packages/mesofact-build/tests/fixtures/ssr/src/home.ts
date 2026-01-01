import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "<!doctype html><title>ssr fixture home</title>",
  cache: { ttl: 3600, tags: ["home"] },
});
