//! The `mesofact` CLI — one subcommand-driven prod binary.
//!
//! Consolidates what used to be three separate binaries scattered across three
//! crates (`mesofact-serve` here, `mesofact-proxy` in mesofact-core,
//! `mesofact-publish` in mesofact-publisher) into `mesofact <subcommand>`, per
//! W174 §Binary surface.
//!
//! **`mesofact-dev` is deliberately NOT a subcommand.** It stays its own binary
//! in its own crate: folding it in would pull the file watcher and the dev S3
//! surface into the prod binary's dependency closure and undo the dev/prod
//! boundary (W225 §2). The two-binary split is the security boundary — the
//! subcommand consolidation applies only *within* the prod binary.
//!
//! Each submodule exposes an args struct + `run`, so the binary is a thin
//! dispatch layer over library code.

pub mod proxy;
pub mod serve;
#[cfg(feature = "publish")]
pub mod publish;
