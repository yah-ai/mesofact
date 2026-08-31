import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "<!doctype html><title>broken hook fixture</title>",
  cache: { ttl: 3600 },
});
