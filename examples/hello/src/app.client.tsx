/// <reference lib="dom" />
// Mode 3 client entry — the `client_entrypoint` of the `/app` route. The build
// bundles this for the browser (content-hashed, into `dist/hydrate/`) and
// weaves a module script for it into the shell src/app_shell.tsx produced.
//
// This is the six-line snippet from @mesofact/runtime's contract.ts, made real.
// mesofact ships no hydration helper on purpose: reading one JSON tag and
// calling your framework's hydrate function is not a thing a framework-agnostic
// runtime should have an opinion about.

import { hydrateRoot } from "react-dom/client";

import { App, type AppState } from "./App.js";

const el = document.getElementById("__MESOFACT_STATE__");
const initial: AppState = el?.textContent
  ? (JSON.parse(el.textContent) as AppState)
  : { greeting: "no state found", clicks: 0 };

const root = document.getElementById("root");
if (root) {
  hydrateRoot(root, <App initial={initial} />);
}
