//! The **dev** binary — the one you run locally, and never ship.
//!
//! Identical to `__PROJECT_NAME__.rs` but for one identifier: `mesofact_dev`
//! instead of `mesofact`. That single difference is what pulls the dev-tier
//! affordances into *this* binary's link graph and no other.
//!
//! What `mesofact_dev::serve_app` adds over the prod call: it starts a local
//! FS-backed object store under `.mesofact-dev/` and publishes its coordinates
//! into this process's environment (`R2_ENDPOINT`, `R2_BUCKET`,
//! `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`), so a handler that builds its
//! store from env talks to a local bucket instead of real R2 — with no
//! dev-specific branch in the handler. It does *not* rebuild on change: your
//! routes are Rust, so an edit needs a re-link. `cargo watch -x 'run --bin
//! __PROJECT_NAME__-dev'` is that loop, and it belongs outside the binary.
//!
//! If `router()` ever builds its object store eagerly rather than per request,
//! swap this for the ordered form so the env is set before the router is
//! constructed:
//!
//! ```ignore
//! let dev = mesofact_dev::DevServer::start(".").await?;
//! dev.export_env();
//! dev.serve(__CRATE_NAME__::router(), addr).await
//! ```

use std::net::{IpAddr, Ipv4Addr};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    // Loopback: nothing on your network should reach a binary with dev
    // affordances linked into it.
    let addr = __CRATE_NAME__::addr(IpAddr::V4(Ipv4Addr::LOCALHOST));
    mesofact_dev::serve_app(__CRATE_NAME__::router(), addr).await
}
