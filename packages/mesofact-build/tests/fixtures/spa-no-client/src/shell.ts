import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "<!doctype html><html><body><div id=\"root\"></div></body></html>",
  cache: { ttl: 0 },
});
