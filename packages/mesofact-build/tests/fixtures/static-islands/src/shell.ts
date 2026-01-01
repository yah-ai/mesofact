import type { RenderFn } from "@mesofact/runtime";

type Items = { items: { id: string; title: string }[] };

// Cell 2 render: bake the data-driven list into static HTML at build time.
// `hydration.initial_state` carries a small handoff (here, the item count) so
// the client bundle can wire up per-route interactivity without re-fetching.
export const render: RenderFn = async (req) => {
  const data = req.data?.["data/items.json"] as Items | undefined;
  const items = data?.items ?? [];
  const list = items.map((it) => `<li data-id="${it.id}">${it.title}</li>`).join("");
  return {
    html:
      `<!doctype html><html><head><title>issues</title></head>` +
      `<body><ul id="issues">${list}</ul></body></html>`,
    cache: { ttl: 3600, tags: ["issues"] },
    hydration: {
      initial_state: { count: items.length },
    },
  };
};
