import type { RenderFn } from "@mesofact/runtime";

// Publish-once instance page: the slug arrives as a route param and the
// page body arrives as explicit render data (the publish op's req.data),
// never from a build-time file.
export const render: RenderFn = async (req) => {
  const slug = req.params.slug ?? "?";
  const body = (req.data?.["chat"] as { title?: string } | undefined)?.title ?? "(no data)";
  return {
    html: `<!doctype html><title>c/${slug}</title><h1>${body}</h1>`,
    cache: { ttl: 3600, tags: [`chat:${slug}`] },
  };
};
