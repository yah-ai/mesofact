//! `mesofact selfcheck` — prove this binary's V8 tier works on THIS machine.
//!
//! Exists because of R823: `deno_core::extension!` bakes the COMPILING
//! machine's absolute cargo-registry paths into the binary and reads them at
//! `JsRuntime::new` time, so a mesofact built on one machine and shipped to
//! another panicked with `No such file or directory (os error 2)` the moment an
//! SSR route was hosted. Every gate the release path had ran where the binary
//! was compiled — the one environment where that defect is invisible — and
//! `mesofact --version` does not touch V8, so it stayed green for months.
//!
//! `selfcheck ssr` boots a real isolate (bootstrap + harness + the deno_web /
//! deno_fetch / deno_net / deno_webidl extension JS) and tears it down. No
//! bundle, no port, no filesystem layout: it is runnable inside a release
//! container, on a fleet node, or straight out of a `curl … | sh` install, which
//! is exactly where the R823 defect is visible and a compile-host check is not.

use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct SelfcheckArgs {
    #[command(subcommand)]
    check: Check,
}

#[derive(Subcommand)]
enum Check {
    /// Boot and tear down an SSR V8 isolate.
    Ssr,
}

pub fn run(args: SelfcheckArgs) -> Result<()> {
    match args.check {
        Check::Ssr => ssr(),
    }
}

#[cfg(feature = "ssr")]
fn ssr() -> Result<()> {
    use anyhow::Context;

    // Empty env: the isolate's `process.env` is caller-supplied (R444) and no
    // route code runs here, so there is nothing to thread in.
    let runtime =
        crate::ssr_runtime::SsrRuntime::start(Vec::new()).context("booting the SSR isolate")?;
    runtime.shutdown();
    println!("mesofact selfcheck: ssr isolate booted");
    Ok(())
}

#[cfg(not(feature = "ssr"))]
fn ssr() -> Result<()> {
    anyhow::bail!(
        "this mesofact was built without the `ssr` feature — there is no V8 tier to check"
    )
}
