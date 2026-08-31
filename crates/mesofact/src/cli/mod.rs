//! The `mesofact` CLI — one subcommand-driven prod binary.
//!
//! Consolidates what used to be three separate binaries scattered across three
//! crates (`mesofact-serve` here, `mesofact-proxy` in mesofact-core,
//! `mesofact-publish` in mesofact-publisher) into `mesofact <subcommand>`, per
//! W174 §Binary surface.
//!
//! **The dev loop is deliberately NOT a subcommand here.** It stays its own
//! binary (`mes`) in its own crate: folding it in would pull the file watcher,
//! the dev S3 surface and the bundler into the prod binary's dependency
//! closure and undo the dev/prod boundary (W225 §2). The two-binary split is
//! the security boundary — the subcommand consolidation applies only *within*
//! the prod binary.
//!
//! The composition runs one way only. `mes` depends on this crate and exposes
//! `serve`/`publish`/`new` by calling straight into the modules below, so a
//! consumer has one CLI to learn without this binary gaining a gram. Nothing
//! in here may ever depend on `mesofact-dev` to get the reverse.
//!
//! Each submodule exposes an args struct + `run`, so the binary is a thin
//! dispatch layer over library code.

pub mod new;
pub mod proxy;
pub mod serve;
#[cfg(feature = "publish")]
pub mod publish;
