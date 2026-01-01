// Map a route pattern to a filesystem-safe key:
//   "/"          → "index"
//   "/about"     → "about"
//   "/p/:id"     → "p_id"
//   "/blog/:slug/*" → "blog_slug_star"
//
// Used for both the bundled entrypoint name (`dist/server/<key>.js`) and the
// prerendered HTML name (`dist/html/<key>.html`). Param values for a
// parametric route are appended in `prerenderKey`.

export function routeKey(route: string): string {
  const cleaned = route.replace(/^\/+|\/+$/g, "");
  if (cleaned === "") return "index";
  return cleaned
    .replace(/:([A-Za-z0-9_]+)/g, "$1")
    .replace(/\*/g, "star")
    .replace(/[^A-Za-z0-9_]+/g, "_")
    .replace(/^_+|_+$/g, "");
}

// Resolved-URL key for a single Mode 1 prerender output. For a non-parametric
// route this is just `routeKey(route)`. For a parametric route we suffix each
// param value, sorted by key, so the order of `Record.keys` doesn't affect the
// emitted filename.
export function prerenderKey(route: string, params: Record<string, string>): string {
  const base = routeKey(route);
  const keys = Object.keys(params).sort();
  if (keys.length === 0) return base;
  const suffix = keys.map((k) => safe(params[k] ?? "")).join("_");
  return `${base}__${suffix}`;
}

function safe(s: string): string {
  return s.replace(/[^A-Za-z0-9_-]+/g, "_");
}
