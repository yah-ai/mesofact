//! axum proxy — boot, manifest reload, Mode 1 dispatch, worker pool, and the
//! P9 Mode 2 SSR slice (response cache + cache-key composition + session +
//! source generations). Mode 3 dispatch is stubbed (501) — wired up in P10.
//! See `.yah/docs/architecture/mesofact.md` §"IPC protocol" and §"Components".

//! ## Why half of this is `cfg(unix)` (yah R918-T1)
//!
//! The yah CLI links this crate transitively (yah -> cloud-admin /
//! control-plane -> mesofact -> mesofact-core) purely for the SSR slice's
//! neutral halves — `cache`, `session` and `source_gen` are what
//! `mesofact_core`'s lib re-exports. The serving half is a Unix daemon: the
//! render pool talks to Bun workers over AF_UNIX and the manifest reloads on
//! SIGHUP. Gating the daemon half keeps the neutral half reachable from a
//! Windows build instead of making the whole crate unbuildable for it.
//!
//! The split is exact rather than convenient: `router` is here because it
//! imports `worker_client` and `worker_pool`, not because it is itself
//! Unix-bound.

pub mod cache;
pub mod config;
pub mod metrics;
pub mod session;
pub mod source_gen;
pub mod trace;

#[cfg(unix)]
pub mod manifest_loader;
#[cfg(unix)]
pub mod router;
#[cfg(unix)]
pub mod worker_client;
#[cfg(unix)]
pub mod worker_pool;
