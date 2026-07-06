import type { RenderFn } from "@mesofact/runtime";

// Deferred (instance-addressed) route: prerenders nothing at build time, so
// it contributes no sitemap URL regardless of its head.
export const render: RenderFn = async (req) => ({
  html: `<!doctype html><html><head></head><body>c/${req.params.slug ?? "?"}</body></html>`,
  cache: { ttl: 3600 },
  head: { title: "Chat", noindex: true },
});
