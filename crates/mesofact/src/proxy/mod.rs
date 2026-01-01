//! axum proxy — boot, manifest reload, Mode 1 dispatch, worker pool, and the
//! P9 Mode 2 SSR slice (response cache + cache-key composition + session +
//! source generations). Mode 3 dispatch is stubbed (501) — wired up in P10.
//! See `.yah/docs/architecture/mesofact.md` §"IPC protocol" and §"Components".

pub mod cache;
pub mod config;
pub mod manifest_loader;
pub mod metrics;
pub mod router;
pub mod session;
pub mod source_gen;
pub mod trace;
pub mod worker_client;
pub mod worker_pool;
