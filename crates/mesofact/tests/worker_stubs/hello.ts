// Stub render entrypoint for the worker harness test (render → ok).
import type { RenderFn } from "@mesofact/runtime";

const render: RenderFn = async (_req) => ({
  html: "hi",
  cache: { ttl: 0 },
});

export default render;
