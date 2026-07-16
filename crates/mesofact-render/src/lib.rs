//! `mesofact-render` — the bundler-free render path (data → HTML against a
//! prebuilt `dist/`), extracted from `mesofact-build` per W225 §2/§3 so lean
//! `serve` / revalidate-receiver consumers link render + `SsgRuntime` WITHOUT
//! pulling the bundler (`rolldown` / `lightningcss`). This mirrors the earlier
//! extraction of `mesofact-ssr` from `mesofact-build`.
//!
//! Why a crate and not a feature: W225 §2 — a crate boundary is immune to the
//! feature-unification footgun (a `--workspace` build enabling the bundler
//! feature elsewhere could re-link `rolldown` into the lean receiver). A
//! separate crate keeps the bundler physically out of `serve`'s dependency
//! closure.
//!
//! Modules moved verbatim from `mesofact-build`; `mesofact-build` now depends
//! on this crate and re-exports these names for source compatibility.

pub mod data;
pub mod js;
pub mod render;
pub mod route_config;
pub mod route_key;

pub use render::{
    render_route, render_route_all, render_route_all_with, render_route_with, RenderAllOptions,
    RenderOptions, RenderOutcome,
};
