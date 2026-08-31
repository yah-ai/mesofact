// Mode 3 — `spa`. Two halves, and keeping them straight is most of what this
// mode is:
//
//   1. THIS file is the SHELL. It renders at build time, exactly like `static`,
//      into a document containing the app's initial markup plus an
//      `initial_state` blob. Nothing about it is dynamic.
//   2. src/app.client.tsx is the CLIENT ENTRY, declared as `client_entrypoint`
//      in mesofact.routes.ts. The build bundles it to `dist/hydrate/` under a
//      content-hashed name and weaves a `<script type="module">` for it into
//      this shell.
//
// After hydration the SPA owns the page and mesofact is out of the request
// path entirely. The shell is a CDN object; the app is a browser program.

import { renderToString } from "react-dom/server";
import type { RenderFn } from "@mesofact/runtime";

import { Page, documentOf } from "./Page.js";
import { App, type AppState } from "./App.js";

// The one definition of the starting state. It is both rendered into the shell
// and shipped in `hydration.initial_state`, so the client cannot disagree with
// the server about what the page said before it loaded.
const INITIAL: AppState = { greeting: "hello from the client bundle", clicks: 0 };

export const render: RenderFn = async () => ({
  // `renderToString`, not `renderToStaticMarkup`: the static variant strips the
  // hydration hints React looks for, and the symptom is a silent full client
  // re-render instead of a hydrate — same pixels, none of the savings.
  html: documentOf(
    renderToString(
      <Page title="hello, browser" mode="spa — shell prerendered, then hydrated">
        {/* `#root` is the container the client entry hands to hydrateRoot. */}
        <div id="root">
          <App initial={INITIAL} />
        </div>
      </Page>,
    ),
  ),
  cache: { ttl: 0 },
  hydration: { initial_state: INITIAL },
});
