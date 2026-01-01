import type { RenderFn } from "@mesofact/runtime";

// Echoes the proxy-injected W3C trace context (req.ctx.trace) into the body so
// a Rust test can assert the traceparent reached the worker.
export const render: RenderFn = async (req) => ({
  html: typeof req.ctx?.trace === "string" ? req.ctx.trace : "no-trace",
  cache: { ttl: 0 },
});
