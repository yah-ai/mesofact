//! @yah:ticket(R012-T4, "SPA example: examples/yah-dev Mode 3 /app route (shell + client hydration entry) + extend smoke-yah-dev.sh to assert hydration/dist-hydrate/shell-state")
//! @yah:at(2026-05-26T16:22:22Z)
//! @yah:status(review)
//! @yah:assignee(agent:claude)
//! @yah:phase(P10)
//! @yah:parent(R012)
//! @yah:handoff("SPA example + e2e smoke shipped. examples/yah-dev gains a Mode 3 /app route: src/app_shell.ts (server render → static shell with a #root mount + hydration.initial_state) and src/app.client.ts (framework-free browser hydration entry — reads __MESOFACT_STATE__, fills #root; DOM lib via triple-slash ref so it typechecks under lib:ES2022). mesofact.routes.ts adds the /app spa route with client_entrypoint. scripts/smoke-yah-dev.sh extended: route list now '/:static,/404:static,/app:spa'; new step asserts manifest.hydration.script is content-hashed (^app\\.[0-9a-z]+\\.js$) + the hashed file exists under dist/hydrate/, app.html carries the __MESOFACT_STATE__ JSON tag + the woven <script type=module src=/{build_id}/hydrate/{script}>. Verified: build emits dist/hydrate/app.<hash>.js, manifest /app route hydration {script, code_split:[]}, shell woven before </body>; in-memory publish uploads the hydrate file (cache_control_for already had the hydrate/ immutable branch from R008-T7). One route of each mode now exists in the example (Mode 1 / + /404, Mode 3 /app); Mode 2 SSR is exercised by the proxy test suite.")
//! @yah:verify("bash scripts/smoke-yah-dev.sh")
//! @yah:verify("cd examples/yah-dev && bun run typecheck")
//! @yah:assumes("Actual browser hydration (DoD: 'SPA loads, hydrates without console errors, fetches its API') is an operator/browser check like R010-T2's live curl steps — the build/publish/shell-weaving is verified headless here. The example client mutates the DOM rather than fetching an API to stay dependency-free.")

import type { RenderFn } from "@mesofact/runtime";

import { layout } from "./layout.js";

// Mode 3 (spa) shell. The server render emits a static document with an empty
// mount point plus the initial state the client hydrates against. The build
// weaves in the `__MESOFACT_STATE__` tag + the content-hashed hydrate script;
// after hydration the SPA owns the page and mesofact is out of the request
// path. See `.yah/docs/architecture/mesofact.md` §"Bundle splitting & hydration
// boundary (Mode 3)".
export const render: RenderFn = async () => ({
  html: layout({
    title: "mesofact · app",
    description: "Minimal mesofact Mode 3 (SPA shell) example — hydrates client-side.",
    body: `
      <h1>mesofact app shell</h1>
      <p id="root">Loading…</p>
      <p>
        This is the <code>Mode 3</code> example route — a static shell pushed to
        the CDN like Mode 1, then hydrated in the browser. The paragraph above
        is replaced by the client bundle reading <code>__MESOFACT_STATE__</code>.
      </p>
    `,
  }),
  cache: { ttl: 0 },
  hydration: {
    initial_state: {
      hydrated_at: "build",
      message: "hydrated from __MESOFACT_STATE__",
    },
  },
});
