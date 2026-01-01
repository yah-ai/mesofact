//! mesofact — tri-mode web server library + proxy binary.
//! See `.yah/docs/working/mesofact.md` for the phased rollout plan.
//!
//! @yah:ticket(R009-T1, "Add axum/tokio deps + binary scaffold + proxy module export")
//! @yah:at(2026-05-15T23:55:09Z)
//! @yah:status(review)
//! @yah:parent(R009)

pub mod manifest;
pub mod proxy;
pub mod validate;

pub use manifest::{
    CachePolicy, ErrorRoutes, Hydration, Manifest, Prerender, Requires, Route, RouteMode,
    StaticAsset, MANIFEST_VERSION,
};
pub use proxy::cache::{compose_key, CacheEntry, CacheState, KeyInputs, ResponseCache};
pub use proxy::session::{CookieSessionResolver, SessionResolver, User};
pub use proxy::source_gen::Generations;
pub use validate::{validate, SourceCatalog, SourceScope, ValidationError, ValidationErrorKind};
