//! `op_mesofact_fetch` — the one Rust op behind `js/ssr_fetch_shim.js`'s
//! `fetch()` (R750-F1, W225 §2b). Replaces deno_fetch's HTTP client.
//!
//! The request body arrives buffered and the response body leaves buffered:
//! the shim models bodies as `Uint8Array`, never streams, which is the whole
//! surface SSR route code was measured to use.
//!
//! TLS: reqwest is taken with `default-features = false`; the crate's `tls`
//! feature adds `reqwest/rustls-tls` (mesofact-core's backend), and mesofact's
//! `ssr` feature turns it on. Without it (the default, and `cargo test -p
//! mesofact-ssr`) only `http://` works and an https URL fails at send.

use std::time::Duration;

use deno_core::{op2, JsBuffer, ToJsBuffer};
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};

/// Per-request timeout. The dispatch context carries no W181
/// `ResiliencePolicy` today, so there is nothing to override it with; and no
/// retry — a route that wants one can loop on `fetch` itself.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
pub(crate) struct FetchArgs {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<JsBuffer>,
}

#[derive(Serialize)]
pub(crate) struct FetchReply {
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    body: ToJsBuffer,
}

thread_local! {
    // Per isolate thread, not process-wide: each isolate drives its own
    // current-thread tokio runtime, and a hyper connection pool must not be
    // shared across runtimes (a pooled connection's task dies with the
    // runtime that spawned it).
    static CLIENT: reqwest::Client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .expect("building the SSR fetch client");
}

#[op2]
#[serde]
pub(crate) async fn op_mesofact_fetch(#[serde] args: FetchArgs) -> Result<FetchReply, JsErrorBox> {
    let type_err = |e: String| JsErrorBox::type_error(format!("fetch failed: {e}"));
    let method = reqwest::Method::from_bytes(args.method.as_bytes())
        .map_err(|e| type_err(format!("invalid method {}: {e}", args.method)))?;
    let mut req = CLIENT.with(|c| c.request(method, &args.url));
    for (k, v) in &args.headers {
        req = req.header(k, v);
    }
    if let Some(body) = args.body {
        req = req.body(body.to_vec());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| type_err(format!("{} {}: {e}", args.method, args.url)))?;
    let status = resp.status();
    let headers = resp
        .headers()
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
        .collect();
    let body = resp
        .bytes()
        .await
        .map_err(|e| type_err(format!("reading body of {}: {e}", args.url)))?;
    Ok(FetchReply {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_owned(),
        headers,
        body: body.to_vec().into(),
    })
}

deno_core::extension!(mesofact_fetch, ops = [op_mesofact_fetch]);
