// Stub render entrypoint that echoes the proxy-resolved user id, for the
// session / `requires: ["user"]` integration test. ttl 0 so it never caches.
import type { RenderFn } from "@mesofact/runtime";

const render: RenderFn = async (req) => ({
  html: `user:${req.user?.id ?? "anon"}`,
  cache: { ttl: 0 },
});

export default render;
