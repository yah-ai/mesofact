// Render contract — the single seam between mesofact and any frontend.
// See `.yah/docs/architecture/mesofact.md` §"The shared seam: one render
// contract" and §"Request context — what Rust pre-resolves".

import type { Head } from "./head.js";

export type Region = string;

export type User = {
  id: string;
  attrs: Record<string, unknown>;
};

export type Project = {
  id: string;
  home_region: Region;
  generation: string;
};

export type RenderRequest = {
  url: string;
  params: Record<string, string>;
  query: Record<string, string>;
  headers: Record<string, string>;
  cookies: Record<string, string>;

  // Proxy-resolved before render is invoked. Routes declare which of these
  // they require in the manifest; the proxy returns 401/404/redirect when a
  // required field can't resolve (render is never called).
  user?: User;
  project?: Project;
  region?: Region;

  // Per-deployment escape hatch for route-specific Rust middleware
  // (feature flags, A/B bucket). Not type-checked across the proxy↔render
  // boundary.
  ctx?: Record<string, unknown>;

  // Build-time data artifacts declared in the route's `data_inputs`.
  // Keys are the artifact paths (relative to project root); values are parsed
  // JSON. Populated only for mode="static" during prerender; absent at runtime.
  data?: Record<string, unknown>;
};

export type CachePolicy = {
  ttl: number;
  tags?: readonly string[];
};

// Mode 3 only. The render only ships `initial_state` — the build owns the
// resolved (content-hashed) entry `script` + `code_split` chunks and writes
// them into the manifest's `hydration`. A render MAY set `script` as a logical
// hint, but the manifest's build-derived value is what the shell references.
//
// The build serializes `initial_state` into a
// `<script id="__MESOFACT_STATE__" type="application/json">` tag in the shell
// HTML. The six-line client snippet that consumes it:
//
//   import { hydrateRoot } from "react-dom/client";          // or any framework
//   const el = document.getElementById("__MESOFACT_STATE__");
//   const initialState = el ? JSON.parse(el.textContent ?? "null") : null;
//   hydrateRoot(document.getElementById("root")!, <App initial={initialState} />);
//
// mesofact ships no runtime helper — the snippet lives in the client entry the
// route declares via `client_entrypoint` and the build bundles to `hydrate/`.
export type Hydration = {
  script?: string;
  initial_state?: unknown;
};

export type RenderResult = {
  html: string;
  headers?: Record<string, string>;
  cache: CachePolicy;
  hydration?: Hydration;
  // Typed <head> contract (W270 §4). Woven into the document head by the
  // prerenderer / SSG dispatch; the framework owns all escaping. Optional —
  // a render that manages its own <head> can omit it.
  head?: Head;
};

export type RenderFn = (req: RenderRequest) => Promise<RenderResult>;
