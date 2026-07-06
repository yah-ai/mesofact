import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async () => ({
  html: "<!doctype html><html><head></head><body>docs</body></html>",
  cache: { ttl: 3600 },
  head: { title: "Docs" },
});
