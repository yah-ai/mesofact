import type { RenderFn } from "@mesofact/runtime";

// Enumerable static route that opts out of indexing. Head is still woven, but
// the emission is dropped from the sitemap.
export const render: RenderFn = async () => ({
  html: "<!doctype html><html><head></head><body>secret</body></html>",
  cache: { ttl: 3600 },
  head: { title: "Secret", noindex: true },
});
