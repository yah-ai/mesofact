import type { RenderFn } from "@mesofact/runtime";

import { layout } from "./layout.js";

export const render: RenderFn = async () => ({
  html: layout({
    title: "Not found · mesofact",
    description: "404 stub for the mesofact example.",
    body: `
      <h1>404</h1>
      <p>That page isn't here.</p>
      <p><a href="/">Back to the example</a></p>
    `,
  }),
  cache: {
    ttl: 3600,
    tags: ["page:404", "site:mesofact-example"],
  },
});
