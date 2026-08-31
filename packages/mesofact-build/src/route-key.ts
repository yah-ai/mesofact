// Emission names. Twin of `crates/mesofact-render/src/route_key.rs` — the two
// pipelines must emit identical names or a Rust build and a Bun build of the
// same project produce different `dist/`s.
//
// Two names, derived from two different things, on purpose:
//
//   routeKey     flattens the route PATTERN ("/p/:id" → "p_id"). It names build
//                artifacts nobody addresses by URL — `dist/server/<key>.js`,
//                `dist/hydrate/<key>.<hash>.js` — so a pattern needs exactly
//                one of each however many instances it renders.
//   prerenderKey names emitted HTML by the PUBLIC PATH the page serves at
//                ("/issues/abc" → "issues/abc"), because that path is the only
//                name any serving layer ever asks for: the edge worker derives
//                its candidates from `url.pathname` (`assetCandidates`,
//                packages/mesofact-edge/src/router.ts) and `mesofact serve`
//                resolves the request path under `dist/html/`.
//
// R600-B1: prerenderKey used to flatten the pattern too and append param values
// ("issues_id__abc"), which agreed with the path for a single-segment literal
// route and disagreed for every other shape. Live effect on yah.dev: 14
// correctly-rendered, correctly-published /issues/:id pages, each at a key no
// request could produce, all 404. Nested literal routes ("/blog/x" → "blog_x")
// were wrong the same way.

export function routeKey(route: string): string {
  const cleaned = route.replace(/^\/+|\/+$/g, "");
  if (cleaned === "") return "index";
  return cleaned
    .replace(/:([A-Za-z0-9_]+)/g, "$1")
    .replace(/\*/g, "star")
    .replace(/[^A-Za-z0-9_]+/g, "_")
    .replace(/^_+|_+$/g, "");
}

// Key for a single prerender emission — `url` (the concrete path the instance
// serves at, as `expandRoute` produced it) minus its leading slash, so
// `dist/html/<key>.html` is literally what a request for that path resolves to.
//   "/"             → "index"
//   "/releases"     → "releases"
//   "/docs/"        → "docs/index"
//   "/issues/abc"   → "issues/abc"
//
// Two shapes have no single public path and keep the flat `routeKey` name:
// an SPA shell rendered with no params (`url` is still the pattern,
// "/item/:id") — the worker serves it for every path under the route via its
// shell fallback, never by this key — and a wildcard route ("/blog/:slug/*"),
// which by construction covers a set of paths rather than one.
//
// A resolved `url` can never be mistaken for either: `expandRoute`
// percent-encodes ":" (and "/") inside param values, so a bare ":name" only
// ever survives from an unexpanded pattern.
export function prerenderKey(route: string, url: string): string {
  if (route.includes("*") || /:[A-Za-z0-9_]/.test(url)) return routeKey(route);
  const rel = url.replace(/^\/+/, "");
  if (rel === "" || rel.endsWith("/")) return `${rel}index`;
  return rel;
}
