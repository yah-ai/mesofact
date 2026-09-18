//! `mesofact-dev` — the dev-tier affordances, and nothing else.
//!
//! **This crate is deliberately small.** The serving engine (`Server`, SSR
//! dispatch, the revalidate receiver, tenants, the same-origin proxy) used to
//! live here, which meant the *prod* `mesofact-serve` binary — which shipped
//! from this crate — linked the file watcher and the dev S3 surface. That broke
//! the dev/prod crate boundary W225 §2 relies on for its security claim
//! ("prod is clean by construction … the crate boundary already keeps it out of
//! prod"). It wasn't: cleanliness rested on linker dead-stripping.
//!
//! The engine now lives in the `mesofact` facade and this crate *depends on*
//! it, holding only the pieces that must never reach a prod binary:
//!
//! - [`watcher`] — the rebuild-on-change file watcher.
//! - [`s3`] — reads the camp-injected dev-tier S3 coordinates that stand in
//!   for R2 during `dev` (R584-T1; W225 §2 "local pond emulation").
//! - [`app`] — the **library-tier** dev entry point, [`serve_app`]: the dev
//!   counterpart of [`mesofact::serve_app`] for a consumer whose routes are
//!   Rust handlers rather than a built `dist/` tree. Read its module doc for
//!   what the dev half of that tier is and, just as load-bearing, what it
//!   deliberately is not.
//! - [`cli`] — the `mes` toolchain CLI, and the two bin targets over it.
//!
//! [`cli`] carries the prod verbs (`serve`, `publish`, `new`) as well as the
//! dev ones, so consumers learn one CLI — but it gets them by *calling into*
//! [`mesofact::cli`], which is the direction that costs the prod binary
//! nothing. The boundary above is about what links into a binary, not about
//! which verbs a binary spells.
//!
//! Engine types are re-exported below so existing `mesofact_dev::Server`-style
//! callsites keep working; new code should prefer `mesofact::…` directly.
//!
//! @arch:see(.yah/docs/working/W225-mesofact-consumer-deployment-model.md)

pub mod app;
pub mod cli;
pub mod s3;
pub mod watcher;

pub use app::{serve_app, DevServer, DEV_STATE_DIR};
pub use s3::{DevStore, StoreProvenance, EMBEDDED_BUCKET};
pub use watcher::{BuildDriver, WatchOptions, Watcher};

// Engine re-exports — the serving path now lives in the `mesofact` facade.
pub use mesofact::proxy;
pub use mesofact::server;
pub use mesofact::{DistPointer, Identity, ProxyMap, ProxyState, Server, DEFAULT_PORT};
#[cfg(feature = "ssr")]
pub use mesofact::{
    revalidate, ssr, tenants, ResiliencePolicy, RetryPolicy, SsrChild, SsrSlot, SsrSpawnOptions,
    DEFAULT_RESILIENCE_TIMEOUT_MS,
};

/// Shared across `s3::tests` and `app::tests` (R584-T1): both mutate the same
/// process-wide `S3_*`/`R2_*` env vars, and a lock private to one module does
/// nothing to stop the default parallel test runner racing it against the
/// other. `tokio::sync::Mutex` rather than `std::sync::Mutex` because
/// `app::tests` holds the guard across an `.await` (`DevServer::start`).
#[cfg(test)]
pub(crate) mod test_support {
    pub static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
}
