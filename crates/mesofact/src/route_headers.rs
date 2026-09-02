//! Per-route response headers for the **sovereign** serving path (R749-F3,
//! W334) — the Rust half of what the `@mesofact/edge` Worker does with its
//! `ROUTE_HEADERS` binding.
//!
//! The domain manifest is the only place that knows about path routing, so it
//! is also where a path's *response headers* belong — not in a `_headers` file
//! (a Pages/Netlify convention nothing in either serving path reads) and not
//! hardcoded for one site's needs. `DomainConfig::route_headers_json`
//! (`oss/yubaba/crates/cloud/src/config.rs`) renders that table; the Worker
//! receives it as the `ROUTE_HEADERS` binding and this process receives the
//! **same JSON string** as `MESOFACT_ROUTE_HEADERS`. A domain whose
//! `front_door` flips `worker` → `passway` must not lose its headers on the
//! way, which is exactly what happened before this module existed: correct
//! bytes, 200 OK, headers gone.
//!
//! **Semantics are the Worker's, mirrored not redesigned** (`applyRouteHeaders`
//! / `matchesRoutePattern` in `packages/mesofact-edge/src/router.ts`): rules
//! apply in manifest order, **first match wins, with no merging across rules**
//! — the same rule the manifest already states for routing. One path therefore
//! has one header set, decided where the route was decided. A rule that matches
//! but declares no headers still *consumes* the match; it does not fall through
//! to a later rule.
//!
//! **Failure posture differs from the Worker's on purpose** (R749-T1): the
//! Worker's `parseRouteHeaders` catches a malformed binding and serves without
//! the extra headers, on the grounds that a dead site beats a missing header.
//! That is right for a cosmetic header and wrong for the case that motivated
//! this work. Cross-origin isolation (`COOP: same-origin` + `COEP:
//! require-corp`) is not decoration: without both, `SharedArrayBuffer` is
//! *undefined on the global object* and `new WebAssembly.Memory({shared:true})`
//! throws, so a wasm app is served 200-OK and dead. Here a malformed table
//! therefore **fails the serve at startup**, naming the problem — a policy that
//! is declared and not enforced by the tier serving it is a hard error, never a
//! warning and never a silent skip. An absent or empty value is not malformed:
//! it reads as "no route headers configured" and serves normally.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};

/// The parsed `MESOFACT_ROUTE_HEADERS` table. Empty = nothing declared, which
/// is the common case and a no-op on every response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteHeaderTable {
    rules: Vec<RouteHeaderRule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteHeaderRule {
    /// The manifest route pattern (`/app/*`, `/*`, `/exact/path`).
    path: String,
    /// Pre-validated header pairs, in the manifest's own (`BTreeMap`) order.
    headers: Vec<(HeaderName, HeaderValue)>,
}

/// One wire entry — mirrors `RouteHeaderRule` in the Worker and the `Rule`
/// struct `DomainConfig::route_headers_json` serializes.
#[derive(serde::Deserialize)]
struct WireRule {
    path: String,
    headers: BTreeMap<String, String>,
}

impl RouteHeaderTable {
    /// Parse the JSON table. **Fails closed**: anything that is not a valid
    /// table is an error the caller is expected to turn into a refused start,
    /// not a reason to serve without the headers. Empty/whitespace input is
    /// "unset" and yields an empty table.
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(Self::default());
        }
        let wire: Vec<WireRule> = serde_json::from_str(trimmed).with_context(|| {
            format!(
                "route header table is not a JSON array of {{\"path\": \"…\", \"headers\": \
                 {{…}}}} rules — got {}. This value is produced by \
                 `DomainConfig::route_headers_json` from `.yah/domains/<name>.toml`, so a \
                 malformed one means the deploy shipped a broken table",
                preview(trimmed),
            )
        })?;
        let mut rules = Vec::with_capacity(wire.len());
        for rule in wire {
            if rule.path.is_empty() {
                bail!("route header rule has an empty `path` — a rule that matches nothing (or everything, depending on who reads it) is not a policy");
            }
            let mut headers = Vec::with_capacity(rule.headers.len());
            for (name, value) in rule.headers {
                let header_name = HeaderName::try_from(name.as_str()).with_context(|| {
                    format!("route {} declares {name:?}, which is not a valid HTTP header name", rule.path)
                })?;
                let header_value = HeaderValue::try_from(value.as_str()).with_context(|| {
                    format!(
                        "route {} declares {name}={value:?}, which is not a valid HTTP header value",
                        rule.path
                    )
                })?;
                headers.push((header_name, header_value));
            }
            // A matching rule with no headers is KEPT, not dropped: in the
            // Worker it consumes the match and stops the search, so dropping it
            // here would let a later catch-all apply where the edge applies
            // nothing. First match wins means first match wins.
            rules.push(RouteHeaderRule {
                path: rule.path,
                headers,
            });
        }
        Ok(Self { rules })
    }

    /// Nothing declared — every `apply` is a no-op.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Number of declared rules (for the startup log line).
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Stamp the first matching rule's headers onto `headers`, overwriting any
    /// same-named header already there (the Worker's `Headers.set`).
    pub fn apply(&self, path: &str, headers: &mut HeaderMap) {
        let Some(rule) = self
            .rules
            .iter()
            .find(|rule| matches_route_pattern(&rule.path, path))
        else {
            return;
        };
        for (name, value) in &rule.headers {
            headers.insert(name.clone(), value.clone());
        }
    }
}

/// Match a domain-manifest route pattern against a request path — a port of
/// `matchesRoutePattern` in `packages/mesofact-edge/src/router.ts`.
///
/// `"/*"` matches everything. `"/app/*"` matches `/app`, `/app/` and everything
/// below — segment-aware (so `/apple` stays out), and deliberately including
/// the BARE prefix: `/app` is the URL a link points at, it resolves to
/// `app/index.html` through the clean-URL rule, and a header set that skipped
/// it would miss the very document it exists for. Any pattern without a
/// trailing `*` is an exact path match.
fn matches_route_pattern(pattern: &str, path: &str) -> bool {
    let Some(head) = pattern.strip_suffix('*') else {
        return path == pattern;
    };
    let prefix = head.trim_end_matches('/');
    if prefix.is_empty() {
        return true;
    }
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

/// Axum middleware applying the table to **every** response the router
/// produces.
///
/// Wrapping the whole router — rather than each `return` inside it — is what
/// makes the guarantee total, and it is the same shape the Worker's exported
/// `fetch` uses: a header a domain declares for a path applies to the asset
/// hit, the clean-URL hit, the branded 404/410/500 and the SSR proxy alike. A
/// header set that only held on the happy path would be worse than none for the
/// case that motivated this, since an isolated document that loses isolation on
/// its error page loses `SharedArrayBuffer` with no server-side symptom.
pub async fn apply_route_headers(
    State(table): State<Arc<RouteHeaderTable>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_owned();
    let mut resp = next.run(req).await;
    table.apply(&path, resp.headers_mut());
    resp
}

/// First 120 chars of a bad value, for an error message that identifies the
/// input without pasting a whole table into a log line.
fn preview(raw: &str) -> String {
    let cut = raw.char_indices().nth(120).map(|(i, _)| i);
    match cut {
        Some(i) => format!("{:?}…", &raw[..i]),
        None => format!("{raw:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISOLATION: &str = r#"[
        {"path":"/app/*","headers":{"Cross-Origin-Opener-Policy":"same-origin","Cross-Origin-Embedder-Policy":"require-corp"}},
        {"path":"/*","headers":{"X-Tier":"marketing"}}
    ]"#;

    fn applied(table: &RouteHeaderTable, path: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        table.apply(path, &mut headers);
        headers
    }

    #[test]
    fn unset_and_empty_read_as_no_table() {
        assert!(RouteHeaderTable::parse("").unwrap().is_empty());
        assert!(RouteHeaderTable::parse("   ").unwrap().is_empty());
        assert!(RouteHeaderTable::parse("[]").unwrap().is_empty());
    }

    #[test]
    fn prefix_rule_covers_the_bare_prefix_and_everything_under_it() {
        let table = RouteHeaderTable::parse(ISOLATION).unwrap();
        for path in ["/app", "/app/", "/app/index.html", "/app/pkg/bundle.wasm"] {
            let headers = applied(&table, path);
            assert_eq!(
                headers.get("cross-origin-opener-policy").unwrap(),
                "same-origin",
                "{path}"
            );
            assert_eq!(
                headers.get("cross-origin-embedder-policy").unwrap(),
                "require-corp",
                "{path}"
            );
        }
    }

    #[test]
    fn matching_is_segment_aware() {
        let table = RouteHeaderTable::parse(ISOLATION).unwrap();
        let headers = applied(&table, "/apple");
        assert!(headers.get("cross-origin-opener-policy").is_none());
        assert_eq!(headers.get("x-tier").unwrap(), "marketing");
    }

    #[test]
    fn first_match_wins_with_no_merging() {
        let table = RouteHeaderTable::parse(ISOLATION).unwrap();
        let headers = applied(&table, "/app/");
        assert!(
            headers.get("x-tier").is_none(),
            "the catch-all must not merge into the app route"
        );
    }

    #[test]
    fn a_matching_rule_with_no_headers_still_consumes_the_match() {
        // Mirrors the Worker: `find` stops at the first matching rule whether or
        // not it declares headers, so a bare rule shadows a later catch-all.
        let table = RouteHeaderTable::parse(
            r#"[{"path":"/app/*","headers":{}},{"path":"/*","headers":{"X-Tier":"marketing"}}]"#,
        )
        .unwrap();
        assert!(applied(&table, "/app/x").is_empty());
        assert_eq!(applied(&table, "/other").get("x-tier").unwrap(), "marketing");
    }

    #[test]
    fn a_pattern_without_a_star_is_an_exact_match() {
        let table =
            RouteHeaderTable::parse(r#"[{"path":"/exact","headers":{"X-One":"1"}}]"#).unwrap();
        assert_eq!(applied(&table, "/exact").get("x-one").unwrap(), "1");
        assert!(applied(&table, "/exact/deeper").is_empty());
    }

    #[test]
    fn declared_headers_overwrite_a_header_the_handler_already_set() {
        let table =
            RouteHeaderTable::parse(r#"[{"path":"/*","headers":{"Cache-Control":"no-store"}}]"#)
                .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("cache-control", HeaderValue::from_static("public"));
        table.apply("/", &mut headers);
        assert_eq!(headers.get("cache-control").unwrap(), "no-store");
        assert_eq!(headers.get_all("cache-control").iter().count(), 1);
    }

    /// R749-T1 / W334: the malformed cases must be errors. There is deliberately
    /// no arm here that returns an empty table — that would be the Worker's
    /// serve-anyway posture, which is what this consumer exists not to repeat.
    #[test]
    fn malformed_tables_are_errors_not_empty_tables() {
        for raw in [
            "{not json",
            r#"{"path":"/*","headers":{}}"#,       // an object, not an array
            r#"[{"path":"/*"}]"#,                  // missing `headers`
            r#"[{"headers":{"X":"1"}}]"#,          // missing `path`
            r#"[{"path":"/*","headers":{"X":1}}]"#, // non-string value
            r#"[{"path":"","headers":{"X":"1"}}]"#, // empty pattern
            r#"[{"path":"/*","headers":{"Bad Name":"1"}}]"#,
            "[{\"path\":\"/*\",\"headers\":{\"X-Nl\":\"a\\nb\"}}]",
        ] {
            assert!(
                RouteHeaderTable::parse(raw).is_err(),
                "expected a hard error for {raw}"
            );
        }
    }

    #[test]
    fn the_parse_error_names_the_offending_input() {
        let err = RouteHeaderTable::parse(r#"[{"path":"/app/*","headers":{"Bad Name":"1"}}]"#)
            .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("/app/*"), "{rendered}");
        assert!(rendered.contains("Bad Name"), "{rendered}");
    }
}
