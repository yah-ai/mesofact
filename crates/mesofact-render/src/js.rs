//! Re-export of the SSG runtime, now hosted in `mesofact-ssr` so the dev-tier
//! SSR path can share the same module loader + harness (W174 / R449-F2).
//! `crate::js::SsgRuntime` remains the build-side public name.

pub use mesofact_ssr::SsgRuntime;
