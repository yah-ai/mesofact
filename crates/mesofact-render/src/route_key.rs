//! Emission names. Port of `packages/mesofact-build/src/route-key.ts` — the
//! two pipelines must emit identical names or a Rust build and a Bun build of
//! the same project produce different `dist/`s.
//!
//! Two names, derived from two different things, on purpose:
//!
//! - [`route_key`] flattens the route PATTERN (`/p/:id` → `p_id`). It names
//!   build artifacts nobody addresses by URL — `dist/server/<key>.js`,
//!   `dist/hydrate/<key>.<hash>.js` — so a pattern needs exactly one of each
//!   however many instances it renders.
//! - [`prerender_key`] names emitted HTML by the PUBLIC PATH the page serves
//!   at (`/issues/abc` → `issues/abc`), because that path is the only name any
//!   serving layer ever asks for: the edge worker derives its candidates from
//!   `url.pathname` (`assetCandidates`, packages/mesofact-edge/src/router.ts)
//!   and `mesofact serve` resolves the request path under `dist/html/`.
//!
//! R600-B1: `prerender_key` used to flatten the pattern too and append param
//! values (`issues_id__abc`), which agreed with the path for a single-segment
//! literal route and disagreed for every other shape. Live effect on yah.dev:
//! 14 correctly-rendered, correctly-published `/issues/:id` pages, each at a
//! key no request could produce, all 404. Nested literal routes (`/blog/x` →
//! `blog_x`) were wrong the same way.

/// `"/"` → `"index"`, `"/p/:id"` → `"p_id"`, `"/blog/:slug/*"` → `"blog_slug_star"`.
pub fn route_key(route: &str) -> String {
    let cleaned = route.trim_matches('/');
    if cleaned.is_empty() {
        return "index".to_string();
    }
    // `:param` → `param`
    let mut s = String::with_capacity(cleaned.len());
    let mut chars = cleaned.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek().is_some_and(|n| n.is_ascii_alphanumeric() || *n == '_') {
            continue; // drop the colon, keep the name
        }
        s.push(c);
    }
    let s = s.replace('*', "star");
    // any run of non-[A-Za-z0-9_] → single '_'
    let mut out = String::with_capacity(s.len());
    let mut in_sep = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
            in_sep = false;
        } else if !in_sep {
            out.push('_');
            in_sep = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// Key for a single prerender emission — `url` (the concrete path the
/// instance serves at, as `expand_route` produced it) minus its leading
/// slash, so `dist/html/<key>.html` is literally what a request for that path
/// resolves to. `/` → `index`, `/releases` → `releases`, `/docs/` →
/// `docs/index`, `/issues/abc` → `issues/abc`.
///
/// Two shapes have no single public path and keep the flat [`route_key`]
/// name instead:
///
/// - an SPA shell rendered with no params (`url` is still the pattern,
///   `/item/:id`) — the worker serves it for every path under the route via
///   its shell fallback, never by this key;
/// - a wildcard route (`/blog/:slug/*`), which by construction covers a set
///   of paths rather than one.
///
/// A resolved `url` can never be mistaken for either: `expand_route`
/// percent-encodes `:` (and `/`) inside param values, so a bare `:name` only
/// ever survives from an unexpanded pattern.
pub fn prerender_key(route: &str, url: &str) -> String {
    if route.contains('*') || has_param(url) {
        return route_key(route);
    }
    let rel = url.trim_start_matches('/');
    if rel.is_empty() || rel.ends_with('/') {
        format!("{rel}index")
    } else {
        rel.to_string()
    }
}

/// `:name` — the unexpanded-param marker, same rule [`route_key`] strips by.
fn has_param(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        c == b':' && b.get(i + 1).is_some_and(|n| n.is_ascii_alphanumeric() || *n == b'_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::expand_route;
    use std::collections::BTreeMap;

    #[test]
    fn matches_ts_route_key_table() {
        assert_eq!(route_key("/"), "index");
        assert_eq!(route_key("/about"), "about");
        assert_eq!(route_key("/p/:id"), "p_id");
        assert_eq!(route_key("/blog/:slug/*"), "blog_slug_star");
        assert_eq!(route_key("/api/users/:id"), "api_users_id");
        assert_eq!(route_key("/issues/:id"), "issues_id");
    }

    /// The R600-B1 contract: an emission is named by the path it serves at,
    /// so `dist/html/<key>.html` is what `assetCandidates(path)` asks for.
    #[test]
    fn prerender_key_is_the_public_path() {
        let key = |route: &str, params: &[(&str, &str)]| {
            let map: BTreeMap<String, String> =
                params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            let url = expand_route(route, &map).unwrap();
            prerender_key(route, &url)
        };
        assert_eq!(key("/", &[]), "index");
        assert_eq!(key("/releases", &[]), "releases");
        // Nested literal routes were flattened to `blog_nested` and 404'd the
        // same way parametric instances did.
        assert_eq!(key("/blog/nested", &[]), "blog/nested");
        assert_eq!(key("/docs/", &[]), "docs/index");
        assert_eq!(key("/p/:id", &[("id", "42")]), "p/42");
        assert_eq!(
            key("/issues/:id", &[("id", "01KZVGVT0DV61ZGGNVHAWQW2CS")]),
            "issues/01KZVGVT0DV61ZGGNVHAWQW2CS"
        );
        assert_eq!(key("/x/:a/:b", &[("a", "1!"), ("b", "2")]), "x/1!/2");
        // Param values are percent-encoded, so a value can neither escape its
        // segment nor forge an unexpanded-param marker.
        assert_eq!(key("/p/:id", &[("id", "a/b")]), "p/a%2Fb");
        assert_eq!(key("/p/:id", &[("id", ":slug")]), "p/%3Aslug");
    }

    /// Shapes with no single public path keep the pattern-derived flat name.
    #[test]
    fn prerender_key_falls_back_for_unresolvable_patterns() {
        // SPA shell: rendered once, with no params, for a parametric route.
        assert_eq!(prerender_key("/item/:id", "/item/:id"), "item_id");
        assert_eq!(prerender_key("/blog/:slug/*", "/blog/x/*"), "blog_slug_star");
    }
}
