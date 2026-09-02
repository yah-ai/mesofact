//! The one seam between this crate and the network.
//!
//! [`RegistryClient`](crate::RegistryClient) is generic over [`Transport`] for
//! a reason the ticket states directly: "resolving a package with a warm cache
//! issues no network request" is only *provable* if a test can count requests,
//! and a test that proves it by hitting `registry.npmjs.org` proves nothing
//! about the cache and breaks whenever CI is offline. So the whole HTTP
//! surface this crate needs is four fields and three response shapes, and the
//! hermetic tests substitute a counting fake for [`HttpTransport`].
//!
//! Blocking, matching `mesofact-build`'s `install.rs:490` client — see the
//! crate manifest for why async is not wanted here.

use anyhow::{bail, Context, Result};

/// npm's abbreviated-packument media type. Sending this is the difference
/// between the document a resolver needs and the one a web page needs.
pub const ABBREVIATED_PACKUMENT: &str = "application/vnd.npm.install-v1+json";

/// One conditional GET.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub accept: &'static str,
    /// The `ETag` a previous response carried, if the cache has one. Present
    /// iff we already hold a body we are willing to serve on a 304.
    pub if_none_match: Option<String>,
    /// Verbatim `Authorization` header value — registry plurality includes
    /// private registries, and this crate never invents one.
    pub authorization: Option<String>,
}

/// What a registry answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// 304 — the cached body is still current. Carries nothing: the body the
    /// caller already holds *is* the response.
    NotModified,
    /// 2xx with a body, plus the validator to store alongside it.
    Body { etag: Option<String>, bytes: Vec<u8> },
    /// 404 — the registry has no such package. Distinguished from an error
    /// because an `optionalDependency` that does not exist is not a failed
    /// install (R773-F6), and a resolver cannot make that call off a message
    /// string.
    NotFound,
}

pub trait Transport {
    fn get(&self, request: &Request) -> Result<Response>;
}

/// [`Transport`] over `reqwest::blocking`.
pub struct HttpTransport {
    client: reqwest::blocking::Client,
}

impl HttpTransport {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .user_agent(concat!("rnpm/", env!("CARGO_PKG_VERSION")))
                .build()
                .context("building the registry HTTP client")?,
        })
    }

    pub fn with_client(client: reqwest::blocking::Client) -> Self {
        Self { client }
    }
}

impl Transport for HttpTransport {
    fn get(&self, request: &Request) -> Result<Response> {
        let mut builder = self.client.get(&request.url).header("accept", request.accept);
        if let Some(etag) = &request.if_none_match {
            builder = builder.header("if-none-match", etag);
        }
        if let Some(auth) = &request.authorization {
            builder = builder.header("authorization", auth);
        }
        let response = builder
            .send()
            .with_context(|| format!("GET {}", request.url))?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Response::NotModified);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(Response::NotFound);
        }
        if !status.is_success() {
            bail!("GET {} -> {status}", request.url);
        }

        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let bytes = response
            .bytes()
            .with_context(|| format!("reading the body of {}", request.url))?
            .to_vec();
        Ok(Response::Body { etag, bytes })
    }
}
