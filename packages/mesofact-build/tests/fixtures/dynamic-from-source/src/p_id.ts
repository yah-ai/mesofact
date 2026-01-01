import type { RenderFn } from "@mesofact/runtime";

// Identical to `static-only/src/p_id.ts` — equivalence test compares the
// emitted HTML byte-for-byte against that fixture.
export const render: RenderFn = async (req) => {
  const id = req.params.id ?? "?";
  // @mesofact-sources assets
  return {
    html: `<!doctype html><title>p/${id}</title><h1>${id}</h1>`,
    cache: { ttl: 60, tags: [`page:${id}`] },
  };
};
