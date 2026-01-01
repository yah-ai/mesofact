import type { RenderFn } from "@mesofact/runtime";

type Release = { id: string; title: string };

export const render: RenderFn = async (req) => {
  const items = (req.data?.["data/sample.json"] as Release[]) ?? [];
  const list = items.map((r) => `<li>${r.id}: ${r.title}</li>`).join("");
  return {
    html: `<!doctype html><title>releases</title><ul>${list}</ul>`,
    cache: { ttl: 3600, tags: ["releases"] },
  };
};
