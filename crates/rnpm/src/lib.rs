//! **`rnpm` — the npm half of the W318 stage-2 resolver.** Working title only;
//! R757 says the shipped name is chosen on the way out.
//!
//! As of R773-F4 this crate is the *registry client* — abbreviated packuments,
//! an injected on-disk cache, conditional-request revalidation ([`client`],
//! [`cache`], [`packument`], [`transport`]) — plus the *package-spec grammar*
//! and npm range semantics ([`spec`], on `node-semver` 2.2.0; not the `semver`
//! crate, whose grammar is Cargo's), and both halves of resolution: the phase-1
//! tree build ([`resolve`], R773-F3) and the phase-2 peer post-pass
//! ([`peers`], R773-F4).
//!
//! The two phases run in that order and only that order. [`resolve`] *records*
//! peer requirements on each node and never follows one; [`peers`] reads them
//! back off the finished tree and nests instances where a requester's peer
//! context differs. Neither backtracks — see [`peers`] for why instantiation
//! replaces npm arborist's re-resolution.
//!
//! # What this crate deliberately does not do
//!
//! - **It does not fetch tarballs and does not know what a store is.**
//!   `mesofact-build` already verifies sha512 against the SRI before any
//!   durable write (`store.rs:118` `Store::insert_tarball`) and materializes
//!   through `crate::materialize`. That is the acquisition path; building a
//!   second one here would be the actual failure this ticket's "reuse, do not
//!   rewrite" note exists to prevent. Resolution's output is a
//!   `dist.integrity` + `dist.tarball` pair, which is exactly what that path
//!   consumes.
//! - **It does not depend on `mesofact-build`.** `mesofact-build` will consume
//!   this crate; the reverse edge is a cycle.
//! - **It does not discover a cache directory.** The caller passes one. See
//!   [`cache`].
//! - **It does not know which registries exist.** [`RegistryEndpoint`] is a
//!   base URL and an id. Which registries there are, and how each spells a
//!   package name, is the `jsget` facade's job (W318 §9).
//!
//! # Attribution
//!
//! W318 §10.1, operator-adopted 2026-08-15: this relay reads pnpm's MIT Rust
//! port rather than working clean-room, so the crate carries pnpm's notice
//! (see `NOTICE`) and any *ported* routine says so at its call site. Nothing
//! in F1 is a port — the registry protocol here was written against npm's
//! published document formats — and `NOTICE` says so plainly.
//!
//! # Shape
//!
//! ```text
//!   jsget::Registry  ──picks──▶ RegistryEndpoint ─┐
//!   (base URL, name mangling)                     ├─▶ RegistryClient::packument
//!   caller ──────────cache dir───▶ PackumentCache ─┘        │
//!                                                     Transport (fake in tests)
//! ```

pub mod cache;
pub mod client;
mod overrides;
pub mod packument;
pub mod peers;
pub mod platform;
pub mod resolve;
pub mod spec;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod transport;

pub use cache::{cache_file_name, CachedPackument, PackumentCache};
pub use client::{CachePolicy, RegistryClient, RegistryEndpoint};
pub use packument::{Dist, Packument, PeerDependencyMeta, VersionManifest};
pub use peers::{resolve_peers, PeerIssue, PeerReport, PeerStats};
pub use platform::{
    gate_mismatch, host_libc, host_platform_arch, manifest_mismatch, node_platform_arch,
    platform_admits, Host, PlatformMismatch,
};
pub use resolve::{
    Node, NodeId, NodeSource, PackumentSource, PeerRequirement, PreferredVersions, RegistrySource,
    ResolveWarning, ResolvedTree, Resolution, Resolver, RootManifest,
};
pub use spec::{
    validate_package_name, GitHost, GitSource, GitSpec, PackageSpec, Range, SpecError,
    SpecErrorKind, Version, VersionSpec,
};
pub use transport::{HttpTransport, Request, Response, Transport, ABBREVIATED_PACKUMENT};
