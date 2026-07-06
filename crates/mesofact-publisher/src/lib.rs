//! mesofact-publisher — `mesofact publish` substrate.
//!
//! Consumes a `dist/` tree emitted by `@mesofact/build` (manifest.json,
//! tag-index.json, server bundles, prerendered HTML, static assets, hydration
//! chunks) and pushes it through three steps:
//!
//! 1. Upload artifacts under `/{build_id}/...` in an [`ObjectStore`].
//! 2. Atomically swap the root `/manifest.json` pointer (commit point).
//! 3. Purge CDN tags via a [`CdnPurger`] for routes whose content changed.
//!
//! The two trait surfaces let production wiring (S3-compatible R2 + Cloudflare)
//! and tests (in-memory) share the orchestrator. R008-T1 shipped the traits
//! plus the [`InMemoryStore`] / [`InMemoryPurger`] backends; T7 added the
//! real-network adapters: [`S3Store`] (reqwest + hand-rolled SigV4) and
//! [`CloudflareCdnPurger`] (POST `/zones/{id}/purge_cache`). They wire in
//! through [`PublishConfig`] / [`PublishCredentials`] — the publish binary
//! loads the config block from `mesofact.config.toml`, resolves creds from
//! env-named vars, and applies CLI flag overrides.
//!
//! See `.yah/docs/working/mesofact.md` (P6) and
//! `.yah/docs/architecture/mesofact.md` (§"Static asset handling",
//! §"Versioning & rolling deploy") for the design.

pub mod cdn;
pub mod cloudflare;
pub mod config;
pub mod object_store;
pub mod pointer;
pub mod publish;
pub mod s3;

pub use cdn::{CdnPurger, InMemoryPurger, PurgeError};
pub use cloudflare::CloudflareCdnPurger;
pub use config::{ConfigError, PublishConfig, PublishCredentials};
pub use object_store::{InMemoryStore, ObjectMeta, ObjectStore, PutOpts, StoreError};
pub use pointer::{
    ObjectPointerStore, Pointer, PointerError, PointerState, PointerStore, POINTER_PREFIX,
};
pub use publish::{publish_dist, publish_pin, PublishError, PublishReport, TagIndex};
pub use s3::S3Store;
