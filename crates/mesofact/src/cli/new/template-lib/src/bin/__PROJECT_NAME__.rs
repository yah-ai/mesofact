//! The **prod** binary — the one CI builds and ships.
//!
//! Three lines over `__CRATE_NAME__::router()`, and that is the point: every
//! route lives in `src/lib.rs`, so this target cannot drift from the dev one.
//!
//! Nothing here reaches `mesofact_dev`, so the file watcher, the local object
//! store and every other dev affordance are outside this binary's reachable
//! graph. That is a property of the dependency graph rather than of a flag —
//! there is no build option that could turn them on.

use std::net::{IpAddr, Ipv4Addr};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    // 0.0.0.0: a container has to be reachable from outside itself.
    let addr = __CRATE_NAME__::addr(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    mesofact::serve_app(__CRATE_NAME__::router(), addr).await
}
