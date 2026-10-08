//! deno_core-hosted JS runtime shared by mesofact-build (SSG, one-shot render)
//! and mesofact-dev (long-lived SSR dispatch). W174 pillar 4 / R449-F2.
//!
//! Two entry points:
//!
//! - [`SsgRuntime`] — one isolate per build; sequential `eval_routes` /
//!   `probe_default` / `render` jobs. Same behavior the old `mesofact-build`
//!   `JsRuntime` had; the runtime lives here so the build crate can stay
//!   focused on bundling + asset orchestration.
//! - `SsrRuntime` (to land in this crate alongside `SsgRuntime`) — long-lived
//!   isolate that pre-loads each SSR route's render_entrypoint at startup and
//!   exposes `dispatch(method, url, headers, body)` for the dev server's
//!   request path. Route code gets a lean pure-JS Fetch surface
//!   (`js/ssr_fetch_shim.js`) over one reqwest-backed op (R750-F1).
//!
//! `JsRuntime` is `!Send`, so each runtime owns a dedicated thread with a
//! current-thread tokio runtime; callers talk to it through a small
//! synchronous handle.

mod ext_sources;
mod ops_fetch;
mod ops_session;
mod ops_sources;
mod ssg;
mod ssr;

pub use ops_sources::{ListOpts, R2Object, SourceBackend, SourceError, SourceFuture};
pub use ssg::SsgRuntime;
#[doc(hidden)]
pub use ssr::{extensions as ssr_extensions, SSR_PRELUDE};
pub use ssr::{
    DispatchRequest, DispatchResponse, R2SourceCoords, SsrPool, SsrRuntime, DEFAULT_POOL_SIZE,
};
