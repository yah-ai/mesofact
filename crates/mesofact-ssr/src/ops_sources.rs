//! Source adapters for SSR route code (R750-F3): `r2(name)` / `sqlite(name)`
//! from `@mesofact/runtime` call these four ops, which call the
//! [`SourceBackend`] the caller attached to the current dispatch.
//!
//! mesofact-ssr owns only the trait and the op plumbing — no R2 client, no
//! sqlite crate. The real backend lives with the caller (`mesofact`'s `ssr`
//! feature: signed S3Store for r2, turso for sqlite), which resolves
//! `[sources.<name>]` from `mesofact.config.toml` once at spawn.
//!
//! Errors cross into JS as data, not exceptions: each op resolves to
//! `{ok: <value>}` or `{err: {kind, message}}`, and the shim throws the
//! matching `SourceNotRegisteredError` / `SourceUnavailableError` /
//! `SourceQueryError`. Per-call `timeout(ms)` is enforced here, Rust-side.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use deno_core::{op2, OpState, ToJsBuffer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `ListOpts` of `packages/mesofact-runtime/src/source.ts`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListOpts {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub delimiter: Option<String>,
}

/// `R2Object` of `packages/mesofact-runtime/src/source.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct R2Object {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// Why a source read failed; maps 1:1 onto the runtime's error classes.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceError {
    /// No `[sources.<name>]` of the requested kind — `SourceNotRegisteredError`.
    NotRegistered,
    /// Backend unreachable / timed out — `SourceUnavailableError` (retryable).
    Unavailable(String),
    /// Backend answered with an error — `SourceQueryError`.
    Query(String),
}

pub type SourceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SourceError>> + 'a>>;

/// The read-only adapter surface (`BlobSource` + `KeyValueSource`), keyed by
/// source name. A backend answers `NotRegistered` for a name it does not hold
/// under the requested kind.
pub trait SourceBackend: Send + Sync {
    fn fetch<'a>(&'a self, source: &'a str, key: &'a str) -> SourceFuture<'a, Option<Vec<u8>>>;
    fn list<'a>(
        &'a self,
        source: &'a str,
        prefix: &'a str,
        opts: ListOpts,
    ) -> SourceFuture<'a, Vec<R2Object>>;
    fn get<'a>(&'a self, source: &'a str, table: &'a str, id: &'a str)
        -> SourceFuture<'a, Option<Value>>;
    fn query<'a>(
        &'a self,
        source: &'a str,
        sql: &'a str,
        params: Vec<Value>,
    ) -> SourceFuture<'a, Vec<Value>>;
}

/// The current dispatch's backend; put/cleared around each dispatch exactly
/// like [`crate::ops_session::DispatchSession`].
#[derive(Default, Clone)]
pub(crate) struct DispatchSources(pub(crate) Option<Arc<dyn SourceBackend>>);

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reply<T> {
    Ok(T),
    Err { kind: &'static str, message: String },
}

fn backend(state: &Rc<RefCell<OpState>>) -> Option<Arc<dyn SourceBackend>> {
    state.borrow().try_borrow::<DispatchSources>().and_then(|s| s.0.clone())
}

fn not_registered<T>() -> Reply<T> {
    // No backend attached (a caller with no `[sources]`) reads the same as a
    // backend that does not hold the name: unregistered.
    Reply::Err { kind: "not_registered", message: String::new() }
}

async fn finish<T>(timeout_ms: u64, call: SourceFuture<'_, T>) -> Reply<T> {
    match tokio::time::timeout(Duration::from_millis(timeout_ms), call).await {
        Ok(Ok(v)) => Reply::Ok(v),
        Ok(Err(SourceError::NotRegistered)) => not_registered(),
        Ok(Err(SourceError::Unavailable(m))) => Reply::Err { kind: "unavailable", message: m },
        Ok(Err(SourceError::Query(m))) => Reply::Err { kind: "query", message: m },
        Err(_) => Reply::Err { kind: "unavailable", message: format!("timed out after {timeout_ms}ms") },
    }
}

#[derive(Deserialize)]
pub(crate) struct FetchArgs {
    source: String,
    key: String,
    timeout_ms: u64,
}

#[op2]
#[serde]
pub(crate) async fn op_mesofact_source_fetch(
    state: Rc<RefCell<OpState>>,
    #[serde] a: FetchArgs,
) -> Reply<Option<ToJsBuffer>> {
    let Some(b) = backend(&state) else { return not_registered() };
    match finish(a.timeout_ms, b.fetch(&a.source, &a.key)).await {
        Reply::Ok(v) => Reply::Ok(v.map(ToJsBuffer::from)),
        Reply::Err { kind, message } => Reply::Err { kind, message },
    }
}

#[derive(Deserialize)]
pub(crate) struct ListArgs {
    source: String,
    prefix: String,
    #[serde(default)]
    opts: ListOpts,
    timeout_ms: u64,
}

#[op2]
#[serde]
pub(crate) async fn op_mesofact_source_list(
    state: Rc<RefCell<OpState>>,
    #[serde] a: ListArgs,
) -> Reply<Vec<R2Object>> {
    let Some(b) = backend(&state) else { return not_registered() };
    finish(a.timeout_ms, b.list(&a.source, &a.prefix, a.opts.clone())).await
}

#[derive(Deserialize)]
pub(crate) struct GetArgs {
    source: String,
    table: String,
    id: String,
    timeout_ms: u64,
}

#[op2]
#[serde]
pub(crate) async fn op_mesofact_source_get(
    state: Rc<RefCell<OpState>>,
    #[serde] a: GetArgs,
) -> Reply<Option<Value>> {
    let Some(b) = backend(&state) else { return not_registered() };
    finish(a.timeout_ms, b.get(&a.source, &a.table, &a.id)).await
}

#[derive(Deserialize)]
pub(crate) struct QueryArgs {
    source: String,
    sql: String,
    #[serde(default)]
    params: Vec<Value>,
    timeout_ms: u64,
}

#[op2]
#[serde]
pub(crate) async fn op_mesofact_source_query(
    state: Rc<RefCell<OpState>>,
    #[serde] a: QueryArgs,
) -> Reply<Vec<Value>> {
    let Some(b) = backend(&state) else { return not_registered() };
    finish(a.timeout_ms, b.query(&a.source, &a.sql, a.params)).await
}

deno_core::extension!(
    mesofact_sources,
    ops = [
        op_mesofact_source_fetch,
        op_mesofact_source_list,
        op_mesofact_source_get,
        op_mesofact_source_query,
    ]
);
