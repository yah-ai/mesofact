import type { RenderFn } from "@mesofact/runtime";

// Wrong shape for mode:"ssr" — exports `render` (the static/spa contract)
// instead of a default Fetch handler.
export const render: RenderFn = async () => ({
  html: "<!doctype html>not actually ssr",
  cache: { ttl: 0 },
});
