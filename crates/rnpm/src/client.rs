//! The packument client: cache, then conditional request, then store.
//!
//! # Endpoint, not registry
//!
//! [`RegistryEndpoint`] is the *minimum* this crate needs to talk to
//! something npm-shaped: an id to namespace the cache with, a base URL, and an
//! optional `Authorization` header. It deliberately does not know that npm or
//! JSR exist, and it carries no name mangling — which registries exist and how
//! each spells a package name is the `jsget` facade's job (W318 §9). That
//! split is what keeps "add a registry" from being a change to this file.
//!
//! # Freshness
//!
//! [`CachePolicy`] decides whether a cached entry can be served without asking
//! the network:
//!
//! - [`CachePolicy::MaxAge`] (the default) serves an entry younger than the
//!   window with **no request at all**. This is the ticket's first
//!   verification criterion, and it is why the policy is not simply "always
//!   revalidate": a 304 is cheap but it is still a round trip, and a BFS over
//!   a real dependency tree makes hundreds of them.
//! - [`CachePolicy::Revalidate`] always asks, replaying the stored `ETag` as
//!   `If-None-Match` so the usual answer is a 304 with no body.
//! - [`CachePolicy::Offline`] never asks; a miss is an error naming the
//!   package.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::cache::{now_unix, CachedPackument, PackumentCache};
use crate::packument::Packument;
use crate::transport::{Request, Response, Transport, ABBREVIATED_PACKUMENT};

/// One npm-protocol endpoint. Built by the facade, consumed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEndpoint {
    /// Cache namespace. One boring path component (`npm`, `jsr`, an internal
    /// registry's short name) — not a URL.
    pub id: String,
    /// Base URL a package name is appended to. A trailing `/` is tolerated.
    pub base_url: String,
    /// Verbatim `Authorization` header value, for private registries.
    pub authorization: Option<String>,
}

impl RegistryEndpoint {
    pub fn new(id: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self { id: id.into(), base_url: base_url.into(), authorization: None }
    }

    pub fn with_authorization(mut self, value: impl Into<String>) -> Self {
        self.authorization = Some(value.into());
        self
    }

    /// The packument URL for a name *already spelled the way this registry
    /// spells it*. Scoped names keep their literal `/`: both
    /// `registry.npmjs.org/@babel/core` and `npm.jsr.io/@jsr/luca__flag` are
    /// served unescaped.
    pub fn packument_url(&self, registry_name: &str) -> String {
        format!("{}/{registry_name}", self.base_url.trim_end_matches('/'))
    }
}

/// When a cached packument may be served without a network round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    /// Serve an entry younger than this without any request.
    MaxAge(Duration),
    /// Always ask, conditionally.
    Revalidate,
    /// Never ask. A miss is an error.
    Offline,
}

impl Default for CachePolicy {
    /// Five minutes. Long enough that one resolve of a large tree — and a
    /// re-resolve moments later after a manifest edit — costs one request per
    /// package, short enough that a package published during a work session is
    /// visible without anybody clearing a cache. Callers that need "exactly
    /// what the registry has right now" ask for [`CachePolicy::Revalidate`].
    fn default() -> Self {
        Self::MaxAge(Duration::from_secs(300))
    }
}

/// Fetches abbreviated packuments, through a disk cache, over a [`Transport`].
pub struct RegistryClient<T: Transport> {
    cache: PackumentCache,
    transport: T,
    policy: CachePolicy,
}

impl<T: Transport> RegistryClient<T> {
    /// `cache_dir` is injected, never discovered — see [`crate::cache`].
    pub fn new(cache_dir: impl Into<PathBuf>, transport: T) -> Self {
        Self {
            cache: PackumentCache::new(cache_dir),
            transport,
            policy: CachePolicy::default(),
        }
    }

    pub fn with_policy(mut self, policy: CachePolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn policy(&self) -> CachePolicy {
        self.policy
    }

    pub fn cache(&self) -> &PackumentCache {
        &self.cache
    }

    /// The transport this client was built with. Exposed so a test that holds
    /// a counting fake can assert on what actually went out — the only way the
    /// warm-cache criterion is checkable from outside this crate.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The packument for `registry_name`, erroring if the registry has no such
    /// package.
    pub fn packument(&self, endpoint: &RegistryEndpoint, registry_name: &str) -> Result<Packument> {
        self.try_packument(endpoint, registry_name)?.ok_or_else(|| {
            anyhow::anyhow!(
                "{registry_name} is not published on the {} registry ({})",
                endpoint.id,
                endpoint.packument_url(registry_name)
            )
        })
    }

    /// The packument for `registry_name`, or `None` when the registry answers
    /// 404. Split out from [`Self::packument`] because a missing
    /// `optionalDependency` is not a failed install (R773-F6) and that caller
    /// must not have to read an error message to find out.
    pub fn try_packument(
        &self,
        endpoint: &RegistryEndpoint,
        registry_name: &str,
    ) -> Result<Option<Packument>> {
        let cached = self.cache.load(&endpoint.id, registry_name)?;

        match (&cached, self.policy) {
            (Some(entry), CachePolicy::Offline) => return Ok(Some(entry.packument.clone())),
            (Some(entry), CachePolicy::MaxAge(window))
                if entry.age(SystemTime::now()).is_some_and(|age| age < window) =>
            {
                return Ok(Some(entry.packument.clone()))
            }
            (None, CachePolicy::Offline) => {
                bail!(
                    "no cached packument for {registry_name} on the {} registry, \
                     and the client is offline",
                    endpoint.id
                )
            }
            _ => {}
        }

        let request = Request {
            url: endpoint.packument_url(registry_name),
            accept: ABBREVIATED_PACKUMENT,
            // Only sent when we hold a body we are willing to serve on a 304 —
            // otherwise a 304 would be unanswerable.
            if_none_match: cached.as_ref().and_then(|e| e.etag.clone()),
            authorization: endpoint.authorization.clone(),
        };

        match self.transport.get(&request)? {
            Response::NotFound => Ok(None),
            Response::NotModified => {
                let Some(entry) = cached else {
                    bail!(
                        "{} answered 304 for {registry_name} without being asked conditionally",
                        endpoint.id
                    );
                };
                // Restart the freshness window: the registry just told us this
                // body is current, so re-asking inside the window would be a
                // round trip we already know the answer to.
                let refreshed = CachedPackument { fetched_at: now_unix(), ..entry };
                self.cache.store(&endpoint.id, registry_name, &refreshed)?;
                Ok(Some(refreshed.packument))
            }
            Response::Body { etag, bytes } => {
                let packument: Packument = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing the packument for {registry_name}"))?;
                let entry = CachedPackument { etag, fetched_at: now_unix(), packument };
                self.cache.store(&endpoint.id, registry_name, &entry)?;
                Ok(Some(entry.packument))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeTransport;
    use crate::transport::ABBREVIATED_PACKUMENT;

    const BODY: &str = r#"{
      "name": "demo",
      "dist-tags": { "latest": "1.0.0" },
      "versions": {
        "1.0.0": {
          "version": "1.0.0",
          "dist": { "tarball": "https://example.test/demo-1.0.0.tgz", "integrity": "sha512-AAAA" }
        }
      }
    }"#;

    fn endpoint() -> RegistryEndpoint {
        RegistryEndpoint::new("npm", "https://registry.npmjs.test")
    }

    fn transport() -> FakeTransport {
        FakeTransport::new().serving("https://registry.npmjs.test/demo", Some("\"v1\""), BODY)
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let e = RegistryEndpoint::new("npm", "https://registry.npmjs.test/");
        assert_eq!(e.packument_url("demo"), "https://registry.npmjs.test/demo");
        assert_eq!(
            endpoint().packument_url("@babel/core"),
            "https://registry.npmjs.test/@babel/core"
        );
    }

    #[test]
    fn the_first_fetch_asks_for_the_abbreviated_document() {
        let dir = tempfile::tempdir().unwrap();
        let client = RegistryClient::new(dir.path(), transport());
        let p = client.packument(&endpoint(), "demo").unwrap();
        assert_eq!(p.dist_tags["latest"], "1.0.0");

        let reqs = client.transport.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].accept, ABBREVIATED_PACKUMENT);
        assert_eq!(reqs[0].if_none_match, None);
    }

    /// The ticket's first verification criterion.
    #[test]
    fn a_warm_cache_issues_no_network_request() {
        let dir = tempfile::tempdir().unwrap();
        let client = RegistryClient::new(dir.path(), transport());
        let first = client.packument(&endpoint(), "demo").unwrap();
        assert_eq!(client.transport.hits(), 1);

        for _ in 0..5 {
            assert_eq!(client.packument(&endpoint(), "demo").unwrap(), first);
        }
        assert_eq!(client.transport.hits(), 1, "the warm cache went to the network");
    }

    /// …and it is the *disk* that is warm, not a process-local memo: a second
    /// client over the same directory is just as cold-free.
    #[test]
    fn the_warm_cache_survives_the_client_that_filled_it() {
        let dir = tempfile::tempdir().unwrap();
        RegistryClient::new(dir.path(), transport())
            .packument(&endpoint(), "demo")
            .unwrap();

        let second = RegistryClient::new(dir.path(), transport());
        second.packument(&endpoint(), "demo").unwrap();
        assert_eq!(second.transport.hits(), 0);
    }

    #[test]
    fn revalidation_replays_the_stored_etag_and_a_304_serves_the_cached_body() {
        let dir = tempfile::tempdir().unwrap();
        let client =
            RegistryClient::new(dir.path(), transport()).with_policy(CachePolicy::Revalidate);

        let first = client.packument(&endpoint(), "demo").unwrap();
        let second = client.packument(&endpoint(), "demo").unwrap();
        assert_eq!(first, second);

        let reqs = client.transport.requests();
        assert_eq!(reqs.len(), 2, "Revalidate must ask every time");
        assert_eq!(reqs[0].if_none_match, None);
        assert_eq!(
            reqs[1].if_none_match.as_deref(),
            Some("\"v1\""),
            "the second request must carry If-None-Match"
        );
    }

    #[test]
    fn a_changed_etag_replaces_the_cached_body() {
        let dir = tempfile::tempdir().unwrap();
        let updated = BODY.replace("1.0.0", "1.0.1");
        let client = RegistryClient::new(
            dir.path(),
            FakeTransport::new().serving("https://registry.npmjs.test/demo", Some("\"v1\""), BODY),
        )
        .with_policy(CachePolicy::Revalidate);
        client.packument(&endpoint(), "demo").unwrap();

        // Same cache directory, a transport whose validator moved on.
        let moved = RegistryClient::new(
            dir.path(),
            FakeTransport::new().serving(
                "https://registry.npmjs.test/demo",
                Some("\"v2\""),
                &updated,
            ),
        )
        .with_policy(CachePolicy::Revalidate);
        let p = moved.packument(&endpoint(), "demo").unwrap();
        assert_eq!(p.dist_tags["latest"], "1.0.1");
        assert!(p.versions.contains_key("1.0.1"));
    }

    #[test]
    fn a_stale_entry_is_revalidated_rather_than_served() {
        let dir = tempfile::tempdir().unwrap();
        let client = RegistryClient::new(dir.path(), transport())
            .with_policy(CachePolicy::MaxAge(Duration::ZERO));
        client.packument(&endpoint(), "demo").unwrap();
        client.packument(&endpoint(), "demo").unwrap();
        assert_eq!(client.transport.hits(), 2);
    }

    #[test]
    fn offline_serves_a_hit_and_names_the_package_on_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        RegistryClient::new(dir.path(), transport())
            .packument(&endpoint(), "demo")
            .unwrap();

        let offline =
            RegistryClient::new(dir.path(), transport()).with_policy(CachePolicy::Offline);
        assert!(offline.packument(&endpoint(), "demo").is_ok());
        assert_eq!(offline.transport.hits(), 0);

        let err = offline.packument(&endpoint(), "absent").unwrap_err().to_string();
        assert!(err.contains("absent"), "{err}");
        assert_eq!(offline.transport.hits(), 0);
    }

    #[test]
    fn a_404_is_none_rather_than_an_error_and_is_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let client = RegistryClient::new(dir.path(), transport());
        assert_eq!(client.try_packument(&endpoint(), "absent").unwrap(), None);
        assert_eq!(client.try_packument(&endpoint(), "absent").unwrap(), None);
        assert_eq!(client.transport.hits(), 2);

        let err = client.packument(&endpoint(), "absent").unwrap_err().to_string();
        assert!(err.contains("absent") && err.contains("npm"), "{err}");
    }

    #[test]
    fn an_authorization_header_reaches_the_transport() {
        let dir = tempfile::tempdir().unwrap();
        let private = endpoint().with_authorization("Bearer tok");
        let client = RegistryClient::new(dir.path(), transport());
        client.packument(&private, "demo").unwrap();
        assert_eq!(
            client.transport.requests()[0].authorization.as_deref(),
            Some("Bearer tok")
        );
    }
}
