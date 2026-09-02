//! The one derivation of `cache_policy` → response headers (R749-T1, R749-B4).
//!
//! `cache_policy` is declared once by the author and has to mean the same thing
//! at every place bytes leave the system. There are three:
//!
//! | consumer | what it does with the derived value |
//! |---|---|
//! | `mesofact serve` / the W272 bundle tier | axum middleware stamps it on the response ([`crate::cache_policy`] via `mesofact::cache_headers`) |
//! | `mesofact proxy` | the in-process [`ResponseCache`](crate::ResponseCache) holds entries for `ttl` |
//! | `mesofact publish` | the `html/` object's `Cache-Control` at PUT time (`mesofact_publisher::publish`) |
//!
//! This module is the shared half so those cannot drift. It lives in
//! `mesofact-core` rather than in the facade because the publisher crate is
//! *below* the facade in the dep graph (`mesofact` → `mesofact-publisher` →
//! `mesofact-core`), so a table defined in the facade is unreachable from the
//! publish path — which is exactly how the publish tier ended up picking
//! `Cache-Control` by path prefix and dropping the declaration (R749-B4).
//!
//! | declared | emitted |
//! |---|---|
//! | `ttl: N` | `Cache-Control: public, max-age=N` |
//! | `swr: N` | `, stale-while-revalidate=N` |
//! | `vary: [h, …]` | `Vary: h, …` |
//! | `negative_ttl: N` | `Cache-Control: public, max-age=N` **on 404/410 only** |
//! | `requires: [...]` | `private` instead of `public`, on every one of the above |
//!
//! That last row is not decoration. A route behind an auth gate whose response
//! carries `public` is a cache-poisoning bug — a shared cache would serve one
//! user's rendered page to the next. The declaration that the route is gated
//! and the declaration of how long it may be held are the same author's two
//! sentences about one route; reading them together is the minimum honest
//! interpretation.
//!
//! The byte-for-byte twin on the edge is
//! `packages/mesofact-edge/src/cache-policy.ts`, and the two are held to the
//! same fixture by `tests/fixtures/cache-policy/parity.json`.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

use crate::manifest::{CachePolicy, Route};

/// Per-route cache headers derived from the built manifest. Empty = no route
/// declared a non-inert policy, and every `apply` is a no-op.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CachePolicyTable {
    rules: Vec<CacheRule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheRule {
    /// The manifest route pattern (`/c/:slug`).
    pattern: String,
    /// `Cache-Control` for a successful response. `None` when the policy only
    /// declared a negative TTL.
    ok: Option<HeaderValue>,
    /// `Cache-Control` for a 404/410. `None` unless `negative_ttl` was set.
    negative: Option<HeaderValue>,
    /// `Vary`, when the policy declared one.
    vary: Option<HeaderValue>,
}

/// The three fields a cache rule is derived from, read without demanding the
/// rest of the manifest.
///
/// A local slice rather than [`crate::Manifest`] on purpose: a full deserialize
/// makes *every* required manifest field a startup dependency of the cache
/// layer, so a manifest missing `version` would refuse the serve with a message
/// about caching. Completeness is not this type's job — the raw-key check in
/// [`crate::policy`] runs first and is what guarantees no policy field reaches
/// here unnoticed.
#[derive(serde::Deserialize)]
struct RouteSlice {
    route: String,
    #[serde(default)]
    requires: Option<Vec<String>>,
    #[serde(default)]
    cache_policy: Option<CachePolicy>,
}

impl CachePolicyTable {
    /// Build from the manifest's routes. Routes whose policy is inert
    /// (`{ ttl: 0 }` — the "never cache this" every route file writes) produce
    /// no rule, so the table stays empty for the common workload and this
    /// layer costs one `is_empty` per response.
    pub fn from_routes(routes: &[Route]) -> Self {
        Self::build(routes.iter().map(|r| {
            (
                r.route.as_str(),
                r.requires.as_ref().is_some_and(|q| !q.is_empty()),
                Some(&r.cache_policy),
            )
        }))
    }

    /// Build from a built `manifest.json`'s bytes.
    pub fn from_manifest_json(raw: &[u8]) -> serde_json::Result<Self> {
        #[derive(serde::Deserialize)]
        struct ManifestSlice {
            #[serde(default)]
            routes: Vec<RouteSlice>,
        }
        let manifest: ManifestSlice = serde_json::from_slice(raw)?;
        Ok(Self::build(manifest.routes.iter().map(|r| {
            (
                r.route.as_str(),
                r.requires.as_ref().is_some_and(|q| !q.is_empty()),
                r.cache_policy.as_ref(),
            )
        })))
    }

    fn build<'a>(
        routes: impl Iterator<Item = (&'a str, bool, Option<&'a CachePolicy>)>,
    ) -> Self {
        let mut rules = Vec::new();
        for (pattern, gated, policy) in routes {
            let Some(policy) = policy else { continue };
            let scope = if gated { "private" } else { "public" };
            let rule = CacheRule {
                pattern: pattern.to_string(),
                ok: positive_cache_control(policy, scope).and_then(|v| HeaderValue::try_from(v).ok()),
                negative: policy
                    .negative_ttl
                    .and_then(|ttl| HeaderValue::try_from(format!("{scope}, max-age={ttl}")).ok()),
                vary: policy
                    .vary
                    .as_ref()
                    .filter(|v| !v.is_empty())
                    .and_then(|v| HeaderValue::try_from(v.join(", ")).ok()),
            };
            if rule.ok.is_some() || rule.negative.is_some() || rule.vary.is_some() {
                rules.push(rule);
            }
        }
        Self { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// The `Cache-Control` a **2xx** at `path` must carry, or `None` when no
    /// route declared a positive policy for it.
    ///
    /// The header-free accessor exists for the publish path, which stamps a
    /// value onto an object at PUT time rather than onto a live response and
    /// has no `axum` dependency of its own. Same rules, same first-match-wins
    /// order — a second derivation over there is precisely the drift this
    /// module was pulled down into `mesofact-core` to prevent.
    pub fn cache_control_for(&self, path: &str) -> Option<&str> {
        self.rules
            .iter()
            .find(|r| match_route_pattern(&r.pattern, path))
            .and_then(|r| r.ok.as_ref())
            .and_then(|v| v.to_str().ok())
    }

    /// Stamp the first matching route's derived headers onto `headers`.
    ///
    /// First match wins, matching the manifest's own routing rule — the same
    /// discipline `mesofact::route_headers` states for the domain table.
    pub fn apply(&self, path: &str, status: StatusCode, headers: &mut HeaderMap) {
        let Some(rule) = self
            .rules
            .iter()
            .find(|r| match_route_pattern(&r.pattern, path))
        else {
            return;
        };
        // 3xx/5xx get nothing: a redirect's lifetime is not the page's, and
        // caching a 500 for the page's TTL turns a blip into an outage.
        let cc = if status.is_success() {
            rule.ok.as_ref()
        } else if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
            rule.negative.as_ref()
        } else {
            None
        };
        if let Some(cc) = cc {
            headers.insert(header::CACHE_CONTROL, cc.clone());
            if let Some(vary) = &rule.vary {
                headers.insert(header::VARY, vary.clone());
            }
        }
    }
}

/// `Cache-Control` for a 2xx. `ttl: 0` with no `swr` is the inert policy and
/// yields nothing; `ttl: 0` *with* `swr` is a real declaration (revalidate
/// always, serve stale while you do).
pub fn positive_cache_control(policy: &CachePolicy, scope: &str) -> Option<String> {
    if policy.ttl == 0 && policy.swr.is_none() {
        return None;
    }
    let mut value = format!("{scope}, max-age={}", policy.ttl);
    if let Some(swr) = policy.swr {
        value.push_str(&format!(", stale-while-revalidate={swr}"));
    }
    Some(value)
}

/// Segment-aware match of a route pattern (`/c/:slug`) against a concrete path
/// (`/c/abc123`) — byte-parallel with the worker's `matchRoutePattern`. A
/// `:param` segment matches any single non-empty segment; segment counts must
/// be equal, so a trailing `:param` never swallows extra segments.
pub fn match_route_pattern(pattern: &str, pathname: &str) -> bool {
    let pat = pattern.split('/').filter(|s| !s.is_empty());
    let path: Vec<&str> = pathname.split('/').filter(|s| !s.is_empty()).collect();
    let pat: Vec<&str> = pat.collect();
    if pat.len() != path.len() {
        return false;
    }
    pat.iter().zip(path.iter()).all(|(seg, actual)| {
        if seg.starts_with(':') {
            !actual.is_empty()
        } else {
            seg == actual
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Requires, RouteMode};

    fn route(path: &str, policy: CachePolicy, requires: Option<Vec<Requires>>) -> Route {
        Route {
            route: path.into(),
            mode: RouteMode::Static,
            render_entrypoint: "e.js".into(),
            requires,
            source_reads: None,
            data_inputs: None,
            cache_policy: policy,
            concurrency: None,
            hydration: None,
            prerender: None,
            placement: None,
            resilience: None,
        }
    }

    fn policy(
        ttl: u64,
        swr: Option<u64>,
        negative_ttl: Option<u64>,
        vary: Option<Vec<String>>,
    ) -> CachePolicy {
        CachePolicy {
            ttl,
            swr,
            negative_ttl,
            vary,
        }
    }

    fn header_of(table: &CachePolicyTable, path: &str, status: StatusCode) -> Option<String> {
        let mut headers = HeaderMap::new();
        table.apply(path, status, &mut headers);
        headers
            .get(header::CACHE_CONTROL)
            .map(|v| v.to_str().unwrap().to_string())
    }

    #[test]
    fn the_inert_policy_every_route_carries_produces_no_rule() {
        let table = CachePolicyTable::from_routes(&[route("/", policy(0, None, None, None), None)]);
        assert!(table.is_empty());
        assert_eq!(header_of(&table, "/", StatusCode::OK), None);
    }

    #[test]
    fn a_declared_ttl_reaches_the_response() {
        let table = CachePolicyTable::from_routes(&[route(
            "/issues",
            policy(3600, Some(86_400), None, None),
            None,
        )]);
        assert_eq!(
            header_of(&table, "/issues", StatusCode::OK).as_deref(),
            Some("public, max-age=3600, stale-while-revalidate=86400"),
        );
    }

    /// The row that is a correctness bug rather than a missing optimization:
    /// a gated route must never be marked shareable.
    #[test]
    fn an_authed_route_is_private_not_public() {
        let table = CachePolicyTable::from_routes(&[route(
            "/app",
            policy(60, None, None, None),
            Some(vec![Requires::User]),
        )]);
        let cc = header_of(&table, "/app", StatusCode::OK).unwrap();
        assert!(cc.starts_with("private,"), "{cc}");
    }

    #[test]
    fn a_param_route_matches_its_instances_only_on_segment_boundaries() {
        let table =
            CachePolicyTable::from_routes(&[route("/c/:slug", policy(60, None, None, None), None)]);
        assert!(header_of(&table, "/c/abc", StatusCode::OK).is_some());
        assert!(header_of(&table, "/c/abc/extra", StatusCode::OK).is_none());
        assert!(header_of(&table, "/c", StatusCode::OK).is_none());
    }

    #[test]
    fn negative_ttl_applies_to_misses_and_the_positive_ttl_does_not() {
        let table =
            CachePolicyTable::from_routes(&[route("/r", policy(600, None, Some(30), None), None)]);
        assert_eq!(
            header_of(&table, "/r", StatusCode::NOT_FOUND).as_deref(),
            Some("public, max-age=30"),
        );
        assert_eq!(
            header_of(&table, "/r", StatusCode::OK).as_deref(),
            Some("public, max-age=600"),
        );
        // A 500 is nobody's cache entry.
        assert_eq!(
            header_of(&table, "/r", StatusCode::INTERNAL_SERVER_ERROR),
            None
        );
    }

    #[test]
    fn vary_rides_along_with_the_cache_control_it_qualifies() {
        let table = CachePolicyTable::from_routes(&[route(
            "/r",
            policy(
                60,
                None,
                None,
                Some(vec!["accept-language".into(), "cookie".into()]),
            ),
            None,
        )]);
        let mut headers = HeaderMap::new();
        table.apply("/r", StatusCode::OK, &mut headers);
        assert_eq!(
            headers.get(header::VARY).unwrap().to_str().unwrap(),
            "accept-language, cookie",
        );
    }

    /// `swr` alone is a real declaration even at `ttl: 0` — "always
    /// revalidate, but serve stale while you do" is a policy, not the absence
    /// of one, and dropping it would be the silent no-op this ticket forbids.
    #[test]
    fn swr_at_ttl_zero_is_still_a_declaration() {
        let table =
            CachePolicyTable::from_routes(&[route("/r", policy(0, Some(60), None, None), None)]);
        assert!(!table.is_empty());
        assert_eq!(
            header_of(&table, "/r", StatusCode::OK).as_deref(),
            Some("public, max-age=0, stale-while-revalidate=60"),
        );
    }

    /// The publish path's accessor and the serve path's middleware must answer
    /// the same string for the same route — they are one table precisely so a
    /// page cannot be published with one TTL and served with another.
    #[test]
    fn the_publish_accessor_agrees_with_the_serve_middleware() {
        let table = CachePolicyTable::from_routes(&[
            route("/issues", policy(3600, Some(86_400), None, None), None),
            route("/app", policy(60, None, None, None), Some(vec![Requires::User])),
            route("/plain", policy(0, None, None, None), None),
        ]);
        for path in ["/issues", "/app", "/plain", "/nope"] {
            assert_eq!(
                table.cache_control_for(path).map(str::to_string),
                header_of(&table, path, StatusCode::OK),
                "{path}",
            );
        }
    }
}
