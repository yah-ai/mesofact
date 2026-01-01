import type { RenderFn } from "@mesofact/runtime";

// Verifies req.data is absent when no data_inputs are declared.
export const render: RenderFn = async (req) => {
  const marker = req.data === undefined ? "no-data" : "has-data";
  return {
    html: `<!doctype html><p>${marker}</p>`,
    cache: { ttl: 3600, tags: ["bare"] },
  };
};
