import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "<!doctype html><title>hooks fixture</title>",
  cache: { ttl: 3600, tags: ["home"] },
});
