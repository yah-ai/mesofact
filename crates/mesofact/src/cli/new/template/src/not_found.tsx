// `static` — the error route `mesofact.routes.ts` maps 404 onto.

import { renderToStaticMarkup } from "react-dom/server";
import type { RenderFn } from "@mesofact/runtime";

import { Page, documentOf } from "./Page.js";

export const render: RenderFn = async () => ({
  html: documentOf(
    renderToStaticMarkup(
      <Page title="404" mode="static — prerendered at build">
        <p>That page isn&rsquo;t here.</p>
      </Page>,
    ),
  ),
  cache: { ttl: 3600, tags: ["page:404"] },
});
