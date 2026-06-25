//! Same-origin reverse proxy for the dev server (R513-F10, W207 Gap #1).
//!
//! The dashboard E2E (Option B) serves a *static* SPA bundle and a *separate*
//! cheers auth service on different loopback ports. A browser `fetch` from the
//! SPA's origin to another port is cross-origin — and `cheers-axum` ships no
//! CORS layer (W207 §"Decision 3"). The fix the spec picked is a **same-origin
//! reverse proxy**: the static server forwards a few path prefixes (`/auth/*`,
//! `/dev/*`, `/api/*`) to the backing service ports, so the browser only ever
//! talks to its own origin — no preflight, no `Access-Control-*`, no cheers
//! change.
//!
//! The prefix→backend map is emitted by the camp at SPA-service spawn (beside
//! the served `config.json`) and handed to the server with `--proxy-map`. The
//! map carries the *server-internal* ephemeral ports; the SPA's `config.json`
//! carries only the same-origin path prefixes, so the bundle stays
//! port-oblivious.
//!
//! Forwarding is **path-preserving** (no prefix strip): `/auth/magic-link/verify`
//! reaches the backend as `/auth/magic-link/verify`. cheers mounts its routes at
//! their natural paths (`/auth/*` under the auth router, `/dev/last-magic-link`
//! and `/health` at root), so a prefix map of `{"/auth": …, "/dev": …}` reaches
//! all of them unchanged. Bodies are **buffered** (auth/api payloads are small
//! JSON); streaming is unnecessary here and keeps the hop simple.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use tracing::warn;

/// Hop-by-hop headers (RFC 7230 §6.1) plus `host`/`content-length`, stripped on
/// both legs of the proxy — they describe the *connection*, not the message, so
/// re-sending them across a fresh hop is wrong.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

fn is_hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP.contains(&name)
}

/// An ordered set of `prefix → backend base URL` proxy routes. Matching is
/// **longest-prefix-first**, so a more specific `/auth/admin` can shadow a
/// broader `/auth` if both are present.
#[derive(Debug, Clone, Default)]
pub struct ProxyMap {
    /// `(prefix, base_url)` sorted by descending prefix length so the first
    /// match in iteration order is the most specific.
    routes: Vec<(String, String)>,
}

impl ProxyMap {
    /// Build from a `prefix → base_url` map. Prefixes are normalised to a
    /// leading `/` with no trailing `/` (so `/auth` matches `/auth` and
    /// `/auth/...` but not `/authx`).
    pub fn new(entries: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut routes: Vec<(String, String)> = entries
            .into_iter()
            .map(|(prefix, base)| (normalize_prefix(&prefix), base))
            .filter(|(prefix, _)| prefix != "/") // a "/" catch-all would swallow the SPA
            .collect();
        // Longest prefix first; stable on ties for determinism.
        routes.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        routes.dedup_by(|a, b| a.0 == b.0);
        Self { routes }
    }

    /// Load a proxy map from a JSON object file, e.g.
    /// `{"/auth": "http://127.0.0.1:8745", "/dev": "http://127.0.0.1:8745"}`.
    pub fn from_json_file(path: &Path) -> anyhow::Result<Self> {
        let body = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading proxy map {}: {e}", path.display()))?;
        let map: BTreeMap<String, String> = serde_json::from_str(&body)
            .map_err(|e| anyhow::anyhow!("parsing proxy map {}: {e}", path.display()))?;
        Ok(Self::new(map))
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    pub fn routes(&self) -> &[(String, String)] {
        &self.routes
    }

    /// Return the backend base URL whose prefix matches `path`, if any. A
    /// prefix matches when `path` equals it or continues with a `/` segment
    /// boundary — `/auth` matches `/auth` and `/auth/x`, never `/authx`.
    pub fn match_base<'a>(&'a self, path: &str) -> Option<&'a str> {
        self.routes.iter().find_map(|(prefix, base)| {
            let is_match = path == prefix
                || (path.starts_with(prefix.as_str())
                    && path.as_bytes().get(prefix.len()) == Some(&b'/'));
            is_match.then_some(base.as_str())
        })
    }
}

/// Normalise a configured prefix to a leading-slash, no-trailing-slash form.
fn normalize_prefix(raw: &str) -> String {
    let trimmed = raw.trim();
    let with_lead = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    let no_trail = with_lead.trim_end_matches('/');
    if no_trail.is_empty() {
        "/".to_string()
    } else {
        no_trail.to_string()
    }
}

/// Shared proxy state: the route map plus a single reusable HTTP client. Held in
/// the server state and cloned (cheaply — both are `Arc`-backed) per request.
#[derive(Clone)]
pub struct ProxyState {
    map: Arc<ProxyMap>,
    client: reqwest::Client,
}

impl ProxyState {
    pub fn new(map: ProxyMap) -> Self {
        Self {
            map: Arc::new(map),
            // Plain-HTTP loopback only (no TLS feature); the backends are
            // camp-vended services on 127.0.0.1.
            client: reqwest::Client::new(),
        }
    }

    pub fn map(&self) -> &ProxyMap {
        &self.map
    }

    /// Forward `req` to `base` (a backend base URL like `http://127.0.0.1:8745`),
    /// preserving the original path + query. Returns the upstream response with
    /// hop-by-hop headers stripped, or `502` on a transport error.
    pub async fn forward(&self, base: &str, req: Request) -> Response {
        let (parts, body) = req.into_parts();
        let path_and_query = parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or_else(|| parts.uri.path());
        let target = format!("{}{}", base.trim_end_matches('/'), path_and_query);

        let body_bytes = match collect_body(body.into_data_stream()).await {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "proxy: failed to buffer request body");
                return (StatusCode::BAD_GATEWAY, "request buffer failed").into_response();
            }
        };

        let method = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
            Ok(m) => m,
            Err(_) => return (StatusCode::BAD_GATEWAY, "bad method").into_response(),
        };

        let mut rb = self.client.request(method, &target);
        for (k, v) in parts.headers.iter() {
            if is_hop_by_hop(&k.as_str().to_ascii_lowercase()) {
                continue;
            }
            rb = rb.header(k.as_str(), v.as_bytes());
        }
        if !body_bytes.is_empty() {
            rb = rb.body(body_bytes);
        }

        let upstream = match rb.send().await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, target = %target, "proxy: upstream request failed");
                return (StatusCode::BAD_GATEWAY, format!("proxy upstream failed: {e}"))
                    .into_response();
            }
        };

        let status = upstream.status();
        let upstream_headers = upstream.headers().clone();
        let upstream_body = match upstream.bytes().await {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "proxy: failed to read upstream body");
                return (StatusCode::BAD_GATEWAY, "upstream body read failed").into_response();
            }
        };

        let mut builder = Response::builder().status(status.as_u16());
        for (k, v) in upstream_headers.iter() {
            if is_hop_by_hop(&k.as_str().to_ascii_lowercase()) {
                continue;
            }
            // content-length is recomputed by axum from the buffered body.
            if k == header::CONTENT_LENGTH {
                continue;
            }
            builder = builder.header(k.as_str(), v.as_bytes());
        }
        builder
            .body(Body::from(upstream_body))
            .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "proxy response build failed").into_response())
    }
}

async fn collect_body(mut stream: axum::body::BodyDataStream) -> Result<Vec<u8>, axum::Error> {
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_prefixes() {
        assert_eq!(normalize_prefix("/auth"), "/auth");
        assert_eq!(normalize_prefix("auth"), "/auth");
        assert_eq!(normalize_prefix("/auth/"), "/auth");
        assert_eq!(normalize_prefix("  /api/  "), "/api");
        assert_eq!(normalize_prefix("/"), "/");
    }

    #[test]
    fn matches_on_segment_boundary_only() {
        let map = ProxyMap::new([("/auth".to_string(), "http://b:1".to_string())]);
        assert_eq!(map.match_base("/auth"), Some("http://b:1"));
        assert_eq!(map.match_base("/auth/magic-link/verify"), Some("http://b:1"));
        assert_eq!(map.match_base("/authx"), None);
        assert_eq!(map.match_base("/other"), None);
    }

    #[test]
    fn longest_prefix_wins() {
        let map = ProxyMap::new([
            ("/auth".to_string(), "http://broad:1".to_string()),
            ("/auth/admin".to_string(), "http://specific:2".to_string()),
        ]);
        assert_eq!(map.match_base("/auth/admin/x"), Some("http://specific:2"));
        assert_eq!(map.match_base("/auth/login"), Some("http://broad:1"));
    }

    #[test]
    fn catch_all_root_is_dropped() {
        let map = ProxyMap::new([("/".to_string(), "http://swallow:1".to_string())]);
        assert!(map.is_empty(), "a '/' catch-all must not be installed");
        assert_eq!(map.match_base("/anything"), None);
    }
}
