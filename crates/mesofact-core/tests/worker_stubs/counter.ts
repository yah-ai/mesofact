// Stub render entrypoint with a TTL so the proxy caches it. A module-level
// counter makes each *render* distinct, so a cache hit (which serves the stored
// body without calling the worker) returns the earlier "render-N" — proving the
// LRU served it rather than re-rendering.
import type { RenderFn } from "@mesofact/runtime";

let n = 0;

const render: RenderFn = async (_req) => {
  n += 1;
  return { html: `render-${n}`, cache: { ttl: 60 } };
};

export default render;
