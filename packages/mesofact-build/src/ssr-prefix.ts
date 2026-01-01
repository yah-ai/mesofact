// W173 § "SSR_PREFIXES derivation rule" — turn a route pattern into the
// path prefix the proxy / Worker uses to decide whether to forward to the
// SSR runtime. Segment-aware match at the call site:
// `path === prefix || path.startsWith(prefix + "/")`.
//
// Rules:
//   - Non-parametric route → the full route is the prefix.
//   - Parametric route (`:foo`) or wildcard (`*`) → prefix is the route up
//     to (but not including) the first such segment.
//
//   /api/health     → /api/health   (exact match only, segment-aware)
//   /api/users/:id  → /api/users/   (matches /api/users/42, NOT /api/usersx)
//   /x/:a/y         → /x/           (over-broad by design; see W173)
//   /feed/*         → /feed/        (matches /feed/anything)

import type { RouteEntry } from "@mesofact/runtime";

export function deriveSsrPrefix(route: string): string {
  const segments = route.split("/");
  const out: string[] = [];
  let truncated = false;
  for (const seg of segments) {
    if (seg.startsWith(":") || seg === "*" || seg.includes("*")) {
      truncated = true;
      break;
    }
    out.push(seg);
  }
  // A truncated route ends at the last static segment + trailing slash so the
  // matcher's `path === prefix || path.startsWith(prefix + "/")` test
  // semantically asserts "the next segment is anything." A non-truncated
  // route keeps its exact shape so `/api/health` doesn't match
  // `/api/healthcheck`.
  if (truncated) return `${out.join("/")}/`;
  return out.join("/");
}

export function deriveSsrPrefixes(routes: readonly RouteEntry[]): readonly string[] {
  const seen = new Set<string>();
  for (const r of routes) {
    if (r.mode !== "ssr") continue;
    seen.add(deriveSsrPrefix(r.route));
  }
  return [...seen].sort();
}
