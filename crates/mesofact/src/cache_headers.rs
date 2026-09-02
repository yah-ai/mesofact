//! `cache_policy` enforcement for the **sovereign** serving path (R749-T1).
//!
//! Before this, `cache_policy` was read by exactly one consumer in the whole
//! system: `mesofact_core::proxy::router`'s in-process [`ResponseCache`], which
//! only the `mesofact proxy` subcommand builds. The serve tier — the binary the
//! W272 bundle path forks, and therefore the one behind passway — constructed
//! no cache and derived no header, so an author writing `ttl: 3600` on a route
//! got a manifest field and nothing else.
//!
//! [`ResponseCache`]: mesofact_core::ResponseCache
//!
//! **What "enforce" means here, precisely.** This module does not add a second
//! in-process cache — it renders the declared policy into the response headers
//! that make every HTTP cache in front of the origin honour it. For a tier
//! whose whole deployment shape is "origin behind a CDN" that *is* the
//! enforcement, and it is the form the proxy tier's in-memory cache cannot
//! reach (a CDN edge in Sydney is not going to consult a `ResponseCache` in
//! us-west). The two implementations are different mechanisms for the same
//! declaration, which is the point of the policy being declarative.
//!
//! **The derivation itself lives one crate down**, in
//! [`mesofact_core::cache_policy`], and the table type is re-exported here
//! unchanged. R749-B4 moved it: the publish path needed the same rules, and the
//! publisher crate sits *below* this facade in the dep graph (`mesofact` →
//! `mesofact-publisher` → `mesofact-core`), so a table defined here could not
//! reach it. That unreachability is why `mesofact publish` spent its life
//! picking `Cache-Control` by path prefix and dropping the declaration. Read
//! [`mesofact_core::cache_policy`] for the declared → emitted table.
//!
//! **Precedence.** This layer sits *inside*
//! [`crate::route_headers::apply_route_headers`], so a domain manifest that
//! declares `Cache-Control` for a path wins over the route's own policy — the
//! domain is the later, more specific operator statement. It sits *outside* the
//! handlers, so it wins over a `Cache-Control` an SSR handler set for itself:
//! anything else would make the declared policy silently unenforced for exactly
//! the routes that also run user code, which is the defect this ticket forbids.
//! A route that wants its handler to own caching declares `{ ttl: 0 }` and gets
//! no rule at all.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

pub use mesofact_core::cache_policy::{
    match_route_pattern, positive_cache_control, CachePolicyTable,
};

/// Axum middleware applying the table to every response the router produces.
pub async fn apply_cache_policy(
    State(table): State<Arc<CachePolicyTable>>,
    req: Request,
    next: Next,
) -> Response {
    if table.is_empty() {
        return next.run(req).await;
    }
    let path = req.uri().path().to_string();
    let mut resp = next.run(req).await;
    let status = resp.status();
    table.apply(&path, status, resp.headers_mut());
    resp
}
