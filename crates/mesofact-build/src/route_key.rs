//! Route pattern → filesystem-safe key. Port of
//! `packages/mesofact-build/src/route-key.ts` — the two pipelines must emit
//! identical `dist/server/<key>.js` and `dist/html/<key>.html` names.

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

/// Key for a single prerender emission: `route_key` plus the param values
/// (sorted by param name) so `Record` iteration order can't change names.
pub fn prerender_key(route: &str, params: &std::collections::BTreeMap<String, String>) -> String {
    let base = route_key(route);
    if params.is_empty() {
        return base;
    }
    let suffix: Vec<String> = params.values().map(|v| safe(v)).collect();
    format!("{base}__{}", suffix.join("_"))
}

fn safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_sep = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
            in_sep = false;
        } else if !in_sep {
            out.push('_');
            in_sep = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn prerender_key_sorts_params() {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), "42".to_string());
        assert_eq!(prerender_key("/p/:id", &params), "p_id__42");
        assert_eq!(prerender_key("/p/:id", &BTreeMap::new()), "p_id");
        let mut multi = BTreeMap::new();
        multi.insert("b".to_string(), "2".to_string());
        multi.insert("a".to_string(), "1!".to_string());
        assert_eq!(prerender_key("/x/:a/:b", &multi), "x_a_b__1__2");
    }
}
