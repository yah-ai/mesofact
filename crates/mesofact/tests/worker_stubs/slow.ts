// Slow render entrypoint — used by the drain test to make sure the worker
// waits for in-flight renders before exiting.
import type { RenderFn } from "@mesofact/runtime";

const render: RenderFn = async (_req) => {
  await new Promise((r) => setTimeout(r, 80));
  return { html: "slow-ok", cache: { ttl: 0 } };
};

export default render;
