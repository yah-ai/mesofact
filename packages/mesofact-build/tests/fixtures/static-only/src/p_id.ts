import type { RenderFn } from "@mesofact/runtime";

// Parametric Mode 1 route. Source-inference should pick `assets` out of this
// file (`r2('assets')`). At test-time we stub the registry, so the call goes
// to a fake adapter that only emits the tag — we don't actually fetch from R2.
export const render: RenderFn = async (req) => {
  const id = req.params.id ?? "?";
  // @mesofact-sources assets
  return {
    html: `<!doctype html><title>p/${id}</title><h1>${id}</h1>`,
    cache: { ttl: 60, tags: [`page:${id}`] },
  };
};
