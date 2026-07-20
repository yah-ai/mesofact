//! @yah:relay(R020, "Rust-native build pipeline — rolldown + lightningcss + deno_core + built-in install (W174/Q445)")
//! @yah:at(2026-06-11T23:30:00Z)
//! @yah:status(review)
//! @yah:assignee(agent:claude)
//! @yah:next("Parallel implementation of packages/mesofact-build per W174: server/client bundling on Rolldown 1.1 (crates.io), SSG + routes evaluation + SSR probing on deno_core, LightningCSS step slot, public/ asset discovery (R490-F4 parity), tag-index, manifest assembly against crates/mesofact types, lockfile-driven npm install (pacquet replacement — see W174 amendment in the parent camp). The --legacy-bun passthrough + dist-diff equivalence harness were removed at R450-T4 once the Rust-native pipeline became the sole build path.")
//! @yah:next("Known scope cuts, documented in the W174 amendment: source-derived prerender (r2 list) unsupported natively (use prerender.from_data); server bundles resolve browser-conditions (deno_core executor) vs Bun's node-flavored target; install step is bun.lock-driven only (no semver resolution).")
//! @yah:verify("cargo test -p mesofact-build")
//! @yah:verify("cargo run -p mesofact-build -- build <app dir>")
//! @arch:see(../../.yah/docs/working/W174-mesofact-rust-native-pipeline.md)
//!
//! Rust-native mesofact build pipeline (W174). See [`pipeline::build`].

pub mod assets;
pub mod bundle;
pub mod bundle_assemble;
pub mod check;
pub mod config;
pub mod css;
pub mod install;
pub mod manifest_build;
pub mod pipeline;
pub mod prerender;
pub mod sitemap;
pub mod source_infer;
pub mod ssr_prefix;
pub mod tag_index;

// The render path was extracted to the bundler-free `mesofact-render` crate
// (W225 §2/§3). Re-exported here so `crate::{data,js,render,route_config,
// route_key}` still resolve for this crate's build modules and for external
// consumers that imported `mesofact_build::render::*` etc.
pub use mesofact_render::{data, js, render, route_config, route_key};

pub use mesofact_render::render::{
    render_route, render_route_all, render_route_all_with, render_route_with, RenderAllOptions,
    RenderOptions, RenderOutcome,
};
pub use pipeline::{build, BuildOptions, BuildResult, InstallMode};
