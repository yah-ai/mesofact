//! **`jsget` — the ecosystem-plural acquisition facade.** Working title only;
//! R757 says the shipped name is chosen on the way out.
//!
//! One npm-shaped resolver ([`rnpm`]) sits underneath, and this crate answers
//! the only question that is actually plural: *which registry, and what does it
//! call this package?* W318 §9 is explicit that the facade is **not** a slot for
//! a second resolution algorithm — JSR proves the point by costing a base URL
//! and a name mangling and nothing else.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! use jsget::{Acquirer, Registry};
//!
//! // The cache directory is injected. `mesofact-build` passes
//! // `$MESOFACT_STORE_DIR/packuments`; nothing below this line reads an
//! // environment variable.
//! let acquirer = Acquirer::new("/tmp/packuments")?;
//!
//! let react = acquirer.packument(&Registry::npm(), "react")?;
//! let flag = acquirer.packument(&Registry::jsr(), "@luca/flag")?;
//! # let _ = (react, flag);
//! # Ok(())
//! # }
//! ```
//!
//! Those two calls are the same function. The second one resolves
//! `https://npm.jsr.io/@jsr/luca__flag` instead of
//! `https://registry.npmjs.org/react`, and that is the entire difference —
//! asserted, not asserted-at, in `tests/two_registries_one_path.rs`.

mod registry;

pub use registry::{
    Ecosystem, Registry, JSR_NPM_REGISTRY_URL, JSR_SCOPE, NPM_REGISTRY_URL,
};

// Re-exported so a consumer needs one dependency, not two, and so the working
// title `rnpm` appears in exactly one place in a caller's manifest.
pub use rnpm::{
    CachePolicy, Dist, HttpTransport, Packument, PeerDependencyMeta, Transport, VersionManifest,
};

use anyhow::Result;
use rnpm::RegistryClient;
use std::path::PathBuf;

/// Fetches packuments from any registry [`Registry`] can describe.
///
/// Generic over the transport for the same reason [`rnpm::RegistryClient`] is:
/// the property worth testing here is that a warm cache issues no request, and
/// that is only observable if a test can count them.
pub struct Acquirer<T: Transport = HttpTransport> {
    client: RegistryClient<T>,
}

impl Acquirer<HttpTransport> {
    /// Over real HTTP, caching under `cache_dir`.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self::with_transport(cache_dir, HttpTransport::new()?))
    }
}

impl<T: Transport> Acquirer<T> {
    pub fn with_transport(cache_dir: impl Into<PathBuf>, transport: T) -> Self {
        Self { client: RegistryClient::new(cache_dir, transport) }
    }

    pub fn with_policy(mut self, policy: CachePolicy) -> Self {
        self.client = self.client.with_policy(policy);
        self
    }

    pub fn client(&self) -> &RegistryClient<T> {
        &self.client
    }

    /// The packument for `name` as `registry` publishes it.
    ///
    /// `name` is the name a *manifest* uses — `@luca/flag`, not
    /// `@jsr/luca__flag`. The mangling to the registry's own spelling happens
    /// here, which is the one place in the stack that knows registries differ.
    pub fn packument(&self, registry: &Registry, name: &str) -> Result<Packument> {
        self.client
            .packument(registry.endpoint(), &registry.registry_name(name)?)
    }

    /// As [`Self::packument`], but `None` rather than an error when the
    /// registry has no such package — the case an `optionalDependency` needs
    /// (R773-F6).
    pub fn try_packument(&self, registry: &Registry, name: &str) -> Result<Option<Packument>> {
        self.client
            .try_packument(registry.endpoint(), &registry.registry_name(name)?)
    }
}
