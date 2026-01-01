// Phase 5 (continued) — `tag-index.json`. Maps each tag to the resolved URLs
// (Mode 1 only) that emitted it at this build. The publisher (P6) consumes
// this on backend-change events to find which routes to re-render.
//
// Shape:
//   {
//     "build_id": "...",
//     "tags": { "<tag>": ["/p/1", "/p/2", ...], ... }
//   }
//
// Each tag's URL list is sorted to keep the file deterministic across runs.

import type { PrerenderEmission } from "./prerender.js";

export type TagIndex = {
  build_id: string;
  tags: Record<string, string[]>;
};

export function buildTagIndex(buildId: string, emissions: readonly PrerenderEmission[]): TagIndex {
  const tags: Record<string, Set<string>> = {};
  for (const e of emissions) {
    for (const tag of e.tags) {
      (tags[tag] ??= new Set()).add(e.url);
    }
  }
  const ordered: Record<string, string[]> = {};
  for (const tag of Object.keys(tags).sort()) {
    ordered[tag] = [...tags[tag]!].sort();
  }
  return { build_id: buildId, tags: ordered };
}
