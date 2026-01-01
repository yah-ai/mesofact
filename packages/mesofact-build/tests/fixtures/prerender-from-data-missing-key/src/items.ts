import type { RenderFn } from "@mesofact/runtime";

export const render: RenderFn = async (req) => ({
  html: `<!doctype html><h1>${req.params.id ?? "?"}</h1>`,
  cache: { ttl: 60 },
});
