import type { RenderFn } from "@mesofact/runtime";

// The 404 page. It renders real, indexable-looking HTML and declares no
// `noindex` — deliberately, because the exclusion has to come from it being
// named in `error_routes`, not from the render opting out. That is the shape
// the sitemap must handle (R821-B2).
export const render: RenderFn = async () => ({
  html: "<!doctype html><html><head></head><body>not found</body></html>",
  cache: { ttl: 3600 },
  head: { title: "Not found" },
});
