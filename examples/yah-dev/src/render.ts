import type { RenderFn } from "@mesofact/runtime";

import { layout } from "./layout.js";

export const render: RenderFn = async () => ({
  html: layout({
    title: "mesofact · hello",
    description: "Minimal mesofact-static example — Mode 1 (prerendered) render.",
    body: `
      <h1>hello from mesofact</h1>
      <p>
        This is the mesofact integration-test example — a minimal showcase
        of <code>Mode 1</code> (per-route prerender → CDN). Two routes,
        <code>/</code> and <code>/404</code>, both prerendered.
      </p>
      <p>
        The yah.dev marketing site that originally lived here has moved
        to <code>app/yah/web/</code> in the
        <a href="https://github.com/anthropics/yah">yah</a> repo. This
        example stays put so mesofact still has a build + publish smoke
        target outside of its consumer.
      </p>
    `,
  }),
  cache: {
    ttl: 3600,
    tags: ["page:home", "site:mesofact-example"],
  },
});
