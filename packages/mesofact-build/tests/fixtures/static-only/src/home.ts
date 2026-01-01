import type { RenderFn } from "@mesofact/runtime";

// Non-parametric Mode 1 home page. No adapter reads — source_reads stays
// empty after inference.
export const render: RenderFn = async () => ({
  html: "<!doctype html><title>fixture home</title>",
  cache: { ttl: 3600, tags: ["home"] },
});
