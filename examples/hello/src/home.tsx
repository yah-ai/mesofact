// Mode 1 — `static`. Rendered ONCE at build time into `dist/html/index.html`.
// At serve time nothing runs: the file is handed to the client as bytes. This
// is the only mode a vanilla W272 bundle can serve today (`mesofact serve
// --bundle`, static v0) — see the README.

import { renderToStaticMarkup } from "react-dom/server";
import type { RenderFn } from "@mesofact/runtime";

import { Page, documentOf } from "./Page.js";

export const render: RenderFn = async () => ({
  html: documentOf(
    renderToStaticMarkup(
      <Page title="hello from mesofact" mode='static — prerendered at build'>
        <p>
          This page was rendered when you ran <code>mesofact-build build</code>.
          Reload it as many times as you like; the bytes are identical, because
          no code runs to produce them.
        </p>
      </Page>,
    ),
  ),
  cache: { ttl: 3600, tags: ["page:home"] },
});
