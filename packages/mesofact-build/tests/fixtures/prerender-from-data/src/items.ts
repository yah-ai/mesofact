import type { RenderFn } from "@mesofact/runtime";

type Item = { id: string; title: string };

export const render: RenderFn = async (req) => {
  const id = req.params.id ?? "?";
  const items = (req.data?.["data/items.json"] as { items?: Item[] } | undefined)?.items ?? [];
  const item = items.find((i) => i.id === id);
  const title = item?.title ?? "missing";
  return {
    html: `<!doctype html><title>items/${id}</title><h1>${id}</h1><p>${title}</p>`,
    cache: { ttl: 60, tags: [`item:${id}`] },
  };
};
