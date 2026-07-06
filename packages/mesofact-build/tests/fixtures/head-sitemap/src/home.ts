import type { RenderFn } from "@mesofact/runtime";

// Full document with a real <head> so the head weave has a </head> to target.
// Returns a typed head value including hostile characters to prove the
// framework escapes them (the render never hand-assembles head markup).
export const render: RenderFn = async () => ({
  html: "<!doctype html><html><head><meta charset=\"utf-8\"></head><body>home</body></html>",
  cache: { ttl: 3600, tags: ["home"] },
  head: {
    title: "Home & <friends>",
    description: "The landing page",
    canonical: "https://example.test/",
    og: { title: "Home", type: "website", image: "https://example.test/og.png" },
  },
});
