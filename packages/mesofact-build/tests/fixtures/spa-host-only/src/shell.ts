import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "<!doctype html><body><div id='root'></div></body>",
  cache: { ttl: 0 },
});
