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
//!   request path. Wires the deno_web/url/fetch/console extension crates so
//!   route code can use the real Fetch API.
//!
//! `JsRuntime` is `!Send`, so each runtime owns a dedicated thread with a
//! current-thread tokio runtime; callers talk to it through a small
//! synchronous handle.

mod ssg;
mod ssr;

pub use ssg::SsgRuntime;
pub use ssr::{DispatchRequest, DispatchResponse, SsrRuntime};
