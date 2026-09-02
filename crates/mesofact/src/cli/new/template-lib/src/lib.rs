//! `__PROJECT_NAME__` — the route table and the handlers.
//!
//! This is the "core" of the one-crate-two-bins shape, in-crate rather than in
//! a separate crate. Both `src/bin/` targets are three lines over
//! [`router`], so there is one route table and the dev and prod servers cannot
//! disagree about what this service does.
//!
//! **This module must never reference `mesofact_dev`.** That is the whole of
//! the prod/dev boundary: the prod binary's reachable graph goes through here,
//! so as long as nothing here reaches the dev crate, no dev affordance can end
//! up in what you ship. It is a structural property, checked by reading this
//! file — not a flag anyone can forget.

use std::net::{IpAddr, SocketAddr};

use axum::{routing::get, Json, Router};

/// Every route this service serves. Both binaries mount exactly this.
pub fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/hello", get(hello))
}

/// Where a binary binds: `$PORT` if set, else 3000, on the `host` its target
/// chose.
///
/// The host differs by target on purpose. The prod binary passes
/// `0.0.0.0` because a container has to be reachable from outside itself; the
/// dev binary passes `127.0.0.1` because nothing on your network should be
/// able to reach a binary with dev affordances linked into it.
pub fn addr(host: IpAddr) -> SocketAddr {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3000);
    SocketAddr::new(host, port)
}

async fn index() -> &'static str {
    "__PROJECT_NAME__ is serving.\n"
}

async fn hello() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "hello": "world" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Handlers are ordinary Rust functions and the router is an ordinary
    /// `axum::Router`, so testing this service needs no server, no port and no
    /// mesofact — which is most of the reason to be on this tier.
    #[tokio::test]
    async fn index_serves() {
        let response = router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
