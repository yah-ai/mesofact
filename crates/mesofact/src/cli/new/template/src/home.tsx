// `static` — rendered ONCE at build time into `dist/html/index.html`. At serve
// time nothing runs: the file is handed to the client as bytes.

import { renderToStaticMarkup } from "react-dom/server";
import type { RenderFn } from "@mesofact/runtime";

import { Page, documentOf } from "./Page.js";

export const render: RenderFn = async () => ({
  html: documentOf(
    renderToStaticMarkup(
      <Page title="__PROJECT_NAME__" mode="static — prerendered at build">
        <p>
          This page was rendered when the build ran. Reload it as many times as
          you like; the bytes are identical, because no code runs to produce
          them.
        </p>
        <p>
          Edit <code>src/home.tsx</code> and <code>mesofact-dev</code> rebuilds
          and reloads.
        </p>
      </Page>,
    ),
  ),
  cache: { ttl: 3600, tags: ["page:home"] },
});
