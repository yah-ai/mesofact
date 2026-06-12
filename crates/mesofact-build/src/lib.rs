//! @yah:relay(R020, "Rust-native build pipeline — rolldown + lightningcss + deno_core + built-in install (W174/Q445)")
//! @yah:at(2026-06-11T23:30:00Z)
//! @yah:status(review)
//! @yah:assignee(agent:claude)
//! @yah:next("Parallel implementation of packages/mesofact-build per W174: server/client bundling on Rolldown 1.1 (crates.io), SSG + routes evaluation + SSR probing on deno_core, LightningCSS step slot, public/ asset discovery (R490-F4 parity), tag-index, manifest assembly against crates/mesofact types, lockfile-driven npm install (pacquet replacement — see W174 amendment in the parent camp), --legacy-bun passthrough + dist-diff equivalence harness (R450-F1/F2 in-repo half).")
//! @yah:next("Known scope cuts, documented in the W174 amendment: source-derived prerender (r2 list) unsupported natively (build with --legacy-bun); server bundles resolve browser-conditions (deno_core executor) vs Bun's node-flavored target; install step is bun.lock-driven only (no semver resolution).")
//! @yah:verify("cargo test -p mesofact-build")
//! @yah:verify("cargo run -p mesofact-build -- build <app dir> && cargo run -p mesofact-build -- diff <legacy dist> <native dist>")
//! @arch:see(../../.yah/docs/working/W174-mesofact-rust-native-pipeline.md)
//!
//! Rust-native mesofact build pipeline (W174). See [`pipeline::build`].

pub mod assets;
pub mod bundle;
pub mod config;
pub mod css;
pub mod data;
pub mod diff;
pub mod install;
pub mod js;
pub mod legacy;
pub mod manifest_build;
pub mod pipeline;
pub mod prerender;
pub mod route_config;
pub mod route_key;
pub mod source_infer;
pub mod ssr_prefix;
pub mod tag_index;

pub use pipeline::{build, BuildOptions, BuildResult, InstallMode};
