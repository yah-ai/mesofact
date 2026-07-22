//! Assemble a built mesofact app into a W272 **bundle**.
//!
//! Part of R599-F2. The implementation MOVED to the `yah-mesofact-bundle`
//! crate under R599-T5 — see `oss/yah-base/crates/mesofact-bundle/src/assemble.rs`.
//! It sat here originally because this is where a build produces its `dist/`,
//! but assembly is pure `std::fs` + blake3 + toml while this crate pulls
//! rolldown + lightningcss + deno_core (V8). The operator driver
//! (`yah cloud bundle`) has to emit a bundle without being able to build one,
//! and the yah CLI is deliberately V8-less (R490-F2), so the code had to live
//! next to the manifest types instead.
//!
//! This module is a pure re-export so build-side callers keep working and there
//! is exactly one implementation to keep correct. The canonical `@yah:` ticket
//! annotation stays in `.yah/docs/working/W272-mesofact-bundles-kamaji-jit-serving.md`.

pub use yah_mesofact_bundle::{
    assemble_bundle, assemble_self_bundle, assemble_vanilla_bundle, collect_dir, BundleFile,
};
