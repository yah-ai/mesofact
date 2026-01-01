import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "should-not-render",
  cache: { ttl: 60 },
});
