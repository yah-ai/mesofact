//! W173 § "SSR_PREFIXES derivation rule" — port of
//! `packages/mesofact-build/src/ssr-prefix.ts`.

use crate::route_config::RouteEntry;
use mesofact_core::manifest::RouteMode;
use std::collections::BTreeSet;

/// Non-parametric route → the full route. Parametric (`:foo`) or wildcard
/// (`*`) → everything up to (not including) the first such segment, with a
/// trailing slash.
pub fn derive_ssr_prefix(route: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut truncated = false;
    for seg in route.split('/') {
        if seg.starts_with(':') || seg == "*" || seg.contains('*') {
            truncated = true;
            break;
        }
        out.push(seg);
    }
    if truncated {
        format!("{}/", out.join("/"))
    } else {
        out.join("/")
    }
}

pub fn derive_ssr_prefixes(routes: &[RouteEntry]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    for r in routes {
        if r.mode != RouteMode::Ssr {
            continue;
        }
        seen.insert(derive_ssr_prefix(&r.route));
    }
    seen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_w173_table() {
        assert_eq!(derive_ssr_prefix("/api/health"), "/api/health");
        assert_eq!(derive_ssr_prefix("/api/users/:id"), "/api/users/");
        assert_eq!(derive_ssr_prefix("/x/:a/y"), "/x/");
        assert_eq!(derive_ssr_prefix("/feed/*"), "/feed/");
    }
}
